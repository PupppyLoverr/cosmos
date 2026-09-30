//! Cloud / GitHub agent mode — the seat's right-rail panes, the bot bar, the
//! Settings → Models / Cloud computer pages, and the Automations + Kanban
//! boards.
//!
//! Every surface gates on the seat's honest `capabilities` (engine-reported,
//! never faked): a pane with no backend renders its empty state plus one line
//! of copy instead of pretending. Secrets never cross this module — RPCs take
//! seat ids; the engine looks keys up in `cloud-keys.json`.

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, IntoElement, MouseButton, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::*,
    px,
};
use serde::Deserialize;
use zeron_engine::cloud::{PrDetail, PrFile, PrRow, RemoteEntry, SeatConfig};
use zeron_proto::{
    ByokProvider, CloudAutomation, CloudPane, CloudRun, CloudRunStatus, CloudSeatBinding,
    CloudSeatKind, SeatCapabilities, SeatProbe, SessionStatus,
};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::icons::{self, icon};
use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

pub mod boards;
pub mod seats;

// ---------------------------------------------------------------------------
// Wire views (the CloudGetConfig reply shape)
// ---------------------------------------------------------------------------

/// One seat row as the engine's `view()` returns it — `SeatConfig` plus the
/// secret-flags and capabilities the reply injects.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SeatRow {
    #[serde(flatten)]
    pub config: SeatConfig,
    pub has_box_token: bool,
    pub has_github_pat: bool,
    pub capabilities: SeatCapabilities,
}

/// The `CloudGetConfig` reply — non-secret snapshot of seats, BYOK rows,
/// automations, and the run ledger.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CloudView {
    pub seats: Vec<SeatRow>,
    pub providers: Vec<zeron_engine::cloud::ProviderConfig>,
    pub automations: Vec<CloudAutomation>,
    pub default_seat: Option<String>,
    pub runs: Vec<CloudRun>,
    /// The shared GitHub PAT exists (the repo picker's token flag).
    pub has_github_pat: bool,
    pub load_error: Option<String>,
}

impl CloudView {
    pub fn seat(&self, seat_id: &str) -> Option<&SeatRow> {
        self.seats.iter().find(|s| s.config.id == seat_id)
    }

    /// Latest ledger run for a chat (the ledger is append-most-recent-last).
    pub fn run_for_chat(&self, chat_id: &str) -> Option<&CloudRun> {
        self.runs.iter().rev().find(|run| run.chat_id == chat_id)
    }

    /// Seats a chat can bind to, grouped for the new-session picker: first
    /// the SSH/Box seats ("Cloud seat"), then GitHub seats ("GitHub agent").
    pub fn runnable_seats(&self) -> impl Iterator<Item = &SeatRow> {
        self.seats.iter()
    }

    /// A seat is startable when it has the credentials its kind needs.
    /// Railway needs nothing but an unexpired hour (the user's ssh key IS the
    /// trial identity); Boat needs only a token — the box provisions lazily.
    pub fn seat_ready(seat: &SeatRow) -> bool {
        match seat.config.kind {
            Some(CloudSeatKind::Ssh) => {
                seat.config.ssh_alias.is_some()
                    && seat
                        .config
                        .last_probe
                        .as_ref()
                        .map(|probe| probe.online)
                        .unwrap_or(true)
            }
            Some(CloudSeatKind::Railway) => !seat.config.expired(),
            Some(CloudSeatKind::Box) => seat.has_box_token,
            Some(CloudSeatKind::GitHub) => seat.config.repo.is_some() && seat.has_github_pat,
            None => false,
        }
    }

    pub fn seat_runnable(seat: &SeatRow) -> bool {
        Self::seat_ready(seat) && seat.config.kind != Some(CloudSeatKind::GitHub)
    }

    /// Model chip label for a seat-bound run: the cloud BYOK default model,
    /// else the seat's own CLI login.
    pub fn cloud_model_label(&self) -> SharedString {
        self.providers
            .iter()
            .find(|p| p.use_for_cloud)
            .and_then(|p| p.model.clone())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "Seat CLI login".into())
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    fn seat(kind: CloudSeatKind) -> SeatRow {
        SeatRow {
            config: SeatConfig {
                kind: Some(kind),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn seat_readiness_matches_credential_and_probe_requirements() {
        let mut ssh = seat(CloudSeatKind::Ssh);
        assert!(!CloudView::seat_ready(&ssh));
        ssh.config.ssh_alias = Some("devbox".into());
        assert!(CloudView::seat_ready(&ssh));
        ssh.config.last_probe = Some(SeatProbe {
            online: false,
            ..Default::default()
        });
        assert!(!CloudView::seat_ready(&ssh));

        let mut railway = seat(CloudSeatKind::Railway);
        railway.config.expires_at = Some(Utc::now() + Duration::minutes(5));
        assert!(CloudView::seat_ready(&railway));
        assert!(CloudView::seat_runnable(&railway));
        railway.config.expires_at = Some(Utc::now() - Duration::minutes(1));
        assert!(!CloudView::seat_ready(&railway));

        let mut boat = seat(CloudSeatKind::Box);
        boat.config.box_id = Some("stale-box".into());
        assert!(!CloudView::seat_ready(&boat));
        boat.has_box_token = true;
        assert!(CloudView::seat_ready(&boat));

        let mut github = seat(CloudSeatKind::GitHub);
        github.config.repo = Some("owner/repo".into());
        github.has_github_pat = true;
        assert!(CloudView::seat_ready(&github));
        assert!(!CloudView::seat_runnable(&github));
    }
}

pub(crate) fn seat_pane_icon(pane: CloudPane) -> &'static str {
    match pane {
        CloudPane::Computer => icons::MONITOR,
        CloudPane::Simulator => icons::SMARTPHONE,
        CloudPane::Files => icons::FILE_TREE,
        CloudPane::Changes => icons::LIST,
        CloudPane::Shell => icons::TERMINAL,
        CloudPane::Progress => icons::SORT_VERTICAL,
        CloudPane::Context => icons::FOLDER_WITH_FILES,
        CloudPane::Tasks => icons::CHECKLIST,
        CloudPane::PullRequests => icons::PULL_REQUEST,
    }
}

/// The green/red/sleeping dot used by the bot bar and seat rows.
pub(crate) fn status_dot(theme: &Theme, probe: Option<&SeatProbe>) -> gpui::Stateful<gpui::Div> {
    let (color, label) = match probe {
        Some(p) if p.sleeping => (theme.warning, "sleeping"),
        Some(p) if p.online => (theme.success, "online"),
        Some(_) => (theme.danger, "offline"),
        None => (theme.text_faint, "not probed"),
    };
    div()
        .id(SharedString::from(format!("seat-dot-{label}")))
        .size(px(7.0))
        .flex_none()
        .rounded_full()
        .bg(color)
        .tooltip(widgets::text_tooltip(label))
}

fn pane_empty(theme: &Theme, icon_path: &'static str, title: &str, copy: &str) -> gpui::Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.0))
        .p(px(24.0))
        .child(
            icon(icon_path)
                .size(px(20.0))
                .text_color(theme.text_muted.opacity(0.5)),
        )
        .child(
            div()
                .text_size(crate::typography::ui_rems(13.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(SharedString::from(title)),
        )
        .child(
            div()
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .text_center()
                .max_w(px(280.0))
                .child(SharedString::from(copy)),
        )
}

// ---------------------------------------------------------------------------
// The seat rail — one entity per (chat, pane)
// ---------------------------------------------------------------------------

struct ShellLine {
    command: String,
    code: Option<i32>,
    output: String,
}

/// The PR detail's sub-tabs — the reference screenshot's strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrTab {
    Changes,
    Description,
    Discussion,
    Commits,
    Checks,
    Bugs,
}

impl PrTab {
    const ALL: [PrTab; 6] = [
        Self::Changes,
        Self::Description,
        Self::Discussion,
        Self::Commits,
        Self::Checks,
        Self::Bugs,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Changes => "Changes",
            Self::Description => "Description",
            Self::Discussion => "Discussion",
            Self::Commits => "Commits",
            Self::Checks => "Checks",
            Self::Bugs => "Bugs",
        }
    }
}

/// The one action the rail delegates to the shell — a side chat opens in the
/// existing right-pane host, never inside the rail itself.
pub enum RailEvent {
    OpenSideChat,
    /// "Attach Boat token" on the keep card → Settings → Cloud computer.
    OpenCloudSettings,
}

impl gpui::EventEmitter<RailEvent> for CloudRail {}
pub struct CloudRail {
    state: Entity<AppState>,
    chat_id: String,
    binding: CloudSeatBinding,
    pane: CloudPane,
    seat: Loadable<SeatRow>,
    // Files
    cwd: String,
    parents: Vec<String>,
    listing: Loadable<Vec<RemoteEntry>>,
    file_path: Option<String>,
    file_text: Loadable<(String, bool)>,
    // Changes
    status: Loadable<serde_json::Value>,
    diff: Loadable<String>,
    // Shell
    shell_input: Entity<ComposerInput>,
    shell_log: Vec<ShellLine>,
    exec_busy: bool,
    // Pull requests
    prs: Loadable<Vec<PrRow>>,
    pr_detail: Option<(u64, Loadable<PrDetail>)>,
    pr_tab: PrTab,
    pr_expanded: std::collections::HashSet<String>,
    /// "Run analysis": the review chat this pane spawned, once created.
    analysis: Loadable<Option<String>>,
    // Computer
    shot: Loadable<Option<Arc<gpui::Image>>>,
    /// The chat's latest ledger run — drives the bot bar's status + buttons.
    run: Option<CloudRun>,
    /// BYOK label for the model chip (the engine view's cloud default).
    model_label: Option<SharedString>,
    /// The pane "+" menu and the destroy confirmation live on the rail chrome.
    pane_menu: popover::Popup<()>,
    confirm_destroy: bool,
    control_busy: bool,
    control_error: Option<String>,
    /// The chat's session stamp at the last ledger refresh — the state
    /// observer reloads the run row only when this actually moves.
    session_stamp: Option<(SessionStatus, chrono::DateTime<chrono::Utc>)>,
    /// One-shot bookkeeping for the "Keep this computer" card: `None` = the
    /// seat was never asked, `Some(false)` = card visible, `Some(true)` =
    /// marked prompted this session (persisted engine-side).
    keep_prompted: Option<bool>,
    task: Option<Task<()>>,
    _input_sub: Subscription,
    _state_sub: Subscription,
    /// Railway countdown ticker — 1s re-render while a deadline is live.
    _countdown: Option<Task<()>>,
    _refresh: Option<Task<()>>,
}

fn cloud_run_finished(previous: Option<&CloudRun>, next: &CloudRun) -> bool {
    matches!(
        next.status,
        CloudRunStatus::Done | CloudRunStatus::Error | CloudRunStatus::Stopped
    ) && previous.is_none_or(|previous| {
        previous.id != next.id
            || matches!(
                previous.status,
                CloudRunStatus::Queued
                    | CloudRunStatus::Working
                    | CloudRunStatus::WaitingForUser
                    | CloudRunStatus::NeedsApproval
            )
    })
}

impl CloudRail {
    pub fn new(
        state: Entity<AppState>,
        chat_id: String,
        binding: CloudSeatBinding,
        pane: CloudPane,
        cx: &mut Context<Self>,
    ) -> Self {
        let shell_input = cx.new(|cx| ComposerInput::new("$ run a command on the seat…", cx));
        let input_sub = cx.subscribe(&shell_input, |this, _, event, cx| {
            if let ComposerInputEvent::Submitted = event {
                this.run_shell_command(cx);
            }
        });
        // A session flip (run started/finished) refreshes the ledger row the
        // bot bar renders — the same watch Kanban cards ride. Gated on this
        // chat's row actually changing: AppState notifies on every keystroke,
        // so an unconditional reload would spam CloudGetConfig.
        let state_sub = cx.observe(&state, |this, _, cx| {
            let stamp = this
                .state
                .read(cx)
                .sessions
                .iter()
                .find(|s| s.chat_id == this.chat_id)
                .map(|s| (s.status, s.updated_at));
            if stamp != this.session_stamp {
                this.session_stamp = stamp;
                this.load_seat(cx);
            }
        });
        let mut rail = Self {
            state,
            chat_id,
            cwd: binding.workspace_path.clone().unwrap_or_else(|| ".".into()),
            parents: Vec::new(),
            binding,
            pane,
            seat: Loadable::Loading,
            listing: Loadable::Idle,
            file_path: None,
            file_text: Loadable::Idle,
            status: Loadable::Idle,
            diff: Loadable::Idle,
            shell_input,
            shell_log: Vec::new(),
            exec_busy: false,
            prs: Loadable::Idle,
            pr_detail: None,
            pr_tab: PrTab::Changes,
            pr_expanded: std::collections::HashSet::new(),
            analysis: Loadable::Idle,
            shot: Loadable::Idle,
            run: None,
            model_label: None,
            pane_menu: popover::Popup::default(),
            confirm_destroy: false,
            control_busy: false,
            control_error: None,
            session_stamp: None,
            keep_prompted: None,
            task: None,
            _input_sub: input_sub,
            _state_sub: state_sub,
            _countdown: None,
            _refresh: None,
        };
        // Live countdown: while a Railway deadline is ahead of the clock the
        // rail re-renders once a second so the chip/banner stay honest.
        if rail.binding.expires_at.is_some() {
            rail._countdown = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(1))
                        .await;
                    let alive = this
                        .update(cx, |rail, cx| {
                            let left = rail
                                .binding
                                .expires_at
                                .map(|at| at.signed_duration_since(chrono::Utc::now()))
                                .map(|d| d.num_seconds())
                                .unwrap_or(-1);
                            cx.notify();
                            left > 0
                        })
                        .unwrap_or(false);
                    if !alive {
                        break;
                    }
                }
            }));
        }
        let state_for_refresh = rail.state.clone();
        rail._refresh = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(30))
                    .await;
                let alive = this
                    .update(cx, |_, cx| {
                        state_for_refresh.update(cx, |state, cx| {
                            state.refresh_cloud(Some(std::time::Duration::from_secs(30)), cx);
                        });
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        }));
        rail.load_seat(cx);
        rail.ensure_loaded(cx);
        rail
    }

    fn capabilities(&self) -> SeatCapabilities {
        self.seat
            .ready()
            .map(|s| s.capabilities.clone())
            .unwrap_or_default()
    }

    fn engine(&self, cx: &App) -> Option<crate::state::EngineHandle> {
        self.state.read(cx).engine().cloned()
    }

    /// The pane's first load — seat row, then whatever the pane shows.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        match self.pane {
            CloudPane::Files
                if matches!(self.listing, Loadable::Idle)
                    || self.listing.ready().is_some_and(Vec::is_empty) =>
            {
                self.load_files(cx)
            }
            CloudPane::Changes if matches!(self.status, Loadable::Idle) => self.load_changes(cx),
            CloudPane::PullRequests if matches!(self.prs, Loadable::Idle) => self.load_prs(cx),
            CloudPane::Computer if matches!(self.shot, Loadable::Idle) => self.capture_shot(cx),
            CloudPane::Simulator => {}
            _ => {}
        }
    }

    fn load_seat(&mut self, cx: &mut Context<Self>) {
        let seat_id = self.binding.seat_id.clone();
        let Some(engine) = self.engine(cx) else {
            self.seat = Loadable::Error("Engine not connected".into());
            return;
        };
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::CLOUD_GET_CONFIG, serde_json::json!({}))
                .await;
            this.update(cx, |rail, cx| {
                let mut run_finished = false;
                rail.seat = match result {
                    Ok(value) => match serde_json::from_value::<CloudView>(value) {
                        Ok(view) => {
                            let run = view.run_for_chat(&rail.chat_id).cloned();
                            run_finished = run
                                .as_ref()
                                .is_some_and(|next| cloud_run_finished(rail.run.as_ref(), next));
                            rail.run = run.or_else(|| rail.run.take());
                            rail.model_label = Some(view.cloud_model_label());
                            match view.seat(&seat_id) {
                                Some(row) => Loadable::Ready(row.clone()),
                                None => {
                                    Loadable::Error(format!("seat `{seat_id}` is not configured"))
                                }
                            }
                        }
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                if run_finished
                    || (rail.pane == CloudPane::Files && matches!(rail.listing, Loadable::Idle))
                {
                    rail.load_files(cx);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Switch the rail's active pane — the only pane visible, per the "one
    /// pane at a time" rule.
    pub fn set_pane(&mut self, pane: CloudPane, cx: &mut Context<Self>) {
        if self.pane == pane {
            self.close_pane_menu(cx);
            self.ensure_loaded(cx);
            return;
        }
        self.pane = pane;
        self.close_pane_menu(cx);
        self.ensure_loaded(cx);
        cx.notify();
    }

    pub(crate) fn close_pane_menu(&mut self, cx: &mut Context<Self>) -> bool {
        let mounted = self.pane_menu.get().is_some();
        if self.pane_menu.begin_close() {
            popover::reap_popup(cx, |rail: &mut Self| &mut rail.pane_menu);
            cx.notify();
        }
        mounted
    }

    /// Bot-bar control: stop / pause / resume / approve / destroy. The
    /// engine decides what each op does for this seat kind; the rail only
    /// reports failures honestly.
    fn seat_control(&mut self, op: &'static str, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            self.control_error = Some("Engine not connected".into());
            return;
        };
        self.control_busy = true;
        self.control_error = None;
        let chat_id = self.chat_id.clone();
        let seat_id = self.binding.seat_id.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_SEAT_CONTROL,
                    serde_json::json!({
                        "chatId": chat_id,
                        "op": op,
                        "seatId": seat_id,
                    }),
                )
                .await;
            this.update(cx, |rail, cx| {
                rail.control_busy = false;
                rail.confirm_destroy = false;
                if let Err(err) = result {
                    rail.control_error = Some(err.to_string());
                }
                rail.load_seat(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Both keep-card answers land here — the engine stamps
    /// `keep_prompted_at` so the card never asks twice, on any device.
    fn mark_keep_prompted(&mut self, cx: &mut Context<Self>) {
        self.keep_prompted = Some(true);
        if let Some(engine) = self.engine(cx) {
            let seat_id = self.binding.seat_id.clone();
            self.task = Some(cx.spawn(async move |this, cx| {
                let _ = engine
                    .client()
                    .call(
                        methods::CLOUD_SEAT_CONTROL,
                        serde_json::json!({
                            "chatId": "",
                            "seatId": seat_id,
                            "op": "markKeepPrompted",
                        }),
                    )
                    .await;
                this.update(cx, |_, cx| cx.notify()).ok();
            }));
        }
        cx.notify();
    }

    // -- Files --

    fn load_files(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            self.listing = Loadable::Error("Engine not connected".into());
            return;
        };
        if self.seat.ready().is_none() {
            return;
        }
        if !self.capabilities().files {
            self.listing = Loadable::Ready(Vec::new());
            return;
        }
        self.listing = Loadable::Loading;
        let seat_id = self.binding.seat_id.clone();
        let dir = self.cwd.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_LIST_DIR,
                    serde_json::json!({ "seatId": seat_id, "path": dir }),
                )
                .await;
            this.update(cx, |rail, cx| {
                rail.listing = match result {
                    Ok(value) => serde_json::from_value::<Vec<RemoteEntry>>(
                        value.get("entries").cloned().unwrap_or_default(),
                    )
                    .map(Loadable::Ready)
                    .unwrap_or_else(|e| Loadable::Error(e.to_string())),
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_entry(&mut self, entry: RemoteEntry, cx: &mut Context<Self>) {
        if entry.is_dir {
            self.parents.push(self.cwd.clone());
            self.cwd = entry.path.clone();
            self.file_path = None;
            self.file_text = Loadable::Idle;
            self.load_files(cx);
        } else {
            self.file_path = Some(entry.path.clone());
            self.file_text = Loadable::Loading;
            let Some(engine) = self.engine(cx) else {
                return;
            };
            let seat_id = self.binding.seat_id.clone();
            let path = entry.path;
            self.task = Some(cx.spawn(async move |this, cx| {
                let result = engine
                    .client()
                    .call(
                        methods::CLOUD_READ_FILE,
                        serde_json::json!({ "seatId": seat_id, "path": path }),
                    )
                    .await;
                this.update(cx, |rail, cx| {
                    rail.file_text = match result {
                        Ok(value) => Loadable::Ready((
                            value
                                .get("content")
                                .and_then(|c| c.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            value
                                .get("truncated")
                                .and_then(|t| t.as_bool())
                                .unwrap_or(false),
                        )),
                        Err(err) => Loadable::Error(err.to_string()),
                    };
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    fn files_up(&mut self, cx: &mut Context<Self>) {
        if let Some(parent) = self.parents.pop() {
            self.cwd = parent;
            self.file_path = None;
            self.file_text = Loadable::Idle;
            self.load_files(cx);
        }
    }

    // -- Changes --

    fn load_changes(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            self.status = Loadable::Error("Engine not connected".into());
            return;
        };
        if !self.capabilities().git {
            self.status = Loadable::Ready(serde_json::Value::Null);
            return;
        }
        self.status = Loadable::Loading;
        self.diff = Loadable::Loading;
        let seat_id = self.binding.seat_id.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let status = engine
                .client()
                .call(
                    methods::CLOUD_GIT_STATUS,
                    serde_json::json!({ "seatId": seat_id }),
                )
                .await;
            let diff = engine
                .client()
                .call(
                    methods::CLOUD_GIT_DIFF,
                    serde_json::json!({ "seatId": seat_id }),
                )
                .await;
            this.update(cx, |rail, cx| {
                rail.status = match status {
                    Ok(v) => Loadable::Ready(v),
                    Err(e) => Loadable::Error(e.to_string()),
                };
                rail.diff = match diff {
                    Ok(v) => Loadable::Ready(
                        v.get("diff")
                            .and_then(|d| d.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    Err(e) => Loadable::Error(e.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    // -- Shell --

    fn run_shell_command(&mut self, cx: &mut Context<Self>) {
        let command = self.shell_input.read(cx).text().trim().to_string();
        if command.is_empty() || self.exec_busy {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.exec_busy = true;
        self.shell_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.shell_log.push(ShellLine {
            command: command.clone(),
            code: None,
            output: String::new(),
        });
        let seat_id = self.binding.seat_id.clone();
        let cwd = self.cwd.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_EXEC,
                    serde_json::json!({
                        "seatId": seat_id,
                        "command": format!("cd {} && {command}", zeron_engine::cloud::shell_word(&cwd)),
                        "timeoutSecs": 120,
                    }),
                )
                .await;
            this.update(cx, |rail, cx| {
                rail.exec_busy = false;
                let line = rail.shell_log.last_mut();
                match (result, line) {
                    (Ok(value), Some(line)) => {
                        line.code = value.get("code").and_then(|c| c.as_i64()).map(|c| c as i32);
                        let stdout = value
                            .get("stdout")
                            .and_then(|s| s.as_str())
                            .unwrap_or_default();
                        let stderr = value
                            .get("stderr")
                            .and_then(|s| s.as_str())
                            .unwrap_or_default();
                        line.output = if stderr.is_empty() {
                            stdout.to_string()
                        } else if stdout.is_empty() {
                            stderr.to_string()
                        } else {
                            format!("{stdout}\n{stderr}")
                        };
                    }
                    (Err(err), Some(line)) => {
                        line.code = Some(-1);
                        line.output = err.to_string();
                    }
                    _ => {}
                }
                cx.notify();
            })
            .ok();
        }));
    }

    // -- Pull requests --

    fn load_prs(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            self.prs = Loadable::Error("Engine not connected".into());
            return;
        };
        if !self.capabilities().pull_requests {
            self.prs = Loadable::Ready(Vec::new());
            return;
        }
        self.prs = Loadable::Loading;
        let seat_id = self.binding.seat_id.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_GITHUB_PRS,
                    serde_json::json!({ "seatId": seat_id }),
                )
                .await;
            this.update(cx, |rail, cx| {
                rail.prs = match result {
                    Ok(value) => serde_json::from_value::<Vec<PrRow>>(
                        value.get("prs").cloned().unwrap_or(value),
                    )
                    .map(Loadable::Ready)
                    .unwrap_or_else(|e| Loadable::Error(e.to_string())),
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_pr(&mut self, number: u64, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.pr_detail = Some((number, Loadable::Loading));
        let seat_id = self.binding.seat_id.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_GITHUB_PR,
                    serde_json::json!({ "seatId": seat_id, "number": number }),
                )
                .await;
            this.update(cx, |rail, cx| {
                if let Some((n, slot)) = &mut rail.pr_detail
                    && *n == number
                {
                    *slot = match result {
                        Ok(value) => serde_json::from_value::<PrDetail>(value)
                            .map(Loadable::Ready)
                            .unwrap_or_else(|e| Loadable::Error(e.to_string())),
                        Err(err) => Loadable::Error(err.to_string()),
                    };
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn merge_pr(&mut self, number: u64, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let seat_id = self.binding.seat_id.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_GITHUB_MERGE,
                    serde_json::json!({ "seatId": seat_id, "number": number }),
                )
                .await;
            this.update(cx, |rail, cx| {
                if let Err(err) = result
                    && let Some((_, slot)) = &mut rail.pr_detail
                {
                    *slot = Loadable::Error(err.to_string());
                }
                rail.load_prs(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// "Run analysis": spawn a review chat bound to this seat, seeded with a
    /// review prompt for the open PR. Same path the composer sends —
    /// `createChat` carries the seat binding, then `QueueCommand` runs.
    fn run_analysis(&mut self, pr: &PrDetail, cx: &mut Context<Self>) {
        if matches!(self.analysis, Loadable::Loading) {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            self.analysis = Loadable::Error("Engine not connected".into());
            cx.notify();
            return;
        };
        let Some(device_id) = self.state.read(cx).effective_device_id() else {
            self.analysis = Loadable::Error("No device selected".into());
            cx.notify();
            return;
        };
        self.analysis = Loadable::Loading;
        let chat_id = uuid::Uuid::new_v4().to_string();
        let binding = self.binding.clone();
        let repo = pr
            .url
            .rsplit("/pull/")
            .next()
            .and_then(|_| self.binding.repo.clone())
            .unwrap_or_else(|| "the repository".into());
        let prompt = format!(
            "Review pull request #{num} in {repo} ({head} → {base}): \"{title}\".\n\n\
             Fetch the diff, look for bugs, regressions, and risky changes. \
             Report findings ordered by severity; cite file and line.",
            num = pr.number,
            repo = repo,
            head = pr.head,
            base = pr.base,
            title = pr.title,
        );
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            let create = engine
                .client()
                .call(
                    methods::MUTATE,
                    serde_json::json!({
                        "op": "createChat",
                        "chatId": chat_id,
                        "deviceId": device_id,
                        "cloudSeat": serde_json::to_value(&binding).unwrap_or_default(),
                    }),
                )
                .await;
            if let Err(err) = create {
                let _ = this.update(cx, |rail, cx| {
                    rail.analysis = Loadable::Error(err.to_string());
                    cx.notify();
                });
                return;
            }
            let command = serde_json::to_value(zeron_doc::SessionCommandPayload::Run {
                request: zeron_proto::RunRequest {
                    prompt,
                    harness: None,
                    model: None,
                    reasoning: None,
                    model_options: serde_json::Map::new(),
                    cwd: "~".into(),
                    sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
                    auto_approve: false,
                    resume: None,
                    attachments: Vec::new(),
                    worktree: None,
                },
                message_id: uuid::Uuid::new_v4().to_string(),
            })
            .unwrap_or_default();
            let run = engine
                .client()
                .call(
                    methods::QUEUE_COMMAND,
                    serde_json::json!({ "chatId": chat_id, "command": command }),
                )
                .await;
            let _ = this.update(cx, |rail, cx| {
                rail.analysis = match run {
                    Ok(_) => Loadable::Ready(Some(chat_id.clone())),
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            });
        }));
    }

    // -- Computer --

    fn capture_shot(&mut self, cx: &mut Context<Self>) {
        if !self.capabilities().computer {
            self.shot = Loadable::Ready(None);
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.shot = Loadable::Loading;
        let seat_id = self.binding.seat_id.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_SCREENSHOT,
                    serde_json::json!({ "seatId": seat_id }),
                )
                .await;
            this.update(cx, |rail, cx| {
                rail.shot = match result {
                    Ok(value) => {
                        let image = value
                            .get("pngBase64")
                            .and_then(|p| p.as_str())
                            .and_then(|b64| {
                                use base64::Engine as _;
                                base64::engine::general_purpose::STANDARD.decode(b64).ok()
                            })
                            .map(|bytes| {
                                Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes))
                            });
                        Loadable::Ready(image)
                    }
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    // -- rendering --

    fn render_files(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if !self.capabilities().files {
            return pane_empty(
                theme,
                icons::FILE_TREE,
                "Files",
                "This seat has no file surface — SSH seats list through the agent, \
                 GitHub seats through the contents API once a repo is bound.",
            )
            .into_any_element();
        }
        let header = div()
            .flex_none()
            .h(px(36.0))
            .px(px(10.0))
            .border_b_1()
            .border_color(theme.border)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .id("rail-files-up")
                    .flex_none()
                    .cursor_pointer()
                    .when(!self.parents.is_empty(), |el| {
                        el.on_click(cx.listener(|this, _, _, cx| this.files_up(cx)))
                    })
                    .opacity(if self.parents.is_empty() { 0.35 } else { 1.0 })
                    .child(
                        icon(icons::ALT_ARROW_UP)
                            .size(px(13.0))
                            .text_color(theme.text_muted),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(11.5))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(self.cwd.clone())),
            )
            .child(
                div()
                    .id("rail-files-refresh")
                    .flex_none()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.load_files(cx)))
                    .child(
                        icon(icons::REFRESH)
                            .size(px(13.0))
                            .text_color(theme.text_muted),
                    ),
            );
        let body = match &self.listing {
            Loadable::Ready(entries) => {
                let mut list = div().flex().flex_col();
                if entries.is_empty() {
                    list = list.child(
                        div()
                            .p(px(12.0))
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child("Empty directory"),
                    );
                }
                for (ix, entry) in entries.iter().enumerate() {
                    let entry = entry.clone();
                    let icon_path = if entry.is_dir {
                        icons::FOLDER
                    } else {
                        icons::DOCUMENT
                    };
                    let fade = SharedString::from(format!("rail-file-{ix}"));
                    list = list.child(
                        div()
                            .id(("rail-file", ix))
                            .w_full()
                            .h(px(28.0))
                            .px(px(10.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .bg(motion_row_bg(&fade, theme))
                            .on_hover(crate::motion::hover_listener(fade.clone()))
                            .on_click({
                                let entry = entry.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.open_entry(entry.clone(), cx)
                                })
                            })
                            .child(
                                icon(icon_path)
                                    .size(px(13.0))
                                    .flex_none()
                                    .text_color(theme.text_muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(12.5))
                                    .text_color(theme.text)
                                    .child(SharedString::from(entry.name)),
                            )
                            .when_some(entry.size.filter(|_| !entry.is_dir), |el, size| {
                                el.child(
                                    div()
                                        .flex_none()
                                        .text_size(crate::typography::ui_rems(10.5))
                                        .text_color(theme.text_faint)
                                        .child(SharedString::from(format_size(size))),
                                )
                            }),
                    );
                }
                div()
                    .id("rail-files-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list)
                    .into_any_element()
            }
            Loadable::Error(err) => {
                pane_empty(theme, icons::DANGER_TRIANGLE, "Files", err).into_any_element()
            }
            _ => div()
                .p(px(12.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .child("Loading…")
                .into_any_element(),
        };
        let file_viewer: Option<AnyElement> = self.file_path.as_ref().map(|path| {
            let content = match &self.file_text {
                Loadable::Ready((text, truncated)) => div()
                    .id("rail-file-text")
                    .p(px(10.0))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .text_size(crate::typography::ui_rems(11.5))
                    .font_family(theme.font_mono.clone())
                    .text_color(theme.text)
                    .child(SharedString::from(text.clone()))
                    .when(*truncated, |el| {
                        el.child(
                            div()
                                .pt(px(8.0))
                                .text_color(theme.text_muted)
                                .child("… truncated"),
                        )
                    })
                    .into_any_element(),
                Loadable::Error(err) => div()
                    .p(px(10.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.danger)
                    .child(SharedString::from(err.clone()))
                    .into_any_element(),
                _ => div()
                    .p(px(10.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child("Loading…")
                    .into_any_element(),
            };
            div()
                .flex_1()
                .min_h_0()
                .border_t_1()
                .border_color(theme.border)
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex_none()
                        .h(px(28.0))
                        .px(px(10.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .id("rail-file-close")
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.file_path = None;
                                    this.file_text = Loadable::Idle;
                                    cx.notify();
                                }))
                                .child(
                                    icon(icons::CLOSE)
                                        .size(px(11.0))
                                        .text_color(theme.text_muted),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(path.clone())),
                        ),
                )
                .child(content)
                .into_any_element()
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .children(file_viewer)
            .into_any_element()
    }

    fn render_changes(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if !self.capabilities().git {
            return pane_empty(
                theme,
                icons::LIST,
                "Changes",
                "This seat has no git surface — bind a repo (GitHub seat) or an \
                 SSH workspace inside a git checkout.",
            )
            .into_any_element();
        }
        let header = div()
            .flex_none()
            .h(px(36.0))
            .px(px(10.0))
            .border_b_1()
            .border_color(theme.border)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                icon(icons::GIT_BRANCH)
                    .size(px(13.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(11.5))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(
                        self.status
                            .ready()
                            .and_then(|v| v.get("branch").and_then(|b| b.as_str()))
                            .unwrap_or("…")
                            .to_string(),
                    )),
            )
            .child(
                div()
                    .id("rail-changes-refresh")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.load_changes(cx)))
                    .child(
                        icon(icons::REFRESH)
                            .size(px(13.0))
                            .text_color(theme.text_muted),
                    ),
            );
        let body = match &self.status {
            Loadable::Ready(value) => {
                let mut list = div().flex().flex_col();
                let entries = value
                    .get("entries")
                    .and_then(|e| e.as_array())
                    .cloned()
                    .unwrap_or_default();
                if entries.is_empty() {
                    list = list.child(
                        div()
                            .p(px(12.0))
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child("Working tree clean"),
                    );
                }
                for (ix, entry) in entries.iter().enumerate() {
                    let status = entry
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("?")
                        .trim()
                        .to_string();
                    let path = entry
                        .get("path")
                        .and_then(|p| p.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let color = match status.as_str() {
                        "M" | "MM" | "AM" => theme.warning,
                        "A" | "??" => theme.success,
                        "D" => theme.danger,
                        _ => theme.text_muted,
                    };
                    list = list.child(
                        div()
                            .id(("rail-change", ix))
                            .w_full()
                            .h(px(26.0))
                            .px(px(10.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .w(px(22.0))
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .font_family(theme.font_mono.clone())
                                    .text_color(color)
                                    .child(SharedString::from(status)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .text_color(theme.text)
                                    .child(SharedString::from(path)),
                            ),
                    );
                }
                if let Some(stat) = value.get("stat").and_then(|s| s.as_str())
                    && !stat.trim().is_empty()
                {
                    list = list.child(
                        div()
                            .mx(px(10.0))
                            .mt(px(8.0))
                            .p(px(8.0))
                            .rounded(px(6.0))
                            .bg(theme.surface_card)
                            .text_size(crate::typography::ui_rems(10.5))
                            .font_family(theme.font_mono.clone())
                            .text_color(theme.text_muted)
                            .child(SharedString::from(stat.trim().to_string())),
                    );
                }
                div()
                    .id("rail-changes-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list)
                    .into_any_element()
            }
            Loadable::Error(err) => {
                pane_empty(theme, icons::DANGER_TRIANGLE, "Changes", err).into_any_element()
            }
            _ => div()
                .p(px(12.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .child("Loading…")
                .into_any_element(),
        };
        let diff_view: Option<AnyElement> = match &self.diff {
            Loadable::Ready(diff) if !diff.trim().is_empty() => Some(
                div()
                    .id("rail-diff")
                    .flex_none()
                    .max_h(px(240.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .overflow_y_scroll()
                    .p(px(10.0))
                    .text_size(crate::typography::ui_rems(10.5))
                    .font_family(theme.font_mono.clone())
                    .text_color(theme.text)
                    .child(SharedString::from(diff.clone()))
                    .into_any_element(),
            ),
            _ => None,
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .children(diff_view)
            .into_any_element()
    }

    fn render_shell(&mut self, theme: &Theme, _cx: &mut Context<Self>) -> AnyElement {
        if !self.capabilities().exec {
            return pane_empty(
                theme,
                icons::TERMINAL,
                "Shell",
                "This seat runs no commands — SSH seats exec through the agent; \
                 GitHub seats run steps as workflow jobs.",
            )
            .into_any_element();
        }
        let mut log = div().flex().flex_col().gap(px(2.0)).p(px(8.0));
        for line in &self.shell_log {
            log = log.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.5))
                            .font_family(theme.font_mono.clone())
                            .text_color(theme.text)
                            .child(SharedString::from(format!("$ {}", line.command))),
                    )
                    .when(!line.output.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(crate::typography::ui_rems(11.0))
                                .font_family(theme.font_mono.clone())
                                .text_color(if line.code.unwrap_or(0) == 0 {
                                    theme.text_muted
                                } else {
                                    theme.danger
                                })
                                .child(SharedString::from(line.output.clone())),
                        )
                    }),
            );
        }
        if self.exec_busy {
            log = log.child(
                div()
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted)
                    .child("…"),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("rail-shell-log")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(log)
                    .when(self.shell_log.is_empty() && !self.exec_busy, |el| {
                        el.child(
                            div()
                                .p(px(12.0))
                                .text_size(crate::typography::ui_rems(12.0))
                                .text_color(theme.text_muted)
                                .child(
                                    "Commands run in the seat's workspace. Output appears here.",
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(theme.border)
                    .p(px(8.0))
                    .child(popover::dialog_field(
                        self.shell_input.clone().into_any_element(),
                    )),
            )
            .into_any_element()
    }

    fn render_pull_requests(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if !self.capabilities().pull_requests {
            return pane_empty(
                theme,
                icons::PULL_REQUEST,
                "Pull requests",
                "Bind a repository to this seat (Settings → Cloud computer) to \
                 list and merge its pull requests.",
            )
            .into_any_element();
        }
        if let Some((number, detail)) = self.pr_detail.clone() {
            return self.render_pr_detail(number, &detail, theme, cx);
        }
        let header = div()
            .flex_none()
            .h(px(36.0))
            .px(px(10.0))
            .border_b_1()
            .border_color(theme.border)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                icon(icons::PULL_REQUEST)
                    .size(px(13.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(11.5))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(
                        self.binding
                            .repo
                            .clone()
                            .unwrap_or_else(|| "repository".into()),
                    )),
            )
            .child(
                div()
                    .id("rail-prs-refresh")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.load_prs(cx)))
                    .child(
                        icon(icons::REFRESH)
                            .size(px(13.0))
                            .text_color(theme.text_muted),
                    ),
            );
        let body = match &self.prs {
            Loadable::Ready(prs) => {
                let mut list = div().flex().flex_col();
                if prs.is_empty() {
                    list = list.child(
                        div()
                            .p(px(12.0))
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child("No open pull requests"),
                    );
                }
                for (ix, pr) in prs.iter().enumerate() {
                    let number = pr.number;
                    let checks = pr.checks.clone();
                    let fade = SharedString::from(format!("rail-pr-{ix}"));
                    list = list.child(
                        div()
                            .id(("rail-pr", ix))
                            .w_full()
                            .px(px(10.0))
                            .py(px(8.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .cursor_pointer()
                            .bg(motion_row_bg(&fade, theme))
                            .on_hover(crate::motion::hover_listener(fade))
                            .on_click(cx.listener(move |this, _, _, cx| this.open_pr(number, cx)))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(crate::typography::ui_rems(11.0))
                                            .font_family(theme.font_mono.clone())
                                            .text_color(theme.text_muted)
                                            .child(SharedString::from(format!("#{number}"))),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(crate::typography::ui_rems(12.5))
                                            .text_color(theme.text)
                                            .child(SharedString::from(pr.title.clone())),
                                    )
                                    .when(pr.draft, |el| el.child(widgets::badge(theme, "Draft"))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.0))
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .text_color(theme.text_faint)
                                    .child(SharedString::from(format!("{} → {}", pr.head, pr.base)))
                                    .child(SharedString::from(pr.author.clone()))
                                    .when_some(checks, |el, checks| {
                                        el.child(SharedString::from(format!("checks: {checks}")))
                                    }),
                            ),
                    );
                }
                div()
                    .id("rail-prs-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list)
                    .into_any_element()
            }
            Loadable::Error(err) => {
                pane_empty(theme, icons::DANGER_TRIANGLE, "Pull requests", err).into_any_element()
            }
            _ => div()
                .p(px(12.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .child("Loading…")
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// The detailed PR card (the reference screenshot's shape): title,
    /// author/branch row, stats, checks list, changed files, actions.
    fn render_pr_detail(
        &mut self,
        number: u64,
        detail: &Loadable<PrDetail>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let back = div()
            .flex_none()
            .h(px(36.0))
            .px(px(10.0))
            .border_b_1()
            .border_color(theme.border)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .id("rail-pr-back")
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pr_detail = None;
                        cx.notify();
                    }))
                    .child(
                        icon(icons::ARROW_LEFT)
                            .size(px(13.0))
                            .text_color(theme.text_muted),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(11.5))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(format!("PR #{number}"))),
            );
        let body = match detail {
            Loadable::Ready(pr) => {
                let open = matches!(pr.state.as_str(), "open");
                let merged = matches!(pr.state.as_str(), "merged");
                let mergeable = pr.mergeable.unwrap_or(false) && open && !pr.draft;
                let state_label: SharedString = if pr.draft {
                    "Draft".into()
                } else {
                    SharedString::from(pr.state.clone())
                };
                let state_bg = if pr.draft {
                    theme.text_muted
                } else if open {
                    theme.success
                } else {
                    theme.accent
                };
                let url = pr.url.clone();
                let url_files = format!("{url}/files");
                let merge_note = if mergeable {
                    "This branch has no conflicts with the base branch".to_string()
                } else if pr.draft {
                    "Draft pull request — mark ready before merging".to_string()
                } else if merged {
                    "Pull request merged".to_string()
                } else if !open {
                    "Pull request closed".to_string()
                } else {
                    match pr.mergeable_state.as_deref() {
                        Some("dirty") => "This branch has conflicts that must be resolved".into(),
                        Some("blocked") => "Merging is blocked by required checks".into(),
                        _ => "Mergeability unknown — GitHub is still computing it".into(),
                    }
                };

                let mut actions = div().flex().flex_row().items_center().gap(px(6.0));
                actions = actions.child(
                    popover::btn_ghost(theme, "GitHub", "rail-pr-gh")
                        .id("rail-pr-gh-btn")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.open_url(&url);
                        })),
                );
                actions = actions.child(
                    popover::btn_ghost(theme, "Full view", "rail-pr-full")
                        .id("rail-pr-full-btn")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.open_url(&url_files);
                        })),
                );

                let header =
                    div()
                        .flex_none()
                        .border_b_1()
                        .border_color(theme.border)
                        .px(px(12.0))
                        .py(px(10.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .rounded(px(10.0))
                                        .px(px(8.0))
                                        .py(px(2.0))
                                        .bg(state_bg.opacity(0.18))
                                        .text_size(crate::typography::ui_rems(10.5))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(state_bg)
                                        .child(state_label),
                                )
                                .child(div().flex_1())
                                .child(actions),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_baseline()
                                .gap(px(8.0))
                                .child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .text_size(crate::typography::ui_rems(13.5))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme.text)
                                        .child(SharedString::from(pr.title.clone())),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(crate::typography::ui_rems(11.0))
                                        .text_color(theme.text_muted)
                                        .child(SharedString::from(format!("#{number}"))),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .items_center()
                                .gap(px(4.0))
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(format!(
                                    "{} wants to merge {} commits into",
                                    pr.author, pr.commits
                                )))
                                .child(widgets::badge(theme, pr.base.clone()))
                                .child("from")
                                .child(widgets::badge(theme, pr.head.clone()))
                                .child(SharedString::from(format!(
                                    "· {}",
                                    format_time(&pr.created_at)
                                ))),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(3.0))
                                        .text_size(crate::typography::ui_rems(10.5))
                                        .font_family(theme.font_mono.clone())
                                        .child(div().text_color(theme.diff_add).child(
                                            SharedString::from(format!("+{}", pr.additions)),
                                        ))
                                        .child(div().text_color(theme.diff_del).child(
                                            SharedString::from(format!("-{}", pr.deletions)),
                                        )),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(px(4.0))
                                        .text_size(crate::typography::ui_rems(10.5))
                                        .text_color(theme.text_muted)
                                        .child(
                                            icon(icons::LIST)
                                                .size(px(11.0))
                                                .text_color(theme.text_faint),
                                        )
                                        .child(SharedString::from(format!(
                                            "{} changed files",
                                            pr.changed_files
                                        ))),
                                )
                                .when(!pr.head_sha.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .text_size(crate::typography::ui_rems(10.5))
                                            .font_family(theme.font_mono.clone())
                                            .text_color(theme.text_faint)
                                            .child(SharedString::from(format!(
                                                "@{}",
                                                pr.head_sha.chars().take(7).collect::<String>()
                                            ))),
                                    )
                                }),
                        )
                        .when(open, |el| {
                            el.child(
                                div()
                                    .rounded(px(6.0))
                                    .border_1()
                                    .border_color(theme.border)
                                    .px(px(8.0))
                                    .py(px(6.0))
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        icon(if mergeable {
                                            icons::QUEUE_CHECK
                                        } else {
                                            icons::DANGER_TRIANGLE
                                        })
                                        .size(px(12.0))
                                        .flex_none()
                                        .text_color(
                                            if mergeable {
                                                theme.success
                                            } else {
                                                theme.warning
                                            },
                                        ),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(crate::typography::ui_rems(10.5))
                                            .text_color(theme.text_muted)
                                            .child(SharedString::from(merge_note)),
                                    )
                                    .child(
                                        popover::btn_primary(theme, "Merge pull request")
                                            .id("rail-pr-merge-btn")
                                            .when(!mergeable, |el| el.opacity(0.4))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if mergeable {
                                                    this.merge_pr(number, cx);
                                                }
                                            })),
                                    ),
                            )
                        });

                let mut tabs = div()
                    .flex_none()
                    .h(px(32.0))
                    .px(px(8.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0));
                for tab in PrTab::ALL {
                    let active = self.pr_tab == tab;
                    tabs = tabs.child(
                        div()
                            .id(SharedString::from(format!("rail-pr-tab-{}", tab.label())))
                            .h_full()
                            .px(px(9.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .border_b_2()
                            .border_color(if active {
                                theme.accent
                            } else {
                                gpui::transparent_black()
                            })
                            .cursor_pointer()
                            .text_size(crate::typography::ui_rems(11.0))
                            .font_weight(if active {
                                gpui::FontWeight::MEDIUM
                            } else {
                                gpui::FontWeight::NORMAL
                            })
                            .text_color(if active { theme.text } else { theme.text_muted })
                            .child(tab.label())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pr_tab = tab;
                                cx.notify();
                            })),
                    );
                }

                let tab_body = self.render_pr_tab(pr, theme, cx);
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(tabs)
                    .child(
                        div()
                            .id("rail-pr-detail")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(tab_body),
                    )
                    .into_any_element()
            }
            Loadable::Error(err) => {
                pane_empty(theme, icons::DANGER_TRIANGLE, "Pull request", err).into_any_element()
            }
            _ => div()
                .p(px(12.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .child("Loading…")
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(back)
            .child(body)
            .into_any_element()
    }

    /// Body of the active PR sub-tab — Changes/Description/Discussion/
    /// Commits/Checks/Bugs, matching the reference screenshot's strip.
    fn render_pr_tab(
        &mut self,
        pr: &PrDetail,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.pr_tab {
            PrTab::Changes => {
                let mut col = div().flex().flex_col().gap(px(8.0)).p(px(10.0));
                let pr_for_analysis = pr.clone();
                col = col.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            popover::btn_primary(theme, "Run analysis")
                                .id("rail-pr-analyze")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.run_analysis(&pr_for_analysis, cx);
                                })),
                        )
                        .child(match &self.analysis {
                            Loadable::Loading => div()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.text_muted)
                                .child("Queueing review chat…")
                                .into_any_element(),
                            Loadable::Ready(Some(_)) => div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.0))
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.success)
                                .child(
                                    icon(icons::QUEUE_CHECK)
                                        .size(px(11.0))
                                        .text_color(theme.success),
                                )
                                .child("Review chat created — see the chat list")
                                .into_any_element(),
                            Loadable::Ready(None) | Loadable::Idle => div().into_any_element(),
                            Loadable::Error(err) => div()
                                .flex_1()
                                .min_w_0()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.danger)
                                .child(SharedString::from(format!("Analysis failed: {err}")))
                                .into_any_element(),
                        }),
                );
                if pr.files.is_empty() {
                    col = col.child(
                        div()
                            .p(px(12.0))
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child("No file changes reported."),
                    );
                } else {
                    let mut files = div().flex().flex_col();
                    for file in &pr.files {
                        files = files.child(self.render_pr_file(file, theme, cx));
                    }
                    col = col.child(
                        div()
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(theme.border)
                            .child(files),
                    );
                }
                col.into_any_element()
            }
            PrTab::Description => {
                if pr.body.trim().is_empty() {
                    return pane_empty(
                        theme,
                        icons::LIST,
                        "No description",
                        "The author did not provide a description for this pull request.",
                    )
                    .into_any_element();
                }
                div()
                    .p(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.5))
                            .text_color(theme.text)
                            .child(SharedString::from(pr.body.clone())),
                    )
                    .into_any_element()
            }
            PrTab::Discussion => {
                if pr.discussion.is_empty() {
                    return pane_empty(
                        theme,
                        icons::LIST,
                        "No discussion yet",
                        "Comments and reviews on this pull request will appear here.",
                    )
                    .into_any_element();
                }
                let mut col = div().flex().flex_col().p(px(10.0)).gap(px(8.0));
                for comment in &pr.discussion {
                    col = col.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(theme.border)
                            .p(px(8.0))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .text_size(crate::typography::ui_rems(10.5))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.text)
                                            .child(SharedString::from(comment.author.clone())),
                                    )
                                    .child(widgets::badge(
                                        theme,
                                        comment
                                            .state
                                            .clone()
                                            .unwrap_or_else(|| comment.kind.clone()),
                                    ))
                                    .child(div().flex_1())
                                    .child(
                                        div()
                                            .text_size(crate::typography::ui_rems(10.0))
                                            .text_color(theme.text_faint)
                                            .child(SharedString::from(format_time(
                                                &comment.created_at,
                                            ))),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(comment.body.clone())),
                            ),
                    );
                }
                col.into_any_element()
            }
            PrTab::Commits => {
                if pr.commit_list.is_empty() {
                    return pane_empty(
                        theme,
                        icons::GIT_BRANCH,
                        "No commits listed",
                        "The commit list for this pull request was not returned.",
                    )
                    .into_any_element();
                }
                let mut col = div().flex().flex_col().p(px(6.0));
                for commit in &pr.commit_list {
                    col = col.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(6.0))
                            .py(px(5.0))
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(10.0))
                                    .font_family(theme.font_mono.clone())
                                    .text_color(theme.accent)
                                    .child(SharedString::from(
                                        commit.sha.chars().take(7).collect::<String>(),
                                    )),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text)
                                    .child(SharedString::from(commit.message.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(10.0))
                                    .text_color(theme.text_faint)
                                    .child(SharedString::from(format!(
                                        "{} · {}",
                                        commit.author,
                                        format_time(&commit.date)
                                    ))),
                            ),
                    );
                }
                col.into_any_element()
            }
            PrTab::Checks => {
                if pr.checks.is_empty() {
                    return pane_empty(
                        theme,
                        icons::CHECKLIST,
                        "No checks",
                        "This pull request has no CI checks configured.",
                    )
                    .into_any_element();
                }
                let mut col = div().flex().flex_col().p(px(6.0));
                for check in &pr.checks {
                    let ok = check
                        .conclusion
                        .as_deref()
                        .map(|c| c == "success")
                        .unwrap_or(false);
                    col = col.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(6.0))
                            .py(px(5.0))
                            .child(
                                icon(if ok {
                                    icons::QUEUE_CHECK
                                } else {
                                    icons::CLOCK_CIRCLE
                                })
                                .size(px(11.0))
                                .flex_none()
                                .text_color(if ok {
                                    theme.success
                                } else {
                                    theme.warning
                                }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text)
                                    .child(SharedString::from(check.name.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(10.0))
                                    .text_color(theme.text_faint)
                                    .child(SharedString::from(
                                        check
                                            .conclusion
                                            .clone()
                                            .unwrap_or_else(|| check.status.clone()),
                                    )),
                            ),
                    );
                }
                col.into_any_element()
            }
            PrTab::Bugs => pane_empty(
                theme,
                icons::DANGER_TRIANGLE,
                "No findings yet",
                "Run analysis to have the seat review this diff for bugs, \
                 regressions, and risky changes.",
            )
            .into_any_element(),
        }
    }

    /// One file row in the Changes tab — expands to show the inline patch.
    fn render_pr_file(
        &mut self,
        file: &PrFile,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = file.path.clone();
        let expanded = self.pr_expanded.contains(&file.path);
        let has_patch = file
            .patch
            .as_deref()
            .map(|p| !p.trim().is_empty())
            .unwrap_or(false);
        let status_label = match file.status.as_str() {
            "added" => "A",
            "removed" | "deleted" => "D",
            "renamed" => "R",
            _ => "M",
        };
        let status_color = match file.status.as_str() {
            "added" => theme.success,
            "removed" | "deleted" => theme.danger,
            "renamed" => theme.accent,
            _ => theme.text_muted,
        };
        let path_for_toggle = path.clone();
        let mut wrap = div().flex().flex_col();
        wrap = wrap.child(
            div()
                .id(SharedString::from(format!("rail-pr-file-{path}")))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .h(px(26.0))
                .px(px(8.0))
                .cursor_pointer()
                .hover(|el| el.bg(theme.element_hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.pr_expanded.remove(&path_for_toggle) {
                        this.pr_expanded.insert(path_for_toggle.clone());
                    }
                    cx.notify();
                }))
                .child(
                    icon(if expanded {
                        icons::ALT_ARROW_DOWN
                    } else {
                        icons::ALT_ARROW_RIGHT
                    })
                    .size(px(9.0))
                    .flex_none()
                    .text_color(theme.text_faint),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(11.0))
                        .font_family(theme.font_mono.clone())
                        .text_color(theme.text)
                        .child(SharedString::from(path)),
                )
                .child(
                    div()
                        .flex_none()
                        .rounded(px(3.0))
                        .px(px(4.0))
                        .py(px(1.0))
                        .bg(status_color.opacity(0.15))
                        .text_size(crate::typography::ui_rems(9.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(status_color)
                        .child(status_label),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(crate::typography::ui_rems(10.0))
                        .font_family(theme.font_mono.clone())
                        .child(
                            gpui::div()
                                .text_color(theme.diff_add)
                                .child(SharedString::from(format!("+{}", file.additions))),
                        )
                        .child(
                            gpui::div()
                                .text_color(theme.diff_del)
                                .child(SharedString::from(format!(" -{}", file.deletions))),
                        ),
                ),
        );
        if expanded {
            match &file.patch {
                Some(patch) if has_patch => {
                    let mut lines = div().flex().flex_col().px(px(4.0)).py(px(4.0));
                    for (kind, text) in diff_lines(patch) {
                        let color = match kind {
                            '+' => theme.diff_add,
                            '-' => theme.diff_del,
                            '@' => theme.accent,
                            _ => theme.text_muted,
                        };
                        lines = lines.child(
                            div()
                                .text_size(px(10.5))
                                .font_family(theme.font_mono.clone())
                                .text_color(color)
                                .child(SharedString::from(text)),
                        );
                    }
                    wrap = wrap.child(
                        div()
                            .border_t_1()
                            .border_color(theme.border)
                            .bg(theme.surface.opacity(0.5))
                            .child(lines),
                    );
                }
                _ => {
                    wrap = wrap.child(
                        div()
                            .border_t_1()
                            .border_color(theme.border)
                            .px(px(8.0))
                            .py(px(6.0))
                            .text_size(crate::typography::ui_rems(10.5))
                            .text_color(theme.text_faint)
                            .child("Patch too large — open the full view on GitHub."),
                    );
                }
            }
        }
        wrap.into_any_element()
    }

    fn render_computer(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let seat = self.seat.ready().cloned();
        let probe = seat.as_ref().and_then(|s| s.config.last_probe.clone());
        let mut col = div().size_full().flex().flex_col();
        col = col.child(
            div()
                .flex_none()
                .h(px(36.0))
                .px(px(10.0))
                .border_b_1()
                .border_color(theme.border)
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(status_dot(theme, probe.as_ref()))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(11.5))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(self.binding.label.clone())),
                )
                .child(
                    div()
                        .id("rail-shot-refresh")
                        .cursor_pointer()
                        .when(self.capabilities().computer, |el| {
                            el.on_click(cx.listener(|this, _, _, cx| this.capture_shot(cx)))
                        })
                        .opacity(if self.capabilities().computer {
                            1.0
                        } else {
                            0.35
                        })
                        .child(
                            icon(icons::REFRESH)
                                .size(px(13.0))
                                .text_color(theme.text_muted),
                        ),
                ),
        );
        let body = match &self.shot {
            Loadable::Ready(Some(image)) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    gpui::img(image.clone())
                        .max_w_full()
                        .max_h_full()
                        .object_fit(gpui::ObjectFit::Contain),
                )
                .into_any_element(),
            Loadable::Ready(None) => {
                let keeps = self.capabilities().keeps_running;
                pane_empty(
                    theme,
                    icons::MONITOR,
                    "seat not controllable yet",
                    if keeps {
                        "No remote CUA endpoint is stamped for this seat. \
                         Files, Changes, Shell and Pull requests still work \
                         through git/ssh/docker."
                    } else {
                        "No remote CUA endpoint is stamped for this seat, and \
                         it runs only while the desktop app is open — no \
                         remote harness is detached yet."
                    },
                )
                .into_any_element()
            }
            Loadable::Error(err) => {
                pane_empty(theme, icons::DANGER_TRIANGLE, "Seat preview", err).into_any_element()
            }
            _ => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .child("Capturing…")
                .into_any_element(),
        };
        col = col.child(body);
        col.into_any_element()
    }

    /// Progress: the run's tool calls as a linear step list, read back out of
    /// the live transcript — same derivation the transcript itself uses.
    fn render_progress(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let steps: Vec<(String, bool)> = self
            .state
            .read(cx)
            .transcript
            .iter()
            .flat_map(|entry| entry.parts.iter())
            .filter_map(|part| match part {
                zeron_doc::MessagePart::Tool {
                    call,
                    resolved,
                    is_error,
                    ..
                } => Some((tool_step_name(call), *resolved && !*is_error)),
                _ => None,
            })
            .collect();
        if steps.is_empty() {
            return pane_empty(
                theme,
                icons::SORT_VERTICAL,
                "Progress",
                "The run's tool calls land here as steps while the agent works.",
            )
            .into_any_element();
        }
        let mut list = div().flex().flex_col().p(px(8.0));
        for (ix, (name, done)) in steps.iter().enumerate() {
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .h(px(24.0))
                    .child(
                        icon(if *done {
                            icons::QUEUE_CHECK
                        } else {
                            icons::CLOCK_CIRCLE
                        })
                        .size(px(11.0))
                        .flex_none()
                        .text_color(if *done {
                            theme.success
                        } else {
                            theme.accent
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(if *done { theme.text_muted } else { theme.text })
                            .child(SharedString::from(name.clone())),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(10.0))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(if *done { "done" } else { "running" })),
                    ),
            );
            let _ = ix;
        }
        div()
            .id("rail-progress-list")
            .size_full()
            .overflow_y_scroll()
            .child(list)
            .into_any_element()
    }

    /// Context: the files the run has touched (edit-shaped tool calls) plus
    /// prompt-attached paths — the seat's read set.
    fn render_context(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut paths: Vec<String> = Vec::new();
        for entry in &self.state.read(cx).transcript {
            for part in &entry.parts {
                if let zeron_doc::MessagePart::Tool { call, .. } = part {
                    for path in tool_paths(call) {
                        if !paths.contains(&path) {
                            paths.push(path);
                        }
                    }
                }
            }
        }
        if paths.is_empty() {
            return pane_empty(
                theme,
                icons::FOLDER_WITH_FILES,
                "Context",
                "Files the run reads or edits attach here as it works.",
            )
            .into_any_element();
        }
        let mut list = div().flex().flex_col().p(px(8.0));
        for path in paths {
            list = list.child(
                div()
                    .h(px(24.0))
                    .px(px(4.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        icon(icons::DOCUMENT)
                            .size(px(12.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(11.5))
                            .font_family(theme.font_mono.clone())
                            .text_color(theme.text)
                            .child(SharedString::from(path)),
                    ),
            );
        }
        div()
            .id("rail-context-list")
            .size_full()
            .overflow_y_scroll()
            .child(list)
            .into_any_element()
    }

    /// Tasks: the run's live checklist — the latest plan-shaped tool part.
    fn render_tasks(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let items: Vec<(String, bool)> = self
            .state
            .read(cx)
            .transcript
            .iter()
            .flat_map(|e| e.parts.iter())
            .filter_map(|part| match part {
                zeron_doc::MessagePart::Tool {
                    call: zeron_proto::ToolCall::Todo { items },
                    ..
                } => Some(
                    items
                        .iter()
                        .map(|i| (i.text.clone(), i.done))
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .next_back()
            .unwrap_or_default();
        if items.is_empty() {
            return pane_empty(
                theme,
                icons::CHECKLIST,
                "Tasks",
                "The agent's checklist lands here once the run publishes a plan.",
            )
            .into_any_element();
        }
        let mut list = div().flex().flex_col().p(px(8.0));
        for (text, done) in items {
            list = list.child(
                div()
                    .min_h(px(24.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        icon(if done {
                            icons::QUEUE_CHECK
                        } else {
                            icons::CHECKLIST
                        })
                        .size(px(11.0))
                        .flex_none()
                        .text_color(if done {
                            theme.success
                        } else {
                            theme.text_muted
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(if done { theme.text_muted } else { theme.text })
                            .child(SharedString::from(text)),
                    ),
            );
        }
        div()
            .id("rail-tasks-list")
            .size_full()
            .overflow_y_scroll()
            .child(list)
            .into_any_element()
    }

    fn render_simulator(&mut self, theme: &Theme, _cx: &mut Context<Self>) -> AnyElement {
        pane_empty(
            theme,
            icons::SMARTPHONE,
            "Android Simulator",
            "No emulator endpoint on this seat — Linux seats can expose one; \
             the pane stays disabled until the seat advertises it.",
        )
        .into_any_element()
    }
}

/// Split a unified-diff patch into styled lines: '+' additions, '-' deletions,
/// '@' hunk headers, ' ' context (the first char drives the color).
fn diff_lines(patch: &str) -> Vec<(char, String)> {
    patch
        .lines()
        .map(|line| {
            let kind = match line.chars().next() {
                Some('+') => '+',
                Some('-') => '-',
                Some('@') => '@',
                _ => ' ',
            };
            (kind, line.to_string())
        })
        .collect()
}

/// Compact relative timestamp for GitHub RFC3339 strings ("5m", "3h", …).
fn format_time(iso: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(iso)
        .map(|at| crate::state::format_time_ago(at.with_timezone(&chrono::Utc), chrono::Utc::now()))
        .unwrap_or_else(|_| iso.chars().take(10).collect())
}

fn format_size(size: u64) -> String {
    if size >= 1_000_000 {
        format!("{:.1}M", size as f64 / 1_000_000.0)
    } else if size >= 1_000 {
        format!("{:.1}K", size as f64 / 1_000.0)
    } else {
        format!("{size}B")
    }
}

fn motion_row_bg(fade: &str, theme: &Theme) -> gpui::Hsla {
    crate::motion::hover_blend(fade, gpui::transparent_black(), theme.element_hover)
}

/// Human name for a tool call on the Progress pane.
fn tool_step_name(call: &zeron_proto::ToolCall) -> String {
    use zeron_proto::ToolCall::*;
    match call {
        ReadFile { path } => format!("Read {path}"),
        EditFile { path, .. } => format!("Edit {path}"),
        WriteFile { path, .. } => format!("Write {path}"),
        ApplyPatch { path } => {
            format!("Patch {}", path.as_deref().unwrap_or("files"))
        }
        Exec { command } => {
            let short: String = command.chars().take(60).collect();
            format!("$ {short}")
        }
        Search { pattern, .. } => format!("Search {pattern}"),
        Glob { pattern } => format!("Glob {pattern}"),
        Todo { .. } => "Plan update".into(),
        WebFetch { url, .. } => format!("Fetch {url}"),
        WebSearch { query } => format!("Web search {query}"),
        Mcp { server, tool, .. } => format!("{server}:{tool}"),
        Unknown { name, .. } => name.clone(),
    }
}

/// File paths a tool call touches — the Context pane's source.
fn tool_paths(call: &zeron_proto::ToolCall) -> Vec<String> {
    use zeron_proto::ToolCall::*;
    match call {
        ReadFile { path } | EditFile { path, .. } | WriteFile { path, .. } => vec![path.clone()],
        ApplyPatch { path } => path.clone().into_iter().collect(),
        Search { path, .. } => path.clone().into_iter().collect(),
        _ => Vec::new(),
    }
}

impl CloudRail {
    /// The rail's pane menu: every `CloudPane`, plus the Side chat escape
    /// that opens the existing side-chat surface in the shell's pane host.
    fn render_pane_menu(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let caps = self.capabilities();
        let mut menu = popover::popover_card_opaque(&theme.for_popup()).w(px(200.0));
        menu = menu.on_mouse_down_out(cx.listener(|this, _, _, cx| {
            this.close_pane_menu(cx);
        }));
        for pane in CloudPane::ALL {
            let disabled = match pane {
                CloudPane::Simulator => !caps.simulator,
                CloudPane::PullRequests => !caps.pull_requests,
                _ => false,
            };
            let active = pane == self.pane;
            let fade = SharedString::from(format!("rail-menu-{}", pane.label()));
            let mut row = popover::menu_row(theme, active, fade)
                .id(SharedString::from(format!("rail-pane-{}", pane.label())))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    icon(seat_pane_icon(pane))
                        .size(px(13.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(pane.label())),
                );
            if disabled {
                row = row.opacity(0.45).child(
                    div()
                        .flex_none()
                        .text_size(crate::typography::ui_rems(9.5))
                        .text_color(theme.text_faint)
                        .child(if pane == CloudPane::Simulator {
                            "no endpoint"
                        } else {
                            "no repo"
                        }),
                );
            } else {
                row = row
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.set_pane(pane, cx)));
            }
            menu = menu.child(row);
        }
        // Side chat opens the existing surface — the rail hands it to the
        // shell via an event, never a second chat column of its own.
        let fade = SharedString::from("rail-menu-sidechat");
        menu = menu.child(
            popover::menu_row(theme, false, fade)
                .id("rail-pane-sidechat")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .cursor_pointer()
                .child(
                    icon(icons::CHAT_ROUND_LINE)
                        .size(px(13.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(div().flex_1().min_w_0().truncate().child("Side chat"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_pane_menu(cx);
                    cx.emit(RailEvent::OpenSideChat);
                })),
        );
        menu.into_any_element()
    }

    /// The bot control area — seat, model, run status, and the honest
    /// controls the engine backs: pause/resume/stop, approve (only while the
    /// run is `NeedsApproval`), preview, destroy.
    fn render_bot_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let caps = self.capabilities();
        let seat_label = self
            .seat
            .ready()
            .map(|s| s.config.label.clone())
            .unwrap_or_else(|| self.binding.label.clone());
        let model = self
            .binding
            .model_label
            .clone()
            .or_else(|| self.model_label.as_ref().map(|m| m.to_string()))
            .unwrap_or_else(|| "seat CLI login".into());
        let run_status = self.run.as_ref().map(|r| r.status);
        let probe = self.seat.ready().and_then(|s| s.config.last_probe.clone());
        let busy = self.control_busy;

        let mut bar = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .h(px(30.0))
            .px(px(10.0))
            .border_t_1()
            .border_color(theme.border)
            .child(status_dot(theme, probe.as_ref()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(11.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(SharedString::from(seat_label)),
            )
            .child(widgets::badge(theme, model))
            .when_some(run_status, |el, status| {
                el.child(widgets::badge(theme, status.label()))
            })
            // The Railway countdown chip — the spec's required live MM:SS.
            // Reads the persisted binding deadline so every device agrees.
            .when_some(
                self.binding
                    .expires_at
                    .map(|at| at.signed_duration_since(chrono::Utc::now())),
                |el, left| {
                    let secs = left.num_seconds().max(0);
                    let text = format!("{:02}:{:02}", secs / 60, secs % 60);
                    el.child(
                        div()
                            .id("rail-countdown")
                            .flex_none()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded(px(6.0))
                            .bg(theme.warning.opacity(0.15))
                            .text_size(crate::typography::ui_rems(10.5))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.warning)
                            .child(text)
                            .tooltip(widgets::text_tooltip("trial time left")),
                    )
                },
            );
        // Controls — each rides CLOUD_SEAT_CONTROL; never decorative.
        let mut controls = div().flex_none().flex().flex_row().gap(px(2.0));
        let small_btn = |id: &'static str, label: &'static str, theme: &Theme| {
            let fade = SharedString::from(format!("rail-ctl-{id}"));
            div()
                .id(SharedString::from(format!("rail-ctl-{id}")))
                .px(px(6.0))
                .py(px(3.0))
                .rounded(px(5.0))
                .text_size(crate::typography::ui_rems(10.5))
                .text_color(theme.text_muted)
                .bg(crate::motion::hover_blend(
                    &fade,
                    gpui::transparent_black(),
                    theme.element_hover,
                ))
                .on_hover(crate::motion::hover_listener(fade))
                .cursor_pointer()
                .child(SharedString::from(label))
        };
        match run_status {
            Some(CloudRunStatus::Working) | Some(CloudRunStatus::Queued) => {
                controls = controls.child(
                    small_btn("pause", "Pause", theme)
                        .when(busy, |el| el.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.seat_control("pause", cx))),
                );
                controls = controls.child(
                    small_btn("stop", "Stop", theme)
                        .when(busy, |el| el.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.seat_control("stop", cx))),
                );
            }
            Some(CloudRunStatus::Stopped) => {
                controls = controls.child(
                    small_btn("resume", "Resume", theme)
                        .when(busy, |el| el.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.seat_control("resume", cx))),
                );
            }
            Some(CloudRunStatus::NeedsApproval) => {
                controls = controls.child(
                    small_btn("approve", "Approve", theme)
                        .when(busy, |el| el.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.seat_control("approve", cx))),
                );
            }
            _ => {}
        }
        // Boat lifecycle rides the same control RPC — Sleep archives the box
        // (billing off); Wake resumes it. Only meaningful for a Box seat.
        let is_boat = self
            .seat
            .ready()
            .is_some_and(|s| s.config.kind == Some(CloudSeatKind::Box));
        let sleeping = probe.as_ref().is_some_and(|p| p.sleeping);
        if is_boat && self.seat.ready().is_some_and(|s| s.config.box_id.is_some()) {
            let (id, label, op) = if sleeping {
                ("wake", "Wake", "wake")
            } else {
                ("sleep", "Sleep", "sleep")
            };
            controls = controls.child(
                small_btn(id, label, theme)
                    .when(busy, |el| el.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| this.seat_control(op, cx))),
            );
        }
        // Open preview: a provider-stamped URL (Railway *.up.railway.app,
        // Boat desktop) opens in the user's browser; else the Computer pane
        // carries the last screenshot.
        let preview_url = self.seat.ready().and_then(|s| s.config.preview_url.clone());
        controls = controls.child(match preview_url {
            Some(url) => {
                small_btn("preview", "Preview", theme).on_click(move |_, _, cx| cx.open_url(&url))
            }
            None => small_btn("preview", "Preview", theme)
                .on_click(cx.listener(|this, _, _, cx| this.set_pane(CloudPane::Computer, cx))),
        });
        let destroy = if self.confirm_destroy {
            small_btn("destroy-yes", "Destroy?", theme)
                .text_color(theme.danger)
                .on_click(cx.listener(|this, _, _, cx| this.seat_control("destroy", cx)))
        } else {
            small_btn("destroy", "Destroy", theme)
                .when(!caps.exec, |el| el.opacity(0.45))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_destroy = true;
                    cx.notify();
                }))
        };
        bar = bar.child(controls.child(destroy));
        bar.into_any_element()
    }
}

impl Render for CloudRail {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let content = match self.pane {
            CloudPane::Files => self.render_files(&theme, cx),
            CloudPane::Changes => self.render_changes(&theme, cx),
            CloudPane::Shell => self.render_shell(&theme, cx),
            CloudPane::PullRequests => self.render_pull_requests(&theme, cx),
            CloudPane::Computer => self.render_computer(&theme, cx),
            CloudPane::Simulator => self.render_simulator(&theme, cx),
            CloudPane::Progress => self.render_progress(&theme, cx),
            CloudPane::Context => self.render_context(&theme, cx),
            CloudPane::Tasks => self.render_tasks(&theme, cx),
        };
        // The + button top-left opens the pane menu — the screenshot's IA.
        let mut plus = div()
            .id("rail-plus")
            .size(px(20.0))
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(px(5.0))
            .text_size(crate::typography::ui_rems(14.0))
            .text_color(theme.text_muted)
            .bg(crate::motion::hover_blend(
                "rail-plus",
                gpui::transparent_black(),
                theme.element_hover,
            ))
            .on_hover(crate::motion::hover_listener("rail-plus"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.pane_menu.note_trigger_press()),
            )
            .cursor_pointer()
            .child("+")
            .on_click(cx.listener(|this, _, _, cx| {
                if this.pane_menu.take_press_was_open() {
                    this.close_pane_menu(cx);
                } else {
                    this.pane_menu.open(());
                    cx.notify();
                }
            }));
        if self.pane_menu.get().is_some() {
            plus = plus.relative().child(popover::anchored_menu_below(
                "cloud-rail-pane-menu",
                self.render_pane_menu(&theme, cx),
                self.pane_menu.closing_since(),
            ));
        }
        let header = div()
            .relative()
            .flex_none()
            .h(px(32.0))
            .px(px(8.0))
            .border_b_1()
            .border_color(theme.border)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(plus)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(11.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(SharedString::from(self.pane.label())),
            );
        let bot_bar = self.render_bot_bar(&theme, cx);
        let seat_row = self.seat.ready().cloned();
        let is_railway = seat_row
            .as_ref()
            .is_some_and(|s| s.config.kind == Some(CloudSeatKind::Railway))
            || self.binding.kind == CloudSeatKind::Railway;
        let railway_expired = is_railway
            && (seat_row.as_ref().is_some_and(|s| s.config.expired())
                || self
                    .binding
                    .expires_at
                    .is_some_and(|at| at <= chrono::Utc::now()));
        let is_boat = seat_row
            .as_ref()
            .is_some_and(|s| s.config.kind == Some(CloudSeatKind::Box));
        let sleeping = seat_row
            .as_ref()
            .and_then(|s| s.config.last_probe.as_ref())
            .is_some_and(|p| p.sleeping);
        let claim_url = seat_row.as_ref().and_then(|s| s.config.claim_url.clone());
        let backed_up = seat_row
            .as_ref()
            .is_some_and(|s| s.config.backup_at.is_some());
        let not_persistent = !self.capabilities().keeps_running && !sleeping && !railway_expired;
        // The "Keep this computer" card shows once: a Railway (or
        // desk-bound) seat that finished a run and has never been asked.
        let show_keep_prompt = is_railway
            && self.run.as_ref().is_some_and(|r| {
                matches!(
                    r.status,
                    CloudRunStatus::Done | CloudRunStatus::Stopped | CloudRunStatus::Error
                )
            })
            && self.keep_prompted.is_none()
            && seat_row
                .as_ref()
                .is_some_and(|s| s.config.keep_prompted_at.is_none());
        let mut col = div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(bot_bar)
            .when(not_persistent, |el| {
                el.child(
                    div()
                        .flex_none()
                        .px(px(10.0))
                        .py(px(6.0))
                        .border_b_1()
                        .border_color(theme.border)
                        .text_size(crate::typography::ui_rems(10.5))
                        .text_color(theme.warning)
                        .child("This seat runs only while the desktop app is open"),
                )
            })
            // Railway's one-line death sentence + the claim escape hatch.
            .when(is_railway && !railway_expired, |el| {
                el.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(10.0))
                        .py(px(6.0))
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.warning)
                                .child(
                                    "This seat dies when the hour ends unless you claim it or attach Boat",
                                ),
                        )
                        .when_some(claim_url.clone(), |el, url| {
                            el.child(
                                div()
                                    .id("rail-claim")
                                    .flex_none()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(5.0))
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.accent)
                                    .bg(crate::motion::hover_blend(
                                        "rail-claim",
                                        gpui::transparent_black(),
                                        theme.element_hover,
                                    ))
                                    .on_hover(crate::motion::hover_listener("rail-claim"))
                                    .cursor_pointer()
                                    .child("Claim")
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        }),
                )
            })
            .when(railway_expired, |el| {
                el.child(
                    div()
                        .flex_none()
                        .px(px(10.0))
                        .py(px(6.0))
                        .border_b_1()
                        .border_color(theme.border)
                        .text_size(crate::typography::ui_rems(10.5))
                        .text_color(theme.danger)
                        .child(if backed_up {
                            "Trial ended — workspace backed up locally; a Boat seat restores it on its first run"
                        } else {
                            "Trial ended — claim the VM from the link or start a new seat"
                        }),
                )
            })
            .when(is_boat && sleeping, |el| {
                el.child(
                    div()
                        .flex_none()
                        .px(px(10.0))
                        .py(px(6.0))
                        .border_b_1()
                        .border_color(theme.border)
                        .text_size(crate::typography::ui_rems(10.5))
                        .text_color(theme.warning)
                        .child("Boat box is sleeping — billing off; Wake to resume"),
                )
            })
            // The one-shot "Keep this computer" card — both answers stamp
            // `markKeepPrompted` so it never asks twice.
            .when(show_keep_prompt, |el| {
                el.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .px(px(10.0))
                        .py(px(8.0))
                        .m(px(8.0))
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(theme.accent.opacity(0.4))
                        .bg(theme.accent.opacity(0.08))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(11.5))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(if railway_expired {
                                    "Keep going on a persistent PC"
                                } else {
                                    "Keep this computer?"
                                }),
                        )
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.text_muted)
                                .child(if railway_expired {
                                    "The trial hour has ended. Claim the box on Railway, or attach a Boat token for a persistent cloud PC."
                                } else {
                                    "Attach a Boat token to keep a persistent cloud PC — the trial's hour is almost up."
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .id("keep-yes")
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(5.0))
                                        .text_size(crate::typography::ui_rems(10.5))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(theme.accent)
                                        .bg(crate::motion::hover_blend(
                                            "keep-yes",
                                            gpui::transparent_black(),
                                            theme.element_hover,
                                        ))
                                        .on_hover(crate::motion::hover_listener("keep-yes"))
                                        .cursor_pointer()
                                        .child("Attach Boat token")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.mark_keep_prompted(cx);
                                            cx.emit(RailEvent::OpenCloudSettings);
                                        })),
                                )
                                .when_some(
                                    railway_expired.then(|| claim_url.clone()).flatten(),
                                    |row, url| {
                                        row.child(
                                            div()
                                                .id("keep-claim")
                                                .px(px(8.0))
                                                .py(px(3.0))
                                                .rounded(px(5.0))
                                                .text_size(crate::typography::ui_rems(10.5))
                                                .font_weight(gpui::FontWeight::MEDIUM)
                                                .text_color(theme.accent)
                                                .bg(crate::motion::hover_blend(
                                                    "keep-claim",
                                                    gpui::transparent_black(),
                                                    theme.element_hover,
                                                ))
                                                .on_hover(crate::motion::hover_listener(
                                                    "keep-claim",
                                                ))
                                                .cursor_pointer()
                                                .child("Claim")
                                                .on_click(move |_, _, cx| cx.open_url(&url)),
                                        )
                                    },
                                )
                                .child(
                                    div()
                                        .id("keep-no")
                                        .px(px(8.0))
                                        .py(px(3.0))
                                        .rounded(px(5.0))
                                        .text_size(crate::typography::ui_rems(10.5))
                                        .text_color(theme.text_muted)
                                        .bg(crate::motion::hover_blend(
                                            "keep-no",
                                            gpui::transparent_black(),
                                            theme.element_hover,
                                        ))
                                        .on_hover(crate::motion::hover_listener("keep-no"))
                                        .cursor_pointer()
                                        .child("Not now")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.mark_keep_prompted(cx);
                                        })),
                                ),
                        ),
                )
            })
            .when_some(self.control_error.clone(), |el, err| {
                el.child(
                    div()
                        .flex_none()
                        .px(px(10.0))
                        .py(px(4.0))
                        .text_size(crate::typography::ui_rems(10.5))
                        .text_color(theme.danger)
                        .child(SharedString::from(err)),
                )
            });
        match &self.seat {
            Loadable::Error(err) => {
                col = col.child(div().flex_1().min_h_0().child(
                    pane_empty(&theme, icons::CLOUD, "Cloud seat", err.as_str()).into_any_element(),
                ));
            }
            _ => {
                col = col.child(div().flex_1().min_h_0().child(content));
            }
        }
        col
    }
}

impl Focusable for CloudRail {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.shell_input.focus_handle(cx)
    }
}

// ---------------------------------------------------------------------------
// Settings → Models (BYOK providers for cloud sessions)
// ---------------------------------------------------------------------------

struct ProviderRow {
    provider: ByokProvider,
    key: Entity<ComposerInput>,
    base_url: Entity<ComposerInput>,
    model: Entity<ComposerInput>,
    use_for_cloud: bool,
    /// `ok` / `error` status line after a Verify or Save.
    status: Option<(bool, String)>,
}

/// Settings → Models: BYOK keys the cloud seat's harness consumes. The key
/// field is write-only — the engine stores it in `cloud-keys.json` and never
/// echoes it back; a saved row shows "Key saved" instead of the secret.
pub struct ModelsPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    view: Loadable<CloudView>,
    rows: Vec<ProviderRow>,
    busy: Option<ByokProvider>,
    error: Option<String>,
    load_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
}

impl ModelsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let rows = ByokProvider::ALL
            .into_iter()
            .map(|provider| ProviderRow {
                provider,
                key: cx.new(|cx| ComposerInput::new("Key / access token…", cx)),
                base_url: cx.new(|cx| ComposerInput::new("Base URL (optional)", cx)),
                model: cx.new(|cx| ComposerInput::new("Default model id (optional)", cx)),
                use_for_cloud: false,
                status: None,
            })
            .collect();
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            view: Loadable::Idle,
            rows,
            busy: None,
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
        self.view = Loadable::Loading;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::CLOUD_GET_CONFIG, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                page.view = match result {
                    Ok(value) => match serde_json::from_value::<CloudView>(value) {
                        Ok(view) => {
                            for row in page.rows.iter_mut() {
                                if let Some(saved) = view
                                    .providers
                                    .iter()
                                    .find(|p| p.provider == Some(row.provider))
                                {
                                    row.use_for_cloud = saved.use_for_cloud;
                                    if let Some(model) = &saved.model {
                                        row.model.update(cx, |i, cx| i.set_text(model, cx));
                                    }
                                    if let Some(base) = &saved.base_url {
                                        row.base_url.update(cx, |i, cx| i.set_text(base, cx));
                                    }
                                    if saved.has_key {
                                        row.key.update(cx, |i, cx| {
                                            i.set_text("", cx);
                                            i.set_placeholder("Key saved — type to replace", cx);
                                        });
                                    }
                                }
                            }
                            Loadable::Ready(view)
                        }
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn verify(&mut self, provider: ByokProvider, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(row) = self.rows.iter().find(|r| r.provider == provider) else {
            return;
        };
        let key = row.key.read(cx).text().trim().to_string();
        let base_url = row.base_url.read(cx).text().trim().to_string();
        self.busy = Some(provider);
        let row_status = &mut self
            .rows
            .iter_mut()
            .find(|r| r.provider == provider)
            .expect("checked")
            .status;
        *row_status = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_VERIFY_PROVIDER,
                    serde_json::json!({
                        "provider": provider.id(),
                        "key": key,
                        "baseUrl": base_url,
                    }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.busy = None;
                if let Some(row) = page.rows.iter_mut().find(|r| r.provider == provider) {
                    row.status = Some(match result {
                        Ok(value) => {
                            let ok = value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                            let message = value
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("")
                                .to_string();
                            (
                                ok,
                                if message.is_empty() {
                                    "Verified".into()
                                } else {
                                    message
                                },
                            )
                        }
                        Err(err) => (false, err.to_string()),
                    });
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn save(&mut self, provider: ByokProvider, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(row) = self.rows.iter().find(|r| r.provider == provider) else {
            return;
        };
        let key = row.key.read(cx).text().trim().to_string();
        let base_url = row.base_url.read(cx).text().trim().to_string();
        let model = row.model.read(cx).text().trim().to_string();
        let use_for_cloud = row.use_for_cloud;
        self.busy = Some(provider);
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_SAVE_PROVIDER,
                    serde_json::json!({
                        "provider": provider.id(),
                        "key": key,
                        "baseUrl": base_url,
                        "model": model,
                        "useForCloud": use_for_cloud,
                    }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.busy = None;
                match result {
                    Ok(_) => {
                        if let Some(row) = page.rows.iter_mut().find(|r| r.provider == provider) {
                            row.status = Some((true, "Saved".into()));
                            row.key.update(cx, |i, cx| {
                                i.set_text("", cx);
                                i.set_placeholder("Key saved — type to replace", cx);
                            });
                        }
                        page.load(cx);
                        page.state
                            .update(cx, |state, cx| state.refresh_cloud(None, cx));
                    }
                    Err(err) => {
                        if let Some(row) = page.rows.iter_mut().find(|r| r.provider == provider) {
                            row.status = Some((false, err.to_string()));
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn remove(&mut self, provider: ByokProvider, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.busy = Some(provider);
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_REMOVE_PROVIDER,
                    serde_json::json!({ "provider": provider.id() }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.busy = None;
                if let Err(err) = result {
                    page.error = Some(err.to_string());
                } else if let Some(row) = page.rows.iter_mut().find(|r| r.provider == provider) {
                    row.status = None;
                    row.use_for_cloud = false;
                    row.key.update(cx, |i, cx| {
                        i.set_text("", cx);
                        i.set_placeholder("Key / access token…", cx);
                    });
                    row.model.update(cx, |i, cx| i.set_text("", cx));
                    row.base_url.update(cx, |i, cx| i.set_text("", cx));
                }
                page.load(cx);
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            })
            .ok();
        }));
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for ModelsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for ModelsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let scrollbar = popover::rail(self, "models-page-scrollbar", &theme, cx);
        let mut column = widgets::page_column()
            .child(widgets::page_header(&theme, "Models", None))
            .child(
                widgets::page_subtitle(
                    &theme,
                    "Keys the cloud seat's harness consumes — one row per provider. Keys are \
                     verified before saving and stored on this device only (cloud-keys.json); \
                     the seat's run gets them as environment variables, never the transcript.",
                )
                .max_w(px(560.0))
                .line_height(px(20.0)),
            );
        for index in 0..self.rows.len() {
            let provider = self.rows[index].provider;
            let saved = self
                .view
                .ready()
                .and_then(|v| v.providers.iter().find(|p| p.provider == Some(provider)))
                .cloned();
            let row = &self.rows[index];
            let has_key = saved.as_ref().map(|s| s.has_key).unwrap_or(false);
            let busy = self.busy == Some(provider);
            let mut card = widgets::section_card(&theme);
            card = card.child(
                widgets::card_row(&theme, true)
                    .child(widgets::row_tile(&theme, icons::KEY_MINIMALISTIC))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(widgets::row_title(&theme, provider.label()))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(11.5))
                                    .text_color(theme.text_muted)
                                    .child(SharedString::from(if provider.is_subscription() {
                                        "Subscription access token"
                                    } else {
                                        provider.default_base_url()
                                    })),
                            ),
                    )
                    .when(has_key, |el| {
                        el.child(widgets::badge_active(&theme, "Key saved"))
                    })
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(widgets::details_label(&theme, "Cloud"))
                            .child(
                                widgets::toggle_switch(
                                    &theme,
                                    row.use_for_cloud,
                                    format!("models-cloud-{index}"),
                                )
                                .id(("models-cloud-toggle", index))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if let Some(row) =
                                            this.rows.iter_mut().find(|r| r.provider == provider)
                                        {
                                            row.use_for_cloud = !row.use_for_cloud;
                                        }
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            );
            card = card.child(
                div()
                    .px(px(20.0))
                    .pb(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(popover::dialog_field(row.key.clone().into_any_element()))
                    .child(popover::dialog_field(
                        row.base_url.clone().into_any_element(),
                    ))
                    .child(popover::dialog_field(row.model.clone().into_any_element()))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                popover::btn_ghost(
                                    &theme,
                                    "Verify",
                                    format!("models-verify-{index}"),
                                )
                                .id(("models-verify", index))
                                .when(busy, |el| el.opacity(0.5))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.verify(provider, cx)),
                                ),
                            )
                            .child(
                                popover::btn_primary(&theme, "Save")
                                    .id(("models-save", index))
                                    .when(busy, |el| el.opacity(0.5))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| this.save(provider, cx)),
                                    ),
                            )
                            .when(has_key, |el| {
                                el.child(
                                    popover::btn_ghost(
                                        &theme,
                                        "Remove",
                                        format!("models-remove-{index}"),
                                    )
                                    .id(("models-remove", index))
                                    .when(busy, |el| el.opacity(0.5))
                                    .on_click(
                                        cx.listener(move |this, _, _, cx| {
                                            this.remove(provider, cx)
                                        }),
                                    ),
                                )
                            }),
                    )
                    .when_some(row.status.clone(), |el, (ok, message)| {
                        el.child(
                            div()
                                .text_size(crate::typography::ui_rems(11.5))
                                .text_color(if ok { theme.success } else { theme.danger })
                                .child(SharedString::from(message)),
                        )
                    }),
            );
            column = column.child(card);
        }
        div()
            .id("models-page-root")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div()
                    .id("models-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .child(column),
            )
            .children(scrollbar)
    }
}
