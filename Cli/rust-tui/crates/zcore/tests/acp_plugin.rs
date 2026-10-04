//! ACP subscription plugins against a fake stdio agent.
//!
//! The fake agent is this test binary itself: run with FAKE_ACP_AGENT=1 and
//! `fake_agent --exact`, the `fake_agent` test speaks ACP on stdio (libtest's
//! own stdout lines are skipped by the client like any non-JSON chatter).
//! A `devin` shim on PATH points at it, so the real plugin lookup, the
//! Windows `.cmd` handling and the agent loop are all exercised.

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{json, Value};
use zcore::acp_client::{AcpSession, Update};

fn send(v: Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{v}").unwrap();
    out.flush().unwrap();
}

/// ~50 lines: initialize, session/new, a prompt that streams "po" + "ng",
/// one tool call gated on session/request_permission, and cancel handling.
#[test]
fn fake_agent() {
    if std::env::var_os("FAKE_ACP_AGENT").is_none() {
        return;
    }
    println!("fake agent starting (not JSON — the client must skip this)");
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    while let Some(Ok(line)) = lines.next() {
        let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
        let id = m["id"].clone();
        let up = |u: Value| send(json!({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "s1", "update": u}}));
        match m["method"].as_str().unwrap_or("") {
            "initialize" => send(json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": 1, "agentInfo": {"name": "fake", "title": "Fake Agent"}, "authMethods": []}})),
            "session/new" => send(json!({"jsonrpc": "2.0", "id": id, "result": {"sessionId": "s1", "models": {"currentModelId": "fake-fast", "availableModels": [
                {"modelId": "fake-fast", "name": "Fake Fast"},
                {"modelId": "fake-smart", "name": "Fake Smart", "description": "slow but thorough"}
            ]}}})),
            "session/set_model" => send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "no model switching"}})),
            "session/prompt" => {
                let text = m["params"]["prompt"][0]["text"].as_str().unwrap_or("").to_string();
                if text.contains("wait") {
                    // stream until the client cancels
                    for l in lines.by_ref() {
                        let c: Value = serde_json::from_str(&l.unwrap()).unwrap_or(Value::Null);
                        if c["method"] == "session/cancel" {
                            break;
                        }
                    }
                    send(json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "cancelled"}}));
                    continue;
                }
                up(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "po"}}));
                up(json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Edit a.rs", "kind": "edit", "status": "pending", "rawInput": {"path": "a.rs"}}));
                send(json!({"jsonrpc": "2.0", "id": 99, "method": "session/request_permission", "params": {"sessionId": "s1",
                    "toolCall": {"toolCallId": "t1", "title": "Edit a.rs", "kind": "edit"},
                    "options": [{"optionId": "yes", "name": "Allow", "kind": "allow_once"}, {"optionId": "no", "name": "Reject", "kind": "reject_once"}]}}));
                let reply: Value = loop {
                    let l = lines.next().unwrap().unwrap();
                    if let Ok(v) = serde_json::from_str::<Value>(&l) {
                        if v["id"] == 99 {
                            break v;
                        }
                    }
                };
                let ok = reply["result"]["outcome"]["optionId"] == "yes";
                up(json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": if ok { "completed" } else { "failed" },
                    "content": [{"type": "content", "content": {"type": "text", "text": if ok { "wrote a.rs" } else { "denied" }}}]}));
                up(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "ng"}}));
                send(json!({"jsonrpc": "2.0", "id": id, "result": {"stopReason": "end_turn"}}));
            }
            "" => {} // notifications (cancel outside a prompt)
            _ => send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "unknown"}})),
        }
    }
    std::process::exit(0);
}

fn fake_cmd() -> std::process::Command {
    let mut c = std::process::Command::new(std::env::current_exe().unwrap());
    c.args(["fake_agent", "--exact", "--nocapture", "--test-threads=1"]).env("FAKE_ACP_AGENT", "1");
    c
}

/// A `devin` launcher on PATH that runs the fake agent.
fn shim(dir: &Path) {
    let exe = std::env::current_exe().unwrap();
    if cfg!(windows) {
        std::fs::write(dir.join("devin.cmd"), format!("@\"{}\" fake_agent --exact --nocapture --test-threads=1 %*\r\n", exe.display())).unwrap();
    } else {
        let p = dir.join("devin");
        std::fs::write(&p, format!("#!/bin/sh\nexec \"{}\" fake_agent --exact --nocapture --test-threads=1 \"$@\"\n", exe.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

fn headless(ws: &Path, model: &str, allow: &[&str]) -> (i32, Vec<(String, Value)>) {
    let opts = zproto::RunOpts {
        prompt: Some("fix the bug".into()),
        model: Some(model.into()),
        cwd: Some(ws.to_string_lossy().into()),
        allow: allow.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    let mut events = Vec::new();
    let code = zcore::agent::run_headless(&opts, "cosmostui", ws, &mut |e: &zproto::Event| events.push((e.kind.clone(), e.data.clone()))).unwrap();
    (code, events)
}

#[test]
fn acp_plugins_end_to_end() {
    if std::env::var_os("FAKE_ACP_AGENT").is_some() {
        return; // we are the fake agent's process
    }
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    // 1. the session protocol: stream, permission, unsupported set_model
    let mut s = AcpSession::spawn(fake_cmd(), "devin", &ws, "some-model").unwrap();
    assert_eq!(s.agent_name, "Fake Agent");
    assert_eq!(s.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["fake-fast", "fake-smart"]);
    assert!(s.model_note.as_deref().is_some_and(|n| n.contains("kept its own model")), "{:?}", s.model_note);
    let mut got = Vec::new();
    let stop = s.prompt("ping", &mut |u| got.push(u), &mut |title, _| title == "Edit a.rs", None).unwrap();
    assert_eq!(stop, "end_turn");
    let text: String = got.iter().filter_map(|u| match u { Update::Text(t) => Some(t.as_str()), _ => None }).collect();
    assert_eq!(text, "pong");
    assert!(got.iter().any(|u| matches!(u, Update::Tool { status, output, .. } if status == "completed" && output == "wrote a.rs")), "{got:?}");

    // 2. interrupt sends session/cancel and the turn ends "cancelled"
    let flag = std::sync::atomic::AtomicBool::new(true);
    let stop = s.prompt("wait for me", &mut |_| {}, &mut |_, _| false, Some(&flag)).unwrap();
    assert_eq!(stop, "cancelled");
    drop(s);

    // the vendors' own install folders must never be found from a test:
    // point every profile variable at an empty folder
    let nohome = dir.path().join("nohome");
    std::fs::create_dir_all(&nohome).unwrap();
    let saved: Vec<(&str, Option<std::ffi::OsString>)> = ["LOCALAPPDATA", "APPDATA", "USERPROFILE", "HOME"].iter().map(|k| (*k, std::env::var_os(k))).collect();
    for (k, _) in &saved {
        std::env::set_var(k, &nohome);
    }

    // 3. through the agent loop, found on PATH as `devin`
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    shim(&bin);
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin.clone()];
    paths.extend(std::env::split_paths(&old_path));
    std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
    std::env::set_var("COSMOS_CLI_HOME", dir.path().join("home"));
    std::env::set_var("FAKE_ACP_AGENT", "1"); // inherited by the shim's child
    assert!(zcore::acp_client::installed("devin"));

    // the plan's real model list comes back over session/new — `live::fetch`
    // keeps `default` on top and preserves the vendor's own ordering
    let listed = zcore::acp_client::list_models("devin", &ws).unwrap();
    assert_eq!(listed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["fake-fast", "fake-smart"]);
    let live = zcore::live::fetch(zcore::catalog::provider("devin").unwrap()).unwrap();
    assert_eq!(live.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["default", "fake-fast", "fake-smart"]);
    zcore::live::store("devin", &live).unwrap();
    let cfg = zcore::config::Config::load(&zcore::cli_home());
    let aliases: Vec<_> = cfg.available_models().iter().filter(|m| m.provider == "devin").map(|m| m.alias.clone()).collect();
    assert_eq!(aliases, ["devin/default", "devin/fake-fast", "devin/fake-smart"]);

    let (code, ev) = headless(&ws, "devin/default", &["devin"]);
    assert_eq!(code, zproto::exit::DONE, "{ev:?}");
    let reply = ev.iter().find(|(k, _)| k == "assistant").unwrap();
    assert_eq!(reply.1["text"], "pong");
    let tool = ev.iter().find(|(k, d)| k == "tool_call" && d["source"] == "acp").unwrap();
    assert_eq!((tool.1["name"].as_str(), tool.1["status"].as_str()), (Some("devin:edit"), Some("completed")));
    let usage = ev.iter().find(|(k, _)| k == "usage").unwrap();
    assert_eq!(usage.1["billing"], "subscription");

    // headless with no --allow: the vendor's permission request is denied
    let (code, ev) = headless(&ws, "devin/default", &[]);
    assert_eq!(code, zproto::exit::DONE);
    let tool = ev.iter().find(|(k, d)| k == "tool_call" && d["source"] == "acp").unwrap();
    assert_eq!(tool.1["status"], "failed");

    // 4. a plugin that isn't installed: error event, exit 4, install hint
    std::env::set_var("PATH", &bin);
    let (code, ev) = headless(&ws, "cursor/default", &[]);
    assert_eq!(code, zproto::exit::PROVIDER, "{ev:?}");
    let err = ev.iter().find(|(k, _)| k == "error").unwrap();
    assert!(err.1["message"].as_str().unwrap().contains("Cursor is not installed"), "{}", err.1);

    std::env::set_var("PATH", old_path);
    std::env::remove_var("FAKE_ACP_AGENT");
    for (k, v) in saved {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
}
