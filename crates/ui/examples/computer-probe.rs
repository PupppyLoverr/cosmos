//! computer-probe — drive the production Cosmos Computer stack end-to-end.
//!
//! Unlike `ax-probe` (raw ApplicationServices AX calls), this binary runs
//! the real shipped path: `ComputerService` on the device's data dir, the
//! pinned Cua Driver subprocess, and the same loopback MCP endpoint a
//! harness's `zeron computer-stdio` would be stamped with.
//!
//! Commands:
//!   computer-probe status           saved settings + status + permission hints
//!   computer-probe install          pinned Cua Driver download → sha256 → place
//!   computer-probe uninstall        remove the component
//!   computer-probe probe            live driver health (spawns the driver)
//!   computer-probe tools            MCP tools/list over the loopback endpoint
//!   computer-probe call NAME JSON   tools/call over the endpoint (the harness path)
//!   computer-probe observe [JSON]   shorthand for `call computer_observe`
//!   computer-probe set JSON         persist ComputerSettings (what the pane writes)
//!
//! Env:
//!   ZERON_DATA_DIR          device data dir (default ~/.zeron)
//!   COMPUTER_PROBE_SHOTS    dir to dump image-block PNGs into
//!   COSMOS_COMPUTER_MOCK    set → mock backend instead of the driver
//!
//! Exit codes: 0 ok · 1 usage · 2 command failure · 3 refused/missing capability.

use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use zeron_computer::{ComputerService, ComputerSettings};

fn data_dir() -> PathBuf {
    std::env::var_os("ZERON_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".zeron"))
}

fn usage() -> ! {
    eprintln!("{}", env!("CARGO_BIN_NAME"));
    eprintln!("  status | install | uninstall | probe | tools | set JSON");
    eprintln!("  observe [JSON] | call NAME JSON | serve | mcp-config");
    std::process::exit(1);
}

async fn mcp_call(url: &str, method: &str, params: Value) -> Result<Value, String> {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let parsed = url::Url::parse(url).map_err(|e| e.to_string())?;
    let addr = format!(
        "{}:{}",
        parsed.host_str().ok_or("no host")?,
        parsed.port().unwrap_or(80)
    );
    let mut stream = tokio::net::TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        parsed.path(),
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;
    stream.shutdown().await.ok();
    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .await
        .map_err(|e| format!("read: {e}"))?;
    let raw = String::from_utf8_lossy(&buf);
    let text = raw.split("\r\n\r\n").nth(1).unwrap_or("").trim();
    serde_json::from_str(text).map_err(|e| format!("parse: {e} — {text}"))
}

/// Unwrap one MCP tools/call reply: `{result.content[]}` or `{error}`.
/// Prints text blocks; writes image blocks under COMPUTER_PROBE_SHOTS.
fn dump_reply(reply: &Value) -> Result<(), String> {
    if let Some(err) = reply.get("error") {
        return Err(format!("jsonrpc error: {err}"));
    }
    let result = reply.get("result").cloned().unwrap_or(json!({}));
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        let text = result["content"][0]["text"].as_str().unwrap_or("?");
        return Err(format!("tool refused: {text}"));
    }
    let mut printed = false;
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for block in content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    println!("{}", block["text"].as_str().unwrap_or(""));
                    printed = true;
                }
                Some("image") => {
                    let bytes = base64_decode(block["data"].as_str().unwrap_or(""));
                    let dir = std::env::var_os("COMPUTER_PROBE_SHOTS").map(PathBuf::from);
                    let out = dir
                        .unwrap_or_else(|| PathBuf::from("/tmp"))
                        .join(format!("probe-{}.png", std::process::id()));
                    std::fs::write(&out, &bytes).map_err(|e| format!("shot write: {e}"))?;
                    println!("[image → {} ({} bytes)]", out.display(), bytes.len());
                }
                other => println!("[block {:?}]", other),
            }
        }
    }
    if !printed && result.get("content").is_none() {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    }
    Ok(())
}

/// snake_case → camelCase so `set` patches accept either spelling.
fn snake_to_camel(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut upper = false;
    for ch in key.chars() {
        if ch == '_' {
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn base64_decode(s: &str) -> Vec<u8> {
    // Minimal decoder — avoids pulling base64 for one image block.
    fn v(c: u8) -> u8 {
        match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => 0,
        }
    }
    let s: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= (v(c) as u32) << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 && chunk[2] != b'=' {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 && chunk[3] != b'=' {
            out.push(n as u8);
        }
    }
    out
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(cmd) = args.next() else { usage() };
    let dir = data_dir();
    let shots = dir.join("uploads").join("computer");
    let service = ComputerService::new(dir.clone(), shots);

    match cmd.as_str() {
        "status" => {
            let status = service.status();
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
        }
        "set" => {
            let Some(body) = args.next() else { usage() };
            // Merge the patch over the persisted settings — unspecified
            // fields keep their saved values rather than resetting to
            // serde defaults.
            let mut merged = serde_json::to_value(service.settings()).unwrap_or_else(|_| json!({}));
            let patch: Value = serde_json::from_str(&body).unwrap_or_else(|e| {
                eprintln!("bad settings json: {e}");
                std::process::exit(2);
            });
            if let (Value::Object(base), Value::Object(patch)) = (&mut merged, &patch) {
                for (key, value) in patch {
                    let camel = snake_to_camel(key);
                    let key = if base.contains_key(key) {
                        key.clone()
                    } else {
                        camel
                    };
                    base.insert(key, value.clone());
                }
            }
            let settings: ComputerSettings = serde_json::from_value(merged).unwrap_or_else(|e| {
                eprintln!("bad settings json: {e}");
                std::process::exit(2);
            });
            if let Err(e) = service.set_settings(settings) {
                eprintln!("set failed: [{}] {}", e.code(), e.message());
                std::process::exit(2);
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&service.status()).unwrap()
            );
        }
        "install" => match service.install().await {
            Ok(()) => println!("installed: {}", service.status().state),
            Err(e) => {
                eprintln!("install failed: [{}] {}", e.code(), e.message());
                std::process::exit(2);
            }
        },
        "uninstall" => match service.uninstall().await {
            Ok(()) => println!("uninstalled"),
            Err(e) => {
                eprintln!("uninstall failed: [{}] {}", e.code(), e.message());
                std::process::exit(2);
            }
        },
        "probe" => {
            let probe = service.probe().await;
            println!("{}", serde_json::to_string_pretty(&probe).unwrap());
            if !probe.driver_ok {
                std::process::exit(3);
            }
        }
        "serve" => {
            let Some(url) = service.endpoint_url() else {
                eprintln!("endpoint unavailable — enable Screen Context in Settings → Computer");
                std::process::exit(3);
            };
            println!("{url}");
            eprintln!("cosmos-computer MCP at {url} — Ctrl-C to stop");
            std::future::pending::<()>().await;
        }
        "mcp-config" => {
            let Some(url) = service.endpoint_url() else {
                eprintln!("endpoint unavailable — enable Screen Context first");
                std::process::exit(3);
            };
            let exe = std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "cosmos".into());
            let session = format!("cosmos-run-{}", std::process::id());
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "mcp": {
                        "cosmos-computer": {
                            "type": "local",
                            "command": [exe, "computer-stdio", "--url", url, "--session", session],
                            "enabled": true,
                            "timeout": 120000
                        }
                    }
                }))
                .unwrap()
            );
            eprintln!("keep this process alive? prefer: computer-probe serve");
        }
        "tools" | "call" | "observe" => {
            let Some(url) = service.endpoint_url() else {
                eprintln!("endpoint unavailable — service disabled?");
                std::process::exit(3);
            };
            let init = mcp_call(&url, "initialize", json!({})).await;
            match init {
                Ok(v) => {
                    let name = v["result"]["serverInfo"]["name"].as_str().unwrap_or("?");
                    let version = v["result"]["serverInfo"]["version"].as_str().unwrap_or("?");
                    eprintln!("[endpoint {url} · {name} {version}]");
                }
                Err(e) => {
                    eprintln!("initialize failed: {e}");
                    std::process::exit(2);
                }
            }
            match cmd.as_str() {
                "tools" => match mcp_call(&url, "tools/list", json!({})).await {
                    Ok(v) => {
                        let tools = v["result"]["tools"].as_array().cloned().unwrap_or_default();
                        for t in &tools {
                            let name = t["name"].as_str().unwrap_or("?");
                            let desc = t["description"].as_str().unwrap_or("");
                            println!("{name:28} {}", desc.chars().take(96).collect::<String>());
                        }
                        eprintln!("[{} tools]", tools.len());
                    }
                    Err(e) => {
                        eprintln!("tools/list failed: {e}");
                        std::process::exit(2);
                    }
                },
                "observe" => {
                    let arguments = args
                        .next()
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or(json!({}));
                    let t0 = Instant::now();
                    match mcp_call(
                        &url,
                        "tools/call",
                        json!({"name": "computer_observe", "arguments": arguments}),
                    )
                    .await
                    {
                        Ok(v) => {
                            eprintln!("[observe {:?}]", t0.elapsed());
                            if let Err(e) = dump_reply(&v) {
                                eprintln!("{e}");
                                std::process::exit(3);
                            }
                        }
                        Err(e) => {
                            eprintln!("{e}");
                            std::process::exit(2);
                        }
                    }
                }
                _ => {
                    let (Some(name), Some(body)) = (args.next(), args.next()) else {
                        usage()
                    };
                    let arguments: Value = serde_json::from_str(&body).unwrap_or_else(|e| {
                        eprintln!("bad args json: {e}");
                        std::process::exit(2);
                    });
                    let t0 = Instant::now();
                    match mcp_call(
                        &url,
                        "tools/call",
                        json!({"name": name, "arguments": arguments}),
                    )
                    .await
                    {
                        Ok(v) => {
                            eprintln!("[{name} {:?}]", t0.elapsed());
                            if let Err(e) = dump_reply(&v) {
                                eprintln!("{e}");
                                std::process::exit(3);
                            }
                        }
                        Err(e) => {
                            eprintln!("{e}");
                            std::process::exit(2);
                        }
                    }
                }
            }
        }
        _ => usage(),
    }
}
