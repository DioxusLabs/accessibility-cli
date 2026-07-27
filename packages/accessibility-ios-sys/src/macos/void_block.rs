//! Owned, signature-bearing `void (^)(void)` blocks backed by a C shim.
//!
//! See `blocks.c` for why these cannot be `block2::RcBlock`s.

use std::ffi::c_void;

unsafe extern "C" {
    fn accessibility_make_void_block(
        callback: unsafe extern "C" fn(*mut c_void),
        context: *mut c_void,
    ) -> *mut c_void;
    fn accessibility_release_block(block: *mut c_void);
}

/// A heap Objective-C block that invokes a Rust closure when called.
///
/// The block and the boxed closure are released together on drop. SimulatorKit
/// retains the block for as long as the registration is live, so a `VoidBlock`
/// must be kept alive until the matching `unregisterScreenCallbacksWithUUID:`.
pub(super) struct VoidBlock {
    block: *mut c_void,
    closure: *mut Box<dyn Fn() + Send + Sync>,
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
        let block =
            unsafe { accessibility_make_void_block(invoke_closure, closure as *mut c_void) };
        Self { block, closure }
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

impl Drop for VoidBlock {
    fn drop(&mut self) {
        unsafe {
            accessibility_release_block(self.block);
            drop(Box::from_raw(self.closure));
        }
    }
}
