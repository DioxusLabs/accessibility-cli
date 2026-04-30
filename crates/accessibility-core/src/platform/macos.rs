//! macOS accessibility implementation using AXUIElement API.
//!
//! This module provides access to the macOS accessibility tree for reading
//! UI element information and performing actions.

// Rust 2024 requires unsafe blocks inside unsafe fns, but objc2 code uses many unsafe calls
#![allow(unsafe_op_in_unsafe_fn, dead_code)]

use crate::accessibility::{
    AccessibilityReader, Element, ElementCache, ElementKey, ElementTree, Point, Rect, TreeFilter,
};
use accesskit::{Action, Role};
use anyhow::{Result, anyhow, bail};
use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{CFArray, CFRetained, CFString, CFType};
use std::collections::HashMap;
use std::ptr::NonNull;

// AX Attribute constants
const AX_ROLE: &str = "AXRole";
const AX_TITLE: &str = "AXTitle";
const AX_DESCRIPTION: &str = "AXDescription";
const AX_VALUE: &str = "AXValue";
const AX_ENABLED: &str = "AXEnabled";
const AX_FOCUSED: &str = "AXFocused";
const AX_POSITION: &str = "AXPosition";
const AX_SIZE: &str = "AXSize";
const AX_CHILDREN: &str = "AXChildren";
const AX_PARENT: &str = "AXParent";
const AX_FOCUSED_UI_ELEMENT: &str = "AXFocusedUIElement";
const AX_FOCUSED_APPLICATION: &str = "AXFocusedApplication";
const AX_WINDOWS: &str = "AXWindows";
const AX_MAIN_WINDOW: &str = "AXMainWindow";

// AX Action constants
const AX_PRESS: &str = "AXPress";
const AX_SHOW_MENU: &str = "AXShowMenu";
const AX_RAISE: &str = "AXRaise";
const AX_CONFIRM: &str = "AXConfirm";
const AX_CANCEL: &str = "AXCancel";
const AX_INCREMENT: &str = "AXIncrement";
const AX_DECREMENT: &str = "AXDecrement";

// AX Role constants
const ROLE_BUTTON: &str = "AXButton";
const ROLE_TEXT_FIELD: &str = "AXTextField";
const ROLE_TEXT_AREA: &str = "AXTextArea";
const ROLE_STATIC_TEXT: &str = "AXStaticText";
const ROLE_CHECKBOX: &str = "AXCheckBox";
const ROLE_RADIO_BUTTON: &str = "AXRadioButton";
const ROLE_POPUP_BUTTON: &str = "AXPopUpButton";
const ROLE_COMBO_BOX: &str = "AXComboBox";
const ROLE_SLIDER: &str = "AXSlider";
const ROLE_TABLE: &str = "AXTable";
const ROLE_LIST: &str = "AXList";
const ROLE_OUTLINE: &str = "AXOutline";
const ROLE_WINDOW: &str = "AXWindow";
const ROLE_SHEET: &str = "AXSheet";
const ROLE_MENU: &str = "AXMenu";
const ROLE_MENU_ITEM: &str = "AXMenuItem";
const ROLE_MENU_BAR: &str = "AXMenuBar";
const ROLE_MENU_BAR_ITEM: &str = "AXMenuBarItem";
const ROLE_WEB_AREA: &str = "AXWebArea";
const ROLE_GROUP: &str = "AXGroup";
const ROLE_IMAGE: &str = "AXImage";
const ROLE_LINK: &str = "AXLink";
const ROLE_APPLICATION: &str = "AXApplication";
const ROLE_SCROLL_AREA: &str = "AXScrollArea";
const ROLE_TOOLBAR: &str = "AXToolbar";
const ROLE_TAB_GROUP: &str = "AXTabGroup";
const ROLE_TAB: &str = "AXTab";
const ROLE_PROGRESS_INDICATOR: &str = "AXProgressIndicator";
const ROLE_SPLIT_GROUP: &str = "AXSplitGroup";
const ROLE_SPLITTER: &str = "AXSplitter";
const ROLE_ROW: &str = "AXRow";
const ROLE_COLUMN: &str = "AXColumn";
const ROLE_CELL: &str = "AXCell";

/// macOS accessibility reader using AXUIElement API.
pub struct MacOSAccessibility {
    /// Cache of elements with their platform handles.
    cache: ElementCache,

    /// Map from ElementKey to AXUIElement handle for performing actions.
    handles: HashMap<ElementKey, CFRetained<AXUIElement>>,

    /// System-wide accessibility element (for hit testing and focus queries).
    system_wide: CFRetained<AXUIElement>,
}

impl MacOSAccessibility {
    /// Create a new macOS accessibility reader.
    ///
    /// Returns an error if accessibility permissions are not granted.
    pub fn new() -> Result<Self> {
        // Check accessibility permissions
        if !Self::is_process_trusted() {
            bail!(
                "Accessibility permissions not granted. \
                 Please enable in System Preferences > Privacy & Security > Accessibility"
            );
        }

        // Safety: AXUIElement::new_system_wide creates a valid system-wide element
        let system_wide = unsafe { AXUIElement::new_system_wide() };

        Ok(Self {
            cache: ElementCache::new(),
            handles: HashMap::new(),
            system_wide,
        })
    }

    /// Check if the process has accessibility permissions.
    pub fn is_process_trusted() -> bool {
        // Safety: AXIsProcessTrusted is a safe C function
        unsafe { AXIsProcessTrusted() }
    }

    /// Get an attribute value from an AXUIElement.
    unsafe fn get_attribute(element: &AXUIElement, attribute: &str) -> Result<CFRetained<CFType>> {
        let attr = CFString::from_str(attribute);
        let mut value: *const CFType = std::ptr::null();
        let value_ptr: *mut *const CFType = &mut value;

        let result =
            unsafe { element.copy_attribute_value(&attr, NonNull::new(value_ptr).unwrap()) };

        if result == AXError::Success && !value.is_null() {
            // Safety: copy_attribute_value returns a +1 retained value
            let retained =
                unsafe { CFRetained::from_raw(NonNull::new(value as *mut CFType).unwrap()) };
            Ok(retained)
        } else {
            Err(anyhow!(
                "Failed to get attribute {}: {:?}",
                attribute,
                result
            ))
        }
    }

    /// Get a string attribute value.
    unsafe fn get_string_attribute(element: &AXUIElement, attribute: &str) -> Option<String> {
        Self::get_attribute(element, attribute)
            .ok()
            .and_then(|value| {
                // Try to cast to CFString
                let cf_string = value.downcast::<CFString>().ok()?;
                Some(cf_string.to_string())
            })
    }

    /// Get a boolean attribute value.
    ///
    /// Note: This is simplified - proper implementation would use CFBoolean.
    /// For now we just check if the attribute exists.
    unsafe fn get_bool_attribute(element: &AXUIElement, attribute: &str) -> Option<bool> {
        // If we can get the attribute, assume it's true
        // A proper implementation would check CFBooleanGetValue
        Self::get_attribute(element, attribute).ok().map(|_| true)
    }

    /// Get the position of an element as a Point.
    unsafe fn get_position(element: &AXUIElement) -> Option<Point> {
        let value = Self::get_attribute(element, AX_POSITION).ok()?;
        let ax_value = value.downcast_ref::<AXValue>()?;

        let mut point = objc2_core_foundation::CGPoint { x: 0.0, y: 0.0 };
        let success = ax_value.value(
            AXValueType::CGPoint,
            NonNull::new(&mut point as *mut _ as *mut _).unwrap(),
        );

        if success {
            Some(Point::new(point.x, point.y))
        } else {
            None
        }
    }

    /// Get the size of an element.
    unsafe fn get_size(element: &AXUIElement) -> Option<(f64, f64)> {
        let value = Self::get_attribute(element, AX_SIZE).ok()?;
        let ax_value = value.downcast_ref::<AXValue>()?;

        let mut size = objc2_core_foundation::CGSize {
            width: 0.0,
            height: 0.0,
        };
        let success = ax_value.value(
            AXValueType::CGSize,
            NonNull::new(&mut size as *mut _ as *mut _).unwrap(),
        );

        if success {
            Some((size.width, size.height))
        } else {
            None
        }
    }

    /// Get the bounds (position + size) of an element.
    unsafe fn get_bounds(element: &AXUIElement) -> Option<Rect> {
        let position = Self::get_position(element)?;
        let (width, height) = Self::get_size(element)?;

        use crate::accessibility::Size;
        Some(Rect::new(position, Size::new(width, height)))
    }

    /// Get the children of an element.
    unsafe fn get_children(element: &AXUIElement) -> Vec<CFRetained<AXUIElement>> {
        let value = match unsafe { Self::get_attribute(element, AX_CHILDREN) } {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        // The returned value is a CFArray of AXUIElements
        // Use cast_unchecked to convert CFType to CFArray<AXUIElement>
        let array: CFRetained<CFArray<AXUIElement>> = unsafe { CFRetained::cast_unchecked(value) };

        let mut children = Vec::new();
        for i in 0..array.len() {
            if let Some(child) = array.get(i) {
                children.push(child);
            }
        }
        children
    }

    /// Get available actions for an element.
    unsafe fn get_actions(element: &AXUIElement) -> Vec<String> {
        let mut names: *const CFArray = std::ptr::null();
        let result = element.copy_action_names(NonNull::new(&mut names).unwrap());

        if result != AXError::Success || names.is_null() {
            return Vec::new();
        }

        let names = NonNull::new(names as *mut CFArray as *mut CFArray<CFString>).unwrap();
        let array: CFRetained<CFArray<CFString>> = CFRetained::from_raw(names);
        let mut actions = Vec::new();

        for i in 0..array.len() {
            if let Some(name) = array.get(i) {
                actions.push(name.to_string());
            }
        }

        actions
    }

    /// Map an AX role string to an accesskit Role.
    fn map_role(ax_role: &str) -> Role {
        match ax_role {
            ROLE_BUTTON => Role::Button,
            ROLE_TEXT_FIELD => Role::TextInput,
            ROLE_TEXT_AREA => Role::MultilineTextInput,
            ROLE_STATIC_TEXT => Role::TextRun,
            ROLE_CHECKBOX => Role::CheckBox,
            ROLE_RADIO_BUTTON => Role::RadioButton,
            ROLE_POPUP_BUTTON | ROLE_COMBO_BOX => Role::ComboBox,
            ROLE_SLIDER => Role::Slider,
            ROLE_TABLE => Role::Table,
            ROLE_LIST => Role::List,
            ROLE_OUTLINE => Role::Tree,
            ROLE_WINDOW => Role::Window,
            ROLE_SHEET => Role::Dialog,
            ROLE_MENU => Role::Menu,
            ROLE_MENU_ITEM => Role::MenuItem,
            ROLE_MENU_BAR => Role::MenuBar,
            ROLE_MENU_BAR_ITEM => Role::MenuItem,
            ROLE_WEB_AREA => Role::WebView,
            ROLE_GROUP => Role::Group,
            ROLE_IMAGE => Role::Image,
            ROLE_LINK => Role::Link,
            ROLE_APPLICATION => Role::Application,
            ROLE_SCROLL_AREA => Role::ScrollView,
            ROLE_TOOLBAR => Role::Toolbar,
            ROLE_TAB_GROUP => Role::TabList,
            ROLE_TAB => Role::Tab,
            ROLE_PROGRESS_INDICATOR => Role::ProgressIndicator,
            ROLE_SPLIT_GROUP => Role::Splitter,
            ROLE_SPLITTER => Role::Splitter,
            ROLE_ROW => Role::Row,
            ROLE_COLUMN => Role::ListItem,
            ROLE_CELL => Role::Cell,
            _ => Role::Unknown,
        }
    }

    /// Map an accesskit Action to an AX action string.
    fn map_action(action: Action) -> Option<&'static str> {
        match action {
            Action::Click => Some(AX_PRESS),
            Action::Focus => None, // Focus is set via attribute
            Action::ShowContextMenu => Some(AX_SHOW_MENU),
            Action::Increment => Some(AX_INCREMENT),
            Action::Decrement => Some(AX_DECREMENT),
            _ => None,
        }
    }

    /// Build an Element from an AXUIElement.
    unsafe fn build_element(
        &mut self,
        ax_element: &AXUIElement,
        filter: &TreeFilter,
        depth: usize,
        element_count: &mut usize,
    ) -> Option<Element> {
        // Check element count limit
        if let Some(max) = filter.max_elements
            && *element_count >= max
        {
            return None;
        }

        // Get role
        let ax_role = Self::get_string_attribute(ax_element, AX_ROLE)?;
        let role = Self::map_role(&ax_role);

        // Allocate ID before storing the platform handle; this preserves the existing
        // handle/cache ordering for macOS while the cache API transition settles.
        #[allow(deprecated)]
        let id = self.cache.next_id();

        // Build element
        let mut element = Element::new(id, role);
        element.title = Self::get_string_attribute(ax_element, AX_TITLE);
        element.description = Self::get_string_attribute(ax_element, AX_DESCRIPTION);
        element.value = Self::get_string_attribute(ax_element, AX_VALUE);
        element.bounds = Self::get_bounds(ax_element);
        element.enabled = Self::get_bool_attribute(ax_element, AX_ENABLED).unwrap_or(true);
        element.focused = Self::get_bool_attribute(ax_element, AX_FOCUSED).unwrap_or(false);
        element.actions = Self::get_actions(ax_element);

        // Check filter
        if !filter.should_include(&element, depth) {
            return None;
        }

        // Store handle for actions - convert reference to NonNull for retain
        self.handles
            .insert(id, unsafe { CFRetained::retain(ax_element.into()) });

        // Store in cache
        #[allow(deprecated)]
        self.cache.store_with_id(id, element.clone());
        *element_count += 1;

        // Process children (if not at max depth)
        let should_recurse = filter.max_depth.is_none_or(|max| depth < max);
        if should_recurse {
            let children = Self::get_children(ax_element);
            for child in children {
                if let Some(child_element) =
                    self.build_element(&child, filter, depth + 1, element_count)
                {
                    element.children.push(child_element);
                }
            }
        }

        Some(element)
    }

    /// Get the focused application's PID using NSWorkspace (most reliable method).
    fn get_frontmost_app_pid() -> Option<u32> {
        use objc2::rc::Retained;
        use objc2_app_kit::{NSRunningApplication, NSWorkspace};

        let workspace = NSWorkspace::sharedWorkspace();
        let frontmost: Option<Retained<NSRunningApplication>> = workspace.frontmostApplication();

        if let Some(app) = frontmost {
            let pid = app.processIdentifier();
            if pid > 0 {
                return Some(pid as u32);
            }
        }

        None
    }

    /// List all visible application windows with their PIDs, app names, window titles, and focus state.
    pub fn list_windows() -> Vec<(u32, String, String, bool)> {
        use objc2_app_kit::NSWorkspace;

        let mut windows = Vec::new();
        let workspace = NSWorkspace::sharedWorkspace();

        // Get frontmost app to determine focus
        let frontmost_pid = workspace
            .frontmostApplication()
            .map(|app| app.processIdentifier() as u32);

        // Get all running applications
        let running_apps = workspace.runningApplications();

        for app in running_apps.iter() {
            let pid = app.processIdentifier();
            if pid <= 0 {
                continue;
            }
            let pid = pid as u32;

            // Skip apps without activation policy (background processes)
            // activationPolicy: 0 = regular, 1 = accessory, 2 = prohibited
            let policy = app.activationPolicy();
            if policy.0 != 0 {
                continue;
            }

            // Get app name
            let app_name: String = app
                .localizedName()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "Unknown".to_string());

            // Try to get window title from accessibility
            let window_title =
                unsafe { Self::get_window_title_for_pid(pid) }.unwrap_or_else(|| app_name.clone());

            let is_focused = frontmost_pid == Some(pid);

            windows.push((pid, app_name, window_title, is_focused));
        }

        windows
    }

    /// Get the window title for a given PID using accessibility APIs.
    unsafe fn get_window_title_for_pid(pid: u32) -> Option<String> {
        let app = AXUIElement::new_application(pid as i32);

        // Try main window first
        if let Ok(main_window) = Self::get_attribute(&app, AX_MAIN_WINDOW) {
            let window: CFRetained<AXUIElement> = CFRetained::cast_unchecked(main_window);
            if let Some(title) = Self::get_string_attribute(&window, AX_TITLE) {
                if !title.is_empty() {
                    return Some(title);
                }
            }
        }

        // Fall back to first window in windows array
        if let Ok(windows_attr) = Self::get_attribute(&app, AX_WINDOWS) {
            let windows: CFRetained<CFArray<AXUIElement>> =
                CFRetained::cast_unchecked(windows_attr);
            if windows.len() > 0 {
                if let Some(window) = windows.get(0) {
                    if let Some(title) = Self::get_string_attribute(&window, AX_TITLE) {
                        if !title.is_empty() {
                            return Some(title);
                        }
                    }
                }
            }
        }

        None
    }

    /// Get the focused application's PID (fallback using AX APIs).
    unsafe fn get_focused_app_pid_ax(&self) -> Option<u32> {
        // Try AXFocusedApplication first (returns the frontmost app element)
        if let Ok(focused_app) = Self::get_attribute(&self.system_wide, AX_FOCUSED_APPLICATION) {
            let ax_element: CFRetained<AXUIElement> = CFRetained::cast_unchecked(focused_app);

            let mut pid: libc::pid_t = 0;
            let pid_ptr = NonNull::new(&mut pid as *mut libc::pid_t).unwrap();
            let result = ax_element.pid(pid_ptr);

            if result == AXError::Success && pid > 0 {
                return Some(pid as u32);
            }
        }

        // Fallback: try AXFocusedUIElement
        if let Ok(focused) = Self::get_attribute(&self.system_wide, AX_FOCUSED_UI_ELEMENT) {
            let ax_element: CFRetained<AXUIElement> = CFRetained::cast_unchecked(focused);

            let mut pid: libc::pid_t = 0;
            let pid_ptr = NonNull::new(&mut pid as *mut libc::pid_t).unwrap();
            let result = ax_element.pid(pid_ptr);

            if result == AXError::Success && pid > 0 {
                return Some(pid as u32);
            }
        }

        None
    }
}

impl AccessibilityReader for MacOSAccessibility {
    fn get_tree(
        &mut self,
        pid: Option<u32>,
        filter: &TreeFilter,
    ) -> impl std::future::Future<Output = Result<ElementTree>> {
        // Clear previous cache
        self.clear_cache();

        let version = self.cache.version();

        let result: Result<ElementTree> = (|| {
            // Get the target application element
            let (app_element, actual_pid) = unsafe {
                if let Some(pid) = pid {
                    (AXUIElement::new_application(pid as libc::pid_t), pid)
                } else {
                    // Get focused application - try NSWorkspace first, then AX APIs
                    let focused_pid = Self::get_frontmost_app_pid()
                        .or_else(|| self.get_focused_app_pid_ax())
                        .ok_or_else(|| anyhow!("No focused application found"))?;
                    (
                        AXUIElement::new_application(focused_pid as libc::pid_t),
                        focused_pid,
                    )
                }
            };

            // Build the tree
            let mut element_count = 0;
            let root = unsafe {
                self.build_element(&app_element, filter, 0, &mut element_count)
                    .ok_or_else(|| anyhow!("Failed to build accessibility tree"))?
            };

            // Try to get app name
            let app_name = unsafe { Self::get_string_attribute(&app_element, AX_TITLE) };

            Ok(ElementTree {
                version,
                pid: Some(actual_pid),
                app_name,
                root,
                element_count,
            })
        })();

        std::future::ready(result)
    }

    fn get_element(&self, id: ElementKey) -> Option<&Element> {
        self.cache.get(id)
    }

    fn perform_action(
        &mut self,
        id: ElementKey,
        action: Action,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result: Result<()> = (|| {
            let handle = self
                .handles
                .get(&id)
                .ok_or_else(|| anyhow!("Element {} not found in cache", id))?;

            // Safety: We're calling AXUIElement methods with valid handles
            unsafe {
                // Map action to AX action string
                let action_name = Self::map_action(action)
                    .ok_or_else(|| anyhow!("Action {:?} not supported on macOS", action))?;

                let action_str = CFString::from_str(action_name);
                let result = handle.perform_action(&action_str);

                if result != AXError::Success {
                    bail!("Failed to perform action {}: {:?}", action_name, result);
                }
            }

            Ok(())
        })();

        std::future::ready(result)
    }

    fn set_value(
        &mut self,
        id: ElementKey,
        value: &str,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result: Result<()> = (|| {
            let handle = self
                .handles
                .get(&id)
                .ok_or_else(|| anyhow!("Element {} not found in cache", id))?;

            unsafe {
                let attr = CFString::from_str(AX_VALUE);
                let cf_value = CFString::from_str(value);
                let result = handle.set_attribute_value(&attr, &cf_value);

                if result != AXError::Success {
                    bail!("Failed to set value: {:?}", result);
                }
            }

            Ok(())
        })();

        std::future::ready(result)
    }

    fn hit_test(
        &mut self,
        x: f64,
        y: f64,
    ) -> impl std::future::Future<Output = Result<Option<ElementKey>>> {
        let result = unsafe {
            let mut element: *const AXUIElement = std::ptr::null();
            let element_ptr: *mut *const AXUIElement = &mut element;
            let result = self.system_wide.copy_element_at_position(
                x as f32,
                y as f32,
                NonNull::new(element_ptr).unwrap(),
            );

            if result != AXError::Success || element.is_null() {
                Ok(None)
            } else {
                // Convert raw pointer to CFRetained
                let ptr = NonNull::new(element as *mut AXUIElement).unwrap();
                let ax_element: CFRetained<AXUIElement> = CFRetained::from_raw(ptr);

                // Build element and add to cache
                let mut count = self.cache.len();
                let element =
                    self.build_element(&ax_element, &TreeFilter::default(), 0, &mut count);

                Ok(element.map(|e| e.id))
            }
        };

        std::future::ready(result)
    }

    fn clear_cache(&mut self) {
        self.cache.clear();
        self.handles.clear();
    }

    fn snapshot_version(&self) -> u64 {
        self.cache.version()
    }
}

use super::ios_simulator;

/// Trait for iOS simulator adapters that can perform HID and accessibility operations.
pub trait IOSAdapter: AccessibilityReader {
    /// HID tap at coordinates.
    fn hid_tap(&mut self, x: f64, y: f64) -> Result<()>;

    /// HID swipe gesture.
    fn hid_swipe(&mut self, start: (f64, f64), end: (f64, f64), duration_ms: u64) -> Result<()>;

    /// HID hardware button press.
    fn hid_button(&mut self, button: ios_simulator::HardwareButton, hold_ms: u64) -> Result<()>;

    /// Accessibility-based tap at coordinates.
    fn tap(&mut self, x: f64, y: f64) -> Result<()>;

    /// Press element by ID.
    fn press(&mut self, id: ElementKey) -> Result<()>;

    /// Get simulator UDID.
    fn device_udid(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_mapping() {
        assert_eq!(MacOSAccessibility::map_role("AXButton"), Role::Button);
        assert_eq!(MacOSAccessibility::map_role("AXTextField"), Role::TextInput);
        assert_eq!(MacOSAccessibility::map_role("AXUnknownRole"), Role::Unknown);
    }
}
