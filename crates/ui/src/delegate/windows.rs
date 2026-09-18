//! Windows Delegate backend: a dedicated thread owns the `RegisterHotKey`
//! registration (hotkeys with `hWnd = NULL` post `WM_HOTKEY` to the owning
//! thread's message queue) and re-reads the shared preferences each poll, so
//! Settings rebinding needs no cross-thread signaling — the same pull model
//! the X11 grab uses.
//!
//! The window dressing (`decorate`) is also Windows-specific: gpui's
//! `WindowKind::PopUp` maps to a borderless `WS_EX_TOOLWINDOW`, which is NOT
//! topmost and keeps square DWM corners — a Spotlight bar needs both fixed.
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use futures::channel::mpsc;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmSetWindowAttribute,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey, VIRTUAL_KEY, VK_BACK, VK_DELETE, VK_DOWN, VK_END, VK_F1, VK_HOME, VK_INSERT,
    VK_LEFT, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_TOPMOST, MSG, PM_REMOVE, PeekMessageW, SET_WINDOW_POS_FLAGS, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SetWindowPos, WM_HOTKEY,
};

use crate::global_hotkey::{GlobalHotkeyId, Shortcut};

const HOTKEY_ID: i32 = GlobalHotkeyId::Delegate as i32;
static READY: AtomicBool = AtomicBool::new(true);

/// `DelegateState` display work only — the settings badge reflects the last
/// registration attempt.
pub(super) fn shortcut_state() -> crate::appshots::CapabilityState {
    if READY.load(Ordering::Relaxed) {
        crate::appshots::CapabilityState::Ready
    } else {
        crate::appshots::CapabilityState::SetupRequired
    }
}

pub(super) fn start_shortcut(_activation_dir: &std::path::Path) -> mpsc::UnboundedReceiver<()> {
    let (tx, rx) = mpsc::unbounded();
    thread::Builder::new()
        .name("delegate-win-hotkey".into())
        .spawn(move || run(tx))
        .ok();
    rx
}

fn run(tx: mpsc::UnboundedSender<()>) {
    let mut registered = false;
    let mut previous: Option<Shortcut> = None;
    while !tx.is_closed() {
        let shortcut = super::current();
        if previous != shortcut {
            if registered {
                unsafe {
                    let _ = UnregisterHotKey(None, HOTKEY_ID);
                }
                registered = false;
            }
            if let Some(shortcut) = &shortcut {
                match shortcut
                    .vk_code()
                    .map(|vk| (vk, modifiers_of(shortcut)))
                    .map(|(vk, modifiers)| unsafe {
                        RegisterHotKey(None, HOTKEY_ID, modifiers, u32::from(vk.0))
                    }) {
                    Some(Ok(())) => {
                        registered = true;
                        READY.store(true, Ordering::Relaxed);
                    }
                    Some(Err(error)) => {
                        READY.store(false, Ordering::Relaxed);
                        tracing::warn!(?error, "Delegate global shortcut unavailable");
                    }
                    None => {
                        READY.store(false, Ordering::Relaxed);
                        tracing::warn!(key = %shortcut.key, "Delegate shortcut key unmappable");
                    }
                }
            }
            previous = shortcut;
        }
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() } {
            if message.message == WM_HOTKEY && message.wParam.0 as i32 == HOTKEY_ID {
                let _ = tx.unbounded_send(());
            }
        }
        thread::sleep(Duration::from_millis(15));
    }
    if registered {
        unsafe {
            let _ = UnregisterHotKey(None, HOTKEY_ID);
        }
    }
}

trait ShortcutVk {
    fn vk_code(&self) -> Option<VIRTUAL_KEY>;
}

impl ShortcutVk for Shortcut {
    fn vk_code(&self) -> Option<VIRTUAL_KEY> {
        Some(match self.key.as_str() {
            "space" => VK_SPACE,
            "tab" => VK_TAB,
            "enter" => VK_RETURN,
            "backspace" => VK_BACK,
            "delete" => VK_DELETE,
            "insert" => VK_INSERT,
            "up" => VK_UP,
            "down" => VK_DOWN,
            "left" => VK_LEFT,
            "right" => VK_RIGHT,
            "home" => VK_HOME,
            "end" => VK_END,
            "pageup" => VK_PRIOR,
            "pagedown" => VK_NEXT,
            key if key.len() == 1 => {
                // A–Z and 0–9 virtual-key codes equal the ASCII byte.
                let byte = key.as_bytes()[0].to_ascii_uppercase();
                if byte.is_ascii_alphanumeric() {
                    VIRTUAL_KEY(u16::from(byte))
                } else {
                    return None;
                }
            }
            key => {
                let index: u16 = key.strip_prefix('f')?.parse().ok()?;
                if !(1..=20).contains(&index) {
                    return None;
                }
                VIRTUAL_KEY(VK_F1.0 + index - 1)
            }
        })
    }
}

fn modifiers_of(shortcut: &Shortcut) -> HOT_KEY_MODIFIERS {
    let mut modifiers = MOD_NOREPEAT;
    if shortcut.control {
        modifiers |= MOD_CONTROL;
    }
    if shortcut.alt {
        modifiers |= MOD_ALT;
    }
    if shortcut.shift {
        modifiers |= MOD_SHIFT;
    }
    if shortcut.platform {
        modifiers |= MOD_WIN;
    }
    modifiers
}

/// gpui's `PopUp` is a borderless tool window but NOT topmost and keeps the
/// compositor's default square corners — promote it once the window exists.
pub(super) fn decorate(window: &mut gpui::Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(win32.hwnd.get() as *mut core::ffi::c_void);
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SET_WINDOW_POS_FLAGS(SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOACTIVATE.0),
        );
        let preference = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const DWM_WINDOW_CORNER_PREFERENCE as *const core::ffi::c_void,
            std::mem::size_of_val(&preference) as u32,
        );
    }
}
