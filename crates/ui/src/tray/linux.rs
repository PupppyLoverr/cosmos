//! Linux tray backend: a StatusNotifierItem on the session D-Bus via `ksni`
//! — the freedesktop protocol KDE Plasma, GNOME (with the AppIndicator
//! extension) and wlroots hosts serve through `org.kde.StatusNotifierWatcher`.
//!
//! `ksni::blocking` runs the service on its own runtime/thread, so
//! `start_icon` stays synchronous like the Windows backend: `spawn()` returns
//! only after the watcher answers. No watcher (bare GNOME, headless) → `None`
//! → `tray` keeps `QuitMode::Default` and the app is never an invisible
//! windowless process.
//!
//! The icon prefers the installed `zeron` theme icon (package-linux.sh drops
//! `dist/zeron.png` into hicolor) with an embedded ARGB pixmap as the
//! uninstalled fallback — SNI hosts honor `icon_name` first, pixmap second.
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use futures::channel::mpsc;
use ksni::blocking::TrayMethods as _;
use ksni::menu::{MenuItem, StandardItem};

use super::TrayAction;

/// The live service handle; `shutdown` takes it to drop the D-Bus
/// registration (one icon per process, same as the other backends).
static HANDLE: Mutex<Option<ksni::blocking::Handle<CosmosTray>>> = Mutex::new(None);

struct CosmosTray {
    tx: mpsc::UnboundedSender<TrayAction>,
}

impl CosmosTray {
    fn send(&self, action: TrayAction) {
        let _ = self.tx.unbounded_send(action);
    }
}

impl ksni::Tray for CosmosTray {
    fn id(&self) -> String {
        "cosmos".into()
    }

    fn title(&self) -> String {
        "Cosmos".into()
    }

    fn icon_name(&self) -> String {
        // Installed theme icon (scripts/package-linux.sh → hicolor/1024x1024).
        "zeron".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        tray_pixmap().to_vec()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        // Left click → Open Cosmos (Windows parity); the menu answers the
        // ContextMenu (right-click) side by itself.
        self.send(TrayAction::Open);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let item = |label: &str, action: TrayAction| {
            MenuItem::Standard(StandardItem {
                label: label.into(),
                activate: Box::new(move |tray: &mut Self| tray.send(action)),
                ..Default::default()
            })
        };
        vec![
            item("Open Cosmos", TrayAction::Open),
            item("Delegate a task", TrayAction::Delegate),
            MenuItem::Separator,
            item("Quit Cosmos", TrayAction::Quit),
        ]
    }
}

/// Install the item and return its command stream. `None` when no
/// StatusNotifierWatcher is present — the caller then keeps
/// `QuitMode::Default` (the app still quits on last-window-close).
pub(super) fn start_icon() -> Option<mpsc::UnboundedReceiver<TrayAction>> {
    if HANDLE.lock().unwrap().is_some() {
        // Already live: the first service keeps dispatching; a closed channel
        // lets the caller's consumer exit immediately (Windows-parity contract).
        return Some(mpsc::unbounded().1);
    }
    let (tx, rx) = mpsc::unbounded::<TrayAction>();
    match (CosmosTray { tx }).spawn() {
        Ok(handle) => {
            *HANDLE.lock().unwrap() = Some(handle);
            Some(rx)
        }
        Err(error) => {
            tracing::warn!(
                ?error,
                "tray SNI registration failed — staying in default quit mode"
            );
            None
        }
    }
}

/// Drop the registration and wait for the service to release the icon —
/// `tray::shutdown` runs before process exit so no ghost item lingers.
pub(super) fn shutdown() {
    let handle = HANDLE.lock().unwrap().take();
    if let Some(handle) = handle {
        handle.shutdown().wait();
    }
}

/// The embedded tray PNG decoded to SNI's ARGB32 wire form. Cached — SNI
/// hosts re-read `IconPixmap` on every watcher registration.
fn tray_pixmap() -> &'static [ksni::Icon] {
    static PIXMAP: OnceLock<Vec<ksni::Icon>> = OnceLock::new();
    PIXMAP.get_or_init(|| {
        let Some(image) = image::load_from_memory_with_format(
            include_bytes!("../../../../dist/zeron-tray.png"),
            image::ImageFormat::Png,
        )
        .ok() else {
            return Vec::new();
        };
        let (width, height) = (image.width() as i32, image.height() as i32);
        let mut data = image.into_rgba8().into_vec();
        for pixel in data.chunks_exact_mut(4) {
            pixel.rotate_right(1); // rgba → argb (SNI byte order)
        }
        vec![ksni::Icon {
            width,
            height,
            data,
        }]
    })
}

/// Launch-at-login via `~/.config/autostart/zeron.desktop` — the freedesktop
/// login-entry counterpart of the Windows Run key / macOS LaunchAgent.
/// `Exec` adds `--tray`, which boots windowless into this same icon.
/// File-only like the other backends: no daemon reload needed, the entry
/// takes effect at next login.
pub(super) fn set_autostart(enabled: bool) {
    let Some(dir) = autostart_dir() else {
        return;
    };
    let path = dir.join("zeron.desktop");
    if enabled {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        if let Err(error) = std::fs::write(&path, render_desktop_entry(&exe)) {
            tracing::warn!(?error, "tray autostart entry write failed");
        }
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(?error, "tray autostart entry removal failed"),
        }
    }
}

/// `$XDG_CONFIG_HOME/autostart`, defaulting to `~/.config/autostart` — same
/// XDG resolution `daemon.rs` uses for the systemd user directory.
fn autostart_dir() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("autostart"))
}

fn render_desktop_entry(exe: &Path) -> String {
    let exe = desktop_exec_escape(&exe.to_string_lossy());
    format!(
        "[Desktop Entry]\n\
        Type=Application\n\
        Name=Cosmos\n\
        GenericName=Coding Agent Controller\n\
        Comment=Multi-device controller for coding agents\n\
        Exec={exe} --tray\n\
        Icon=zeron\n\
        Terminal=false\n\
        Categories=Development;Utility;\n\
        X-GNOME-Autostart-enabled=true\n"
    )
}

/// Desktop `Exec` quoting: wrap in double quotes when the path needs it,
/// escaping the characters the spec treats specially inside quotes.
fn desktop_exec_escape(exe: &str) -> String {
    let escaped = exe
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");
    if exe.bytes().any(|b| b.is_ascii_whitespace()) {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_launches_exe_windowless() {
        let entry = render_desktop_entry(Path::new("/home/u/.local/bin/zeron"));
        assert!(entry.contains("Exec=/home/u/.local/bin/zeron --tray\n"));
        assert!(entry.contains("Type=Application\n"));
        assert!(entry.contains("Icon=zeron\n"));
        assert!(entry.contains("X-GNOME-Autostart-enabled=true\n"));
    }

    #[test]
    fn desktop_exec_quotes_and_escapes() {
        // Whitespace wraps the path in double quotes; specials escape inline.
        assert_eq!(
            desktop_exec_escape("/opt/my app/zeron"),
            "\"/opt/my app/zeron\""
        );
        assert_eq!(desktop_exec_escape("/opt/plain"), "/opt/plain");
        assert_eq!(desktop_exec_escape("/opt/we$rd"), "/opt/we\\$rd");
        assert_eq!(desktop_exec_escape("/opt/a\"b"), "/opt/a\\\"b");
        assert_eq!(desktop_exec_escape("/opt/a\\b"), "/opt/a\\\\b");
    }
}
