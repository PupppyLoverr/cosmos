//! `boot.context` — the single source of the stable prompt prefix (layer 3)
//! for all three harnesses. Budget: ≤ 2,000 o200k tokens.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::tokens;
use crate::vault::Vault;

/// Token budget for the assembled boot context (contract §4 bridge layer).
pub const BOOT_BUDGET: usize = 2000;

const CONTRACT_MD: &str = include_str!("../../CONTRACT.md");

/// The `<!-- BEGIN SYSTEM --> … <!-- END SYSTEM -->` block of CONTRACT.md.
pub fn system_snippet() -> String {
    const BEGIN: &str = "<!-- BEGIN SYSTEM -->";
    const END: &str = "<!-- END SYSTEM -->";
    let start = CONTRACT_MD.find(BEGIN).map(|i| i + BEGIN.len());
    let end = CONTRACT_MD.find(END);
    match (start, end) {
        (Some(s), Some(e)) if s < e => CONTRACT_MD[s..e].trim().to_string(),
        _ => "You are Cosmos, a coding agent running in the operator's terminal."
            .to_string(),
    }
}

/// The `## …` schema + retrieval section of MERIDIAN.md (everything from the
/// first `##` heading on), so the model knows the vault layout.
fn meridian_schema(vault: &Vault) -> String {
    let Ok(text) = fs::read_to_string(vault.meridian_md()) else {
        return String::new();
    };
    match text.find("\n## ") {
        Some(i) => text[i..].trim().to_string(),
        None => text.trim().to_string(),
    }
}

fn parse_frontmatter(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let mut name = String::new();
    let mut desc = String::new();
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            match k.trim() {
                "name" => name = v.trim().trim_matches('"').to_string(),
                "description" => desc = v.trim().trim_matches('"').to_string(),
                _ => {}
            }
        }
    }
    if name.is_empty() && desc.is_empty() {
        None
    } else {
        Some((name, desc))
    }
}

/// `name — description — path` lines for every SKILL.md in the repo and home.
/// Descriptions are capped at 120 chars — the boot index is a directory, not
/// the body (the model reads SKILL.md via `read` when it needs detail).
fn skill_index(repo: &Path) -> Vec<String> {
    let mut dirs = vec![repo.join(".agents").join("skills")];
    if let Some(home) = std::env::var_os("COSMOS_CLI_HOME") {
        dirs.push(PathBuf::from(home).join("skills"));
    }
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let skill = e.path().join("SKILL.md");
            let Ok(text) = fs::read_to_string(&skill) else { continue };
            let Some((name, desc)) = parse_frontmatter(&text) else { continue };
            let name = if name.is_empty() {
                e.file_name().to_string_lossy().to_string()
            } else {
                name
            };
            let desc = if desc.chars().count() > 120 {
                format!("{}…", desc.chars().take(120).collect::<String>())
            } else {
                desc
            };
            out.push(format!("- {name} — {desc} — {}", skill.display()));
        }
    }
    out.sort();
    out
}

fn agents_md(repo: &Path) -> String {
    let mut parts = Vec::new();
    let root = repo.join("AGENTS.md");
    if let Ok(t) = fs::read_to_string(&root) {
        parts.push(t);
    }
    // Nearest AGENTS.md to cwd if different from repo root.
    if let Ok(cwd) = std::env::current_dir() {
        let local = cwd.join("AGENTS.md");
        if local != root {
            if let Ok(t) = fs::read_to_string(&local) {
                parts.push(t);
            }
        }
    }
    parts.join("\n\n")
}

#[derive(Debug, Serialize)]
pub struct BootContext {
    pub text: String,
    pub tokens: usize,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Assemble the boot context: system snippet + MERIDIAN.md schema + skills +
/// trimmed AGENTS.md, capped at [`BOOT_BUDGET`] tokens.
pub fn context(vault: &Vault, repo: &Path, _harness: &str) -> Result<BootContext> {
    let mut warnings = Vec::new();
    let system = system_snippet();
    let schema = meridian_schema(vault);
    let skills = skill_index(repo);
    let skills_block = if skills.is_empty() {
        String::new()
    } else {
        format!("\n\n## Skills\n{}", skills.join("\n"))
    };

    let mut head = format!("{system}\n\n## Meridian vault\n{schema}{skills_block}");

    // If skills alone blow the budget, keep only names (the index entry shape
    // stays `name — — path`); AGENTS.md then competes for what is left.
    if tokens::count(&head) > BOOT_BUDGET && !skills.is_empty() {
        let names: Vec<String> = skills
            .iter()
            .map(|l| {
                l.split(" — ")
                    .next()
                    .unwrap_or(l.as_str())
                    .to_string()
            })
            .collect();
        let compact = format!("\n\n## Skills\n{}", names.join("\n"));
        let candidate = format!("{system}\n\n## Meridian vault\n{schema}{compact}");
        if tokens::count(&candidate) <= tokens::count(&head) {
            head = candidate;
            warnings.push(
                "skill descriptions trimmed to names to fit the boot budget".to_string(),
            );
        }
    }

    let mut body = agents_md(repo);
    let mut text = if body.trim().is_empty() {
        head.clone()
    } else {
        format!("{head}\n\n## AGENTS.md\n{body}")
    };
    let mut total = tokens::count(&text);
    if total > BOOT_BUDGET && !body.is_empty() {
        // Trim AGENTS.md to fit; report a warning so the operator can move the
        // overflow into Meridian playbooks.
        let mut lo = 0usize;
        let mut hi = body.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            let candidate = format!("{head}\n\n## AGENTS.md\n{}\n\n[truncated]", &body[..mid]);
            if tokens::count(&candidate) <= BOOT_BUDGET {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let cut = body[..lo].rfind('\n').unwrap_or(lo);
        body.truncate(cut);
        text = format!("{head}\n\n## AGENTS.md\n{body}\n\n[AGENTS.md truncated to fit the 2000-token boot budget — move detail into Meridian playbooks]");
        total = tokens::count(&text);
        warnings.push(
            "AGENTS.md truncated to fit the boot budget; move detail into Meridian playbooks"
                .to_string(),
        );
    }
    // Last resort: the head itself (schema + names-only skills) can still
    // overflow on pathological vaults. Hard-truncate the tail.
    if total > BOOT_BUDGET {
        let mut lo = 0usize;
        let mut hi = text.len();
        while lo < hi {
            let mid = (lo + hi) / 2;
            if tokens::count(&text[..mid]) <= BOOT_BUDGET {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let cut = text[..lo].rfind('\n').unwrap_or(lo);
        text.truncate(cut);
        total = tokens::count(&text);
        warnings.push(format!("boot context hard-truncated to {total} tokens"));
    }
    Ok(BootContext {
        text,
        tokens: total,
        warnings,
    })
}
