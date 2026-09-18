//! The harness-side front of the endpoint: `cosmos computer-stdio --url
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
            let body = post(&target, line.as_bytes()).await?;
            if !body.is_empty() {
                stdout.write_all(&body).await?;
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
