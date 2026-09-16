//! A live conversation docked in the right pane: a bottom-anchored
//! [`Transcript`] over any chat's doc plus a slim composer that sends into
//! it — independent of the conversation selected in the main column.
//!
//! The doc feed reuses the subagent-doc machinery (`watch_subagent_doc`
//! subscribes `WatchDocMessages` on the chat id); sends reuse the composer's
//! command paths directly: `QueueCommand(Run)` when idle, `QueueMessage`
//! while the chat is busy. Optimistic echoes and pending-send overlays land
//! in [`AppState::echoes`] / [`AppState::pending_sends`] under the bound chat
//! id — the watched doc adopts them by message id, or (for queued sends,
//! which promote under a server id) by identical user text.

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, SharedString, Subscription,
    Window, div, prelude::*, px,
};
use zeron_doc::{MessagePart, MessageRole, SessionCommandPayload, SessionMessageEntry};
use zeron_proto::{RunRequest, SandboxLevel, capabilities};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, Submit};
use crate::icons::{self, icon};
use crate::state::{AppState, Indicator};
use crate::theme::{self, Theme};
use crate::transcript::Transcript;
use crate::{motion, popover};

/// How many conversations the switcher menu lists.
const SWITCHER_MENU_ROWS: usize = 14;

/// One side-chat tab. The bound conversation is the send target AND the
/// transcript doc; switching conversations rebinds both.
pub struct SideChat {
    state: Entity<AppState>,
    /// The chat this surface is bound to; `None` = the pick-a-conversation
    /// empty state.
    chat_id: Option<String>,
    /// Doc this surface currently holds a watch on (equals `chat_id` while
    /// bound — kept separate so rebinds can release the previous watch).
    watched_doc: Option<String>,
    transcript: Option<Entity<Transcript>>,
    input: Entity<ComposerInput>,
    /// Conversation switcher dropdown (same popup machinery as the `+` menu).
    switcher: popover::Popup<()>,
    failure: Option<SharedString>,
    /// The user's own words after a failed send — restored on click.
    failed_text: Option<String>,
    /// `(chat_id, message_id)` of every echo this surface pushed — dropped on
    /// close so a surface can't orphan its optimistic rows.
    sent: Vec<(String, String)>,
    _input_sub: Subscription,
}

impl SideChat {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| ComposerInput::new("Message…", cx));
        let input_sub = cx.subscribe(
            &input,
            |_, _, _: &crate::composer::ComposerInputEvent, cx| {
                cx.notify();
            },
        );
        Self {
            state,
            chat_id: None,
            watched_doc: None,
            transcript: None,
            input,
            switcher: popover::Popup::default(),
            failure: None,
            failed_text: None,
            sent: Vec::new(),
            _input_sub: input_sub,
        }
    }

    /// The doc this surface holds a watch on — the shell consults it before
    /// releasing the subscription (another tab may share the doc).
    pub fn watched_doc(&self) -> Option<&str> {
        self.watched_doc.as_deref()
    }

    /// The transcript entity — the shell reads its jump-pill state.
    pub fn transcript_entity(&self) -> Option<&Entity<Transcript>> {
        self.transcript.as_ref()
    }

    /// Tab-strip title: the bound conversation's title, or the surface name.
    pub fn title(&self, cx: &App) -> SharedString {
        self.chat_id
            .as_deref()
            .and_then(|id| self.state.read(cx).chat_row(id))
            .and_then(|c| c.title.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| "Side Chat".to_string())
            .into()
    }

    /// Bind to a conversation: watch its doc (idempotent — a no-op when
    /// another surface already watches it) and mount the transcript.
    /// Returns the previously bound doc id so the caller can release it.
    pub fn bind(&mut self, chat_id: String, cx: &mut Context<Self>) -> Option<String> {
        if self.chat_id.as_deref() == Some(chat_id.as_str()) {
            return None;
        }
        let previous = self.watched_doc.take();
        self.state.update(cx, |s, cx| {
            s.watch_subagent_doc(chat_id.clone(), cx);
        });
        self.watched_doc = Some(chat_id.clone());
        self.chat_id = Some(chat_id.clone());
        self.transcript =
            Some(cx.new(|cx| Transcript::for_side_chat(self.state.clone(), chat_id, cx)));
        self.failure = None;
        self.failed_text = None;
        cx.notify();
        previous
    }

    /// Focus the input (the pane calls this on mount).
    pub fn focus_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.input.focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// `Submit` (Enter in the composer key context) or the send button.
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat_id) = self.chat_id.clone() else {
            return;
        };
        let text = self.input.read(cx).text().trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.failure = Some("Not connected to an engine".into());
            cx.notify();
            return;
        };
        let now = chrono::Utc::now();
        let (config, cwd, busy) = {
            let s = self.state.read(cx);
            let Some(chat) = s.chat_row(&chat_id) else {
                return;
            };
            let busy = matches!(
                s.indicator_for(&chat_id, now),
                Indicator::Working | Indicator::AwaitingInput
            );
            (
                chat.config.clone(),
                chat.cwd.clone().unwrap_or_else(|| ".".to_string()),
                busy,
            )
        };
        // Busy chats go through the doc's message queue — same rule as the
        // composer. Engines too old for QueueMessage fail loudly instead of
        // silently dropping the steer.
        if busy
            && !(engine
                .engine_info()
                .supports(capabilities::MESSAGE_QUEUE_V1)
                && self
                    .state
                    .read(cx)
                    .chat_host_supports(&chat_id, capabilities::MESSAGE_QUEUE_V1))
        {
            self.failure =
                Some("Update the chat's engine to queue messages during a response.".into());
            cx.notify();
            return;
        }
        let message_id = uuid::Uuid::new_v4().to_string();
        let echo = SessionMessageEntry {
            id: message_id.clone(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.clone(),
            }],
            created_at: now.timestamp_millis(),
            device_id: "local".into(),
            status: None,
            continuation_of: None,
        };
        self.state.update(cx, |s, _| {
            s.push_echo(&chat_id, echo);
            if !busy {
                // Working overlay until the host executes the command — the
                // watched doc adopts it by message id (`adopt_sub_echoes`).
                s.begin_pending_send(&chat_id, &message_id, now);
            }
        });
        self.sent.push((chat_id.clone(), message_id.clone()));
        self.input.update(cx, |input, cx| input.set_text("", cx));
        self.failure = None;
        self.failed_text = None;
        cx.notify();

        let typed = text.clone();
        let send_chat_id = chat_id.clone();
        let send_message_id = message_id.clone();
        let _ = window;
        cx.spawn(async move |this, cx| {
            let result: Result<(), String> = async {
                if busy {
                    let params = serde_json::json!({
                        "chatId": send_chat_id,
                        "text": text,
                        "attachments": [],
                        "holdForTurnEnd": true,
                    });
                    crate::attachments::call_with_timeout(
                        &engine,
                        cx.background_executor(),
                        methods::QUEUE_MESSAGE,
                        params,
                        std::time::Duration::from_secs(30),
                    )
                    .await
                    .map(|_| ())
                } else {
                    let command = SessionCommandPayload::Run {
                        request: RunRequest {
                            prompt: text,
                            harness: config.as_ref().map(|c| c.harness),
                            model: config.as_ref().and_then(|c| c.model.clone()),
                            reasoning: config.as_ref().and_then(|c| c.reasoning),
                            model_options: config
                                .as_ref()
                                .map(|c| c.model_options.clone())
                                .unwrap_or_default(),
                            cwd,
                            sandbox: config
                                .as_ref()
                                .map(|c| c.sandbox)
                                .unwrap_or(SandboxLevel::WorkspaceWrite),
                            auto_approve: false,
                            resume: None,
                            attachments: Vec::new(),
                            worktree: None,
                        },
                        message_id: send_message_id.clone(),
                    };
                    let command =
                        serde_json::to_value(&command).map_err(|e| format!("Send failed: {e}"))?;
                    let params = serde_json::json!({ "chatId": send_chat_id, "command": command });
                    crate::attachments::call_with_timeout(
                        &engine,
                        cx.background_executor(),
                        methods::QUEUE_COMMAND,
                        params,
                        std::time::Duration::from_secs(30),
                    )
                    .await
                    .map(|_| ())
                }
            }
            .await;
            this.update(cx, |this, cx| {
                if let Err(message) = result {
                    this.failure = Some(message.into());
                    this.failed_text = Some(typed);
                    this.state.update(cx, |s, _| {
                        s.remove_echo(&send_chat_id, &send_message_id);
                        s.end_pending_send(&send_chat_id, &send_message_id);
                    });
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Tab closed: drop the doc watch (the shell releases it once no surface
    /// still holds it) and retire any echoes this surface pushed.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        for (chat_id, message_id) in self.sent.drain(..) {
            self.state.update(cx, |s, _| {
                s.remove_echo(&chat_id, &message_id);
                s.end_pending_send(&chat_id, &message_id);
            });
        }
        self.transcript = None;
        self.chat_id = None;
    }

    fn close_switcher(&mut self, cx: &mut Context<Self>) {
        if self.switcher.begin_close() {
            popover::reap_popup(cx, |this: &mut Self| &mut this.switcher);
        }
    }

    /// The conversation-switcher dropdown: recent chats newest-first.
    fn render_switcher_menu(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut chats: Vec<zeron_proto::Chat> =
            self.state.read(cx).visible_chats().cloned().collect();
        chats.sort_by(|a, b| {
            (b.last_message_at.unwrap_or(b.created_at))
                .cmp(&a.last_message_at.unwrap_or(a.created_at))
        });
        chats.truncate(SWITCHER_MENU_ROWS);
        let bound = self.chat_id.clone();
        let mut list = div().flex().flex_col().gap(px(2.0));
        for chat in chats {
            let title = chat
                .title
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| "Untitled".to_string());
            let active = bound.as_deref() == Some(chat.id.as_str());
            let chat_id = chat.id.clone();
            list = list.child(
                popover::menu_row(theme, active, format!("side-chat-switch-{}", chat.id))
                    .id(SharedString::from(format!(
                        "side-chat-switch-row-{}",
                        chat.id
                    )))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.close_switcher(cx);
                        let previous = this.bind(chat_id.clone(), cx);
                        if let Some(doc) = previous {
                            cx.emit(SideChatEvent::DocReleased { doc_id: doc });
                        }
                    }))
                    .child(
                        icon(icons::CHAT_ROUND_LINE)
                            .size(px(13.0))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(SharedString::from(title)),
                    ),
            );
        }
        popover::popover_card(theme)
            .w(px(240.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_switcher(cx)))
            .child(list)
    }

    fn render_empty(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.switcher.get().is_some();
        let mut trigger = div()
            .id("side-chat-pick")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(14.0))
            .py(px(8.0))
            .rounded(px(10.0))
            .cursor_pointer()
            .bg(motion::hover_blend(
                "side-chat-pick-fade",
                theme::wash(0.0),
                theme::wash(0.10),
            ))
            .on_hover(motion::hover_listener("side-chat-pick-fade"))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, _| {
                    window.prevent_default();
                    this.switcher.note_trigger_press();
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                if this.switcher.take_press_was_open() {
                    this.close_switcher(cx);
                } else {
                    this.switcher.open(());
                    cx.notify();
                }
            }))
            .child(
                icon(icons::CHAT_ROUND_LINE)
                    .size(px(14.0))
                    .text_color(theme.text_muted),
            )
            .child(SharedString::from("Pick a conversation"));
        if open {
            let menu = self.render_switcher_menu(theme, cx).into_any_element();
            trigger = trigger.relative().child(popover::anchored_menu_below_gap(
                "side-chat-switcher",
                menu,
                self.switcher.closing_since(),
                6.0,
            ));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .child(
                icon(icons::CHAT_ROUND_LINE)
                    .size(px(28.0))
                    .text_color(theme.text_muted.opacity(0.5)),
            )
            .child(
                div()
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child("Chat with any conversation from here"),
            )
            .child(trigger)
    }
}

/// The shell listens: released docs get unwatched once no surface holds them.
#[derive(Debug, Clone)]
pub enum SideChatEvent {
    DocReleased { doc_id: String },
}

impl gpui::EventEmitter<SideChatEvent> for SideChat {}

impl Focusable for SideChat {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for SideChat {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let Some(chat_id) = self.chat_id.clone() else {
            return div()
                .size_full()
                .child(self.render_empty(&theme, cx))
                .into_any_element();
        };
        let (title, indicator) = {
            let s = self.state.read(cx);
            let title = s
                .chat_row(&chat_id)
                .and_then(|c| c.title.clone())
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| "Untitled".to_string());
            (title, s.indicator_for(&chat_id, chrono::Utc::now()))
        };
        let dot = match indicator {
            Indicator::Working | Indicator::AwaitingInput => Some(theme.accent),
            Indicator::Errored => Some(theme.danger),
            Indicator::None => None,
        };
        let has_text = !self.input.read(cx).text().trim().is_empty();
        let open = self.switcher.get().is_some();
        // The conversation switcher lives on the header title itself.
        let mut title_el = div()
            .id("side-chat-title")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .min_w_0()
            .flex_1()
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, window, _| {
                    window.prevent_default();
                    this.switcher.note_trigger_press();
                }),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                if this.switcher.take_press_was_open() {
                    this.close_switcher(cx);
                } else {
                    this.switcher.open(());
                    cx.notify();
                }
            }))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(crate::typography::ui_rems(12.5))
                    .text_color(theme.text)
                    .child(SharedString::from(title)),
            )
            .child(
                icon(icons::ALT_ARROW_DOWN)
                    .size(px(11.0))
                    .text_color(theme.text_muted),
            );
        if open {
            let menu = self.render_switcher_menu(&theme, cx).into_any_element();
            title_el = title_el.relative().child(popover::anchored_menu_below_gap(
                "side-chat-switcher",
                menu,
                self.switcher.closing_since(),
                6.0,
            ));
        }
        let header = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .pl(px(12.0))
            .pr(px(8.0))
            .py(px(7.0))
            .border_b_1()
            .border_color(theme::hairline(0.08))
            .children(dot.map(|c| div().size(px(6.0)).flex_none().rounded_full().bg(c)))
            .child(title_el)
            .child(
                // Jump the main column to this conversation.
                div()
                    .id("side-chat-open-main")
                    .size(px(22.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .bg(motion::hover_blend(
                        "side-chat-open-fade",
                        theme::wash(0.0),
                        theme::wash(0.11),
                    ))
                    .on_hover(motion::hover_listener("side-chat-open-fade"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(chat_id) = this.chat_id.clone() {
                            this.state.update(cx, |s, cx| {
                                s.select_chat(Some(chat_id), cx);
                            });
                        }
                    }))
                    .child(
                        icon(icons::ARROW_UP_RIGHT)
                            .size(px(12.0))
                            .text_color(theme.text_muted),
                    ),
            );
        let transcript = self
            .transcript
            .as_ref()
            .map(|t| div().flex_1().min_h_0().child(t.clone()).into_any_element())
            .unwrap_or_else(|| div().flex_1().min_h_0().into_any_element());
        let failure = self.failure.clone().map(|message| {
            div()
                .flex_none()
                .mx(px(8.0))
                .mb(px(4.0))
                .px(px(10.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .bg(theme.danger.opacity(0.12))
                .text_size(crate::typography::ui_rems(11.5))
                .text_color(theme.danger)
                .cursor_pointer()
                .id("side-chat-failure")
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(text) = this.failed_text.take() {
                        this.input.update(cx, |input, cx| input.set_text(text, cx));
                    }
                    this.failure = None;
                    cx.notify();
                }))
                .child(format!("{message} — click to restore"))
                .into_any_element()
        });
        let send_fade = "side-chat-send-fade";
        let input_bar = div()
            .flex_none()
            .m(px(8.0))
            .mt(px(0.0))
            .flex()
            .flex_row()
            .items_end()
            .gap(px(6.0))
            .pl(px(10.0))
            .pr(px(4.0))
            .py(px(4.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(theme::hairline(0.10))
            .bg(theme.input_glass_bg())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_h(px(120.0))
                    .overflow_hidden()
                    .child(self.input.clone()),
            )
            .child(
                div()
                    .id("side-chat-send")
                    .size(px(24.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(if has_text {
                        theme.accent
                    } else {
                        theme::wash(0.08)
                    })
                    .when(has_text, |b| {
                        b.bg(motion::hover_blend(
                            send_fade,
                            theme.accent,
                            theme.accent.opacity(0.85),
                        ))
                    })
                    .on_hover(motion::hover_listener(send_fade))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.send(window, cx);
                    }))
                    .child(
                        icon(icons::ARROW_UP)
                            .size(px(12.0))
                            .text_color(if has_text { theme.bg } else { theme.text_muted }),
                    ),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(|this, _: &Submit, window, cx| {
                this.send(window, cx);
            }))
            .child(header)
            .child(transcript)
            .children(failure)
            .child(input_bar)
            .into_any_element()
    }
}
