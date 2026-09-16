//! A loopback HTTP endpoint that lets the chat's own agent drive the
//! embedded browser tabs. It speaks the MCP streamable-HTTP subset the CLI
//! harnesses consume over plain JSON POSTs: `initialize`, `ping`,
//! `notifications/*`, `tools/list`, and `tools/call`.
//!
//! Requests arrive on a Tokio task (no app access), then hop a channel into
//! the shell's foreground dispatch — `WeakEntity`s and `&mut Window` never
//! leave the UI thread. The URL embeds a per-window token in the path, so a
//! stray local process cannot steer the user's browser, and only local
//! chats get the endpoint stamped onto their run request.
use gpui::AppContext;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

/// `RunRequest.model_options` key the harness reads the bridge endpoint
/// from. Kept namespaced so a model's own options can never collide.
pub const MODEL_OPTIONS_KEY: &str = zeron_proto::BROWSER_BRIDGE_OPTION;

/// What a harness receives — the full MCP endpoint URL (token in the path).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct BridgeInfo {
    pub url: String,
}

/// One tool invocation routed into the UI.
pub struct BridgeRequest {
    pub tool: String,
    pub args: Value,
    pub reply: oneshot::Sender<Result<BridgeReply, String>>,
}

/// Tool output: text content, or a PNG the agent receives as an MCP image.
pub struct BridgeReply {
    pub text: Option<String>,
    pub image: Option<Vec<u8>>,
}

impl BridgeReply {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            image: None,
        }
    }
}

/// Owns the listener task; dropping it stops the server.
pub struct BrowserBridge {
    pub info: BridgeInfo,
    _serve: gpui::Task<Result<(), tokio::task::JoinError>>,
}

const MAX_HEADER: usize = 16 * 1024;
const MAX_BODY: usize = 256 * 1024;
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

impl BrowserBridge {
    /// Binds 127.0.0.1 on an ephemeral port and starts serving. Returns the
    /// bridge plus the request queue the shell drains on the UI thread.
    /// Returns None where `gpui_tokio` was never initialized (test harnesses
    /// that build a Shell without `crate::init`) — `Tokio::spawn` panics on
    /// the missing global and `GlobalTokio` is a private type, so the panic
    /// is caught rather than checked.
    pub fn start<C: AppContext>(cx: &C) -> Option<(Self, mpsc::Receiver<BridgeRequest>)> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| Self::bind(cx)))
            .ok()
            .flatten()
    }
    fn bind<C: AppContext>(cx: &C) -> Option<(Self, mpsc::Receiver<BridgeRequest>)> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
        listener.set_nonblocking(true).ok()?;
        let port = listener.local_addr().ok()?.port();
        let token = uuid::Uuid::new_v4().simple().to_string();
        let info = BridgeInfo {
            url: format!("http://127.0.0.1:{port}/mcp/{token}"),
        };
        let (tx, rx) = mpsc::channel::<BridgeRequest>(64);
        let serve = gpui_tokio::Tokio::spawn(cx, async move {
            // `from_std` needs the runtime's reactor — convert inside the
            // task, not on the UI thread.
            let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                return;
            };
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let tx = tx.clone();
                let token = token.clone();
                tokio::spawn(serve_connection(stream, tx, token));
            }
        });
        Some((
            Self {
                info,
                _serve: serve,
            },
            rx,
        ))
    }
}

async fn serve_connection(
    mut stream: tokio::net::TcpStream,
    tx: mpsc::Sender<BridgeRequest>,
    token: String,
) {
    let mut buffer = Vec::with_capacity(4096);
    loop {
        let request = match read_request(&mut stream, &mut buffer).await {
            Ok(Some(request)) => request,
            Ok(None) => return,
            Err(response) => {
                let _ = respond(&mut stream, response.0, response.1).await;
                return;
            }
        };
        let (head, body) = request;
        if !head.starts_with("POST ") {
            let _ = respond(
                &mut stream,
                405,
                b"{\"error\":\"method not allowed\"}".to_vec(),
            )
            .await;
            continue;
        }
        let path = head
            .split_whitespace()
            .nth(1)
            .unwrap_or("")
            .split('?')
            .next()
            .unwrap_or("");
        if path != format!("/mcp/{token}") {
            let _ = respond(&mut stream, 404, b"{\"error\":\"not found\"}".to_vec()).await;
            continue;
        }
        let messages: Vec<Value> = match serde_json::from_slice::<Value>(&body) {
            Ok(Value::Array(batch)) => batch,
            Ok(single) => vec![single],
            Err(_) => {
                let _ = respond(
                    &mut stream,
                    200,
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})
                        .to_string()
                        .into_bytes(),
                )
                .await;
                continue;
            }
        };
        let mut replies = Vec::new();
        for message in messages {
            if let Some(reply) = handle_message(message, &tx).await {
                replies.push(reply);
            }
        }
        if replies.is_empty() {
            // Notifications carry no id — the streamable transport answers
            // them with a bare 202.
            let _ = respond_status(&mut stream, 202).await;
        } else {
            let body = if replies.len() == 1 {
                replies.pop().unwrap()
            } else {
                Value::Array(replies)
            };
            let _ = respond(&mut stream, 200, body.to_string().into_bytes()).await;
        }
    }
}

/// Reads one HTTP request (headers + Content-Length body) off the stream.
/// Returns Ok(None) on clean close, Err((status, body)) for a request that
/// deserves an error response.
async fn read_request(
    stream: &mut tokio::net::TcpStream,
    buffer: &mut Vec<u8>,
) -> Result<Option<(String, Vec<u8>)>, (u16, Vec<u8>)> {
    let header_end = loop {
        if let Some(end) = find_sequence(buffer, b"\r\n\r\n") {
            break end + 4;
        }
        if buffer.len() > MAX_HEADER {
            return Err((431, b"{\"error\":\"header too large\"}".to_vec()));
        }
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk).await {
            Ok(0) => return Ok(None),
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(_) => return Ok(None),
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let length = head
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    if length > MAX_BODY {
        return Err((413, b"{\"error\":\"body too large\"}".to_vec()));
    }
    while buffer.len() < header_end + length {
        let mut chunk = [0u8; 8192];
        match stream.read(&mut chunk).await {
            Ok(0) => return Ok(None),
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(_) => return Ok(None),
        }
    }
    let body = buffer[header_end..header_end + length].to_vec();
    buffer.drain(..header_end + length);
    Ok(Some((head, body)))
}

fn find_sequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

async fn handle_message(message: Value, tx: &mpsc::Sender<BridgeRequest>) -> Option<Value> {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "initialize" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "zeron-browser", "title": "Cosmos Browser", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Drive the embedded browser tab the user is looking at. Prefer browser_state first; take a browser_screenshot when the user asks what they see."
            }
        })),
        "ping" => Some(json!({"jsonrpc":"2.0","id":id,"result":{}})),
        "tools/list" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"tools": tools()}
        })),
        "tools/call" => {
            let params = message.get("params").cloned().unwrap_or(json!({}));
            let tool = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if !tools().iter().any(|t| t["name"] == tool) {
                return Some(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":format!("Unknown tool: {tool}")}}),
                );
            }
            let (reply_tx, reply_rx) = oneshot::channel();
            let request = BridgeRequest {
                tool: tool.to_string(),
                args,
                reply: reply_tx,
            };
            if tx.send(request).await.is_err() {
                return Some(
                    json!({"jsonrpc":"2.0","id":id,"result":{"isError":true,"content":[{"type":"text","text":"The app is no longer listening for browser commands."}]}}),
                );
            }
            let reply = match tokio::time::timeout(CALL_TIMEOUT, reply_rx).await {
                Ok(Ok(Ok(reply))) => reply,
                Ok(Ok(Err(error))) => {
                    return Some(
                        json!({"jsonrpc":"2.0","id":id,"result":{"isError":true,"content":[{"type":"text","text":error}]}}),
                    );
                }
                _ => {
                    return Some(
                        json!({"jsonrpc":"2.0","id":id,"result":{"isError":true,"content":[{"type":"text","text":"The browser did not answer in time."}]}}),
                    );
                }
            };
            let mut content = Vec::new();
            if let Some(text) = reply.text {
                content.push(json!({"type":"text","text":text}));
            }
            if let Some(png) = reply.image {
                content.push(json!({
                    "type": "image",
                    "data": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png),
                    "mimeType": "image/png",
                }));
            }
            Some(json!({"jsonrpc":"2.0","id":id,"result":{"content":content}}))
        }
        other if other.starts_with("notifications/") => None,
        // Unrecognized notifications are ignored; unrecognized requests
        // (they carry an id) get a method-not-found error.
        _ if id.is_null() => None,
        _ => Some(
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("Method not found: {method}")}}),
        ),
    }
}

fn tools() -> Vec<Value> {
    vec![
        json!({"name":"browser_state","description":"Read the embedded browser tab's URL, title, loading flag, and history availability.","inputSchema":{"type":"object","properties":{"tab":{"type":"integer","description":"Tab id from browser_tabs; defaults to the visible tab."}},"additionalProperties":false}}),
        json!({"name":"browser_tabs","description":"List the embedded browser tabs the user has open (id, url, title, which is active).","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}),
        json!({"name":"browser_open","description":"Open a new embedded browser tab at the URL.","inputSchema":{"type":"object","properties":{"url":{"type":"string","description":"http(s) URL to open."}},"required":["url"],"additionalProperties":false}}),
        json!({"name":"browser_navigate","description":"Navigate the browser tab to a URL.","inputSchema":{"type":"object","properties":{"tab":{"type":"integer"},"url":{"type":"string"}},"required":["url"],"additionalProperties":false}}),
        json!({"name":"browser_reload","description":"Reload the browser tab.","inputSchema":{"type":"object","properties":{"tab":{"type":"integer"}},"additionalProperties":false}}),
        json!({"name":"browser_back","description":"Go back one step in the browser tab's history.","inputSchema":{"type":"object","properties":{"tab":{"type":"integer"}},"additionalProperties":false}}),
        json!({"name":"browser_forward","description":"Go forward one step in the browser tab's history.","inputSchema":{"type":"object","properties":{"tab":{"type":"integer"}},"additionalProperties":false}}),
        json!({"name":"browser_evaluate","description":"Evaluate a JavaScript expression in the page and return its JSON value. Wrap statements in an IIFE.","inputSchema":{"type":"object","properties":{"tab":{"type":"integer"},"script":{"type":"string","description":"A JavaScript expression."}},"required":["script"],"additionalProperties":false}}),
        json!({"name":"browser_click","description":"Click the element matching a CSS selector.","inputSchema":{"type":"object","properties":{"tab":{"type":"integer"},"selector":{"type":"string"}},"required":["selector"],"additionalProperties":false}}),
        json!({"name":"browser_type","description":"Focus the element matching a CSS selector and set its value, dispatching input/change events.","inputSchema":{"type":"object","properties":{"tab":{"type":"integer"},"selector":{"type":"string"},"text":{"type":"string"}},"required":["selector","text"],"additionalProperties":false}}),
        json!({"name":"browser_scroll","description":"Scroll the page by x/y pixels.","inputSchema":{"type":"object","properties":{"tab":{"type":"integer"},"x":{"type":"number"},"y":{"type":"number"}},"additionalProperties":false}}),
        json!({"name":"browser_text","description":"Return the page's visible text (body innerText, truncated).","inputSchema":{"type":"object","properties":{"tab":{"type":"integer"}},"additionalProperties":false}}),
        json!({"name":"browser_screenshot","description":"Capture the visible page as a PNG image.","inputSchema":{"type":"object","properties":{"tab":{"type":"integer"}},"additionalProperties":false}}),
    ]
}

async fn respond(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    body: Vec<u8>,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&body).await
}

async fn respond_status(stream: &mut tokio::net::TcpStream, status: u16) -> std::io::Result<()> {
    stream
        .write_all(
            format!(
                "HTTP/1.1 {status} Accepted\r\nContent-Length: 0\r\nCache-Control: no-store\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
}

// ---------------------------------------------------------------------------
// Stdio front — `zeron browser-bridge --url <endpoint>`
// ---------------------------------------------------------------------------

/// The harness-side half of the bridge: ACP agents spawn this binary as an
/// MCP server process. Each stdin JSON-RPC line (MCP stdio is NDJSON) is
/// POSTed to the loopback endpoint and the reply written to stdout; a 202
/// notification response produces no output. Runs its own tiny runtime —
/// stdout must stay protocol-clean, so callers must not attach a tracing
/// writer to it.
pub fn serve_stdio(endpoint: &str) -> anyhow::Result<()> {
    let target = parse_endpoint(endpoint)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let mut stdout = tokio::io::stdout();
        let mut lines = tokio::io::AsyncBufReadExt::lines(stdin);
        while let Some(line) = lines.next_line().await? {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(body) = post(&target, line.as_bytes()).await
                && !body.is_empty()
            {
                stdout.write_all(&body).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }
        Ok(())
    })
}

/// `http://127.0.0.1:<port>/mcp/<token>` → ("127.0.0.1:<port>", "/mcp/<token>").
fn parse_endpoint(endpoint: &str) -> anyhow::Result<(String, String)> {
    let url = url::Url::parse(endpoint)?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("bridge URL has no host"))?;
    let port = url.port().unwrap_or(80);
    let path = if url.path().is_empty() {
        "/".to_string()
    } else {
        url.path().to_string()
    };
    Ok((format!("{host}:{port}"), path))
}

/// One JSON-RPC POST; returns the response body (empty for a 202).
async fn post(target: &(String, String), body: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut stream = tokio::net::TcpStream::connect(&target.0).await?;
    let head = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        target.1,
        target.0,
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.shutdown().await?;
    let mut buffer = Vec::with_capacity(4096);
    stream.read_to_end(&mut buffer).await?;
    let Some(at) = find_sequence(&buffer, b"\r\n\r\n") else {
        anyhow::bail!("bridge returned no HTTP response");
    };
    Ok(buffer[at + 4..].to_vec())
}
