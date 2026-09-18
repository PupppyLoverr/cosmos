//! Upstream MCP connections — the two transports the gateway merges.
//!
//! [`HttpUpstream`] speaks streamable-HTTP MCP: one POST endpoint carrying
//! `initialize` → `notifications/initialized` → `tools/list` → `tools/call`,
//! static headers (where the Composio key rides), `Mcp-Session-Id` capture,
//! and both `application/json` and `text/event-stream` response shapes.
//!
//! [`StdioUpstream`] is a line-delimited JSON-RPC child (MCP stdio = one
//! message per line), adapted from Cosmos Computer's driver client — one
//! client owns one process; dropping it kills the child.

use std::collections::BTreeMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::ToolError;

/// Meta-tool executions (COMPOSIO_MULTI_EXECUTE_TOOL runs ≤50 calls) and
/// slow stdio servers get real headroom — same order as the computer
/// endpoint's 90s cap, plus margin so the gateway's own timeout fires second.
const CALL_TIMEOUT: Duration = Duration::from_secs(110);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
/// A single stdio line can carry a large tool schema — bounded, not unbounded.
const MAX_LINE: usize = 32 * 1024 * 1024;

/// One connected upstream. `list_tools` is the handshake-captured
/// `tools/list`; `call_tool` returns the CallTool result payload verbatim
/// (content blocks untouched so images/audio pass through).
#[async_trait::async_trait]
pub trait Upstream: Send + Sync + 'static {
    async fn list_tools(&self) -> Result<Vec<Value>, ToolError>;
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, ToolError>;
    /// Downcast seam — Composio management calls need the concrete
    /// `HttpUpstream` (meta-tool helpers live off the trait).
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Extract the JSON payload a `tools/call` result carries: MCP
/// `structuredContent` when present, else the first text block parsed as
/// JSON, else the raw text as a string. The Composio meta-tools answer in
/// `structuredContent`; text is the fallback for every other server.
pub fn result_payload(result: &Value) -> Value {
    if let Some(structured) = result.get("structuredContent") {
        return structured.clone();
    }
    if let Some(text) = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|content| {
            content
                .iter()
                .find_map(|c| c.get("text").and_then(Value::as_str))
        })
    {
        return serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()));
    }
    result.clone()
}

/// The first text block of a CallTool result, when there is one.
pub fn result_text(result: &Value) -> Option<String> {
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

/// Map a CallTool result into an error when it carries `isError: true`.
pub fn refusal(result: &Value) -> Option<ToolError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        let text = result_text(result).unwrap_or_else(|| "the tool refused the call".into());
        Some(ToolError::from_tool_error(&text))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Streamable HTTP
// ---------------------------------------------------------------------------

/// A streamable-HTTP MCP endpoint — Composio Connect and arbitrary remote
/// servers share this path; only the headers differ.
pub struct HttpUpstream {
    client: reqwest::Client,
    url: String,
    /// Static config headers (secrets live here, never in logs or argv).
    headers: reqwest::header::HeaderMap,
    session_id: Mutex<Option<String>>,
    /// Advertised tools, captured during the handshake.
    tools: Mutex<Vec<Value>>,
}

impl HttpUpstream {
    /// Handshake (`initialize` → `notifications/initialized` → `tools/list`)
    /// and return a ready upstream. `label` names the provider in errors.
    pub async fn connect(
        url: &str,
        headers: &BTreeMap<String, String>,
        label: &str,
    ) -> Result<Self, ToolError> {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in headers {
            let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| ToolError::InvalidArgs(format!("bad header name '{name}'")))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| ToolError::InvalidArgs(format!("bad value for header '{name}'")))?;
            map.insert(name, value);
        }
        let upstream = Self {
            client: reqwest::Client::new(),
            url: url.to_string(),
            headers: map,
            session_id: Mutex::new(None),
            tools: Mutex::new(Vec::new()),
        };
        upstream.handshake(label).await?;
        Ok(upstream)
    }

    async fn handshake(&self, label: &str) -> Result<(), ToolError> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "cosmos-tools", "title": "Cosmos Tools", "version": env!("CARGO_PKG_VERSION")},
            }),
            HANDSHAKE_TIMEOUT,
        )
        .await?;
        self.notify("notifications/initialized", json!({})).await?;
        let result = self
            .request("tools/list", json!({}), HANDSHAKE_TIMEOUT)
            .await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if tools.is_empty() {
            return Err(ToolError::Unavailable(format!("{label} exposed no tools")));
        }
        *self.tools.lock().unwrap_or_else(|p| p.into_inner()) = tools;
        Ok(())
    }

    /// One JSON-RPC request → its `result`. Both streamable-HTTP response
    /// shapes are accepted: a plain JSON body, or an SSE stream whose first
    /// `data:` frame carrying our id wins.
    async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ToolError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let body = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let send = async {
            let mut request = self
                .client
                .post(&self.url)
                .headers(self.headers.clone())
                .header(
                    reqwest::header::ACCEPT,
                    "application/json, text/event-stream",
                )
                .json(&body);
            if let Some(session) = self.session_id.lock().unwrap().clone() {
                request = request.header("mcp-session-id", session);
            }
            let response = request.send().await.map_err(|e| {
                ToolError::Unavailable(format!("could not reach the tool server: {e}"))
            })?;
            if let Some(session) = response
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
            {
                *self.session_id.lock().unwrap() = Some(session.to_string());
            }
            let status = response.status();
            if status == reqwest::StatusCode::NOT_FOUND {
                // A lapsed session id: clear it so the retry re-initializes.
                *self.session_id.lock().unwrap() = None;
                return Err(ToolError::Unavailable(
                    "the tool server dropped the session".into(),
                ));
            }
            if !status.is_success() {
                let text = response.text().await.unwrap_or_default();
                return Err(ToolError::Unavailable(format!(
                    "the tool server answered {status}: {}",
                    text.lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(160)
                        .collect::<String>()
                )));
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let body = response.bytes().await.map_err(|e| {
                ToolError::Unavailable(format!("could not read the tool server reply: {e}"))
            })?;
            let message = if content_type.contains("text/event-stream") {
                parse_sse(&body, id)
            } else {
                parse_json_reply(&body, id)
            };
            let message = message.ok_or_else(|| {
                ToolError::Unavailable("the tool server reply held no response".into())
            })?;
            if let Some(error) = message.get("error") {
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("tool server error");
                return Err(ToolError::from_tool_error(text));
            }
            Ok(message.get("result").cloned().unwrap_or(Value::Null))
        };
        match tokio::time::timeout(timeout, send).await {
            Ok(result) => result,
            Err(_) => Err(ToolError::Unavailable(format!(
                "the tool server did not answer `{method}` in time"
            ))),
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), ToolError> {
        let body = json!({"jsonrpc":"2.0","method":method,"params":params});
        let mut request = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(&body);
        if let Some(session) = self.session_id.lock().unwrap().clone() {
            request = request.header("mcp-session-id", session);
        }
        let response = request
            .send()
            .await
            .map_err(|e| ToolError::Unavailable(format!("could not reach the tool server: {e}")))?;
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *self.session_id.lock().unwrap() = Some(session.to_string());
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Upstream for HttpUpstream {
    async fn list_tools(&self) -> Result<Vec<Value>, ToolError> {
        Ok(self.tools.lock().unwrap_or_else(|p| p.into_inner()).clone())
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, ToolError> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
                CALL_TIMEOUT,
            )
            .await?;
        if let Some(error) = refusal(&result) {
            return Err(error);
        }
        Ok(result)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Pick the JSON-RPC reply to `id` out of a plain JSON body (single object
/// or batch array).
fn parse_json_reply(body: &[u8], id: u64) -> Option<Value> {
    let value: Value = serde_json::from_slice(body).ok()?;
    match value {
        Value::Array(messages) => messages
            .into_iter()
            .find(|m| m.get("id").and_then(Value::as_u64) == Some(id)),
        single => Some(single),
    }
}

/// Scan SSE frames (`data: <json>` lines, blank-line delimited) for the
/// reply to `id`.
fn parse_sse(body: &[u8], id: u64) -> Option<Value> {
    let text = String::from_utf8_lossy(body);
    let mut found = None;
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let Ok(message) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        if message.get("id").and_then(Value::as_u64) == Some(id) {
            found = Some(message);
        }
    }
    found
}

// ---------------------------------------------------------------------------
// Stdio (adapted from Cosmos Computer's driver client)
// ---------------------------------------------------------------------------

type Pending = Arc<Mutex<BTreeMap<u64, tokio::sync::oneshot::Sender<Result<Value, ToolError>>>>>;

/// A spawned stdio MCP server — one JSON-RPC message per stdout line.
pub struct StdioUpstream {
    stdin: tokio::sync::Mutex<tokio::process::ChildStdin>,
    pending: Pending,
    next_id: AtomicU64,
    #[allow(dead_code)]
    child: Mutex<tokio::process::Child>,
    tools: Mutex<Vec<Value>>,
}

impl StdioUpstream {
    /// Spawn `command args…` with `env`, run the MCP handshake and return a
    /// ready upstream. The child is killed when the upstream drops.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<Self, ToolError> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
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
        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::Unavailable(format!("could not start '{command}': {e}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::Internal("server stdout missing".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::Internal("server stderr missing".into()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ToolError::Internal("server stdin missing".into()))?;

        let pending: Pending = Arc::new(Mutex::new(BTreeMap::new()));
        let upstream = Self {
            stdin: tokio::sync::Mutex::new(stdin),
            pending: pending.clone(),
            next_id: AtomicU64::new(1),
            child: Mutex::new(child),
            tools: Mutex::new(Vec::new()),
        };

        // Response reader: resolves pending calls by id; drops notifications
        // and any server→client requests.
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stdout).lines();
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
                        let Some(id) = message.get("id").and_then(Value::as_u64) else {
                            continue;
                        };
                        let reply = if let Some(error) = message.get("error") {
                            let text = error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("tool server error");
                            Err(ToolError::from_tool_error(text))
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
                            let _ = tx
                                .send(Err(ToolError::Unavailable("the tool server exited".into())));
                        }
                        return;
                    }
                }
            }
        });
        // Server stderr is diagnostics, not protocol — log at warn.
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if !line.is_empty() {
                    tracing::warn!(target: "zeron_tools::upstream", "{line}");
                }
            }
        });

        upstream.handshake(command).await?;
        Ok(upstream)
    }

    async fn handshake(&self, command: &str) -> Result<(), ToolError> {
        self.call_inner(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "cosmos-tools", "title": "Cosmos Tools", "version": env!("CARGO_PKG_VERSION")},
            }),
            HANDSHAKE_TIMEOUT,
        )
        .await?;
        self.notify("notifications/initialized", json!({})).await?;
        let result = self
            .call_inner("tools/list", json!({}), HANDSHAKE_TIMEOUT)
            .await?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if tools.is_empty() {
            return Err(ToolError::Unavailable(format!(
                "'{command}' exposed no tools"
            )));
        }
        *self.tools.lock().unwrap_or_else(|p| p.into_inner()) = tools;
        Ok(())
    }

    async fn call_inner(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ToolError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut map = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            map.insert(id, tx);
        }
        let line = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string();
        let write = {
            use tokio::io::AsyncWriteExt;
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
            return Err(ToolError::Unavailable(format!(
                "could not write to the tool server: {e}"
            )));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(ToolError::Unavailable(
                "the tool server dropped the reply".into(),
            )),
            Err(_) => {
                self.pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&id);
                Err(ToolError::Unavailable(format!(
                    "the tool server did not answer `{method}` in time"
                )))
            }
        }
    }

    /// Notifications carry no id and expect no reply.
    async fn notify(&self, method: &str, params: Value) -> Result<(), ToolError> {
        use tokio::io::AsyncWriteExt;
        let line = json!({"jsonrpc":"2.0","method":method,"params":params}).to_string();
        let mut stdin = self.stdin.lock().await;
        async move {
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await
        }
        .await
        .map_err(|e: std::io::Error| {
            ToolError::Unavailable(format!("could not write to the tool server: {e}"))
        })
    }
}

#[async_trait::async_trait]
impl Upstream for StdioUpstream {
    async fn list_tools(&self) -> Result<Vec<Value>, ToolError> {
        Ok(self.tools.lock().unwrap_or_else(|p| p.into_inner()).clone())
    }

    async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, ToolError> {
        let result = self
            .call_inner(
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
                CALL_TIMEOUT,
            )
            .await?;
        if let Some(error) = refusal(&result) {
            return Err(error);
        }
        Ok(result)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
