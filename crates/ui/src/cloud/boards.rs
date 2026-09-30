//! Automations and Kanban — the two always-visible sidebar destinations for
//! cloud + local agent runs.
//!
//! Kanban cards derive from the cloud run ledger (`cloud-runs.json`), the
//! Kanban drafts, and the local session rows the engine already syncs — a
//! card is a real run (or a real draft chat), never a decoration. The board
//! reads the shared `AppState` cloud view and refreshes it on a short timer
//! while visible. Automations persist next to the seat config and are fired
//! by the engine's scheduler; a fire on an unreachable seat lands as `missed`
//! on the row and as a red card on the board.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use gpui::{
    AnyElement, App, Context, Entity, Focusable, Hsla, IntoElement, Render, SharedString,
    Subscription, Task, Window, div, prelude::*, px,
};
use zeron_proto::{
    Chat, CloudAutomation, CloudRun, CloudRunStatus, CloudSeatKind, HarnessId, SessionStatus,
};
use zeron_rpc::methods;

use crate::cloud::{CloudView, SeatRow};
use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::icons::{self, icon};
use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// Click-through from a board to the rest of the app.
pub enum BoardsEvent {
    OpenChat(String),
    /// A missed-automation card → the Automations page.
    OpenAutomations,
}

impl gpui::EventEmitter<BoardsEvent> for KanbanPage {}
impl gpui::EventEmitter<BoardsEvent> for AutomationsPage {}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// "just now" / "4m ago" / "3h ago" / "2d ago".
pub fn relative(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (now - at).num_seconds();
    if secs < 45 {
        return "just now".into();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m ago", mins.max(1));
    }
    let hours = mins / 60;
    if hours < 48 {
        return format!("{hours}h ago");
    }
    format!("{}d ago", hours / 24)
}

/// "in 4m" / "in 3h" / "in 2d" (or "now").
pub fn until(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let secs = (at - now).num_seconds();
    if secs <= 30 {
        return "now".into();
    }
    let mins = (secs + 59) / 60;
    if mins < 60 {
        return format!("in {mins}m");
    }
    let hours = mins / 60;
    if hours < 48 {
        return format!("in {hours}h");
    }
    format!("in {}d", hours / 24)
}

/// Elapsed run time: `42s` / `3m 10s` / `1h 5m`.
fn format_elapsed(
    started: DateTime<Utc>,
    finished: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> String {
    let secs = (finished.unwrap_or(now) - started).num_seconds().max(0);
    if secs >= 3600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

/// The schedule as a person reads it. Daily/weekly fire at 09:00 UTC in the
/// engine; the local clock time rides along.
pub fn describe_schedule(schedule: &str) -> String {
    let local_nine = || {
        let utc = Utc::now().date_naive().and_hms_opt(9, 0, 0).map(|t| {
            t.and_utc()
                .with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        });
        match utc {
            Some(local) if local != "09:00" => format!("09:00 UTC ({local} local)"),
            _ => "09:00 UTC".to_string(),
        }
    };
    match schedule.trim() {
        "hourly" => "Every hour".into(),
        "daily" => format!("Every day at {}", local_nine()),
        "weekly" => format!("Every Monday at {}", local_nine()),
        cron => describe_cron(cron).unwrap_or_else(|| format!("Cron · {cron}")),
    }
}

/// Plain-words reading of the common cron shapes; `None` falls back to the
/// raw expression.
fn describe_cron(expr: &str) -> Option<String> {
    let f: Vec<&str> = expr.split_whitespace().collect();
    if f.len() != 5 {
        return None;
    }
    match (f[0], f[1], f[2], f[3], f[4]) {
        (m, "*", "*", "*", "*") if m.starts_with("*/") => {
            Some(format!("Every {} minutes", &m[2..]))
        }
        ("0", h, "*", "*", "*") if h.starts_with("*/") => Some(format!("Every {} hours", &h[2..])),
        (m, h, "*", "*", "*") if m.parse::<u8>().is_ok() && h.parse::<u8>().is_ok() => {
            Some(format!("Every day at {:0>2}:{:0>2} UTC", h, m))
        }
        (m, h, "*", "*", "1-5") if m.parse::<u8>().is_ok() && h.parse::<u8>().is_ok() => {
            Some(format!("Weekdays at {:0>2}:{:0>2} UTC", h, m))
        }
        _ => None,
    }
}

/// Brand mark for a seat's harness command (`claude -p`, `opencode run`, …).
fn command_mark(command: &str) -> Option<(&'static str, Option<Hsla>)> {
    let bin = command
        .split_whitespace()
        .next()?
        .rsplit('/')
        .next()?
        .to_lowercase();
    let id = if bin.contains("claude") {
        HarnessId::ClaudeCode
    } else if bin.contains("codex") {
        HarnessId::Codex
    } else if bin.contains("opencode") {
        HarnessId::Opencode
    } else if bin.contains("cursor") {
        HarnessId::Cursor
    } else if bin.contains("grok") {
        HarnessId::Grok
    } else if bin.contains("devin") {
        HarnessId::Devin
    } else if bin.contains("amp") {
        HarnessId::Amp
    } else if bin.contains("kimi") {
        HarnessId::KimiCode
    } else if bin.contains("hermes") {
        HarnessId::Hermes
    } else if bin == "pi" {
        HarnessId::Pi
    } else {
        return None;
    };
    Some(crate::pickers::harness_brand_icon(id))
}

fn seat_chip_label(kind: Option<CloudSeatKind>) -> &'static str {
    match kind {
        Some(CloudSeatKind::Railway) => "Railway",
        Some(CloudSeatKind::Box) => "Boat",
        Some(CloudSeatKind::Ssh) => "VPS",
        Some(CloudSeatKind::GitHub) => "GitHub",
        None => "Cloud",
    }
}

fn seat_chip_icon(kind: Option<CloudSeatKind>) -> &'static str {
    match kind {
        None => icons::LAPTOP,
        Some(CloudSeatKind::GitHub) => icons::GIT_BRANCH,
        Some(CloudSeatKind::Ssh) => icons::HARD_DRIVE,
        _ => icons::CLOUD,
    }
}

/// A small pill: icon + label. Radius 8, one tone.
fn chip(
    theme: &Theme,
    icon_path: Option<&'static str>,
    label: impl Into<SharedString>,
) -> gpui::Div {
    div()
        .flex_none()
        .h(px(18.0))
        .px(px(6.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .rounded(px(8.0))
        .bg(theme.element_hover.opacity(0.7))
        .text_size(crate::typography::ui_rems(10.0))
        .text_color(theme.text_muted)
        .when_some(icon_path, |el, path| {
            el.child(icon(path).size(px(10.0)).text_color(theme.text_muted))
        })
        .child(label.into())
}

/// A segmented choice button (filters, schedule kinds, targets).
fn segment(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    active: bool,
) -> gpui::Stateful<gpui::Div> {
    let hover = theme.element_hover;
    div()
        .id(id)
        .flex_none()
        .h(px(26.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .rounded(px(7.0))
        .text_size(crate::typography::ui_rems(11.5))
        .cursor_pointer()
        .text_color(if active { theme.text } else { theme.text_muted })
        .bg(if active {
            theme.element_hover
        } else {
            gpui::transparent_black()
        })
        .hover(move |s| s.bg(hover))
        .child(label.into())
}

fn segmented_bar(theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .p(px(2.0))
        .rounded(px(9.0))
        .border_1()
        .border_color(theme.border)
}

// ---------------------------------------------------------------------------
// Kanban
// ---------------------------------------------------------------------------

/// Which cards the board shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KanbanFilter {
    All,
    /// Chats in the sidebar's selected project.
    Workspace,
    CloudOnly,
    LocalOnly,
}

/// The board's five columns (the spec's IA). Error rides Done as a red
/// card; a stopped run waits on its user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Column {
    Draft,
    InProgress,
    Waiting,
    NeedsApproval,
    Done,
}

impl Column {
    const ALL: [Column; 5] = [
        Column::Draft,
        Column::InProgress,
        Column::Waiting,
        Column::NeedsApproval,
        Column::Done,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Draft => "Draft",
            Self::InProgress => "In progress",
            Self::Waiting => "Waiting",
            Self::NeedsApproval => "Needs approval",
            Self::Done => "Done",
        }
    }

    fn of(status: CloudRunStatus) -> Self {
        match status {
            CloudRunStatus::Queued => Self::Draft,
            CloudRunStatus::Working => Self::InProgress,
            CloudRunStatus::WaitingForUser | CloudRunStatus::Stopped => Self::Waiting,
            CloudRunStatus::NeedsApproval => Self::NeedsApproval,
            CloudRunStatus::Done | CloudRunStatus::Error => Self::Done,
        }
    }

    fn tone(self, theme: &Theme) -> Hsla {
        match self {
            Self::Draft => theme.text_faint,
            Self::InProgress => theme.accent,
            Self::Waiting => theme.warning,
            Self::NeedsApproval => theme.warning,
            Self::Done => theme.success,
        }
    }

    fn empty_copy(self) -> &'static str {
        match self {
            Self::Draft => {
                "Drafts wait here. + New task, then drag a card to In progress to start it."
            }
            Self::InProgress => "Running agents show up here.",
            Self::Waiting => "Runs waiting on you — a question, a pause, or a stop.",
            Self::NeedsApproval => "Tool calls parked on your approval.",
            Self::Done => "Finished today — errors stay red.",
        }
    }
}

#[derive(Clone)]
enum CardKind {
    /// A chat with a run (cloud ledger row or local session).
    Run,
    /// A Kanban draft (its chat exists; the run doesn't yet).
    Draft,
    /// An automation fire that couldn't reach its seat.
    Missed,
}

#[derive(Clone)]
struct KanbanCard {
    kind: CardKind,
    chat_id: String,
    title: String,
    subtitle: Option<String>,
    status: CloudRunStatus,
    column: Column,
    seat_kind: Option<CloudSeatKind>,
    seat_label: String,
    mark: Option<(&'static str, Option<Hsla>)>,
    repo: Option<String>,
    elapsed: Option<String>,
    at: DateTime<Utc>,
    error: Option<String>,
    cloud: bool,
}

/// Drag payload: a Draft card headed for In progress.
#[derive(Clone)]
struct DraftDrag {
    chat_id: String,
    title: SharedString,
}

struct DraftGhost {
    title: SharedString,
}

impl Render for DraftGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .w(px(220.0))
            .px(px(10.0))
            .py(px(8.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(theme.accent.opacity(0.6))
            .bg(theme.surface_card)
            .shadow_md()
            .text_size(crate::typography::ui_rems(12.0))
            .text_color(theme.text)
            .child(self.title.clone())
    }
}

/// The "+ New task" form.
struct NewTask {
    prompt: Entity<ComposerInput>,
    /// `None` = this computer.
    seat: Option<String>,
    busy: bool,
    error: Option<String>,
    _sub: Subscription,
}

/// `Route::Kanban` — five columns of cards for local and cloud runs.
pub struct KanbanPage {
    state: Entity<AppState>,
    filter: KanbanFilter,
    new_task: Option<NewTask>,
    /// Cards with an in-flight start/stop/discard (dimmed, not re-clickable).
    pending: HashSet<String>,
    error: Option<String>,
    action_task: Option<Task<()>>,
    /// Last paint — the refresh timer only polls while the board is on
    /// screen (the entity outlives navigation away from it).
    last_render: std::time::Instant,
    _observe: Subscription,
    _refresh: Task<()>,
}

impl KanbanPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        state.update(cx, |state, cx| state.refresh_cloud(None, cx));
        // Live while visible: the ledger moves under running agents.
        let refresh = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(4))
                    .await;
                let alive = this
                    .update(cx, |page, cx| {
                        if page.last_render.elapsed() < std::time::Duration::from_secs(6) {
                            page.state.update(cx, |state, cx| {
                                state.refresh_cloud(Some(std::time::Duration::from_secs(3)), cx)
                            });
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        });
        Self {
            state,
            filter: KanbanFilter::All,
            new_task: None,
            pending: HashSet::new(),
            error: None,
            action_task: None,
            last_render: std::time::Instant::now(),
            _observe: observe,
            _refresh: refresh,
        }
    }

    /// Fold drafts, the cloud ledger (latest run per chat), bound chats that
    /// never ran, missed automations, and local sessions into cards.
    fn cards(&self, cx: &App) -> Vec<KanbanCard> {
        let state = self.state.read(cx);
        let now = Utc::now();
        let empty = CloudView::default();
        let view = state.cloud_view().unwrap_or(&empty);
        let selected_space = state.selected_space.clone();
        let chat_in_scope = |chat: Option<&Chat>, cloud: bool| -> bool {
            match self.filter {
                KanbanFilter::All => true,
                KanbanFilter::CloudOnly => cloud,
                KanbanFilter::LocalOnly => !cloud,
                KanbanFilter::Workspace => match &selected_space {
                    Some(space) => chat.and_then(|c| c.space_id.as_ref()) == Some(space),
                    None => true,
                },
            }
        };
        let seat_meta = |seat_id: &str| -> (Option<CloudSeatKind>, String, Option<String>) {
            match view.seat(seat_id) {
                Some(seat) => (
                    seat.config.kind,
                    seat.config.label.clone(),
                    seat.config.harness_command.clone(),
                ),
                None => (None, "Cloud seat".into(), None),
            }
        };
        let mut cards: Vec<KanbanCard> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        // Drafts first — they carry their own prompt.
        for draft in &view.drafts {
            let chat = state.chat_row(&draft.chat_id);
            if chat.is_some_and(|c| c.archived) {
                continue;
            }
            let cloud = draft.seat_id.is_some();
            if !chat_in_scope(chat, cloud) {
                continue;
            }
            let (seat_kind, seat_label, command) = match &draft.seat_id {
                Some(id) => seat_meta(id),
                None => (None, "This computer".into(), None),
            };
            seen.insert(draft.chat_id.clone());
            cards.push(KanbanCard {
                kind: CardKind::Draft,
                chat_id: draft.chat_id.clone(),
                title: chat
                    .and_then(|c| c.title.clone())
                    .unwrap_or_else(|| zeron_engine::cloud::task_title(&draft.prompt)),
                subtitle: Some(draft.prompt.clone()),
                status: CloudRunStatus::Queued,
                column: Column::Draft,
                seat_kind,
                seat_label,
                mark: draft
                    .harness
                    .map(crate::pickers::harness_brand_icon)
                    .or_else(|| command.as_deref().and_then(command_mark)),
                repo: chat
                    .and_then(|c| c.cloud_seat.as_ref())
                    .and_then(|b| b.repo.clone()),
                elapsed: None,
                at: draft.created_at,
                error: None,
                cloud,
            });
        }

        // Cloud ledger — the latest run per chat is the card.
        let mut latest: HashMap<&str, &CloudRun> = HashMap::new();
        for run in &view.runs {
            latest
                .entry(run.chat_id.as_str())
                .and_modify(|cur| {
                    if run.started_at > cur.started_at {
                        *cur = run;
                    }
                })
                .or_insert(run);
        }
        for run in latest.values() {
            if seen.contains(&run.chat_id) {
                continue;
            }
            let chat = state.chat_row(&run.chat_id);
            if chat.is_some_and(|c| c.archived) || !chat_in_scope(chat, true) {
                continue;
            }
            // Done cards age off after a day; everything else stays until
            // the user acts on it.
            if run.status == CloudRunStatus::Done
                && run
                    .finished_at
                    .is_some_and(|at| (now - at).num_hours() >= 24)
            {
                continue;
            }
            let (seat_kind, seat_label, command) = seat_meta(&run.seat_id);
            seen.insert(run.chat_id.clone());
            cards.push(KanbanCard {
                kind: CardKind::Run,
                chat_id: run.chat_id.clone(),
                title: chat
                    .and_then(|c| c.title.clone())
                    .or_else(|| run.prompt.clone())
                    .unwrap_or_else(|| "Cloud run".into()),
                subtitle: run.prompt.clone().filter(|p| {
                    chat.and_then(|c| c.title.as_deref())
                        .is_some_and(|t| t != p)
                }),
                status: run.status,
                column: Column::of(run.status),
                seat_kind,
                seat_label,
                mark: run
                    .command
                    .as_deref()
                    .or(command.as_deref())
                    .and_then(command_mark),
                repo: chat
                    .and_then(|c| c.cloud_seat.as_ref())
                    .and_then(|b| b.repo.clone()),
                elapsed: Some(format_elapsed(run.started_at, run.finished_at, now)),
                at: run.finished_at.unwrap_or(run.updated_at),
                error: run.error.clone(),
                cloud: true,
            });
        }

        // Seat-bound chats that never ran (older builds' drafts).
        for chat in &state.chats {
            if chat.archived || seen.contains(&chat.id) {
                continue;
            }
            let Some(binding) = chat.cloud_seat.as_ref() else {
                continue;
            };
            if chat.last_message_at.is_some() || !chat_in_scope(Some(chat), true) {
                continue;
            }
            let (seat_kind, seat_label, command) = seat_meta(&binding.seat_id);
            seen.insert(chat.id.clone());
            cards.push(KanbanCard {
                kind: CardKind::Run,
                chat_id: chat.id.clone(),
                title: chat.title.clone().unwrap_or_else(|| "Untitled".into()),
                subtitle: None,
                status: CloudRunStatus::Queued,
                column: Column::Draft,
                seat_kind,
                seat_label,
                mark: command.as_deref().and_then(command_mark),
                repo: binding.repo.clone(),
                elapsed: None,
                at: chat.created_at,
                error: None,
                cloud: true,
            });
        }

        // Missed automation fires — no chat exists; the card opens the page.
        if self.filter != KanbanFilter::LocalOnly {
            for automation in &view.automations {
                let Some(status) = automation.last_status.as_deref() else {
                    continue;
                };
                let error = if status == "missed" {
                    "Missed — the seat was unreachable at fire time".to_string()
                } else if let Some(err) = status.strip_prefix("error:") {
                    err.to_string()
                } else {
                    continue;
                };
                let seat = automation.seat_id.as_deref().map(seat_meta);
                cards.push(KanbanCard {
                    kind: CardKind::Missed,
                    chat_id: automation.id.clone(),
                    title: automation.name.clone(),
                    subtitle: Some(describe_schedule(&automation.schedule)),
                    status: CloudRunStatus::Error,
                    column: Column::Done,
                    seat_kind: seat.as_ref().and_then(|s| s.0),
                    seat_label: seat.map(|s| s.1).unwrap_or_else(|| "This computer".into()),
                    mark: None,
                    repo: None,
                    elapsed: None,
                    at: automation.last_run_at.unwrap_or(now),
                    error: Some(error),
                    cloud: automation.seat_id.is_some(),
                });
            }
        }

        // Local sessions — chats with no seat binding.
        if self.filter != KanbanFilter::CloudOnly {
            for session in &state.sessions {
                if seen.contains(&session.chat_id) {
                    continue;
                }
                let Some(chat) = state.chat_row(&session.chat_id) else {
                    continue;
                };
                if chat.archived || chat.cloud_seat.is_some() || !chat_in_scope(Some(chat), false) {
                    continue;
                }
                let status = match session.status {
                    SessionStatus::Working => CloudRunStatus::Working,
                    SessionStatus::AwaitingInput => CloudRunStatus::WaitingForUser,
                    SessionStatus::Errored => CloudRunStatus::Error,
                    SessionStatus::Idle => CloudRunStatus::Done,
                };
                // Resting chats only show while fresh: today's finished work.
                if status == CloudRunStatus::Done && (now - session.updated_at).num_hours() >= 12 {
                    continue;
                }
                if status == CloudRunStatus::Error && (now - session.updated_at).num_hours() >= 24 {
                    continue;
                }
                seen.insert(session.chat_id.clone());
                cards.push(KanbanCard {
                    kind: CardKind::Run,
                    chat_id: session.chat_id.clone(),
                    title: chat.title.clone().unwrap_or_else(|| "Session".into()),
                    subtitle: chat
                        .last_message_preview
                        .clone()
                        .filter(|p| !p.trim().is_empty()),
                    status,
                    column: Column::of(status),
                    seat_kind: None,
                    seat_label: "This computer".into(),
                    mark: chat
                        .config
                        .as_ref()
                        .map(|cfg| crate::pickers::harness_brand_icon(cfg.harness)),
                    repo: chat.branch.clone(),
                    elapsed: session
                        .started_at
                        .filter(|_| status == CloudRunStatus::Working)
                        .map(|start| format_elapsed(start, None, now)),
                    at: session.updated_at,
                    error: None,
                    cloud: false,
                });
            }
        }
        // Newest activity first inside each column.
        cards.sort_by(|a, b| b.at.cmp(&a.at));
        cards
    }

    fn call(
        &mut self,
        chat_id: String,
        method: &'static str,
        params: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.error = Some("Engine not connected".into());
            cx.notify();
            return;
        };
        self.pending.insert(chat_id.clone());
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, params).await;
            this.update(cx, |page, cx| {
                page.pending.remove(&chat_id);
                if let Err(err) = result {
                    page.error = Some(err.to_string());
                }
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn start_draft(&mut self, chat_id: String, cx: &mut Context<Self>) {
        let params = serde_json::json!({ "chatId": chat_id });
        self.call(chat_id, methods::CLOUD_TASK_START, params, cx);
    }

    fn discard_draft(&mut self, chat_id: String, cx: &mut Context<Self>) {
        let params = serde_json::json!({ "chatId": chat_id });
        self.call(chat_id, methods::CLOUD_TASK_DISCARD, params, cx);
    }

    fn stop_run(&mut self, chat_id: String, cx: &mut Context<Self>) {
        let params = serde_json::json!({ "chatId": chat_id, "op": "stop" });
        self.call(chat_id, methods::CLOUD_SEAT_CONTROL, params, cx);
    }

    fn open_new_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.new_task.is_some() {
            self.new_task = None;
            cx.notify();
            return;
        }
        let prompt = cx.new(|cx| ComposerInput::new("What should the agent do?", cx));
        let sub = cx.subscribe(&prompt, |this, _, event, cx| {
            if let ComposerInputEvent::Submitted = event {
                this.create_task(true, cx);
            }
        });
        // Default target follows the sidebar mode: a Cloud-mode user's new
        // task lands on their picked seat.
        let seat = {
            let state = self.state.read(cx);
            state
                .active_cloud_seat()
                .filter(|seat| CloudView::seat_runnable(seat))
                .map(|seat| seat.config.id.clone())
        };
        let focus = prompt.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.new_task = Some(NewTask {
            prompt,
            seat,
            busy: false,
            error: None,
            _sub: sub,
        });
        cx.notify();
    }

    fn create_task(&mut self, start: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let space_id = self.state.read(cx).selected_space.clone();
        let Some(form) = self.new_task.as_mut() else {
            return;
        };
        if form.busy {
            return;
        }
        let prompt = form.prompt.read(cx).text().trim().to_string();
        if prompt.is_empty() {
            form.error = Some("Describe the task first".into());
            cx.notify();
            return;
        }
        form.busy = true;
        form.error = None;
        let seat_id = form.seat.clone();
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_TASK_CREATE,
                    serde_json::json!({
                        "prompt": prompt,
                        "seatId": seat_id,
                        "spaceId": space_id,
                        "start": start,
                    }),
                )
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(_) => page.new_task = None,
                    Err(err) => {
                        if let Some(form) = page.new_task.as_mut() {
                            form.busy = false;
                            form.error = Some(err.to_string());
                        }
                    }
                }
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_new_task(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.new_task.as_ref()?;
        let seats: Vec<SeatRow> = self
            .state
            .read(cx)
            .cloud_seats()
            .iter()
            .filter(|seat| CloudView::seat_runnable(seat))
            .cloned()
            .collect();
        let mut targets = div().flex().flex_row().flex_wrap().gap(px(4.0)).child(
            segment(
                theme,
                "task-target-local",
                "This computer",
                form.seat.is_none(),
            )
            .child(
                icon(icons::LAPTOP)
                    .size(px(11.0))
                    .text_color(theme.text_muted),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(form) = this.new_task.as_mut() {
                    form.seat = None;
                }
                cx.notify();
            })),
        );
        for (ix, seat) in seats.iter().enumerate() {
            let id = seat.config.id.clone();
            let active = form.seat.as_deref() == Some(id.as_str());
            targets = targets.child(
                segment(
                    theme,
                    ("task-target-seat", ix),
                    format!(
                        "{} · {}",
                        seat_chip_label(seat.config.kind),
                        seat.config.label
                    ),
                    active,
                )
                .child(
                    icon(seat_chip_icon(seat.config.kind))
                        .size(px(11.0))
                        .text_color(theme.text_muted),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = this.new_task.as_mut() {
                        form.seat = Some(id.clone());
                    }
                    cx.notify();
                })),
            );
        }
        Some(
            div()
                .mx(px(20.0))
                .mb(px(12.0))
                .p(px(12.0))
                .rounded(px(12.0))
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface_card)
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(popover::dialog_field(
                    form.prompt.clone().into_any_element(),
                ))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_none()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_faint)
                                .child("Run on"),
                        )
                        .child(targets),
                )
                .when_some(form.error.clone(), |el, err| {
                    el.child(widgets::error_strip(theme, err))
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_1()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.text_faint)
                                .child("⏎ starts now · Save as draft to start it later"),
                        )
                        .child(
                            popover::btn_ghost(theme, "Cancel", "task-cancel")
                                .id("task-cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.new_task = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            popover::btn_ghost(theme, "Save as draft", "task-draft")
                                .id("task-draft")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.create_task(false, cx)),
                                ),
                        )
                        .child(
                            popover::btn_primary(
                                theme,
                                if form.busy { "Starting…" } else { "Start" },
                            )
                            .id("task-start")
                            .when(form.busy, |el| el.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| this.create_task(true, cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_card(
        &self,
        card: &KanbanCard,
        ix: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Utc::now();
        let pending = self.pending.contains(&card.chat_id);
        let errored = card.status == CloudRunStatus::Error;
        let group = SharedString::from(format!("kanban-card-{ix}"));
        let chat_id = card.chat_id.clone();
        let kind = card.kind.clone();
        let hover = theme.element_hover;
        let status_line: Option<(SharedString, Hsla)> = if let Some(err) = &card.error {
            Some((err.clone().into(), theme.danger))
        } else {
            match card.status {
                CloudRunStatus::Working => Some((
                    format!("Working · {}", card.elapsed.clone().unwrap_or_default()).into(),
                    theme.accent,
                )),
                CloudRunStatus::WaitingForUser => Some(("Waiting for you".into(), theme.warning)),
                CloudRunStatus::Stopped => {
                    Some(("Stopped — resume from the chat".into(), theme.warning))
                }
                CloudRunStatus::NeedsApproval => {
                    Some(("Needs your approval".into(), theme.warning))
                }
                _ => None,
            }
        };
        let mut meta = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(4.0))
            .child(chip(
                theme,
                Some(seat_chip_icon(card.seat_kind)),
                if card.cloud {
                    seat_chip_label(card.seat_kind).to_string()
                } else {
                    "Local".to_string()
                },
            ));
        if let Some(repo) = &card.repo {
            let short = repo.rsplit('/').next().unwrap_or(repo).to_string();
            meta = meta.child(chip(theme, Some(icons::GIT_BRANCH), short));
        }
        let mut actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .invisible()
            .group_hover(group.clone(), |s| s.visible());
        let action = |id: SharedString, icon_path: &'static str, tip: &'static str| {
            div()
                .id(id)
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .tooltip(widgets::text_tooltip(tip))
                .child(icon(icon_path).size(px(12.0)).text_color(theme.text_muted))
        };
        match card.kind {
            CardKind::Draft => {
                let start_id = chat_id.clone();
                let discard_id = chat_id.clone();
                actions = actions
                    .child(
                        action(
                            format!("kanban-start-{ix}").into(),
                            icons::PLAY,
                            "Start now",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.start_draft(start_id.clone(), cx);
                        })),
                    )
                    .child(
                        action(
                            format!("kanban-discard-{ix}").into(),
                            icons::TRASH_BIN_MINIMALISTIC,
                            "Discard draft",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.discard_draft(discard_id.clone(), cx);
                        })),
                    );
            }
            CardKind::Run if card.status == CloudRunStatus::Working => {
                let stop_id = chat_id.clone();
                actions = actions.child(
                    action(
                        format!("kanban-stop-{ix}").into(),
                        icons::STOP,
                        "Stop the run",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.stop_run(stop_id.clone(), cx);
                    })),
                );
            }
            _ => {}
        }
        let mut el = div()
            .id(("kanban-card", ix))
            .group(group)
            .w_full()
            .rounded(px(12.0))
            .border_1()
            .border_color(if errored {
                theme.danger.opacity(0.55)
            } else {
                theme.border
            })
            .bg(if errored {
                theme.danger.opacity(0.06)
            } else {
                theme.surface_card
            })
            .hover(move |s| s.border_color(theme_border_hover(hover)))
            .px(px(10.0))
            .py(px(9.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .cursor_pointer()
            .when(pending, |el| el.opacity(0.5))
            .on_click(cx.listener(move |_, _, _, cx| match kind {
                CardKind::Missed => cx.emit(BoardsEvent::OpenAutomations),
                _ => cx.emit(BoardsEvent::OpenChat(chat_id.clone())),
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(6.0))
                    .child(match card.mark {
                        Some((path, tint)) => div().pt(px(2.0)).child(
                            icon(path)
                                .size(px(13.0))
                                .flex_none()
                                .text_color(tint.unwrap_or(theme.text_muted)),
                        ),
                        None => div().pt(px(2.0)).child(
                            icon(match card.kind {
                                CardKind::Missed => icons::CLOCK_CIRCLE,
                                _ => icons::BOT,
                            })
                            .size(px(13.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                        ),
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(crate::typography::ui_rems(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .line_clamp(2)
                            .child(SharedString::from(card.title.clone())),
                    )
                    .child(actions),
            )
            .when_some(card.subtitle.clone(), |el, sub| {
                el.child(
                    div()
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .line_clamp(2)
                        .child(SharedString::from(sub)),
                )
            })
            .when_some(status_line, |el, (line, color)| {
                el.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(5.0))
                        .child(div().size(px(6.0)).flex_none().rounded_full().bg(color))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(color)
                                .child(line),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().flex_1().min_w_0().child(meta))
                    .child(
                        div()
                            .flex_none()
                            .text_size(crate::typography::ui_rems(10.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(relative(card.at, now))),
                    ),
            );
        if matches!(card.kind, CardKind::Draft) && !pending {
            let payload = DraftDrag {
                chat_id: card.chat_id.clone(),
                title: card.title.clone().into(),
            };
            el = el.on_drag(payload, |payload, _, _, cx| {
                cx.stop_propagation();
                let title = payload.title.clone();
                cx.new(|_| DraftGhost { title })
            });
        }
        el.into_any_element()
    }
}

fn theme_border_hover(hover: Hsla) -> Hsla {
    hover.opacity(1.0)
}

impl Render for KanbanPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        self.last_render = std::time::Instant::now();
        let cards = self.cards(cx);
        let running = cards
            .iter()
            .filter(|c| c.column == Column::InProgress)
            .count();
        let waiting = cards
            .iter()
            .filter(|c| matches!(c.column, Column::Waiting | Column::NeedsApproval))
            .count();
        let summary = match (running, waiting) {
            (0, 0) => "Nothing running".to_string(),
            (r, 0) => format!("{r} running"),
            (0, w) => format!("{w} waiting on you"),
            (r, w) => format!("{r} running · {w} waiting on you"),
        };
        let has_space = self.state.read(cx).selected_space.is_some();

        let filters = [
            (KanbanFilter::All, "All"),
            (KanbanFilter::Workspace, "This workspace"),
            (KanbanFilter::CloudOnly, "Cloud"),
            (KanbanFilter::LocalOnly, "Local"),
        ];
        let mut filter_bar = segmented_bar(&theme);
        for (ix, (filter, label)) in filters.into_iter().enumerate() {
            if filter == KanbanFilter::Workspace && !has_space {
                continue;
            }
            filter_bar = filter_bar.child(
                segment(&theme, ("kanban-filter", ix), label, self.filter == filter).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.filter = filter;
                        cx.notify();
                    }),
                ),
            );
        }
        let header = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .px(px(20.0))
            .pt(px(16.0))
            .pb(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(18.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child("Kanban"),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.5))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(summary)),
                    ),
            )
            .child(div().flex_1())
            .child(filter_bar)
            .child(
                popover::btn_primary(&theme, "+ New task")
                    .id("kanban-new-task")
                    .on_click(cx.listener(|this, _, window, cx| this.open_new_task(window, cx))),
            );

        let mut board = div()
            .flex()
            .flex_row()
            .gap(px(12.0))
            .px(px(20.0))
            .pb(px(16.0))
            .h_full()
            .min_w_full();
        let mut ix = 0usize;
        for column in Column::ALL {
            let members: Vec<&KanbanCard> = cards.iter().filter(|c| c.column == column).collect();
            let tone = column.tone(&theme);
            let accepts_drop = column == Column::InProgress;
            let accent = theme.accent;
            let mut body = div()
                .id(SharedString::from(format!("kanban-col-{}", column.label())))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(6.0));
            if members.is_empty() {
                body = body.child(
                    div()
                        .rounded(px(12.0))
                        .border_1()
                        .border_dashed()
                        .border_color(theme.border)
                        .p(px(12.0))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_faint)
                        .child(column.empty_copy()),
                );
            }
            for card in members.iter() {
                body = body.child(self.render_card(card, ix, &theme, cx));
                ix += 1;
            }
            let mut col = div()
                .id(SharedString::from(format!(
                    "kanban-column-{}",
                    column.label()
                )))
                .flex_1()
                .min_w(px(220.0))
                .max_w(px(340.0))
                .h_full()
                .flex()
                .flex_col()
                .rounded(px(14.0))
                .bg(theme.element_hover.opacity(0.25))
                .border_1()
                .border_color(theme.border.opacity(0.6))
                .child(
                    div()
                        .flex_none()
                        .h(px(36.0))
                        .px(px(12.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(7.0))
                        .child(div().size(px(7.0)).rounded_full().bg(tone))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(12.0))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(column.label()),
                        )
                        .child(
                            div()
                                .px(px(6.0))
                                .rounded(px(8.0))
                                .bg(theme.element_hover)
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(members.len().to_string())),
                        ),
                )
                .child(body);
            if accepts_drop {
                col = col
                    .drag_over::<DraftDrag>(move |style, _, _, _| {
                        style.border_color(accent).bg(accent.opacity(0.06))
                    })
                    .on_drop(cx.listener(|this, drag: &DraftDrag, _, cx| {
                        this.start_draft(drag.chat_id.clone(), cx);
                    }));
            }
            board = board.child(col);
        }

        let new_task = self.render_new_task(&theme, cx);
        div()
            .id("kanban-page-root")
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .children(new_task)
            .when_some(self.error.clone(), |el, err| {
                el.child(
                    div()
                        .px(px(20.0))
                        .pb(px(8.0))
                        .child(widgets::error_strip(&theme, err)),
                )
            })
            .child(
                div()
                    .id("kanban-board-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scroll()
                    .child(board),
            )
    }
}

// ---------------------------------------------------------------------------
// Automations
// ---------------------------------------------------------------------------

/// Starter prompts for the empty state — one click prefills the form.
const TEMPLATES: [(&str, &str, usize); 3] = [
    (
        "Morning PR digest",
        "Summarize every open pull request in this workspace's repo: what changed, CI state, and what's blocking each one. Keep it to one short paragraph per PR.",
        1,
    ),
    (
        "Nightly dependency check",
        "Check this project's dependencies for available updates and known vulnerabilities. Update what is safe, run the tests, and report what changed.",
        1,
    ),
    (
        "Hourly uptime check",
        "Open the production site in the browser, take a screenshot, and report anything broken or slow. Only speak up when something is wrong.",
        0,
    ),
];

/// Create/edit form state.
struct AutomationForm {
    /// `Some(id)` when editing an existing automation.
    editing: Option<String>,
    name: Entity<ComposerInput>,
    prompt: Entity<ComposerInput>,
    cron: Entity<ComposerInput>,
    /// 0 = hourly, 1 = daily, 2 = weekly, 3 = cron.
    schedule_kind: usize,
    /// `None` = this computer.
    seat: Option<String>,
    /// Local harness; `None` = the app default.
    harness: Option<HarnessId>,
    /// Local project; `None` = no project (home directory).
    space: Option<String>,
    enabled: bool,
    busy: bool,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

/// `Route::Automations` — scheduled prompts against this computer or a
/// persistent cloud seat.
pub struct AutomationsPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    form: Option<AutomationForm>,
    harnesses: Loadable<Vec<zeron_engine::registry::HarnessDescriptor>>,
    confirm_delete: Option<String>,
    running_now: HashSet<String>,
    error: Option<String>,
    note: Option<String>,
    load_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
    _observe: Subscription,
}

impl AutomationsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        state.update(cx, |state, cx| state.refresh_cloud(None, cx));
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            form: None,
            harnesses: Loadable::Idle,
            confirm_delete: None,
            running_now: HashSet::new(),
            error: None,
            note: None,
            load_task: None,
            action_task: None,
            _observe: observe,
        };
        page.load_harnesses(cx);
        page
    }

    fn load_harnesses(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.harnesses = Loadable::Loading;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::LIST_HARNESSES, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                page.harnesses = match result
                    .map(serde_json::from_value::<Vec<zeron_engine::registry::HarnessDescriptor>>)
                {
                    Ok(Ok(list)) => Loadable::Ready(crate::pickers::offered_harnesses(&list)),
                    Ok(Err(err)) => Loadable::Error(err.to_string()),
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn view<'a>(&self, cx: &'a App) -> Option<&'a CloudView> {
        self.state.read(cx).cloud_view()
    }

    fn open_form(
        &mut self,
        existing: Option<CloudAutomation>,
        template: Option<(&str, &str, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name =
            cx.new(|cx| ComposerInput::new("Name — e.g. Morning PR digest", cx).with_single_line());
        let prompt = cx.new(|cx| ComposerInput::new("What the agent does each time it fires…", cx));
        let cron = cx.new(|cx| {
            ComposerInput::new(
                "Cron — minute hour day month weekday, e.g. */30 * * * *",
                cx,
            )
            .with_single_line()
        });
        let subs = vec![
            cx.observe(&cron, |_, _, cx| cx.notify()),
            cx.subscribe(&name, |this, _, event, cx| {
                if let ComposerInputEvent::Submitted = event {
                    this.save(cx);
                }
            }),
        ];
        let mut form = AutomationForm {
            editing: None,
            name,
            prompt,
            cron,
            schedule_kind: 1,
            seat: None,
            harness: None,
            space: self.state.read(cx).selected_space.clone(),
            enabled: true,
            busy: false,
            error: None,
            _subs: subs,
        };
        if let Some(a) = existing {
            form.name.update(cx, |i, cx| i.set_text(a.name.clone(), cx));
            form.prompt
                .update(cx, |i, cx| i.set_text(a.prompt.clone(), cx));
            form.schedule_kind = match a.schedule.as_str() {
                "hourly" => 0,
                "daily" => 1,
                "weekly" => 2,
                cron => {
                    form.cron
                        .update(cx, |i, cx| i.set_text(cron.to_string(), cx));
                    3
                }
            };
            form.seat = a.seat_id.clone();
            form.harness = a.harness;
            form.space = a.space_id.clone();
            form.enabled = a.enabled;
            form.editing = Some(a.id);
        } else if let Some((name, prompt, kind)) = template {
            form.name
                .update(cx, |i, cx| i.set_text(name.to_string(), cx));
            form.prompt
                .update(cx, |i, cx| i.set_text(prompt.to_string(), cx));
            form.schedule_kind = kind;
        }
        let focus = form.name.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        self.form = Some(form);
        self.note = None;
        cx.notify();
    }

    fn form_schedule(form: &AutomationForm, cx: &App) -> String {
        match form.schedule_kind {
            0 => "hourly".into(),
            1 => "daily".into(),
            2 => "weekly".into(),
            _ => form.cron.read(cx).text().trim().to_string(),
        }
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let existing: Option<CloudAutomation> = {
            let editing = self.form.as_ref().and_then(|f| f.editing.clone());
            editing.and_then(|id| {
                self.view(cx)
                    .and_then(|v| v.automations.iter().find(|a| a.id == id).cloned())
            })
        };
        let Some(form) = self.form.as_mut() else {
            return;
        };
        if form.busy {
            return;
        }
        let name = form.name.read(cx).text().trim().to_string();
        let prompt = form.prompt.read(cx).text().trim().to_string();
        let schedule = Self::form_schedule(form, cx);
        let problem = if name.is_empty() {
            Some("Give the automation a name".to_string())
        } else if prompt.is_empty() {
            Some("Write the prompt it runs".to_string())
        } else if zeron_engine::cloud::next_run(&schedule, Utc::now()).is_none() {
            Some(format!(
                "`{schedule}` isn't a schedule — use 5 cron fields like */30 * * * *"
            ))
        } else {
            None
        };
        if let Some(problem) = problem {
            form.error = Some(problem);
            cx.notify();
            return;
        }
        form.busy = true;
        form.error = None;
        let is_local = form.seat.is_none();
        let automation = serde_json::json!({
            "id": form
                .editing
                .clone()
                .unwrap_or_else(|| format!("auto-{}", uuid::Uuid::new_v4().simple())),
            "name": name,
            "prompt": prompt,
            "schedule": schedule,
            "seatId": form.seat,
            "spaceId": if is_local { form.space.clone() } else { None },
            "harness": if is_local { form.harness } else { None },
            "enabled": form.enabled,
            // Keep history on edits; the engine re-stamps next_run_at.
            "lastRunAt": existing.as_ref().and_then(|a| a.last_run_at),
            "lastStatus": existing.as_ref().and_then(|a| a.last_status.clone()),
            "lastChatId": existing.as_ref().and_then(|a| a.last_chat_id.clone()),
        });
        let editing = form.editing.is_some();
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_AUTOMATION_SAVE,
                    serde_json::json!({ "automation": automation }),
                )
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(_) => {
                        page.form = None;
                        page.note = Some(if editing {
                            "Automation updated".into()
                        } else {
                            "Automation created — it fires on its schedule, or Run now".into()
                        });
                    }
                    Err(err) => {
                        if let Some(form) = page.form.as_mut() {
                            form.busy = false;
                            form.error = Some(err.to_string());
                        }
                    }
                }
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn toggle(&mut self, automation: CloudAutomation, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let mut next = automation;
        next.enabled = !next.enabled;
        // Re-enabling re-stamps the next fire from now.
        next.next_run_at = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_AUTOMATION_SAVE,
                    serde_json::json!({ "automation": next }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.error = result.err().map(|err| err.to_string());
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            })
            .ok();
        }));
    }

    fn run_now(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.running_now.insert(id.clone());
        cx.notify();
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_AUTOMATION_RUN_NOW,
                    serde_json::json!({ "id": id }),
                )
                .await;
            // The scheduler fires within a moment; give it a beat before the
            // refresh so the row shows the new chat.
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
            this.update(cx, |page, cx| {
                page.running_now.remove(&id);
                match result {
                    Ok(_) => {
                        page.note = Some("Fired — the run's chat is on the Kanban board".into())
                    }
                    Err(err) => page.error = Some(err.to_string()),
                }
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            })
            .ok();
        }));
    }

    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        if self.confirm_delete.as_deref() != Some(id.as_str()) {
            self.confirm_delete = Some(id);
            cx.notify();
            return;
        }
        self.confirm_delete = None;
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_AUTOMATION_REMOVE,
                    serde_json::json!({ "id": id }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.error = result.err().map(|err| err.to_string());
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            })
            .ok();
        }));
    }

    fn automation_card(
        &self,
        automation: &CloudAutomation,
        ix: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = Utc::now();
        let view = self.view(cx);
        let seat = automation
            .seat_id
            .as_deref()
            .and_then(|id| view.and_then(|v| v.seat(id)));
        let (target_icon, target) = match seat {
            Some(seat) => (
                seat_chip_icon(seat.config.kind),
                format!(
                    "{} · {}",
                    seat_chip_label(seat.config.kind),
                    seat.config.label
                ),
            ),
            None if automation.seat_id.is_some() => {
                (icons::DANGER_TRIANGLE, "Seat removed".to_string())
            }
            None => (icons::LAPTOP, "This computer".to_string()),
        };
        let space_name = automation.space_id.as_deref().and_then(|id| {
            self.state
                .read(cx)
                .space_row(id)
                .map(|space| space.display_name().to_string())
        });
        let (last_text, last_color): (String, Hsla) =
            match (automation.last_status.as_deref(), automation.last_run_at) {
                (Some("ok"), Some(at)) => (format!("Ran {}", relative(at, now)), theme.success),
                (Some("missed"), Some(at)) => (
                    format!("Missed {} — seat unreachable", relative(at, now)),
                    theme.danger,
                ),
                (Some(status), Some(at)) if status.starts_with("error:") => (
                    format!("Failed {} — {}", relative(at, now), &status[6..]),
                    theme.danger,
                ),
                _ => ("Hasn't run yet".into(), theme.text_faint),
            };
        let next_text = if !automation.enabled {
            "Paused".to_string()
        } else {
            automation
                .next_run_at
                .map(|at| format!("Next {}", until(at, now)))
                .unwrap_or_else(|| "Next: scheduling…".into())
        };
        let id = automation.id.clone();
        let row = automation.clone();
        let edit_row = automation.clone();
        let last_chat = automation.last_chat_id.clone();
        let confirming = self.confirm_delete.as_deref() == Some(id.as_str());
        let running = self.running_now.contains(&id);
        let hover = theme.element_hover;
        let harness_mark = automation
            .harness
            .filter(|_| automation.seat_id.is_none())
            .map(crate::pickers::harness_brand_icon)
            .or_else(|| {
                seat.and_then(|s| s.config.harness_command.as_deref())
                    .and_then(command_mark)
            });
        div()
            .id(("automation-card", ix))
            .w_full()
            .rounded(px(12.0))
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface_card)
            .p(px(14.0))
            .flex()
            .flex_row()
            .items_start()
            .gap(px(12.0))
            .when(!automation.enabled, |el| el.opacity(0.7))
            .child(
                div()
                    .flex_none()
                    .size(px(32.0))
                    .rounded(px(9.0))
                    .bg(theme.element_hover)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(icons::CLOCK_CIRCLE).size(px(15.0)).text_color(
                        if automation.enabled {
                            theme.accent
                        } else {
                            theme.text_muted
                        },
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(13.0))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child(SharedString::from(automation.name.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(describe_schedule(
                                        &automation.schedule,
                                    ))),
                            ),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.5))
                            .text_color(theme.text_muted)
                            .line_clamp(2)
                            .child(SharedString::from(automation.prompt.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.0))
                            .child(chip(theme, Some(target_icon), target))
                            .when_some(space_name, |el, name| {
                                el.child(chip(theme, Some(icons::FOLDER), name))
                            })
                            .when_some(harness_mark, |el, (path, tint)| {
                                el.child(
                                    icon(path)
                                        .size(px(12.0))
                                        .text_color(tint.unwrap_or(theme.text_muted)),
                                )
                            })
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .text_color(theme.text_faint)
                                    .child(SharedString::from(next_text)),
                            )
                            .child(
                                div()
                                    .id(("automation-last", ix))
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .text_color(last_color)
                                    .when(last_chat.is_some(), |el| {
                                        el.cursor_pointer().hover(|s| s.underline())
                                    })
                                    .child(SharedString::from(last_text))
                                    .when_some(last_chat, |el, chat| {
                                        el.on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(BoardsEvent::OpenChat(chat.clone()))
                                        }))
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .child({
                        let run_id = id.clone();
                        popover::btn_ghost(
                            theme,
                            if running { "Firing…" } else { "Run now" },
                            format!("auto-run-{id}"),
                        )
                        .id(SharedString::from(format!("auto-run-{id}")))
                        .when(running, |el| el.opacity(0.5))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.running_now.contains(&run_id) {
                                this.run_now(run_id.clone(), cx)
                            }
                        }))
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("auto-edit-{id}")))
                            .size(px(26.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(7.0))
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .tooltip(widgets::text_tooltip("Edit"))
                            .child(icon(icons::PEN).size(px(13.0)).text_color(theme.text_muted))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_form(Some(edit_row.clone()), None, window, cx)
                            })),
                    )
                    .child({
                        let del_id = id.clone();
                        div()
                            .id(SharedString::from(format!("auto-del-{id}")))
                            .h(px(26.0))
                            .min_w(px(26.0))
                            .px(px(if confirming { 8.0 } else { 0.0 }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(7.0))
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .tooltip(widgets::text_tooltip("Delete"))
                            .when(confirming, |el| {
                                el.bg(theme.danger.opacity(0.12))
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.danger)
                                    .child("Delete?")
                            })
                            .when(!confirming, |el| {
                                el.child(
                                    icon(icons::TRASH_BIN_MINIMALISTIC)
                                        .size(px(13.0))
                                        .text_color(theme.text_muted),
                                )
                            })
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.remove(del_id.clone(), cx)),
                            )
                    })
                    .child(
                        widgets::toggle_switch(theme, automation.enabled, format!("auto-{id}"))
                            .id(SharedString::from(format!("auto-toggle-{id}")))
                            .tooltip(widgets::text_tooltip(if automation.enabled {
                                "Pause"
                            } else {
                                "Resume"
                            }))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle(row.clone(), cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_form(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let form = self.form.as_ref()?;
        let label = |text: &'static str| {
            div()
                .text_size(crate::typography::ui_rems(11.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(text)
        };
        let now = Utc::now();
        let schedule = Self::form_schedule(form, cx);
        let preview: (SharedString, Hsla) = match zeron_engine::cloud::next_run(&schedule, now) {
            Some(at) => (
                format!(
                    "{} · first run {} ({})",
                    describe_schedule(&schedule),
                    until(at, now),
                    at.with_timezone(&chrono::Local).format("%a %H:%M")
                )
                .into(),
                theme.text_muted,
            ),
            None if schedule.is_empty() => ("Type a cron expression".into(), theme.text_faint),
            None => ("Not a valid schedule yet".into(), theme.danger),
        };
        let mut kinds = segmented_bar(theme);
        for (ix, name) in ["Hourly", "Daily", "Weekly", "Custom"]
            .into_iter()
            .enumerate()
        {
            kinds = kinds.child(
                segment(theme, ("auto-sched", ix), name, form.schedule_kind == ix).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if let Some(form) = this.form.as_mut() {
                            form.schedule_kind = ix;
                        }
                        cx.notify();
                    }),
                ),
            );
        }
        // Targets: this computer + persistent seats. Railway trials are shown
        // disabled — they die at the hour, so an automation can't live there.
        let seats: Vec<SeatRow> = self.view(cx).map(|v| v.seats.clone()).unwrap_or_default();
        let mut targets = div().flex().flex_row().flex_wrap().gap(px(4.0)).child(
            segment(
                theme,
                "auto-target-local",
                "This computer",
                form.seat.is_none(),
            )
            .child(
                icon(icons::LAPTOP)
                    .size(px(11.0))
                    .text_color(theme.text_muted),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(form) = this.form.as_mut() {
                    form.seat = None;
                }
                cx.notify();
            })),
        );
        for (ix, seat) in seats.iter().enumerate() {
            let trial = seat.config.kind == Some(CloudSeatKind::Railway);
            let usable = !trial && CloudView::seat_runnable(seat);
            let id = seat.config.id.clone();
            let active = form.seat.as_deref() == Some(id.as_str());
            targets = targets.child(
                segment(
                    theme,
                    ("auto-target-seat", ix),
                    format!(
                        "{} · {}",
                        seat_chip_label(seat.config.kind),
                        seat.config.label
                    ),
                    active,
                )
                .child(
                    icon(seat_chip_icon(seat.config.kind))
                        .size(px(11.0))
                        .text_color(theme.text_muted),
                )
                .when(!usable, |el| {
                    el.opacity(0.4).tooltip(widgets::text_tooltip(if trial {
                        "Trial seats die at the hour — automations need Boat or a VPS"
                    } else {
                        "This seat isn't ready — finish it in Settings → Cloud computer"
                    }))
                })
                .when(usable, |el| {
                    el.on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(form) = this.form.as_mut() {
                            form.seat = Some(id.clone());
                        }
                        cx.notify();
                    }))
                }),
            );
        }
        let local = form.seat.is_none();
        let harness_row = match (&self.harnesses, local) {
            (Loadable::Ready(list), true) if !list.is_empty() => {
                let mut row = div().flex().flex_row().flex_wrap().gap(px(4.0)).child(
                    segment(
                        theme,
                        "auto-harness-default",
                        "Default",
                        form.harness.is_none(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(form) = this.form.as_mut() {
                            form.harness = None;
                        }
                        cx.notify();
                    })),
                );
                for (ix, harness) in list.iter().enumerate() {
                    let id = harness.id;
                    let (mark, tint) = crate::pickers::harness_brand_icon(id);
                    row = row.child(
                        segment(
                            theme,
                            ("auto-harness", ix),
                            harness.name.clone(),
                            form.harness == Some(id),
                        )
                        .child(
                            icon(mark)
                                .size(px(11.0))
                                .text_color(tint.unwrap_or(theme.text_muted)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(form) = this.form.as_mut() {
                                form.harness = Some(id);
                            }
                            cx.notify();
                        })),
                    );
                }
                Some(row)
            }
            _ => None,
        };
        let spaces = self.state.read(cx).spaces_sorted();
        let space_row = local.then(|| {
            let mut row = div().flex().flex_row().flex_wrap().gap(px(4.0)).child(
                segment(theme, "auto-space-none", "No project", form.space.is_none()).on_click(
                    cx.listener(|this, _, _, cx| {
                        if let Some(form) = this.form.as_mut() {
                            form.space = None;
                        }
                        cx.notify();
                    }),
                ),
            );
            for (ix, space) in spaces.iter().take(12).enumerate() {
                let id = space.id.clone();
                row = row.child(
                    segment(
                        theme,
                        ("auto-space", ix),
                        space.display_name().to_string(),
                        form.space.as_deref() == Some(space.id.as_str()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(form) = this.form.as_mut() {
                            form.space = Some(id.clone());
                        }
                        cx.notify();
                    })),
                );
            }
            row
        });
        let title = if form.editing.is_some() {
            "Edit automation"
        } else {
            "New automation"
        };
        Some(
            div()
                .w_full()
                .rounded(px(14.0))
                .border_1()
                .border_color(theme.accent.opacity(0.35))
                .bg(theme.surface_card)
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(14.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(title),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(label("Name"))
                        .child(popover::dialog_field(form.name.clone().into_any_element())),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(5.0))
                        .child(label("Prompt"))
                        .child(popover::dialog_field(
                            form.prompt.clone().into_any_element(),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(label("Schedule"))
                        .child(kinds)
                        .when(form.schedule_kind == 3, |el| {
                            el.child(popover::dialog_field(form.cron.clone().into_any_element()))
                        })
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(preview.1)
                                .child(preview.0),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(label("Runs on"))
                        .child(targets),
                )
                .when_some(space_row, |el, row| {
                    el.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(label("Project"))
                            .child(row),
                    )
                })
                .when_some(harness_row, |el, row| {
                    el.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(label("Agent"))
                            .child(row),
                    )
                })
                .when_some(form.error.clone(), |el, err| {
                    el.child(widgets::error_strip(theme, err))
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            popover::btn_ghost(theme, "Cancel", "auto-cancel")
                                .id("auto-cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.form = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            popover::btn_primary(
                                theme,
                                match (form.busy, form.editing.is_some()) {
                                    (true, _) => "Saving…",
                                    (false, true) => "Save changes",
                                    (false, false) => "Create automation",
                                },
                            )
                            .id("auto-save")
                            .when(form.busy, |el| el.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_empty(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let hover = theme.element_hover;
        let mut templates = div().flex().flex_col().gap(px(6.0)).w_full();
        for (ix, template) in TEMPLATES.into_iter().enumerate() {
            let (name, prompt, _) = template;
            templates = templates.child(
                div()
                    .id(("auto-template", ix))
                    .w_full()
                    .px(px(12.0))
                    .py(px(9.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(prompt),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_form(None, Some(template), window, cx)
                    })),
            );
        }
        div()
            .w_full()
            .rounded(px(14.0))
            .border_1()
            .border_dashed()
            .border_color(theme.border)
            .p(px(20.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(10.0))
            .child(
                icon(icons::CLOCK_CIRCLE)
                    .size(px(22.0))
                    .text_color(theme.text_muted.opacity(0.6)),
            )
            .child(
                div()
                    .text_size(crate::typography::ui_rems(13.5))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child("No automations yet"),
            )
            .child(
                div()
                    .max_w(px(420.0))
                    .text_center()
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child(
                        "A prompt on a schedule — on this computer, or on a Boat / VPS seat that \
                         keeps running while this app is closed. Start from one of these:",
                    ),
            )
            .child(templates)
            .into_any_element()
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for AutomationsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for AutomationsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let scrollbar = popover::rail(self, "automations-scrollbar", &theme, cx);
        let automations = self
            .view(cx)
            .map(|v| v.automations.clone())
            .unwrap_or_default();
        let active = automations.iter().filter(|a| a.enabled).count();
        let mut column = widgets::page_column().gap(px(14.0)).child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(widgets::page_header(&theme, "Automations", None))
                        .child(widgets::page_subtitle(
                            &theme,
                            if automations.is_empty() {
                                "Prompts that run on a schedule.".to_string()
                            } else {
                                format!(
                                    "{} automation{} · {active} active · missed fires show on the Kanban board",
                                    automations.len(),
                                    if automations.len() == 1 { "" } else { "s" }
                                )
                            },
                        )),
                )
                .when(self.form.is_none(), |el| {
                    el.child(
                        popover::btn_primary(&theme, "+ New automation")
                            .id("auto-new")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_form(None, None, window, cx)
                            })),
                    )
                }),
        );
        if let Some(note) = self.note.clone() {
            column = column.child(
                div()
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(px(10.0))
                    .bg(theme.success.opacity(0.08))
                    .text_size(crate::typography::ui_rems(11.5))
                    .text_color(theme.success)
                    .child(SharedString::from(note)),
            );
        }
        if let Some(error) = self.error.clone() {
            column = column.child(widgets::error_strip(&theme, error));
        }
        if let Some(form) = self.render_form(&theme, cx) {
            column = column.child(form);
        }
        if automations.is_empty() && self.form.is_none() {
            column = column.child(self.render_empty(&theme, cx));
        }
        for (ix, automation) in automations.iter().enumerate() {
            column = column.child(self.automation_card(automation, ix, &theme, cx));
        }
        div()
            .id("automations-page-root")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div()
                    .id("automations-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .child(column),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedules_read_as_words() {
        assert_eq!(describe_schedule("hourly"), "Every hour");
        assert!(describe_schedule("daily").starts_with("Every day at 09:00 UTC"));
        assert_eq!(describe_schedule("*/30 * * * *"), "Every 30 minutes");
        assert_eq!(describe_schedule("0 */6 * * *"), "Every 6 hours");
        assert_eq!(describe_schedule("5 14 * * *"), "Every day at 14:05 UTC");
        assert_eq!(describe_schedule("0 9 * * 1-5"), "Weekdays at 09:00 UTC");
        assert_eq!(describe_schedule("0 9 1 * *"), "Cron · 0 9 1 * *");
    }

    #[test]
    fn relative_times_are_compact() {
        let now = Utc::now();
        assert_eq!(relative(now, now), "just now");
        assert_eq!(relative(now - chrono::Duration::minutes(5), now), "5m ago");
        assert_eq!(relative(now - chrono::Duration::hours(3), now), "3h ago");
        assert_eq!(relative(now - chrono::Duration::days(4), now), "4d ago");
        assert_eq!(until(now + chrono::Duration::minutes(10), now), "in 10m");
        assert_eq!(until(now, now), "now");
    }

    #[test]
    fn columns_follow_the_spec() {
        assert_eq!(Column::of(CloudRunStatus::Queued), Column::Draft);
        assert_eq!(Column::of(CloudRunStatus::Stopped), Column::Waiting);
        assert_eq!(Column::of(CloudRunStatus::Error), Column::Done);
        assert_eq!(
            command_mark("opencode run -m x").map(|m| m.0),
            Some(icons::OPENCODE_MARK)
        );
        assert!(command_mark("my-agent").is_none());
    }
}
