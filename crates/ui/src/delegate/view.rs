//! The floating Delegate window's root view: a transparent full-window layer
//! that centers the composer card and, after a send, a status pill naming the
//! delegated session.
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement as _, Render, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, prelude::FluentBuilder as _, px,
};

use crate::composer::{Composer, ComposerEvent};
use crate::state::AppState;
use crate::theme::Theme;

/// Last delegated chat id, persisted app-wide so reopening the bar still
/// shows the session it launched (the composer entity itself is also held
/// here — its draft, staged attachments and picks survive window teardown).
pub(crate) struct DelegateState {
    pub composer: Entity<Composer>,
    pub last_chat: Option<String>,
}

impl gpui::Global for DelegateState {}

/// The composer used by every Delegate window. The first `open` builds it;
/// later opens reuse the same entity so a half-written task and the staged
/// screen context survive Spotlight-style dismiss/reopen.
pub(crate) fn composer(state: &Entity<AppState>, cx: &mut App) -> Entity<Composer> {
    if let Some(existing) = cx.try_global::<DelegateState>() {
        return existing.composer.clone();
    }
    let composer = cx.new(|cx| Composer::new_delegate(state.clone(), cx));
    cx.set_global(DelegateState {
        composer: composer.clone(),
        last_chat: None,
    });
    composer
}

pub(crate) fn note_delegated(chat_id: &str, cx: &mut App) {
    if cx.try_global::<DelegateState>().is_some() {
        cx.global_mut::<DelegateState>().last_chat = Some(chat_id.to_string());
    }
}

pub(crate) struct DelegateBar {
    state: Entity<AppState>,
    composer: Entity<Composer>,
    delegated: Option<String>,
    /// Activation observers fire once on registration — the bar must not
    /// interpret "not yet active" as a dismissal. Only a real deactivate
    /// after an observed activation closes the window.
    seen_active: bool,
    focus: FocusHandle,
    _events: Subscription,
    _state_observe: Subscription,
    _activation: Subscription,
}

impl DelegateBar {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = composer(&state, cx);
        let delegated = cx
            .try_global::<DelegateState>()
            .and_then(|global| global.last_chat.clone());
        let events = cx.subscribe(&composer, |this, _, event, cx| {
            if let ComposerEvent::Sent { chat_id, .. } = event {
                this.delegated = Some(chat_id.clone());
                note_delegated(chat_id, cx);
                cx.notify();
            }
        });
        let state_observe = cx.observe(&state, |_, _, cx| cx.notify());
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.seen_active = true;
                return;
            }
            if this.seen_active && !this.composer.read(cx).has_modal_task() {
                window.remove_window();
            }
        });
        Self {
            state,
            composer,
            delegated,
            seen_active: false,
            focus: cx.focus_handle(),
            _events: events,
            _state_observe: state_observe,
            _activation: activation,
        }
    }

    /// The status pill under the card: session title + live run indicator +
    /// an Open button that surfaces the session in the main window.
    fn delegated_pill(&self, theme: &Theme, cx: &App) -> Option<AnyElement> {
        let chat_id = self.delegated.as_deref()?;
        let (title, indicator) = {
            let state = self.state.read(cx);
            let title = state
                .chat_row(chat_id)
                .and_then(|chat| chat.title.clone())
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "New task".into());
            (title, state.indicator_for(chat_id, chrono::Utc::now()))
        };
        let (label, dot) = match indicator {
            zeron_proto::view::Indicator::Working => ("Working", theme.accent),
            zeron_proto::view::Indicator::AwaitingInput => ("Needs input", theme.warning),
            zeron_proto::view::Indicator::Errored => ("Failed", theme.danger),
            zeron_proto::view::Indicator::None => ("Delegated", theme.success),
        };
        let open_chat_id = chat_id.to_string();
        let open_state = self.state.clone();
        Some(
            div()
                .id("delegate-status")
                .mt(px(8.0))
                .mx(px(8.0))
                .h(px(30.0))
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .rounded_full()
                .bg(theme.input_glass_bg())
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .child(div().size(px(7.0)).rounded_full().flex_none().bg(dot))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .flex_none()
                        .child(label),
                )
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text)
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(title),
                )
                .child(
                    div()
                        .id("delegate-open-session")
                        .px_2()
                        .h(px(20.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.accent)
                        .bg(crate::motion::hover_blend(
                            "delegate-open-session",
                            gpui::transparent_black(),
                            crate::theme::ink(0.10),
                        ))
                        .on_hover(crate::motion::hover_listener("delegate-open-session"))
                        .on_click(move |_, _, cx| {
                            crate::open_notified_chat(open_chat_id.clone(), &open_state, cx);
                        })
                        .child("Open"),
                )
                .into_any_element(),
        )
    }
}

impl Focusable for DelegateBar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DelegateBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        div()
            .size_full()
            .key_context("DelegateBar")
            .track_focus(&self.focus)
            .on_key_down(|event: &KeyDownEvent, window, _cx| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            })
            .px(px(super::BAR_SHADOW_MARGIN))
            .pt(px(super::BAR_SHADOW_MARGIN))
            .flex()
            .flex_col()
            .child(crate::motion::menu_in(
                "delegate-card",
                div().w_full().child(self.composer.clone()),
            ))
            .when_some(self.delegated_pill(&theme, cx), |el, pill| el.child(pill))
    }
}
