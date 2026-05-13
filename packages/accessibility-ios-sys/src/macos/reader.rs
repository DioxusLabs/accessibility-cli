use super::common::{ElementCache, find_booted_device, get_translator, map_ax_role_ios};
use super::dispatcher::{
    CFRetain, ensure_dispatcher_registered, generate_token, get_dispatcher_state,
};
use super::hid::SimulatorHID;
use super::*;

mod actions;

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
    unsafe fn get_element_role(&self, element: *mut AnyObject) -> Role {
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

    pub fn map_role(role: &str) -> Role {
        map_ax_role_ios(role)
    }

    /// Check if element is interactive based on role and actions.
    pub fn is_interactive(role: &Role, actions: &[String]) -> bool {
        // Interactive by role
        let interactive_roles = [
            Role::Button,
            Role::Link,
            Role::TextInput,
            Role::MultilineTextInput,
            Role::CheckBox,
            Role::RadioButton,
            Role::ComboBox,
            Role::Slider,
            Role::Switch,
            Role::Tab,
            Role::MenuItem,
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
}
