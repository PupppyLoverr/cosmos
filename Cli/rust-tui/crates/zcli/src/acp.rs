//! ACP mode (`cosmostui acp`): newline-delimited JSON-RPC over stdio.
//! Contract §11: initialize, session/new, session/prompt, session/update
//! notifications, session/request_permission (auto-deny headless),
//! session/cancel.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{json, Value};
use zcore::agent::Agent;
use zproto::{Event, RunOpts};

fn send(w: &mut impl Write, v: &Value) {
    let _ = writeln!(w, "{v}");
    let _ = w.flush();
}

fn result(w: &mut impl Write, id: &Value, r: Value) {
    send(w, &json!({"jsonrpc": "2.0", "id": id, "result": r}));
}

fn err(w: &mut impl Write, id: &Value, code: i64, msg: &str) {
    send(
        w,
        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}}),
    );
}

/// Map a session event to an ACP `session/update` notification payload.
fn update_for(session_id: &str, ev: &Event) -> Option<Value> {
    let update = match ev.kind.as_str() {
        "assistant" => json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": ev.data["text"].as_str().unwrap_or("")}
        }),
        "tool_call" => json!({
            "sessionUpdate": "tool_call",
            "toolCallId": ev.data["id"].as_str().unwrap_or(""),
            "title": ev.data["name"].as_str().unwrap_or("")
        }),
        "tool_result" => json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": ev.data["tool_use_id"].as_str().unwrap_or(""),
            "status": if ev.data["is_error"].as_bool().unwrap_or(false) { "failed" } else { "completed" },
            "content": [{"type": "content", "content": {"type": "text",
                "text": ev.data["content"].as_str().unwrap_or("")}}]
        }),
        _ => return None,
    };
    Some(json!({
        "jsonrpc": "2.0", "method": "session/update",
        "params": {"sessionId": session_id, "update": update}
    }))
}

pub fn run() -> i32 {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut w = stdout.lock();
    let mut agents: HashMap<String, Agent> = HashMap::new();
    let mut cwds: HashMap<String, PathBuf> = HashMap::new();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let method = req["method"].as_str().unwrap_or("");
        let id = req["id"].clone();
        let params = &req["params"];

        match method {
            "initialize" => result(
                &mut w,
                &id,
                json!({
                    "protocolVersion": 1,
                    "agentInfo": {"name": zcore::NAME, "version": env!("CARGO_PKG_VERSION")},
                    "agentCapabilities": {"loadSession": false,
                        "promptCapabilities": {"embeddedContext": true}}
                }),
            ),
            "session/new" => {
                let cwd = params["cwd"]
                    .as_str()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                let opts = RunOpts::default();
                match Agent::new(&opts, zcore::NAME, &cwd) {
                    Ok(agent) => {
                        let sid = agent.session.id.clone();
                        cwds.insert(sid.clone(), cwd);
                        agents.insert(sid.clone(), agent);
                        result(&mut w, &id, json!({"sessionId": sid}));
                    }
                    Err(e) => err(&mut w, &id, -32603, &format!("{e:#}")),
                }
            }
            "session/prompt" => {
                let sid = params["sessionId"].as_str().unwrap_or("");
                let prompt = params["prompt"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|b| b["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                let Some(agent) = agents.get_mut(sid) else {
                    err(&mut w, &id, -32602, "unknown sessionId");
                    continue;
                };
                let sid_owned = sid.to_string();
                let mut emit = |ev: &Event| {
                    if let Some(n) = update_for(&sid_owned, ev) {
                        send(&mut w, &n);
                    }
                };
                agent.send_user(&prompt, &mut emit);
                let max_turns = agent.opts.max_turns.unwrap_or(16);
                let mut reason = "end_turn";
                'turns: for _ in 0..max_turns {
                    match agent.turn(&mut emit) {
                        Ok(true) => break 'turns,
                        Ok(false) => {}
                        Err(_) => {
                            reason = "refusal";
                            break 'turns;
                        }
                    }
                    let calls: Vec<(String, String, Value)> = agent
                        .messages
                        .last()
                        .map(|m| {
                            m.content
                                .iter()
                                .filter_map(|b| match b {
                                    zproto::Block::ToolUse { id, name, input } => {
                                        Some((id.clone(), name.clone(), input.clone()))
                                    }
                                    _ => None,
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    if calls.is_empty() {
                        break 'turns;
                    }
                    for (tid, name, input) in calls {
                        agent.exec_tool(&tid, &name, &input, &mut emit);
                    }
                }
                if reason != "refusal" && max_turns == 0 {
                    reason = "max_turns";
                }
                result(&mut w, &id, json!({"stopReason": reason}));
            }
            "session/cancel" => {
                // Notification (may carry no id) — single-threaded loop means a
                // cancel can only land between prompts; nothing else to do.
            }
            _ => {
                if !id.is_null() {
                    err(&mut w, &id, -32601, "method not found");
                }
            }
        }
    }
    zproto::exit::DONE
}
