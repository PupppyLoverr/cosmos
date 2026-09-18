//! The `ComputerService` — one per device, owned by the engine. Owns the
//! component settings, the Cua driver child process, the loopback endpoint,
//! and every policy gate (enabled → installed → control mode → delivery).

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::watch;

use crate::backend::{ComputerBackend, Delivery, InspectKind, ObserveRequest, ObserveScope};
use crate::digest;
use crate::install::{self, InstallProgress};
use crate::surface;
use crate::{
    ComputerError, ComputerPaths, ComputerSettings, ComputerSettingsEcho, ComputerStatus,
    ControlMode, Observation, StatusCapabilities, StatusPermissions, linux_session_label,
    load_settings, platform_label, release_asset, save_settings,
};

/// One device's computer state. Everything the engine/RPC/UI talks to.
pub struct ComputerService {
    inner: Arc<Inner>,
}

struct Inner {
    /// Device-local Cosmos data dir (settings file lives directly inside).
    data_dir: PathBuf,
    paths: ComputerPaths,
    /// Attachment-staging jail observe writes PNGs into.
    screenshot_dir: PathBuf,
    settings: RwLock<ComputerSettings>,
    backend: Mutex<Option<Arc<dyn ComputerBackend>>>,
    /// Serializes backend spawns so two concurrent calls can't double-spawn.
    spawn_lock: tokio::sync::Mutex<()>,
    last_error: Mutex<Option<String>>,
    endpoint: Mutex<Option<Endpoint>>,
    install_tx: watch::Sender<InstallProgress>,
    /// Bumped on every status-affecting change (settings, install, driver
    /// spawn/drop, last_error) — `WatchComputer` re-snapshots `status()` on
    /// each tick rather than caching a second status copy here.
    status_tx: watch::Sender<()>,
    installing: AtomicBool,
    /// Consecutive identical action failures — `"{tool}:{pid}:{target}"` →
    /// count. The loop-breaker: after REPEAT_FAIL_CAP identical failures the
    /// service refuses with `repeated_failure` and a fresh-state hint instead
    /// of letting a harness hammer a dead target forever.
    failures: Mutex<HashMap<String, u32>>,
}

struct Endpoint {
    url: String,
    _serve: tokio::task::JoinHandle<()>,
}

/// Identical action failures tolerated before the service loop-breaks with a
/// `repeated_failure` refusal — a hammering harness gets state, not a 4th noop.
const REPEAT_FAIL_CAP: u32 = 3;

/// The content blocks an MCP `tools/call` returns. `image` carries raw PNG
/// bytes — the endpoint base64s them into an MCP image block.
#[derive(Debug)]
pub struct ToolReply {
    pub text: String,
    pub image: Option<Vec<u8>>,
}

impl ToolReply {
    fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            image: None,
        }
    }
}

impl ComputerService {
    /// `data_dir` = the device-local Cosmos data dir. `screenshot_dir` = the
    /// attachment-staging jail (the engine passes its uploads root so the UI
    /// can read staged PNGs through the attachment-chunk RPC).
    pub fn new(data_dir: PathBuf, screenshot_dir: PathBuf) -> Self {
        let settings = load_settings(&data_dir);
        let (install_tx, _) = watch::channel(InstallProgress::Idle);
        let (status_tx, _) = watch::channel(());
        Self {
            inner: Arc::new(Inner {
                paths: ComputerPaths::new(&data_dir),
                data_dir,
                screenshot_dir,
                settings: RwLock::new(settings),
                backend: Mutex::new(None),
                spawn_lock: tokio::sync::Mutex::new(()),
                last_error: Mutex::new(None),
                endpoint: Mutex::new(None),
                install_tx,
                status_tx,
                installing: AtomicBool::new(false),
                failures: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// A service whose backend is supplied directly — tests use this to skip
    /// the driver install entirely.
    pub fn with_backend(
        data_dir: PathBuf,
        screenshot_dir: PathBuf,
        backend: Arc<dyn ComputerBackend>,
    ) -> Self {
        let service = Self::new(data_dir, screenshot_dir);
        *service.inner.backend.lock().unwrap() = Some(backend);
        service
    }

    fn mock_enabled() -> bool {
        std::env::var("COSMOS_COMPUTER_MOCK").is_ok()
    }

    // -- settings ------------------------------------------------------------

    pub fn settings(&self) -> ComputerSettings {
        self.inner.settings.read().unwrap().clone()
    }

    /// Persist new settings and restart the backend — the driver env contract
    /// (permission mode, telemetry) is fixed at spawn, so a change respawns.
    pub fn set_settings(&self, settings: ComputerSettings) -> Result<(), ComputerError> {
        save_settings(&self.inner.data_dir, &settings)?;
        *self.inner.settings.write().unwrap() = settings;
        self.drop_backend();
        self.bump_status();
        Ok(())
    }

    /// Notify `WatchComputer` subscribers that `status()` changed.
    fn bump_status(&self) {
        let _ = self.inner.status_tx.send(());
    }

    /// Status-change ticks — the RPC layer re-snapshots `status()` per tick
    /// and merges the install progress watch into one stream.
    pub fn subscribe_status(&self) -> watch::Receiver<()> {
        self.inner.status_tx.subscribe()
    }

    // -- status --------------------------------------------------------------

    /// The snapshot the Settings page and `computer_status` render. Sync —
    /// cheap filesystem checks only; live driver facts come from `probe`.
    pub fn status(&self) -> ComputerStatus {
        let settings = self.settings();
        let install = self.inner.paths.load_state_lenient();
        let verified = install::verify(&self.inner.paths);
        let backend_running = self.inner.backend.lock().unwrap().is_some();
        let platform_supported = release_asset().is_some();
        let last_error = self.inner.last_error.lock().unwrap().clone();
        let state = self.state_label(
            &settings,
            &install,
            verified || backend_running,
            platform_supported,
            &last_error,
        );
        let install_dir = install.as_ref().map(|i| i.install_dir.clone());
        ComputerStatus {
            state: state.to_string(),
            platform: platform_label().to_string(),
            arch: std::env::consts::ARCH.to_string(),
            session: linux_session_label().map(str::to_string),
            driver_version: install.as_ref().map(|i| i.version.clone()),
            install_dir: install_dir.as_ref().map(|p| p.display().to_string()),
            component_download_bytes: release_asset().map(|a| a.size_bytes).unwrap_or(0),
            component_installed_bytes: install_dir.as_deref().map(install::installed_bytes),
            settings: ComputerSettingsEcho {
                enabled: settings.enabled,
                semantic: settings.semantic,
                screenshots: settings.screenshots,
                control: settings.control,
                telemetry: settings.telemetry,
                inspect_max_elements: settings.inspect_max_elements,
                screenshot_max_dimension: settings.screenshot_max_dimension,
                permission_mode: settings.permission_mode().as_str().to_string(),
            },
            capabilities: StatusCapabilities {
                semantic: settings.semantic,
                screenshots: settings.screenshots,
                background: settings.control.acts(),
                foreground: settings.control == ControlMode::FullControl,
            },
            permissions: self.permission_hints(),
            last_error,
            session_id: None,
            checked_at: chrono::Utc::now(),
        }
    }

    fn state_label(
        &self,
        settings: &ComputerSettings,
        install: &Option<crate::InstallState>,
        verified: bool,
        platform_supported: bool,
        last_error: &Option<String>,
    ) -> &'static str {
        if !settings.enabled {
            "disabled"
        } else if !platform_supported {
            "unsupported"
        } else if Self::mock_enabled() || verified {
            if last_error.is_some() {
                "error"
            } else {
                "ready"
            }
        } else if install.is_some() {
            "incomplete"
        } else {
            "missing"
        }
    }

    /// Static platform permission posture. `probe` refines this with the
    /// driver's own health report when a backend is running.
    fn permission_hints(&self) -> StatusPermissions {
        let (screen, accessibility) = if cfg!(target_os = "macos") {
            // TCC decides per-app; the driver reports the real grants.
            (
                "check System Settings → Privacy & Security → Screen Recording",
                "check System Settings → Privacy & Security → Accessibility",
            )
        } else if cfg!(target_os = "linux") {
            match linux_session_label() {
                Some("wayland") => (
                    "needs the xdg-desktop-portal ScreenCast grant",
                    "not required",
                ),
                _ => ("not required", "not required"),
            }
        } else {
            ("not required", "not required")
        };
        let note = if cfg!(target_os = "macos") {
            Some("The driver appears as 'CuaDriver' in macOS permission lists — grant it once, then restart the driver from here.".into())
        } else if cfg!(target_os = "linux") && linux_session_label() == Some("wayland") {
            Some(
                "Wayland limits background input — some actions may report background_unavailable."
                    .into(),
            )
        } else {
            None
        };
        StatusPermissions {
            screen: screen.to_string(),
            accessibility: accessibility.to_string(),
            note,
        }
    }

    /// Live driver facts — ping + the driver's own health report (which
    /// carries macOS TCC grant state). Cheap enough for settings-page load.
    pub async fn probe(&self) -> Probe {
        let backend = match self.ensure_backend().await {
            Ok(backend) => backend,
            Err(error) => {
                return Probe {
                    driver_ok: false,
                    health: None,
                    error: Some(error.message()),
                };
            }
        };
        match backend.ping().await {
            Ok(()) => Probe {
                driver_ok: true,
                health: backend.health_report().await,
                error: None,
            },
            Err(error) => {
                *self.inner.last_error.lock().unwrap() = Some(error.message());
                self.bump_status();
                Probe {
                    driver_ok: false,
                    health: None,
                    error: Some(error.message()),
                }
            }
        }
    }

    /// Subscribe to install progress for the settings page.
    pub fn subscribe_install(&self) -> watch::Receiver<InstallProgress> {
        self.inner.install_tx.subscribe()
    }

    // -- endpoint ------------------------------------------------------------

    /// The harness-facing URL — bound on first use. `None` when the computer
    /// isn't ready or there's no Tokio runtime (same contract as the browser
    /// bridge: harnesses only get an endpoint that truly exists).
    pub fn endpoint_url(&self) -> Option<String> {
        if !self.ready() {
            return None;
        }
        let mut guard = self.inner.endpoint.lock().unwrap();
        if let Some(endpoint) = guard.as_ref() {
            return Some(endpoint.url.clone());
        }
        let endpoint = self.bind_endpoint()?;
        let url = endpoint.url.clone();
        *guard = Some(endpoint);
        Some(url)
    }

    fn bind_endpoint(&self) -> Option<Endpoint> {
        let handle = tokio::runtime::Handle::try_current().ok()?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
        listener.set_nonblocking(true).ok()?;
        let port = listener.local_addr().ok()?.port();
        let url = format!("http://127.0.0.1:{port}/mcp/{token}");
        let service = self.clone();
        let serve = handle.spawn(async move {
            let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                return;
            };
            crate::endpoint::serve(listener, token, service).await;
        });
        Some(Endpoint { url, _serve: serve })
    }

    /// Stamped-onto-runs readiness: enabled + platform + installed component
    /// (or the mock escape hatch). The driver itself spawns lazily on first
    /// tool call so a slow driver can't delay run setup.
    fn ready(&self) -> bool {
        let settings = self.settings();
        if !settings.enabled {
            return false;
        }
        // A live backend (spawned or injected) counts as ready regardless of
        // the on-disk install — `with_backend` services and the mock both
        // ride this branch.
        if self.inner.backend.lock().unwrap().is_some() {
            return true;
        }
        release_asset().is_some() && (Self::mock_enabled() || install::verify(&self.inner.paths))
    }

    // -- install / uninstall -------------------------------------------------

    /// Download + verify + unpack the pinned Cua driver. Progress lands on
    /// the watch channel. Refuses while an install is already running.
    pub async fn install(&self) -> Result<(), ComputerError> {
        if self.inner.installing.swap(true, Ordering::SeqCst) {
            return Err(ComputerError::Install {
                stage: "install".into(),
                message: "an install is already running".into(),
            });
        }
        let result = self.install_inner().await;
        self.inner.installing.store(false, Ordering::SeqCst);
        self.bump_status();
        result
    }

    /// The RPC install path — replies immediately; progress and failure ride
    /// `subscribe_install()`/`subscribe_status()` so a ~100MB download never
    /// holds an IPC call open. Needs a Tokio runtime.
    pub fn start_install(&self) -> Result<(), ComputerError> {
        if self.inner.installing.swap(true, Ordering::SeqCst) {
            return Err(ComputerError::Install {
                stage: "install".into(),
                message: "an install is already running".into(),
            });
        }
        let service = self.clone();
        tokio::spawn(async move {
            let result = service.install_inner().await;
            service.inner.installing.store(false, Ordering::SeqCst);
            service.bump_status();
            result
        });
        Ok(())
    }

    async fn install_inner(&self) -> Result<(), ComputerError> {
        let asset = release_asset().ok_or(ComputerError::UnsupportedPlatform)?;
        self.drop_backend();
        let result = install::install(&self.inner.paths, &asset, &self.inner.install_tx).await;
        if let Err(error) = &result {
            let _ = self
                .inner
                .install_tx
                .send(InstallProgress::Failed(error.message()));
        }
        result.map(|_| ())
    }

    pub async fn uninstall(&self) -> Result<(), ComputerError> {
        self.drop_backend();
        if let Some(endpoint) = self.inner.endpoint.lock().unwrap().take() {
            endpoint._serve.abort();
        }
        let result = install::uninstall(&self.inner.paths);
        self.bump_status();
        result
    }

    // -- dispatch ------------------------------------------------------------

    /// The ONE funnel for `tools/call` — the HTTP endpoint and the engine's
    /// `ComputerToolCall` RPC both come through here, so control-mode policy
    /// is enforced in exactly one place.
    pub async fn dispatch_tool(&self, name: &str, args: Value) -> Result<ToolReply, ComputerError> {
        // `computer_status` answers even when disabled — it's how a harness
        // learns the computer exists and why it isn't usable.
        if name == "computer_status" {
            return Ok(ToolReply::text(
                serde_json::to_string_pretty(&self.status()).unwrap_or_default(),
            ));
        }

        let settings = self.settings();
        if !settings.enabled {
            return Err(ComputerError::Disabled);
        }
        let backend = self.ensure_backend().await?;

        match name {
            "computer_observe" => {
                let observation = self.observe_with(&*backend, &settings, &args).await?;
                Ok(self.observation_reply(&observation, &settings, &args))
            }
            "computer_list_apps" => {
                let value = backend.inspect(&InspectKind::Apps).await?;
                Ok(ToolReply::text(
                    serde_json::to_string_pretty(&value).unwrap_or_default(),
                ))
            }
            "computer_list_windows" => {
                let kind = InspectKind::Windows {
                    pid: get_i64(&args, "pid"),
                    on_screen_only: get_bool(&args, "onScreenOnly").unwrap_or(false),
                };
                let value = backend.inspect(&kind).await?;
                Ok(ToolReply::text(
                    serde_json::to_string_pretty(&value).unwrap_or_default(),
                ))
            }
            "computer_find" => self.find_tool(&*backend, &settings, &args).await,
            "computer_verify" => {
                let pid = get_i64(&args, "pid").ok_or_else(|| {
                    ComputerError::InvalidArgs("computer_verify needs 'pid'".into())
                })?;
                let expect = args.get("expect").cloned().ok_or_else(|| {
                    ComputerError::InvalidArgs("needs 'expect' predicates".into())
                })?;
                let outcome = backend
                    .verify(pid, get_i64(&args, "windowId"), expect)
                    .await?;
                Ok(ToolReply::text(
                    serde_json::to_string_pretty(&outcome).unwrap_or_default(),
                ))
            }
            "computer_wait" => self.wait_tool(&*backend, &settings, &args).await,
            "computer_clipboard_read" => {
                let request = crate::backend::ActionRequest {
                    tool: "computer_clipboard_read".into(),
                    args: args.clone(),
                    delivery: Delivery::Background,
                    session: None,
                    screenshot_dir: self.inner.screenshot_dir.clone(),
                    observe_screenshot: false,
                    observe_semantic: false,
                    max_elements: 0,
                    max_dimension: 0,
                };
                let outcome = backend.act(&request).await?;
                let mut text = outcome.summary;
                if let Some(details) = &outcome.details {
                    text.push('\n');
                    text.push_str(&serde_json::to_string_pretty(details).unwrap_or_default());
                }
                Ok(ToolReply::text(text))
            }
            name if surface::is_action_tool(name) => {
                self.dispatch_action(&*backend, &settings, name, &args)
                    .await
            }
            _ => Err(ComputerError::InvalidArgs(format!("unknown tool '{name}'"))),
        }
    }

    /// The control-mode gate. Every action lands here before the backend
    /// sees it — observe-only refuses everything; background mode refuses
    /// `deliveryMode:"foreground"`; full control passes both through.
    /// Persistent-foreground tools (`computer_focus_app`) need Full Control
    /// regardless — they change what the user looks at, not just borrow it.
    /// Identical calls that keep failing are loop-broken after
    /// `REPEAT_FAIL_CAP` consecutive failures with a re-observe hint.
    async fn dispatch_action(
        &self,
        backend: &dyn ComputerBackend,
        settings: &ComputerSettings,
        name: &str,
        args: &Value,
    ) -> Result<ToolReply, ComputerError> {
        if surface::requires_full_control(name) && settings.control != ControlMode::FullControl {
            return Err(ComputerError::refused(
                "control_mode",
                format!(
                    "'{name}' takes persistent foreground — enable Full Control in Settings → Computer"
                ),
            ));
        }
        let delivery = match get_str(args, "deliveryMode") {
            Some("foreground") => {
                if settings.control != ControlMode::FullControl {
                    return Err(ComputerError::refused(
                        "control_mode",
                        "deliveryMode 'foreground' needs Full Control — enable it in Settings → Computer",
                    ));
                }
                Delivery::Foreground
            }
            _ => Delivery::Background,
        };
        if !settings.control.acts() {
            return Err(ComputerError::refused(
                "control_mode",
                "Cosmos Computer is in observe-only mode — enable Control in Settings → Computer to act",
            ));
        }

        // The repeat-failure loop breaker — identical calls that already
        // failed REPEAT_FAIL_CAP times in a row are refused with a state hint
        // instead of being re-delivered to the driver.
        let fail_key = format!("{name}:{}", serde_json::to_string(args).unwrap_or_default());
        if self.failure_count(&fail_key) >= REPEAT_FAIL_CAP {
            return Err(ComputerError::refused(
                "repeated_failure",
                format!(
                    "'{name}' already failed {REPEAT_FAIL_CAP} times with these arguments — \
                     the UI state has moved on. Re-observe (computer_observe) and pick a fresh target."
                ),
            ));
        }

        // scroll_until is a service-level loop (observe → find → scroll →
        // repeat) — the backend only knows single actions.
        if name == "computer_scroll_until" {
            return self.scroll_until(backend, settings, delivery, args).await;
        }

        let request = crate::backend::ActionRequest {
            tool: name.to_string(),
            args: args.clone(),
            delivery,
            session: None,
            screenshot_dir: self.inner.screenshot_dir.clone(),
            observe_screenshot: settings.screenshots,
            observe_semantic: settings.semantic,
            max_elements: settings.inspect_max_elements,
            max_dimension: settings.screenshot_max_dimension,
        };
        let outcome = match backend.act(&request).await {
            Ok(outcome) => {
                self.record_success(&fail_key);
                outcome
            }
            Err(error) => {
                self.record_failure(&fail_key);
                return Err(error);
            }
        };
        let mut text = outcome.summary;
        if let Some(details) = &outcome.details {
            text.push('\n');
            text.push_str(&serde_json::to_string_pretty(details).unwrap_or_default());
        }

        // Inline post-action steps the caller asked for: `expect` runs the
        // driver's verify_state; `afterObserve` re-snapshots the window so the
        // result is visible without a second round-trip.
        let pid = get_i64(args, "pid");
        let window_id = get_i64(args, "windowId");
        if let Some(expect) = args.get("expect") {
            match (pid, window_id) {
                (Some(pid), _) => match backend.verify(pid, window_id, expect.clone()).await {
                    Ok(verification) => {
                        text.push_str("\n--- verification ---\n");
                        text.push_str(
                            &serde_json::to_string_pretty(&verification).unwrap_or_default(),
                        );
                    }
                    Err(error) => {
                        text.push_str(&format!("\n--- verification failed to run: {error}"));
                    }
                },
                _ => {
                    text.push_str("\n--- 'expect' skipped: needs pid (+windowId) to bind the check")
                }
            }
        }
        if let Some(post) = &outcome.observe {
            text.push_str("\n--- post-action state ---\n");
            text.push_str(&post.text);
        }
        Ok(ToolReply {
            text,
            image: outcome
                .observe
                .as_ref()
                .and_then(|o| o.screenshot.as_ref())
                .and_then(|s| std::fs::read(&s.path).ok()),
        })
    }

    /// `computer_find` — fresh full-state snapshot + ranked query match.
    async fn find_tool(
        &self,
        backend: &dyn ComputerBackend,
        settings: &ComputerSettings,
        args: &Value,
    ) -> Result<ToolReply, ComputerError> {
        let pid = get_i64(args, "pid")
            .ok_or_else(|| ComputerError::InvalidArgs("computer_find needs 'pid'".into()))?;
        let query = get_str(args, "query")
            .ok_or_else(|| ComputerError::InvalidArgs("computer_find needs 'query'".into()))?;
        let role = get_str(args, "role");
        let limit = get_i64(args, "maxResults").unwrap_or(8).clamp(1, 20) as usize;
        // The driver's `query` projection filters the FULL tree driver-side
        // (matches + ancestors, original indices preserved) — strictly better
        // recall than filtering the locally capped element set.
        let state = backend
            .window_state(
                pid,
                get_i64(args, "windowId"),
                true,
                false,
                settings.inspect_max_elements,
                0,
                &self.inner.screenshot_dir,
                Some(query),
            )
            .await?;
        let hits = digest::find(&state, query, role.as_deref(), limit);
        let projection_note = if state.returned_element_count < state.total_element_count {
            format!(
                " (driver projected {} of {} elements)",
                state.returned_element_count, state.total_element_count
            )
        } else {
            String::new()
        };
        Ok(ToolReply::text(format!(
            "pid {pid} window {}{}\n{}",
            state.window_id,
            projection_note,
            digest::render_hits(&hits, &state.snapshot_id)
        )))
    }

    /// `computer_wait` — poll until a condition holds or the timeout elapses.
    /// Element/text conditions ride `verify_state` or a re-snapshot; window
    /// and stability conditions re-snapshot directly. Polls ~3×/s.
    async fn wait_tool(
        &self,
        backend: &dyn ComputerBackend,
        settings: &ComputerSettings,
        args: &Value,
    ) -> Result<ToolReply, ComputerError> {
        let pid = get_i64(args, "pid")
            .ok_or_else(|| ComputerError::InvalidArgs("computer_wait needs 'pid'".into()))?;
        let window_id = get_i64(args, "windowId");
        let condition = args
            .get("for")
            .cloned()
            .ok_or_else(|| ComputerError::InvalidArgs("computer_wait needs 'for'".into()))?;
        let timeout = Duration::from_millis(
            get_i64(args, "timeoutMs")
                .unwrap_or(10_000)
                .clamp(200, 45_000) as u64,
        );
        let deadline = Instant::now() + timeout;
        let mut last_fingerprint: Option<String> = None;
        let mut last_summary = String::from("unavailable");
        loop {
            match self
                .wait_check(
                    backend,
                    settings,
                    pid,
                    window_id,
                    &condition,
                    &mut last_fingerprint,
                )
                .await
            {
                Ok(Some(summary)) => return Ok(ToolReply::text(format!("satisfied: {summary}"))),
                Ok(None) => {}
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                return Err(ComputerError::InvalidArgs(format!(
                    "timeout: condition not met within {}ms. Last observed: {last_summary}",
                    timeout.as_millis()
                )));
            }
            if let Ok(state) = backend
                .window_state(
                    pid,
                    window_id,
                    true,
                    false,
                    settings.inspect_max_elements.min(600),
                    0,
                    &self.inner.screenshot_dir,
                    None,
                )
                .await
            {
                last_summary = format!(
                    "snapshot {} — {} elements",
                    state.snapshot_id, state.returned_element_count
                );
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// One wait-condition evaluation → `Some(summary)` when satisfied.
    async fn wait_check(
        &self,
        backend: &dyn ComputerBackend,
        settings: &ComputerSettings,
        pid: i64,
        window_id: Option<i64>,
        condition: &Value,
        last_fingerprint: &mut Option<String>,
    ) -> Result<Option<String>, ComputerError> {
        let semantic_cap = settings.inspect_max_elements.min(600);
        if let Some(selector) = condition.get("element") {
            let expect = json!([{ "element": { "selector": selector, "exists": true } }]);
            let outcome = backend.verify(pid, window_id, expect).await?;
            // verify_state reports `status` = satisfied|unsatisfied|unknown —
            // `unknown` is NOT success.
            let satisfied = outcome.get("status").and_then(Value::as_str) == Some("satisfied");
            return Ok(satisfied.then(|| format!("element matched {}", selector)));
        }
        if let Some(selector) = condition.get("elementGone") {
            let expect = json!([{ "element": { "selector": selector, "exists": true } }]);
            return match backend.verify(pid, window_id, expect).await {
                Ok(outcome) => Ok(
                    if outcome.get("status").and_then(Value::as_str) == Some("satisfied") {
                        None
                    } else {
                        Some(format!("element gone ({selector})"))
                    },
                ),
                Err(error)
                    if matches!(
                        error.code(),
                        "stale_element" | "element_not_found" | "window_not_found"
                    ) =>
                {
                    Ok(Some("element gone (window re-rendered)".into()))
                }
                Err(error) => Err(error),
            };
        }
        if let Some(text) = condition.get("text").and_then(Value::as_str) {
            let state = backend
                .window_state(
                    pid,
                    window_id,
                    true,
                    false,
                    semantic_cap,
                    0,
                    &self.inner.screenshot_dir,
                    None,
                )
                .await?;
            let needle = text.to_lowercase();
            return Ok(state
                .elements
                .iter()
                .any(|e| {
                    e.label
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&needle)
                        || e.value
                            .as_deref()
                            .unwrap_or_default()
                            .to_lowercase()
                            .contains(&needle)
                })
                .then(|| format!("text '{text}' visible")));
        }
        if condition.get("window").and_then(Value::as_bool) == Some(true) {
            return match backend
                .window_state(
                    pid,
                    window_id,
                    false,
                    false,
                    1,
                    0,
                    &self.inner.screenshot_dir,
                    None,
                )
                .await
            {
                Ok(state) => Ok(Some(format!(
                    "window {} present ({})",
                    state.window_id,
                    state.window_title.as_deref().unwrap_or("untitled")
                ))),
                Err(_) => Ok(None),
            };
        }
        if condition.get("windowGone").and_then(Value::as_bool) == Some(true) {
            return match backend
                .window_state(
                    pid,
                    window_id,
                    false,
                    false,
                    1,
                    0,
                    &self.inner.screenshot_dir,
                    None,
                )
                .await
            {
                Ok(_) => Ok(None),
                Err(_) => Ok(Some("window gone".into())),
            };
        }
        if condition.get("stable").and_then(Value::as_bool) == Some(true) {
            let state = backend
                .window_state(
                    pid,
                    window_id,
                    true,
                    false,
                    semantic_cap,
                    0,
                    &self.inner.screenshot_dir,
                    None,
                )
                .await?;
            let fingerprint = digest::render(&state);
            if last_fingerprint.as_deref() == Some(fingerprint.as_str()) {
                return Ok(Some("UI stable (two identical snapshots)".into()));
            }
            *last_fingerprint = Some(fingerprint);
            return Ok(None);
        }
        Err(ComputerError::InvalidArgs(
            "'for' must name one condition: element|elementGone|text|window|windowGone|stable"
                .into(),
        ))
    }

    /// `computer_scroll_until` — the scroll loop Cosmos owns: observe → find
    /// → scroll → re-observe, capped, with an edge stop when content stops
    /// changing between steps.
    async fn scroll_until(
        &self,
        backend: &dyn ComputerBackend,
        settings: &ComputerSettings,
        delivery: Delivery,
        args: &Value,
    ) -> Result<ToolReply, ComputerError> {
        let pid = get_i64(args, "pid")
            .ok_or_else(|| ComputerError::InvalidArgs("scroll_until needs 'pid'".into()))?;
        let window_id = get_i64(args, "windowId");
        let query = get_str(args, "query")
            .ok_or_else(|| ComputerError::InvalidArgs("scroll_until needs 'query'".into()))?;
        let role = get_str(args, "role");
        let direction = get_str(args, "direction").unwrap_or_else(|| "down".into());
        let by = get_str(args, "by").unwrap_or_else(|| "page".into());
        let max_steps = get_i64(args, "maxSteps").unwrap_or(8).clamp(1, 20);

        let mut last_fingerprint = String::new();
        for step in 0..max_steps {
            // Full unprojected state — the edge detector compares whole
            // digests, and a `query` projection would change the fingerprint
            // independent of scrolling.
            let state = backend
                .window_state(
                    pid,
                    window_id,
                    true,
                    false,
                    settings.inspect_max_elements,
                    0,
                    &self.inner.screenshot_dir,
                    None,
                )
                .await?;
            let hits = digest::find(&state, query, role.as_deref(), 3);
            if !hits.is_empty() {
                return Ok(ToolReply::text(format!(
                    "found after {step} scroll step(s):\npid {pid} window {}\n{}",
                    state.window_id,
                    digest::render_hits(&hits, &state.snapshot_id)
                )));
            }
            let fingerprint = digest::render(&state);
            if step > 0 && fingerprint == last_fingerprint {
                return Ok(ToolReply::text(format!(
                    "reached scroll edge after {step} step(s) — '{query}' not found. \
                     Snapshot {} ({} elements).",
                    state.snapshot_id, state.returned_element_count
                )));
            }
            last_fingerprint = fingerprint;
            let request = crate::backend::ActionRequest {
                tool: "computer_scroll".into(),
                args: json!({
                    "pid": pid,
                    "windowId": window_id,
                    "direction": direction,
                    "by": by,
                    "amount": 3,
                }),
                delivery,
                session: None,
                screenshot_dir: self.inner.screenshot_dir.clone(),
                observe_screenshot: false,
                observe_semantic: false,
                max_elements: 0,
                max_dimension: 0,
            };
            if let Err(error) = backend.act(&request).await {
                return Ok(ToolReply::text(format!(
                    "scroll stopped at step {step}: {error}. Re-observe and retry \
                     or pick another container."
                )));
            }
            tokio::time::sleep(Duration::from_millis(220)).await;
        }
        Err(ComputerError::InvalidArgs(format!(
            "scroll_until exhausted {max_steps} steps without finding '{query}' — \
             re-observe; the target may live in a different container."
        )))
    }

    fn failure_count(&self, key: &str) -> u32 {
        self.inner
            .failures
            .lock()
            .map(|map| map.get(key).copied().unwrap_or(0))
            .unwrap_or(0)
    }

    fn record_failure(&self, key: &str) {
        if let Ok(mut map) = self.inner.failures.lock() {
            *map.entry(key.to_string()).or_insert(0) += 1;
        }
    }

    fn record_success(&self, key: &str) {
        if let Ok(mut map) = self.inner.failures.lock() {
            map.remove(key);
        }
    }

    async fn observe_with(
        &self,
        backend: &dyn ComputerBackend,
        settings: &ComputerSettings,
        args: &Value,
    ) -> Result<Observation, ComputerError> {
        let scope = match get_str(args, "scope").unwrap_or("frontmost") {
            "desktop" => ObserveScope::Desktop,
            "window" => {
                let pid = get_i64(args, "pid").ok_or_else(|| {
                    ComputerError::InvalidArgs("scope 'window' needs 'pid'".into())
                })?;
                ObserveScope::Window {
                    pid,
                    window_id: get_i64(args, "windowId"),
                }
            }
            _ => ObserveScope::Frontmost,
        };
        backend
            .observe(&ObserveRequest {
                scope,
                semantic: settings.semantic,
                screenshot: get_bool(args, "includeScreenshot").unwrap_or(false)
                    && settings.screenshots,
                full_tree: get_str(args, "format") == Some("full"),
                max_elements: get_u32(args, "maxElements").unwrap_or(settings.inspect_max_elements),
                max_dimension: settings.screenshot_max_dimension,
                screenshot_dir: self.inner.screenshot_dir.clone(),
            })
            .await
    }

    /// Serialize an Observation for the MCP surface: semantic text plus a
    /// JSON header; a screenshot PNG rides as a sibling image block.
    fn observation_reply(
        &self,
        observation: &Observation,
        settings: &ComputerSettings,
        args: &Value,
    ) -> ToolReply {
        let image = observation
            .screenshot
            .as_ref()
            .and_then(|s| std::fs::read(&s.path).ok());
        let mut text = String::new();
        if let Some(window) = &observation.window {
            let mut header = serde_json::json!({
                "scope": observation.scope,
                "appName": window.app_name,
                "windowTitle": window.window_title,
                "bundleId": window.bundle_id,
                "pid": window.pid,
                "windowId": window.window_id,
                "truncated": observation.truncated,
                "modality": observation.modality,
            });
            if get_bool(args, "includeScreenshot") == Some(true) && !settings.screenshots {
                header["screenshotNote"] =
                    json!("screenshots are disabled in Settings → Computer — semantic data only");
            }
            text.push_str(&serde_json::to_string_pretty(&header).unwrap_or_default());
            text.push('\n');
        }
        text.push_str(&observation.text);
        ToolReply { text, image }
    }

    /// The UI's observe path — returns the typed Observation (screenshot as
    /// a STAGED FILE the UI reads via attachment plumbing, not inline bytes).
    pub async fn observe(&self, args: Value) -> Result<Observation, ComputerError> {
        let settings = self.settings();
        if !settings.enabled {
            return Err(ComputerError::Disabled);
        }
        let backend = self.ensure_backend().await?;
        self.observe_with(&*backend, &settings, &args).await
    }

    // -- backend lifecycle -----------------------------------------------------

    async fn ensure_backend(&self) -> Result<Arc<dyn ComputerBackend>, ComputerError> {
        if let Some(backend) = self.inner.backend.lock().unwrap().as_ref() {
            return Ok(backend.clone());
        }
        let _spawn = self.inner.spawn_lock.lock().await;
        if let Some(backend) = self.inner.backend.lock().unwrap().as_ref() {
            return Ok(backend.clone());
        }
        let backend = self.spawn_backend().await.map_err(|error| {
            *self.inner.last_error.lock().unwrap() = Some(error.message());
            self.bump_status();
            error
        })?;
        *self.inner.backend.lock().unwrap() = Some(backend.clone());
        *self.inner.last_error.lock().unwrap() = None;
        self.bump_status();
        Ok(backend)
    }

    async fn spawn_backend(&self) -> Result<Arc<dyn ComputerBackend>, ComputerError> {
        if let Some(mock) = crate::mock::MockComputer::from_env() {
            return Ok(Arc::new(mock));
        }
        // Gate order: platform → installed → spawn. Each error maps straight
        // to a remediation hint on the settings page.
        if release_asset().is_none() {
            return Err(ComputerError::UnsupportedPlatform);
        }
        let install = self.inner.paths.installed()?;
        let install = match install {
            Some(install) if ComputerPaths::binary_in(&install.install_dir).is_some() => install,
            Some(_) => return Err(ComputerError::Incomplete),
            None => return Err(ComputerError::NotInstalled),
        };
        let settings = self.settings();
        let backend = crate::cua::CuaBackend::spawn(&self.inner.paths, &settings, &install).await?;
        Ok(Arc::new(backend))
    }

    /// Kill the driver. Settings changes and uninstalls call this — the next
    /// use respawns lazily. `kill_on_drop` does the actual teardown.
    fn drop_backend(&self) {
        drop(self.inner.backend.lock().unwrap().take());
    }
}

impl Clone for ComputerService {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// What `probe` learns about a live driver.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub driver_ok: bool,
    /// The driver's own `health_report` payload (platform checks, TCC grants).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// -- arg helpers -------------------------------------------------------------

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn get_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

fn get_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(Value::as_i64)
}

fn get_u32(args: &Value, key: &str) -> Option<u32> {
    args.get(key).and_then(Value::as_u64).map(|v| v as u32)
}
