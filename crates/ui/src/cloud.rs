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
    AnyElement, App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, Window, div, prelude::*, px,
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
    /// Per-chat workspace snapshots held on this computer.
    pub backups: Vec<zeron_proto::WorkspaceBackup>,
    /// Kanban tasks saved but not started.
    pub drafts: Vec<zeron_proto::TaskDraft>,
    /// The shared GitHub PAT exists (the repo picker's token flag).
    pub has_github_pat: bool,
    pub load_error: Option<String>,
}

impl CloudView {
    pub fn seat(&self, seat_id: &str) -> Option<&SeatRow> {
        self.seats.iter().find(|s| s.config.id == seat_id)
    }

    pub fn backup_for_chat(&self, chat_id: &str) -> Option<&zeron_proto::WorkspaceBackup> {
        self.backups
            .iter()
            .filter(|b| b.chat_id == chat_id)
            .max_by_key(|b| b.taken_at)
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

    /// The seat new cloud chats should land on: the user's own persistent
    /// computer first (Boat, then a VPS), a live Railway trial as the
    /// on-ramp, a GitHub workspace last (it can't run a harness).
    pub fn preferred_seat(seats: &[SeatRow]) -> Option<&SeatRow> {
        let rank = |seat: &SeatRow| match seat.config.kind {
            Some(CloudSeatKind::Box) => 0,
            Some(CloudSeatKind::Ssh) => 1,
            Some(CloudSeatKind::Railway) => 2,
            Some(CloudSeatKind::GitHub) => 3,
            None => 4,
        };
        seats
            .iter()
            .filter(|seat| Self::seat_ready(seat))
            .min_by_key(|seat| rank(seat))
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
    fn countdown_ticks_at_the_next_minute_boundary() {
        assert_eq!(
            countdown_tick(3600),
            Some(std::time::Duration::from_secs(1))
        );
        assert_eq!(
            countdown_tick(3599),
            Some(std::time::Duration::from_secs(60))
        );
        assert_eq!(countdown_tick(61), Some(std::time::Duration::from_secs(2)));
        assert_eq!(countdown_tick(59), Some(std::time::Duration::from_secs(60)));
        assert_eq!(countdown_tick(0), None);
        assert_eq!(countdown_tick(-5), None);
    }

    #[gpui::test]
    fn cloud_rail_timers_follow_active_state(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let state = cx.new(|_| AppState::new());
            let binding = CloudSeatBinding {
                seat_id: "trial".into(),
                kind: CloudSeatKind::Railway,
                label: "Trial".into(),
                repo: None,
                workspace_path: None,
                runner_device_id: None,
                branch: None,
                model_label: None,
                expires_at: Some(Utc::now() + Duration::minutes(30)),
            };
            let rail = cx.new(|cx| {
                CloudRail::new(state.clone(), "chat".into(), binding, CloudPane::Files, cx)
            });
            rail.update(cx, |rail, cx| {
                assert!(rail._countdown.is_some());
                assert!(rail._refresh.is_some());
                rail.task = Some(cx.spawn(async move |_, _| {
                    std::future::pending::<()>().await;
                }));
                rail.act_task = Some(cx.spawn(async move |_, _| {
                    std::future::pending::<()>().await;
                }));
                rail.confirm_task = Some(cx.spawn(async move |_, _| {
                    std::future::pending::<()>().await;
                }));
                rail.control_task = Some(cx.spawn(async move |_, _| {
                    std::future::pending::<()>().await;
                }));
                rail.screen_task = Some(cx.spawn(async move |_, _| {
                    std::future::pending::<()>().await;
                }));
                rail.exec_busy = true;
                rail.desktop_starting = true;
                rail.backup_busy = true;
                assert!(rail.screen_task.is_some());
                rail.set_active(false, cx);
                assert!(rail._countdown.is_none());
                assert!(rail._refresh.is_none());
                assert!(rail.screen_task.is_none());
                assert!(rail.task.is_none());
                assert!(rail.act_task.is_none());
                assert!(rail.confirm_task.is_none());
                assert!(rail.control_task.is_some());
                assert!(!rail.exec_busy);
                assert!(!rail.desktop_starting);
                assert!(!rail.backup_busy);
                rail.set_active(true, cx);
                rail.control_task = None;
                assert!(rail._countdown.is_some());
                assert!(rail._refresh.is_some());
            });
        });
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

    #[test]
    fn trial_seats_read_honestly_on_the_chip() {
        let mut trial = seat(CloudSeatKind::Railway);
        trial.config.label = "Railway trial".into();
        assert_eq!(
            seat_chip_label(Some(&trial), false, false),
            "Railway trial — starting…"
        );
        assert_eq!(
            seat_chip_label(Some(&trial), true, false),
            "Railway trial — unavailable"
        );
        assert_eq!(
            seat_chip_label(Some(&trial), true, true),
            "Railway trial — starting…"
        );
        trial.config.signup_url = Some("https://railway.com/signup".into());
        assert_eq!(
            seat_chip_label(Some(&trial), false, false),
            "Railway trial — sign in to continue"
        );
        assert_eq!(seat_status_note(&trial), Some("sign in"));
        trial.config.signup_url = None;
        // Never connected: picking it re-asks Railway.
        assert!(stalled_trial(&trial));
        trial.config.expires_at = Some(Utc::now() + Duration::minutes(42));
        trial.config.last_probe = Some(SeatProbe {
            online: true,
            ..Default::default()
        });
        assert!(!stalled_trial(&trial));
        assert!(seat_chip_label(Some(&trial), false, false).starts_with("Railway trial — 4"));
        assert_eq!(seat_status_note(&trial), None);
        trial.config.expires_at = Some(Utc::now() - Duration::minutes(1));
        assert!(stalled_trial(&trial));
        assert_eq!(
            seat_chip_label(Some(&trial), false, false),
            "Railway trial — ended"
        );
        assert_eq!(seat_status_note(&trial), Some("trial ended"));

        let mut boat = seat(CloudSeatKind::Box);
        boat.config.label = "box".into();
        assert!(!stalled_trial(&boat));
        assert_eq!(seat_status_note(&boat), Some("needs token"));
        boat.has_box_token = true;
        assert_eq!(seat_status_note(&boat), None);
        assert_eq!(seat_chip_label(Some(&boat), false, false), "Cloud — box");
        assert_eq!(seat_chip_label(None, false, false), "Choose a seat…");
    }

    #[test]
    fn preview_clicks_map_through_the_letterbox() {
        // 1280×800 in a 640×800 pane: scale 0.5, drawn 640×400, 200px bars.
        let bounds = (100.0, 50.0, 640.0, 800.0);
        assert_eq!(
            seat_point_in(bounds, (1280, 800), (100.0, 250.0)),
            Some((0, 0))
        );
        assert_eq!(
            seat_point_in(bounds, (1280, 800), (420.0, 450.0)),
            Some((640, 400))
        );
        assert_eq!(seat_point_in(bounds, (1280, 800), (420.0, 100.0)), None);
        assert_eq!(seat_point_in(bounds, (0, 0), (1.0, 1.0)), None);
    }

    #[test]
    fn saved_boat_token_wins_over_the_trial() {
        let mut railway = seat(CloudSeatKind::Railway);
        railway.config.id = "trial".into();
        railway.config.expires_at = Some(Utc::now() + Duration::minutes(30));
        let mut boat = seat(CloudSeatKind::Box);
        boat.config.id = "boat".into();
        let seats = vec![railway.clone(), boat.clone()];
        // No token yet: the live trial is the pick.
        assert_eq!(
            CloudView::preferred_seat(&seats).map(|s| s.config.id.as_str()),
            Some("trial")
        );
        boat.has_box_token = true;
        let seats = vec![railway.clone(), boat];
        assert_eq!(
            CloudView::preferred_seat(&seats).map(|s| s.config.id.as_str()),
            Some("boat")
        );
        railway.config.expires_at = Some(Utc::now() - Duration::minutes(1));
        assert!(CloudView::preferred_seat(&[railway]).is_none());
    }

    #[test]
    fn available_panes_follow_the_seat_capabilities_in_pane_order() {
        assert_eq!(
            available_panes_for(&SeatCapabilities::default()),
            vec![
                CloudPane::Computer,
                CloudPane::Files,
                CloudPane::Changes,
                CloudPane::Shell,
                CloudPane::Progress,
                CloudPane::Context,
                CloudPane::Tasks,
            ]
        );
        let caps = SeatCapabilities {
            simulator: true,
            pull_requests: true,
            ..Default::default()
        };
        assert_eq!(available_panes_for(&caps), CloudPane::ALL);
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

fn available_panes_for(caps: &SeatCapabilities) -> Vec<CloudPane> {
    CloudPane::ALL
        .into_iter()
        .filter(|pane| match pane {
            CloudPane::Simulator => caps.simulator,
            CloudPane::PullRequests => caps.pull_requests,
            _ => true,
        })
        .collect()
}

pub(crate) fn seat_pane_detail(pane: CloudPane) -> &'static str {
    match pane {
        CloudPane::Computer => "Watch and control the seat's desktop",
        CloudPane::Simulator => "Drive the seat's device emulator",
        CloudPane::Files => "Browse and preview files on the seat",
        CloudPane::Changes => "See the agent's file edits",
        CloudPane::Shell => "Run commands on the seat",
        CloudPane::Progress => "Follow the run's history and actions",
        CloudPane::Context => "Files shared in this session",
        CloudPane::Tasks => "The agent's task list",
        CloudPane::PullRequests => "View proposed code changes",
    }
}

/// One entry in a right-pane launcher list: icon, title, one-line detail.
/// Shared by the local surface picker and a cloud seat's pane picker so
/// both modes present the same panel.
pub(crate) fn surface_pick_row(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    icon_path: &'static str,
    title: &'static str,
    detail: &'static str,
) -> gpui::Stateful<gpui::Div> {
    let hover = theme.element_hover;
    div()
        .id(id)
        .w_full()
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .flex()
        .flex_row()
        .items_start()
        .gap(px(12.0))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(
            div().pt(px(2.0)).child(
                icon(icon_path)
                    .size(px(16.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(13.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child(detail),
                ),
        )
}

/// A Railway trial seat that isn't live — it never connected, or its hour
/// is up. Picking one re-asks Railway (the honest retry).
pub(crate) fn stalled_trial(seat: &SeatRow) -> bool {
    seat.config.kind == Some(CloudSeatKind::Railway)
        && (seat.config.expired()
            || seat
                .config
                .last_probe
                .as_ref()
                .is_none_or(|probe| !probe.online))
}

/// The seat's one-word problem for menu rows, if any.
pub(crate) fn seat_status_note(seat: &SeatRow) -> Option<&'static str> {
    match seat.config.kind {
        Some(CloudSeatKind::Railway) if seat.config.signup_url.is_some() => Some("sign in"),
        Some(CloudSeatKind::Railway) if seat.config.expired() => Some("trial ended"),
        Some(CloudSeatKind::Ssh)
            if seat
                .config
                .last_probe
                .as_ref()
                .is_some_and(|probe| !probe.online) =>
        {
            Some("offline")
        }
        Some(CloudSeatKind::GitHub) if seat.config.repo.is_none() || !seat.has_github_pat => {
            Some("needs token")
        }
        Some(CloudSeatKind::Box) if !seat.has_box_token => Some("needs token"),
        _ if !CloudView::seat_ready(seat) => Some("needs credentials"),
        _ => None,
    }
}

/// How a picked seat reads on a chip: "Railway trial — 42m", "Cloud — box".
pub(crate) fn seat_chip_label(
    seat: Option<&SeatRow>,
    trial_failed: bool,
    trial_starting: bool,
) -> String {
    match seat {
        Some(seat) if seat.config.kind == Some(CloudSeatKind::GitHub) => {
            format!("GitHub — {}", seat.config.label)
        }
        Some(seat) if seat.config.kind == Some(CloudSeatKind::Railway) => {
            if trial_starting {
                "Railway trial — starting…".into()
            } else if seat.config.signup_url.is_some() {
                "Railway trial — sign in to continue".into()
            } else if seat.config.expired() {
                "Railway trial — ended".into()
            } else if let Some(at) = seat.config.expires_at {
                format!(
                    "Railway trial — {}m",
                    at.signed_duration_since(chrono::Utc::now())
                        .num_minutes()
                        .max(0)
                )
            } else if trial_failed
                || seat
                    .config
                    .last_probe
                    .as_ref()
                    .is_some_and(|probe| !probe.online)
            {
                "Railway trial — unavailable".into()
            } else {
                "Railway trial — starting…".into()
            }
        }
        Some(seat) => format!("Cloud — {}", seat.config.label),
        None => "Choose a seat…".into(),
    }
}

/// The monochrome status dot used by the bot bar and seat rows.
pub(crate) fn status_dot(
    theme: &Theme,
    probe: Option<&SeatProbe>,
    ended: bool,
) -> gpui::Stateful<gpui::Div> {
    let (color, label) = if ended {
        (theme.text_faint, "ended")
    } else {
        match probe {
            Some(p) if p.sleeping => (theme.text_faint, "sleeping"),
            Some(p) if p.online => (theme.text_muted, "online"),
            Some(_) => (theme.text_faint, "offline"),
            None => (theme.text_faint, "not probed"),
        }
    };
    div()
        .id(SharedString::from(format!("seat-dot-{label}")))
        .size(px(7.0))
        .flex_none()
        .rounded_full()
        .bg(color)
}

fn countdown_tick(secs_left: i64) -> Option<std::time::Duration> {
    (secs_left > 0).then(|| std::time::Duration::from_secs((secs_left % 60) as u64 + 1))
}

/// A headless Linux seat's live console: a monitor frame with the box's
/// identity, resource gauges and top processes.
fn render_vm_screen(theme: &Theme, info: &serde_json::Value) -> AnyElement {
    let field = |k: &str| {
        info.get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("—")
            .to_string()
    };
    let num = |k: &str| {
        info.get(k)
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
    };
    let gib = |kb: f64| kb / 1024.0 / 1024.0;
    let mem = match (num("mem_total_kb"), num("mem_avail_kb")) {
        (Some(t), Some(a)) if t > 0.0 => {
            Some(((t - a) / t, format!("{:.1} / {:.1} GB", gib(t - a), gib(t))))
        }
        _ => None,
    };
    let disk = match (num("disk_total_kb"), num("disk_used_kb")) {
        (Some(t), Some(u)) if t > 0.0 => Some((u / t, format!("{:.1} / {:.1} GB", gib(u), gib(t)))),
        _ => None,
    };
    let cpus = num("cpus").unwrap_or(1.0).max(1.0);
    let load = num("load").map(|l| ((l / cpus).min(1.0), format!("{l:.2} load · {cpus:.0} vCPU")));
    let uptime = num("uptime").map(|s| {
        let s = s as u64;
        if s >= 86_400 {
            format!("up {}d {}h", s / 86_400, (s % 86_400) / 3600)
        } else if s >= 3600 {
            format!("up {}h {}m", s / 3600, (s % 3600) / 60)
        } else {
            format!("up {}m", s / 60)
        }
    });
    let mono = theme.font_mono.clone();
    let screen_bg = gpui::hsla(0.0, 0.0, 0.07, 1.0);
    let screen_text = gpui::hsla(0.0, 0.0, 0.86, 1.0);
    let screen_muted = gpui::hsla(0.0, 0.0, 0.55, 1.0);
    let gauge = |label: &'static str, value: Option<(f64, String)>| {
        let (frac, text) = value.unwrap_or((0.0, "—".into()));
        let fill = if frac > 0.85 {
            theme.danger
        } else if frac > 0.6 {
            theme.warning
        } else {
            theme.success
        };
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .text_size(crate::typography::ui_rems(11.0))
                    .child(div().text_color(screen_muted).child(label))
                    .child(
                        div()
                            .text_color(screen_text)
                            .child(SharedString::from(text)),
                    ),
            )
            .child(
                div()
                    .h(px(4.0))
                    .w_full()
                    .rounded(px(2.0))
                    .bg(gpui::hsla(0.0, 0.0, 1.0, 0.08))
                    .child(
                        div()
                            .h_full()
                            .w(gpui::relative(frac.clamp(0.0, 1.0) as f32))
                            .rounded(px(2.0))
                            .bg(fill),
                    ),
            )
    };
    let prompt = format!("{}@{}", field("user"), field("host"));
    let mut procs = div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .font_family(mono.clone());
    procs = procs.child(
        div()
            .flex()
            .flex_row()
            .gap(px(8.0))
            .text_size(crate::typography::ui_rems(10.5))
            .text_color(screen_muted)
            .child(div().w(px(48.0)).child("PID"))
            .child(div().w(px(44.0)).child("CPU%"))
            .child(div().w(px(44.0)).child("MEM%"))
            .child(div().flex_1().child("COMMAND")),
    );
    for p in info
        .get("procs")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let col = |k: &str| {
            SharedString::from(p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string())
        };
        procs = procs.child(
            div()
                .flex()
                .flex_row()
                .gap(px(8.0))
                .text_size(crate::typography::ui_rems(10.5))
                .text_color(screen_text)
                .child(div().w(px(48.0)).child(col("pid")))
                .child(div().w(px(44.0)).child(col("cpu")))
                .child(div().w(px(44.0)).child(col("mem")))
                .child(div().flex_1().min_w_0().truncate().child(col("name"))),
        );
    }
    div()
        .id("vm-screen")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .p(px(12.0))
        .child(
            div()
                .w_full()
                .rounded(px(10.0))
                .border_1()
                .border_color(theme.border)
                .bg(screen_bg)
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(28.0))
                        .px(px(10.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .border_b_1()
                        .border_color(gpui::hsla(0.0, 0.0, 1.0, 0.08))
                        .child(icon(icons::MONITOR).size(px(12.0)).text_color(screen_muted))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(screen_text)
                                .child(SharedString::from(field("os"))),
                        )
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(screen_muted)
                                .child(SharedString::from(uptime.unwrap_or_default())),
                        ),
                )
                .child(
                    div()
                        .p(px(12.0))
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            div()
                                .font_family(mono.clone())
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(screen_text)
                                .child(SharedString::from(format!(
                                    "{prompt}  ·  {} {}",
                                    field("kernel"),
                                    field("arch")
                                ))),
                        )
                        .child(gauge("CPU", load))
                        .child(gauge("Memory", mem))
                        .child(gauge("Disk", disk))
                        .child(procs)
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(screen_muted)
                                .child("Headless Linux — no display is running on this seat. Live every 4s."),
                        ),
                ),
        )
        .into_any_element()
}

/// Map a point in window space onto seat pixels for an image of `size`
/// drawn `object-fit: contain` inside `bounds` (x, y, w, h). `None` when the
/// point falls in the letterbox.
fn seat_point_in(
    bounds: (f32, f32, f32, f32),
    size: (u32, u32),
    point: (f32, f32),
) -> Option<(i32, i32)> {
    let (bx, by, bw, bh) = bounds;
    let (iw, ih) = (size.0 as f32, size.1 as f32);
    if bw <= 0.0 || bh <= 0.0 || iw <= 0.0 || ih <= 0.0 {
        return None;
    }
    let scale = (bw / iw).min(bh / ih);
    let (dw, dh) = (iw * scale, ih * scale);
    let (ox, oy) = (bx + (bw - dw) / 2.0, by + (bh - dh) / 2.0);
    let (x, y) = ((point.0 - ox) / scale, (point.1 - oy) / scale);
    if x < 0.0 || y < 0.0 || x >= iw || y >= ih {
        return None;
    }
    Some((x.floor() as i32, y.floor() as i32))
}

/// `size<TAB>name` lines from the evidence listing.
fn parse_evidence(stdout: &str) -> Vec<(String, u64)> {
    stdout
        .lines()
        .filter_map(|line| {
            let (size, name) = line.split_once('\t')?;
            let name = name.trim();
            (!name.is_empty() && !name.contains('/'))
                .then(|| (name.to_string(), size.trim().parse().unwrap_or(0)))
        })
        .collect()
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
#[allow(clippy::enum_variant_names)]
pub enum RailEvent {
    OpenPane(CloudPane),
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
    /// Headless Linux seats: the live system snapshot shown in place of a
    /// desktop capture.
    vm_info: Option<serde_json::Value>,
    screen_task: Option<Task<()>>,
    /// Seat pixels of the last capture — "Take control" maps clicks on the
    /// scaled preview back onto them.
    shot_size: Option<(u32, u32)>,
    /// Where the preview painted last frame (window coords).
    shot_bounds: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
    /// Clicks/keys on the preview drive the seat's desktop.
    controlling: bool,
    type_input: Entity<ComposerInput>,
    act_task: Option<Task<()>>,
    act_error: Option<String>,
    /// "Start desktop" in flight (first run installs Xvfb on the seat).
    desktop_starting: bool,
    desktop_error: Option<String>,
    // Context
    /// Screenshots / recordings the agent saved under `~/.cosmos/shots`.
    evidence: Loadable<Vec<(String, u64)>>,
    evidence_preview: Option<(String, Loadable<(Arc<gpui::Image>, f32)>)>,
    backup_busy: bool,
    backup_note: Option<Result<String, String>>,
    /// The chat's latest ledger run — drives the bot bar's status + buttons.
    run: Option<CloudRun>,
    runs: Vec<CloudRun>,
    /// BYOK label for the model chip (the engine view's cloud default).
    model_label: Option<SharedString>,
    /// The pane "+" menu and the destroy confirmation live on the rail chrome.
    pane_menu: popover::Popup<()>,
    /// The pane launcher (the empty right-pane list) is showing instead of
    /// a pane body.
    picking: bool,
    active: bool,
    confirm_destroy: bool,
    confirm_task: Option<Task<()>>,
    control_busy: bool,
    control_error: Option<String>,
    control_task: Option<Task<()>>,
    toolbar_status_tooltip: SharedString,
    /// The chat's session stamp at the last ledger refresh — the state
    /// observer reloads the run row only when this actually moves.
    session_stamp: Option<(SessionStatus, chrono::DateTime<chrono::Utc>)>,
    /// One-shot bookkeeping for the "Keep this computer" card: `None` = the
    /// seat was never asked, `Some(false)` = card visible, `Some(true)` =
    /// marked prompted this session (persisted engine-side).
    keep_prompted: Option<bool>,
    task: Option<Task<()>>,
    _input_sub: Subscription,
    _type_sub: Subscription,
    _state_sub: Subscription,
    /// Railway countdown ticker — re-renders at the next minute boundary.
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

fn cloud_run_prompt(run: &CloudRun, entries: &[zeron_doc::SessionMessageEntry]) -> String {
    let text = entries
        .iter()
        .filter(|entry| {
            entry.role == zeron_doc::MessageRole::User
                && entry.created_at <= run.started_at.timestamp_millis()
        })
        .max_by_key(|entry| entry.created_at)
        .map(|entry| {
            entry
                .parts
                .iter()
                .filter_map(|part| match part {
                    zeron_doc::MessagePart::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .unwrap_or_default();
    let first_line = text.lines().next().unwrap_or_default().trim();
    if first_line.is_empty() {
        return "Cloud run".into();
    }
    let mut chars = first_line.chars();
    let truncated = chars.by_ref().take(80).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    }
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
        let type_input = cx.new(|cx| ComposerInput::new("Type on the seat — ⏎ sends", cx));
        let type_sub = cx.subscribe(&type_input, |this, _, event, cx| {
            if let ComposerInputEvent::Submitted = event {
                this.type_on_seat(cx);
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
                if !this.hosted_by_remote_runner(cx) {
                    this.load_seat(cx);
                }
            }
        });
        let toolbar_status_tooltip = format!("{} · Not checked yet", binding.label).into();
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
            vm_info: None,
            screen_task: None,
            shot_size: None,
            shot_bounds: Default::default(),
            controlling: false,
            type_input,
            act_task: None,
            act_error: None,
            desktop_starting: false,
            desktop_error: None,
            evidence: Loadable::Idle,
            evidence_preview: None,
            backup_busy: false,
            backup_note: None,
            run: None,
            runs: Vec::new(),
            model_label: None,
            pane_menu: popover::Popup::default(),
            picking: true,
            active: true,
            confirm_destroy: false,
            confirm_task: None,
            control_busy: false,
            control_error: None,
            control_task: None,
            toolbar_status_tooltip,
            session_stamp: None,
            keep_prompted: None,
            task: None,
            _input_sub: input_sub,
            _type_sub: type_sub,
            _state_sub: state_sub,
            _countdown: None,
            _refresh: None,
        };
        rail.start_timers(cx);
        rail.load_seat(cx);
        rail.ensure_loaded(cx);
        rail
    }

    fn start_timers(&mut self, cx: &mut Context<Self>) {
        self._countdown = None;
        self._refresh = None;
        if !self.active {
            return;
        }
        if self.binding.expires_at.is_some() {
            self._countdown = Some(cx.spawn(async move |this, cx| {
                loop {
                    let delay = this
                        .update(cx, |rail, cx| {
                            if !rail.active {
                                return None;
                            }
                            let left = rail
                                .binding
                                .expires_at
                                .map(|at| at.signed_duration_since(chrono::Utc::now()))
                                .map(|d| d.num_seconds())
                                .unwrap_or(-1);
                            rail.refresh_toolbar_status_tooltip();
                            cx.notify();
                            countdown_tick(left)
                        })
                        .unwrap_or(None);
                    match delay {
                        None => break,
                        Some(delay) => {
                            cx.background_executor().timer(delay).await;
                        }
                    }
                }
            }));
        }
        let state_for_refresh = self.state.clone();
        self._refresh = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(30))
                    .await;
                let alive = this
                    .update(cx, |rail, cx| {
                        if !rail.active {
                            return false;
                        }
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
    }

    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            self.start_timers(cx);
            if self.pane == CloudPane::Computer && !self.picking {
                self.capture_shot(cx);
            }
            self.state.update(cx, |state, cx| {
                state.refresh_cloud(Some(std::time::Duration::from_secs(30)), cx);
            });
        } else {
            self._countdown = None;
            self._refresh = None;
            self.screen_task = None;
            self.task = None;
            self.act_task = None;
            self.confirm_task = None;
            self.confirm_destroy = false;
            self.act_error = None;
            self.exec_busy = false;
            self.desktop_starting = false;
            self.backup_busy = false;
            self.controlling = false;
            self.control_busy = self.control_task.is_some();
        }
        cx.notify();
    }

    fn hosted_by_remote_runner(&self, cx: &App) -> bool {
        self.binding
            .runner_device_id
            .as_ref()
            .is_some_and(|runner_id| {
                self.state.read(cx).local_device_id.as_deref() != Some(runner_id.as_str())
            })
    }

    fn refresh_toolbar_status_tooltip(&mut self) {
        let seat = self.seat.ready();
        let seat_label = seat
            .map(|seat| seat.config.label.as_str())
            .unwrap_or(self.binding.label.as_str());
        let probe = seat.and_then(|seat| seat.config.last_probe.as_ref());
        let run_status = self.run.as_ref().map(|run| run.status);
        let sleeping = probe.is_some_and(|probe| probe.sleeping);
        let railway = seat.is_some_and(|seat| seat.config.kind == Some(CloudSeatKind::Railway))
            || self.binding.kind == CloudSeatKind::Railway;
        let trial_left = self
            .binding
            .expires_at
            .map(|at| at.signed_duration_since(chrono::Utc::now()).num_seconds());
        let trial_ended = railway
            && (seat.is_some_and(|seat| seat.config.expired())
                || self
                    .binding
                    .expires_at
                    .is_some_and(|at| at <= chrono::Utc::now()));
        let meta = if trial_ended {
            "Trial ended".to_string()
        } else {
            match (trial_left, run_status) {
                (Some(secs), _) => {
                    if secs < 60 {
                        "Trial · <1m left".to_string()
                    } else {
                        format!("Trial · {}m left", secs / 60)
                    }
                }
                (None, _) if sleeping => "Sleeping · billing off".to_string(),
                (None, Some(status)) => status.label().to_string(),
                (None, None) => match probe {
                    Some(probe) if probe.online => "Online".to_string(),
                    Some(_) => "Offline".to_string(),
                    None => "Not checked yet".to_string(),
                },
            }
        };
        let meta = match (&meta[..], run_status) {
            (meta, Some(status)) if meta.starts_with("Trial ·") => {
                format!("{meta} · {}", status.label())
            }
            _ => meta,
        };
        self.toolbar_status_tooltip = format!("{seat_label} · {meta}").into();
    }

    fn capabilities(&self) -> SeatCapabilities {
        self.seat
            .ready()
            .map(|s| s.capabilities.clone())
            .unwrap_or_default()
    }

    pub(crate) fn available_panes(&self) -> Vec<CloudPane> {
        available_panes_for(&self.capabilities())
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
            CloudPane::Context if matches!(self.evidence, Loadable::Idle) => self.load_evidence(cx),
            CloudPane::Simulator => {}
            _ => {}
        }
    }

    fn load_seat(&mut self, cx: &mut Context<Self>) {
        let seat_id = self.binding.seat_id.clone();
        let runner_hosted_remotely = self.hosted_by_remote_runner(cx);
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
                if runner_hosted_remotely {
                    rail.runs.clear();
                    rail.run = None;
                }
                let mut run_finished = false;
                rail.seat = match result {
                    Ok(value) => match serde_json::from_value::<CloudView>(value) {
                        Ok(view) => {
                            let run = if runner_hosted_remotely {
                                rail.runs.clear();
                                None
                            } else {
                                rail.runs = view
                                    .runs
                                    .iter()
                                    .filter(|run| run.chat_id == rail.chat_id)
                                    .cloned()
                                    .collect();
                                view.run_for_chat(&rail.chat_id).cloned()
                            };
                            run_finished = run
                                .as_ref()
                                .is_some_and(|next| cloud_run_finished(rail.run.as_ref(), next));
                            rail.run = if runner_hosted_remotely {
                                None
                            } else {
                                run.or_else(|| rail.run.take())
                            };
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
                rail.refresh_toolbar_status_tooltip();
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
        if self.pane == pane && !self.picking {
            return;
        }
        let confirmation_changed = self.confirm_destroy || self.confirm_task.is_some();
        self.confirm_destroy = false;
        self.confirm_task = None;
        if confirmation_changed {
            cx.notify();
        }
        if self.picking {
            self.picking = false;
            cx.notify();
        }
        self.pane = pane;
        self.close_pane_menu(cx);
        self.ensure_loaded(cx);
        cx.notify();
    }

    pub fn show_picker(&mut self, cx: &mut Context<Self>) {
        if self.picking {
            return;
        }
        self.close_pane_menu(cx);
        self.picking = true;
        cx.notify();
    }

    /// The pane launcher — the same centered list the local right pane
    /// shows when empty, listing what this seat can actually back.
    fn render_picker(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let row = |id: SharedString, icon_path, title, detail| {
            surface_pick_row(theme, id, icon_path, title, detail)
        };
        let mut list = div()
            .w(px(320.0))
            .max_w_full()
            .flex()
            .flex_col()
            .gap(px(2.0));
        for pane in self.available_panes() {
            list = list.child(
                row(
                    SharedString::from(format!("rail-pick-{}", pane.label())),
                    seat_pane_icon(pane),
                    pane.label(),
                    seat_pane_detail(pane),
                )
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(RailEvent::OpenPane(pane)))),
            );
        }
        list = list.child(
            row(
                "rail-pick-sidechat".into(),
                icons::CHAT_ROUND_LINE,
                "Side chat",
                "Ask questions without interrupting the agent",
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(RailEvent::OpenSideChat))),
        );
        div()
            .id("rail-picker")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(16.0))
            .pt(px(16.0))
            .pb(px(64.0))
            .child(list)
            .into_any_element()
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
    /// The chat's checkout on the seat — every pane reads and runs there.
    fn scope(&self) -> (Option<String>, Option<String>) {
        (
            self.binding.workspace_path.clone(),
            self.binding.repo.clone(),
        )
    }

    fn seat_control(&mut self, op: &'static str, cx: &mut Context<Self>) {
        self.confirm_task = None;
        self.confirm_destroy = false;
        let Some(engine) = self.engine(cx) else {
            self.control_error = Some("Engine not connected".into());
            return;
        };
        self.control_busy = true;
        self.control_error = None;
        let chat_id = self.chat_id.clone();
        let seat_id = self.binding.seat_id.clone();
        let scope = self.scope();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_SEAT_CONTROL,
                    serde_json::json!({
                        "chatId": chat_id,
                        "op": op,
                        "seatId": seat_id, "workspace": &scope.0, "repo": &scope.1,
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

    /// Re-check the seat now (the header's refresh button): the engine probes
    /// it and stamps `last_probe`; the rail then re-reads the row.
    fn probe_seat(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            self.control_error = Some("Engine not connected".into());
            cx.notify();
            return;
        };
        self.control_busy = true;
        self.control_error = None;
        let seat_id = self.binding.seat_id.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_PROBE_SEAT,
                    serde_json::json!({ "seatId": seat_id }),
                )
                .await;
            this.update(cx, |rail, cx| {
                rail.control_busy = false;
                match result {
                    Err(err) => rail.control_error = Some(err.to_string()),
                    Ok(value) => {
                        if value.get("online").and_then(|v| v.as_bool()) == Some(false) {
                            rail.control_error = value
                                .get("detail")
                                .and_then(|v| v.as_str())
                                .map(str::to_string)
                                .or_else(|| Some("The seat didn't answer".into()));
                        }
                    }
                }
                rail.load_seat(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Move this chat to another seat (its trial ended): the engine rebinds
    /// the chat row; the chat's workspace backup restores there on the next
    /// run. The rail follows the new binding at once.
    fn move_chat(&mut self, seat_id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.control_busy = true;
        self.control_error = None;
        let chat_id = self.chat_id.clone();
        self.control_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_MOVE_CHAT,
                    serde_json::json!({ "chatId": chat_id, "seatId": seat_id }),
                )
                .await;
            this.update(cx, |rail, cx| {
                rail.control_task = None;
                rail.control_busy = false;
                match result.map(|v| v.get("binding").cloned()) {
                    Ok(Some(value)) => match serde_json::from_value::<CloudSeatBinding>(value) {
                        Ok(binding) => rail.rebind(binding, cx),
                        Err(err) => rail.control_error = Some(err.to_string()),
                    },
                    Ok(None) => rail.control_error = Some("the engine didn't return a seat".into()),
                    Err(err) => rail.control_error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn return_chat(&mut self, push_wip: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            self.control_error = Some("Engine not connected".into());
            cx.notify();
            return;
        };
        self.control_busy = true;
        self.control_error = None;
        let chat_id = self.chat_id.clone();
        self.control_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_RETURN_CHAT,
                    serde_json::json!({ "chatId": chat_id, "pushWip": push_wip }),
                )
                .await;
            this.update(cx, |rail, cx| {
                rail.control_task = None;
                rail.control_busy = false;
                if let Err(err) = result {
                    rail.control_error = Some(err.to_string());
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn rebind(&mut self, binding: CloudSeatBinding, cx: &mut Context<Self>) {
        self.cwd = binding.workspace_path.clone().unwrap_or_else(|| ".".into());
        self.binding = binding;
        self.parents.clear();
        self.seat = Loadable::Loading;
        self.listing = Loadable::Idle;
        self.file_path = None;
        self.file_text = Loadable::Idle;
        self.status = Loadable::Idle;
        self.diff = Loadable::Idle;
        self.prs = Loadable::Idle;
        self.shot = Loadable::Idle;
        self.vm_info = None;
        self.shot_size = None;
        self.controlling = false;
        self.evidence = Loadable::Idle;
        self.run = None;
        self.refresh_toolbar_status_tooltip();
        self.keep_prompted = None;
        self.state
            .update(cx, |state, cx| state.refresh_cloud(None, cx));
        self.load_seat(cx);
        self.ensure_loaded(cx);
    }

    /// Both keep-card answers land here — the engine stamps
    /// `keep_prompted_at` so the card never asks twice, on any device.
    fn mark_keep_prompted(&mut self, cx: &mut Context<Self>) {
        self.keep_prompted = Some(true);
        if let Some(engine) = self.engine(cx) {
            let seat_id = self.binding.seat_id.clone();
            let scope = self.scope();
            self.task = Some(cx.spawn(async move |this, cx| {
                let _ = engine
                    .client()
                    .call(
                        methods::CLOUD_SEAT_CONTROL,
                        serde_json::json!({
                            "chatId": "",
                            "seatId": seat_id, "workspace": &scope.0, "repo": &scope.1,
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
        let scope = self.scope();
        let dir = self.cwd.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_LIST_DIR,
                    serde_json::json!({ "seatId": seat_id, "workspace": &scope.0, "repo": &scope.1, "path": dir }),
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
            let scope = self.scope();
            let path = entry.path;
            self.task = Some(cx.spawn(async move |this, cx| {
                let result = engine
                    .client()
                    .call(
                        methods::CLOUD_READ_FILE,
                        serde_json::json!({ "seatId": seat_id, "workspace": &scope.0, "repo": &scope.1, "path": path }),
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
        let scope = self.scope();
        self.task = Some(cx.spawn(async move |this, cx| {
            let status = engine
                .client()
                .call(
                    methods::CLOUD_GIT_STATUS,
                    serde_json::json!({ "seatId": seat_id, "workspace": &scope.0, "repo": &scope.1 }),
                )
                .await;
            let diff = engine
                .client()
                .call(
                    methods::CLOUD_GIT_DIFF,
                    serde_json::json!({ "seatId": seat_id, "workspace": &scope.0, "repo": &scope.1 }),
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
        let scope = self.scope();
        let cwd = self.cwd.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_EXEC,
                    serde_json::json!({
                        "seatId": seat_id, "workspace": &scope.0, "repo": &scope.1,
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
                        let output = if stderr.is_empty() {
                            stdout.to_string()
                        } else if stdout.is_empty() {
                            stderr.to_string()
                        } else {
                            format!("{stdout}\n{stderr}")
                        };
                        line.output = zeron_engine::railway::redact_trial_banner(&output);
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
        let scope = self.scope();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_GITHUB_PRS,
                    serde_json::json!({ "seatId": seat_id, "workspace": &scope.0, "repo": &scope.1 }),
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
        let scope = self.scope();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_GITHUB_PR,
                    serde_json::json!({ "seatId": seat_id, "workspace": &scope.0, "repo": &scope.1, "number": number }),
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
        let scope = self.scope();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_GITHUB_MERGE,
                    serde_json::json!({ "seatId": seat_id, "workspace": &scope.0, "repo": &scope.1, "number": number }),
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
        if !matches!(
            self.state.read(cx).workspace_scope,
            Some(zeron_proto::WorkspaceScope::Synced | zeron_proto::WorkspaceScope::Development)
        ) {
            self.analysis = Loadable::Error("Cloud needs you signed in".into());
            cx.notify();
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
        if !self.active {
            return;
        }
        let caps = self.capabilities();
        if !caps.computer && !caps.shell {
            self.shot = Loadable::Ready(None);
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        if !matches!(self.shot, Loadable::Ready(_)) {
            self.shot = Loadable::Loading;
        }
        let seat_id = self.binding.seat_id.clone();
        let scope = self.scope();
        self.screen_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let result = engine
                    .client()
                    .call(
                        methods::CLOUD_SCREENSHOT,
                        serde_json::json!({ "seatId": seat_id, "workspace": &scope.0, "repo": &scope.1 }),
                    )
                    .await;
                let watching = this
                    .update(cx, |rail, cx| {
                        match result {
                            Ok(value) => {
                                let image = value
                                    .get("pngBase64")
                                    .and_then(|p| p.as_str())
                                    .and_then(|b64| {
                                        use base64::Engine as _;
                                        base64::engine::general_purpose::STANDARD.decode(b64).ok()
                                    })
                                    .map(|bytes| {
                                        Arc::new(gpui::Image::from_bytes(
                                            gpui::ImageFormat::Png,
                                            bytes,
                                        ))
                                    });
                                rail.shot_size = match (
                                    value.get("width").and_then(|w| w.as_u64()),
                                    value.get("height").and_then(|h| h.as_u64()),
                                ) {
                                    (Some(w), Some(h)) => Some((w as u32, h as u32)),
                                    _ => None,
                                };
                                if image.is_none() {
                                    rail.controlling = false;
                                }
                                rail.vm_info = value.get("system").cloned();
                                rail.shot = Loadable::Ready(image);
                            }
                            Err(err) => {
                                rail.vm_info = None;
                                rail.shot = Loadable::Error(err.to_string());
                            }
                        }
                        cx.notify();
                        rail.pane == CloudPane::Computer
                            && !rail.picking
                            && rail.active
                            && matches!(rail.shot, Loadable::Ready(_))
                    })
                    .unwrap_or(false);
                if !watching {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(4))
                    .await;
                let still = this
                    .update(cx, |rail, _| {
                        rail.active && rail.pane == CloudPane::Computer && !rail.picking
                    })
                    .unwrap_or(false);
                if !still {
                    break;
                }
            }
        }));
    }

    /// A bounded command on the seat through `CloudExec` (the chat's scope).
    fn seat_exec_call(
        &self,
        command: String,
        cx: &App,
    ) -> Option<impl std::future::Future<Output = anyhow::Result<serde_json::Value>> + use<>> {
        let engine = self.engine(cx)?;
        let seat_id = self.binding.seat_id.clone();
        let scope = self.scope();
        Some(async move {
            engine
                .client()
                .call(
                    methods::CLOUD_EXEC,
                    serde_json::json!({
                        "seatId": seat_id,
                        "workspace": scope.0,
                        "repo": scope.1,
                        "command": command,
                        "timeoutSecs": 30,
                    }),
                )
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))
        })
    }

    /// The agent's evidence folder: newest first, with sizes.
    fn load_evidence(&mut self, cx: &mut Context<Self>) {
        let Some(call) = self.seat_exec_call(
            "cd \"$HOME/.cosmos/shots\" 2>/dev/null || exit 0; ls -1t | head -40 | \
             while IFS= read -r f; do [ -f \"$f\" ] && printf '%s\\t%s\\n' \"$(wc -c < \"$f\")\" \"$f\"; done"
                .to_string(),
            cx,
        ) else {
            return;
        };
        if !matches!(self.evidence, Loadable::Ready(_)) {
            self.evidence = Loadable::Loading;
        }
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |rail, cx| {
                rail.evidence = match result {
                    Ok(value) => {
                        let stdout = value
                            .get("stdout")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default();
                        Loadable::Ready(parse_evidence(stdout))
                    }
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    fn preview_evidence(&mut self, name: String, cx: &mut Context<Self>) {
        if self
            .evidence_preview
            .as_ref()
            .is_some_and(|(open, _)| *open == name)
        {
            self.evidence_preview = None;
            cx.notify();
            return;
        }
        let Some(call) = self.seat_exec_call(
            format!(
                "base64 < \"$HOME/.cosmos/shots\"/{} | tr -d '\\n'",
                zeron_engine::cloud::shell_word(&name)
            ),
            cx,
        ) else {
            return;
        };
        self.evidence_preview = Some((name.clone(), Loadable::Loading));
        self.act_task = Some(cx.spawn(async move |this, cx| {
            let result = call.await;
            this.update(cx, |rail, cx| {
                let image = result.map_err(|e| e.to_string()).and_then(|value| {
                    use base64::Engine as _;
                    let b64 = value
                        .get("stdout")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .map_err(|e| format!("couldn't read the image: {e}"))
                        .map(|bytes| {
                            // Width / height, so the preview box hugs the shot.
                            let ratio = zeron_engine::seat_desktop::png_size(&bytes)
                                .map(|(w, h)| w as f32 / h as f32)
                                .unwrap_or(1.6);
                            (
                                Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes)),
                                ratio,
                            )
                        })
                });
                if let Some((open, slot)) = rail.evidence_preview.as_mut()
                    && *open == name
                {
                    *slot = match image {
                        Ok(image) => Loadable::Ready(image),
                        Err(err) => Loadable::Error(err),
                    };
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn backup_now(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        if self.backup_busy {
            return;
        }
        self.backup_busy = true;
        self.backup_note = None;
        let chat_id = self.chat_id.clone();
        self.act_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_BACKUP_CHAT,
                    serde_json::json!({ "chatId": chat_id }),
                )
                .await;
            this.update(cx, |rail, cx| {
                rail.backup_busy = false;
                rail.backup_note = Some(match result {
                    Ok(value) => Ok(format!(
                        "Backed up {} to this computer",
                        zeron_engine::cloud::human_bytes(
                            value
                                .get("backup")
                                .and_then(|b| b.get("bytes"))
                                .and_then(|b| b.as_u64())
                                .unwrap_or(0)
                        )
                    )),
                    Err(err) => Err(err.to_string()),
                });
                rail.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Bring the seat's virtual desktop up (the first time installs Xvfb +
    /// xdotool on the seat — about a minute), then start watching it.
    fn start_desktop(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        if self.desktop_starting {
            return;
        }
        self.desktop_starting = true;
        self.desktop_error = None;
        let seat_id = self.binding.seat_id.clone();
        let scope = self.scope();
        self.act_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_DESKTOP_START,
                    serde_json::json!({ "seatId": seat_id, "workspace": &scope.0, "repo": &scope.1 }),
                )
                .await;
            this.update(cx, |rail, cx| {
                rail.desktop_starting = false;
                match result {
                    Ok(_) => rail.capture_shot(cx),
                    Err(err) => rail.desktop_error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// One "Take control" action on the seat desktop, then an immediate
    /// re-capture so the preview shows the result.
    fn act(&mut self, action: serde_json::Value, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let seat_id = self.binding.seat_id.clone();
        let scope = self.scope();
        self.act_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_COMPUTER_ACT,
                    serde_json::json!({
                        "seatId": seat_id,
                        "workspace": &scope.0,
                        "repo": &scope.1,
                        "action": action,
                    }),
                )
                .await;
            this.update(cx, |rail, cx| {
                rail.act_error = result.err().map(|err| err.to_string());
                rail.capture_shot(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Window position → seat pixel, through the preview's `object-fit:
    /// contain` letterboxing. `None` outside the drawn image.
    fn seat_point(&self, position: gpui::Point<gpui::Pixels>) -> Option<(i32, i32)> {
        let bounds = self.shot_bounds.get()?;
        let (w, h) = self.shot_size?;
        seat_point_in(
            (
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            ),
            (w, h),
            (f32::from(position.x), f32::from(position.y)),
        )
    }

    fn click_preview(&mut self, event: &gpui::MouseDownEvent, cx: &mut Context<Self>) {
        if !self.controlling {
            return;
        }
        let Some((x, y)) = self.seat_point(event.position) else {
            return;
        };
        let action = match (event.button, event.click_count) {
            (gpui::MouseButton::Right, _) => {
                serde_json::json!({ "kind": "click", "x": x, "y": y, "button": "right" })
            }
            (gpui::MouseButton::Middle, _) => {
                serde_json::json!({ "kind": "click", "x": x, "y": y, "button": "middle" })
            }
            (_, n) if n >= 2 => serde_json::json!({ "kind": "doubleClick", "x": x, "y": y }),
            _ => serde_json::json!({ "kind": "click", "x": x, "y": y }),
        };
        self.act(action, cx);
    }

    fn scroll_preview(&mut self, event: &gpui::ScrollWheelEvent, cx: &mut Context<Self>) {
        if !self.controlling {
            return;
        }
        let Some((x, y)) = self.seat_point(event.position) else {
            return;
        };
        let dy = match event.delta {
            gpui::ScrollDelta::Lines(delta) => delta.y,
            gpui::ScrollDelta::Pixels(delta) => f32::from(delta.y) / 40.0,
        };
        if dy.abs() < 0.5 {
            return;
        }
        let amount = dy.abs().round().clamp(1.0, 10.0) as u32;
        let direction = if dy > 0.0 { "up" } else { "down" };
        self.act(
            serde_json::json!({ "kind": "scroll", "direction": direction, "amount": amount, "x": x, "y": y }),
            cx,
        );
    }

    fn type_on_seat(&mut self, cx: &mut Context<Self>) {
        let text = self.type_input.read(cx).text().to_string();
        if text.is_empty() {
            self.act(serde_json::json!({ "kind": "key", "combo": "Return" }), cx);
            return;
        }
        self.type_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.act(serde_json::json!({ "kind": "type", "text": text }), cx);
    }

    /// "Open": the bar's URL launches in a browser on the seat's desktop.
    fn open_on_seat(&mut self, cx: &mut Context<Self>) {
        let text = self.type_input.read(cx).text().trim().to_string();
        let url = if text.starts_with("http://") || text.starts_with("https://") {
            text
        } else if !text.is_empty() && !text.contains(' ') && text.contains('.') {
            format!("https://{text}")
        } else {
            self.act_error = Some("Type a URL in the bar, then Open".into());
            cx.notify();
            return;
        };
        self.type_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.act(serde_json::json!({ "kind": "openUrl", "url": url }), cx);
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
                    let file_icon = if entry.is_dir {
                        crate::file_icons::icon(
                            crate::file_icons::FileIconIdentity::directory(&entry.name, false),
                            theme.appearance,
                        )
                    } else {
                        crate::file_icons::icon(
                            crate::file_icons::FileIconIdentity::file(&entry.name),
                            theme.appearance,
                        )
                    };
                    let fade = SharedString::from(format!("rail-file-{ix}"));
                    list = list.child(
                        div()
                            .id(("rail-file", ix))
                            .w_full()
                            .h(px(crate::files::tree::TREE_ROW_HEIGHT))
                            .px(px(8.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .cursor_pointer()
                            .bg(crate::motion::hover_blend(
                                &fade,
                                gpui::transparent_black(),
                                crate::theme::wash(0.055),
                            ))
                            .on_hover(crate::motion::hover_listener(fade.clone()))
                            .on_click({
                                let entry = entry.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.open_entry(entry.clone(), cx)
                                })
                            })
                            .child(file_icon.size(px(14.0)).flex_none())
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(theme.font_sans.clone())
                                    .text_size(px(11.5))
                                    .text_color(theme.text_muted)
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
                    .text_color(theme.text_muted)
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
            .child(body)
            .children(file_viewer)
            .into_any_element()
    }

    fn render_changes(&mut self, theme: &Theme, _cx: &mut Context<Self>) -> AnyElement {
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
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(theme.border)
                    .overflow_y_scroll()
                    .p(px(10.0))
                    .child({
                        let mut lines = div().flex().flex_col();
                        for (kind, text) in diff_lines(diff) {
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
                        lines
                    })
                    .into_any_element(),
            ),
            _ => None,
        };
        div()
            .size_full()
            .flex()
            .flex_col()
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
        let mut log = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .p(px(8.0))
            .font_family(theme.font_mono.clone());
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
                    .h(px(crate::surface_chrome::HEADER_HEIGHT))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_none()
                            .font_family(theme.font_mono.clone())
                            .text_size(crate::typography::ui_rems(11.5))
                            .text_color(theme.text_muted)
                            .child("$"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(theme.font_mono.clone())
                            .child(self.shell_input.clone().into_any_element()),
                    ),
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
                                .text_color(theme.text_muted)
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
        let caps = self.capabilities();
        let has_image = matches!(self.shot, Loadable::Ready(Some(_)));
        let desktop_status = self
            .vm_info
            .as_ref()
            .and_then(|v| v.get("desktop"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let mut col = div().size_full().flex().flex_col();
        let start_button = |label: &'static str, this: &Self, cx: &mut Context<Self>| {
            popover::btn_primary(theme, label)
                .id("rail-start-desktop")
                .when(this.desktop_starting, |el| el.opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| this.start_desktop(cx)))
        };
        let body = match &self.shot {
            Loadable::Ready(Some(image)) => {
                let bounds = self.shot_bounds.clone();
                div()
                    .id("rail-shot-surface")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .m(px(8.0))
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .border_1()
                    .border_color(if self.controlling {
                        theme.accent.opacity(0.6)
                    } else {
                        theme.border
                    })
                    .bg(gpui::hsla(0.0, 0.0, 0.05, 1.0))
                    .when(self.controlling, |el| el.cursor_crosshair())
                    .child(
                        gpui::img(image.clone())
                            .size_full()
                            .object_fit(gpui::ObjectFit::Contain),
                    )
                    .child(
                        gpui::canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                            .absolute()
                            .inset_0(),
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, event, _, cx| this.click_preview(event, cx)),
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(|this, event, _, cx| this.click_preview(event, cx)),
                    )
                    .on_scroll_wheel(
                        cx.listener(|this, event, _, cx| this.scroll_preview(event, cx)),
                    )
                    .into_any_element()
            }
            Loadable::Ready(None) if self.vm_info.is_some() => {
                let installing = matches!(desktop_status.as_str(), "installing" | "starting");
                let copy: SharedString = if self.desktop_starting || installing {
                    "Bringing up a virtual display (Xvfb) on the seat. The first start installs \
                     it — about a minute."
                        .into()
                } else if let Some(err) = self.desktop_error.clone() {
                    err.into()
                } else if let Some(failed) = desktop_status.strip_prefix("failed") {
                    format!("The desktop couldn't start{failed}").into()
                } else {
                    "The agent gets a desktop on every run — or start it now to watch and \
                     drive it: screenshots, clicks, typing, a browser, recordings."
                        .into()
                };
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex_none()
                            .p(px(12.0))
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .text_color(theme.text_muted)
                                    .child(copy),
                            )
                            .when(caps.computer, |el| {
                                el.child(div().flex().child(start_button(
                                    if self.desktop_starting || installing {
                                        "Starting…"
                                    } else {
                                        "Start desktop"
                                    },
                                    self,
                                    cx,
                                )))
                            }),
                    )
                    .child(render_vm_screen(
                        theme,
                        self.vm_info.as_ref().unwrap_or(&serde_json::Value::Null),
                    ))
                    .into_any_element()
            }
            Loadable::Ready(None) => pane_empty(
                theme,
                icons::MONITOR,
                "No desktop yet",
                "This seat can't show a screen until it can run commands — Files, \
                 Changes and Pull requests still work.",
            )
            .into_any_element(),
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
        if has_image && self.controlling {
            let key = |id: &'static str, label: &'static str, combo: &'static str| {
                div()
                    .id(id)
                    .flex_none()
                    .px(px(7.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(theme.border)
                    .text_size(crate::typography::ui_rems(10.5))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.element_hover))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.act(serde_json::json!({ "kind": "key", "combo": combo }), cx)
                    }))
            };
            col = col.child(
                div()
                    .flex_none()
                    .px(px(8.0))
                    .pb(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .child(div().flex_1().min_w_0().child(popover::dialog_field(
                                self.type_input.clone().into_any_element(),
                            )))
                            .child(
                                popover::btn_ghost(theme, "Open", "rail-open-url")
                                    .id("rail-open-url")
                                    .tooltip(widgets::text_tooltip(
                                        "Open the URL in the bar in a browser on the seat",
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| this.open_on_seat(cx))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(4.0))
                            .child(key("seat-key-enter", "Enter", "Return"))
                            .child(key("seat-key-tab", "Tab", "Tab"))
                            .child(key("seat-key-esc", "Esc", "Escape"))
                            .child(key("seat-key-bksp", "⌫", "BackSpace"))
                            .child(key("seat-key-up", "↑", "Up"))
                            .child(key("seat-key-down", "↓", "Down"))
                            .child(key("seat-key-selall", "Ctrl+A", "ctrl+a"))
                            .child(key("seat-key-copy", "Ctrl+C", "ctrl+c"))
                            .child(key("seat-key-paste", "Ctrl+V", "ctrl+v"))
                            .child(key("seat-key-url", "Ctrl+L", "ctrl+l")),
                    ),
            );
        }
        if let Some(err) = self.act_error.clone() {
            col = col.child(
                div()
                    .flex_none()
                    .px(px(12.0))
                    .pb(px(8.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.danger)
                    .child(SharedString::from(err)),
            );
        }
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
            if self.runs.is_empty() {
                return pane_empty(
                    theme,
                    icons::SORT_VERTICAL,
                    "Progress",
                    "Runs on this seat show up here.",
                )
                .into_any_element();
            }
            let state = self.state.read(cx);
            let mut list = div().flex().flex_col().p(px(8.0));
            for run in &self.runs {
                let prompt = cloud_run_prompt(run, &state.transcript);
                let (status_icon, status_color) = match run.status {
                    CloudRunStatus::Done => (icons::QUEUE_CHECK, theme.success),
                    CloudRunStatus::Error | CloudRunStatus::Stopped => {
                        (icons::DANGER_TRIANGLE, theme.danger)
                    }
                    _ => (icons::CLOCK_CIRCLE, theme.text_muted),
                };
                let end = run.finished_at.unwrap_or_else(chrono::Utc::now);
                let elapsed =
                    crate::transcript::format_elapsed((end - run.started_at).num_seconds().max(0));
                list = list.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .h(px(24.0))
                        .child(
                            icon(status_icon)
                                .size(px(11.0))
                                .flex_none()
                                .text_color(status_color),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::typography::ui_rems(12.0))
                                .text_color(theme.text)
                                .child(SharedString::from(prompt)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(crate::typography::ui_rems(10.0))
                                .text_color(theme.text_faint)
                                .child(SharedString::from(format!(
                                    "{} · {elapsed}",
                                    run.status.label()
                                ))),
                        ),
                );
            }
            return div()
                .id("rail-progress-cloud-runs")
                .size_full()
                .overflow_y_scroll()
                .child(list)
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
        let caps = self.capabilities();
        let backup = self
            .state
            .read(cx)
            .cloud_view()
            .and_then(|view| view.backup_for_chat(&self.chat_id))
            .cloned();
        let workspace = self
            .binding
            .workspace_path
            .clone()
            .unwrap_or_else(|| "seat workspace".into());
        let section = |title: &'static str| {
            div()
                .pt(px(12.0))
                .pb(px(4.0))
                .px(px(4.0))
                .text_size(crate::typography::ui_rems(10.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text_faint)
                .child(title)
        };
        let mut list = div().flex().flex_col().px(px(8.0)).pb(px(12.0));

        // Workspace: where the work lives and how it's kept.
        let storage: SharedString = match (&self.binding.repo, &backup) {
            (Some(repo), _) => {
                format!("Checkout of {repo} — commits and PRs are the record").into()
            }
            (None, Some(b)) => format!(
                "Local git checkpoint every turn · backed up to this computer {} ({})",
                crate::cloud::boards::relative(b.taken_at, chrono::Utc::now()),
                zeron_engine::cloud::human_bytes(b.bytes)
            )
            .into(),
            (None, None) => {
                "Local git checkpoint every turn · backs up to this computer after each turn".into()
            }
        };
        list = list.child(section("WORKSPACE")).child(
            div()
                .rounded(px(10.0))
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface_card)
                .p(px(10.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            icon(if self.binding.repo.is_some() {
                                icons::GIT_BRANCH
                            } else {
                                icons::FOLDER
                            })
                            .size(px(12.0))
                            .text_color(theme.text_muted),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_family(theme.font_mono.clone())
                                .text_size(crate::typography::ui_rems(11.5))
                                .text_color(theme.text)
                                .child(SharedString::from(workspace)),
                        ),
                )
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .child(storage),
                )
                .when(caps.exec, |el| {
                    el.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                popover::btn_ghost(
                                    theme,
                                    if self.backup_busy {
                                        "Backing up…"
                                    } else {
                                        "Back up now"
                                    },
                                    "rail-backup-now",
                                )
                                .id("rail-backup-now")
                                .on_click(cx.listener(|this, _, _, cx| this.backup_now(cx))),
                            )
                            .when_some(self.backup_note.clone(), |el, note| {
                                let (text, color) = match note {
                                    Ok(text) => (text, theme.success),
                                    Err(text) => (text, theme.danger),
                                };
                                el.child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_size(crate::typography::ui_rems(10.5))
                                        .text_color(color)
                                        .child(SharedString::from(text)),
                                )
                            }),
                    )
                }),
        );

        // Evidence: what the agent captured to show its work.
        list = list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .child(section("SCREENSHOTS & RECORDINGS"))
                .child(div().flex_1())
                .when(caps.exec, |el| {
                    el.child(
                        div()
                            .id("rail-evidence-refresh")
                            .pt(px(8.0))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.load_evidence(cx)))
                            .child(
                                icon(icons::REFRESH)
                                    .size(px(12.0))
                                    .text_color(theme.text_muted),
                            ),
                    )
                }),
        );
        match &self.evidence {
            Loadable::Ready(items) if !items.is_empty() => {
                for (ix, (name, size)) in items.iter().enumerate() {
                    let is_image = name.ends_with(".png");
                    let open = self
                        .evidence_preview
                        .as_ref()
                        .filter(|(open, _)| open == name)
                        .map(|(_, slot)| slot.clone());
                    let row_name = name.clone();
                    list = list.child(
                        div()
                            .id(("rail-evidence", ix))
                            .h(px(26.0))
                            .px(px(4.0))
                            .rounded(px(6.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .when(is_image, |el| {
                                el.cursor_pointer()
                                    .hover(|s| s.bg(theme.element_hover))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.preview_evidence(row_name.clone(), cx)
                                    }))
                            })
                            .child(
                                icon(if is_image {
                                    icons::MONITOR
                                } else {
                                    icons::DOCUMENT
                                })
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
                                    .text_color(theme.text)
                                    .child(SharedString::from(name.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(10.0))
                                    .text_color(theme.text_faint)
                                    .child(SharedString::from(zeron_engine::cloud::human_bytes(
                                        *size,
                                    ))),
                            ),
                    );
                    if let Some(slot) = open {
                        list = list.child(match slot {
                            Loadable::Ready((image, ratio)) => div()
                                .mx(px(4.0))
                                .mb(px(6.0))
                                .rounded(px(8.0))
                                .overflow_hidden()
                                .border_1()
                                .border_color(theme.border)
                                .child(
                                    gpui::img(image)
                                        .w_full()
                                        .aspect_ratio(ratio)
                                        .object_fit(gpui::ObjectFit::Contain),
                                )
                                .into_any_element(),
                            Loadable::Error(err) => div()
                                .px(px(4.0))
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(err))
                                .into_any_element(),
                            _ => div()
                                .px(px(4.0))
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child("Loading…")
                                .into_any_element(),
                        });
                    }
                }
            }
            Loadable::Error(err) => {
                list = list.child(
                    div()
                        .px(px(4.0))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(err.clone())),
                );
            }
            Loadable::Loading => {
                list = list.child(
                    div()
                        .px(px(4.0))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .child("Loading…"),
                );
            }
            _ => {
                list = list.child(
                    div()
                        .px(px(4.0))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .child(
                            "Nothing yet — the agent saves screenshots and recordings here \
                             (cosmos-computer screenshot / record).",
                        ),
                );
            }
        }

        // Files the run touched (structured tool calls).
        if !paths.is_empty() {
            list = list.child(section("FILES TOUCHED"));
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
    /// One shared surface toolbar with seat controls and pane-specific content.
    fn render_toolbar(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let caps = self.capabilities();
        let seat = self.seat.ready();
        let seat_label = seat
            .map(|s| s.config.label.clone())
            .unwrap_or_else(|| self.binding.label.clone());
        let run_status = self.run.as_ref().map(|r| r.status);
        let probe = seat.and_then(|s| s.config.last_probe.clone());
        let busy = self.control_busy;
        let is_boat = seat.is_some_and(|s| s.config.kind == Some(CloudSeatKind::Box));
        let sleeping = probe.as_ref().is_some_and(|p| p.sleeping);

        // The status line under the name: the most useful single fact.
        let now = chrono::Utc::now();
        let is_railway = seat.is_some_and(|s| s.config.kind == Some(CloudSeatKind::Railway))
            || self.binding.kind == CloudSeatKind::Railway;
        let trial_left = self
            .binding
            .expires_at
            .map(|at| at.signed_duration_since(now).num_seconds());
        let trial_ended = is_railway
            && (seat.is_some_and(|s| s.config.expired())
                || self.binding.expires_at.is_some_and(|at| at <= now));
        let (meta, meta_color) = if trial_ended {
            ("Trial ended".to_string(), theme.text_muted)
        } else {
            match (trial_left, run_status) {
                (Some(secs), _) => (
                    if secs < 60 {
                        "Trial · <1m left".to_string()
                    } else {
                        format!("Trial · {}m left", secs / 60)
                    },
                    if secs < 10 * 60 {
                        theme.text
                    } else {
                        theme.text_muted
                    },
                ),
                (None, _) if sleeping => ("Sleeping · billing off".to_string(), theme.text_muted),
                (None, Some(status)) => (status.label().to_string(), theme.text_muted),
                (None, None) => (
                    match probe.as_ref() {
                        Some(p) if p.online => "Online".to_string(),
                        Some(_) => "Offline".to_string(),
                        None => "Not checked yet".to_string(),
                    },
                    theme.text_muted,
                ),
            }
        };
        let meta = match (&meta[..], run_status) {
            // A live trial still reports what the agent is doing.
            (m, Some(status)) if m.starts_with("Trial ·") => format!("{m} · {}", status.label()),
            _ => meta,
        };

        let mut buttons = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0));
        match run_status {
            Some(CloudRunStatus::Working) | Some(CloudRunStatus::Queued) => {
                buttons = buttons
                    .child(
                        rail_icon_button(
                            "pause",
                            icons::PAUSE,
                            "Pause run",
                            theme.text_muted,
                            busy,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.seat_control("pause", cx))),
                    )
                    .child(
                        rail_icon_button("stop", icons::STOP, "Stop run", theme.text_muted, busy)
                            .on_click(cx.listener(|this, _, _, cx| this.seat_control("stop", cx))),
                    );
            }
            Some(CloudRunStatus::Stopped) => {
                buttons = buttons.child(
                    rail_icon_button("resume", icons::PLAY, "Resume run", theme.text_muted, busy)
                        .on_click(cx.listener(|this, _, _, cx| this.seat_control("resume", cx))),
                );
            }
            Some(CloudRunStatus::NeedsApproval) => {
                buttons = buttons.child(
                    rail_icon_button(
                        "approve",
                        icons::CHECK_CIRCLE,
                        "Approve",
                        theme.text_muted,
                        busy,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.seat_control("approve", cx))),
                );
            }
            _ => {}
        }
        let left: AnyElement = match self.pane {
            CloudPane::Files => div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    rail_icon_button(
                        "files-up",
                        icons::ALT_ARROW_UP,
                        "Go up",
                        theme.text_muted,
                        self.parents.is_empty(),
                    )
                    .when(!self.parents.is_empty(), |button| {
                        button.on_click(cx.listener(|this, _, _, cx| this.files_up(cx)))
                    }),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(11.5))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(self.cwd.clone())),
                )
                .into_any_element(),
            CloudPane::Changes => div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    icon(icons::GIT_BRANCH)
                        .size(px(14.0))
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
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
                .into_any_element(),
            CloudPane::Computer => {
                let has_image = matches!(self.shot, Loadable::Ready(Some(_)));
                let mode: SharedString = match &self.shot {
                    Loadable::Ready(Some(_)) if self.controlling => "You're in control".into(),
                    Loadable::Ready(Some(_)) => match self.shot_size {
                        Some((w, h)) => format!("Live desktop · {w}×{h} · every 4s").into(),
                        None => "Live desktop · every 4s".into(),
                    },
                    _ if self.desktop_starting => "Starting the seat's desktop…".into(),
                    Loadable::Ready(None) if self.vm_info.is_some() => {
                        "Linux console · no display".into()
                    }
                    Loadable::Error(_) => "Not reachable".into(),
                    Loadable::Ready(None) => "No display".into(),
                    _ => "Connecting…".into(),
                };
                let take_control = div()
                    .id("rail-take-control")
                    .h(px(24.0))
                    .px(px(8.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text)
                    .bg(if self.controlling {
                        crate::theme::wash(0.12)
                    } else {
                        crate::theme::wash(0.0)
                    })
                    .hover(|el| el.bg(crate::theme::wash(0.12)))
                    .tooltip(widgets::text_tooltip(if has_image {
                        "Drive the seat's desktop yourself — the agent keeps its own control"
                    } else {
                        "Start the desktop to take control"
                    }))
                    .child(icon(icons::MONITOR).size(px(14.0)).text_color(theme.text))
                    .child(if self.controlling {
                        "Release"
                    } else {
                        "Take control"
                    })
                    .when(has_image, |el| {
                        el.on_click(cx.listener(|this, _, _, cx| {
                            this.controlling = !this.controlling;
                            this.act_error = None;
                            cx.notify();
                        }))
                    });
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(crate::typography::ui_rems(11.5))
                            .text_color(theme.text_muted)
                            .child(mode),
                    )
                    .child(take_control)
                    .into_any_element()
            }
            CloudPane::Shell => div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(crate::typography::ui_rems(11.5))
                .text_color(theme.text_muted)
                .font_family(theme.font_mono.clone())
                .child(SharedString::from(
                    self.binding
                        .workspace_path
                        .as_deref()
                        .unwrap_or("seat workspace")
                        .to_string(),
                ))
                .into_any_element(),
            _ => div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(11.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(SharedString::from(seat_label.clone())),
                )
                .child(status_dot(theme, probe.as_ref(), trial_ended))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(10.5))
                        .text_color(meta_color)
                        .child(SharedString::from(format!("· {meta}"))),
                )
                .into_any_element(),
        };
        let refresh = match self.pane {
            CloudPane::Files => Some(
                rail_icon_button(
                    "files-refresh",
                    icons::REFRESH,
                    "Refresh files",
                    theme.text_muted,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.load_files(cx)))
                .into_any_element(),
            ),
            CloudPane::Changes => Some(
                rail_icon_button(
                    "changes-refresh",
                    icons::REFRESH,
                    "Refresh changes",
                    theme.text_muted,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.load_changes(cx)))
                .into_any_element(),
            ),
            CloudPane::PullRequests => Some(
                rail_icon_button(
                    "prs-refresh",
                    icons::REFRESH,
                    "Refresh pull requests",
                    theme.text_muted,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.load_prs(cx)))
                .into_any_element(),
            ),
            CloudPane::Computer if caps.computer || caps.shell => Some(
                rail_icon_button(
                    "computer-refresh",
                    icons::REFRESH,
                    "Refresh screen",
                    theme.text_muted,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| this.capture_shot(cx)))
                .into_any_element(),
            ),
            _ => None,
        };
        let status = matches!(
            self.pane,
            CloudPane::Files | CloudPane::Changes | CloudPane::Computer | CloudPane::Shell
        )
        .then(|| {
            div()
                .id("rail-toolbar-status")
                .flex_none()
                .child(status_dot(theme, probe.as_ref(), trial_ended))
                .tooltip(widgets::text_tooltip(self.toolbar_status_tooltip.clone()))
                .into_any_element()
        });
        let mut trailing = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(2.0))
            .children(refresh)
            .child(buttons)
            .children(status);
        let menu_open = self.pane_menu.get().is_some();
        let menu_trigger = div()
            .id("rail-pane-menu-trigger")
            .size(px(crate::surface_chrome::CONTROL_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(crate::surface_chrome::CONTROL_RADIUS))
            .cursor_pointer()
            .bg(if menu_open {
                crate::theme::wash(0.11)
            } else {
                crate::theme::wash(0.0)
            })
            .hover(|el| el.bg(crate::theme::wash(0.11)))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.pane_menu.note_trigger_press()),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                if this.pane_menu.take_press_was_open() {
                    this.close_pane_menu(cx);
                } else {
                    this.pane_menu.open(());
                    cx.notify();
                }
            }))
            .tooltip(widgets::text_tooltip("Seat actions"))
            .child(
                icon(icons::MORE_HORIZONTAL)
                    .size(px(crate::surface_chrome::ICON_SIZE))
                    .text_color(theme.text_muted),
            );
        if menu_open {
            let closing = self.pane_menu.closing_since();
            let mut menu = div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .px(px(10.0))
                        .py(px(8.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(11.5))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(SharedString::from(seat_label)),
                        )
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(meta)),
                        ),
                )
                .child(popover::menu_separator())
                .child(
                    popover::menu_row(theme, false, "rail-menu-probe")
                        .id("rail-menu-probe")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_pane_menu(cx);
                            this.probe_seat(cx);
                        }))
                        .child(
                            icon(icons::REFRESH)
                                .size(px(13.0))
                                .text_color(theme.text_muted),
                        )
                        .child("Check seat"),
                );
            if self.state.read(cx).workspace_scope == Some(zeron_proto::WorkspaceScope::Synced) {
                menu = menu.child(
                    popover::menu_row(theme, false, "rail-menu-return")
                        .id("rail-menu-return")
                        .when(self.control_busy, |row| row.opacity(0.45))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.control_busy {
                                return;
                            }
                            this.close_pane_menu(cx);
                            this.return_chat(false, cx);
                        }))
                        .child(
                            icon(icons::ARROW_LEFT)
                                .size(px(13.0))
                                .text_color(theme.text_muted),
                        )
                        .child("Return to this Mac"),
                );
            }
            match seat.and_then(|s| s.config.preview_url.clone()) {
                Some(url) => {
                    menu = menu.child(
                        popover::menu_row(theme, false, "rail-menu-preview")
                            .id("rail-menu-preview")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.close_pane_menu(cx);
                                cx.open_url(&url);
                            }))
                            .child(
                                icon(icons::GLOBE)
                                    .size(px(13.0))
                                    .text_color(theme.text_muted),
                            )
                            .child("Open preview"),
                    );
                }
                None => {
                    menu = menu.child(
                        popover::menu_row(theme, false, "rail-menu-screen")
                            .id("rail-menu-screen")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_pane_menu(cx);
                                cx.emit(RailEvent::OpenPane(CloudPane::Computer));
                            }))
                            .child(
                                icon(icons::MONITOR)
                                    .size(px(13.0))
                                    .text_color(theme.text_muted),
                            )
                            .child("Show screen"),
                    );
                }
            }
            if is_boat && seat.is_some_and(|s| s.config.box_id.is_some()) {
                let (label, glyph, op) = if sleeping {
                    ("Wake box", icons::SUN, "wake")
                } else {
                    ("Sleep box", icons::MOON, "sleep")
                };
                menu = menu.child(
                    popover::menu_row(theme, false, "rail-menu-box")
                        .id("rail-menu-box")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.close_pane_menu(cx);
                            this.seat_control(op, cx);
                        }))
                        .child(icon(glyph).size(px(13.0)).text_color(theme.text_muted))
                        .child(label),
                );
            }
            if self.confirm_destroy {
                menu = menu.child(
                    popover::menu_row(theme, false, "rail-menu-destroy-confirm")
                        .id("rail-menu-destroy-confirm")
                        .text_color(theme.danger)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_pane_menu(cx);
                            this.seat_control("destroy", cx);
                        }))
                        .child(
                            icon(icons::TRASH_BIN_MINIMALISTIC)
                                .size(px(13.0))
                                .text_color(theme.danger),
                        )
                        .child("Click again to destroy"),
                );
            } else {
                menu = menu.child(
                    popover::menu_row(theme, false, "rail-menu-destroy")
                        .id("rail-menu-destroy")
                        .when(!caps.exec, |row| row.opacity(0.45))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.control_busy || !this.capabilities().exec {
                                return;
                            }
                            this.confirm_destroy = true;
                            this.confirm_task = Some(cx.spawn(async move |this, cx| {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_secs(4))
                                    .await;
                                this.update(cx, |rail, cx| {
                                    rail.confirm_destroy = false;
                                    rail.confirm_task = None;
                                    cx.notify();
                                })
                                .ok();
                            }));
                            cx.notify();
                        }))
                        .child(
                            icon(icons::TRASH_BIN_MINIMALISTIC)
                                .size(px(13.0))
                                .text_color(theme.text_muted),
                        )
                        .child("Destroy seat"),
                );
            }
            let menu = popover::popover_card(theme)
                .w(px(220.0))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.close_pane_menu(cx);
                }))
                .child(menu)
                .into_any_element();
            trailing = trailing.child(menu_trigger.relative().child(
                popover::anchored_menu_below_gap("rail-pane-menu", menu, closing, 10.0),
            ));
        } else {
            trailing = trailing.child(menu_trigger);
        }
        let toolbar = crate::surface_chrome::toolbar(theme)
            .child(left)
            .child(trailing);
        toolbar.into_any_element()
    }

    /// The rail's one notice card — the single most important thing the user
    /// should know about this seat right now, with its fix as a button.
    /// Ordered: a dead trial, the keep prompt, a live trial's clock, a
    /// sleeping box, a desk-bound seat. A control error rides above it.
    fn render_notices(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
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
        // Where a dead trial's thread can continue: the best live seat.
        let move_target: Option<(String, String)> = railway_expired
            .then(|| {
                let state = self.state.read(cx);
                let seats: Vec<SeatRow> = state
                    .cloud_seats()
                    .iter()
                    .filter(|s| s.config.id != self.binding.seat_id && CloudView::seat_runnable(s))
                    .cloned()
                    .collect();
                CloudView::preferred_seat(&seats).map(|s| {
                    (
                        s.config.id.clone(),
                        match s.config.kind {
                            Some(CloudSeatKind::Railway) => "a new trial".to_string(),
                            _ => s.config.label.clone(),
                        },
                    )
                })
            })
            .flatten();
        // The "Keep this computer" prompt shows once: a Railway seat that
        // finished a run and has never been asked.
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

        let mut cards: Vec<AnyElement> = Vec::new();
        if let Some(err) = self.control_error.clone() {
            let error_card = notice_card(
                    theme,
                    NoticeTone::Danger,
                    icons::DANGER_TRIANGLE,
                    "Seat error",
                    err,
                )
                .child(notice_actions()
                    .when(
                        self.control_error.as_deref()
                            == Some("uncommitted changes on the runner — push them to a WIP branch first"),
                        |actions| actions.child(
                            notice_button(
                                "runner-push-wip",
                                "Push a WIP branch and continue",
                                theme,
                                true,
                            )
                            .when(self.control_busy, |el| el.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.control_busy {
                                    this.return_chat(true, cx);
                                }
                            })),
                        ),
                    )
                    .child(
                        notice_button("notice-dismiss", "Dismiss", theme, false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.control_error = None;
                                cx.notify();
                            },
                        )),
                    ));
            cards.push(error_card.into_any_element());
        }

        let card = if railway_expired {
            let body = match (&move_target, backed_up) {
                (Some(_), true) => {
                    "Your work is backed up on this computer. Continue on another seat and it comes back."
                }
                (Some(_), false) => "Continue this chat on another seat to keep going.",
                (None, true) => {
                    "Your work is backed up here. Add a Boat token to continue on a persistent cloud PC."
                }
                (None, false) => {
                    "Claim the VM from Railway, or add a Boat token for a persistent cloud PC."
                }
            };
            let mut actions = notice_actions();
            if let Some((seat_id, label)) = move_target {
                actions = actions.child(
                    notice_button(
                        "rail-move-chat",
                        &format!("Continue on {label}"),
                        theme,
                        true,
                    )
                    .when(self.control_busy, |el| el.opacity(0.5))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.move_chat(seat_id.clone(), cx)),
                    ),
                );
            } else {
                actions = actions.child(
                    notice_button("rail-attach-boat", "Add Boat token", theme, true).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.mark_keep_prompted(cx);
                            cx.emit(RailEvent::OpenCloudSettings);
                        }),
                    ),
                );
            }
            if let Some(url) = claim_url.clone() {
                actions = actions.child(
                    notice_button("rail-claim", "Claim", theme, false)
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
            Some(
                notice_card(
                    theme,
                    NoticeTone::Muted,
                    icons::CLOCK_CIRCLE,
                    "Trial ended",
                    body,
                )
                .child(actions),
            )
        } else if show_keep_prompt {
            let mut actions = notice_actions().child(
                notice_button("keep-yes", "Add Boat token", theme, true).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.mark_keep_prompted(cx);
                        cx.emit(RailEvent::OpenCloudSettings);
                    },
                )),
            );
            if let Some(url) = claim_url.clone() {
                actions = actions.child(
                    notice_button("keep-claim", "Claim", theme, false)
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
            actions = actions.child(
                notice_button("keep-no", "Not now", theme, false)
                    .on_click(cx.listener(|this, _, _, cx| this.mark_keep_prompted(cx))),
            );
            Some(
                notice_card(
                    theme,
                    NoticeTone::Muted,
                    icons::STAR,
                    "Keep this computer?",
                    "The trial seat lasts an hour. Add a Boat token for a persistent cloud PC, or claim this one on Railway.",
                )
                .child(actions),
            )
        } else if is_railway {
            let card = notice_card(
                theme,
                NoticeTone::Warning,
                icons::CLOCK_CIRCLE,
                "Free trial seat",
                "It shuts down when the hour ends unless you claim it or add a Boat token.",
            );
            Some(match claim_url {
                Some(url) => card.child(
                    notice_actions().child(
                        notice_button("rail-claim", "Claim on Railway", theme, false)
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    ),
                ),
                None => card,
            })
        } else if is_boat && sleeping {
            Some(
                notice_card(
                    theme,
                    NoticeTone::Warning,
                    icons::MOON,
                    "Box is sleeping",
                    "Billing is off. Wake it to keep working — your files are kept.",
                )
                .child(
                    notice_actions().child(
                        notice_button("rail-wake", "Wake", theme, true)
                            .when(self.control_busy, |el| el.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| this.seat_control("wake", cx))),
                    ),
                ),
            )
        } else if not_persistent && self.seat.ready().is_some() {
            Some(notice_card(
                theme,
                NoticeTone::Muted,
                icons::INFO_CIRCLE,
                "Runs while Cosmos is open",
                "This seat stops working when you close the desktop app.",
            ))
        } else {
            None
        };
        cards.extend(card.map(IntoElement::into_any_element));
        (!cards.is_empty()).then(|| {
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .p(px(8.0))
                .border_b_1()
                .border_color(theme.border)
                .children(cards)
                .into_any_element()
        })
    }
}

/// A toolbar icon button with the shared surface-control metrics.
fn rail_icon_button(
    id: &'static str,
    glyph: &'static str,
    tip: &'static str,
    color: gpui::Hsla,
    dimmed: bool,
) -> gpui::Stateful<gpui::Div> {
    let fade = format!("rail-icon-{id}");
    div()
        .id(SharedString::from(fade.clone()))
        .size(px(crate::surface_chrome::CONTROL_SIZE))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(crate::surface_chrome::CONTROL_RADIUS))
        .cursor_pointer()
        .bg(crate::motion::hover_blend(
            &fade,
            crate::theme::wash(0.0),
            crate::theme::wash(0.11),
        ))
        .on_hover(crate::motion::hover_listener(fade))
        .when(dimmed, |el| el.opacity(0.45))
        .tooltip(widgets::text_tooltip(tip))
        .child(
            icon(glyph)
                .size(px(crate::surface_chrome::ICON_SIZE))
                .text_color(color),
        )
}

#[derive(Clone, Copy)]
enum NoticeTone {
    Danger,
    Warning,
    Muted,
}

/// The rail's notice card: a tinted rounded-10 card with a glyph, a title,
/// one line of body, and (via `.child(notice_actions()…)`) its buttons.
fn notice_card(
    theme: &Theme,
    tone: NoticeTone,
    glyph: &'static str,
    title: &'static str,
    body: impl Into<SharedString>,
) -> gpui::Div {
    let tint = if matches!(tone, NoticeTone::Danger) {
        theme.danger
    } else {
        theme.text_muted
    };
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .p(px(10.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(theme.border)
        .bg(crate::theme::wash(0.04))
        .child(
            div()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(8.0))
                .child(
                    div()
                        .pt(px(1.0))
                        .flex_none()
                        .child(icon(glyph).size(px(14.0)).text_color(tint)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(11.5))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.text_muted)
                                .child(body.into()),
                        ),
                ),
        )
}

/// The notice card's button row, indented under the title.
fn notice_actions() -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(6.0))
        .pl(px(22.0))
}

/// A notice-card button: compact rounded-md pill; `primary` uses the solid
/// theme fill, the rest the agent-mode wash.
fn notice_button(
    id: &'static str,
    label: &str,
    theme: &Theme,
    primary: bool,
) -> gpui::Stateful<gpui::Div> {
    let fade = format!("rail-notice-{id}");
    let (rest, hover, text) = if primary {
        (theme.solid, theme.solid.opacity(0.85), theme.on_solid)
    } else {
        (
            crate::theme::wash(0.06),
            crate::theme::wash(0.12),
            theme.text,
        )
    };
    div()
        .id(SharedString::from(fade.clone()))
        .h(px(24.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .text_size(crate::typography::ui_rems(11.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(text)
        .bg(crate::motion::hover_blend(&fade, rest, hover))
        .on_hover(crate::motion::hover_listener(fade))
        .child(SharedString::from(label.to_string()))
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
        let notices = self.render_notices(&theme, cx);
        let mut col = div().size_full().flex().flex_col();
        let show_picker = self.picking && !matches!(&self.seat, Loadable::Error(_));
        if !show_picker {
            col = col.child(self.render_toolbar(&theme, cx));
        }
        col = col.children(notices);
        match &self.seat {
            Loadable::Error(err) => {
                col = col.child(div().flex_1().min_h_0().child(
                    pane_empty(&theme, icons::CLOUD, "Cloud seat", err.as_str()).into_any_element(),
                ));
            }
            _ if show_picker => {
                col = col.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_picker(&theme, cx)),
                );
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
