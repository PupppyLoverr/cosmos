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

use crate::cloud::{CloudView, SeatRow, status_dot};
use crate::composer::ComposerInput;
use crate::icons;
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

pub struct CloudSeatsPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    view: Loadable<CloudView>,
    /// `Some(kind)` while the add-seat card is open.
    draft_kind: Option<CloudSeatKind>,
    form: SeatForm,
    probing: Option<String>,
    error: Option<String>,
    note: Option<String>,
    load_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
}

impl CloudSeatsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            view: Loadable::Idle,
            draft_kind: None,
            form: SeatForm::new(cx),
            probing: None,
            error: None,
            note: None,
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
                cx.notify();
            })
            .ok();
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
            Some(CloudSeatKind::Box) => seat
                .config
                .box_endpoint
                .clone()
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
                    .child(
                        popover::btn_ghost(theme, "Probe", format!("seat-probe-{seat_id}"))
                            .id(SharedString::from(format!("seat-probe-{seat_id}")))
                            .when(probing, |el| el.opacity(0.5))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.probe(seat_id.clone(), cx)),
                            ),
                    )
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
                    el.child(popover::dialog_field(
                        self.form.repo.clone().into_any_element(),
                    ))
                    .child(popover::dialog_field(
                        self.form.branch.clone().into_any_element(),
                    ))
                    .child(popover::dialog_field(
                        self.form.github_pat.clone().into_any_element(),
                    ))
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
                     token, an SSH alias to a Linux Docker host, or a GitHub repo + PAT each \
                     give an agent somewhere to run; SSH keys stay in your agent, tokens stay \
                     in this device's keys file.",
                )
                .max_w(px(560.0))
                .line_height(px(20.0)),
            )
            .child(card);
        if self.draft_kind.is_none() {
            column = column.child(
                div().flex().flex_row().gap(px(8.0)).pt(px(4.0)).children(
                    [
                        (CloudSeatKind::Box, "Box"),
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
