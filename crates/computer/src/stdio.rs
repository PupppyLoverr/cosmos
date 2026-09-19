//! The harness-side front of the endpoint: `cosmos computer-stdio --url
//! <endpoint>` runs as an MCP stdio server. Each stdin JSON-RPC line is
//! POSTed to the loopback endpoint and the reply written to stdout; a 202
//! notification response produces no output. Runs its own tiny runtime —
//! stdout must stay protocol-clean, so no tracing writer may attach to it.
//!
//! Optional `--session` / `--pin-pid` / `--pin-window` isolate one harness
//! from another: the shim injects those into every `tools/call` so the
//! engine-side [`ComputerService`] can pin actions to one Cua session and
//! one window target set.

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

/// Isolation + endpoint the stdio shim forwards to.
#[derive(Debug, Clone, Default)]
pub struct StdioOptions {
    pub endpoint: String,
    pub session: Option<String>,
    pub pin_pid: Option<i64>,
    pub pin_window: Option<i64>,
}

/// Block serving MCP-stdio ↔ HTTP bridging until stdin closes.
pub fn serve_stdio(endpoint: &str) -> anyhow::Result<()> {
    serve_stdio_opts(StdioOptions {
        endpoint: endpoint.to_string(),
        ..StdioOptions::default()
    })
}

/// Same as [`serve_stdio`] with per-agent isolation flags.
pub fn serve_stdio_opts(opts: StdioOptions) -> anyhow::Result<()> {
    let target = parse_endpoint(&opts.endpoint)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let mut stdout = tokio::io::stdout();
        let mut lines = AsyncBufReadExt::lines(stdin);
        while let Some(line) = lines.next_line().await? {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let forwarded = inject_isolation(line, &opts);
            let body = post(&target, forwarded.as_bytes()).await?;
            if !body.is_empty() {
                stdout.write_all(&body).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }
        Ok(())
    })
}

/// Inject session/pin into a `tools/call` JSON-RPC body. Other methods pass
/// through unchanged. Returns owned JSON (pretty-free, compact).
pub fn inject_isolation(line: &str, opts: &StdioOptions) -> String {
    if opts.session.is_none() && opts.pin_pid.is_none() && opts.pin_window.is_none() {
        return line.to_string();
    }
    let Ok(mut value) = serde_json::from_str::<Value>(line) else {
        return line.to_string();
    };
    let method = value.get("method").and_then(Value::as_str).unwrap_or("");
    if method != "tools/call" {
        return line.to_string();
    }
    let Some(params) = value.get_mut("params").and_then(Value::as_object_mut) else {
        return line.to_string();
    };
    let arguments = params.entry("arguments").or_insert_with(|| json!({}));
    let Some(args) = arguments.as_object_mut() else {
        return line.to_string();
    };
    if let Some(session) = &opts.session {
        args.entry("_cosmosSession")
            .or_insert_with(|| json!(session));
    }
    if let Some(pid) = opts.pin_pid {
        args.entry("_cosmosPinPid").or_insert_with(|| json!(pid));
    }
    if let Some(window) = opts.pin_window {
        args.entry("_cosmosPinWindow")
            .or_insert_with(|| json!(window));
    }
    value.to_string()
}

/// `http://127.0.0.1:<port>/mcp/<token>` → ("127.0.0.1:<port>", "/mcp/<token>").
/// Loopback-only is enforced — a remote URL is rejected outright.
fn parse_endpoint(endpoint: &str) -> anyhow::Result<(String, String)> {
    let url = url::Url::parse(endpoint)?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("computer endpoint has no host"))?;
    anyhow::ensure!(
        host == "127.0.0.1" || host == "::1" || host.eq_ignore_ascii_case("localhost"),
        "computer endpoint must be loopback, got {host}"
    );
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "computer endpoint must not carry credentials"
    );
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
        anyhow::bail!("computer endpoint returned no HTTP response");
    };
    Ok(buffer[at + 4..].to_vec())
}

fn find_sequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_session_and_pin_on_tools_call_only() {
        let opts = StdioOptions {
            endpoint: "http://127.0.0.1:9/mcp/x".into(),
            session: Some("cosmos-run-1".into()),
            pin_pid: Some(42),
            pin_window: Some(7),
        };
        let ping = inject_isolation(
            r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{}}"#,
            &opts,
        );
        assert!(!ping.contains("_cosmosSession"));
        let call = inject_isolation(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"computer_click","arguments":{"pid":42}}}"#,
            &opts,
        );
        let value: Value = serde_json::from_str(&call).unwrap();
        let args = &value["params"]["arguments"];
        assert_eq!(args["_cosmosSession"], "cosmos-run-1");
        assert_eq!(args["_cosmosPinPid"], 42);
        assert_eq!(args["_cosmosPinWindow"], 7);
        assert_eq!(args["pid"], 42);
    }
}
