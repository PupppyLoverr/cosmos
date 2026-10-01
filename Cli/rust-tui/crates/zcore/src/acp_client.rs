//! Subscription plugins over the Agent Client Protocol.
//!
//! Cursor, Devin and Codex each ship an ACP agent with their CLI. cosmostui
//! spawns the one the operator installed and signed in to, sends the task
//! with `session/prompt`, and streams what comes back. The vendor's own
//! agent (its models, tools and sign-in) does the work and its plan pays for
//! it; Cosmos renders it, asks the operator about permissions and keeps the
//! session log. Nothing is spoofed and no Cosmos-held key is involved.
//!
//! JSON-RPC 2.0, one message per line on stdio:
//! `initialize` → `session/new` → `session/prompt` (with `session/update`
//! notifications streaming) → `session/cancel` on interrupt. The agent may
//! call back with `session/request_permission`; we advertise no fs/terminal
//! capabilities, so other callbacks are answered "method not found".

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// One installable subscription agent.
pub struct Plugin {
    pub id: &'static str,
    pub name: &'static str,
    /// executables to look for on PATH, first match wins
    pub bins: &'static [&'static str],
    /// arguments that start the ACP server
    pub args: &'static [&'static str],
    /// the vendor's own sign-in command (run with the terminal attached)
    pub login: &'static [&'static str],
    pub install: &'static str,
}

pub const PLUGINS: &[Plugin] = &[
    Plugin {
        id: "cursor",
        name: "Cursor",
        bins: &["agent", "cursor-agent"],
        args: &["acp"],
        login: &["agent", "login"],
        install: "install the Cursor CLI (https://cursor.com/docs/cli/installation), then run `agent login`",
    },
    Plugin {
        id: "devin",
        name: "Devin",
        bins: &["devin"],
        args: &["acp"],
        login: &["devin", "auth", "login"],
        install: "install the Devin CLI (Windows: irm https://static.devin.ai/cli/setup.ps1 | iex), then run `devin auth login`",
    },
    Plugin {
        id: "codex",
        name: "Codex",
        bins: &["codex-acp"],
        args: &[],
        login: &["codex", "login"],
        install: "install the Codex ACP adapter (npm i -g @zed-industries/codex-acp) and the Codex CLI, then run `codex login`",
    },
];

pub fn plugin(id: &str) -> Option<&'static Plugin> {
    PLUGINS.iter().find(|p| p.id == id)
}

/// `acp://<id>` — how an ACP provider is addressed in `ProviderCfg.base_url`.
pub fn url(id: &str) -> String {
    format!("acp://{id}")
}

pub fn id_of(base_url: &str) -> Option<&str> {
    base_url.strip_prefix("acp://")
}

/// Find `bin` on PATH (with PATHEXT on Windows, so npm `.cmd` shims count).
pub fn which(bin: &str) -> Option<PathBuf> {
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.to_ascii_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let p = dir.join(format!("{bin}{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// The plugin's executable, if installed.
pub fn executable(p: &Plugin) -> Option<PathBuf> {
    p.bins.iter().find_map(|b| which(b))
}

pub fn installed(id: &str) -> bool {
    plugin(id).and_then(executable).is_some()
}

/// One status line for menus and `cosmostui auth`.
pub fn status(id: &str) -> String {
    match plugin(id) {
        Some(p) => match executable(p) {
            Some(exe) => format!("{} agent at {} — signs in with `{}`", p.name, exe.display(), p.login.join(" ")),
            None => format!("not installed — {}", p.install),
        },
        None => format!("unknown plugin {id}"),
    }
}

/// Run the vendor's own sign-in with this terminal attached.
pub fn login(id: &str) -> Result<()> {
    let p = plugin(id).with_context(|| format!("unknown plugin {id}"))?;
    let exe = which(p.login[0]).with_context(|| format!("{} not found — {}", p.login[0], p.install))?;
    let st = command(&exe).args(&p.login[1..]).status().with_context(|| format!("running {}", p.login.join(" ")))?;
    if !st.success() {
        bail!("`{}` exited with {st}", p.login.join(" "));
    }
    Ok(())
}

/// `cmd /C` for batch shims (npm installs `.cmd` launchers on Windows).
fn command(exe: &Path) -> Command {
    let ext = exe.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if cfg!(windows) && (ext == "cmd" || ext == "bat") {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(exe);
        c
    } else {
        Command::new(exe)
    }
}

/// What the agent streamed during one prompt.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    Text(String),
    /// a vendor tool started or changed: (id, kind, title, status, raw input, output text)
    Tool { id: String, kind: String, title: String, status: String, input: Value, output: String },
}

/// The operator's answer to a permission request.
pub type Decide<'a> = &'a mut dyn FnMut(&str, &Value) -> bool;

pub struct AcpSession {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    pub session_id: String,
    pub plugin_id: String,
    pub agent_name: String,
    /// set when a model was asked for and the agent didn't accept it
    pub model_note: Option<String>,
    next_id: u64,
}

impl Drop for AcpSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const SETUP_TIMEOUT: Duration = Duration::from_secs(90);

impl AcpSession {
    /// Spawn the plugin, `initialize`, `session/new`. `model` other than
    /// "default" is requested with `session/set_model` (best effort).
    pub fn start(plugin_id: &str, cwd: &Path, model: &str) -> Result<AcpSession> {
        let p = plugin(plugin_id).with_context(|| format!("unknown ACP plugin {plugin_id}"))?;
        let exe = executable(p).with_context(|| format!("{} is not installed — {}", p.name, p.install))?;
        let mut cmd = command(&exe);
        cmd.args(p.args);
        Self::spawn(cmd, p.id, cwd, model)
    }

    /// Spawn an arbitrary ACP agent command (tests drive a fake one).
    pub fn spawn(mut cmd: Command, plugin_id: &str, cwd: &Path, model: &str) -> Result<AcpSession> {
        let mut child = cmd
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting the {plugin_id} ACP agent"))?;
        let stdin = child.stdin.take().context("agent stdin")?;
        let stdout = child.stdout.take().context("agent stdout")?;
        let mut err_pipe = child.stderr.take().context("agent stderr")?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err_pipe.read_to_end(&mut buf);
            let text = String::from_utf8_lossy(&buf).to_string();
            if let Ok(mut t) = tail.lock() {
                for l in text.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev() {
                    t.push_back(l.to_string());
                }
            }
        });
        let mut s = AcpSession {
            child,
            stdin,
            rx,
            stderr,
            session_id: String::new(),
            plugin_id: plugin_id.into(),
            agent_name: plugin_id.into(),
            model_note: None,
            next_id: 0,
        };
        let init = s.call_simple(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
                "clientInfo": {"name": crate::NAME, "title": "Cosmos", "version": env!("CARGO_PKG_VERSION")},
            }),
        )?;
        if let Some(n) = init["agentInfo"]["title"].as_str().or(init["agentInfo"]["name"].as_str()) {
            s.agent_name = n.to_string();
        }
        let created = s.call_simple(
            "session/new",
            json!({"cwd": cwd.display().to_string(), "mcpServers": []}),
        );
        let created = match created {
            Ok(v) => v,
            Err(e) => {
                let login = plugin(plugin_id).map(|p| p.login.join(" ")).unwrap_or_default();
                let msg = format!("{e:#}");
                if msg.to_lowercase().contains("auth") || msg.to_lowercase().contains("login") || msg.to_lowercase().contains("sign") {
                    bail!("{msg} — sign in with `{login}` and try again");
                }
                return Err(e);
            }
        };
        s.session_id = created["sessionId"].as_str().context("session/new returned no sessionId")?.to_string();
        if !model.is_empty() && model != "default" {
            if let Err(e) = s.call_simple("session/set_model", json!({"sessionId": s.session_id, "modelId": model})) {
                s.model_note = Some(format!("{} kept its own model ({e:#})", s.agent_name));
            }
        }
        Ok(s)
    }

    fn stderr_tail(&self) -> String {
        self.stderr.lock().map(|t| t.iter().cloned().collect::<Vec<_>>().join(" | ")).unwrap_or_default()
    }

    fn send(&mut self, v: &Value) -> Result<()> {
        writeln!(self.stdin, "{v}").context("writing to the ACP agent")?;
        self.stdin.flush().context("writing to the ACP agent")
    }

    /// A request with no streaming expected (setup calls).
    fn call_simple(&mut self, method: &str, params: Value) -> Result<Value> {
        let deadline = Instant::now() + SETUP_TIMEOUT;
        self.call(method, params, &mut |_| {}, &mut |_, _| false, None, Some(deadline))
    }

    /// Send a request and pump messages until its response arrives.
    fn call(
        &mut self,
        method: &str,
        params: Value,
        on_update: &mut dyn FnMut(&Value),
        decide: Decide,
        interrupt: Option<&AtomicBool>,
        deadline: Option<Instant>,
    ) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let mut cancelled = false;
        loop {
            if let Some(flag) = interrupt {
                if !cancelled && flag.load(Ordering::Relaxed) {
                    cancelled = true;
                    let sid = self.session_id.clone();
                    self.send(&json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": sid}}))?;
                }
            }
            if deadline.is_some_and(|d| Instant::now() > d) {
                bail!("{} did not answer {method} in time", self.agent_name);
            }
            let line = match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(l) => l,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(Duration::from_millis(50)); // let stderr drain
                    bail!("{} exited during {method}: {}", self.agent_name, self.stderr_tail());
                }
            };
            // agents may log to stdout — only JSON-RPC lines count
            let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else { continue };
            let has_method = msg.get("method").and_then(Value::as_str);
            match (msg.get("id"), has_method) {
                (Some(rid), None) if rid.as_u64() == Some(id) => {
                    if let Some(e) = msg.get("error") {
                        bail!("{} {method}: {}", self.agent_name, e["message"].as_str().unwrap_or(&e.to_string()));
                    }
                    return Ok(msg["result"].clone());
                }
                (Some(rid), Some(m)) => {
                    // a request from the agent
                    let reply = match m {
                        "session/request_permission" => {
                            let params = &msg["params"];
                            let allow = decide(params["toolCall"]["title"].as_str().unwrap_or("tool"), params);
                            json!({"outcome": choose(params, allow)})
                        }
                        _ => {
                            let rid = rid.clone();
                            self.send(&json!({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": format!("{m} is not supported by this client")}}))?;
                            continue;
                        }
                    };
                    let rid = rid.clone();
                    self.send(&json!({"jsonrpc": "2.0", "id": rid, "result": reply}))?;
                }
                (None, Some("session/update")) => on_update(&msg["params"]["update"]),
                _ => {}
            }
        }
    }

    /// Send one prompt; stream updates; return the stop reason.
    pub fn prompt(
        &mut self,
        text: &str,
        on: &mut dyn FnMut(Update),
        decide: Decide,
        interrupt: Option<&AtomicBool>,
    ) -> Result<String> {
        let sid = self.session_id.clone();
        let mut handle = |u: &Value| {
            if let Some(up) = parse_update(u) {
                on(up);
            }
        };
        let r = self.call(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": text}]}),
            &mut handle,
            decide,
            interrupt,
            None,
        )?;
        Ok(r["stopReason"].as_str().unwrap_or("end_turn").to_string())
    }
}

/// Pick the option matching the decision (`allow_once` / `reject_once`
/// preferred), or report the request cancelled when none fits.
fn choose(params: &Value, allow: bool) -> Value {
    let opts = params["options"].as_array().cloned().unwrap_or_default();
    let want: &[&str] = if allow { &["allow_once", "allow_always"] } else { &["reject_once", "reject_always"] };
    for k in want {
        if let Some(o) = opts.iter().find(|o| o["kind"].as_str() == Some(k)) {
            return json!({"outcome": "selected", "optionId": o["optionId"]});
        }
    }
    json!({"outcome": "cancelled"})
}

fn content_text(c: &Value) -> String {
    let mut out = Vec::new();
    for item in c.as_array().cloned().unwrap_or_default() {
        match item["type"].as_str() {
            Some("content") => {
                if let Some(t) = item["content"]["text"].as_str() {
                    out.push(t.to_string());
                }
            }
            Some("diff") => out.push(format!("edited {}", item["path"].as_str().unwrap_or("?"))),
            Some("terminal") => out.push("ran a command".into()),
            _ => {}
        }
    }
    out.join("\n")
}

/// `session/update` → what the UI and the log need.
pub fn parse_update(u: &Value) -> Option<Update> {
    match u["sessionUpdate"].as_str()? {
        "agent_message_chunk" => u["content"]["text"].as_str().map(|t| Update::Text(t.to_string())),
        "tool_call" | "tool_call_update" => Some(Update::Tool {
            id: u["toolCallId"].as_str().unwrap_or("").to_string(),
            kind: u["kind"].as_str().unwrap_or("").to_string(),
            title: u["title"].as_str().unwrap_or("").to_string(),
            status: u["status"].as_str().unwrap_or("").to_string(),
            input: u.get("rawInput").cloned().unwrap_or(Value::Null),
            output: content_text(&u["content"]),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_updates_and_permission_choices() {
        let t = parse_update(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "po"}}));
        assert_eq!(t, Some(Update::Text("po".into())));
        let u = parse_update(&json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
            "content": [{"type": "content", "content": {"type": "text", "text": "3 files"}}, {"type": "diff", "path": "a.rs"}]}));
        let Some(Update::Tool { id, status, output, .. }) = u else { panic!() };
        assert_eq!((id.as_str(), status.as_str(), output.as_str()), ("c1", "completed", "3 files\nedited a.rs"));
        assert!(parse_update(&json!({"sessionUpdate": "plan", "entries": []})).is_none());
        let p = json!({"options": [{"optionId": "a", "kind": "allow_always"}, {"optionId": "r", "kind": "reject_once"}]});
        assert_eq!(choose(&p, true), json!({"outcome": "selected", "optionId": "a"}));
        assert_eq!(choose(&p, false), json!({"outcome": "selected", "optionId": "r"}));
        assert_eq!(choose(&json!({"options": []}), true), json!({"outcome": "cancelled"}));
    }

    #[test]
    fn plugins_have_ids_and_urls_round_trip() {
        for p in PLUGINS {
            assert_eq!(id_of(&url(p.id)), Some(p.id));
            assert!(!p.bins.is_empty() && !p.login.is_empty());
        }
        assert!(plugin("nope").is_none());
        assert!(which("definitely-not-a-real-binary-xyz").is_none());
    }
}
