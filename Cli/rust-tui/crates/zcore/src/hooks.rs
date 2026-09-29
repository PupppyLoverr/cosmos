//! §11 hooks — `[[hooks]] event matcher command`. The command gets the event
//! JSON on stdin; exit 0 continues, exit 2 blocks (stderr becomes the tool
//! result), anything else logs `error` and continues.

use std::io::Write;

use serde_json::{json, Value};

use crate::config::Config;

fn matches(matcher: &str, name: &str) -> bool {
    matcher == "*" || matcher.is_empty() || matcher == name
}

/// Run every hook matching `event`+`tool`. Returns Some(stderr) when a hook
/// blocked (exit 2).
pub fn run(
    cfg: &Config,
    event: &str,
    tool: &str,
    payload: &Value,
    session: &str,
) -> Option<String> {
    let ev = json!({"event": event, "tool": tool, "data": payload, "session": session});
    for h in &cfg.hooks {
        if h.event != event || !matches(&h.matcher, tool) {
            continue;
        }
        let mut command = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/c", &h.command]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", &h.command]);
            c
        };
        let spawned = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn();
        let Ok(mut child) = spawned else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(ev.to_string().as_bytes());
        }
        let Ok(out) = child.wait_with_output() else {
            continue;
        };
        match out.status.code() {
            Some(0) => {}
            Some(2) => {
                return Some(String::from_utf8_lossy(&out.stderr).to_string());
            }
            _ => {
                eprintln!("hook '{}' exited {} — continuing", h.command, out.status);
            }
        }
    }
    None
}
