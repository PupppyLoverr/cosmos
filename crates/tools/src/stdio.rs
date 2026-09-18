//! The harness-side front of the endpoint: `cosmos tools-stdio --url
//! <endpoint>` runs as an MCP stdio server. Each stdin JSON-RPC line is
//! POSTed to the loopback endpoint and the reply written to stdout; a 202
//! notification response produces no output. Runs its own tiny runtime —
//! stdout must stay protocol-clean, so no tracing writer may attach to it.

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

/// Block serving MCP-stdio ↔ HTTP bridging until stdin closes.
pub fn serve_stdio(endpoint: &str) -> anyhow::Result<()> {
    let target = parse_endpoint(endpoint)?;
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
            let (status, body) = post(&target, line.as_bytes()).await?;
            // A dead or re-tokenized endpoint answers non-2xx — translate
            // that into a JSON-RPC error so stdout stays protocol-clean.
            let out = if status == 202 {
                Vec::new()
            } else if status == 200 {
                body
            } else {
                let id = serde_json::from_slice::<serde_json::Value>(line.as_bytes())
                    .ok()
                    .and_then(|request| request.get("id").cloned())
                    .unwrap_or(serde_json::Value::Null);
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32000, "message": format!("tools gateway answered HTTP {status}")},
                })
                .to_string()
                .into_bytes()
            };
            if !out.is_empty() {
                stdout.write_all(&out).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }
        Ok(())
    })
}

/// `http://127.0.0.1:<port>/mcp/<token>` → ("127.0.0.1:<port>", "/mcp/<token>").
/// Loopback-only is enforced — a remote URL is rejected outright.
fn parse_endpoint(endpoint: &str) -> anyhow::Result<(String, String)> {
    let url = url::Url::parse(endpoint)?;
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("tools endpoint has no host"))?;
    anyhow::ensure!(
        host == "127.0.0.1"
            || host == "::1"
            || host == "[::1]"
            || host.eq_ignore_ascii_case("localhost"),
        "tools endpoint must be loopback, got {host}"
    );
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "tools endpoint must not carry credentials"
    );
    let port = url.port().unwrap_or(80);
    let path = if url.path().is_empty() {
        "/".to_string()
    } else {
        url.path().to_string()
    };
    Ok((format!("{host}:{port}"), path))
}

/// One JSON-RPC POST; returns (status, body). Chunked transfer-encoding is
/// decoded — the gateway speaks Content-Length today, but stdout must stay
/// protocol-clean no matter what the endpoint sends back.
async fn post(target: &(String, String), body: &[u8]) -> anyhow::Result<(u16, Vec<u8>)> {
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
        anyhow::bail!("tools endpoint returned no HTTP response");
    };
    let head = String::from_utf8_lossy(&buffer[..at]).to_string();
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let mut body = &buffer[at + 4..];
    let chunked = head.lines().any(|line| {
        line.split_once(':')
            .map(|(name, value)| {
                name.trim().eq_ignore_ascii_case("transfer-encoding")
                    && value.trim().eq_ignore_ascii_case("chunked")
            })
            .unwrap_or(false)
    });
    let decoded;
    if chunked {
        decoded = dechunk(body)?;
        body = &decoded;
    }
    Ok((status, body.to_vec()))
}

/// `<hex-len>\r\n<chunk>\r\n … 0\r\n\r\n` → concatenated chunks.
fn dechunk(mut body: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(body.len());
    loop {
        let Some(end) = find_sequence(body, b"\r\n") else {
            anyhow::bail!("truncated chunk header");
        };
        let size_text = std::str::from_utf8(&body[..end])?;
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)?;
        body = &body[end + 2..];
        if size == 0 {
            return Ok(out);
        }
        anyhow::ensure!(body.len() >= size + 2, "truncated chunk");
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

fn find_sequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::parse_endpoint;

    /// The shim is a local bridge only — a remote or credentialed endpoint
    /// is rejected before a socket ever opens.
    #[test]
    fn endpoint_parsing_enforces_loopback() {
        for url in [
            "http://127.0.0.1:8042/mcp/tok",
            "http://localhost:1/mcp/tok",
            "http://[::1]:2/mcp/tok",
        ] {
            assert!(parse_endpoint(url).is_ok(), "{url}");
        }
        for url in [
            "https://tools.example.com/mcp/tok",
            "http://192.168.1.4:8042/mcp/tok",
            "http://user:pass@127.0.0.1:8042/mcp/tok",
            "not a url",
        ] {
            assert!(parse_endpoint(url).is_err(), "{url}");
        }
        let (host, path) = parse_endpoint("http://127.0.0.1:8042/mcp/abc?q=1").unwrap();
        assert_eq!(host, "127.0.0.1:8042");
        assert_eq!(path, "/mcp/abc");
    }
}
