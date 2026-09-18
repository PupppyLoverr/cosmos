//! Linux Delegate backend: the Global Shortcuts portal on Wayland, a passive
//! root key grab on X11, plus a `delegate-activation.sock` datagram socket so
//! desktops that bind shortcuts natively can wire `cosmos delegate`.
//!
//! Both registration paths consume `delegate::subscribe()`/`current()` — a
//! combo change or Settings recording gate rebinds without restarting the
//! backend (same pull model as the Appshot hotkey).
use std::fs;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};
use std::thread;

use futures::channel::mpsc;
use futures::{FutureExt as _, StreamExt as _};

use crate::appshots::CapabilityState;
use crate::global_hotkey::Shortcut;

const ACTIVATION_SOCKET: &str = "delegate-activation.sock";
const SHORTCUT_ID: &str = "delegate-task";

static X11_READY: AtomicBool = AtomicBool::new(true);
static WAYLAND_STATE: OnceLock<RwLock<CapabilityState>> = OnceLock::new();

fn wayland_state() -> &'static RwLock<CapabilityState> {
    WAYLAND_STATE.get_or_init(|| RwLock::new(CapabilityState::Checking))
}

fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty())
}

pub(super) fn shortcut_state() -> CapabilityState {
    if is_wayland() {
        wayland_state()
            .read()
            .map(|state| *state)
            .unwrap_or(CapabilityState::Checking)
    } else if X11_READY.load(Ordering::Relaxed) {
        CapabilityState::Ready
    } else {
        CapabilityState::SetupRequired
    }
}

pub(super) fn start_shortcut(activation_dir: &Path) -> mpsc::UnboundedReceiver<()> {
    let (tx, rx) = mpsc::unbounded();
    start_activation_socket(activation_dir.to_path_buf(), tx.clone());
    if is_wayland() {
        portal::start(tx);
    } else {
        x11::start(tx);
    }
    rx
}

pub fn activation_socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join(ACTIVATION_SOCKET)
}

fn start_activation_socket(data_dir: PathBuf, tx: mpsc::UnboundedSender<()>) {
    thread::Builder::new()
        .name("delegate-linux-activation".into())
        .spawn(move || {
            if fs::create_dir_all(&data_dir).is_err() {
                return;
            }
            let path = activation_socket_path(&data_dir);
            // The socket belongs exclusively to this feature. Removing a stale
            // inode here is safe and lets a restarted Zeron become reachable.
            let _ = fs::remove_file(&path);
            let Ok(socket) = UnixDatagram::bind(&path) else {
                return;
            };
            let mut buffer = [0_u8; 32];
            while let Ok(length) = socket.recv(&mut buffer) {
                if length > 0 && tx.unbounded_send(()).is_err() {
                    break;
                }
            }
            let _ = fs::remove_file(path);
        })
        .ok();
}

pub fn request_running_delegate(data_dir: &Path) -> Result<(), crate::appshots::CaptureError> {
    let socket = UnixDatagram::unbound().map_err(|error| {
        crate::appshots::CaptureError::CaptureFailed(format!(
            "Could not create Delegate activation socket: {error}"
        ))
    })?;
    socket
        .connect(activation_socket_path(data_dir))
        .and_then(|_| socket.send(b"delegate"))
        .map(|_| ())
        .map_err(|error| {
            crate::appshots::CaptureError::CaptureFailed(format!(
                "Could not reach a running Zeron instance for Delegate: {error}"
            ))
        })
}

mod portal {
    use super::*;
    use ashpd::desktop::CreateSessionOptions;
    use ashpd::desktop::global_shortcuts::{
        BindShortcutsOptions, ConfigureShortcutsOptions, GlobalShortcuts, NewShortcut,
    };

    pub(super) fn start(tx: mpsc::UnboundedSender<()>) {
        thread::Builder::new()
            .name("delegate-wayland-portal".into())
            .spawn(move || {
                futures::executor::block_on(async {
                    let mut updates = crate::delegate::subscribe();
                    let mut desired = updates.next().await.flatten();
                    let mut last_bound = None;
                    loop {
                        let Some(shortcut) = desired.take() else {
                            let Some(next) = updates.next().await else {
                                break;
                            };
                            desired = next;
                            continue;
                        };
                        let configure = last_bound.as_ref().is_some_and(|old| old != &shortcut);
                        match run(tx.clone(), &shortcut, configure, &mut updates).await {
                            Ok(next) => {
                                last_bound = Some(shortcut);
                                desired = next;
                            }
                            Err(error) => {
                                tracing::warn!(
                                    ?error,
                                    "Wayland global-shortcut portal unavailable"
                                );
                                if let Ok(mut state) = super::wayland_state().write() {
                                    *state = CapabilityState::SetupRequired;
                                }
                                let Some(next) = updates.next().await else {
                                    break;
                                };
                                desired = next;
                            }
                        }
                    }
                });
            })
            .ok();
    }

    async fn run(
        tx: mpsc::UnboundedSender<()>,
        selected: &Shortcut,
        configure: bool,
        updates: &mut mpsc::UnboundedReceiver<Option<Shortcut>>,
    ) -> Result<Option<Shortcut>, ashpd::Error> {
        let portal = GlobalShortcuts::new().await?;
        let session = portal
            .create_session(CreateSessionOptions::default())
            .await?;
        // Always close the old session before rebinding, including when
        // consent is denied or the feature is switched off. Dropping the
        // proxy does not close it.
        let result = async {
            let trigger = selected.portal_trigger();
            let shortcut =
                NewShortcut::new(SHORTCUT_ID, "Delegate a task to Cosmos")
                    .preferred_trigger(Some(trigger.as_str()));
            portal
                .bind_shortcuts(&session, &[shortcut], None, BindShortcutsOptions::default())
                .await?
                .response()?;
            if configure && portal.version() >= 2 {
                portal
                    .configure_shortcuts(&session, None, ConfigureShortcutsOptions::default())
                    .await?;
            }
            if let Ok(mut state) = super::wayland_state().write() {
                *state = CapabilityState::Ready;
            }
            let mut activations = portal.receive_activated().await?;
            loop {
                futures::select! {
                    next = updates.next().fuse() => {
                        let next = next.flatten();
                        if next.as_ref() != Some(selected) { return Ok(next); }
                    },
                    activation = activations.next().fuse() => {
                        let Some(activation) = activation else { return Ok(None); };
                        if activation.shortcut_id() == SHORTCUT_ID && tx.unbounded_send(()).is_err() {
                            return Ok(None);
                        }
                    },
                }
            }
        }
        .await;
        let _ = session.close().await;
        result
    }
}

mod x11 {
    use super::*;
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{ConnectionExt as _, GrabMode, ModMask, Window};
    use x11rb::rust_connection::RustConnection;

    pub(super) fn start(tx: mpsc::UnboundedSender<()>) {
        thread::Builder::new()
            .name("delegate-x11-hotkey".into())
            .spawn(move || {
                if let Err(error) = run(tx) {
                    super::X11_READY.store(false, Ordering::Relaxed);
                    tracing::warn!(?error, "X11 Delegate shortcut could not be registered");
                }
            })
            .ok();
    }

    fn run(tx: mpsc::UnboundedSender<()>) -> anyhow::Result<()> {
        let (connection, screen_index) = x11rb::connect(None)?;
        let root = connection.setup().roots[screen_index].root;
        let mut previous = None;
        let mut keycode = None;
        while !tx.is_closed() {
            let shortcut = super::super::current();
            if previous.as_ref() != Some(&shortcut) {
                if let Some(keycode) = keycode.take() {
                    connection
                        .ungrab_key(keycode, root, ModMask::ANY)?
                        .check()?;
                }
                if let Some(shortcut) = &shortcut {
                    match register(&connection, root, shortcut) {
                        Ok(code) => {
                            keycode = Some(code);
                            super::X11_READY.store(true, Ordering::Relaxed);
                        }
                        Err(error) => {
                            super::X11_READY.store(false, Ordering::Relaxed);
                            tracing::warn!(?error, "X11 Delegate shortcut could not be registered");
                        }
                    }
                }
                connection.flush()?;
                while connection.poll_for_event()?.is_some() {}
                previous = Some(shortcut);
            }
            while let Some(event) = connection.poll_for_event()? {
                if let Event::KeyPress(event) = event
                    && Some(event.detail) == keycode
                {
                    let _ = tx.unbounded_send(());
                }
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
        Ok(())
    }

    fn register(
        connection: &RustConnection,
        root: Window,
        shortcut: &Shortcut,
    ) -> anyhow::Result<u8> {
        let setup = connection.setup();
        let minimum = setup.min_keycode;
        let count = setup.max_keycode.saturating_sub(minimum).saturating_add(1);
        let mapping = connection.get_keyboard_mapping(minimum, count)?.reply()?;
        let per_keycode = usize::from(mapping.keysyms_per_keycode);
        anyhow::ensure!(per_keycode > 0, "X11 returned an empty keyboard map");
        let keysym =
            keysym(&shortcut.key).ok_or_else(|| anyhow::anyhow!("unsupported Delegate key"))?;
        let keycode = mapping
            .keysyms
            .chunks(per_keycode)
            .position(|symbols| symbols.contains(&keysym))
            .map(|index| minimum.saturating_add(index as u8))
            .ok_or_else(|| anyhow::anyhow!("Delegate key is absent from the X11 keyboard map"))?;
        let base = (if shortcut.control {
            ModMask::CONTROL
        } else {
            Default::default()
        }) | (if shortcut.alt {
            ModMask::M1
        } else {
            Default::default()
        }) | (if shortcut.shift {
            ModMask::SHIFT
        } else {
            Default::default()
        }) | (if shortcut.platform {
            ModMask::M4
        } else {
            Default::default()
        });
        for modifiers in [
            base,
            base | ModMask::LOCK,
            base | ModMask::M2,
            base | ModMask::LOCK | ModMask::M2,
        ] {
            if let Err(error) = connection
                .grab_key(
                    false,
                    root,
                    modifiers,
                    keycode,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )?
                .check()
            {
                // A partial registration must not keep consuming an old/new chord.
                connection
                    .ungrab_key(keycode, root, ModMask::ANY)?
                    .check()?;
                return Err(error.into());
            }
        }
        Ok(keycode)
    }

    fn keysym(key: &str) -> Option<u32> {
        Some(match key {
            "space" => 0x20,
            "tab" => 0xff09,
            "enter" => 0xff0d,
            "backspace" => 0xff08,
            "delete" => 0xffff,
            "insert" => 0xff63,
            "up" => 0xff52,
            "down" => 0xff54,
            "left" => 0xff51,
            "right" => 0xff53,
            "home" => 0xff50,
            "end" => 0xff57,
            "pageup" => 0xff55,
            "pagedown" => 0xff56,
            key if key.len() == 1 => u32::from(key.as_bytes()[0]),
            key => 0xffbe + u32::from(key.strip_prefix('f')?.parse::<u8>().ok()?.checked_sub(1)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_path_stays_inside_data_directory() {
        assert_eq!(
            activation_socket_path(Path::new("/tmp/zeron-test")),
            Path::new("/tmp/zeron-test/delegate-activation.sock")
        );
    }
}
