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
use ratatui_core::style::{Color, Style};
use ratatui_core::text::{Line, Span};
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
    /// ctrl+o: a kept tool output — `idx` 0 is the newest.
    Pager { idx: usize, scroll: usize },
    /// `/customise`: quick settings, each changed with ←/→
    Customise { sel: usize },
}

/// A tool output kept for the ctrl+o pager.
pub struct Output {
    pub title: String,
    pub body: Vec<String>,
    /// the diff, drawn in colour when the pager shows it
    pub diff: Option<Diff>,
}

/// `/customise reset classic`: the look cosmostui had before skins.
const CLASSIC_UI: &str = r#"# the look cosmostui had before skins
[theme]
name = "classic"

[tools]
style = "classic"

[diff]
style = "classic"

[markdown]
style = "classic"

[glyphs]
tool = "●"

[buddy]
enabled = false
"#;

/// Tool outputs kept for ctrl+o, and the most lines kept from each.
const KEEP_OUTPUTS: usize = 50;
const KEEP_OUTPUT_LINES: usize = 5000;

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
    pub started: Instant,
}

/// The assistant reply currently streaming in.
pub struct Stream {
    pub md: Md,
    pub partial: String,
    started: bool,
    pub chars: usize,
    /// full text, for /copy
    all: String,
    /// (when, total chars by then) — recent deltas, for the fade-in
    pub arrivals: std::collections::VecDeque<(Instant, usize)>,
}

impl Stream {
    /// Characters that arrived within the last `ms`.
    pub fn fresh(&self, ms: u64) -> usize {
        let cutoff = Instant::now() - Duration::from_millis(ms);
        let before = self.arrivals.iter().rev().find(|(t, _)| *t < cutoff).map(|(_, n)| *n);
        match before {
            Some(n) => self.chars - n,
            None => self.chars - self.arrivals.front().map(|(_, n)| *n).unwrap_or(self.chars),
        }
    }
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
    /// picks this turn's thinking verb (fixed when the turn starts)
    pub verb_seed: u64,
    /// characters streamed this turn (footer fallback when usage is unknown)
    pub turn_chars: usize,
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
    /// finished reads/searches waiting to be committed as one group
    pub explore: Vec<crate::toolview::Explored>,
    /// recent tool outputs, oldest first (ctrl+o)
    pub outputs: std::collections::VecDeque<Output>,
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
    /// a subscription to install and/or sign in to with the terminal handed over
    pub handoff: Option<String>,
    /// `ui.toml` was just written by a command — reload it now, not in a second
    pub skin_reload: bool,
    /// the companion beside the composer
    pub buddy: crate::buddy::Buddy,
    /// terminal focus, once the terminal has reported it
    pub focused: Option<bool>,
    /// title / bell escapes to write between frames
    pub term_out: Vec<u8>,
    /// we've set the terminal title (reset it on exit)
    pub titled: bool,
    /// an error arrived during this turn
    turn_err: Option<String>,
    /// /pettalk waiting for the pet's answer
    pet_talk: Option<crate::pettalk::Talk>,
    /// the chat with the pet so far (it remembers)
    pet_chat: Vec<zproto::Message>,
    /// what pet chats have cost this session
    pub pet_usd: f64,
    /// (when, what) reminders the pet keeps
    pub reminders: Vec<(Instant, String)>,
    /// (left at, turns then, fixes then) while the terminal is unfocused
    away: Option<(Instant, u64, u64)>,
    /// `/customise <request>` running in the background
    pub customise: Option<crate::customise::Runner>,
    /// what the customiser is doing (status strip)
    pub mod_status: String,
    /// its steps, for `/customise log`
    mod_log: Vec<String>,
    /// what customising has cost this session
    pub mod_usd: f64,
    /// a draft waits for ctrl+y / ctrl+n
    pub draft_ready: bool,
    /// ctrl+p: the live UI drawn in the draft's look before applying it
    pub preview_skin: Option<crate::skin::Skin>,
    /// messages typed while a task runs: (as typed, as sent)
    pub queue: Vec<(String, String)>,
    /// the queued message being managed (↑ from an empty composer)
    pub queue_sel: Option<usize>,
    /// where an edited queued message goes back
    queue_edit: Option<usize>,
    /// "send now": handed to the worker, which slips it into the running task
    pub steer: crate::worker::Steer,
    /// subscription-agent tools already drawn live this turn
    acp_done: std::collections::HashSet<String>,
    /// the reply being written streamed already (so it isn't drawn twice)
    turn_streamed: bool,
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
            verb_seed: 0,
            turn_chars: 0,
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
            explore: Vec::new(),
            outputs: std::collections::VecDeque::new(),
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
            handoff: None,
            skin_reload: false,
            buddy: crate::buddy::Buddy::new(),
            focused: None,
            term_out: Vec::new(),
            titled: false,
            turn_err: None,
            pet_talk: None,
            pet_chat: Vec::new(),
            pet_usd: 0.0,
            reminders: Vec::new(),
            away: None,
            customise: None,
            mod_status: String::new(),
            mod_log: Vec::new(),
            mod_usd: 0.0,
            draft_ready: crate::customise::tidy(&crate::customise::Paths::new(&zcore::cli_home())),
            preview_skin: None,
            queue: Vec::new(),
            queue_sel: None,
            queue_edit: None,
            steer: crate::worker::Steer::default(),
            acp_done: std::collections::HashSet::new(),
            turn_streamed: false,
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

    /// Commit any reads/searches still waiting to be grouped.
    fn flush_explore(&mut self) {
        if self.explore.is_empty() {
            return;
        }
        let items = std::mem::take(&mut self.explore);
        self.block(crate::toolview::group(&items, self.width));
    }

    /// Keep a tool's full output for the ctrl+o pager.
    fn keep_output_styled(&mut self, title: String, mut body: Vec<String>, diff: Option<Diff>) {
        if body.len() > KEEP_OUTPUT_LINES {
            let more = body.len() - KEEP_OUTPUT_LINES;
            body.truncate(KEEP_OUTPUT_LINES);
            body.push(format!("… {more} more lines not kept"));
        }
        self.outputs.push_back(Output { title, body, diff });
        while self.outputs.len() > KEEP_OUTPUTS {
            self.outputs.pop_front();
        }
    }

    /// A finished tool call: kept for ctrl+o, then grouped or committed.
    fn finish_tool(&mut self, v: crate::toolview::ToolView) {
        let w = self.width;
        let title = match (crate::toolview::vendor(v.name), v.input["title"].as_str()) {
            (Some((vendor, _)), Some(t)) => format!("{t} ({vendor})"),
            _ => format!("{} {}", view::tool_label(v.name), view::arg_summary(v.name, v.input, 200)).trim().to_string(),
        };
        let body: Vec<String> = match v.diff {
            Some(d) => crate::toolview::diff_text(d),
            None => crate::toolview::body_of(v.name, v.content.unwrap_or("")).lines().map(str::to_string).collect(),
        };
        self.keep_output_styled(title, body, v.diff.cloned());
        let sk = crate::skin::current();
        if !v.interrupted && v.diff.is_none() && crate::toolview::groups(&sk, v.name, v.err) {
            self.explore.push(crate::toolview::Explored {
                name: v.name.to_string(),
                input: v.input.clone(),
                content: v.content.unwrap_or("").to_string(),
                elapsed: v.elapsed,
            });
            self.dirty = true;
            return;
        }
        self.block(crate::toolview::render(&v, w));
    }

    /// Commit a block, separated from the previous one by one blank row.
    pub fn block(&mut self, lines: Vec<Line<'static>>) {
        self.flush_explore();
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
        self.flush_explore();
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

    /// Tell the buddy something happened.
    fn buddy_on(&mut self, m: crate::buddy::Moment) {
        let sk = crate::skin::current();
        if !sk.buddy.enabled {
            return;
        }
        let name = view::buddy_identity(self).name;
        self.buddy.on(m, &sk.buddy, &name);
        self.pet_progress();
        self.dirty = true;
    }

    /// Save the pet's progress and announce a level-up.
    fn pet_progress(&mut self) {
        if std::mem::take(&mut self.buddy.progress_dirty) && !cfg!(test) {
            self.buddy.progress.save(&zcore::cli_home());
        }
        if let Some(level) = self.buddy.leveled.take() {
            let sk = crate::skin::current();
            let id = view::buddy_identity(self);
            self.buddy.say_kind("level", &sk.buddy, &id.name);
            self.buddy.trick(crate::buddy::Trick::Sparkle);
            let unlock = crate::buddy::UNLOCKS.iter().find(|(at, _)| *at == level).map(|(_, what)| format!(" — unlocked {what}")).unwrap_or_default();
            self.flash(format!("✦ {} reached level {level}{unlock}", id.name), theme::ACCENT());
        }
    }

    /// Reminders, break nudges and the pet's chat (once per loop turn).
    fn poll_pet(&mut self) {
        let sk = crate::skin::current();
        if !sk.buddy.enabled {
            return;
        }
        let now = Instant::now();
        let due: Vec<String> = self.reminders.iter().filter(|(t, _)| *t <= now).map(|(_, s)| s.clone()).collect();
        if !due.is_empty() {
            self.reminders.retain(|(t, _)| *t > now);
            let name = view::buddy_identity(self).name;
            for d in due {
                self.buddy.speak(format!("⏰ {d}"), 30);
                self.buddy.trick(crate::buddy::Trick::Hop);
                self.note(format!("⏰ {name}: {d}"));
                let a = crate::notify::Alert {
                    term: [crate::notify::title(&format!("⏰ {d} · cosmostui")), vec![0x07]].concat(),
                    desktop: Some((format!("{name} reminds you"), d.clone())),
                    chime: sk.notify.sound,
                };
                self.term_out.extend(&a.term);
                self.titled = true;
                if !cfg!(test) {
                    crate::notify::fire(&a);
                }
            }
            self.dirty = true;
        }
        if self.buddy.break_due(sk.buddy.break_mins) {
            let name = view::buddy_identity(self).name;
            self.buddy.say_kind("break", &sk.buddy, &name);
            self.dirty = true;
        }
        self.poll_pet_talk();
    }

    /// `/pettalk <message>`
    fn pet_talk_command(&mut self, msg: &str) {
        let sk = crate::skin::current();
        let id = view::buddy_identity(self);
        if !sk.buddy.enabled {
            self.note("Your pet is off — /buddy on brings it back");
            return;
        }
        if msg == "stop" {
            if let Some(t) = self.pet_talk.take() {
                t.stop();
                self.buddy.listening = false;
                self.flash(format!("{} stopped thinking", id.name), theme::YELLOW());
            }
            return;
        }
        if msg.is_empty() {
            self.note(format!(
                "Talk to {}: /pettalk how's it going? · remind me in 20 minutes to stretch · what changed in the repo? · do a trick · let's change your colours",
                id.name
            ));
            return;
        }
        if self.pet_talk.is_some() {
            self.error(format!("{} is still thinking…", id.name));
            return;
        }
        let ctx = crate::pettalk::Context { pet: self.pet_profile(&id), session: self.pet_session(), cwd: self.cwd.clone() };
        self.pet_chat.push(crate::pettalk::user(msg));
        crate::pettalk::trim(&mut self.pet_chat, 12);
        self.block(view::pet_heard(self, msg, self.width));
        self.buddy.listening = true;
        self.pet_talk = Some(crate::pettalk::ask(self.alias.clone(), zcore::cli_home(), ctx, self.pet_chat.clone()));
        self.dirty = true;
    }

    /// Who the pet is, for its prompt.
    fn pet_profile(&self, id: &crate::buddy::Identity) -> String {
        let p = &self.buddy.progress;
        format!(
            "You are {}, a {} {} (personality: {}). Level {} — {} turns worked together, {} red tests turned green, {} chats, a {}-day streak.",
            id.name,
            id.rarity.label(),
            id.species().name,
            id.personality(),
            p.level(),
            p.turns,
            p.fixes,
            p.chats,
            p.streak
        )
    }

    /// What's happening in the session, for the pet's prompt.
    fn pet_session(&self) -> String {
        let mut s = vec![
            format!("- model: {} · mode: {}", self.alias, self.mode.name()),
            format!("- folder: {}", self.cwd.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()),
            format!("- session: {} min, {} turns, spent {}", self.session_started.elapsed().as_secs() / 60, self.usage.turns, view::ledger(self).1),
        ];
        if self.busy {
            s.push(format!("- the assistant is working right now ({}s in)", self.busy_since.elapsed().as_secs()));
        }
        let recent: Vec<String> = self.history.iter().rev().take(3).map(|h| format!("\"{}\"", crate::wrap::ellipsize(h, 120))).collect();
        if !recent.is_empty() {
            s.push(format!("- their latest requests (newest first): {}", recent.join(", ")));
        }
        if !self.last_reply.is_empty() {
            s.push(format!("- the assistant's last reply began: \"{}\"", crate::wrap::ellipsize(&self.last_reply.replace('\n', " "), 300)));
        }
        if !self.reminders.is_empty() {
            let r: Vec<String> = self.reminders.iter().map(|(t, w)| format!("\"{w}\" in {} min", t.saturating_duration_since(Instant::now()).as_secs().div_ceil(60))).collect();
            s.push(format!("- reminders you're keeping: {}", r.join(", ")));
        }
        s.join("
")
    }

    fn poll_pet_talk(&mut self) {
        let Some(t) = &self.pet_talk else { return };
        if t.started.elapsed() > Duration::from_secs(180) {
            t.stop();
            self.pet_talk = None;
            self.buddy.listening = false;
            self.error(format!("{} took too long to answer — try again", view::buddy_identity(self).name));
            return;
        }
        let Ok(reply) = t.rx.try_recv() else { return };
        self.pet_talk = None;
        self.buddy.listening = false;
        self.dirty = true;
        match reply {
            crate::pettalk::Reply::Failed(e) => self.error(format!("{} couldn't answer: {e}", view::buddy_identity(self).name)),
            crate::pettalk::Reply::Text { text, actions, usd } => {
                self.pet_usd += usd.unwrap_or(0.0);
                let shown = if text.is_empty() { "…".to_string() } else { text.clone() };
                self.pet_chat.push(crate::pettalk::assistant(&text));
                self.block(view::pet_reply(self, &shown, self.width));
                let first = shown.split(['.', '!', '?', '\n']).next().unwrap_or("").trim().to_string();
                if !first.is_empty() {
                    self.buddy.speak(crate::wrap::ellipsize(&first, 60), 8);
                }
                self.buddy.chatted();
                for a in actions {
                    self.pet_act(a);
                }
                self.pet_progress();
            }
        }
    }

    /// Do what the pet decided to do.
    fn pet_act(&mut self, a: crate::pettalk::Action) {
        use crate::pettalk::Action;
        use crate::skin::edit::{set_in_file, Val};
        let name = view::buddy_identity(self).name;
        let ui = zcore::cli_home().join("ui.toml");
        match a {
            Action::Remind { mins, text } => {
                self.reminders.push((Instant::now() + Duration::from_secs(mins * 60), text.clone()));
                self.note(format!("⏰ {name} will remind you in {mins} min: {text}"));
            }
            Action::Trick(t) => self.buddy.trick(t),
            Action::Rename(n) => match set_in_file(&ui, "buddy", "name", &Val::Str(n.clone())) {
                Ok(()) => {
                    self.skin_reload = true;
                    self.note(format!("✎ {name} is now called {n}"));
                }
                Err(e) => self.error(e),
            },
            Action::Species(s) => match set_in_file(&ui, "buddy", "species", &Val::Str(s.clone())) {
                Ok(()) => {
                    self.skin_reload = true;
                    self.buddy.trick(crate::buddy::Trick::Spin);
                    self.note(format!("✎ {name} turned into a {s}"));
                }
                Err(e) => self.error(e),
            },
            Action::Customise(req) => {
                self.note(format!("✎ {name} passed that to /customise: {req}"));
                self.start_customise(&req);
            }
        }
    }

    fn set_title(&mut self, text: &str) {
        if crate::skin::current().notify.title {
            self.term_out.extend(crate::notify::title(text));
            self.titled = true;
        }
    }

    fn busy_title(&mut self) {
        let sk = crate::skin::current();
        let verb = crate::motion::pick_verb(&sk.spinner, self.verb_seed).0;
        self.set_title(&format!("{} {verb}… · cosmostui", sk.spinner.footer_glyph));
    }

    /// Title, bell, desktop notification — as `[notify]` and focus allow.
    fn alert(&mut self, kind: crate::notify::Kind, head: &str, body: &str) {
        let sk = crate::skin::current();
        let secs = self.busy_since.elapsed().as_secs();
        let a = crate::notify::alert(&sk.notify, kind, self.focused, secs, head, body);
        if !a.term.is_empty() {
            self.titled = true;
        }
        self.term_out.extend(&a.term);
        if !cfg!(test) {
            crate::notify::fire(&a);
        }
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
        self.turn_streamed = true;
        let st = self.stream.get_or_insert_with(|| Stream {
            md: Md::new(),
            partial: String::new(),
            started: false,
            chars: 0,
            all: String::new(),
            arrivals: std::collections::VecDeque::from([(Instant::now(), 0)]),
        });
        st.partial.push_str(s);
        st.all.push_str(s);
        st.chars += s.chars().count();
        st.arrivals.push_back((Instant::now(), st.chars));
        while st.arrivals.len() > 64 {
            st.arrivals.pop_front();
        }
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
            self.turn_chars += st.chars;
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
                    let degraded = degraded.filter(|d| !d.is_empty());
                    if let Some(d) = &degraded {
                        self.block(view::warn(&format!("Sandbox degraded — {d}"), self.width));
                    }
                    self.buddy_on(if degraded.is_some() { crate::buddy::Moment::Degraded } else { crate::buddy::Moment::Hello });
                }
            }
            Msg::Event(ev) => self.on_event(&ev),
            Msg::Delta(s) => self.on_delta(&s),
            Msg::AcpTool(v) => self.on_acp_tool(&v),
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
                let body = format!("{} {}", view::tool_label(&tool), detail).trim().to_string();
                self.perm = Some(Perm { tool, detail, sel: 0, reply });
                self.buddy_on(crate::buddy::Moment::Waiting);
                self.alert(crate::notify::Kind::Waiting, "Needs your OK", &body);
            }
            Msg::Note(s) => {
                if s.starts_with("compacted") {
                    self.buddy_on(crate::buddy::Moment::Compact);
                }
                self.note(s)
            }
            Msg::Err(s) => {
                if self.busy {
                    self.turn_err = Some(s.lines().next().unwrap_or("").to_string());
                }
                self.error(s)
            }
            Msg::TurnDone => {
                self.end_stream();
                // anything still pending was cut short
                for t in std::mem::take(&mut self.tools) {
                    let v = crate::toolview::ToolView {
                        name: &t.name,
                        input: &t.input,
                        content: None,
                        err: false,
                        diff: None,
                        interrupted: true,
                        elapsed: Some(t.started.elapsed()),
                    };
                    self.block(crate::toolview::render(&v, self.width));
                }
                self.flush_explore();
                self.inflight = self.inflight.saturating_sub(1);
                if self.inflight == 0 {
                    self.busy = false;
                    let late: Vec<String> = self.steer.lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default();
                    for (k, t) in late.into_iter().enumerate() {
                        self.queue.insert(k, (t.clone(), t));
                    }
                    let lines = view::turn_footer(self);
                    if !lines.is_empty() {
                        self.cont(lines);
                    }
                    let secs = self.busy_since.elapsed().as_secs();
                    match self.turn_err.take() {
                        Some(e) => {
                            self.buddy_on(crate::buddy::Moment::Error);
                            self.alert(crate::notify::Kind::Error, "Something went wrong", &e);
                        }
                        None => {
                            self.buddy_on(crate::buddy::Moment::Done { secs });
                            let body = self.last_reply.lines().find(|l| !l.trim().is_empty()).unwrap_or("Finished.");
                            let body = crate::wrap::ellipsize(body.trim().trim_start_matches('#').trim(), 120);
                            self.alert(crate::notify::Kind::Done, &format!("Done in {}", crate::motion::elapsed(secs)), &body);
                        }
                    }
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
                } else if !self.turn_streamed && !text.trim().is_empty() {
                    self.block(markdown::render(text.trim_matches('\n'), w));
                }
                if !text.trim().is_empty() {
                    self.last_reply = text.trim().to_string();
                }
                self.turn_streamed = false;
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
                            started: Instant::now(),
                        });
                    }
                }
            }
            "tool_result" => {
                let id = ev.data["tool_use_id"].as_str().unwrap_or("");
                let err = ev.data["is_error"].as_bool().unwrap_or(false);
                let content = ev.data["content"].as_str().unwrap_or("");
                let (name, input, elapsed) = match self.tools.iter().position(|t| t.id == id) {
                    Some(i) => {
                        let t = self.tools.remove(i);
                        (t.name, t.input, Some(t.started.elapsed()))
                    }
                    None => (ev.data["name"].as_str().unwrap_or("tool").to_string(), Value::Null, None),
                };
                let diff = self.diffs.remove(id);
                if err {
                    self.buddy_on(crate::buddy::Moment::ToolError);
                } else if name == "bash" {
                    match crate::buddy::test_outcome(input["command"].as_str().unwrap_or(""), content) {
                        Some(true) => self.buddy_on(crate::buddy::Moment::TestsPass),
                        Some(false) => self.buddy_on(crate::buddy::Moment::TestsFail),
                        None => {}
                    }
                }
                if name != "submit_plan" {
                    self.finish_tool(crate::toolview::ToolView {
                        name: &name,
                        input: &input,
                        content: Some(content),
                        err,
                        diff: diff.as_ref(),
                        interrupted: false,
                        elapsed,
                    });
                }
            }
            // a subscription agent's own tool (ACP) — shown, never replayed
            // drawn live already (Msg::AcpTool); this is the log's copy
            "tool_call" if ev.data["source"] == "acp" && self.acp_done.contains(ev.data["id"].as_str().unwrap_or("")) => {}
            "tool_call" if ev.data["source"] == "acp" => {
                self.end_stream();
                self.tools.retain(|t| t.id != ev.data["id"].as_str().unwrap_or(""));
                let name = ev.data["name"].as_str().unwrap_or("tool");
                let status = ev.data["status"].as_str().unwrap_or("");
                let input = serde_json::json!({"title": ev.data["title"]});
                let out = ev.data["output"].as_str().unwrap_or("");
                self.finish_tool(crate::toolview::ToolView {
                    name,
                    input: &input,
                    content: Some(out),
                    err: status == "failed",
                    diff: None,
                    interrupted: false,
                    elapsed: None,
                });
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
                    Mode::Plan => theme::YELLOW(),
                    Mode::Yolo => theme::RED(),
                    Mode::AcceptEdits => theme::ACCENT(),
                    Mode::Agent => theme::GREEN(),
                });
            }
            _ => {}
        }
    }

    // ---- background jobs ---------------------------------------------------

    /// Poll dictation, sign-in and shell jobs (once per loop turn).
    fn poll_jobs(&mut self) {
        self.poll_customise();
        self.poll_pet();
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
                        self.flash(format!("Transcribed with {label} — review, then ↵ to send"), theme::GREEN());
                    }
                    Err(e) if e.to_string() == "cancelled" => self.flash("Dictation cancelled", theme::YELLOW()),
                    Err(e) => self.flash(format!("Dictation failed: {e}"), theme::RED()),
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
                            self.flash("ChatGPT connected", theme::GREEN());
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
            self.flash("A ChatGPT sign-in is already waiting in your browser", theme::YELLOW());
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
        self.flash("Opening your browser to sign in with ChatGPT… (esc cancels)", theme::ACCENT());
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
        if matches!(self.popup, Some(Popup::Model { .. } | Popup::Connect { .. } | Popup::Resume { .. } | Popup::Help | Popup::Pager { .. } | Popup::Customise { .. })) || self.key_entry.is_some() {
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
            self.verb_seed = crate::motion::now_ms() as u64 / 7;
            self.turn_chars = 0;
            self.turn_err = None;
            self.turn_streamed = false;
            self.acp_done.clear();
            self.buddy_on(crate::buddy::Moment::TurnStart);
            self.busy_title();
        }
        self.inflight += 1;
        self.busy = true;
    }

    /// Send a prompt to the agent (`shown` is what the transcript displays).
    /// What the model gets: @mentions expanded, pending `!` output in front.
    fn for_model(&mut self, text: &str) -> String {
        let mut text = files::expand_mentions(text, &self.cwd);
        if !self.pending_ctx.is_empty() {
            let ctx = std::mem::take(&mut self.pending_ctx).join("\n\n");
            text = format!("{ctx}\n\n{text}");
        }
        text
    }

    fn send_prompt(&mut self, shown: &str, text: String, cmd: &Sender<Cmd>) {
        let text = self.for_model(&text);
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
                self.flash("No key entered", theme::YELLOW());
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
        if self.busy || self.inflight > 0 {
            let at = self.queue_edit.take().unwrap_or(self.queue.len()).min(self.queue.len());
            self.queue.insert(at, (raw, text));
            self.dirty = true;
            return;
        }
        self.queue_edit = None;
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
            "spinner" => {
                let name = args.trim();
                let styles = crate::motion::STYLES;
                if name.is_empty() {
                    let now = crate::skin::current().spinner.style.clone();
                    let list = styles
                        .iter()
                        .map(|s| format!("{}{}", crate::motion::frames(s).join(""), if *s == now { format!(" {s} ✓") } else { format!(" {s}") }))
                        .collect::<Vec<_>>()
                        .join("  ");
                    self.note(format!("Spinners: {list}"));
                } else if !styles.contains(&name) {
                    self.error(format!("No spinner {name:?} — try {}", styles.join(", ")));
                } else {
                    let path = zcore::cli_home().join("ui.toml");
                    match crate::skin::edit::set_in_file(&path, "spinner", "style", &crate::skin::edit::Val::Str(name.into())) {
                        Ok(()) => {
                            self.skin_reload = true;
                            self.flash(format!("Spinner → {name}"), theme::ACCENT());
                        }
                        Err(e) => self.error(format!("Couldn't save the spinner: {e}")),
                    }
                }
            }
            "buddy" => self.buddy_command(args.trim()),
            "customise" | "customize" => self.customise_command(args.trim()),
            "pettalk" | "talk" => self.pet_talk_command(args.trim()),
            "verbs" => self.verbs_command(args.trim()),
            "notify" => {
                let sk = crate::skin::current();
                let n = &sk.notify;
                if args.trim() == "test" {
                    let a = crate::notify::Alert {
                        term: [crate::notify::title("✓ Test · cosmostui"), if n.sound { Vec::new() } else { vec![0x07] }].concat(),
                        desktop: Some(("cosmostui — test".into(), "Notifications work.".into())),
                        chime: n.sound,
                    };
                    self.term_out.extend(&a.term);
                    self.titled = true;
                    crate::notify::fire(&a);
                    self.flash("Sent a test notification", theme::ACCENT());
                } else {
                    self.note(format!(
                        "Notify: title {} · bell {} · desktop {} · sound {} · turns ≥ {}s · on {} — change these in ui.toml [notify]; /notify test sends one",
                        if n.title { "on" } else { "off" },
                        n.bell,
                        n.desktop,
                        if n.sound { "on" } else { "off" },
                        n.min_secs,
                        n.events.join(", ")
                    ));
                }
            }
            "theme" => {
                let name = args.trim();
                let themes = crate::skin::themes();
                let now = crate::skin::current().theme.name.clone();
                if name.is_empty() {
                    let list = themes.iter().map(|t| if *t == now { format!("{t} ✓") } else { t.to_string() }).collect::<Vec<_>>().join(" · ");
                    self.note(format!("Themes: {list} — /theme <name> switches; ~/.cosmos-cli/ui.toml holds the rest"));
                } else if !themes.iter().any(|t| t == name) {
                    self.error(format!("No theme {name:?} — try {}", themes.join(", ")));
                } else {
                    let path = zcore::cli_home().join("ui.toml");
                    match crate::skin::edit::set_in_file(&path, "theme", "name", &crate::skin::edit::Val::Str(name.into())) {
                        Ok(()) => {
                            self.skin_reload = true;
                            self.flash(format!("Theme → {name}"), theme::ACCENT());
                        }
                        Err(e) => self.error(format!("Couldn't save the theme: {e}")),
                    }
                }
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
                    self.flash(format!("Copied the last reply ({} chars)", self.last_reply.chars().count()), theme::GREEN());
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
                self.flash(format!("Paste your {} API key and press ↵ (or set {env}) · esc cancels", p.name), theme::ACCENT());
            }
            None => self.error(format!("Unknown provider: {id}")),
        }
    }

    /// A subscription agent needs only its CLI on PATH and its own sign-in;
    /// it runs on the operator's plan, never on a key Cosmos holds.
    fn connect_acp(&mut self, id: &str) {
        let Some(p) = zcore::acp_client::plugin(id) else { return };
        if !zcore::acp_client::installed(id) {
            // no CLI yet: install the vendor's official one, then sign in
            self.handoff = Some(id.to_string());
            return;
        }
        // signed in already? the agent reports the plan's model list on session/new
        match zcore::catalog::provider(id).map(zcore::live::fetch) {
            Some(Ok(models)) => {
                let _ = zcore::live::store(id, &models);
                self.reload_models();
                self.note(format!("{} is connected — it runs on your {} plan", p.name, p.name));
                self.open_model_picker_for(id);
            }
            // most likely not signed in: run the vendor's sign-in
            _ => self.handoff = Some(id.to_string()),
        }
    }

/// A subscription agent's tool, live: shown running the moment it starts
    /// (after the text so far, so the transcript keeps its order), drawn
    /// with its diff when it finishes.
    fn on_acp_tool(&mut self, v: &serde_json::Value) {
        let id = v["id"].as_str().unwrap_or("").to_string();
        if id.is_empty() || self.acp_done.contains(&id) {
            return;
        }
        let status = v["status"].as_str().unwrap_or("");
        let done = matches!(status, "completed" | "failed");
        let name = v["name"].as_str().unwrap_or("tool").to_string();
        let input = serde_json::json!({"title": v["title"]});
        self.dirty = true;
        let pos = self.tools.iter().position(|t| t.id == id);
        if pos.is_none() {
            self.end_stream();
        }
        if !done {
            match pos {
                Some(i) => self.tools[i].input = input,
                None => self.tools.push(PendingTool { id, name, input, started: Instant::now() }),
            }
            return;
        }
        let started = pos.map(|i| self.tools.remove(i).started);
        let diff = v.get("diff").filter(|d| d.is_object()).map(|d| {
            let old = d["old"].as_str();
            let (lines, added, removed) = crate::worker::diff_rows(old.unwrap_or(""), d["new"].as_str().unwrap_or(""));
            Diff { path: d["path"].as_str().unwrap_or("?").to_string(), added, removed, created: old.is_none(), lines }
        });
        self.finish_tool(crate::toolview::ToolView {
            name: &name,
            input: &input,
            content: Some(v["output"].as_str().unwrap_or("")),
            err: status == "failed",
            diff: diff.as_ref(),
            interrupted: false,
            elapsed: started.map(|s| s.elapsed()),
        });
        self.acp_done.insert(id);
    }

    // ---- the queue ------------------------------------------------------

    /// Send the next queued message once the task is done.
    pub fn drain_queue(&mut self, cmd: &Sender<Cmd>) {
        if self.busy || self.inflight > 0 || self.perm.is_some() || self.queue.is_empty() {
            return;
        }
        let (shown, text) = self.queue.remove(0);
        self.queue_sel = match self.queue_sel {
            Some(0) | None => None,
            Some(i) => Some(i - 1),
        };
        self.send_prompt(&shown, text, cmd);
    }

    /// "send now": into the running task, context intact.
    fn send_now(&mut self, i: usize, cmd: &Sender<Cmd>) {
        if i >= self.queue.len() {
            return;
        }
        let (shown, text) = self.queue.remove(i);
        self.queue_sel = None;
        if !self.busy {
            self.send_prompt(&shown, text, cmd);
            return;
        }
        let text = self.for_model(&text);
        if let Ok(mut s) = self.steer.lock() {
            s.push(text);
        }
        self.flash("Sent — it joins the task at the next step", theme::ACCENT());
        self.dirty = true;
    }

    /// Keys while a queued message is selected. True when handled.
    fn queue_key(&mut self, k: &KeyEvent, cmd: &Sender<Cmd>) -> bool {
        let Some(i) = self.queue_sel else { return false };
        // ctrl+c, ctrl+o … keep working while a queued message is selected
        if k.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        let n = self.queue.len();
        if n == 0 {
            self.queue_sel = None;
            return false;
        }
        let i = i.min(n - 1);
        match k.code {
            // past the top of the queue: on into prompt history
            KeyCode::Up if i == 0 => {
                self.queue_sel = None;
                self.history_prev();
            }
            KeyCode::Up => self.queue_sel = Some(i - 1),
            KeyCode::Down if i + 1 < n => self.queue_sel = Some(i + 1),
            KeyCode::Down | KeyCode::Esc => self.queue_sel = None,
            KeyCode::Enter | KeyCode::Char('s') => self.send_now(i, cmd),
            KeyCode::Char('e') => {
                let (shown, _) = self.queue.remove(i);
                self.queue_sel = None;
                self.queue_edit = Some(i);
                self.input = shown.chars().collect();
                self.cursor = self.input.len();
            }
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
                self.queue.remove(i);
                self.queue_sel = if self.queue.is_empty() { None } else { Some(i.min(self.queue.len() - 1)) };
            }
            _ => return true,
        }
        self.dirty = true;
        true
    }

    /// After the terminal comes back from an install / sign-in.
    fn after_handoff(&mut self, id: &str, res: Result<(), String>) {
        let name = zcore::acp_client::plugin(id).map(|p| p.name).unwrap_or(id);
        if let Err(e) = res {
            self.error(format!("Couldn't connect {name}: {e}"));
            return;
        }
        match zcore::catalog::provider(id).map(zcore::live::fetch) {
            Some(Ok(models)) => {
                let n = models.len();
                let _ = zcore::live::store(id, &models);
                self.reload_models();
                self.note(format!("{name} is connected · {n} models on your plan"));
                self.open_model_picker_for(id);
            }
            Some(Err(e)) => {
                self.reload_models();
                self.note(format!("{name} is set up, but its model list didn't load ({e:#}) — showing its default"));
                self.open_model_picker_for(id);
            }
            None => {}
        }
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
            // long enough for the worker to stop a subscription agent cleanly
            self.quit_deadline = Some(Instant::now() + Duration::from_secs(3));
            let _ = cmd.send(Cmd::Quit);
        }
    }

    // ---- /customise ----------------------------------------------------------

    fn customise_paths() -> crate::customise::Paths {
        crate::customise::Paths::new(&zcore::cli_home())
    }

    /// `/customise`: the picker, a subcommand, or a request for a model.
    fn customise_command(&mut self, args: &str) {
        use crate::skin::edit;
        let p = Self::customise_paths();
        let (sub, rest) = args.split_once(' ').map(|(a, b)| (a, b.trim())).unwrap_or((args, ""));
        let lone = rest.is_empty();
        match sub {
            "" => self.popup = Some(Popup::Customise { sel: 0 }),
            "stop" if lone => match &self.customise {
                Some(r) => {
                    r.stop();
                    self.flash("Stopping the customiser after this step…", theme::YELLOW());
                }
                None => self.note("Nothing is being customised"),
            },
            "apply" if lone => self.apply_draft(),
            "discard" if lone => self.discard_draft(),
            "undo" if lone => match edit::undo(&p.ui) {
                Ok(()) => {
                    self.skin_reload = true;
                    self.flash("Restored the previous ui.toml", theme::ACCENT());
                }
                Err(e) => self.error(format!("Couldn't undo: {e}")),
            },
            "reset" if lone || matches!(rest, "defaults" | "classic") || crate::skin::schema::SECTIONS.contains(&rest) => {
                let text = match rest {
                    "" | "defaults" => String::new(),
                    "classic" => CLASSIC_UI.to_string(),
                    s => edit::remove_table(&std::fs::read_to_string(&p.ui).unwrap_or_default(), s),
                };
                match edit::write(&p.ui, &text) {
                    Ok(()) => {
                        self.skin_reload = true;
                        self.flash(format!("Reset {} — /customise undo brings it back", if lone { "to defaults" } else { rest }), theme::ACCENT());
                    }
                    Err(e) => self.error(format!("Couldn't reset: {e}")),
                }
            }
            "log" if lone => {
                if self.mod_log.is_empty() {
                    self.note("No customise activity yet");
                } else {
                    let rows: Vec<Line<'static>> = self.mod_log.iter().map(|l| Line::from(Span::styled(format!(" {l}"), theme::DIM()))).collect();
                    self.block(rows);
                }
            }
            "diff" if lone => {
                let ch = crate::customise::changes(&p);
                if ch.is_empty() {
                    self.note("No draft changes");
                } else {
                    let mut rows = vec![Line::from(Span::styled(" ✎ Draft changes", theme::bold(Style::new())))];
                    rows.extend(ch.into_iter().map(|c| Line::from(Span::styled(format!("   {c}"), theme::DIM()))));
                    self.block(rows);
                }
            }
            "schema" if lone => {
                let rows: Vec<Line<'static>> = crate::skin::schema::text().lines().map(|l| Line::from(Span::styled(format!(" {l}"), theme::DIM()))).collect();
                self.block(rows);
            }
            "preview" if lone => self.show_draft(),
            "model" if !rest.contains(' ') => {
                if lone {
                    let m = crate::skin::current().customise.model.clone();
                    self.note(format!("Customising with {} — /customise model <alias> picks another", if m.is_empty() { format!("{} (this session's model)", self.alias) } else { m }));
                } else if zcore::config::Config::load(&zcore::cli_home()).provider_for(rest).is_none() {
                    self.error(format!("No model {rest:?} — see /model"));
                } else {
                    match edit::set_in_file(&p.ui, "customise", "model", &edit::Val::Str(rest.into())) {
                        Ok(()) => {
                            self.skin_reload = true;
                            self.flash(format!("Customising with {rest}"), theme::ACCENT());
                        }
                        Err(e) => self.error(e),
                    }
                }
            }
            _ => self.start_customise(args),
        }
    }

    fn start_customise(&mut self, request: &str) {
        if let Some(r) = &self.customise {
            self.error(format!("Already customising ({:?}) — wait for it, or /customise stop", crate::wrap::ellipsize(&r.request, 60)));
            return;
        }
        let sk = crate::skin::current();
        let model = if sk.customise.model.is_empty() { self.alias.clone() } else { sk.customise.model.clone() };
        if (model.is_empty() || model == "fixture") && !request.starts_with("tool:") {
            self.note("No model is connected, so here's the picker — /connect adds a model for /customise <request>");
            self.popup = Some(Popup::Customise { sel: 0 });
            return;
        }
        let c = &sk.customise;
        self.mod_log.push(format!("» {request}  ({model})"));
        self.mod_status = format!("starting with {model}");
        self.customise = Some(crate::customise::Runner::start(request.to_string(), model, Self::customise_paths(), c.auto_apply, c.max_turns, c.max_usd));
        self.dirty = true;
    }

    /// Drain the customiser's messages (once per loop turn).
    fn poll_customise(&mut self) {
        use crate::customise::{Effect, Up};
        let Some(r) = &self.customise else { return };
        let msgs: Vec<Up> = r.rx.try_iter().collect();
        if msgs.is_empty() {
            return;
        }
        self.dirty = true;
        let mut finished = None;
        for m in msgs {
            match m {
                Up::Status(s) => self.mod_status = s,
                Up::Log(s) => {
                    self.mod_log.push(s);
                    if self.mod_log.len() > 300 {
                        self.mod_log.drain(..100);
                    }
                }
                Up::Effect(Effect::Draft) => {
                    self.draft_ready = true;
                    if self.preview_skin.is_some() {
                        self.preview_skin = Self::load_draft();
                    }
                }
                Up::Effect(Effect::Apply) => self.apply_draft(),
                Up::Effect(Effect::Config) => self.reload_models(),
                Up::Effect(Effect::Command) => self.commands = slash::load(&self.cwd),
                Up::Spent(c) => self.mod_usd += c,
                Up::Done(s) => finished = Some(Ok(s)),
                Up::Failed(e) => finished = Some(Err(e)),
            }
        }
        let Some(res) = finished else { return };
        self.customise = None;
        self.mod_status.clear();
        match res {
            Ok(s) => {
                self.mod_log.push(format!("✓ {}", s.lines().next().unwrap_or("done")));
                if !s.trim().is_empty() {
                    let mut rows = vec![Line::from(vec![Span::styled(" ✎ ", theme::fg(theme::ACCENT())), Span::styled("Customise", theme::bold(Style::new()))])];
                    rows.extend(markdown::render(s.trim(), self.width));
                    self.block(rows);
                }
                self.show_draft();
            }
            Err(e) => {
                self.mod_log.push(format!("✗ {e}"));
                // keep whatever it managed before failing; drop an empty draft
                self.draft_ready = crate::customise::tidy(&Self::customise_paths());
                self.error(format!("Customise: {e}"));
            }
        }
    }

    /// The pending draft, drawn in its own look, with how to keep it.
    fn show_draft(&mut self) {
        let p = Self::customise_paths();
        if !p.has_draft() {
            self.draft_ready = false;
            return;
        }
        let ch = crate::customise::changes(&p);
        if ch.is_empty() {
            crate::customise::discard(&p);
            self.draft_ready = false;
            return;
        }
        self.draft_ready = true;
        let skin = crate::skin::parse(&std::fs::read_to_string(&p.draft).unwrap_or_default()).unwrap_or_default();
        let mut rows = vec![Line::from(vec![
            Span::styled(" ✎ Draft", theme::bold(Style::new())),
            Span::styled(format!(" · {} change{}", ch.len(), if ch.len() == 1 { "" } else { "s" }), theme::DIM()),
        ])];
        rows.extend(ch.iter().take(8).map(|c| Line::from(Span::styled(format!("   {c}"), theme::DIM()))));
        if ch.len() > 8 {
            rows.push(Line::from(Span::styled(format!("   … +{} more · /customise diff", ch.len() - 8), theme::DIM())));
        }
        rows.push(Line::default());
        rows.extend(crate::customise::preview(&skin, self.width.min(100)));
        rows.push(Line::default());
        rows.push(Line::from(Span::styled(" ctrl+p try it on · ctrl+y apply · ctrl+n discard · /customise diff", theme::fg(theme::ACCENT()))));
        self.block(rows);
    }

    fn load_draft() -> Option<crate::skin::Skin> {
        crate::skin::parse(&std::fs::read_to_string(Self::customise_paths().draft).ok()?).ok()
    }

    /// ctrl+p: try the draft on the live UI (spinner, pet, composer, status).
    fn toggle_preview(&mut self) {
        if self.preview_skin.take().is_some() {
            self.flash("Back to your current look", crate::skin::palette().dim);
            return;
        }
        match Self::load_draft() {
            Some(sk) => {
                self.preview_skin = Some(sk);
                self.flash("Previewing the draft — ctrl+y keep it · ctrl+n drop it · ctrl+p back", theme::ACCENT());
            }
            None => self.error("The draft doesn't load — /customise diff shows it"),
        }
    }

    fn apply_draft(&mut self) {
        self.preview_skin = None;
        match crate::customise::apply(&Self::customise_paths()) {
            Ok(a) => {
                self.draft_ready = false;
                self.skin_reload = true;
                let mut msg = match a.changed.len() {
                    0 => "Nothing new in the draft".to_string(),
                    1 => "Applied 1 change".to_string(),
                    n => format!("Applied {n} changes"),
                };
                if !a.kept.is_empty() {
                    msg.push_str(&format!(" · kept your {}", a.kept.join(", ")));
                }
                msg.push_str(" — /customise undo reverts");
                self.flash(msg, theme::GREEN());
            }
            Err(e) => self.error(format!("Couldn't apply the draft: {e}")),
        }
    }

    fn discard_draft(&mut self) {
        self.preview_skin = None;
        crate::customise::discard(&Self::customise_paths());
        self.draft_ready = false;
        self.flash("Draft discarded", theme::YELLOW());
    }

    /// `/verbs`, `/verbs add Warping:Warped`, `/verbs remove Warping`, `/verbs reset`.
    fn verbs_command(&mut self, args: &str) {
        use crate::skin::edit;
        let sk = crate::skin::current();
        let path = zcore::cli_home().join("ui.toml");
        let (sub, rest) = args.split_once(' ').map(|(a, b)| (a, b.trim())).unwrap_or((args, ""));
        let mut verbs = sk.spinner.verbs.clone();
        let res = match sub {
            "" => {
                let now: Vec<String> = verbs.iter().map(|v| crate::motion::verb_forms(v).0).collect();
                self.note(format!("Verbs: {} — /verbs add Present:Past · /verbs remove <verb> · /verbs reset", now.join(", ")));
                return;
            }
            "add" if !rest.is_empty() => {
                for v in rest.split(',').map(str::trim).filter(|v| !v.is_empty()) {
                    if !verbs.iter().any(|x| x.eq_ignore_ascii_case(v)) {
                        verbs.push(v.to_string());
                    }
                }
                Ok(())
            }
            "remove" if !rest.is_empty() => {
                verbs.retain(|v| !crate::motion::verb_forms(v).0.eq_ignore_ascii_case(rest) && !v.eq_ignore_ascii_case(rest));
                Ok(())
            }
            "reset" => {
                let text = edit::remove(&std::fs::read_to_string(&path).unwrap_or_default(), "spinner", "verbs");
                edit::write(&path, &text).map(|_| verbs = crate::skin::SpinnerCfg::default().verbs)
            }
            _ => Err("Usage: /verbs [add Present:Past[, …] | remove <verb> | reset]".to_string()),
        };
        let res = res.and_then(|_| {
            if sub == "reset" {
                return Ok(());
            }
            let (t, k, v) = crate::skin::schema::check("spinner.verbs", &serde_json::json!(verbs))?;
            edit::set_in_file(&path, &t, &k, &v)
        });
        match res {
            Ok(()) => {
                self.skin_reload = true;
                self.flash(format!("{} verbs", verbs.len()), theme::ACCENT());
            }
            Err(e) => self.error(e),
        }
    }

    /// `/buddy` and its subcommands (settings land in ui.toml [buddy]).
    fn buddy_command(&mut self, args: &str) {
        use crate::skin::edit::{set_in_file, Val};
        let sk = crate::skin::current();
        let (sub, rest) = args.split_once(' ').map(|(a, b)| (a, b.trim())).unwrap_or((args, ""));
        let path = zcore::cli_home().join("ui.toml");
        let save = |k: &str, v: Val| set_in_file(&path, "buddy", k, &v);
        let res = match sub {
            "" => {
                if sk.buddy.enabled {
                    let id = view::buddy_identity(self);
                    self.block(crate::buddy::card(&id, &sk.buddy, &self.buddy, self.busy));
                } else {
                    self.note("Your buddy is off — /buddy on brings it back");
                }
                return;
            }
            "pet" => {
                if !sk.buddy.enabled {
                    self.note("Your buddy is off — /buddy on brings it back");
                    return;
                }
                self.buddy_on(crate::buddy::Moment::Pet);
                return;
            }
            "name" if rest.is_empty() => {
                self.error("Usage: /buddy name <name>");
                return;
            }
            "name" => save("name", Val::Str(rest.chars().take(24).collect())).map(|_| format!("Say hi to {rest}")),
            "trick" => {
                match crate::buddy::Trick::parse(if rest.is_empty() { "hop" } else { rest }) {
                    Some(t) => self.buddy.trick(t),
                    None => self.error("Tricks: hop, spin, sparkle, nap"),
                }
                self.dirty = true;
                return;
            }
            "species" if rest.is_empty() => {
                let all: Vec<String> = crate::pixel::SPECIES.iter().map(|s| format!("{} ({})", s.name, s.blurb)).collect();
                self.note(format!("Species: {} — /buddy species <name>", all.join(" · ")));
                return;
            }
            "species" => match crate::pixel::canonical(rest) {
                Some(s) => save("species", Val::Str(s.name.into())).map(|_| format!("Your pet is now a {}", s.name)),
                None => {
                    self.error(format!("No species {rest:?} — /buddy species lists them"));
                    return;
                }
            },
            "reroll" => {
                let mut cfg = sk.buddy.clone();
                cfg.seed += 1;
                let r = save("seed", Val::Int(cfg.seed as i64)).and_then(|_| save("species", Val::Str(String::new())));
                r.map(|_| "A new buddy hatched — /buddy to meet it".to_string())
            }
            "mini" | "full" => save("size", Val::Str(sub.into())).and_then(|_| save("enabled", Val::Bool(true))).map(|_| format!("Buddy → {sub}")),
            "on" => save("enabled", Val::Bool(true)).map(|_| "Buddy's back".to_string()),
            "off" => save("enabled", Val::Bool(false)).map(|_| "Buddy off — /buddy on to bring it back".to_string()),
            "quiet" => save("quips", Val::Str("off".into())).map(|_| "Buddy will keep its thoughts to itself".to_string()),
            "chatty" => save("quips", Val::Str("builtin".into())).map(|_| "Buddy will speak up again".to_string()),
            _ => {
                self.error("Usage: /buddy [pet | trick <hop|spin|sparkle|nap> | name <name> | species [name] | reroll | mini | full | quiet | chatty | on | off]");
                return;
            }
        };
        match res {
            Ok(msg) => {
                self.skin_reload = true;
                self.flash(msg, theme::ACCENT());
            }
            Err(e) => self.error(format!("Couldn't save the buddy settings: {e}")),
        }
    }

    /// Back at the terminal after a while: what happened meanwhile.
    fn welcome_back(&mut self) {
        let Some((at, turns, fixes)) = self.away.take() else { return };
        if at.elapsed() < Duration::from_secs(180) || !crate::skin::current().buddy.enabled {
            return;
        }
        let p = &self.buddy.progress;
        let (dt, df) = (p.turns.saturating_sub(turns), p.fixes.saturating_sub(fixes));
        if dt == 0 && df == 0 {
            return;
        }
        let mut s = format!("While you were away: {dt} turn{} done", if dt == 1 { "" } else { "s" });
        if df > 0 {
            s.push_str(&format!(", {df} red test{} fixed", if df == 1 { "" } else { "s" }));
        }
        if self.busy {
            s.push_str(", still working");
        }
        self.buddy.speak(s, 15);
        self.buddy.trick(crate::buddy::Trick::Hop);
        self.dirty = true;
    }

    fn answer_perm(&mut self, code: u8) {
        if let Some(p) = self.perm.take() {
            let verdict = match code {
                1 => "Allowed",
                2 => "Allowed for this session",
                _ => "Denied",
            };
            let _ = p.reply.send(code);
            self.flash(format!("{verdict} — {}", view::tool_label(&p.tool)), if code == 0 { theme::RED() } else { theme::GREEN() });
            self.buddy_on(crate::buddy::Moment::Answered);
            if self.busy {
                self.busy_title();
            }
        }
    }

    fn interrupt(&mut self, flag: &Arc<AtomicBool>) {
        flag.store(true, Ordering::Relaxed);
        self.flash("Interrupting — stops after the current step", theme::YELLOW());
    }

    // ---- keys --------------------------------------------------------------

    fn on_key(&mut self, k: KeyEvent, cmd: &Sender<Cmd>, interrupt: &Arc<AtomicBool>) {
        self.buddy.on(crate::buddy::Moment::Input, &crate::skin::current().buddy, "");
        if self.perm.is_none() && self.popup.is_none() && self.queue_key(&k, cmd) {
            return;
        }
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
                self.flash("Sign-in cancelled", theme::YELLOW());
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
                self.flash("Press Ctrl+C again to exit", theme::YELLOW());
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
        if ctrl && self.draft_ready && self.perm.is_none() && matches!(k.code, KeyCode::Char('y') | KeyCode::Char('n') | KeyCode::Char('p')) {
            match k.code {
                KeyCode::Char('y') => self.apply_draft(),
                KeyCode::Char('n') => self.discard_draft(),
                _ => self.toggle_preview(),
            }
            return;
        }
        if ctrl && k.code == KeyCode::Char('o') && self.perm.is_none() {
            if matches!(self.popup, Some(Popup::Pager { .. })) {
                self.popup = None;
            } else if self.outputs.is_empty() {
                self.flash("Nothing to expand yet", crate::skin::palette().dim);
            } else {
                self.popup = Some(Popup::Pager { idx: 0, scroll: 0 });
            }
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
            self.flash("Cancelled", theme::YELLOW());
            return;
        }
        if k.code == KeyCode::Esc {
            if let Some(l) = self.login.take() {
                l.cancel.store(true, Ordering::Relaxed);
                self.flash("Sign-in cancelled", theme::YELLOW());
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
            Some(Popup::Customise { sel }) => {
                let n = crate::customise::PICKER.len();
                let mut step = 0;
                match k.code {
                    KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                        self.popup = None;
                        return;
                    }
                    KeyCode::Up | KeyCode::Char('k') => *sel = (*sel + n - 1) % n,
                    KeyCode::Down | KeyCode::Tab | KeyCode::Char('j') => *sel = (*sel + 1) % n,
                    KeyCode::Left | KeyCode::Char('h') => step = -1,
                    KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => step = 1,
                    _ => {}
                }
                let row = *sel;
                if step != 0 {
                    match crate::customise::picker_step(&zcore::cli_home().join("ui.toml"), row, step) {
                        Ok(msg) => {
                            self.skin_reload = true;
                            self.flash(msg, theme::ACCENT());
                        }
                        Err(e) => self.error(e),
                    }
                }
                return;
            }
            Some(Popup::Pager { idx, scroll }) => {
                let n = self.outputs.len();
                let w = self.width;
                let len = self.outputs.iter().rev().nth(*idx).map(|o| view::pager_len(o, w)).unwrap_or(0);
                let last = len.saturating_sub(view::PAGER_ROWS);
                let page = view::PAGER_ROWS - 1;
                match k.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => self.popup = None,
                    KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1).min(last),
                    KeyCode::Down | KeyCode::Char('j') => *scroll = (*scroll + 1).min(last),
                    KeyCode::PageUp => *scroll = scroll.saturating_sub(page).min(last),
                    KeyCode::PageDown | KeyCode::Char(' ') => *scroll = (*scroll + page).min(last),
                    KeyCode::Home | KeyCode::Char('g') => *scroll = 0,
                    KeyCode::End | KeyCode::Char('G') => *scroll = last,
                    KeyCode::Left | KeyCode::Char('h') if *idx + 1 < n => {
                        *idx += 1;
                        *scroll = 0;
                    }
                    KeyCode::Right | KeyCode::Char('l') if *idx > 0 => {
                        *idx -= 1;
                        *scroll = 0;
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
            // ↑ from an empty composer manages the queue
            KeyCode::Up if self.input.is_empty() && !self.queue.is_empty() && self.queue_sel.is_none() && self.key_entry.is_none() && self.hist_pos.is_none() => {
                self.queue_sel = Some(self.queue.len() - 1);
            }
            KeyCode::Up => {
                let (_, crow, _) = view::wrapped_input(&self.input, self.cursor, view::input_width(self));
                if crow == 0 && self.key_entry.is_none() {
                    self.history_prev();
                } else {
                    self.move_vertical(-1);
                }
            }
            KeyCode::Down => {
                let (lines, crow, _) = view::wrapped_input(&self.input, self.cursor, view::input_width(self));
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
        let (lines, crow, ccol) = view::wrapped_input(&self.input, self.cursor, view::input_width(self));
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
    let app_steer = worker::Steer::default();
    let (cmd, rx) = worker::spawn(opts.clone(), cwd.to_path_buf(), interrupt.clone(), app_steer.clone());
    let mut app = App::new(cwd, models, screen.text_width());
    app.steer = app_steer;
    if let Some(d) = &opts.cwd {
        app.cwd = PathBuf::from(d);
    }
    app.user = zcore::welcome::user_name();
    app.buddy.progress = crate::buddy::Progress::load(&zcore::cli_home());
    app.cont(crate::logo::banner("cosmos", concat!("v", env!("CARGO_PKG_VERSION"))));

    let (mut skin, notes) = crate::skin::Watcher::start(&app.cwd);
    for n in notes {
        app.error(n);
    }
    let r = run_loop(&mut screen, &mut app, &rx, &cmd, &interrupt, &mut skin);
    if app.titled {
        // hand the title back to the shell / terminal profile
        use std::io::Write;
        let mut o = std::io::stdout();
        let _ = o.write_all(&crate::notify::title(""));
        let _ = o.flush();
    }
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

fn run_loop(screen: &mut Screen, app: &mut App, rx: &Receiver<Msg>, cmd: &Sender<Cmd>, interrupt: &Arc<AtomicBool>, skin: &mut crate::skin::Watcher) -> Result<i32> {
    let mut last_tick = Instant::now();
    let mut buddy_was = view::buddy_key(app);
    loop {
        while let Ok(m) = rx.try_recv() {
            app.on_msg(m);
        }
        app.drain_queue(cmd);
        // the look changed on disk (hand edit, /theme, /customise): swap it in
        // here, between frames — never while one is being drawn
        let reloaded = if std::mem::take(&mut app.skin_reload) { Some(skin.force()) } else { skin.poll() };
        if let Some(notes) = reloaded {
            for n in notes {
                app.flash(n, theme::RED());
            }
            app.dirty = true;
        }
        app.poll_jobs();
        if let Some(id) = app.handoff.take() {
            // the installer and the vendor's sign-in need the real terminal
            let out = std::mem::take(&mut app.out);
            screen.draw(&out, Frame { lines: &[], cursor: None })?;
            screen.leave(&[]);
            println!();
            let res = zcore::acp_client::connect(&id, &mut |s| println!("[1m· {s}[0m"));
            if let Err(e) = &res {
                println!("
✗ {e:#}
Press Enter to go back to cosmostui.");
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
            }
            *screen = Screen::enter()?;
            app.after_handoff(&id, res.map_err(|e| format!("{e:#}")));
            app.dirty = true;
        }
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
        let animating = app.busy || !app.tools.is_empty() || app.rec.is_some() || app.shell.is_some() || app.login.is_some() || app.customise.is_some();
        let sk = crate::skin::current();
        let frame_ms = if sk.motion.reduce { 1000 } else { 1000 / u64::from(sk.spinner.fps.clamp(1, 60)) };
        if animating && last_tick.elapsed() >= Duration::from_millis(frame_ms) {
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
        // the buddy blinks, dozes off, and its quips fade — repaint only then
        let buddy_now = view::buddy_key(app);
        if buddy_now != buddy_was {
            buddy_was = buddy_now;
            app.dirty = true;
        }
        if !app.term_out.is_empty() {
            let mut o = std::io::stdout();
            let _ = std::io::Write::write_all(&mut o, &std::mem::take(&mut app.term_out));
            let _ = std::io::Write::flush(&mut o);
        }
        if app.dirty {
            app.dirty = false;
            let (lines, cursor) = match &app.preview_skin {
                // trying a draft on: draw the live region in its look
                Some(sk) => crate::skin::scoped_skin(sk.clone(), || view::live(app)),
                None => view::live(app),
            };
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
                    CEvent::FocusGained => {
                        app.focused = Some(true);
                        app.welcome_back();
                        // back from elsewhere: the "done" title has been seen
                        if !app.busy && app.titled {
                            app.set_title("cosmostui");
                        }
                    }
                    CEvent::FocusLost => {
                        app.focused = Some(false);
                        let p = &app.buddy.progress;
                        app.away = Some((Instant::now(), p.turns, p.fixes));
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
        assert!(rows.iter().any(|r| r.contains("Edit a.rs") && r.contains("devin")), "{rows:?}");
        assert!(rows.iter().any(|r| r.contains("wrote a.rs")), "{rows:?}");
    }

    #[test]
    fn explores_group_then_ctrl_o_expands() {
        let _env = crate::env_lock();
        crate::skin::scoped_skin(crate::skin::Skin::default(), || {
            let mut a = app();
            let (cmd, _rx) = std::sync::mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let ev = |kind: &str, data: Value| zproto::Event {
                v: 1, id: "e".into(), parent: None, ts: String::new(), session: "s".into(),
                harness: "cosmostui".into(), kind: kind.into(), data,
            };
            let call = |id: &str, name: &str, input: Value| serde_json::json!({"text": "", "tool_calls": [{"id": id, "name": name, "input": input}]});
            let done = |id: &str, name: &str, content: &str| serde_json::json!({"tool_use_id": id, "name": name, "content": content, "is_error": false});
            a.on_msg(Msg::Event(ev("assistant", call("r1", "read", serde_json::json!({"path": "a.rs"})))));
            a.on_msg(Msg::Event(ev("tool_result", done("r1", "read", "one
two
"))));
            a.on_msg(Msg::Event(ev("assistant", call("g1", "grep", serde_json::json!({"pattern": "fn"})))));
            let long: String = (1..=40).map(|i| format!("a.rs:{i}:fn x{i}
")).collect();
            a.on_msg(Msg::Event(ev("tool_result", done("g1", "grep", &long))));
            assert!(a.out.is_empty(), "reads and searches wait to be grouped");
            assert_eq!(a.explore.len(), 2);
            let live: Vec<String> = view::live(&a).0.iter().map(text).collect();
            assert!(live.iter().any(|r| r.contains("Explored 1 file · 1 search")), "{live:?}");

            // anything else commits the group first, in order
            a.on_msg(Msg::Event(ev("assistant", call("b1", "bash", serde_json::json!({"command": "ls"})))));
            a.on_msg(Msg::Event(ev("tool_result", done("b1", "bash", "exit_code=0 log_path=x omitted_chars=0
<<<UNTRUSTED_SHELL
ok
>>>"))));
            let rows: Vec<String> = a.out.iter().map(text).collect();
            let g = rows.iter().position(|r| r.contains("Explored")).expect("group committed");
            let b = rows.iter().position(|r| r.contains("Bash ls")).expect("bash committed");
            assert!(g < b, "{rows:?}");
            assert!(a.explore.is_empty());

            // ctrl+o opens the newest output; ← walks back to the grep
            a.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL), &cmd, &stop);
            assert!(matches!(a.popup, Some(Popup::Pager { idx: 0, .. })));
            let page: Vec<String> = view::live(&a).0.iter().map(text).collect();
            assert!(page.iter().any(|r| r.contains("Bash ls")) && page.iter().any(|r| r.trim_start().starts_with("1  ok")), "{page:?}");
            a.on_key(key(KeyCode::Left), &cmd, &stop);
            a.on_key(key(KeyCode::End), &cmd, &stop);
            let page: Vec<String> = view::live(&a).0.iter().map(text).collect();
            assert!(page.iter().any(|r| r.contains("fn x40")), "scrolled to the end: {page:?}");
            assert!(page.iter().any(|r| r.contains("23–40 of 40")), "{page:?}");
            a.on_key(key(KeyCode::Esc), &cmd, &stop);
            assert!(a.popup.is_none());
        });
    }

    #[test]
    fn turns_set_the_title_and_move_the_buddy() {
        let _env = crate::env_lock();
        crate::skin::scoped_skin(crate::skin::Skin::default(), || {
            let mut a = app();
            let (cmd, _rx) = std::sync::mpsc::channel();
            let ev = |kind: &str, data: Value| zproto::Event {
                v: 1, id: "e".into(), parent: None, ts: String::new(), session: "s".into(),
                harness: "cosmostui".into(), kind: kind.into(), data,
            };
            a.input = "hi".chars().collect();
            a.cursor = 2;
            a.on_key(key(KeyCode::Enter), &cmd, &Arc::new(AtomicBool::new(false)));
            assert!(a.busy);
            let t = String::from_utf8_lossy(&std::mem::take(&mut a.term_out)).to_string();
            assert!(t.starts_with("]0;✻ ") && t.contains("… · cosmostui"), "busy title: {t:?}");
            assert_eq!(a.buddy.mood(a.busy), crate::buddy::Mood::Focused);

            // a red test run worries it
            a.on_msg(Msg::Event(ev("assistant", serde_json::json!({"text": "", "tool_calls": [{"id": "b", "name": "bash", "input": {"command": "cargo test"}}]}))));
            a.on_msg(Msg::Event(ev("tool_result", serde_json::json!({"tool_use_id": "b", "name": "bash", "is_error": false,
                "content": "exit_code=101 log_path=x omitted_chars=0
<<<UNTRUSTED_SHELL
FAILED
>>>"}))));
            assert_eq!(a.buddy.mood(a.busy), crate::buddy::Mood::Worried);

            // the turn ends: done title, pleased buddy (focus known: no bell)
            a.focused = Some(true);
            a.on_msg(Msg::TurnDone);
            let t = String::from_utf8_lossy(&std::mem::take(&mut a.term_out)).to_string();
            assert!(t.contains("✓ Done in 0s · cosmostui") && t.matches('').count() == 1, "title, no bell: {t:?}");
            assert!(!a.busy);

            // looking away during a permission prompt rings
            a.focused = Some(false);
            let (reply, _r) = std::sync::mpsc::channel();
            a.on_msg(Msg::PermissionAsk { tool: "bash".into(), reply });
            let t = String::from_utf8_lossy(&std::mem::take(&mut a.term_out)).to_string();
            assert!(t.contains("⚠ Needs your OK") && t.matches('').count() == 2, "title + bell: {t:?}");
            assert_eq!(a.buddy.mood(false), crate::buddy::Mood::Waiting);
        });
    }

    #[test]
    fn buddy_commands_write_ui_toml() {
        let _env = crate::env_lock();
        let mut a = app();
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("COSMOS_CLI_HOME", home.path());
        let (cmd, _rx) = std::sync::mpsc::channel();
        for c in ["/buddy name Nova", "/buddy mini", "/buddy reroll", "/buddy quiet"] {
            a.input = c.chars().collect();
            a.cursor = a.input.len();
            a.on_key(key(KeyCode::Enter), &cmd, &Arc::new(AtomicBool::new(false)));
        }
        let toml = std::fs::read_to_string(home.path().join("ui.toml")).unwrap();
        let sk: crate::skin::Skin = toml::from_str(&toml).unwrap();
        assert_eq!((sk.buddy.name.as_str(), sk.buddy.size.as_str(), sk.buddy.seed, sk.buddy.quips.as_str()), ("Nova", "mini", 1, "off"));
        a.input = "/buddy".chars().collect();
        a.cursor = a.input.len();
        a.out.clear();
        a.on_key(key(KeyCode::Enter), &cmd, &Arc::new(AtomicBool::new(false)));
        assert!(a.out.iter().map(text).any(|r| r.contains("focus") && r.contains('█')), "the card shows stats");
    }

    #[test]
    fn customise_end_to_end_while_you_work() {
        let _env = crate::env_lock();
        crate::skin::scoped_skin(crate::skin::Skin::default(), || {
            let mut a = app();
            let home = tempfile::tempdir().unwrap();
            std::env::set_var("COSMOS_CLI_HOME", home.path());
            a.alias = "fixture".into();
            let (cmd, _rx) = std::sync::mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let enter = |a: &mut App, s: &str| {
                a.input = s.chars().collect();
                a.cursor = a.input.len();
                a.on_key(key(KeyCode::Enter), &cmd, &stop);
            };
            // no real model: the picker opens instead
            enter(&mut a, "/customise make it purple");
            assert!(matches!(a.popup, Some(Popup::Customise { .. })));
            let rows: Vec<String> = view::live(&a).0.iter().map(text).collect();
            assert!(rows.iter().any(|r| r.contains("Theme") && r.contains("‹ cosmos ›")), "{rows:#?}");
            a.on_key(key(KeyCode::Right), &cmd, &stop);
            let ui = std::fs::read_to_string(home.path().join("ui.toml")).unwrap();
            assert!(ui.contains("name = \"classic\""), "the picker writes ui.toml: {ui}");
            a.on_key(key(KeyCode::Esc), &cmd, &stop);
            assert!(a.popup.is_none());

            // the fixture model drives a real customiser run
            enter(&mut a, r#"/customise tool:ui_set {"changes":[{"path":"spinner.style","value":"comet"},{"path":"hooks.cmd","value":"x"}]}"#);
            assert!(a.customise.is_some());
            let rows: Vec<String> = view::live(&a).0.iter().map(text).collect();
            assert!(rows.iter().any(|r| r.contains("customising with fixture")), "{rows:#?}");
            // you keep working meanwhile: typing is untouched
            a.input = "still typing".chars().collect();
            let t0 = Instant::now();
            while a.customise.is_some() && t0.elapsed() < Duration::from_secs(20) {
                a.poll_customise();
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(a.customise.is_none(), "the run finished");
            assert_eq!(a.input.iter().collect::<String>(), "still typing");
            assert!(a.draft_ready);
            let out: Vec<String> = a.out.iter().map(text).collect();
            assert!(out.iter().any(|r| r.contains("✎ Draft") && r.contains("1 change")), "{out:#?}");
            assert!(out.iter().any(|r| r.contains("ctrl+y apply")), "{out:#?}");
            let ui = std::fs::read_to_string(home.path().join("ui.toml")).unwrap();
            assert!(!ui.contains("comet"), "nothing lands before you say so");
            assert!(a.mod_log.iter().any(|l| l.contains("hard boundary")), "the hook write was refused: {:?}", a.mod_log);

            // ctrl+y applies it (your earlier picker change survives)
            a.input.clear();
            a.cursor = 0;
            a.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL), &cmd, &stop);
            let ui = std::fs::read_to_string(home.path().join("ui.toml")).unwrap();
            assert!(ui.contains("style = \"comet\"") && ui.contains("name = \"classic\"") && !ui.contains("hooks"), "{ui}");
            assert!(!a.draft_ready && a.skin_reload);
            // and /customise undo puts the old file back
            enter(&mut a, "/customise undo");
            let ui = std::fs::read_to_string(home.path().join("ui.toml")).unwrap();
            assert!(!ui.contains("comet"), "{ui}");
        });
    }

    /// Live: a real model customises the real `COSMOS_CLI_HOME`, driven
    /// through the App exactly as typed. Ignored — run with
    /// `COSMOS_LIVE_MODEL=<alias> COSMOS_LIVE_PROMPT=<file> cargo test -p ztui live_customise -- --ignored --nocapture`
    /// (`COSMOS_LIVE_HAND_EDIT="/spinner pulse"` edits ui.toml mid-run;
    /// `COSMOS_LIVE_FINISH=apply|discard|keep`, default apply).
    #[test]
    #[ignore]
    fn live_customise() {
        let _env = crate::env_lock();
        let model = std::env::var("COSMOS_LIVE_MODEL").expect("set COSMOS_LIVE_MODEL");
        let prompt = std::fs::read_to_string(std::env::var("COSMOS_LIVE_PROMPT").expect("set COSMOS_LIVE_PROMPT")).unwrap();
        let hand_edit = std::env::var("COSMOS_LIVE_HAND_EDIT").ok();
        let finish = std::env::var("COSMOS_LIVE_FINISH").unwrap_or_else(|_| "apply".into());
        let home = zcore::cli_home();
        let cwd = std::env::current_dir().unwrap();
        let (mut watch, notes) = crate::skin::Watcher::start(&cwd);
        println!("home {} · skin notes {notes:?}", home.display());
        let mut a = App::new(&cwd, model_rows(), 100);
        let (cmd, _rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let enter = |a: &mut App, s: &str| {
            a.input = s.chars().collect();
            a.cursor = a.input.len();
            a.on_key(key(KeyCode::Enter), &cmd, &stop);
        };
        let reload = |a: &mut App, w: &mut crate::skin::Watcher| {
            if std::mem::take(&mut a.skin_reload) {
                for n in w.force() {
                    println!("  skin note: {n}");
                }
            }
        };
        let text_of = |l: &Line| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();

        enter(&mut a, &format!("/customise model {model}"));
        reload(&mut a, &mut watch);
        println!("{:?}", a.notice.as_ref().map(|n| n.0.clone()));
        a.out.clear();
        enter(&mut a, &format!("/customise {}", prompt.trim()));
        assert!(a.customise.is_some(), "runner started: {:?}", a.out.iter().map(text_of).collect::<Vec<_>>());
        // you keep working meanwhile
        a.input = "still typing while it works".chars().collect();
        a.cursor = a.input.len();
        let t0 = Instant::now();
        let (mut seen, mut edited, mut strip_seen) = (0, false, false);
        while a.customise.is_some() && t0.elapsed() < Duration::from_secs(900) {
            a.poll_customise();
            reload(&mut a, &mut watch);
            for l in &a.mod_log[seen..] {
                println!("[{:>5.1}s] {l}", t0.elapsed().as_secs_f32());
            }
            seen = a.mod_log.len();
            if !strip_seen {
                if let Some(s) = view::live(&a).0.iter().map(text_of).find(|r| r.contains("customising with")) {
                    println!("strip: {}", s.trim());
                    strip_seen = true;
                }
            }
            if !edited && a.draft_ready {
                if let Some(h) = &hand_edit {
                    let typed = std::mem::take(&mut a.input);
                    enter(&mut a, h);
                    reload(&mut a, &mut watch);
                    println!("[{:>5.1}s] (operator) {h} → {:?}", t0.elapsed().as_secs_f32(), a.notice.as_ref().map(|n| n.0.clone()));
                    a.input = typed;
                    a.cursor = a.input.len();
                    edited = true;
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        a.poll_customise();
        for l in &a.mod_log[seen..] {
            println!("[{:>5.1}s] {l}", t0.elapsed().as_secs_f32());
        }
        assert!(a.customise.is_none(), "finished within 15 min");
        assert_eq!(a.input.iter().collect::<String>(), "still typing while it works", "the composer was never touched");
        println!("\n==== committed to scrollback ====");
        for l in &a.out {
            println!("{}", text_of(l));
        }
        a.out.clear();
        a.input.clear();
        a.cursor = 0;
        enter(&mut a, "/customise diff");
        println!("\n==== /customise diff ====");
        for l in &a.out {
            println!("{}", text_of(l));
        }
        match finish.as_str() {
            "apply" => a.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL), &cmd, &stop),
            "discard" => a.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL), &cmd, &stop),
            _ => {}
        }
        reload(&mut a, &mut watch);
        println!("\n==== {finish}: {:?} ====", a.notice.as_ref().map(|n| n.0.clone()));
        println!("{}", std::fs::read_to_string(home.join("ui.toml")).unwrap_or_default());
        println!("mod spend: ${:.4}", a.mod_usd);
    }

    #[test]
    fn pettalk_round_trips_and_the_pet_acts() {
        let _env = crate::env_lock();
        let mut sk = crate::skin::Skin::default();
        sk.buddy.species = "owl".into();
        sk.buddy.name = "Hoot".into();
        sk.buddy.images = "off".into();
        crate::skin::scoped_skin(sk, || {
            let mut a = app();
            let home = tempfile::tempdir().unwrap();
            std::env::set_var("COSMOS_CLI_HOME", home.path());
            a.alias = "fixture".into();
            let (cmd, _rx) = std::sync::mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            a.input = "/pettalk ping".chars().collect();
            a.cursor = a.input.len();
            a.on_key(key(KeyCode::Enter), &cmd, &stop);
            assert!(a.pet_talk.is_some() && a.buddy.listening);
            assert_eq!(a.buddy.mood(false), crate::buddy::Mood::Listening);
            let t0 = Instant::now();
            while a.pet_talk.is_some() && t0.elapsed() < Duration::from_secs(20) {
                a.poll_pet();
                std::thread::sleep(Duration::from_millis(20));
            }
            let rows: Vec<String> = a.out.iter().map(text).collect();
            assert!(rows.iter().any(|r| r.contains("you → Hoot: ping")), "{rows:#?}");
            assert!(rows.iter().any(|r| r.contains("Hoot")) && rows.iter().any(|r| r.contains("pong")), "{rows:#?}");
            assert!(rows.iter().any(|r| r.contains('▀') || r.contains('▄')), "its picture is drawn: {rows:#?}");
            assert_eq!(a.buddy.progress.chats, 1);
            assert_eq!(a.pet_chat.len(), 2, "it remembers the exchange");

            // what the pet can do
            a.pet_act(crate::pettalk::Action::Remind { mins: 1, text: "stretch".into() });
            assert_eq!(a.reminders.len(), 1);
            a.reminders[0].0 = Instant::now() - Duration::from_secs(1);
            a.term_out.clear();
            a.poll_pet();
            assert!(a.reminders.is_empty());
            assert!(a.buddy.current_quip(&crate::skin::current().buddy).is_some_and(|q| q.contains("stretch")));
            assert!(String::from_utf8_lossy(&a.term_out).contains("⏰ stretch"), "title + bell");
            a.pet_act(crate::pettalk::Action::Species("penguin".into()));
            a.pet_act(crate::pettalk::Action::Rename("Waddles".into()));
            let ui = std::fs::read_to_string(home.path().join("ui.toml")).unwrap();
            assert!(ui.contains("species = \"penguin\"") && ui.contains("name = \"Waddles\""), "{ui}");
            a.pet_act(crate::pettalk::Action::Customise("tool:ui_set {\"path\":\"spinner.style\",\"value\":\"moon\"}".into()));
            assert!(a.customise.is_some(), "handed to /customise (with its boundaries)");
            while a.customise.is_some() {
                a.poll_customise();
                std::thread::sleep(Duration::from_millis(20));
            }
        });
    }

    #[test]
    fn the_pet_welcomes_you_back_and_levels_up() {
        let _env = crate::env_lock();
        crate::skin::scoped_skin(crate::skin::Skin::default(), || {
            let mut a = app();
            a.away = Some((Instant::now() - Duration::from_secs(600), 0, 0));
            a.buddy.progress.turns = 3;
            a.buddy.progress.fixes = 1;
            a.welcome_back();
            let q = a.buddy.current_quip(&crate::skin::current().buddy).unwrap_or_default().to_string();
            assert!(q.contains("3 turns done") && q.contains("1 red test fixed"), "{q}");
            // a short absence says nothing
            a.buddy.quip = None;
            a.away = Some((Instant::now(), 0, 0));
            a.welcome_back();
            assert!(a.buddy.quip.is_none());
            // enough XP announces the level
            a.buddy.grant(500);
            a.pet_progress();
            assert!(a.notice.as_ref().is_some_and(|n| n.0.contains("reached level")), "{:?}", a.notice);
        });
    }

    #[test]
    fn ctrl_p_tries_a_draft_on() {
        let _env = crate::env_lock();
        crate::skin::scoped_skin(crate::skin::Skin::default(), || {
            let mut a = app();
            let home = tempfile::tempdir().unwrap();
            std::env::set_var("COSMOS_CLI_HOME", home.path());
            let p = crate::customise::Paths::new(home.path());
            crate::customise::exec(&p, false, "ui_set", &serde_json::json!({"path": "spinner.style", "value": "hop"}));
            a.draft_ready = true;
            let (cmd, _rx) = std::sync::mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
            a.on_key(ctrl('p'), &cmd, &stop);
            assert_eq!(a.preview_skin.as_ref().map(|s| s.spinner.style.as_str()), Some("hop"));
            let rows: Vec<String> = view::live(&a).0.iter().map(text).collect();
            assert!(rows.iter().any(|r| r.contains("previewing the draft")), "{rows:#?}");
            a.on_key(ctrl('p'), &cmd, &stop);
            assert!(a.preview_skin.is_none(), "ctrl+p again goes back");
            a.on_key(ctrl('p'), &cmd, &stop);
            a.on_key(ctrl('y'), &cmd, &stop);
            assert!(a.preview_skin.is_none() && !a.draft_ready, "applying ends the preview");
            assert!(std::fs::read_to_string(home.path().join("ui.toml")).unwrap().contains("style = \"hop\""));
        });
    }

    /// Live: talk to the pet on a real model, through the App, on the real
    /// home. Ignored — `COSMOS_LIVE_MODEL=<alias> COSMOS_LIVE_SAY="<msg>"
    /// cargo test -p ztui live_pettalk -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_pettalk() {
        let _env = crate::env_lock();
        let model = std::env::var("COSMOS_LIVE_MODEL").expect("set COSMOS_LIVE_MODEL");
        let say = std::env::var("COSMOS_LIVE_SAY").expect("set COSMOS_LIVE_SAY");
        let cwd = std::env::current_dir().unwrap();
        let (_w, _) = crate::skin::Watcher::start(&cwd);
        let mut a = App::new(&cwd, model_rows(), 100);
        a.alias = model;
        a.buddy.progress = crate::buddy::Progress::load(&zcore::cli_home());
        a.history = vec!["fix the flaky markdown test".into(), "make the pets nicer".into()];
        a.last_reply = "I replaced the ASCII pets with pixel art and added /pettalk.".into();
        let (cmd, _rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        a.input = format!("/pettalk {say}").chars().collect();
        a.cursor = a.input.len();
        a.on_key(key(KeyCode::Enter), &cmd, &stop);
        let t0 = Instant::now();
        while a.pet_talk.is_some() && t0.elapsed() < Duration::from_secs(200) {
            a.poll_pet();
            std::thread::sleep(Duration::from_millis(100));
        }
        println!("answered in {:.1}s", t0.elapsed().as_secs_f32());
        for l in &a.out {
            println!("{}", l.spans.iter().map(|s| s.content.to_string()).collect::<String>());
        }
        println!("reminders: {:?}", a.reminders.iter().map(|(_, s)| s).collect::<Vec<_>>());
        println!("bubble: {:?} · chats {} · pet spend ${:.4}", a.buddy.quip.as_ref().map(|q| q.0.clone()), a.buddy.progress.chats, a.pet_usd);
    }

    #[test]
    fn subscription_tools_show_live_and_in_order() {
        let _env = crate::env_lock();
        crate::skin::scoped_skin(crate::skin::Skin::default(), || {
            let mut a = app();
            a.start_busy();
            let ev = |kind: &str, data: Value| zproto::Event {
                v: 1, id: "e".into(), parent: None, ts: String::new(), session: "s".into(),
                harness: "cosmostui".into(), kind: kind.into(), data,
            };
            // text streams, then a tool starts: the text is committed first
            a.on_msg(Msg::Delta("Writing the platform layer first.".into()));
            a.on_msg(Msg::AcpTool(serde_json::json!({"id": "t1", "name": "devin:edit", "title": "Edit src/a.rs", "status": "in_progress"})));
            let rows: Vec<String> = a.out.iter().map(text).collect();
            assert!(rows.iter().any(|r| r.contains("Writing the platform layer first.")), "text first: {rows:#?}");
            assert_eq!(a.tools.len(), 1, "the tool shows as running right away");
            let live: Vec<String> = view::live(&a).0.iter().map(text).collect();
            assert!(live.iter().any(|r| r.contains("Edit src/a.rs")), "{live:#?}");
            // it finishes with a diff: +/- counts, folded or inline
            a.on_msg(Msg::AcpTool(serde_json::json!({"id": "t1", "name": "devin:edit", "title": "Edit src/a.rs", "status": "completed",
                "output": "edited src/a.rs", "diff": {"path": "src/a.rs", "old": "a\nb\n", "new": "a\nB\nc\n"}})));
            assert!(a.tools.is_empty());
            let rows: Vec<String> = a.out.iter().map(text).collect();
            let head = rows.iter().position(|r| r.contains("Edit src/a.rs")).expect("committed");
            assert!(rows[head].contains("+2") && rows[head].contains("−1") && rows[head].contains("devin"), "{rows:#?}");
            assert!(a.outputs.back().is_some_and(|o| o.diff.is_some()), "ctrl+o shows the diff in colour");
            // more text after the tool is its own block, after it
            a.on_msg(Msg::Delta("Fixing two bugs.\n".into()));
            a.on_msg(Msg::Event(ev("tool_call", serde_json::json!({"id": "t1", "name": "devin:edit", "title": "Edit src/a.rs",
                "status": "completed", "output": "edited src/a.rs", "source": "acp"}))));
            a.on_msg(Msg::Event(ev("assistant", serde_json::json!({"text": "Writing the platform layer first.\n\nFixing two bugs.", "tool_calls": []}))));
            let rows: Vec<String> = a.out.iter().map(text).collect();
            assert_eq!(rows.iter().filter(|r| r.contains("Edit src/a.rs")).count(), 1, "the log's copy isn't drawn twice: {rows:#?}");
            assert_eq!(rows.iter().filter(|r| r.contains("Writing the platform layer first.")).count(), 1, "nor the reply: {rows:#?}");
            let fix = rows.iter().position(|r| r.contains("Fixing two bugs.")).unwrap();
            assert!(fix > head, "in order: {rows:#?}");
        });
    }

    #[test]
    fn big_diffs_fold_and_small_ones_show() {
        crate::skin::scoped_skin(crate::skin::Skin::default(), || {
            let input = serde_json::json!({"path": "a.rs"});
            let big = crate::worker::diff_rows("", &"x\n".repeat(30)).0;
            let d = Diff { path: "a.rs".into(), added: 30, removed: 0, created: true, lines: big };
            let v = crate::toolview::ToolView { name: "write", input: &input, content: Some("ok"), err: false, diff: Some(&d), interrupted: false, elapsed: None };
            let rows: Vec<String> = crate::toolview::render(&v, 80).iter().map(text).collect();
            assert_eq!(rows.len(), 2, "{rows:#?}");
            assert!(rows[1].contains("30 diff lines") && rows[1].contains("ctrl+o"), "{rows:#?}");
            let small = crate::worker::diff_rows("a\n", "b\n").0;
            let d = Diff { path: "a.rs".into(), added: 1, removed: 1, created: false, lines: small };
            let v = crate::toolview::ToolView { name: "edit", input: &input, content: Some("ok"), err: false, diff: Some(&d), interrupted: false, elapsed: None };
            assert!(crate::toolview::render(&v, 80).len() > 2, "small diffs stay inline");
        });
    }

    #[test]
    fn messages_typed_while_busy_queue_above_the_composer() {
        let _env = crate::env_lock();
        crate::skin::scoped_skin(crate::skin::Skin::default(), || {
            let mut a = app();
            let (cmd, rx) = std::sync::mpsc::channel();
            let stop = Arc::new(AtomicBool::new(false));
            let enter = |a: &mut App, s: &str| {
                a.input = s.chars().collect();
                a.cursor = a.input.len();
                a.on_key(key(KeyCode::Enter), &cmd, &stop);
            };
            enter(&mut a, "first task");
            assert!(a.busy);
            let committed = a.out.len();
            enter(&mut a, "second");
            enter(&mut a, "third");
            assert_eq!(a.queue.len(), 2);
            assert_eq!(a.out.len(), committed, "queued messages are not in the transcript yet");
            let live: Vec<String> = view::live(&a).0.iter().map(text).collect();
            let q = live.iter().position(|r| r.contains("2 queued")).expect("the queue shows");
            let composer = live.iter().position(|r| r.starts_with("❯ ")).unwrap();
            assert!(q < composer && live[q + 1].contains("second"), "{live:#?}");
            // ↑ from the empty composer selects; e edits; x removes
            a.on_key(key(KeyCode::Up), &cmd, &stop);
            assert_eq!(a.queue_sel, Some(1));
            let live: Vec<String> = view::live(&a).0.iter().map(text).collect();
            assert!(live.iter().any(|r| r.contains("third") && r.contains("send now") && r.contains("✎") && r.contains("✕")), "{live:#?}");
            a.on_key(key(KeyCode::Char('e')), &cmd, &stop);
            assert_eq!(a.input.iter().collect::<String>(), "third");
            a.input.push('!');
            a.cursor = a.input.len();
            a.on_key(key(KeyCode::Enter), &cmd, &stop);
            assert_eq!(a.queue[1].0, "third!", "an edit goes back to its place");
            a.on_key(key(KeyCode::Up), &cmd, &stop);
            a.on_key(key(KeyCode::Up), &cmd, &stop);
            a.on_key(key(KeyCode::Char('x')), &cmd, &stop);
            assert_eq!(a.queue.iter().map(|q| q.0.as_str()).collect::<Vec<_>>(), vec!["third!"]);
            // ↵ sends it now: into the running task, not after it
            a.on_key(key(KeyCode::Enter), &cmd, &stop);
            assert!(a.queue.is_empty());
            assert_eq!(*a.steer.lock().unwrap(), vec!["third!".to_string()]);
            // a steer the task ended before taking comes back, first in line
            enter(&mut a, "later");
            while rx.try_recv().is_ok() {}
            a.on_msg(Msg::TurnDone);
            assert_eq!(a.queue.iter().map(|q| q.0.as_str()).collect::<Vec<_>>(), vec!["third!", "later"]);
            assert!(a.steer.lock().unwrap().is_empty());
            // when the task ends, queued messages go out in order
            a.drain_queue(&cmd);
            assert!(a.busy && a.queue.len() == 1);
            assert!(matches!(rx.try_recv(), Ok(Cmd::Prompt(t)) if t == "third!"));
            a.on_msg(Msg::TurnDone);
            a.drain_queue(&cmd);
            assert!(matches!(rx.try_recv(), Ok(Cmd::Prompt(t)) if t == "later"));
            assert!(a.out.iter().map(text).any(|r| r.contains("later")), "shown when it's actually sent");
            // ↑ past the top of the queue reaches prompt history
            enter(&mut a, "one more");
            a.on_key(key(KeyCode::Up), &cmd, &stop);
            assert_eq!(a.queue_sel, Some(0));
            a.on_key(key(KeyCode::Up), &cmd, &stop);
            assert!(a.queue_sel.is_none() && !a.input.is_empty(), "history recalled");
            // ctrl keys aren't swallowed by the queue
            a.input.clear();
            a.cursor = 0;
            a.on_key(key(KeyCode::Up), &cmd, &stop);
            a.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL), &cmd, &stop);
            assert!(matches!(a.popup, Some(Popup::Pager { .. }) | None));
        });
    }

    #[test]
    fn classic_preset_is_the_classic_skin() {
        let sk: crate::skin::Skin = toml::from_str(CLASSIC_UI).unwrap();
        assert_eq!(sk, crate::skin::Skin::classic());
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
    fn busy_line_and_turn_footer() {
        let _env = crate::env_lock();
        let mut a = app();
        a.busy = true;
        a.busy_since = Instant::now() - Duration::from_secs(14);
        a.verb_seed = 2; // Cooking / Cooked
        let line: String = view::busy_line(&a, 0).spans.iter().map(|s| s.content.to_string()).collect();
        assert!(line.contains("Cooking…") && line.contains("14s") && line.contains("esc to interrupt"), "{line}");
        // a running tool names what it is doing
        a.tools.push(PendingTool { id: "t".into(), name: "grep".into(), input: Value::Null, started: Instant::now() });
        let line: String = view::busy_line(&a, 0).spans.iter().map(|s| s.content.to_string()).collect();
        assert!(line.contains("Searching…"), "{line}");
        a.tools.clear();
        a.turn_chars = 4000;
        let foot: String = view::turn_footer(&a).iter().flat_map(|l| l.spans.iter().map(|s| s.content.to_string())).collect();
        assert!(foot.contains("Cooked for 14s · ↓ 1.0k tokens"), "{foot}");
        a.busy_since = Instant::now();
        assert!(view::turn_footer(&a).is_empty(), "no footer for an instant turn");
    }

    #[test]
    fn stream_tracks_fresh_characters() {
        let _env = crate::env_lock();
        let mut a = app();
        a.on_msg(Msg::Delta("hello ".into()));
        a.on_msg(Msg::Delta("world".into()));
        let st = a.stream.as_ref().unwrap();
        assert_eq!(st.fresh(10_000), 11, "everything is fresh within the window");
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(a.stream.as_ref().unwrap().fresh(10), 0, "settled after the window");
    }

    #[test]
    fn theme_command_writes_ui_toml() {
        let _env = crate::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let mut a = app();
        std::env::set_var("COSMOS_CLI_HOME", dir.path());
        let (tx, _rx) = channel();
        a.run_slash("theme", "nebula", &tx);
        assert!(a.skin_reload);
        let text = std::fs::read_to_string(dir.path().join("ui.toml")).unwrap();
        assert!(text.contains("name = \"nebula\""), "{text}");
        a.skin_reload = false;
        a.run_slash("theme", "plaid", &tx);
        assert!(!a.skin_reload, "unknown themes are not written");
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
