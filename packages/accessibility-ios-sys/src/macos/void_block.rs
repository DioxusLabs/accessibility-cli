//! Owned, signature-bearing `void (^)(void)` blocks backed by an Objective-C
//! shim.
//!
//! See `blocks.m` for why these cannot be `block2::RcBlock`s.

use std::ffi::c_void;

unsafe extern "C" {
    fn accessibility_make_void_block(
        callback: unsafe extern "C" fn(*mut c_void),
        dispose: unsafe extern "C" fn(*mut c_void),
        context: *mut c_void,
    ) -> *mut c_void;
    fn accessibility_release_block(block: *mut c_void);
}

/// A heap Objective-C block that invokes a Rust closure when called.
///
/// The block owns the boxed closure and frees it when the last reference to
/// the block goes away. Dropping the handle only gives up this side's
/// reference: SimulatorKit retains a registered block and still delivers
/// callbacks that were queued before `unregisterScreenCallbacksWithUUID:`, and
/// those must find the closure alive.
pub(super) struct VoidBlock {
    block: *mut c_void,
}

// The closure is `Send + Sync` and the block is invoked by GCD from an
// arbitrary thread, so the handle is safe to move between threads.
unsafe impl Send for VoidBlock {}
unsafe impl Sync for VoidBlock {}

impl VoidBlock {
    pub(super) fn new<F>(closure: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        let boxed: Box<Box<dyn Fn() + Send + Sync>> = Box::new(Box::new(closure));
        let closure = Box::into_raw(boxed);
        let block = unsafe {
            accessibility_make_void_block(invoke_closure, dispose_closure, closure as *mut c_void)
        };
        Self { block }
    }

    /// The raw `id`-compatible block pointer to hand to Objective-C.
    pub(super) fn as_ptr(&self) -> *mut c_void {
        self.block
    }
}

unsafe extern "C" fn invoke_closure(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    let closure = unsafe { &*(context as *const Box<dyn Fn() + Send + Sync>) };
    closure();
}

unsafe extern "C" fn dispose_closure(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(context as *mut Box<dyn Fn() + Send + Sync>) });
}

impl Drop for VoidBlock {
    fn drop(&mut self) {
        unsafe { accessibility_release_block(self.block) };
    }
}
