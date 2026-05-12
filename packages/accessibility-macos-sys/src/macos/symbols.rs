use objc2_application_services::{AXError, AXUIElement};
use objc2_core_graphics::{CGEvent, CGWindowID};
use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

pub(crate) type SLEventPostToPidFn = unsafe extern "C-unwind" fn(libc::pid_t, Option<&CGEvent>);
pub(crate) type AXUIElementGetWindowFn =
    unsafe extern "C-unwind" fn(&AXUIElement, *mut CGWindowID) -> AXError;
pub(crate) type CGSConnectionID = i32;
pub(crate) type CGSMainConnectionIDFn = unsafe extern "C-unwind" fn() -> CGSConnectionID;
pub(crate) type CGSSetWindowAlphaFn =
    unsafe extern "C-unwind" fn(CGSConnectionID, CGWindowID, f32) -> i32;
pub(crate) type SLSMainConnectionIDFn = unsafe extern "C-unwind" fn() -> CGSConnectionID;
pub(crate) type SLSSetWindowAlphaFn =
    unsafe extern "C-unwind" fn(CGSConnectionID, CGWindowID, f32) -> i32;

fn dlerror_message() -> String {
    unsafe {
        let error = libc::dlerror();
        if error.is_null() {
            "unknown dynamic loader error".to_string()
        } else {
            CStr::from_ptr(error).to_string_lossy().into_owned()
        }
    }
}

fn skylight_handle() -> Option<*mut c_void> {
    static HANDLE: OnceLock<Option<usize>> = OnceLock::new();

    HANDLE
        .get_or_init(|| unsafe {
            let path = b"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight\0";
            let handle = libc::dlopen(
                path.as_ptr() as *const c_char,
                libc::RTLD_NOW | libc::RTLD_GLOBAL,
            );
            if handle.is_null() {
                let _ = dlerror_message();
                None
            } else {
                Some(handle as usize)
            }
        })
        .map(|handle| handle as *mut c_void)
}

fn core_graphics_handle() -> Option<*mut c_void> {
    static HANDLE: OnceLock<Option<usize>> = OnceLock::new();

    HANDLE
        .get_or_init(|| unsafe {
            let path = b"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics\0";
            let handle = libc::dlopen(
                path.as_ptr() as *const c_char,
                libc::RTLD_NOW | libc::RTLD_GLOBAL,
            );
            if handle.is_null() {
                let _ = dlerror_message();
                None
            } else {
                Some(handle as usize)
            }
        })
        .map(|handle| handle as *mut c_void)
}

pub(crate) fn skylight_event_post_to_pid() -> Option<SLEventPostToPidFn> {
    static SYMBOL: OnceLock<Option<SLEventPostToPidFn>> = OnceLock::new();

    *SYMBOL.get_or_init(|| unsafe {
        let handle = skylight_handle()?;
        let symbol = libc::dlsym(handle, c"SLEventPostToPid".as_ptr());
        if symbol.is_null() {
            let _ = dlerror_message();
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, SLEventPostToPidFn>(
                symbol,
            ))
        }
    })
}

pub(crate) fn ax_ui_element_get_window() -> Option<AXUIElementGetWindowFn> {
    static SYMBOL: OnceLock<Option<AXUIElementGetWindowFn>> = OnceLock::new();

    *SYMBOL.get_or_init(|| unsafe {
        let symbol = libc::dlsym(libc::RTLD_DEFAULT, c"_AXUIElementGetWindow".as_ptr());
        if symbol.is_null() {
            let _ = dlerror_message();
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, AXUIElementGetWindowFn>(
                symbol,
            ))
        }
    })
}

pub(crate) fn cgs_main_connection_id() -> Option<CGSMainConnectionIDFn> {
    static SYMBOL: OnceLock<Option<CGSMainConnectionIDFn>> = OnceLock::new();

    *SYMBOL.get_or_init(|| unsafe {
        let handle = core_graphics_handle()?;
        let symbol = libc::dlsym(handle, c"CGSMainConnectionID".as_ptr());
        if symbol.is_null() {
            let _ = dlerror_message();
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, CGSMainConnectionIDFn>(
                symbol,
            ))
        }
    })
}

pub(crate) fn cgs_set_window_alpha() -> Option<CGSSetWindowAlphaFn> {
    static SYMBOL: OnceLock<Option<CGSSetWindowAlphaFn>> = OnceLock::new();

    *SYMBOL.get_or_init(|| unsafe {
        let handle = core_graphics_handle()?;
        let symbol = libc::dlsym(handle, c"CGSSetWindowAlpha".as_ptr());
        if symbol.is_null() {
            let _ = dlerror_message();
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, CGSSetWindowAlphaFn>(
                symbol,
            ))
        }
    })
}

pub(crate) fn sls_main_connection_id() -> Option<SLSMainConnectionIDFn> {
    static SYMBOL: OnceLock<Option<SLSMainConnectionIDFn>> = OnceLock::new();

    *SYMBOL.get_or_init(|| unsafe {
        let handle = skylight_handle()?;
        let symbol = libc::dlsym(handle, c"SLSMainConnectionID".as_ptr());
        if symbol.is_null() {
            let _ = dlerror_message();
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, SLSMainConnectionIDFn>(
                symbol,
            ))
        }
    })
}

pub(crate) fn sls_set_window_alpha() -> Option<SLSSetWindowAlphaFn> {
    static SYMBOL: OnceLock<Option<SLSSetWindowAlphaFn>> = OnceLock::new();

    *SYMBOL.get_or_init(|| unsafe {
        let handle = skylight_handle()?;
        let symbol = libc::dlsym(handle, c"SLSSetWindowAlpha".as_ptr());
        if symbol.is_null() {
            let _ = dlerror_message();
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, SLSSetWindowAlphaFn>(
                symbol,
            ))
        }
    })
}
