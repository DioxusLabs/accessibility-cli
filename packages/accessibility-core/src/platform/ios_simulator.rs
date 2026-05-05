//! iOS Simulator accessibility and HID support.
//!
//! This module provides:
//! - **Accessibility tree reading** for iOS apps via `AccessibilityPlatformTranslation` framework
//! - **HID injection** (taps, swipes, buttons) via the Indigo protocol and `SimulatorKit`
//!
//! # Accessibility Architecture
//!
//! ```text
//! Rust (IOSSimulatorAccessibility)
//!     ↓ objc2 FFI
//! AccessibilityPlatformTranslation.framework
//!     ↓
//! AXPTranslator singleton ← bridgeTokenDelegate (TranslationDispatcher)
//!     ↓
//! AXPMacPlatformElement
//!     ↓
//! CoreSimulator.framework → SimDevice.sendAccessibilityRequestAsync
//!     ↓
//! XPC → iOS Simulator
//! ```
//!
//! # HID Architecture (Indigo Protocol)
//!
//! ```text
//! Rust (SimulatorHID)
//!     ↓ objc2 FFI
//! SimulatorKit.framework → SimDeviceLegacyHIDClient
//!     ↓
//! IndigoMessage (binary protocol)
//!     ↓
//! Mach messaging → iOS Simulator HID subsystem
//! ```
//!
//! # Multi-Simulator Support
//!
//! The `AXPTranslator` is a singleton, so we use tokens to route requests to the correct
//! simulator. Each accessibility request gets a unique UUID token that maps to a `SimDevice`.

#![allow(unsafe_op_in_unsafe_fn)]

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Result, anyhow};
use block2::RcBlock;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, NSObject, Sel};
use objc2::{ClassType, msg_send, sel};
use objc2_core_foundation::CGRect;
use objc2_foundation::{NSString, NSUUID};

use crate::accessibility::{
    Element, ElementCache, ElementKey, ElementTree, Point, Rect, Screenshot, Size, TreeFilter,
    roles,
};
use slotmap::SecondaryMap;

/// Load the AccessibilityPlatformTranslation private framework.
fn load_axp_framework() -> Result<()> {
    let path = b"/System/Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation\0";

    let handle = unsafe {
        libc::dlopen(
            path.as_ptr() as *const c_char,
            libc::RTLD_NOW | libc::RTLD_GLOBAL,
        )
    };
    if handle.is_null() {
        let error = unsafe { CStr::from_ptr(libc::dlerror()) };
        return Err(anyhow!(
            "Failed to load AccessibilityPlatformTranslation: {}",
            error.to_string_lossy()
        ));
    }
    Ok(())
}

/// Load the CoreSimulator private framework.
fn load_coresimulator_framework() -> Result<()> {
    // Try Xcode's location first
    let paths: &[&[u8]] = &[
        b"/Library/Developer/PrivateFrameworks/CoreSimulator.framework/CoreSimulator\0",
        b"/Applications/Xcode.app/Contents/Developer/Library/PrivateFrameworks/CoreSimulator.framework/CoreSimulator\0",
    ];

    for path in paths {
        let handle = unsafe {
            libc::dlopen(
                path.as_ptr() as *const c_char,
                libc::RTLD_NOW | libc::RTLD_GLOBAL,
            )
        };
        if !handle.is_null() {
            return Ok(());
        }
    }

    let error = unsafe { CStr::from_ptr(libc::dlerror()) };
    Err(anyhow!(
        "Failed to load CoreSimulator framework: {}",
        error.to_string_lossy()
    ))
}

/// Load the SimulatorKit framework from Xcode (needed for HID injection).
fn load_simulatorkit_framework() -> Result<*mut c_void> {
    // First try to get Xcode path dynamically via xcode-select
    let mut paths_to_try: Vec<String> = Vec::new();

    if let Ok(output) = std::process::Command::new("xcode-select")
        .arg("-p")
        .output()
        && output.status.success()
    {
        let dev_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // SimulatorKit is in Library/PrivateFrameworks under the Developer directory
        paths_to_try.push(format!(
            "{}/Library/PrivateFrameworks/SimulatorKit.framework/SimulatorKit",
            dev_path
        ));
    }

    // Fallback hardcoded paths
    paths_to_try.extend([
        "/Applications/Xcode.app/Contents/Developer/Library/PrivateFrameworks/SimulatorKit.framework/SimulatorKit".to_string(),
        "/Applications/Xcode-beta.app/Contents/Developer/Library/PrivateFrameworks/SimulatorKit.framework/SimulatorKit".to_string(),
    ]);

    for path in &paths_to_try {
        let c_path = std::ffi::CString::new(path.as_str()).unwrap();
        let handle = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
        if !handle.is_null() {
            return Ok(handle);
        }
    }

    let error = unsafe { CStr::from_ptr(libc::dlerror()) };
    Err(anyhow!(
        "Failed to load SimulatorKit framework: {}. Tried paths: {:?}",
        error.to_string_lossy(),
        paths_to_try
    ))
}

/// Load all required private frameworks.
pub fn load_frameworks() -> Result<()> {
    load_axp_framework()?;
    load_coresimulator_framework()?;
    // SimulatorKit is loaded lazily when HID is needed
    Ok(())
}

/// Mach message header for Indigo messages.
/// Kept for documentation - we construct messages using raw byte offsets.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
struct MachMessageHeader {
    msgh_bits: u32,
    msgh_size: u32,
    msgh_remote_port: u32,
    msgh_local_port: u32,
    msgh_voucher_port: u32,
    msgh_id: i32,
}

/// Touch event data in Indigo protocol.
/// Coordinates are normalized ratios (0.0 to 1.0).
/// Size: 0x70 (112 bytes)
/// Kept for documentation - we construct messages using raw byte offsets.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Debug, Default)]
#[allow(dead_code)]
struct IndigoTouch {
    field1: u32,  // 0x00 - touch state flags
    field2: u32,  // 0x04 - touch state flags
    field3: u32,  // 0x08
    x_ratio: f64, // 0x0c - 0.0 = left, 1.0 = right
    y_ratio: f64, // 0x14 - 0.0 = top, 1.0 = bottom
    field6: f64,  // 0x1c
    field7: f64,  // 0x24
    field8: f64,  // 0x2c
    field9: u32,  // 0x34
    field10: u32, // 0x38
    field11: u32, // 0x3c
    field12: u32, // 0x40
    field13: u32, // 0x44
    field14: f64, // 0x48
    field15: f64, // 0x50
    field16: f64, // 0x58
    field17: f64, // 0x60
    field18: f64, // 0x68
}

/// Button event data in Indigo protocol.
#[repr(C, packed(4))]
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
struct IndigoButton {
    event_source: u32,
    event_type: u32,
    event_target: u32,
    key_code: u32,
    field5: u32,
}

/// Indigo event union - we use the largest variant (touch) for sizing.
/// The actual event type is determined by IndigoMessage.event_type.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
#[allow(dead_code)]
union IndigoEvent {
    touch: IndigoTouch,
    // button, wheel, etc. are smaller and fit within touch's space
}

impl Default for IndigoEvent {
    fn default() -> Self {
        IndigoEvent {
            touch: IndigoTouch::default(),
        }
    }
}

/// Payload embedded inside an IndigoMessage.
/// Size: 0x80 (128 bytes) - field1(4) + timestamp(8) + field3(4) + event(112)
#[repr(C, packed(4))]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
struct IndigoPayload {
    field1: u32,        // 0x00
    timestamp: u64,     // 0x04 - mach_absolute_time
    field3: u32,        // 0x0c
    event: IndigoEvent, // 0x10
}

/// Complete Indigo message structure.
/// Base size: 0xb0 (176 bytes)
/// For touch events, we allocate extra space for duplicated payload.
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct IndigoMessage {
    header: MachMessageHeader, // 0x00 - 0x18 (24 bytes)
    inner_size: u32,           // 0x18
    event_type: u8,            // 0x1c
    _padding: [u8; 3],         // 0x1d-0x1f
    payload: IndigoPayload,    // 0x20
}

/// Hardware button identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum HardwareButton {
    Home = 0x0,
    Lock = 0x1,
    ApplePay = 0x1f4,
    SideButton = 0xbb8,
    Siri = 0x400002,
}

/// Button event direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ButtonDirection {
    Down = 0x1,
    Up = 0x2,
}

// Button target constants
const BUTTON_EVENT_TARGET_HARDWARE: u32 = 0x33;

/// Indigo event types.
#[allow(dead_code)]
const INDIGO_EVENT_TYPE_BUTTON: u8 = 1;
const INDIGO_EVENT_TYPE_TOUCH: u8 = 2;

// External function for getting mach absolute time
unsafe extern "C" {
    fn mach_absolute_time() -> u64;
}

/// Create a touch message from a template message (from IndigoHIDMessageForMouseNSEvent).
///
/// This extracts the touch payload from the template and creates a proper message
/// with duplicated payloads as required by the iOS Simulator.
fn create_touch_message_from_template(
    template: *mut c_void,
    x_ratio: f64,
    y_ratio: f64,
    direction: ButtonDirection,
) -> *mut c_void {
    const MESSAGE_SIZE: usize = 0x140; // 320 bytes
    const PAYLOAD_STRIDE: usize = 0x80; // 128 bytes

    let message = unsafe { libc::calloc(1, MESSAGE_SIZE) as *mut u8 };
    if message.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let template_ptr = template as *mut u8;

        // Copy the header portion from template (first 0x20 bytes)
        std::ptr::copy_nonoverlapping(template_ptr, message, 0x20);

        // Set inner_size to payload stride
        std::ptr::write_unaligned(message.add(0x18) as *mut u32, PAYLOAD_STRIDE as u32);

        // Set event_type to touch
        *message.add(0x1c) = INDIGO_EVENT_TYPE_TOUCH;

        // Payload at offset 0x20
        let payload_ptr = message.add(0x20);

        // payload.field1 = 0x0b (from idb)
        std::ptr::write_unaligned(payload_ptr as *mut u32, 0x0000000bu32);

        // payload.timestamp
        std::ptr::write_unaligned(payload_ptr.add(0x04) as *mut u64, mach_absolute_time());

        // Copy the touch event data from template (at offset 0x30)
        // Touch data is 0x70 bytes
        std::ptr::copy_nonoverlapping(template_ptr.add(0x30), message.add(0x30), 0x70);

        // Patch x/y ratios
        let touch_ptr = message.add(0x30);
        std::ptr::write_unaligned(touch_ptr.add(0x0c) as *mut f64, x_ratio);
        std::ptr::write_unaligned(touch_ptr.add(0x14) as *mut f64, y_ratio);

        // Set touch state flags
        let (field1_val, field2_val) = match direction {
            ButtonDirection::Down => (0x01u32, 0x01u32),
            ButtonDirection::Up => (0x00u32, 0x00u32),
        };
        std::ptr::write_unaligned(touch_ptr as *mut u32, field1_val);
        std::ptr::write_unaligned(touch_ptr.add(0x04) as *mut u32, field2_val);

        // Duplicate the payload
        let second_payload_ptr = payload_ptr.add(PAYLOAD_STRIDE);
        std::ptr::copy_nonoverlapping(payload_ptr, second_payload_ptr, PAYLOAD_STRIDE);

        // Adjust second payload's touch fields
        let second_touch_ptr = second_payload_ptr.add(0x10);
        std::ptr::write_unaligned(second_touch_ptr as *mut u32, 0x00000001u32);
        std::ptr::write_unaligned(second_touch_ptr.add(0x04) as *mut u32, 0x00000002u32);
    }

    message as *mut c_void
}

/// Get the AXPTranslator singleton.
///
/// # Safety
/// Frameworks must be loaded first via `load_frameworks()`.
unsafe fn get_translator() -> Result<*mut AnyObject> {
    let cls =
        AnyClass::get(c"AXPTranslator").ok_or_else(|| anyhow!("AXPTranslator class not found"))?;

    let translator: *mut AnyObject = msg_send![cls, sharedInstance];
    if translator.is_null() {
        return Err(anyhow!("Failed to get AXPTranslator sharedInstance"));
    }

    Ok(translator)
}

/// Get the default SimDeviceSet via SimServiceContext.
///
/// # Safety
/// CoreSimulator framework must be loaded.
unsafe fn get_device_set() -> Result<*mut AnyObject> {
    // Get SimServiceContext class
    let ctx_cls = AnyClass::get(c"SimServiceContext")
        .ok_or_else(|| anyhow!("SimServiceContext class not found"))?;

    // Get shared context for current developer dir (nil = use default)
    let mut error: *mut AnyObject = std::ptr::null_mut();
    let context: *mut AnyObject = msg_send![
        ctx_cls,
        sharedServiceContextForDeveloperDir: std::ptr::null::<AnyObject>(),
        error: &mut error
    ];

    if context.is_null() {
        if !error.is_null() {
            let desc: *mut AnyObject = msg_send![error, localizedDescription];
            let error_str =
                nsstring_to_string_static(desc).unwrap_or_else(|| "Unknown error".to_string());
            return Err(anyhow!("Failed to get SimServiceContext: {}", error_str));
        }
        return Err(anyhow!("Failed to get SimServiceContext: unknown error"));
    }

    // Get default device set
    let mut error: *mut AnyObject = std::ptr::null_mut();
    let device_set: *mut AnyObject = msg_send![context, defaultDeviceSetWithError: &mut error];

    if device_set.is_null() {
        if !error.is_null() {
            let desc: *mut AnyObject = msg_send![error, localizedDescription];
            let error_str =
                nsstring_to_string_static(desc).unwrap_or_else(|| "Unknown error".to_string());
            return Err(anyhow!("Failed to get default device set: {}", error_str));
        }
        return Err(anyhow!("Failed to get default device set: unknown error"));
    }

    Ok(device_set)
}

/// Convert NSString to Rust String (standalone function).
unsafe fn nsstring_to_string_static(ns_string: *mut AnyObject) -> Option<String> {
    if ns_string.is_null() {
        return None;
    }
    let cstr: *const c_char = msg_send![ns_string, UTF8String];
    if cstr.is_null() {
        return None;
    }
    Some(CStr::from_ptr(cstr).to_string_lossy().to_string())
}

/// Find a booted simulator device by UDID or return the first booted one.
///
/// # Safety
/// CoreSimulator framework must be loaded.
unsafe fn find_booted_device(udid: Option<&str>) -> Result<*mut AnyObject> {
    let device_set = get_device_set()?;

    // Get all devices
    let devices: *mut AnyObject = msg_send![device_set, devices];
    if devices.is_null() {
        return Err(anyhow!("No devices found in SimDeviceSet"));
    }

    let count: usize = msg_send![devices, count];

    for i in 0..count {
        let device: *mut AnyObject = msg_send![devices, objectAtIndex: i];
        if device.is_null() {
            continue;
        }

        // Check if booted (state == 3)
        let state: i64 = msg_send![device, state];
        if state != 3 {
            // Not booted
            continue;
        }

        // Get UDID
        let device_udid: *mut AnyObject = msg_send![device, UDID];
        if device_udid.is_null() {
            continue;
        }

        let udid_string: *mut AnyObject = msg_send![device_udid, UUIDString];
        if udid_string.is_null() {
            continue;
        }

        let udid_cstr: *const c_char = msg_send![udid_string, UTF8String];
        let device_udid_str = CStr::from_ptr(udid_cstr).to_string_lossy();

        // If we're looking for a specific UDID, check it
        if let Some(target_udid) = udid {
            if device_udid_str == target_udid {
                return Ok(device);
            }
        } else {
            // Return first booted device
            return Ok(device);
        }
    }

    if let Some(target_udid) = udid {
        Err(anyhow!(
            "No booted simulator found with UDID: {}",
            target_udid
        ))
    } else {
        Err(anyhow!("No booted simulator found"))
    }
}

/// Global state for routing accessibility requests to the correct simulator.
///
/// The `AXPTranslator` is a singleton, so we use tokens to route requests.
static DISPATCHER_STATE: OnceLock<Mutex<DispatcherState>> = OnceLock::new();

struct DispatcherState {
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

    fn register_device(&mut self, token: String, device: *mut AnyObject) {
        self.token_to_device.insert(token, device);
    }

    fn unregister_device(&mut self, token: &str) {
        self.token_to_device.remove(token);
    }

    fn get_device(&self, token: &str) -> Option<*mut AnyObject> {
        self.token_to_device.get(token).copied()
    }

    fn callback_queue(&self) -> *mut AnyObject {
        self.callback_queue
    }
}

fn get_dispatcher_state() -> &'static Mutex<DispatcherState> {
    DISPATCHER_STATE.get_or_init(|| Mutex::new(DispatcherState::new()))
}

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *mut c_void) -> *mut AnyObject;
    fn dispatch_group_create() -> *mut AnyObject;
    fn dispatch_group_enter(group: *mut AnyObject);
    fn dispatch_group_leave(group: *mut AnyObject);
    fn dispatch_group_wait(group: *mut AnyObject, timeout: u64) -> i64;
}

// CoreFoundation retain/release for objects that might not be standard ObjC
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRetain(cf: *const c_void) -> *const c_void;
    #[allow(dead_code)]
    fn CFRelease(cf: *const c_void);
}

const DISPATCH_TIME_FOREVER: u64 = !0u64;

/// Wrapper for raw pointer to make it Send+Sync.
/// Safety: The dispatcher is only created once and accessed from the main thread.
struct DispatcherPtr(*mut AnyObject);
unsafe impl Send for DispatcherPtr {}
unsafe impl Sync for DispatcherPtr {}

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
            #[allow(clippy::arc_with_non_send_sync)]
            let response_ptr: Arc<Mutex<*mut AnyObject>> =
                Arc::new(Mutex::new(std::ptr::null_mut()));
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
                *response = retained_response;
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
            let response = response_ptr.lock().unwrap();
            *response
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
fn ensure_dispatcher_registered(translator: *mut AnyObject) -> Result<()> {
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
fn generate_token() -> String {
    let uuid = NSUUID::new();
    uuid.UUIDString().to_string()
}

/// Function pointer types for Indigo message creation (loaded from SimulatorKit via dlsym).
type IndigoMessageForButtonFn =
    unsafe extern "C" fn(source: i32, action: i32, target: i32) -> *mut c_void;
type IndigoMessageForTouchFn = unsafe extern "C" fn(
    point0: *const objc2_core_foundation::CGPoint,
    point1: *const objc2_core_foundation::CGPoint,
    target: i32,
    event_type: i32,
    something: Bool,
) -> *mut c_void;
type IndigoMessageForKeyboardFn = unsafe extern "C" fn(key_code: i32, action: i32) -> *mut c_void;

/// HID injection client for iOS Simulator.
///
/// Uses the Indigo protocol via SimulatorKit's SimDeviceLegacyHIDClient
/// to inject touch events, button presses, and keyboard input directly
/// into the simulator's HID subsystem.
pub struct SimulatorHID {
    client: *mut AnyObject, // SimDeviceLegacyHIDClient
    queue: *mut AnyObject,  // dispatch_queue_t
    screen_size: (f64, f64),
    screen_scale: f64,
    // Function pointers for message creation
    msg_for_button: IndigoMessageForButtonFn,
    msg_for_touch: IndigoMessageForTouchFn,
    msg_for_keyboard: IndigoMessageForKeyboardFn,
}

unsafe impl Send for SimulatorHID {}

impl SimulatorHID {
    /// Create a new HID client for a simulator device.
    ///
    /// # Arguments
    /// * `device` - A SimDevice pointer (from CoreSimulator)
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn new(device: *mut AnyObject) -> Result<Self> {
        // Load SimulatorKit and get function pointers
        let handle = load_simulatorkit_framework()?;

        let msg_for_button: IndigoMessageForButtonFn = unsafe {
            let sym = libc::dlsym(handle, c"IndigoHIDMessageForButton".as_ptr());
            if sym.is_null() {
                return Err(anyhow!("Failed to find IndigoHIDMessageForButton"));
            }
            std::mem::transmute(sym)
        };

        let msg_for_touch: IndigoMessageForTouchFn = unsafe {
            let sym = libc::dlsym(handle, c"IndigoHIDMessageForMouseNSEvent".as_ptr());
            if sym.is_null() {
                return Err(anyhow!("Failed to find IndigoHIDMessageForMouseNSEvent"));
            }
            std::mem::transmute(sym)
        };

        let msg_for_keyboard: IndigoMessageForKeyboardFn = unsafe {
            let sym = libc::dlsym(handle, c"IndigoHIDMessageForKeyboardArbitrary".as_ptr());
            if sym.is_null() {
                return Err(anyhow!(
                    "Failed to find IndigoHIDMessageForKeyboardArbitrary"
                ));
            }
            std::mem::transmute(sym)
        };

        // Get SimDeviceLegacyHIDClient class
        // Try both the ObjC module-qualified name and the Swift mangled name
        let client_class = AnyClass::get(c"SimulatorKit.SimDeviceLegacyHIDClient")
            .or_else(|| AnyClass::get(c"_TtC12SimulatorKit24SimDeviceLegacyHIDClient"))
            .ok_or_else(|| {
                anyhow!("SimDeviceLegacyHIDClient class not found. Is SimulatorKit loaded?")
            })?;

        // Create HID client instance
        // Selector: initWithDevice:sessionResetQueue:error:sessionResetHandler:
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let client: *mut AnyObject = unsafe {
            let alloc: *mut AnyObject = msg_send![client_class, alloc];
            let null_ptr: *mut AnyObject = std::ptr::null_mut();
            msg_send![alloc, initWithDevice: device, sessionResetQueue: null_ptr, error: &mut error, sessionResetHandler: null_ptr]
        };

        if client.is_null() {
            let error_msg = if !error.is_null() {
                unsafe {
                    let desc: *mut AnyObject = msg_send![error, localizedDescription];
                    nsstring_to_string_static(desc).unwrap_or_else(|| "Unknown error".to_string())
                }
            } else {
                "Unknown error".to_string()
            };
            return Err(anyhow!("Failed to create HID client: {}", error_msg));
        }

        // Get screen size from device type
        let (screen_size, screen_scale) = unsafe {
            let device_type: *mut AnyObject = msg_send![device, deviceType];
            if device_type.is_null() {
                ((390.0, 844.0), 3.0) // Default iPhone 14 size
            } else {
                let size: objc2_core_foundation::CGSize = msg_send![device_type, mainScreenSize];
                let scale: f32 = msg_send![device_type, mainScreenScale];
                ((size.width, size.height), scale as f64)
            }
        };

        // Create dispatch queue for HID operations
        let queue_label = b"com.accessibility_cli.hid\0";
        let queue: *mut AnyObject = unsafe {
            dispatch_queue_create(queue_label.as_ptr() as *const c_char, std::ptr::null_mut())
        };

        Ok(Self {
            client,
            queue,
            screen_size,
            screen_scale,
            msg_for_button,
            msg_for_touch,
            msg_for_keyboard,
        })
    }

    /// Get the screen size in points.
    pub fn screen_size(&self) -> (f64, f64) {
        self.screen_size
    }

    /// Tap at screen coordinates (in points).
    ///
    /// This sends a touch-down followed by touch-up at the given position.
    pub fn tap(&self, x: f64, y: f64) -> Result<()> {
        // Convert point coordinates to ratio (0.0 - 1.0)
        let x_ratio = (x * self.screen_scale) / self.screen_size.0;
        let y_ratio = (y * self.screen_scale) / self.screen_size.1;

        // Touch down
        self.send_touch(x_ratio, y_ratio, ButtonDirection::Down)?;

        // Small delay (matches idb behavior)
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Touch up
        self.send_touch(x_ratio, y_ratio, ButtonDirection::Up)?;

        Ok(())
    }

    /// Perform a swipe gesture from one point to another.
    ///
    /// # Arguments
    /// * `start` - Starting coordinates (x, y) in points
    /// * `end` - Ending coordinates (x, y) in points
    /// * `duration_ms` - Duration of the swipe in milliseconds
    pub fn swipe(&self, start: (f64, f64), end: (f64, f64), duration_ms: u64) -> Result<()> {
        let steps = (duration_ms / 16).max(5) as usize; // ~60fps, minimum 5 steps
        let step_delay = std::time::Duration::from_millis(duration_ms / steps as u64);

        // Convert to ratios
        let start_x_ratio = (start.0 * self.screen_scale) / self.screen_size.0;
        let start_y_ratio = (start.1 * self.screen_scale) / self.screen_size.1;
        let end_x_ratio = (end.0 * self.screen_scale) / self.screen_size.0;
        let end_y_ratio = (end.1 * self.screen_scale) / self.screen_size.1;

        // Touch down at start
        self.send_touch(start_x_ratio, start_y_ratio, ButtonDirection::Down)?;

        // Move through intermediate points
        for i in 1..steps {
            let t = i as f64 / steps as f64;
            let x = start_x_ratio + (end_x_ratio - start_x_ratio) * t;
            let y = start_y_ratio + (end_y_ratio - start_y_ratio) * t;

            std::thread::sleep(step_delay);
            self.send_touch(x, y, ButtonDirection::Down)?;
        }

        // Touch up at end
        std::thread::sleep(step_delay);
        self.send_touch(end_x_ratio, end_y_ratio, ButtonDirection::Up)?;

        Ok(())
    }

    /// Press a hardware button.
    ///
    /// # Arguments
    /// * `button` - Which button to press
    /// * `hold_ms` - How long to hold the button (0 for tap)
    pub fn press_button(&self, button: HardwareButton, hold_ms: u64) -> Result<()> {
        // Button down
        self.send_button(button, ButtonDirection::Down)?;

        if hold_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(hold_ms));
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        // Button up
        self.send_button(button, ButtonDirection::Up)?;

        Ok(())
    }

    /// Send a keyboard key press.
    ///
    /// # Arguments
    /// * `key_code` - The key code (from HIToolbox/Events.h)
    pub fn send_key(&self, key_code: u32) -> Result<()> {
        // Key down
        self.send_keyboard(key_code, ButtonDirection::Down)?;

        std::thread::sleep(std::time::Duration::from_millis(30));

        // Key up
        self.send_keyboard(key_code, ButtonDirection::Up)?;

        Ok(())
    }

    /// Send a touch event at the given ratio coordinates.
    fn send_touch(&self, x_ratio: f64, y_ratio: f64, direction: ButtonDirection) -> Result<()> {
        // First get a template message from IndigoHIDMessageForMouseNSEvent
        let point = objc2_core_foundation::CGPoint {
            x: x_ratio,
            y: y_ratio,
        };

        let event_type = match direction {
            ButtonDirection::Down => 1,
            ButtonDirection::Up => 2,
        };

        let template_msg =
            unsafe { (self.msg_for_touch)(&point, std::ptr::null(), 0x32, event_type, Bool::NO) };

        if template_msg.is_null() {
            return Err(anyhow!("Failed to create template touch message"));
        }

        // Patch the x/y ratios like idb does
        unsafe {
            let touch_ptr = (template_msg as *mut u8).add(0x30);
            std::ptr::write_unaligned(touch_ptr.add(0x0c) as *mut f64, x_ratio);
            std::ptr::write_unaligned(touch_ptr.add(0x14) as *mut f64, y_ratio);
        }

        // Now create the proper touch message with duplicated payload
        let message = create_touch_message_from_template(template_msg, x_ratio, y_ratio, direction);

        // Free the template
        unsafe { libc::free(template_msg) };

        if message.is_null() {
            return Err(anyhow!("Failed to create touch message"));
        }

        self.send_message(message, true)
    }

    /// Send a button event.
    fn send_button(&self, button: HardwareButton, direction: ButtonDirection) -> Result<()> {
        let message = unsafe {
            (self.msg_for_button)(
                button as i32,
                direction as i32,
                BUTTON_EVENT_TARGET_HARDWARE as i32,
            )
        };

        if message.is_null() {
            return Err(anyhow!("Failed to create button message"));
        }

        self.send_message(message, true)
    }

    /// Send a keyboard event.
    fn send_keyboard(&self, key_code: u32, direction: ButtonDirection) -> Result<()> {
        let message = unsafe { (self.msg_for_keyboard)(key_code as i32, direction as i32) };

        if message.is_null() {
            return Err(anyhow!("Failed to create keyboard message"));
        }

        self.send_message(message, true)
    }

    /// Send an Indigo message to the HID client.
    fn send_message(&self, message: *mut c_void, free_when_done: bool) -> Result<()> {
        // Create dispatch group for synchronization
        let group = unsafe { dispatch_group_create() };
        unsafe { dispatch_group_enter(group) };

        let error_ptr: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let error_ptr_clone = error_ptr.clone();

        // Create completion block
        let completion = RcBlock::new(move |error: *mut AnyObject| {
            if !error.is_null() {
                let desc: *mut AnyObject = unsafe { msg_send![error, localizedDescription] };
                if let Some(msg) = unsafe { nsstring_to_string_static(desc) } {
                    *error_ptr_clone.lock().unwrap() = Some(msg);
                }
            }
            unsafe { dispatch_group_leave(group) };
        });

        // Use objc_msgSend directly to bypass Swift's strict type checking
        // Selector: sendWithMessage:freeWhenDone:completionQueue:completion:
        unsafe {
            let sel = objc2::sel!(sendWithMessage:freeWhenDone:completionQueue:completion:);

            type MsgSendFn = unsafe extern "C" fn(
                *mut AnyObject,
                objc2::runtime::Sel,
                *mut c_void,
                Bool,
                *mut AnyObject,
                *const block2::Block<dyn Fn(*mut AnyObject)>,
            );
            let msg_send_fn: MsgSendFn = std::mem::transmute(objc2::ffi::objc_msgSend as *const ());

            msg_send_fn(
                self.client,
                sel,
                message,
                Bool::from(free_when_done),
                self.queue,
                &*completion as *const _,
            );
        }

        // Wait for completion
        unsafe { dispatch_group_wait(group, DISPATCH_TIME_FOREVER) };

        // Check for error
        if let Some(error_msg) = error_ptr.lock().unwrap().take() {
            return Err(anyhow!("HID send failed: {}", error_msg));
        }

        Ok(())
    }
}

impl Drop for SimulatorHID {
    fn drop(&mut self) {
        // Client and queue will be released by ARC when they go out of scope
        // No explicit cleanup needed
    }
}

/// iOS Simulator accessibility reader.
///
/// Provides access to the accessibility tree of iOS apps running in the iOS Simulator.
pub struct IOSSimulatorAccessibility {
    translator: *mut AnyObject,
    device: *mut AnyObject,
    device_udid: String,
    cache: ElementCache,
    /// Map of element keys to retained ObjC element pointers for action support.
    /// Uses SecondaryMap which is automatically synchronized with the primary SlotMap in cache.
    /// These are retained with CFRetain and must be released on clear.
    element_ptrs: SecondaryMap<ElementKey, *mut AnyObject>,
    /// The token used for the current tree query (needed for actions).
    current_token: Option<String>,
    /// HID client for direct input injection (lazy-initialized).
    hid: Option<SimulatorHID>,
    /// The app's bounds in macOS screen coordinates (from root element's accessibilityFrame).
    /// Used to convert accessibility coordinates to device-local coordinates for screenshots.
    app_bounds: Option<Rect>,
}

// Raw pointers are not Send/Sync, but we manage thread safety via the global DISPATCHER
unsafe impl Send for IOSSimulatorAccessibility {}

impl IOSSimulatorAccessibility {
    /// Create a new iOS Simulator accessibility reader.
    ///
    /// If `udid` is None, uses the first booted simulator found.
    pub fn new(udid: Option<&str>) -> Result<Self> {
        // Load frameworks
        load_frameworks()?;

        // Get translator singleton
        let translator = unsafe { get_translator()? };

        // Find booted device
        let device = unsafe { find_booted_device(udid)? };

        // Get device UDID for identification
        let device_udid = unsafe {
            let udid_obj: *mut AnyObject = msg_send![device, UDID];
            let udid_string: *mut AnyObject = msg_send![udid_obj, UUIDString];
            let udid_cstr: *const c_char = msg_send![udid_string, UTF8String];
            CStr::from_ptr(udid_cstr).to_string_lossy().to_string()
        };

        // Register our delegate with the translator
        ensure_dispatcher_registered(translator)?;

        Ok(Self {
            translator,
            device,
            device_udid,
            cache: ElementCache::new(),
            element_ptrs: SecondaryMap::new(),
            current_token: None,
            hid: None,
            app_bounds: None,
        })
    }

    /// Get the device UDID.
    pub fn device_udid(&self) -> &str {
        &self.device_udid
    }

    /// Get the accessibility tree from the frontmost app in the simulator.
    pub fn get_tree(&mut self, filter: &TreeFilter) -> Result<ElementTree> {
        // Clear previous cache
        self.clear_cache();

        let token = generate_token();

        // Register this device with the token
        {
            let mut state = get_dispatcher_state().lock().unwrap();
            state.register_device(token.clone(), self.device);
        }

        // Try to get the frontmost application
        let result = unsafe { self.query_frontmost_app(&token, filter) };

        // Store the token for later action use (don't unregister yet)
        // The token will be unregistered when clear_cache is called
        self.current_token = Some(token);

        result
    }

    /// Query the frontmost application's accessibility tree.
    unsafe fn query_frontmost_app(
        &mut self,
        token: &str,
        filter: &TreeFilter,
    ) -> Result<ElementTree> {
        self.query_frontmost_app_with_retry(token, filter, true)
    }

    /// Query the frontmost application with optional retry on accessibility failure.
    unsafe fn query_frontmost_app_with_retry(
        &mut self,
        token: &str,
        filter: &TreeFilter,
        allow_remediation: bool,
    ) -> Result<ElementTree> {
        let token_ns = NSString::from_str(token);

        // Call frontmostApplicationWithDisplayId:bridgeDelegateToken:
        let translation: *mut AnyObject = msg_send![
            self.translator,
            frontmostApplicationWithDisplayId: 0u32,
            bridgeDelegateToken: &*token_ns
        ];

        if translation.is_null() {
            return Err(anyhow!(
                "Failed to get frontmost application. Ensure a simulator is running with an app in focus."
            ));
        }

        // Set the token on the translation object
        let _: () = msg_send![translation, setBridgeDelegateToken: &*token_ns];

        // Convert to platform element
        let element: *mut AnyObject = msg_send![
            self.translator,
            macPlatformElementFromTranslation: translation
        ];

        if element.is_null() {
            return Err(anyhow!("Failed to get platform element from translation"));
        }

        // IMPORTANT: Set token on element.translation as well (may be different from original translation)
        let element_translation: *mut AnyObject = msg_send![element, translation];
        if !element_translation.is_null() {
            let _: () = msg_send![element_translation, setBridgeDelegateToken: &*token_ns];
        }

        // Check for zero-sized frame (indicates accessibility subsystem problem)
        // This typically happens when SpringBoard has crashed and CoreSimulatorBridge
        // needs to be restarted.
        let frame: CGRect = msg_send![element, accessibilityFrame];
        if frame.size.width == 0.0 && frame.size.height == 0.0 && allow_remediation {
            // Try remediation: restart CoreSimulatorBridge
            if self.remediate_accessibility()? {
                // Retry the query after remediation (without allowing further remediation)
                return self.query_frontmost_app_with_retry(token, filter, false);
            }
        }

        // Store the app bounds for screenshot coordinate conversion.
        // iOS accessibility coordinates are in macOS screen space, but xcrun simctl screenshot
        // captures device-local coordinates starting at (0,0). We need to subtract the app's
        // origin to convert accessibility bounds to device-local coordinates.
        self.app_bounds = Some(Rect::new(
            Point::new(frame.origin.x, frame.origin.y),
            Size::new(frame.size.width, frame.size.height),
        ));

        // Get app info
        let pid: i32 = msg_send![translation, pid];
        let app_name = self.get_element_label(element);

        // Build tree recursively
        let root = self.build_element_tree(element, token, filter, 0)?;

        let element_count = self.count_elements(&root);

        Ok(ElementTree {
            root,
            app_name,
            pid: Some(pid as u32),
            version: self.cache.version(),
            element_count,
        })
    }

    /// Attempt to remediate accessibility issues by restarting CoreSimulatorBridge.
    ///
    /// This is based on idb's approach: when the accessibility frame is zero-sized,
    /// it typically means SpringBoard has crashed and the bridge needs restarting.
    ///
    /// Returns `Ok(true)` if remediation was attempted, `Ok(false)` if not needed,
    /// or an error if remediation failed.
    fn remediate_accessibility(&self) -> Result<bool> {
        eprintln!("[WARN] Detected zero-sized accessibility frame - attempting remediation");
        eprintln!(
            "[WARN] This usually means SpringBoard crashed and CoreSimulatorBridge needs restart"
        );

        // Get the device UDID for the launchctl command
        let udid = &self.device_udid;

        // Restart CoreSimulatorBridge via launchctl
        // The service name pattern is: com.apple.CoreSimulator.bridge.<UDID>
        let service_name = format!("com.apple.CoreSimulator.bridge.{}", udid);

        // Use xcrun simctl to stop and restart the bridge
        // This is safer than directly calling launchctl
        let output = std::process::Command::new("xcrun")
            .args([
                "simctl",
                "spawn",
                udid,
                "launchctl",
                "kickstart",
                "-k",
                &format!("system/{}", service_name),
            ])
            .output();

        match output {
            Ok(output) => {
                if output.status.success() {
                    eprintln!("[INFO] Successfully restarted CoreSimulatorBridge");
                    // Give the bridge a moment to restart
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    Ok(true)
                } else {
                    // If kickstart fails, try using simctl directly
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    eprintln!(
                        "[WARN] Failed to restart via launchctl ({}), trying alternative...",
                        stderr.trim()
                    );

                    // Alternative: use simctl shutdown and boot
                    // This is more disruptive but more reliable
                    // For now, just return an error with instructions
                    Err(anyhow!(
                        "Accessibility subsystem appears to be in a bad state (zero-sized frame). \
                        This typically happens when SpringBoard has crashed. \
                        Try restarting the simulator or running: \
                        xcrun simctl shutdown {} && xcrun simctl boot {}",
                        udid,
                        udid
                    ))
                }
            }
            Err(e) => Err(anyhow!(
                "Failed to restart CoreSimulatorBridge: {}. \
                    Try restarting the simulator manually.",
                e
            )),
        }
    }

    /// Build an Element from an AXPMacPlatformElement.
    unsafe fn build_element_tree(
        &mut self,
        element: *mut AnyObject,
        token: &str,
        filter: &TreeFilter,
        depth: usize,
    ) -> Result<Element> {
        // Check depth limit
        if let Some(max_depth) = filter.max_depth
            && depth > max_depth
        {
            return self.build_leaf_element(element);
        }

        // Check element count limit
        if let Some(max_elements) = filter.max_elements
            && self.cache.len() >= max_elements
        {
            return self.build_leaf_element(element);
        }

        // IMPORTANT: Always set token on element's translation before accessing any properties
        // This ensures the delegate callback can route requests to the correct simulator
        let token_ns = NSString::from_str(token);
        let translation: *mut AnyObject = msg_send![element, translation];
        if !translation.is_null() {
            let _: () = msg_send![translation, setBridgeDelegateToken: &*token_ns];
        }

        // Extract properties
        let role = self.get_element_role(element);
        let title = self.get_element_label(element);
        let value = self.get_element_value(element);
        let description = self.get_element_title(element);
        let url = self.get_element_url(element);
        let bounds = self.get_element_frame(element);
        let enabled = self.get_element_enabled(element);
        let focused = self.get_element_focused(element);
        let actions = self.get_element_actions(element);

        // Check interactive filter
        if filter.interactive_only && !Self::is_interactive(&role, &actions) {
            // Skip non-interactive elements but still process children
        }

        // Get children
        let mut children = Vec::new();
        let children_array: *mut AnyObject = msg_send![element, accessibilityChildren];

        if !children_array.is_null() {
            let count: usize = msg_send![children_array, count];

            for i in 0..count {
                let child: *mut AnyObject = msg_send![children_array, objectAtIndex: i];
                if child.is_null() {
                    continue;
                }

                // Set token on child's translation BEFORE accessing any properties
                let child_translation: *mut AnyObject = msg_send![child, translation];
                if !child_translation.is_null() {
                    let _: () = msg_send![child_translation, setBridgeDelegateToken: &*token_ns];
                }

                if let Ok(child_element) = self.build_element_tree(child, token, filter, depth + 1)
                {
                    children.push(child_element);
                }
            }
        }

        // Store in cache with the final ID
        let (id, elem) = self.cache.store_with_clone(|id| Element {
            id,
            role,
            title,
            value,
            description,
            url,
            help: None,
            role_description: None,
            identifier: None,
            bounds,
            enabled,
            focused,
            actions,
            children,
        });

        // Retain the element pointer for later action support
        let retained = CFRetain(element as *const c_void) as *mut AnyObject;
        self.element_ptrs.insert(id, retained);

        Ok(elem)
    }

    /// Build a leaf element (no children due to depth/count limit).
    unsafe fn build_leaf_element(&mut self, element: *mut AnyObject) -> Result<Element> {
        let role = self.get_element_role(element);
        let title = self.get_element_label(element);
        let value = self.get_element_value(element);
        let description = self.get_element_title(element);
        let url = self.get_element_url(element);
        let bounds = self.get_element_frame(element);
        let enabled = self.get_element_enabled(element);
        let focused = self.get_element_focused(element);
        let actions = self.get_element_actions(element);

        // Store in cache with the final ID
        let (id, elem) = self.cache.store_with_clone(|id| Element {
            id,
            role,
            title,
            value,
            description,
            url,
            help: None,
            role_description: None,
            identifier: None,
            bounds,
            enabled,
            focused,
            actions,
            children: Vec::new(),
        });

        // Retain the element pointer for later action support
        let retained = CFRetain(element as *const c_void) as *mut AnyObject;
        self.element_ptrs.insert(id, retained);

        Ok(elem)
    }

    /// Get element label (accessibilityLabel).
    unsafe fn get_element_label(&self, element: *mut AnyObject) -> Option<String> {
        let label: *mut AnyObject = msg_send![element, accessibilityLabel];
        self.nsstring_to_string(label)
    }

    /// Get element title (accessibilityTitle).
    unsafe fn get_element_title(&self, element: *mut AnyObject) -> Option<String> {
        let title: *mut AnyObject = msg_send![element, accessibilityTitle];
        self.nsstring_to_string(title)
    }

    /// Get element value (accessibilityValue).
    unsafe fn get_element_value(&self, element: *mut AnyObject) -> Option<String> {
        let value: *mut AnyObject = msg_send![element, accessibilityValue];
        if value.is_null() {
            return None;
        }

        // Value can be various types, try to get string representation
        let desc: *mut AnyObject = msg_send![value, description];
        self.nsstring_to_string(desc)
    }

    /// Get element URL (accessibilityURL).
    /// Returns the URL as a string for link elements.
    unsafe fn get_element_url(&self, element: *mut AnyObject) -> Option<String> {
        // Try accessibilityURL first (standard accessibility API)
        let responds_url: Bool = msg_send![element, respondsToSelector: sel!(accessibilityURL)];
        if responds_url.as_bool() {
            let url: *mut AnyObject = msg_send![element, accessibilityURL];
            if !url.is_null() {
                // URL is an NSURL, get absoluteString
                let abs_string: *mut AnyObject = msg_send![url, absoluteString];
                if let Some(s) = self.nsstring_to_string(abs_string) {
                    return Some(s);
                }
            }
        }

        // Try accessibilityAttributeValue: with AXURL
        let responds_attr: Bool =
            msg_send![element, respondsToSelector: sel!(accessibilityAttributeValue:)];
        if responds_attr.as_bool() {
            let attr = NSString::from_str("AXURL");
            let url: *mut AnyObject = msg_send![element, accessibilityAttributeValue: &*attr];
            if !url.is_null() {
                let abs_string: *mut AnyObject = msg_send![url, absoluteString];
                if let Some(s) = self.nsstring_to_string(abs_string) {
                    return Some(s);
                }
            }
        }

        None
    }

    /// Get element role (accessibilityRole).
    unsafe fn get_element_role(&self, element: *mut AnyObject) -> accesskit::Role {
        let role: *mut AnyObject = msg_send![element, accessibilityRole];
        let role_str = self.nsstring_to_string(role).unwrap_or_default();
        Self::map_role(&role_str)
    }

    /// Get element frame (accessibilityFrame).
    unsafe fn get_element_frame(&self, element: *mut AnyObject) -> Option<Rect> {
        let frame: CGRect = msg_send![element, accessibilityFrame];
        Some(Rect::new(
            Point::new(frame.origin.x, frame.origin.y),
            Size::new(frame.size.width, frame.size.height),
        ))
    }

    /// Get element enabled state.
    /// Note: AXPMacPlatformElement might not have accessibilityEnabled, so default to true
    unsafe fn get_element_enabled(&self, element: *mut AnyObject) -> bool {
        // Try isAccessibilityEnabled first, then accessibilityEnabled
        // If neither works, default to true
        let responds_to_enabled: Bool =
            msg_send![element, respondsToSelector: sel!(isAccessibilityEnabled)];
        if responds_to_enabled.as_bool() {
            let enabled: Bool = msg_send![element, isAccessibilityEnabled];
            return enabled.as_bool();
        }

        let responds_to_enabled2: Bool =
            msg_send![element, respondsToSelector: sel!(accessibilityEnabled)];
        if responds_to_enabled2.as_bool() {
            let enabled: Bool = msg_send![element, accessibilityEnabled];
            return enabled.as_bool();
        }

        // Default to enabled if no method available
        true
    }

    /// Get whether an element currently has focus.
    unsafe fn get_element_focused(&self, element: *mut AnyObject) -> bool {
        // The translated AX element exposes focus via either `isAccessibilityFocused`
        // (UIKit-style) or `accessibilityFocused` (older AppKit-style). If neither
        // responds, assume not focused.
        let responds_to_focused: Bool =
            msg_send![element, respondsToSelector: sel!(isAccessibilityFocused)];
        if responds_to_focused.as_bool() {
            let focused: Bool = msg_send![element, isAccessibilityFocused];
            return focused.as_bool();
        }

        let responds_to_focused2: Bool =
            msg_send![element, respondsToSelector: sel!(accessibilityFocused)];
        if responds_to_focused2.as_bool() {
            let focused: Bool = msg_send![element, accessibilityFocused];
            return focused.as_bool();
        }

        false
    }

    /// Get element action names.
    unsafe fn get_element_actions(&self, element: *mut AnyObject) -> Vec<String> {
        let actions: *mut AnyObject = msg_send![element, accessibilityActionNames];
        if actions.is_null() {
            return Vec::new();
        }

        let count: usize = msg_send![actions, count];
        let mut result = Vec::with_capacity(count);

        for i in 0..count {
            let action: *mut AnyObject = msg_send![actions, objectAtIndex: i];
            if let Some(action_str) = self.nsstring_to_string(action) {
                result.push(action_str);
            }
        }

        result
    }

    /// Convert NSString to Rust String.
    unsafe fn nsstring_to_string(&self, ns_string: *mut AnyObject) -> Option<String> {
        if ns_string.is_null() {
            return None;
        }

        let cstr: *const c_char = msg_send![ns_string, UTF8String];
        if cstr.is_null() {
            return None;
        }

        Some(CStr::from_ptr(cstr).to_string_lossy().to_string())
    }

    fn map_role(role: &str) -> accesskit::Role {
        roles::map_ax_role_ios(role)
    }

    /// Check if element is interactive based on role and actions.
    fn is_interactive(role: &accesskit::Role, actions: &[String]) -> bool {
        // Interactive by role
        let interactive_roles = [
            accesskit::Role::Button,
            accesskit::Role::Link,
            accesskit::Role::TextInput,
            accesskit::Role::MultilineTextInput,
            accesskit::Role::CheckBox,
            accesskit::Role::RadioButton,
            accesskit::Role::ComboBox,
            accesskit::Role::Slider,
            accesskit::Role::Switch,
            accesskit::Role::Tab,
            accesskit::Role::MenuItem,
        ];

        if interactive_roles.contains(role) {
            return true;
        }

        // Interactive by actions
        actions.iter().any(|a| a == "AXPress" || a == "AXActivate")
    }

    /// Count total elements in tree.
    fn count_elements(&self, element: &Element) -> usize {
        1 + element
            .children
            .iter()
            .map(|c| self.count_elements(c))
            .sum::<usize>()
    }

    /// Clear the element cache and release retained element pointers.
    pub fn clear_cache(&mut self) {
        // Unregister the token from the dispatcher state
        if let Some(token) = self.current_token.take() {
            let mut state = get_dispatcher_state().lock().unwrap();
            state.unregister_device(&token);
        }

        // Release all retained element pointers
        for (_id, ptr) in self.element_ptrs.drain() {
            if !ptr.is_null() {
                unsafe { CFRelease(ptr as *const c_void) };
            }
        }
        self.cache.clear();
    }

    /// Perform an action on an element by ID.
    ///
    /// Supported actions:
    /// - `Action::Click` / `Action::Default` - Press the element (AXPress)
    /// - `Action::Focus` - Focus the element (AXActivate)
    /// - `Action::Blur` - Remove focus from the element
    /// - `Action::Increment` - Increment value (AXIncrement)
    /// - `Action::Decrement` - Decrement value (AXDecrement)
    pub fn perform_action(&mut self, id: ElementKey, action: accesskit::Action) -> Result<()> {
        // Look up the element pointer
        let element_ptr =
            self.element_ptrs.get(id).copied().ok_or_else(|| {
                anyhow!("Element {} not found in cache. Call get_tree() first.", id)
            })?;

        if element_ptr.is_null() {
            return Err(anyhow!("Element pointer is null"));
        }

        // Handle Blur specially - set focused state to false
        if action == accesskit::Action::Blur {
            return unsafe { self.perform_blur(element_ptr) };
        }

        // Map accesskit action to AX action name
        let action_name = match action {
            accesskit::Action::Click => "AXPress",
            accesskit::Action::Focus => "AXActivate",
            accesskit::Action::Increment => "AXIncrement",
            accesskit::Action::Decrement => "AXDecrement",
            accesskit::Action::ScrollLeft => "AXScrollLeft",
            accesskit::Action::ScrollRight => "AXScrollRight",
            accesskit::Action::ScrollUp => "AXScrollUp",
            accesskit::Action::ScrollDown => "AXScrollDown",
            accesskit::Action::Expand => "AXExpand",
            accesskit::Action::Collapse => "AXCollapse",
            _ => return Err(anyhow!("Action {:?} not supported", action)),
        };

        unsafe { self.perform_ax_action(element_ptr, action_name) }
    }

    /// Perform a named accessibility action on an element.
    unsafe fn perform_ax_action(&self, element: *mut AnyObject, action_name: &str) -> Result<()> {
        // Check if the element supports this action
        let actions = self.get_element_actions(element);
        if !actions.iter().any(|a| a == action_name) {
            return Err(anyhow!(
                "Element does not support action '{}'. Available actions: {:?}",
                action_name,
                actions
            ));
        }

        // For AXPress, use the specific accessibilityPerformPress method
        // which actually triggers the action in the iOS Simulator
        if action_name == "AXPress" {
            let result: Bool = msg_send![element, accessibilityPerformPress];
            if result.as_bool() {
                return Ok(());
            } else {
                return Err(anyhow!("accessibilityPerformPress returned false"));
            }
        }

        // For other actions, use accessibilityPerformAction:
        let action_ns = NSString::from_str(action_name);
        let _: () = msg_send![element, accessibilityPerformAction: &*action_ns];

        Ok(())
    }

    /// Perform blur (remove focus) on an element.
    ///
    /// iOS doesn't have a direct "blur" action, so we try to set the focused state to false.
    unsafe fn perform_blur(&self, element: *mut AnyObject) -> Result<()> {
        // Try setAccessibilityFocused: if available
        let responds: Bool = msg_send![element, respondsToSelector: sel!(setAccessibilityFocused:)];
        if responds.as_bool() {
            let _: () = msg_send![element, setAccessibilityFocused: Bool::NO];
            return Ok(());
        }

        // Try accessibilityPerformEscape which can dismiss focus
        let responds_escape: Bool =
            msg_send![element, respondsToSelector: sel!(accessibilityPerformEscape)];
        if responds_escape.as_bool() {
            let result: Bool = msg_send![element, accessibilityPerformEscape];
            if result.as_bool() {
                return Ok(());
            }
        }

        // If neither method is available, return an error
        Err(anyhow!(
            "Blur not supported on this element. iOS does not have a direct blur action."
        ))
    }

    /// Tap at screen coordinates.
    ///
    /// This finds the element at the given point and performs AXPress on it.
    pub fn tap(&mut self, x: f64, y: f64) -> Result<()> {
        // Need a current token for the translator
        let token = self
            .current_token
            .clone()
            .ok_or_else(|| anyhow!("No current token. Call get_tree() first."))?;

        unsafe { self.tap_at_point(x, y, &token) }
    }

    /// Tap at a point using the translator's objectAtPoint method.
    unsafe fn tap_at_point(&self, x: f64, y: f64, token: &str) -> Result<()> {
        let token_ns = NSString::from_str(token);

        // Create CGPoint
        let point = objc2_core_foundation::CGPoint { x, y };

        // Call objectAtPoint:displayId:bridgeDelegateToken:
        let translation: *mut AnyObject = msg_send![
            self.translator,
            objectAtPoint: point,
            displayId: 0u32,
            bridgeDelegateToken: &*token_ns
        ];

        if translation.is_null() {
            return Err(anyhow!("No element found at point ({}, {})", x, y));
        }

        // Set token on translation
        let _: () = msg_send![translation, setBridgeDelegateToken: &*token_ns];

        // Convert to platform element
        let element: *mut AnyObject = msg_send![
            self.translator,
            macPlatformElementFromTranslation: translation
        ];

        if element.is_null() {
            return Err(anyhow!(
                "Failed to get platform element at point ({}, {})",
                x,
                y
            ));
        }

        // Perform press action
        self.perform_ax_action(element, "AXPress")
    }

    /// Get element at screen coordinates.
    ///
    /// Returns the element at the given point, or None if no element is found.
    pub fn element_at_point(&mut self, x: f64, y: f64) -> Result<Option<Element>> {
        // Need a current token for the translator
        let token = self
            .current_token
            .clone()
            .ok_or_else(|| anyhow!("No current token. Call get_tree() first."))?;

        unsafe { self.get_element_at_point(x, y, &token) }
    }

    /// Get element at a point using the translator's objectAtPoint method.
    unsafe fn get_element_at_point(
        &mut self,
        x: f64,
        y: f64,
        token: &str,
    ) -> Result<Option<Element>> {
        let token_ns = NSString::from_str(token);

        // Create CGPoint
        let point = objc2_core_foundation::CGPoint { x, y };

        // Call objectAtPoint:displayId:bridgeDelegateToken:
        let translation: *mut AnyObject = msg_send![
            self.translator,
            objectAtPoint: point,
            displayId: 0u32,
            bridgeDelegateToken: &*token_ns
        ];

        if translation.is_null() {
            return Ok(None);
        }

        // Set token on translation
        let _: () = msg_send![translation, setBridgeDelegateToken: &*token_ns];

        // Convert to platform element
        let element: *mut AnyObject = msg_send![
            self.translator,
            macPlatformElementFromTranslation: translation
        ];

        if element.is_null() {
            return Ok(None);
        }

        // Build element (as a leaf - no children)
        let filter = TreeFilter {
            max_depth: Some(0),
            max_elements: Some(1),
            interactive_only: false,
            visible_only: false,
            within_bounds: None,
            roles: None,
        };
        let elem = self.build_element_tree(element, token, &filter, 0)?;
        Ok(Some(elem))
    }

    /// Perform a press action on an element by ID.
    ///
    /// Convenience method equivalent to `perform_action(id, Action::Click)`.
    pub fn press(&mut self, id: ElementKey) -> Result<()> {
        self.perform_action(id, accesskit::Action::Click)
    }

    /// Set text value on a text field element.
    ///
    /// This uses AXSetValue to set the accessibility value.
    pub fn set_value(&mut self, id: ElementKey, value: &str) -> Result<()> {
        let element_ptr =
            self.element_ptrs.get(id).copied().ok_or_else(|| {
                anyhow!("Element {} not found in cache. Call get_tree() first.", id)
            })?;

        if element_ptr.is_null() {
            return Err(anyhow!("Element pointer is null"));
        }

        unsafe {
            let value_ns = NSString::from_str(value);

            // Check if element responds to setAccessibilityValue:
            let responds: Bool =
                msg_send![element_ptr, respondsToSelector: sel!(setAccessibilityValue:)];
            if !responds.as_bool() {
                return Err(anyhow!("Element does not support setting value"));
            }

            let _: () = msg_send![element_ptr, setAccessibilityValue: &*value_ns];
            Ok(())
        }
    }

    // HID Injection Methods (Indigo Protocol)

    /// Get or create the HID client for direct input injection.
    fn get_hid(&mut self) -> Result<&SimulatorHID> {
        if self.hid.is_none() {
            self.hid = Some(SimulatorHID::new(self.device)?);
        }
        Ok(self.hid.as_ref().unwrap())
    }

    /// Get the screen size in points.
    pub fn screen_size(&mut self) -> Result<(f64, f64)> {
        Ok(self.get_hid()?.screen_size())
    }

    /// Tap at screen coordinates using HID injection.
    ///
    /// Unlike `tap()` which uses accessibility APIs, this sends actual touch
    /// events to the simulator's HID subsystem. This works on any screen
    /// coordinate, not just accessibility elements.
    ///
    /// # Arguments
    /// * `x` - X coordinate in points
    /// * `y` - Y coordinate in points
    pub fn hid_tap(&mut self, x: f64, y: f64) -> Result<()> {
        // Create HID if needed, then tap
        if self.hid.is_none() {
            self.hid = Some(SimulatorHID::new(self.device)?);
        }
        self.hid.as_ref().unwrap().tap(x, y)
    }

    /// Perform a swipe gesture using HID injection.
    ///
    /// # Arguments
    /// * `start` - Starting coordinates (x, y) in points
    /// * `end` - Ending coordinates (x, y) in points
    /// * `duration_ms` - Duration of the swipe in milliseconds
    pub fn hid_swipe(
        &mut self,
        start: (f64, f64),
        end: (f64, f64),
        duration_ms: u64,
    ) -> Result<()> {
        if self.hid.is_none() {
            self.hid = Some(SimulatorHID::new(self.device)?);
        }
        self.hid.as_ref().unwrap().swipe(start, end, duration_ms)
    }

    /// Press a hardware button using HID injection.
    ///
    /// # Arguments
    /// * `button` - Which button to press
    /// * `hold_ms` - How long to hold the button (0 for quick tap)
    pub fn hid_button(&mut self, button: HardwareButton, hold_ms: u64) -> Result<()> {
        if self.hid.is_none() {
            self.hid = Some(SimulatorHID::new(self.device)?);
        }
        self.hid.as_ref().unwrap().press_button(button, hold_ms)
    }

    /// Send a keyboard key press using HID injection.
    ///
    /// # Arguments
    /// * `key_code` - The key code (from HIToolbox/Events.h)
    ///
    /// Common key codes:
    /// - 0x00: A, 0x01: S, 0x02: D, ... (letters)
    /// - 0x24: Return, 0x33: Delete, 0x35: Escape
    /// - 0x7B: Left Arrow, 0x7C: Right Arrow, 0x7D: Down Arrow, 0x7E: Up Arrow
    pub fn hid_key(&mut self, key_code: u32) -> Result<()> {
        if self.hid.is_none() {
            self.hid = Some(SimulatorHID::new(self.device)?);
        }
        self.hid.as_ref().unwrap().send_key(key_code)
    }

    /// Capture a screenshot of the entire simulator screen.
    ///
    /// Uses `xcrun simctl io` to capture the screenshot as PNG.
    pub fn capture_screen(&self) -> Result<Screenshot> {
        use std::io::Read;

        // Create a temporary file for the screenshot
        let temp_dir = std::env::temp_dir();
        let screenshot_path = temp_dir.join(format!(
            "accessibility_cli_screenshot_{}.png",
            std::process::id()
        ));

        // Run xcrun simctl io <udid> screenshot <path>
        let output = std::process::Command::new("xcrun")
            .args([
                "simctl",
                "io",
                &self.device_udid,
                "screenshot",
                "--type=png",
                screenshot_path.to_str().unwrap(),
            ])
            .output()
            .map_err(|e| anyhow!("Failed to execute xcrun simctl: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Clean up temp file if it exists
            let _ = std::fs::remove_file(&screenshot_path);
            return Err(anyhow!("Screenshot capture failed: {}", stderr.trim()));
        }

        // Read the PNG file
        let mut file = std::fs::File::open(&screenshot_path)
            .map_err(|e| anyhow!("Failed to open screenshot file: {}", e))?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|e| anyhow!("Failed to read screenshot file: {}", e))?;

        // Clean up temp file
        let _ = std::fs::remove_file(&screenshot_path);

        // Decode PNG to get dimensions
        let (width, height) = {
            use image::ImageReader;
            use std::io::Cursor;
            let img = ImageReader::new(Cursor::new(&data))
                .with_guessed_format()?
                .decode()
                .map_err(|e| anyhow!("Failed to decode screenshot: {}", e))?;
            (img.width(), img.height())
        };

        Ok(Screenshot {
            data,
            width,
            height,
        })
    }

    /// Get the screen bounds for the simulator.
    ///
    /// Returns the app bounds in macOS screen coordinates.
    /// This is needed for converting accessibility coordinates to device-local
    /// coordinates for screenshot cropping.
    pub fn get_screen_bounds(&self) -> Result<Rect> {
        self.app_bounds
            .ok_or_else(|| anyhow!("App bounds not available. Call get_tree() first."))
    }

    /// Capture a screenshot of a specific element.
    ///
    /// This captures the full screen and crops to the element's bounds.
    pub fn capture_element(&mut self, id: ElementKey) -> Result<Screenshot> {
        // Get element bounds from cache
        let element_ptr =
            self.element_ptrs.get(id).copied().ok_or_else(|| {
                anyhow!("Element {} not found in cache. Call get_tree() first.", id)
            })?;

        if element_ptr.is_null() {
            return Err(anyhow!("Element pointer is null"));
        }

        let bounds = unsafe { self.get_element_frame(element_ptr) }
            .ok_or_else(|| anyhow!("Element has no bounds"))?;

        // Capture full screen
        let screenshot = self.capture_screen()?;

        // Get screen bounds for coordinate conversion
        let screen_bounds = self.get_screen_bounds()?;

        // Crop to element bounds
        screenshot.crop(&bounds, &screen_bounds)
    }
}

use crate::accessibility::AccessibilityReader;
use accesskit::Action;

impl AccessibilityReader for IOSSimulatorAccessibility {
    fn get_tree(
        &mut self,
        _pid: Option<u32>,
        filter: &TreeFilter,
    ) -> impl std::future::Future<Output = Result<ElementTree>> {
        // iOS always queries the frontmost app, ignoring the PID parameter
        let result = IOSSimulatorAccessibility::get_tree(self, filter);
        std::future::ready(result)
    }

    fn get_element(&self, _id: ElementKey) -> Option<&Element> {
        // iOS uses element_ptrs HashMap instead of caching elements
        // The cache is currently not populated with Element references
        None
    }

    fn perform_action(
        &mut self,
        id: ElementKey,
        action: Action,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result = IOSSimulatorAccessibility::perform_action(self, id, action);
        std::future::ready(result)
    }

    fn set_value(
        &mut self,
        id: ElementKey,
        value: &str,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result = IOSSimulatorAccessibility::set_value(self, id, value);
        std::future::ready(result)
    }

    fn hit_test(
        &mut self,
        x: f64,
        y: f64,
    ) -> impl std::future::Future<Output = Result<Option<ElementKey>>> {
        let result = match self.element_at_point(x, y) {
            Ok(Some(elem)) => Ok(Some(elem.id)),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        };
        std::future::ready(result)
    }

    fn clear_cache(&mut self) {
        IOSSimulatorAccessibility::clear_cache(self)
    }

    fn snapshot_version(&self) -> u64 {
        self.cache.version()
    }

    // Platform adapter methods

    fn capture_screen(&self, _pid: Option<u32>) -> Result<Screenshot> {
        // iOS simulator doesn't use PID - it always captures the current simulator
        IOSSimulatorAccessibility::capture_screen(self)
    }

    fn get_screen_bounds(
        &self,
        _pid: Option<u32>,
    ) -> impl std::future::Future<Output = Result<Rect>> {
        // iOS simulator doesn't use PID - it always returns simulator bounds
        let result = IOSSimulatorAccessibility::get_screen_bounds(self);
        std::future::ready(result)
    }

    fn platform_name(&self) -> &'static str {
        "iOS"
    }

    fn supports_hit_test(&self) -> bool {
        true
    }
}

use super::macos::IOSAdapter;

impl IOSAdapter for IOSSimulatorAccessibility {
    fn hid_tap(&mut self, x: f64, y: f64) -> Result<()> {
        IOSSimulatorAccessibility::hid_tap(self, x, y)
    }

    fn hid_swipe(&mut self, start: (f64, f64), end: (f64, f64), duration_ms: u64) -> Result<()> {
        IOSSimulatorAccessibility::hid_swipe(self, start, end, duration_ms)
    }

    fn hid_button(&mut self, button: HardwareButton, hold_ms: u64) -> Result<()> {
        IOSSimulatorAccessibility::hid_button(self, button, hold_ms)
    }

    fn tap(&mut self, x: f64, y: f64) -> Result<()> {
        IOSSimulatorAccessibility::tap(self, x, y)
    }

    fn press(&mut self, id: ElementKey) -> Result<()> {
        IOSSimulatorAccessibility::press(self, id)
    }

    fn device_udid(&self) -> &str {
        IOSSimulatorAccessibility::device_udid(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_mapping() {
        assert_eq!(
            IOSSimulatorAccessibility::map_role("AXButton"),
            accesskit::Role::Button
        );
        assert_eq!(
            IOSSimulatorAccessibility::map_role("AXTextField"),
            accesskit::Role::TextInput
        );
        assert_eq!(
            IOSSimulatorAccessibility::map_role("Button"),
            accesskit::Role::Button
        );
        assert_eq!(
            IOSSimulatorAccessibility::map_role("Unknown"),
            accesskit::Role::Unknown
        );
    }

    #[test]
    fn test_is_interactive() {
        assert!(IOSSimulatorAccessibility::is_interactive(
            &accesskit::Role::Button,
            &[]
        ));
        assert!(IOSSimulatorAccessibility::is_interactive(
            &accesskit::Role::Unknown,
            &["AXPress".to_string()]
        ));
        assert!(!IOSSimulatorAccessibility::is_interactive(
            &accesskit::Role::Label,
            &[]
        ));
    }
}
