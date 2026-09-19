//! `CuaBackend` — production [`ComputerBackend`] over a locally installed
//! `cua-driver` binary (`cua-driver mcp`, NDJSON MCP on stdio).
//!
//! Cosmos tools are deliberately a curated surface: the harness asks for
//! `computer_*` tools; this module picks the driver's concrete call, fills
//! shared parameters (delivery_mode, session), and normalizes the result.
//! Semantic context is always preferred — screenshots flow only when the
//! request explicitly asks and settings allow.
//!
//! ## Session lifecycle
//!
//! The driver's sessions expire after ~5 minutes idle ("this session has
//! ended; call start_session explicitly"). [`CuaBackend::call`] attaches the
//! shared `cosmos` label to every call and transparently revives an ended
//! session once — harnesses never see the driver's lifecycle.
//!
//! ## Element handles
//!
//! `get_window_state` returns `structuredContent.elements[]` — each with an
//! `element_index` (per-snapshot), an `element_token` (`s{snap}:{index}` —
//! snapshot-bound, stale-safe), role/label/value/frame/actions. Snapshots
//! replace the element map per `(pid, window_id)`; stale handles fail closed
//! driver-side and surface here as `stale_element` — the harness re-observes.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::backend::{
    ActionOutcome, ActionRequest, BackendDescriptor, ComputerBackend, ElementFrame, InspectKind,
    ObserveRequest, ObserveScope, WindowElement, WindowState,
};
use crate::digest;
use crate::mcp_client::McpClient;
use crate::{
    ComputerError, ComputerPaths, ComputerSettings, InstallState, Observation, ObservedWindow,
    StagedScreenshot,
};

/// Session-label prefix forwarded on every driver call — groups all
/// Cosmos-driven calls into one driver-side lifecycle session (idle TTL
/// ~5 min; revived transparently by [`CuaBackend::call`]). The label is
/// unique per backend spawn (`cosmos-<pid>`): names tombstone daemon-side
/// when their transport dies, so a shared static label wedges the first
/// respawn inside the TTL window with `session_unavailable`.
const SESSION_LABEL_PREFIX: &str = "cosmos";

pub struct CuaBackend {
    client: McpClient,
    /// This backend's unique driver session label.
    session: String,
    version: Option<String>,
    /// One in-flight driver call at a time. Overlapping `list_windows` after
    /// a cancelled AT-SPI walk is how the Linux stdio child wedges.
    call_lock: tokio::sync::Mutex<()>,
}

impl CuaBackend {
    /// Spawn `cua-driver mcp` for an installed component. The permission
    /// profile comes from the Cosmos control mode (observe → `bounded` with
    /// the generated observe-only manifest; control modes → `standard`), so
    /// driver-side enforcement matches the Cosmos surface policy.
    pub async fn spawn(
        paths: &ComputerPaths,
        settings: &ComputerSettings,
        install: &InstallState,
    ) -> Result<Self, ComputerError> {
        let binary = ComputerPaths::binary_in(&install.install_dir).ok_or_else(|| {
            ComputerError::Unavailable(format!(
                "no cua-driver binary under {}",
                install.install_dir.display()
            ))
        })?;
        let manifest = if settings.permission_mode() == crate::PermissionMode::Bounded {
            let manifest = paths.observe_manifest();
            if let Some(parent) = manifest.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(&manifest, crate::OBSERVE_MANIFEST_YAML)
                .map_err(|e| ComputerError::Internal(e.to_string()))?;
            Some(manifest)
        } else {
            None
        };
        let env = settings.driver_env(manifest.as_deref());
        let client = McpClient::spawn(&binary, &env, &[]).await?;
        let backend = Self {
            client,
            session: format!("{SESSION_LABEL_PREFIX}-{}", std::process::id()),
            version: None,
            call_lock: tokio::sync::Mutex::new(()),
        };
        // Name the shared session up-front — `start_session` is idempotent and
        // this also proves the session tools exist before first real use.
        backend.start_session().await?;
        let version = backend
            .call("get_config", json!({}))
            .await
            .ok()
            .and_then(|result| result_text(&result))
            .and_then(|text| {
                serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v.get("version").and_then(Value::as_str).map(str::to_string))
            })
            .or_else(|| Some(install.version.clone()));
        Ok(Self {
            client: backend.client,
            session: backend.session,
            version,
            call_lock: backend.call_lock,
        })
    }

    /// `start_session` for this backend's label — creates or revives it.
    async fn start_session(&self) -> Result<(), ComputerError> {
        // Called directly on the client (not `call`) — the revive path calls
        // this, and wrapping it would recurse.
        match self
            .client
            .call_tool("start_session", json!({ "session": self.session }))
            .await
        {
            // Defensive: a live lease on the same label (theoretically a
            // colliding pid reused within the tombstone TTL) is ended and
            // retried once rather than wedging the spawn.
            Err(error) if Self::is_session_unavailable(&error) => {
                let _ = self
                    .client
                    .call_tool("end_session", json!({ "session": self.session }))
                    .await;
                self.client
                    .call_tool("start_session", json!({ "session": self.session }))
                    .await
                    .map(|_| ())
            }
            other => other.map(|_| ()),
        }
    }

    /// Is this error the driver's dead-session refusal?
    pub(crate) fn is_session_ended(error: &ComputerError) -> bool {
        match error {
            ComputerError::Refused { code, message } => {
                code == "session_ended"
                    || Self::is_session_unavailable(error)
                    || message.contains("session has ended")
                    || message.contains("call start_session")
            }
            _ => false,
        }
    }

    /// Is this error the stale-lease refusal (`session is not available to
    /// this transport`)? Distinct from `session_ended`: the name survives,
    /// still bound to a transport that no longer exists.
    pub(crate) fn is_session_unavailable(error: &ComputerError) -> bool {
        match error {
            ComputerError::Refused { code, message } => {
                code == "session_unavailable" || message.contains("not available to this transport")
            }
            _ => false,
        }
    }

    /// Every driver call funnels through here: attaches the `cosmos` session
    /// label (tools that accept it — the label is ignored by the rest), then
    /// on a dead-session refusal revives the session and retries ONCE. All
    /// other errors pass through untouched.
    async fn call(&self, tool: &str, args: Value) -> Result<Value, ComputerError> {
        self.call_with_timeout(tool, args, crate::mcp_client::CALL_TIMEOUT)
            .await
    }

    async fn call_with_timeout(
        &self,
        tool: &str,
        mut args: Value,
        timeout: std::time::Duration,
    ) -> Result<Value, ComputerError> {
        let _guard = self.call_lock.lock().await;
        if let Some(obj) = args.as_object_mut()
            && !obj.contains_key("session")
        {
            obj.insert("session".into(), json!(self.session));
        }
        if let Some(label) = args.get("session").and_then(Value::as_str)
            && label != self.session
        {
            self.ensure_named_session(label).await?;
        }
        match self
            .client
            .call_tool_timeout(tool, args.clone(), timeout)
            .await
        {
            Err(error) if Self::is_session_ended(&error) => {
                self.start_session().await?;
                self.client.call_tool_timeout(tool, args, timeout).await
            }
            other => other,
        }
    }

    async fn ensure_named_session(&self, label: &str) -> Result<(), ComputerError> {
        match self
            .client
            .call_tool("start_session", json!({ "session": label }))
            .await
        {
            Ok(_) => Ok(()),
            Err(error) if Self::is_session_unavailable(&error) => {
                let _ = self
                    .client
                    .call_tool("end_session", json!({ "session": label }))
                    .await;
                self.client
                    .call_tool("start_session", json!({ "session": label }))
                    .await
                    .map(|_| ())
            }
            Err(error) => Err(error),
        }
    }

    /// The driver's own health report (platform checks, permission state,
    /// overall verdict) — the diagnostics backbone for `computer_status`.
    pub async fn health_report(&self) -> Option<Value> {
        self.client
            .call_tool("health_report", json!({}))
            .await
            .ok()
            .and_then(|result| {
                result
                    .get("structuredContent")
                    .cloned()
                    .or_else(|| result_text(&result).and_then(|t| serde_json::from_str(&t).ok()))
            })
    }

    /// All top-level windows, thinly normalized (`window_id`, `pid`,
    /// `app_name`, `title`, `z_index`, `minimized`, `is_on_screen`).
    async fn list_windows(&self, pid: Option<i64>) -> Result<Vec<Value>, ComputerError> {
        #[cfg(target_os = "linux")]
        if let Ok(value) = crate::linux_x11::list_windows(pid, false).await
            && let Some(arr) = value.get("windows").and_then(Value::as_array)
            && (!arr.is_empty() || pid.is_none())
        {
            return Ok(arr.clone());
        }
        let args = match pid {
            Some(pid) => json!({"pid": pid}),
            None => json!({}),
        };
        let result = self
            .call_with_timeout("list_windows", args, crate::mcp_client::INSPECT_TIMEOUT)
            .await?;
        Ok(result_windows(&result))
    }

    /// Pick the window `computer_observe{scope:"frontmost"}` describes: the
    /// topmost on-screen window not owned by this process (Cosmos itself is
    /// frontmost while the user clicks around in it — capturing it would be
    /// the appshot self-capture bug) nor by the driver.
    async fn frontmost_window(&self) -> Result<Value, ComputerError> {
        let own_pid = std::process::id() as i64;
        let driver_pid = self.client.child_pid().map(|p| p as i64);
        let windows = self.list_windows(None).await?;
        let mut best: Option<&Value> = None;
        let mut best_z = i64::MIN;
        let mut any_z = false;
        for window in &windows {
            if window.get("minimized").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let pid = window.get("pid").and_then(Value::as_i64);
            if pid == Some(own_pid) || (driver_pid.is_some() && pid == driver_pid) {
                continue;
            }
            let title = window.get("title").and_then(Value::as_str).unwrap_or("");
            let app = window.get("app_name").and_then(Value::as_str).unwrap_or("");
            if matches!(title, "xfce4-panel" | "plank" | "Desktop")
                || matches!(app, "Xfce4-panel" | "Plank" | "Xfdesktop")
            {
                continue;
            }
            match window.get("z_index").and_then(Value::as_i64) {
                Some(z) => {
                    any_z = true;
                    if z > best_z {
                        best_z = z;
                        best = Some(window);
                    }
                }
                None => {
                    if best.is_none() && !any_z {
                        best = Some(window);
                    }
                }
            }
        }
        best.cloned().ok_or_else(|| {
            ComputerError::refused(
                "no_window",
                "No foreground application window is available to observe.",
            )
        })
    }

    /// Resolve a pid (+optional window_id) to a concrete window record.
    /// `window_id` absent → the pid's frontmost on-screen window.
    async fn window_for(&self, pid: i64, window_id: Option<i64>) -> Result<Value, ComputerError> {
        let windows = self.list_windows(Some(pid)).await?;
        if let Some(id) = window_id {
            return windows
                .iter()
                .find(|w| w.get("window_id").and_then(Value::as_i64) == Some(id))
                .cloned()
                .ok_or_else(|| {
                    ComputerError::refused(
                        "window_not_found",
                        format!("window {id} was not found for pid {pid}"),
                    )
                });
        }
        // Prefer on-screen windows: macOS reports off-screen remnants (dismissed
        // sheets, phantom document windows) whose AX tree yields zero elements.
        let on_screen: Vec<&Value> = windows
            .iter()
            .filter(|w| w.get("minimized").and_then(Value::as_bool) != Some(true))
            .filter(|w| w.get("is_on_screen").and_then(Value::as_bool) != Some(false))
            .collect();
        let fallback: Vec<&Value> = windows.iter().collect();
        let candidates: &[&Value] = if on_screen.is_empty() {
            &fallback
        } else {
            &on_screen
        };
        candidates
            .iter()
            .copied()
            .max_by_key(|w| w.get("z_index").and_then(Value::as_i64).unwrap_or(0))
            .or_else(|| windows.first())
            .cloned()
            .ok_or_else(|| {
                ComputerError::refused(
                    "window_not_found",
                    format!("pid {pid} has no window to observe"),
                )
            })
    }

    /// `get_window_state` → typed [`WindowState`]. The structured `elements[]`
    /// (with `element_token` handles) is the grounding surface — the digest
    /// renders from it, never from `tree_markdown`.
    async fn window_state_inner(
        &self,
        window: &Value,
        semantic: bool,
        screenshot: bool,
        max_elements: u32,
        max_dimension: u32,
        screenshot_dir: &Path,
        query: Option<&str>,
    ) -> Result<WindowState, ComputerError> {
        let pid = window
            .get("pid")
            .and_then(Value::as_i64)
            .ok_or_else(|| ComputerError::Internal("window record has no pid".into()))?;
        let window_id = window
            .get("window_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| ComputerError::Internal("window record has no window_id".into()))?;
        let screenshot_path = screenshot
            .then(|| screenshot_dir.join(format!("computer-{}.png", uuid::Uuid::new_v4())));
        let mut args = json!({
            "pid": pid,
            "window_id": window_id,
            "include_accessibility_tree": semantic,
            "include_screenshot": screenshot,
            "max_elements": max_elements,
            "max_depth": 25,
            "max_dimension": max_dimension,
            "query": query,
        });
        strip_nulls(&mut args);
        if let Some(path) = &screenshot_path {
            args["screenshot_out_file"] = json!(path.to_string_lossy());
        }
        let result = match self
            .call_with_timeout("get_window_state", args, crate::mcp_client::OBSERVE_TIMEOUT)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                #[cfg(target_os = "linux")]
                if let Ok(record) = crate::linux_x11::window_record(pid, Some(window_id)).await {
                    let title = record
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("window");
                    let app = record
                        .get("app_name")
                        .and_then(Value::as_str)
                        .unwrap_or("app");
                    let text = format!(
                        "X11 fallback observe (Cua get_window_state: {error}).\n\
                         app={app} title={title} pid={pid} windowId={window_id}\n\
                         Semantic AT-SPI tree unavailable this pass — re-observe or use computer_list_windows."
                    );
                    return Ok(WindowState {
                        snapshot_id: "x11-fallback".into(),
                        pid,
                        window_id,
                        app_name: Some(app.into()),
                        window_title: Some(title.into()),
                        window_bounds: None,
                        elements: Vec::new(),
                        tree_markdown: Some(text.clone()),
                        returned_element_count: 0,
                        total_element_count: 0,
                        note: Some(text),
                        screenshot: None,
                    });
                }
                return Err(error);
            }
        };
        let structured = result
            .get("structuredContent")
            .cloned()
            .unwrap_or(json!({}));
        let elements = structured
            .get("elements")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().map(parse_element).collect())
            .unwrap_or_default();
        let screenshot = finish_screenshot(screenshot_path).await;
        // Caveats worth surfacing to the model — a degraded/projected/Chromium
        // snapshot changes how much the element set can be trusted.
        let mut notes: Vec<String> = Vec::new();
        if let Some(reason) = structured.get("degraded_reason").and_then(Value::as_str) {
            notes.push(format!("degraded: {reason}"));
        }
        if let Some(coverage) = structured.get("capture_coverage") {
            notes.push(format!(
                "capture coverage: {}",
                coverage
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| coverage.to_string())
            ));
        }
        if let Some(reason) = structured.get("truncation_reason").and_then(Value::as_str) {
            notes.push(format!("truncated: {reason}"));
        }
        let note = (!notes.is_empty()).then(|| notes.join("; "));
        Ok(WindowState {
            snapshot_id: structured
                .get("snapshot_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            pid,
            window_id,
            app_name: structured
                .get("app_name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    window
                        .get("app_name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                }),
            window_title: structured
                .get("window_title")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    window
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                }),
            window_bounds: structured.get("window_bounds").and_then(parse_frame),
            elements,
            tree_markdown: structured
                .get("tree_markdown")
                .and_then(Value::as_str)
                .map(str::to_string),
            returned_element_count: structured
                .get("returned_element_count")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            total_element_count: structured
                .get("total_element_count")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            note,
            screenshot,
        })
    }

    /// `get_window_state` → Observation: the compact digest by default; the
    /// driver's `tree_markdown` only when the caller asked for `format:"full"`.
    async fn observe_window(
        &self,
        window: &Value,
        request: &ObserveRequest,
    ) -> Result<Observation, ComputerError> {
        #[cfg(target_os = "linux")]
        if request.semantic && !request.screenshot && !request.full_tree {
            return Ok(crate::linux_x11::observation_from_record(window));
        }
        let state = self
            .window_state_inner(
                window,
                request.semantic,
                request.screenshot,
                request.max_elements,
                request.max_dimension,
                &request.screenshot_dir,
                None,
            )
            .await?;
        let truncated = state.returned_element_count < state.total_element_count
            && state.total_element_count > 0;
        let mut text = if request.semantic {
            if request.full_tree {
                state
                    .tree_markdown
                    .clone()
                    .unwrap_or_else(|| digest::render(&state))
            } else {
                digest::render(&state)
            }
        } else {
            String::new()
        };
        if text.trim().is_empty() {
            if let Some(md) = &state.tree_markdown {
                text = md.clone();
            } else if let Some(note) = &state.note {
                text = note.clone();
            }
        }
        if text.is_empty() && !request.semantic {
            text = format!(
                "Screenshot of {} captured.",
                state.app_name.as_deref().unwrap_or("the window")
            );
        }
        Ok(Observation {
            scope: "window".into(),
            window: Some(ObservedWindow {
                app_name: state.app_name.clone(),
                window_title: state.window_title.clone(),
                bundle_id: None,
                pid: Some(state.pid),
                window_id: Some(state.window_id),
            }),
            text,
            truncated,
            modality: match (&state.screenshot, request.semantic) {
                (Some(_), true) => "both",
                (Some(_), false) => "visual",
                (None, _) => "semantic",
            }
            .to_string(),
            screenshot: state.screenshot,
        })
    }

    /// Desktop scope: a compact grouped window/app inventory (from
    /// `list_windows` + `list_apps` for active markers) plus an optional
    /// full-screen capture. This is the "what's on my screen" shape —
    /// grouped by app, windows listed with pid/hwnd/bounds so the harness
    /// can target precisely without another call.
    async fn observe_desktop(
        &self,
        request: &ObserveRequest,
    ) -> Result<Observation, ComputerError> {
        let mut text = String::new();
        if request.semantic {
            let windows = self.list_windows(None).await?;
            text = render_desktop_windows(&windows);
        }
        let screenshot_path = request.screenshot.then(|| {
            request
                .screenshot_dir
                .join(format!("computer-{}.png", uuid::Uuid::new_v4()))
        });
        if let Some(path) = &screenshot_path {
            self.call(
                "get_desktop_state",
                json!({ "screenshot_out_file": path.to_string_lossy() }),
            )
            .await?;
        }
        let screenshot = finish_screenshot(screenshot_path).await;
        if text.is_empty() && !request.semantic {
            text = "Desktop screenshot captured.".into();
        }
        Ok(Observation {
            scope: "desktop".into(),
            window: None,
            text,
            truncated: false,
            modality: match (&screenshot, request.semantic) {
                (Some(_), true) => "both",
                (Some(_), false) => "visual",
                (None, _) => "semantic",
            }
            .to_string(),
            screenshot,
        })
    }

    /// Translate a `computer_*` tool call into the concrete driver call and
    /// normalize the outcome. `request.delivery` was already gated by the
    /// service — forward it verbatim.
    async fn act_inner(&self, request: &ActionRequest) -> Result<ActionOutcome, ComputerError> {
        let args = &request.args;
        let delivery = request.delivery.as_str();
        let (tool, mut call_args) = match request.tool.as_str() {
            "computer_launch_app" => (
                "launch_app",
                json!({
                    "name": str_arg(args, "name"),
                    "path": str_arg(args, "path"),
                    "bundle_id": str_arg(args, "bundleId"),
                    "aumid": str_arg(args, "aumid"),
                    "launch_path": str_arg(args, "launchPath"),
                    "additional_arguments": args.get("args").cloned().unwrap_or(Value::Null),
                    "urls": args.get("urls").cloned().unwrap_or(Value::Null),
                    "start_minimized": bool_arg(args, "startMinimized").unwrap_or(false),
                }),
            ),
            "computer_close_app" => ("kill_app", json!({ "pid": int_arg(args, "pid") })),
            "computer_focus_app" => (
                "bring_to_front",
                json!({ "pid": int_arg(args, "pid"), "window_id": int_arg(args, "windowId") }),
            ),
            "computer_click" | "computer_right_click" | "computer_double_click" => {
                let tool = match request.tool.as_str() {
                    "computer_right_click" => "right_click",
                    "computer_double_click" => "double_click",
                    _ => "click",
                };
                (
                    tool,
                    target_args(
                        args,
                        delivery,
                        json!({
                            "button": str_arg(args, "button"),
                            "count": int_arg(args, "count"),
                            "modifier": args.get("modifiers").cloned().unwrap_or(Value::Null),
                        }),
                    ),
                )
            }
            "computer_type_text" => (
                "type_text",
                target_args(
                    args,
                    delivery,
                    json!({
                        "text": str_arg(args, "text"),
                        "delay_ms": int_arg(args, "delayMs"),
                    }),
                ),
            ),
            "computer_set_value" => (
                "set_value",
                target_args(args, delivery, json!({ "value": str_arg(args, "value") })),
            ),
            "computer_press_key" => (
                "press_key",
                target_args(
                    args,
                    delivery,
                    json!({
                        "key": str_arg(args, "key"),
                        "modifiers": args.get("modifiers").cloned().unwrap_or(Value::Null),
                    }),
                ),
            ),
            "computer_hotkey" => (
                "hotkey",
                target_args(
                    args,
                    delivery,
                    json!({ "keys": args.get("keys").cloned().unwrap_or(json!([])) }),
                ),
            ),
            "computer_scroll" | "computer_scroll_until" => (
                "scroll",
                target_args(
                    args,
                    delivery,
                    json!({
                        "direction": str_arg(args, "direction").unwrap_or_else(|| "down".into()),
                        "amount": int_arg(args, "amount"),
                        "by": str_arg(args, "by"),
                    }),
                ),
            ),
            "computer_drag" => (
                "drag",
                target_args(
                    args,
                    delivery,
                    json!({
                        "from_x": num_arg(args, "fromX"),
                        "from_y": num_arg(args, "fromY"),
                        "to_x": num_arg(args, "toX"),
                        "to_y": num_arg(args, "toY"),
                        "duration_ms": int_arg(args, "durationMs"),
                        "steps": int_arg(args, "steps"),
                    }),
                ),
            ),
            "computer_invoke_menu" => (
                // invoke_menu resolves the menu item by path — the driver
                // accepts only pid/window_id/path/session here (no
                // delivery_mode; menu invocation is already background AX).
                // The surface's "File > Save" string becomes the driver's
                // segment array ["File","Save"]. window_id is driver-required;
                // resolved after this match when absent.
                "invoke_menu",
                json!({
                    "pid": int_arg(args, "pid"),
                    "window_id": int_arg(args, "windowId"),
                    "path": menu_path(args),
                }),
            ),
            "computer_set_window_frame" => (
                "set_window_frame",
                json!({
                    "pid": int_arg(args, "pid"),
                    "window_id": int_arg(args, "windowId"),
                    "x": num_arg(args, "x"),
                    "y": num_arg(args, "y"),
                    "width": num_arg(args, "width"),
                    "height": num_arg(args, "height"),
                }),
            ),
            "computer_clipboard_read" => (
                "clipboard_read",
                json!({ "include_text": bool_arg(args, "includeText").unwrap_or(false) }),
            ),
            "computer_clipboard_write" => (
                "clipboard_write",
                json!({
                    "text": str_arg(args, "text"),
                    "image_path": str_arg(args, "imagePath"),
                    "file_path": str_arg(args, "filePath"),
                }),
            ),
            "computer_move_cursor" => (
                // The overlay cursor only — `scope:"desktop"` (real pointer)
                // is intentionally never forwarded.
                "move_cursor",
                json!({
                    "x": num_arg(args, "x"),
                    "y": num_arg(args, "y"),
                    "scope": "window",
                    "target": args.get("target").cloned().unwrap_or(Value::Null),
                }),
            ),
            other => {
                return Err(ComputerError::InvalidArgs(format!(
                    "unknown computer tool: {other}"
                )));
            }
        };
        strip_nulls(&mut call_args);
        if let Some(session) = &request.session
            && let Some(obj) = call_args.as_object_mut()
        {
            obj.insert("session".into(), json!(session));
        }
        // invoke_menu requires window_id driver-side; the surface treats it
        // as "defaults to the pid's frontmost window" — resolve it here.
        if tool == "invoke_menu" && call_args.get("window_id").is_none() {
            let pid = call_args
                .get("pid")
                .and_then(Value::as_i64)
                .ok_or_else(|| ComputerError::InvalidArgs("invoke_menu needs 'pid'".into()))?;
            let window = self.window_for(pid, None).await?;
            if let Some(id) = window.get("window_id").and_then(Value::as_i64) {
                call_args["window_id"] = json!(id);
            }
        }
        let result = self.call(tool, call_args).await?;
        let details = result.get("structuredContent").cloned();
        let mut summary = result_text(&result).unwrap_or_else(|| format!("{tool} completed"));
        // Surface the driver's effect report inline — a successful tool call
        // is NOT proof the user's task happened; effect+route+delivery+escalation
        // is. `suspected_noop`/`unverifiable` get an explicit follow-up hint.
        if let Some(sc) = &details {
            let effect = sc.get("effect").and_then(Value::as_str);
            let route = sc.get("route").and_then(Value::as_str);
            let mode = sc
                .get("delivery")
                .and_then(|d| d.get("mode"))
                .and_then(Value::as_str);
            let escalation = sc
                .get("escalation")
                .and_then(|e| e.get("reason"))
                .and_then(Value::as_str);
            let mut facts = Vec::new();
            if let Some(e) = effect {
                facts.push(format!("effect:{e}"));
            }
            if let Some(r) = route {
                facts.push(format!("route:{r}"));
            }
            if let Some(m) = mode {
                facts.push(format!("delivery:{m}"));
            }
            if let Some(e) = escalation {
                facts.push(format!("escalation:{e}"));
            }
            if !facts.is_empty() {
                summary.push_str(&format!(" [{}]", facts.join(" ")));
            }
            match effect {
                Some("suspected_noop") => {
                    summary.push_str(" — the action likely did nothing; re-observe before retrying")
                }
                Some("unverifiable") | Some("partial") => {
                    summary.push_str(" — verify the outcome before continuing")
                }
                Some("refused") => summary.push_str(" — refused by the driver"),
                _ => {}
            }
        }
        Ok(ActionOutcome {
            summary,
            details,
            observe: None,
        })
    }
}

/// Shared target/addressing args for action tools — `pid`/`window_id`/
/// `element_token`/`element_index`/`snapshot_id`/`x`/`y`/`scope`/`target`/
/// `delivery_mode` pass straight through; `extra` adds the tool-specific ones.
fn target_args(args: &Value, delivery: &str, extra: Value) -> Value {
    let mut out = json!({
        "pid": int_arg(args, "pid"),
        "window_id": int_arg(args, "windowId"),
        "element_token": str_arg(args, "elementToken"),
        "element_index": int_arg(args, "elementIndex"),
        "snapshot_id": str_arg(args, "snapshotId"),
        "x": num_arg(args, "x"),
        "y": num_arg(args, "y"),
        "scope": str_arg(args, "scope"),
        "target": args.get("target").cloned().unwrap_or(Value::Null),
        "delivery_mode": delivery,
    });
    if let (Some(out_obj), Some(extra_obj)) = (out.as_object_mut(), extra.as_object()) {
        for (key, value) in extra_obj {
            out_obj.insert(key.clone(), value.clone());
        }
    }
    out
}

/// Surface menu path ("File > Save" string, or an already-array path) → the
/// driver's `path` segment array.
fn menu_path(args: &Value) -> Value {
    match args.get("path") {
        Some(Value::String(path)) => json!(
            path.split('>')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        ),
        other => other.cloned().unwrap_or(Value::Null),
    }
}

fn strip_nulls(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.retain(|_, v| !v.is_null());
    }
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

fn int_arg(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(Value::as_i64)
}

fn num_arg(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64)
}

fn bool_arg(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

/// `structuredContent.elements[i]` → typed element. Tolerant of missing
/// fields — the driver omits unset state flags entirely.
fn parse_element(v: &Value) -> WindowElement {
    WindowElement {
        index: v.get("element_index").and_then(Value::as_u64).unwrap_or(0) as u32,
        token: v
            .get("element_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        role: v
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("Element")
            .to_string(),
        label: v.get("label").and_then(Value::as_str).map(str::to_string),
        value: v.get("value").and_then(Value::as_str).map(str::to_string),
        enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        frame: v.get("frame").and_then(parse_frame),
        depth: v.get("depth").and_then(Value::as_u64).unwrap_or(0) as u32,
        actions: v
            .get("actions")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        parent_index: v
            .get("parent_index")
            .and_then(Value::as_u64)
            .map(|i| i as u32),
        focused: v.get("focused").and_then(Value::as_bool),
        selected: v.get("selected").and_then(Value::as_bool),
        checked: v.get("checked").and_then(Value::as_bool),
        expanded: v.get("expanded").and_then(Value::as_bool),
        scrollable: v.get("scrollable").and_then(Value::as_bool),
        in_web_content: v.get("in_web_content").and_then(Value::as_bool),
    }
}

fn parse_frame(v: &Value) -> Option<ElementFrame> {
    let x = v.get("x").and_then(Value::as_f64)?;
    let y = v.get("y").and_then(Value::as_f64)?;
    let w = v
        .get("w")
        .or_else(|| v.get("width"))
        .and_then(Value::as_f64)?;
    let h = v
        .get("h")
        .or_else(|| v.get("height"))
        .and_then(Value::as_f64)?;
    Some(ElementFrame { x, y, w, h })
}

/// Stat a driver-written PNG into a StagedScreenshot (dimension sniffed from
/// the IHDR; the file may be absent when the driver skipped capture).
async fn finish_screenshot(path: Option<PathBuf>) -> Option<StagedScreenshot> {
    let path = path?;
    let meta = tokio::fs::metadata(&path).await.ok()?;
    if !meta.is_file() {
        return None;
    }
    let dims = tokio::fs::read(&path)
        .await
        .ok()
        .and_then(|bytes| png_dimensions(&bytes));
    Some(StagedScreenshot {
        path,
        width: dims.map(|d| d.0),
        height: dims.map(|d| d.1),
        bytes: meta.len(),
    })
}

/// Compact desktop inventory — windows grouped by app, each with the handles
/// an action needs (pid, hwnd, bounds, z).
fn render_desktop_windows(windows: &[Value]) -> String {
    // Group by app name preserving z order.
    let mut by_app: Vec<(String, Vec<&Value>)> = Vec::new();
    for window in windows {
        let app = window
            .get("app_name")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        if let Some(entry) = by_app.iter_mut().find(|(name, _)| *name == app) {
            entry.1.push(window);
        } else {
            by_app.push((app, vec![window]));
        }
    }
    let mut out = String::with_capacity(windows.len() * 80);
    out.push_str(&format!("{} windows on screen\n", windows.len()));
    for (app, wins) in &by_app {
        out.push_str(&format!("{app}\n"));
        for w in wins {
            let title = w.get("title").and_then(Value::as_str).unwrap_or("");
            let pid = w.get("pid").and_then(Value::as_i64).unwrap_or(0);
            let hwnd = w.get("window_id").and_then(Value::as_i64).unwrap_or(0);
            let z = w.get("z_index").and_then(Value::as_i64).unwrap_or(0);
            let mut line = format!("  pid {pid} hwnd {hwnd} z{z} \"{title}\"");
            if let Some(b) = w.get("bounds") {
                line.push_str(&format!(
                    " @{},{} {}x{}",
                    b.get("x").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                    b.get("y").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                    b.get("width").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                    b.get("height").and_then(Value::as_f64).unwrap_or(0.0) as i64,
                ));
            }
            if w.get("minimized").and_then(Value::as_bool) == Some(true) {
                line.push_str(" [minimized]");
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// First text block of a CallTool result — the driver's `✅ …` summary /
/// markdown payload line.
fn result_text(result: &Value) -> Option<String> {
    result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| {
            content
                .iter()
                .find_map(|c| c.get("text").and_then(Value::as_str))
        })
        .map(str::to_string)
}

/// `list_windows` result → the windows array (structuredContent first, then a
/// JSON parse of the text block — the driver's text payload is JSON here).
fn result_windows(result: &Value) -> Vec<Value> {
    if let Some(windows) = result
        .get("structuredContent")
        .and_then(|s| s.get("windows"))
        .and_then(Value::as_array)
    {
        return windows.clone();
    }
    result_text(result)
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| {
            v.get("windows")
                .and_then(Value::as_array)
                .cloned()
                .or_else(|| v.as_array().cloned())
        })
        .unwrap_or_default()
}

/// PNG IHDR width/height without decoding the image.
fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != PNG_SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

#[async_trait::async_trait]
impl ComputerBackend for CuaBackend {
    async fn ping(&self) -> Result<(), ComputerError> {
        self.call("get_screen_size", json!({})).await.map(|_| ())
    }

    async fn observe(&self, request: &ObserveRequest) -> Result<Observation, ComputerError> {
        match &request.scope {
            ObserveScope::Desktop => self.observe_desktop(request).await,
            ObserveScope::Frontmost => {
                let window = self.frontmost_window().await?;
                self.observe_window(&window, request).await
            }
            ObserveScope::Window { pid, window_id } => {
                let window = self.window_for(*pid, *window_id).await?;
                self.observe_window(&window, request).await
            }
        }
    }

    async fn window_state(
        &self,
        pid: i64,
        window_id: Option<i64>,
        semantic: bool,
        screenshot: bool,
        max_elements: u32,
        max_dimension: u32,
        screenshot_dir: &Path,
        query: Option<&str>,
    ) -> Result<WindowState, ComputerError> {
        let window = self.window_for(pid, window_id).await?;
        self.window_state_inner(
            &window,
            semantic,
            screenshot,
            max_elements,
            max_dimension,
            screenshot_dir,
            query,
        )
        .await
    }

    async fn verify(
        &self,
        pid: i64,
        window_id: Option<i64>,
        expect: Value,
    ) -> Result<Value, ComputerError> {
        // verify_state requires a concrete window_id — resolve the pid's
        // frontmost on-screen window when the caller didn't pin one.
        let window_id = match window_id {
            Some(id) => id,
            None => self
                .window_for(pid, None)
                .await?
                .get("window_id")
                .and_then(Value::as_i64)
                .ok_or_else(|| ComputerError::Internal("window record has no window_id".into()))?,
        };
        let result = self
            .call(
                "verify_state",
                json!({ "pid": pid, "window_id": window_id, "expect": expect }),
            )
            .await?;
        Ok(result
            .get("structuredContent")
            .cloned()
            .or_else(|| result_text(&result).and_then(|t| serde_json::from_str(&t).ok()))
            .unwrap_or(result))
    }

    async fn inspect(&self, kind: &InspectKind) -> Result<Value, ComputerError> {
        let (tool, args, pid, on_screen_only) = match kind {
            InspectKind::Apps => ("list_apps", json!({}), None, false),
            InspectKind::Windows {
                pid,
                on_screen_only,
            } => (
                "list_windows",
                json!({
                    "pid": pid,
                    "on_screen_only": on_screen_only,
                }),
                *pid,
                *on_screen_only,
            ),
        };
        let mut args = args;
        strip_nulls(&mut args);
        let timeout = if tool == "list_windows" || tool == "list_apps" {
            crate::mcp_client::INSPECT_TIMEOUT
        } else {
            crate::mcp_client::CALL_TIMEOUT
        };
        match self.call_with_timeout(tool, args, timeout).await {
            Ok(result) => Ok(result
                .get("structuredContent")
                .cloned()
                .or_else(|| result_text(&result).and_then(|t| serde_json::from_str(&t).ok()))
                .unwrap_or(result)),
            Err(error) => {
                #[cfg(target_os = "linux")]
                if tool == "list_windows" {
                    if let Ok(fallback) = crate::linux_x11::list_windows(pid, on_screen_only).await
                    {
                        return Ok(fallback);
                    }
                }
                Err(error)
            }
        }
    }

    async fn act(&self, request: &ActionRequest) -> Result<ActionOutcome, ComputerError> {
        self.act_inner(request).await
    }

    async fn health_report(&self) -> Option<Value> {
        self.health_report().await
    }

    fn describe(&self) -> BackendDescriptor {
        BackendDescriptor {
            kind: "cua-driver",
            version: self.version.clone(),
            platform: crate::platform_label(),
            semantic: true,
            screenshots: true,
            background_control: true,
            foreground_control: true,
        }
    }
}
