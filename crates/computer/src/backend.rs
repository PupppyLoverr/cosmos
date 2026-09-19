//! The pluggable perception/control backend. `CuaBackend` is the production
//! implementation over `cua-driver mcp`; `MockComputer` drives tests and the
//! `COSMOS_COMPUTER_MOCK=1` escape hatch. The service, MCP surface and RPC
//! layer only ever see this trait — backend details never leak upward.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{ComputerError, Observation};

/// How a backend reports its own reach. Surfaced in `computer_status` so a
/// harness can adapt (e.g. skip pixel actions when screenshots are off).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendDescriptor {
    /// `cua-driver` | `mock`
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub platform: &'static str,
    pub semantic: bool,
    pub screenshots: bool,
    /// Background input delivery exists at all (OS/session may still refuse
    /// per action — refusals stay structured).
    pub background_control: bool,
    /// Foreground escalation exists (the service's control-mode gate decides
    /// whether a harness may use it).
    pub foreground_control: bool,
}

/// What one `computer_observe` call should return.
#[derive(Debug, Clone)]
pub struct ObserveRequest {
    /// `"frontmost"` (the top window that is not Cosmos or the driver — the
    /// "app you were just using"), `"window"` (explicit pid/window), or
    /// `"desktop"` (whole-screen window/app inventory).
    pub scope: ObserveScope,
    /// Include semantic (accessibility) context.
    pub semantic: bool,
    /// Include a screenshot PNG (settings already gate this).
    pub screenshot: bool,
    /// Element-walk bound for the semantic tree.
    pub max_elements: u32,
    /// `format:"full"` — return the driver's raw `tree_markdown` instead of
    /// the compact digest (diagnostics/deep dives only; it can be 30KB+).
    pub full_tree: bool,
    /// Screenshot long-edge cap in pixels.
    pub max_dimension: u32,
    /// Directory the backend may write `screenshot-*.png` into (the service
    /// points this inside the uploads jail so remote viewers can read the
    /// file through the attachment-chunk RPC).
    pub screenshot_dir: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub enum ObserveScope {
    /// The frontmost window not owned by Cosmos or the driver.
    #[default]
    Frontmost,
    /// An explicit target. `window_id` defaults to the pid's frontmost window.
    Window { pid: i64, window_id: Option<i64> },
    /// The whole desktop (app/window inventory; screenshot if asked).
    Desktop,
}

/// A single interactive element from a window snapshot — the normalized form
/// of the driver's `structuredContent.elements[]` entries. `index` is the
/// per-snapshot `element_index` (`E{n}` in digests); `token` is the driver's
/// snapshot-bound `element_token` — the robust action handle.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowElement {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub enabled: bool,
    /// Element frame in screen pixels as reported by the driver (`None` for
    /// offscreen/virtual); frames are window-local only when the driver's
    /// window root frame is at the origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<ElementFrame>,
    pub depth: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index: Option<u32>,
    // -- state flags the driver reports (all optional — absent = unknown) --
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focused: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrollable: Option<bool>,
    /// Inside web content (browser DOM projection) — less trustworthy for
    /// pixel targeting; background delivery often unavailable there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_web_content: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementFrame {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A full window snapshot — the driver's `get_window_state` normalized into
/// typed data. Element handles (`index`/`token`) are ONLY valid against
/// `snapshot_id` and become stale on the next snapshot of the same window.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub snapshot_id: String,
    pub pid: i64,
    pub window_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_bounds: Option<ElementFrame>,
    #[serde(default)]
    pub elements: Vec<WindowElement>,
    /// The driver's own markdown rendering — kept for `full` observations and
    /// diagnostics; the compact digest is derived from `elements`.
    #[serde(skip_serializing)]
    pub tree_markdown: Option<String>,
    /// Total nodes walked vs returned (driver-side truncation signal).
    pub returned_element_count: u32,
    pub total_element_count: u32,
    /// Driver-reported caveat worth surfacing to the model — degradation
    /// (`degraded_reason`), Chromium compositing gaps (`capture_coverage`),
    /// truncation reasons. Rendered as a `note:` line in digests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Staged window screenshot when one was captured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<crate::StagedScreenshot>,
}

/// One `verify_state` outcome as the driver reports it — predicates ANDed,
/// each landing `satisfied | unsatisfied | unknown` (`unknown` never means
/// success; `unknown_reason` says why). Kept as JSON — the predicate
/// vocabulary is the driver's and evolves with it.
pub type VerifyOutcome = serde_json::Value;

/// How a backend reports its own reach. Surfaced in `computer_status` so a
/// harness can adapt (e.g. skip pixel actions when screenshots are off).
#[derive(Debug, Clone)]
pub struct ActionRequest {
    /// Cosmos tool name (`computer_click`, `computer_type_text`, …).
    pub tool: String,
    /// Raw `arguments` object from the tool call.
    pub args: serde_json::Value,
    /// Resolved delivery: `background` always unless the caller explicitly
    /// escalated and the service's Full-Control gate allowed `foreground`.
    pub delivery: Delivery,
    /// Optional session label forwarded to the driver for multi-call work.
    pub session: Option<String>,
    /// Directory for action screenshots (post-action observation).
    pub screenshot_dir: PathBuf,
    /// Whether the post-action observation may include a screenshot.
    pub observe_screenshot: bool,
    /// Whether the post-action observation may include semantic text.
    pub observe_semantic: bool,
    /// Observe bounds for the post-action check.
    pub max_elements: u32,
    pub max_dimension: u32,
}

/// Lightweight inventory reads (the `computer_list_*` tools).
#[derive(Debug, Clone)]
pub enum InspectKind {
    /// `list_apps` — running + installed applications.
    Apps,
    /// `list_windows` — top-level windows (optional pid/on-screen filters).
    Windows {
        pid: Option<i64>,
        on_screen_only: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Delivery {
    Background,
    Foreground,
}

impl Delivery {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Foreground => "foreground",
        }
    }
}

/// What an action produced: the driver's own result text plus an optional
/// post-action observation ("did it work" — semantic, cheap).
#[derive(Debug, Clone, Default)]
pub struct ActionOutcome {
    /// Human/model-readable summary of what the driver did.
    pub summary: String,
    /// Driver-structured payload worth surfacing (launched pid, window
    /// handles, previous-foreground handles for restore…). Opaque to the
    /// service; passed through to the caller.
    pub details: Option<serde_json::Value>,
    /// Post-action observation when the tool contract includes one.
    pub observe: Option<Observation>,
}

#[async_trait::async_trait]
pub trait ComputerBackend: Send + Sync {
    /// Cheap liveness probe (e.g. `tools/list`).
    async fn ping(&self) -> Result<(), ComputerError>;

    /// Semantic/visual observation per the request's modality flags.
    async fn observe(&self, request: &ObserveRequest) -> Result<Observation, ComputerError>;

    /// A structured window snapshot — elements, snapshot id, window bounds.
    /// The primitive behind `computer_observe` digests, `computer_find`, and
    /// the service's wait/scroll loops. `screenshot_dir` may be empty when no
    /// image is requested. `query` is the driver's case-insensitive substring
    /// projection (matches + ancestors, original indices preserved) — pass
    /// it for `computer_find` so huge trees are filtered driver-side.
    async fn window_state(
        &self,
        pid: i64,
        window_id: Option<i64>,
        semantic: bool,
        screenshot: bool,
        max_elements: u32,
        max_dimension: u32,
        screenshot_dir: &std::path::Path,
        query: Option<&str>,
    ) -> Result<WindowState, ComputerError>;

    /// Deterministic postcondition check (`verify_state`). `expect` is the
    /// driver's predicate array (element selector {role,label_contains} +
    /// exists/enabled/selected/value_equals; window exists/bounds).
    async fn verify(
        &self,
        pid: i64,
        window_id: Option<i64>,
        expect: serde_json::Value,
    ) -> Result<VerifyOutcome, ComputerError>;

    /// Lightweight inventory read for `computer_list_apps`/`computer_list_windows`.
    /// Returns the driver's structured payload verbatim (the service passes
    /// it through unmodified).
    async fn inspect(&self, kind: &InspectKind) -> Result<serde_json::Value, ComputerError>;

    /// Perform an action, honoring `request.delivery` exactly.
    async fn act(&self, request: &ActionRequest) -> Result<ActionOutcome, ComputerError>;

    /// The backend's own diagnostics payload (the driver's `health_report` —
    /// platform checks, permission grants). Backends without one return None.
    async fn health_report(&self) -> Option<serde_json::Value> {
        None
    }

    /// A view of this backend bound to `label`: concurrent agents each get a
    /// distinct driver-side session (own cursor, recording, lifecycle) instead
    /// of all calls sharing the process-wide label. Backends without driver
    /// sessions return None and the caller falls back to the shared backend.
    fn session_scoped(&self, _label: &str) -> Option<std::sync::Arc<dyn ComputerBackend>> {
        None
    }

    /// Session-scoped driver utilities that are neither observe nor UI act:
    /// recording controls today (`computer_recording` → `start_recording` /
    /// `stop_recording` / `get_recording_state`). The result is the driver's
    /// structured payload verbatim. Backends without driver sessions refuse.
    async fn session_tool(
        &self,
        _tool: &str,
        _args: &serde_json::Value,
    ) -> Result<serde_json::Value, ComputerError> {
        Err(ComputerError::Unavailable(
            "backend has no session utilities".into(),
        ))
    }

    fn describe(&self) -> BackendDescriptor;
}
