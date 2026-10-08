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
#[derive(Debug)]
pub enum Cmd {
    Prompt(String),
    SetMode(Mode),
    SetModel(String),
    /// reasoning effort for OpenAI-compatible models; `None` = provider default
    SetEffort(Option<String>),
    Approve,
    /// continue a stored session (replaces the current agent)
    Resume(String),
    /// harness-side commands that never reach the model
    Local(LocalCmd),
    Quit,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalCmd {
    /// /compact [focus]
    Compact(Option<String>),
    /// /undo
    Undo,
    /// /skillify <name>
    Skillify(String),
}

/// One row of a rendered file diff.
#[derive(Clone, Debug, PartialEq)]
pub struct DiffLine {
    /// `+` added, `-` removed, ` ` context, `…` elided gap
    pub kind: char,
    /// line number in the new file (old file for removals)
    pub no: Option<usize>,
    /// line number in the old file (context and removals)
    pub old: Option<usize>,
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
    /// A subscription agent's tool started or changed (live, as it happens).
    AcpTool(serde_json::Value),
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

/// Messages to slip into the running task ("send now" on a queued message).
pub type Steer = zcore::agent::SteerQueue;

pub fn spawn(
    opts: RunOpts,
    cwd: PathBuf,
    interrupt: Arc<AtomicBool>,
    steer: Steer,
) -> (Sender<Cmd>, Receiver<Msg>) {
    let (ctx, crx) = channel::<Cmd>();
    let (mtx, mrx) = channel::<Msg>();
    thread::spawn(move || worker(opts, cwd, interrupt, steer, crx, mtx));
    (ctx, mrx)
}

fn take_steer(steer: &Steer) -> Vec<String> {
    std::mem::take(&mut *steer.lock().unwrap_or_else(|e| e.into_inner()))
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
    let t2 = tx.clone();
    agent.on_tool = Some(Box::new(move |v: &serde_json::Value| {
        let _ = t2.send(Msg::AcpTool(v.clone()));
    }));
    let tx = tx.clone();
    agent.on_delta = Some(Box::new(move |t: &str| {
        let _ = tx.send(Msg::Delta(t.to_string()));
    }));
}

/// Most diff rows kept per call. The skin shows fewer (`[diff] max_rows`);
/// the rest wait in the ctrl+o pager.
const MAX_DIFF_ROWS: usize = 2000;

/// Unified-style rows (2 lines of context) between two file versions.
pub fn diff_rows(old: &str, new: &str) -> (Vec<DiffLine>, usize, usize) {
    use similar::{ChangeTag, TextDiff};
    let d = TextDiff::from_lines(old, new);
    let (mut added, mut removed) = (0, 0);
    let mut rows = Vec::new();
    let context = crate::skin::current().diff.context.min(20);
    for (gi, group) in d.grouped_ops(context).iter().enumerate() {
        if gi > 0 {
            rows.push(DiffLine { kind: '…', no: None, old: None, text: String::new() });
        }
        for op in group {
            for ch in d.iter_changes(op) {
                let text = ch.value().trim_end_matches(['\n', '\r']).to_string();
                let old = ch.old_index().map(|i| i + 1);
                let (kind, no) = match ch.tag() {
                    ChangeTag::Insert => {
                        added += 1;
                        ('+', ch.new_index().map(|i| i + 1))
                    }
                    ChangeTag::Delete => {
                        removed += 1;
                        ('-', old)
                    }
                    ChangeTag::Equal => (' ', ch.new_index().map(|i| i + 1)),
                };
                rows.push(DiffLine { kind, no, old, text });
            }
        }
    }
    if rows.len() > MAX_DIFF_ROWS {
        let more = rows.len() - MAX_DIFF_ROWS;
        rows.truncate(MAX_DIFF_ROWS);
        rows.push(DiffLine { kind: '…', no: None, old: None, text: format!("{more} more lines") });
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
    steer: &Steer,
) {
    agent.send_user(task, emit);
    run_turns(agent, emit, interrupt, always, tx, steer);
    // "send now" arrived after the last step (or cut a subscription agent's
    // prompt short): carry on in the same session, context intact. An Esc
    // stops here — whatever's left goes back to the UI's queue.
    loop {
        if interrupt.load(Ordering::Relaxed) {
            break;
        }
        let next = take_steer(steer);
        if next.is_empty() {
            break;
        }
        for t in next {
            agent.send_user(&t, emit);
        }
        run_turns(agent, emit, interrupt, always, tx, steer);
    }
}

/// Same turn/tool sequence as the line REPL, plus interrupt polling between
/// turns and tool calls.
fn run_turns(
    agent: &mut Agent,
    emit: &mut dyn FnMut(&Event),
    interrupt: &AtomicBool,
    always: &Arc<Mutex<Vec<String>>>,
    tx: &Sender<Msg>,
    steer: &Steer,
) {
    let max = agent.opts.max_turns.unwrap_or(32);
    for _ in 0..max {
        if interrupt.load(Ordering::Relaxed) {
            send(tx, Msg::Note("· interrupted".into()));
            return;
        }
        // messages sent while it works join before the next step
        for t in take_steer(steer) {
            agent.send_user(&t, emit);
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
    steer: Steer,
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
    agent.steer = Some(steer.clone());
    let mut effort: Option<String> = None;

    let tx2 = tx.clone();
    let mut emit = move |ev: &Event| {
        let _ = tx2.send(Msg::Event(ev.clone()));
    };
    send(&tx, ready(&agent));
    let start = agent.session.append(
        "session_start",
        agent.start_data(json!({"resume": opts.resume, "cwd": agent.ctx.workspace})),
    );
    emit(&start);

    // A prompt carried in via -p runs first (same as the line REPL).
    if let Some(p) = &opts.prompt {
        if p != "-" {
            interrupt.store(false, Ordering::Relaxed);
            run_task(&mut agent, p, &mut emit, &interrupt, &always, &tx, &steer);
            send(&tx, Msg::TurnDone);
        }
    }

    for cmd in rx {
        match cmd {
            Cmd::Prompt(text) => {
                interrupt.store(false, Ordering::Relaxed);
                run_task(&mut agent, &text, &mut emit, &interrupt, &always, &tx, &steer);
                agent.stop_hook(if interrupt.load(Ordering::Relaxed) { "interrupted" } else { "done" });
                send(&tx, Msg::TurnDone);
            }
            Cmd::Local(lc) => {
                let note = match lc {
                    LocalCmd::Compact(focus) => {
                        let before = agent.context_report();
                        agent.compact_now(focus.as_deref(), &mut emit);
                        let after = agent.context_report();
                        let pct = |r: &str| serde_json::from_str::<serde_json::Value>(r).ok().and_then(|v| v["percent"].as_u64()).unwrap_or(0);
                        format!("compacted · context {}% → {}%", pct(&before), pct(&after))
                    }
                    LocalCmd::Undo => agent.undo_last(&mut emit),
                    LocalCmd::Skillify(name) => agent.skillify(&name, &mut emit),
                };
                send(&tx, Msg::Note(note));
                send(&tx, Msg::TurnDone);
            }
            Cmd::Approve => {
                interrupt.store(false, Ordering::Relaxed);
                agent.approve_plan(&mut emit);
                run_turns(&mut agent, &mut emit, &interrupt, &always, &tx, &steer);
                agent.stop_hook(if interrupt.load(Ordering::Relaxed) { "interrupted" } else { "done" });
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
                        a.steer = Some(steer.clone());
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
                        a.steer = Some(steer.clone());
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
                // shut the agent down *before* saying so: the UI exits on
                // QuitDone, and a subscription agent (Cursor/Devin) must be
                // stopped by our drop, not left running
                drop(agent);
                send(&tx, Msg::QuitDone);
                return;
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
        assert!(rows.contains(&DiffLine { kind: '-', no: Some(3), old: Some(3), text: "c".into() }));
        assert!(rows.contains(&DiffLine { kind: '+', no: Some(3), old: None, text: "C".into() }));
        assert!(rows.contains(&DiffLine { kind: '+', no: Some(6), old: None, text: "f".into() }));
    }

    #[test]
    fn diff_rows_are_capped() {
        let old = String::new();
        let new: String = (0..2500).map(|i| format!("line {i}\n")).collect();
        let (rows, added, _) = diff_rows(&old, &new);
        assert_eq!(added, 2500);
        assert_eq!(rows.len(), MAX_DIFF_ROWS + 1);
        assert_eq!(rows.last().unwrap().kind, '…');
    }

    /// "send now": a message steered into a running task joins it before the
    /// next step — same session, context intact.
    #[test]
    fn steered_messages_join_the_running_task() {
        let _env = crate::env_lock();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("COSMOS_CLI_HOME", home.path());
        let opts = RunOpts { model: Some("fixture".into()), no_sandbox: true, ..Default::default() };
        let steer = Steer::default();
        let (cmd, rx) = spawn(opts, home.path().to_path_buf(), Arc::new(AtomicBool::new(false)), steer.clone());
        steer.lock().unwrap().push("ping".into());
        cmd.send(Cmd::Prompt("tool:ls {}".into())).unwrap();
        let mut users = Vec::new();
        let mut replies = Vec::new();
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(30)).expect("worker answers") {
                Msg::Event(e) if e.kind == "user" => users.push(e.data["text"].as_str().unwrap_or("").to_string()),
                Msg::Event(e) if e.kind == "assistant" => replies.push(e.data["text"].as_str().unwrap_or("").to_string()),
                Msg::TurnDone => break,
                _ => {}
            }
        }
        assert_eq!(users, vec!["tool:ls {}".to_string(), "ping".to_string()], "one task, both messages");
        assert!(replies.iter().any(|r| r == "pong"), "the model saw the steered message: {replies:?}");
        assert!(steer.lock().unwrap().is_empty());
        cmd.send(Cmd::Quit).unwrap();
    }

    /// Esc means stop, even with a "send now" waiting: the task ends and the
    /// message stays for the UI to put back in its queue.
    #[test]
    fn an_interrupt_wins_over_a_pending_steer() {
        let _env = crate::env_lock();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("COSMOS_CLI_HOME", home.path());
        let opts = RunOpts { model: Some("fixture".into()), no_sandbox: true, ..Default::default() };
        let mut agent = Agent::new(&opts, zcore::NAME, home.path()).unwrap();
        let (tx, _rx) = channel();
        let steer = Steer::default();
        steer.lock().unwrap().push("ping".into());
        let stop = AtomicBool::new(true); // Esc pressed
        let always = Arc::new(Mutex::new(Vec::new()));
        let mut users = 0;
        run_task(&mut agent, "hello", &mut |e: &Event| users += usize::from(e.kind == "user"), &stop, &always, &tx, &steer);
        assert_eq!(users, 1, "only the task itself — nothing ran after the stop");
        assert_eq!(*steer.lock().unwrap(), vec!["ping".to_string()], "the message is kept for the queue");
    }

    /// Live: a subscription agent's tools reach the UI as they happen, in
    /// order with its text. `COSMOS_LIVE_MODEL=cursor/… cargo test -p ztui
    /// live_acp_streams_tools -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_acp_streams_tools() {
        let _env = crate::env_lock();
        let model = std::env::var("COSMOS_LIVE_MODEL").expect("set COSMOS_LIVE_MODEL");
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("notes.txt"), "one\ntwo\n").unwrap();
        let opts = RunOpts { model: Some(model), yolo: true, ..Default::default() };
        let (cmd, rx) = spawn(opts, ws.path().to_path_buf(), Arc::new(AtomicBool::new(false)), Steer::default());
        cmd.send(Cmd::Prompt("Say 'Looking now.' first, then read notes.txt, then change 'two' to 'three' in it, then say 'Done.'".into())).unwrap();
        let t0 = std::time::Instant::now();
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(240)).expect("worker answers") {
                Msg::Delta(d) => println!("[{:>5.1}s] text  {d:?}", t0.elapsed().as_secs_f32()),
                Msg::AcpTool(v) => println!(
                    "[{:>5.1}s] tool  {} {} {} diff={}",
                    t0.elapsed().as_secs_f32(),
                    v["status"],
                    v["name"],
                    v["title"],
                    v.get("diff").is_some()
                ),
                Msg::Event(e) if e.kind == "tool_call" || e.kind == "assistant" => println!("[{:>5.1}s] event {}", t0.elapsed().as_secs_f32(), e.kind),
                Msg::TurnDone => {
                    println!("[{:>5.1}s] done", t0.elapsed().as_secs_f32());
                    break;
                }
                _ => {}
            }
        }
        println!("notes.txt now: {:?}", std::fs::read_to_string(ws.path().join("notes.txt")).unwrap());
        cmd.send(Cmd::Quit).unwrap();
        // the agent is stopped before QuitDone arrives
        while !matches!(rx.recv_timeout(std::time::Duration::from_secs(20)), Ok(Msg::QuitDone) | Err(_)) {}
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
        let (cmd, rx) = spawn(opts, std::env::temp_dir(), Arc::new(AtomicBool::new(false)), Steer::default());
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
