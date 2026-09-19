//! Settings → Computer: the Cosmos Computer surface — Screen Context
//! (semantic/screenshot observation) plus the optional local Cua Driver
//! component, control mode, OS permission posture, and first-time remote
//! pairing.
//!
//! Install and settings writes stay host-local. Paired phones may forward
//! observe/tool/stop/live-watch after an explicit Approve on this page.
//! Status and install progress arrive on `WatchComputer` — one stream
//! merging `status()` snapshots with `install` ticks — and every mutating
//! call replies with the fresh status, so the page always repaints from
//! the engine's authoritative state.

use gpui::{
    AnyElement, Context, Entity, IntoElement, Render, SharedString, Task, Window, div, prelude::*,
    px,
};

use zeron_computer::install::InstallProgress;
use zeron_computer::{ComputerSettings, ComputerStatus, ControlMode, Probe};
use zeron_rpc::methods;

use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

pub struct ComputerPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    status: Loadable<ComputerStatus>,
    probe: Loadable<Probe>,
    /// Latest install tick off `WatchComputer` (`{status, install}` frames).
    install: InstallProgress,
    /// Last refused/failed mutation (install in-flight, save failure) — the
    /// error strip under the cards.
    error: Option<String>,
    saving: bool,
    load_task: Option<Task<()>>,
    watch_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
}

impl ComputerPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            status: Loadable::Idle,
            probe: Loadable::Idle,
            install: InstallProgress::Idle,
            error: None,
            saving: false,
            load_task: None,
            watch_task: None,
            action_task: None,
        };
        page.load(cx);
        page.watch(cx);
        page.probe(cx);
        page
    }

    /// `ComputerStatus` — the page's first paint; the watch keeps it live.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.status = Loadable::Loading;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::COMPUTER_STATUS, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                page.status = match result {
                    Ok(value) => match serde_json::from_value::<ComputerStatus>(value) {
                        Ok(status) => Loadable::Ready(status),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// `WatchComputer` — `{status, install}` frames: status snapshots ride
    /// the same stream as install progress, so the page never polls.
    fn watch(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.watch_task = Some(cx.spawn(async move |this, cx| {
            const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);
            loop {
                let mut rx = match engine
                    .client()
                    .subscribe(methods::WATCH_COMPUTER, serde_json::json!({}))
                    .await
                {
                    Ok(rx) => rx,
                    Err(err) => {
                        tracing::debug!(error = %err, "computer watch unavailable; retrying");
                        if this.update(cx, |_, _| {}).is_err() {
                            return;
                        }
                        cx.background_executor().timer(RETRY_DELAY).await;
                        continue;
                    }
                };
                while let Some(value) = rx.recv().await {
                    let status = value
                        .get("status")
                        .cloned()
                        .and_then(|v| serde_json::from_value::<ComputerStatus>(v).ok());
                    let install = value
                        .get("install")
                        .cloned()
                        .and_then(|v| serde_json::from_value::<InstallProgress>(v).ok());
                    let alive = this.update(cx, |page, cx| {
                        if let Some(status) = status {
                            page.status = Loadable::Ready(status);
                        }
                        if let Some(install) = install {
                            page.install = install;
                        }
                        cx.notify();
                    });
                    if alive.is_err() {
                        return;
                    }
                }
                if this.update(cx, |_, _| {}).is_err() {
                    return;
                }
                cx.background_executor().timer(RETRY_DELAY).await;
            }
        }));
    }

    /// `ComputerProbe` — live driver facts (ping + the driver's own health
    /// report, which carries macOS TCC grant state).
    fn probe(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.probe = Loadable::Loading;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::COMPUTER_PROBE, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                page.probe = match result {
                    Ok(value) => match serde_json::from_value::<Probe>(value) {
                        Ok(probe) => Loadable::Ready(probe),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// The full persisted settings, rebuilt from the status echo so every
    /// `ComputerSetSettings` round-trips the untouched fields faithfully.
    fn current_settings(&self) -> Option<ComputerSettings> {
        let Loadable::Ready(status) = &self.status else {
            return None;
        };
        Some(ComputerSettings {
            enabled: status.settings.enabled,
            semantic: status.settings.semantic,
            screenshots: status.settings.screenshots,
            control: status.settings.control,
            telemetry: status.settings.telemetry,
            inspect_max_elements: status.settings.inspect_max_elements,
            screenshot_max_dimension: status.settings.screenshot_max_dimension,
        })
    }

    /// One settings write — the reply carries the fresh status (the same
    /// self-correcting repaint the harness toggles use).
    fn set_settings(&mut self, mutate: impl FnOnce(&mut ComputerSettings), cx: &mut Context<Self>) {
        let Some(mut settings) = self.current_settings() else {
            return;
        };
        mutate(&mut settings);
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Ok(params) = serde_json::to_value(&settings) else {
            return;
        };
        self.saving = true;
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::COMPUTER_SET_SETTINGS, params)
                .await;
            this.update(cx, |page, cx| {
                page.saving = false;
                match result {
                    Ok(value) => match serde_json::from_value::<ComputerStatus>(value) {
                        Ok(status) => page.status = Loadable::Ready(status),
                        Err(err) => page.error = Some(err.to_string()),
                    },
                    Err(err) => page.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Host-local pairing approve / revoke. Not relay-forwardable from the
    /// phone that requested trust — the desktop operator clicks here.
    fn pairing_action(&mut self, method: &'static str, device_id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(method, serde_json::json!({ "deviceId": device_id }))
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(value) => {
                        // Trust RPCs return TrustPublic; reload status so the
                        // pairing card and WatchComputer stay aligned.
                        page.load(cx);
                        let _ = value;
                    }
                    Err(err) => page.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// `ComputerInstall` / `ComputerUninstall` — install replies immediately
    /// (progress rides the watch); uninstall returns the fresh status.
    fn component_action(&mut self, method: &'static str, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, serde_json::json!({})).await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(value) => match serde_json::from_value::<ComputerStatus>(value) {
                        Ok(status) => page.status = Loadable::Ready(status),
                        Err(err) => page.error = Some(err.to_string()),
                    },
                    Err(err) => page.error = Some(err.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn row(
        theme: &Theme,
        first: bool,
        title: &str,
        description: &str,
        control: gpui::AnyElement,
    ) -> gpui::Div {
        widgets::card_row(theme, first)
            .flex_wrap()
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(widgets::row_title(theme, title.to_string()))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .line_height(px(18.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(description.to_string())),
                    ),
            )
            .child(control)
    }

    /// The state badge in the page header — mirrors `status.state` verbatim
    /// so every engine state reads the same here and in `computer_status`.
    fn state_badge(&self, theme: &Theme, status: &ComputerStatus) -> AnyElement {
        let label = match status.state.as_str() {
            "disabled" => "Off",
            "ready" => "Ready",
            "missing" => "Component not installed",
            "incomplete" => "Component incomplete",
            "unsupported" => "Unsupported platform",
            "error" => "Error",
            other => other,
        };
        let badge = if status.state == "ready" {
            widgets::badge_active(theme, label)
        } else {
            widgets::badge(theme, label)
        };
        badge.into_any_element()
    }

    /// Segmented control for the three control modes — the appshots
    /// destination picker's pattern.
    fn control_mode_picker(
        &self,
        theme: &Theme,
        current: ControlMode,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        const MODES: [ControlMode; 3] = [
            ControlMode::Observe,
            ControlMode::Background,
            ControlMode::FullControl,
        ];
        div()
            .flex()
            .flex_wrap()
            .gap(px(6.0))
            .children(MODES.into_iter().enumerate().map(|(ix, mode)| {
                let selected = current == mode;
                let mut el = div()
                    .id(("computer-control", ix))
                    .role(gpui::Role::Button)
                    .aria_label(mode.label())
                    .aria_toggled(if selected {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .tab_index(0)
                    .focus_visible(|style| style.border_2().border_color(theme.accent))
                    .px(px(10.0))
                    .py(px(7.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(if selected {
                        theme.text.opacity(0.24)
                    } else {
                        theme.border
                    })
                    .bg(if selected {
                        crate::theme::ink(0.09)
                    } else {
                        gpui::transparent_black()
                    })
                    .text_size(px(11.0))
                    .text_color(if selected {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .child(mode.label());
                if interactive {
                    el = el
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_settings(|s| s.control = mode, cx);
                        }));
                }
                el
            }))
            .into_any_element()
    }

    /// The component card: download-size disclosure always visible, then
    /// Install / Reinstall / Uninstall per state, and the live progress line
    /// from `WatchComputer`'s install ticks.
    fn component_card(
        &self,
        theme: &Theme,
        status: &ComputerStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let download_mb = status.component_download_bytes / (1024 * 1024);
        let installing = matches!(
            self.install,
            InstallProgress::Downloading { .. }
                | InstallProgress::Verifying
                | InstallProgress::Extracting
        );
        let action = |id: &'static str, label: &'static str| {
            widgets::ghost_action(theme)
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(label)
                .tab_index(0)
                .focus_visible(|style| style.border_2().border_color(theme.accent))
                .cursor_pointer()
                .child(label)
        };
        // The component state line: version + dir when installed, the stage
        // while a job runs, or the not-installed disclosure otherwise.
        let component_line: String = match &self.install {
            InstallProgress::Downloading { received, total } => format!(
                "Downloading component… {:.0} / {:.0} MB",
                *received as f64 / 1_048_576.0,
                *total as f64 / 1_048_576.0
            ),
            InstallProgress::Verifying => "Verifying checksum…".into(),
            InstallProgress::Extracting => "Extracting component…".into(),
            InstallProgress::Failed(message) => format!("Install failed: {message}"),
            _ => match status.state.as_str() {
                "ready" | "error" => format!(
                    "Cua Driver {} installed{}",
                    status.driver_version.as_deref().unwrap_or(""),
                    status
                        .component_installed_bytes
                        .map(|b| format!(" ({} MB on disk)", b / (1024 * 1024)))
                        .unwrap_or_default(),
                ),
                "incomplete" => "A partial install was found — reinstall to repair it.".into(),
                "unsupported" => "No driver build exists for this OS/architecture.".into(),
                _ => format!(
                    "Optional component (~{download_mb} MB) — the open-source Cua Driver, \
                     installed locally under your Cosmos data directory. Nothing is sent to \
                     the cloud."
                ),
            },
        };
        let mut card = widgets::section_card(theme).mt(px(20.0)).child(Self::row(
            theme,
            true,
            "Computer component",
            &component_line,
            if status.state == "unsupported" {
                widgets::badge(theme, "Unavailable").into_any_element()
            } else if installing {
                widgets::badge(theme, "Installing…").into_any_element()
            } else if status.state == "ready" || status.state == "error" {
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(
                        action("computer-reinstall", "Reinstall").on_click(cx.listener(
                            |this, _, _, cx| this.component_action(methods::COMPUTER_INSTALL, cx),
                        )),
                    )
                    .child(
                        action("computer-uninstall", "Uninstall").on_click(cx.listener(
                            |this, _, _, cx| this.component_action(methods::COMPUTER_UNINSTALL, cx),
                        )),
                    )
                    .into_any_element()
            } else {
                action("computer-install", "Install component")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.component_action(methods::COMPUTER_INSTALL, cx)
                    }))
                    .into_any_element()
            },
        ));
        if let Some(dir) = &status.install_dir {
            card = card.child(
                div()
                    .px(px(16.0))
                    .pb(px(12.0))
                    .text_size(px(11.0))
                    .text_color(theme.text_muted.opacity(0.8))
                    .child(SharedString::from(dir.clone())),
            );
        }
        card.into_any_element()
    }

    /// Permissions & driver card: static platform hints plus the live probe
    /// (the driver's own health report refines the hints on macOS).
    fn permissions_card(
        &self,
        theme: &Theme,
        status: &ComputerStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut card = widgets::section_card(theme).mt(px(20.0)).child(Self::row(
            theme,
            true,
            "Screen capture",
            &status.permissions.screen,
            widgets::badge(theme, "macOS / Linux").into_any_element(),
        ));
        card = card.child(Self::row(
            theme,
            false,
            "Accessibility control",
            &status.permissions.accessibility,
            widgets::badge(theme, "OS").into_any_element(),
        ));
        if let Some(note) = &status.permissions.note {
            card = card.child(
                div()
                    .px(px(16.0))
                    .py(px(10.0))
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(note.clone())),
            );
        }
        // Live driver facts — the probe answers "is the driver actually
        // healthy right now", which the static hints can't.
        let probe_line: SharedString = match &self.probe {
            Loadable::Ready(probe) if probe.driver_ok => {
                let grants = probe
                    .health
                    .as_ref()
                    .map(|h| serde_json::to_string_pretty(h).unwrap_or_default())
                    .unwrap_or_default();
                if grants.is_empty() {
                    "Driver is running.".into()
                } else {
                    format!("Driver is running.\n{grants}").into()
                }
            }
            Loadable::Ready(probe) => format!(
                "Driver did not answer: {}",
                probe.error.as_deref().unwrap_or("unknown error")
            )
            .into(),
            Loadable::Error(message) => format!("Driver probe failed: {message}").into(),
            _ => "Checking driver…".into(),
        };
        let check = widgets::ghost_action(theme)
            .id("computer-probe")
            .role(gpui::Role::Button)
            .aria_label("Check the driver again")
            .tab_index(0)
            .focus_visible(|style| style.border_2().border_color(theme.accent))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.probe(cx)))
            .child("Check driver");
        card = card.child(
            div()
                .px(px(16.0))
                .py(px(10.0))
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(theme.text_muted)
                        .child(probe_line),
                )
                .child(check),
        );
        if let Some(error) = &status.last_error {
            card = card.child(widgets::warning_strip(theme, error.clone()));
        }
        card.into_any_element()
    }

    /// First-time remote pairing: phones request trust, this desktop Approves.
    fn pairing_card(
        &self,
        theme: &Theme,
        status: &ComputerStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pending = status.pending_trust.clone();
        let trusted = status.trusted_devices.clone();
        let mut card = widgets::section_card(theme).mt(px(20.0)).child(Self::row(
            theme,
            true,
            "Remote pairing",
            "Same-account phones can discover this desktop. Computer Use still needs an explicit Approve here. Tokens stay on the phone; this machine stores only a hash.",
            widgets::badge(
                theme,
                if pending.is_empty() && trusted.is_empty() {
                    "None"
                } else if pending.is_empty() {
                    "Paired"
                } else {
                    "Pending"
                },
            )
            .into_any_element(),
        ));
        if pending.is_empty() && trusted.is_empty() {
            card = card.child(
                div()
                    .px(px(16.0))
                    .pb(px(12.0))
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(
                        "No pairing requests. When an Android or iOS client taps Request trust, it appears here.",
                    )),
            );
        }
        for (ix, req) in pending.into_iter().enumerate() {
            let id = req.device_id.clone();
            let id_deny = req.device_id.clone();
            let platform = req.platform.clone().unwrap_or_else(|| "device".into());
            let desc = format!("{} · {}", req.device_name, platform);
            let approve = widgets::ghost_action(theme)
                .id(("computer-pair-approve", ix))
                .role(gpui::Role::Button)
                .aria_label("Approve pairing")
                .tab_index(0)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.pairing_action(methods::COMPUTER_APPROVE_TRUST, id.clone(), cx);
                }))
                .child("Approve");
            let deny = widgets::ghost_action(theme)
                .id(("computer-pair-deny", ix))
                .role(gpui::Role::Button)
                .aria_label("Deny pairing")
                .tab_index(0)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.pairing_action(methods::COMPUTER_REVOKE_TRUST, id_deny.clone(), cx);
                }))
                .child("Deny");
            card = card.child(Self::row(
                theme,
                false,
                "Waiting for approval",
                &desc,
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(approve)
                    .child(deny)
                    .into_any_element(),
            ));
        }
        for (ix, req) in trusted.into_iter().enumerate() {
            let id = req.device_id.clone();
            let platform = req.platform.clone().unwrap_or_else(|| "device".into());
            let desc = format!("{} · {}", req.device_name, platform);
            let revoke = widgets::ghost_action(theme)
                .id(("computer-pair-revoke", ix))
                .role(gpui::Role::Button)
                .aria_label("Revoke pairing")
                .tab_index(0)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.pairing_action(methods::COMPUTER_REVOKE_TRUST, id.clone(), cx);
                }))
                .child("Revoke");
            card = card.child(Self::row(
                theme,
                false,
                "Trusted device",
                &desc,
                revoke.into_any_element(),
            ));
        }
        card.into_any_element()
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for ComputerPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for ComputerPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let accent = theme.accent;
        let body: AnyElement = match &self.status {
            Loadable::Idle | Loadable::Loading => widgets::section_card(&theme)
                .p(px(16.0))
                .child(popover::skeleton_rows(
                    "computer-skeleton",
                    &theme,
                    4,
                    cx.entity_id(),
                    cx,
                ))
                .into_any_element(),
            Loadable::Error(message) => {
                let message = message.clone();
                div()
                    .child(widgets::error_strip(&theme, message))
                    .child(
                        widgets::ghost_action(&theme)
                            .id("computer-retry")
                            .mt(px(8.0))
                            .hover(|s| widgets::ghost_hover(&theme, s))
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.load(cx);
                                cx.notify();
                            }))
                            .child(SharedString::from("Retry")),
                    )
                    .into_any_element()
            }
            Loadable::Ready(status) => {
                let status = status.clone();
                let enabled = status.settings.enabled;
                let component_ready = status.state == "ready" || status.state == "error";
                let toggle = |id: &'static str, label: &'static str, on: bool| {
                    widgets::toggle_switch(&theme, on)
                        .id(id)
                        .role(gpui::Role::Switch)
                        .aria_label(label)
                        .aria_toggled(if on {
                            gpui::Toggled::True
                        } else {
                            gpui::Toggled::False
                        })
                        .tab_index(0)
                        .focus_visible(move |style| style.border_2().border_color(accent))
                        .cursor_pointer()
                };
                let mut card = widgets::section_card(&theme).child(Self::row(
                    &theme,
                    true,
                    "Screen Context",
                    "Let agents see this device's screen — semantic UI text first, \
                     screenshots only when pixels are needed. Observation never moves \
                     your cursor or steals focus.",
                    toggle("computer-enabled", "Screen Context", enabled)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.set_settings(|s| s.enabled = !s.enabled, cx);
                        }))
                        .into_any_element(),
                ));
                if enabled {
                    card = card
                        .child(Self::row(
                            &theme,
                            false,
                            "Semantic context",
                            "Accessibility/UI-element text — the cheapest and most reliable way \
                             for an agent to read the screen.",
                            toggle(
                                "computer-semantic",
                                "Semantic context",
                                status.settings.semantic,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_settings(|s| s.semantic = !s.semantic, cx);
                            }))
                            .into_any_element(),
                        ))
                        .child(Self::row(
                            &theme,
                            false,
                            "Screenshots",
                            "Allow pixel captures when an observation needs them. On macOS this \
                             needs Screen Recording; on Wayland the desktop portal.",
                            toggle(
                                "computer-screenshots",
                                "Screenshots",
                                status.settings.screenshots,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_settings(|s| s.screenshots = !s.screenshots, cx);
                            }))
                            .into_any_element(),
                        ))
                        .child(Self::row(
                            &theme,
                            false,
                            "Control",
                            "What agents may do on the desktop. Background delivery never \
                             touches focus or the cursor; Full control adds foreground \
                             escalation only after an explicit refusal.",
                            self.control_mode_picker(
                                &theme,
                                status.settings.control,
                                component_ready,
                                cx,
                            ),
                        ))
                        .child(Self::row(
                            &theme,
                            false,
                            "Driver telemetry",
                            "The Cua Driver's own product telemetry (content-free, EU-hosted). \
                             Off by default — Cosmos posture, not the driver's.",
                            toggle(
                                "computer-telemetry",
                                "Driver telemetry",
                                status.settings.telemetry,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_settings(|s| s.telemetry = !s.telemetry, cx);
                            }))
                            .into_any_element(),
                        ));
                }
                let mut column = div().child(card);
                if enabled {
                    column = column
                        .child(self.component_card(&theme, &status, cx))
                        .child(self.pairing_card(&theme, &status, cx))
                        .child(self.permissions_card(&theme, &status, cx));
                }
                column.into_any_element()
            }
        };
        let error = self
            .error
            .clone()
            .map(|message| widgets::error_strip(&theme, message).into_any_element());
        let badge = match &self.status {
            Loadable::Ready(status) => Some(self.state_badge(&theme, status)),
            _ => None,
        };
        let scrollbar = popover::rail(self, "computer-page-scrollbar", &theme, cx);

        div()
            .id("computer-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div()
                    .id("computer-page")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll.scroll)
                    .child(
                        widgets::page_column()
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .child(widgets::page_header(&theme, "Computer", None))
                                    .children(badge),
                            )
                            .child(
                                widgets::page_subtitle(
                                    &theme,
                                    "Cosmos Computer lets agents see and act on this device's \
                                     desktop through a locally installed, open-source driver. \
                                     Your agent and model stay in charge — Cosmos only supplies \
                                     screen context and control. No cloud, no account.",
                                )
                                .max_w(px(512.0))
                                .line_height(px(20.0)),
                            )
                            .child(body)
                            .children(error),
                    ),
            )
            .children(scrollbar)
    }
}
