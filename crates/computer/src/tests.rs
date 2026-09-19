//! Crate tests — policy gating runs against `MockComputer`, the endpoint
//! test exercises the real loopback HTTP path.

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::backend::{ActionRequest, ComputerBackend, Delivery, ObserveRequest, ObserveScope};
use crate::cua::{CuaBackend, pid_from_launch_result};
#[cfg(unix)]
use crate::mcp_client::McpClient;
use crate::mock::MockComputer;
use crate::service::ComputerService;
use crate::{
    ComputerError, ComputerPaths, ComputerSettings, ControlMode, InstallState, save_settings,
};

fn enabled_settings(control: ControlMode) -> ComputerSettings {
    ComputerSettings {
        enabled: true,
        screenshots: true,
        control,
        ..ComputerSettings::default()
    }
}

#[test]
fn pid_from_launch_result_prefers_structured_content() {
    let result = json!({
        "structuredContent": {"pid": 584},
        "content": [{"text": "Launched TextEdit (pid 999) in background."}]
    });
    assert_eq!(pid_from_launch_result(&result), Some(584));
}

#[test]
fn pid_from_launch_result_parses_summary_text() {
    let result = json!({
        "content": [{"text": "Launched TextEdit (pid 584) in background."}]
    });
    assert_eq!(pid_from_launch_result(&result), Some(584));
}

#[cfg(unix)]
async fn scripted_cua_backend(
    windows: Value,
    states: Value,
) -> (CuaBackend, tempfile::TempDir, tempfile::TempDir) {
    scripted_cua_backend_with_actions(windows, states, json!({})).await
}

#[cfg(unix)]
async fn scripted_cua_backend_with_actions(
    windows: Value,
    states: Value,
    actions: Value,
) -> (CuaBackend, tempfile::TempDir, tempfile::TempDir) {
    use std::os::unix::fs::PermissionsExt;

    let data = tempfile::tempdir().unwrap();
    let install_dir = data.path().join("driver");
    std::fs::create_dir_all(&install_dir).unwrap();
    let binary = install_dir.join("cua-driver");
    let windows = serde_json::to_string(&serde_json::to_string(&windows).unwrap()).unwrap();
    let states = serde_json::to_string(&serde_json::to_string(&states).unwrap()).unwrap();
    let actions = serde_json::to_string(&serde_json::to_string(&actions).unwrap()).unwrap();
    let calls = serde_json::to_string(
        &serde_json::to_string(&data.path().join("calls.jsonl").display().to_string()).unwrap(),
    )
    .unwrap();
    let script = format!(
        r#"#!/usr/bin/env python3
import json
import sys

windows = json.loads({windows})
states = json.loads({states})
actions = json.loads({actions})
calls_path = json.loads({calls})
action_indexes = {{}}
state_indexes = {{}}

for line in sys.stdin:
    request = json.loads(line)
    request_id = request.get("id")
    if request_id is None:
        continue
    method = request["method"]
    name = None
    args = {{}}
    if method == "tools/call":
        name = request["params"]["name"]
        args = request["params"].get("arguments", {{}})
    with open(calls_path, "a") as calls_file:
        calls_file.write(json.dumps({{"method": method, "name": name, "args": args}}) + "\n")
        calls_file.flush()
    if method == "initialize":
        result = {{"protocolVersion": "2025-06-18", "capabilities": {{}}, "serverInfo": {{"name": "fake-cua", "version": "test"}}}}
    elif method == "tools/list":
        result = {{"tools": [{{"name": "start_session"}}, {{"name": "get_config"}}, {{"name": "list_windows"}}, {{"name": "get_window_state"}}]}}
    elif method == "tools/call":
        params = request["params"]
        if name == "start_session":
            result = {{}}
        elif name == "get_config":
            result = {{"content": [{{"type": "text", "text": "{{\"version\":\"test\"}}"}}]}}
        elif name == "list_windows":
            result = {{"structuredContent": {{"windows": windows}}}}
        elif name == "get_window_state":
            state = states[str(args["window_id"])]
            if isinstance(state, list):
                index = state_indexes.get(str(args["window_id"]), 0)
                state_indexes[str(args["window_id"])] = index + 1
                state = state[min(index, len(state) - 1)]
            result = {{"structuredContent": state}}
        elif name in actions:
            responses = actions[name]
            index = action_indexes.get(name, 0)
            action_indexes[name] = index + 1
            response = responses[min(index, len(responses) - 1)]
            if "error" in response:
                result = {{
                    "isError": True,
                    "content": [{{"type": "text", "text": response["error"]}}]
                }}
            else:
                result = response.get("result", {{}})
        else:
            result = {{}}
    else:
        result = {{}}
    print(json.dumps({{"jsonrpc": "2.0", "id": request_id, "result": result}}), flush=True)
"#,
    );
    std::fs::write(&binary, script).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let paths = ComputerPaths::new(data.path());
    let install = InstallState {
        version: "test".into(),
        tag: "test".into(),
        asset: "test".into(),
        sha256: "test".into(),
        install_dir,
        installed_at: chrono::Utc::now(),
    };
    let settings = ComputerSettings {
        enabled: true,
        screenshots: false,
        ..ComputerSettings::default()
    };
    let backend = CuaBackend::spawn(&paths, &settings, &install)
        .await
        .expect("fake Cua driver should spawn");
    let screenshots = tempfile::tempdir().unwrap();
    (backend, data, screenshots)
}

#[cfg(unix)]
#[tokio::test]
async fn sheet_window_degraded_ok_retargets_to_owning_window() {
    let pid = 42;
    let windows = json!([
        {
            "pid": pid,
            "window_id": 374,
            "title": "Save",
            "app_name": "TextEdit",
            "z_index": 98,
            "is_on_screen": true,
            "minimized": false
        },
        {
            "pid": pid,
            "window_id": 324,
            "title": "Untitled 2",
            "app_name": "TextEdit",
            "z_index": 78,
            "is_on_screen": true,
            "minimized": false
        },
        {
            "pid": pid,
            "window_id": 320,
            "title": "Untitled",
            "app_name": "TextEdit",
            "z_index": 77,
            "is_on_screen": false,
            "minimized": false
        }
    ]);
    let states = json!({
        "374": {
            "pid": pid,
            "window_id": 374,
            "app_name": "TextEdit",
            "window_title": "Save",
            "degraded": true,
            "degraded_reason": "ax_window_unresolved: Save sheet",
            "elements": []
        },
        "320": {
            "pid": pid,
            "window_id": 320,
            "app_name": "TextEdit",
            "window_title": "Untitled",
            "degraded": true,
            "degraded_reason": "ax_window_unresolved: stale window",
            "elements": []
        },
        "324": {
            "pid": pid,
            "window_id": 324,
            "app_name": "TextEdit",
            "window_title": "Untitled 2",
            "elements": [
                {"element_index": 0, "role": "AXSheet", "label": "Save", "depth": 0},
                {"element_index": 1, "role": "AXButton", "label": "Save", "depth": 1}
            ]
        }
    });
    let (backend, _data, screenshots) = scripted_cua_backend(windows, states).await;
    let state = backend
        .window_state(
            pid,
            Some(374),
            true,
            false,
            100,
            1200,
            screenshots.path(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(state.window_id, 324);
    assert!(!state.elements.is_empty());
    assert!(
        state
            .note
            .as_deref()
            .is_some_and(|note| note.contains("unresolved on the CG↔AX map"))
    );

    let observation = backend
        .observe(&ObserveRequest {
            scope: ObserveScope::Window {
                pid,
                window_id: Some(374),
            },
            semantic: true,
            screenshot: false,
            max_elements: 100,
            full_tree: false,
            max_dimension: 1200,
            screenshot_dir: screenshots.path().into(),
        })
        .await
        .unwrap();
    let observed_window = observation.window.unwrap();
    assert_eq!(observed_window.window_id, Some(324));
    assert!(observation.text.contains("Save"));
}

#[cfg(unix)]
#[tokio::test]
async fn degraded_window_without_resolvable_sibling_is_returned_as_is() {
    let pid = 42;
    let windows = json!([{
        "pid": pid,
        "window_id": 374,
        "title": "Save",
        "app_name": "TextEdit",
        "z_index": 98,
        "is_on_screen": true,
        "minimized": false
    }]);
    let states = json!({
        "374": {
            "pid": pid,
            "window_id": 374,
            "app_name": "TextEdit",
            "window_title": "Save",
            "degraded": true,
            "degraded_reason": "ax_window_unresolved: Save sheet",
            "elements": []
        }
    });
    let (backend, _data, screenshots) = scripted_cua_backend(windows, states).await;
    let state = backend
        .window_state(
            pid,
            Some(374),
            true,
            false,
            100,
            1200,
            screenshots.path(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(state.window_id, 374);
    assert!(state.elements.is_empty());
    assert!(
        state
            .note
            .as_deref()
            .is_some_and(|note| note.contains("ax_window_unresolved"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn sheet_retarget_prefers_candidate_with_sheet_subtree() {
    let pid = 42;
    let windows = json!([
        {
            "pid": pid,
            "window_id": 374,
            "title": "Save",
            "app_name": "TextEdit",
            "z_index": 98,
            "is_on_screen": true,
            "minimized": false
        },
        {
            "pid": pid,
            "window_id": 401,
            "title": "Untitled",
            "app_name": "TextEdit",
            "z_index": 95,
            "is_on_screen": true,
            "minimized": false
        },
        {
            "pid": pid,
            "window_id": 409,
            "title": "Untitled 2",
            "app_name": "TextEdit",
            "z_index": 70,
            "is_on_screen": true,
            "minimized": false
        }
    ]);
    let states = json!({
        "374": {
            "pid": pid,
            "window_id": 374,
            "app_name": "TextEdit",
            "window_title": "Save",
            "degraded": true,
            "degraded_reason": "ax_window_unresolved: Save sheet",
            "elements": []
        },
        "401": {
            "pid": pid,
            "window_id": 401,
            "app_name": "TextEdit",
            "window_title": "Untitled",
            "elements": [
                {"element_index": 0, "role": "AXButton", "label": "New"}
            ]
        },
        "409": {
            "pid": pid,
            "window_id": 409,
            "app_name": "TextEdit",
            "window_title": "Untitled 2",
            "elements": [
                {"element_index": 0, "role": "dialog", "label": "Save"},
                {"element_index": 1, "role": "AXButton", "label": "Save"}
            ]
        }
    });
    let (backend, _data, screenshots) = scripted_cua_backend(windows, states).await;
    let state = backend
        .window_state(
            pid,
            Some(374),
            true,
            false,
            100,
            1200,
            screenshots.path(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(state.window_id, 409);
    assert!(
        state
            .note
            .as_deref()
            .is_some_and(|note| note.contains("owning window 409"))
    );
    assert!(
        state
            .note
            .as_deref()
            .is_some_and(|note| note.contains("sheet elements are inside this tree"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn sheet_retarget_fallback_reports_unconfirmed_ownership() {
    let pid = 42;
    let windows = json!([
        {
            "pid": pid,
            "window_id": 374,
            "title": "Save",
            "app_name": "TextEdit",
            "z_index": 98,
            "is_on_screen": true,
            "minimized": false
        },
        {
            "pid": pid,
            "window_id": 401,
            "title": "Untitled",
            "app_name": "TextEdit",
            "z_index": 95,
            "is_on_screen": true,
            "minimized": false
        },
        {
            "pid": pid,
            "window_id": 409,
            "title": "Untitled 2",
            "app_name": "TextEdit",
            "z_index": 70,
            "is_on_screen": true,
            "minimized": false
        }
    ]);
    let states = json!({
        "374": {
            "pid": pid,
            "window_id": 374,
            "app_name": "TextEdit",
            "window_title": "Save",
            "degraded": true,
            "degraded_reason": "ax_window_unresolved: Save sheet",
            "elements": []
        },
        "401": {
            "pid": pid,
            "window_id": 401,
            "app_name": "TextEdit",
            "window_title": "Untitled",
            "elements": [
                {"element_index": 0, "role": "AXButton", "label": "New"}
            ]
        },
        "409": {
            "pid": pid,
            "window_id": 409,
            "app_name": "TextEdit",
            "window_title": "Untitled 2",
            "elements": [
                {"element_index": 0, "role": "AXButton", "label": "New"}
            ]
        }
    });
    let (backend, _data, screenshots) = scripted_cua_backend(windows, states).await;
    let state = backend
        .window_state(
            pid,
            Some(374),
            true,
            false,
            100,
            1200,
            screenshots.path(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(state.window_id, 401);
    assert!(
        state
            .note
            .as_deref()
            .is_some_and(|note| note.contains("could not confirm the owning window"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn invoke_menu_raises_explicit_window_first() {
    let pid = 42;
    let windows = json!([{
        "pid": pid,
        "window_id": 401,
        "title": "Untitled",
        "app_name": "TextEdit",
        "z_index": 95,
        "is_on_screen": true,
        "minimized": false
    }]);
    let states = json!({});
    let (backend, data, screenshots) = scripted_cua_backend(windows, states).await;
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_invoke_menu".into(),
            args: json!({"pid": pid, "windowId": 401, "path": "File > Save…"}),
            delivery: Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(
        outcome
            .summary
            .contains("raised window 401 first so the menu command targets it")
    );
    let calls = std::fs::read_to_string(data.path().join("calls.jsonl")).unwrap();
    let calls: Vec<Value> = calls
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    let tool_calls: Vec<&Value> = calls
        .iter()
        .filter(|call| call.get("name").and_then(Value::as_str).is_some())
        .collect();
    let names: Vec<&str> = tool_calls
        .iter()
        .filter_map(|call| call.get("name").and_then(Value::as_str))
        .collect();
    let focus = names
        .iter()
        .position(|name| *name == "bring_to_front")
        .expect("bring_to_front call recorded");
    let invoke = names
        .iter()
        .position(|name| *name == "invoke_menu")
        .expect("invoke_menu call recorded");
    assert!(focus < invoke, "focus call must precede invoke_menu");
    assert_eq!(
        tool_calls[focus]["args"]["pid"].as_i64(),
        Some(pid),
        "focus call must target the requested pid"
    );
    assert_eq!(
        tool_calls[focus]["args"]["window_id"].as_i64(),
        Some(401),
        "focus call must target the requested window"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn invoke_menu_waits_for_app_activation_before_dispatch() {
    let pid = 42;
    let windows = json!([{
        "pid": pid,
        "window_id": 401,
        "title": "Untitled",
        "app_name": "TextEdit",
        "z_index": 95,
        "is_on_screen": true,
        "minimized": false
    }]);
    let states = json!({
        "401": {
            "pid": pid,
            "window_id": 401,
            "app_name": "TextEdit",
            "window_title": "Untitled",
            "elements": []
        }
    });
    let actions = json!({
        "list_apps": [
            {"result": {"structuredContent": {"apps": [{"pid": pid, "active": false}]}}},
            {"result": {"structuredContent": {"apps": [{"pid": pid, "active": true}]}}}
        ],
        "invoke_menu": [{"result": {"content": [{"type": "text", "text": "menu invoked"}]}}]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_invoke_menu".into(),
            args: json!({"pid": pid, "windowId": 401, "path": "File > Save"}),
            delivery: Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(
        !outcome
            .summary
            .contains("app did not report active before menu dispatch")
    );
    let calls: Vec<Value> = std::fs::read_to_string(data.path().join("calls.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|call: &Value| call.get("name").and_then(Value::as_str).is_some())
        .collect();
    let list_apps: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter_map(|(index, call)| {
            (call.get("name").and_then(Value::as_str) == Some("list_apps")).then_some(index)
        })
        .collect();
    let invoke = calls
        .iter()
        .position(|call| call.get("name").and_then(Value::as_str) == Some("invoke_menu"))
        .expect("invoke_menu call recorded");
    assert_eq!(list_apps.len(), 2);
    assert!(
        list_apps[1] < invoke,
        "invoke_menu must follow the active list_apps response"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn invoke_menu_dispatches_when_app_never_reports_active() {
    let pid = 42;
    let windows = json!([{
        "pid": pid,
        "window_id": 401,
        "title": "Untitled",
        "app_name": "TextEdit",
        "z_index": 95,
        "is_on_screen": true,
        "minimized": false
    }]);
    let states = json!({
        "401": {
            "pid": pid,
            "window_id": 401,
            "app_name": "TextEdit",
            "window_title": "Untitled",
            "elements": []
        }
    });
    let actions = json!({
        "list_apps": [{"result": {"structuredContent": {"apps": [{"pid": pid, "active": false}]}}}],
        "invoke_menu": [{"result": {"content": [{"type": "text", "text": "menu invoked"}]}}]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_invoke_menu".into(),
            args: json!({"pid": pid, "windowId": 401, "path": "File > Save"}),
            delivery: Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(
        outcome
            .summary
            .contains("app did not report active before menu dispatch")
    );
    let calls: Vec<Value> = std::fs::read_to_string(data.path().join("calls.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|call: &Value| call.get("name").and_then(Value::as_str).is_some())
        .collect();
    let list_apps = calls
        .iter()
        .position(|call| call.get("name").and_then(Value::as_str) == Some("list_apps"))
        .expect("list_apps call recorded");
    let invoke = calls
        .iter()
        .position(|call| call.get("name").and_then(Value::as_str) == Some("invoke_menu"))
        .expect("invoke_menu call recorded");
    assert!(
        list_apps < invoke,
        "activation polling must precede dispatch"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn invoke_menu_normalizes_semantic_ellipsis_path() {
    let pid = 42;
    let windows = json!([{
        "pid": pid,
        "window_id": 401,
        "title": "Untitled",
        "app_name": "TextEdit",
        "z_index": 95,
        "is_on_screen": true,
        "minimized": false
    }]);
    let menu_state = json!({
            "pid": pid,
            "window_id": 401,
            "app_name": "TextEdit",
            "window_title": "Untitled",
            "elements": [
                {"element_index": 0, "role": "AXMenuBarItem", "label": "File"},
                {"element_index": 1, "role": "AXMenuItem", "label": "Save…"}
            ]
    });
    let closed_state = json!({
        "pid": pid,
        "window_id": 401,
        "app_name": "TextEdit",
        "window_title": "Untitled",
        "elements": []
    });
    let states = json!({"401": [menu_state, closed_state]});
    let actions = json!({
        "invoke_menu": [
            {"error": "invoke_menu: path segment 1 was not found (refused)"},
            {"result": {"content": [{"type": "text", "text": "menu invoked"}]}}
        ]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_invoke_menu".into(),
            args: json!({"pid": pid, "windowId": 401, "path": "File > Save"}),
            delivery: Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(outcome.summary.contains("menu path normalized"));
    let calls = std::fs::read_to_string(data.path().join("calls.jsonl")).unwrap();
    let invokes: Vec<Value> = calls
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|call| call.get("name").and_then(Value::as_str) == Some("invoke_menu"))
        .collect();
    assert_eq!(invokes.len(), 2);
    assert_eq!(invokes[1]["args"]["path"], json!(["File", "Save…"]));
}

#[cfg(unix)]
#[tokio::test]
async fn invoke_menu_uses_ellipsis_fallback_without_menu_items() {
    let pid = 42;
    let windows = json!([{
        "pid": pid,
        "window_id": 401,
        "title": "Untitled",
        "app_name": "TextEdit",
        "z_index": 95,
        "is_on_screen": true,
        "minimized": false
    }]);
    let states = json!({
        "401": {
            "pid": pid,
            "window_id": 401,
            "app_name": "TextEdit",
            "window_title": "Untitled",
            "elements": []
        }
    });
    let actions = json!({
        "invoke_menu": [
            {"error": "invoke_menu: path segment 1 was not found (refused)"},
            {"result": {"content": [{"type": "text", "text": "menu invoked"}]}}
        ]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_invoke_menu".into(),
            args: json!({"pid": pid, "windowId": 401, "path": "File > Save"}),
            delivery: Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(outcome.summary.contains("menu path normalized"));
    let calls = std::fs::read_to_string(data.path().join("calls.jsonl")).unwrap();
    let invokes: Vec<Value> = calls
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|call| call.get("name").and_then(Value::as_str) == Some("invoke_menu"))
        .collect();
    assert_eq!(invokes.len(), 2);
    assert_eq!(invokes[1]["args"]["path"], json!(["File", "Save…"]));
}

#[cfg(unix)]
#[tokio::test]
async fn invoke_menu_recovers_when_driver_leaves_menu_open() {
    let pid = 42;
    let windows = json!([{
        "pid": pid,
        "window_id": 401,
        "title": "Untitled",
        "app_name": "TextEdit",
        "z_index": 95,
        "is_on_screen": true,
        "minimized": false
    }]);
    let menu_state = json!({
        "pid": pid,
        "window_id": 401,
        "app_name": "TextEdit",
        "window_title": "Untitled",
        "window_bounds": {"x": 100.0, "y": 80.0, "w": 500.0, "h": 400.0},
        "elements": [
            {
                "element_index": 0,
                "role": "AXWindow",
                "depth": 0,
                "frame": {"x": 100.0, "y": 80.0, "w": 500.0, "h": 400.0}
            },
            {
                "element_index": 1,
                "role": "AXMenuItem",
                "label": "Save…",
                "frame": {"x": 200.0, "y": 100.0, "w": 80.0, "h": 20.0}
            }
        ]
    });
    let closed_state = json!({
        "pid": pid,
        "window_id": 401,
        "app_name": "TextEdit",
        "window_title": "Untitled",
        "window_bounds": {"x": 100.0, "y": 80.0, "w": 500.0, "h": 400.0},
        "elements": [{
            "element_index": 0,
            "role": "AXWindow",
            "depth": 0,
            "frame": {"x": 100.0, "y": 80.0, "w": 500.0, "h": 400.0}
        }, {
            "element_index": 1,
            "role": "AXMenuItem",
            "label": "Save…"
        }]
    });
    let states = json!({"401": [menu_state.clone(), menu_state, closed_state]});
    let actions = json!({
        "invoke_menu": [{"result": {"content": [{"type": "text", "text": "menu invoked"}]}}],
        "type_text": [{"result": {"content": [{"type": "text", "text": "typed"}]}}],
        "press_key": [{"result": {"content": [{"type": "text", "text": "pressed"}]}}],
        "list_apps": [{"result": {"structuredContent": {"apps": [{"pid": pid, "active": true}]}}}]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_invoke_menu".into(),
            args: json!({"pid": pid, "windowId": 401, "path": "File > Save"}),
            delivery: Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(outcome.summary.contains(
        "driver's menu press left the menu open; completed \"Save\" via menu type-select + Return"
    ));
    let calls: Vec<Value> = std::fs::read_to_string(data.path().join("calls.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|call: &Value| {
            matches!(
                call.get("name").and_then(Value::as_str),
                Some("type_text" | "press_key")
            )
        })
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["name"], json!("type_text"));
    assert_eq!(calls[0]["args"]["scope"], json!("desktop"));
    assert_eq!(calls[0]["args"]["text"], json!("Save"));
    assert_eq!(calls[1]["name"], json!("press_key"));
    assert_eq!(calls[1]["args"]["scope"], json!("desktop"));
    assert_eq!(calls[1]["args"]["key"], json!("return"));
    assert!(!calls.iter().any(|call| {
        call["name"] == json!("press_key") && call["args"]["key"] == json!("escape")
    }));
}

#[cfg(unix)]
#[tokio::test]
async fn invoke_menu_does_not_recover_when_menu_is_closed() {
    let pid = 42;
    let windows = json!([{
        "pid": pid,
        "window_id": 401,
        "title": "Untitled",
        "app_name": "TextEdit",
        "z_index": 95,
        "is_on_screen": true,
        "minimized": false
    }]);
    let states = json!({"401": {
        "pid": pid,
        "window_id": 401,
        "app_name": "TextEdit",
        "window_title": "Untitled",
        "window_bounds": {"x": 100.0, "y": 80.0, "w": 500.0, "h": 400.0},
        "elements": [{
            "element_index": 0,
            "role": "AXWindow",
            "depth": 0,
            "frame": {"x": 100.0, "y": 80.0, "w": 500.0, "h": 400.0}
        }, {
            "element_index": 1,
            "role": "AXMenuItem",
            "label": "Save…"
        }]
    }});
    let actions = json!({
        "invoke_menu": [{"result": {"content": [{"type": "text", "text": "menu invoked"}]}}]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_invoke_menu".into(),
            args: json!({"pid": pid, "windowId": 401, "path": "File > Save"}),
            delivery: Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(!outcome.summary.contains("menu type-select"));
    let calls = std::fs::read_to_string(data.path().join("calls.jsonl")).unwrap();
    assert!(!calls.contains("\"name\": \"type_text\""));
}

#[cfg(unix)]
#[tokio::test]
async fn invoke_menu_reports_unconfirmed_command_when_menu_stays_open() {
    let pid = 42;
    let windows = json!([{
        "pid": pid,
        "window_id": 401,
        "title": "Untitled",
        "app_name": "TextEdit",
        "z_index": 95,
        "is_on_screen": true,
        "minimized": false
    }]);
    let menu_state = json!({
        "pid": pid,
        "window_id": 401,
        "app_name": "TextEdit",
        "window_title": "Untitled",
        "window_bounds": {"x": 100.0, "y": 80.0, "w": 500.0, "h": 400.0},
        "elements": [{
            "element_index": 1,
            "role": "AXMenuItem",
            "label": "Save…",
            "frame": {"x": 200.0, "y": 100.0, "w": 80.0, "h": 20.0}
        }]
    });
    let states = json!({"401": [menu_state.clone(), menu_state]});
    let actions = json!({
        "invoke_menu": [{"result": {"content": [{"type": "text", "text": "menu invoked"}]}}],
        "press_key": [{"result": {"content": [{"type": "text", "text": "pressed"}]}}],
        "type_text": [{"result": {"content": [{"type": "text", "text": "typed"}]}}],
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    let error = backend
        .act(&ActionRequest {
            tool: "computer_invoke_menu".into(),
            args: json!({"pid": pid, "windowId": 401, "path": "File > Save"}),
            delivery: Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .expect_err("persistent menu should not report success");
    assert!(matches!(
        error,
        ComputerError::Refused { ref code, ref message }
            if code == "suspected_noop"
                && message.contains("menu type-select + Return")
                && message.contains("menu command not confirmed")
    ));
    let calls: Vec<Value> = std::fs::read_to_string(data.path().join("calls.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|call: &Value| {
            matches!(
                call.get("name").and_then(Value::as_str),
                Some("type_text" | "press_key")
            )
        })
        .collect();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0]["name"], json!("type_text"));
    assert_eq!(calls[0]["args"]["scope"], json!("desktop"));
    assert_eq!(calls[0]["args"]["text"], json!("Save"));
    assert_eq!(calls[1]["name"], json!("press_key"));
    assert_eq!(calls[1]["args"]["scope"], json!("desktop"));
    assert_eq!(calls[1]["args"]["key"], json!("return"));
    assert_eq!(calls[2]["name"], json!("press_key"));
    assert_eq!(calls[2]["args"]["scope"], json!("desktop"));
    assert_eq!(calls[2]["args"]["key"], json!("escape"));
}

#[cfg(unix)]
#[tokio::test]
async fn sheet_owner_alias_routes_actions_to_owner_window() {
    let pid = 42;
    let windows = json!([
        {
            "pid": pid,
            "window_id": 688,
            "title": "Save",
            "app_name": "TextEdit",
            "z_index": 98,
            "is_on_screen": true,
            "minimized": false
        },
        {
            "pid": pid,
            "window_id": 700,
            "title": "Untitled",
            "app_name": "TextEdit",
            "z_index": 78,
            "is_on_screen": true,
            "minimized": false
        }
    ]);
    let states = json!({
        "688": {
            "pid": pid,
            "window_id": 688,
            "app_name": "TextEdit",
            "window_title": "Save",
            "degraded": true,
            "degraded_reason": "ax_window_unresolved: Save sheet",
            "elements": []
        },
        "700": {
            "pid": pid,
            "window_id": 700,
            "app_name": "TextEdit",
            "window_title": "Untitled",
            "window_bounds": {"x": 100.0, "y": 200.0, "w": 500.0, "h": 400.0},
            "elements": [
                {"element_index": 0, "role": "AXSheet", "label": "Save"},
                {
                    "element_index": 1,
                    "element_token": "s000000d6:121",
                    "role": "AXTextField",
                    "label": "File name",
                    "frame": {"x": 20.0, "y": 30.0, "w": 40.0, "h": 20.0}
                }
            ]
        }
    });
    let actions = json!({
        "set_value": [
            {"error": "set_value: element_outside_target_window (refused)"}
        ],
        "click": [{"result": {"content": [{"type": "text", "text": "clicked"}]}}],
        "press_key": [{"result": {"content": [{"type": "text", "text": "selected"}]}}],
        "type_text": [{"result": {"content": [{"type": "text", "text": "typed"}]}}]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    backend
        .observe(&ObserveRequest {
            scope: ObserveScope::Window {
                pid,
                window_id: Some(688),
            },
            semantic: true,
            screenshot: false,
            max_elements: 100,
            full_tree: false,
            max_dimension: 1200,
            screenshot_dir: screenshots.path().into(),
        })
        .await
        .unwrap();
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_set_value".into(),
            args: json!({
                "pid": pid,
                "windowId": 688,
                "elementToken": "s000000d6:121",
                "snapshotId": "s000000d6",
                "elementIndex": 121,
                "value": "new value"
            }),
            delivery: Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(outcome.summary.contains("is a sheet of window 700"));
    assert!(outcome.summary.contains("out-of-process sheet"));
    let calls = std::fs::read_to_string(data.path().join("calls.jsonl")).unwrap();
    let set_values: Vec<Value> = calls
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|call| call.get("name").and_then(Value::as_str) == Some("set_value"))
        .collect();
    assert_eq!(set_values.len(), 1);
    assert_eq!(set_values[0]["args"]["window_id"], json!(700));
    let clicks: Vec<Value> = calls
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|call| call.get("name").and_then(Value::as_str) == Some("click"))
        .collect();
    assert_eq!(clicks.len(), 1);
    assert_eq!(clicks[0]["args"]["scope"], json!("desktop"));
}

#[cfg(unix)]
#[tokio::test]
async fn token_conflict_retargets_without_stale_snapshot_fields() {
    let pid = 42;
    let windows = json!([
        {
            "pid": pid,
            "window_id": 613,
            "title": "Save",
            "app_name": "TextEdit",
            "z_index": 98,
            "is_on_screen": true,
            "minimized": false
        },
        {
            "pid": pid,
            "window_id": 587,
            "title": "Untitled",
            "app_name": "TextEdit",
            "z_index": 78,
            "is_on_screen": true,
            "minimized": false
        }
    ]);
    let states = json!({});
    let actions = json!({
        "set_value": [
            {"error": "set_value: element_token conflicts with element_index, snapshot_id, or window_id (refused)"},
            {"result": {"content": [{"type": "text", "text": "value set"}]}}
        ]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    backend
        .act(&ActionRequest {
            tool: "computer_set_value".into(),
            args: json!({
                "pid": pid,
                "windowId": 613,
                "elementToken": "s000000d6:121",
                "snapshotId": "s000000d6",
                "elementIndex": 121,
                "value": "new value"
            }),
            delivery: Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    let calls = std::fs::read_to_string(data.path().join("calls.jsonl")).unwrap();
    let set_values: Vec<Value> = calls
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|call| call.get("name").and_then(Value::as_str) == Some("set_value"))
        .collect();
    assert_eq!(set_values.len(), 2);
    assert_eq!(set_values[1]["args"]["window_id"], json!(587));
    assert_eq!(
        set_values[1]["args"]["element_token"],
        json!("s000000d6:121")
    );
    assert!(set_values[1]["args"].get("snapshot_id").is_none());
    assert!(set_values[1]["args"].get("element_index").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn element_outside_target_window_falls_back_to_screen_click() {
    let pid = 67439;
    let token = "s00000113:90";
    let windows = json!([{
        "pid": pid,
        "window_id": 688,
        "title": "Save",
        "app_name": "TextEdit",
        "z_index": 98,
        "is_on_screen": true,
        "minimized": false
    }]);
    let states = json!({
        "688": {
            "snapshot_id": "s00000113",
            "pid": pid,
            "window_id": 688,
            "window_bounds": {"x": 100.0, "y": 200.0, "w": 500.0, "h": 400.0},
            "elements": [{
                "element_index": 0,
                "role": "AXWindow",
                "depth": 0,
                "frame": {"x": 100.0, "y": 200.0, "w": 500.0, "h": 400.0}
            }, {
                "element_index": 90,
                "element_token": token,
                "role": "AXMenuItem",
                "label": "Save",
                "frame": {"x": 120.0, "y": 230.0, "w": 40.0, "h": 20.0}
            }]
        }
    });
    let actions = json!({
        "click": [
            {"error": "click: element_outside_target_window (refused)"},
            {"result": {"content": [{"type": "text", "text": "clicked"}]}}
        ]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    backend
        .observe(&ObserveRequest {
            scope: ObserveScope::Window {
                pid,
                window_id: Some(688),
            },
            semantic: true,
            screenshot: false,
            max_elements: 100,
            full_tree: false,
            max_dimension: 1200,
            screenshot_dir: screenshots.path().into(),
        })
        .await
        .unwrap();
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_click".into(),
            args: json!({
                "pid": pid,
                "windowId": 688,
                "elementToken": token,
                "snapshotId": "s00000113",
                "elementIndex": 90
            }),
            delivery: crate::backend::Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(
        outcome
            .summary
            .contains("menu item lives in the menu-bar window")
    );
    assert!(outcome.summary.contains("(140,240)"));
    let calls: Vec<Value> = std::fs::read_to_string(data.path().join("calls.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|call: &Value| call.get("name").and_then(Value::as_str) == Some("click"))
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["args"]["scope"], json!("desktop"));
    assert_eq!(calls[1]["args"]["x"], json!(140.0));
    assert_eq!(calls[1]["args"]["y"], json!(240.0));
    assert!(calls[1]["args"].get("element_token").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn element_outside_target_window_snapshot_index_falls_back_to_screen_click() {
    let pid = 67439;
    let token = "s00000113:90";
    let windows = json!([{
        "pid": pid,
        "window_id": 688,
        "title": "Save",
        "app_name": "TextEdit",
        "z_index": 98,
        "is_on_screen": true,
        "minimized": false
    }]);
    let states = json!({
        "688": {
            "snapshot_id": "s00000113",
            "pid": pid,
            "window_id": 688,
            "window_bounds": {"x": 100.0, "y": 200.0, "w": 500.0, "h": 400.0},
            "elements": [{
                "element_index": 0,
                "role": "AXWindow",
                "depth": 0,
                "frame": {"x": 100.0, "y": 200.0, "w": 500.0, "h": 400.0}
            }, {
                "element_index": 90,
                "element_token": token,
                "role": "AXMenuItem",
                "label": "Save",
                "frame": {"x": 120.0, "y": 230.0, "w": 40.0, "h": 20.0}
            }]
        }
    });
    let actions = json!({
        "click": [
            {"error": "click: element_outside_target_window (refused)"},
            {"result": {"content": [{"type": "text", "text": "clicked"}]}}
        ]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    backend
        .observe(&ObserveRequest {
            scope: ObserveScope::Window {
                pid,
                window_id: Some(688),
            },
            semantic: true,
            screenshot: false,
            max_elements: 100,
            full_tree: false,
            max_dimension: 1200,
            screenshot_dir: screenshots.path().into(),
        })
        .await
        .unwrap();
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_click".into(),
            args: json!({
                "pid": pid,
                "windowId": 688,
                "snapshotId": "s00000113",
                "elementIndex": 90
            }),
            delivery: crate::backend::Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(
        outcome
            .summary
            .contains("menu item lives in the menu-bar window")
    );
    assert!(outcome.summary.contains("(140,240)"));
    let calls: Vec<Value> = std::fs::read_to_string(data.path().join("calls.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|call: &Value| call.get("name").and_then(Value::as_str) == Some("click"))
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["args"]["scope"], json!("desktop"));
    assert_eq!(calls[1]["args"]["x"], json!(140.0));
    assert_eq!(calls[1]["args"]["y"], json!(240.0));
    assert!(calls[1]["args"].get("element_token").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn element_outside_target_window_local_frame_adds_window_bounds() {
    let pid = 67439;
    let token = "s00000114:90";
    let windows = json!([{
        "pid": pid,
        "window_id": 688,
        "title": "Save",
        "app_name": "TextEdit",
        "z_index": 98,
        "is_on_screen": true,
        "minimized": false
    }]);
    let states = json!({
        "688": {
            "snapshot_id": "s00000114",
            "pid": pid,
            "window_id": 688,
            "window_bounds": {"x": 100.0, "y": 200.0, "w": 500.0, "h": 400.0},
            "elements": [{
                "element_index": 0,
                "role": "AXWindow",
                "depth": 0,
                "frame": {"x": 0.0, "y": 0.0, "w": 500.0, "h": 400.0}
            }, {
                "element_index": 90,
                "element_token": token,
                "role": "AXButton",
                "label": "Save",
                "frame": {"x": 20.0, "y": 30.0, "w": 40.0, "h": 20.0}
            }]
        }
    });
    let actions = json!({
        "click": [
            {"error": "click: element_outside_target_window (refused)"},
            {"result": {"content": [{"type": "text", "text": "clicked"}]}}
        ]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    backend
        .observe(&ObserveRequest {
            scope: ObserveScope::Window {
                pid,
                window_id: Some(688),
            },
            semantic: true,
            screenshot: false,
            max_elements: 100,
            full_tree: false,
            max_dimension: 1200,
            screenshot_dir: screenshots.path().into(),
        })
        .await
        .unwrap();
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_click".into(),
            args: json!({
                "pid": pid,
                "windowId": 688,
                "elementToken": token,
                "snapshotId": "s00000114",
                "elementIndex": 90
            }),
            delivery: crate::backend::Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(outcome.summary.contains("(140,240)"));
    let calls: Vec<Value> = std::fs::read_to_string(data.path().join("calls.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|call: &Value| call.get("name").and_then(Value::as_str) == Some("click"))
        .collect();
    assert_eq!(calls[1]["args"]["x"], json!(140.0));
    assert_eq!(calls[1]["args"]["y"], json!(240.0));
}

#[cfg(unix)]
#[tokio::test]
async fn element_outside_target_window_absolute_frame_without_bounds_falls_back() {
    let pid = 67439;
    let token = "s00000115:90";
    let windows = json!([{
        "pid": pid,
        "window_id": 688,
        "title": "Save",
        "app_name": "TextEdit",
        "z_index": 98,
        "is_on_screen": true,
        "minimized": false
    }]);
    let states = json!({
        "688": {
            "snapshot_id": "s00000115",
            "pid": pid,
            "window_id": 688,
            "elements": [{
                "element_index": 0,
                "role": "AXWindow",
                "depth": 0,
                "frame": {"x": 100.0, "y": 200.0, "w": 500.0, "h": 400.0}
            }, {
                "element_index": 90,
                "element_token": token,
                "role": "AXButton",
                "label": "Save",
                "frame": {"x": 120.0, "y": 230.0, "w": 40.0, "h": 20.0}
            }]
        }
    });
    let actions = json!({
        "click": [
            {"error": "click: element_outside_target_window (refused)"},
            {"result": {"content": [{"type": "text", "text": "clicked"}]}}
        ]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    backend
        .observe(&ObserveRequest {
            scope: ObserveScope::Window {
                pid,
                window_id: Some(688),
            },
            semantic: true,
            screenshot: false,
            max_elements: 100,
            full_tree: false,
            max_dimension: 1200,
            screenshot_dir: screenshots.path().into(),
        })
        .await
        .unwrap();
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_click".into(),
            args: json!({
                "pid": pid,
                "windowId": 688,
                "elementToken": token,
                "snapshotId": "s00000115",
                "elementIndex": 90
            }),
            delivery: crate::backend::Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(outcome.summary.contains("(140,240)"));
    let calls: Vec<Value> = std::fs::read_to_string(data.path().join("calls.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|call: &Value| call.get("name").and_then(Value::as_str) == Some("click"))
        .collect();
    assert_eq!(calls[1]["args"]["x"], json!(140.0));
    assert_eq!(calls[1]["args"]["y"], json!(240.0));
}

#[cfg(unix)]
#[tokio::test]
async fn element_outside_target_window_set_value_falls_back_in_order() {
    let pid = 67439;
    let token = "s00000113:90";
    let windows = json!([{
        "pid": pid,
        "window_id": 688,
        "title": "Save",
        "app_name": "TextEdit",
        "z_index": 98,
        "is_on_screen": true,
        "minimized": false
    }]);
    let states = json!({
        "688": {
            "snapshot_id": "s00000113",
            "pid": pid,
            "window_id": 688,
            "window_bounds": {"x": 100.0, "y": 200.0, "w": 500.0, "h": 400.0},
            "elements": [{
                "element_index": 0,
                "role": "AXWindow",
                "depth": 0,
                "frame": {"x": 100.0, "y": 200.0, "w": 500.0, "h": 400.0}
            }, {
                "element_index": 90,
                "element_token": token,
                "role": "AXTextField",
                "label": "File name",
                "frame": {"x": 120.0, "y": 230.0, "w": 40.0, "h": 20.0}
            }]
        }
    });
    let actions = json!({
        "set_value": [
            {"error": "set_value: element_outside_target_window (refused)"}
        ],
        "click": [{"result": {"content": [{"type": "text", "text": "clicked"}]}}],
        "press_key": [{"result": {"content": [{"type": "text", "text": "selected"}]}}],
        "type_text": [{"result": {"content": [{"type": "text", "text": "typed"}]}}]
    });
    let (backend, data, screenshots) =
        scripted_cua_backend_with_actions(windows, states, actions).await;
    backend
        .observe(&ObserveRequest {
            scope: ObserveScope::Window {
                pid,
                window_id: Some(688),
            },
            semantic: true,
            screenshot: false,
            max_elements: 100,
            full_tree: false,
            max_dimension: 1200,
            screenshot_dir: screenshots.path().into(),
        })
        .await
        .unwrap();
    let outcome = backend
        .act(&ActionRequest {
            tool: "computer_set_value".into(),
            args: json!({
                "pid": pid,
                "windowId": 688,
                "elementToken": token,
                "snapshotId": "s00000113",
                "elementIndex": 90,
                "value": "report.txt"
            }),
            delivery: crate::backend::Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap();
    assert!(outcome.summary.contains("out-of-process sheet"));
    let calls: Vec<Value> = std::fs::read_to_string(data.path().join("calls.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let tool_calls: Vec<&Value> = calls
        .iter()
        .filter(|call| call.get("name").and_then(Value::as_str).is_some())
        .collect();
    let names: Vec<&str> = tool_calls
        .iter()
        .filter_map(|call| call.get("name").and_then(Value::as_str))
        .collect();
    let set = tool_calls
        .iter()
        .position(|call| call.get("name").and_then(Value::as_str) == Some("set_value"))
        .unwrap();
    assert_eq!(
        &names[set..set + 4],
        &["set_value", "click", "press_key", "type_text"]
    );
    let press = &tool_calls[set + 2]["args"];
    assert_eq!(press["scope"], json!("desktop"));
    assert_eq!(press["key"], json!("a"));
    assert_eq!(press["modifiers"], json!(["command"]));
    assert_eq!(tool_calls[set + 3]["args"]["scope"], json!("desktop"));
    assert_eq!(tool_calls[set + 3]["args"]["text"], json!("report.txt"));
}

#[cfg(unix)]
#[tokio::test]
async fn unknown_element_outside_target_window_returns_original_error() {
    let pid = 67439;
    let windows = json!([{
        "pid": pid,
        "window_id": 688,
        "title": "Save",
        "app_name": "TextEdit",
        "z_index": 98,
        "is_on_screen": true,
        "minimized": false
    }]);
    let actions = json!({
        "click": [{"error": "click: element_outside_target_window (refused)"}]
    });
    let (backend, _data, screenshots) =
        scripted_cua_backend_with_actions(windows, json!({}), actions).await;
    let error = backend
        .act(&ActionRequest {
            tool: "computer_click".into(),
            args: json!({
                "pid": pid,
                "windowId": 688,
                "elementToken": "s00000113:999"
            }),
            delivery: crate::backend::Delivery::Background,
            session: None,
            screenshot_dir: screenshots.path().into(),
            observe_screenshot: false,
            observe_semantic: false,
            max_elements: 100,
            max_dimension: 1200,
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("element_outside_target_window"));
}

#[tokio::test]
async fn dead_backend_is_evicted_and_respawned() {
    let dead = Arc::new(MockComputer::working());
    dead.set_alive(false);
    let fresh = Arc::new(MockComputer::working());
    let (service, _data) = service(enabled_settings(ControlMode::Background), dead.clone());
    service.set_spawn_backend_for_test(fresh.clone());
    service.install_session_backend_for_test("dead-session", dead);

    let probe = service.probe().await;

    assert!(probe.driver_ok);
    assert!(fresh.calls.load(std::sync::atomic::Ordering::Relaxed) > 0);
    assert_eq!(service.session_backend_count_for_test(), 0);
    assert_eq!(service.last_error_for_test(), None);
}

#[cfg(unix)]
#[tokio::test]
async fn mcp_client_marks_itself_dead_after_child_exits() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let script = temp.path().join("fake-driver");
    std::fs::write(
        &script,
        r#"#!/usr/bin/env python3
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    if request.get("id") is None:
        continue
    if request["method"] == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": {"name": "fake"}}
    elif request["method"] == "tools/list":
        result = {"tools": [{"name": "fake"}]}
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
        break
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&script).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&script, permissions).unwrap();

    let client = McpClient::spawn(&script, &[], &[]).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    assert!(!client.is_alive());
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
