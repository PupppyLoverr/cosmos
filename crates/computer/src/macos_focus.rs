#[derive(Debug, Clone, Copy)]
pub struct FocusedWindow {
    pub pid: i32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub minimized: bool,
}

#[cfg(target_os = "macos")]
mod platform {
    use super::FocusedWindow;
    use core_foundation::base::{CFType, CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;
    use std::ptr;

    type AXUIElementRef = *const c_void;
    type AXError = i32;

    const AX_SUCCESS: AXError = 0;
    const K_AX_VALUE_TYPE_CG_POINT: u32 = 1;
    const K_AX_VALUE_TYPE_CG_SIZE: u32 = 2;

    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[repr(C)]
    struct CGSize {
        width: f64,
        height: f64,
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> AXError;
        fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
        fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, seconds: f32) -> AXError;
        fn AXValueGetValue(value: CFTypeRef, kind: u32, result: *mut c_void) -> bool;
    }

    struct AxElement(CFType);

    impl AxElement {
        unsafe fn owned(pointer: AXUIElementRef) -> Option<Self> {
            (!pointer.is_null())
                .then(|| Self(unsafe { CFType::wrap_under_create_rule(pointer as CFTypeRef) }))
        }

        fn from_value(value: CFType) -> Self {
            Self(value)
        }

        fn as_ref(&self) -> AXUIElementRef {
            self.0.as_CFTypeRef() as AXUIElementRef
        }
    }

    unsafe fn copy_attribute(element: AXUIElementRef, attribute: &str) -> Option<CFType> {
        let attribute = CFString::new(attribute);
        let mut value: CFTypeRef = ptr::null();
        let code = unsafe {
            AXUIElementCopyAttributeValue(element, attribute.as_concrete_TypeRef(), &mut value)
        };
        if code != AX_SUCCESS || value.is_null() {
            return None;
        }
        Some(unsafe { CFType::wrap_under_create_rule(value) })
    }

    fn set_timeout(element: AXUIElementRef) -> Option<()> {
        (unsafe { AXUIElementSetMessagingTimeout(element, 0.25) } == AX_SUCCESS).then_some(())
    }

    fn point_attribute(element: AXUIElementRef, attribute: &str) -> Option<(f64, f64)> {
        let value = unsafe { copy_attribute(element, attribute) }?;
        let mut point = CGPoint { x: 0.0, y: 0.0 };
        let ok = unsafe {
            AXValueGetValue(
                value.as_CFTypeRef(),
                K_AX_VALUE_TYPE_CG_POINT,
                (&mut point as *mut CGPoint).cast(),
            )
        };
        ok.then_some((point.x, point.y))
    }

    fn size_attribute(element: AXUIElementRef, attribute: &str) -> Option<(f64, f64)> {
        let value = unsafe { copy_attribute(element, attribute) }?;
        let mut size = CGSize {
            width: 0.0,
            height: 0.0,
        };
        let ok = unsafe {
            AXValueGetValue(
                value.as_CFTypeRef(),
                K_AX_VALUE_TYPE_CG_SIZE,
                (&mut size as *mut CGSize).cast(),
            )
        };
        ok.then_some((size.width, size.height))
    }

    pub fn focused_window_frame() -> Option<FocusedWindow> {
        if !unsafe { AXIsProcessTrusted() } {
            return None;
        }

        let system = unsafe { AxElement::owned(AXUIElementCreateSystemWide()) }?;
        set_timeout(system.as_ref())?;
        let application = AxElement::from_value(unsafe {
            copy_attribute(system.as_ref(), "AXFocusedApplication")
        }?);
        set_timeout(application.as_ref())?;

        let mut pid = 0;
        if unsafe { AXUIElementGetPid(application.as_ref(), &mut pid) } != AX_SUCCESS {
            return None;
        }
        let window = AxElement::from_value(unsafe {
            copy_attribute(application.as_ref(), "AXFocusedWindow")
        }?);
        set_timeout(window.as_ref())?;
        let (x, y) = point_attribute(window.as_ref(), "AXPosition")?;
        let (width, height) = size_attribute(window.as_ref(), "AXSize")?;
        let minimized = unsafe { copy_attribute(window.as_ref(), "AXMinimized") }?
            .downcast::<CFBoolean>()
            .map(bool::from)?;

        Some(FocusedWindow {
            pid,
            x,
            y,
            width,
            height,
            minimized,
        })
    }
}

#[cfg(target_os = "macos")]
pub use platform::focused_window_frame;

#[cfg(not(target_os = "macos"))]
pub fn focused_window_frame() -> Option<FocusedWindow> {
    None
}
