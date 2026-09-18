//! macOS Delegate backend: a second Carbon `RegisterEventHotKey` under the
//! `ZDLG` signature, sharing the same keymap plumbing as the Appshot hotkey.
//! Registration lives on the main thread (`refresh_global_shortcut` is
//! invoked from `delegate::refresh`), the handler itself only forwards into
//! an activation channel.
use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use core_foundation::string::CFStringRef;
use futures::channel::mpsc;

use crate::appshots::CapabilityState;
use crate::global_hotkey::Shortcut;

const COMMAND_KEY: u32 = 1 << 8;
const SHIFT_KEY: u32 = 1 << 9;
const OPTION_KEY: u32 = 1 << 11;
const CONTROL_KEY: u32 = 1 << 12;
const EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const EVENT_HOT_KEY_PRESSED: u32 = 5;
const DELEGATE_HOT_KEY_SIGNATURE: u32 = u32::from_be_bytes(*b"ZDLG");

static READY: AtomicBool = AtomicBool::new(true);

/// The settings badge reflects the last registration attempt.
pub(super) fn shortcut_state() -> CapabilityState {
    if READY.load(Ordering::Relaxed) {
        CapabilityState::Ready
    } else {
        CapabilityState::SetupRequired
    }
}

type EventTargetRef = *mut c_void;
type EventHandlerCallRef = *mut c_void;
type EventRef = *mut c_void;
type EventHotKeyRef = *mut c_void;

#[repr(C)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
struct EventHotKeyId {
    signature: u32,
    id: u32,
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> core_foundation::base::CFTypeRef;
    fn TISGetInputSourceProperty(
        source: core_foundation::base::CFTypeRef,
        property: CFStringRef,
    ) -> *const c_void;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    fn UCKeyTranslate(
        layout: *const c_void,
        key_code: u16,
        action: u16,
        modifiers: u32,
        keyboard_type: u32,
        options: u32,
        dead_key_state: *mut u32,
        max_length: usize,
        actual_length: *mut usize,
        output: *mut u16,
    ) -> i32;
    fn LMGetKbdType() -> u16;
    fn UnregisterEventHotKey(hot_key: EventHotKeyRef) -> i32;
    fn GetApplicationEventTarget() -> EventTargetRef;
    fn InstallEventHandler(
        target: EventTargetRef,
        handler: Option<unsafe extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> i32>,
        event_type_count: u32,
        event_types: *const EventTypeSpec,
        user_data: *mut c_void,
        out_handler: *mut *mut c_void,
    ) -> i32;
    fn RegisterEventHotKey(
        key_code: u32,
        modifiers: u32,
        hot_key_id: EventHotKeyId,
        target: EventTargetRef,
        options: u32,
        out_ref: *mut EventHotKeyRef,
    ) -> i32;
}

struct HotKeyRegistration {
    target: EventTargetRef,
    hot_key: EventHotKeyRef,
    shortcut: Option<Shortcut>,
}

thread_local! {
    // Carbon registration and replacement always run on GPUI's main thread.
    static HOT_KEY: RefCell<Option<HotKeyRegistration>> = const { RefCell::new(None) };
}

pub(super) fn start_shortcut(_activation_dir: &std::path::Path) -> mpsc::UnboundedReceiver<()> {
    let (tx, rx) = mpsc::unbounded();
    let sender = Box::into_raw(Box::new(tx)).cast::<c_void>();
    let event_type = EventTypeSpec {
        event_class: EVENT_CLASS_KEYBOARD,
        event_kind: EVENT_HOT_KEY_PRESSED,
    };
    let target = unsafe { GetApplicationEventTarget() };
    let status = unsafe {
        InstallEventHandler(
            target,
            Some(delegate_hot_key_handler),
            1,
            &event_type,
            sender,
            ptr::null_mut(),
        )
    };
    if status != 0 {
        unsafe { drop(Box::from_raw(sender.cast::<mpsc::UnboundedSender<()>>())) };
        READY.store(false, Ordering::Relaxed);
        tracing::warn!(status, "Delegate global shortcut handler unavailable");
        return rx;
    }
    HOT_KEY.with(|registration| {
        *registration.borrow_mut() = Some(HotKeyRegistration {
            target,
            hot_key: ptr::null_mut(),
            shortcut: None,
        })
    });
    refresh_global_shortcut(super::current());
    rx
}

/// Re-register the combo after a preferences change. Called from
/// `delegate::refresh` on the main thread; `None` tears the registration down
/// (feature disabled or a new combo is being recorded).
pub(super) fn refresh_global_shortcut(shortcut: Option<Shortcut>) {
    HOT_KEY.with(|registration| {
        let mut registration = registration.borrow_mut();
        let Some(registration) = registration.as_mut() else {
            return;
        };
        if registration.shortcut == shortcut {
            return;
        }
        if !registration.hot_key.is_null() {
            unsafe {
                UnregisterEventHotKey(registration.hot_key);
            }
            registration.hot_key = ptr::null_mut();
        }
        registration.shortcut = shortcut.clone();
        let Some(shortcut) = shortcut else {
            return;
        };
        let Some(key_code) = shortcut_keycode(&shortcut.key) else {
            READY.store(false, Ordering::Relaxed);
            return;
        };
        let modifiers = (if shortcut.control { CONTROL_KEY } else { 0 })
            | (if shortcut.alt { OPTION_KEY } else { 0 })
            | (if shortcut.shift { SHIFT_KEY } else { 0 })
            | (if shortcut.platform { COMMAND_KEY } else { 0 });
        let status = unsafe {
            RegisterEventHotKey(
                key_code,
                modifiers,
                EventHotKeyId {
                    signature: DELEGATE_HOT_KEY_SIGNATURE,
                    id: crate::global_hotkey::GlobalHotkeyId::Delegate as u32,
                },
                registration.target,
                0,
                &mut registration.hot_key,
            )
        };
        READY.store(status == 0, Ordering::Relaxed);
        if status != 0 {
            tracing::warn!(status, "Delegate global shortcut unavailable");
        }
    });
}

fn shortcut_keycode(key: &str) -> Option<u32> {
    let named = match key {
        "space" => 49,
        "tab" => 48,
        "enter" => 36,
        "backspace" => 51,
        "delete" => 117,
        "insert" => 114,
        "up" => 126,
        "down" => 125,
        "left" => 123,
        "right" => 124,
        "home" => 115,
        "end" => 119,
        "pageup" => 116,
        "pagedown" => 121,
        _ => {
            if let Some(index) = key.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
                return index.checked_sub(1).and_then(|index| {
                    [
                        122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106,
                        64, 79, 80, 90,
                    ]
                    .get(index)
                    .copied()
                });
            }
            return layout_keycode(key);
        }
    };
    Some(named)
}

fn layout_keycode(key: &str) -> Option<u32> {
    // Resolve the printed key against the active ASCII-capable layout. A US
    // positional table would capture Q when an AZERTY user records A.
    unsafe {
        let source = TISCopyCurrentASCIICapableKeyboardLayoutInputSource();
        if source.is_null() {
            return None;
        }
        let data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        let result = if data.is_null() {
            None
        } else {
            let layout = core_foundation::data::CFDataGetBytePtr(data.cast()).cast();
            let mut found = None;
            'levels: for modifiers in [0, 2] {
                for code in 0..128_u16 {
                    let mut output = [0_u16; 4];
                    let mut length = 0;
                    let mut dead = 0;
                    if UCKeyTranslate(
                        layout,
                        code,
                        0,
                        modifiers,
                        u32::from(LMGetKbdType()),
                        1,
                        &mut dead,
                        output.len(),
                        &mut length,
                        output.as_mut_ptr(),
                    ) == 0
                        && length <= output.len()
                        && String::from_utf16_lossy(&output[..length]).to_lowercase() == key
                    {
                        found = Some(u32::from(code));
                        break 'levels;
                    }
                }
            }
            found
        };
        core_foundation::base::CFRelease(source);
        result
    }
}

unsafe extern "C" fn delegate_hot_key_handler(
    _call: EventHandlerCallRef,
    _event: EventRef,
    user_data: *mut c_void,
) -> i32 {
    if !user_data.is_null() {
        let sender = unsafe { &*user_data.cast::<mpsc::UnboundedSender<()>>() };
        let _ = sender.unbounded_send(());
    }
    0
}
