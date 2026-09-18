//! macOS tray backend: an `NSStatusItem` carrying an `NSMenu` (Open / Delegate
//! / Quit) — the menu picks forward into the channel `tray::start` drains on
//! the gpui side, the same bridge shape `notify` uses for its click delegate.
//!
//! AppKit requires status items to be created on the main thread; `tray::start`
//! and `set_keep_running` both run inside gpui's main-thread context, so the
//! backend is thread-local — no worker thread (unlike the Windows message-pump
//! backend). `shutdown` runs `removeStatusItem:` from `tray::shutdown` on the
//! quit path so no ghost item lingers in the menu bar.
use std::cell::RefCell;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use futures::channel::mpsc;
use objc::declare::ClassDecl;
use objc::runtime::{Object, Sel};
use objc::{class, msg_send, sel, sel_impl};

use super::TrayAction;

/// `NSVariableStatusItemLength` — the item sizes itself to its image.
const VARIABLE_LENGTH: f64 = -1.0;
/// `NSMenuItem::tag` → `TrayAction`.
const TAG_OPEN: isize = 1;
const TAG_DELEGATE: isize = 2;
const TAG_QUIT: isize = 3;
/// The launchd label for the tray login item. `daemon.rs` owns `sh.zeron.app`
/// (the headless service) — this is a separate per-user LaunchAgent that
/// boots the headed app windowless via `--tray`.
const TRAY_LABEL: &str = "sh.zeron.app.tray";

thread_local! {
    /// The live status item (raw pointer as usize — main thread only, so no
    /// Send needed) and the channel its menu feeds.
    static LIVE: RefCell<Option<(usize, mpsc::UnboundedSender<TrayAction>)>> =
        const { RefCell::new(None) };
}

/// Install the status item and return its command stream. `None` when AppKit
/// can't vend one (e.g. a headless session); callers keep `QuitMode::Default`
/// in that case so the app still quits on last-window-close.
pub(super) fn start_icon() -> Option<mpsc::UnboundedReceiver<TrayAction>> {
    if LIVE.with(|live| live.borrow().is_some()) {
        // Already live: the existing item keeps dispatching; a closed channel
        // lets the caller's consumer exit immediately (Windows-parity contract).
        return Some(mpsc::unbounded().1);
    }
    unsafe {
        let bar: *mut Object = msg_send![class!(NSStatusBar), systemStatusBar];
        if bar.is_null() {
            return None;
        }
        let item: *mut Object = msg_send![bar, statusItemWithLength: VARIABLE_LENGTH];
        if item.is_null() {
            return None;
        }
        // `statusItemWithLength:`'s ownership is convention-muddled; an
        // explicit retain makes `shutdown`'s release balanced either way.
        let item: *mut Object = msg_send![item, retain];
        let button: *mut Object = msg_send![item, button];
        if !button.is_null() {
            if let Some(icon) = status_icon() {
                let _: () = msg_send![button, setImage: icon];
                let _: () = msg_send![icon, release];
            }
            let _: () = msg_send![button, setToolTip: ns_string("Cosmos")];
        }
        let (tx, rx) = mpsc::unbounded::<TrayAction>();
        let menu = build_menu();
        if menu.is_null() {
            let _: () = msg_send![bar, removeStatusItem: item];
            let _: () = msg_send![item, release];
            return None;
        }
        let _: () = msg_send![item, setMenu: menu];
        let _: () = msg_send![menu, release];
        LIVE.with_borrow_mut(|live| *live = Some((item as usize, tx)));
        Some(rx)
    }
}

/// Drop the status item and close the channel (the consumer in `tray::start`
/// then exits). Called from the quit path — must leave no ghost icon.
pub(super) fn shutdown() {
    LIVE.with_borrow_mut(|live| {
        let Some((item, _)) = live.take() else {
            return;
        };
        unsafe {
            let bar: *mut Object = msg_send![class!(NSStatusBar), systemStatusBar];
            let _: () = msg_send![bar, removeStatusItem: item as *mut Object];
            let _: () = msg_send![item as *mut Object, release];
        }
    });
}

/// The menu-bar image: the bundle's app icon when packaged (proper multi-res
/// icns), else the embedded tray PNG so unbundled dev runs still show the mark.
/// Sized to the 18pt status-item convention either way.
fn status_icon() -> Option<*mut Object> {
    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let bundle_icon: *mut Object = msg_send![app, applicationIconImage];
        let image: *mut Object = if bundle_icon.is_null() {
            let png = include_bytes!("../../../../dist/zeron-tray.png");
            let data: *mut Object =
                msg_send![class!(NSData), dataWithBytes: png.as_ptr() length: png.len()];
            let image: *mut Object = msg_send![class!(NSImage), alloc];
            msg_send![image, initWithData: data]
        } else {
            // Copy before resizing — the dock keeps using the shared original.
            msg_send![bundle_icon, copy]
        };
        if image.is_null() {
            return None;
        }
        let _: () = msg_send![image, setSize: core_graphics::geometry::CGSize::new(18.0, 18.0)];
        Some(image)
    }
}

/// Open Cosmos / Delegate a task / Quit Cosmos — same entries and order as
/// the Windows popup menu. With `menu` set, AppKit drops the menu on any
/// click; individual items carry their `TrayAction` in `tag`.
fn build_menu() -> *mut Object {
    unsafe {
        let menu: *mut Object = msg_send![class!(NSMenu), alloc];
        let menu: *mut Object = msg_send![menu, initWithTitle: ns_string("")];
        if menu.is_null() {
            return menu;
        }
        // Our items are always enabled; autoenable would need validateMenuItem.
        let _: () = msg_send![menu, setAutoenablesItems: false];
        let target = target_instance();
        add_item(menu, target, "Open Cosmos", TAG_OPEN);
        add_item(menu, target, "Delegate a task", TAG_DELEGATE);
        let separator: *mut Object = msg_send![class!(NSMenuItem), separatorItem];
        let _: () = msg_send![menu, addItem: separator];
        add_item(menu, target, "Quit Cosmos", TAG_QUIT);
        menu
    }
}

fn add_item(menu: *mut Object, target: *mut Object, title: &str, tag: isize) {
    unsafe {
        let item: *mut Object = msg_send![class!(NSMenuItem), alloc];
        let item: *mut Object = msg_send![item, initWithTitle: ns_string(title)
            action: sel!(trayAction:)
            keyEquivalent: ns_string("")];
        if item.is_null() {
            return;
        }
        let _: () = msg_send![item, setTag: tag];
        let _: () = msg_send![item, setTarget: target];
        let _: () = msg_send![menu, addItem: item];
        let _: () = msg_send![item, release];
    }
}

/// The menu-action target: a leaked `NSObject` subclass whose `trayAction:`
/// reads the clicked item's tag and forwards the `TrayAction` into the channel
/// `tray::start` consumes. Leaked deliberately — `NSMenuItem.target` is weak.
fn target_instance() -> *mut Object {
    static TARGET: OnceLock<usize> = OnceLock::new();
    *TARGET.get_or_init(|| unsafe {
        let mut decl = ClassDecl::new("ZeronTrayTarget", class!(NSObject))
            .expect("ZeronTrayTarget registered twice");
        decl.add_method(
            sel!(trayAction:),
            tray_action as extern "C" fn(&Object, Sel, *mut Object),
        );
        let class = decl.register();
        let instance: *mut Object = msg_send![class, new];
        instance as usize
    }) as *mut Object
}

extern "C" fn tray_action(_this: &Object, _sel: Sel, sender: *mut Object) {
    let tag: isize = unsafe { msg_send![sender, tag] };
    let action = match tag {
        TAG_OPEN => TrayAction::Open,
        TAG_DELEGATE => TrayAction::Delegate,
        TAG_QUIT => TrayAction::Quit,
        _ => return,
    };
    LIVE.with_borrow(|live| {
        if let Some((_, tx)) = &*live {
            let _ = tx.unbounded_send(action);
        }
    });
}

fn ns_string(s: &str) -> *mut Object {
    let cstring = CString::new(s).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: cstring.as_ptr()] }
}

/// Launch-at-login via `~/Library/LaunchAgents/sh.zeron.app.tray.plist` — the
/// same per-user launchd mechanism `daemon.rs` uses for `zeron headless`, but
/// scoped to GUI sessions (`LimitLoadToSessionType=Aqua`) and without
/// `KeepAlive`: a tray Quit must never respawn.
///
/// The plist is only written/removed — never `bootstrap`/`bootout` here.
/// Bootstrapping would spawn a second instance immediately; bootout would
/// kill THIS process when the running instance is the launchd job itself.
/// The file alone takes effect at next login.
pub(super) fn set_autostart(enabled: bool) {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return;
    };
    let plist = home
        .join("Library/LaunchAgents")
        .join(format!("{TRAY_LABEL}.plist"));
    if enabled {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        if std::fs::create_dir_all(plist.parent().expect("LaunchAgents parent")).is_err() {
            return;
        }
        if let Err(error) = std::fs::write(&plist, render_plist(&exe)) {
            tracing::warn!(?error, "tray autostart plist write failed");
        }
    } else {
        match std::fs::remove_file(&plist) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(?error, "tray autostart plist removal failed"),
        }
    }
}

fn render_plist(exe: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key><string>{TRAY_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
      <string>{}</string>
      <string>--tray</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>LimitLoadToSessionType</key><string>Aqua</string>
    <key>ProcessType</key><string>Interactive</string>
  </dict>
</plist>
"#,
        xml_escape(&exe.to_string_lossy()),
    )
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_launches_exe_windowless_in_gui_sessions() {
        let plist = render_plist(Path::new("/Applications/Zeron.app/Contents/MacOS/zeron"));
        assert!(plist.contains("<key>Label</key><string>sh.zeron.app.tray</string>"));
        assert!(plist.contains("<string>/Applications/Zeron.app/Contents/MacOS/zeron</string>"));
        assert!(plist.contains("<string>--tray</string>"));
        assert!(plist.contains("<key>RunAtLoad</key><true/>"));
        assert!(plist.contains("<key>LimitLoadToSessionType</key><string>Aqua</string>"));
        // No KeepAlive — quitting from the tray must not respawn the app.
        assert!(!plist.contains("KeepAlive"));
    }

    #[test]
    fn plist_escapes_executable_path() {
        let plist = render_plist(Path::new("/tmp/a&b<zeron>"));
        assert!(plist.contains("<string>/tmp/a&amp;b&lt;zeron&gt;</string>"));
    }
}
