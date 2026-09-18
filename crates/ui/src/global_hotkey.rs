//! Shared parsing for OS-level global hotkeys.
//!
//! `Shortcut` is the platform-neutral description of a registered global
//! shortcut: a printable key plus the modifiers every backend understands.
//! The Appshots and Delegate features each keep their own preferences and
//! platform backends but share this parsing layer, so a combo that parses
//! for one is registerable by the other.
use gpui::Keystroke;

/// Identifier for a registered global hotkey. Windows backends use it as the
/// `RegisterHotKey` id; macOS uses it as the Carbon `EventHotKeyID.id`
/// discriminator so two features never collide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub(crate) enum GlobalHotkeyId {
    Appshot = 1,
    Delegate = 2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Shortcut {
    pub key: String,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
}

impl Shortcut {
    pub fn parse(combo: &str) -> Result<Self, ()> {
        let stroke = Keystroke::parse(&crate::settings::platform_combo(combo)).map_err(|_| ())?;
        let modifiers = stroke.modifiers;
        let key = stroke.key;
        let supported = (key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric())
            || matches!(
                key.as_str(),
                "space"
                    | "tab"
                    | "enter"
                    | "backspace"
                    | "delete"
                    | "insert"
                    | "up"
                    | "down"
                    | "left"
                    | "right"
                    | "home"
                    | "end"
                    | "pageup"
                    | "pagedown"
            )
            || key
                .strip_prefix('f')
                .and_then(|n| n.parse::<u8>().ok())
                .is_some_and(|n| (1..=20).contains(&n));
        if !supported
            || modifiers.function
            || !(modifiers.control || modifiers.alt || modifiers.platform)
        {
            return Err(());
        }
        Ok(Self {
            key,
            control: modifiers.control,
            alt: modifiers.alt,
            shift: modifiers.shift,
            platform: modifiers.platform,
        })
    }

    #[cfg(any(target_os = "linux", test))]
    pub fn portal_trigger(&self) -> String {
        let mut parts = Vec::new();
        if self.control {
            parts.push("CTRL".to_owned());
        }
        if self.alt {
            parts.push("ALT".to_owned());
        }
        if self.shift {
            parts.push("SHIFT".to_owned());
        }
        if self.platform {
            parts.push("LOGO".to_owned());
        }
        parts.push(match self.key.as_str() {
            "enter" => "Return".into(),
            "backspace" => "BackSpace".into(),
            "pageup" => "Page_Up".into(),
            "pagedown" => "Page_Down".into(),
            "space" => "space".into(),
            key if key.len() == 1 => key.into(),
            key => {
                let mut chars = key.chars();
                chars.next().unwrap().to_uppercase().collect::<String>() + chars.as_str()
            }
        });
        parts.join("+")
    }
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
