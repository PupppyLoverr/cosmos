//! Windows tray backend: a message-only HWND on a dedicated thread owns the
//! `Shell_NotifyIcon` registration and its context menu — the same thread +
//! channel bridge `delegate::windows` uses for the global hotkey.
//!
//! The events channel travels into the WndProc through `CreateWindowExW`'s
//! `lpParam` (stashed in `GWLP_USERDATA` on `WM_NCCREATE`, reclaimed on
//! `WM_NCDESTROY`) so the module needs no global sender.
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc as stdm;
use std::thread;
use std::time::Duration;

use futures::channel::mpsc;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NIN_SELECT,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CREATESTRUCTW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, DispatchMessageW, GWLP_USERDATA, GetCursorPos, GetMessageW, GetWindowLongPtrW,
    HICON, HWND_MESSAGE, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED, LoadIconW,
    LoadImageW, MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SetForegroundWindow, SetWindowLongPtrW, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONUP, WM_NCCREATE, WM_NCDESTROY, WM_NULL, WM_RBUTTONUP,
    WM_USER, WNDCLASSEXW,
};
use windows::core::{PCWSTR, w};

use super::TrayAction;

/// App-defined messages on the tray HWND.
const WM_TRAYICON: u32 = WM_USER + 0x40;
const WM_TRAY_SHUTDOWN: u32 = WM_USER + 0x41;
/// `NOTIFYICONDATAW::uID` — one icon per process.
const TRAY_ICON_ID: u32 = 1;
/// Menu command ids (`TPM_RETURNCMD` resolves them, no WM_COMMAND needed).
const IDM_OPEN: usize = 0x101;
const IDM_DELEGATE: usize = 0x102;
const IDM_QUIT: usize = 0x103;

/// The live tray HWND (`PostMessage` target for `shutdown`); 0 when none.
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
/// Explorer's broadcast for "notification area rebuilt" — re-add the icon.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
/// The worker signals after `NIM_DELETE` so shutdown can wait for it.
static DONE: Mutex<Option<stdm::Receiver<()>>> = Mutex::new(None);

/// Install the icon on a dedicated thread and return its command stream.
/// `Some` only after `NIM_ADD` is confirmed — `tray::start` gates
/// `QuitMode::Explicit` on this.
pub(super) fn start_icon() -> Option<mpsc::UnboundedReceiver<TrayAction>> {
    if TRAY_HWND.load(Ordering::SeqCst) != 0 {
        // Already live: the first bridge keeps dispatching; a closed channel
        // lets the caller's consumer exit immediately.
        return Some(mpsc::unbounded().1);
    }
    let (tx, rx) = mpsc::unbounded::<TrayAction>();
    let (ready_tx, ready_rx) = stdm::channel::<bool>();
    let (done_tx, done_rx) = stdm::channel::<()>();
    thread::Builder::new()
        .name("cosmos-tray".into())
        .spawn(move || run(tx, ready_tx, done_tx))
        .ok()?;
    *DONE.lock().unwrap() = Some(done_rx);
    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(true) => Some(rx),
        _ => None,
    }
}

/// Remove the icon and stop the worker. Waits briefly for `NIM_DELETE` to
/// land — the process must not exit first or the ghost icon lingers.
pub(super) fn shutdown() {
    let hwnd = TRAY_HWND.swap(0, Ordering::SeqCst);
    if hwnd == 0 {
        return;
    }
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd as *mut core::ffi::c_void)),
            WM_TRAY_SHUTDOWN,
            WPARAM(0),
            LPARAM(0),
        );
    }
    if let Some(done) = DONE.lock().unwrap().take() {
        let _ = done.recv_timeout(Duration::from_millis(800));
    }
}

/// Launch-at-login via the per-user Run key — `"exe" --tray` boots the app
/// windowless into the tray. Mirrors `apps/zeron/src/daemon.rs`'s per-user
/// Task Scheduler approach; the Run key is the lighter-weight equivalent
/// for an interactive tray app.
pub(super) fn set_autostart(enabled: bool) {
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
        RegOpenKeyExW, RegSetValueExW,
    };
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            None,
            KEY_SET_VALUE,
            &mut key,
        )
        .is_err()
        {
            return;
        }
        let result = if enabled {
            let Ok(exe) = std::env::current_exe() else {
                let _ = RegCloseKey(key);
                return;
            };
            let command: Vec<u16> = format!("\"{}\" --tray", exe.display())
                .encode_utf16()
                .chain([0])
                .collect();
            RegSetValueExW(
                key,
                w!("Cosmos"),
                None,
                REG_SZ,
                Some(std::slice::from_raw_parts(
                    command.as_ptr().cast::<u8>(),
                    command.len() * 2,
                )),
            )
        } else {
            RegDeleteValueW(key, w!("Cosmos"))
        };
        // Deleting an absent value answers ERROR_FILE_NOT_FOUND — that's the
        // steady state for "autostart was never on", not a failure.
        let missing = !enabled && result.0 == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.0;
        if result.is_err() && !missing {
            tracing::warn!(code = result.0, "tray autostart registry update failed");
        }
        let _ = RegCloseKey(key);
    }
}

fn run(tx: mpsc::UnboundedSender<TrayAction>, ready: stdm::Sender<bool>, done: stdm::Sender<()>) {
    unsafe {
        let instance = GetModuleHandleW(None)
            .map(|module| HINSTANCE(module.0))
            .unwrap_or_default();
        let class_name = w!("CosmosTrayIcon");
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(w!("TaskbarCreated")),
            Ordering::SeqCst,
        );
        let wndclass = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(tray_wndproc),
            hInstance: instance,
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassExW(&wndclass);
        let tx = Box::into_raw(Box::new(tx));
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("CosmosTray"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            Some(tx.cast()),
        ) {
            Ok(hwnd) if !hwnd.0.is_null() => hwnd,
            _ => {
                drop(Box::from_raw(tx));
                let _ = ready.send(false);
                return;
            }
        };
        let mut data = notify_data(hwnd, load_icon());
        let added = Shell_NotifyIconW(NIM_ADD, &mut data).as_bool();
        if added {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &mut data);
            TRAY_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        } else {
            let _ = DestroyWindow(hwnd);
            tracing::warn!("tray icon registration failed — staying in default quit mode");
        }
        let _ = ready.send(added);
        if !added {
            return;
        }
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            let _ = DispatchMessageW(&message);
        }
        let _ = done.send(());
    }
}

fn notify_data(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAYICON,
        hIcon: icon,
        ..Default::default()
    };
    for (slot, ch) in data
        .szTip
        .iter_mut()
        .zip("Cosmos".encode_utf16().chain([0]))
    {
        *slot = ch;
    }
    data
}

/// The exe embeds `zeron.ico` as resource id 1 (apps/zeron/build.rs) — the
/// same icon gpui's `load_icon` resolves for the window and taskbar.
fn load_icon() -> HICON {
    unsafe {
        if let Ok(module) = GetModuleHandleW(None)
            && !module.0.is_null()
            && let Ok(handle) = LoadImageW(
                Some(HINSTANCE(module.0)),
                PCWSTR(1usize as *const u16),
                IMAGE_ICON,
                0,
                0,
                LR_DEFAULTSIZE | LR_SHARED,
            )
            && !handle.0.is_null()
        {
            return HICON(handle.0);
        }
        LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
    }
}

fn send(hwnd: HWND, action: TrayAction) {
    let tx = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) }
        as *const mpsc::UnboundedSender<TrayAction>;
    if !tx.is_null() {
        let _ = unsafe { &*tx }.unbounded_send(action);
    }
}

fn remove_icon(hwnd: HWND) {
    unsafe {
        let mut data = notify_data(hwnd, HICON::default());
        let _ = Shell_NotifyIconW(NIM_DELETE, &mut data);
    }
}

fn readd_icon(hwnd: HWND) {
    unsafe {
        let mut data = notify_data(hwnd, load_icon());
        let _ = Shell_NotifyIconW(NIM_ADD, &mut data);
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &mut data);
    }
}

/// Right-click menu: Open / Delegate / Quit. `TPM_RETURNCMD` resolves the
/// pick synchronously so no `WM_COMMAND` plumbing is needed.
fn show_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        let _ = AppendMenuW(menu, MF_STRING, IDM_OPEN, w!("Open Cosmos"));
        let _ = AppendMenuW(menu, MF_STRING, IDM_DELEGATE, w!("Delegate a task"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, IDM_QUIT, w!("Quit Cosmos"));
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        // The menu only dismisses on outside clicks when its owner is the
        // foreground window (TrackPopupMenu contract); WM_NULL afterwards
        // completes the handoff.
        let _ = SetForegroundWindow(hwnd);
        let command = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            point.x,
            point.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        let action = match command.0 as usize {
            IDM_OPEN => Some(TrayAction::Open),
            IDM_DELEGATE => Some(TrayAction::Delegate),
            IDM_QUIT => Some(TrayAction::Quit),
            _ => None,
        };
        if let Some(action) = action {
            send(hwnd, action);
        }
    }
}

unsafe extern "system" fn tray_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => unsafe {
            let params = lparam.0 as *const CREATESTRUCTW;
            if let Some(params) = params.as_ref() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, params.lpCreateParams as isize);
            }
        },
        WM_TRAYICON => {
            // Version-4 callbacks carry the mouse/keyboard event in LOWORD.
            match (lparam.0 & 0xFFFF) as u32 {
                WM_LBUTTONUP | NIN_SELECT => send(hwnd, TrayAction::Open),
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            return LRESULT(0);
        }
        WM_TRAY_SHUTDOWN => unsafe {
            remove_icon(hwnd);
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        },
        WM_DESTROY => unsafe {
            // Whatever destroys the window also removes the icon — a ghost
            // icon in the notification area is the classic tray bug.
            remove_icon(hwnd);
            PostQuitMessage(0);
            return LRESULT(0);
        },
        WM_NCDESTROY => unsafe {
            let raw =
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut mpsc::UnboundedSender<TrayAction>;
            if !raw.is_null() {
                drop(Box::from_raw(raw));
            }
            // A restarted worker may already own the slot — only clear when
            // it's still THIS window's hwnd, or the fresh icon becomes
            // unreachable (shutdown would skip its NIM_DELETE → ghost icon).
            let _ =
                TRAY_HWND.compare_exchange(hwnd.0 as isize, 0, Ordering::SeqCst, Ordering::SeqCst);
        },
        _ => {}
    }
    if msg != 0 && msg == TASKBAR_CREATED.load(Ordering::SeqCst) {
        // Explorer restarted and rebuilt the notification area empty.
        readd_icon(hwnd);
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
