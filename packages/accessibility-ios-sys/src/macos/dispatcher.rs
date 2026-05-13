use super::*;

/// Global state for routing accessibility requests to the correct simulator.
///
/// The `AXPTranslator` is a singleton, so we use tokens to route requests.
static DISPATCHER_STATE: OnceLock<Mutex<DispatcherState>> = OnceLock::new();

pub(super) struct DispatcherState {
    token_to_device: HashMap<String, *mut AnyObject>,
    callback_queue: *mut AnyObject, // dispatch_queue_t
}

// SimDevice and dispatch_queue_t pointers are not Send, but we manage thread safety
// via the Mutex and only access them appropriately.
unsafe impl Send for DispatcherState {}

impl DispatcherState {
    fn new() -> Self {
        // Create a serial dispatch queue for callbacks
        let queue_label = b"com.accessibility_cli.translator.callback\0";
        let callback_queue: *mut AnyObject = unsafe {
            dispatch_queue_create(
                queue_label.as_ptr() as *const c_char,
                std::ptr::null_mut(), // DISPATCH_QUEUE_SERIAL
            )
        };

        Self {
            token_to_device: HashMap::new(),
            callback_queue,
        }
    }

    pub(super) fn register_device(&mut self, token: String, device: *mut AnyObject) {
        self.token_to_device.insert(token, device);
    }

    pub(super) fn unregister_device(&mut self, token: &str) {
        self.token_to_device.remove(token);
    }

    fn get_device(&self, token: &str) -> Option<*mut AnyObject> {
        self.token_to_device.get(token).copied()
    }

    fn callback_queue(&self) -> *mut AnyObject {
        self.callback_queue
    }
}

pub(super) fn get_dispatcher_state() -> &'static Mutex<DispatcherState> {
    DISPATCHER_STATE.get_or_init(|| Mutex::new(DispatcherState::new()))
}

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    pub(super) fn dispatch_queue_create(label: *const c_char, attr: *mut c_void) -> *mut AnyObject;
    pub(super) fn dispatch_group_create() -> *mut AnyObject;
    pub(super) fn dispatch_group_enter(group: *mut AnyObject);
    pub(super) fn dispatch_group_leave(group: *mut AnyObject);
    pub(super) fn dispatch_group_wait(group: *mut AnyObject, timeout: u64) -> i64;
}

// CoreFoundation retain/release for objects that might not be standard ObjC
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub(super) fn CFRetain(cf: *const c_void) -> *const c_void;
    #[allow(dead_code)]
    pub(super) fn CFRelease(cf: *const c_void);
}

pub(super) const DISPATCH_TIME_FOREVER: u64 = !0u64;

/// Wrapper for raw pointer to make it Send+Sync.
/// Safety: The dispatcher is only created once and accessed from the main thread.
struct DispatcherPtr(*mut AnyObject);
unsafe impl Send for DispatcherPtr {}
unsafe impl Sync for DispatcherPtr {}

struct ResponsePtr(*mut AnyObject);
unsafe impl Send for ResponsePtr {}

/// Global dispatcher instance pointer.
static DISPATCHER_INSTANCE: OnceLock<DispatcherPtr> = OnceLock::new();

/// Register the TranslationDispatcher class and create an instance.
///
/// This creates an Objective-C class at runtime that implements
/// the `AXPTranslationTokenDelegateHelper` protocol.
fn create_dispatcher_class() -> &'static AnyClass {
    static DISPATCHER_CLASS: OnceLock<&'static AnyClass> = OnceLock::new();

    DISPATCHER_CLASS.get_or_init(|| {
        let mut builder =
            ClassBuilder::new(c"AccessibilityCliTranslationDispatcher", NSObject::class())
                .expect("Failed to create TranslationDispatcher class");

        // Add method: accessibilityTranslationDelegateBridgeCallbackWithToken:
        unsafe extern "C-unwind" fn callback_with_token(
            _this: &AnyObject,
            _cmd: Sel,
            token: *mut AnyObject, // NSString *
        ) -> *mut AnyObject {
            callback_with_token_impl(token)
        }

        unsafe {
            builder.add_method(
                sel!(accessibilityTranslationDelegateBridgeCallbackWithToken:),
                callback_with_token as unsafe extern "C-unwind" fn(_, _, _) -> _,
            );
        }

        // Add method: accessibilityTranslationConvertPlatformFrameToSystem:withToken:
        unsafe extern "C-unwind" fn convert_frame(
            _this: &AnyObject,
            _cmd: Sel,
            rect: CGRect,
            _token: *mut AnyObject,
        ) -> CGRect {
            // Return rect unchanged - we're not in a view hierarchy
            rect
        }

        unsafe {
            builder.add_method(
                sel!(accessibilityTranslationConvertPlatformFrameToSystem:withToken:),
                convert_frame as unsafe extern "C-unwind" fn(_, _, _, _) -> _,
            );
        }

        // Add method: accessibilityTranslationRootParentWithToken:
        unsafe extern "C-unwind" fn root_parent(
            _this: &AnyObject,
            _cmd: Sel,
            _token: *mut AnyObject,
        ) -> *mut AnyObject {
            // Return nil - we're not in a view hierarchy
            std::ptr::null_mut()
        }

        unsafe {
            builder.add_method(
                sel!(accessibilityTranslationRootParentWithToken:),
                root_parent as unsafe extern "C-unwind" fn(_, _, _) -> _,
            );
        }

        builder.register()
    })
}

/// Implementation of the callback method.
///
/// Returns a block that synchronously queries the SimDevice for accessibility data.
fn callback_with_token_impl(token_ns: *mut AnyObject) -> *mut AnyObject {
    if token_ns.is_null() {
        return create_empty_response_block();
    }

    let token_str: String = unsafe {
        let cstr: *const c_char = msg_send![token_ns, UTF8String];
        if cstr.is_null() {
            return create_empty_response_block();
        }
        CStr::from_ptr(cstr).to_string_lossy().to_string()
    };

    // Look up the device for this token
    let state = get_dispatcher_state().lock().unwrap();
    let device = state.get_device(&token_str);
    let queue = state.callback_queue();
    drop(state);

    let Some(device) = device else {
        return create_empty_response_block();
    };

    // Create the callback block that will query the SimDevice
    // The block signature is: AXPTranslatorResponse *(^)(AXPTranslatorRequest *)
    let block: RcBlock<dyn Fn(*mut AnyObject) -> *mut AnyObject> =
        RcBlock::new(move |request: *mut AnyObject| -> *mut AnyObject {
            if request.is_null() {
                return create_empty_response();
            }

            // Create dispatch group for synchronization
            let group = unsafe { dispatch_group_create() };
            unsafe { dispatch_group_enter(group) };

            // This will hold the response. The Arc/Mutex is shared with a dispatch
            // block but never crosses threads outside this dispatch group.
            let response_ptr = Arc::new(Mutex::new(ResponsePtr(std::ptr::null_mut())));
            let response_ptr_clone = response_ptr.clone();

            // Create the completion handler block
            // Signature: void (^)(AXPTranslatorResponse *)
            // eprintln!("[DEBUG] Creating completion handler block");
            let completion = RcBlock::new(move |inner_response: *mut AnyObject| {
                // Retain the response to keep it alive across queue boundaries
                // The response might be autoreleased on this queue
                let retained_response = if !inner_response.is_null() {
                    // Use CFRetain since it might be a CF type
                    let ptr = unsafe { CFRetain(inner_response as *const c_void) };
                    ptr as *mut AnyObject
                } else {
                    inner_response
                };

                let mut response = response_ptr_clone.lock().unwrap();
                response.0 = retained_response;
                unsafe { dispatch_group_leave(group) };
            });
            // Call sendAccessibilityRequestAsync:completionQueue:completionHandler:
            unsafe {
                let _: () = msg_send![
                    device,
                    sendAccessibilityRequestAsync: request,
                    completionQueue: queue,
                    completionHandler: &*completion
                ];
            }

            // Wait for the response
            unsafe { dispatch_group_wait(group, DISPATCH_TIME_FOREVER) };

            // Return the response
            response_ptr.lock().unwrap().0
        });

    // Return the block as an Objective-C object
    rcblock_to_objc_ptr(block)
}

/// Create an empty response block.
fn create_empty_response_block() -> *mut AnyObject {
    let block: RcBlock<dyn Fn(*mut AnyObject) -> *mut AnyObject> =
        RcBlock::new(|_request: *mut AnyObject| -> *mut AnyObject { create_empty_response() });
    rcblock_to_objc_ptr(block)
}

/// Convert an RcBlock to a raw pointer for ObjC.
/// The block is leaked and ObjC takes ownership.
///
/// RcBlock<dyn Fn(A) -> R> is a fat pointer (data_ptr + vtable_ptr).
/// ObjC only needs the data_ptr which points to the actual Block struct.
fn rcblock_to_objc_ptr<A: 'static, R: 'static>(block: RcBlock<dyn Fn(A) -> R>) -> *mut AnyObject {
    // RcBlock<dyn Fn(...)> is a fat pointer: (data_ptr, vtable_ptr)
    // The data_ptr points to the heap-allocated Block struct which has
    // the proper ObjC block header layout.
    //
    // Safety: We extract the data pointer and forget the RcBlock so Rust
    // doesn't decrement the refcount. ObjC will call Block_release when done.
    unsafe {
        // Fat pointer is (data_ptr, vtable_ptr) - we need just data_ptr
        // Use raw pointer arithmetic to read the first pointer-sized word
        let fat_ptr_addr = &block as *const RcBlock<dyn Fn(A) -> R> as *const *mut AnyObject;
        let data_ptr = *fat_ptr_addr;
        std::mem::forget(block); // Don't drop, ObjC now owns it
        data_ptr
    }
}

/// Create an empty AXPTranslatorResponse.
fn create_empty_response() -> *mut AnyObject {
    unsafe {
        if let Some(cls) = AnyClass::get(c"AXPTranslatorResponse") {
            msg_send![cls, emptyResponse]
        } else {
            std::ptr::null_mut()
        }
    }
}

/// Get or create the global dispatcher and register it with AXPTranslator.
pub(super) fn ensure_dispatcher_registered(translator: *mut AnyObject) -> Result<()> {
    let dispatcher = DISPATCHER_INSTANCE.get_or_init(|| {
        let cls = create_dispatcher_class();
        let instance: *mut AnyObject = unsafe { msg_send![cls, new] };
        DispatcherPtr(instance)
    });

    // Register as bridgeTokenDelegate
    unsafe {
        // Set supportsDelegateTokens = YES
        let _: () = msg_send![translator, setSupportsDelegateTokens: Bool::YES];

        // Set bridgeTokenDelegate = dispatcher
        let _: () = msg_send![translator, setBridgeTokenDelegate: dispatcher.0];
    }

    Ok(())
}

/// Generate a new UUID token string.
pub(super) fn generate_token() -> String {
    let uuid = NSUUID::new();
    uuid.UUIDString().to_string()
}
