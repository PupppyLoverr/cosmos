//! Settings → Devices (feature-inventory §1.5): the device registry — name,
//! platform, last-seen, an Online/Offline badge, a "This device" badge, click-to-copy id,
//! and a Rename dialog (Mutate renameDevice).

use chrono::{DateTime, Utc};
use gpui::{
    AnyElement, ClipboardItem, Context, Entity, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use std::time::Duration;

use zeron_proto::WorkspaceScope;
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::popover;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// A device that pinged within this window shows a presence dot (engines
/// heartbeat every 15s; 70s tolerates a couple of missed beats).
pub const DEVICE_ONLINE_WINDOW_SECS: i64 = 70;

/// Presence: last-seen within the online window (future timestamps count). Pure.
pub fn device_online(last_seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    last_seen
        .is_some_and(|at| now.signed_duration_since(at).num_seconds() <= DEVICE_ONLINE_WINDOW_SECS)
}

/// Compact last-seen line. Pure.
pub fn format_last_seen(last_seen: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(at) = last_seen else {
        return "never seen".to_string();
    };
    let secs = now.signed_duration_since(at).num_seconds();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Scope-aware copy: a local registry describes only the active local
/// workspace and must not imply that account device metadata is already live.
pub fn devices_subtitle(scope: Option<WorkspaceScope>) -> &'static str {
    match scope {
        Some(WorkspaceScope::Local) => "Devices in this local workspace.",
        Some(WorkspaceScope::Synced) => "Devices synced to this workspace.",
        Some(WorkspaceScope::Development) | None => "Devices in this workspace.",
    }
}

struct RenameDialog {
    device_id: String,
    input: Entity<ComposerInput>,
    _events: Subscription,
}

/// The live "Pair phone" offer returned by `PhonePairingOffer` — a 5-minute,
/// single-use code + QR bound to this account and device.
struct PairOffer {
    code: String,
    qr: String,
    expires_at_ms: i64,
}

/// A paired phone grant (ListPairedPhones rows — never carries the token).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairedPhone {
    id: String,
    label: Option<String>,
    created_at: i64,
}

pub struct DevicesPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    rename: Option<RenameDialog>,
    /// Device id whose id-chip shows "Copied" right now.
    copied: Option<String>,
    error: Option<SharedString>,
    task: Option<Task<()>>,
    copy_task: Option<Task<()>>,
    pair_open: bool,
    pair_offer: Option<PairOffer>,
    /// A `PhonePairingOffer` call is in flight — the engine only redeems the
    /// latest issued code, so overlapping fetches could display a stale QR.
    /// Suppress rotation/clicks until the pending call settles.
    pair_offer_pending: bool,
    pair_phones: Vec<PairedPhone>,
    pair_phones_authoritative: bool,
    pair_tick: Option<Task<()>>,
    _observe: Subscription,
}

impl DevicesPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            scroll: widgets::PageScroll::default(),
            rename: None,
            copied: None,
            error: None,
            task: None,
            copy_task: None,
            pair_open: false,
            pair_offer: None,
            pair_offer_pending: false,
            pair_phones: Vec::new(),
            pair_phones_authoritative: false,
            pair_tick: None,
            _observe: observe,
        }
    }

    /// "Pair phone": open the offer dialog — mint a fresh code, publish the
    /// `phonePair` registry row, refresh the paired-phones list, and tick the
    /// countdown (auto-rotate at expiry so a dead code is never on screen).
    ///
    /// Signed-out desktops must not mint offers: the host relay stays quiet
    /// without credentials, so the phone would redeem the code into an empty
    /// device room and sit waiting forever.
    fn open_pair(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).auth_user().is_none() {
            self.error = Some(
                "Sign in first — phone pairing rides the account relay.".into(),
            );
            cx.notify();
            return;
        }
        self.pair_open = true;
        self.fetch_pair_offer(true, cx);
        self.fetch_pair_phones(cx);
        if self.pair_tick.is_none() {
            self.pair_tick = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                    let open = this
                        .update(cx, |page, cx| {
                            if page.pair_open {
                                let expired = page
                                    .pair_offer
                                    .as_ref()
                                    .is_some_and(|o| Self::now_ms() >= o.expires_at_ms);
                                if expired {
                                    page.fetch_pair_offer(true, cx);
                                }
                                cx.notify();
                            }
                            page.pair_open
                        })
                        .unwrap_or(false);
                    if !open {
                        break;
                    }
                }
                this.update(cx, |page, _| page.pair_tick = None).ok();
            }));
        }
        cx.notify();
    }

    fn fetch_pair_offer(&mut self, rotate: bool, cx: &mut Context<Self>) {
        if self.pair_offer_pending {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.pair_offer_pending = true;
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::PHONE_PAIRING_OFFER,
                    serde_json::json!({ "rotate": rotate }),
                )
                .await;
            this.update(cx, |page, cx| {
                page.pair_offer_pending = false;
                match result {
                    Ok(v) => {
                        page.pair_offer = Some(PairOffer {
                            code: v.get("code").and_then(|c| c.as_str()).unwrap_or("").into(),
                            qr: v.get("qr").and_then(|c| c.as_str()).unwrap_or("").into(),
                            expires_at_ms: v.get("expiresAt").and_then(|c| c.as_i64()).unwrap_or(0),
                        });
                    }
                    Err(err) => {
                        page.error = Some(format!("Couldn't start pairing: {err}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn fetch_pair_phones(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::LIST_PAIRED_PHONES, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                if let Ok(v) = result {
                    page.pair_phones = v
                        .get("phones")
                        .and_then(|p| serde_json::from_value::<Vec<PairedPhone>>(p.clone()).ok())
                        .unwrap_or_default();
                    page.pair_phones_authoritative = true;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn revoke_phone(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.pair_phones.retain(|p| p.id != id);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::REVOKE_PHONE, serde_json::json!({ "id": id }))
                .await;
            this.update(cx, |page, cx| {
                if let Err(err) = result {
                    page.error = Some(format!("Revoke failed: {err}").into());
                }
                page.fetch_pair_phones(cx);
            })
            .ok();
        })
        .detach();
    }

    fn close_pair(&mut self, cx: &mut Context<Self>) {
        self.pair_open = false;
        cx.notify();
    }

    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    /// QR matrix → module grid painted as quads: dark modules on the white
    /// pad so the scan target survives dark themes.
    fn render_qr(payload: &str, size: f32) -> AnyElement {
        let matrix = qrcode::QrCode::new(payload.as_bytes()).ok().map(|code| {
            let n = code.width();
            let dark: Vec<bool> = code
                .to_colors()
                .iter()
                .map(|c| *c == qrcode::Color::Dark)
                .collect();
            (n, dark)
        });
        gpui::canvas(
            move |_, _, _| matrix.clone(),
            |bounds, matrix, window, _| {
                let Some((n, dark)) = matrix else {
                    return;
                };
                let cell = f32::from(bounds.size.width) / n as f32;
                for (i, filled) in dark.iter().enumerate() {
                    if !filled {
                        continue;
                    }
                    let col = (i % n) as f32;
                    let row = (i / n) as f32;
                    let module = gpui::Bounds {
                        origin: bounds.origin + gpui::point(px(col * cell), px(row * cell)),
                        // Half-pixel bleed kills hairline gaps between modules.
                        size: gpui::size(px(cell + 0.5), px(cell + 0.5)),
                    };
                    window.paint_quad(gpui::quad(
                        module,
                        px(0.0),
                        gpui::black(),
                        px(0.0),
                        gpui::transparent_black(),
                        gpui::BorderStyle::default(),
                    ));
                }
            },
        )
        .w(px(size))
        .h(px(size))
        .into_any_element()
    }

    /// The "Phone pairing" section above the device list: explainer, the
    /// "Pair phone" action, and the paired-phones list with per-row revoke.
    fn render_pair_section(&mut self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let signed_in = self.state.read(cx).auth_user().is_some();
        let mut card = widgets::section_card(theme).child(
            widgets::card_row(theme, true)
                .child(widgets::row_tile(theme, crate::icons::MONITOR))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(widgets::row_title(theme, "Pair phone"))
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(12.0))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(if signed_in {
                                    "Scan the QR with the Cosmos app — your phone can steer this desktop from anywhere, not just this network."
                                } else {
                                    "Sign in to pair a phone for remote control from anywhere."
                                })),
                        ),
                )
                .child(
                    widgets::ghost_action(theme)
                        .id("pair-phone-open")
                        .opacity(if signed_in { 0.7 } else { 0.35 })
                        .on_click(cx.listener(|this, _, _, cx| this.open_pair(cx)))
                        .child(SharedString::from("Show code")),
                ),
        );
        for (ix, phone) in self.pair_phones.iter().enumerate() {
            let id = phone.id.clone();
            let revoke_id = phone.id.clone();
            let label = phone
                .label
                .clone()
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| format!("Phone {}", short_id(&phone.id)));
            let when = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(phone.created_at)
                .map(|t| format!("Paired {}", format_last_seen(Some(t), Utc::now())))
                .unwrap_or_default();
            card = card.child(
                widgets::card_row(theme, false)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(theme, label))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .font_family(theme.font_mono.clone())
                                    .text_color(theme.text_muted.opacity(0.5))
                                    .child(SharedString::from(format!(
                                        "{} · {}",
                                        when,
                                        short_id(&id)
                                    ))),
                            ),
                    )
                    .child(
                        widgets::ghost_action(theme)
                            .id(("phone-revoke", ix))
                            .opacity(0.7)
                            .text_color(theme.danger.opacity(0.85))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.revoke_phone(revoke_id.clone(), cx);
                            }))
                            .child(SharedString::from("Revoke")),
                    ),
            );
        }
        card
    }

    /// The offer dialog: white QR pad + the same code as text + countdown.
    /// Single-use; rotates automatically at expiry.
    fn render_pair_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let offer = self.pair_offer.as_ref();
        let mut card = popover::dialog_card(theme)
            .w(px(380.0))
            .child(popover::dialog_title(theme, "Pair phone"))
            .child(
                div()
                    .mt(px(8.0))
                    .text_size(crate::typography::ui_rems(12.5))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(
                        "Open Cosmos on your phone, sign in with the same account, then scan — or type the code.",
                    )),
            );
        if let Some(offer) = offer {
            let secs_left = ((offer.expires_at_ms - Self::now_ms()) / 1000).max(0);
            let grouped = offer
                .code
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    if i == 3 {
                        format!(" {}", c)
                    } else {
                        c.to_string()
                    }
                })
                .collect::<String>();
            card = card
                .child(
                    div().mt(px(16.0)).flex().justify_center().child(
                        div()
                            .p(px(14.0))
                            .rounded(px(14.0))
                            .bg(gpui::white())
                            .child(Self::render_qr(&offer.qr, 184.0)),
                    ),
                )
                .child(
                    div().mt(px(14.0)).flex().justify_center().child(
                        div()
                            .font_family(theme.font_mono.clone())
                            .text_size(crate::typography::ui_rems(26.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .child(SharedString::from(grouped)),
                    ),
                )
                .child(
                    div()
                        .mt(px(6.0))
                        .flex()
                        .justify_center()
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted.opacity(0.7))
                        .child(SharedString::from(format!(
                            "Single use · expires in {}:{:02}",
                            secs_left / 60,
                            secs_left % 60
                        ))),
                );
        } else {
            card = card.child(
                div()
                    .mt(px(16.0))
                    .py(px(30.0))
                    .text_center()
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from("Minting a code…")),
            );
        }
        card = card.child(
            div()
                .mt(px(18.0))
                .flex()
                .flex_row()
                .justify_end()
                .gap(px(8.0))
                .child(
                    popover::btn_ghost(theme, "New code", "pair-rotate")
                        .id("pair-rotate")
                        .on_click(cx.listener(|this, _, _, cx| this.fetch_pair_offer(true, cx))),
                )
                .child(
                    popover::btn_primary(theme, "Done")
                        .id("pair-done")
                        .on_click(cx.listener(|this, _, _, cx| this.close_pair(cx))),
                ),
        );
        popover::modal("pair-phone-dialog", viewport, card.into_any_element())
    }

    pub(crate) fn open_rename(
        &mut self,
        device_id: String,
        current: String,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| ComposerInput::new("Device name", cx));
        input.update(cx, |input, cx| input.set_text(current, cx));
        let events = cx.subscribe(&input, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.submit_rename(cx);
            }
        });
        self.rename = Some(RenameDialog {
            device_id,
            input,
            _events: events,
        });
        cx.notify();
    }

    /// Escape that reached Settings unclaimed closes the rename dialog first,
    /// so it never closes Settings under the dialog. Returns whether it did.
    pub(crate) fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.rename.take().is_none() {
            return false;
        }
        cx.notify();
        true
    }

    fn submit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.rename.take() else {
            return;
        };
        let name = dialog.input.read(cx).text().trim().to_string();
        if name.is_empty() {
            cx.notify();
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = serde_json::json!({
            "op": "renameDevice",
            "deviceId": dialog.device_id,
            "name": name,
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::MUTATE, params).await;
            this.update(cx, |page, cx| {
                if let Err(err) = result {
                    page.error = Some(format!("Rename failed: {err}").into());
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn copy_id(&mut self, device_id: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(device_id.clone()));
        self.copied = Some(device_id);
        self.copy_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            this.update(cx, |page, cx| {
                page.copied = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_rename_dialog(
        &mut self,
        viewport: gpui::Size<gpui::Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let dialog = self.rename.as_ref()?;
        let input = dialog.input.clone();
        let card = popover::dialog_card(&theme)
            .id("rename-device-card")
            .role(gpui::Role::Dialog)
            .aria_label("Rename device")
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.rename = None;
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(popover::dialog_title(&theme, "Rename device"))
            .child(
                div()
                    .mt(px(12.0))
                    .child(popover::dialog_field(input.into_any_element())),
            )
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        widgets::text_action(&theme, widgets::ActionTone::Quiet, "Cancel")
                            .id("rename-cancel")
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.rename = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        widgets::text_action(&theme, widgets::ActionTone::Solid, "Rename")
                            .id("rename-save")
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                            .on_click(cx.listener(|this, _, _, cx| this.submit_rename(cx))),
                    ),
            )
            .into_any_element();
        Some(popover::modal("rename-device-dialog", viewport, card))
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for DevicesPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

/// Human platform label (zeron settings.devices.tsx `platformLabel`).
pub fn platform_label(platform: &str) -> &str {
    match platform {
        "macos" | "darwin" => "macOS",
        "linux" => "Linux",
        "windows" => "Windows",
        "web" => "Web",
        "ios" => "iOS",
        "android" => "Android",
        other => other,
    }
}

/// Short device id for the click-to-copy chip (`abcd1234…wxyz`).
pub fn short_id(id: &str) -> String {
    if id.len() > 12 {
        format!("{}…{}", &id[..8], &id[id.len() - 4..])
    } else {
        id.to_string()
    }
}

impl Render for DevicesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let now = Utc::now();
        let (devices, local_id, workspace_scope) = {
            let state = self.state.read(cx);
            (
                state.devices.clone(),
                state.local_device_id.clone(),
                state.workspace_scope,
            )
        };
        let copied = self.copied.clone();
        let dialog = self.render_rename_dialog(window.viewport_size(), cx);
        let viewport = window.viewport_size();
        let count = devices.len();
        // Split into this device and the rest; each renders as rows in one
        // block, like every other settings page.
        let (local, others): (Vec<_>, Vec<_>) = devices
            .into_iter()
            .enumerate()
            .partition(|(_, device)| local_id.as_deref() == Some(device.id.as_str()));
        let device_row = |ix: usize, device: zeron_proto::Device, first: bool| {
            let online = device_online(device.last_seen_at, now);
            let is_local = local_id.as_deref() == Some(device.id.as_str());
            let id_copied = copied.as_deref() == Some(device.id.as_str());
            let copy_id = device.id.clone();
            let rename_id = device.id.clone();
            let rename_name = device.name.clone();
            let mut meta: Vec<AnyElement> = vec![
                div()
                    .child(SharedString::from(platform_label(&device.platform).to_string()))
                    .into_any_element(),
            ];
            if let Some(version) = device.version.as_deref().filter(|v| !v.is_empty()) {
                meta.push(
                    div()
                        .child(SharedString::from(format!("v{version}")))
                        .into_any_element(),
                );
            }
            // Presence only says something about other devices.
            if !is_local {
                meta.push(if online {
                    div()
                        .text_color(theme.success_muted)
                        .child(SharedString::from("Online"))
                        .into_any_element()
                } else {
                    div()
                        .child(SharedString::from(format!(
                            "Last seen {}",
                            format_last_seen(device.last_seen_at, now)
                        )))
                        .into_any_element()
                });
            }
            widgets::card_row(&theme, first)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.0))
                        .child(widgets::row_title(&theme, device.name.clone()))
                        .child(widgets::meta_line(&theme, meta)),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            widgets::text_action(
                                &theme,
                                widgets::ActionTone::Quiet,
                                if id_copied { "Copied" } else { "Copy ID" },
                            )
                            .id(("device-id", ix))
                            .tab_index(0)
                            .role(gpui::Role::Button)
                            .aria_label(format!("Copy device ID {}", device.id))
                            .focus_visible(|s| s.border_2().border_color(theme.accent))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.copy_id(copy_id.clone(), cx);
                            })),
                        )
                        .child(
                            widgets::text_action(&theme, widgets::ActionTone::Filled, "Rename")
                                .id(("device-rename", ix))
                                .tab_index(0)
                                .role(gpui::Role::Button)
                                .aria_label(format!("Rename {}", device.name))
                                .focus_visible(|s| s.border_2().border_color(theme.accent))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_rename(rename_id.clone(), rename_name.clone(), cx);
                                })),
                        ),
                )
                .into_any_element()
        };
        let local_block = (!local.is_empty()).then(|| {
            let mut block = widgets::section_card(&theme).mt(px(8.0));
            for (n, (ix, device)) in local.into_iter().enumerate() {
                block = block.child(device_row(ix, device, n == 0));
            }
            block
        });
        let others_block = if others.is_empty() {
            widgets::section_card(&theme).mt(px(8.0)).child(
                widgets::card_row(&theme, true).child(
                    div()
                        .text_size(crate::typography::ui_rems(13.0))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(
                            "Sign in on another device to see it here.",
                        )),
                ),
            )
        } else {
            let mut block = widgets::section_card(&theme).mt(px(8.0));
            for (n, (ix, device)) in others.into_iter().enumerate() {
                block = block.child(device_row(ix, device, n == 0));
            }
            block
        };
        let card = div()
            .flex()
            .flex_col()
            .when_some(local_block, |el, block| {
                el.child(widgets::section_label(&theme, "This device").mt(px(28.0)))
                    .child(block)
            })
            // A local-only workspace never has other devices to list.
            .when(workspace_scope != Some(WorkspaceScope::Local), |el| {
                el.child(widgets::section_label(&theme, "Other devices").mt(px(28.0)))
                    .child(others_block)
            });

        let pair_section = self.render_pair_section(&theme, cx);
        let scrollbar = popover::rail(self, "devices-page-scrollbar", &theme, cx);
        let pair_dialog = if self.pair_open {
            Some(self.render_pair_dialog(viewport, &theme, cx))
        } else {
            None
        };
        div()
            .id("devices-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("devices-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(widgets::page_header(
                                    &theme,
                                    "Devices",
                                    (count > 0).then_some(count),
                                ))
                                .child(widgets::page_subtitle(
                                    &theme,
                                    devices_subtitle(workspace_scope),
                                ))
                                .when_some(self.error.clone(), |el, message| {
                                    el.child(
                                        widgets::error_strip(&theme, message)
                                            .id("devices-error")
                                            .cursor_pointer()
                                            .tab_index(0)
                                            .role(gpui::Role::Button)
                                            .focus_visible(|s| {
                                                s.border_2().border_color(theme.accent).opacity(1.0)
                                            })
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.error = None;
                                                cx.notify();
                                            })),
                                    )
                                })
                                .child(pair_section)
                                .child(card),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
            .when_some(dialog, |el, dialog| el.child(dialog))
            .when_some(pair_dialog, |el, dialog| el.child(dialog))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;

    #[test]
    fn presence_window() {
        let now = Utc::now();
        assert!(device_online(Some(now - TimeDelta::seconds(10)), now));
        assert!(device_online(Some(now - TimeDelta::seconds(70)), now));
        assert!(!device_online(Some(now - TimeDelta::seconds(71)), now));
        assert!(!device_online(None, now));
        // Clock skew (future) counts as online.
        assert!(device_online(Some(now + TimeDelta::seconds(30)), now));
    }

    #[test]
    fn last_seen_formatting() {
        let now = Utc::now();
        assert_eq!(format_last_seen(None, now), "never seen");
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::seconds(30)), now),
            "just now"
        );
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::minutes(5)), now),
            "5m ago"
        );
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::hours(3)), now),
            "3h ago"
        );
        assert_eq!(
            format_last_seen(Some(now - TimeDelta::days(2)), now),
            "2d ago"
        );
    }

    #[test]
    fn local_subtitle_does_not_claim_synced_metadata() {
        let copy = devices_subtitle(Some(WorkspaceScope::Local));
        assert!(copy.contains("local workspace"));
        assert!(!copy.contains("synced"));
    }
}
