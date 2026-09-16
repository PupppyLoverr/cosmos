//! Device-local browser tabs. GPUI owns chrome; the native host owns pages.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
use linux as native;
#[cfg(target_os = "macos")]
use macos as native;
#[cfg(target_os = "windows")]
use windows as native;
pub mod bridge;
pub mod model;
mod view;

use crate::composer::{ComposerInput, ComposerInputEvent};
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Subscription, Window,
};
use model::{PageState, Presentation};
gpui::actions!(
    browser,
    [Reload, FocusAddress, NewTab, CloseTab, Back, Forward]
);

pub(crate) fn bind_keys(cx: &mut App, keymap: &crate::settings::KeymapConfig) {
    use crate::settings::{ShortcutId, platform_combo};
    // Existing customized app chords win. In particular old settings may
    // already assign mod-shift-r to the right pane.
    let available = |combo: &str, own: Option<ShortcutId>| {
        !combo.is_empty()
            && !ShortcutId::ALL.iter().any(|id| {
                Some(*id) != own
                    && gpui::Keystroke::parse(&platform_combo(keymap.get(*id))).ok()
                        == gpui::Keystroke::parse(&platform_combo(combo)).ok()
            })
    };
    macro_rules! bind {
        ($combo:literal, $action:ident) => {
            if available($combo, None) {
                cx.bind_keys([gpui::KeyBinding::new(
                    &platform_combo($combo),
                    $action,
                    Some("Browser"),
                )]);
            }
        };
    }
    bind!("mod-l", FocusAddress);
    bind!("mod-t", NewTab);
    bind!("mod-w", CloseTab);
    bind!("mod-[", Back);
    bind!("mod-]", Forward);
    let reload = keymap.get(ShortcutId::BrowserReload);
    if available(reload, Some(ShortcutId::BrowserReload))
        && gpui::Keystroke::parse(&platform_combo(reload)).is_ok()
    {
        cx.bind_keys([gpui::KeyBinding::new(
            &platform_combo(reload),
            Reload,
            Some("Browser"),
        )]);
    }
}

#[derive(Clone, Debug)]
pub enum BrowserEvent {
    Changed,
    NewTab(Option<String>),
    Close,
}

/// A window/profile's ephemeral website data, allocated on first navigation.
#[derive(Clone, Default)]
pub struct BrowserContext {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    data: native::BrowserData,
}

pub struct BrowserSurface {
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    context: BrowserContext,
    address: Entity<ComposerInput>,
    focus: FocusHandle,
    pub page: PageState,
    pub favicon: Option<std::sync::Arc<gpui::Image>>,
    address_edited: bool,
    validation: Option<String>,
    remote: bool,
    previews: zeron_proto::PreviewSnapshot,
    previews_loading: bool,
    previews_task: Option<gpui::Task<()>>,
    #[cfg(feature = "browser-fixture")]
    fixture_preview_open: std::rc::Rc<std::cell::Cell<Option<gpui::Point<gpui::Pixels>>>>,
    presentation: Presentation,
    #[cfg(target_os = "macos")]
    resize_inset: gpui::Pixels,
    _input_sub: Subscription,
    /// FIFO replies for `evaluate` — every platform answers evaluations in
    /// submission order, so a drained queue is enough to pair them up.
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    eval_waiters: std::collections::VecDeque<tokio::sync::oneshot::Sender<serde_json::Value>>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    shot_waiters: std::collections::VecDeque<tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>>,
    /// Element-pick → comment → screenshot → composer staging.
    annotate: Option<Annotate>,
    /// The shell's composer — where annotated screenshots land.
    composer: Option<gpui::WeakEntity<crate::composer::Composer>>,
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    native: Option<native::NativePage>,
    /// A WebView2 build in flight. `build_as_child` pumps the Win32
    /// message loop, so creation runs on a foreground task — gpui only
    /// polls those while the `App` cell is unborrowed, letting the nested
    /// loop dispatch queued work safely. The latest navigation, and any
    /// shortcuts pushed mid-build, replay when the page lands.
    #[cfg(target_os = "windows")]
    pending_native: Option<PendingNative>,
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    native_tx: tokio::sync::mpsc::Sender<native::NativeEvent>,
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    _native_task: gpui::Task<()>,
    #[cfg(target_os = "macos")]
    favicon_task: Option<gpui::Task<()>>,
    #[cfg(target_os = "macos")]
    favicon_generation: u64,
}

impl EventEmitter<BrowserEvent> for BrowserSurface {}
impl Focusable for BrowserSurface {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl BrowserSurface {
    pub fn new(
        context: BrowserContext,
        remote: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let address = cx.new(|cx| {
            ComposerInput::with_context("Website or localhost:3000", "PaletteSearch", cx)
                .with_text_metrics(11.0, 16.0)
                .with_single_line()
                .with_accessibility_role(gpui::Role::TextInput)
        });
        let input_sub = cx.subscribe(&address, |this, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                this.address_edited =
                    this.address.read(cx).text() != this.page.url.as_deref().unwrap_or_default();
                this.validation = None;
                cx.notify();
            }
        });
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        let (native_tx, mut events) = tokio::sync::mpsc::channel(64);
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        let native_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this
                    .update_in(cx, |this, window, cx| {
                        this.on_native_event(event, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        let _ = (window, context);
        Self {
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            context,
            address,
            focus: cx.focus_handle(),
            page: PageState::default(),
            favicon: None,
            address_edited: false,
            validation: None,
            remote,
            previews: zeron_proto::PreviewSnapshot::default(),
            previews_loading: true,
            previews_task: None,
            #[cfg(feature = "browser-fixture")]
            fixture_preview_open: Default::default(),
            presentation: Presentation::Hidden,
            #[cfg(target_os = "macos")]
            resize_inset: gpui::px(0.0),
            _input_sub: input_sub,
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            eval_waiters: Default::default(),
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            shot_waiters: Default::default(),
            annotate: None,
            composer: None,
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            native: None,
            #[cfg(target_os = "windows")]
            pending_native: None,
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            native_tx,
            #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
            _native_task: native_task,
            #[cfg(target_os = "macos")]
            favicon_task: None,
            #[cfg(target_os = "macos")]
            favicon_generation: 0,
        }
    }

    pub fn title(&self) -> gpui::SharedString {
        self.page.label().into()
    }
    pub fn set_remote(&mut self, remote: bool) {
        self.remote = remote;
    }
    pub fn set_shortcuts(&mut self, keymap: &crate::settings::KeymapConfig) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let shortcuts: Vec<String> = crate::settings::ShortcutId::ALL
                .iter()
                .filter(|id| **id != crate::settings::ShortcutId::SaveFile)
                .map(|id| crate::settings::platform_combo(keymap.get(*id)))
                .collect();
            // An in-flight WebView2 build applies them when the page lands.
            #[cfg(target_os = "windows")]
            if let Some(pending) = &mut self.pending_native {
                pending.shortcuts = Some(shortcuts.clone());
            }
            if let Some(native) = &self.native {
                native.set_shortcuts(shortcuts);
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let _ = keymap;
    }

    pub fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(native) = &self.native {
            native.focus_chrome();
        }
        window.focus(&self.address.focus_handle(cx), cx);
        window.dispatch_action(Box::new(crate::composer::SelectAll), cx);
    }

    /// Reserve the overlapping part of the shell divider for GPUI hit testing.
    #[cfg(target_os = "macos")]
    pub fn set_resize_inset(&mut self, inset: gpui::Pixels, cx: &mut Context<Self>) {
        if self.resize_inset != inset {
            self.resize_inset = inset;
            cx.notify();
        }
    }

    pub fn set_presentation(&mut self, presentation: Presentation, cx: &mut Context<Self>) {
        if self.presentation == presentation {
            return;
        }
        self.presentation = presentation;
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        if let Some(native) = &mut self.native {
            native.present(presentation);
        }
        cx.notify();
    }

    #[cfg(feature = "browser-fixture")]
    pub fn fixture_previews(&self) -> zeron_proto::PreviewSnapshot {
        self.previews.clone()
    }
    #[cfg(feature = "browser-fixture")]
    pub fn fixture_preview_open_position(&self) -> Option<gpui::Point<gpui::Pixels>> {
        self.fixture_preview_open.get()
    }

    /// The daemon resolves the session's current cwd for every update, so a
    /// checkout change cannot leave this tab discovering the previous project.
    pub fn watch_previews(
        &mut self,
        handle: crate::state::EngineHandle,
        chat_id: String,
        cx: &mut Context<Self>,
    ) {
        self.previews_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let subscription = handle
                    .client()
                    .subscribe(
                        zeron_rpc::methods::WATCH_PREVIEWS,
                        serde_json::json!({"chatId": chat_id}),
                    )
                    .await;
                if let Ok(mut updates) = subscription {
                    while let Some(value) = updates.recv().await {
                        if let Ok(snapshot) =
                            serde_json::from_value::<zeron_proto::PreviewSnapshot>(value)
                        {
                            if this
                                .update(cx, |this, cx| {
                                    #[cfg(target_os = "macos")]
                                    for service in &snapshot.services {
                                        this.context
                                            .data
                                            .register_preview(&service.url(snapshot.proxy_port));
                                    }
                                    this.previews = snapshot;
                                    this.previews_loading = false;
                                    cx.notify();
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
                if this
                    .update(cx, |this, cx| {
                        this.previews.services.clear();
                        this.previews.error = Some("Connecting to preview discovery…".into());
                        this.previews_loading = false;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
            }
        }));
    }

    pub fn navigate(&mut self, input: &str, window: &mut Window, cx: &mut Context<Self>) {
        let url = match model::normalize_address(input) {
            Ok(url) => url,
            Err(message) => {
                self.validation = Some(message.into());
                cx.notify();
                return;
            }
        };
        self.validation = None;
        self.address
            .update(cx, |input, cx| input.set_text(url.clone(), cx));
        self.address_edited = false;
        self.page.url = Some(url.clone());
        self.page.title.clear();
        self.page.error = None;
        self.clear_favicon(cx);
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            #[cfg(target_os = "macos")]
            {
                self.favicon_generation += 1;
                self.favicon_task = None;
            }
            let result = if let Some(native) = &self.native {
                native.load(&url)
            } else {
                self.create_native(&url, window, cx)
            };
            self.page.loading = result.is_ok();
            if let Err(error) = result {
                self.page.error = Some(format!("Could not open this page: {error}"));
            }
            window.focus(&self.focus, cx);
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = window;
            cx.open_url(&url);
        }
        cx.emit(BrowserEvent::Changed);
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.address.read(cx).text().to_owned();
        self.navigate(&url, window, cx);
    }

    /// Attach the native page and start `url` — synchronous here: neither
    /// platform pumps the window's message loop while building, so running
    /// inside the current `App` borrow is safe.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn create_native(
        &mut self,
        url: &str,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Result<(), String> {
        native::NativePage::new(window, &self.context.data, self.native_tx.clone())
            .map(|mut native| {
                native.present(self.presentation);
                self.native = Some(native);
            })
            .and_then(|_| self.native.as_ref().unwrap().load(url))
    }

    /// WebView2's `build_as_child` pumps the Win32 message loop while the
    /// environment and controller come up. Run under the `App` borrow an
    /// event or update callback holds, that pump dispatches queued gpui
    /// tasks — which then panic on the second `AppCell` borrow and abort
    /// across the `CallWindowProcW` boundary. The build therefore runs on
    /// a foreground task, which gpui only polls with the cell unborrowed;
    /// the page lands back in context and replays the pending target.
    #[cfg(target_os = "windows")]
    fn create_native(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if let Some(pending) = &mut self.pending_native {
            pending.url = url.to_owned();
            return Ok(());
        }
        let parent = native::ParentWindow::of(window)?;
        let data = self.context.data.clone();
        let tx = self.native_tx.clone();
        self.pending_native = Some(PendingNative {
            url: url.to_owned(),
            shortcuts: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            // The task body runs with no `App` borrow held, so the message
            // pump inside `NativePage::new` is free to dispatch queued gpui
            // work — including this entity's own event handlers — without
            // re-entering a borrowed `AppCell`.
            let created = native::NativePage::new(&parent, &data, tx);
            let _ = this.update_in(cx, |this, _, cx| {
                if let Some(pending) = this.pending_native.take() {
                    match created {
                        Ok(mut native) => {
                            native.present(this.presentation);
                            if let Some(shortcuts) = pending.shortcuts {
                                native.set_shortcuts(shortcuts);
                            }
                            let result = native.load(&pending.url);
                            this.page.loading = result.is_ok();
                            if let Err(error) = result {
                                this.page.error =
                                    Some(format!("Could not open this page: {error}"));
                            }
                            this.native = Some(native);
                        }
                        Err(error) => {
                            this.page.loading = false;
                            this.page.error =
                                Some(format!("Could not open this page: {error}"));
                        }
                    }
                } else {
                    // `close` ran while the host was building — the page
                    // drops rather than attaching to a closed surface.
                    this.page.loading = false;
                }
                cx.emit(BrowserEvent::Changed);
                cx.notify();
            });
        })
        .detach();
        Ok(())
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        if let Some(native) = &self.native {
            if self.page.error.is_some() {
                if let Some(url) = &self.page.url {
                    let _ = native.load(url);
                }
            } else {
                native.reload();
            }
            self.page.error = None;
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        self.open_external(cx);
        cx.notify();
    }

    fn open_external(&self, cx: &mut Context<Self>) {
        if let Some(url) = self
            .page
            .url
            .as_deref()
            .filter(|url| model::allowed_navigation(url))
        {
            cx.open_url(url);
        }
    }

    pub fn history(&mut self, forward: bool) {
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        if let Some(native) = &self.native {
            native.history(forward);
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        let _ = forward;
    }

    pub fn set_composer(&mut self, composer: gpui::WeakEntity<crate::composer::Composer>) {
        self.composer = Some(composer);
    }

    /// Run a JavaScript expression in the live page and answer with its JSON
    /// value. The reply resolves `{ok:true, v:<result>}` or
    /// `{ok:false, e:<error>}` — every platform funnels the return value
    /// through a JSON string so types survive the native boundary.
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    pub fn evaluate(
        &mut self,
        expression: &str,
        reply: tokio::sync::oneshot::Sender<serde_json::Value>,
    ) {
        let Some(native) = &self.native else {
            let _ = reply.send(serde_json::json!({"ok":false,"e":"No page is loaded"}));
            return;
        };
        self.eval_waiters.push_back(reply);
        let script = format!(
            "(()=>{{try{{var v=({expression});return JSON.stringify({{ok:true,v:v===undefined?null:v}})}}catch(e){{return JSON.stringify({{ok:false,e:String(e)}})}}}})()"
        );
        native.evaluate(&script);
    }

    /// Evaluate without waiting for the result (teardown hooks, labels).
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn evaluate_detached(&mut self, expression: &str) {
        let (tx, _rx) = tokio::sync::oneshot::channel();
        self.evaluate(expression, tx);
    }

    /// PNG bytes of the visible page. macOS/Windows snapshot the native
    /// view; Linux re-encodes the latest composited frame.
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    pub fn screenshot(&mut self, reply: tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>) {
        #[cfg(target_os = "linux")]
        {
            let result = self
                .native
                .as_ref()
                .ok_or_else(|| "No page is loaded".to_string())
                .and_then(|native| native.screenshot());
            let _ = reply.send(result);
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let Some(native) = &self.native else {
                let _ = reply.send(Err("No page is loaded".into()));
                return;
            };
            self.shot_waiters.push_back(reply);
            native.screenshot();
        }
    }

    /// Platforms without an embedded page answer eval/screenshot with a
    /// clear error so bridge callers degrade instead of hanging.
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    pub fn evaluate(
        &mut self,
        _expression: &str,
        reply: tokio::sync::oneshot::Sender<serde_json::Value>,
    ) {
        let _ = reply.send(
            serde_json::json!({"ok":false,"e":"Embedded browser is not available on this platform"}),
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    pub fn screenshot(&mut self, reply: tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>) {
        let _ = reply.send(Err(
            "Embedded browser is not available on this platform".into()
        ));
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn handle_eval_result(&mut self, value: serde_json::Value) {
        if let Some(reply) = self.eval_waiters.pop_front() {
            let _ = reply.send(value);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn handle_screenshot(&mut self, result: Result<Vec<u8>, String>) {
        if let Some(reply) = self.shot_waiters.pop_front() {
            let _ = reply.send(result);
        }
    }

    /// A navigation replaces the document: any injected overlay and pick
    /// state are gone, so annotate mode ends with the page it ran on.
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn annotate_on_navigation(&mut self, cx: &mut Context<Self>) {
        if self.annotate.is_some() {
            self.annotate_cancel(cx);
        }
    }

    /// Toggle element-annotate mode: the page gets a hover highlight; a
    /// click freezes the pick and opens the comment bar; Enter captures a
    /// screenshot (highlight + comment included) and stages it — plus a
    /// note naming the element — into the composer.
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    pub fn toggle_annotate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.annotate.is_some() {
            self.annotate_cancel(cx);
            return;
        }
        if self.native.is_none() || self.page.url.is_none() {
            return;
        }
        let input = cx.new(|cx| {
            ComposerInput::with_context(
                "Comment on this element — Enter captures",
                "PaletteSearch",
                cx,
            )
            .with_text_metrics(11.0, 16.0)
            .with_single_line()
        });
        self.evaluate_detached(ANNOTATE_ENTER);
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                let (tx, rx) = tokio::sync::oneshot::channel();
                let Ok(()) = this.update_in(cx, |this, _, _| {
                    this.evaluate("({p:window.__z9pick||null,e:window.__z9esc===true})", tx);
                }) else {
                    return;
                };
                use futures::FutureExt;
                let result = futures::select! {
                    r = rx.fuse() => r.ok(),
                    // A dropped native reply would otherwise stall the poll
                    // forever; two seconds is far past any real evaluation.
                    _ = cx
                        .background_executor()
                        .timer(std::time::Duration::from_secs(2))
                        .fuse() => None,
                };
                let Some(serde_json::Value::Object(envelope)) = result else {
                    continue;
                };
                let Some(value) = envelope.get("v") else {
                    continue;
                };
                if value.get("e").and_then(|e| e.as_bool()).unwrap_or(false) {
                    let _ = this.update_in(cx, |this, _, cx| this.annotate_cancel(cx));
                    return;
                }
                if let Some(pick) = value.get("p").filter(|p| p.is_object()) {
                    if this
                        .update_in(cx, |this, window, cx| {
                            this.annotate_pick(pick.clone(), window, cx)
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                let still = this
                    .update_in(cx, |this, _, _| this.annotate.is_some())
                    .unwrap_or(false);
                if !still {
                    return;
                }
            }
        });
        self.annotate = Some(Annotate {
            input,
            picked: None,
            _poll: poll,
        });
        cx.notify();
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn annotate_pick(
        &mut self,
        pick: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(annotate) = &mut self.annotate else {
            return;
        };
        annotate.picked = Some(pick);
        let focus = annotate.input.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Drop the injected overlay and close the comment bar without staging.
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    pub fn annotate_cancel(&mut self, cx: &mut Context<Self>) {
        if self.annotate.take().is_some() {
            self.evaluate_detached(ANNOTATE_EXIT);
            cx.notify();
        }
    }

    /// Screenshot the page with the highlight and the comment label still
    /// on it, then stage the PNG and a note naming the element into the
    /// composer so the agent sees both the picture and the pointer.
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    fn annotate_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(annotate) = &mut self.annotate else {
            return;
        };
        let comment = annotate.input.read(cx).text().trim().to_string();
        let Some(picked) = annotate.picked.clone() else {
            // No element picked yet — keep the mode, keep the comment draft.
            return;
        };
        let selector = picked
            .get("selector")
            .and_then(|s| s.as_str())
            .unwrap_or("element")
            .to_string();
        let element_text = picked
            .get("text")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .chars()
            .take(160)
            .collect::<String>();
        let page_url = self.page.url.clone().unwrap_or_default();
        // Render the comment onto the page next to the highlight so the
        // capture itself carries the user's words.
        self.evaluate_detached(&format!(
            "window.__z9label&&window.__z9label({})",
            serde_json::json!(comment)
        ));
        let composer = self.composer.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.screenshot(tx);
        let selector_note = selector.clone();
        cx.spawn_in(window, async move |this, cx| {
            // Give the label paint a beat before the snapshot lands.
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            use futures::FutureExt;
            let bytes = futures::select! {
                r = rx.fuse() => r.ok(),
                _ = cx
                    .background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .fuse() => None,
            };
            let _ = this.update_in(cx, |this, _, cx| {
                this.annotate_cancel(cx);
            });
            let Some(Ok(png)) = bytes else {
                return;
            };
            let note = if comment.is_empty() {
                format!("Annotated element `{selector_note}` on {page_url}: {element_text}")
            } else {
                format!("{comment}\n\nAnnotated `{selector_note}` on {page_url}: {element_text}")
            };
            let Some(composer) = composer.and_then(|c| c.upgrade()) else {
                return;
            };
            let _ = cx.update(|_, cx| {
                composer.update(cx, |composer, cx| {
                    composer.stage_browser_capture(note, png, cx);
                })
            });
        })
        .detach();
    }

    /// Explicitly close even if an async callback temporarily retains an entity.
    fn clear_favicon(&mut self, cx: &mut Context<Self>) {
        if let Some(image) = self.favicon.take() {
            cx.defer(move |cx| gpui::ImageSource::Image(image).evict(None, cx));
        }
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.set_presentation(Presentation::Hidden, cx);
        self.clear_favicon(cx);
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        {
            if let Some(native) = &mut self.native {
                native.present(Presentation::Hidden);
            }
            #[cfg(target_os = "linux")]
            if let Some(image) = self.native.as_mut().and_then(|native| native.image.take()) {
                cx.defer(move |cx| gpui::ImageSource::Render(image).evict(None, cx));
            }
            self.native = None;
            // Cancels an in-flight WebView2 build: it drops on landing
            // instead of attaching to a closed surface.
            #[cfg(target_os = "windows")]
            {
                self.pending_native = None;
            }
            #[cfg(target_os = "macos")]
            {
                self.favicon_task = None;
                self.favicon_generation += 1;
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl BrowserSurface {
    fn on_native_event(
        &mut self,
        event: native::NativeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(native) = &mut self.native else {
            return;
        };
        match event {
            native::NativeEvent::Changed | native::NativeEvent::Finished => {
                let finished = matches!(event, native::NativeEvent::Finished);
                let mut page = native.state();
                // A failed provisional request has no committed WebKit URL.
                if page.url.is_none() {
                    page.url = self.page.url.clone();
                }
                if page.error.is_some() {
                    page.loading = false;
                }
                if page.url != self.page.url || (!self.page.loading && page.loading) {
                    if let Some(image) = self.favicon.take() {
                        cx.defer(move |cx| gpui::ImageSource::Image(image).evict(None, cx));
                    }
                    self.favicon_generation += 1;
                    self.favicon_task = None;
                }
                if !self.address.focus_handle(cx).is_focused(window) {
                    if let Some(url) = &page.url {
                        if self.address.read(cx).text() != url {
                            self.address
                                .update(cx, |input, cx| input.set_text(url.clone(), cx));
                        }
                    }
                    self.address_edited = false;
                }
                native.present(self.presentation);
                if finished && let Some(url) = &page.url {
                    native.discover_favicon(url.clone());
                }
                if page.url != self.page.url {
                    self.annotate_on_navigation(cx);
                }
                if page != self.page {
                    self.page = page;
                    cx.emit(BrowserEvent::Changed);
                }
                cx.notify();
            }
            native::NativeEvent::NewTab(url) => {
                if self.presentation == Presentation::Live {
                    cx.emit(BrowserEvent::NewTab(Some(url)));
                }
            }
            native::NativeEvent::Key(key) => {
                if self.presentation == Presentation::Live {
                    window.focus(&self.focus, cx);
                    window.defer(cx, move |window, cx| {
                        window.dispatch_keystroke(key, cx);
                    });
                }
            }
            native::NativeEvent::EvalResult(value) => self.handle_eval_result(value),
            native::NativeEvent::Screenshot(result) => self.handle_screenshot(result),
            native::NativeEvent::Favicon { page, url } => {
                if self.page.url.as_deref() != Some(&page) || !model::allowed_navigation(&url) {
                    return;
                }
                let generation = self.favicon_generation;
                let download = gpui_tokio::Tokio::spawn(cx, async move {
                    let client = reqwest::Client::builder()
                        .timeout(std::time::Duration::from_secs(5))
                        .redirect(reqwest::redirect::Policy::limited(3))
                        .build()
                        .ok()?;
                    let mut response =
                        client.get(url).send().await.ok()?.error_for_status().ok()?;
                    if response.content_length().is_some_and(|n| n > 1024 * 1024) {
                        return None;
                    }
                    let mut bytes = Vec::new();
                    while let Some(chunk) = response.chunk().await.ok()? {
                        if bytes.len() + chunk.len() > 1024 * 1024 {
                            return None;
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                        .with_guessed_format()
                        .ok()?;
                    let mut limits = image::Limits::default();
                    limits.max_image_width = Some(1024);
                    limits.max_image_height = Some(1024);
                    limits.max_alloc = Some(8 * 1024 * 1024);
                    reader.limits(limits);
                    let icon = reader.decode().ok()?.thumbnail(32, 32);
                    let mut png = std::io::Cursor::new(Vec::new());
                    icon.write_to(&mut png, image::ImageFormat::Png).ok()?;
                    Some(png.into_inner())
                });
                self.favicon_task = Some(cx.spawn(async move |this, cx| {
                    let Ok(Some(bytes)) = download.await else {
                        return;
                    };
                    let _ = this.update(cx, |this, cx| {
                        if this.favicon_generation == generation
                            && this.page.url.as_deref() == Some(&page)
                        {
                            this.favicon = Some(std::sync::Arc::new(gpui::Image::from_bytes(
                                gpui::ImageFormat::Png,
                                bytes,
                            )));
                            cx.emit(BrowserEvent::Changed);
                            cx.notify();
                        }
                    });
                }));
            }
        }
    }
}

#[cfg(target_os = "windows")]
impl BrowserSurface {
    fn on_native_event(
        &mut self,
        event: native::NativeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(native) = &mut self.native else {
            return;
        };
        match event {
            native::NativeEvent::Changed | native::NativeEvent::Finished => {
                let mut page = native.state();
                // A failed provisional request has no committed WebView2 URL.
                if page.url.is_none() {
                    page.url = self.page.url.clone();
                }
                if page.error.is_some() {
                    page.loading = false;
                }
                if !self.address.focus_handle(cx).is_focused(window) {
                    if let Some(url) = &page.url {
                        if self.address.read(cx).text() != url {
                            self.address
                                .update(cx, |input, cx| input.set_text(url.clone(), cx));
                        }
                    }
                    self.address_edited = false;
                }
                native.present(self.presentation);
                if page.url != self.page.url {
                    self.annotate_on_navigation(cx);
                }
                if page != self.page {
                    self.page = page;
                    cx.emit(BrowserEvent::Changed);
                }
                cx.notify();
            }
            native::NativeEvent::NewTab(url) => {
                if self.presentation == Presentation::Live {
                    cx.emit(BrowserEvent::NewTab(Some(url)));
                }
            }
            native::NativeEvent::Key(key) => {
                if self.presentation == Presentation::Live {
                    window.focus(&self.focus, cx);
                    window.defer(cx, move |window, cx| {
                        window.dispatch_keystroke(key, cx);
                    });
                }
            }
            native::NativeEvent::EvalResult(value) => self.handle_eval_result(value),
            native::NativeEvent::Screenshot(result) => self.handle_screenshot(result),
        }
    }
}

/// Element-annotate session state: the comment field, the picked element,
/// and the pick-poll task that ends when annotate mode does.
struct Annotate {
    input: Entity<ComposerInput>,
    picked: Option<serde_json::Value>,
    _poll: gpui::Task<()>,
}

/// State for an in-flight WebView2 build; see the `pending_native` field
/// for why creation is deferred to a foreground task.
#[cfg(target_os = "windows")]
struct PendingNative {
    /// Latest navigation target — re-navigating mid-build just retargets.
    url: String,
    /// App chords pushed while the build was in flight.
    shortcuts: Option<Vec<String>>,
}

/// Every platform returns the evaluated script's *string* result — the
/// caller-side wrapper JSON-encodes the real value into that string, so the
/// native boundary only ever ferries text. Windows and Linux then JSON-wrap
/// the string once more (wry's callback / `jsc_value_to_json`), which is why
/// decoding unwraps string layers until a structured value appears.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn decode_eval(raw: &str) -> serde_json::Value {
    let mut value = serde_json::from_str::<serde_json::Value>(raw)
        .unwrap_or_else(|_| serde_json::Value::String(raw.to_string()));
    while let serde_json::Value::String(inner) = &value {
        match serde_json::from_str::<serde_json::Value>(inner) {
            Ok(deeper) => value = deeper,
            Err(_) => break,
        }
    }
    value
}

/// Injected element picker. Kept name-scoped under `__z9*` so page globals
/// never collide. `__z9pick` carries the last pick (null until clicked),
/// `__z9esc` reports an in-page Escape, `__z9label` rewrites the comment
/// chip before capture, and `__z9off` removes everything.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
const ANNOTATE_ENTER: &str = r#"(() => {
  if (window.__z9a) return true;
  window.__z9a = true; window.__z9pick = null; window.__z9esc = false;
  const box = document.createElement('div');
  box.style.cssText = 'position:fixed;z-index:2147483646;pointer-events:none;border:2px solid #4f8cff;background:rgba(79,140,255,0.12);border-radius:3px;display:none;box-sizing:border-box';
  const tip = document.createElement('div');
  tip.style.cssText = 'position:fixed;z-index:2147483647;pointer-events:none;background:#1c1f26;color:#e6e8ee;font:11px/1.5 system-ui,sans-serif;padding:3px 8px;border-radius:5px;display:none;max-width:70vw;white-space:pre-wrap;box-shadow:0 2px 10px rgba(0,0,0,.35)';
  document.documentElement.append(box, tip);
  let el = null;
  const name = t => t.tagName.toLowerCase() + (t.id ? '#' + t.id : (t.className && typeof t.className === 'string' ? '.' + t.className.trim().split(/\s+/).slice(0, 3).join('.') : ''));
  const place = t => {
    const r = t.getBoundingClientRect();
    Object.assign(box.style, { display: 'block', left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px' });
    Object.assign(tip.style, { display: 'block', left: Math.min(r.left, innerWidth - 180) + 'px', top: Math.max(4, r.top - 30) + 'px' });
  };
  const pick = t => {
    const r = t.getBoundingClientRect();
    return { selector: name(t), tag: t.tagName.toLowerCase(), id: t.id || null,
      text: (t.innerText || t.value || t.alt || '').trim().slice(0, 300),
      rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) } };
  };
  const move = e => { el = e.target; if (el instanceof Element) { place(el); tip.textContent = name(el); } };
  const eat = e => { e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation(); };
  const click = e => { eat(e); if (el) { window.__z9pick = pick(el); place(el); tip.textContent = name(el); } };
  const key = e => { if (e.key === 'Escape') { eat(e); window.__z9esc = true; } };
  window.__z9label = c => { if (el) { tip.textContent = name(el) + (c ? ' — ' + c : ''); } };
  window.__z9off = () => {
    removeEventListener('mousemove', move, true);
    removeEventListener('click', click, true);
    removeEventListener('keydown', key, true);
    for (const t of ['pointerdown', 'pointerup', 'mousedown', 'mouseup', 'dblclick', 'submit']) removeEventListener(t, eat, true);
    box.remove(); tip.remove();
    window.__z9a = false; window.__z9pick = null; window.__z9esc = false;
  };
  addEventListener('mousemove', move, true);
  addEventListener('click', click, true);
  addEventListener('keydown', key, true);
  for (const t of ['pointerdown', 'pointerup', 'mousedown', 'mouseup', 'dblclick', 'submit']) addEventListener(t, eat, true);
  return true;
})()"#;

/// Removes the picker overlay and resets the pick globals.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
const ANNOTATE_EXIT: &str = "(window.__z9off&&window.__z9off(),true)";

#[cfg(feature = "browser-fixture")]
impl BrowserSurface {
    pub fn fixture_history(&mut self, forward: bool) {
        self.history(forward);
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_move_cursor(&self, x: f64, y: f64) {
        self.native.as_ref().unwrap().fixture_move_cursor(x, y);
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_origin(&self) -> (f32, f32) {
        self.native.as_ref().unwrap().fixture_origin()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_overlay_visible(&self) -> bool {
        self.native.as_ref().unwrap().fixture_overlay_visible()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_visibility_changes(&self) -> u64 {
        self.native.as_ref().unwrap().fixture_visibility_changes()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_focus(&self) {
        self.native.as_ref().unwrap().fixture_focus();
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_focused(&self) -> bool {
        self.native.as_ref().unwrap().fixture_focused()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_overlay_at(&self, x: f64, y: f64) -> bool {
        self.native.as_ref().unwrap().fixture_overlay_at(x, y)
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_click(&self, x: f64, y: f64) {
        self.native.as_ref().unwrap().fixture_click(x, y);
    }
    pub fn fixture_native_visible(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.native
                .as_ref()
                .is_some_and(|native| native.fixture_visible())
        }
        #[cfg(target_os = "linux")]
        {
            self.presentation != Presentation::Hidden
                && self.native.as_ref().is_some_and(|n| n.image.is_some())
        }
        #[cfg(target_os = "windows")]
        {
            self.presentation != Presentation::Hidden && self.native.is_some()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            false
        }
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_backdrop_layers(&self) -> String {
        self.native.as_ref().unwrap().fixture_backdrop_layers()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_page_hit(&self, x: f64, y: f64) -> bool {
        self.native.as_ref().unwrap().fixture_page_hit(x, y)
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_backdrops(&self) -> Vec<(f64, f64, f64, f64)> {
        self.native.as_ref().unwrap().fixture_backdrops()
    }
    #[cfg(target_os = "macos")]
    pub fn fixture_geometry(&self) -> (f32, f32, f32) {
        self.native.as_ref().unwrap().fixture_geometry()
    }
    #[cfg(target_os = "windows")]
    pub fn fixture_host_info(&self) -> String {
        self.native
            .as_ref()
            .map(|native| native.host_info())
            .unwrap_or_else(|| "no native page".into())
    }
    pub fn fixture_eval(&self, script: &str) {
        #[cfg(target_os = "macos")]
        if let Some(native) = &self.native {
            native.fixture_eval(script);
        }
        #[cfg(target_os = "linux")]
        if let Some(native) = &self.native {
            native.evaluate(script);
        }
        #[cfg(target_os = "windows")]
        if let Some(native) = &self.native {
            native.evaluate(script);
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        let _ = script;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn address_enter_rejects_unsupported_schemes_and_escape_restores_url(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_base::init(cx);
            crate::composer::init(cx, Default::default());
            cx.set_global(crate::theme::Theme::default());
            bind_keys(cx, &crate::settings::KeymapConfig::default());
        });
        let window = cx.add_window(|window, cx| {
            BrowserSurface::new(BrowserContext::default(), false, window, cx)
        });
        window
            .update(cx, |browser, window, cx| {
                browser.page.url = Some("https://example.com/".into());
                browser
                    .address
                    .update(cx, |input, cx| input.set_text("javascript:alert(1)", cx));
                browser.focus_address(window, cx);
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "enter");
        window
            .update(cx, |browser, _, _| {
                assert!(
                    browser.validation.is_some(),
                    "Enter did not reach browser address handling"
                );
                assert_eq!(browser.page.url.as_deref(), Some("https://example.com/"));
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "escape");
        window
            .update(cx, |browser, window, cx| {
                assert!(browser.validation.is_none());
                assert_eq!(browser.address.read(cx).text(), "https://example.com/");
                assert!(browser.focus.is_focused(window));
                window.blur();
            })
            .unwrap();
    }

    #[gpui::test]
    fn customized_app_shortcuts_win_over_browser_defaults(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let mut config = crate::settings::KeymapConfig::default();
            bind_keys(cx, &config);
            assert_eq!(
                cx.key_bindings()
                    .borrow()
                    .bindings_for_action(&FocusAddress)
                    .count(),
                1
            );
            assert_eq!(
                cx.key_bindings()
                    .borrow()
                    .bindings_for_action(&Reload)
                    .count(),
                1
            );
            cx.clear_key_bindings();
            config.toggle_sidebar = crate::settings::platform_combo("mod-l");
            config.toggle_changes = "mod-shift-r".into();
            bind_keys(cx, &config);
            assert_eq!(
                cx.key_bindings()
                    .borrow()
                    .bindings_for_action(&FocusAddress)
                    .count(),
                0
            );
            assert_eq!(
                cx.key_bindings()
                    .borrow()
                    .bindings_for_action(&Reload)
                    .count(),
                0
            );
            assert_eq!(
                cx.key_bindings()
                    .borrow()
                    .bindings_for_action(&NewTab)
                    .count(),
                1
            );
        });
    }
}
