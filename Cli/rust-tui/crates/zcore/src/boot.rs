//! Layer 3 of the stable prefix (contract §4): what the model knows about
//! this repo and this operator before the first turn. Built once per
//! session, in-process, and never changed mid-session.
//!
//! Progressive disclosure: skills are listed as `name — description` (the
//! body is read with `read` when needed), deferred tools by name only, the
//! repo as a short map. Budget: `BOOT_BUDGET` tokens (chars / 4).

use std::path::{Path, PathBuf};

/// Hard cap for the whole boot context, in estimated tokens.
pub const BOOT_BUDGET: usize = 2000;
const REPO_MAP_TOKENS: usize = 400;
const MEMORY_TOKENS: usize = 300;
const DESC_CHARS: usize = 120;

pub struct Boot {
    pub text: String,
    pub tokens: u64,
    pub warnings: Vec<String>,
}

pub fn est(s: &str) -> usize {
    s.len().div_ceil(4)
}

fn frontmatter(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let (mut name, mut desc) = (String::new(), String::new());
    for l in lines {
        let l = l.trim();
        if l == "---" {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            let v = v.trim().trim_matches('"').to_string();
            match k.trim() {
                "name" => name = v,
                "description" => desc = v,
                _ => {}
            }
        }
    }
    (!name.is_empty() || !desc.is_empty()).then_some((name, desc))
}

/// Skill directories, most specific first.
pub fn skill_dirs(repo: &Path, home: &Path) -> Vec<PathBuf> {
    vec![
        repo.join(".agents").join("skills"),
        repo.join(".cosmos").join("skills"),
        repo.join(".claude").join("skills"),
        home.join("skills"),
    ]
}

/// `(name, description, dir)` for every SKILL.md; a repo skill shadows a
/// home skill of the same name.
pub fn skills(repo: &Path, home: &Path) -> Vec<(String, String, PathBuf)> {
    let mut out: Vec<(String, String, PathBuf)> = Vec::new();
    for dir in skill_dirs(repo, home) {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let Ok(text) = std::fs::read_to_string(p.join("SKILL.md")) else { continue };
            let Some((name, desc)) = frontmatter(&text) else { continue };
            let name = if name.is_empty() { p.file_name().unwrap_or_default().to_string_lossy().to_string() } else { name };
            if out.iter().any(|(n, _, _)| *n == name) {
                continue;
            }
            let desc = if desc.chars().count() > DESC_CHARS {
                format!("{}…", desc.chars().take(DESC_CHARS).collect::<String>())
            } else {
                desc
            };
            out.push((name, desc, dir.clone()));
        }
    }
    out
}

/// Top-level layout: directories (with a few children) and files,
/// .gitignore-aware, cut at `REPO_MAP_TOKENS`.
fn repo_map(repo: &Path) -> String {
    let walk = |root: &Path| -> Vec<(String, bool)> {
        let mut v: Vec<(String, bool)> = ignore::WalkBuilder::new(root)
            .max_depth(Some(1))
            .hidden(true)
            .build()
            .flatten()
            .filter(|e| e.depth() == 1)
            .map(|e| (e.file_name().to_string_lossy().to_string(), e.file_type().is_some_and(|t| t.is_dir())))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    };
    let mut out = String::new();
    for (name, is_dir) in walk(repo) {
        let line = if is_dir {
            let kids: Vec<String> = walk(&repo.join(&name))
                .into_iter()
                .take(8)
                .map(|(n, d)| if d { format!("{n}/") } else { n })
                .collect();
            format!("{name}/ {}\n", kids.join(" "))
        } else {
            format!("{name}\n")
        };
        if est(&out) + est(&line) > REPO_MAP_TOKENS {
            out.push_str("…\n");
            break;
        }
        out.push_str(&line);
    }
    out
}

/// Repo instructions: AGENTS.md (root, then nearest to cwd), CLAUDE.md,
/// `.cosmos/instructions.md`. Each file once.
fn instructions(repo: &Path) -> Vec<(String, String)> {
    let mut paths = vec![repo.join("AGENTS.md")];
    if let Ok(cwd) = std::env::current_dir() {
        if cwd.starts_with(repo) && cwd != repo {
            paths.push(cwd.join("AGENTS.md"));
        }
    }
    paths.push(repo.join("CLAUDE.md"));
    paths.push(crate::project_path(repo, "instructions.md"));
    let mut out: Vec<(String, String)> = Vec::new();
    for p in paths {
        let Ok(t) = std::fs::read_to_string(&p) else { continue };
        let t = t.trim();
        if t.is_empty() || out.iter().any(|(_, b)| b == t) {
            continue;
        }
        let label = p.strip_prefix(repo).unwrap_or(&p).display().to_string().replace('\\', "/");
        out.push((label, t.to_string()));
    }
    out
}

/// Preferences and corrections the operator taught earlier, newest first —
/// the "gets better over time" part of the prefix, kept small.
fn memories(home: &Path, repo: &Path) -> String {
    let repo_scope = format!("repo:{}", repo.file_name().unwrap_or_default().to_string_lossy());
    let scopes = ["global", repo_scope.as_str()];
    let mut out = String::new();
    for e in ztools::memory::recent(home, &["preference", "correction", "playbook", "decision"], &scopes, 24) {
        let line = format!("- [{}] {}\n", e.kind, e.text);
        if est(&out) + est(&line) > MEMORY_TOKENS {
            break;
        }
        out.push_str(&line);
    }
    out
}

fn cut_to(s: &str, tokens: usize) -> &str {
    let mut end = (tokens * 4).min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    match s[..end].rfind('\n') {
        Some(nl) if end < s.len() && nl > end / 2 => &s[..nl],
        _ => &s[..end],
    }
}

/// Assemble layer 3 for `repo`.
pub fn build(repo: &Path, home: &Path) -> Boot {
    let mut warnings = Vec::new();
    let mut parts: Vec<String> = Vec::new();

    let map = repo_map(repo);
    if !map.is_empty() {
        parts.push(format!("## Repo {}\n{}", repo.display(), map.trim_end()));
    }
    let sk = skills(repo, home);
    if !sk.is_empty() {
        let lines: Vec<String> = sk
            .iter()
            .map(|(n, d, _)| if d.is_empty() { format!("- {n}") } else { format!("- {n} — {d}") })
            .collect();
        let dirs: Vec<String> = {
            let mut d: Vec<String> = sk.iter().map(|(_, _, dir)| dir.display().to_string()).collect();
            d.dedup();
            d
        };
        parts.push(format!(
            "## Skills\nRead <dir>/<name>/SKILL.md before using one (dirs: {}).\n{}",
            dirs.join(", "),
            lines.join("\n")
        ));
    }
    let mem = memories(home, repo);
    if !mem.is_empty() {
        parts.push(format!("## Operator memory\n{}", mem.trim_end()));
    }
    parts.push(format!("## Shell\n`bash` runs {} in the workspace.", ztools::shell::shell_name()));
    parts.push(format!(
        "## More tools\n{} — find with tool_search, call with mcp_call {{\"server\": \"{}\", \"tool\", \"arguments\"}}.",
        ztools::DEFERRED_TOOLS.join(", "),
        ztools::SELF_SERVER
    ));

    // instructions get whatever budget is left
    let head = parts.join("\n\n");
    let mut left = BOOT_BUDGET.saturating_sub(est(&head) + 8);
    for (label, body) in instructions(repo) {
        let block_head = format!("## {label}\n");
        let room = left.saturating_sub(est(&block_head));
        if room < 40 {
            warnings.push(format!("{label} left out: boot budget ({BOOT_BUDGET} tokens) is full"));
            continue;
        }
        let body_cut = cut_to(&body, room);
        if body_cut.len() < body.len() {
            warnings.push(format!(
                "{label} truncated to fit the {BOOT_BUDGET}-token boot budget — move detail into skills or memory"
            ));
            parts.push(format!("{block_head}{body_cut}\n[{label} truncated]"));
        } else {
            parts.push(format!("{block_head}{body_cut}"));
        }
        left = left.saturating_sub(est(parts.last().unwrap()));
    }
    let text = parts.join("\n\n");
    Boot { tokens: est(&text) as u64, text, warnings }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_is_small_and_ordered() {
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let r = repo.path();
        std::fs::create_dir_all(r.join("src")).unwrap();
        std::fs::write(r.join("src/main.rs"), "fn main(){}").unwrap();
        std::fs::write(r.join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(r.join("AGENTS.md"), "Run cargo test before you finish.").unwrap();
        let sk = home.path().join("skills/release");
        std::fs::create_dir_all(&sk).unwrap();
        std::fs::write(sk.join("SKILL.md"), "---\nname: release\ndescription: cut a release tag\n---\nsteps…").unwrap();
        std::env::set_var("COSMOS_MEMORY_DIR", home.path().join("mem"));
        ztools::memory::add(home.path(), "preference", "use tabs in Makefiles", "global", "t").unwrap();
        let b = build(r, home.path());
        std::env::remove_var("COSMOS_MEMORY_DIR");
        assert!(b.text.contains("src/ main.rs"), "{}", b.text);
        assert!(b.text.contains("- release — cut a release tag"), "{}", b.text);
        assert!(!b.text.contains("steps…"), "skill body is not in the prefix");
        assert!(b.text.contains("- [preference] use tabs in Makefiles"), "{}", b.text);
        assert!(b.text.contains("memory_search") && b.text.contains("mcp_call"), "{}", b.text);
        assert!(b.text.contains("## AGENTS.md\nRun cargo test"), "{}", b.text);
        assert!(b.tokens < 400, "{}", b.tokens);
        assert!(b.warnings.is_empty());
    }

    #[test]
    fn huge_agents_md_is_truncated_with_warning() {
        let repo = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(repo.path().join("AGENTS.md"), "rule\n".repeat(5000)).unwrap();
        let b = build(repo.path(), home.path());
        assert!(b.tokens as usize <= BOOT_BUDGET + 20, "{}", b.tokens);
        assert!(b.warnings.iter().any(|w| w.contains("AGENTS.md truncated")));
    }
}
