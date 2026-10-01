//! Worker thread owning the `Agent`. The UI sends `Cmd`s and receives `Msg`s;
//! permission prompts are bridged through `Msg::PermissionAsk` + a reply
//! channel so the agent can block mid-tool while the popup is open.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::json;
use zcore::agent::{Agent, Mode};
use zproto::{Block, Event, RunOpts};

/// UI → worker.
pub enum Cmd {
    Prompt(String),
    SetMode(Mode),
    SetModel(String),
    /// reasoning effort for OpenAI-compatible models; `None` = provider default
    SetEffort(Option<String>),
    Approve,
    /// continue a stored session (replaces the current agent)
    Resume(String),
    Quit,
}

/// One row of a rendered file diff.
#[derive(Clone, Debug, PartialEq)]
pub struct DiffLine {
    /// `+` added, `-` removed, ` ` context, `…` elided gap
    pub kind: char,
    /// line number in the new file (old file for removals)
    pub no: Option<usize>,
    pub text: String,
}

/// What an `edit`/`write` call changed, computed around the tool run.
#[derive(Clone, Debug)]
pub struct Diff {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub created: bool,
    pub lines: Vec<DiffLine>,
}

/// worker → UI.
pub enum Msg {
    /// Agent constructed (or rebuilt after /model) — refresh header state.
    Ready {
        session: String,
        alias: String,
        model: String,
        mode: Mode,
        degraded: Option<String>,
    },
    Event(Event),
    /// A streamed text fragment of the in-flight assistant turn.
    Delta(String),
    /// File change made by an `edit`/`write` tool call (arrives before its result).
    Diff { id: String, diff: Diff },
    /// The agent's `asker` fired — `reply`: 0 = no, 1 = yes, 2 = always.
    PermissionAsk {
        tool: String,
        reply: Sender<u8>,
    },
    /// Dim transcript note.
    Note(String),
    /// Red transcript note.
    Err(String),
    /// A prompt/approve run finished (worker is idle again).
    TurnDone,
    /// Quit acknowledged — session_end appended, agent dropped.
    QuitDone,
    /// Conversation of a resumed session, oldest first: (is_user, text).
    History(Vec<(bool, String)>),
}

pub fn spawn(
    opts: RunOpts,
    cwd: PathBuf,
    interrupt: Arc<AtomicBool>,
) -> (Sender<Cmd>, Receiver<Msg>) {
    let (ctx, crx) = channel::<Cmd>();
    let (mtx, mrx) = channel::<Msg>();
    thread::spawn(move || worker(opts, cwd, interrupt, crx, mtx));
    (ctx, mrx)
}

fn send(tx: &Sender<Msg>, m: Msg) {
    let _ = tx.send(m);
}

fn ready(agent: &Agent) -> Msg {
    // model_alias is "{alias} ({model id})" — recover the bare alias for
    // the ✳ marker and /model matching.
    let alias = agent.opts.model.clone().unwrap_or_else(|| {
        agent
            .model_alias
            .split(" (")
            .next()
            .unwrap_or_default()
            .to_string()
    });
    Msg::Ready {
        session: agent.session.id.clone(),
        alias,
        model: agent.model_alias.clone(),
        mode: agent.mode,
        degraded: agent.ctx.sandbox_degraded.clone(),
    }
}

/// Bridge `agent.asker` → `Msg::PermissionAsk` → UI popup → reply channel.
/// "always" replies are collected in `always` and folded into
/// `agent.opts.allow` by the run loop (the asker can't touch the agent — it
/// is borrowed inside it).
fn install_asker(agent: &mut Agent, tx: &Sender<Msg>, always: &Arc<Mutex<Vec<String>>>) {
    let tx = tx.clone();
    let always = always.clone();
    agent.asker = Some(Box::new(move |tool: &str| {
        let (rtx, rrx) = channel::<u8>();
        if tx
            .send(Msg::PermissionAsk {
                tool: tool.to_string(),
                reply: rtx,
            })
            .is_err()
        {
            return false;
        }
        match rrx.recv() {
            Ok(2) => {
                always.lock().unwrap().push(tool.to_string());
                true
            }
            Ok(1) => true,
            _ => false,
        }
    }));
}

fn drain_always(agent: &mut Agent, always: &Arc<Mutex<Vec<String>>>) {
    let mut l = always.lock().unwrap();
    for t in l.drain(..) {
        if !agent.opts.allow.iter().any(|x| x == &t) {
            agent.opts.allow.push(t);
        }
    }
}

/// Streamed text goes straight to the UI.
fn install_delta(agent: &mut Agent, tx: &Sender<Msg>) {
    let tx = tx.clone();
    agent.on_delta = Some(Box::new(move |t: &str| {
        let _ = tx.send(Msg::Delta(t.to_string()));
    }));
}

/// Most diff rows shown per call — enough to review, never a wall.
const MAX_DIFF_ROWS: usize = 40;

/// Unified-style rows (2 lines of context) between two file versions.
pub fn diff_rows(old: &str, new: &str) -> (Vec<DiffLine>, usize, usize) {
    use similar::{ChangeTag, TextDiff};
    let d = TextDiff::from_lines(old, new);
    let (mut added, mut removed) = (0, 0);
    let mut rows = Vec::new();
    for (gi, group) in d.grouped_ops(2).iter().enumerate() {
        if gi > 0 {
            rows.push(DiffLine { kind: '…', no: None, text: String::new() });
        }
        for op in group {
            for ch in d.iter_changes(op) {
                let text = ch.value().trim_end_matches(['\n', '\r']).to_string();
                let (kind, no) = match ch.tag() {
                    ChangeTag::Insert => {
                        added += 1;
                        ('+', ch.new_index().map(|i| i + 1))
                    }
                    ChangeTag::Delete => {
                        removed += 1;
                        ('-', ch.old_index().map(|i| i + 1))
                    }
                    ChangeTag::Equal => (' ', ch.new_index().map(|i| i + 1)),
                };
                rows.push(DiffLine { kind, no, text });
            }
        }
    }
    if rows.len() > MAX_DIFF_ROWS {
        let more = rows.len() - MAX_DIFF_ROWS;
        rows.truncate(MAX_DIFF_ROWS);
        rows.push(DiffLine { kind: '…', no: None, text: format!("{more} more lines") });
    }
    (rows, added, removed)
}

/// Before-image of the file an `edit`/`write` call is about to touch.
fn snapshot(agent: &Agent, name: &str, input: &serde_json::Value) -> Option<(PathBuf, String, Option<String>)> {
    if name != "edit" && name != "write" {
        return None;
    }
    let rel = input["path"].as_str()?;
    let abs = ztools::resolve(&agent.ctx, rel).ok()?;
    let before = std::fs::read_to_string(&abs).ok();
    Some((abs, rel.to_string(), before))
}

/// Run one tool call. File-changing calls get a diff sent to the UI just
/// ahead of their `tool_result`, so the transcript shows what changed.
fn exec_with_diff(
    agent: &mut Agent,
    id: &str,
    name: &str,
    input: &serde_json::Value,
    emit: &mut dyn FnMut(&Event),
    tx: &Sender<Msg>,
) {
    let Some((abs, rel, before)) = snapshot(agent, name, input) else {
        agent.exec_tool(id, name, input, emit);
        return;
    };
    let mut held: Vec<Event> = Vec::new();
    agent.exec_tool(id, name, input, &mut |e: &Event| held.push(e.clone()));
    if let Ok(after) = std::fs::read_to_string(&abs) {
        let old = before.clone().unwrap_or_default();
        if old != after {
            let (lines, added, removed) = diff_rows(&old, &after);
            send(
                tx,
                Msg::Diff {
                    id: id.to_string(),
                    diff: Diff { path: rel, added, removed, created: before.is_none(), lines },
                },
            );
        }
    }
    for e in &held {
        emit(e);
    }
}

fn mode_event(agent: &mut Agent, emit: &mut dyn FnMut(&Event)) {
    let ev = agent
        .session
        .append("mode", json!({"to": agent.mode.name()}));
    emit(&ev);
}

fn run_task(
    agent: &mut Agent,
    task: &str,
    emit: &mut dyn FnMut(&Event),
    interrupt: &AtomicBool,
    always: &Arc<Mutex<Vec<String>>>,
    tx: &Sender<Msg>,
) {
    agent.send_user(task, emit);
    run_turns(agent, emit, interrupt, always, tx);
}

/// Same turn/tool sequence as the line REPL, plus interrupt polling between
/// turns and tool calls.
fn run_turns(
    agent: &mut Agent,
    emit: &mut dyn FnMut(&Event),
    interrupt: &AtomicBool,
    always: &Arc<Mutex<Vec<String>>>,
    tx: &Sender<Msg>,
) {
    let max = agent.opts.max_turns.unwrap_or(32);
    for _ in 0..max {
        if interrupt.load(Ordering::Relaxed) {
            send(tx, Msg::Note("· interrupted".into()));
            return;
        }
        match agent.turn(emit) {
            Ok(true) => break,
            Ok(false) => {
                let calls: Vec<(String, String, serde_json::Value)> = agent
                    .messages
                    .last()
                    .map(|m| {
                        m.content
                            .iter()
                            .filter_map(|b| match b {
                                Block::ToolUse { id, name, input } => {
                                    Some((id.clone(), name.clone(), input.clone()))
                                }
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                for (id, name, input) in calls {
                    if interrupt.load(Ordering::Relaxed) {
                        send(tx, Msg::Note("· interrupted".into()));
                        return;
                    }
                    exec_with_diff(agent, &id, &name, &input, emit, tx);
                    drain_always(agent, always);
                    if name == "submit_plan" {
                        return; // wait for /approve
                    }
                }
            }
            Err(e) => {
                send(tx, Msg::Err(format!("turn failed: {e:#}")));
                return;
            }
        }
    }
}

fn worker(
    opts: RunOpts,
    cwd: PathBuf,
    interrupt: Arc<AtomicBool>,
    rx: Receiver<Cmd>,
    tx: Sender<Msg>,
) {
    let mut agent = match Agent::new(&opts, zcore::NAME, &cwd) {
        Ok(a) => a,
        Err(e) => {
            send(&tx, Msg::Err(format!("agent init failed: {e:#}")));
            send(&tx, Msg::QuitDone);
            return;
        }
    };
    let always = Arc::new(Mutex::new(Vec::new()));
    install_asker(&mut agent, &tx, &always);
    install_delta(&mut agent, &tx);
    agent.interrupt = Some(interrupt.clone());
    let mut effort: Option<String> = None;

    let tx2 = tx.clone();
    let mut emit = move |ev: &Event| {
        let _ = tx2.send(Msg::Event(ev.clone()));
    };
    send(&tx, ready(&agent));
    let start = agent.session.append(
        "session_start",
        json!({
            "model": agent.model_alias, "mode": agent.mode.name(),
            "sandbox": if agent.ctx.no_sandbox {"off"} else {"appcontainer"},
            "boot_tokens": agent.boot_tokens,
            "resume": opts.resume,
        }),
    );
    emit(&start);

    // A prompt carried in via -p runs first (same as the line REPL).
    if let Some(p) = &opts.prompt {
        if p != "-" {
            interrupt.store(false, Ordering::Relaxed);
            run_task(&mut agent, p, &mut emit, &interrupt, &always, &tx);
            send(&tx, Msg::TurnDone);
        }
    }

    for cmd in rx {
        match cmd {
            Cmd::Prompt(text) => {
                interrupt.store(false, Ordering::Relaxed);
                run_task(&mut agent, &text, &mut emit, &interrupt, &always, &tx);
                send(&tx, Msg::TurnDone);
            }
            Cmd::Approve => {
                interrupt.store(false, Ordering::Relaxed);
                agent.approve_plan(&mut emit);
                run_turns(&mut agent, &mut emit, &interrupt, &always, &tx);
                send(&tx, Msg::TurnDone);
            }
            Cmd::SetEffort(e) => {
                effort = e;
                agent.provider.effort = effort.clone();
            }
            Cmd::SetMode(m) => {
                agent.mode = m;
                mode_event(&mut agent, &mut emit);
                send(&tx, ready(&agent));
            }
            Cmd::SetModel(alias) => {
                let mut o = agent.opts.clone();
                o.model = Some(alias.clone());
                o.resume = Some(agent.session.id.clone());
                o.prompt = None;
                o.mode = Some(agent.mode.name().to_string());
                o.yolo = agent.mode == Mode::Yolo;
                match Agent::new(&o, zcore::NAME, &cwd) {
                    Ok(mut a) => {
                        install_asker(&mut a, &tx, &always);
                        install_delta(&mut a, &tx);
                        a.interrupt = Some(interrupt.clone());
                        a.provider.effort = effort.clone();
                        let ev = a.session.append(
                            "session_start",
                            json!({"model": a.model_alias, "mode": a.mode.name(),
                                   "resume": o.resume, "switch": true}),
                        );
                        let from = agent.model_alias.clone();
                        agent = a;
                        emit(&ev);
                        let ev = agent
                            .session
                            .append("model_switch", json!({"from": from, "to": agent.model_alias}));
                        emit(&ev);
                        agent.log_cache_break("model_switch", &mut emit);
                        send(&tx, ready(&agent));
                        send(&tx, Msg::Note(format!("model → {alias}")));
                    }
                    Err(e) => send(&tx, Msg::Err(format!("model switch failed: {e:#}"))),
                }
            }
            Cmd::Resume(id) => {
                let mut o = agent.opts.clone();
                o.resume = Some(id.clone());
                o.prompt = None;
                match Agent::new(&o, zcore::NAME, &cwd) {
                    Ok(mut a) => {
                        install_asker(&mut a, &tx, &always);
                        install_delta(&mut a, &tx);
                        a.interrupt = Some(interrupt.clone());
                        a.provider.effort = effort.clone();
                        let ev = a.session.append(
                            "session_start",
                            json!({"model": a.model_alias, "mode": a.mode.name(), "resume": id}),
                        );
                        a.log_cache_break("resume", &mut emit);
                        let history: Vec<(bool, String)> = a
                            .messages
                            .iter()
                            .filter_map(|m| {
                                let text: String = m
                                    .content
                                    .iter()
                                    .filter_map(|b| match b {
                                        Block::Text { text } => Some(text.as_str()),
                                        _ => None,
                                    })
                                    .collect::<Vec<_>>()
                                    .join("
");
                                (!text.trim().is_empty()).then(|| (m.role == zproto::Role::User, text))
                            })
                            .collect();
                        agent = a;
                        emit(&ev);
                        send(&tx, ready(&agent));
                        send(&tx, Msg::History(history));
                    }
                    Err(e) => send(&tx, Msg::Err(format!("couldn't resume {id}: {e:#}"))),
                }
            }
            Cmd::Quit => {
                agent
                    .session
                    .append("session_end", json!({"reason": "quit"}));
                send(&tx, Msg::QuitDone);
                return; // agent drops here → worktree cleanup
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn diff_rows_counts_and_numbers() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nb\nC\nd\ne\nf\n";
        let (rows, added, removed) = diff_rows(old, new);
        assert_eq!((added, removed), (2, 1));
        assert!(rows.contains(&DiffLine { kind: '-', no: Some(3), text: "c".into() }));
        assert!(rows.contains(&DiffLine { kind: '+', no: Some(3), text: "C".into() }));
        assert!(rows.contains(&DiffLine { kind: '+', no: Some(6), text: "f".into() }));
    }

    #[test]
    fn diff_rows_are_capped() {
        let old = String::new();
        let new: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let (rows, added, _) = diff_rows(&old, &new);
        assert_eq!(added, 200);
        assert_eq!(rows.len(), MAX_DIFF_ROWS + 1);
        assert_eq!(rows.last().unwrap().kind, '…');
    }

    /// End-to-end worker protocol: Ready → Prompt("ping") → assistant "pong"
    /// (fixture provider) → TurnDone → Quit → QuitDone.
    #[test]
    fn round_trip_fixture() {
        let _env = crate::env_lock();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("COSMOS_CLI_HOME", home.path());
        let opts = RunOpts {
            model: Some("fixture".into()),
            no_sandbox: true,
            ..Default::default()
        };
        let (cmd, rx) = spawn(opts, std::env::temp_dir(), Arc::new(AtomicBool::new(false)));
        cmd.send(Cmd::Prompt("ping".into())).unwrap();
        let (mut ready, mut pong, mut done) = (false, false, false);
        for _ in 0..200 {
            match rx.recv_timeout(Duration::from_secs(15)) {
                Ok(Msg::Ready { .. }) => ready = true,
                Ok(Msg::Event(ev)) => {
                    if ev.kind == "assistant" && ev.data["text"].as_str() == Some("pong") {
                        pong = true;
                    }
                }
                Ok(Msg::TurnDone) => {
                    done = true;
                    break;
                }
                Ok(_) => {}
                Err(e) => panic!("worker stopped: {e}"),
            }
        }
        cmd.send(Cmd::Quit).unwrap();
        let mut quit = false;
        for _ in 0..50 {
            if matches!(rx.recv_timeout(Duration::from_secs(5)), Ok(Msg::QuitDone)) {
                quit = true;
                break;
            }
        }
        assert!(
            ready && pong && done && quit,
            "ready={ready} pong={pong} done={done} quit={quit}"
        );
    }
}
