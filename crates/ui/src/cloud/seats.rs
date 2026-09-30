//! Settings → Cloud computer: the seat list (Box | BYO VPS | GitHub
//! workspace), per-seat health, and the add-seat card. Private keys never
//! touch this page — SSH rides the user's ssh-agent, tokens go straight into
//! the engine's `cloud-keys.json`.

use gpui::{
    AnyElement, Context, Entity, IntoElement, Render, SharedString, Task, Window, div, prelude::*,
    px,
};
use zeron_proto::CloudSeatKind;
use zeron_rpc::methods;

use zeron_engine::cloud::{RepoHit, SeatConfig};

use crate::cloud::{CloudView, SeatRow, status_dot};
use crate::composer::ComposerInput;
use crate::icons::{self, icon};
use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// The add-seat form's editable fields — one `ComposerInput` per row; which
/// apply is decided by `draft_kind`.
struct SeatForm {
    label: Entity<ComposerInput>,
    ssh_alias: Entity<ComposerInput>,
    container: Entity<ComposerInput>,
    workspace: Entity<ComposerInput>,
    repo: Entity<ComposerInput>,
    branch: Entity<ComposerInput>,
    box_endpoint: Entity<ComposerInput>,
    cua_endpoint: Entity<ComposerInput>,
    harness_command: Entity<ComposerInput>,
    box_token: Entity<ComposerInput>,
    github_pat: Entity<ComposerInput>,
    cua_token: Entity<ComposerInput>,
    detached: bool,
}

impl SeatForm {
    fn new(cx: &mut Context<CloudSeatsPage>) -> Self {
        let mut field =
            |placeholder: &'static str| cx.new(|cx| ComposerInput::new(placeholder, cx));
        Self {
            label: field("Seat name"),
            ssh_alias: field("SSH config alias (from ~/.ssh/config)"),
            container: field("Container name — default cosmos-cloud-<id>"),
            workspace: field("Workspace path on the seat"),
            repo: field("GitHub repo — owner/name"),
            branch: field("Default branch"),
            box_endpoint: field("Box API endpoint (https://…)"),
            cua_endpoint: field("CUA endpoint URL (remote cua-driver, optional)"),
            harness_command: field("Harness command (e.g. claude -p)"),
            box_token: field("Box token"),
            github_pat: field("GitHub PAT (repo + workflow scope)"),
            cua_token: field("CUA token (optional)"),
            detached: false,
        }
    }

    fn text(input: &Entity<ComposerInput>, cx: &gpui::App) -> String {
        input.read(cx).text().trim().to_string()
    }
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
    /// `Some(kind)` while the add-seat card is open.
    draft_kind: Option<CloudSeatKind>,
    editing_seat: Option<SeatConfig>,
    form: SeatForm,
    /// The shared GitHub PAT input — the repo picker's token when a seat has
    /// none of its own. Saved to `cloud-keys.json` under the reserved key.
    shared_pat: Entity<ComposerInput>,
    /// The GitHub draft card's repo picker: debounced search hits rendered
    /// under the repo field; picking one writes `owner/name` into it.
    repo_hits: Vec<RepoHit>,
    repo_orgs: Vec<String>,
    repo_truncated: bool,
    repo_orgs_error: Option<String>,
    /// Generation counter kills stale debounced searches (300ms).
    repo_search_gen: u64,
    /// `pick_repo` writes the field programmatically — suppress the search
    /// that write would otherwise trigger.
    suppress_repo_edit: bool,
    /// The Cosmos GitHub App device flow while it waits on the user.
    gh_device: Option<GitHubDevice>,
    gh_login: Option<String>,
    gh_task: Option<Task<()>>,
    show_pat: bool,
    probing: Option<String>,
    error: Option<String>,
    note: Option<String>,
    load_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
    _repo_sub: gpui::Subscription,
}

impl CloudSeatsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let form = SeatForm::new(cx);
        // Repo picker: typing in the repo field debounces a search.
        let repo_input = form.repo.clone();
        let repo_sub = cx.subscribe(&repo_input, |this, _, event, cx| {
            if matches!(event, crate::composer::ComposerInputEvent::Edited)
                && this.draft_kind == Some(CloudSeatKind::GitHub)
            {
                if this.suppress_repo_edit {
                    this.suppress_repo_edit = false;
                } else {
                    this.search_repos(cx);
                }
            }
        });
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            view: Loadable::Idle,
            load_generation: 0,
            draft_kind: None,
            editing_seat: None,
            form,
            shared_pat: cx
                .new(|cx| ComposerInput::new("GitHub PAT — shared (repo + workflow scope)", cx)),
            repo_hits: Vec::new(),
            repo_orgs: Vec::new(),
            repo_truncated: false,
            repo_orgs_error: None,
            repo_search_gen: 0,
            suppress_repo_edit: false,
            gh_device: None,
            gh_login: None,
            gh_task: None,
            show_pat: false,
            probing: None,
            error: None,
            note: None,
            load_task: None,
            action_task: None,
            _repo_sub: repo_sub,
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
                cx.notify();
            })
            .ok();
        }));
    }

    fn edit_harness(&mut self, seat: SeatConfig, cx: &mut Context<Self>) {
        let command = seat.harness_command.clone().unwrap_or_default();
        self.form
            .harness_command
            .update(cx, |input, cx| input.set_text(command, cx));
        self.editing_seat = Some(seat);
        self.draft_kind = None;
        cx.notify();
    }

    fn save_harness(&mut self, cx: &mut Context<Self>) {
        let Some(mut seat) = self.editing_seat.clone() else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let command = SeatForm::text(&self.form.harness_command, cx);
        seat.harness_command = (!command.is_empty()).then_some(command);
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_SAVE_SEAT,
                    serde_json::json!({ "seat": seat, "secrets": {} }),
                )
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(_) => {
                        page.editing_seat = None;
                        page.note = Some("Harness command saved.".into());
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

    fn probe(&mut self, seat_id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.probing = Some(seat_id.clone());
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_PROBE_SEAT,
                    serde_json::json!({ "seatId": seat_id }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.probing = None;
                match result {
                    Ok(probe) => {
                        let online = probe
                            .get("online")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let detail = probe
                            .get("detail")
                            .and_then(|d| d.as_str())
                            .unwrap_or(if online { "online" } else { "offline" });
                        page.note = Some(format!("{seat_id} — {detail}"));
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

    /// Seat-level lifecycle op (`sleep` archives a Boat box — billing off;
    /// `wake` resumes it). No chat needed — `chatId` rides empty.
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

    /// Debounced repo search — 300ms after the last keystroke wins; stale
    /// replies drop on the generation counter. Empty query = recents +
    /// orgs, per the spec's picker shape.
    fn search_repos(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.repo_search_gen += 1;
        let generation = self.repo_search_gen;
        let query = SeatForm::text(&self.form.repo, cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            let stale = this
                .update(cx, |page, _| page.repo_search_gen != generation)
                .unwrap_or(true);
            if stale {
                return;
            }
            let result = engine
                .client()
                .call(
                    methods::CLOUD_GITHUB_REPOS,
                    serde_json::json!({ "query": query }),
                )
                .await;
            this.update(cx, |page, cx| {
                if page.repo_search_gen != generation {
                    return;
                }
                match result {
                    Ok(v) => {
                        page.repo_hits =
                            serde_json::from_value(v.get("repos").cloned().unwrap_or_default())
                                .unwrap_or_default();
                        page.repo_orgs =
                            serde_json::from_value(v.get("orgs").cloned().unwrap_or_default())
                                .unwrap_or_default();
                        page.repo_truncated = v
                            .get("truncated")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        page.repo_orgs_error = v
                            .get("orgsError")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                    }
                    Err(_) => {
                        page.repo_hits.clear();
                        page.repo_orgs.clear();
                        page.repo_truncated = false;
                        page.repo_orgs_error = None;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Pick a repo row — the field takes `owner/name`, hits close.
    fn pick_repo(&mut self, full_name: String, cx: &mut Context<Self>) {
        let default_branch = self
            .repo_hits
            .iter()
            .find(|r| r.full_name == full_name)
            .map(|r| r.default_branch.clone());
        self.repo_hits.clear();
        self.repo_search_gen += 1;
        self.suppress_repo_edit = true;
        self.form
            .repo
            .update(cx, |input, cx| input.set_text(full_name, cx));
        if let Some(branch) = default_branch.filter(|b| !b.is_empty()) {
            self.form
                .branch
                .update(cx, |input, cx| input.set_text(branch, cx));
        }
        cx.notify();
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
                        page.state.update(cx, |state, _| {
                            state.cloud_github_login = None;
                            state.cloud_repos = crate::popover::Loadable::Idle;
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

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        let Some(kind) = self.draft_kind else { return };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let text = |input: &Entity<ComposerInput>| SeatForm::text(input, cx);
        let label = text(&self.form.label);
        let non_empty = |s: String| (!s.is_empty()).then_some(s);
        let id = format!("seat-{}", uuid::Uuid::new_v4().simple());
        let seat = serde_json::json!({
            "id": id,
            "kind": kind,
            "label": if label.is_empty() { kind.label() } else { label.as_str() },
            "sshAlias": non_empty(text(&self.form.ssh_alias)),
            "container": non_empty(text(&self.form.container)),
            "workspacePath": non_empty(text(&self.form.workspace)),
            "repo": non_empty(text(&self.form.repo)),
            "branch": non_empty(text(&self.form.branch)),
            "boxEndpoint": non_empty(text(&self.form.box_endpoint)),
            "cuaEndpoint": non_empty(text(&self.form.cua_endpoint)),
            "harnessCommand": non_empty(text(&self.form.harness_command)),
            "detached": self.form.detached,
        });
        let secrets = serde_json::json!({
            "boxToken": non_empty(text(&self.form.box_token)),
            "githubPat": non_empty(text(&self.form.github_pat)),
            "cuaToken": non_empty(text(&self.form.cua_token)),
        });
        // A persistent computer the user just configured becomes the seat
        // new cloud chats use (Boat token / VPS alias beat the trial).
        let prefer = matches!(kind, CloudSeatKind::Box | CloudSeatKind::Ssh)
            .then(|| seat.get("id").and_then(|v| v.as_str()).map(str::to_string))
            .flatten();
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::CLOUD_SAVE_SEAT,
                    serde_json::json!({ "seat": seat, "secrets": secrets }),
                )
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(_) => {
                        if let Some(prefer) = prefer.clone() {
                            page.state
                                .update(cx, |state, _| state.cloud_prefer_seat = Some(prefer));
                        }
                        page.draft_kind = None;
                        page.form = SeatForm::new(cx);
                        page.note = Some(
                            "Seat saved — probe it to confirm reachability before starting a run."
                                .into(),
                        );
                    }
                    Err(err) => page.error = Some(err.to_string()),
                }
                page.load(cx);
                // The shared view feeds the composer's seat picker — force it.
                page.state
                    .update(cx, |state, cx| state.refresh_cloud(None, cx));
                cx.notify();
            })
            .ok();
        }));
    }

    fn seat_row(
        &self,
        seat: &SeatRow,
        first: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let probe = seat.config.last_probe.clone();
        let capabilities = &seat.capabilities;
        let seat_id = seat.config.id.clone();
        let edit_harness_id = format!("seat-edit-harness-{seat_id}");
        let kind_label = seat
            .config
            .kind
            .map(|k| k.label())
            .unwrap_or("Unknown")
            .to_string();
        let detail: String = match seat.config.kind {
            Some(CloudSeatKind::Ssh) => seat
                .config
                .ssh_alias
                .clone()
                .unwrap_or_else(|| "no ssh alias".into()),
            Some(CloudSeatKind::Railway) => {
                // Status + time left per spec; the claim URL surfaces on the
                // seat's card actions when one was minted. A saved snapshot
                // (pulled ~10 min before expiry) is what a Boat seat restores.
                let backup = if seat.config.backup_at.is_some() {
                    " · workspace backed up locally"
                } else {
                    ""
                };
                match seat.config.expires_at {
                    Some(at) => {
                        let left = at.signed_duration_since(chrono::Utc::now());
                        if left.num_seconds() <= 0 {
                            format!("trial ended{backup}")
                        } else {
                            format!("trial · {}m left{backup}", left.num_minutes())
                        }
                    }
                    None => "railway.new trial — probe to provision".into(),
                }
            }
            Some(CloudSeatKind::Box) => seat
                .config
                .box_id
                .clone()
                .or_else(|| seat.config.box_endpoint.clone())
                .unwrap_or_else(|| "token-only seat".into()),
            Some(CloudSeatKind::GitHub) => {
                seat.config.repo.clone().unwrap_or_else(|| "no repo".into())
            }
            None => String::new(),
        };
        let mut caps: Vec<&'static str> = Vec::new();
        if capabilities.exec {
            caps.push("exec");
        }
        if capabilities.shell {
            caps.push("shell");
        }
        if capabilities.files {
            caps.push("files");
        }
        if capabilities.git {
            caps.push("git");
        }
        if capabilities.pull_requests {
            caps.push("prs");
        }
        if capabilities.computer {
            caps.push("computer");
        }
        if capabilities.keeps_running {
            caps.push("24/7");
        }
        let probing = self.probing.as_deref() == Some(seat_id.as_str());
        widgets::card_row(theme, first)
            .child(widgets::row_tile(theme, icons::CLOUD))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(widgets::row_title(theme, seat.config.label.clone()))
                            .child(widgets::badge(theme, kind_label))
                            .child(status_dot(theme, probe.as_ref())),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.5))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(if detail.is_empty() {
                                "Not configured".to_string()
                            } else {
                                detail
                            })),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(10.5))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(if caps.is_empty() {
                                "no capabilities yet — add credentials and probe".to_string()
                            } else {
                                caps.join(" · ")
                            })),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .gap(px(6.0))
                    // Railway seats carry a claim link — opens in the user's
                    // browser, never embedded.
                    .when_some(seat.config.claim_url.clone(), |el, url| {
                        el.child(
                            popover::btn_ghost(theme, "Claim", format!("seat-claim-{seat_id}"))
                                .id(SharedString::from(format!("seat-claim-{seat_id}")))
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    })
                    // Boat seats with a live box expose Sleep (billing off).
                    .when(
                        seat.config.kind == Some(CloudSeatKind::Box)
                            && seat.config.box_id.is_some()
                            && !probe.as_ref().is_some_and(|p| p.sleeping),
                        |el| {
                            let seat_id = seat_id.clone();
                            el.child(
                                popover::btn_ghost(theme, "Sleep", format!("seat-sleep-{seat_id}"))
                                    .id(SharedString::from(format!("seat-sleep-{seat_id}")))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.seat_control(&seat_id, "sleep", cx)
                                    })),
                            )
                        },
                    )
                    .when(probe.as_ref().is_some_and(|p| p.sleeping), |el| {
                        let seat_id = seat_id.clone();
                        el.child(
                            popover::btn_ghost(theme, "Wake", format!("seat-wake-{seat_id}"))
                                .id(SharedString::from(format!("seat-wake-{seat_id}")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.seat_control(&seat_id, "wake", cx)
                                })),
                        )
                    })
                    .child(
                        popover::btn_ghost(theme, "Probe", format!("seat-probe-{seat_id}"))
                            .id(SharedString::from(format!("seat-probe-{seat_id}")))
                            .when(probing, |el| el.opacity(0.5))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.probe(seat_id.clone(), cx)),
                            ),
                    )
                    .child({
                        let seat = seat.config.clone();
                        popover::btn_ghost(theme, "Edit harness", edit_harness_id.clone())
                            .id(SharedString::from(edit_harness_id))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit_harness(seat.clone(), cx)
                            }))
                    })
                    .child({
                        let seat_id = seat.config.id.clone();
                        popover::btn_ghost(theme, "Remove", format!("seat-remove-{seat_id}"))
                            .id(SharedString::from(format!("seat-remove-{seat_id}")))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_seat(seat_id.clone(), cx)
                            }))
                    }),
            )
            .into_any_element()
    }

    fn draft_card(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(kind) = self.draft_kind else {
            return div().into_any_element();
        };
        let mut card = widgets::section_card(theme);
        card = card.child(widgets::card_row(theme, true).child(widgets::row_title(
            theme,
            format!("New {} seat", kind.label()),
        )));
        card = card.child(
            div()
                .px(px(20.0))
                .pb(px(12.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(popover::dialog_field(
                    self.form.label.clone().into_any_element(),
                ))
                // Railway needs nothing but the user's ssh key — the first
                // probe provisions the trial VM and stamps the countdown.
                .when(kind == CloudSeatKind::Railway, |el| {
                    el.child(
                        div()
                            .text_size(crate::typography::ui_rems(11.5))
                            .text_color(theme.text_muted)
                            .child(
                                "Provisions on first probe via `ssh railway.new` — your ssh key \
                                 is the trial identity. 60 minutes unless you claim it.",
                            ),
                    )
                })
                .when(kind == CloudSeatKind::Ssh, |el| {
                    el.child(popover::dialog_field(
                        self.form.ssh_alias.clone().into_any_element(),
                    ))
                    .child(popover::dialog_field(
                        self.form.container.clone().into_any_element(),
                    ))
                    .child(popover::dialog_field(
                        self.form.workspace.clone().into_any_element(),
                    ))
                })
                .when(kind == CloudSeatKind::Box, |el| {
                    el.child(popover::dialog_field(
                        self.form.box_token.clone().into_any_element(),
                    ))
                    .child(popover::dialog_field(
                        self.form.box_endpoint.clone().into_any_element(),
                    ))
                })
                .when(kind == CloudSeatKind::GitHub, |el| {
                    let mut col = el
                        .child(popover::dialog_field(
                            self.form.repo.clone().into_any_element(),
                        ))
                        .child(popover::dialog_field(
                            self.form.branch.clone().into_any_element(),
                        ))
                        .when(self.show_pat, |el| {
                            el.child(popover::dialog_field(
                                self.form.github_pat.clone().into_any_element(),
                            ))
                        });
                    // Repo picker — debounced hits (or recents on open) under
                    // the field; a click writes `owner/name` + branch.
                    if !self.repo_hits.is_empty() {
                        col = col.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .rounded(px(8.0))
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.surface)
                                .p(px(4.0))
                                .max_h(px(200.0))
                                .overflow_y_hidden()
                                .children(self.repo_hits.iter().take(8).enumerate().map(
                                    |(ix, hit)| {
                                        let name = hit.full_name.clone();
                                        let desc = hit.description.clone().unwrap_or_default();
                                        let row_fade = SharedString::from(format!("repo-hit-{ix}"));
                                        div()
                                            .id(SharedString::from(format!("repo-hit-{ix}")))
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap(px(8.0))
                                            .px(px(8.0))
                                            .py(px(5.0))
                                            .rounded(px(6.0))
                                            .cursor_pointer()
                                            .bg(crate::motion::hover_blend(
                                                &row_fade,
                                                gpui::transparent_black(),
                                                theme.element_hover,
                                            ))
                                            .on_hover(crate::motion::hover_listener(row_fade))
                                            .child(
                                                icon(icons::GIT_BRANCH)
                                                    .size(px(12.0))
                                                    .flex_none()
                                                    .text_color(theme.text_muted),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .flex()
                                                    .flex_col()
                                                    .child(
                                                        div()
                                                            .truncate()
                                                            .text_size(crate::typography::ui_rems(
                                                                11.5,
                                                            ))
                                                            .text_color(theme.text)
                                                            .child(SharedString::from(
                                                                name.clone(),
                                                            )),
                                                    )
                                                    .when(!desc.is_empty(), |el| {
                                                        el.child(
                                                            div()
                                                                .truncate()
                                                                .text_size(
                                                                    crate::typography::ui_rems(9.5),
                                                                )
                                                                .text_color(theme.text_faint)
                                                                .child(SharedString::from(desc)),
                                                        )
                                                    }),
                                            )
                                            .when(hit.private, |el| {
                                                el.child(widgets::badge(theme, "private"))
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.pick_repo(name.clone(), cx)
                                            }))
                                    },
                                )),
                        );
                    } else if !self.repo_orgs.is_empty() {
                        col = col.child(
                            div()
                                .text_size(crate::typography::ui_rems(10.0))
                                .text_color(theme.text_faint)
                                .child(SharedString::from(format!(
                                    "orgs: {}",
                                    self.repo_orgs.join(" · ")
                                ))),
                        );
                    }
                    if self.repo_truncated {
                        col = col.child(
                            div()
                                .text_size(crate::typography::ui_rems(10.0))
                                .text_color(theme.text_muted)
                                .child("Showing first 500 — refine search"),
                        );
                    }
                    if let Some(error) = &self.repo_orgs_error {
                        col = col.child(
                            div()
                                .text_size(crate::typography::ui_rems(10.0))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(format!("Organizations: {error}"))),
                        );
                    }
                    col
                })
                .child(popover::dialog_field(
                    self.form.harness_command.clone().into_any_element(),
                ))
                .child(popover::dialog_field(
                    self.form.cua_endpoint.clone().into_any_element(),
                ))
                .child(popover::dialog_field(
                    self.form.cua_token.clone().into_any_element(),
                ))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            widgets::toggle_switch(theme, self.form.detached, "seat-detached")
                                .id("seat-detached-toggle")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.form.detached = !this.form.detached;
                                    cx.notify();
                                })),
                        )
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(11.5))
                                .text_color(theme.text_muted)
                                .child(
                                    "Run detached — the harness keeps going when this app closes",
                                ),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(8.0))
                        .child(
                            popover::btn_primary(theme, "Save seat")
                                .id("seat-save")
                                .on_click(cx.listener(|this, _, _, cx| this.save_draft(cx))),
                        )
                        .child(
                            popover::btn_ghost(theme, "Cancel", "seat-cancel")
                                .id("seat-cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.draft_kind = None;
                                    cx.notify();
                                })),
                        ),
                ),
        );
        card.into_any_element()
    }

    fn harness_editor(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(seat) = &self.editing_seat else {
            return div().into_any_element();
        };
        widgets::section_card(theme)
            .child(widgets::card_row(theme, true).child(widgets::row_title(
                theme,
                format!("Edit harness — {}", seat.label),
            )))
            .child(
                div()
                    .px(px(20.0))
                    .pb(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(popover::dialog_field(
                        self.form.harness_command.clone().into_any_element(),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(8.0))
                            .child(
                                popover::btn_primary(theme, "Save harness")
                                    .id("seat-harness-save")
                                    .on_click(cx.listener(|this, _, _, cx| this.save_harness(cx))),
                            )
                            .child(
                                popover::btn_ghost(theme, "Cancel", "seat-harness-cancel")
                                    .id("seat-harness-cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.editing_seat = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .into_any_element()
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
        let seats = self
            .view
            .ready()
            .map(|v| v.seats.clone())
            .unwrap_or_default();
        let load_error = self.view.ready().and_then(|view| view.load_error.clone());
        let mut card = widgets::section_card(&theme);
        if seats.is_empty() {
            card = card.child(
                widgets::card_row(&theme, true)
                    .child(widgets::row_tile(&theme, icons::CLOUD))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(widgets::row_title(&theme, "No seats yet"))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(11.5))
                                    .text_color(theme.text_muted)
                                    .child(
                                        "Add a Box, a BYO VPS over SSH, or a GitHub workspace \
                                         to run agents in the cloud.",
                                    ),
                            ),
                    ),
            );
        }
        for (ix, seat) in seats.iter().enumerate() {
            card = card.child(self.seat_row(seat, ix == 0, &theme, cx));
        }
        let mut column = widgets::page_column()
            .child(widgets::page_header(&theme, "Cloud computer", None))
            .child(
                widgets::page_subtitle(
                    &theme,
                    "Seats Cosmos attaches to and drives — Cosmos does not host the VM. A Box \
                     token, an SSH alias to a Linux Docker host, or a connected GitHub repo each \
                     give an agent somewhere to run; SSH keys stay in your agent, tokens stay \
                     in this device's keys file.",
                )
                .max_w(px(560.0))
                .line_height(px(20.0)),
            )
            .child(card);
        if let Some(error) = load_error {
            column = column.child(widgets::error_strip(&theme, error));
        }
        if self.editing_seat.is_some() {
            column = column.child(self.harness_editor(&theme, cx));
        }
        if self.draft_kind.is_none() && self.editing_seat.is_none() {
            column = column.child(
                div().flex().flex_row().gap(px(8.0)).pt(px(4.0)).children(
                    [
                        (CloudSeatKind::Railway, "Railway trial"),
                        (CloudSeatKind::Box, "Boat"),
                        (CloudSeatKind::Ssh, "BYO VPS (SSH)"),
                        (CloudSeatKind::GitHub, "GitHub workspace"),
                    ]
                    .into_iter()
                    .enumerate()
                    .map(|(ix, (kind, label))| {
                        popover::btn_ghost(&theme, label, format!("seat-add-{ix}"))
                            .id(("seat-add", ix))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.draft_kind = Some(kind);
                                this.note = None;
                                // Open with recents already loaded — the
                                // picker's empty-query path.
                                if kind == CloudSeatKind::GitHub {
                                    this.search_repos(cx);
                                }
                                cx.notify();
                            }))
                    }),
                ),
            );
        }
        if self.draft_kind.is_some() {
            column = column.child(self.draft_card(&theme, cx));
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

        // GitHub via the Cosmos GitHub App — powers the repo picker and
        // every GitHub seat without its own token.
        let has_pat = self.view.ready().map(|v| v.has_github_pat).unwrap_or(false);
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
            match login.as_deref().filter(|l| !l.is_empty()) {
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
                        .hover(|s| s.text_color(theme.text))
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
                        .hover(|s| s.text_color(theme.text))
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
