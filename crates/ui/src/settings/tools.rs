//! Settings → Tools: the Cosmos Tools gateway surface — Composio app
//! connections (one `ck_…` key reaches Gmail, Slack, Notion and the rest of
//! the Connect catalog), user-configured MCP servers (stdio children and
//! remote HTTP endpoints), and the Cosmos Computer summary.
//!
//! Tool configuration is DEVICE-LOCAL account state: every `*Tool*`/
//! `*Composio*` RPC is relay-forwardable, so the page carries the same
//! header device switcher as Accounts (`targetDeviceId` passthrough). The
//! Computer group is the exception — physical desktop control stays
//! device-local, so the summary only renders for "This device" and links
//! out to Settings → Computer for the full driver surface.
//!
//! Secrets never round-trip the page: the Composio key input is write-only,
//! custom-server headers/env echo back as NAMES only, and an empty secret
//! field on edit means "keep the stored values" (service-side merge rule).

use std::collections::BTreeMap;
use std::time::Duration;

use gpui::{
    AnyElement, Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px,
};

use zeron_computer::{ComputerSettings, ComputerStatus};
use zeron_rpc::methods;
use zeron_tools::{
    AppConnectStart, COMPOSIO_BROWSE_URL, ComposioApp, ComposioSearchHit, FEATURED_APP_COUNT,
    POPULAR_APPS, PRESET_SERVERS, PresetServer, ProbeResult, ProviderConfig, ProviderStatus,
    ToolProvider, ToolsSnapshot, toolkit_logo_url,
};

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::icons::{self, icon};
use crate::popover::{self, Loadable};
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

/// Emitted when the Computer group's "Open Computer settings" control fires —
/// the Shell owns routing, so the page asks rather than navigates.
pub enum ToolsPageEvent {
    OpenComputerSettings,
}

/// The add/edit form's transport picker — matches `ProviderConfig`'s
/// non-Composio variants (Composio is managed by the key flow, not the form).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormTransport {
    Http,
    Stdio,
}

impl FormTransport {
    fn label(self) -> &'static str {
        match self {
            Self::Http => "Remote (HTTP)",
            Self::Stdio => "Local (stdio)",
        }
    }
}

/// One toolkit's in-flight OAuth connect, keyed by toolkit slug.
#[derive(Debug, Clone)]
enum ConnectPhase {
    /// `StartAppConnect` hasn't returned yet.
    Starting,
    /// The Connect Link was opened — polling until ACTIVE. Carries the URL
    /// so "Open link" can re-open it (the link is short-lived but re-opening
    /// the same one beats a failed redirect hunt).
    AwaitingAuth(String),
}

/// The add/edit server form's identity state (fields live on the page's
/// `ComposerInput`s so focus/IME behave like every other input).
struct ServerForm {
    /// Provider id when editing; `None` = creating a new server.
    editing: Option<String>,
    transport: FormTransport,
}

pub struct ToolsPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    /// Which device's gateway is shown; `None` = this device.
    target_device: Option<String>,
    device_menu: popover::Popup<()>,
    snapshot: Loadable<ToolsSnapshot>,
    /// Composio toolkit connection rows (curated + remembered). `Idle` when
    /// no Composio key is stored.
    apps: Loadable<Vec<ComposioApp>>,
    /// toolkit → connect phase while an OAuth flow is in flight.
    connecting: BTreeMap<String, ConnectPhase>,
    search_results: Option<Vec<ComposioSearchHit>>,
    searching: bool,
    form: Option<ServerForm>,
    probe: Option<ProbeResult>,
    probing: bool,
    /// In-flight provider mutation (save/remove/toggle) or Composio key
    /// connect — disables the matching controls.
    busy: bool,
    computer: Loadable<ComputerStatus>,
    computer_saving: bool,
    key_input: Entity<ComposerInput>,
    search_input: Entity<ComposerInput>,
    name_input: Entity<ComposerInput>,
    url_input: Entity<ComposerInput>,
    headers_input: Entity<ComposerInput>,
    command_input: Entity<ComposerInput>,
    args_input: Entity<ComposerInput>,
    env_input: Entity<ComposerInput>,
    error: Option<SharedString>,
    load_task: Option<Task<()>>,
    watch_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
    apps_task: Option<Task<()>>,
    search_task: Option<Task<()>>,
    /// `StartAppConnect` rides its own slot — a provider action (save /
    /// remove / key connect) must not cancel an in-flight OAuth start.
    connect_task: Option<Task<()>>,
    poll_task: Option<Task<()>>,
    computer_task: Option<Task<()>>,
    _observe: Subscription,
    _key_events: Subscription,
    _search_events: Subscription,
}

impl ToolsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        let key_input = cx.new(|cx| {
            ComposerInput::new("Composio API key (ck_…)", cx)
                .with_single_line()
                .with_accessibility_role(gpui::Role::TextInput)
        });
        let search_input = cx.new(|cx| {
            ComposerInput::new("Search all Composio apps — Shopify, Jira, Reddit…", cx)
                .with_single_line()
                .with_accessibility_role(gpui::Role::TextInput)
        });
        let name_input = cx.new(|cx| {
            ComposerInput::new("Server name (e.g. GitHub)", cx)
                .with_single_line()
                .with_accessibility_role(gpui::Role::TextInput)
        });
        let url_input = cx.new(|cx| {
            ComposerInput::new("https://example.com/mcp", cx)
                .with_single_line()
                .with_accessibility_role(gpui::Role::TextInput)
        });
        let headers_input = cx.new(|cx| {
            ComposerInput::new("Authorization=Bearer …, X-Custom=…", cx)
                .with_single_line()
                .with_accessibility_role(gpui::Role::TextInput)
        });
        let command_input = cx.new(|cx| {
            ComposerInput::new("Command (e.g. npx)", cx)
                .with_single_line()
                .with_accessibility_role(gpui::Role::TextInput)
        });
        let args_input = cx.new(|cx| {
            ComposerInput::new("Args — space separated (e.g. -y @playwright/mcp)", cx)
                .with_single_line()
                .with_accessibility_role(gpui::Role::TextInput)
        });
        let env_input = cx.new(|cx| {
            ComposerInput::new("Env — NAME=VALUE, NAME2=VALUE2", cx)
                .with_single_line()
                .with_accessibility_role(gpui::Role::TextInput)
        });
        let key_events = cx.subscribe(&key_input, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.connect_composio(cx);
            }
        });
        let search_events = cx.subscribe(&search_input, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.run_search(cx);
            }
        });
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            target_device: None,
            device_menu: popover::Popup::default(),
            snapshot: Loadable::Idle,
            apps: Loadable::Idle,
            connecting: BTreeMap::new(),
            search_results: None,
            searching: false,
            form: None,
            probe: None,
            probing: false,
            busy: false,
            computer: Loadable::Idle,
            computer_saving: false,
            key_input,
            search_input,
            name_input,
            url_input,
            headers_input,
            command_input,
            args_input,
            env_input,
            error: None,
            load_task: None,
            watch_task: None,
            action_task: None,
            apps_task: None,
            search_task: None,
            connect_task: None,
            poll_task: None,
            computer_task: None,
            _observe: observe,
            _key_events: key_events,
            _search_events: search_events,
        };
        page.load(cx);
        page.watch(cx);
        page.load_computer(cx);
        page
    }

    /// Params with the `targetDeviceId` passthrough merged in — every tools
    /// RPC is relay-forwardable.
    fn params(&self, value: serde_json::Value) -> serde_json::Value {
        let mut value = value;
        if let (Some(target), Some(object)) = (&self.target_device, value.as_object_mut()) {
            object.insert("targetDeviceId".into(), serde_json::json!(target));
        }
        value
    }

    fn composio_connected(&self) -> bool {
        matches!(&self.snapshot, Loadable::Ready(s) if s.providers.iter().any(|p| p.kind == "composio" && p.enabled))
    }

    /// `ListTools` — the page's first paint; the watch keeps it live.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.params(serde_json::json!({}));
        self.snapshot = Loadable::Loading;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::LIST_TOOLS, params).await;
            this.update(cx, |page, cx| {
                page.snapshot = match result {
                    Ok(value) => match serde_json::from_value::<ToolsSnapshot>(value) {
                        Ok(snapshot) => Loadable::Ready(snapshot),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                if page.composio_connected() {
                    page.load_apps(cx);
                } else {
                    page.apps = Loadable::Idle;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// `WatchTools` — one `ToolsSnapshot` first, then one per provider change.
    fn watch(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.params(serde_json::json!({}));
        self.watch_task = Some(cx.spawn(async move |this, cx| {
            const RETRY_DELAY: Duration = Duration::from_secs(2);
            loop {
                let mut rx = match engine
                    .client()
                    .subscribe(methods::WATCH_TOOLS, params.clone())
                    .await
                {
                    Ok(rx) => rx,
                    Err(err) => {
                        tracing::debug!(error = %err, "tools watch unavailable; retrying");
                        if this.update(cx, |_, _| {}).is_err() {
                            return;
                        }
                        cx.background_executor().timer(RETRY_DELAY).await;
                        continue;
                    }
                };
                while let Some(value) = rx.recv().await {
                    let snapshot = serde_json::from_value::<ToolsSnapshot>(value).ok();
                    let alive = this.update(cx, |page, cx| {
                        if let Some(snapshot) = snapshot {
                            page.snapshot = Loadable::Ready(snapshot);
                            if page.composio_connected() {
                                if !matches!(page.apps, Loadable::Loading | Loadable::Ready(_)) {
                                    page.load_apps(cx);
                                }
                            } else {
                                page.apps = Loadable::Idle;
                            }
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

    /// `ListComposioApps` — the connect grid's live per-toolkit states.
    fn load_apps(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.params(serde_json::json!({}));
        self.apps = Loadable::Loading;
        self.apps_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::LIST_COMPOSIO_APPS, params)
                .await;
            this.update(cx, |page, cx| {
                page.apps = match result {
                    Ok(value) => match serde_json::from_value::<Vec<ComposioApp>>(value) {
                        Ok(apps) => Loadable::Ready(apps),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    /// `ComputerStatus` — the Computer group's summary (local device only).
    fn load_computer(&mut self, cx: &mut Context<Self>) {
        if self.target_device.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.computer = Loadable::Loading;
        self.computer_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::COMPUTER_STATUS, serde_json::json!({}))
                .await;
            this.update(cx, |page, cx| {
                page.computer = match result {
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

    /// `ConnectComposio` — validate the key against Connect, then the app
    /// grid loads off the fresh provider. The key itself is never echoed.
    fn connect_composio(&mut self, cx: &mut Context<Self>) {
        let key = self.key_input.read(cx).text().trim().to_string();
        if key.is_empty() || self.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.params(serde_json::json!({ "apiKey": key }));
        self.busy = true;
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::CONNECT_COMPOSIO, params)
                .await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match result {
                    Ok(_) => {
                        page.key_input
                            .update(cx, |input, cx| input.set_text("", cx));
                        // The watch refreshes the snapshot; apps need one
                        // explicit load off the new provider.
                        page.load_apps(cx);
                    }
                    Err(err) => page.error = Some(err.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// `DisconnectComposio` — removes the provider, key and app state.
    fn disconnect_composio(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.params(serde_json::json!({}));
        self.busy = true;
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::DISCONNECT_COMPOSIO, params)
                .await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match result {
                    Ok(value) => {
                        page.apps = Loadable::Idle;
                        page.connecting.clear();
                        page.search_results = None;
                        if let Ok(snapshot) = serde_json::from_value::<ToolsSnapshot>(value) {
                            page.snapshot = Loadable::Ready(snapshot);
                        }
                    }
                    Err(err) => page.error = Some(err.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// `StartAppConnect` — kicks off one toolkit's OAuth flow. The reply's
    /// `authUrl` is the hosted Connect Link the user signs in through; once
    /// opened, the poller tracks the connection until ACTIVE.
    fn start_app_connect(&mut self, toolkit: String, cx: &mut Context<Self>) {
        if self.connecting.contains_key(&toolkit) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.connecting
            .insert(toolkit.clone(), ConnectPhase::Starting);
        self.error = None;
        let params = self.params(serde_json::json!({ "toolkit": toolkit }));
        self.connect_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::START_APP_CONNECT, params)
                .await;
            this.update(cx, |page, cx| {
                match result.and_then(|value| {
                    serde_json::from_value::<AppConnectStart>(value)
                        .map_err(|e| zeron_rpc::RpcError::Failed(e.to_string()))
                }) {
                    Ok(start) => {
                        if start.connected {
                            page.connecting.remove(&start.toolkit);
                            page.note_app(ComposioApp {
                                toolkit: start.toolkit.clone(),
                                status: start.status.clone(),
                                connected: true,
                                account: None,
                            });
                        } else if let Some(url) = start.auth_url {
                            cx.open_url(&url);
                            page.connecting
                                .insert(start.toolkit.clone(), ConnectPhase::AwaitingAuth(url));
                            page.ensure_poller(cx);
                        } else {
                            page.connecting.remove(&start.toolkit);
                        }
                    }
                    Err(err) => {
                        page.connecting.remove(&toolkit);
                        page.error = Some(err.to_string().into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// One shared poller for every in-flight connect: `PollAppConnect` per
    /// toolkit every 3s for ~2 minutes, dropping flows that never land.
    fn ensure_poller(&mut self, cx: &mut Context<Self>) {
        if self.poll_task.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let target = self.target_device.clone();
        self.poll_task = Some(cx.spawn(async move |this, cx| {
            for _ in 0..40 {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let toolkits = match this.update(cx, |page, _| {
                    page.connecting.keys().cloned().collect::<Vec<_>>()
                }) {
                    Ok(t) => t,
                    Err(_) => return,
                };
                if toolkits.is_empty() {
                    break;
                }
                for toolkit in toolkits {
                    let mut params = serde_json::json!({ "toolkit": toolkit });
                    if let Some(target) = &target {
                        params["targetDeviceId"] = serde_json::json!(target);
                    }
                    let app = engine
                        .client()
                        .call(methods::POLL_APP_CONNECT, params)
                        .await
                        .ok()
                        .and_then(|v| serde_json::from_value::<ComposioApp>(v).ok());
                    let alive = this.update(cx, |page, cx| {
                        if let Some(app) = app {
                            if app.connected {
                                page.connecting.remove(&toolkit);
                            }
                            page.note_app(app);
                        }
                        cx.notify();
                    });
                    if alive.is_err() {
                        return;
                    }
                }
            }
            this.update(cx, |page, cx| {
                page.poll_task = None;
                // Expired flows revert to a plain Connect button.
                page.connecting.clear();
                cx.notify();
            })
            .ok();
        }));
    }

    /// `SearchComposio` — the full catalog beyond the curated grid.
    fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).text().trim().to_string();
        if query.is_empty() || self.searching {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.params(serde_json::json!({ "query": query }));
        self.searching = true;
        self.error = None;
        self.search_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::SEARCH_COMPOSIO, params).await;
            this.update(cx, |page, cx| {
                page.searching = false;
                match result {
                    Ok(value) => match serde_json::from_value::<Vec<ComposioSearchHit>>(value) {
                        Ok(hits) => page.search_results = Some(hits),
                        Err(err) => page.error = Some(err.to_string().into()),
                    },
                    Err(err) => page.error = Some(err.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Upsert one app row in place (poll/connect replies), appending when the
    /// toolkit wasn't in the last list.
    fn note_app(&mut self, app: ComposioApp) {
        if let Loadable::Ready(apps) = &mut self.apps {
            match apps.iter_mut().find(|a| a.toolkit == app.toolkit) {
                Some(existing) => *existing = app,
                None => apps.push(app),
            }
        }
    }

    // -- server form ----------------------------------------------------------

    fn open_new_form(&mut self, cx: &mut Context<Self>) {
        self.form = Some(ServerForm {
            editing: None,
            transport: FormTransport::Http,
        });
        self.probe = None;
        for input in [
            &self.name_input,
            &self.url_input,
            &self.headers_input,
            &self.command_input,
            &self.args_input,
            &self.env_input,
        ] {
            input.update(cx, |input, cx| input.set_text("", cx));
        }
        cx.notify();
    }

    /// Edit prefills the non-secret fields from the masked config echo —
    /// header/env inputs stay empty, meaning "keep the stored values".
    fn open_edit_form(&mut self, provider: &ProviderStatus, cx: &mut Context<Self>) {
        let transport = match provider.kind.as_str() {
            "stdio" => FormTransport::Stdio,
            _ => FormTransport::Http,
        };
        self.form = Some(ServerForm {
            editing: Some(provider.id.clone()),
            transport,
        });
        self.probe = None;
        self.name_input
            .update(cx, |input, cx| input.set_text(&provider.name, cx));
        let config = &provider.config;
        match transport {
            FormTransport::Http => {
                self.url_input.update(cx, |input, cx| {
                    input.set_text(
                        config
                            .get("url")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default(),
                        cx,
                    )
                });
                self.headers_input
                    .update(cx, |input, cx| input.set_text("", cx));
            }
            FormTransport::Stdio => {
                self.command_input.update(cx, |input, cx| {
                    input.set_text(
                        config
                            .get("command")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default(),
                        cx,
                    )
                });
                self.args_input.update(cx, |input, cx| {
                    let args = config
                        .get("args")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str())
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                    input.set_text(args, cx)
                });
                self.env_input
                    .update(cx, |input, cx| input.set_text("", cx));
            }
        }
        cx.notify();
    }

    /// One-click preset: prefill the form from a curated entry. OAuth-only
    /// presets prefill too — the note tells the user what auth to add.
    fn apply_preset(&mut self, preset: &PresetServer, cx: &mut Context<Self>) {
        self.form = Some(ServerForm {
            editing: None,
            transport: if preset.transport == "stdio" {
                FormTransport::Stdio
            } else {
                FormTransport::Http
            },
        });
        self.probe = None;
        self.name_input
            .update(cx, |input, cx| input.set_text(preset.name, cx));
        self.url_input
            .update(cx, |input, cx| input.set_text(preset.url, cx));
        self.command_input
            .update(cx, |input, cx| input.set_text(preset.command, cx));
        self.args_input
            .update(cx, |input, cx| input.set_text(preset.args.join(" "), cx));
        self.headers_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.env_input
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    fn close_form(&mut self, cx: &mut Context<Self>) {
        self.form = None;
        self.probe = None;
        cx.notify();
    }

    /// Build the `ProviderConfig` the form describes (validation errors are
    /// shown inline, not as RPC failures).
    fn form_config(&self, cx: &Context<Self>) -> Result<ProviderConfig, SharedString> {
        let Some(form) = &self.form else {
            return Err("No server form is open".into());
        };
        match form.transport {
            FormTransport::Http => {
                let url = self.url_input.read(cx).text().trim().to_string();
                if url.is_empty() {
                    return Err("A server URL is required.".into());
                }
                Ok(ProviderConfig::Http {
                    url,
                    headers: parse_pairs(self.headers_input.read(cx).text()),
                })
            }
            FormTransport::Stdio => {
                let command = self.command_input.read(cx).text().trim().to_string();
                if command.is_empty() {
                    return Err("A command is required.".into());
                }
                Ok(ProviderConfig::Stdio {
                    command,
                    args: parse_args(self.args_input.read(cx).text()),
                    env: parse_pairs(self.env_input.read(cx).text()),
                })
            }
        }
    }

    /// `ProbeMcpServer` — a throwaway handshake against the form's config;
    /// nothing is persisted.
    fn probe(&mut self, cx: &mut Context<Self>) {
        let config = match self.form_config(cx) {
            Ok(config) => config,
            Err(message) => {
                self.error = Some(message);
                cx.notify();
                return;
            }
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.params(serde_json::to_value(config).unwrap_or_default());
        self.probing = true;
        self.probe = None;
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::PROBE_MCP_SERVER, params)
                .await;
            this.update(cx, |page, cx| {
                page.probing = false;
                match result {
                    Ok(value) => match serde_json::from_value::<ProbeResult>(value) {
                        Ok(probe) => page.probe = Some(probe),
                        Err(err) => page.error = Some(err.to_string().into()),
                    },
                    Err(err) => page.error = Some(err.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// `UpsertToolProvider` — the reply carries the fresh snapshot.
    fn save_provider(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.form else { return };
        let name = self.name_input.read(cx).text().trim().to_string();
        if name.is_empty() {
            self.error = Some("A server name is required.".into());
            cx.notify();
            return;
        }
        let config = match self.form_config(cx) {
            Ok(config) => config,
            Err(message) => {
                self.error = Some(message);
                cx.notify();
                return;
            }
        };
        let (id, enabled) = match &form.editing {
            Some(id) => {
                let enabled = match &self.snapshot {
                    Loadable::Ready(s) => s
                        .providers
                        .iter()
                        .find(|p| &p.id == id)
                        .map(|p| p.enabled)
                        .unwrap_or(true),
                    _ => true,
                };
                (id.clone(), enabled)
            }
            None => (
                format!("server-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
                true,
            ),
        };
        let provider = ToolProvider {
            id,
            name,
            enabled,
            config,
            toolkits: Vec::new(),
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.params(serde_json::to_value(provider).unwrap_or_default());
        self.busy = true;
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::UPSERT_TOOL_PROVIDER, params)
                .await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match result {
                    Ok(value) => {
                        page.form = None;
                        page.probe = None;
                        if let Ok(snapshot) = serde_json::from_value::<ToolsSnapshot>(value) {
                            page.snapshot = Loadable::Ready(snapshot);
                        }
                    }
                    Err(err) => page.error = Some(err.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// `RemoveToolProvider`.
    fn remove_provider(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.params(serde_json::json!({ "id": id }));
        self.busy = true;
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::REMOVE_TOOL_PROVIDER, params)
                .await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match result {
                    Ok(value) => {
                        if let Ok(snapshot) = serde_json::from_value::<ToolsSnapshot>(value) {
                            page.snapshot = Loadable::Ready(snapshot);
                        }
                    }
                    Err(err) => page.error = Some(err.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// `SetToolProviderEnabled`.
    fn toggle_provider(&mut self, id: String, enabled: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.params(serde_json::json!({ "id": id, "enabled": enabled }));
        self.error = None;
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::SET_TOOL_PROVIDER_ENABLED, params)
                .await;
            this.update(cx, |page, cx| {
                match result {
                    Ok(value) => {
                        if let Ok(snapshot) = serde_json::from_value::<ToolsSnapshot>(value) {
                            page.snapshot = Loadable::Ready(snapshot);
                        }
                    }
                    Err(err) => page.error = Some(err.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The full persisted computer settings, rebuilt from the status echo.
    fn computer_settings(&self) -> Option<ComputerSettings> {
        let Loadable::Ready(status) = &self.computer else {
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

    /// `ComputerSetSettings` — the summary card's Screen Context toggle.
    fn toggle_computer(&mut self, cx: &mut Context<Self>) {
        let Some(mut settings) = self.computer_settings() else {
            return;
        };
        settings.enabled = !settings.enabled;
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Ok(params) = serde_json::to_value(&settings) else {
            return;
        };
        self.computer_saving = true;
        self.error = None;
        self.computer_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::COMPUTER_SET_SETTINGS, params)
                .await;
            this.update(cx, |page, cx| {
                page.computer_saving = false;
                match result {
                    Ok(value) => match serde_json::from_value::<ComputerStatus>(value) {
                        Ok(status) => page.computer = Loadable::Ready(status),
                        Err(err) => page.error = Some(err.to_string().into()),
                    },
                    Err(err) => page.error = Some(err.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    // -- device switcher (accounts.rs parity) ----------------------------------

    fn close_device_menu(&mut self, cx: &mut Context<Self>) {
        if self.device_menu.begin_close() {
            popover::reap_popup(cx, |page: &mut Self| &mut page.device_menu);
            cx.notify();
        }
    }

    fn set_target_device(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        self.close_device_menu(cx);
        if self.target_device == target {
            cx.notify();
            return;
        }
        self.target_device = target;
        // A different device = a different gateway: drop every device-local
        // view state and reload. The watch task keeps its original params —
        // re-subscribe so frames land for the retargeted device.
        self.watch_task = None;
        self.connecting.clear();
        self.search_results = None;
        self.form = None;
        self.probe = None;
        self.computer = Loadable::Idle;
        self.error = None;
        self.load(cx);
        self.watch(cx);
        self.load_computer(cx);
    }

    /// The page-header device switcher — the accounts page's dropdown
    /// verbatim with tools-scoped ids.
    fn render_device_switcher(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (mut devices, local_id) = {
            let s = self.state.read(cx);
            (s.devices.clone(), s.local_device_id.clone())
        };
        devices.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        let effective = self.target_device.clone().or_else(|| local_id.clone());
        let selected = devices
            .iter()
            .find(|d| Some(d.id.as_str()) == effective.as_deref())
            .cloned();
        let platform_glyph = |platform: &str| match platform {
            "macos" | "darwin" => icons::LAPTOP,
            "ios" | "android" => icons::SMARTPHONE,
            _ => icons::MONITOR,
        };
        let trigger_glyph = platform_glyph(
            selected
                .as_ref()
                .map(|d| d.platform.as_str())
                .unwrap_or("macos"),
        );
        let trigger_label: SharedString = selected
            .as_ref()
            .map(|d| d.name.clone().into())
            .unwrap_or_else(|| SharedString::from("This device"));
        let emerald = theme.success;
        let open = self.device_menu.is_open();

        let mut trigger =
            div()
                .id("tools-device-switcher")
                .flex_none()
                .h(px(28.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .cursor_pointer()
                .bg(if open {
                    crate::theme::ink(0.06)
                } else {
                    gpui::transparent_black()
                })
                .when(!open, |el| el.hover(|s| s.bg(crate::theme::ink(0.04))))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, _| this.device_menu.note_trigger_press()),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.device_menu.take_press_was_open() {
                        this.close_device_menu(cx);
                    } else {
                        this.device_menu.open(());
                    }
                    cx.notify();
                }))
                .child(
                    icon(trigger_glyph)
                        .size(px(16.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(trigger_label),
                )
                .child(div().size(px(6.0)).rounded_full().flex_none().bg(
                    if effective == local_id {
                        emerald
                    } else {
                        crate::theme::ink(0.2)
                    },
                ))
                .child(
                    icon(icons::SORT_VERTICAL)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(theme.text_muted.opacity(if open { 0.9 } else { 0.4 })),
                );

        if self.device_menu.get().is_some() {
            let closing = self.device_menu.closing_since();
            let menu = popover::popover_card(theme)
                .w(px(220.0))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.close_device_menu(cx);
                }))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(popover::menu_heading(theme, "Devices"))
                .children(devices.into_iter().enumerate().map(|(ix, d)| {
                    let is_active = Some(d.id.as_str()) == effective.as_deref();
                    let is_local = local_id.as_deref() == Some(d.id.as_str());
                    let glyph = platform_glyph(&d.platform);
                    let name: SharedString = d.name.clone().into();
                    let pick_id = d.id.clone();
                    popover::menu_row(theme, is_active, format!("tools-device-row-{ix}"))
                        .id(("tools-device-row", ix))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let target = (!is_local).then(|| pick_id.clone());
                            this.set_target_device(target, cx);
                        }))
                        .child(
                            icon(glyph)
                                .size(px(16.0))
                                .flex_none()
                                .text_color(theme.text_muted),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(name))
                        .when(is_local, |el| {
                            el.child(
                                div()
                                    .flex_none()
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .text_color(theme.text_muted.opacity(0.35))
                                    .child(SharedString::from("You")),
                            )
                        })
                        .child(
                            div()
                                .size(px(6.0))
                                .rounded_full()
                                .flex_none()
                                .bg(if is_local {
                                    emerald
                                } else {
                                    crate::theme::ink(0.2)
                                }),
                        )
                }));
            trigger = trigger.child(popover::anchored_menu(
                "tools-device-menu",
                menu.into_any_element(),
                closing,
            ));
        }
        trigger.into_any_element()
    }

    // -- rendering --------------------------------------------------------------

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

    fn ghost(
        theme: &Theme,
        id: impl Into<gpui::ElementId>,
        label: &'static str,
    ) -> gpui::Stateful<gpui::Div> {
        widgets::ghost_action(theme)
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(label)
            .tab_index(0)
            .focus_visible(|style| style.border_2().border_color(theme.accent))
            .cursor_pointer()
            .child(label)
    }

    /// The "Apps" card: key entry when no Composio provider exists; masked
    /// key + app grid + catalog search once connected.
    fn composio_section(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let provider = match &self.snapshot {
            Loadable::Ready(s) => s.providers.iter().find(|p| p.kind == "composio").cloned(),
            _ => None,
        };
        let mut card = widgets::section_card(theme);
        match provider {
            None => {
                card = card.child(Self::row(
                    theme,
                    true,
                    "Composio",
                    "One API key connects Gmail, Slack, Notion, GitHub and 1,000+ more \
                     apps. Agents reach them through the cosmos-tools gateway in \
                     every harness.",
                    Self::ghost(theme, "tools-composio-connect", "Connect")
                        .when(self.busy, |el| el.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.connect_composio(cx)))
                        .into_any_element(),
                ));
                card = card.child(div().px(px(20.0)).pb(px(16.0)).child(popover::dialog_field(
                    self.key_input.clone().into_any_element(),
                )));
                card = card.child(
                    div()
                        .px(px(20.0))
                        .pb(px(16.0))
                        .text_size(crate::typography::ui_rems(11.5))
                        .text_color(theme.text_muted.opacity(0.8))
                        .child(SharedString::from(
                            "The key is validated against Composio Connect and stored \
                             only on this device — agents see tool names, never the key.",
                        )),
                );
            }
            Some(provider) => {
                let masked = provider
                    .config
                    .get("apiKeyMasked")
                    .and_then(|v| v.as_str())
                    .unwrap_or("ck_…")
                    .to_string();
                let status_line = match provider.status.as_str() {
                    "connected" => format!("{masked} — {} meta-tools live", provider.tool_count),
                    other => match &provider.last_error {
                        Some(err) => format!("{masked} — {other}: {err}"),
                        None => format!("{masked} — {other}"),
                    },
                };
                let enabled = provider.enabled;
                let toggle = widgets::toggle_switch(theme, enabled, "tools-composio-enabled")
                    .id("tools-composio-enabled")
                    .role(gpui::Role::Switch)
                    .aria_label("Composio")
                    .aria_toggled(if enabled {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .tab_index(0)
                    .focus_visible(|style| style.border_2().border_color(theme.accent))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggle_provider("composio".into(), !enabled, cx);
                    }));
                let control = div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(toggle)
                    .child(
                        Self::ghost(theme, "tools-composio-disconnect", "Disconnect")
                            .on_click(cx.listener(|this, _, _, cx| this.disconnect_composio(cx))),
                    );
                card = card.child(Self::row(
                    theme,
                    true,
                    "Composio",
                    &status_line,
                    control.into_any_element(),
                ));
                card = card.child(self.apps_block(theme, provider.enabled, cx));
                card = card.child(self.search_block(theme, provider.enabled, cx));
            }
        }
        card.into_any_element()
    }

    /// Whether a toolkit row earns a slot in the grid: the first
    /// [`FEATURED_APP_COUNT`] curated picks always show; anything else
    /// shows once it's connected, mid-connect, or remembered from earlier
    /// use — the other curated picks fold into search/browse.
    fn show_app(&self, app: &ComposioApp) -> bool {
        if app.connected || self.connecting.contains_key(&app.toolkit) {
            return true;
        }
        match POPULAR_APPS.iter().position(|p| p.toolkit == app.toolkit) {
            Some(rank) => rank < FEATURED_APP_COUNT,
            None => true,
        }
    }

    /// A toolkit's brand tile: Composio's logo feed inside the standard
    /// row tile, with a letter monogram while it loads or when it 404s.
    fn app_icon(theme: &Theme, toolkit: &str, label: &str) -> gpui::Div {
        let letter: SharedString = label
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect::<String>())
            .unwrap_or_else(|| "?".into())
            .into();
        let monogram = |theme: &Theme, letter: &SharedString| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(crate::typography::ui_rems(15.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text_muted.opacity(0.8))
                .child(letter.clone())
                .into_any_element()
        };
        div()
            .flex_none()
            .size(px(36.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(theme.border)
            .bg(crate::theme::ink(0.03))
            .overflow_hidden()
            .flex()
            .items_center()
            .justify_center()
            .child(
                gpui::img(toolkit_logo_url(toolkit))
                    .size(px(22.0))
                    .with_fallback({
                        let theme = theme.clone();
                        let letter = letter.clone();
                        move || monogram(&theme, &letter)
                    })
                    .with_loading({
                        let theme = theme.clone();
                        move || monogram(&theme, &letter)
                    }),
            )
    }

    /// One app row: brand tile, label + status line, and the
    /// Connect/Connected/sign-in affordance.
    fn app_row(
        &self,
        theme: &Theme,
        app: &ComposioApp,
        first: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let popular = POPULAR_APPS.iter().find(|p| p.toolkit == app.toolkit);
        let label = popular
            .map(|p| p.label.to_string())
            .unwrap_or_else(|| prettify_toolkit(&app.toolkit));
        let phase = self.connecting.get(&app.toolkit);
        let control: AnyElement = if app.connected {
            widgets::badge_active(theme, "Connected").into_any_element()
        } else {
            match phase {
                Some(ConnectPhase::Starting) => {
                    widgets::badge(theme, "Starting…").into_any_element()
                }
                Some(ConnectPhase::AwaitingAuth(url)) => {
                    let url = url.clone();
                    Self::ghost(
                        theme,
                        format!("tools-app-reopen-{}", app.toolkit),
                        "Open sign-in",
                    )
                    .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url)))
                    .into_any_element()
                }
                None => {
                    let toolkit = app.toolkit.clone();
                    Self::ghost(
                        theme,
                        format!("tools-app-connect-{}", app.toolkit),
                        "Connect",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.start_app_connect(toolkit.clone(), cx)
                    }))
                    .into_any_element()
                }
            }
        };
        let desc: SharedString = if app.connected {
            app.account
                .as_deref()
                .map(|account| format!("Connected as {account}"))
                .unwrap_or_else(|| app.status.clone())
                .into()
        } else {
            match phase {
                Some(ConnectPhase::Starting) => "Requesting a sign-in link…".into(),
                Some(ConnectPhase::AwaitingAuth(_)) => {
                    "Finish sign-in in the browser — this updates automatically.".into()
                }
                None => match app.status.as_str() {
                    "not-connected" | "unknown" => {
                        popular.map(|p| p.blurb).unwrap_or("Not connected").into()
                    }
                    other => other.into(),
                },
            }
        };
        widgets::card_row(theme, first)
            .flex_wrap()
            .child(Self::app_icon(theme, &app.toolkit, &label))
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(widgets::row_title(theme, label))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(12.0))
                            .line_height(px(18.0))
                            .text_color(theme.text_muted)
                            .child(desc),
                    ),
            )
            .child(control)
            .into_any_element()
    }

    /// The app grid: ~5 curated picks plus every connected or remembered
    /// toolkit, then the marketplace escape hatch — the other thousand
    /// apps are one search or one dashboard visit away.
    fn apps_block(&self, theme: &Theme, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        if !enabled {
            return div()
                .px(px(20.0))
                .py(px(12.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .child("Composio is off — flip the toggle to manage app connections.")
                .into_any_element();
        }
        let mut block = div().flex().flex_col();
        match &self.apps {
            Loadable::Loading => {
                block = block.child(
                    div()
                        .px(px(20.0))
                        .py(px(12.0))
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child("Loading apps…"),
                );
            }
            Loadable::Error(message) => {
                block = block.child(widgets::warning_strip(theme, message.clone()));
            }
            Loadable::Ready(apps) => {
                let mut first = true;
                for app in apps.iter().filter(|app| self.show_app(app)) {
                    block = block.child(self.app_row(theme, app, first, cx));
                    first = false;
                }
                // The catalog escape hatch — everything past the grid.
                block = block.child(
                    widgets::card_row(theme, first)
                        .flex_wrap()
                        .child(
                            div()
                                .flex_none()
                                .size(px(36.0))
                                .rounded(px(10.0))
                                .border_1()
                                .border_color(theme.border)
                                .bg(crate::theme::ink(0.03))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    icon(icons::GLOBE)
                                        .size(px(16.0))
                                        .text_color(theme.text_muted),
                                ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(160.0))
                                .flex()
                                .flex_col()
                                .gap(px(4.0))
                                .child(widgets::row_title(theme, "Browse all apps"))
                                .child(
                                    div()
                                        .text_size(crate::typography::ui_rems(12.0))
                                        .line_height(px(18.0))
                                        .text_color(theme.text_muted)
                                        .child(SharedString::from(
                                            "1,000+ apps on the Composio marketplace — \
                                             connect there or search below.",
                                        )),
                                ),
                        )
                        .child(
                            Self::ghost(theme, "tools-browse-composio", "Open Composio").on_click(
                                cx.listener(|_, _, _, cx| cx.open_url(COMPOSIO_BROWSE_URL)),
                            ),
                        ),
                );
            }
            Loadable::Idle => {}
        }
        block.into_any_element()
    }

    /// The catalog search field + result rows — hits carry the same brand
    /// tile, live connect state and sign-in flow as the grid.
    fn search_block(&self, theme: &Theme, enabled: bool, cx: &mut Context<Self>) -> AnyElement {
        if !enabled {
            return div().into_any_element();
        }
        let mut block = div()
            .border_t_1()
            .border_color(theme.border)
            .px(px(20.0))
            .py(px(14.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(widgets::field_label(theme, "Search the full catalog"));
        block = block.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(div().flex_1().child(popover::dialog_field(
                    self.search_input.clone().into_any_element(),
                )))
                .child(
                    Self::ghost(
                        theme,
                        "tools-search",
                        if self.searching {
                            "Searching…"
                        } else {
                            "Search"
                        },
                    )
                    .when(self.searching, |el| el.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| this.run_search(cx))),
                ),
        );
        if let Some(hits) = &self.search_results {
            if hits.is_empty() {
                block = block.child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child("No apps matched that search."),
                );
            } else {
                for hit in hits.iter().take(8) {
                    let connected = hit.connected
                        || matches!(&self.apps, Loadable::Ready(apps)
                            if apps.iter().any(|a| a.toolkit == hit.toolkit && a.connected));
                    let phase = self.connecting.get(&hit.toolkit);
                    let control: AnyElement = if connected {
                        widgets::badge_active(theme, "Connected").into_any_element()
                    } else {
                        match phase {
                            Some(ConnectPhase::Starting) => {
                                widgets::badge(theme, "Starting…").into_any_element()
                            }
                            Some(ConnectPhase::AwaitingAuth(url)) => {
                                let url = url.clone();
                                Self::ghost(
                                    theme,
                                    format!("tools-hit-reopen-{}", hit.toolkit),
                                    "Open sign-in",
                                )
                                .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url)))
                                .into_any_element()
                            }
                            None => {
                                let toolkit = hit.toolkit.clone();
                                Self::ghost(
                                    theme,
                                    format!("tools-hit-connect-{}", hit.toolkit),
                                    "Connect",
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.start_app_connect(toolkit.clone(), cx)
                                }))
                                .into_any_element()
                            }
                        }
                    };
                    let label = POPULAR_APPS
                        .iter()
                        .find(|p| p.toolkit == hit.toolkit)
                        .map(|p| p.label.to_string())
                        .unwrap_or_else(|| prettify_toolkit(&hit.toolkit));
                    let desc: SharedString = hit
                        .description
                        .clone()
                        .unwrap_or_else(|| {
                            if hit.tools.is_empty() {
                                "Composio app".to_string()
                            } else {
                                format!(
                                    "e.g. {}",
                                    hit.tools
                                        .iter()
                                        .take(2)
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                )
                            }
                        })
                        .into();
                    block = block.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .py(px(6.0))
                            .child(Self::app_icon(theme, &hit.toolkit, &label))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(crate::typography::ui_rems(12.5))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.text)
                                            .child(SharedString::from(label)),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_size(crate::typography::ui_rems(11.5))
                                            .text_color(theme.text_muted.opacity(0.85))
                                            .child(desc),
                                    ),
                            )
                            .child(control),
                    );
                }
            }
        }
        block.into_any_element()
    }

    /// The "MCP servers & plugins" card: provider rows, preset chips, and
    /// the add/edit form.
    fn servers_section(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut card = widgets::section_card(theme);
        let providers: Vec<ProviderStatus> = match &self.snapshot {
            Loadable::Ready(s) => s
                .providers
                .iter()
                .filter(|p| p.kind != "composio")
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        if providers.is_empty() && self.form.is_none() {
            card = card.child(Self::row(
                theme,
                true,
                "No custom servers yet",
                "Add any MCP server — a local stdio command or a remote HTTP \
                 endpoint — and every harness gets its tools.",
                Self::ghost(theme, "tools-add-server", "Add server")
                    .on_click(cx.listener(|this, _, _, cx| this.open_new_form(cx)))
                    .into_any_element(),
            ));
        } else {
            for (ix, provider) in providers.iter().enumerate() {
                card = card.child(self.provider_row(theme, provider, ix == 0, cx));
            }
            if self.form.is_none() {
                card = card.child(
                    div()
                        .border_t_1()
                        .border_color(theme.border)
                        .px(px(20.0))
                        .py(px(12.0))
                        .child(
                            Self::ghost(theme, "tools-add-server", "Add server")
                                .on_click(cx.listener(|this, _, _, cx| this.open_new_form(cx))),
                        ),
                );
            }
        }
        // Preset chips — one-click form prefill from the curated list.
        let chips = div()
            .border_t_1()
            .border_color(theme.border)
            .px(px(20.0))
            .py(px(14.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(widgets::field_label(theme, "Popular servers"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .children(PRESET_SERVERS.iter().map(|preset| {
                        let label: SharedString = if preset.needs_auth {
                            format!("{} · sign-in", preset.name).into()
                        } else {
                            preset.name.into()
                        };
                        let p: &'static PresetServer = preset;
                        div()
                            .id(format!("tools-preset-{}", preset.id))
                            .role(gpui::Role::Button)
                            .aria_label(format!("Add {}", preset.name))
                            .tab_index(0)
                            .focus_visible(|style| style.border_2().border_color(theme.accent))
                            .cursor_pointer()
                            .px(px(10.0))
                            .py(px(6.0))
                            .rounded(px(7.0))
                            .border_1()
                            .border_color(theme.border)
                            .text_size(crate::typography::ui_rems(11.5))
                            .text_color(theme.text)
                            .hover(|s| s.bg(crate::theme::ink(0.04)))
                            .on_click(cx.listener(move |this, _, _, cx| this.apply_preset(p, cx)))
                            .child(label)
                    })),
            )
            .child(
                div()
                    .text_size(crate::typography::ui_rems(11.5))
                    .text_color(theme.text_muted.opacity(0.8))
                    .child(SharedString::from(
                        "Chips prefill the form below — add any required headers \
                         (PATs, tokens) before saving. Sign-in-required presets \
                         need an authorization header today.",
                    )),
            );
        card = card.child(chips);
        if self.form.is_some() {
            card = card.child(self.form_block(theme, cx));
        }
        card.into_any_element()
    }

    /// One provider row: name + transport badge + status line + toggle +
    /// Edit/Remove.
    fn provider_row(
        &self,
        theme: &Theme,
        provider: &ProviderStatus,
        first: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = provider.id.clone();
        let enabled = provider.enabled;
        let detail = match provider.status.as_str() {
            "connected" => format!("{} tools", provider.tool_count),
            "disabled" => "Disabled".into(),
            "idle" => "Not connected yet".into(),
            other => match &provider.last_error {
                Some(err) => format!("{other} — {err}"),
                None => other.to_string(),
            },
        };
        let config_hint = match provider.kind.as_str() {
            "http" => provider
                .config
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            "stdio" => {
                let command = provider
                    .config
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let args = provider
                    .config
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                format!("{command} {args}").trim().to_string()
            }
            _ => String::new(),
        };
        let toggle = widgets::toggle_switch(
            theme,
            enabled,
            format!("tools-provider-toggle-{}", provider.id),
        )
        .id(format!("tools-provider-toggle-{}", provider.id))
        .role(gpui::Role::Switch)
        .aria_label(provider.name.clone())
        .aria_toggled(if enabled {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .tab_index(0)
        .focus_visible(|style| style.border_2().border_color(theme.accent))
        .cursor_pointer()
        .on_click(
            cx.listener(move |this, _, _, cx| this.toggle_provider(id.clone(), !enabled, cx)),
        );
        let edit_provider = provider.clone();
        let remove_id = provider.id.clone();
        let controls = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(toggle)
            .child(
                Self::ghost(
                    theme,
                    format!("tools-provider-edit-{}", provider.id),
                    "Edit",
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.open_edit_form(&edit_provider, cx)),
                ),
            )
            .child(
                Self::ghost(
                    theme,
                    format!("tools-provider-remove-{}", provider.id),
                    "Remove",
                )
                .on_click(
                    cx.listener(move |this, _, _, cx| this.remove_provider(remove_id.clone(), cx)),
                ),
            );
        widgets::card_row(theme, first)
            .flex_wrap()
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(widgets::row_title(theme, provider.name.clone()))
                            .child(widgets::badge(theme, provider.kind.clone())),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .line_height(px(18.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(if config_hint.is_empty() {
                                detail
                            } else {
                                format!("{detail} · {config_hint}")
                            })),
                    ),
            )
            .child(controls)
            .into_any_element()
    }

    /// The add/edit form block: transport picker, transport-specific fields,
    /// probe result, and Save/Test/Cancel.
    fn form_block(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.form else {
            return div().into_any_element();
        };
        let editing = form.editing.is_some();
        let transport = form.transport;
        let mut block = div()
            .border_t_1()
            .border_color(theme.border)
            .px(px(20.0))
            .py(px(14.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(widgets::field_label(
                theme,
                if editing { "Edit server" } else { "Add server" },
            ));
        // Transport segmented control.
        block = block.child(
            div().flex().gap(px(6.0)).children(
                [FormTransport::Http, FormTransport::Stdio]
                    .into_iter()
                    .enumerate()
                    .map(|(ix, t)| {
                        let selected = t == transport;
                        div()
                            .id(("tools-transport", ix))
                            .role(gpui::Role::Button)
                            .aria_label(t.label())
                            .aria_toggled(if selected {
                                gpui::Toggled::True
                            } else {
                                gpui::Toggled::False
                            })
                            .tab_index(0)
                            .focus_visible(|style| style.border_2().border_color(theme.accent))
                            .cursor_pointer()
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
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(form) = &mut this.form {
                                    form.transport = t;
                                }
                                this.probe = None;
                                cx.notify();
                            }))
                            .child(t.label())
                    }),
            ),
        );
        block = block.child(popover::dialog_field(
            self.name_input.clone().into_any_element(),
        ));
        match transport {
            FormTransport::Http => {
                block = block
                    .child(popover::dialog_field(
                        self.url_input.clone().into_any_element(),
                    ))
                    .child(popover::dialog_field(
                        self.headers_input.clone().into_any_element(),
                    ))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.5))
                            .text_color(theme.text_muted.opacity(0.8))
                            .child(SharedString::from(if editing {
                                "Headers stay on this device. Leave blank to keep the \
                                 stored values; the echo above lists names only."
                            } else {
                                "Optional headers — comma-separated NAME=VALUE pairs \
                                 (e.g. Authorization=Bearer …). Stored only on this device."
                            })),
                    );
            }
            FormTransport::Stdio => {
                block = block
                    .child(popover::dialog_field(
                        self.command_input.clone().into_any_element(),
                    ))
                    .child(popover::dialog_field(
                        self.args_input.clone().into_any_element(),
                    ))
                    .child(popover::dialog_field(
                        self.env_input.clone().into_any_element(),
                    ))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.5))
                            .text_color(theme.text_muted.opacity(0.8))
                            .child(SharedString::from(if editing {
                                "Env values stay on this device. Leave blank to keep the \
                                 stored values; the echo above lists names only."
                            } else {
                                "Optional env — comma-separated NAME=VALUE pairs. The \
                                 server spawns on this device when an agent needs it."
                            })),
                    );
            }
        }
        // Probe feedback.
        if let Some(probe) = &self.probe {
            let line = if probe.ok {
                let sample = probe
                    .tools
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                if sample.is_empty() {
                    format!("Connected — {} tools advertised.", probe.tool_count)
                } else {
                    format!("Connected — {} tools (e.g. {sample}).", probe.tool_count)
                }
            } else {
                format!(
                    "Connection failed: {}",
                    probe.error.as_deref().unwrap_or("unknown error")
                )
            };
            block = block.child(if probe.ok {
                div()
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.success)
                    .child(SharedString::from(line))
            } else {
                widgets::warning_strip(theme, line)
            });
        }
        // Actions.
        block = block.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    Self::ghost(
                        theme,
                        "tools-server-save",
                        if editing {
                            "Save changes"
                        } else {
                            "Add server"
                        },
                    )
                    .when(self.busy, |el| el.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| this.save_provider(cx))),
                )
                .child(
                    Self::ghost(
                        theme,
                        "tools-server-probe",
                        if self.probing { "Testing…" } else { "Test" },
                    )
                    .when(self.probing, |el| el.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| this.probe(cx))),
                )
                .child(
                    Self::ghost(theme, "tools-server-cancel", "Cancel")
                        .on_click(cx.listener(|this, _, _, cx| this.close_form(cx))),
                ),
        );
        block.into_any_element()
    }

    /// The "Computer use" summary — local device only. Full driver/install
    /// surface stays on Settings → Computer; this card mirrors status and
    /// the master toggle so the tools story reads on one page.
    fn computer_section(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if self.target_device.is_some() {
            return div().into_any_element();
        }
        let mut card = widgets::section_card(theme);
        match &self.computer {
            Loadable::Ready(status) => {
                let enabled = status.settings.enabled;
                let state_label = match status.state.as_str() {
                    "disabled" => "Off",
                    "ready" => "Ready",
                    "missing" => "Component not installed",
                    "incomplete" => "Component incomplete",
                    "unsupported" => "Unsupported platform",
                    "error" => "Error",
                    other => other,
                };
                let badge = if status.state == "ready" {
                    widgets::badge_active(theme, state_label)
                } else {
                    widgets::badge(theme, state_label)
                };
                let toggle = widgets::toggle_switch(theme, enabled, "tools-computer-enabled")
                    .id("tools-computer-enabled")
                    .role(gpui::Role::Switch)
                    .aria_label("Screen Context")
                    .aria_toggled(if enabled {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .tab_index(0)
                    .focus_visible(|style| style.border_2().border_color(theme.accent))
                    .cursor_pointer()
                    .when(self.computer_saving, |el| el.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_computer(cx)));
                card = card.child(Self::row(
                    theme,
                    true,
                    "Screen Context",
                    "Let agents see this device's screen — semantic UI text first, \
                     screenshots only when needed.",
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(badge)
                        .child(toggle)
                        .into_any_element(),
                ));
                card =
                    card.child(Self::row(
                        theme,
                        false,
                        "Control",
                        &format!(
                            "{} mode · driver {}",
                            status.settings.control.label(),
                            status
                                .driver_version
                                .as_deref()
                                .map(|v| format!("{v} installed"))
                                .unwrap_or_else(|| "not installed".into()),
                        ),
                        Self::ghost(theme, "tools-computer-open", "Manage")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(ToolsPageEvent::OpenComputerSettings)
                            }))
                            .into_any_element(),
                    ));
            }
            Loadable::Error(message) => {
                card = card.child(Self::row(
                    theme,
                    true,
                    "Screen Context",
                    "Let agents see this device's screen.",
                    Self::ghost(theme, "tools-computer-retry", "Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.load_computer(cx)))
                        .into_any_element(),
                ));
                card = card.child(widgets::warning_strip(theme, message.clone()));
            }
            _ => {
                card = card.child(Self::row(
                    theme,
                    true,
                    "Screen Context",
                    "Checking computer status…",
                    widgets::badge(theme, "…").into_any_element(),
                ));
            }
        }
        card.into_any_element()
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for ToolsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl gpui::EventEmitter<ToolsPageEvent> for ToolsPage {}

impl Render for ToolsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let body: AnyElement = match &self.snapshot {
            Loadable::Idle | Loadable::Loading => widgets::section_card(&theme)
                .p(px(16.0))
                .child(popover::skeleton_rows(
                    "tools-skeleton",
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
                            .id("tools-retry")
                            .mt(px(8.0))
                            .hover(|s| s.bg(theme.glass_hover()).text_color(theme.text))
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.load(cx);
                                cx.notify();
                            }))
                            .child(SharedString::from("Retry")),
                    )
                    .into_any_element()
            }
            Loadable::Ready(_) => div()
                .child(widgets::field_label(&theme, "Apps"))
                .child(self.composio_section(&theme, cx))
                .child(
                    div()
                        .mt(px(28.0))
                        .child(widgets::field_label(&theme, "MCP servers & plugins")),
                )
                .child(self.servers_section(&theme, cx))
                .child(
                    div()
                        .mt(px(28.0))
                        .child(widgets::field_label(&theme, "Computer use")),
                )
                .child(self.computer_section(&theme, cx))
                .into_any_element(),
        };
        let error = self
            .error
            .clone()
            .map(|message| widgets::error_strip(&theme, message).into_any_element());
        let tool_count = match &self.snapshot {
            Loadable::Ready(s) => Some(s.tool_count),
            _ => None,
        };
        let scrollbar = popover::rail(self, "tools-page-scrollbar", &theme, cx);

        div()
            .id("tools-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                div()
                    .id("tools-page")
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
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(10.0))
                                            .child(widgets::page_header(&theme, "Tools", None))
                                            .children(tool_count.map(|n| {
                                                widgets::badge(&theme, format!("{n} tools"))
                                                    .into_any_element()
                                            })),
                                    )
                                    .child(self.render_device_switcher(&theme, cx)),
                            )
                            .child(
                                widgets::page_subtitle(
                                    &theme,
                                    "One gateway gives every agent real-world tools — \
                                     Composio apps, MCP servers, and computer use — no \
                                     matter which harness runs the session. Configuration \
                                     lives on the device that hosts the agent.",
                                )
                                .max_w(px(512.0))
                                .line_height(px(20.0)),
                            )
                            .children(error)
                            .child(body),
                    ),
            )
            .children(scrollbar)
    }
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// `NAME=VALUE, NAME2=VALUE2` → map (first `=` splits; surrounding whitespace
/// trimmed). Used for headers and env fields alike.
fn parse_pairs(text: &str) -> BTreeMap<String, String> {
    text.split(',')
        .filter_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            let name = name.trim();
            (!name.is_empty()).then(|| (name.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// Space-separated args field → argv (no quoting — stdio presets keep to
/// bare flags/paths; quoting complexity belongs to a proper arg editor).
fn parse_args(text: &str) -> Vec<String> {
    text.split_whitespace().map(String::from).collect()
}

/// `google_calendar` → `Google Calendar` for toolkits outside the curated
/// grid (search results, remembered toolkits).
fn prettify_toolkit(slug: &str) -> String {
    slug.split('_')
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
