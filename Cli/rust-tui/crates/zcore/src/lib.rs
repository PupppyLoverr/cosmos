//! zcore — harness core for `cosmostui`: config, session store, permissions,
//! the agent loop, compaction. zdev (Python) and zagy (Go) implement the same
//! semantics from CONTRACT.md — this crate is the Rust reference.

pub mod acp_client;
pub mod agent;
pub mod boot;
pub mod catalog;
pub mod chatgpt;
pub mod compact;
pub mod config;
pub mod creds;
#[cfg(feature = "devin-direct")]
pub mod devin;
pub mod hooks;
pub mod improve;
pub mod live;
pub mod session;
pub mod welcome;
pub mod world;

use std::path::{Path, PathBuf};

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

pub fn init_project(workspace: &std::path::Path) -> std::io::Result<String> {
    let path = workspace.join("AGENTS.md");
    if path.exists() {
        return Ok("AGENTS.md already exists".into());
    }
    let mut commands = Vec::new();
    if workspace.join("package.json").exists() {
        commands.push("Build/test: inspect package.json scripts (for example, `npm run build` and `npm test`).");
    }
    if workspace.join("Cargo.toml").exists() {
        commands.push("Build/test: `cargo build` and `cargo test`.");
    }
    if workspace.join("go.mod").exists() {
        commands.push("Build/test: `go build ./...` and `go test ./...`.");
    }
    if workspace.join("Makefile").exists() {
        commands.push("Build/test: inspect the Makefile targets.");
    }
    if commands.is_empty() {
        commands.push("Build/test: document the repository's build and test commands.");
    }
    let claude = if workspace.join("CLAUDE.md").exists() {
        "\n\nSee `CLAUDE.md` for additional repository instructions."
    } else {
        ""
    };
    let body = format!(
        "# Agent instructions\n\n## Project summary\n\n<!-- Describe this project's purpose and key directories. -->\n\n## Build and test\n\n{}{}\n",
        commands.join("\n"), claude
    );
    std::fs::write(path, body)?;
    Ok("created AGENTS.md".into())
}

/// The command users type, and the `harness` name in session events.
/// (`cosmos` alone is the desktop app's launcher — never take that name.)
pub const NAME: &str = "cosmostui";

/// `COSMOS_<key>`, falling back to the pre-rename `ZCLI_<key>`.
pub fn env_var(key: &str) -> Option<String> {
    std::env::var(format!("COSMOS_{key}"))
        .or_else(|_| std::env::var(format!("ZCLI_{key}")))
        .ok()
}

/// A file in COSMOS_CLI_HOME that had a `cosmos` name before the rename.
/// The legacy file is moved over the first time; if that fails (another
/// harness holds it) it is used where it is.
pub fn home_file(name: &str, legacy: &str) -> PathBuf {
    let home = cli_home();
    let p = home.join(name);
    if !p.exists() {
        let old = home.join(legacy);
        if old.exists() && std::fs::rename(&old, &p).is_err() {
            return old;
        }
    }
    p
}

/// Project-level config dir: `.cosmos/<rel>`, or the legacy `.zcli/<rel>`
/// when only that exists.
pub fn project_path(root: &Path, rel: &str) -> PathBuf {
    let p = root.join(".cosmos").join(rel);
    let old = root.join(".zcli").join(rel);
    if !p.exists() && old.exists() {
        old
    } else {
        p
    }
}

/// COSMOS_CLI_HOME — shared by cosmos, zdev, zagy (contract §1).
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
