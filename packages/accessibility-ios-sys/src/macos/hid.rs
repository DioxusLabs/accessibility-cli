use std::mem::offset_of;
use std::sync::mpsc;

use objc2_core_foundation::CGPoint;

use super::common::{
    BUTTON_EVENT_TARGET_HARDWARE, ButtonDirection, HardwareButton, nsstring_to_string_static,
};
use super::dispatcher::dispatch_queue_create;
use super::dynamic::send_hid_message;
use super::indigo::{
    BuilderMessage, FingerState, MachMessageHeader, MultiTouchMessage, SingleTouchMessage,
};
use super::*;

/// Function pointer types for Indigo message creation (loaded from SimulatorKit via dlsym).
type IndigoMessageForButtonFn =
    unsafe extern "C" fn(source: i32, action: i32, target: i32) -> *mut c_void;
type IndigoMessageForHIDArbitraryFn =
    unsafe extern "C" fn(target: u32, page: u32, usage: u32, action: u32) -> *mut c_void;
/// `IndigoHIDMessageForMouseNSEvent(CGPoint*, CGPoint*, IndigoHIDTarget,
///  NSEventType, NSSize, IndigoHIDEdge)`
///
/// On arm64 the integer and floating-point arguments are numbered
/// independently, so the pointers, target, event type and edge land in x0-x4
/// while the `NSSize` occupies d0/d1. Declaring the size last therefore still
/// produces the correct register assignment.
///
/// Apple's Simulator.app always passes `NSSize(1.0, 1.0)`, which makes the
/// ratio computation inside the function reduce to the point itself.
type IndigoMessageForTouchFn = unsafe extern "C" fn(
    point0: *const CGPoint,
    point1: *const CGPoint,
    target: i32,
    event_type: i32,
    edge: u32,
    size_width: f64,
    size_height: f64,
) -> *mut c_void;
type IndigoMessageForKeyboardFn = unsafe extern "C" fn(key_code: i32, action: i32) -> *mut c_void;

/// `IndigoHIDTarget` for touches and arbitrary HID buttons.
const TOUCH_TARGET: i32 = 0x32;

/// Indigo has no distinct "move" phase; contact is maintained by repeating the
/// down event at the new position.
const TOUCH_EVENT_DOWN: i32 = 1;
const TOUCH_EVENT_UP: i32 = 2;

/// Resolve a function in a `dlopen`ed framework.
///
/// # Safety
/// `F` must be a function pointer type matching the symbol's real signature.
unsafe fn symbol<F: Copy>(handle: *mut c_void, name: &CStr) -> Result<F> {
    const { assert!(size_of::<F>() == size_of::<*mut c_void>()) };
    let sym = unsafe { libc::dlsym(handle, name.as_ptr()) };
    if sym.is_null() {
        return Err(anyhow!("Failed to find {}", name.to_string_lossy()));
    }
    Ok(unsafe { std::mem::transmute_copy(&sym) })
}

/// HID injection client for iOS Simulator.
///
/// Uses the Indigo protocol via SimulatorKit's SimDeviceLegacyHIDClient
/// to inject touch events, button presses, and keyboard input directly
/// into the simulator's HID subsystem.
/// Screen edge a touch is treated as originating from.
///
/// iOS only recognizes system gestures — most importantly swipe-up-to-home on
/// Face ID devices — when the touch is flagged with the edge it started from.
/// Without this a drag from the bottom is just an in-app drag.
///
/// These are edges of the *raw framebuffer*, which never rotates, so callers
/// working in display space have to map through the current orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum TouchEdge {
    None = 0,
    Left = 1,
    Top = 2,
    Bottom = 3,
    Right = 4,
}

/// How many fingers every [`SimulatorHID::touch_normalized`] event carries:
/// the most an Indigo touch message has room for.
pub const FINGERS: usize = 2;

/// One finger of a [`SimulatorHID::touch_normalized`] event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchContact {
    /// 0..1 fraction of the raw framebuffer width.
    pub x: f64,

    /// 0..1 fraction of the raw framebuffer height.
    pub y: f64,

    /// Whether the finger is on the screen. `false` lifts it.
    pub touching: bool,

    /// The edge the finger's gesture started from. It must stay the same for
    /// every event of the gesture.
    pub edge: TouchEdge,
}

/// Device orientation, using the GSEvent numbering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Orientation {
    Portrait = 1,
    PortraitUpsideDown = 2,
    LandscapeRight = 3,
    LandscapeLeft = 4,
}

/// Direct simulator HID client.
///
/// Sends synchronously wait for a dispatch-queue round trip. Composite taps,
/// swipes, buttons, and key presses also sleep to preserve event timing, so
/// callers should keep this client off async runtime threads.
pub struct SimulatorHID {
    client: *mut AnyObject, // SimDeviceLegacyHIDClient
    device: *mut AnyObject, // SimDevice, retained for GSEvent port lookup
    queue: *mut AnyObject,  // dispatch_queue_t
    screen_size: (f64, f64),
    screen_scale: f64,
    // Function pointers for message creation
    msg_for_button: IndigoMessageForButtonFn,
    msg_for_hid_arbitrary: Option<IndigoMessageForHIDArbitraryFn>,
    msg_for_touch: IndigoMessageForTouchFn,
    msg_for_keyboard: IndigoMessageForKeyboardFn,
}

unsafe impl Send for SimulatorHID {}

impl SimulatorHID {
    /// Create a new HID client for a simulator device.
    ///
    /// # Arguments
    /// * `device` - A SimDevice pointer (from CoreSimulator)
    pub(super) fn new(device: *mut AnyObject) -> Result<Self> {
        // Load SimulatorKit and get function pointers
        let handle = load_simulatorkit_framework()?;

        // SAFETY: the signatures are documented on the function pointer types.
        let (msg_for_button, msg_for_hid_arbitrary, msg_for_touch, msg_for_keyboard) = unsafe {
            (
                symbol::<IndigoMessageForButtonFn>(handle, c"IndigoHIDMessageForButton")?,
                symbol::<IndigoMessageForHIDArbitraryFn>(
                    handle,
                    c"IndigoHIDMessageForHIDArbitrary",
                )
                .ok(),
                symbol::<IndigoMessageForTouchFn>(handle, c"IndigoHIDMessageForMouseNSEvent")?,
                symbol::<IndigoMessageForKeyboardFn>(
                    handle,
                    c"IndigoHIDMessageForKeyboardArbitrary",
                )?,
            )
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
            device,
            queue,
            screen_size,
            screen_scale,
            msg_for_button,
            msg_for_hid_arbitrary,
            msg_for_touch,
            msg_for_keyboard,
        })
    }

    /// Create a HID client for a booted device, resolving it by UDID.
    ///
    /// `None` picks the first booted simulator. This exists so an input path
    /// can be opened independently of the accessibility reader, which keeps
    /// pointer events from queueing behind slow AX tree fetches.
    pub fn for_device(udid: Option<&str>) -> Result<Self> {
        crate::frameworks::load_coresimulator_framework()?;
        let device = unsafe { super::common::find_booted_device(udid)? };
        Self::new(device)
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

    /// Send one interactive touch event in normalized screen space.
    ///
    /// Unlike [`Self::tap`] and [`Self::swipe`], this does not synthesize a
    /// whole gesture: the caller drives the phases itself, which is what a live
    /// pointer stream from a browser needs.
    ///
    /// Every event carries all [`FINGERS`] fingers, in the message Simulator.app
    /// sends for its Option-drag pinch. Fingers keep their identity by index: a
    /// finger that stays `touching` across events is one moving touch, and a
    /// finger lifts by being sent with `touching: false`. A gesture ends once
    /// every finger has lifted, so a one-finger gesture sends the other fingers
    /// lifted throughout.
    ///
    /// `x` and `y` are 0..1 fractions of the screen, matching what the web UI
    /// already computes, so no point/pixel/scale conversion is involved.
    pub fn touch_normalized(&self, fingers: [TouchContact; FINGERS]) -> Result<()> {
        let fingers = fingers.map(|finger| TouchContact {
            x: finger.x.clamp(0.0, 1.0),
            y: finger.y.clamp(0.0, 1.0),
            ..finger
        });

        // Only the first finger picks up the edge passed to the builder, so
        // each finger's flags come from a single-finger message instead.
        let mut states = [FingerState::default(); FINGERS];
        for (state, finger) in states.iter_mut().zip(fingers) {
            *state = FingerState {
                x_ratio: finger.x,
                y_ratio: finger.y,
                touching: finger.touching,
                edge_flags: self.touch_edge_flags(finger)?,
            };
        }

        let event_type = if fingers.iter().any(|finger| finger.touching) {
            TOUCH_EVENT_DOWN
        } else {
            TOUCH_EVENT_UP
        };
        let [first, second] = fingers.map(|finger| CGPoint {
            x: finger.x,
            y: finger.y,
        });
        let mut message: MultiTouchMessage = self
            .build_touch(&first, Some(&second), event_type, fingers[0].edge)?
            .into_multi_touch()
            .ok_or_else(|| anyhow!("Unexpected touch message layout"))?;
        message.set_fingers(states);
        self.send(&message)
    }

    /// Rotate the device.
    ///
    /// Orientation does not travel over Indigo like touches do. It is a
    /// GSEvent delivered by mach message to the simulator's
    /// `PurpleWorkspacePort`, which is the same path Simulator.app itself uses
    /// when you pick Device > Rotate.
    pub fn set_orientation(&self, orientation: Orientation) -> Result<()> {
        // GSEvent constants, as used by Simulator.app and idb.
        const GSEVENT_MACH_MESSAGE_ID: i32 = 0x7B;
        const GSEVENT_TYPE_ORIENTATION_CHANGED: u32 = 50;
        const GSEVENT_HOST_FLAG: u32 = 0x0002_0000;
        const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
        /// `align4(4 + 0x6B)` — a GSEvent header plus a 4-byte payload.
        const MESSAGE_SIZE: u32 = 108;

        /// A GSEvent mach message carrying a 4-byte orientation record, in a
        /// buffer padded past `MESSAGE_SIZE`.
        #[repr(C, packed(4))]
        struct OrientationEvent {
            header: MachMessageHeader,
            event_type: u32,
            _unknown: [u8; 0x2c],
            record_info_size: u32,
            orientation: u32,
            _tail: [u8; 0x20],
        }
        const _: () = {
            assert!(size_of::<OrientationEvent>() == 112);
            assert!(offset_of!(OrientationEvent, event_type) == 0x18);
            assert!(offset_of!(OrientationEvent, record_info_size) == 0x48);
            assert!(offset_of!(OrientationEvent, orientation) == 0x4c);
        };

        unsafe extern "C" {
            fn mach_msg_send(message: *mut c_void) -> i32;
        }

        let port = self.purple_workspace_port()?;

        let mut event = OrientationEvent {
            header: MachMessageHeader {
                bits: MACH_MSG_TYPE_COPY_SEND,
                size: MESSAGE_SIZE,
                remote_port: port,
                local_port: 0,
                voucher_port: 0,
                id: GSEVENT_MACH_MESSAGE_ID,
            },
            event_type: GSEVENT_TYPE_ORIENTATION_CHANGED | GSEVENT_HOST_FLAG,
            _unknown: [0; 0x2c],
            record_info_size: 4,
            orientation: orientation as u32,
            _tail: [0; 0x20],
        };

        // SAFETY: `event` is a complete mach message of `MESSAGE_SIZE` bytes.
        let result = unsafe { mach_msg_send((&raw mut event).cast()) };
        if result != 0 {
            return Err(anyhow!("mach_msg_send for orientation failed: {result}"));
        }
        Ok(())
    }

    /// Look up the simulator's `PurpleWorkspacePort` mach port.
    fn purple_workspace_port(&self) -> Result<u32> {
        let name = NSString::from_str("PurpleWorkspacePort");
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let port: u32 = unsafe { msg_send![self.device, lookup: &*name, error: &mut error] };

        if port == 0 {
            let detail = unsafe {
                (!error.is_null())
                    .then(|| {
                        let description: *mut AnyObject = msg_send![error, localizedDescription];
                        nsstring_to_string_static(description)
                    })
                    .flatten()
            };
            // The port is published by Simulator.app, not by the runtime, so a
            // headless `simctl boot` will not have one.
            return Err(anyhow!(
                "PurpleWorkspacePort unavailable ({}). Rotation needs Simulator.app running.",
                detail.as_deref().unwrap_or("no error detail")
            ));
        }
        Ok(port)
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

    pub fn supports_hid_arbitrary(&self) -> bool {
        self.msg_for_hid_arbitrary.is_some()
    }

    pub fn press_hid_button(&self, page: u32, usage: u32, hold_ms: u64) -> Result<()> {
        self.send_hid_button(page, usage, ButtonDirection::Down)?;

        std::thread::sleep(std::time::Duration::from_millis(if hold_ms > 0 {
            hold_ms
        } else {
            50
        }));

        self.send_hid_button(page, usage, ButtonDirection::Up)
    }

    /// Send a keyboard key press.
    ///
    /// # Arguments
    /// * `key_code` - The key code (from HIToolbox/Events.h)
    pub fn send_key(&self, key_code: u32) -> Result<()> {
        self.send_key_with_modifiers(key_code, &[])
    }

    /// Send a key press with modifier keys held down around it.
    ///
    /// Modifiers are ordinary key events, not a bitmask: they are pressed in
    /// order, then the key is pressed and released, then they are released in
    /// reverse. This is the only way to produce capitals and shifted symbols —
    /// there is no shift flag on the Indigo message.
    ///
    /// `key_code` and `modifiers` are **USB HID usage codes** (page 0x07),
    /// not HIToolbox virtual keycodes. The ranges overlap and the meanings
    /// differ, so passing the wrong kind types different letters rather than
    /// failing. Left Shift is 225.
    pub fn send_key_with_modifiers(&self, key_code: u32, modifiers: &[u32]) -> Result<()> {
        for modifier in modifiers {
            self.send_keyboard(*modifier, ButtonDirection::Down)?;
        }

        self.send_keyboard(key_code, ButtonDirection::Down)?;
        std::thread::sleep(std::time::Duration::from_millis(12));
        self.send_keyboard(key_code, ButtonDirection::Up)?;

        for modifier in modifiers.iter().rev() {
            self.send_keyboard(*modifier, ButtonDirection::Up)?;
        }
        Ok(())
    }

    /// Send a touch event at the given ratio coordinates.
    fn send_touch(&self, x_ratio: f64, y_ratio: f64, direction: ButtonDirection) -> Result<()> {
        self.send_touch_edge(x_ratio, y_ratio, direction, TouchEdge::None)
    }

    fn send_touch_edge(
        &self,
        x_ratio: f64,
        y_ratio: f64,
        direction: ButtonDirection,
        edge: TouchEdge,
    ) -> Result<()> {
        let point = CGPoint {
            x: x_ratio,
            y: y_ratio,
        };
        let event_type = match direction {
            ButtonDirection::Down => TOUCH_EVENT_DOWN,
            ButtonDirection::Up => TOUCH_EVENT_UP,
        };

        // The builder's message is only a template; idb's duplicated-payload
        // layout is what actually gets sent.
        let template = self.build_touch(&point, None, event_type, edge)?;
        let message = SingleTouchMessage::from_template(&template, x_ratio, y_ratio, direction)
            .ok_or_else(|| anyhow!("Unexpected touch message layout"))?;
        self.send(&message)
    }

    fn touch_edge_flags(&self, contact: TouchContact) -> Result<u32> {
        let point = CGPoint {
            x: contact.x,
            y: contact.y,
        };
        let template = self.build_touch(&point, None, TOUCH_EVENT_UP, contact.edge)?;
        template
            .touch(1)
            .map(|touch| touch.edge_flags)
            .ok_or_else(|| anyhow!("Unexpected touch message layout"))
    }

    /// Call `IndigoHIDMessageForMouseNSEvent`. A second point selects the
    /// two-finger message layout.
    fn build_touch(
        &self,
        first: &CGPoint,
        second: Option<&CGPoint>,
        event_type: i32,
        edge: TouchEdge,
    ) -> Result<BuilderMessage> {
        let second = second.map_or(std::ptr::null(), |point| point as *const CGPoint);
        // SAFETY: both points outlive the call; the signature is documented on
        // `IndigoMessageForTouchFn`.
        let message = unsafe {
            (self.msg_for_touch)(
                first,
                second,
                TOUCH_TARGET,
                event_type,
                edge as u32,
                1.0,
                1.0,
            )
        };
        BuilderMessage::new(message).ok_or_else(|| anyhow!("Failed to create touch message"))
    }

    /// Send a button event.
    fn send_button(&self, button: HardwareButton, direction: ButtonDirection) -> Result<()> {
        // SAFETY: the signature is documented on `IndigoMessageForButtonFn`.
        let message = unsafe {
            (self.msg_for_button)(
                button as i32,
                direction as i32,
                BUTTON_EVENT_TARGET_HARDWARE as i32,
            )
        };
        let message = BuilderMessage::new(message)
            .ok_or_else(|| anyhow!("Failed to create button message"))?;
        self.send_raw(message.as_ptr())
    }

    fn send_hid_button(&self, page: u32, usage: u32, direction: ButtonDirection) -> Result<()> {
        let msg_for_hid_arbitrary = self
            .msg_for_hid_arbitrary
            .ok_or_else(|| anyhow!("IndigoHIDMessageForHIDArbitrary is unavailable"))?;
        // SAFETY: the signature is documented on `IndigoMessageForHIDArbitraryFn`.
        let message =
            unsafe { msg_for_hid_arbitrary(TOUCH_TARGET as u32, page, usage, direction as u32) };
        let message = BuilderMessage::new(message)
            .ok_or_else(|| anyhow!("Failed to create arbitrary HID message"))?;
        self.send_raw(message.as_ptr())
    }

    /// Send a keyboard event.
    fn send_keyboard(&self, key_code: u32, direction: ButtonDirection) -> Result<()> {
        // SAFETY: the signature is documented on `IndigoMessageForKeyboardFn`.
        let message = unsafe { (self.msg_for_keyboard)(key_code as i32, direction as i32) };
        let message = BuilderMessage::new(message)
            .ok_or_else(|| anyhow!("Failed to create keyboard message"))?;
        self.send_raw(message.as_ptr())
    }

    /// Send a message value to the HID client and wait for the outcome.
    fn send<T: Copy>(&self, message: &T) -> Result<()> {
        self.send_raw((message as *const T).cast())
    }

    /// Send an Indigo message to the HID client and wait for the outcome.
    ///
    /// The client does not take ownership: `message` is kept alive by the
    /// caller, which is safe because this returns only after the completion
    /// block has run or been released.
    fn send_raw(&self, message: *const c_void) -> Result<()> {
        let (sender, receiver) = mpsc::channel();
        let completion = RcBlock::new(move |error: *mut AnyObject| {
            let error = (!error.is_null())
                .then(|| {
                    // SAFETY: a non-null `error` is an `NSError`.
                    let description: *mut AnyObject =
                        unsafe { msg_send![error, localizedDescription] };
                    unsafe { nsstring_to_string_static(description) }
                })
                .flatten();
            let _ = sender.send(error);
        });

        // SAFETY: `client` and `queue` are live for `self`'s lifetime, and
        // `message` and `completion` outlive the wait below.
        unsafe { send_hid_message(self.client, message, self.queue, &*completion) };

        match receiver.recv() {
            Ok(None) => Ok(()),
            Ok(Some(error)) => Err(anyhow!("HID send failed: {error}")),
            Err(_) => Err(anyhow!(
                "HID client released its completion without running it"
            )),
        }
    }
}

impl Drop for SimulatorHID {
    fn drop(&mut self) {
        // Client and queue will be released by ARC when they go out of scope
        // No explicit cleanup needed
    }
}
