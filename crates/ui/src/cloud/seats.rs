//! Settings → Cloud computer: Railway trial, Boat token, and GitHub connection.

use gpui::{
    AnyElement, Context, Entity, IntoElement, Render, SharedString, Task, Window, div, prelude::*,
    px,
};
use zeron_proto::{ByokProvider, CloudSeatKind, HarnessId, WorkspaceScope};
use zeron_rpc::methods;

use zeron_engine::cloud::SeatConfig;

use crate::cloud::{CloudView, SeatRow};
use crate::composer::ComposerInput;
use crate::icons::{self, icon};
use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

struct SeatForm {
    box_token: Entity<ComposerInput>,
}

impl SeatForm {
    fn new(cx: &mut Context<CloudSeatsPage>) -> Self {
        let mut field =
            |placeholder: &'static str| cx.new(|cx| ComposerInput::new(placeholder, cx));
        Self {
            box_token: field("Boat token (boat_…)"),
        }
    }

    fn text(input: &Entity<ComposerInput>, cx: &gpui::App) -> String {
        input.read(cx).text().trim().to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloudSeatsEvent {
    StartTrial,
    ContinueWithRailway,
    SignIn,
}

impl gpui::EventEmitter<CloudSeatsEvent> for CloudSeatsPage {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RailwayCard {
    Start,
    Starting,
    AwaitingSignup,
    SignIn,
    Live {
        minutes_left: Option<i64>,
        can_keep: bool,
    },
    Ended {
        can_keep: bool,
    },
    Failed(String),
}

pub(crate) fn railway_card(
    seat: Option<&SeatRow>,
    starting: bool,
    waiting: bool,
    failure: Option<&str>,
) -> RailwayCard {
    if waiting {
        return RailwayCard::AwaitingSignup;
    }
    if starting {
        return RailwayCard::Starting;
    }
    let Some(seat) = seat else {
        return RailwayCard::Start;
    };
    if seat.config.signup_url.is_some() {
        return RailwayCard::SignIn;
    }
    if seat.config.expired() {
        return RailwayCard::Ended {
            can_keep: seat.config.claim_url.is_some(),
        };
    }
    let minutes_left = seat.config.expires_at.map(|at| {
        at.signed_duration_since(chrono::Utc::now())
            .num_minutes()
            .max(0)
    });
    if minutes_left.is_some()
        || seat
            .config
            .last_probe
            .as_ref()
            .is_some_and(|probe| probe.online)
    {
        return RailwayCard::Live {
            minutes_left,
            can_keep: seat.config.claim_url.is_some(),
        };
    }
    if failure.is_some()
        || seat
            .config
            .last_probe
            .as_ref()
            .is_some_and(|probe| !probe.online)
    {
        let reason = failure
            .filter(|reason| !reason.trim().is_empty())
            .or_else(|| {
                seat.config
                    .last_probe
                    .as_ref()
                    .and_then(|probe| probe.detail.as_deref())
                    .filter(|reason| !reason.trim().is_empty())
            })
            .unwrap_or("Railway didn't answer");
        return RailwayCard::Failed(reason.to_string());
    }
    RailwayCard::Start
}

struct GitHubDevice {
    user_code: String,
    verification_uri: String,
}

pub struct CloudSeatsPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    view: Loadable<CloudView>,
    load_generation: u64,
    runner_seat_id: Option<String>,
    runner_device_id: Option<String>,
    runner_info: Option<serde_json::Value>,
    runner_error: Option<String>,
    runner_note: Option<String>,
    runner_provider: Entity<ComposerInput>,
    runner_key: Entity<ComposerInput>,
    runner_base_url: Entity<ComposerInput>,
    runner_task: Option<Task<()>>,
    form: SeatForm,
    /// The shared GitHub PAT input — the repo picker's token when a seat has
    /// none of its own. Saved to `cloud-keys.json` under the reserved key.
    shared_pat: Entity<ComposerInput>,
    /// The Cosmos GitHub App device flow while it waits on the user.
    gh_device: Option<GitHubDevice>,
    gh_login: Option<String>,
    gh_task: Option<Task<()>>,
    show_pat: bool,
    error: Option<String>,
    note: Option<String>,
    load_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
    _state_sub: gpui::Subscription,
}

impl CloudSeatsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let form = SeatForm::new(cx);
        let state_sub = cx.observe(&state, |_, _, cx| cx.notify());
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            view: Loadable::Idle,
            load_generation: 0,
            runner_seat_id: None,
            runner_device_id: None,
            runner_info: None,
            runner_error: None,
            runner_note: None,
            runner_provider: cx
                .new(|cx| ComposerInput::new("Provider (anthropic, openai, github…)", cx)),
            runner_key: cx.new(|cx| ComposerInput::new("API key / token", cx)),
            runner_base_url: cx.new(|cx| ComposerInput::new("Base URL (optional)", cx)),
            runner_task: None,
            form,
            shared_pat: cx
                .new(|cx| ComposerInput::new("GitHub PAT — shared (repo + workflow scope)", cx)),
            gh_device: None,
            gh_login: None,
            gh_task: None,
            show_pat: false,
            error: None,
            note: None,
            load_task: None,
            action_task: None,
            _state_sub: state_sub,
        };
        page.load(cx);
        page
    }

    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.view = Loadable::Error("Engine not connected".into());
            return;
        };
        self.load_generation += 1;
        let generation = self.load_generation;
        self.view = Loadable::Loading;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::CLOUD_GET_CONFIG, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                if page.load_generation != generation {
                    return;
                }
                page.load_task = None;
                page.view = match result {
                    Ok(value) => match serde_json::from_value::<CloudView>(value) {
                        Ok(view) => Loadable::Ready(view),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            })
            .ok();
        }));
    }

    fn open_runner(&mut self, seat: &SeatConfig, cx: &mut Context<Self>) {
        self.runner_seat_id = Some(seat.id.clone());
        self.runner_device_id = seat.runner_device_id.clone();
        self.runner_info = None;
        self.runner_error = None;
        self.runner_note = None;
        if let Some(device_id) = self.runner_device_id.clone() {
            self.load_runner_info(device_id, cx);
        }
        cx.notify();
    }

    fn load_runner_info(&mut self, device_id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.runner_error = Some("Engine not connected".into());
            return;
        };
        self.runner_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::RUNNER_INFO,
                    serde_json::json!({ "targetDeviceId": device_id }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.runner_task = None;
                match result {
                    Ok(info) => {
                        page.runner_info = Some(info);
                        page.runner_error = None;
                    }
                    Err(err) => {
                        page.runner_info = None;
                        page.runner_error = Some(err.to_string());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn assign_runner(&mut self, device_id: Option<String>, cx: &mut Context<Self>) {
        let (Some(seat_id), Some(engine)) = (
            self.runner_seat_id.clone(),
            self.state.read(cx).engine().cloned(),
        ) else {
            self.runner_error = Some("Engine not connected".into());
            cx.notify();
            return;
        };
        self.runner_error = None;
        self.runner_note = None;
        self.runner_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_SET_SEAT_RUNNER,
                    serde_json::json!({ "seatId": seat_id, "deviceId": device_id }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.runner_task = None;
                match result {
                    Ok(_) => {
                        page.runner_device_id = device_id.clone();
                        page.runner_note = Some(if device_id.is_some() {
                            "Runner assigned to this seat.".into()
                        } else {
                            "Runner removed from this seat.".into()
                        });
                        page.runner_info = None;
                        page.load(cx);
                        if let Some(device_id) = device_id {
                            page.load_runner_info(device_id, cx);
                        }
                    }
                    Err(err) => page.runner_error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn seal_runner_key(&mut self, cx: &mut Context<Self>) {
        let Some(seat_id) = self.runner_seat_id.clone() else {
            return;
        };
        let provider = SeatForm::text(&self.runner_provider, cx);
        let key = SeatForm::text(&self.runner_key, cx);
        let base_url = SeatForm::text(&self.runner_base_url, cx);
        if provider.is_empty() || key.is_empty() {
            self.runner_error = Some("Enter a provider and key.".into());
            cx.notify();
            return;
        }
        self.runner_key
            .update(cx, |input, cx| input.set_text("", cx));
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.runner_error = Some("Engine not connected".into());
            cx.notify();
            return;
        };
        self.runner_error = None;
        self.runner_note = None;
        self.runner_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_SEAL_FOR_RUNNER,
                    serde_json::json!({
                        "seatId": seat_id,
                        "provider": provider,
                        "key": key,
                        "baseUrl": (!base_url.is_empty()).then_some(base_url),
                    }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.runner_task = None;
                match result {
                    Ok(_) => {
                        page.runner_note = Some("Credential sealed to the runner.".into());
                        if let Some(device_id) = page.runner_device_id.clone() {
                            page.load_runner_info(device_id, cx);
                        }
                    }
                    Err(err) => page.runner_error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn runner_action(
        &mut self,
        method: &'static str,
        params: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.runner_error = Some("Engine not connected".into());
            cx.notify();
            return;
        };
        let Some(device_id) = self.runner_device_id.clone() else {
            return;
        };
        self.runner_error = None;
        self.runner_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, params).await;
            this.update(cx, |page, cx| {
                page.runner_task = None;
                match result {
                    Ok(_) => {
                        page.runner_note = None;
                        page.load_runner_info(device_id, cx);
                    }
                    Err(err) => page.runner_error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn revoke_runner_credential(
        &mut self,
        provider: Option<String>,
        kind: String,
        cx: &mut Context<Self>,
    ) {
        let mut params = serde_json::json!({ "kind": kind });
        if let Some(provider) = provider {
            params["provider"] = provider.into();
        }
        if let Some(device_id) = self.runner_device_id.clone() {
            params["targetDeviceId"] = device_id.into();
        }
        self.runner_action(methods::RUNNER_REVOKE_CREDENTIAL, params, cx);
    }

    fn set_runner_login(&mut self, harness: HarnessId, enabled: bool, cx: &mut Context<Self>) {
        let Some(device_id) = self.runner_device_id.clone() else {
            return;
        };
        let harness = serde_json::to_value(harness).unwrap_or_default();
        self.runner_action(
            methods::RUNNER_SET_LOGIN,
            serde_json::json!({
                "targetDeviceId": device_id,
                "harness": harness,
                "enabled": enabled,
            }),
            cx,
        );
    }

    fn seat_control(&mut self, seat_id: &str, op: &str, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let seat_id = seat_id.to_string();
        let op = op.to_string();
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_SEAT_CONTROL,
                    serde_json::json!({ "chatId": "", "seatId": seat_id, "op": op }),
                )
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(_) => page.note = Some(format!("{op} sent — re-probing")),
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

    /// Verify + save the shared GitHub PAT — the repo picker and every
    /// PAT-less GitHub seat fall back to it.
    fn save_shared_pat(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let pat = SeatForm::text(&self.shared_pat, cx);
        if pat.is_empty() {
            return;
        }
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_GITHUB_SAVE,
                    serde_json::json!({ "pat": pat }),
                )
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(_) => {
                        page.note = Some("GitHub token verified and saved.".into());
                        page.shared_pat
                            .update(cx, |input, cx| input.set_text("", cx));
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

    /// Connect GitHub through the Cosmos GitHub App (device flow): show the
    /// one-time code, open GitHub, then poll until the user approves.
    fn connect_github(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.error = None;
        self.note = None;
        self.gh_task = Some(cx.spawn(async move |this, cx| {
            let start = engine
                .client()
                .call(methods::CLOUD_GITHUB_DEVICE_START, serde_json::json!({}))
                .await;
            let start = match start {
                Ok(v) => v,
                Err(err) => {
                    let _ = this.update(cx, |page, cx| {
                        page.error = Some(err.to_string());
                        cx.notify();
                    });
                    return;
                }
            };
            let text = |k: &str| {
                start
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let device_code = text("deviceCode");
            let user_code = text("userCode");
            let uri = text("verificationUri");
            let mut interval = start
                .get("interval")
                .and_then(|v| v.as_u64())
                .unwrap_or(5)
                .max(1);
            let expires = start
                .get("expiresIn")
                .and_then(|v| v.as_u64())
                .unwrap_or(900);
            let _ = this.update(cx, |page, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(user_code.clone()));
                cx.open_url(&uri);
                page.gh_device = Some(GitHubDevice {
                    user_code: user_code.clone(),
                    verification_uri: uri.clone(),
                });
                cx.notify();
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(expires);
            while std::time::Instant::now() < deadline {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(interval))
                    .await;
                let poll = engine
                    .client()
                    .call(
                        methods::CLOUD_GITHUB_DEVICE_POLL,
                        serde_json::json!({ "deviceCode": device_code }),
                    )
                    .await;
                let status = match &poll {
                    Ok(v) => v
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                    Err(_) => String::new(),
                };
                if status == "slow_down" {
                    interval += 5;
                    continue;
                }
                if status == "authorization_pending" {
                    continue;
                }
                let _ = this.update(cx, |page, cx| {
                    page.gh_device = None;
                    match poll {
                        Ok(v) => {
                            let login = v
                                .get("login")
                                .and_then(|l| l.as_str())
                                .unwrap_or("")
                                .to_string();
                            page.note = Some(format!("GitHub connected as {login}."));
                            page.gh_login = Some(login);
                        }
                        Err(err) => page.error = Some(err.to_string()),
                    }
                    page.load(cx);
                    page.state
                        .update(cx, |state, cx| state.refresh_cloud(None, cx));
                    cx.notify();
                });
                return;
            }
            let _ = this.update(cx, |page, cx| {
                page.gh_device = None;
                page.error = Some("The GitHub code expired — connect again".into());
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn disconnect_github(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.gh_device = None;
        self.gh_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::CLOUD_GITHUB_DISCONNECT, serde_json::json!({}))
                .await;
            let _ = this.update(cx, |page, cx| {
                match result {
                    Ok(_) => {
                        page.gh_login = None;
                        page.note = Some("GitHub disconnected.".into());
                        page.state.update(cx, |state, cx| {
                            state.cloud_github_login = None;
                            state.cloud_repos = crate::popover::Loadable::Idle;
                            state.set_cloud_repo_pick(None);
                            state.cloud_branch_pick = None;
                            state.persist_cloud_picks(cx);
                        });
                    }
                    Err(err) => page.error = Some(err.to_string()),
                }
                page.load(cx);
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            });
        }));
    }

    fn remove_seat(&mut self, seat_id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let _ = engine
                .client()
                .call(
                    methods::CLOUD_REMOVE_SEAT,
                    serde_json::json!({ "seatId": seat_id }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.load(cx);
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            })
            .ok();
        }));
    }

    fn save_boat(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).workspace_scope == Some(WorkspaceScope::Local) {
            self.error = Some("Sign in to add a Boat token.".into());
            cx.notify();
            return;
        }
        let token = SeatForm::text(&self.form.box_token, cx);
        if token.is_empty() {
            self.error = Some("Paste your Boat token first.".into());
            cx.notify();
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let existing_id = self
            .state
            .read(cx)
            .cloud_seats()
            .iter()
            .find(|seat| seat.config.kind == Some(CloudSeatKind::Box))
            .map(|seat| seat.config.id.clone());
        let seat_id =
            existing_id.unwrap_or_else(|| format!("seat-{}", uuid::Uuid::new_v4().simple()));
        let seat = serde_json::json!({
            "id": seat_id.clone(),
            "kind": CloudSeatKind::Box,
            "label": "Boat",
        });
        let secrets = serde_json::json!({ "boxToken": token });
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let saved = engine
                .client()
                .call(
                    methods::CLOUD_SAVE_SEAT,
                    serde_json::json!({ "seat": seat, "secrets": secrets }),
                )
                .await;
            if let Err(err) = saved {
                let _ = this.update(cx, |page, cx| {
                    page.error = Some(err.to_string());
                    cx.notify();
                });
                return;
            }
            let _ = this.update(cx, |page, cx| {
                page.state.update(cx, |state, cx| {
                    state.cloud_prefer_seat = Some(seat_id.clone());
                    state.persist_cloud_picks(cx);
                    state.refresh_cloud(None, cx);
                });
                page.form
                    .box_token
                    .update(cx, |input, cx| input.set_text("", cx));
                cx.notify();
            });
            let probe = engine
                .client()
                .call(
                    methods::CLOUD_PROBE_SEAT,
                    serde_json::json!({ "seatId": seat_id }),
                )
                .await;
            let _ = this.update(cx, |page, cx| {
                match probe {
                    Ok(_) => page.note = Some("Boat token saved.".into()),
                    Err(err) => page.error = Some(err.to_string()),
                }
                page.load(cx);
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            });
        }));
    }

    fn runner_editor(
        &self,
        seat: &SeatConfig,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let devices = self.state.read(cx).devices.clone();
        let mut card = widgets::section_card(theme).child(widgets::card_row(theme, true).child(
            widgets::row_title(theme, format!("Runner — {}", seat.label)),
        ));
        let mut assignment = div()
            .px(px(20.0))
            .pb(px(12.0))
            .flex()
            .flex_col()
            .gap(px(7.0))
            .child(
                div()
                    .text_size(crate::typography::ui_rems(11.5))
                    .text_color(theme.text_muted)
                    .child("Choose a signed-in device in this profile to run this seat's chats."),
            );
        for device in devices {
            let selected = self.runner_device_id.as_deref() == Some(device.id.as_str());
            let device_id = device.id.clone();
            let label =
                if self.state.read(cx).local_device_id.as_deref() == Some(device.id.as_str()) {
                    format!("{} · {} · This device", device.name, device.platform)
                } else {
                    format!("{} · {}", device.name, device.platform)
                };
            assignment = assignment.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(crate::typography::ui_rems(11.5))
                            .text_color(theme.text)
                            .child(SharedString::from(label)),
                    )
                    .child(
                        popover::btn_ghost(
                            theme,
                            if selected {
                                "Assigned"
                            } else {
                                "Use as runner"
                            },
                            format!("runner-device-{}", device.id),
                        )
                        .id(SharedString::from(format!("runner-device-{}", device.id)))
                        .when(selected || self.runner_task.is_some(), |el| el.opacity(0.5))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !selected && this.runner_task.is_none() {
                                this.assign_runner(Some(device_id.clone()), cx);
                            }
                        })),
                    ),
            );
        }
        if self.runner_device_id.is_some() {
            assignment = assignment.child(
                popover::btn_ghost(theme, "Remove runner", "runner-remove")
                    .id("runner-remove")
                    .when(self.runner_task.is_some(), |el| el.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.runner_task.is_none() {
                            this.assign_runner(None, cx);
                        }
                    })),
            );
        }
        let install_command = "curl -fsSL https://raw.githubusercontent.com/PupppyLoverr/cosmos-releases/main/install-runner.sh | sh";
        assignment = assignment.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .mt(px(4.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(theme.font_mono.clone())
                        .text_size(crate::typography::ui_rems(9.5))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(install_command)),
                )
                .child(
                    popover::btn_ghost(theme, "Copy", "runner-install-copy")
                        .id("runner-install-copy")
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                install_command.to_owned(),
                            ));
                        }),
                ),
        );
        card = card.child(assignment);

        if self.runner_device_id.is_some() {
            let mut detail = div()
                .px(px(20.0))
                .pb(px(14.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(widgets::row_title(theme, "Runner keys"));
            if let Some(info) = &self.runner_info {
                let credentials = info
                    .get("credentials")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if credentials.is_empty() {
                    detail = detail.child(
                        div()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child("No credentials sealed to this runner."),
                    );
                }
                for (index, credential) in credentials.iter().enumerate() {
                    let kind = credential
                        .get("kind")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("apiKey")
                        .to_string();
                    let provider = credential
                        .get("provider")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    let provider_label = provider
                        .as_deref()
                        .and_then(|value| {
                            serde_json::from_value::<ByokProvider>(serde_json::Value::String(
                                value.to_string(),
                            ))
                            .ok()
                        })
                        .map(ByokProvider::label)
                        .unwrap_or_else(|| {
                            if kind == "github" {
                                "GitHub"
                            } else {
                                provider.as_deref().unwrap_or("API key")
                            }
                        });
                    let sealed_at = credential
                        .get("sealedAt")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unknown date");
                    let expires_at = credential
                        .get("expiresAt")
                        .and_then(serde_json::Value::as_str);
                    let row_provider = provider.clone();
                    let row_kind = kind.clone();
                    let mut row = div().flex().items_center().gap(px(8.0)).child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(crate::typography::ui_rems(10.5))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(format!(
                                "{provider_label} · sealed {}{}",
                                sealed_at.chars().take(10).collect::<String>(),
                                expires_at
                                    .map(|at| format!(
                                        " · expires {}",
                                        at.chars().take(10).collect::<String>()
                                    ))
                                    .unwrap_or_default()
                            ))),
                    );
                    row = row.child(
                        popover::btn_ghost(
                            theme,
                            "Revoke",
                            format!("runner-credential-revoke-{index}"),
                        )
                        .id(SharedString::from(format!(
                            "runner-credential-revoke-{index}"
                        )))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.revoke_runner_credential(
                                row_provider.clone(),
                                row_kind.clone(),
                                cx,
                            );
                        })),
                    );
                    detail = detail.child(row);
                }

                let runner_logins = info
                    .get("runnerLogins")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                detail = detail.child(widgets::row_title(theme, "Use runner's own login"));
                for harness in [
                    HarnessId::ClaudeCode,
                    HarnessId::Codex,
                    HarnessId::Cursor,
                    HarnessId::Devin,
                    HarnessId::Grok,
                    HarnessId::Hermes,
                    HarnessId::Pi,
                    HarnessId::Opencode,
                    HarnessId::Antigravity,
                    HarnessId::Amp,
                    HarnessId::KimiCode,
                ] {
                    let encoded_harness = serde_json::to_value(harness).unwrap_or_default();
                    let enabled = runner_logins.iter().any(|login| login == &encoded_harness);
                    let label = match harness {
                        HarnessId::ClaudeCode => "Claude Code",
                        HarnessId::Codex => "Codex",
                        HarnessId::Cursor => "Cursor",
                        HarnessId::Devin => "Devin",
                        HarnessId::Grok => "Grok",
                        HarnessId::Hermes => "Hermes",
                        HarnessId::Pi => "Pi",
                        HarnessId::Opencode => "OpenCode",
                        HarnessId::Antigravity => "Antigravity",
                        HarnessId::Amp => "Amp",
                        HarnessId::KimiCode => "Kimi Code",
                        HarnessId::Mock => continue,
                    };
                    detail = detail.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                widgets::toggle_switch(
                                    theme,
                                    enabled,
                                    format!("runner-login-{harness:?}"),
                                )
                                .id(SharedString::from(format!("runner-login-{harness:?}")))
                                .when(self.runner_task.is_some(), |el| el.opacity(0.5))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if this.runner_task.is_none() {
                                            this.set_runner_login(harness, !enabled, cx);
                                        }
                                    },
                                )),
                            )
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_muted)
                                    .child(label),
                            ),
                    );
                }
            }
            detail = detail
                .child(widgets::row_title(theme, "Seal a credential"))
                .child(popover::dialog_field(
                    self.runner_provider.clone().into_any_element(),
                ))
                .child(popover::dialog_field(
                    self.runner_key.clone().into_any_element(),
                ))
                .child(popover::dialog_field(
                    self.runner_base_url.clone().into_any_element(),
                ))
                .child(
                    popover::btn_primary(theme, "Seal to runner")
                        .id("runner-seal")
                        .when(self.runner_task.is_some(), |el| el.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if this.runner_task.is_none() {
                                this.seal_runner_key(cx);
                            }
                        })),
                );
            if let Some(note) = &self.runner_note {
                detail = detail.child(
                    div()
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(note.clone())),
                );
            }
            if let Some(error) = &self.runner_error {
                detail = detail.child(widgets::error_strip(theme, error.clone()));
            }
            card = card.child(detail);
        }
        card.into_any_element()
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for CloudSeatsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for CloudSeatsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let scrollbar = popover::rail(self, "cloud-seats-scrollbar", &theme, cx);
        let seats = self.state.read(cx).cloud_seats().to_vec();
        let load_error = self.view.ready().and_then(|view| view.load_error.clone());
        let state = self.state.read(cx);
        let starting = state.cloud_trial_starting;
        let waiting = state.cloud_trial_signup_waiting;
        let failure = state.cloud_trial_failure.clone();
        let local_profile = state.workspace_scope == Some(WorkspaceScope::Local);
        let railway = seats
            .iter()
            .find(|seat| seat.config.kind == Some(CloudSeatKind::Railway));
        let boat = seats
            .iter()
            .find(|seat| seat.config.kind == Some(CloudSeatKind::Box));
        let legacy: Vec<_> = seats
            .iter()
            .filter(|seat| {
                matches!(
                    seat.config.kind,
                    Some(CloudSeatKind::Ssh | CloudSeatKind::GitHub)
                )
            })
            .collect();
        let card_state = railway_card(railway, starting, waiting, failure.as_deref());
        let (railway_copy, railway_action) = match &card_state {
            RailwayCard::Start => (
                "Free Linux computer for 60 minutes. No account needed.".to_string(),
                Some(("Start free trial", true, CloudSeatsEvent::StartTrial)),
            ),
            RailwayCard::Starting => ("Starting your free computer…".into(), None),
            RailwayCard::AwaitingSignup => (
                "Finish signing in on Railway — Cosmos continues on its own.".into(),
                Some(("Open Railway", false, CloudSeatsEvent::ContinueWithRailway)),
            ),
            RailwayCard::SignIn => (
                "Railway wants you to sign in to keep using the free trial.".into(),
                Some((
                    "Continue with Railway",
                    true,
                    CloudSeatsEvent::ContinueWithRailway,
                )),
            ),
            RailwayCard::Live {
                minutes_left,
                can_keep,
            } => (
                minutes_left
                    .map(|minutes| format!("Running · {minutes}m left"))
                    .unwrap_or_else(|| "Running".into()),
                if *can_keep {
                    Some((
                        "Continue with Railway",
                        false,
                        CloudSeatsEvent::ContinueWithRailway,
                    ))
                } else {
                    None
                },
            ),
            RailwayCard::Ended { can_keep: true } => (
                "Your trial hour is up. Continue on Railway within 24 hours to keep your work."
                    .into(),
                Some((
                    "Continue with Railway",
                    true,
                    CloudSeatsEvent::ContinueWithRailway,
                )),
            ),
            RailwayCard::Ended { can_keep: false } => (
                "Your trial hour is up.".into(),
                Some(("Start again", true, CloudSeatsEvent::StartTrial)),
            ),
            RailwayCard::Failed(reason) => (
                reason.clone(),
                Some(("Try again", true, CloudSeatsEvent::StartTrial)),
            ),
        };
        let mut railway_actions = div().flex_none().flex().items_center().gap(px(6.0));
        if let Some((label, primary, event)) = railway_action {
            railway_actions = railway_actions.child(if primary {
                popover::btn_primary(&theme, label)
                    .id("railway-action")
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(event)))
                    .into_any_element()
            } else {
                popover::btn_ghost(&theme, label, "railway-action")
                    .id("railway-action")
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(event)))
                    .into_any_element()
            });
        }
        let railway_section = widgets::section_card(&theme).child(
            widgets::card_row(&theme, true)
                .child(widgets::row_tile(&theme, icons::CLOUD))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(widgets::row_title(&theme, "Railway"))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(11.5))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(railway_copy)),
                        ),
                )
                .child(railway_actions),
        );

        let boat_copy = if let Some(seat) = boat.filter(|seat| seat.has_box_token) {
            if seat
                .config
                .last_probe
                .as_ref()
                .is_some_and(|probe| probe.sleeping)
            {
                "Connected — cloud chats run here, sleeping"
            } else {
                "Connected — cloud chats run here"
            }
        } else {
            "A cloud computer that stays on when you close Cosmos."
        };
        let mut boat_actions = div().flex_none().flex().items_center().gap(px(6.0));
        let mut boat_section = widgets::section_card(&theme).child(
            widgets::card_row(&theme, true)
                .child(widgets::row_tile(&theme, icons::CLOUD))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(widgets::row_title(&theme, "Boat"))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(11.5))
                                .text_color(theme.text_muted)
                                .child(boat_copy),
                        ),
                )
                .child({
                    if let Some(seat) = boat.filter(|seat| seat.has_box_token) {
                        let seat_id = seat.config.id.clone();
                        let sleeping = seat
                            .config
                            .last_probe
                            .as_ref()
                            .is_some_and(|probe| probe.sleeping);
                        if seat.config.box_id.is_some() {
                            let control_seat_id = seat_id.clone();
                            boat_actions = boat_actions.child(
                                popover::btn_ghost(
                                    &theme,
                                    if sleeping { "Wake" } else { "Sleep" },
                                    "boat-control",
                                )
                                .id("boat-control")
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.seat_control(
                                            &control_seat_id,
                                            if sleeping { "wake" } else { "sleep" },
                                            cx,
                                        )
                                    },
                                )),
                            );
                        }
                        let runner_seat = seat.config.clone();
                        boat_actions = boat_actions
                            .child(
                                popover::btn_ghost(&theme, "Runner", "boat-runner")
                                    .id("boat-runner")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_runner(&runner_seat, cx)
                                    })),
                            )
                            .child(
                                popover::btn_ghost(&theme, "Remove", "boat-remove")
                                    .id("boat-remove")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_seat(seat_id.clone(), cx)
                                    })),
                            );
                        boat_actions.into_any_element()
                    } else {
                        div()
                            .flex_none()
                            .child(
                                popover::btn_primary(&theme, "Add")
                                    .id("boat-add")
                                    .on_click(cx.listener(|this, _, _, cx| this.save_boat(cx))),
                            )
                            .into_any_element()
                    }
                }),
        );
        if boat.is_none_or(|seat| !seat.has_box_token) {
            boat_section = boat_section.child(div().px(px(20.0)).pb(px(12.0)).child(
                popover::dialog_field(self.form.box_token.clone().into_any_element()),
            ));
        }

        let local_sign_in_section = widgets::section_card(&theme).child(
            widgets::card_row(&theme, true)
                .child(widgets::row_tile(&theme, icons::CLOUD))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(widgets::row_title(&theme, "Sign in to use a cloud computer"))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(11.5))
                                .text_color(theme.text_muted)
                                .child("Cloud computers need a Cosmos account. Sign in, then start the free Railway trial or add a Boat token."),
                        ),
                )
                .child(
                    popover::btn_primary(&theme, "Log in")
                        .id("cloud-sign-in")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(CloudSeatsEvent::SignIn))),
                ),
        );

        let mut column = widgets::page_column()
            .child(widgets::page_header(&theme, "Cloud computer", None))
            .child(
                widgets::page_subtitle(
                    &theme,
                    "Where cloud agents run. Use the free Railway trial, or add a Boat token for a computer that stays on.",
                )
                .max_w(px(560.0))
                .line_height(px(20.0)),
            );
        if local_profile {
            column = column.child(local_sign_in_section);
        } else {
            column = column.child(railway_section).child(boat_section);
        }

        if let Some(seat) = self.runner_seat_id.as_deref().and_then(|seat_id| {
            seats
                .iter()
                .find(|row| row.config.id == seat_id)
                .map(|row| row.config.clone())
        }) {
            column = column.child(self.runner_editor(&seat, &theme, cx));
        }

        if !legacy.is_empty() {
            let mut other = widgets::section_card(&theme);
            for (index, seat) in legacy.iter().enumerate() {
                let seat_id = seat.config.id.clone();
                let kind = seat
                    .config
                    .kind
                    .map(CloudSeatKind::label)
                    .unwrap_or("Unknown");
                let mut actions = div().flex_none().flex().items_center().gap(px(6.0));
                if seat.config.kind == Some(CloudSeatKind::Ssh) {
                    let runner_seat = seat.config.clone();
                    actions = actions.child(
                        popover::btn_ghost(&theme, "Runner", format!("legacy-runner-{seat_id}"))
                            .id(SharedString::from(format!("legacy-runner-{seat_id}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_runner(&runner_seat, cx)
                            })),
                    );
                }
                actions =
                    actions.child(
                        popover::btn_ghost(&theme, "Remove", format!("legacy-remove-{seat_id}"))
                            .id(SharedString::from(format!("legacy-remove-{seat_id}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_seat(seat_id.clone(), cx)
                            })),
                    );
                other = other.child(
                    widgets::card_row(&theme, index == 0)
                        .child(widgets::row_tile(&theme, icons::CLOUD))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(widgets::row_title(&theme, seat.config.label.clone()))
                                .child(
                                    div()
                                        .text_size(crate::typography::ui_rems(11.5))
                                        .text_color(theme.text_muted)
                                        .child(kind),
                                ),
                        )
                        .child(actions),
                );
            }
            column = column.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(widgets::row_title(&theme, "Other seats"))
                    .child(other),
            );
        }

        if let Some(error) = load_error {
            column = column.child(widgets::error_strip(&theme, error));
        }
        if let Some(note) = &self.note {
            column = column.child(
                div()
                    .text_size(crate::typography::ui_rems(11.5))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(note.clone())),
            );
        }
        if let Some(error) = &self.error {
            column = column.child(widgets::error_strip(&theme, error.clone()));
        }

        let has_pat = self
            .view
            .ready()
            .map(|view| view.has_github_pat)
            .unwrap_or(false);
        let waiting = self.gh_device.as_ref();
        let status: SharedString = if let Some(device) = waiting {
            format!(
                "Enter {} on GitHub to finish — copied to your clipboard.",
                device.user_code
            )
            .into()
        } else if has_pat {
            if self.gh_login.is_none() {
                let state = self.state.read(cx);
                if state.cloud_github_login.is_none()
                    && matches!(state.cloud_repos, crate::popover::Loadable::Idle)
                {
                    self.state
                        .update(cx, |state, cx| state.refresh_cloud_repos(cx));
                }
            }
            let login = self
                .gh_login
                .clone()
                .or_else(|| self.state.read(cx).cloud_github_login.clone());
            match login.as_deref().filter(|login| !login.is_empty()) {
                Some(login) => {
                    format!("Connected as {login} — repos, pull requests and GitHub seats use it.")
                        .into()
                }
                None => "Connected — repos, pull requests and GitHub seats use it.".into(),
            }
        } else {
            "Connect GitHub to pick repos, run cloud agents on them and open pull requests.".into()
        };
        let mut pat_card = widgets::section_card(&theme);
        let mut actions = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0));
        if let Some(device) = waiting {
            let uri = device.verification_uri.clone();
            actions = actions
                .child(widgets::badge(&theme, device.user_code.clone()))
                .child(
                    popover::btn_ghost(&theme, "Open GitHub", "gh-open")
                        .id("gh-open")
                        .on_click(move |_, _, cx| cx.open_url(&uri)),
                );
        } else if has_pat {
            actions = actions.child(
                popover::btn_ghost(&theme, "Disconnect", "gh-disconnect")
                    .id("gh-disconnect")
                    .on_click(cx.listener(|this, _, _, cx| this.disconnect_github(cx))),
            );
        } else {
            actions = actions.child(
                popover::btn_primary(&theme, "Connect GitHub")
                    .id("gh-connect")
                    .on_click(cx.listener(|this, _, _, cx| this.connect_github(cx))),
            );
        }
        pat_card = pat_card.child(
            widgets::card_row(&theme, true)
                .child(widgets::row_tile(&theme, icons::GIT_BRANCH))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(widgets::row_title(&theme, "GitHub"))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(11.5))
                                .text_color(theme.text_muted)
                                .child(status),
                        ),
                )
                .child(actions),
        );
        pat_card = pat_card.child(
            div()
                .px(px(20.0))
                .pb(px(12.0))
                .flex()
                .flex_row()
                .gap(px(12.0))
                .text_size(crate::typography::ui_rems(11.5))
                .child(
                    div()
                        .id("gh-install")
                        .cursor_pointer()
                        .text_color(theme.text_muted)
                        .hover(|style| style.text_color(theme.text))
                        .child("Choose repositories")
                        .on_click(|_, _, cx| {
                            cx.open_url(zeron_engine::cloud::COSMOS_GITHUB_INSTALL_URL)
                        }),
                )
                .child(
                    div()
                        .id("gh-pat-toggle")
                        .cursor_pointer()
                        .text_color(theme.text_muted)
                        .hover(|style| style.text_color(theme.text))
                        .child(if self.show_pat {
                            "Hide token option"
                        } else {
                            "Use a token instead"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_pat = !this.show_pat;
                            cx.notify();
                        })),
                ),
        );
        if self.show_pat {
            pat_card = pat_card.child(
                div()
                    .px(px(20.0))
                    .pb(px(12.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().flex_1().child(popover::dialog_field(
                        self.shared_pat.clone().into_any_element(),
                    )))
                    .child(
                        popover::btn_primary(&theme, "Verify & save")
                            .id("pat-save")
                            .on_click(cx.listener(|this, _, _, cx| this.save_shared_pat(cx))),
                    ),
            );
        }
        column = column.child(pat_card);

        div()
            .id("seats-page-root")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div()
                    .id("cloud-seats-page")
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
    use chrono::{Duration, Utc};
    use zeron_proto::SeatProbe;

    fn railway() -> SeatRow {
        SeatRow {
            config: SeatConfig {
                kind: Some(CloudSeatKind::Railway),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn railway_card_prioritizes_waiting_and_starting() {
        assert_eq!(
            railway_card(None, true, true, None),
            RailwayCard::AwaitingSignup
        );
        assert_eq!(railway_card(None, true, false, None), RailwayCard::Starting);
        assert_eq!(railway_card(None, false, false, None), RailwayCard::Start);
    }

    #[test]
    fn railway_card_covers_signup_live_and_ended_trials() {
        let mut seat = railway();
        seat.config.signup_url = Some("https://railway.com/signup".into());
        seat.config.expires_at = Some(Utc::now() - Duration::minutes(1));
        assert_eq!(
            railway_card(Some(&seat), false, false, None),
            RailwayCard::SignIn
        );

        seat.config.signup_url = None;
        seat.config.expires_at = Some(Utc::now() + Duration::minutes(31));
        assert_eq!(
            railway_card(Some(&seat), false, false, None),
            RailwayCard::Live {
                minutes_left: Some(30),
                can_keep: false,
            }
        );
        seat.config.expires_at = None;
        seat.config.last_probe = Some(SeatProbe {
            online: true,
            ..Default::default()
        });
        assert_eq!(
            railway_card(Some(&seat), false, false, None),
            RailwayCard::Live {
                minutes_left: None,
                can_keep: false,
            }
        );

        seat.config.expires_at = Some(Utc::now() - Duration::minutes(1));
        seat.config.claim_url = Some("https://railway.com/claim".into());
        assert_eq!(
            railway_card(Some(&seat), false, false, None),
            RailwayCard::Ended { can_keep: true }
        );
        seat.config.claim_url = None;
        assert_eq!(
            railway_card(Some(&seat), false, false, None),
            RailwayCard::Ended { can_keep: false }
        );
    }

    #[test]
    fn railway_card_uses_probe_detail_for_failures() {
        let mut seat = railway();
        seat.config.last_probe = Some(SeatProbe {
            online: false,
            detail: Some("Trial unavailable".into()),
            ..Default::default()
        });
        assert_eq!(
            railway_card(Some(&seat), false, false, None),
            RailwayCard::Failed("Trial unavailable".into())
        );
        assert_eq!(
            railway_card(Some(&seat), false, false, Some("Try again")),
            RailwayCard::Failed("Try again".into())
        );
        assert_eq!(
            railway_card(None, false, false, Some("Try again")),
            RailwayCard::Start
        );
        seat.config.last_probe = Some(SeatProbe {
            online: false,
            ..Default::default()
        });
        assert_eq!(
            railway_card(Some(&seat), false, false, None),
            RailwayCard::Failed("Railway didn't answer".into())
        );
    }
}
