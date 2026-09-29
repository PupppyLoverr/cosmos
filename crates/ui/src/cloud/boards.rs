//! Automations and Kanban — the two always-visible sidebar destinations for
//! cloud + local agent runs.
//!
//! Kanban cards derive from the cloud run ledger (`cloud-runs.json`) plus the
//! local session rows the engine already syncs — a card is a real run, never
//! a decoration. Automations persist next to the seat config and are fired by
//! the engine's scheduler task; a fire on an unreachable seat lands as
//! `Missed` here and on the board.

use gpui::{
    AnyElement, App, Context, Entity, IntoElement, Render, SharedString, Task, Window, div,
    prelude::*, px,
};
use zeron_proto::{Chat, CloudAutomation, CloudRunStatus, SessionStatus};
use zeron_rpc::methods;

use crate::cloud::{CloudView, SeatRow};
use crate::composer::ComposerInput;
use crate::icons;
use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// Click-through: the shell opens the chat a card belongs to.
pub enum BoardsEvent {
    OpenChat(String),
}

fn harness_mark(chat: Option<&Chat>) -> &'static str {
    chat.and_then(|c| c.config.as_ref())
        .map(|cfg| crate::settings::harnesses::cli_name(cfg.harness))
        .unwrap_or("agent")
}

impl gpui::EventEmitter<BoardsEvent> for KanbanPage {}
impl gpui::EventEmitter<BoardsEvent> for AutomationsPage {}

// ---------------------------------------------------------------------------
// Kanban
// ---------------------------------------------------------------------------

/// Which runs the board shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KanbanFilter {
    Workspace,
    CloudOnly,
    LocalOnly,
}

struct KanbanCard {
    chat_id: String,
    title: String,
    status: CloudRunStatus,
    seat: SharedString,
    harness: SharedString,
    elapsed: Option<String>,
}

/// `Route::Kanban` — one column per `CloudRunStatus`, cards for local and
/// cloud runs, live via the session watch already running on AppState.
pub struct KanbanPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    view: Loadable<CloudView>,
    filter: KanbanFilter,
    load_task: Option<Task<()>>,
}

impl KanbanPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            view: Loadable::Idle,
            filter: KanbanFilter::Workspace,
            load_task: None,
        };
        page.load(cx);
        page
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.view = Loadable::Error("Engine not connected".into());
            return;
        };
        if !matches!(self.view, Loadable::Ready(_)) {
            self.view = Loadable::Loading;
        }
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::CLOUD_GET_CONFIG, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                page.view = match result {
                    Ok(value) => match serde_json::from_value::<CloudView>(value) {
                        Ok(view) => Loadable::Ready(view),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// Fold local session rows and the cloud run ledger into board cards.
    fn cards(&self, cx: &App) -> Vec<KanbanCard> {
        let state = self.state.read(cx);
        let now = chrono::Utc::now();
        let mut cards = Vec::new();
        // Cloud ledger rows — authoritative for seat runs.
        if self.filter != KanbanFilter::LocalOnly
            && let Some(view) = self.view.ready()
        {
            for run in &view.runs {
                let chat = state.chat_row(&run.chat_id);
                cards.push(KanbanCard {
                    chat_id: run.chat_id.clone(),
                    title: chat
                        .and_then(|c| c.title.clone())
                        .unwrap_or_else(|| run.id.clone()),
                    status: run.status,
                    // Seat chip: provider mark + seat label — `Railway ·
                    // trial`, `Boat · box`, `Local` for unbound sessions.
                    seat: view
                        .seat(&run.seat_id)
                        .map(|s| match s.config.kind {
                            Some(kind) => format!("{} · {}", kind.chip(), s.config.label),
                            None => s.config.label.clone(),
                        })
                        .unwrap_or_else(|| run.seat_id.clone())
                        .into(),
                    harness: harness_mark(chat).into(),
                    elapsed: Some(format_elapsed(run.started_at, run.finished_at, now)),
                });
            }
        }
        // Drafts — chats bound to a seat that were never run. They have no
        // ledger row yet, and the local pass below skips bound chats, so
        // without this they'd be invisible on the board. Clicking opens the
        // session; sending the first message starts the run (Draft →
        // In progress).
        if self.filter != KanbanFilter::LocalOnly
            && let Some(view) = self.view.ready()
        {
            for chat in &state.chats {
                if chat.archived || cloud_bound_has_run(&view.runs, &chat.id) {
                    continue;
                }
                let Some(binding) = chat.cloud_seat.as_ref() else {
                    continue;
                };
                let seat = view
                    .seat(&binding.seat_id)
                    .map(|s| match s.config.kind {
                        Some(kind) => format!("{} · {}", kind.chip(), s.config.label),
                        None => s.config.label.clone(),
                    })
                    .unwrap_or_else(|| binding.label.clone());
                cards.push(KanbanCard {
                    chat_id: chat.id.clone(),
                    title: chat.title.clone().unwrap_or_else(|| "Draft".into()),
                    status: CloudRunStatus::Queued,
                    seat: seat.into(),
                    harness: harness_mark(Some(chat)).into(),
                    elapsed: None,
                });
            }
        }
        // Local sessions — `Session` rows whose chat has no cloud binding.
        if self.filter != KanbanFilter::CloudOnly {
            let cloud_chats: std::collections::HashSet<&str> = self
                .view
                .ready()
                .map(|v| v.runs.iter().map(|r| r.chat_id.as_str()).collect())
                .unwrap_or_default();
            for session in &state.sessions {
                if cloud_chats.contains(session.chat_id.as_str()) {
                    continue;
                }
                let Some(chat) = state.chat_row(&session.chat_id) else {
                    continue;
                };
                if chat.cloud_seat.is_some() {
                    continue;
                }
                if chat.archived {
                    continue;
                }
                let status = match session.status {
                    SessionStatus::Working => CloudRunStatus::Working,
                    SessionStatus::AwaitingInput => CloudRunStatus::WaitingForUser,
                    SessionStatus::Errored => CloudRunStatus::Error,
                    SessionStatus::Idle => CloudRunStatus::Done,
                };
                // The board shows work, not resting chats — an Idle session
                // only counts while it was touched today.
                if matches!(status, CloudRunStatus::Done) {
                    continue;
                }
                cards.push(KanbanCard {
                    chat_id: session.chat_id.clone(),
                    title: chat.title.clone().unwrap_or_else(|| "Session".into()),
                    status,
                    seat: "This computer".into(),
                    harness: harness_mark(Some(chat)).into(),
                    elapsed: session
                        .started_at
                        .map(|start| format_elapsed(start, None, now)),
                });
            }
        }
        cards
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

fn cloud_bound_has_run(runs: &[zeron_proto::CloudRun], chat_id: &str) -> bool {
    runs.iter().any(|r| r.chat_id == chat_id)
}

fn format_elapsed(
    started: chrono::DateTime<chrono::Utc>,
    finished: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let end = finished.unwrap_or(now);
    let secs = (end - started).num_seconds().max(0);
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

impl popover::ScrollRailHost for KanbanPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for KanbanPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let scrollbar = popover::rail(self, "kanban-scrollbar", &theme, cx);
        let cards = self.cards(cx);
        // The spec's five columns: Draft | In progress | Waiting | Needs
        // approval | Done — Error rides Done as a red card, Stopped rides
        // Waiting (a paused run awaits its user).
        const BOARD_COLUMNS: [(&str, &[CloudRunStatus]); 5] = [
            ("Draft", &[CloudRunStatus::Queued]),
            ("In progress", &[CloudRunStatus::Working]),
            (
                "Waiting",
                &[CloudRunStatus::WaitingForUser, CloudRunStatus::Stopped],
            ),
            ("Needs approval", &[CloudRunStatus::NeedsApproval]),
            ("Done", &[CloudRunStatus::Done, CloudRunStatus::Error]),
        ];
        let mut columns = div()
            .flex()
            .flex_row()
            .gap(px(10.0))
            .p(px(16.0))
            .flex_1()
            .min_w_0();
        for (label, members) in BOARD_COLUMNS {
            let mut col = div()
                .w(px(220.0))
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(6.0));
            let count = cards.iter().filter(|c| members.contains(&c.status)).count();
            col = col.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .pb(px(4.0))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_muted)
                            .child(SharedString::from(label)),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(10.5))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(count.to_string())),
                    ),
            );
            for (ix, card) in cards
                .iter()
                .enumerate()
                .filter(|(_, c)| members.contains(&c.status))
            {
                let chat_id = card.chat_id.clone();
                let errored = card.status == CloudRunStatus::Error;
                let fade = SharedString::from(format!("kanban-card-{ix}"));
                col = col.child(
                    div()
                        .id(("kanban-card", ix))
                        .w_full()
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(if errored { theme.danger } else { theme.border })
                        .bg(if errored {
                            theme.danger.opacity(0.08)
                        } else {
                            theme.surface_card
                        })
                        .p(px(8.0))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .cursor_pointer()
                        .on_hover(crate::motion::hover_listener(fade.clone()))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(BoardsEvent::OpenChat(chat_id.clone()));
                        }))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(12.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(SharedString::from(card.title.clone())),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .text_size(crate::typography::ui_rems(10.0))
                                        .text_color(theme.text_faint)
                                        .child(SharedString::from(format!(
                                            "{} · {}",
                                            card.harness, card.seat
                                        ))),
                                )
                                .when_some(card.elapsed.clone(), |el, elapsed| {
                                    el.child(
                                        div()
                                            .text_size(crate::typography::ui_rems(10.0))
                                            .text_color(theme.text_faint)
                                            .child(SharedString::from(elapsed)),
                                    )
                                }),
                        ),
                );
            }
            columns = columns.child(col);
        }
        let filter_row = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(16.0))
            .pt(px(12.0))
            .child(widgets::page_header(&theme, "Kanban", None))
            .child(div().flex_1())
            .children(
                [
                    (KanbanFilter::Workspace, "This workspace"),
                    (KanbanFilter::CloudOnly, "Cloud only"),
                    (KanbanFilter::LocalOnly, "Local only"),
                ]
                .into_iter()
                .enumerate()
                .map(|(ix, (filter, label))| {
                    let active = self.filter == filter;
                    div()
                        .id(("kanban-filter", ix))
                        .px(px(10.0))
                        .py(px(5.0))
                        .rounded(px(7.0))
                        .text_size(crate::typography::ui_rems(11.5))
                        .cursor_pointer()
                        .text_color(if active { theme.text } else { theme.text_muted })
                        .bg(if active {
                            theme.element_hover
                        } else {
                            gpui::transparent_black()
                        })
                        .child(SharedString::from(label))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.filter = filter;
                            cx.notify();
                        }))
                }),
            );
        div()
            .id("kanban-page-root")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div().size_full().flex().flex_col().child(filter_row).child(
                    div()
                        .id("kanban-columns")
                        .flex_1()
                        .min_h_0()
                        .overflow_x_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(columns),
                ),
            )
            .children(scrollbar)
    }
}

// ---------------------------------------------------------------------------
// Automations
// ---------------------------------------------------------------------------

/// `Route::Automations` — the cron list plus a create form. A fire on an
/// unreachable seat marks the row `missed` and surfaces on Kanban.
pub struct AutomationsPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    view: Loadable<CloudView>,
    creating: bool,
    name: Entity<ComposerInput>,
    prompt: Entity<ComposerInput>,
    cron: Entity<ComposerInput>,
    /// 0 = hourly, 1 = daily, 2 = weekly, 3 = cron string.
    schedule_kind: usize,
    /// Selected seat id; `None` = this computer.
    seat: Option<String>,
    busy: bool,
    error: Option<String>,
    load_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
}

impl AutomationsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            view: Loadable::Idle,
            creating: false,
            name: cx.new(|cx| ComposerInput::new("Automation name", cx)),
            prompt: cx.new(|cx| ComposerInput::new("Prompt the run starts with…", cx)),
            cron: cx.new(|cx| ComposerInput::new("Cron — e.g. */30 * * * *", cx)),
            schedule_kind: 0,
            seat: None,
            busy: false,
            error: None,
            load_task: None,
            action_task: None,
        };
        page.load(cx);
        page
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.view = Loadable::Error("Engine not connected".into());
            return;
        };
        if !matches!(self.view, Loadable::Ready(_)) {
            self.view = Loadable::Loading;
        }
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::CLOUD_GET_CONFIG, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                page.view = match result {
                    Ok(value) => match serde_json::from_value::<CloudView>(value) {
                        Ok(view) => Loadable::Ready(view),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn schedule(&self, cx: &App) -> String {
        match self.schedule_kind {
            0 => "hourly".into(),
            1 => "daily".into(),
            2 => "weekly".into(),
            _ => self.cron.read(cx).text().trim().to_string(),
        }
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let name = self.name.read(cx).text().trim().to_string();
        let prompt = self.prompt.read(cx).text().trim().to_string();
        if name.is_empty() || prompt.is_empty() {
            self.error = Some("Name and prompt are required".into());
            cx.notify();
            return;
        }
        let schedule = self.schedule(cx);
        if schedule.is_empty() {
            self.error = Some("Cron string is empty".into());
            cx.notify();
            return;
        }
        let seat_id = self.seat.clone();
        self.busy = true;
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_AUTOMATION_SAVE,
                    serde_json::json!({
                        "automation": {
                            "id": format!("auto-{}", uuid::Uuid::new_v4().simple()),
                            "name": name,
                            "prompt": prompt,
                            "schedule": schedule,
                            "seatId": seat_id,
                            "enabled": true,
                        }
                    }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match result {
                    Ok(_) => {
                        page.creating = false;
                        page.name.update(cx, |i, cx| i.set_text("", cx));
                        page.prompt.update(cx, |i, cx| i.set_text("", cx));
                    }
                    Err(err) => page.error = Some(err.to_string()),
                }
                page.load(cx);
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            })
            .ok();
        }));
    }

    fn toggle(&mut self, automation: CloudAutomation, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_AUTOMATION_SAVE,
                    serde_json::json!({
                        "automation": {
                            "id": automation.id,
                            "name": automation.name,
                            "prompt": automation.prompt,
                            "schedule": automation.schedule,
                            "seatId": automation.seat_id,
                            "spaceId": automation.space_id,
                            "enabled": !automation.enabled,
                        }
                    }),
                )
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(_) => {
                        page.error = None;
                        page.load(cx);
                        page.state
                            .update(cx, |state, cx| state.refresh_cloud(None, cx));
                    }
                    Err(err) => page.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
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
                match result {
                    Ok(_) => {
                        page.error = None;
                        page.load(cx);
                        page.state
                            .update(cx, |state, cx| state.refresh_cloud(None, cx));
                    }
                    Err(err) => page.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn automation_row(
        &self,
        automation: &CloudAutomation,
        first: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = automation.id.clone();
        let seat = automation
            .seat_id
            .as_deref()
            .and_then(|id| self.view.ready().and_then(|v| v.seat(id)))
            .map(|s| s.config.label.clone())
            .unwrap_or_else(|| "This computer".to_string());
        let meta = format!(
            "{} · {}{}",
            automation.schedule,
            seat,
            automation
                .last_status
                .as_deref()
                .map(|s| format!(" · last: {s}"))
                .unwrap_or_default(),
        );
        let enabled = automation.enabled;
        let row = automation.clone();
        widgets::card_row(theme, first)
            .child(widgets::row_tile(theme, icons::CLOCK_CIRCLE))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(widgets::row_title(theme, automation.name.clone()))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(meta)),
                    ),
            )
            .child(
                widgets::toggle_switch(theme, enabled, format!("auto-{id}"))
                    .id(SharedString::from(format!("auto-toggle-{id}")))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(row.clone(), cx))),
            )
            .child(
                popover::btn_ghost(theme, "Delete", format!("auto-del-{id}"))
                    .id(SharedString::from(format!("auto-del-{id}")))
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(id.clone(), cx))),
            )
            .into_any_element()
    }

    fn create_card(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        // Automations never target Railway trial seats — they die at the
        // hour. Local + Boat + VPS are the honest choices.
        let seats: Vec<SeatRow> = self
            .view
            .ready()
            .map(|v| {
                v.seats
                    .iter()
                    .filter(|s| !matches!(s.config.kind, Some(zeron_proto::CloudSeatKind::Railway)))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let mut card = widgets::section_card(theme);
        card = card.child(
            widgets::card_row(theme, true).child(widgets::row_title(theme, "New automation")),
        );
        card = card.child(
            div()
                .px(px(20.0))
                .pb(px(14.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(popover::dialog_field(self.name.clone().into_any_element()))
                .child(popover::dialog_field(
                    self.prompt.clone().into_any_element(),
                ))
                // Schedule kind chips.
                .child(
                    div().flex().flex_row().gap(px(6.0)).children(
                        ["Hourly", "Daily", "Weekly", "Cron"]
                            .into_iter()
                            .enumerate()
                            .map(|(ix, label)| {
                                let active = self.schedule_kind == ix;
                                div()
                                    .id(("auto-sched", ix))
                                    .px(px(10.0))
                                    .py(px(5.0))
                                    .rounded(px(7.0))
                                    .text_size(crate::typography::ui_rems(11.5))
                                    .cursor_pointer()
                                    .text_color(if active { theme.text } else { theme.text_muted })
                                    .bg(if active {
                                        theme.element_hover
                                    } else {
                                        gpui::transparent_black()
                                    })
                                    .child(SharedString::from(label))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.schedule_kind = ix;
                                        cx.notify();
                                    }))
                            }),
                    ),
                )
                .when(self.schedule_kind == 3, |el| {
                    el.child(popover::dialog_field(self.cron.clone().into_any_element()))
                })
                // Seat chips: this computer, then one per configured seat.
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap(px(6.0))
                        .child({
                            let active = self.seat.is_none();
                            div()
                                .id("auto-seat-local")
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(7.0))
                                .text_size(crate::typography::ui_rems(11.5))
                                .cursor_pointer()
                                .text_color(if active { theme.text } else { theme.text_muted })
                                .bg(if active {
                                    theme.element_hover
                                } else {
                                    gpui::transparent_black()
                                })
                                .child("This computer")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.seat = None;
                                    cx.notify();
                                }))
                        })
                        .children(seats.iter().enumerate().map(|(ix, seat)| {
                            let id = seat.config.id.clone();
                            let active = self.seat.as_deref() == Some(id.as_str());
                            div()
                                .id(("auto-seat", ix))
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded(px(7.0))
                                .text_size(crate::typography::ui_rems(11.5))
                                .cursor_pointer()
                                .text_color(if active { theme.text } else { theme.text_muted })
                                .bg(if active {
                                    theme.element_hover
                                } else {
                                    gpui::transparent_black()
                                })
                                .child(SharedString::from(seat.config.label.clone()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.seat = Some(id.clone());
                                    cx.notify();
                                }))
                        })),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(8.0))
                        .child(
                            popover::btn_primary(theme, "Create")
                                .id("auto-create")
                                .when(self.busy, |el| el.opacity(0.5))
                                .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                        )
                        .child(
                            popover::btn_ghost(theme, "Cancel", "auto-cancel")
                                .id("auto-cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.creating = false;
                                    cx.notify();
                                })),
                        ),
                ),
        );
        card.into_any_element()
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
            .view
            .ready()
            .map(|v| v.automations.clone())
            .unwrap_or_default();
        let mut card = widgets::section_card(&theme);
        if automations.is_empty() && !self.creating {
            card = card.child(
                widgets::card_row(&theme, true)
                    .child(widgets::row_tile(&theme, icons::CLOCK_CIRCLE))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(widgets::row_title(&theme, "No automations"))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(11.5))
                                    .text_color(theme.text_muted)
                                    .child(
                                        "A prompt on a schedule — fired by the engine's timer. \
                                         A cloud seat that's unreachable marks the run missed \
                                         instead of starting blind.",
                                    ),
                            ),
                    ),
            );
        }
        for (ix, automation) in automations.iter().enumerate() {
            card = card.child(self.automation_row(automation, ix == 0, &theme, cx));
        }
        let mut column = widgets::page_column()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(widgets::page_header(&theme, "Automations", None))
                    .when(!self.creating, |el| {
                        el.child(
                            popover::btn_ghost(&theme, "New automation", "auto-new")
                                .id("auto-new")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.creating = true;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(
                widgets::page_subtitle(
                    &theme,
                    "Runs on a schedule against this computer or a cloud seat. Missed fires \
                     land on the Kanban board.",
                )
                .max_w(px(560.0)),
            )
            .child(card);
        if self.creating {
            column = column.child(self.create_card(&theme, cx));
        }
        if let Some(error) = &self.error {
            column = column.child(widgets::error_strip(&theme, error.clone()));
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
