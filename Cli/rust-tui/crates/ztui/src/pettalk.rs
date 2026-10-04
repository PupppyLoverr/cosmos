//! `/pettalk` — talk to your pet. It answers in character with the model
//! you're using right now (its own side conversation: your main transcript
//! and prompt cache never see it), knows what's going on in the session and
//! the repo, and can *do* small things by ending its reply with action
//! lines: set a reminder, do a trick, take a new name or form, or hand a
//! look-and-feel request to `/customise`.
//!
//! Subscription agents (Cursor, Devin over ACP) are asked from an empty
//! scratch folder, like `/customise` does, so they have nothing to poke at.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

use zproto::{Block, Chunk, Message, Role, Usage};

use crate::buddy::Trick;

/// Something the pet decided to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Remind { mins: u64, text: String },
    Trick(Trick),
    Rename(String),
    Species(String),
    Customise(String),
}

/// Split a reply into what to show and what to do. Action lines start with
/// `@` (anywhere in the reply); anything malformed is dropped quietly.
pub fn parse(reply: &str) -> (String, Vec<Action>) {
    let mut text = Vec::new();
    let mut acts = Vec::new();
    for line in reply.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix('@') else {
            text.push(line);
            continue;
        };
        let (verb, arg) = rest.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((rest, ""));
        let act = match verb.to_ascii_lowercase().as_str() {
            "remind" => {
                let (n, what) = arg.split_once(char::is_whitespace).unwrap_or((arg, ""));
                match n.trim_end_matches(['m', 'M']).parse::<u64>() {
                    Ok(mins) if (1..=24 * 60).contains(&mins) && !what.trim().is_empty() => {
                        Some(Action::Remind { mins, text: what.trim().chars().take(120).collect() })
                    }
                    _ => None,
                }
            }
            "trick" => Trick::parse(arg).map(Action::Trick),
            "rename" | "name" => {
                let n: String = arg.chars().filter(|c| !c.is_control()).take(24).collect();
                (!n.trim().is_empty()).then(|| Action::Rename(n.trim().to_string()))
            }
            "species" | "form" => crate::pixel::canonical(arg).map(|s| Action::Species(s.name.to_string())),
            "customise" | "customize" => (!arg.is_empty()).then(|| Action::Customise(arg.chars().take(500).collect())),
            _ => None,
        };
        match act {
            Some(a) => acts.push(a),
            None => text.push(line),
        }
    }
    let text = text.join("\n").trim().to_string();
    (text, acts)
}

/// Everything the pet knows about itself and the session.
pub struct Context {
    pub pet: String,
    pub session: String,
    pub cwd: PathBuf,
}

/// A quick read of the repo (run on the talk thread, never the UI).
fn git_facts(cwd: &Path) -> String {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let Some(branch) = git(&["rev-parse", "--abbrev-ref", "HEAD"]) else {
        return "not a git repository".into();
    };
    let changed = git(&["status", "--porcelain"]).map(|s| s.lines().count()).unwrap_or(0);
    let last = git(&["log", "-1", "--format=%s (%cr)"]).unwrap_or_default();
    format!("git branch {branch}, {changed} changed file(s), last commit: {last}")
}

fn system(ctx: &Context) -> String {
    format!(
        "You are a pet companion living inside cosmostui, a terminal coding assistant. {pet}\n\n\
         Talk to your human like a loyal pet would: short (1-3 sentences), warm, in character, a little playful; emoji sparingly. \
         You can't run commands or edit code (the main assistant does that) — you help by noticing, remembering, encouraging and keeping them company. \
         Be honest: if you don't know, say so.\n\n\
         What you can see right now:\n{session}\n\n\
         You can act by ending your reply with lines like these (one per line, only when it helps or they asked):\n\
         @remind <minutes> <what to remind them>\n\
         @trick <hop|spin|sparkle|nap>\n\
         @rename <new name>\n\
         @species <fox|cat|axolotl|owl|penguin|dragon|slime|ghost|octopus|robot>   (only if they ask you to change form)\n\
         @customise <request>   (only if they ask to change how cosmostui looks or behaves)",
        pet = ctx.pet,
        session = ctx.session,
    )
}

/// What a talk thread sends back.
#[derive(Debug)]
pub enum Reply {
    Text { text: String, actions: Vec<Action>, usd: Option<f64> },
    Failed(String),
}

pub struct Talk {
    pub rx: Receiver<Reply>,
    cancel: Arc<AtomicBool>,
    pub started: Instant,
}

impl Talk {
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Ask the pet: `history` ends with the operator's new message.
pub fn ask(model: String, home: PathBuf, mut ctx: Context, history: Vec<Message>) -> Talk {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    std::thread::spawn(move || {
        ctx.session.push_str(&format!("\n- repo: {}", git_facts(&ctx.cwd)));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&model, &home, &ctx, &history, &c)));
        let msg = match r {
            Ok(Ok((reply, usd))) => {
                let (text, actions) = parse(&reply);
                Reply::Text { text, actions, usd }
            }
            Ok(Err(e)) => Reply::Failed(e),
            Err(_) => Reply::Failed("the pet got confused (crashed)".into()),
        };
        if !c.load(Ordering::Relaxed) {
            let _ = tx.send(msg);
        }
    });
    Talk { rx, cancel, started: Instant::now() }
}

fn run(alias: &str, home: &Path, ctx: &Context, history: &[Message], cancel: &AtomicBool) -> Result<(String, Option<f64>), String> {
    let cfg = zcore::config::Config::load(home);
    let (mut provider, model_id) = cfg.provider_for(alias).ok_or_else(|| format!("no model {alias:?} — pick one with /model"))?;
    match provider.kind {
        zproviders::ProviderKind::Acp => return run_acp(alias, home, ctx, history, cancel),
        zproviders::ProviderKind::Devin => return Err("pets can't talk through the experimental Devin-direct mode — pick another model".into()),
        zproviders::ProviderKind::ChatGpt => {
            if let Some(t) = zcore::chatgpt::fresh() {
                provider.api_key = Some(t.access);
                provider.account_id = Some(t.account_id);
            }
        }
        _ => {}
    }
    let session = format!("pet-{}", zproto::new_id());
    let req = zproviders::Request { system: vec![system(ctx)], tools: &[], messages: history, session_id: &session, max_tokens: 400 };
    let mut text = String::new();
    let mut usage = Usage::default();
    let mut err = None;
    zproviders::complete(
        &provider,
        &req,
        &mut |c| match c {
            Chunk::Text(t) => text.push_str(&t),
            Chunk::Done(u) => {
                usage.input += u.input;
                usage.cache_read += u.cache_read;
                usage.cache_write += u.cache_write;
                usage.output += u.output;
            }
            Chunk::Err(e) => err = Some(e),
            Chunk::ToolUse { .. } => {}
        },
        &cfg.allowed_hosts(),
    )
    .map_err(|e| format!("{model_id}: {e}"))?;
    if let Some(e) = err {
        return Err(format!("{model_id}: {e}"));
    }
    Ok((text.trim().to_string(), provider.prices.cost(&usage)))
}

/// Subscription agents: one prompt with the chat so far, from an empty folder.
fn run_acp(alias: &str, home: &Path, ctx: &Context, history: &[Message], cancel: &AtomicBool) -> Result<(String, Option<f64>), String> {
    let scratch = home.join("pet").join(format!("scratch-{}", zproto::new_id()));
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(scratch.clone());
    let opts = zproto::RunOpts { model: Some(alias.to_string()), cwd: Some(scratch.display().to_string()), ..Default::default() };
    let mut agent = zcore::agent::Agent::new(&opts, "cosmostui-pet", &scratch).map_err(|e| e.to_string())?;
    let flag = Arc::new(AtomicBool::new(false));
    agent.interrupt = Some(flag.clone());
    let transcript: Vec<String> = history
        .iter()
        .map(|m| {
            let who = if m.role == Role::Assistant { "You (the pet)" } else { "Human" };
            let t: Vec<&str> = m.content.iter().filter_map(|b| if let Block::Text { text } = b { Some(text.as_str()) } else { None }).collect();
            format!("{who}: {}", t.join(" "))
        })
        .collect();
    let prompt = format!(
        "{}\n\nDon't use any tools, don't read or change files. Just reply as the pet.\n\nConversation so far:\n{}\n\nReply to the human's last message.",
        system(ctx),
        transcript.join("\n")
    );
    agent.send_user(&prompt, &mut |_| {});
    for _ in 0..3 {
        if cancel.load(Ordering::Relaxed) {
            flag.store(true, Ordering::Relaxed);
            return Err("stopped".into());
        }
        match agent.turn(&mut |_| {}) {
            Ok(true) => break,
            Ok(false) => continue,
            Err(e) => return Err(format!("{alias}: {e}")),
        }
    }
    let reply = agent
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant)
        .map(|m| m.content.iter().filter_map(|b| if let Block::Text { text } = b { Some(text.as_str()) } else { None }).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    Ok((reply.trim().to_string(), None))
}

/// Keep the chat short: the last `n` messages.
pub fn trim(history: &mut Vec<Message>, n: usize) {
    if history.len() > n {
        let cut = history.len() - n;
        history.drain(..cut);
    }
}

pub fn user(text: &str) -> Message {
    Message::text(Role::User, text)
}

pub fn assistant(text: &str) -> Message {
    Message::text(Role::Assistant, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_split_into_words_and_actions() {
        let (text, acts) = parse("Sure thing! I'll nudge you.\n@remind 15 check the deploy\n@trick hop\n@species Dragon\n@frobnicate x\n@remind soon nothing");
        assert_eq!(text, "Sure thing! I'll nudge you.\n@frobnicate x\n@remind soon nothing");
        assert_eq!(
            acts,
            vec![
                Action::Remind { mins: 15, text: "check the deploy".into() },
                Action::Trick(Trick::Hop),
                Action::Species("dragon".into()),
            ]
        );
        let (_, acts) = parse("@rename  Bloop \n@customise make it purple");
        assert_eq!(acts, vec![Action::Rename("Bloop".into()), Action::Customise("make it purple".into())]);
        assert!(parse("@remind 99999 x").1.is_empty(), "a day at most");
        assert!(parse("@species unicorn").1.is_empty(), "only real species");
    }

    #[test]
    fn the_prompt_carries_the_pet_and_the_session() {
        let ctx = Context { pet: "You are Pip, a common fox.".into(), session: "- model: r9".into(), cwd: PathBuf::from(".") };
        let s = system(&ctx);
        assert!(s.contains("Pip") && s.contains("- model: r9") && s.contains("@remind"));
        let mut h = vec![user("a"), assistant("b"), user("c")];
        trim(&mut h, 2);
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn talking_to_the_fixture_model_round_trips() {
        let _env = crate::env_lock();
        let d = tempfile::tempdir().unwrap();
        let ctx = Context { pet: "You are Pip.".into(), session: "- idle".into(), cwd: d.path().to_path_buf() };
        let t = ask("fixture".into(), d.path().to_path_buf(), ctx, vec![user("ping")]);
        match t.rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap() {
            Reply::Text { text, actions, .. } => {
                assert_eq!(text, "pong");
                assert!(actions.is_empty());
            }
            Reply::Failed(e) => panic!("{e}"),
        }
    }
}
