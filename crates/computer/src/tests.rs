//! Crate tests — policy gating runs against `MockComputer`, the endpoint
//! test exercises the real loopback HTTP path.

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::mock::MockComputer;
use crate::service::ComputerService;
use crate::{ComputerError, ComputerSettings, ControlMode, save_settings};

fn enabled_settings(control: ControlMode) -> ComputerSettings {
    ComputerSettings {
        enabled: true,
        screenshots: true,
        control,
        ..ComputerSettings::default()
    }
}

/// A service with saved settings and an injected mock backend — exercises
/// the real dispatch/policy path without a driver install.
fn service(
    settings: ComputerSettings,
    backend: Arc<MockComputer>,
) -> (ComputerService, tempfile::TempDir) {
    let data = tempfile::tempdir().unwrap();
    let shots = tempfile::tempdir().unwrap();
    save_settings(data.path(), &settings).unwrap();
    let service = ComputerService::with_backend(data.path().into(), shots.path().into(), backend);
    (service, data)
}

#[test]
fn error_codes_are_stable() {
    assert_eq!(ComputerError::Disabled.code(), "disabled");
    assert_eq!(
        ComputerError::NotInstalled.code(),
        "component_not_installed"
    );
    assert_eq!(
        ComputerError::UnsupportedPlatform.code(),
        "unsupported_platform"
    );
    // The driver's own refusal vocabulary passes through verbatim.
    let refused =
        ComputerError::from_tool_error("error=background_unavailable: window is minimized");
    assert_eq!(refused.code(), "background_unavailable");
    let stale = ComputerError::from_tool_error("stale_element: token no longer resolves");
    assert_eq!(stale.code(), "stale_element");
    let perm = ComputerError::from_tool_error("screen_recording_denied: grant in System Settings");
    assert_eq!(perm.code(), "permission_required");
    assert!(matches!(perm, ComputerError::Permission(_)));
}

#[test]
fn settings_round_trip() {
    let data = tempfile::tempdir().unwrap();
    let settings = ComputerSettings {
        enabled: true,
        semantic: false,
        screenshots: true,
        control: ControlMode::FullControl,
        telemetry: true,
        inspect_max_elements: 42,
        screenshot_max_dimension: 800,
    };
    save_settings(data.path(), &settings).unwrap();
    assert_eq!(crate::load_settings(data.path()), settings);
    // A corrupt file falls back to defaults instead of failing the app.
    std::fs::write(crate::settings_path(data.path()), b"{not json").unwrap();
    assert_eq!(
        crate::load_settings(data.path()),
        ComputerSettings::default()
    );
}

#[tokio::test]
async fn disabled_service_refuses() {
    let (service, _data) = service(
        ComputerSettings::default(),
        Arc::new(MockComputer::working()),
    );
    let err = service
        .dispatch_tool("computer_observe", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err, ComputerError::Disabled);
    // Status still answers — it's how a harness learns the computer exists.
    let reply = service
        .dispatch_tool("computer_status", json!({}))
        .await
        .unwrap();
    assert!(reply.text.contains("\"state\": \"disabled\""));
}

#[tokio::test]
async fn observe_mode_blocks_actions() {
    let (service, _data) = service(
        enabled_settings(ControlMode::Observe),
        Arc::new(MockComputer::working()),
    );
    // Observing works.
    let reply = service
        .dispatch_tool("computer_observe", json!({}))
        .await
        .unwrap();
    assert!(reply.text.contains("Mock App"));
    // Acting is refused with the policy code, before the backend sees it.
    let err = service
        .dispatch_tool("computer_click", json!({"pid": 4242, "x": 5, "y": 5}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "control_mode");
}

#[tokio::test]
async fn background_mode_blocks_foreground() {
    let backend = Arc::new(MockComputer::working());
    let (service, _data) = service(enabled_settings(ControlMode::Background), backend.clone());
    let err = service
        .dispatch_tool(
            "computer_click",
            json!({"pid": 4242, "x": 5, "y": 5, "deliveryMode": "foreground"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), "control_mode");
    // Background delivery passes and is what the backend records.
    service
        .dispatch_tool("computer_click", json!({"pid": 4242, "x": 5, "y": 5}))
        .await
        .unwrap();
    assert_eq!(
        backend.last_delivery.lock().unwrap().as_deref(),
        Some("background")
    );
}

#[tokio::test]
async fn full_control_allows_foreground_escalation() {
    let backend = Arc::new(MockComputer::working());
    let (service, _data) = service(enabled_settings(ControlMode::FullControl), backend.clone());
    service
        .dispatch_tool(
            "computer_click",
            json!({"pid": 4242, "x": 5, "y": 5, "deliveryMode": "foreground"}),
        )
        .await
        .unwrap();
    assert_eq!(
        backend.last_delivery.lock().unwrap().as_deref(),
        Some("foreground")
    );
}

#[tokio::test]
async fn observe_window_scope_requires_pid() {
    let (service, _data) = service(
        enabled_settings(ControlMode::Observe),
        Arc::new(MockComputer::working()),
    );
    let err = service
        .dispatch_tool("computer_observe", json!({"scope": "window"}))
        .await
        .unwrap_err();
    assert!(matches!(err, ComputerError::InvalidArgs(_)));
    service
        .dispatch_tool("computer_observe", json!({"scope": "window", "pid": 4242}))
        .await
        .unwrap();
}

#[test]
fn every_action_tool_is_advertised() {
    let tools = crate::surface::tools();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for action in [
        "computer_launch_app",
        "computer_close_app",
        "computer_focus_app",
        "computer_click",
        "computer_right_click",
        "computer_double_click",
        "computer_type_text",
        "computer_set_value",
        "computer_press_key",
        "computer_scroll",
        "computer_drag",
        "computer_move_cursor",
    ] {
        assert!(names.contains(&action), "{action} missing from tools/list");
        assert!(crate::surface::is_action_tool(action), "{action} not gated");
    }
    assert!(!crate::surface::is_action_tool("computer_observe"));
    assert!(!crate::surface::is_action_tool("computer_status"));
}

/// Raw JSON-RPC POST to the endpoint; returns the response body.
async fn post(url: &str, body: &str) -> String {
    let parsed = url::Url::parse(url).unwrap();
    let addr = format!("{}:{}", parsed.host_str().unwrap(), parsed.port().unwrap());
    let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        parsed.path(),
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let raw = String::from_utf8_lossy(&buf).into_owned();
    raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string()
}

#[tokio::test]
async fn endpoint_serves_the_mcp_surface() {
    let (service, _data) = service(
        enabled_settings(ControlMode::Background),
        Arc::new(MockComputer::working()),
    );
    let url = service.endpoint_url().expect("endpoint should bind");
    assert!(url.starts_with("http://127.0.0.1:"));

    let init = post(
        &url,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    )
    .await;
    let init: Value = serde_json::from_str(&init).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "cosmos-computer");

    let list = post(
        &url,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    )
    .await;
    let list: Value = serde_json::from_str(&list).unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"computer_observe"));

    let call = post(
        &url,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"computer_observe","arguments":{}}}"#,
    )
    .await;
    let call: Value = serde_json::from_str(&call).unwrap();
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Mock App"));

    // Wrong token path → 404, and a closed service keeps nothing listening.
    let wrong = url::Url::parse(&url).unwrap();
    let bad = format!("http://127.0.0.1:{}/mcp/deadbeef", wrong.port().unwrap());
    let mut stream = tokio::net::TcpStream::connect(
        bad.trim_start_matches("http://").split('/').next().unwrap(),
    )
    .await
    .unwrap();
    stream
        .write_all(
            b"POST /mcp/deadbeef HTTP/1.1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        )
        .await
        .unwrap();
    stream.shutdown().await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf).starts_with("HTTP/1.1 404"));
}

// ---------------------------------------------------------------------------
// digest — the model-facing compaction
// ---------------------------------------------------------------------------

/// A fixture mirroring a real UIA snapshot: actionable controls buried in
/// Group/Text noise, with state flags and frames.
fn fixture_state() -> crate::backend::WindowState {
    use crate::backend::{ElementFrame, WindowElement, WindowState};
    let el = |index: u32, role: &str, label: Option<&str>, depth: u32| WindowElement {
        index,
        token: Some(format!("s0000000a:{index}")),
        role: role.into(),
        label: label.map(str::to_string),
        depth,
        parent_index: Some(0),
        ..Default::default()
    };
    let mut edit = el(6, "Edit", Some("Address and search bar"), 8);
    edit.value = Some("https://example.com/docs".into());
    edit.focused = Some(true);
    edit.actions = vec!["set_value".into(), "text".into()];
    edit.frame = Some(ElementFrame {
        x: 130.0,
        y: 60.0,
        w: 1469.0,
        h: 30.0,
    });
    let mut save = el(7, "Button", Some("Save"), 8);
    save.actions = vec!["invoke".into()];
    save.frame = Some(ElementFrame {
        x: 700.0,
        y: 500.0,
        w: 90.0,
        h: 32.0,
    });
    let mut list = el(9, "List", Some("Results"), 4);
    list.scrollable = Some(true);
    let mut checked = el(10, "CheckBox", Some("Remember me"), 6);
    checked.checked = Some(true);
    WindowState {
        snapshot_id: "s0000000a".into(),
        pid: 4242,
        window_id: 9001,
        app_name: Some("Mock App".into()),
        window_title: Some("Editor — file.txt".into()),
        window_bounds: Some(ElementFrame {
            x: 0.0,
            y: 0.0,
            w: 1600.0,
            h: 900.0,
        }),
        elements: vec![
            el(0, "Window", Some("Editor"), 0),
            el(1, "Group", None, 1),          // noise — pruned
            el(2, "Text", Some("status"), 2), // label-bearing text survives
            el(3, "Group", None, 2),          // noise
            el(4, "Image", None, 3),          // noise
            el(5, "MenuBar", Some("Application"), 1),
            edit,
            save,
            el(8, "Text", None, 9), // unlabeled text — pruned
            list,
            checked,
        ],
        returned_element_count: 11,
        total_element_count: 11,
        ..Default::default()
    }
}

#[test]
fn digest_compacts_and_landmarks() {
    let state = fixture_state();
    let out = crate::digest::render(&state);
    // Header carries window identity + the handles actions need.
    assert!(out.contains("pid 4242"), "missing pid: {out}");
    assert!(
        out.contains("snapshot s0000000a"),
        "missing snapshot: {out}"
    );
    assert!(out.contains("hwnd 9001"), "missing hwnd: {out}");
    // Landmarks: focused element + scroll containers surfaced up top.
    assert!(out.contains("focused: E6"), "missing focused: {out}");
    assert!(out.contains("scrollable: E9"), "missing scrollable: {out}");
    // Actionable lines keep E-refs, values, flags and geometry.
    assert!(
        out.contains("E6 Edit \"Address and search bar\""),
        "edit line: {out}"
    );
    assert!(
        out.contains("= \"https://example.com/docs\""),
        "edit value: {out}"
    );
    assert!(out.contains("E7 Button \"Save\""), "save line: {out}");
    assert!(out.contains("act:invoke"), "actions: {out}");
    assert!(out.contains("@700,500 90x32"), "frame: {out}");
    assert!(out.contains("chk"), "checked flag: {out}");
    // Noise pruned: unnamed Groups/Images/Text never get lines.
    assert!(!out.contains("E1 Group"), "group leaked: {out}");
    assert!(!out.contains("E4 Image"), "image leaked: {out}");
    assert!(!out.contains("E8 Text"), "unlabeled text leaked: {out}");
    // Compact: an 11-element window stays a page, not a tree dump.
    assert!(out.len() < 2_500, "digest too big: {} bytes", out.len());
}

#[test]
fn digest_find_ranks_and_filters() {
    let state = fixture_state();
    // Exact label match wins and carries the actionable ref.
    let hits = crate::digest::find(&state, "Save", None, 8);
    assert_eq!(hits.first().map(|h| h.index), Some(7));
    assert_eq!(hits.first().unwrap().token.as_deref(), Some("s0000000a:7"));
    // Word-insensitive contains also finds.
    let hits = crate::digest::find(&state, "search", None, 8);
    assert!(hits.iter().any(|h| h.index == 6));
    // Value text is searchable too.
    let hits = crate::digest::find(&state, "example.com", None, 8);
    assert!(hits.iter().any(|h| h.index == 6));
    // Role filtering narrows the set.
    let hits = crate::digest::find(&state, "e", Some("checkbox"), 8);
    assert!(hits.iter().all(|h| h.role == "CheckBox"));
    // No match → empty, not noise.
    assert!(crate::digest::find(&state, "zzzz-nothing", None, 8).is_empty());
}

// ---------------------------------------------------------------------------
// service-level control loop — find / wait / verify / scroll_until / failures
// ---------------------------------------------------------------------------

#[tokio::test]
async fn find_tool_returns_ranked_hits() {
    let (service, _data) = service(
        enabled_settings(ControlMode::Observe),
        Arc::new(MockComputer::working()),
    );
    let reply = service
        .dispatch_tool("computer_find", json!({"pid": 4242, "query": "bluetooth"}))
        .await
        .unwrap();
    assert!(reply.text.contains("Bluetooth"), "{}", reply.text);
    assert!(reply.text.contains('E'), "expected E-refs: {}", reply.text);
    // Find is a read — Observe mode allows it.
    let err = service
        .dispatch_tool("computer_find", json!({"query": "x"}))
        .await
        .unwrap_err();
    assert!(matches!(err, ComputerError::InvalidArgs(_)));
}

#[tokio::test]
async fn verify_tool_returns_driver_status() {
    let (service, _data) = service(
        enabled_settings(ControlMode::Observe),
        Arc::new(MockComputer::working()),
    );
    let reply = service
        .dispatch_tool(
            "computer_verify",
            json!({"pid": 4242, "expect": [{"element": {"selector": {"labelContains": "OK"}, "exists": true}}]}),
        )
        .await
        .unwrap();
    assert!(
        reply.text.contains("\"status\": \"satisfied\""),
        "{}",
        reply.text
    );
}

#[tokio::test]
async fn wait_tool_satisfies_and_times_out() {
    let (service, _data) = service(
        enabled_settings(ControlMode::Observe),
        Arc::new(MockComputer::working()),
    );
    // Element present → satisfied immediately.
    let reply = service
        .dispatch_tool(
            "computer_wait",
            json!({"pid": 4242, "for": {"element": {"labelContains": "OK"}}, "timeoutMs": 2000}),
        )
        .await
        .unwrap();
    assert!(reply.text.contains("satisfied"), "{}", reply.text);
    // Text present in the mock snapshot → satisfied.
    service
        .dispatch_tool(
            "computer_wait",
            json!({"pid": 4242, "for": {"text": "mock driver"}, "timeoutMs": 2000}),
        )
        .await
        .unwrap();
    // Absent text → timeout error, not a hang.
    let err = service
        .dispatch_tool(
            "computer_wait",
            json!({"pid": 4242, "for": {"text": "never-present-zzz"}, "timeoutMs": 700}),
        )
        .await
        .unwrap_err();
    assert!(err.message().contains("timeout"), "{err}");
    // Window condition resolves against the mock's window record.
    let reply = service
        .dispatch_tool(
            "computer_wait",
            json!({"pid": 4242, "for": {"window": true}, "timeoutMs": 2000}),
        )
        .await
        .unwrap();
    assert!(reply.text.contains("satisfied"), "{}", reply.text);
}

#[tokio::test]
async fn scroll_until_finds_without_scrolling() {
    let (service, _data) = service(
        enabled_settings(ControlMode::Background),
        Arc::new(MockComputer::working()),
    );
    // "Bluetooth" exists in the first snapshot → found at step 0, no scroll.
    let reply = service
        .dispatch_tool(
            "computer_scroll_until",
            json!({"pid": 4242, "query": "bluetooth"}),
        )
        .await
        .unwrap();
    assert!(
        reply.text.contains("found after 0 scroll"),
        "{}",
        reply.text
    );
    assert!(reply.text.contains("Bluetooth"), "{}", reply.text);
}

#[tokio::test]
async fn repeated_failures_loop_break() {
    // ObserveOnly mock refuses every act — the service must loop-break after
    // REPEAT_FAIL_CAP identical failures rather than pass a 4th to the driver.
    let backend = Arc::new(MockComputer::with_mode(crate::mock::MockMode::ObserveOnly));
    let (service, _data) = service(enabled_settings(ControlMode::FullControl), backend);
    let args = json!({"pid": 4242, "x": 1, "y": 1});
    for _ in 0..3 {
        let err = service
            .dispatch_tool("computer_click", args.clone())
            .await
            .unwrap_err();
        assert_eq!(err.code(), "observe_only");
    }
    let err = service
        .dispatch_tool("computer_click", args)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "repeated_failure");
    // A DIFFERENT call is not loop-broken.
    let err = service
        .dispatch_tool("computer_click", json!({"pid": 4242, "x": 9, "y": 9}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "observe_only");
}

#[tokio::test]
async fn focus_app_needs_full_control() {
    let (service, _data) = service(
        enabled_settings(ControlMode::Background),
        Arc::new(MockComputer::working()),
    );
    let err = service
        .dispatch_tool("computer_focus_app", json!({"pid": 4242}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "control_mode");
}

#[test]
fn session_ended_detection() {
    // The driver's dead-session vocabulary revives via start_session.
    assert!(crate::cua::CuaBackend::is_session_ended(
        &ComputerError::refused(
            "refused",
            "this session has ended; call start_session explicitly",
        )
    ));
    assert!(crate::cua::CuaBackend::is_session_ended(
        &ComputerError::refused("session_ended", "expired",)
    ));
    assert!(!crate::cua::CuaBackend::is_session_ended(
        &ComputerError::refused("stale_element", "token stale",)
    ));
    assert!(!crate::cua::CuaBackend::is_session_ended(
        &ComputerError::Unavailable("down".into())
    ));
}

#[test]
fn new_tools_are_advertised_and_gated() {
    let tools = crate::surface::tools();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for name in [
        "computer_find",
        "computer_wait",
        "computer_verify",
        "computer_scroll_until",
        "computer_invoke_menu",
        "computer_set_window_frame",
        "computer_clipboard_read",
        "computer_clipboard_write",
        "computer_hotkey",
    ] {
        assert!(names.contains(&name), "{name} missing from tools/list");
    }
    // Actions gated; reads not.
    for action in [
        "computer_scroll_until",
        "computer_invoke_menu",
        "computer_set_window_frame",
        "computer_clipboard_write",
        "computer_hotkey",
        "computer_focus_app",
    ] {
        assert!(crate::surface::is_action_tool(action), "{action} not gated");
    }
    for read in [
        "computer_find",
        "computer_wait",
        "computer_verify",
        "computer_clipboard_read",
    ] {
        assert!(
            !crate::surface::is_action_tool(read),
            "{read} wrongly gated"
        );
    }
    // Persistent-foreground tools need Full Control specifically.
    assert!(crate::surface::requires_full_control("computer_focus_app"));
    assert!(!crate::surface::requires_full_control("computer_click"));
}

/// Real-driver end-to-end - runs only with COSMOS_COMPUTER_E2E=1 and a
/// driver installed under the real data dir (%LOCALAPPDATA%\Zeron on
/// Windows). Exercises spawn + start_session, list_windows, observe,
/// find, verify, and the overlay cursor against the shipped cua-driver.
#[tokio::test]
async fn e2e_real_driver() {
    if std::env::var("COSMOS_COMPUTER_E2E").as_deref() != Ok("1") {
        return;
    }
    let data = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("Zeron");
    let paths = crate::ComputerPaths::new(&data);
    let Some(install) = paths.installed().unwrap() else {
        eprintln!(
            "e2e: no installed driver under {} - skipping",
            data.display()
        );
        return;
    };
    let settings = enabled_settings(ControlMode::Background);
    let backend = crate::cua::CuaBackend::spawn(&paths, &settings, &install)
        .await
        .expect("driver spawn");
    let temp = tempfile::tempdir().unwrap();
    let shots = tempfile::tempdir().unwrap();
    save_settings(temp.path(), &settings).unwrap();
    let service =
        ComputerService::with_backend(temp.path().into(), shots.path().into(), Arc::new(backend));

    let started = std::time::Instant::now();
    let status = service
        .dispatch_tool("computer_status", json!({}))
        .await
        .unwrap();
    eprintln!(
        "e2e status ({}ms): {}",
        started.elapsed().as_millis(),
        status.text
    );

    let started = std::time::Instant::now();
    let windows = service
        .dispatch_tool("computer_list_windows", json!({}))
        .await
        .expect("list_windows");
    eprintln!(
        "e2e windows ({}ms): {}",
        started.elapsed().as_millis(),
        windows.text
    );

    // Pick the frontmost window of the shell process as the observe target
    // when present; otherwise fall back to the desktop scope.
    let started = std::time::Instant::now();
    let observe = service
        .dispatch_tool("computer_observe", json!({"scope": "frontmost"}))
        .await
        .expect("observe");
    eprintln!(
        "e2e observe ({}ms): {} chars",
        started.elapsed().as_millis(),
        observe.text.len()
    );
    assert!(!observe.text.trim().is_empty(), "empty digest");

    // Find + verify round-trip on a real on-screen window (skip the
    // driver's own cursor-overlay windows).
    let parsed: Value = serde_json::from_str(&windows.text).unwrap();
    let list = parsed
        .get("windows")
        .or_else(|| parsed.get("items"))
        .cloned()
        .unwrap_or_else(|| parsed.clone());
    let target = list
        .as_array()
        .and_then(|list| {
            list.iter().find(|w| {
                w.get("is_on_screen").and_then(Value::as_bool) == Some(true)
                    && w.get("app_name").and_then(Value::as_str) != Some("Cua.AgentCursorOverlay")
            })
        })
        .cloned();
    if let Some(target) = target {
        let pid = target["pid"].as_i64().unwrap();
        let window_id = target["window_id"].as_i64().unwrap();
        let app = target["app_name"].as_str().unwrap_or("?").to_string();
        let started = std::time::Instant::now();
        let found = service
            .dispatch_tool(
                "computer_find",
                json!({"pid": pid, "windowId": window_id, "query": "a"}),
            )
            .await;
        eprintln!(
            "e2e find on {app} ({}ms): {:?}",
            started.elapsed().as_millis(),
            found
                .as_ref()
                .map(|r| r.text.chars().take(200).collect::<String>())
        );
        let verified = service
            .dispatch_tool(
                "computer_verify",
                json!({"pid": pid, "windowId": window_id, "expect": [{"window": {"exists": true}}]}),
            )
            .await;
        eprintln!("e2e verify: {:?}", verified.map(|r| r.text));
        // Overlay cursor is safe to wiggle — never touches the physical one.
        let moved = service
            .dispatch_tool("computer_move_cursor", json!({"x": 40, "y": 40}))
            .await;
        eprintln!("e2e move_cursor: {:?}", moved.map(|r| r.text));
    }
}
