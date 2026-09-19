//! `MockComputer` — a scriptable in-process backend for tests and for the
//! `COSMOS_COMPUTER_MOCK=1` dev escape hatch (exercises the full service →
//! endpoint → harness path without a driver install).
//!
//! Behavior knobs come from env so an e2e shell test can flip them:
//! - `COSMOS_COMPUTER_MOCK`           enable the mock (`1`, `observe`, `act`,
//!                                   `error` → backend fails every call)
//! - `COSMOS_COMPUTER_MOCK_SCREENSHOT`  when `1`, observe returns a 1×1 PNG

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde_json::{Value, json};

use crate::backend::{
    ActionOutcome, ActionRequest, BackendDescriptor, ComputerBackend, ElementFrame, InspectKind,
    ObserveRequest, ObserveScope, WindowElement, WindowState,
};
use crate::{ComputerError, Observation, ObservedWindow, StagedScreenshot};

/// A tiny valid PNG (1×1 transparent) — enough for attachment staging paths.
const PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

pub struct MockComputer {
    mode: MockMode,
    alive: AtomicBool,
    pub calls: AtomicUsize,
    /// Last action tool the mock saw (asserted in service tests).
    pub last_action: Mutex<Option<String>>,
    pub last_delivery: Mutex<Option<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockMode {
    /// Everything works.
    Full,
    /// Acts fail `refused`; observes work.
    ObserveOnly,
    /// Every call fails `driver_unavailable`.
    Broken,
}

impl MockComputer {
    /// Build the mock from env (`COSMOS_COMPUTER_MOCK`). Returns None when the
    /// knob is unset — `Some` covers every non-empty value.
    pub fn from_env() -> Option<Self> {
        let value = std::env::var("COSMOS_COMPUTER_MOCK").ok()?;
        let mode = match value.as_str() {
            "observe" => MockMode::ObserveOnly,
            "error" | "broken" => MockMode::Broken,
            _ => MockMode::Full,
        };
        Some(Self::with_mode(mode))
    }

    pub fn with_mode(mode: MockMode) -> Self {
        Self {
            mode,
            alive: AtomicBool::new(true),
            calls: AtomicUsize::new(0),
            last_action: Mutex::new(None),
            last_delivery: Mutex::new(None),
        }
    }

    /// A working mock for unit tests (no env).
    pub fn working() -> Self {
        Self::with_mode(MockMode::Full)
    }

    pub fn set_alive(&self, alive: bool) {
        self.alive.store(alive, Ordering::Release);
    }
}

#[async_trait::async_trait]
impl ComputerBackend for MockComputer {
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    async fn ping(&self) -> Result<(), ComputerError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        match self.mode {
            MockMode::Broken => Err(ComputerError::Unavailable("mock driver is down".into())),
            _ => Ok(()),
        }
    }

    async fn observe(&self, request: &ObserveRequest) -> Result<Observation, ComputerError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.mode == MockMode::Broken {
            return Err(ComputerError::Unavailable("mock driver is down".into()));
        }
        let (scope, window) = match &request.scope {
            ObserveScope::Frontmost => (
                "window",
                Some(ObservedWindow {
                    app_name: Some("Mock App".into()),
                    window_title: Some("Mock Window".into()),
                    bundle_id: Some("dev.mock.app".into()),
                    pid: Some(4242),
                    window_id: Some(9001),
                }),
            ),
            ObserveScope::Window { pid, window_id } => (
                "window",
                Some(ObservedWindow {
                    app_name: Some("Mock App".into()),
                    window_title: Some("Mock Window".into()),
                    bundle_id: None,
                    pid: Some(*pid),
                    window_id: *window_id,
                }),
            ),
            ObserveScope::Desktop => ("desktop", None),
        };
        let text = if request.semantic {
            match request.scope {
                ObserveScope::Desktop => {
                    "windows:\n- app: Mock App\n  title: Mock Window\n  pid: 4242".to_string()
                }
                _ => {
                    let pid = match &request.scope {
                        ObserveScope::Window { pid, .. } => *pid,
                        _ => 4242,
                    };
                    let state = mock_state(pid, None, None);
                    if request.full_tree {
                        "- window \"Mock Window\" [Mock App]\n  - button \"OK\"\n  - text \"Hello from the mock driver\"".to_string()
                    } else {
                        crate::digest::render(&state)
                    }
                }
            }
        } else {
            String::new()
        };
        let screenshot = if request.screenshot {
            let path = request
                .screenshot_dir
                .join(format!("computer-mock-{}.png", uuid::Uuid::new_v4()));
            if std::env::var("COSMOS_COMPUTER_MOCK_SCREENSHOT").as_deref() == Ok("1") {
                std::fs::create_dir_all(&request.screenshot_dir).ok();
                std::fs::write(&path, PIXEL_PNG).ok();
            }
            path.is_file().then(|| StagedScreenshot {
                path,
                width: Some(1),
                height: Some(1),
                bytes: PIXEL_PNG.len() as u64,
            })
        } else {
            None
        };
        let modality = match (&screenshot, request.semantic) {
            (Some(_), true) => "both",
            (Some(_), false) => "visual",
            (None, _) => "semantic",
        }
        .to_string();
        Ok(Observation {
            scope: scope.into(),
            window,
            text,
            truncated: false,
            screenshot,
            modality,
        })
    }

    async fn window_state(
        &self,
        pid: i64,
        window_id: Option<i64>,
        semantic: bool,
        screenshot: bool,
        _max_elements: u32,
        _max_dimension: u32,
        screenshot_dir: &std::path::Path,
        query: Option<&str>,
    ) -> Result<WindowState, ComputerError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.mode == MockMode::Broken {
            return Err(ComputerError::Unavailable("mock driver is down".into()));
        }
        let mut state = mock_state(pid, window_id, query);
        if !semantic {
            state.elements.clear();
            state.returned_element_count = 0;
            state.total_element_count = 0;
        }
        let shot = if screenshot {
            let path = screenshot_dir.join(format!("computer-mock-{}.png", uuid::Uuid::new_v4()));
            if std::env::var("COSMOS_COMPUTER_MOCK_SCREENSHOT").as_deref() == Ok("1") {
                std::fs::create_dir_all(screenshot_dir).ok();
                std::fs::write(&path, PIXEL_PNG).ok();
            }
            path.is_file().then(|| StagedScreenshot {
                path,
                width: Some(1),
                height: Some(1),
                bytes: PIXEL_PNG.len() as u64,
            })
        } else {
            None
        };
        state.screenshot = shot;
        Ok(state)
    }

    async fn verify(
        &self,
        _pid: i64,
        _window_id: Option<i64>,
        expect: serde_json::Value,
    ) -> Result<serde_json::Value, ComputerError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.mode == MockMode::Broken {
            return Err(ComputerError::Unavailable("mock driver is down".into()));
        }
        // Real verify_state shape: status + per-predicate results.
        let predicates = expect.as_array().cloned().unwrap_or_else(|| vec![expect]);
        let results: Vec<Value> = predicates
            .iter()
            .enumerate()
            .map(|(i, p)| {
                json!({
                    "index": i,
                    "status": "satisfied",
                    "unknown_reason": null,
                    "observed_json": p,
                })
            })
            .collect();
        Ok(json!({
            "status": "satisfied",
            "stable": true,
            "elapsed_ms": 0,
            "samples": 1,
            "predicates": results,
        }))
    }

    async fn inspect(&self, kind: &InspectKind) -> Result<serde_json::Value, ComputerError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.mode == MockMode::Broken {
            return Err(ComputerError::Unavailable("mock driver is down".into()));
        }
        Ok(match kind {
            InspectKind::Apps => json!({
                "apps": [{"name": "Mock App", "pid": 4242, "running": true, "active": true, "kind": "desktop", "launch_path": "/mock/app"}],
            }),
            InspectKind::Windows { pid, .. } => {
                let pid = pid.unwrap_or(4242);
                json!({
                    "windows": [{"window_id": 9001, "pid": pid, "app_name": "Mock App", "title": "Mock Window", "bounds": {"x": 100, "y": 100, "width": 800, "height": 600}, "z_index": 10, "is_on_screen": true, "minimized": false}],
                })
            }
        })
    }

    async fn act(&self, request: &ActionRequest) -> Result<ActionOutcome, ComputerError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        *self.last_action.lock().unwrap() = Some(request.tool.clone());
        *self.last_delivery.lock().unwrap() = Some(request.delivery.as_str().into());
        match self.mode {
            MockMode::Broken => Err(ComputerError::Unavailable("mock driver is down".into())),
            MockMode::ObserveOnly => Err(ComputerError::refused(
                "observe_only",
                "the mock driver only observes",
            )),
            MockMode::Full => Ok(ActionOutcome {
                summary: format!(
                    "mock {} performed ({})",
                    request.tool,
                    request.delivery.as_str()
                ),
                details: Some(json!({"mock": true, "tool": request.tool})),
                observe: None,
            }),
        }
    }

    fn describe(&self) -> BackendDescriptor {
        BackendDescriptor {
            kind: "mock",
            version: Some("mock-1".into()),
            platform: crate::platform_label(),
            semantic: true,
            screenshots: true,
            background_control: true,
            foreground_control: true,
        }
    }
}

/// The mock element set — a window with a button, a focused edit, a text,
/// and a scrollable list (enough surface for digest/find/wait tests).
fn mock_elements() -> Vec<WindowElement> {
    vec![
        WindowElement {
            index: 0,
            token: Some("s00000001:0".into()),
            role: "Window".into(),
            label: Some("Mock Window".into()),
            enabled: true,
            frame: Some(ElementFrame {
                x: 0.0,
                y: 0.0,
                w: 800.0,
                h: 600.0,
            }),
            ..Default::default()
        },
        WindowElement {
            index: 1,
            token: Some("s00000001:1".into()),
            role: "Button".into(),
            label: Some("OK".into()),
            enabled: true,
            frame: Some(ElementFrame {
                x: 100.0,
                y: 500.0,
                w: 80.0,
                h: 30.0,
            }),
            depth: 1,
            actions: vec!["invoke".into()],
            parent_index: Some(0),
            ..Default::default()
        },
        WindowElement {
            index: 2,
            token: Some("s00000001:2".into()),
            role: "Edit".into(),
            label: Some("Search".into()),
            value: Some(String::new()),
            enabled: true,
            frame: Some(ElementFrame {
                x: 100.0,
                y: 40.0,
                w: 400.0,
                h: 28.0,
            }),
            depth: 1,
            actions: vec!["set_value".into(), "text".into()],
            focused: Some(true),
            parent_index: Some(0),
            ..Default::default()
        },
        WindowElement {
            index: 3,
            token: Some("s00000001:3".into()),
            role: "Text".into(),
            label: Some("Hello from the mock driver".into()),
            enabled: true,
            depth: 1,
            parent_index: Some(0),
            ..Default::default()
        },
        WindowElement {
            index: 4,
            token: Some("s00000001:4".into()),
            role: "List".into(),
            label: Some("Results".into()),
            enabled: true,
            frame: Some(ElementFrame {
                x: 100.0,
                y: 120.0,
                w: 400.0,
                h: 300.0,
            }),
            depth: 1,
            scrollable: Some(true),
            parent_index: Some(0),
            ..Default::default()
        },
        WindowElement {
            index: 5,
            token: Some("s00000001:5".into()),
            role: "MenuItem".into(),
            label: Some("Bluetooth".into()),
            enabled: true,
            depth: 2,
            actions: vec!["invoke".into()],
            parent_index: Some(4),
            ..Default::default()
        },
    ]
}

/// A WindowState over the mock elements. `query` mimics the driver's
/// projection — matching elements plus their ancestors, original indices
/// preserved (the mock tree is shallow so ancestors collapse to the root).
fn mock_state(pid: i64, window_id: Option<i64>, query: Option<&str>) -> WindowState {
    let elements = mock_elements();
    let (elements, total) = match query {
        Some(q) => {
            let needle = q.to_lowercase();
            let mut keep: Vec<bool> = elements
                .iter()
                .map(|e| {
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
                .collect();
            // Ancestors of matches stay (like the driver's projection).
            for i in 0..elements.len() {
                if keep[i] {
                    let mut p = elements[i].parent_index;
                    while let Some(pi) = p {
                        if let Some(pos) = elements.iter().position(|e| e.index == pi) {
                            keep[pos] = true;
                            p = elements[pos].parent_index;
                        } else {
                            break;
                        }
                    }
                }
            }
            let total = elements.len();
            (
                elements
                    .into_iter()
                    .zip(keep)
                    .filter_map(|(e, k)| k.then_some(e))
                    .collect::<Vec<_>>(),
                total,
            )
        }
        None => {
            let total = elements.len();
            (elements, total)
        }
    };
    WindowState {
        snapshot_id: "s00000001".into(),
        pid,
        window_id: window_id.unwrap_or(9001),
        app_name: Some("Mock App".into()),
        window_title: Some("Mock Window".into()),
        window_bounds: Some(ElementFrame {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        }),
        returned_element_count: elements.len() as u32,
        total_element_count: total as u32,
        elements,
        ..Default::default()
    }
}
