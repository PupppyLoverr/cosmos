//! Bounded self-improvement (contract §13): the harness gets better at this
//! operator's work by writing *skills* (playbooks as `SKILL.md`) and
//! *memory bullets* (preference / correction / playbook / decision). It never
//! edits its binary, `CONTRACT.md`, config or sandbox policy — the write and
//! edit tools fence those without `--allow-self-mutate`.
//!
//! Three ways in:
//! - at run time, a correction the agent just learned (an edit that went
//!   STALE twice) becomes a `correction` bullet;
//! - `/skillify <name>` distils the current session into a skill now;
//! - after an interactive session ends cleanly, `cosmostui review --session`
//!   runs detached: deterministic extraction always, plus one model call
//!   (when a real model is configured) that may add at most five bullets and
//!   one new skill. Everything is deduplicated and bounded.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{json, Value};
use zproto::{Chunk, Event, Message, Role};

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

/// Run-time memory writes per session.
pub const AUTO_WRITE_CAP: u32 = 3;
/// Review-pass limits.
pub const REVIEW_MAX_MEMORIES: usize = 5;
pub const SKILL_MAX_LINES: usize = 200;

/// Memory types a review may write (`map` is the operator's own).
pub const REVIEW_TYPES: &[&str] = &["preference", "correction", "playbook", "decision"];

pub fn skill_dir(home: &Path, name: &str, draft: bool) -> PathBuf {
    if draft {
        home.join("skills").join("drafts").join(name)
    } else {
        home.join("skills").join(name)
    }
}

/// `Fix the Login Form!` → `fix-the-login-form` (≤ 48 chars).
pub fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
        if s.len() >= 48 {
            break;
        }
    }
    s.trim_matches('-').to_string()
}

/// Repo scope for memory bullets: `repo:<folder name>`.
pub fn repo_scope(workspace: &Path) -> String {
    format!("repo:{}", workspace.file_name().unwrap_or_default().to_string_lossy())
}

fn write_skill(home: &Path, name: &str, description: &str, body: &str, draft: bool) -> Result<PathBuf, String> {
    let name = slug(name);
    if name.is_empty() {
        return Err("skill needs a name".into());
    }
    let dir = skill_dir(home, &name, draft);
    if !draft && dir.join("SKILL.md").is_file() {
        return Err(format!("skill {name} exists — edit it by hand, it is never overwritten"));
    }
    let description = description.replace(['"', '\n'], " ");
    let body: String = body.lines().take(SKILL_MAX_LINES).collect::<Vec<_>>().join("\n");
    let doc = format!("---\nname: {name}\ndescription: \"{}\"\n---\n\n{}\n", description.trim(), body.trim());
    if let Err(why) = ztools::memory::scan(&doc) {
        return Err(format!("self-improve refused: {why}"));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let dest = dir.join("SKILL.md");
    std::fs::write(&dest, doc).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// What a session did, in the order it happened — the raw material for a
/// skill and for the review prompt.
#[derive(Debug, Default, Clone)]
pub struct Digest {
    pub goal: String,
    pub user_lines: Vec<String>,
    /// successful steps: `read src/a.rs`, `bash cargo test`, …
    pub steps: Vec<String>,
    pub failures: Vec<String>,
    pub files: Vec<String>,
    pub last_reply: String,
}

pub fn digest(events: &[Event]) -> Digest {
    let mut d = Digest::default();
    let mut calls: std::collections::HashMap<String, (String, Value)> = std::collections::HashMap::new();
    for e in events {
        match e.kind.as_str() {
            "user" if e.data["synthetic"] != true => {
                if let Some(t) = e.data["text"].as_str() {
                    let t = t.trim();
                    if t.starts_with('/') || t.is_empty() {
                        continue;
                    }
                    if d.goal.is_empty() {
                        d.goal = t.chars().take(300).collect();
                    }
                    d.user_lines.push(t.chars().take(400).collect());
                }
            }
            "assistant" => {
                if let Some(t) = e.data["text"].as_str().filter(|t| !t.trim().is_empty()) {
                    d.last_reply = t.chars().take(600).collect();
                }
                for c in e.data["tool_calls"].as_array().cloned().unwrap_or_default() {
                    let name = c["name"].as_str().unwrap_or("").to_string();
                    let input = c["input"].clone();
                    let (name, input) = ztools::unwrap_self_call(&name, &input).unwrap_or((name, input));
                    calls.insert(c["id"].as_str().unwrap_or("").to_string(), (name, input));
                }
            }
            "tool_result" => {
                let id = e.data["tool_use_id"].as_str().unwrap_or("");
                let Some((name, input)) = calls.get(id) else { continue };
                let what = input["path"]
                    .as_str()
                    .or(input["command"].as_str())
                    .or(input["pattern"].as_str())
                    .unwrap_or("")
                    .chars()
                    .take(120)
                    .collect::<String>();
                let step = format!("{name} {what}").trim().to_string();
                let content = e.data["content"].as_str().unwrap_or("");
                let failed = e.data["is_error"] == true
                    || content.contains("STALE")
                    || (name == "bash" && !content.starts_with("exit_code=0") && content.starts_with("exit_code="));
                if failed {
                    d.failures.push(format!("{step}: {}", content.lines().next().unwrap_or("").chars().take(160).collect::<String>()));
                } else if !d.steps.contains(&step) {
                    d.steps.push(step);
                }
                if matches!(name.as_str(), "write" | "edit") && !what.is_empty() && !d.files.contains(&what) {
                    d.files.push(what);
                }
            }
            _ => {}
        }
    }
    d
}

/// A skill body from what the session actually did.
fn skill_body(name: &str, d: &Digest, world: &World) -> String {
    let goal = if d.goal.is_empty() { world.goal.clone() } else { d.goal.clone() };
    let mut s = format!("# {name}\n\n## When to use\n{goal}\n\n## Steps that worked\n");
    for (i, step) in d.steps.iter().take(40).enumerate() {
        s.push_str(&format!("{}. `{step}`\n", i + 1));
    }
    if !d.files.is_empty() {
        s.push_str(&format!("\n## Files involved\n{}\n", d.files.iter().take(30).map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n")));
    }
    if !d.failures.is_empty() {
        s.push_str(&format!("\n## Pitfalls seen\n{}\n", d.failures.iter().take(10).map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n")));
    }
    if !world.next.is_empty() {
        s.push_str(&format!("\n## Open next step\n{}\n", world.next));
    }
    s
}

/// `/skillify <name>`: distil this session into `skills/<name>/SKILL.md`.
pub fn skillify(home: &Path, name: &str, world: &World, events: &[Event], draft: bool) -> Result<(PathBuf, Value), String> {
    let d = digest(events);
    let name = slug(name);
    let description = if d.goal.is_empty() { world.goal.clone() } else { d.goal.clone() };
    let description: String = description.chars().take(120).collect();
    let dest = write_skill(home, &name, &description, &skill_body(&name, &d, world), draft)?;
    Ok((dest, json!({"kind": "skill", "name": name, "draft": draft, "steps": d.steps.len()})))
}

/// Taste signal: user rejected an edit.
pub fn preference_bullet(action: &str) -> String {
    format!("do not do {action} in this repo")
}

/// Failure mining: same command failed twice, or STALE twice.
pub fn correction_row(command: &str, fix: &str) -> String {
    format!("`{command}` failed twice; fix: {fix}")
}

/// Event payload for JSONL `kind=self_improve`.
pub fn event_data(surface: &str, detail: Value) -> Value {
    json!({"surface": surface, "detail": detail})
}

/// Operator instructions that read like standing preferences.
fn preference_lines(d: &Digest) -> Vec<String> {
    const CUES: &[&str] = &["always ", "never ", "don't ", "do not ", "dont ", "prefer ", "please always ", "please never ", "from now on"];
    let mut out = Vec::new();
    for l in &d.user_lines {
        for sentence in l.split(['.', '\n', '!']) {
            let s = sentence.trim();
            let low = s.to_lowercase();
            if s.len() >= 12 && s.len() <= 240 && CUES.iter().any(|c| low.starts_with(c) || low.contains(&format!(" {c}"))) {
                out.push(s.to_string());
            }
        }
    }
    out.dedup();
    out
}

/// Pull the JSON object out of a model reply that may wrap it in prose.
fn json_in(text: &str) -> Option<Value> {
    let (a, b) = (text.find('{')?, text.rfind('}')?);
    serde_json::from_str(&text[a..=b]).ok()
}

const REVIEW_PROMPT: &str = "You review a finished coding session to make a coding agent better next time. \
Reply with one JSON object and nothing else: \
{\"memories\": [{\"type\": \"preference|correction|playbook|decision\", \"text\": \"...\"}], \
\"skill\": {\"name\": \"kebab-name\", \"description\": \"one line\", \"body\": \"markdown steps\"} or null}. \
Memories: at most 5, each one durable fact about how this operator or repo works (not about this one task), \
under 200 characters. Skill: only if the session shows a reusable multi-step procedure; otherwise null.";

/// Outcome of a review pass.
#[derive(Debug, Default)]
pub struct Review {
    pub memories: Vec<(String, String)>,
    pub skill: Option<PathBuf>,
    pub notes: Vec<String>,
}

/// The post-session pass. `provider` = None (or fixture/ACP) → deterministic
/// only. Writes at most `REVIEW_MAX_MEMORIES` new bullets and one new skill.
pub fn review(home: &Path, workspace: &Path, events: &[Event], provider: Option<(&zproviders::ProviderCfg, &[String])>) -> Result<Review> {
    let d = digest(events);
    let mut out = Review::default();
    if d.user_lines.is_empty() {
        out.notes.push("nothing to review".into());
        return Ok(out);
    }
    let scope = repo_scope(workspace);
    let known: std::collections::HashSet<String> = ztools::memory::all(home).into_iter().map(|e| e.text.to_lowercase()).collect();
    let mut candidates: Vec<(String, String)> = preference_lines(&d).into_iter().map(|t| ("preference".to_string(), t)).collect();
    let mut skill: Option<(String, String, String)> = None;

    let live = provider.filter(|(p, _)| !matches!(p.kind, zproviders::ProviderKind::Fixture | zproviders::ProviderKind::Acp | zproviders::ProviderKind::Devin));
    if let Some((cfg, allowed)) = live {
        let digest_text = format!(
            "goal: {}\noperator said:\n{}\nsteps that worked:\n{}\nfailures:\n{}\nfiles changed: {}\nlast reply: {}",
            d.goal,
            d.user_lines.iter().take(20).map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n"),
            d.steps.iter().take(40).map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n"),
            d.failures.iter().take(15).map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n"),
            d.files.join(", "),
            d.last_reply
        );
        let digest_text: String = digest_text.chars().take(8000).collect();
        let msgs = vec![Message::text(Role::User, digest_text)];
        let req = zproviders::Request { system: vec![REVIEW_PROMPT.into()], tools: &[], messages: &msgs, session_id: "review", max_tokens: 1200 };
        let mut text = String::new();
        match zproviders::complete(cfg, &req, &mut |c| {
            if let Chunk::Text(t) = c {
                text.push_str(&t)
            }
        }, allowed)
        {
            Ok(()) => match json_in(&text) {
                Some(v) => {
                    for m in v["memories"].as_array().cloned().unwrap_or_default() {
                        if let (Some(t), Some(x)) = (m["type"].as_str(), m["text"].as_str()) {
                            if REVIEW_TYPES.contains(&t) && x.len() <= 300 {
                                candidates.push((t.to_string(), x.trim().to_string()));
                            }
                        }
                    }
                    if let (Some(n), Some(body)) = (v["skill"]["name"].as_str(), v["skill"]["body"].as_str()) {
                        skill = Some((n.to_string(), v["skill"]["description"].as_str().unwrap_or("").to_string(), body.to_string()));
                    }
                }
                None => out.notes.push("model reply had no JSON — kept the deterministic results".into()),
            },
            Err(e) => out.notes.push(format!("model review skipped: {e:#}")),
        }
    }

    for (kind, text) in candidates {
        if out.memories.len() >= REVIEW_MAX_MEMORIES {
            break;
        }
        let low = text.to_lowercase();
        if known.contains(&low) || out.memories.iter().any(|(_, t)| t.to_lowercase() == low) {
            continue;
        }
        match ztools::memory::add(home, &kind, &text, &scope, "cosmostui/review") {
            Ok(_) => out.memories.push((kind, text)),
            Err(e) => out.notes.push(format!("memory rejected: {e:#}")),
        }
    }
    if let Some((name, desc, body)) = skill {
        match write_skill(home, &name, &desc, &body, false) {
            Ok(p) => out.skill = Some(p),
            Err(e) => out.notes.push(e),
        }
    }
    Ok(out)
}

/// Start `cosmostui review --session <id>` detached: it outlives the TUI
/// and never touches the terminal.
pub fn spawn_review(session: &str) {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut c = std::process::Command::new(exe);
    c.args(["review", "--session", session])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        c.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    }
    let _ = c.spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, data: Value) -> Event {
        Event::new("s", "cosmostui", None, kind, data)
    }

    fn session() -> Vec<Event> {
        vec![
            ev("user", json!({"text": "Fix the flaky login test. Always run cargo fmt before committing."})),
            ev("assistant", json!({"text": "", "tool_calls": [
                {"id": "1", "name": "read", "input": {"path": "src/login.rs"}},
                {"id": "2", "name": "edit", "input": {"path": "src/login.rs"}},
                {"id": "3", "name": "mcp_call", "input": {"server": "cosmos", "tool": "bash", "arguments": {"command": "cargo test login"}}}]})),
            ev("tool_result", json!({"tool_use_id": "1", "content": "file_sha256=abc lines=10", "is_error": false})),
            ev("tool_result", json!({"tool_use_id": "2", "content": "STALE: anchor", "is_error": true})),
            ev("tool_result", json!({"tool_use_id": "3", "content": "exit_code=0 log_path=x", "is_error": false})),
            ev("assistant", json!({"text": "Fixed: the test now waits for the server.", "tool_calls": []})),
        ]
    }

    #[test]
    fn digest_keeps_order_failures_and_files() {
        let d = digest(&session());
        assert_eq!(d.goal, "Fix the flaky login test. Always run cargo fmt before committing.");
        assert_eq!(d.steps, ["read src/login.rs", "bash cargo test login"]);
        assert_eq!(d.failures.len(), 1);
        assert!(d.failures[0].starts_with("edit src/login.rs: STALE"));
        assert_eq!(d.files, ["src/login.rs"]);
        assert!(d.last_reply.starts_with("Fixed"));
    }

    #[test]
    fn skillify_writes_a_bounded_skill_and_never_overwrites() {
        let home = tempfile::tempdir().unwrap();
        let (p, detail) = skillify(home.path(), "Fix Login Test!", &World::default(), &session(), false).unwrap();
        assert!(p.ends_with("skills/fix-login-test/SKILL.md") || p.ends_with("skills\\fix-login-test\\SKILL.md"), "{}", p.display());
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.starts_with("---\nname: fix-login-test\ndescription: \"Fix the flaky login test."), "{body}");
        assert!(body.contains("1. `read src/login.rs`") && body.contains("## Pitfalls seen"), "{body}");
        assert!(body.lines().count() <= SKILL_MAX_LINES + 6);
        assert_eq!(detail["steps"], 2);
        assert!(skillify(home.path(), "fix-login-test", &World::default(), &session(), false).is_err());
    }

    #[test]
    fn review_without_a_model_writes_preferences_once() {
        let _env = crate::env_lock();
        let home = tempfile::tempdir().unwrap();
        std::env::remove_var("COSMOS_MEMORY_DIR");
        std::env::remove_var("MERIDIAN_VAULT");
        let ws = Path::new("/work/shop");
        let r = review(home.path(), ws, &session(), None).unwrap();
        assert_eq!(r.memories, [("preference".to_string(), "Always run cargo fmt before committing".to_string())]);
        assert!(r.skill.is_none());
        let saved = ztools::memory::all(home.path());
        assert_eq!(saved[0].scope, "repo:shop");
        // a second review of the same session adds nothing
        let again = review(home.path(), ws, &session(), None).unwrap();
        assert!(again.memories.is_empty());
    }

    #[test]
    fn slugs_and_json_extraction() {
        assert_eq!(slug("  Deploy to Fly.io (prod) "), "deploy-to-fly-io-prod");
        assert_eq!(json_in("sure: {\"a\": 1} done").unwrap()["a"], 1);
        assert!(json_in("no json").is_none());
    }
}

#[cfg(test)]
mod model_review {
    use super::*;
    use std::io::{Read, Write};

    /// One-shot OpenAI-compatible server answering with `reply` as content.
    fn serve(reply: &str) -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let chunk = json!({"choices": [{"delta": {"content": reply}}]}).to_string();
        let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(std::time::Duration::from_millis(300))).unwrap();
            // read the whole request (headers + body) before answering
            let mut req = Vec::new();
            let mut buf = [0u8; 8192];
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
                let t = String::from_utf8_lossy(&req).to_string();
                if let Some(i) = t.find("

") {
                    let len = t[..i].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                    if req.len() >= i + 4 + len {
                        break;
                    }
                }
            }
            let _ = write!(s, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
        });
        port
    }

    #[test]
    fn a_model_review_adds_bounded_memories_and_one_skill() {
        let _env = crate::env_lock();
        let home = tempfile::tempdir().unwrap();
        std::env::remove_var("COSMOS_MEMORY_DIR");
        std::env::remove_var("MERIDIAN_VAULT");
        let reply = r#"Here you go: {"memories": [
            {"type": "decision", "text": "login tests need the dev server on :8080"},
            {"type": "map", "text": "not a review type — dropped"},
            {"type": "correction", "text": "run tests with --test-threads=1 here"}],
          "skill": {"name": "Fix Flaky Login", "description": "stabilise the login test", "body": "1. start the server\n2. run cargo test login"}}"#;
        let port = serve(reply);
        let cfg = zproviders::ProviderCfg {
            kind: zproviders::ProviderKind::OpenAI,
            base_url: format!("http://127.0.0.1:{port}/v1"),
            model: "m".into(),
            api_key: Some("k".into()),
            ..Default::default()
        };
        let events = vec![
            Event::new("s", "cosmostui", None, "user", json!({"text": "make the login test stable"})),
            Event::new("s", "cosmostui", None, "assistant", json!({"text": "done", "tool_calls": []})),
        ];
        let r = review(home.path(), Path::new("/w/app"), &events, Some((&cfg, &["127.0.0.1".to_string()]))).unwrap();
        let kinds: Vec<&str> = r.memories.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["decision", "correction"], "{:?} {:?}", r.memories, r.notes);
        let skill = r.skill.expect("skill written");
        let body = std::fs::read_to_string(skill).unwrap();
        assert!(body.starts_with("---\nname: fix-flaky-login\ndescription: \"stabilise the login test\""), "{body}");
    }
}
