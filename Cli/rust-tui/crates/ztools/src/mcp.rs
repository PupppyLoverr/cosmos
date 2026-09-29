//! Minimal MCP stdio client (contract §11): initialize, tools/list, tools/call.
//! Servers are spawned lazily per call — simple and stateless.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::ToolContext;

struct Mcp {
    child: Child,
}

impl Mcp {
    fn spawn(cmd: &str, args: &[String], env: &std::collections::HashMap<String, String>) -> Result<Self> {
        let child = Command::new(cmd)
            .args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn mcp server {cmd}"))?;
        Ok(Self { child })
    }

    fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        let req = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
        let stdin = self.child.stdin.as_mut().context("mcp stdin")?;
        stdin.write_all(req.to_string().as_bytes())?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        let stdout = self.child.stdout.as_mut().context("mcp stdout")?;
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line)?;
        let v: Value = serde_json::from_str(&line).context("mcp response parse")?;
        if let Some(e) = v.get("error") {
            bail!("mcp error: {e}");
        }
        Ok(v["result"].clone())
    }
}

fn with_server(
    ctx: &ToolContext,
    server: &str,
    f: impl FnOnce(&mut Mcp) -> Result<Value>,
) -> Result<Value> {
    let cfg = ctx
        .mcp_servers
        .get(server)
        .cloned()
        .with_context(|| format!("unknown MCP server {server}"))?;
    let mut m = Mcp::spawn(&cfg.command, &cfg.args, &cfg.env)?;
    let _ = m.rpc(
        "initialize",
        json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"zcli","version":"0.1.0"}}),
    );
    // notifications/initialized — one-way
    if let Some(stdin) = m.child.stdin.as_mut() {
        let _ = writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        );
    }
    let r = f(&mut m);
    let _ = m.child.kill();
    r
}

pub fn run(ctx: &mut ToolContext, name: &str, input: &Value) -> Result<String> {
    if ctx.mcp_servers.is_empty() {
        bail!("no MCP servers configured")
    }
    match name {
        "tool_search" => {
            let q = input["query"].as_str().unwrap_or("").to_lowercase();
            let mut hits = String::new();
            for server in ctx.mcp_servers.keys().cloned().collect::<Vec<_>>() {
                if let Ok(res) = with_server(ctx, &server, |m| m.rpc("tools/list", json!({}))) {
                    if let Some(tools) = res["tools"].as_array() {
                        for t in tools {
                            let name = t["name"].as_str().unwrap_or("");
                            let desc = t["description"].as_str().unwrap_or("");
                            if q.is_empty()
                                || name.to_lowercase().contains(&q)
                                || desc.to_lowercase().contains(&q)
                            {
                                hits.push_str(&format!(
                                    "{server}.{name} — {desc}\n{}\n",
                                    t["inputSchema"]
                                ));
                            }
                        }
                    }
                }
            }
            Ok(if hits.is_empty() {
                "no MCP tools matched".into()
            } else {
                hits
            })
        }
        "mcp_call" => {
            let server = input["server"].as_str().unwrap_or("");
            let tool = input["tool"].as_str().unwrap_or("");
            let args = input["arguments"].clone();
            let res = with_server(ctx, server, |m| {
                m.rpc("tools/call", json!({"name": tool, "arguments": args}))
            })?;
            Ok(res.to_string())
        }
        _ => unreachable!(),
    }
}
