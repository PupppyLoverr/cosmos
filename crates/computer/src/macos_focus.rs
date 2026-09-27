#[derive(Debug, Clone, Copy)]
pub struct FrontmostWindow {
    pub window_id: i64,
    pub pid: i32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[cfg(target_os = "macos")]
mod platform {
    use super::FrontmostWindow;
    use core_foundation::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
    use core_foundation::base::{CFIndex, CFRelease};
    use core_foundation::base::{CFType, CFTypeRef, TCFType};
    use core_foundation::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use std::ffi::c_void;

    const K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
    const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
    }

    fn number_value(dictionary: CFDictionaryRef, key: &'static str) -> Option<CFNumber> {
        let key = CFString::from_static_string(key);
        let value =
            unsafe { CFDictionaryGetValue(dictionary, key.as_concrete_TypeRef() as *const c_void) };
        if value.is_null() {
            return None;
        }
        unsafe { CFType::wrap_under_get_rule(value as CFTypeRef) }.downcast::<CFNumber>()
    }

    fn number_i32(dictionary: CFDictionaryRef, key: &'static str) -> Option<i32> {
        number_value(dictionary, key)?.to_i32()
    }

    fn number_i64(dictionary: CFDictionaryRef, key: &'static str) -> Option<i64> {
        let value = number_value(dictionary, key)?;
        value.to_i64().or_else(|| value.to_i32().map(i64::from))
    }

    fn number_f64(dictionary: CFDictionaryRef, key: &'static str) -> Option<f64> {
        number_value(dictionary, key)?.to_f64()
    }

    fn bounds(dictionary: CFDictionaryRef) -> Option<(f64, f64, f64, f64)> {
        let key = CFString::from_static_string("kCGWindowBounds");
        let value =
            unsafe { CFDictionaryGetValue(dictionary, key.as_concrete_TypeRef() as *const c_void) };
        if value.is_null() {
            return None;
        }
        let bounds = unsafe {
            core_foundation::dictionary::CFDictionary::<
                *const c_void,
                *const c_void,
            >::wrap_under_get_rule(value as CFDictionaryRef)
        };
        let bounds = bounds.as_concrete_TypeRef();
        Some((
            number_f64(bounds, "X")?,
            number_f64(bounds, "Y")?,
            number_f64(bounds, "Width")?,
            number_f64(bounds, "Height")?,
        ))
    }

    pub fn frontmost_window() -> Option<FrontmostWindow> {
        let windows = unsafe {
            CGWindowListCopyWindowInfo(
                K_CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS,
                0,
            )
        };
        if windows.is_null() {
            return None;
        }
        let count = unsafe { CFArrayGetCount(windows) };
        let result = (0..count).find_map(|index: CFIndex| {
            let value = unsafe { CFArrayGetValueAtIndex(windows, index) };
            if value.is_null() {
                return None;
            }
            let dictionary = value as CFDictionaryRef;
            if number_i32(dictionary, "kCGWindowLayer")? != 0 {
                return None;
            }
            let (x, y, width, height) = bounds(dictionary)?;
            Some(FrontmostWindow {
                window_id: number_i64(dictionary, "kCGWindowNumber")?,
                pid: number_i32(dictionary, "kCGWindowOwnerPID")?,
                x,
                y,
                width,
                height,
            })
        });
        unsafe {
            CFRelease(windows as *const c_void);
        }
        result
    }
}

#[cfg(target_os = "macos")]
pub use platform::frontmost_window;

#[cfg(not(target_os = "macos"))]
pub fn frontmost_window() -> Option<FrontmostWindow> {
    None
}
