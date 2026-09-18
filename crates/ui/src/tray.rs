//! Close-to-tray: a notification-area icon keeps Cosmos reachable after the
//! last window closes.
//!
//! With `UiSettings::keep_running_in_tray` on and the icon successfully
//! registered, `run_app` flips gpui to `QuitMode::Explicit` — closing the
//! window then leaves the engine, the Delegate global hotkey and this icon
//! alive (running tasks keep going; completion notifications still fire).
//! The icon's menu is the always-visible way back (Open Cosmos) and the
//! clean way out (Quit — routed through `app_menus::request_quit`, the
//! flush-then-quit path).
//!
//! Ordering is the guardrail: `start` runs BEFORE `QuitMode::Explicit` is
//! set, and only a confirmed registration flips the mode — a failed icon
//! plus explicit quit would strand the process windowless with no exit.
//!
//! Each platform backend owns its icon lifetime behind the same
//! `start_icon`/`shutdown`/`set_autostart` contract:
//!
//! - Windows: a message-only HWND + `GetMessage` loop on a dedicated thread
//!   (`Shell_NotifyIcon`), the bridge pattern `delegate::windows` uses.
//! - macOS: an `NSStatusItem` + `NSMenu` on the main thread, with a
//!   `~/Library/LaunchAgents` plist for launch-at-login.
//! - Linux: a StatusNotifierItem over D-Bus (`ksni`), with an XDG autostart
//!   desktop entry; no StatusNotifierWatcher → `start_icon` reports no icon
//!   and callers keep `QuitMode::Default`.

use futures::StreamExt as _;
use gpui::{App, Entity};

use crate::state::AppState;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Menu commands the platform backend forwards into gpui.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayAction {
    Open,
    Delegate,
    Quit,
}

/// Install the icon and start forwarding its menu commands. Returns `false`
/// when the backend couldn't register — callers MUST keep the default
/// last-window-closed quit mode in that case.
pub(crate) fn start(state: &Entity<AppState>, cx: &mut App) -> bool {
    let Some(mut events) = platform::start_icon() else {
        return false;
    };
    let state = state.clone();
    cx.spawn(async move |cx| {
        while let Some(action) = events.next().await {
            let state = state.clone();
            let _ = cx.update(|cx| dispatch(action, &state, cx));
        }
    })
    .detach();
    true
}

fn dispatch(action: TrayAction, state: &Entity<AppState>, cx: &mut App) {
    match action {
        TrayAction::Open => crate::open_main(cx),
        TrayAction::Delegate => crate::delegate::open(state, cx),
        TrayAction::Quit => crate::app_menus::request_quit(cx),
    }
}

/// Remove the icon and stop the backend. Called from the quit path so no
/// ghost icon lingers in the notification area.
pub(crate) fn shutdown() {
    platform::shutdown();
}

/// Live toggle from Settings: install or remove the icon and align the quit
/// mode with what actually happened. Enabling with a failed backend is a
/// no-op — same guardrail as boot.
pub(crate) fn set_keep_running(state: &Entity<AppState>, enabled: bool, cx: &mut App) {
    if enabled {
        if start(state, cx) {
            cx.set_quit_mode(gpui::QuitMode::Explicit);
        }
    } else {
        cx.set_quit_mode(gpui::QuitMode::Default);
        shutdown();
    }
}

/// Sync the OS launch-at-login entry with the setting. Entries launch the
/// exe with `--tray`, which boots windowless into this same icon.
/// Independent of `keep_running_in_tray` — a user can autostart the app
/// (window opens normally) without close-to-tray.
pub(crate) fn sync_autostart(enabled: bool) {
    platform::set_autostart(enabled);
}

#[cfg(target_os = "windows")]
mod platform {
    pub(super) use super::windows::{set_autostart, shutdown, start_icon};
}

#[cfg(target_os = "macos")]
mod platform {
    pub(super) use super::macos::{set_autostart, shutdown, start_icon};
}

#[cfg(target_os = "linux")]
mod platform {
    pub(super) use super::linux::{set_autostart, shutdown, start_icon};
}
