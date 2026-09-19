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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use crate::backend::{
    ActionOutcome, ActionRequest, BackendDescriptor, ComputerBackend, ElementFrame, InspectKind,
    ObserveRequest, ObserveScope, WindowElement, WindowState,
};
use crate::digest;
use crate::mcp_client::McpClient;
use crate::menu::{NativeMenuOutcome, normalized_menu_segment};
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

#[derive(Clone)]
struct CachedElement {
    pid: i64,
    window_id: i64,
    frame: ElementFrame,
    window_bounds: Option<ElementFrame>,
    screen_absolute: bool,
    role: String,
}

pub struct CuaBackend {
    /// Shared driver MCP subprocess — `Arc` so per-session views of the same
    /// transport stay cheap (one `cua-driver mcp` process multiplexes every
    /// named session; each label gets its own cursor/recording/lifecycle).
    client: Arc<McpClient>,
    /// This backend's unique driver session label.
    session: String,
    version: Option<String>,
    /// (pid, unresolved sheet CG window) → owning AX window, learned by
    /// retargeted observes; shared across session views so an alias learned
    /// by one caller routes every later action on that window.
    sheet_owners: Arc<Mutex<HashMap<(i64, i64), i64>>>,
    /// Snapshot element frames used to fall back to screen-point actions when
    /// an out-of-process sheet rejects a token addressed to its owner window.
    element_frames: Arc<Mutex<HashMap<String, CachedElement>>>,
    native_menu: Option<NativeMenuPress>,
}

pub(crate) type NativeMenuPress = Arc<dyn Fn(i32, &[String]) -> NativeMenuOutcome + Send + Sync>;

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
        #[cfg(target_os = "macos")]
        let native_menu: Option<NativeMenuPress> =
            Some(Arc::new(crate::macos_menu::press_menu_path));
        #[cfg(not(target_os = "macos"))]
        let native_menu = None;
        let backend = Self {
            client: Arc::new(client),
            session: format!("{SESSION_LABEL_PREFIX}-{}", std::process::id()),
            version: None,
            sheet_owners: Arc::new(Mutex::new(HashMap::new())),
            element_frames: Arc::new(Mutex::new(HashMap::new())),
            native_menu,
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
            sheet_owners: backend.sheet_owners,
            element_frames: backend.element_frames,
            native_menu: backend.native_menu,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_native_menu(mut self, native_menu: NativeMenuPress) -> Self {
        self.native_menu = Some(native_menu);
        self
    }

    /// A session-scoped view of this backend: same driver MCP transport, a
    /// different `session` label on every call. One MCP connection multiplexes
    /// any number of named sessions, so concurrent agents each get their own
    /// driver-side lifecycle (cursor overlay, recording, idle-TTL cleanup)
    /// instead of all writes landing in the process-wide `cosmos-<pid>` name.
    pub fn session_scoped(&self, label: &str) -> Self {
        Self {
            client: self.client.clone(),
            session: format!("{}-{}", self.session, label),
            version: self.version.clone(),
            sheet_owners: self.sheet_owners.clone(),
            element_frames: self.element_frames.clone(),
            native_menu: self.native_menu.clone(),
        }
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

    /// Is this error the driver's dead-session refusal? The driver's phrasing
    /// varies across versions ("session 'x' has ended", "Call start_session…"),
    /// so match case-insensitively on the message as well as the wire code.
    pub(crate) fn is_session_ended(error: &ComputerError) -> bool {
        match error {
            ComputerError::Refused { code, message } => {
                let m = message.to_lowercase();
                code == "session_ended"
                    || Self::is_session_unavailable(error)
                    || m.contains("has ended")
                    || m.contains("call start_session")
                    || m.contains("start_session with this id")
            }
            _ => false,
        }
    }

    /// The CG-window → AX-window mapping failure for sheet/dialog windows:
    /// the driver sees the CG window but its AX node hangs under the parent,
    /// so the lookup can't resolve. Surfaced as `ax_window_unresolved` (or the
    /// message text when the code isn't propagated).
    pub(crate) fn is_ax_window_unresolved(error: &ComputerError) -> bool {
        match error {
            ComputerError::Refused { code, message } => {
                let message = message.to_ascii_lowercase();
                code == "ax_window_unresolved"
                    || message.contains("ax_window_unresolved")
                    || message.contains("element_outside_target_window")
                    || message.contains("no ax window")
                    || message.contains("element_token conflicts")
                    || message.contains("off_space_or_ax_unresolved")
            }
            _ => false,
        }
    }

    fn is_element_outside_target_window(error: &ComputerError) -> bool {
        matches!(
            error,
            ComputerError::Refused { message, .. }
                if message
                    .to_ascii_lowercase()
                    .contains("element_outside_target_window")
        )
    }

    fn screen_absolute_for_state(state: &WindowState) -> bool {
        match state.window_bounds {
            None => true,
            Some(bounds) => state
                .elements
                .iter()
                .find(|element| {
                    element.frame.is_some()
                        && (element.role.eq_ignore_ascii_case("AXWindow")
                            || element.role.to_ascii_lowercase().ends_with("window"))
                })
                .and_then(|element| element.frame)
                .is_some_and(|frame| {
                    (frame.x - bounds.x).abs() <= 1.0 && (frame.y - bounds.y).abs() <= 1.0
                }),
        }
    }

    fn frame_center(
        frame: ElementFrame,
        window_bounds: Option<ElementFrame>,
        screen_absolute: bool,
    ) -> Option<(f64, f64)> {
        if screen_absolute {
            Some((frame.x + frame.w / 2.0, frame.y + frame.h / 2.0))
        } else {
            let bounds = window_bounds?;
            Some((
                bounds.x + frame.x + frame.w / 2.0,
                bounds.y + frame.y + frame.h / 2.0,
            ))
        }
    }

    fn cache_window_state(&self, state: &WindowState) {
        let screen_absolute = Self::screen_absolute_for_state(state);
        let mut cache = self
            .element_frames
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Keep the cache bounded without retaining bookkeeping for snapshot
        // generations; a fresh observation repopulates entries after a clear.
        if cache.len() > 20_000 {
            cache.clear();
        }
        for element in &state.elements {
            let (Some(token), Some(frame)) = (element.token.as_ref(), element.frame) else {
                continue;
            };
            cache.insert(
                token.clone(),
                CachedElement {
                    pid: state.pid,
                    window_id: state.window_id,
                    frame,
                    window_bounds: state.window_bounds,
                    screen_absolute,
                    role: element.role.clone(),
                },
            );
        }
    }

    async fn retry_out_of_process_sheet(
        &self,
        tool: &str,
        call_args: &Value,
        first_error: &ComputerError,
    ) -> Option<Result<(Value, String), ComputerError>> {
        let token = call_args
            .get("element_token")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                let snapshot_id = call_args.get("snapshot_id").and_then(Value::as_str)?;
                let element_index = call_args.get("element_index").and_then(Value::as_i64)?;
                Some(format!("{snapshot_id}:{element_index}"))
            })?;
        let cached = self
            .element_frames
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&token)
            .cloned()?;
        if call_args.get("pid").and_then(Value::as_i64) != Some(cached.pid) {
            return None;
        }
        let (x, y) =
            Self::frame_center(cached.frame, cached.window_bounds, cached.screen_absolute)?;
        let fallback_note = if cached.role.to_ascii_lowercase().starts_with("axmenu") {
            format!(
                " (menu item lives in the menu-bar window; fell back to a screen-point click at ({x:.0},{y:.0}))"
            )
        } else {
            format!(
                " (element lives in an out-of-process sheet; fell back to a screen-point action at ({x:.0},{y:.0}))"
            )
        };
        match tool {
            "click" | "double_click" | "right_click" => {
                let result = self
                    .call(tool, json!({ "scope": "desktop", "x": x, "y": y }))
                    .await;
                Some(result.map(|result| (result, fallback_note)))
            }
            "set_value" => {
                let value = call_args.get("value").and_then(Value::as_str)?;
                let click = self
                    .call("click", json!({ "scope": "desktop", "x": x, "y": y }))
                    .await;
                if click.is_err() {
                    return Some(Err(first_error.clone()));
                }
                let key = self
                    .call(
                        "press_key",
                        json!({
                            "scope": "desktop",
                            "key": "a",
                            "modifiers": ["command"],
                        }),
                    )
                    .await;
                if key.is_err() {
                    return Some(Err(first_error.clone()));
                }
                let typed = self
                    .call("type_text", json!({ "scope": "desktop", "text": value }))
                    .await;
                Some(typed.map(|result| (result, fallback_note)))
            }
            "type_text" => {
                let text = call_args.get("text").and_then(Value::as_str)?;
                Some(
                    self.call("type_text", json!({ "scope": "desktop", "text": text }))
                        .await
                        .map(|result| (result, fallback_note)),
                )
            }
            _ => None,
        }
    }

    fn is_ax_unresolved_state(state: &WindowState) -> bool {
        state.elements.is_empty()
            && state
                .note
                .as_deref()
                .is_some_and(|note| note.contains("ax_window_unresolved"))
    }

    fn has_sheet_or_dialog(state: &WindowState) -> bool {
        state.elements.iter().any(|element| {
            element.role.eq_ignore_ascii_case("AXSheet")
                || element.role.eq_ignore_ascii_case("AXDialog")
                || element.role.eq_ignore_ascii_case("sheet")
                || element.role.eq_ignore_ascii_case("dialog")
        })
    }

    fn is_menu_path_not_found(error: &ComputerError) -> bool {
        match error {
            ComputerError::Refused { message, .. } => {
                let message = message.to_ascii_lowercase();
                message.contains("path segment") && message.contains("not found")
            }
            _ => false,
        }
    }

    fn normalized_menu_segment(segment: &str) -> String {
        normalized_menu_segment(segment)
    }

    fn menu_type_select_segment(segment: &str) -> String {
        let mut segment = segment.trim();
        segment = segment.strip_suffix('…').unwrap_or(segment).trim_end();
        segment = segment.strip_suffix("...").unwrap_or(segment).trim_end();
        segment.to_string()
    }

    /// Closed menu items remain in the AX tree but have no frame.
    fn open_menu_item_visible(state: &WindowState, segment: &str) -> bool {
        let normalized_segment = Self::normalized_menu_segment(segment);
        digest::find(state, segment, Some("AXMenuItem"), 5)
            .into_iter()
            .any(|hit| {
                hit.frame.is_some()
                    && hit
                        .label
                        .as_deref()
                        .or(hit.value.as_deref())
                        .is_some_and(|label| {
                            Self::normalized_menu_segment(label) == normalized_segment
                        })
            })
    }

    fn menu_path_from_state(state: &WindowState, requested: &[String]) -> Option<Vec<String>> {
        let titles: Vec<String> = state
            .elements
            .iter()
            .filter(|element| {
                matches!(
                    element.role.to_ascii_lowercase().as_str(),
                    "axmenubaritem" | "axmenuitem" | "menubaritem" | "menuitem"
                )
            })
            .filter_map(|element| {
                element
                    .label
                    .as_deref()
                    .or(element.value.as_deref())
                    .map(str::to_string)
            })
            .collect();
        requested
            .iter()
            .map(|segment| {
                let normalized = Self::normalized_menu_segment(segment);
                titles
                    .iter()
                    .find(|title| Self::normalized_menu_segment(title) == normalized)
                    .cloned()
            })
            .collect()
    }

    async fn retry_invoke_menu(
        &self,
        call_args: &Value,
        first_error: ComputerError,
        screenshot_dir: &Path,
    ) -> Result<(Value, Vec<String>), ComputerError> {
        let Some(requested) = call_args
            .get("path")
            .and_then(Value::as_array)
            .map(|segments| {
                segments
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|segments| !segments.is_empty())
        else {
            return Err(first_error);
        };
        let resolved = match (
            call_args.get("pid").and_then(Value::as_i64),
            call_args.get("window_id").and_then(Value::as_i64),
        ) {
            (Some(pid), Some(window_id)) => {
                let window = self.list_windows(Some(pid)).await.ok().and_then(|windows| {
                    windows.into_iter().find(|window| {
                        window.get("window_id").and_then(Value::as_i64) == Some(window_id)
                    })
                });
                match window {
                    Some(window) => self
                        .window_state_inner(&window, true, false, 2000, 1440, screenshot_dir, None)
                        .await
                        .ok()
                        .and_then(|state| Self::menu_path_from_state(&state, &requested)),
                    None => None,
                }
            }
            _ => None,
        };
        let path = resolved.unwrap_or_else(|| {
            let mut fallback = requested.clone();
            if let Some(last) = fallback.last_mut() {
                let base = last
                    .trim()
                    .trim_end_matches('…')
                    .trim_end_matches("...")
                    .trim_end();
                *last = format!("{base}…");
            }
            fallback
        });
        let mut retry_args = call_args.clone();
        retry_args["path"] = json!(path);
        match self.call("invoke_menu", retry_args).await {
            Ok(result) => Ok((result, path)),
            Err(_) => Err(first_error),
        }
    }

    async fn recover_invoke_menu(
        &self,
        call_args: &Value,
        screenshot_dir: &Path,
    ) -> Result<Option<String>, ComputerError> {
        let Some(path) = call_args.get("path").and_then(Value::as_array) else {
            return Ok(None);
        };
        let Some(segment) = path.last().and_then(Value::as_str) else {
            return Ok(None);
        };
        let Some(pid) = call_args.get("pid").and_then(Value::as_i64) else {
            return Ok(None);
        };
        let Some(window_id) = call_args.get("window_id").and_then(Value::as_i64) else {
            return Ok(None);
        };

        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let Some(window) = self.list_windows(Some(pid)).await.ok().and_then(|windows| {
            windows
                .into_iter()
                .find(|window| window.get("window_id").and_then(Value::as_i64) == Some(window_id))
        }) else {
            return Ok(None);
        };
        let Ok(state) = self
            .window_state_inner(&window, true, false, 2000, 1440, screenshot_dir, None)
            .await
        else {
            return Ok(None);
        };
        if !Self::open_menu_item_visible(&state, segment) {
            return Ok(None);
        }
        let _ = self.wait_for_app_active(pid).await;
        let Some(window) = self.list_windows(Some(pid)).await.ok().and_then(|windows| {
            windows
                .into_iter()
                .find(|window| window.get("window_id").and_then(Value::as_i64) == Some(window_id))
        }) else {
            return Ok(None);
        };
        let Ok(state) = self
            .window_state_inner(&window, true, false, 2000, 1440, screenshot_dir, None)
            .await
        else {
            return Ok(None);
        };
        if !Self::open_menu_item_visible(&state, segment) {
            return Ok(Some(" (menu settled after activation)".into()));
        }
        let type_select_segment = Self::menu_type_select_segment(segment);
        self.call(
            "type_text",
            json!({"scope": "desktop", "text": type_select_segment}),
        )
        .await?;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        self.call("press_key", json!({"scope": "desktop", "key": "return"}))
            .await?;

        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let Ok(state_after_type_select) = self
            .window_state_inner(&window, true, false, 2000, 1440, screenshot_dir, None)
            .await
        else {
            return Ok(None);
        };
        if !Self::open_menu_item_visible(&state_after_type_select, segment) {
            return Ok(Some(format!(
                " (driver's menu press left the menu open; completed \"{segment}\" via menu type-select + Return)"
            )));
        }

        self.call("press_key", json!({ "scope": "desktop", "key": "escape" }))
            .await?;
        Err(ComputerError::refused(
            "suspected_noop",
            format!(
                "invoke_menu: the menu stayed open after dispatch and menu type-select + Return; menu command not confirmed"
            ),
        ))
    }

    /// `verify_state`/`window_state`/`act` retarget for sheet windows: when
    /// `window_id` can't resolve on the CG↔AX map, try the app's other
    /// on-screen windows (topmost first). For pixel actions the window-local
    /// `x,y` are translated by the CG-bounds delta — the sheet's screen
    /// position projected into the owning window's coordinate space.
    async fn retarget_call(
        &self,
        tool: &str,
        call_args: &Value,
        first_error: ComputerError,
    ) -> Result<Value, ComputerError> {
        let Some(obj) = call_args.as_object() else {
            return Err(first_error);
        };
        let (Some(pid), Some(window_id)) = (
            obj.get("pid").and_then(Value::as_i64),
            obj.get("window_id").and_then(Value::as_i64),
        ) else {
            return Err(first_error);
        };
        let sheet_origin = self
            .list_windows(Some(pid))
            .await
            .unwrap_or_default()
            .into_iter()
            .find(|w| w.get("window_id").and_then(Value::as_i64) == Some(window_id))
            .and_then(|w| w.get("bounds").cloned());
        let (sx, sy) = sheet_origin
            .as_ref()
            .map(|b| {
                (
                    b.get("x").and_then(Value::as_f64).unwrap_or(0.0),
                    b.get("y").and_then(Value::as_f64).unwrap_or(0.0),
                )
            })
            .unwrap_or((0.0, 0.0));
        let siblings = self.list_windows(Some(pid)).await.unwrap_or_default();
        let mut alternates: Vec<Value> = siblings
            .into_iter()
            .filter(|w| w.get("window_id").and_then(Value::as_i64) != Some(window_id))
            .filter(|w| w.get("minimized").and_then(Value::as_bool) != Some(true))
            .filter(|w| w.get("is_on_screen").and_then(Value::as_bool) != Some(false))
            .collect();
        alternates.sort_by_key(|w| {
            std::cmp::Reverse(w.get("z_index").and_then(Value::as_i64).unwrap_or(0))
        });
        for candidate in alternates {
            let Some(candidate_id) = candidate.get("window_id").and_then(Value::as_i64) else {
                continue;
            };
            let mut retry = call_args.clone();
            retry["window_id"] = json!(candidate_id);
            if retry.get("element_token").and_then(Value::as_str).is_some() {
                if let Some(retry_obj) = retry.as_object_mut() {
                    retry_obj.remove("snapshot_id");
                    retry_obj.remove("element_index");
                }
            }
            if let (Some(x), Some(y)) = (
                obj.get("x").and_then(Value::as_f64),
                obj.get("y").and_then(Value::as_f64),
            ) {
                if let Some(pb) = candidate.get("bounds") {
                    let px = pb.get("x").and_then(Value::as_f64).unwrap_or(0.0);
                    let py = pb.get("y").and_then(Value::as_f64).unwrap_or(0.0);
                    retry["x"] = json!(x + sx - px);
                    retry["y"] = json!(y + sy - py);
                }
            }
            if let Ok(result) = self.call(tool, retry).await {
                return Ok(result);
            }
        }
        Err(first_error)
    }

    /// Is this error the stale-lease refusal (`session is not available to
    /// this transport`)? Distinct from `session_ended`: the name survives,
    /// still bound to a transport that no longer exists.
    pub(crate) fn is_session_unavailable(error: &ComputerError) -> bool {
        match error {
            ComputerError::Refused { code, message } => {
                code == "session_unavailable"
                    || message
                        .to_lowercase()
                        .contains("not available to this transport")
            }
            _ => false,
        }
    }

    /// Every driver call funnels through here: attaches the `cosmos` session
    /// label (tools that accept it — the label is ignored by the rest), then
    /// on a dead-session refusal revives the session and retries ONCE. All
    /// other errors pass through untouched.
    async fn call(&self, tool: &str, mut args: Value) -> Result<Value, ComputerError> {
        if let Some(obj) = args.as_object_mut()
            && !obj.contains_key("session")
        {
            obj.insert("session".into(), json!(self.session));
        }
        match self.client.call_tool(tool, args.clone()).await {
            Err(error)
                if Self::is_session_ended(&error) || Self::is_session_unavailable(&error) =>
            {
                // First use of a per-agent label (or its TTL expiry) reads as
                // `session_unavailable`; a stale same-name lease from a dead
                // transport reads `session_ended`. `start_session` resolves
                // both, then the call retries once.
                self.start_session().await?;
                self.client.call_tool(tool, args).await
            }
            other => other,
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
        let args = match pid {
            Some(pid) => json!({"pid": pid}),
            None => json!({}),
        };
        let result = self.call("list_windows", args).await?;
        Ok(result_windows(&result))
    }

    async fn app_is_active(&self, pid: i64) -> Option<bool> {
        let result = self.call("list_apps", json!({})).await.ok()?;
        result_apps(&result)
            .into_iter()
            .find(|app| app.get("pid").and_then(Value::as_i64) == Some(pid))
            .and_then(|app| app.get("active").and_then(Value::as_bool))
    }

    /// Poll app activation; an unavailable list_apps response stops polling.
    async fn wait_for_app_active(&self, pid: i64) -> bool {
        for _ in 0..15 {
            match self.app_is_active(pid).await {
                Some(true) => return true,
                Some(false) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                None => return false,
            }
        }
        false
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

    #[cfg(target_os = "macos")]
    async fn window_is_ax_resolvable(&self, window: &Value) -> bool {
        let (Some(pid), Some(window_id)) = (
            window.get("pid").and_then(Value::as_i64),
            window.get("window_id").and_then(Value::as_i64),
        ) else {
            return false;
        };
        self.call(
            "get_window_state",
            json!({
                "pid": pid,
                "window_id": window_id,
                "include_accessibility_tree": false,
                "include_screenshot": false,
                "max_elements": 1,
                "max_depth": 25,
            }),
        )
        .await
        .is_ok()
    }

    #[cfg(target_os = "macos")]
    async fn ax_window_for_pid(&self, pid: i64) -> Option<Value> {
        let window = self.window_for(pid, None).await.ok()?;
        self.window_is_ax_resolvable(&window)
            .await
            .then_some(window)
    }

    #[cfg(target_os = "macos")]
    async fn wait_for_ax_window(&self, pid: i64) -> Option<Value> {
        for attempt in 0..=16 {
            if let Ok(windows) = self.list_windows(Some(pid)).await {
                for window in windows {
                    if self.window_is_ax_resolvable(&window).await {
                        return Some(window);
                    }
                }
            }
            if attempt < 16 {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
        }
        None
    }

    #[cfg(target_os = "macos")]
    async fn reopen_running_app_if_needed(
        &self,
        pid: i64,
        launch_args: &Value,
        summary: &mut String,
    ) {
        if self.ax_window_for_pid(pid).await.is_some() {
            return;
        }

        let reopened = if let Some(bundle_id) = launch_args.get("bundle_id").and_then(Value::as_str)
        {
            tokio::process::Command::new("open")
                .args(["-b", bundle_id])
                .status()
                .await
                .is_ok()
        } else if let Some(name) = launch_args.get("name").and_then(Value::as_str) {
            tokio::process::Command::new("open")
                .args(["-a", name])
                .status()
                .await
                .is_ok()
        } else {
            false
        };

        if reopened && self.wait_for_ax_window(pid).await.is_some() {
            let windows = self.list_windows(Some(pid)).await.unwrap_or_default();
            *summary = render_launch_summary(summary, pid, &windows);
            summary.push_str(" [reopened: app was running with no document window]");
        } else {
            summary.push_str(
                " — app is running but exposes no accessibility window; try computer_invoke_menu with the app's menu bar or relaunch",
            );
        }
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
        let result = self.call("get_window_state", args).await?;
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
        let state = WindowState {
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
        };
        self.cache_window_state(&state);
        Ok(state)
    }

    /// `get_window_state` → Observation: the compact digest by default; the
    /// driver's `tree_markdown` only when the caller asked for `format:"full"`.
    async fn observe_window(
        &self,
        window: &Value,
        request: &ObserveRequest,
    ) -> Result<Observation, ComputerError> {
        let state = self
            .window_state_retargeted(
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
        let mut sheet_owner_note = None;
        if let (Some(pid), Some(window_id)) = (
            call_args.get("pid").and_then(Value::as_i64),
            call_args.get("window_id").and_then(Value::as_i64),
        ) {
            let owner = self
                .sheet_owners
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&(pid, window_id))
                .copied();
            if let Some(owner) = owner.filter(|owner| *owner != window_id) {
                call_args["window_id"] = json!(owner);
                sheet_owner_note = Some(format!(
                    " (window {window_id} is a sheet of window {owner}; addressed {owner})"
                ));
            }
        }
        let mut app_activation_note: Option<String> = None;
        let raised_invoke_menu_window = if tool == "invoke_menu" {
            let pid = call_args.get("pid").and_then(Value::as_i64);
            let window_id = call_args.get("window_id").and_then(Value::as_i64);
            if let (Some(pid), Some(window_id)) = (pid, window_id) {
                let _ = self
                    .call(
                        "bring_to_front",
                        json!({ "pid": pid, "window_id": window_id }),
                    )
                    .await;
                if !self.wait_for_app_active(pid).await {
                    app_activation_note =
                        Some(" (app did not report active before menu dispatch)".into());
                }
                Some(window_id)
            } else {
                None
            }
        } else {
            None
        };
        let mut element_fallback_note = None;
        let mut menu_path_note = None;
        let mut native_menu_note = None;
        let result = if tool == "invoke_menu" {
            let native = self
                .native_menu
                .as_ref()
                .zip(call_args.get("pid").and_then(Value::as_i64))
                .zip(call_args.get("path").and_then(Value::as_array))
                .and_then(|((press, pid), path)| {
                    i32::try_from(pid).ok().map(|pid| {
                        press(
                            pid,
                            &path
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect::<Vec<_>>(),
                        )
                    })
                });
            match native {
                Some(NativeMenuOutcome::Pressed) => {
                    native_menu_note =
                        Some(" (dispatched natively via accessibility menu press)".into());
                    Ok(json!({
                    "content": [{
                        "type": "text",
                        "text": "menu invoked"
                    }]
                    }))
                }
                Some(NativeMenuOutcome::NotFound { segment, available }) => {
                    native_menu_note = Some(format!(
                        " (native menu lookup: segment '{segment}' not found; available: {})",
                        available.join(", ")
                    ));
                    self.call(tool, call_args.clone()).await
                }
                Some(NativeMenuOutcome::Unavailable(message)) => {
                    native_menu_note = Some(format!(" (native menu press unavailable: {message})"));
                    self.call(tool, call_args.clone()).await
                }
                None => self.call(tool, call_args.clone()).await,
            }
        } else {
            self.call(tool, call_args.clone()).await
        };
        let result = match result {
            Err(error) if tool == "invoke_menu" && Self::is_menu_path_not_found(&error) => {
                match self
                    .retry_invoke_menu(&call_args, error.clone(), &request.screenshot_dir)
                    .await
                {
                    Ok((result, path)) => {
                        menu_path_note =
                            Some(format!(" (menu path normalized to {})", path.join(" > ")));
                        result
                    }
                    Err(original) => return Err(original),
                }
            }
            Err(error)
                if Self::is_element_outside_target_window(&error)
                    && (call_args
                        .get("element_token")
                        .and_then(Value::as_str)
                        .is_some()
                        || (call_args
                            .get("snapshot_id")
                            .and_then(Value::as_str)
                            .is_some()
                            && call_args
                                .get("element_index")
                                .and_then(Value::as_i64)
                                .is_some()))
                    && matches!(
                        tool,
                        "click" | "double_click" | "right_click" | "set_value" | "type_text"
                    ) =>
            {
                match self
                    .retry_out_of_process_sheet(tool, &call_args, &error)
                    .await
                {
                    Some(Ok((result, note))) => {
                        element_fallback_note = Some(note);
                        result
                    }
                    Some(Err(_)) | None if Self::is_ax_window_unresolved(&error) => {
                        self.retarget_call(tool, &call_args, error).await?
                    }
                    Some(Err(_)) | None => return Err(error),
                }
            }
            Err(error) if Self::is_ax_window_unresolved(&error) && tool != "launch_app" => {
                // Sheet windows: retarget to the owning window (window-local
                // x,y translated by the CG-bounds delta when present).
                self.retarget_call(tool, &call_args, error).await?
            }
            other => other?,
        };
        let details = result.get("structuredContent").cloned();
        let mut summary = result_text(&result).unwrap_or_else(|| format!("{tool} completed"));
        let mut invoke_menu_recovery_note = None;
        if tool == "invoke_menu" {
            match self
                .recover_invoke_menu(&call_args, &request.screenshot_dir)
                .await
            {
                Ok(note) => invoke_menu_recovery_note = note,
                Err(error) => return Err(error),
            }
        }
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
        if let Some(window_id) = raised_invoke_menu_window {
            summary.push_str(&format!(
                " (raised window {window_id} first so the menu command targets it)"
            ));
        }
        if let Some(note) = app_activation_note {
            summary.push_str(&note);
        }
        if let Some(note) = sheet_owner_note {
            summary.push_str(&note);
        }
        if let Some(note) = menu_path_note {
            summary.push_str(&note);
        }
        if let Some(note) = native_menu_note {
            summary.push_str(&note);
        }
        if let Some(note) = invoke_menu_recovery_note {
            summary.push_str(&note);
        }
        if let Some(note) = element_fallback_note {
            summary.push_str(&note);
        }
        #[cfg(target_os = "macos")]
        if tool == "launch_app"
            && let Some(pid) = pid_from_launch_result(&result)
        {
            self.reopen_running_app_if_needed(pid, &call_args, &mut summary)
                .await;
        }
        Ok(ActionOutcome {
            summary,
            details,
            observe: None,
        })
    }

    async fn window_state_retargeted(
        &self,
        window: &Value,
        semantic: bool,
        screenshot: bool,
        max_elements: u32,
        max_dimension: u32,
        screenshot_dir: &Path,
        query: Option<&str>,
    ) -> Result<WindowState, ComputerError> {
        let primary = self
            .window_state_inner(
                window,
                semantic,
                screenshot,
                max_elements,
                max_dimension,
                screenshot_dir,
                query,
            )
            .await;
        let unresolved = match &primary {
            Err(error) => Self::is_ax_window_unresolved(error),
            Ok(state) => semantic && Self::is_ax_unresolved_state(state),
        };
        if !unresolved {
            return primary;
        }
        // Sheets/dialogs own a CG window id but hang off the PARENT window's
        // AX tree — the driver's CG↔AX map then reports ax_window_unresolved
        // even though the content is reachable. Retry the app's other
        // on-screen windows (topmost first): the parent's snapshot exposes the
        // sheet subtree, and its element tokens resolve against the parent.
        let pid = window
            .get("pid")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let siblings = self.list_windows(Some(pid)).await.unwrap_or_default();
        let mut alternates: Vec<&Value> = siblings
            .iter()
            .filter(|w| {
                w.get("window_id").and_then(Value::as_i64)
                    != window.get("window_id").and_then(Value::as_i64)
            })
            .filter(|w| w.get("minimized").and_then(Value::as_bool) != Some(true))
            .filter(|w| w.get("is_on_screen").and_then(Value::as_bool) != Some(false))
            .collect();
        alternates.sort_by_key(|w| {
            std::cmp::Reverse(w.get("z_index").and_then(Value::as_i64).unwrap_or(0))
        });
        let mut resolvable = Vec::new();
        for candidate in alternates {
            let retry = self
                .window_state_inner(
                    candidate,
                    semantic,
                    screenshot,
                    max_elements,
                    max_dimension,
                    screenshot_dir,
                    query,
                )
                .await;
            let Ok(state) = retry else { continue };
            if Self::is_ax_unresolved_state(&state) {
                continue;
            }
            resolvable.push(state);
        }
        let Some(mut state) = resolvable
            .iter()
            .position(Self::has_sheet_or_dialog)
            .map(|index| resolvable.remove(index))
            .or_else(|| (!resolvable.is_empty()).then(|| resolvable.remove(0)))
        else {
            return primary;
        };
        let requested_window_id = window.get("window_id").and_then(Value::as_i64);
        if let Some(requested_window_id) = requested_window_id {
            self.sheet_owners
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert((pid, requested_window_id), state.window_id);
        }
        let note = if Self::has_sheet_or_dialog(&state) {
            format!(
                "sheet/dialog window {} unresolved on the CG↔AX map; showing its owning window {} ({}) — sheet elements are inside this tree — address actions with windowId {}",
                window.get("window_id").and_then(Value::as_i64).unwrap_or(0),
                state.window_id,
                state.window_title.as_deref().unwrap_or("untitled"),
                state.window_id
            )
        } else {
            format!(
                "sheet/dialog window {} unresolved on the CG↔AX map; showing on-screen sibling window {} ({}) — could not confirm the owning window: no sheet subtree found in it; re-observe the app's other windows — address actions with windowId {}",
                window.get("window_id").and_then(Value::as_i64).unwrap_or(0),
                state.window_id,
                state.window_title.as_deref().unwrap_or("untitled"),
                state.window_id
            )
        };
        state.note = Some(match state.note {
            Some(existing) => format!("{existing}; {note}"),
            None => note,
        });
        Ok(state)
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

pub(crate) fn pid_from_launch_result(result: &Value) -> Option<i64> {
    result
        .get("structuredContent")
        .and_then(|content| content.get("pid"))
        .and_then(Value::as_i64)
        .or_else(|| {
            result_text(result).and_then(|text| {
                text.match_indices("pid ").find_map(|(index, _)| {
                    let rest = text.get(index + "pid ".len()..)?;
                    let digits: String = rest
                        .chars()
                        .take_while(|character| character.is_ascii_digit())
                        .collect();
                    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
                })
            })
        })
}

fn render_launch_summary(summary: &str, pid: i64, windows: &[Value]) -> String {
    let prefix = summary
        .split_once("\n\nWindows:")
        .map(|(prefix, _)| prefix)
        .unwrap_or(summary)
        .trim_end();
    let mut rendered = format!("{prefix}\n\nWindows:\n");
    if windows.is_empty() {
        rendered.push_str("- (no windows)\n");
    } else {
        for window in windows {
            let title = window
                .get("title")
                .and_then(Value::as_str)
                .filter(|title| !title.is_empty())
                .map(|title| format!("\"{title}\""))
                .unwrap_or_else(|| "(no title)".into());
            let window_id = window
                .get("window_id")
                .and_then(Value::as_i64)
                .map(|id| id.to_string())
                .unwrap_or_else(|| "?".into());
            rendered.push_str(&format!("- {title} [window_id: {window_id}]\n"));
        }
    }
    rendered.push_str(&format!(
        "→ Call get_window_state(pid: {pid}, window_id) to inspect."
    ));
    rendered
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

/// `list_apps` result → the apps array (structuredContent first, then a JSON
/// parse of the text block).
fn result_apps(result: &Value) -> Vec<Value> {
    if let Some(apps) = result
        .get("structuredContent")
        .and_then(|s| s.get("apps"))
        .and_then(Value::as_array)
    {
        return apps.clone();
    }
    result_text(result)
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| {
            v.get("apps")
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
    fn is_alive(&self) -> bool {
        self.client.is_alive()
    }

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
        self.window_state_retargeted(
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
        let verify_args = json!({ "pid": pid, "window_id": window_id, "expect": expect });
        let result = match self.call("verify_state", verify_args.clone()).await {
            Err(error) if Self::is_ax_window_unresolved(&error) => {
                // Sheets/dialogs: the caller pinned the CG window id of a
                // sheet; its AX content lives under the owning window.
                self.retarget_call("verify_state", &verify_args, error)
                    .await?
            }
            other => other?,
        };
        Ok(result
            .get("structuredContent")
            .cloned()
            .or_else(|| result_text(&result).and_then(|t| serde_json::from_str(&t).ok()))
            .unwrap_or(result))
    }

    async fn inspect(&self, kind: &InspectKind) -> Result<Value, ComputerError> {
        let (tool, args) = match kind {
            InspectKind::Apps => ("list_apps", json!({})),
            InspectKind::Windows {
                pid,
                on_screen_only,
            } => (
                "list_windows",
                json!({
                    "pid": pid,
                    "on_screen_only": on_screen_only,
                }),
            ),
        };
        let mut args = args;
        strip_nulls(&mut args);
        let result = self.call(tool, args).await?;
        Ok(result
            .get("structuredContent")
            .cloned()
            .or_else(|| result_text(&result).and_then(|t| serde_json::from_str(&t).ok()))
            .unwrap_or(result))
    }

    async fn act(&self, request: &ActionRequest) -> Result<ActionOutcome, ComputerError> {
        self.act_inner(request).await
    }

    async fn health_report(&self) -> Option<Value> {
        self.health_report().await
    }

    fn session_scoped(&self, label: &str) -> Option<Arc<dyn ComputerBackend>> {
        Some(Arc::new(self.session_scoped(label)))
    }

    async fn session_tool(&self, tool: &str, args: &Value) -> Result<Value, ComputerError> {
        // Recording is session-scoped driver-side, so `self.session` (this
        // backend's label) routes it to the right agent's trajectory.
        let (driver_tool, mut call_args) = match tool {
            "start" => ("start_recording", args.clone()),
            "stop" => ("stop_recording", args.clone()),
            "status" => ("get_recording_state", args.clone()),
            other => {
                return Err(ComputerError::InvalidArgs(format!(
                    "unknown session tool '{other}'"
                )));
            }
        };
        strip_nulls(&mut call_args);
        let result = self.call(driver_tool, call_args).await?;
        Ok(result
            .get("structuredContent")
            .cloned()
            .or_else(|| result_text(&result).and_then(|t| serde_json::from_str(&t).ok()))
            .or_else(|| result_text(&result).map(Value::String))
            .unwrap_or(result))
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
