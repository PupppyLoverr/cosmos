//! ztools — contract §2 tools. Schemas are the canonical `contracts/tools.json`
//! bytes so `tools dump` is byte-identical across harnesses.

pub mod ckpt;
mod files;
pub mod jobs;
mod mcp;
pub mod memory;
pub mod repair;
pub mod shell;
pub mod supermemory;
pub mod verify;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;
use zproto::ToolSpec;

pub const TOOLS_JSON: &str = include_str!("../../../../contracts/tools.json");

/// Hard cap on any tool result (contract §2).
pub const RESULT_CAP: usize = 30_000;
/// Lines allowed in one unbounded read before the guard fires (bridge config
/// can override via `large_read_lines`).
pub const LARGE_READ_LINES: usize = 200;

/// Raw bytes of contracts/tools.json — print verbatim so `tools dump` is
/// byte-identical across harnesses (conformance gate).
pub fn tools_dump() -> &'static str {
    TOOLS_JSON
}

pub fn tool_specs() -> Vec<ToolSpec> {
    serde_json::from_str(TOOLS_JSON).expect("contracts/tools.json parses")
}

/// Sent with every request — contract §4 layer 2. Small on purpose: every
/// schema here is paid for (or cache-read) on every turn.
pub const CORE_TOOLS: &[&str] = &["read", "edit", "write", "bash", "grep", "glob", "tool_search", "mcp_call"];

/// Loaded on demand: `tool_search` returns their schemas as text, and the
/// model calls them through `mcp_call {server: "cosmos", tool, arguments}`,
/// so the tools block — and the prompt cache behind it — never changes.
pub const DEFERRED_TOOLS: &[&str] = &[
    "ls",
    "memory_search",
    "memory_write",
    "task",
    "context_remaining",
    "new_context",
    "request_escalation",
    "submit_plan",
];

/// The MCP server name the harness answers for itself.
pub const SELF_SERVER: &str = "cosmos";

/// Tools on the wire. A session that starts in plan mode also carries
/// `submit_plan` (kept for the whole session so the prefix never changes).
pub fn wire_tools(plan: bool) -> Vec<ToolSpec> {
    tool_specs()
        .into_iter()
        .filter(|t| CORE_TOOLS.contains(&t.name.as_str()) || (plan && t.name == "submit_plan"))
        .collect()
}

/// `mcp_call {server:"cosmos", tool, arguments}` → the deferred call it wraps.
pub fn unwrap_self_call(name: &str, input: &Value) -> Option<(String, Value)> {
    if name != "mcp_call" || input["server"].as_str() != Some(SELF_SERVER) {
        return None;
    }
    let tool = input["tool"].as_str()?.to_string();
    let args = match &input["arguments"] {
        Value::Null => serde_json::json!({}),
        v => v.clone(),
    };
    Some((tool, args))
}

/// Per-session tool state.
pub struct ToolContext {
    /// workspace root — all file tools are confined to it
    pub workspace: PathBuf,
    /// Additional roots granted by --add-dir.
    pub allowed_roots: Vec<PathBuf>,
    /// sandbox executable (zsandbox.exe)
    pub sandbox_bin: Option<PathBuf>,
    /// files read this session → sha256 at read time (write STALE guard)
    pub read_state: HashMap<String, String>,
    /// operator-granted escalations this session
    pub escalations: Vec<String>,
    /// network allowed inside the sandbox this session
    pub net_allow: bool,
    /// sandbox availability: None = ok, Some(reason) = degraded
    pub sandbox_degraded: Option<String>,
    /// sandbox disabled entirely (--no-sandbox)
    pub no_sandbox: bool,
    /// large-read guard threshold
    pub large_read_lines: usize,
    /// MCP server name → config (spawned lazily)
    pub mcp_servers: HashMap<String, McpServerCfg>,
    /// session id (logs / checkpoints / jobs)
    pub session_id: String,
    /// COSMOS_CLI_HOME
    pub home: PathBuf,
    /// sandbox dial: read-only | workspace-write | full
    pub sandbox_mode: String,
    /// current tool_use id (for bash logs)
    pub current_tool_id: String,
    /// deny-write globs even inside workspace (default ~/.ssh, .env)
    pub deny_write: Vec<String>,
    /// §12 outbound allowlist for tools that talk HTTP (the memory mirror)
    pub allowed_hosts: Vec<String>,
}

#[derive(Clone)]
pub struct McpServerCfg {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

impl ToolContext {
    pub fn new(workspace: &Path) -> Self {
        // Absolute + normalized root — a relative root would let
        // `resolve`'s starts_with check be fooled by `..` components.
        let ws = std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
        let ws = strip_verbatim(&ws);
        ToolContext {
            workspace: ws,
            allowed_roots: Vec::new(),
            sandbox_bin: None,
            read_state: HashMap::new(),
            escalations: Vec::new(),
            net_allow: false,
            sandbox_degraded: None,
            no_sandbox: false,
            large_read_lines: LARGE_READ_LINES,
            mcp_servers: HashMap::new(),
            session_id: String::new(),
            home: crate_home(),
            sandbox_mode: "workspace-write".into(),
            current_tool_id: String::new(),
            deny_write: vec![
                "~/.ssh".into(),
                "~/.gnupg".into(),
                "**/.env".into(),
                "**/.env.*".into(),
            ],
            allowed_hosts: Vec::new(),
        }
    }
}

fn crate_home() -> PathBuf {
    if let Some(p) = std::env::var_os("COSMOS_CLI_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cosmos-cli")
}

/// Truncate to RESULT_CAP with an explicit notice (contract §2).
pub fn cap_result(s: String) -> String {
    if s.len() <= RESULT_CAP {
        s
    } else {
        // back off to a char boundary — slicing mid-UTF-8 would panic
        let mut keep = RESULT_CAP.min(s.len());
        while !s.is_char_boundary(keep) {
            keep -= 1;
        }
        format!(
            "{}\n[truncated: {} bytes > {} cap]",
            &s[..keep],
            s.len(),
            RESULT_CAP
        )
    }
}

/// Resolve a model-supplied path inside the workspace; reject escapes.
pub fn resolve(ctx: &ToolContext, path: &str) -> Result<PathBuf> {
    let p = PathBuf::from(path);
    let joined = if p.is_absolute() {
        p
    } else {
        ctx.workspace.join(p)
    };
    let norm = normalize(&joined);
    let root = normalize(&ctx.workspace);
    let extra = ctx
        .allowed_roots
        .iter()
        .map(|p| normalize(p))
        .any(|p| norm.starts_with(&p));
    if !norm.starts_with(&root) && !extra {
        anyhow::bail!("path escapes workspace: {path}");
    }
    Ok(norm)
}

/// Strip the `\\?\` verbatim prefix that canonicalize produces on Windows.
pub fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or_else(|| p.to_path_buf())
}

/// Lexical normalization (no filesystem access needed).
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Tools callable in plan mode (contract §3).
pub const PLAN_TOOLS: &[&str] = &[
    "read",
    "grep",
    "glob",
    "ls",
    "memory_search",
    "submit_plan",
    "tool_search",
    "context_remaining",
];

/// Read-only tools that may run concurrently.
pub const READONLY_TOOLS: &[&str] = &[
    "read",
    "grep",
    "glob",
    "ls",
    "memory_search",
    "tool_search",
    "context_remaining",
];

pub fn is_readonly(name: &str) -> bool {
    READONLY_TOOLS.contains(&name)
}

/// Execute a tool by name. Permission decisions happen in zcore — this assumes
/// the call was allowed.
pub fn run(ctx: &mut ToolContext, name: &str, input: &Value) -> String {
    if let Some((tool, args)) = unwrap_self_call(name, input) {
        return run(ctx, &tool, &args);
    }
    let r = match name {
        "read" => files::read(ctx, input),
        "edit" => files::edit(ctx, input),
        "write" => files::write(ctx, input),
        "ls" => files::ls(ctx, input),
        "glob" => files::glob(ctx, input),
        "grep" => files::grep(ctx, input),
        "bash" => shell::bash(ctx, input),
        "memory_search" => memory::search_tool(ctx, input),
        "memory_write" => memory::write_tool(ctx, input),
        "tool_search" | "mcp_call" => mcp::run(ctx, name, input),
        "submit_plan" | "task" | "request_escalation" => Err(anyhow::anyhow!(
            "{name} is handled by the harness, not the tool layer"
        )),
        _ => Err(anyhow::anyhow!("unknown tool: {name}")),
    };
    match r {
        Ok(s) => cap_result(s),
        Err(e) => format!("ERROR: {e:#}"),
    }
}
