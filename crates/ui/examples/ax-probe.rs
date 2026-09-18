//! Computer Use — macOS semantic-observation probe / regression harness.
//!
//! Exercises the SAME Accessibility + CoreGraphics APIs the Appshot semantic
//! layer uses (crates/ui/src/appshots/macos.rs) against the live WindowServer
//! and AX server: enumerates on-screen windows, resolves each owning app into
//! an AXUIElement, and traverses one window into a bounded semantic tree —
//! reporting per-phase latency. Prints the capability verdict for both
//! permissions Computer Use depends on (Accessibility for the semantic tree,
//! Screen Recording for pixel capture).
//!
//! Usage: cargo run --release -p zeron-ui --example ax-probe
//!            [--traverse "<window title or app name>"] [--self-test]
//!            [--press "<element title>"] [--write "<text>"]
//!            [--menu "<Menu bar item title>"]
//! Exit 0 = Accessibility trusted and at least one window enumerated.
//!
//! --press finds the first element whose title/role/description matches and
//! performs AXPress (a background action — no cursor or focus steal). --write
//! sets AXValue on the first settable text element. --menu presses a menu-bar
//! item by title. Each action re-reads the element afterwards for VERIFY.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr;
use std::time::{Duration, Instant};

use core_foundation::array::CFArray;
use core_foundation::base::{CFRelease, CFRetain, CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::access::ScreenCaptureAccess;
use core_graphics::geometry::{CGPoint, CGSize};
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowIsOnscreen, kCGWindowLayer,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName,
    kCGWindowOwnerName, kCGWindowOwnerPID,
};

const MAX_DEPTH: usize = 24;
const MAX_NODES: usize = 800;
const DEADLINE: Duration = Duration::from_millis(1500);

type AXUIElementRef = *const c_void;
type AXError = i32;
const AX_SUCCESS: AXError = 0;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, seconds: f32) -> AXError;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXUIElementIsAttributeSettable(
        element: AXUIElementRef,
        attribute: CFStringRef,
        settable: *mut bool,
    ) -> AXError;
    fn AXValueGetTypeID() -> core_foundation::base::CFTypeID;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, result: *mut c_void) -> bool;
}

/// An AXUIElement we own. Elements surfaced inside an AXChildren array are
/// only retained by that array — retain them to outlive the snapshot, and
/// release on drop.
struct AxEl(AXUIElementRef);
impl AxEl {
    fn retained(ptr: AXUIElementRef) -> Self {
        if !ptr.is_null() {
            unsafe { CFRetain(ptr) };
        }
        Self(ptr)
    }
    fn owned(ptr: AXUIElementRef) -> Self {
        Self(ptr)
    }
}
impl Clone for AxEl {
    fn clone(&self) -> Self {
        Self::retained(self.0)
    }
}
impl Drop for AxEl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) }
        }
    }
}

unsafe fn copy_ax_value(element: AXUIElementRef, attribute: &str) -> Option<CFType> {
    let attribute = CFString::new(attribute);
    let mut value: CFTypeRef = ptr::null();
    if unsafe {
        AXUIElementCopyAttributeValue(element, attribute.as_concrete_TypeRef(), &mut value)
    } != AX_SUCCESS
        || value.is_null()
    {
        return None;
    }
    Some(unsafe { CFType::wrap_under_create_rule(value) })
}

unsafe fn ax_string(element: AXUIElementRef, attribute: &str) -> Option<String> {
    unsafe { copy_ax_value(element, attribute) }?
        .downcast::<CFString>()
        .map(|value| value.to_string())
}

/// CFString or CFNumber rendered as a string (checkbox/stepper AXValue).
unsafe fn ax_scalar_string(element: AXUIElementRef, attribute: &str) -> Option<String> {
    let value = unsafe { copy_ax_value(element, attribute) }?;
    if let Some(string) = value.downcast::<CFString>() {
        return Some(string.to_string());
    }
    if let Some(number) = value.downcast::<CFNumber>() {
        return number
            .to_i64()
            .map(|n| n.to_string())
            .or_else(|| number.to_f64().map(|n| n.to_string()));
    }
    None
}

unsafe fn ax_children(element: AXUIElementRef) -> Vec<AxEl> {
    let Some(array) = (unsafe { copy_ax_value(element, "AXChildren") })
        .and_then(|value| value.downcast::<CFArray>())
    else {
        return Vec::new();
    };
    array
        .iter()
        .map(|item| AxEl::retained(*item as AXUIElementRef))
        .collect()
}

unsafe fn ax_point(element: AXUIElementRef) -> Option<CGPoint> {
    let value = unsafe { copy_ax_value(element, "AXPosition") }?;
    if value.type_of() != unsafe { AXValueGetTypeID() } {
        return None;
    }
    let mut point = CGPoint::new(0.0, 0.0);
    unsafe { AXValueGetValue(value.as_CFTypeRef(), 1, (&mut point as *mut CGPoint).cast()) }
        .then_some(point)
}

unsafe fn ax_size(element: AXUIElementRef) -> Option<CGSize> {
    let value = unsafe { copy_ax_value(element, "AXSize") }?;
    if value.type_of() != unsafe { AXValueGetTypeID() } {
        return None;
    }
    let mut size = CGSize::new(0.0, 0.0);
    unsafe { AXValueGetValue(value.as_CFTypeRef(), 2, (&mut size as *mut CGSize).cast()) }
        .then_some(size)
}

/// True when the attribute is present and writable (AXUIElementIsAttributeSettable).
unsafe fn ax_settable(element: AXUIElementRef, attribute: &str) -> bool {
    let attribute = CFString::new(attribute);
    let mut settable = false;
    let err = unsafe {
        AXUIElementIsAttributeSettable(element, attribute.as_concrete_TypeRef(), &mut settable)
    };
    err == AX_SUCCESS && settable
}

unsafe fn ax_press(element: AXUIElementRef) -> bool {
    let action = CFString::new("AXPress");
    unsafe { AXUIElementPerformAction(element, action.as_concrete_TypeRef()) == AX_SUCCESS }
}

unsafe fn ax_set_value(element: AXUIElementRef, text: &str) -> bool {
    let attribute = CFString::new("AXValue");
    let value = CFString::new(text);
    let err = unsafe {
        AXUIElementSetAttributeValue(
            element,
            attribute.as_concrete_TypeRef(),
            value.as_concrete_TypeRef() as CFTypeRef,
        )
    };
    err == AX_SUCCESS
}

/// Breadth-first find over the app's windows: the first element whose title,
/// description, role, or string value contains `needle` (case-insensitive).
/// Same budgets as `traverse`.
fn find(app: AXUIElementRef, needle: &str, deadline: Instant) -> Option<AxEl> {
    let needle = needle.to_lowercase();
    let mut visited = 0usize;
    let mut queue: VecDeque<AxEl> = VecDeque::new();
    let mut roots = unsafe { ax_children(app) };
    if roots.is_empty() {
        roots.push(AxEl::retained(app));
    }
    queue.extend(roots);
    while let Some(el) = queue.pop_front() {
        if Instant::now() > deadline || visited >= MAX_NODES {
            break;
        }
        visited += 1;
        for attr in [
            "AXTitle",
            "AXDescription",
            "AXRole",
            "AXRoleDescription",
            "AXValue",
        ] {
            if unsafe { ax_string(el.0, attr) }
                .is_some_and(|value| value.to_lowercase().contains(&needle))
            {
                return Some(el);
            }
        }
        queue.extend(unsafe { ax_children(el.0) });
    }
    None
}

/// Depth-limited search for the first element that exposes a settable AXValue
/// string. Prefers the document surface: AXTextArea > AXTextField > AXComboBox.
fn find_editable(app: AXUIElementRef, deadline: Instant) -> Option<AxEl> {
    fn rank(role: &str) -> Option<usize> {
        match role {
            "AXTextArea" => Some(0),
            "AXTextField" => Some(1),
            "AXComboBox" => Some(2),
            _ => None,
        }
    }
    let mut visited = 0usize;
    let mut best: Option<(usize, AxEl)> = None;
    let mut queue: VecDeque<AxEl> = unsafe { ax_children(app) }.into();
    while let Some(el) = queue.pop_front() {
        if Instant::now() > deadline || visited >= MAX_NODES {
            break;
        }
        visited += 1;
        if unsafe { ax_settable(el.0, "AXValue") } {
            let role = unsafe { ax_string(el.0, "AXRole") }.unwrap_or_default();
            if let Some(rank) = rank(&role) {
                if best.as_ref().is_none_or(|(r, _)| rank < *r) {
                    if rank == 0 {
                        return Some(el);
                    }
                    best = Some((rank, el.clone()));
                }
            }
        }
        queue.extend(unsafe { ax_children(el.0) });
    }
    best.map(|(_, el)| el)
}

/// Menu-bar traversal: AXMenuBar → AXMenuBarItem (title match) → AXMenu →
/// AXMenuItem (title match). Mirrors how a computer-use layer drives menus
/// without opening them on screen.
fn find_menu_item(
    app: AXUIElementRef,
    top_level: &str,
    item: &str,
    deadline: Instant,
) -> Option<AxEl> {
    let bar = unsafe { copy_ax_value(app, "AXMenuBar") }?;
    let bar_ref = bar.as_CFTypeRef() as AXUIElementRef;
    for bar_item in unsafe { ax_children(bar_ref) } {
        if Instant::now() > deadline {
            return None;
        }
        let title = unsafe { ax_string(bar_item.0, "AXTitle") }.unwrap_or_default();
        if !title.eq_ignore_ascii_case(top_level) {
            continue;
        }
        for menu in unsafe { ax_children(bar_item.0) } {
            for entry in unsafe { ax_children(menu.0) } {
                let entry_title = unsafe { ax_string(entry.0, "AXTitle") }.unwrap_or_default();
                if entry_title.eq_ignore_ascii_case(item) {
                    return Some(entry);
                }
            }
        }
    }
    None
}

fn dictionary_value(dict: &CFDictionary, key: CFStringRef) -> Option<CFType> {
    let value = dict.find(key.cast::<c_void>())?;
    Some(unsafe { CFType::wrap_under_get_rule(*value as CFTypeRef) })
}

fn dictionary_number(dict: &CFDictionary, key: CFStringRef) -> Option<CFNumber> {
    dictionary_value(dict, key)?.downcast::<CFNumber>()
}

fn dictionary_string(dict: &CFDictionary, key: CFStringRef) -> Option<String> {
    dictionary_value(dict, key)?
        .downcast::<CFString>()
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
}

struct WindowInfo {
    pid: i32,
    owner: String,
    title: String,
}

/// On-screen, user-layer windows with an owner PID — the same universe the
/// Appshot picker enumerates (desktop elements excluded).
fn list_windows() -> Vec<WindowInfo> {
    let Some(list) = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    ) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in list.iter() {
        let cf = unsafe { CFType::wrap_under_get_rule(*item as CFTypeRef) };
        let Some(dict) = cf.downcast::<CFDictionary>() else {
            continue;
        };
        let layer = dictionary_number(&dict, unsafe { kCGWindowLayer }).and_then(|n| n.to_i32());
        let onscreen = dictionary_value(&dict, unsafe { kCGWindowIsOnscreen })
            .and_then(|value| value.downcast::<CFBoolean>())
            .is_some_and(|value| value == CFBoolean::true_value());
        // Layer 0 = normal app windows; menu bar/dock/desktop chrome live on
        // other layers and are noise for semantic observation.
        if layer != Some(0) || !onscreen {
            continue;
        }
        let Some(pid) =
            dictionary_number(&dict, unsafe { kCGWindowOwnerPID }).and_then(|n| n.to_i32())
        else {
            continue;
        };
        out.push(WindowInfo {
            pid,
            owner: dictionary_string(&dict, unsafe { kCGWindowOwnerName }).unwrap_or_default(),
            title: dictionary_string(&dict, unsafe { kCGWindowName }).unwrap_or_default(),
        });
    }
    out
}

/// Depth-bounded semantic-tree dump of one AX element. Mirrors the Appshot
/// serializer's caps (depth/node budgets, per-call messaging deadline) so
/// probe numbers translate directly to in-app capture cost.
fn traverse(root: AxEl, deadline: Instant) -> (usize, usize) {
    let mut visited = 0usize;
    let mut max_seen_depth = 0usize;
    let mut queue: VecDeque<(AxEl, usize)> = VecDeque::new();
    unsafe {
        AXUIElementSetMessagingTimeout(root.0, 0.25);
    }
    queue.push_back((root, 0));
    while let Some((el, depth)) = queue.pop_front() {
        if Instant::now() > deadline || visited >= MAX_NODES {
            break;
        }
        visited += 1;
        max_seen_depth = max_seen_depth.max(depth);
        let role = unsafe { ax_string(el.0, "AXRole") }.unwrap_or_else(|| "?".into());
        let title = unsafe { ax_string(el.0, "AXTitle") }
            .or_else(|| unsafe { ax_string(el.0, "AXDescription") })
            .unwrap_or_default();
        let pos = unsafe { ax_point(el.0) };
        let size = unsafe { ax_size(el.0) };
        let mut label = role;
        if !title.is_empty() {
            let trimmed: String = title.chars().take(48).collect();
            label.push_str(&format!(" \"{trimmed}\""));
        }
        if let (Some(p), Some(s)) = (pos, size) {
            label.push_str(&format!(
                " @({:.0},{:.0} {:.0}x{:.0})",
                p.x, p.y, s.width, s.height
            ));
        }
        println!("{:indent$}{label}", "", indent = depth * 2);
        if depth < MAX_DEPTH {
            for child in unsafe { ax_children(el.0) } {
                queue.push_back((child, depth + 1));
            }
        }
    }
    (visited, max_seen_depth)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg_after = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1).cloned())
    };
    let want_title = arg_after("--traverse").or_else(|| arg_after("--app"));
    let press_want = arg_after("--press");
    let write_want = arg_after("--write");
    let menu_want = arg_after("--menu");
    let self_test = args.iter().any(|a| a == "--self-test");

    println!("== Cosmos Computer Use / AX probe ==");
    let ax_trusted = unsafe { AXIsProcessTrusted() };
    let screen_ok = ScreenCaptureAccess.preflight();
    println!(
        "capability verdict: accessibility={}, screen_recording={}",
        if ax_trusted { "Ready" } else { "NotGranted" },
        if screen_ok { "Ready" } else { "NotGranted" },
    );

    let t0 = Instant::now();
    let windows = list_windows();
    println!(
        "observe(windows): {} on-screen windows in {} ms",
        windows.len(),
        t0.elapsed().as_millis()
    );
    for w in &windows {
        println!("  pid={:<6} {:<24} \"{}\"", w.pid, w.owner, w.title);
    }
    if windows.is_empty() {
        eprintln!("FAIL: no on-screen windows enumerated (screen recording likely denied)");
        std::process::exit(1);
    }

    // Actions imply a target app: --press/--write/--menu select it by name.
    let want_title = want_title.or_else(|| {
        (press_want.is_some() || write_want.is_some() || menu_want.is_some())
            .then(|| arg_after("--in"))
            .flatten()
    });

    let target: Option<i32> = if let Some(want) = &want_title {
        let want_lc = want.to_lowercase();
        windows
            .iter()
            .find(|w| {
                w.title.to_lowercase().contains(&want_lc)
                    || w.owner.to_lowercase().contains(&want_lc)
            })
            .map(|w| w.pid)
    } else if self_test {
        // Finder is always running and exposes a stable AX tree.
        windows
            .iter()
            .find(|w| w.owner == "Finder")
            .or_else(|| windows.first())
            .map(|w| w.pid)
    } else {
        None
    };

    if let Some(pid) = target {
        let t1 = Instant::now();
        let app = AxEl::owned(unsafe { AXUIElementCreateApplication(pid) });
        let app_name = unsafe { ax_string(app.0, "AXTitle") }.unwrap_or_else(|| "?".into());
        let ax_window_count = unsafe { copy_ax_value(app.0, "AXWindows") }
            .and_then(|value| value.downcast::<CFArray>())
            .map(|array| array.len())
            .unwrap_or(0);
        println!(
            "resolve(app): pid {pid} \"{app_name}\" exposes {ax_window_count} AX windows in {} ms",
            t1.elapsed().as_millis()
        );

        let t2 = Instant::now();
        let kids = unsafe { ax_children(app.0) };
        let first_window = kids
            .iter()
            .find(|el| unsafe { ax_string(el.0, "AXRole") }.as_deref() == Some("AXWindow"))
            .or_else(|| kids.first());
        match first_window {
            Some(win) => {
                let deadline = Instant::now() + DEADLINE;
                let (visited, depth) = traverse(win.clone(), deadline);
                println!(
                    "traverse(window): {visited} nodes, depth {depth}, {} ms",
                    t2.elapsed().as_millis()
                );
            }
            None => println!("traverse(window): no AX children on pid {pid}"),
        }

        // ACT — bounded, semantic actions against the resolved app.
        if let Some(want) = &press_want {
            let t3 = Instant::now();
            match find(app.0, want, Instant::now() + DEADLINE) {
                Some(el) => {
                    let role = unsafe { ax_string(el.0, "AXRole") }.unwrap_or_else(|| "?".into());
                    let found_in = t3.elapsed();
                    let ok = unsafe { ax_press(el.0) };
                    println!(
                        "act(press): {role} \"{want}\" found in {} ms, AXPress {} in {} ms",
                        found_in.as_millis(),
                        if ok { "ok" } else { "FAILED" },
                        t3.elapsed().as_millis() - found_in.as_millis(),
                    );
                    // VERIFY — re-read state after the action.
                    let post = unsafe { ax_scalar_string(el.0, "AXValue") }
                        .or_else(|| unsafe { ax_string(el.0, "AXTitle") })
                        .unwrap_or_default();
                    println!("verify(press): post-state AXValue/AXTitle = \"{post}\"");
                    if !ok {
                        std::process::exit(1);
                    }
                }
                None => {
                    eprintln!("FAIL: no element matching \"{want}\" on pid {pid}");
                    std::process::exit(1);
                }
            }
        }
        if let Some(text) = &write_want {
            let t3 = Instant::now();
            match find_editable(app.0, Instant::now() + DEADLINE) {
                Some(el) => {
                    let role = unsafe { ax_string(el.0, "AXRole") }.unwrap_or_else(|| "?".into());
                    let ok = unsafe { ax_set_value(el.0, text) };
                    println!(
                        "act(write): {role} AXValue set {} in {} ms",
                        if ok { "ok" } else { "FAILED" },
                        t3.elapsed().as_millis()
                    );
                    let post = unsafe { ax_string(el.0, "AXValue") }.unwrap_or_default();
                    let verified = post == *text;
                    println!(
                        "verify(write): read back {} chars, match={verified}",
                        post.chars().count()
                    );
                    if !ok || !verified {
                        std::process::exit(1);
                    }
                }
                None => {
                    eprintln!("FAIL: no settable text element on pid {pid}");
                    std::process::exit(1);
                }
            }
        }
        if let Some(menu_path) = &menu_want {
            let mut parts = menu_path.splitn(2, '>');
            let top = parts.next().unwrap_or("").trim().to_string();
            let item = parts.next().unwrap_or("").trim().to_string();
            let t3 = Instant::now();
            let found = find_menu_item(app.0, &top, &item, Instant::now() + DEADLINE);
            match found {
                Some(el) => {
                    let ok = unsafe { ax_press(el.0) };
                    println!(
                        "act(menu): \"{menu_path}\" AXPress {} in {} ms",
                        if ok { "ok" } else { "FAILED" },
                        t3.elapsed().as_millis()
                    );
                    if !ok {
                        std::process::exit(1);
                    }
                }
                None => {
                    eprintln!("FAIL: menu item \"{menu_path}\" not found on pid {pid}");
                    std::process::exit(1);
                }
            }
        }
    } else {
        println!("traverse: skipped (pass --traverse \"<title>\" or --self-test)");
    }

    if !ax_trusted {
        eprintln!("FAIL: process not AX-trusted — grant Accessibility for the terminal");
        std::process::exit(1);
    }
    println!("ok");
}
