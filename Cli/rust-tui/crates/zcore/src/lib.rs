//! zcore — harness core for `zcli`: config, session store, permissions,
//! the agent loop, compaction. zdev (Python) and zagy (Go) implement the same
//! semantics from CONTRACT.md — this crate is the Rust reference.

pub mod agent;
pub mod catalog;
pub mod chatgpt;
pub mod config;
pub mod creds;
pub mod hooks;
pub mod live;
pub mod session;
pub mod welcome;

use std::path::PathBuf;

/// CONTRACT.md's system snippet — layer 1 of the stable prefix.
pub fn system_snippet() -> String {
    const CONTRACT_MD: &str = include_str!("../../../../CONTRACT.md");
    const BEGIN: &str = "<!-- BEGIN SYSTEM -->";
    const END: &str = "<!-- END SYSTEM -->";
    let start = CONTRACT_MD.find(BEGIN).map(|i| i + BEGIN.len());
    let end = CONTRACT_MD.find(END);
    match (start, end) {
        (Some(s), Some(e)) if s < e => CONTRACT_MD[s..e].trim().to_string(),
        _ => "You are Cosmos, a coding agent running in the operator's terminal.".into(),
    }
}

/// Project instructions the repo ships for agents — `AGENTS.md` (Codex,
/// Devin, pi), `CLAUDE.md` (Claude Code) and `.zcli/instructions.md` — read
/// from the workspace root, each capped at 32 KB.
pub fn project_instructions(workspace: &std::path::Path) -> Option<String> {
    const CAP: usize = 32 * 1024;
    let mut parts = Vec::new();
    for name in ["AGENTS.md", "CLAUDE.md", ".zcli/instructions.md"] {
        if let Ok(text) = std::fs::read_to_string(workspace.join(name)) {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let body: String = if text.len() > CAP { text.chars().take(CAP).collect() } else { text.to_string() };
            parts.push(format!("<project-instructions file=\"{name}\">\n{body}\n</project-instructions>"));
        }
    }
    (!parts.is_empty()).then(|| format!("The repository provides these instructions; follow them.\n\n{}", parts.join("\n\n")))
}

/// COSMOS_CLI_HOME — shared by zcli, zdev, zagy (contract §1).
pub fn cli_home() -> PathBuf {
    if let Some(p) = std::env::var_os("COSMOS_CLI_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cosmos-cli")
}

/// Tests that set process-wide env vars take this lock so they don't race.
#[cfg(test)]
pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    L.lock().unwrap_or_else(|e| e.into_inner())
}
