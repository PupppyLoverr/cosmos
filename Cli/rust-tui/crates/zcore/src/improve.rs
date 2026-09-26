//! Bounded self-improve: skills, preferences, corrections. Never rewrite
//! the binary, CONTRACT.md, or sandbox policy without `--allow-self-mutate`.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::world::World;

#[derive(Debug, Clone)]
pub struct SelfImproveCfg {
    pub enabled: bool,
    pub write_skills: bool,
    pub write_preferences: bool,
    pub write_corrections: bool,
    pub require_gate: bool,
    pub allow_self_mutate: bool,
    pub auto_writes: u32,
}

impl Default for SelfImproveCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            write_skills: true,
            write_preferences: true,
            write_corrections: true,
            require_gate: false,
            allow_self_mutate: false,
            auto_writes: 0,
        }
    }
}

pub const AUTO_WRITE_CAP: u32 = 3;

const FORBIDDEN: &[&str] = &[
    "CONTRACT.md",
    "Cli/CONTRACT.md",
    "zsandbox",
    "auth_cmd.rs",
    "creds.rs",
    "oauth.ts",
    "wincred.ts",
];

pub fn forbidden_path(path: &str) -> bool {
    FORBIDDEN.iter().any(|f| path.contains(f))
}

pub fn skill_dir(home: &Path, name: &str, draft: bool) -> PathBuf {
    if draft {
        home.join("skills").join("drafts").join(name)
    } else {
        home.join("skills").join(name)
    }
}

/// Distill a session into `skills/<name>/SKILL.md`.
pub fn skillify(
    home: &Path,
    name: &str,
    world: &World,
    draft: bool,
    allow_self_mutate: bool,
) -> Result<(PathBuf, Value), String> {
    if !allow_self_mutate && forbidden_path(name) {
        return Err("self-improve refused: target is fenced".into());
    }
    let dir = skill_dir(home, name, draft);
    if !draft && dir.join("SKILL.md").is_file() {
        return Err(format!(
            "skill {name} exists — use /refine, do not overwrite"
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let body = format!(
        "---\nname: {name}\ndescription: \"{}\"\n---\n\n# {name}\n\n## Goal\n{}\n\n## Files\n{}\n\n## Next\n{}\n",
        world.goal.chars().take(120).collect::<String>().replace('"', "'"),
        world.goal,
        world
            .files
            .iter()
            .map(|(p, h)| format!("- {p} ({h})"))
            .collect::<Vec<_>>()
            .join("\n"),
        world.next,
    );
    if hooks_secret_like(&body) {
        return Err("self-improve refused: secret scanner".into());
    }
    let dest = dir.join("SKILL.md");
    std::fs::write(&dest, body).map_err(|e| e.to_string())?;
    Ok((
        dest,
        json!({"kind": "skill", "name": name, "draft": draft}),
    ))
}

/// Taste signal: user rejected an edit.
pub fn preference_bullet(action: &str) -> String {
    format!("do not do {action} in this repo")
}

/// Failure mining: same command failed twice, or STALE twice.
pub fn correction_row(command: &str, fix: &str) -> String {
    format!("command `{command}` failed twice; fix: {fix}")
}

/// Cheap secret scan (Meridian has the real one; this is a local fence).
pub fn hooks_secret_like(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("sk-") && t.contains("api")
        || t.contains("begin private key")
        || t.contains("aws_secret")
}

/// Event payload for JSONL `kind=self_improve`.
pub fn event_data(surface: &str, detail: Value) -> Value {
    json!({"surface": surface, "detail": detail})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skillify_writes_draft() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = World::default();
        w.goal = "fix the login form".into();
        let (p, _) = skillify(dir.path(), "login-form", &w, true, false).unwrap();
        let body = std::fs::read_to_string(p).unwrap();
        assert!(body.contains("fix the login form"));
        assert!(body.contains("name: login-form"));
    }

    #[test]
    fn will_not_overwrite_named() {
        let dir = tempfile::tempdir().unwrap();
        let w = World::default();
        skillify(dir.path(), "exists", &w, false, false).unwrap();
        let err = skillify(dir.path(), "exists", &w, false, false).unwrap_err();
        assert!(err.contains("exists"));
    }
}
