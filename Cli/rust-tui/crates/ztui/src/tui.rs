//! Interactive front-end — Devin-CLI layout on an inline renderer.
//!
//! Finished output (banner, user turns, streamed replies, tool results with
//! diffs) is committed to the terminal's scrollback as it completes; the live
//! region underneath holds whatever is still in motion — the streaming line,
//! running tools, the working indicator, a dictation meter — plus the ruled
//! composer, the status line and any menu. The agent runs on a worker thread
//! (`worker.rs`).
//!
//! Input modes beyond plain prompts: `/` commands, `@` file mentions,
//! `!` runs a shell command (its output rides along with the next prompt),
//! `#` saves a note to shared memory, ctrl+space dictates.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event as CEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui_core::style::Color;
use ratatui_core::text::Line;
use serde_json::Value;
use zcore::agent::Mode;
use zproto::RunOpts;

use crate::files;
use crate::markdown::{self, Md};
use crate::slash::{self, Slash};
use crate::term::{Frame, Screen};
use crate::theme;
use crate::view;
use crate::voice::{self, Recorder};
use crate::worker::{self, Cmd, Diff, LocalCmd, Msg};

pub(crate) const PLACEHOLDER: &str = "Ask Cosmos to build features, fix bugs, or work on your code";

/// Reasoning-effort choices in the model picker (index 0 = provider default).
pub const EFFORTS: &[(&str, Option<&str>)] =
    &[("Auto", None), ("Low", Some("low")), ("Medium", Some("medium")), ("High", Some("high"))];

#[derive(Default, Clone)]
pub struct UsageInfo {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    /// prompt size of the last turn (input + cache) — the context in use
    pub last_prompt: u64,
    pub turns: u64,
    pub cum_usd: Option<f64>,
    pub model_usd: HashMap<String, f64>,
    /// the last turn alone: input, cache read, cache write, output
    pub last: [u64; 4],
    /// the last turn's price; None = unknown (or a subscription plan)
    pub last_usd: Option<f64>,
    /// the last turn ran on a subscription (no per-token price)
    pub plan: bool,
}

/// A `/model` picker row: a config.toml alias or a catalog `provider/model`.
#[derive(Clone)]
pub struct ModelRow {
    pub alias: String,
    pub provider: String,
    pub id: String,
    /// display name ("MiMo-V2.6-Pro"); the alias for config models
    pub name: String,
    pub context: u64,
    pub price: zproto::Prices,
    /// provider honours a reasoning-effort setting
    pub effort_ok: bool,
}

impl ModelRow {
    pub fn free(&self) -> bool {
        self.price == zproto::Prices::default()
    }
}

pub enum Popup {
    /// `/` command menu — `sel` indexes the fuzzy-ranked list.
    Slash { sel: usize },
    /// `@` file mentions under the cursor.
    Files { sel: usize },
    /// model picker — typed text filters, ←/→ set effort.
    Model { filter: Vec<char>, sel: usize, effort: usize },
    /// provider picker for /connect.
    Connect { filter: Vec<char>, sel: usize },
    /// earlier sessions for /resume: (id, preview, started)
    Resume { items: Vec<(String, String, String)>, sel: usize },
    Help,
}

pub struct Perm {
    pub tool: String,
    pub detail: String,
    pub sel: usize,
    reply: Sender<u8>,
}

/// A tool call that has been issued but hasn't returned yet (lives in the
/// live region with a spinner until its result arrives).
pub struct PendingTool {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// The assistant reply currently streaming in.
pub struct Stream {
    pub md: Md,
    pub partial: String,
    started: bool,
    pub chars: usize,
    /// full text, for /copy
    all: String,
}

/// Typing an API key for /connect — the composer is masked.
pub struct KeyEntry {
    pub provider: String,
    pub name: String,
}

enum LoginMsg {
    Url(String),
    Done(Result<zcore::chatgpt::Tokens>),
}

/// A ChatGPT browser sign-in in flight.
pub struct Login {
    rx: Receiver<LoginMsg>,
    cancel: Arc<AtomicBool>,
    pub url: Option<String>,
}

/// A `!command` running in the background.
pub struct ShellJob {
    pub cmd: String,
    rx: Receiver<(Option<i32>, String)>,
}

pub struct App {
    /// lines to commit to scrollback on the next frame
    pub out: Vec<Line<'static>>,
    pub width: usize,
    last_blank: bool,
    any_out: bool,

    pub input: Vec<char>,
    pub cursor: usize,
    history: Vec<String>,
    hist_pos: Option<usize>,
    draft: Vec<char>,
    pastes: Vec<String>,

    pub busy: bool,
    inflight: usize,
    pub busy_since: Instant,
    pub mode: Mode,
    pub model: String,
    pub alias: String,
    pub session: String,
    pub cwd: PathBuf,
    pub session_started: Instant,
    pub usage: UsageInfo,
    pub rate: Option<zproviders::RateLimits>,
    pub popup: Option<Popup>,
    pub perm: Option<Perm>,
    pub models: Vec<ModelRow>,
    pub commands: Vec<Slash>,
    pub files: files::Index,
    pub effort: usize,
    pub stream: Option<Stream>,
    pub tools: Vec<PendingTool>,
    diffs: HashMap<String, Diff>,
    pub plan_pending: bool,
    echoes: Vec<String>,
    quitting: bool,
    quit_deadline: Option<Instant>,
    done: bool,
    ctrl_c_at: Option<Instant>,
    pub notice: Option<(String, Color, Instant)>,
    ready_seen: bool,
    clear_screen: bool,
    pub dirty: bool,
    pub rec: Option<Recorder>,
    pub key_entry: Option<KeyEntry>,
    pub login: Option<Login>,
    pub shell: Option<ShellJob>,
    /// shell output waiting to ride along with the next prompt
    pending_ctx: Vec<String>,
    last_reply: String,
    /// operator name from onboarding — shown in the info line
    pub user: Option<String>,
    /// `/onboarding` asked the loop to hand the terminal to cosmostui-welcome
    pub replay_onboard: bool,
    /// the dictation key is physically down (Windows reports releases), so
    /// its auto-repeat can't stop a recording it just started
    toggle_held: bool,
    toggle_last: Option<Instant>,
}

fn model_rows() -> Vec<ModelRow> {
    let cfg = zcore::config::Config::load(&zcore::cli_home());
    cfg.available_models()
        .into_iter()
        .map(|m| ModelRow {
            effort_ok: matches!(m.kind.as_str(), "openai" | "responses" | "chatgpt"),
            alias: m.alias,
            provider: if m.from_config { m.provider } else { m.provider_name },
            id: m.model,
            name: m.name,
            context: m.context,
            price: m.price,
        })
        .collect()
}

impl App {
    pub fn new(cwd: &Path, models: Vec<ModelRow>, width: usize) -> App {
        App {
            out: Vec::new(),
            width,
            last_blank: true,
            any_out: false,
            input: Vec::new(),
            cursor: 0,
            history: load_history(),
            hist_pos: None,
            draft: Vec::new(),
            pastes: Vec::new(),
            busy: false,
            inflight: 0,
            busy_since: Instant::now(),
            mode: Mode::Agent,
            model: "…".into(),
            alias: String::new(),
            session: String::new(),
            cwd: cwd.to_path_buf(),
            session_started: Instant::now(),
            usage: UsageInfo::default(),
            rate: None,
            popup: None,
            perm: None,
            models,
            commands: slash::load(cwd),
            files: files::Index::default(),
            effort: 0,
            stream: None,
            tools: Vec::new(),
            diffs: HashMap::new(),
            plan_pending: false,
            echoes: Vec::new(),
            quitting: false,
            quit_deadline: None,
            done: false,
            ctrl_c_at: None,
            notice: None,
            ready_seen: false,
            clear_screen: false,
            dirty: true,
            rec: None,
            key_entry: None,
            login: None,
            shell: None,
            pending_ctx: Vec::new(),
            last_reply: String::new(),
            user: None,
            replay_onboard: false,
            toggle_held: false,
            toggle_last: None,
        }
    }

    // ---- scrollback --------------------------------------------------------

    /// Whether the last committed row is blank (so live rows needn't add a gap).
    pub fn last_out_blank(&self) -> bool {
        self.last_blank
    }

    fn blank_check(&mut self) {
        self.last_blank = self
            .out
            .last()
            .map(|l| l.spans.iter().all(|s| s.content.trim().is_empty()))
            .unwrap_or(self.last_blank);
    }

    /// Commit a block, separated from the previous one by one blank row.
    pub fn block(&mut self, lines: Vec<Line<'static>>) {
        if lines.is_empty() {
            return;
        }
        if self.any_out && !self.last_blank {
            self.out.push(Line::default());
        }
        self.out.extend(lines);
        self.any_out = true;
        self.blank_check();
        self.dirty = true;
    }

    /// Continue the previous block (streamed rows).
    fn cont(&mut self, lines: Vec<Line<'static>>) {
        if lines.is_empty() {
            return;
        }
        self.out.extend(lines);
        self.any_out = true;
        self.blank_check();
        self.dirty = true;
    }

    pub fn note(&mut self, s: impl Into<String>) {
        self.block(view::note(&s.into(), self.width));
    }

    pub fn error(&mut self, s: impl Into<String>) {
        self.block(view::error(&s.into(), self.width));
    }

    fn flash(&mut self, s: impl Into<String>, c: Color) {
        self.notice = Some((s.into(), c, Instant::now()));
        self.dirty = true;
    }

    fn reload_models(&mut self) {
        self.models = model_rows();
    }

    // ---- streaming ---------------------------------------------------------

    fn on_delta(&mut self, s: &str) {
        let w = self.width;
        let st = self.stream.get_or_insert_with(|| Stream { md: Md::new(), partial: String::new(), started: false, chars: 0, all: String::new() });
        st.partial.push_str(s);
        st.all.push_str(s);
        st.chars += s.chars().count();
        let mut ready: Vec<Line<'static>> = Vec::new();
        while let Some(i) = st.partial.find('\n') {
            let line: String = st.partial[..i].to_string();
            st.partial.drain(..=i);
            let rows = st.md.push(&line, w);
            // skip blank rows a reply opens with
            if !st.started && ready.is_empty() && rows.iter().all(|l| l.spans.iter().all(|s| s.content.trim().is_empty())) {
                continue;
            }
            ready.extend(rows);
        }
        let first = !ready.is_empty() && !st.started;
        if !ready.is_empty() {
            st.started = true;
        }
        if first {
            self.block(ready);
        } else {
            self.cont(ready);
        }
        self.dirty = true;
    }

    /// Close the streaming reply: flush its last line and any held table.
    fn end_stream(&mut self) {
        if let Some(mut st) = self.stream.take() {
            let w = self.width;
            let mut rows = Vec::new();
            if !st.partial.is_empty() {
                rows.extend(st.md.push(&std::mem::take(&mut st.partial), w));
            }
            rows.extend(st.md.finish(w));
            while rows.last().is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty())) {
                rows.pop();
            }
            if !st.all.trim().is_empty() {
                self.last_reply = st.all.trim().to_string();
            }
            if st.started {
                self.cont(rows);
            } else {
                self.block(rows);
            }
        }
    }

    // ---- worker messages ---------------------------------------------------

    fn on_msg(&mut self, m: Msg) {
        self.dirty = true;
        match m {
            Msg::Ready { session, alias, model, mode, degraded } => {
                self.session = session;
                if alias != self.alias && !alias.is_empty() {
                    save_state_model(&alias);
                }
                self.alias = alias;
                self.model = model;
                self.mode = mode;
                if !self.ready_seen {
                    self.ready_seen = true;
                    let info = view::info_line(self);
                    self.cont(vec![Line::default(), info]);
                    if let Some(d) = degraded.filter(|d| !d.is_empty()) {
                        self.block(view::warn(&format!("Sandbox degraded — {d}"), self.width));
                    }
                }
            }
            Msg::Event(ev) => self.on_event(&ev),
            Msg::Delta(s) => self.on_delta(&s),
            Msg::Diff { id, diff } => {
                self.diffs.insert(id, diff);
            }
            Msg::PermissionAsk { tool, reply } => {
                let detail = self
                    .tools
                    .iter()
                    .rev()
                    .find(|t| t.name == tool)
                    .map(|t| view::arg_summary(&t.name, &t.input, 200))
                    .unwrap_or_default();
                self.popup = None;
                self.perm = Some(Perm { tool, detail, sel: 0, reply });
            }
            Msg::Note(s) => self.note(s),
            Msg::Err(s) => self.error(s),
            Msg::TurnDone => {
                self.end_stream();
                // anything still pending was cut short
                for t in std::mem::take(&mut self.tools) {
                    let lines = view::tool_block(&t.name, &t.input, None, false, None, self.width, true);
                    self.block(lines);
                }
                self.inflight = self.inflight.saturating_sub(1);
                if self.inflight == 0 {
                    self.busy = false;
                }
                if let Some(r) = zproviders::last_rate_limits() {
                    self.rate = Some(r);
                }
            }
            Msg::History(items) => {
                let w = self.width;
                let n = items.len();
                let skip = n.saturating_sub(12);
                if skip > 0 {
                    self.note(format!("{skip} earlier messages not shown"));
                }
                for (is_user, text) in items.into_iter().skip(skip) {
                    if is_user {
                        self.block(view::user(&text, w));
                    } else {
                        self.block(markdown::render(text.trim_matches('\n'), w));
                    }
                }
                self.note(format!("Resumed session {} · {n} messages", self.session));
            }
            Msg::QuitDone => self.done = true,
        }
    }

    fn on_event(&mut self, ev: &zproto::Event) {
        let w = self.width;
        match ev.kind.as_str() {
            "user" => {
                let text = ev.data["text"].as_str().unwrap_or("").to_string();
                if let Some(i) = self.echoes.iter().position(|e| e == &text) {
                    self.echoes.remove(i);
                } else {
                    self.block(view::user(&text, w));
                }
            }
            "assistant" => {
                let text = ev.data["text"].as_str().unwrap_or("");
                if self.stream.is_some() {
                    self.end_stream();
                } else if !text.trim().is_empty() {
                    self.last_reply = text.trim().to_string();
                    self.block(markdown::render(text.trim_matches('\n'), w));
                }
                if let Some(calls) = ev.data["tool_calls"].as_array() {
                    for c in calls {
                        let name = c["name"].as_str().unwrap_or("tool");
                        // a deferred tool through the built-in server shows as itself
                        let (name, input) = ztools::unwrap_self_call(name, &c["input"])
                            .unwrap_or_else(|| (name.to_string(), c["input"].clone()));
                        self.tools.push(PendingTool {
                            id: c["id"].as_str().unwrap_or("").to_string(),
                            name,
                            input,
                        });
                    }
                }
            }
            "tool_result" => {
                let id = ev.data["tool_use_id"].as_str().unwrap_or("");
                let err = ev.data["is_error"].as_bool().unwrap_or(false);
                let content = ev.data["content"].as_str().unwrap_or("");
                let (name, input) = match self.tools.iter().position(|t| t.id == id) {
                    Some(i) => {
                        let t = self.tools.remove(i);
                        (t.name, t.input)
                    }
                    None => (ev.data["name"].as_str().unwrap_or("tool").to_string(), Value::Null),
                };
                let diff = self.diffs.remove(id);
                if name != "submit_plan" {
                    let lines = view::tool_block(&name, &input, Some(content), err, diff.as_ref(), w, false);
                    self.block(lines);
                }
            }
            // a subscription agent's own tool (ACP) — shown, never replayed
            "tool_call" if ev.data["source"] == "acp" => {
                let name = ev.data["name"].as_str().unwrap_or("tool");
                let status = ev.data["status"].as_str().unwrap_or("");
                let input = serde_json::json!({"title": ev.data["title"]});
                let out = ev.data["output"].as_str().unwrap_or("");
                self.block(view::tool_block(name, &input, Some(out), status == "failed", None, w, false));
            }
            "plan" => {
                self.plan_pending = true;
                self.block(view::plan(ev.data["plan"].as_str().unwrap_or(""), w));
            }
            "usage" => {
                let g = |k: &str| ev.data[k].as_u64().unwrap_or(0);
                let u = &mut self.usage;
                u.input += g("input");
                u.cache_read += g("cache_read");
                u.cache_write += g("cache_write");
                u.output += g("output");
                u.last_prompt = g("input") + g("cache_read") + g("cache_write");
                u.last = [g("input"), g("cache_read"), g("cache_write"), g("output")];
                u.last_usd = ev.data["usd"].as_f64();
                u.plan = ev.data["billing"] == "subscription";
                u.turns += 1;
                if ev.data["usd"].is_number() {
                    u.cum_usd = ev.data["cum_usd"].as_f64();
                }
                if let (Some(model), Some(turn_usd)) = (
                    ev.data["model"].as_str(),
                    ev.data["turn_usd"].as_f64(),
                ) {
                    *u.model_usd.entry(model.to_owned()).or_default() += turn_usd;
                }
            }
            "compaction" => {
                let (b, a) = (ev.data["before_tokens"].as_u64().unwrap_or(0), ev.data["after_tokens"].as_u64().unwrap_or(0));
                self.note(format!("Compacted context {} → {} tokens", view::kfmt(b), view::kfmt(a)));
            }
            "subagent_start" => {
                let p = ev.data["prompt"].as_str().unwrap_or("");
                self.note(format!("Subagent started — {}", crate::wrap::ellipsize(p, 80)));
            }
            "subagent_end" | "child_session" => self.note("Subagent finished"),
            "mode" => {
                let to = ev.data["to"].as_str().unwrap_or("");
                self.mode = Mode::parse(to);
                let label = match self.mode {
                    Mode::Plan => "Plan mode — Cosmos proposes before it changes anything",
                    Mode::Yolo => "Yolo mode — tools run without asking (sandbox stays on)",
                    Mode::AcceptEdits => "Accept-edits — writes auto-allow inside the workspace",
                    Mode::Agent => "Agent mode",
                };
                self.flash(label, match self.mode {
                    Mode::Plan => theme::YELLOW,
                    Mode::Yolo => theme::RED,
                    Mode::AcceptEdits => theme::ACCENT,
                    Mode::Agent => theme::GREEN,
                });
            }
            _ => {}
        }
    }

    // ---- background jobs ---------------------------------------------------

    /// Poll dictation, sign-in and shell jobs (once per loop turn).
    fn poll_jobs(&mut self) {
        if let Some(rec) = &mut self.rec {
            if !rec.transcribing {
                rec.tick();
                self.dirty = true;
            }
            if let Some(r) = rec.poll() {
                let label = rec.backend.label();
                self.rec = None;
                self.dirty = true;
                match r {
                    Ok(text) => {
                        if self.cursor > 0 && !self.input[self.cursor - 1].is_whitespace() {
                            self.insert(' ');
                        }
                        self.insert_str(&text);
                        self.flash(format!("Transcribed with {label} — review, then ↵ to send"), theme::GREEN);
                    }
                    Err(e) if e.to_string() == "cancelled" => self.flash("Dictation cancelled", theme::YELLOW),
                    Err(e) => self.flash(format!("Dictation failed: {e}"), theme::RED),
                }
            }
        }
        let login_msgs: Vec<LoginMsg> = self.login.as_ref().map(|l| l.rx.try_iter().collect()).unwrap_or_default();
        for m in login_msgs {
            self.dirty = true;
            match m {
                LoginMsg::Url(u) => {
                    if let Some(l) = &mut self.login {
                        l.url = Some(u.clone());
                    }
                    let lines = view::login_url(&u, self.width);
                    self.block(lines);
                }
                LoginMsg::Done(r) => {
                    self.login = None;
                    match r {
                        Ok(t) => {
                            self.reload_models();
                            let who = t.email.clone().map(|e| format!(" as {e}")).unwrap_or_default();
                            let plan = t.plan.clone().map(|p| format!(" ({p})")).unwrap_or_default();
                            self.note(format!("Signed in to ChatGPT{who}{plan} · pick a chatgpt/… model in /model"));
                            self.flash("ChatGPT connected", theme::GREEN);
                            self.open_model_picker_for("chatgpt");
                        }
                        Err(e) => self.error(format!("ChatGPT sign-in failed: {e:#}")),
                    }
                    break;
                }
            }
        }
        if let Some(job) = &self.shell {
            if let Ok((code, out)) = job.rx.try_recv() {
                let cmd = job.cmd.clone();
                self.shell = None;
                let w = self.width;
                self.block(view::shell_block(&cmd, &out, code, w));
                let tail: String = out.chars().rev().take(12_000).collect::<Vec<_>>().into_iter().rev().collect();
                self.pending_ctx.push(format!("<shell-output command={cmd:?} exit={}>\n{tail}\n</shell-output>", code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())));
                self.dirty = true;
            }
        }
    }

    /// True when this Ctrl+Space press is the key's auto-repeat. Windows
    /// reports key releases, so "held" is exact there (with a timeout in case
    /// a release goes missing); elsewhere a short debounce does the job.
    fn toggle_repeat(&mut self) -> bool {
        let now = Instant::now();
        let since = self.toggle_last.map(|t| now.duration_since(t));
        self.toggle_last = Some(now);
        if cfg!(windows) {
            let repeat = self.toggle_held && since.is_some_and(|d| d < Duration::from_secs(2));
            self.toggle_held = true;
            repeat
        } else {
            since.is_some_and(|d| d < Duration::from_millis(250))
        }
    }

    /// A key came up (Windows only reports these).
    pub fn on_release(&mut self, k: KeyEvent) {
        if matches!(k.code, KeyCode::Char(' ') | KeyCode::Null) {
            self.toggle_held = false;
        }
    }

    fn toggle_dictation(&mut self) {
        if let Some(rec) = &mut self.rec {
            if !rec.transcribing {
                rec.finish();
                self.dirty = true;
            }
            return;
        }
        match voice::pick_backend() {
            None => self.error("Dictation needs an OpenAI or Groq key (/connect openai) — or Windows for offline speech"),
            Some(b) => match Recorder::start(b) {
                Ok(r) => {
                    self.popup = None;
                    self.rec = Some(r);
                }
                Err(e) => self.error(format!("Couldn't start dictation: {e:#}")),
            },
        }
        self.dirty = true;
    }

    fn start_login(&mut self) {
        if self.login.is_some() {
            self.flash("A ChatGPT sign-in is already waiting in your browser", theme::YELLOW);
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::spawn(move || {
            let tx2 = tx.clone();
            let r = zcore::chatgpt::login(&c, move |url| {
                let _ = tx2.send(LoginMsg::Url(url.to_string()));
                zcore::chatgpt::open_browser(url);
            });
            let _ = tx.send(LoginMsg::Done(r));
        });
        self.login = Some(Login { rx, cancel, url: None });
        self.flash("Opening your browser to sign in with ChatGPT… (esc cancels)", theme::ACCENT);
    }

    fn start_shell(&mut self, cmd: String) {
        let (tx, rx) = channel();
        let cwd = self.cwd.clone();
        let c = cmd.clone();
        std::thread::spawn(move || {
            #[cfg(windows)]
            let out = std::process::Command::new("cmd").args(["/D", "/S", "/C", &c]).current_dir(&cwd).output();
            #[cfg(not(windows))]
            let out = std::process::Command::new("sh").args(["-c", &c]).current_dir(&cwd).output();
            let r = match out {
                Ok(o) => {
                    let mut s = String::from_utf8_lossy(&o.stdout).to_string();
                    let e = String::from_utf8_lossy(&o.stderr);
                    if !e.trim().is_empty() {
                        if !s.is_empty() && !s.ends_with('\n') {
                            s.push('\n');
                        }
                        s.push_str(&e);
                    }
                    (o.status.code(), s)
                }
                Err(e) => (None, format!("couldn't run: {e}")),
            };
            let _ = tx.send(r);
        });
        self.shell = Some(ShellJob { cmd, rx });
    }

    // ---- input buffer ------------------------------------------------------

    fn insert(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += 1;
        self.hist_pos = None;
    }

    fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert(c);
        }
    }

    fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.hist_pos = None;
    }

    fn set_input(&mut self, s: &[char]) {
        self.input = s.to_vec();
        self.cursor = self.input.len();
    }

    fn word_left(&self) -> usize {
        let mut i = self.cursor;
        while i > 0 && self.input[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.input[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    fn word_right(&self) -> usize {
        let mut i = self.cursor;
        let n = self.input.len();
        while i < n && self.input[i].is_whitespace() {
            i += 1;
        }
        while i < n && !self.input[i].is_whitespace() {
            i += 1;
        }
        i
    }

    /// Pasted text from any source: into a popup's filter, or the composer.
    pub fn paste_text(&mut self, s: &str) {
        if self.quitting || self.rec.is_some() {
            return;
        }
        match &mut self.popup {
            Some(Popup::Model { filter, .. } | Popup::Connect { filter, .. }) => filter.extend(s.chars().filter(|c| !c.is_control())),
            _ if self.perm.is_none() => {
                self.paste(s);
                self.refresh_popups();
            }
            _ => {}
        }
        self.dirty = true;
    }

    fn paste(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        if self.key_entry.is_some() {
            self.insert_str(s.trim());
            return;
        }
        let lines = s.lines().count();
        if lines > 8 || s.chars().count() > 2000 {
            self.pastes.push(s);
            let token = format!("[Pasted text #{} +{} lines]", self.pastes.len(), lines);
            self.insert_str(&token);
        } else {
            self.insert_str(&s);
        }
    }

    /// Expand `[Pasted text #n …]` placeholders back into their content.
    fn expand_pastes(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (i, p) in self.pastes.iter().enumerate() {
            let lines = p.lines().count();
            out = out.replace(&format!("[Pasted text #{} +{} lines]", i + 1, lines), p);
        }
        out
    }

    pub(crate) fn slash_state(&self) -> Option<(String, String)> {
        if self.key_entry.is_some() {
            return None;
        }
        slash::parse(&self.input)
    }

    /// Open / close the `/` and `@` menus to match the input.
    fn refresh_popups(&mut self) {
        if matches!(self.popup, Some(Popup::Model { .. } | Popup::Connect { .. } | Popup::Resume { .. } | Popup::Help)) || self.key_entry.is_some() {
            return;
        }
        let slash_open = self.slash_state().is_some_and(|(_, a)| a.is_empty() && !self.input.contains(&' '));
        let mention = files::mention_at(&self.input, self.cursor);
        if slash_open {
            if !matches!(self.popup, Some(Popup::Slash { .. })) {
                self.popup = Some(Popup::Slash { sel: 0 });
            }
        } else if mention.is_some() {
            self.files.ensure(&self.cwd);
            if !matches!(self.popup, Some(Popup::Files { .. })) {
                self.popup = Some(Popup::Files { sel: 0 });
            }
        } else if matches!(self.popup, Some(Popup::Slash { .. } | Popup::Files { .. })) {
            self.popup = None;
        }
    }

    /// Replace the `@token` under the cursor with `@path `.
    fn accept_mention(&mut self, path: &str) {
        if let Some((start, _)) = files::mention_at(&self.input, self.cursor) {
            let end = self.cursor;
            let rep: Vec<char> = format!("@{path} ").chars().collect();
            self.input.splice(start..end, rep.iter().copied());
            self.cursor = start + rep.len();
        }
        self.popup = None;
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let i = match self.hist_pos {
            None => {
                self.draft = self.input.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.hist_pos = Some(i);
        let h: Vec<char> = self.history[i].chars().collect();
        self.set_input(&h);
    }

    fn history_next(&mut self) {
        match self.hist_pos {
            None => {}
            Some(i) if i + 1 < self.history.len() => {
                self.hist_pos = Some(i + 1);
                let h: Vec<char> = self.history[i + 1].chars().collect();
                self.set_input(&h);
            }
            Some(_) => {
                self.hist_pos = None;
                let d = std::mem::take(&mut self.draft);
                self.set_input(&d);
            }
        }
    }

    // ---- actions -----------------------------------------------------------

    fn start_busy(&mut self) {
        if self.inflight == 0 {
            self.busy_since = Instant::now();
        }
        self.inflight += 1;
        self.busy = true;
    }

    /// Send a prompt to the agent (`shown` is what the transcript displays).
    fn send_prompt(&mut self, shown: &str, text: String, cmd: &Sender<Cmd>) {
        let mut text = files::expand_mentions(&text, &self.cwd);
        if !self.pending_ctx.is_empty() {
            let ctx = std::mem::take(&mut self.pending_ctx).join("\n\n");
            text = format!("{ctx}\n\n{text}");
        }
        self.echoes.push(text.clone());
        self.block(view::user(shown, self.width));
        self.start_busy();
        let _ = cmd.send(Cmd::Prompt(text));
    }

    fn submit(&mut self, cmd: &Sender<Cmd>) {
        let raw: String = self.input.iter().collect();
        if let Some(k) = self.key_entry.take() {
            self.clear_input();
            let key = raw.trim();
            if key.is_empty() {
                self.flash("No key entered", theme::YELLOW);
                return;
            }
            match zcore::creds::store_key(&k.provider, key) {
                Ok(src) => {
                    if let Some(p) = zcore::catalog::provider(&k.provider) {
                        match zcore::live::fetch(p) {
                            Ok(models) => {
                                let _ = zcore::live::store(&k.provider, &models);
                            }
                            Err(e) => self.note(format!("Saved the key, but the live list failed: {e:#}")),
                        }
                    }
                    self.reload_models();
                    let first = self.models.iter().find(|m| m.alias.starts_with(&format!("{}/", k.provider))).map(|m| m.alias.clone());
                    self.note(format!("Saved {} key {} to the {}", k.name, zcore::creds::mask(key), src.label()));
                    if first.is_some() {
                        self.open_model_picker_for(&k.provider);
                    }
                }
                Err(e) => self.error(format!("Couldn't save the key: {e:#}")),
            }
            return;
        }
        let raw = raw.trim().to_string();
        if raw.is_empty() {
            return;
        }
        self.clear_input();
        self.popup = None;
        if let Some((name, args)) = slash::parse(&raw.chars().collect::<Vec<_>>()) {
            push_history(&mut self.history, &raw);
            self.run_slash(&name, &args, cmd);
            return;
        }
        push_history(&mut self.history, &raw);
        if let Some(sh) = raw.strip_prefix('!') {
            let sh = sh.trim();
            if !sh.is_empty() {
                self.start_shell(sh.to_string());
            }
            return;
        }
        if let Some(note) = raw.strip_prefix('#') {
            let note = note.trim();
            if !note.is_empty() {
                let mut ctx = ztools::ToolContext::new(&self.cwd);
                let r = ztools::run(&mut ctx, "memory_write", &serde_json::json!({"type": "preference", "text": note, "scope": "global"}));
                if r.to_lowercase().contains("error") {
                    self.error(format!("Memory: {r}"));
                } else {
                    self.note(format!("Remembered — \"{}\"", crate::wrap::ellipsize(note, 80)));
                }
            }
            return;
        }
        let text = self.expand_pastes(&raw);
        self.pastes.clear();
        self.send_prompt(&raw, text, cmd);
    }

    fn run_slash(&mut self, name: &str, args: &str, cmd: &Sender<Cmd>) {
        match name {
            "model" => {
                if args.is_empty() {
                    self.open_model_picker();
                } else if self.models.iter().any(|m| m.alias == args) || zcore::catalog::split_ref(args).is_some() {
                    let _ = cmd.send(Cmd::SetModel(args.to_string()));
                } else {
                    self.error(format!("Unknown model: {args} — try provider/model, e.g. anthropic/claude-opus-5-5"));
                }
            }
            "connect" | "providers" => {
                if args.is_empty() {
                    self.popup = Some(Popup::Connect { filter: Vec::new(), sel: 0 });
                } else {
                    self.connect(args);
                }
            }
            "login" => self.start_login(),
            "logout" => {
                let id = if args.is_empty() { "chatgpt" } else { args };
                let r = if id == "chatgpt" { zcore::chatgpt::logout() } else { zcore::creds::delete_key(id) };
                match r {
                    Ok(()) => {
                        self.reload_models();
                        self.note(format!("Removed {id} credentials"));
                    }
                    Err(e) => self.error(format!("{e:#}")),
                }
            }
            "voice" => self.toggle_dictation(),
            "mode" => match args {
                "" => self.note(format!("Mode: {} · shift+tab cycles plan → agent → accept-edits → yolo", self.mode.name())),
                "plan" => {
                    let _ = cmd.send(Cmd::SetMode(Mode::Plan));
                }
                "agent" => {
                    let _ = cmd.send(Cmd::SetMode(Mode::Agent));
                }
                "accept-edits" | "accept_edits" => {
                    let _ = cmd.send(Cmd::SetMode(Mode::AcceptEdits));
                }
                "yolo" => {
                    let _ = cmd.send(Cmd::SetMode(Mode::Yolo));
                }
                other => self.error(format!("Unknown mode: {other} (plan, agent, accept-edits or yolo)")),
            },
            // kept as shortcuts for muscle memory
            "plan" => {
                let _ = cmd.send(Cmd::SetMode(Mode::Plan));
            }
            "agent" => {
                let _ = cmd.send(Cmd::SetMode(Mode::Agent));
            }
            "yolo" => {
                let _ = cmd.send(Cmd::SetMode(Mode::Yolo));
            }
            "accept-edits" | "accept_edits" => {
                let _ = cmd.send(Cmd::SetMode(Mode::AcceptEdits));
            }
            "approve" => {
                if self.plan_pending {
                    self.plan_pending = false;
                    self.start_busy();
                    let _ = cmd.send(Cmd::Approve);
                } else {
                    self.note("No plan is waiting for approval");
                }
            }
            // these run in the harness, never as a model turn (contract §7)
            "compact" => {
                let focus = args.strip_prefix("focus:").or_else(|| args.strip_prefix("focus")).unwrap_or(args).trim();
                let _ = cmd.send(Cmd::Local(LocalCmd::Compact((!focus.is_empty()).then(|| focus.to_string()))));
            }
            "undo" => {
                let _ = cmd.send(Cmd::Local(LocalCmd::Undo));
            }
            "skillify" => {
                let name = if args.is_empty() { "session" } else { args };
                let _ = cmd.send(Cmd::Local(LocalCmd::Skillify(name.to_string())));
            }
            "usage" | "cost" => {
                let lines = view::usage(self);
                self.block(lines);
            }
            "context" => {
                let lines = view::context(self);
                self.block(lines);
            }
            "resume" => {
                let items: Vec<(String, String, String)> = zcore::session::Session::list(&zcore::cli_home())
                    .into_iter()
                    .rev()
                    .filter(|s| s["id"].as_str() != Some(self.session.as_str()) && !s["preview"].as_str().unwrap_or("").is_empty())
                    .take(40)
                    .map(|s| {
                        (
                            s["id"].as_str().unwrap_or("").to_string(),
                            s["preview"].as_str().unwrap_or("").to_string(),
                            s["started"].as_str().unwrap_or("").to_string(),
                        )
                    })
                    .collect();
                if items.is_empty() {
                    self.note("No earlier sessions to resume");
                } else {
                    self.popup = Some(Popup::Resume { items, sel: 0 });
                }
            }
            "init" => {
                match zcore::init_project(&self.cwd) {
                    Ok(msg) => self.note(msg),
                    Err(err) => self.note(format!("could not create AGENTS.md: {err}")),
                }
                self.send_prompt(
                    "/init",
                    "Review AGENTS.md and fill in its project summary, build, test, and contribution guidance accurately. Keep it short and preserve existing instructions.".into(),
                    cmd,
                );
            }
            "review" => self.send_prompt(
                "/review",
                format!(
                    "Review the uncommitted changes in this repository (use `git status` and `git diff`, plus `git diff --staged`). Report real problems first — bugs, security issues, broken edge cases, missing tests — each with file:line and a concrete fix. Skip praise and style nits. Don't modify files.{}",
                    if args.is_empty() { String::new() } else { format!(" Focus: {args}") }
                ),
                cmd,
            ),
            "diff" => {
                let run = |a: &[&str]| {
                    std::process::Command::new("git").args(a).current_dir(&self.cwd).output().ok().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
                };
                let stat = run(&["diff", "--stat", "HEAD"]);
                let status = run(&["status", "--short"]);
                let w = self.width;
                if stat.trim().is_empty() && status.trim().is_empty() {
                    self.note("Working tree is clean (or not a git repository)");
                } else {
                    self.block(view::shell_block("git diff --stat HEAD", &format!("{status}{stat}"), Some(0), w));
                }
            }
            "copy" => {
                if self.last_reply.is_empty() {
                    self.note("Nothing to copy yet");
                } else {
                    copy_osc52(&self.last_reply);
                    self.flash(format!("Copied the last reply ({} chars)", self.last_reply.chars().count()), theme::GREEN);
                }
            }
            "session" => self.note(format!("Session {} · resume with  cosmostui --resume {}", self.session, self.session)),
            "tree" => {
                let lines = view::tree(self);
                self.block(lines);
            }
            "clear" => self.clear_screen = true,
            "onboarding" => self.replay_onboard = true,
            "help" => self.popup = Some(Popup::Help),
            "quit" | "exit" | "q" => self.request_quit(cmd),
            other => {
                if let Some(body) = self.commands.iter().find(|s| s.name == other).and_then(|s| s.body.clone()) {
                    let prompt = body.replace("$ARGUMENTS", args);
                    let shown = if args.is_empty() { format!("/{other}") } else { format!("/{other} {args}") };
                    self.send_prompt(&shown, prompt, cmd);
                } else {
                    self.error(format!("Unknown command: /{other}"));
                }
            }
        }
    }

    /// Begin /connect for a provider: ChatGPT signs in, subscription agents
    /// (Cursor, Devin, Codex) use their own CLI, others ask for a key.
    fn connect(&mut self, id: &str) {
        self.popup = None;
        if id == "chatgpt" {
            self.start_login();
            return;
        }
        if zcore::acp_client::plugin(id).is_some() {
            self.connect_acp(id);
            return;
        }
        match zcore::catalog::provider(id) {
            Some(p) => {
                self.clear_input();
                self.key_entry = Some(KeyEntry { provider: p.id.clone(), name: p.name.clone() });
                let env = p.env.first().cloned().unwrap_or_default();
                self.flash(format!("Paste your {} API key and press ↵ (or set {env}) · esc cancels", p.name), theme::ACCENT);
            }
            None => self.error(format!("Unknown provider: {id}")),
        }
    }

    /// A subscription agent needs only its CLI on PATH and its own sign-in;
    /// it runs on the operator's plan, never on a key Cosmos holds.
    fn connect_acp(&mut self, id: &str) {
        let Some(p) = zcore::acp_client::plugin(id) else { return };
        if !zcore::acp_client::installed(id) {
            self.error(format!("{} isn't installed — {}", p.name, p.install));
            return;
        }
        // the agent reports the plan's real model list on session/new —
        // grab it now so the picker shows more than "<id>/default"
        match zcore::catalog::provider(id).map(zcore::live::fetch) {
            Some(Ok(models)) => {
                let _ = zcore::live::store(id, &models);
            }
            Some(Err(e)) => self.note(format!("Couldn't get {}'s model list ({e:#}) — showing its default", p.name)),
            None => {}
        }
        self.reload_models();
        self.note(format!(
            "{} found — it runs on your {} plan. Not signed in yet? Run `{}` in a terminal (or `cosmostui auth login {id}`).",
            p.name,
            p.name,
            p.login.join(" ")
        ));
        self.open_model_picker_for(id);
    }

    fn open_model_picker(&mut self) {
        let order = view::model_order(&self.models, "");
        let sel = order.iter().position(|&i| self.models[i].alias == self.alias).unwrap_or(0);
        self.popup = Some(Popup::Model { filter: Vec::new(), sel, effort: self.effort });
    }

    fn open_model_picker_for(&mut self, provider: &str) {
        let filter: Vec<char> = provider.chars().collect();
        self.popup = Some(Popup::Model { filter, sel: 0, effort: self.effort });
    }

    fn cycle_mode(&mut self, cmd: &Sender<Cmd>) {
        let _ = cmd.send(Cmd::SetMode(self.mode.cycle()));
    }

    fn request_quit(&mut self, cmd: &Sender<Cmd>) {
        if let Some(r) = &self.rec {
            r.cancel();
        }
        if let Some(l) = &self.login {
            l.cancel.store(true, Ordering::Relaxed);
        }
        if !self.quitting {
            self.quitting = true;
            self.quit_deadline = Some(Instant::now() + Duration::from_millis(800));
            let _ = cmd.send(Cmd::Quit);
        }
    }

    fn answer_perm(&mut self, code: u8) {
        if let Some(p) = self.perm.take() {
            let verdict = match code {
                1 => "Allowed",
                2 => "Allowed for this session",
                _ => "Denied",
            };
            let _ = p.reply.send(code);
            self.flash(format!("{verdict} — {}", view::tool_label(&p.tool)), if code == 0 { theme::RED } else { theme::GREEN });
        }
    }

    fn interrupt(&mut self, flag: &Arc<AtomicBool>) {
        flag.store(true, Ordering::Relaxed);
        self.flash("Interrupting — stops after the current step", theme::YELLOW);
    }

    // ---- keys --------------------------------------------------------------

    fn on_key(&mut self, k: KeyEvent, cmd: &Sender<Cmd>, interrupt: &Arc<AtomicBool>) {
        if self.quitting {
            return;
        }
        self.dirty = true;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);

        // Ctrl+Space toggles dictation: once to start, once more to stop
        let toggle = (ctrl && k.code == KeyCode::Char(' ')) || k.code == KeyCode::Null;
        if toggle && self.toggle_repeat() {
            return; // the key is still held — auto-repeat, not a second press
        }
        // dictation owns the keyboard while recording
        if let Some(rec) = &mut self.rec {
            match k.code {
                KeyCode::Esc => rec.cancel(),
                KeyCode::Char('c') if ctrl => rec.cancel(),
                KeyCode::Enter if !rec.transcribing => rec.finish(),
                _ if toggle && !rec.transcribing => rec.finish(),
                _ => {}
            }
            return;
        }
        if toggle {
            self.toggle_dictation();
            return;
        }

        // Ctrl+C: interrupt → clear → (twice) quit
        if ctrl && k.code == KeyCode::Char('c') {
            if self.perm.is_some() {
                self.answer_perm(0);
            } else if let Some(l) = self.login.take() {
                l.cancel.store(true, Ordering::Relaxed);
                self.flash("Sign-in cancelled", theme::YELLOW);
            } else if self.busy {
                self.interrupt(interrupt);
            } else if !self.input.is_empty() || self.popup.is_some() || self.key_entry.is_some() {
                self.clear_input();
                self.popup = None;
                self.key_entry = None;
            } else if self.ctrl_c_at.is_some_and(|t| t.elapsed() < Duration::from_millis(1500)) {
                self.request_quit(cmd);
            } else {
                self.ctrl_c_at = Some(Instant::now());
                self.flash("Press Ctrl+C again to exit", theme::YELLOW);
            }
            return;
        }
        if ctrl && k.code == KeyCode::Char('d') && self.input.is_empty() {
            self.request_quit(cmd);
            return;
        }
        if ctrl && k.code == KeyCode::Char('l') {
            self.clear_screen = true;
            return;
        }

        // permission prompt owns the keyboard
        if let Some(p) = &mut self.perm {
            match k.code {
                KeyCode::Up => p.sel = p.sel.saturating_sub(1),
                KeyCode::Down => p.sel = (p.sel + 1).min(2),
                KeyCode::Char('y') | KeyCode::Char('1') => self.answer_perm(1),
                KeyCode::Char('a') | KeyCode::Char('2') => self.answer_perm(2),
                KeyCode::Char('n') | KeyCode::Char('3') | KeyCode::Esc => self.answer_perm(0),
                KeyCode::Enter => {
                    let code = [1, 2, 0][p.sel];
                    self.answer_perm(code);
                }
                _ => {}
            }
            return;
        }
        if self.key_entry.is_some() && k.code == KeyCode::Esc {
            self.key_entry = None;
            self.clear_input();
            self.flash("Cancelled", theme::YELLOW);
            return;
        }
        if k.code == KeyCode::Esc {
            if let Some(l) = self.login.take() {
                l.cancel.store(true, Ordering::Relaxed);
                self.flash("Sign-in cancelled", theme::YELLOW);
                return;
            }
        }

        match &mut self.popup {
            Some(Popup::Help) => {
                if matches!(k.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') | KeyCode::Char('?')) {
                    self.popup = None;
                }
                return;
            }
            Some(Popup::Model { filter, sel, effort }) => {
                let q: String = filter.iter().collect();
                let n = view::model_order(&self.models, &q).len() + 1; // + "connect" row
                match k.code {
                    KeyCode::Esc => self.popup = None,
                    KeyCode::Up => *sel = if *sel == 0 { n - 1 } else { *sel - 1 },
                    KeyCode::Down | KeyCode::Tab => *sel = if *sel + 1 >= n { 0 } else { *sel + 1 },
                    KeyCode::Left => *effort = effort.saturating_sub(1),
                    KeyCode::Right => *effort = (*effort + 1).min(EFFORTS.len() - 1),
                    KeyCode::Backspace => {
                        filter.pop();
                        *sel = 0;
                    }
                    KeyCode::Enter => {
                        let (sel, effort) = (*sel, *effort);
                        let order = view::model_order(&self.models, &q);
                        if sel >= order.len() {
                            self.popup = Some(Popup::Connect { filter: Vec::new(), sel: 0 });
                            return;
                        }
                        let alias = self.models[order[sel]].alias.clone();
                        self.popup = None;
                        if effort != self.effort {
                            self.effort = effort;
                            let _ = cmd.send(Cmd::SetEffort(EFFORTS[effort].1.map(String::from)));
                        }
                        if alias != self.alias {
                            let _ = cmd.send(Cmd::SetModel(alias));
                        }
                    }
                    KeyCode::Char(c) if !ctrl => {
                        filter.push(c);
                        *sel = 0;
                    }
                    _ => {}
                }
                return;
            }
            Some(Popup::Connect { filter, sel }) => {
                let q: String = filter.iter().collect();
                let order = view::connect_order(&q);
                let n = order.len().max(1);
                match k.code {
                    KeyCode::Esc => self.popup = None,
                    KeyCode::Up => *sel = if *sel == 0 { n - 1 } else { *sel - 1 },
                    KeyCode::Down | KeyCode::Tab => *sel = if *sel + 1 >= n { 0 } else { *sel + 1 },
                    KeyCode::Backspace => {
                        filter.pop();
                        *sel = 0;
                    }
                    KeyCode::Enter => {
                        if let Some(&i) = order.get(*sel) {
                            let id = zcore::catalog::providers()[i].id.clone();
                            self.connect(&id);
                        }
                    }
                    KeyCode::Char(c) if !ctrl => {
                        filter.push(c);
                        *sel = 0;
                    }
                    _ => {}
                }
                return;
            }
            Some(Popup::Resume { items, sel }) => {
                let n = items.len().max(1);
                match k.code {
                    KeyCode::Esc => self.popup = None,
                    KeyCode::Up => *sel = if *sel == 0 { n - 1 } else { *sel - 1 },
                    KeyCode::Down | KeyCode::Tab => *sel = if *sel + 1 >= n { 0 } else { *sel + 1 },
                    KeyCode::Enter => {
                        if let Some((id, _, _)) = items.get(*sel) {
                            let _ = cmd.send(Cmd::Resume(id.clone()));
                        }
                        self.popup = None;
                    }
                    _ => {}
                }
                return;
            }
            Some(Popup::Files { sel }) => {
                let q = files::mention_at(&self.input, self.cursor).map(|(_, q)| q).unwrap_or_default();
                let hits = self.files.search(&q, view::MENU_ROWS);
                let n = hits.len();
                match k.code {
                    KeyCode::Esc => {
                        self.popup = None;
                        return;
                    }
                    KeyCode::Up if n > 0 => {
                        *sel = if *sel == 0 { n - 1 } else { *sel - 1 };
                        return;
                    }
                    KeyCode::Down if n > 0 => {
                        *sel = if *sel + 1 >= n { 0 } else { *sel + 1 };
                        return;
                    }
                    KeyCode::Tab | KeyCode::Enter if n > 0 => {
                        let path = hits[(*sel).min(n - 1)].clone();
                        self.accept_mention(&path);
                        return;
                    }
                    _ => {}
                }
            }
            Some(Popup::Slash { sel }) => {
                let token = slash::parse(&self.input).map(|(t, _)| t).unwrap_or_default();
                let rows = slash::filter(&self.commands, &token);
                let n = rows.len();
                match k.code {
                    KeyCode::Esc => {
                        self.popup = None;
                        return;
                    }
                    KeyCode::Up | KeyCode::BackTab => {
                        *sel = if *sel == 0 { n.saturating_sub(1) } else { *sel - 1 };
                        return;
                    }
                    KeyCode::Down | KeyCode::Tab => {
                        *sel = if *sel + 1 >= n { 0 } else { *sel + 1 };
                        return;
                    }
                    KeyCode::Right if self.cursor == self.input.len() => {
                        if let Some(s) = rows.get(*sel) {
                            let t: Vec<char> = format!("/{} ", s.name).chars().collect();
                            self.set_input(&t);
                            self.popup = None;
                        }
                        return;
                    }
                    KeyCode::Enter => {
                        let name = rows.get(*sel).map(|s| s.name.clone()).unwrap_or(token);
                        push_history(&mut self.history, &format!("/{name}"));
                        self.clear_input();
                        self.popup = None;
                        self.run_slash(&name, "", cmd);
                        return;
                    }
                    _ => {}
                }
            }
            None => {}
        }

        match k.code {
            KeyCode::Enter if shift || alt => self.insert('\n'),
            KeyCode::Char('j') if ctrl => self.insert('\n'),
            KeyCode::Enter => self.submit(cmd),
            KeyCode::BackTab => self.cycle_mode(cmd),
            KeyCode::Esc => {
                if self.busy {
                    self.interrupt(interrupt);
                } else if !self.input.is_empty() {
                    self.clear_input();
                }
            }
            KeyCode::Backspace if ctrl || alt => {
                let s = self.word_left();
                self.input.drain(s..self.cursor);
                self.cursor = s;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.input.remove(self.cursor);
                }
            }
            KeyCode::Char('w') if ctrl => {
                let s = self.word_left();
                self.input.drain(s..self.cursor);
                self.cursor = s;
            }
            KeyCode::Char('u') if ctrl => {
                let start = self.input[..self.cursor].iter().rposition(|c| *c == '\n').map(|i| i + 1).unwrap_or(0);
                self.input.drain(start..self.cursor);
                self.cursor = start;
            }
            KeyCode::Char('k') if ctrl => {
                let end = self.input[self.cursor..].iter().position(|c| *c == '\n').map(|i| self.cursor + i).unwrap_or(self.input.len());
                self.input.drain(self.cursor..end);
            }
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.input.len(),
            KeyCode::Delete => {
                if self.cursor < self.input.len() {
                    self.input.remove(self.cursor);
                }
            }
            KeyCode::Left if ctrl || alt => self.cursor = self.word_left(),
            KeyCode::Right if ctrl || alt => self.cursor = self.word_right(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.input.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::Up => {
                let (_, crow, _) = view::wrapped_input(&self.input, self.cursor, self.width.saturating_sub(2));
                if crow == 0 && self.key_entry.is_none() {
                    self.history_prev();
                } else {
                    self.move_vertical(-1);
                }
            }
            KeyCode::Down => {
                let (lines, crow, _) = view::wrapped_input(&self.input, self.cursor, self.width.saturating_sub(2));
                if crow + 1 >= lines.len() && self.key_entry.is_none() {
                    self.history_next();
                } else {
                    self.move_vertical(1);
                }
            }
            KeyCode::Char('?') if self.input.is_empty() && self.key_entry.is_none() => self.popup = Some(Popup::Help),
            KeyCode::Char(c) if !ctrl => self.insert(c),
            _ => {}
        }
        self.refresh_popups();
    }

    fn move_vertical(&mut self, dir: isize) {
        let (lines, crow, ccol) = view::wrapped_input(&self.input, self.cursor, self.width.saturating_sub(2));
        let target = (crow as isize + dir).clamp(0, lines.len() as isize - 1) as usize;
        let (start, text) = &lines[target];
        self.cursor = start + ccol.min(text.chars().count());
    }
}

/// Copy to the system clipboard via OSC 52 (Windows Terminal, iTerm2,
/// kitty, WezTerm, tmux…) — no helper process, works over SSH.
fn copy_osc52(text: &str) {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]52;c;{b64}\x07");
    let _ = out.flush();
}

// ---- persistence --------------------------------------------------------------

fn history_path() -> PathBuf {
    zcore::home_file("history.jsonl", "zcli_history.jsonl")
}

fn load_history() -> Vec<String> {
    std::fs::read_to_string(history_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<String>(l).ok())
        .collect()
}

fn push_history(h: &mut Vec<String>, entry: &str) {
    if h.last().map(String::as_str) == Some(entry) {
        return;
    }
    h.push(entry.to_string());
    if h.len() > 1000 {
        h.drain(..h.len() - 1000);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(history_path()) {
        let _ = writeln!(f, "{}", serde_json::to_string(entry).unwrap_or_default());
    }
}

fn state_path() -> PathBuf {
    zcore::home_file("state.json", "zcli_state.json")
}

/// The model picked last time — used when cosmostui starts without `--model`.
fn saved_model() -> Option<String> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(state_path()).ok()?).ok()?;
    v["model"].as_str().map(str::to_string)
}

fn save_state_model(alias: &str) {
    let _ = std::fs::write(state_path(), serde_json::json!({"model": alias}).to_string());
}

// ---- entry ----------------------------------------------------------------------

/// Run the TUI. `Ok(None)` = no usable terminal → caller falls back to the
/// line REPL (pipes, tests).
pub fn try_run(opts: &RunOpts, cwd: &Path) -> Result<Option<i32>> {
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return Ok(None);
    }
    // first launch: the one-shot `cosmostui-welcome` binary (galaxy intro → name
    // → keys → how-to) runs before anything else so the keys and default
    // model it writes land in this session. It's deleted once it completes.
    if zcore::welcome::needed() {
        if let zcore::welcome::Outcome::Quit = zcore::welcome::launch() {
            return Ok(Some(zproto::exit::DONE));
        }
    } else {
        zcore::welcome::retire();
    }
    let mut screen = match Screen::enter() {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };
    let models = model_rows();
    // start on the last model used, if it still resolves
    let mut opts = opts.clone();
    if opts.model.is_none() {
        if let Some(m) = saved_model() {
            let cfg = zcore::config::Config::load(&zcore::cli_home());
            if cfg.provider_for(&m).is_some() {
                opts.model = Some(m);
            }
        }
    }
    let interrupt = Arc::new(AtomicBool::new(false));
    let (cmd, rx) = worker::spawn(opts.clone(), cwd.to_path_buf(), interrupt.clone());
    let mut app = App::new(cwd, models, screen.text_width());
    if let Some(d) = &opts.cwd {
        app.cwd = PathBuf::from(d);
    }
    app.user = zcore::welcome::user_name();
    app.cont(crate::logo::banner("cosmos", concat!("v", env!("CARGO_PKG_VERSION"))));

    let r = run_loop(&mut screen, &mut app, &rx, &cmd, &interrupt);
    let bye = view::farewell(&app);
    screen.leave(&bye);
    // contract §13: after a clean interactive session, learn from it in the
    // background (skills + memory only; the operator can turn it off)
    if matches!(r, Ok(0)) && !app.session.is_empty() && zcore::config::Config::load(&zcore::cli_home()).self_improve.enabled {
        zcore::improve::spawn_review(&app.session);
    }
    r.map(Some)
}

/// The character a key press types, if it's plain text (Enter is a
/// newline inside a paste). AltGr arrives as Ctrl+Alt and still types.
fn typed_char(k: &KeyEvent) -> Option<char> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let alt = k.modifiers.contains(KeyModifiers::ALT);
    match k.code {
        KeyCode::Char(c) if !ctrl || alt => Some(c),
        KeyCode::Enter if !ctrl && !alt => Some('\n'),
        KeyCode::Tab if k.modifiers.is_empty() => Some('\t'),
        _ => None,
    }
}

/// Starting from `first`, gather keys that arrive back to back (`next`
/// yields the next event only if one is already waiting). A burst of more
/// than one typed character is a paste → `Some(text)`; otherwise `None` and
/// `first` is handled as a normal key. Events that aren't text end the burst
/// and come back in the second slot, in order.
fn collect_burst(
    first: KeyEvent,
    next: &mut dyn FnMut() -> std::io::Result<Option<CEvent>>,
) -> std::io::Result<(Option<String>, Vec<CEvent>)> {
    let Some(c0) = typed_char(&first) else { return Ok((None, Vec::new())) };
    let mut text = String::from(c0);
    let mut rest = Vec::new();
    while let Some(ev) = next()? {
        match ev {
            CEvent::Key(k) if k.kind == KeyEventKind::Release => rest.push(CEvent::Key(k)),
            CEvent::Key(k) => match typed_char(&k) {
                Some(c) => text.push(c),
                _ => {
                    rest.push(CEvent::Key(k));
                    break;
                }
            },
            other => {
                rest.push(other);
                break;
            }
        }
    }
    if text.chars().count() > 1 {
        // key releases inside a paste carry nothing
        rest.retain(|e| !matches!(e, CEvent::Key(k) if k.kind == KeyEventKind::Release));
        Ok((Some(text), rest))
    } else {
        Ok((None, rest))
    }
}

fn run_loop(screen: &mut Screen, app: &mut App, rx: &Receiver<Msg>, cmd: &Sender<Cmd>, interrupt: &Arc<AtomicBool>) -> Result<i32> {
    let mut last_tick = Instant::now();
    loop {
        while let Ok(m) = rx.try_recv() {
            app.on_msg(m);
        }
        app.poll_jobs();
        if app.replay_onboard {
            app.replay_onboard = false;
            if zcore::welcome::exe().is_none() {
                app.note("onboarding has retired — /connect adds keys, /login signs in, /model picks a default");
            } else {
                // hand the terminal to cosmostui-welcome, then take it back
                let out = std::mem::take(&mut app.out);
                screen.draw(&out, Frame { lines: &[], cursor: None })?;
                screen.leave(&[]);
                let outcome = zcore::welcome::launch();
                *screen = Screen::enter()?;
                match outcome {
                    zcore::welcome::Outcome::Quit => app.request_quit(cmd),
                    zcore::welcome::Outcome::Done => {
                        app.models = model_rows();
                        app.user = zcore::welcome::user_name();
                        let cfg = zcore::config::Config::load(&zcore::cli_home());
                        if let Some(m) = cfg.default_model.filter(|m| *m != app.alias) {
                            let _ = cmd.send(Cmd::SetModel(m));
                        }
                    }
                    zcore::welcome::Outcome::Missing => {}
                    zcore::welcome::Outcome::Failed(e) => app.error(e),
                }
            }
            app.dirty = true;
        }
        if app.done {
            break;
        }
        if app.quitting && app.quit_deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        if app.notice.as_ref().is_some_and(|(_, _, t)| t.elapsed() > Duration::from_secs(5)) {
            app.notice = None;
            app.dirty = true;
        }
        // animate spinners ~12 fps while something is moving
        let animating = app.busy || !app.tools.is_empty() || app.rec.is_some() || app.shell.is_some() || app.login.is_some();
        if animating && last_tick.elapsed() >= Duration::from_millis(80) {
            last_tick = Instant::now();
            app.dirty = true;
        }
        // the @ index finishes in the background; repaint once it lands
        if matches!(app.popup, Some(Popup::Files { .. })) && app.files.ready() {
            app.dirty = true;
        }
        if app.clear_screen {
            app.clear_screen = false;
            screen.clear_screen()?;
            app.dirty = true;
        }
        if app.dirty {
            app.dirty = false;
            let (lines, cursor) = view::live(app);
            let out = std::mem::take(&mut app.out);
            screen.draw(&out, Frame { lines: &lines, cursor })?;
        }
        let wait = if animating { Duration::from_millis(16) } else { Duration::from_millis(120) };
        if event::poll(wait)? {
            // drain everything queued so pastes and fast typing coalesce
            loop {
                match event::read()? {
                    CEvent::Key(k) if k.kind == KeyEventKind::Release => app.on_release(k),
                    CEvent::Key(k) => {
                        // Windows consoles deliver a paste as a stream of key
                        // presses (no bracketed paste there), so each Enter in it
                        // would send a line. Keys arriving back to back faster
                        // than hands can type are a paste: take them as one.
                        let mut next = || -> std::io::Result<Option<CEvent>> {
                            Ok(if event::poll(Duration::from_millis(4))? { Some(event::read()?) } else { None })
                        };
                        let (pasted, rest) = collect_burst(k, &mut next)?;
                        match pasted {
                            Some(s) => app.paste_text(&s),
                            None => app.on_key(k, cmd, interrupt),
                        }
                        for ev in rest {
                            match ev {
                                CEvent::Key(k) if k.kind == KeyEventKind::Release => app.on_release(k),
                                CEvent::Key(k) => app.on_key(k, cmd, interrupt),
                                CEvent::Paste(s) => app.paste_text(&s),
                                CEvent::Resize(w, h) => {
                                    screen.resized(w, h);
                                    app.width = screen.text_width();
                                }
                                _ => {}
                            }
                            app.dirty = true;
                        }
                    }
                    CEvent::Paste(s) if !app.quitting => {
                        app.paste_text(&s);
                        app.dirty = true;
                    }
                    CEvent::Resize(w, h) => {
                        screen.resized(w, h);
                        app.width = screen.text_width();
                        app.dirty = true;
                    }
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
    }
    Ok(zproto::exit::DONE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn app() -> App {
        std::env::set_var("COSMOS_CLI_HOME", std::env::temp_dir().join("cosmos-tui-test"));
        std::env::set_var("COSMOS_CLI_KEYRING", "0");
        let _ = std::fs::create_dir_all(std::env::temp_dir().join("cosmos-tui-test"));
        let row = |alias: &str, provider: &str, id: &str, context: u64, effort_ok: bool| ModelRow {
            alias: alias.into(),
            provider: provider.into(),
            id: id.into(),
            name: alias.into(),
            context,
            price: zproto::Prices::default(),
            effort_ok,
        };
        let mut a = App::new(
            Path::new("."),
            vec![row("fixture", "fixture", "fixture", 32_768, false), row("r9", "router9", "kr/claude-sonnet-4.5", 200_000, true)],
            78,
        );
        a.alias = "r9".into();
        a.model = "r9 (kr/claude-sonnet-4.5)".into();
        a.session = "01k5zq8m3v".into();
        a
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn text(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn streaming_commits_complete_lines_only() {
        let _env = crate::env_lock();
        let mut a = app();
        a.on_msg(Msg::Delta("Hello **wor".into()));
        assert!(a.out.is_empty(), "partial line must stay live");
        a.on_msg(Msg::Delta("ld**\n- one\n- tw".into()));
        let rows: Vec<String> = a.out.iter().map(text).collect();
        assert!(rows.contains(&" Hello world".to_string()));
        assert!(rows.contains(&"   • one".to_string()));
        assert!(!rows.iter().any(|r| r.contains("tw")));
        a.on_msg(Msg::TurnDone);
        let rows: Vec<String> = a.out.iter().map(text).collect();
        assert!(rows.contains(&"   • tw".to_string()));
        assert_eq!(a.last_reply, "Hello **world**\n- one\n- tw");
    }

    #[test]
    fn slash_menu_opens_and_runs() {
        let _env = crate::env_lock();
        let mut a = app();
        let (tx, _rx) = channel();
        let flag = Arc::new(AtomicBool::new(false));
        a.on_key(key(KeyCode::Char('/')), &tx, &flag);
        assert!(matches!(a.popup, Some(Popup::Slash { .. })));
        for c in "ctx".chars() {
            a.on_key(key(KeyCode::Char(c)), &tx, &flag);
        }
        a.on_key(key(KeyCode::Enter), &tx, &flag);
        assert!(a.popup.is_none());
        let rows: Vec<String> = a.out.iter().map(text).collect();
        assert!(rows.iter().any(|r| r.contains("Context")), "{rows:?}");
    }

    #[test]
    fn model_picker_sets_effort_and_model() {
        let _env = crate::env_lock();
        let mut a = app();
        let (tx, rx) = channel();
        let flag = Arc::new(AtomicBool::new(false));
        a.open_model_picker();
        a.on_key(key(KeyCode::Right), &tx, &flag);
        a.on_key(key(KeyCode::Right), &tx, &flag);
        a.on_key(key(KeyCode::Up), &tx, &flag); // r9 → fixture
        a.on_key(key(KeyCode::Enter), &tx, &flag);
        let sent: Vec<Cmd> = rx.try_iter().collect();
        assert!(matches!(&sent[0], Cmd::SetEffort(Some(e)) if e == "medium"));
        assert!(matches!(&sent[1], Cmd::SetModel(m) if m == "fixture"));
    }

    #[test]
    fn big_paste_collapses_and_expands() {
        let _env = crate::env_lock();
        let mut a = app();
        let body = (0..20).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        a.paste(&body);
        let shown: String = a.input.iter().collect();
        assert_eq!(shown, "[Pasted text #1 +20 lines]");
        assert_eq!(a.expand_pastes(&shown), body);
    }

    /// A Windows console paste: press/release per character, Enter for newlines.
    fn windows_paste(text: &str) -> Vec<CEvent> {
        let mut v = Vec::new();
        for c in text.chars() {
            let code = if c == '\n' { KeyCode::Enter } else { KeyCode::Char(c) };
            v.push(CEvent::Key(KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Press)));
            v.push(CEvent::Key(KeyEvent::new_with_kind(code, KeyModifiers::NONE, KeyEventKind::Release)));
        }
        v
    }

    fn burst(events: Vec<CEvent>) -> (Option<String>, Vec<CEvent>) {
        let mut it = events.into_iter();
        let first = match it.next() {
            Some(CEvent::Key(k)) => k,
            _ => panic!("needs a key first"),
        };
        let mut next = || -> std::io::Result<Option<CEvent>> { Ok(it.next()) };
        collect_burst(first, &mut next).unwrap()
    }

    #[test]
    fn a_multi_line_paste_arrives_as_one_text_not_as_enters() {
        let (text, rest) = burst(windows_paste("fn main() {\n    println!(\"hi\");\n}\n"));
        assert_eq!(text.as_deref(), Some("fn main() {\n    println!(\"hi\");\n}\n"));
        assert!(rest.is_empty(), "releases inside the paste are dropped");
    }

    #[test]
    fn a_single_typed_key_and_enter_stay_keys() {
        let (text, rest) = burst(windows_paste("a"));
        assert_eq!(text, None);
        assert_eq!(rest.len(), 1); // its release, handled normally
        let (text, _) = burst(vec![CEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))]);
        assert_eq!(text, None, "a lone Enter still sends");
    }

    #[test]
    fn a_burst_stops_at_a_non_text_key_and_hands_it_back() {
        let mut ev = windows_paste("ab");
        ev.push(CEvent::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)));
        ev.extend(windows_paste("cd"));
        let (text, rest) = burst(ev);
        assert_eq!(text.as_deref(), Some("ab"));
        assert!(matches!(rest.first(), Some(CEvent::Key(k)) if k.code == KeyCode::Up));
        // ctrl+c is a command, not text; AltGr (ctrl+alt) still types
        assert_eq!(typed_char(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), None);
        assert_eq!(typed_char(&KeyEvent::new(KeyCode::Char('@'), KeyModifiers::CONTROL | KeyModifiers::ALT)), Some('@'));
    }

    #[test]
    fn pasted_lines_land_in_the_composer_unsent() {
        let _env = crate::env_lock();
        let mut a = app();
        a.paste_text("first line\nsecond line");
        let shown: String = a.input.iter().collect();
        assert_eq!(shown, "first line\nsecond line");
    }

    #[test]
    fn a_held_ctrl_space_only_toggles_once() {
        let _env = crate::env_lock();
        let mut a = app();
        assert!(!a.toggle_repeat(), "first press toggles");
        if cfg!(windows) {
            // auto-repeat while held — ignored however long it runs
            assert!(a.toggle_repeat());
            assert!(a.toggle_repeat());
            a.on_release(KeyEvent::new_with_kind(KeyCode::Char(' '), KeyModifiers::NONE, KeyEventKind::Release));
            assert!(!a.toggle_repeat(), "a new press after release toggles again");
        }
    }

    #[test]
    fn tool_result_waits_for_its_call() {
        let _env = crate::env_lock();
        let mut a = app();
        let ev = |kind: &str, data: Value| zproto::Event {
            v: 1, id: "e".into(), parent: None, ts: String::new(), session: "s".into(),
            harness: "cosmostui".into(), kind: kind.into(), data,
        };
        a.on_msg(Msg::Event(ev("assistant", serde_json::json!({"text": "", "tool_calls": [{"id": "t1", "name": "bash", "input": {"command": "cargo test"}}]}))));
        assert_eq!(a.tools.len(), 1);
        assert!(a.out.is_empty(), "running tool stays in the live region");
        a.on_msg(Msg::Event(ev("tool_result", serde_json::json!({"tool_use_id": "t1", "name": "bash", "content": "ok. 3 passed", "is_error": false}))));
        assert!(a.tools.is_empty());
        let rows: Vec<String> = a.out.iter().map(text).collect();
        assert!(rows.iter().any(|r| r.contains("Bash") && r.contains("cargo test")), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("ok. 3 passed")));

        // a deferred tool via the built-in server renders as itself
        a.on_msg(Msg::Event(ev("assistant", serde_json::json!({"text": "", "tool_calls": [{"id": "t2", "name": "mcp_call",
            "input": {"server": "cosmos", "tool": "memory_search", "arguments": {"query": "tabs"}}}]}))));
        assert_eq!(a.tools[0].name, "memory_search");
        a.on_msg(Msg::Event(ev("tool_result", serde_json::json!({"tool_use_id": "t2", "name": "memory_search", "content": "{\"results\":[]}", "is_error": false}))));
        let rows: Vec<String> = a.out.iter().map(text).collect();
        assert!(rows.iter().any(|r| r.contains("Recall") && r.contains("tabs")), "{rows:?}");

        // a subscription agent's tool shows up from its tool_call event
        a.on_msg(Msg::Event(ev("tool_call", serde_json::json!({"id": "x", "name": "devin:edit", "title": "Edit a.rs",
            "status": "completed", "output": "wrote a.rs", "source": "acp"}))));
        let rows: Vec<String> = a.out.iter().map(text).collect();
        assert!(rows.iter().any(|r| r.contains("devin:edit") && r.contains("Edit a.rs")), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("wrote a.rs")), "{rows:?}");
    }

    #[test]
    fn status_line_shows_the_cost_ledger() {
        let _env = crate::env_lock();
        let mut a = app();
        let ev = |data: Value| zproto::Event {
            v: 1, id: "e".into(), parent: None, ts: String::new(), session: "s".into(),
            harness: "cosmostui".into(), kind: "usage".into(), data,
        };
        a.on_msg(Msg::Event(ev(serde_json::json!({"input": 1204, "cache_read": 18400, "cache_write": 0, "output": 312, "usd": 0.0041, "cum_usd": 0.0213}))));
        // narrow terminals get the short form
        let (lines, _) = view::live(&a);
        assert!(lines.iter().map(text).any(|r| r.contains("Σ $0.0213 · ctx") && !r.contains("turn 1")));
        a.width = 160;
        let (lines, _) = view::live(&a);
        let rows: Vec<String> = lines.iter().map(text).collect();
        let status = rows.iter().find(|r| r.contains("ctx")).unwrap_or_else(|| panic!("{rows:?}"));
        assert!(status.contains("turn 1 · in 1.2k · cache_r 18k · cache_w 0 · out 312 · $0.0041 · Σ $0.0213 · ctx"), "{status}");
        // a subscription turn has no per-token price: "plan", never a guess
        a.on_msg(Msg::Event(ev(serde_json::json!({"input": 0, "cache_read": 0, "cache_write": 0, "output": 0, "usd": null, "billing": "subscription"}))));
        let (lines, _) = view::live(&a);
        let rows: Vec<String> = lines.iter().map(text).collect();
        assert!(rows.iter().any(|r| r.contains("turn 2") && r.contains("· plan ·")), "{rows:?}");
    }

    #[test]
    fn harness_commands_never_reach_the_model() {
        let _env = crate::env_lock();
        let mut a = app();
        let (tx, rx) = channel();
        a.run_slash("compact", "focus: the parser", &tx);
        a.run_slash("undo", "", &tx);
        a.run_slash("skillify", "release steps", &tx);
        let sent: Vec<Cmd> = rx.try_iter().collect();
        assert!(matches!(&sent[0], Cmd::Local(LocalCmd::Compact(Some(f))) if f == "the parser"), "{sent:?}");
        assert!(matches!(&sent[1], Cmd::Local(LocalCmd::Undo)));
        assert!(matches!(&sent[2], Cmd::Local(LocalCmd::Skillify(n)) if n == "release steps"));
        assert!(!sent.iter().any(|c| matches!(c, Cmd::Prompt(_))), "no prompt sent to the model");
    }

    #[test]
    fn connect_stores_a_masked_key() {
        let _env = crate::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let mut a = app();
        std::env::set_var("COSMOS_CLI_HOME", dir.path());
        let (tx, _rx) = channel();
        let flag = Arc::new(AtomicBool::new(false));
        a.run_slash("connect", "xiaomi", &tx);
        assert!(a.key_entry.is_some());
        a.paste("sk-mimo-0123456789");
        a.on_key(key(KeyCode::Enter), &tx, &flag);
        assert!(a.key_entry.is_none());
        assert_eq!(zcore::creds::stored_key("xiaomi").unwrap().0, "sk-mimo-0123456789");
        let rows: Vec<String> = a.out.iter().map(text).collect();
        assert!(rows.iter().any(|r| r.contains("sk-m…6789")), "{rows:?}");
        assert!(!rows.iter().any(|r| r.contains("sk-mimo-0123456789")), "key must never be printed");
        assert!(a.models.iter().any(|m| m.alias.starts_with("xiaomi/")));
    }

    #[test]
    fn bang_runs_shell_and_feeds_next_prompt() {
        let _env = crate::env_lock();
        let mut a = app();
        let (tx, rx) = channel();
        let flag = Arc::new(AtomicBool::new(false));
        for c in "!echo cosmos-shell-ok".chars() {
            a.on_key(key(KeyCode::Char(c)), &tx, &flag);
        }
        a.on_key(key(KeyCode::Enter), &tx, &flag);
        for _ in 0..200 {
            a.poll_jobs();
            if a.shell.is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let rows: Vec<String> = a.out.iter().map(text).collect();
        assert!(rows.iter().any(|r| r.contains("cosmos-shell-ok")), "{rows:?}");
        for c in "what happened?".chars() {
            a.on_key(key(KeyCode::Char(c)), &tx, &flag);
        }
        a.on_key(key(KeyCode::Enter), &tx, &flag);
        let sent: Vec<Cmd> = rx.try_iter().collect();
        assert!(matches!(&sent[0], Cmd::Prompt(p) if p.contains("<shell-output") && p.contains("cosmos-shell-ok") && p.ends_with("what happened?")));
    }
}
