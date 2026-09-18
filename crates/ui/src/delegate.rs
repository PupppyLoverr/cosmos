//! Delegate — a system-wide "give Cosmos a task" bar.
//!
//! A configurable global hotkey (default `Alt+Space`) summons a floating
//! Spotlight-style card from any application. The card is a real composer in
//! delegate mode: every send mints a fresh session through the ordinary
//! new-chat pipeline, so the delegated task appears in the sidebar and
//! notifies like any other run — without yanking the main window's
//! selection away from whatever the user was reading.
//!
//! Shortcut plumbing mirrors the Appshots feature: a small preferences block
//! (combo + recording gate + Linux live-rebind subscribers) feeding a
//! per-platform registration backend.
//!
//! - Windows: a dedicated thread registers `RegisterHotKey` against its own
//!   message queue (`hWnd = NULL`) and polls `WM_HOTKEY`.
//! - macOS: a second Carbon `RegisterEventHotKey` under the `ZDLG` signature.
//! - Linux Wayland: the Global Shortcuts portal; X11: a passive key grab;
//!   either way a `delegate-activation.sock` datagram socket accepts manual
//!   triggers (`cosmos delegate`) for desktops that bind shortcuts natively.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod view;
#[cfg(target_os = "windows")]
mod windows;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui::{
    App, AppContext as _, Bounds, Entity, WindowBounds, WindowKind, WindowOptions, point, px, size,
};

use crate::appshots::CapabilityState;
use crate::global_hotkey::Shortcut;
use crate::state::AppState;
use view::DelegateBar;

/// Bar dimensions. The card is 96px of composer plus a delegated-status row
/// that materializes after the first send; the window is transparent, so a
/// margin leaves room for the drop shadow.
pub(crate) const BAR_WIDTH: f32 = 560.0;
const BAR_HEIGHT: f32 = 168.0;
const BAR_SHADOW_MARGIN: f32 = 12.0;

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Settings → Shortcuts toggle + boot wiring. `set_enabled` re-evaluates the
/// registered combo so the hotkey is torn down the moment the feature is off.
pub fn set_enabled(enabled: bool) {
    if ENABLED.swap(enabled, Ordering::Relaxed) != enabled {
        refresh();
    }
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

// ---- shortcut preferences (combo + recording gate + subscribers) ----

struct Preferences {
    shortcut: Shortcut,
    recording: bool,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    subscribers: Vec<mpsc::UnboundedSender<Option<Shortcut>>>,
}

fn preferences() -> &'static Mutex<Preferences> {
    static PREFERENCES: OnceLock<Mutex<Preferences>> = OnceLock::new();
    PREFERENCES.get_or_init(|| {
        Mutex::new(Preferences {
            shortcut: Shortcut::parse(crate::settings::ShortcutId::DelegateTask.default_combo())
                .unwrap(),
            recording: false,
            subscribers: Vec::new(),
        })
    })
}

impl Preferences {
    fn active(&self) -> Option<Shortcut> {
        (enabled() && !self.recording).then(|| self.shortcut.clone())
    }
}

/// The combo currently registered with the OS (`None` while disabled or
/// while a new combo is being recorded in Settings).
pub(crate) fn current() -> Option<Shortcut> {
    preferences().lock().unwrap().active()
}

/// Linux backends rebind live: the portal/X11 loops consume these updates
/// instead of being restarted per combo change.
#[cfg(target_os = "linux")]
pub(crate) fn subscribe() -> mpsc::UnboundedReceiver<Option<Shortcut>> {
    let (tx, rx) = mpsc::unbounded();
    let mut preferences = preferences().lock().unwrap();
    let _ = tx.unbounded_send(preferences.active());
    preferences.subscribers.push(tx);
    rx
}

/// Push the active combo at the platform backend after any preference change.
fn refresh() {
    let active = {
        let mut preferences = preferences().lock().unwrap();
        let active = preferences.active();
        preferences
            .subscribers
            .retain(|tx| tx.unbounded_send(active.clone()).is_ok());
        active
    };
    // macOS registers on this (main) thread and needs the push. Windows and
    // Linux backends pull `current()`/`subscribe()` on their own threads.
    #[cfg(target_os = "macos")]
    macos::refresh_global_shortcut(active);
    #[cfg(not(target_os = "macos"))]
    let _ = active;
}

pub fn set_shortcut(combo: &str) {
    let shortcut = Shortcut::parse(combo).unwrap_or_else(|_| {
        Shortcut::parse(crate::settings::ShortcutId::DelegateTask.default_combo()).unwrap()
    });
    if preferences().lock().unwrap().shortcut == shortcut {
        return;
    }
    {
        let mut preferences = preferences().lock().unwrap();
        preferences.shortcut = shortcut;
    }
    refresh();
}

/// While Settings records a combo, the live registration is suspended so the
/// recorded chord is never swallowed by the OS-level handler.
pub fn set_recording(recording: bool) {
    if preferences().lock().unwrap().recording == recording {
        return;
    }
    {
        let mut preferences = preferences().lock().unwrap();
        preferences.recording = recording;
    }
    refresh();
}

pub fn validate_shortcut(combo: &str) -> Result<(), ()> {
    Shortcut::parse(combo).map(|_| ())
}

/// Settings → Shortcuts capability badge for the Delegate row.
pub fn shortcut_state() -> CapabilityState {
    #[cfg(target_os = "linux")]
    {
        linux::shortcut_state()
    }
    #[cfg(target_os = "windows")]
    {
        windows::shortcut_state()
    }
    #[cfg(target_os = "macos")]
    {
        macos::shortcut_state()
    }
}

fn activation_allowed() -> bool {
    current().is_some()
}

// ---- service: activation stream → toggle ----

/// Wire the platform registration loop and (on Linux) the activation socket
/// into the app: every activation toggles the bar.
pub fn start(state: Entity<AppState>, activation_dir: PathBuf, cx: &mut App) {
    let mut activations = platform::start_shortcut(&activation_dir);
    cx.spawn(async move |cx| {
        while activations.next().await.is_some() {
            if !activation_allowed() {
                continue;
            }
            cx.update(|cx| toggle(&state, cx));
        }
    })
    .detach();
}

/// Hotkey toggle: close the bar when it's open, summon it otherwise.
pub(crate) fn toggle(state: &Entity<AppState>, cx: &mut App) {
    if let Some(handle) = bar_window(cx) {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        return;
    }
    open(state, cx);
}

/// Open the bar (the tray menu's "Delegate a task" entry point): an existing
/// bar gets activated rather than duplicated.
pub(crate) fn open(state: &Entity<AppState>, cx: &mut App) {
    if let Some(handle) = bar_window(cx) {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
        return;
    }
    open_window(state, cx);
}

fn bar_window(cx: &App) -> Option<gpui::WindowHandle<DelegateBar>> {
    cx.windows()
        .into_iter()
        .find_map(|window| window.downcast::<DelegateBar>())
}

fn open_window(state: &Entity<AppState>, cx: &mut App) {
    let card = size(px(BAR_WIDTH), px(BAR_HEIGHT));
    let bounds = positioned_bounds(card, cx);
    let state = state.clone();
    let result = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: true,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: true,
            is_resizable: false,
            is_minimizable: false,
            window_background: gpui::WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        move |window, cx| {
            window.set_rem_size(px(crate::typography::font_size(cx).pixels()));
            crate::appearance::observe_window(window, cx).detach();
            cx.new(|cx| DelegateBar::new(state.clone(), window, cx))
        },
    );
    if let Ok(handle) = result {
        #[cfg(target_os = "windows")]
        handle
            .update(cx, |_, window, _| windows::decorate(window))
            .ok();
    }
}

/// Centered horizontally, ~30% down the primary display — the Spotlight slot.
/// `Bounds::centered` alone would put the card dead-center.
fn positioned_bounds(card: gpui::Size<gpui::Pixels>, cx: &App) -> Bounds<gpui::Pixels> {
    let Some(display) = cx.primary_display() else {
        return Bounds::centered(None, card, cx);
    };
    let screen = display.bounds();
    Bounds {
        origin: point(
            screen.center().x - card.width / 2.0 - px(BAR_SHADOW_MARGIN),
            screen.origin.y + screen.size.height * 0.28,
        ),
        size: gpui::size(
            card.width + px(BAR_SHADOW_MARGIN * 2.0),
            card.height + px(BAR_SHADOW_MARGIN * 2.0),
        ),
    }
}

/// Ask the running headed Zeron process to toggle the bar. Linux desktops
/// that lack the Global Shortcuts portal can bind `cosmos delegate` in their
/// native keyboard settings.
#[cfg(target_os = "linux")]
pub fn request_running_delegate(data_dir: &Path) -> Result<(), crate::appshots::CaptureError> {
    linux::request_running_delegate(data_dir)
}

#[cfg(not(target_os = "linux"))]
pub fn request_running_delegate(_data_dir: &Path) -> Result<(), crate::appshots::CaptureError> {
    Err(crate::appshots::CaptureError::ShortcutUnavailable)
}

// ---- platform backend dispatch ----

#[cfg(target_os = "macos")]
mod platform {
    pub(super) use super::macos::start_shortcut;
}

#[cfg(target_os = "windows")]
mod platform {
    pub(super) use super::windows::start_shortcut;
}

#[cfg(target_os = "linux")]
mod platform {
    pub(super) use super::linux::start_shortcut;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_delegate_combo_parses() {
        assert!(Shortcut::parse(crate::settings::ShortcutId::DelegateTask.default_combo()).is_ok());
    }
}
