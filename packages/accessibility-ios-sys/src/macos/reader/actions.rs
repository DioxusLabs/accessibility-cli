use super::*;
use crate::macos::dispatcher::{CFRelease, get_dispatcher_state};

impl IOSSimulatorAccessibility {
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

    pub fn get_element(&self, id: ElementKey) -> Option<&Element> {
        self.cache.get(id)
    }

    pub fn snapshot_version(&self) -> u64 {
        self.cache.version()
    }

    /// Perform an action on an element by ID.
    ///
    /// Supported actions:
    /// - `Action::Click` / `Action::Default` - Press the element (AXPress)
    /// - `Action::Focus` - Focus the element (AXActivate)
    /// - `Action::Blur` - Remove focus from the element
    /// - `Action::Increment` - Increment value (AXIncrement)
    /// - `Action::Decrement` - Decrement value (AXDecrement)
    pub fn perform_action(&mut self, id: ElementKey, action: Action) -> Result<()> {
        // Look up the element pointer
        let element_ptr =
            self.element_ptrs.get(id).copied().ok_or_else(|| {
                anyhow!("Element {} not found in cache. Call get_tree() first.", id)
            })?;

        if element_ptr.is_null() {
            return Err(anyhow!("Element pointer is null"));
        }

        // Handle Blur specially - set focused state to false
        if action == Action::Blur {
            return unsafe { self.perform_blur(element_ptr) };
        }

        // Map accesskit action to AX action name
        let action_name = match action {
            Action::Click => "AXPress",
            Action::Focus => "AXActivate",
            Action::Increment => "AXIncrement",
            Action::Decrement => "AXDecrement",
            Action::ScrollLeft => "AXScrollLeft",
            Action::ScrollRight => "AXScrollRight",
            Action::ScrollUp => "AXScrollUp",
            Action::ScrollDown => "AXScrollDown",
            Action::Expand => "AXExpand",
            Action::Collapse => "AXCollapse",
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

        // The platform element carries its own translation, which is not
        // necessarily the one we just tokenized. Without the token on *that*
        // object every attribute read is routed through a delegate that
        // cannot reach the device, so the element comes back with an empty
        // label and a zero frame rather than failing outright.
        let element_translation: *mut AnyObject = msg_send![element, translation];
        if !element_translation.is_null() {
            let _: () = msg_send![element_translation, setBridgeDelegateToken: &*token_ns];
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
        self.perform_action(id, Action::Click)
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
