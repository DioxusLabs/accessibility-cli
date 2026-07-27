//! Dynamic Objective-C message sends for proxied private objects.
//!
//! CoreSimulator vends much of its IO graph as `ROCKRemoteProxy` objects, which
//! implement their interface through forwarding rather than real methods. That
//! breaks `objc2::msg_send!`, whose debug-build verification looks the selector
//! up with `class_getInstanceMethod` and panics when it is absent.
//!
//! These helpers call `objc_msgSend` directly so the message reaches the
//! proxy's forwarding machinery, which is what the selector is for. Callers
//! must guard with [`responds_to`] first — an unrecognized selector reaching
//! forwarding is a hard crash, not a recoverable error.

use std::ffi::c_void;

use objc2::runtime::{AnyObject, Sel};
use objc2::sel;

/// Send a zero-argument message returning an object pointer.
pub(super) unsafe fn send_id(receiver: *mut AnyObject, selector: Sel) -> *mut AnyObject {
    if receiver.is_null() {
        return std::ptr::null_mut();
    }
    type Imp = unsafe extern "C" fn(*mut AnyObject, Sel) -> *mut AnyObject;
    let imp: Imp = unsafe { std::mem::transmute(objc2::ffi::objc_msgSend as *const c_void) };
    unsafe { imp(receiver, selector) }
}

/// Send a one-argument message returning an object pointer.
pub(super) unsafe fn send_id_with_id(
    receiver: *mut AnyObject,
    selector: Sel,
    argument: *mut AnyObject,
) -> *mut AnyObject {
    if receiver.is_null() {
        return std::ptr::null_mut();
    }
    type Imp = unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject) -> *mut AnyObject;
    let imp: Imp = unsafe { std::mem::transmute(objc2::ffi::objc_msgSend as *const c_void) };
    unsafe { imp(receiver, selector, argument) }
}

/// Send `registerScreenCallbacksWithUUID:callbackQueue:frameCallback:
/// surfacesChangedCallback:propertiesChangedCallback:`.
///
/// SimulatorKit retains the three blocks for the lifetime of the registration,
/// so the caller must keep them alive until it unregisters the same UUID.
pub(super) unsafe fn send_register_screen_callbacks(
    receiver: *mut AnyObject,
    selector: Sel,
    uuid: *mut AnyObject,
    queue: *mut c_void,
    frame_callback: *mut c_void,
    surfaces_changed_callback: *mut c_void,
    properties_changed_callback: *mut c_void,
) {
    type Imp = unsafe extern "C" fn(
        *mut AnyObject,
        Sel,
        *mut AnyObject,
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *mut c_void,
    );
    let imp: Imp = unsafe { std::mem::transmute(objc2::ffi::objc_msgSend as *const c_void) };
    unsafe {
        imp(
            receiver,
            selector,
            uuid,
            queue,
            frame_callback,
            surfaces_changed_callback,
            properties_changed_callback,
        )
    }
}

/// Send a one-argument message returning nothing.
pub(super) unsafe fn send_void_with_id(
    receiver: *mut AnyObject,
    selector: Sel,
    argument: *mut AnyObject,
) {
    if receiver.is_null() {
        return;
    }
    type Imp = unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject);
    let imp: Imp = unsafe { std::mem::transmute(objc2::ffi::objc_msgSend as *const c_void) };
    unsafe { imp(receiver, selector, argument) }
}

/// Send a zero-argument message returning an unsigned short.
pub(super) unsafe fn send_u16(receiver: *mut AnyObject, selector: Sel) -> u16 {
    if receiver.is_null() {
        return 0;
    }
    type Imp = unsafe extern "C" fn(*mut AnyObject, Sel) -> u16;
    let imp: Imp = unsafe { std::mem::transmute(objc2::ffi::objc_msgSend as *const c_void) };
    unsafe { imp(receiver, selector) }
}

/// Whether `receiver` will handle `selector`, including via forwarding.
pub(super) unsafe fn responds_to(receiver: *mut AnyObject, selector: Sel) -> bool {
    if receiver.is_null() {
        return false;
    }
    type Imp = unsafe extern "C" fn(*mut AnyObject, Sel, Sel) -> bool;
    let imp: Imp = unsafe { std::mem::transmute(objc2::ffi::objc_msgSend as *const c_void) };
    unsafe { imp(receiver, sel!(respondsToSelector:), selector) }
}
