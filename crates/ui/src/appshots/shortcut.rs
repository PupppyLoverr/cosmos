//! Shared user preference and live registration updates for desktop hotkeys.
use std::sync::{Mutex, OnceLock};

use futures::channel::mpsc;

pub(crate) use crate::global_hotkey::Shortcut;

struct Preferences {
    shortcut: Shortcut,
    recording: bool,
    subscribers: Vec<mpsc::UnboundedSender<Option<Shortcut>>>,
}

fn preferences() -> &'static Mutex<Preferences> {
    static PREFERENCES: OnceLock<Mutex<Preferences>> = OnceLock::new();
    PREFERENCES.get_or_init(|| {
        Mutex::new(Preferences {
            shortcut: Shortcut::parse(crate::settings::ShortcutId::CaptureAppshot.default_combo())
                .unwrap(),
            recording: false,
            subscribers: Vec::new(),
        })
    })
}

impl Preferences {
    fn active(&self) -> Option<Shortcut> {
        (super::enabled() && !self.recording).then(|| self.shortcut.clone())
    }
}

pub(crate) fn current() -> Option<Shortcut> {
    preferences().lock().unwrap().active()
}

#[cfg(target_os = "linux")]
pub(crate) fn subscribe() -> mpsc::UnboundedReceiver<Option<Shortcut>> {
    let (tx, rx) = mpsc::unbounded();
    let mut preferences = preferences().lock().unwrap();
    let _ = tx.unbounded_send(preferences.active());
    preferences.subscribers.push(tx);
    rx
}

fn update(mutate: impl FnOnce(&mut Preferences)) {
    let active = {
        let mut preferences = preferences().lock().unwrap();
        mutate(&mut preferences);
        let active = preferences.active();
        preferences
            .subscribers
            .retain(|tx| tx.unbounded_send(active.clone()).is_ok());
        active
    };
    #[cfg(target_os = "macos")]
    super::macos::refresh_global_shortcut(active);
    #[cfg(not(target_os = "macos"))]
    let _ = active;
}

pub(super) fn enabled_changed() {
    update(|_| {});
}

pub fn set_shortcut(combo: &str) {
    if !super::is_desktop() {
        return;
    }
    let shortcut = Shortcut::parse(combo).unwrap_or_else(|_| {
        Shortcut::parse(crate::settings::ShortcutId::CaptureAppshot.default_combo()).unwrap()
    });
    if preferences().lock().unwrap().shortcut == shortcut {
        return;
    }
    update(|preferences| preferences.shortcut = shortcut);
}

pub fn set_recording(recording: bool) {
    if !super::is_desktop() || preferences().lock().unwrap().recording == recording {
        return;
    }
    update(|preferences| preferences.recording = recording);
}

pub fn validate_shortcut(combo: &str) -> Result<(), ()> {
    Shortcut::parse(combo).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn global_shortcuts_require_a_supported_modified_key() {
        for combo in ["a", "shift-a", "fn-a", "ctrl-mystery", "ctrl-f25"] {
            assert!(Shortcut::parse(combo).is_err(), "{combo}");
        }
        for combo in ["ctrl-alt-space", "mod-shift-k", "alt-f12", "ctrl-pageup"] {
            assert!(Shortcut::parse(combo).is_ok(), "{combo}");
        }
    }
    #[test]
    fn portal_uses_xkb_key_names_and_selected_modifiers() {
        assert_eq!(
            Shortcut::parse("ctrl-alt-space").unwrap().portal_trigger(),
            "CTRL+ALT+space"
        );
        assert_eq!(
            Shortcut::parse("ctrl-shift-pageup")
                .unwrap()
                .portal_trigger(),
            "CTRL+SHIFT+Page_Up"
        );
    }
}

pub(crate) fn capture_allowed() -> bool {
    current().is_some()
}
