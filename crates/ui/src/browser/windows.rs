//! Windows browser host: a WebView2 child window (wry) bound to the GPUI
//! window's HWND. GPUI owns chrome; WebView2 owns page input inside its
//! bounds. `gpui_windows` captures the mouse on button-down, so pane resizes
//! and tab drags keep working even when the pointer crosses the page.
//!
//! Two seams differ from the macOS host:
//! - Airspace: the child HWND always paints above GPUI's swapchain, so GPUI
//!   chrome over the page region (drag overlays, popovers) is covered while a
//!   page is live. Bounds intersect the content mask instead.
//! - Focus: clicks into the page give Win32 focus to the child HWND, not the
//!   GPUI focus handle. `AcceleratorKeyPressed` forwards app/browser chords
//!   back into the keymap; `focus_webview`/`focus_chrome` push focus across
//!   the boundary on edges only, so the two never fight.
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{Bounds, Pixels, Window, px};
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use tokio::sync::mpsc::Sender;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_KEY_EVENT_KIND, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
    COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
};
use wry::WebViewExtWindows as _;
use wry::dpi::{LogicalPosition, LogicalSize};

use super::model::{PageState, Presentation, allowed_navigation};

pub enum NativeEvent {
    Changed,
    Finished,
    NewTab(String),
    /// A chord the page did not consume, replayed through GPUI's dispatcher.
    Key(gpui::Keystroke),
    /// A script result for the surface's FIFO eval queue.
    EvalResult(serde_json::Value),
    /// PNG bytes (or the capture error) for the surface's FIFO shot queue.
    Screenshot(Result<Vec<u8>, String>),
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetKeyState(key: i32) -> i16;
}

fn key_down(vk: i32) -> bool {
    unsafe { GetKeyState(vk) < 0 }
}

/// Virtual-key → gpui key name for the chords the browser chrome owns
/// (navigation, tabs, address). Pages keep every other key.
fn chord_key(vk: u32) -> Option<&'static str> {
    Some(match vk {
        0x4C => "l",  // VK_L
        0x54 => "t",  // VK_T
        0x57 => "w",  // VK_W
        0x52 => "r",  // VK_R
        0x4B => "k",  // VK_K
        0xBC => ",",  // VK_OEM_COMMA
        0xDB => "[",  // VK_OEM_4
        0xDD => "]",  // VK_OEM_6
        0x74 => "f5", // VK_F5
        _ => return None,
    })
}

/// One `BrowserContext` maps to one ephemeral WebView2 profile: a fresh
/// user-data directory under %TEMP%, removed on drop once the runtime has
/// released it. macOS uses a `nonPersistentDataStore` for the same semantic —
/// a profile switch or app quit discards every cookie.
///
/// Only the directory is shared: on Windows a `WebContext` is read solely
/// for this path, so each build wraps it in a fresh local `WebContext`
/// rather than holding a shared `RefCell` borrow across `build_as_child`'s
/// nested message pump — where a second build dispatched by that pump
/// would panic on the borrow and abort across the FFI boundary.
pub(super) struct BrowserData {
    dir: Rc<std::path::PathBuf>,
}

impl Clone for BrowserData {
    /// Clones share the profile: a `BrowserContext` is one ephemeral session.
    fn clone(&self) -> Self {
        Self {
            dir: self.dir.clone(),
        }
    }
}

impl Default for BrowserData {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "zeron-webview2-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Self { dir: Rc::new(dir) }
    }
}

impl Drop for BrowserData {
    fn drop(&mut self) {
        // Best effort: the runtime still holds the directory while any of its
        // webviews live; stale dirs are reclaimed by the OS temp cleaner.
        if Rc::strong_count(&self.dir) == 1 {
            let _ = std::fs::remove_dir_all(self.dir.as_path());
        }
    }
}

/// Last-reported page fields, fed by the WebView2 handlers. `state()` reads
/// these plus the live history flags.
struct HostPage {
    url: Option<String>,
    title: String,
    loading: bool,
}

pub(super) struct Host {
    webview: wry::WebView,
    page: Rc<RefCell<HostPage>>,
    presented: Presentation,
    /// Last sync inputs, for the fixture's `host_info` diagnostics.
    last_sync: Option<String>,
    /// Whether Win32 focus currently sits inside the child HWND. Edge-guarded
    /// so paint-time sync never spams `MoveFocus`.
    focus_pushed: bool,
    /// Registration token — the handler dies with the controller, so this is
    /// retained only for ownership documentation.
    _accelerator_token: i64,
    /// Kept alive for the controller's lifetime.
    _accelerator_handler: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2AcceleratorKeyPressedEventHandler,
    shortcuts: Rc<RefCell<Vec<String>>>,
    /// Feeds `NativeEvent`s back to the surface — script results and page
    /// captures complete on WebView2's own callbacks and re-enter here.
    tx: Sender<NativeEvent>,
}

/// An owned snapshot of the GPUI window's HWND for `build_as_child`.
///
/// WebView2 environment + controller creation pumps the Win32 message
/// loop (`webview2_com::wait_with_pump`). If that pump runs while gpui's
/// `App` cell is borrowed — inside any event or update callback — a
/// queued gpui task it dispatches panics on the second `borrow_mut`,
/// which then aborts across the `CallWindowProcW` FFI boundary. The
/// surface therefore captures the handle up front and calls
/// [`NativePage::new`] from a foreground task, which gpui only polls
/// with the cell unborrowed.
pub(super) struct ParentWindow(Win32WindowHandle);

impl ParentWindow {
    pub fn of(window: &Window) -> Result<Self, String> {
        // `Window` has an inherent `window_handle` returning gpui's own
        // `AnyWindowHandle` — call the trait method explicitly.
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|error| format!("Window handle unavailable: {error}"))?;
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => Ok(Self(handle)),
            _ => Err("Window does not expose a Win32 handle".into()),
        }
    }
}

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(self.0)) })
    }
}

pub(super) struct NativePage {
    host: Rc<RefCell<Host>>,
}

impl NativePage {
    /// `parent` must come from [`ParentWindow::of`]. Creation pumps the
    /// Win32 message loop, so call it only where no `App` borrow is held
    /// (a foreground task body) — never inside an event or update
    /// callback.
    pub fn new(
        parent: &ParentWindow,
        data: &BrowserData,
        tx: Sender<NativeEvent>,
    ) -> Result<Self, String> {
        let page = Rc::new(RefCell::new(HostPage {
            url: None,
            title: String::new(),
            loading: false,
        }));
        let nav_tx = tx.clone();
        let nav_page = page.clone();
        let load_tx = tx.clone();
        let load_page = page.clone();
        let title_tx = tx.clone();
        let title_page = page.clone();
        let new_tab = tx.clone();
        // A local context over the shared profile dir — see `BrowserData`.
        let mut context = wry::WebContext::new(Some((*data.dir).clone()));
        let webview = wry::WebViewBuilder::new_with_web_context(&mut context)
            .with_visible(false)
            .with_focused(false)
            .with_navigation_handler(move |url| {
                let allowed = allowed_navigation(&url);
                if allowed {
                    let mut page = nav_page.borrow_mut();
                    page.url = Some(url);
                    page.loading = true;
                    let _ = nav_tx.try_send(NativeEvent::Changed);
                }
                allowed
            })
            .with_document_title_changed_handler(move |title| {
                title_page.borrow_mut().title = title;
                let _ = title_tx.try_send(NativeEvent::Changed);
            })
            .with_on_page_load_handler(move |event, url| {
                let mut page = load_page.borrow_mut();
                match event {
                    wry::PageLoadEvent::Started => {
                        page.url = Some(url);
                        page.loading = true;
                    }
                    wry::PageLoadEvent::Finished => {
                        page.url = Some(url);
                        page.loading = false;
                    }
                }
                drop(page);
                let _ = load_tx.try_send(match event {
                    wry::PageLoadEvent::Started => NativeEvent::Changed,
                    wry::PageLoadEvent::Finished => NativeEvent::Finished,
                });
            })
            .with_new_window_req_handler(move |url, _| {
                if allowed_navigation(&url) {
                    let _ = new_tab.try_send(NativeEvent::NewTab(url));
                }
                wry::NewWindowResponse::Deny
            })
            .with_download_started_handler(|_, _| false)
            .build_as_child(parent)
            .map_err(|error| error.to_string())?;

        // While the page holds Win32 focus, GPUI never sees keystrokes.
        // WebView2's AcceleratorKeyPressed runs before the page does: forward
        // the browser chords and the app's own customized ones, leave the
        // rest for the page.
        let shortcuts = Rc::new(RefCell::new(Vec::<String>::new()));
        let key_tx = tx.clone();
        let key_shortcuts = shortcuts.clone();
        let handler = webview2_com::AcceleratorKeyPressedEventHandler::create(Box::new(
            move |_controller, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                unsafe {
                    let mut kind = COREWEBVIEW2_KEY_EVENT_KIND(0);
                    args.KeyEventKind(&mut kind)?;
                    if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                        && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                    {
                        return Ok(());
                    }
                    let mut vk = 0u32;
                    args.VirtualKey(&mut vk)?;
                    let Some(key) = chord_key(vk) else {
                        return Ok(());
                    };
                    let mut combo = String::new();
                    if key_down(0x11) {
                        combo.push_str("ctrl-");
                    }
                    if key_down(0x5B) || key_down(0x5C) {
                        combo.push_str("cmd-");
                    }
                    if key_down(0x12) {
                        combo.push_str("alt-");
                    }
                    if key_down(0x10) {
                        combo.push_str("shift-");
                    }
                    combo.push_str(key);
                    let browser_chord = matches!(
                        combo.as_str(),
                        "ctrl-l" | "ctrl-t" | "ctrl-w" | "ctrl-[" | "ctrl-]" | "ctrl-r" | "f5"
                    );
                    let Ok(keystroke) = gpui::Keystroke::parse(&combo) else {
                        return Ok(());
                    };
                    let app_chord = key_shortcuts
                        .borrow()
                        .iter()
                        .any(|s| gpui::Keystroke::parse(s).is_ok_and(|s| s == keystroke));
                    if !(browser_chord || app_chord) {
                        return Ok(());
                    }
                    if key_tx.try_send(NativeEvent::Key(keystroke)).is_ok() {
                        args.SetHandled(true)?;
                    }
                    Ok(())
                }
            },
        ));
        let mut accelerator_token = 0i64;
        unsafe {
            webview
                .controller()
                .add_AcceleratorKeyPressed(&handler, &mut accelerator_token)
                .map_err(|error| format!("WebView2 key hook unavailable: {error}"))?;
        }

        Ok(Self {
            host: Rc::new(RefCell::new(Host {
                webview,
                page,
                presented: Presentation::Hidden,
                last_sync: None,
                focus_pushed: false,
                _accelerator_token: accelerator_token,
                _accelerator_handler: handler,
                shortcuts,
                tx,
            })),
        })
    }

    pub fn handle(&self) -> Rc<RefCell<Host>> {
        self.host.clone()
    }

    /// GPUI chrome is taking focus (address field, toolbar). The webview's
    /// controller is asked to hand focus back to the parent HWND.
    pub fn focus_chrome(&self) {
        let mut host = self.host.borrow_mut();
        host.focus_pushed = false;
        let _ = host.webview.focus_parent();
    }

    pub fn set_shortcuts(&self, shortcuts: Vec<String>) {
        *self.host.borrow().shortcuts.borrow_mut() = shortcuts;
    }

    pub fn present(&mut self, presentation: Presentation) {
        let mut host = self.host.borrow_mut();
        host.presented = presentation;
        if presentation == Presentation::Hidden {
            host.focus_pushed = false;
        }
        host.apply_visibility(None);
    }

    pub fn load(&self, url: &str) -> Result<(), String> {
        self.host
            .borrow()
            .webview
            .load_url(url)
            .map_err(|error| error.to_string())
    }

    pub fn reload(&self) {
        let _ = self.host.borrow().webview.reload();
    }

    pub fn history(&self, forward: bool) {
        let host = self.host.borrow();
        let _ = if forward {
            host.webview.go_forward()
        } else {
            host.webview.go_back()
        };
    }

    pub fn state(&self) -> PageState {
        let host = self.host.borrow();
        let page = host.page.borrow();
        PageState {
            url: host
                .webview
                .url()
                .ok()
                .filter(|url| !url.is_empty() && url != "about:blank")
                .or_else(|| page.url.clone()),
            title: page.title.clone(),
            loading: page.loading,
            can_back: host.webview.can_go_back().unwrap_or(false),
            can_forward: host.webview.can_go_forward().unwrap_or(false),
            error: None,
        }
    }

    /// Runs `script` in the page; its return value — a JSON string produced
    /// by the caller's own wrapper — is unwrapped and arrives as
    /// `NativeEvent::EvalResult` in submission order.
    pub fn evaluate(&self, script: &str) {
        let host = self.host.borrow();
        let tx = host.tx.clone();
        let _ = host
            .webview
            .evaluate_script_with_callback(script, move |raw| {
                // The callback runs on the UI thread — a blocking send on a
                // full channel would freeze it, so a saturated queue drops
                // instead (awaiters time out rather than hang).
                let _ = tx.try_send(NativeEvent::EvalResult(super::decode_eval(&raw)));
            });
    }

    /// Diagnostic snapshot of the container HWND's placement, for the
    /// browser fixture: parent hwnd, screen rect, visibility, and the
    /// WebView2 controller's own bounds.
    pub fn host_info(&self) -> String {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{GetParent, GetWindowRect, IsWindowVisible};
        let host = self.host.borrow();
        let hwnd = host.webview.hwnd();
        let parent = unsafe { GetParent(hwnd) };
        let mut rect = RECT::default();
        let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
        let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
        let mut cbounds = RECT::default();
        let bounds_result = unsafe { host.webview.controller().Bounds(&mut cbounds) };
        format!(
            "container={:?} parent={:?} visible={} rect={},{},{}-{} controller={},{},{}-{} ctrl_err={:?} presented={:?} last_sync={:?}",
            hwnd,
            parent,
            visible,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            cbounds.left,
            cbounds.top,
            cbounds.right,
            cbounds.bottom,
            bounds_result.err(),
            host.presented,
            host.last_sync,
        )
    }

    /// `ICoreWebView2::CapturePreview` — the page's rendered PNG, including
    /// injected overlay DOM (the annotate highlight rides along for free).
    /// The stream must outlive the async capture, so it moves into the
    /// completion handler.
    pub fn screenshot(&self) {
        use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG;
        use windows::Win32::Foundation::HGLOBAL;
        use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
        let host = self.host.borrow();
        let stream = unsafe {
            match CreateStreamOnHGlobal(HGLOBAL::default(), true) {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = host.tx.try_send(NativeEvent::Screenshot(Err(format!(
                        "Could not open the capture stream: {error}"
                    ))));
                    return;
                }
            }
        };
        let reader = stream.clone();
        let tx = host.tx.clone();
        let handler =
            webview2_com::CapturePreviewCompletedHandler::create(Box::new(move |result| {
                // The callback runs on the UI thread — try_send only, so a
                // saturated queue drops instead of freezing it.
                let _ = tx.try_send(NativeEvent::Screenshot(
                    result
                        .and_then(|()| read_stream(&reader))
                        .map_err(|error| format!("Could not capture the page: {error}")),
                ));
                Ok(())
            }));
        unsafe {
            if let Err(error) = host.webview.webview().CapturePreview(
                COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                &stream,
                &handler,
            ) {
                let _ = host.tx.try_send(NativeEvent::Screenshot(Err(format!(
                    "Could not capture the page: {error}"
                ))));
            }
        }
    }
}

/// Drains an `IStream` written by `CapturePreview` back into bytes.
#[cfg(target_os = "windows")]
fn read_stream(stream: &windows::Win32::System::Com::IStream) -> windows_core::Result<Vec<u8>> {
    use windows::Win32::System::Com::{STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET};
    unsafe {
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME)?;
        let size = stat.cbSize as usize;
        stream.Seek(0, STREAM_SEEK_SET, None)?;
        let mut data = vec![0u8; size];
        let mut done = 0usize;
        while done < size {
            let mut count = 0u32;
            stream
                .Read(
                    data[done..].as_mut_ptr() as *mut _,
                    (size - done).min(u32::MAX as usize) as u32,
                    Some(&mut count),
                )
                .ok()?;
            if count == 0 {
                break;
            }
            done += count as usize;
        }
        data.truncate(done);
        Ok(data)
    }
}

impl Host {
    /// The webview is a child HWND of the GPUI window: its bounds live in the
    /// same client-area coordinate space the paint callback reports. The GPUI
    /// content mask cannot clip a sibling HWND, so clip the rect instead —
    /// and hide outright when nothing of the page remains visible.
    pub fn sync(&mut self, bounds: Bounds<Pixels>, mask: Bounds<Pixels>) {
        let clipped = bounds.intersect(&mask);
        let visible = clipped.size.width > px(0.0) && clipped.size.height > px(0.0);
        self.last_sync = Some(format!(
            "bounds={bounds:?} mask={mask:?} clipped={clipped:?} visible={visible}"
        ));
        self.apply_visibility(Some(visible));
        if visible && self.presented != Presentation::Hidden {
            let _ = self.webview.set_bounds(wry::Rect {
                position: wry::dpi::Position::Logical(LogicalPosition::new(
                    f32::from(clipped.origin.x) as f64,
                    f32::from(clipped.origin.y) as f64,
                )),
                size: wry::dpi::Size::Logical(LogicalSize::new(
                    f32::from(clipped.size.width) as f64,
                    f32::from(clipped.size.height) as f64,
                )),
            });
        }
    }

    pub fn sync_focus(&mut self, focused: bool) {
        if focused {
            if !self.focus_pushed && self.presented == Presentation::Live {
                self.focus_pushed = true;
                let _ = self.webview.focus();
            }
        } else {
            self.focus_pushed = false;
        }
    }

    fn apply_visibility(&self, within_mask: Option<bool>) {
        let _ = self
            .webview
            .set_visible(self.presented != Presentation::Hidden && within_mask.unwrap_or(true));
    }
}
