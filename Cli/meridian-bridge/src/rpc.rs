//! Newline-delimited JSON-RPC 2.0 over stdio (`meridian serve --stdio`) plus
//! the in-process Rust API that mirrors every method one to one.

use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Value, json};

use crate::boot::{self, BootContext};
use crate::memory;
use crate::search::{self, SearchHit};
use crate::summarize;
use crate::vault::Vault;

/// In-process API — the surface the stdio server exposes over JSON-RPC.
pub struct Bridge {
    vault: Vault,
}

impl Bridge {
    pub fn open(vault: Vault) -> Self {
        Self { vault }
    }

    pub fn open_default() -> Result<Self> {
        Ok(Self::open(Vault::open_default()?))
    }

    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    /// `boot.context {repo, harness}` → `{text, tokens, warnings}`.
    pub fn boot_context(&self, repo: &Path, harness: &str) -> Result<BootContext> {
        boot::context(&self.vault, repo, harness)
    }

    /// `memory.search {query, k, scope?, seen_ids[]}` → `[{id,text,type,date,score}]`.
    pub fn memory_search(
        &self,
        query: &str,
        k: usize,
        scope: Option<&str>,
        seen_ids: &[String],
    ) -> Vec<SearchHit> {
        search::search(&self.vault, query, k, scope, seen_ids)
    }

    /// `memory.write {type, text, scope, src}` → `{id}`.
    pub fn memory_write(&self, mtype: &str, text: &str, scope: &str, src: &str) -> Result<String> {
        memory::write(&self.vault, mtype, text, scope, src)
    }

    /// `cache.read {path, max_lines}` → summary text.
    pub fn cache_read(&self, path: &Path, max_lines: usize) -> Result<String> {
        summarize::cache_read(&self.vault, path, max_lines)
    }

    /// `playbook.get {name}` → markdown body.
    pub fn playbook_get(&self, name: &str) -> Result<String> {
        memory::playbook(&self.vault, name)
    }

    /// `session.register {session_id, harness, log_path, title}` → `{path}`.
    pub fn session_register(
        &self,
        session_id: &str,
        harness: &str,
        log_path: &str,
        title: &str,
    ) -> Result<PathBuf> {
        memory::register_session(&self.vault, session_id, harness, log_path, title)
    }

    /// Dispatch one JSON-RPC request object; returns the `result`/`error`
    /// payload fields for the response.
    pub fn dispatch(&self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "boot.context" => {
                let repo = params
                    .get("repo")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .unwrap_or(std::env::current_dir()?);
                let harness = params
                    .get("harness")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                Ok(serde_json::to_value(self.boot_context(&repo, harness)?)?)
            }
            "memory.search" => {
                let query = params.get("query").and_then(Value::as_str).unwrap_or("");
                let k = params.get("k").and_then(Value::as_u64).unwrap_or(5) as usize;
                let scope = params.get("scope").and_then(Value::as_str);
                let seen: Vec<String> = params
                    .get("seen_ids")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(json!({ "results": self.memory_search(query, k, scope, &seen) }))
            }
            "memory.write" => {
                let mtype = params.get("type").and_then(Value::as_str).unwrap_or("");
                let text = params.get("text").and_then(Value::as_str).unwrap_or("");
                let scope = params.get("scope").and_then(Value::as_str).unwrap_or("misc");
                let src = params.get("src").and_then(Value::as_str).unwrap_or("");
                let id = self.memory_write(mtype, text, scope, src)?;
                Ok(json!({ "id": id }))
            }
            "cache.read" => {
                let path = params
                    .get("path")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow::anyhow!("cache.read needs params.path"))?;
                let max_lines = params
                    .get("max_lines")
                    .and_then(Value::as_u64)
                    .unwrap_or(200) as usize;
                Ok(json!({ "summary": self.cache_read(&path, max_lines)? }))
            }
            "playbook.get" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                Ok(json!({ "body": self.playbook_get(name)? }))
            }
            "session.register" => {
                let session_id = params
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let harness = params.get("harness").and_then(Value::as_str).unwrap_or("");
                let log_path = params.get("log_path").and_then(Value::as_str).unwrap_or("");
                let title = params.get("title").and_then(Value::as_str).unwrap_or("");
                let p = self.session_register(session_id, harness, log_path, title)?;
                Ok(json!({ "path": p.display().to_string() }))
            }
            other => anyhow::bail!("unknown method '{other}'"),
        }
    }
}

/// `meridian serve --stdio`: read one JSON request per line on stdin, write
/// one JSON response per line on stdout.
pub fn serve_stdio(bridge: &Bridge) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Value>(&line) {
            Ok(req) => {
                let id = req.get("id").cloned().unwrap_or(Value::Null);
                let method = req.get("method").and_then(Value::as_str).unwrap_or("");
                let params = req.get("params").cloned().unwrap_or(Value::Null);
                match bridge.dispatch(method, &params) {
                    Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                    Err(e) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32000, "message": e.to_string()}
                    }),
                }
            }
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {"code": -32700, "message": format!("parse error: {e}")}
            }),
        };
        serde_json::to_writer(&mut out, &resp)?;
        out.write_all(b"\n")?;
        out.flush()?;
    }
    Ok(())
}
