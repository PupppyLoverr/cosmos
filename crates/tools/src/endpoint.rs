//! The loopback MCP endpoint harnesses POST to — the same streamable-HTTP
//! subset the Cosmos browser bridge and Cosmos Computer speak
//! (`initialize`, `ping`, `notifications/*`, `tools/list`, `tools/call`).
//! The [`ToolService`] is engine-owned and Send+Sync, so connections
//! dispatch straight into it with no UI hop.
//!
//! Security posture is identical to the other bridges: 127.0.0.1 only, a
//! per-bind 128-bit token in the path, no persistence, no forwarding —
//! and nothing upstream-related (keys, headers, provider config) ever
//! appears on the wire. Harnesses see the merged, prefixed tool list and
//! the calls' results; that is all.

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::service::{self, ToolService};

const MAX_HEADER: usize = 16 * 1024;
/// Tool arguments can carry file content — bounded, but generous.
const MAX_BODY: usize = 8 * 1024 * 1024;
/// Meta-tool executions (COMPOSIO_MULTI_EXECUTE_TOOL batches ≤50 calls) get
/// real headroom; the upstream's own timeout fires first.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Accept loop — the service binds the listener and spawns this per endpoint.
pub async fn serve(listener: tokio::net::TcpListener, token: String, service: ToolService) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let service = service.clone();
        let token = token.clone();
        tokio::spawn(serve_connection(stream, service, token));
    }
}

async fn serve_connection(mut stream: tokio::net::TcpStream, service: ToolService, token: String) {
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
            if let Some(reply) = handle_message(message, &service).await {
                replies.push(reply);
            }
        }
        if replies.is_empty() {
            let _ = respond_status(&mut stream).await;
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

async fn handle_message(message: Value, service: &ToolService) -> Option<Value> {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "initialize" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "cosmos-tools", "title": "Cosmos Tools", "version": env!("CARGO_PKG_VERSION")},
                "instructions": service::gateway_instructions(),
            }
        })),
        "ping" => Some(json!({"jsonrpc":"2.0","id":id,"result":{}})),
        "tools/list" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {"tools": service.merged_tools().await}
        })),
        "tools/call" => {
            let params = message.get("params").cloned().unwrap_or(json!({}));
            let tool = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if !service.tool_exists(tool) {
                return Some(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":format!("Unknown tool: {tool}")}}),
                );
            }
            let outcome =
                tokio::time::timeout(CALL_TIMEOUT, service.dispatch_tool(tool, args)).await;
            match outcome {
                // The upstream's result passes through verbatim — content
                // blocks, structuredContent, and isError all intact.
                Ok(Ok(result)) => Some(json!({"jsonrpc":"2.0","id":id,"result":result})),
                // Gateway-side failures surface as tool errors so harnesses
                // see them in-band like every other refusal.
                Ok(Err(error)) => Some(
                    json!({"jsonrpc":"2.0","id":id,"result":{"isError":true,"content":[{"type":"text","text":format!("{} ({})", error.message(), error.code())}]}}),
                ),
                Err(_) => Some(
                    json!({"jsonrpc":"2.0","id":id,"result":{"isError":true,"content":[{"type":"text","text":"The tool server did not answer in time."}]}}),
                ),
            }
        }
        other if other.starts_with("notifications/") => None,
        _ if id.is_null() => None,
        _ => Some(
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":format!("Method not found: {method}")}}),
        ),
    }
}

async fn respond(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    body: Vec<u8>,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
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

async fn respond_status(stream: &mut tokio::net::TcpStream) -> std::io::Result<()> {
    stream
        .write_all(
            "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nCache-Control: no-store\r\n\r\n"
                .as_bytes(),
        )
        .await
}
