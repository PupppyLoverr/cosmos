use std::ffi::c_void;
use std::ptr;

use core_foundation::array::CFArray;
use core_foundation::base::{CFRelease, CFType, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};

use crate::menu::{NativeMenuOutcome, normalized_menu_segment};

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
}

struct AxElement(AXUIElementRef);

impl AxElement {
    unsafe fn owned(pointer: AXUIElementRef) -> Self {
        Self(pointer)
    }

    unsafe fn retained(pointer: AXUIElementRef) -> Self {
        if !pointer.is_null() {
            unsafe { core_foundation::base::CFRetain(pointer) };
        }
        Self(pointer)
    }

    fn as_ref(&self) -> AXUIElementRef {
        self.0
    }
}

impl Drop for AxElement {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}

unsafe fn copy_attribute(element: AXUIElementRef, attribute: &str) -> Result<CFType, AXError> {
    let attribute = CFString::new(attribute);
    let mut value: CFTypeRef = ptr::null();
    let code = unsafe {
        AXUIElementCopyAttributeValue(element, attribute.as_concrete_TypeRef(), &mut value)
    };
    if code != AX_SUCCESS || value.is_null() {
        return Err(code);
    }
    Ok(unsafe { CFType::wrap_under_create_rule(value) })
}

fn string_attribute(element: AXUIElementRef, attribute: &str) -> Option<String> {
    unsafe { copy_attribute(element, attribute).ok()? }
        .downcast::<CFString>()
        .map(|value| value.to_string())
}

unsafe fn children(element: AXUIElementRef) -> Vec<AxElement> {
    let Some(array) = (unsafe { copy_attribute(element, "AXChildren").ok() })
        .and_then(|value| value.downcast::<CFArray>())
    else {
        return Vec::new();
    };
    array
        .iter()
        .map(|item| unsafe { AxElement::retained(*item as AXUIElementRef) })
        .collect()
}

fn matching_child<'a>(children: &'a [AxElement], segment: &str) -> Option<&'a AxElement> {
    let wanted = normalized_menu_segment(segment);
    children.iter().find(|child| {
        string_attribute(child.as_ref(), "AXTitle")
            .or_else(|| string_attribute(child.as_ref(), "AXValue"))
            .is_some_and(|title| normalized_menu_segment(&title) == wanted)
    })
}

fn available_titles(children: &[AxElement]) -> Vec<String> {
    children
        .iter()
        .filter_map(|child| string_attribute(child.as_ref(), "AXTitle"))
        .filter(|title| !title.trim().is_empty())
        .collect()
}

fn role(element: AXUIElementRef) -> Option<String> {
    string_attribute(element, "AXRole")
}

pub fn press_menu_path(pid: i32, segments: &[String]) -> NativeMenuOutcome {
    if segments.is_empty() {
        return NativeMenuOutcome::Unavailable("menu path is empty".into());
    }
    if !unsafe { AXIsProcessTrusted() } {
        return NativeMenuOutcome::Unavailable(
            "this process is not trusted for accessibility (grant Cosmos in System Settings → Privacy & Security → Accessibility)"
                .into(),
        );
    }

    let app = unsafe { AxElement::owned(AXUIElementCreateApplication(pid)) };
    if app.as_ref().is_null() {
        return NativeMenuOutcome::Unavailable("app is not running".into());
    }
    if unsafe { AXUIElementSetMessagingTimeout(app.as_ref(), 1.0) } != AX_SUCCESS {
        return NativeMenuOutcome::Unavailable("could not set accessibility timeout".into());
    }

    let menu_bar = match unsafe { copy_attribute(app.as_ref(), "AXMenuBar") } {
        Ok(value) => value,
        Err(code) => {
            return NativeMenuOutcome::Unavailable(format!(
                "app has no accessibility menu bar (AXError {code})"
            ));
        }
    };
    let mut current = unsafe { AxElement::retained(menu_bar.as_CFTypeRef() as AXUIElementRef) };

    for (index, segment) in segments.iter().enumerate() {
        let level_children = unsafe { children(current.as_ref()) };
        let Some(child) = matching_child(&level_children, segment) else {
            return NativeMenuOutcome::NotFound {
                segment: segment.clone(),
                available: available_titles(&level_children),
            };
        };

        let is_last = index + 1 == segments.len();
        let child_role = role(child.as_ref()).unwrap_or_default();
        if is_last {
            if child_role != "AXMenuBarItem" && child_role != "AXMenuItem" {
                return NativeMenuOutcome::Unavailable(format!(
                    "menu segment has unexpected accessibility role {child_role}"
                ));
            }
            let action = CFString::new("AXPress");
            let code =
                unsafe { AXUIElementPerformAction(child.as_ref(), action.as_concrete_TypeRef()) };
            return if code == AX_SUCCESS {
                NativeMenuOutcome::Pressed
            } else {
                NativeMenuOutcome::Unavailable(format!("AXPress failed: {code}"))
            };
        }

        if child_role != "AXMenuBarItem" && child_role != "AXMenuItem" {
            return NativeMenuOutcome::Unavailable(format!(
                "menu segment has unexpected accessibility role {child_role}"
            ));
        }
        let child_children = unsafe { children(child.as_ref()) };
        let Some(menu) = child_children
            .iter()
            .find(|candidate| role(candidate.as_ref()).as_deref() == Some("AXMenu"))
        else {
            return NativeMenuOutcome::NotFound {
                segment: segments[index + 1].clone(),
                available: Vec::new(),
            };
        };
        current = unsafe { AxElement::retained(menu.as_ref()) };
    }

    NativeMenuOutcome::Unavailable("menu path could not be dispatched".into())
}
