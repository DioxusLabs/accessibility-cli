use super::common::{
    BUTTON_EVENT_TARGET_HARDWARE, ButtonDirection, HardwareButton,
    create_touch_message_from_template, nsstring_to_string_static,
};
use super::dispatcher::{
    DISPATCH_TIME_FOREVER, dispatch_group_create, dispatch_group_enter, dispatch_group_leave,
    dispatch_group_wait, dispatch_queue_create,
};
use super::*;

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
pub(super) struct SimulatorHID {
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
    pub(super) fn new(device: *mut AnyObject) -> Result<Self> {
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
