//! Line-delimited JSON-RPC client over a child process's stdio — the
//! transport `cua-driver mcp` speaks (MCP stdio = one JSON-RPC message per
//! line). One client owns one driver process; dropping it kills the child.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

use crate::ComputerError;

/// Long enough for heavy calls — a full UIA walk on an Electron tree can
/// take several seconds, verify_state waits for stability, and invoke_menu
/// can block on a modal. Timed-out calls get an MCP `notifications/cancelled`
/// so the driver can abort in-flight work instead of landing a late effect.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const PING_TIMEOUT: Duration = Duration::from_secs(10);
/// A single tools/call payload or response can carry a screenshot or a large
/// UIA tree — bound it well above the 256KB HTTP bridge cap but below
/// "unbounded memory" (semantic trees are capped by max_elements anyway).
const MAX_LINE: usize = 48 * 1024 * 1024;

type Pending = Arc<Mutex<BTreeMap<u64, oneshot::Sender<Result<Value, ComputerError>>>>>;

pub struct McpClient {
    /// Serializes writes so concurrent calls can't interleave a line. A Tokio
    /// mutex — the guard is held across `write_all` awaits and must be Send.
    stdin: tokio::sync::Mutex<ChildStdin>,
    pending: Pending,
    next_id: AtomicU64,
    child: Mutex<Child>,
    /// Advertised tools, captured after `tools/list` during handshake.
    tools: Mutex<Vec<String>>,
}

impl McpClient {
    /// Spawn `binary mcp` with `env`, run the MCP handshake
    /// (`initialize` → `notifications/initialized` → `tools/list`) and return
    /// a ready client. The child is killed when the client drops.
    pub async fn spawn(
        binary: &Path,
        env: &[(String, String)],
        extra_args: &[&str],
    ) -> Result<Self, ComputerError> {
        let mut cmd = Command::new(binary);
        cmd.arg("mcp")
            .args(extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // No console window may pop up beside the app on Windows.
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().map_err(|e| {
            ComputerError::Unavailable(format!("could not start {}: {e}", binary.display()))
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ComputerError::Internal("driver stdout missing".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ComputerError::Internal("driver stderr missing".into()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ComputerError::Internal("driver stdin missing".into()))?;

        let pending: Pending = Arc::new(Mutex::new(BTreeMap::new()));
        let client = Self {
            stdin: tokio::sync::Mutex::new(stdin),
            pending: pending.clone(),
            next_id: AtomicU64::new(1),
            child: Mutex::new(child),
            tools: Mutex::new(Vec::new()),
        };

        // Response reader: resolves pending calls by id; drops notifications
        // and any server→client requests (the driver does not issue them).
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let line = line.trim();
                        if line.is_empty() || line.len() > MAX_LINE {
                            continue;
                        }
                        let Ok(message) = serde_json::from_str::<Value>(line) else {
                            continue;
                        };
                        // Server→client requests/notifications carry `method`
                        // — they are NOT replies; never let one hijack a
                        // pending id.
                        if message.get("method").is_some() {
                            continue;
                        }
                        let Some(id) = message.get("id").and_then(Value::as_u64) else {
                            continue;
                        };
                        let reply = if let Some(error) = message.get("error") {
                            let text = error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("driver error");
                            Err(ComputerError::from_tool_error(text))
                        } else {
                            Ok(message.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let mut map = pending.lock().unwrap_or_else(|p| p.into_inner());
                        if let Some(tx) = map.remove(&id) {
                            let _ = tx.send(reply);
                        }
                    }
                    Ok(None) | Err(_) => {
                        // EOF: fail every pending call so callers don't hang.
                        let mut map = pending.lock().unwrap_or_else(|p| p.into_inner());
                        for (_, tx) in std::mem::take(&mut *map) {
                            let _ = tx.send(Err(ComputerError::Unavailable(
                                "the computer driver exited".into(),
                            )));
                        }
                        return;
                    }
                }
            }
        });
        // Driver stderr is diagnostics, not protocol — log at warn. It never
        // carries screen content (the driver does not print observations).
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if !line.is_empty() {
                    tracing::warn!(target: "zeron_computer::driver", "{line}");
                }
            }
        });

        client.handshake().await?;
        Ok(client)
    }

    async fn handshake(&self) -> Result<(), ComputerError> {
        self.call_inner(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "cosmos", "title": "Cosmos Computer", "version": env!("CARGO_PKG_VERSION")},
            }),
            PING_TIMEOUT,
        )
        .await?;
        self.notify("notifications/initialized", json!({})).await?;
        let result = self
            .call_inner("tools/list", json!({}), PING_TIMEOUT)
            .await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|t| t.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if tools.is_empty() {
            return Err(ComputerError::Unavailable(
                "the computer driver exposed no tools".into(),
            ));
        }
        *self.tools.lock().unwrap_or_else(|p| p.into_inner()) = tools;
        Ok(())
    }

    /// The driver process id — used to exclude it from "frontmost" picks.
    pub fn child_pid(&self) -> Option<u32> {
        self.child.lock().unwrap_or_else(|p| p.into_inner()).id()
    }

    /// Names of the driver's advertised tools (post-handshake).
    pub fn tools(&self) -> Vec<String> {
        self.tools.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub fn has_tool(&self, name: &str) -> bool {
        self.tools().iter().any(|t| t == name)
    }

    /// `tools/call` shorthand — returns the CallTool result payload and maps
    /// `isError: true` into a classified [`ComputerError`].
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, ComputerError> {
        let result = self
            .call_inner(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
                CALL_TIMEOUT,
            )
            .await?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            let text = result
                .get("content")
                .and_then(Value::as_array)
                .and_then(|content| {
                    content
                        .iter()
                        .find_map(|c| c.get("text").and_then(Value::as_str))
                })
                .unwrap_or("the driver refused the call");
            return Err(ComputerError::from_tool_error(text));
        }
        Ok(result)
    }

    async fn call_inner(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ComputerError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            map.insert(id, tx);
        }
        let line = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string();
        // Two writes then a flush, all while holding the guard so lines from
        // concurrent calls can't interleave.
        let write = {
            let mut stdin = self.stdin.lock().await;
            async move {
                stdin.write_all(line.as_bytes()).await?;
                stdin.write_all(b"\n").await?;
                stdin.flush().await
            }
            .await
        };
        if let Err(e) = write {
            self.pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            return Err(ComputerError::Unavailable(format!(
                "could not write to the computer driver: {e}"
            )));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(ComputerError::Unavailable(
                "the computer driver dropped the reply".into(),
            )),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&id);
                // Tell the driver to abort the in-flight call — a late click
                // landing after the caller gave up would be a ghost action.
                let cancel = json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/cancelled",
                    "params": {"requestId": id, "reason": "timeout"},
                })
                .to_string();
                let mut stdin = self.stdin.lock().await;
                let _ = async move {
                    stdin.write_all(cancel.as_bytes()).await?;
                    stdin.write_all(b"\n").await?;
                    stdin.flush().await
                }
                .await;
                Err(ComputerError::Unavailable(format!(
                    "the computer driver did not answer `{method}` in time (cancel sent)"
                )))
            }
        }
    }

    /// Notifications carry no id and expect no reply.
    pub async fn notify(&self, method: &str, params: Value) -> Result<(), ComputerError> {
        let line = json!({"jsonrpc":"2.0","method":method,"params":params}).to_string();
        let mut stdin = self.stdin.lock().await;
        async move {
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await
        }
        .await
        .map_err(|e: std::io::Error| {
            ComputerError::Unavailable(format!("could not write to the computer driver: {e}"))
        })
    }
}
