// `AccessibilityReader` declares its async methods as `-> impl Future<Output = ...>`
// so this impl mirrors that style. clippy prefers `async fn` but mixing the two
// forms across trait declarations and impls is gratuitous churn.
#![allow(clippy::manual_async_fn)]

//! Android device/emulator accessibility via ADB.
//!
//! This module provides accessibility support for Android devices and emulators through
//! the Android Debug Bridge (ADB). Unlike other platforms that use native accessibility APIs,
//! Android support works through the ADB server smartsocket protocol.
//!
//! # Architecture
//!
//! ```text
//! Rust (AndroidAccessibility)
//!     ↓ TCP smartsocket
//! ADB server
//!     ↓
//! Android Device/Emulator
//! ```
//!
//! # Key Differences from Other Platforms
//!
//! - **Cross-platform host support**: Works on macOS, Windows, and Linux hosts (wherever ADB is available)
//! - **No native accessibility API**: Uses `uiautomator dump` for UI tree and `input` commands for actions
//! - **No event listening**: Android via ADB doesn't support real-time accessibility events
//!
//! # Example
//!
//! ```ignore
//! use accessibility_core::platform::android::AndroidAccessibility;
//! use accessibility_core::accessibility::{AccessibilityReader, TreeFilter};
//!
//! // Connect to the default device
//! let mut reader = AndroidAccessibility::new(None).await?;
//!
//! // Get the UI tree
//! let tree = reader.get_tree(&Target::Android(AndroidTarget::DefaultDevice), &TreeFilter::default()).await?;
//! println!("{:?}", tree);
//!
//! // Press the back button
//! reader.press_back().await?;
//! ```

use std::future::Future;

use accesskit::{Action, Role};
use anyhow::{Result, anyhow, bail};
use quick_xml::Reader;
use quick_xml::events::Event;
use slotmap::SecondaryMap;

use crate::accessibility::{
    AccessibilityEvent, AccessibilityEventType, AccessibilityReader, Element, ElementCache,
    ElementKey, ElementTree, ListenerConfig, ListenerHandle, Point, Rect, Screenshot, Size, Target,
    TreeFilter,
};
use crate::input::{Code, Modifiers, MouseButton};
use crate::video::{FrameSink, ScreenGeometry, VideoCapture, VideoConfig};

pub mod ax;
pub mod input;
pub mod session;
pub mod video;
pub use accessibility_android_sys::{AdbClient, AndroidKeyCode};
pub use input::{HardwareButton, InputCommand, Orientation, TouchPhase, spawn_input_worker};
pub use video::AndroidVideoCapture;

/// Parse Android bounds string like "[0,0][1080,1920]" into a Rect.
fn parse_bounds(bounds_str: &str) -> Option<Rect> {
    // Format: "[left,top][right,bottom]"
    let trimmed = bounds_str.trim();
    if !trimmed.starts_with('[') || !trimmed.contains("][") {
        return None;
    }

    // Split into two coordinate pairs
    let parts: Vec<&str> = trimmed.split("][").collect();
    if parts.len() != 2 {
        return None;
    }

    // Parse first pair (left, top)
    let first = parts[0].trim_start_matches('[');
    let first_coords: Vec<&str> = first.split(',').collect();
    if first_coords.len() != 2 {
        return None;
    }

    // Parse second pair (right, bottom)
    let second = parts[1].trim_end_matches(']');
    let second_coords: Vec<&str> = second.split(',').collect();
    if second_coords.len() != 2 {
        return None;
    }

    let left: f64 = first_coords[0].parse().ok()?;
    let top: f64 = first_coords[1].parse().ok()?;
    let right: f64 = second_coords[0].parse().ok()?;
    let bottom: f64 = second_coords[1].parse().ok()?;

    Some(Rect::new(
        Point::new(left, top),
        Size::new(right - left, bottom - top),
    ))
}

/// Map Android class name to AccessKit Role.
fn map_android_class_to_role(class: &str) -> Role {
    // Extract the simple class name (last part after dot)
    let simple_name = class.rsplit('.').next().unwrap_or(class);

    match simple_name {
        // Buttons
        "Button"
        | "ImageButton"
        | "FloatingActionButton"
        | "MaterialButton"
        | "AppCompatButton" => Role::Button,

        // Text inputs
        "EditText"
        | "AutoCompleteTextView"
        | "MultiAutoCompleteTextView"
        | "TextInputEditText"
        | "AppCompatEditText" => Role::TextInput,

        // Text display
        "TextView" | "AppCompatTextView" => Role::Label,

        // Checkboxes and toggles
        "CheckBox" | "AppCompatCheckBox" | "MaterialCheckBox" => Role::CheckBox,
        "Switch" | "SwitchCompat" | "SwitchMaterial" => Role::Switch,
        "ToggleButton" => Role::Switch,
        "RadioButton" | "AppCompatRadioButton" | "MaterialRadioButton" => Role::RadioButton,

        // Images
        "ImageView" | "AppCompatImageView" => Role::Image,

        // Lists and scrolling
        "ListView" => Role::List,
        "RecyclerView" => Role::List,
        "GridView" => Role::Grid,
        "ScrollView" | "HorizontalScrollView" | "NestedScrollView" => Role::ScrollView,

        // Containers
        "LinearLayout" | "RelativeLayout" | "FrameLayout" | "ConstraintLayout"
        | "CoordinatorLayout" | "TableLayout" => Role::GenericContainer,
        "ViewGroup" => Role::GenericContainer,
        "CardView" | "MaterialCardView" => Role::GenericContainer,

        // Tabs
        "TabLayout" | "TabItem" => Role::Tab,
        "ViewPager" | "ViewPager2" => Role::TabList,

        // Navigation
        "NavigationView" | "NavigationRailView" | "BottomNavigationView" => Role::Navigation,
        "Toolbar" | "ActionBar" | "MaterialToolbar" => Role::Toolbar,

        // Dialogs
        "AlertDialog" | "Dialog" => Role::Dialog,

        // Progress indicators
        "ProgressBar" | "CircularProgressIndicator" | "LinearProgressIndicator" => {
            Role::ProgressIndicator
        }

        // Sliders
        "SeekBar" | "RatingBar" | "Slider" => Role::Slider,

        // Spinners (dropdowns)
        "Spinner" | "AppCompatSpinner" => Role::ComboBox,

        // Web views
        "WebView" => Role::Document,

        // Menu items
        "MenuItem" => Role::MenuItem,

        // Links
        "URLSpan" => Role::Link,

        // Default for unknown classes
        _ => {
            // Check for common patterns
            if simple_name.contains("Button") {
                Role::Button
            } else if simple_name.contains("Text") && simple_name.contains("Edit") {
                Role::TextInput
            } else if simple_name.contains("Text") {
                Role::Label
            } else if simple_name.contains("Check") {
                Role::CheckBox
            } else if simple_name.contains("Radio") {
                Role::RadioButton
            } else if simple_name.contains("Image") {
                Role::Image
            } else if simple_name.contains("List") {
                Role::List
            } else if simple_name.contains("Scroll") {
                Role::ScrollView
            } else if simple_name.contains("Layout") || simple_name.contains("Container") {
                Role::GenericContainer
            } else if simple_name.contains("Dialog") {
                Role::Dialog
            } else if simple_name.contains("Progress") {
                Role::ProgressIndicator
            } else if simple_name.contains("Seek") || simple_name.contains("Slider") {
                Role::Slider
            } else if simple_name.contains("Spinner") {
                Role::ComboBox
            } else if simple_name.contains("Tab") {
                Role::Tab
            } else if simple_name.contains("Menu") {
                Role::MenuItem
            } else {
                Role::Unknown
            }
        }
    }
}

/// Parsed node from uiautomator XML.
#[derive(Debug)]
struct UiNode {
    /// Android class name (e.g., "android.widget.Button").
    class: String,
    /// Text content.
    text: Option<String>,
    /// Resource ID (e.g., "com.example:id/button").
    resource_id: Option<String>,
    /// Content description (accessibility label).
    content_desc: Option<String>,
    /// Bounds string.
    bounds_str: String,
    /// Whether the node is clickable.
    clickable: bool,
    /// Whether the node is focusable.
    focusable: bool,
    /// Whether the node is enabled.
    enabled: bool,
    /// Whether the node is focused.
    focused: bool,
    /// Whether the node is checkable.
    checkable: bool,
    /// Whether the node is checked.
    checked: bool,
    /// Whether the node is scrollable.
    scrollable: bool,
    /// Whether the node is long-clickable.
    long_clickable: bool,
    /// Package name.
    package: Option<String>,
    /// Child nodes.
    children: Vec<UiNode>,
}

impl UiNode {
    fn new() -> Self {
        Self {
            class: String::new(),
            text: None,
            resource_id: None,
            content_desc: None,
            bounds_str: String::new(),
            clickable: false,
            focusable: false,
            enabled: true,
            focused: false,
            checkable: false,
            checked: false,
            scrollable: false,
            long_clickable: false,
            package: None,
            children: Vec::new(),
        }
    }

    /// Convert to Element, recursively processing children.
    fn to_element(
        &self,
        cache: &mut ElementCache,
        bounds_map: &mut SecondaryMap<ElementKey, String>,
    ) -> Element {
        let role = map_android_class_to_role(&self.class);
        let bounds = parse_bounds(&self.bounds_str);

        let (id, mut element) = cache.store_with_clone(|id| {
            let mut elem = Element::new(id, role);
            elem.title = self
                .text
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| self.content_desc.clone().filter(|s| !s.is_empty()));
            elem.description = self.content_desc.clone().filter(|s| !s.is_empty());
            elem.identifier = self.resource_id.clone();
            elem.bounds = bounds;
            elem.enabled = self.enabled;
            elem.focused = self.focused;

            // Build actions list
            let mut actions = Vec::new();
            if self.clickable {
                actions.push("click".to_string());
            }
            if self.long_clickable {
                actions.push("longClick".to_string());
            }
            if self.scrollable {
                actions.push("scroll".to_string());
            }
            if self.checkable {
                actions.push("toggle".to_string());
            }
            if self.focusable {
                actions.push("focus".to_string());
            }
            elem.actions = actions;

            // Store value for checkable items
            if self.checkable {
                elem.value = Some(if self.checked { "true" } else { "false" }.to_string());
            }

            elem
        });

        // Store bounds string for later tap coordinate calculation
        bounds_map.insert(id, self.bounds_str.clone());

        // Process children
        for child in &self.children {
            element.children.push(child.to_element(cache, bounds_map));
        }

        element
    }
}

/// Parse uiautomator XML dump into a tree of UiNodes.
fn parse_ui_xml(xml: &str) -> Result<UiNode> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut node_stack: Vec<UiNode> = vec![UiNode::new()]; // Root container
    node_stack[0].class = "hierarchy".to_string();

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(ref e)) => {
                // Self-closing tag like <node ... />
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                if tag_name == "node" {
                    let mut node = UiNode::new();

                    for attr in e.attributes().filter_map(|a| a.ok()) {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let value = String::from_utf8_lossy(&attr.value).to_string();

                        match key.as_str() {
                            "class" => node.class = value,
                            "text" => node.text = Some(value).filter(|s| !s.is_empty()),
                            "resource-id" => {
                                node.resource_id = Some(value).filter(|s| !s.is_empty())
                            }
                            "content-desc" => {
                                node.content_desc = Some(value).filter(|s| !s.is_empty())
                            }
                            "bounds" => node.bounds_str = value,
                            "clickable" => node.clickable = value == "true",
                            "focusable" => node.focusable = value == "true",
                            "enabled" => node.enabled = value == "true",
                            "focused" => node.focused = value == "true",
                            "checkable" => node.checkable = value == "true",
                            "checked" => node.checked = value == "true",
                            "scrollable" => node.scrollable = value == "true",
                            "long-clickable" => node.long_clickable = value == "true",
                            "package" => node.package = Some(value),
                            _ => {}
                        }
                    }

                    // Add to parent
                    if let Some(parent) = node_stack.last_mut() {
                        parent.children.push(node);
                    }
                }
            }
            Ok(Event::Start(ref e)) => {
                // Opening tag like <node ...> with children
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();

                if tag_name == "node" {
                    let mut node = UiNode::new();

                    for attr in e.attributes().filter_map(|a| a.ok()) {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let value = String::from_utf8_lossy(&attr.value).to_string();

                        match key.as_str() {
                            "class" => node.class = value,
                            "text" => node.text = Some(value).filter(|s| !s.is_empty()),
                            "resource-id" => {
                                node.resource_id = Some(value).filter(|s| !s.is_empty())
                            }
                            "content-desc" => {
                                node.content_desc = Some(value).filter(|s| !s.is_empty())
                            }
                            "bounds" => node.bounds_str = value,
                            "clickable" => node.clickable = value == "true",
                            "focusable" => node.focusable = value == "true",
                            "enabled" => node.enabled = value == "true",
                            "focused" => node.focused = value == "true",
                            "checkable" => node.checkable = value == "true",
                            "checked" => node.checked = value == "true",
                            "scrollable" => node.scrollable = value == "true",
                            "long-clickable" => node.long_clickable = value == "true",
                            "package" => node.package = Some(value),
                            _ => {}
                        }
                    }

                    // Push onto stack to collect children
                    node_stack.push(node);
                } else if tag_name == "hierarchy" {
                    // Parse hierarchy attributes if present
                    for attr in e.attributes().filter_map(|a| a.ok()) {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let value = String::from_utf8_lossy(&attr.value).to_string();
                        if key == "rotation" {
                            // Could store rotation info if needed
                            let _ = value;
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let tag_name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag_name == "node" {
                    // Pop this node and add to parent
                    if node_stack.len() > 1 {
                        let node = node_stack.pop().unwrap();
                        if let Some(parent) = node_stack.last_mut() {
                            parent.children.push(node);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => bail!(
                "Error parsing UI XML at position {}: {:?}",
                reader.buffer_position(),
                e
            ),
            _ => {}
        }
        buf.clear();
    }

    // Return the root node
    Ok(node_stack.remove(0))
}

/// Android accessibility reader using ADB.
///
/// This implementation uses the Android Debug Bridge (ADB) to:
/// - Query the UI hierarchy via `uiautomator dump`
/// - Capture screenshots via `screencap`
/// - Send input events via `input` commands
///
/// Unlike native accessibility APIs on other platforms, Android via ADB:
/// - Works on any host OS (macOS, Linux, Windows) where ADB is available
/// - Has higher latency due to process spawning for each command
/// - Does not support real-time event listening
pub struct AndroidAccessibility {
    /// ADB client for command execution.
    adb: AdbClient,
    /// Element cache for ID management.
    cache: ElementCache,
    /// Map from element key to bounds string (for tap coordinate calculation).
    element_bounds: SecondaryMap<ElementKey, String>,
    /// Cached screen size.
    screen_size: Option<(u32, u32)>,
    /// Last known app package (for PID-like targeting).
    last_package: Option<String>,
}

impl AndroidAccessibility {
    /// Create a new Android accessibility reader.
    ///
    /// # Arguments
    /// * `serial` - Optional device serial number. Use `adb devices` to list connected devices.
    ///   If None, uses the default (only) connected device.
    ///
    /// # Errors
    /// Returns an error if ADB is not available or no device is connected.
    ///
    /// # Example
    /// ```ignore
    /// // Connect to default device
    /// let reader = AndroidAccessibility::new(None).await?;
    ///
    /// // Connect to specific device
    /// let reader = AndroidAccessibility::new(Some("emulator-5554")).await?;
    /// ```
    pub async fn new(serial: Option<&str>) -> Result<Self> {
        let adb = AdbClient::new(serial);
        adb.check_connection().await?;

        // Get initial screen size
        let screen_size = adb.get_screen_size().await.ok();

        Ok(Self {
            adb,
            cache: ElementCache::new(),
            element_bounds: SecondaryMap::new(),
            screen_size,
            last_package: None,
        })
    }

    /// Create a new Android accessibility reader with a custom ADB path.
    pub async fn with_adb_path(serial: Option<&str>, adb_path: &str) -> Result<Self> {
        let adb = AdbClient::with_adb_path(serial, adb_path);
        adb.check_connection().await?;

        let screen_size = adb.get_screen_size().await.ok();

        Ok(Self {
            adb,
            cache: ElementCache::new(),
            element_bounds: SecondaryMap::new(),
            screen_size,
            last_package: None,
        })
    }

    /// Get the ADB client for direct command access.
    pub fn adb(&self) -> &AdbClient {
        &self.adb
    }

    /// Get the screen size (width, height) in pixels.
    pub fn screen_size(&self) -> Option<(u32, u32)> {
        self.screen_size
    }

    /// Refresh the cached screen size.
    pub async fn refresh_screen_size(&mut self) -> Result<(u32, u32)> {
        let size = self.adb.get_screen_size().await?;
        self.screen_size = Some(size);
        Ok(size)
    }

    /// Get the center point of an element by its ID.
    fn get_element_center(&self, id: ElementKey) -> Option<Point> {
        let bounds_str = self.element_bounds.get(id)?;
        let bounds = parse_bounds(bounds_str)?;
        Some(bounds.center())
    }
}

impl AccessibilityReader for AndroidAccessibility {
    fn get_tree(
        &mut self,
        _target: &Target,
        filter: &TreeFilter,
    ) -> impl Future<Output = Result<ElementTree>> {
        async move {
            // Clear previous cache
            self.cache.clear();
            self.element_bounds.clear();

            // Dump UI hierarchy
            let xml = self.adb.dump_ui().await?;

            // Parse XML into node tree
            let root_node = parse_ui_xml(&xml)?;

            // Get package name if available
            let app_name = root_node.children.first().and_then(|n| n.package.clone());
            self.last_package = app_name.clone();

            // Convert to Element tree
            let root = root_node.to_element(&mut self.cache, &mut self.element_bounds);
            let element_count = self.cache.len();

            // Apply filter if needed
            let filtered_root =
                if filter.interactive_only || filter.visible_only || filter.max_depth.is_some() {
                    filter_element(root, filter, 0)
                } else {
                    root
                };

            Ok(ElementTree {
                version: self.cache.version(),
                pid: None, // Android doesn't use PID
                app_name,
                root: filtered_root,
                element_count,
            })
        }
    }

    fn get_element(&self, id: ElementKey) -> Option<&Element> {
        self.cache.get(id)
    }

    fn perform_action(
        &mut self,
        id: ElementKey,
        action: Action,
    ) -> impl Future<Output = Result<()>> {
        async move {
            match action {
                Action::Click => {
                    // Get element center and tap
                    let center = self
                        .get_element_center(id)
                        .ok_or_else(|| anyhow!("Element {} not found or has no bounds", id))?;
                    self.adb.tap(center.x, center.y).await?;
                    Ok(())
                }
                Action::Focus => {
                    // Tap to focus
                    let center = self
                        .get_element_center(id)
                        .ok_or_else(|| anyhow!("Element {} not found or has no bounds", id))?;
                    self.adb.tap(center.x, center.y).await?;
                    Ok(())
                }
                Action::ScrollIntoView => {
                    // Basic implementation: swipe up to scroll down
                    if let Some(size) = self.screen_size {
                        let center_x = size.0 as f64 / 2.0;
                        let start_y = size.1 as f64 * 0.7;
                        let end_y = size.1 as f64 * 0.3;
                        self.adb
                            .swipe((center_x, start_y), (center_x, end_y), 300)
                            .await?;
                    }
                    Ok(())
                }
                _ => {
                    bail!("Action {:?} not supported on Android", action)
                }
            }
        }
    }

    fn set_value(&mut self, id: ElementKey, value: &str) -> impl Future<Output = Result<()>> {
        async move {
            // First tap to focus the element
            let center = self
                .get_element_center(id)
                .ok_or_else(|| anyhow!("Element {} not found or has no bounds", id))?;
            self.adb.tap(center.x, center.y).await?;

            // Small delay to ensure focus
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // Clear existing text (select all and delete)
            self.adb.key_event(AndroidKeyCode::CtrlLeft as u32).await?;
            self.adb.key_event(AndroidKeyCode::A as u32).await?;
            self.adb.key_event(AndroidKeyCode::Del as u32).await?;

            // Type the new value
            if !value.is_empty() {
                self.adb.input_text(value).await?;
            }

            Ok(())
        }
    }

    fn hit_test(&mut self, x: f64, y: f64) -> impl Future<Output = Result<Option<ElementKey>>> {
        async move {
            let point = Point::new(x, y);

            // Search through cached elements for one containing the point
            // Return the deepest (most specific) element
            let mut best_match: Option<(ElementKey, f64)> = None; // (id, area)

            for (id, element) in self.cache.iter() {
                if let Some(bounds) = &element.bounds
                    && bounds.contains(point)
                {
                    let area = bounds.size.width * bounds.size.height;
                    // Prefer smaller (more specific) elements
                    if best_match.is_none() || area < best_match.unwrap().1 {
                        best_match = Some((id, area));
                    }
                }
            }

            Ok(best_match.map(|(id, _)| id))
        }
    }

    fn clear_cache(&mut self) {
        self.cache.clear();
        self.element_bounds.clear();
    }

    fn snapshot_version(&self) -> u64 {
        self.cache.version()
    }

    fn capture_screen(&self, _target: &Target) -> impl Future<Output = Result<Screenshot>> {
        async move {
            let data = self.adb.screenshot().await?;

            // Get image dimensions from PNG header
            let (width, height) = if data.len() > 24 {
                // PNG header: 8 bytes signature, then IHDR chunk
                // IHDR starts at byte 8, width at 16, height at 20 (both big-endian u32)
                let width = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
                let height = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
                (width, height)
            } else {
                self.screen_size.unwrap_or_default()
            };

            Ok(Screenshot {
                data,
                width,
                height,
            })
        }
    }

    fn get_screen_bounds(&self, _target: &Target) -> impl Future<Output = Result<Rect>> {
        async move {
            let (width, height) = self.screen_size.ok_or_else(|| {
                anyhow!("Screen size not available. Call refresh_screen_size() first.")
            })?;
            Ok(Rect::new(
                Point::new(0.0, 0.0),
                Size::new(width as f64, height as f64),
            ))
        }
    }

    fn platform_name(&self) -> &'static str {
        "Android"
    }

    fn keystroke(
        &mut self,
        _target: &Target,
        key: Code,
        modifiers: Modifiers,
    ) -> impl Future<Output = Result<()>> {
        async move {
            // Press modifiers
            if modifiers.contains(Modifiers::SHIFT) {
                self.adb.key_event(AndroidKeyCode::ShiftLeft as u32).await?;
            }
            if modifiers.contains(Modifiers::CONTROL) {
                self.adb.key_event(AndroidKeyCode::CtrlLeft as u32).await?;
            }
            if modifiers.contains(Modifiers::ALT) {
                self.adb.key_event(AndroidKeyCode::AltLeft as u32).await?;
            }
            if modifiers.contains(Modifiers::META) {
                self.adb.key_event(AndroidKeyCode::MetaLeft as u32).await?;
            }

            // Press main key
            if let Some(keycode) = AndroidKeyCode::from_code(key) {
                self.adb.key_event(keycode as u32).await?;
            } else {
                bail!("Unsupported key code: {:?}", key);
            }

            Ok(())
        }
    }

    fn type_raw(&mut self, _target: &Target, text: &str) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.input_text(text).await?;
            Ok(())
        }
    }

    fn mouse_click_at(
        &mut self,
        _target: &Target,
        x: f64,
        y: f64,
        _button: MouseButton,
    ) -> impl Future<Output = Result<()>> {
        async move {
            // Android only supports single tap (no right-click)
            self.adb.tap(x, y).await?;
            Ok(())
        }
    }

    fn mouse_scroll(
        &mut self,
        _target: &Target,
        delta_x: f64,
        delta_y: f64,
    ) -> impl Future<Output = Result<()>> {
        async move {
            let (width, height) = self
                .screen_size
                .ok_or_else(|| anyhow!("Screen size not available"))?;

            let center_x = width as f64 / 2.0;
            let center_y = height as f64 / 2.0;

            // Convert scroll deltas to swipe
            // Positive delta_y = scroll up, which is swipe down (start high, end low)
            let swipe_distance = 200.0; // pixels
            let start_x = center_x - delta_x * swipe_distance / 2.0;
            let start_y = center_y + delta_y * swipe_distance / 2.0;
            let end_x = center_x + delta_x * swipe_distance / 2.0;
            let end_y = center_y - delta_y * swipe_distance / 2.0;

            self.adb
                .swipe((start_x, start_y), (end_x, end_y), 100)
                .await?;
            Ok(())
        }
    }

    fn supports_keystroke(&self) -> bool {
        true
    }

    fn supports_mouse_click(&self) -> bool {
        true
    }

    fn supports_hit_test(&self) -> bool {
        true
    }

    fn start_video_capture(
        &self,
        config: &VideoConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn VideoCapture>> {
        let (width, height) = self
            .screen_size
            .ok_or_else(|| anyhow!("Android screen size is unavailable"))?;
        let capture = AndroidVideoCapture::start(
            self.adb.clone(),
            ScreenGeometry { width, height },
            config,
            sink,
        )?;
        Ok(Box::new(capture))
    }

    fn supports_video_capture(&self) -> bool {
        true
    }

    fn supports_terminal_display(&self) -> bool {
        true
    }

    fn supports_event_listening(&self) -> bool {
        false // ADB doesn't support real-time event listening
    }

    fn supported_event_types(&self) -> Vec<AccessibilityEventType> {
        Vec::new() // No events supported via ADB
    }

    fn start_listening(
        &mut self,
        _config: ListenerConfig,
        _callback: Box<dyn FnMut(AccessibilityEvent) + Send + 'static>,
    ) -> Result<ListenerHandle> {
        bail!("Event listening is not supported on Android via ADB")
    }
}

/// Filter an element tree according to TreeFilter settings.
fn filter_element(elem: Element, filter: &TreeFilter, depth: usize) -> Element {
    let mut filtered = elem;

    // Filter children recursively
    filtered.children = filtered
        .children
        .into_iter()
        .filter(|child| filter.should_include(child, depth + 1))
        .map(|child| filter_element(child, filter, depth + 1))
        .collect();

    // Apply max_elements limit if set (this is a simple implementation)
    if let Some(max) = filter.max_elements {
        let total = count_elements(&filtered);
        if total > max {
            // Truncate children to fit
            filtered.children.truncate(max.saturating_sub(1));
        }
    }

    filtered
}

/// Count total elements in a subtree.
fn count_elements(elem: &Element) -> usize {
    1 + elem.children.iter().map(count_elements).sum::<usize>()
}

/// Android-specific extension methods.
///
/// These methods provide convenient access to Android-specific functionality
/// that isn't available on other platforms.
pub trait AndroidExtensions {
    /// Press the Android Back button.
    fn press_back(&mut self) -> impl Future<Output = Result<()>>;

    /// Press the Android Home button.
    fn press_home(&mut self) -> impl Future<Output = Result<()>>;

    /// Press the Recent Apps (App Switcher) button.
    fn press_recent_apps(&mut self) -> impl Future<Output = Result<()>>;

    /// Press the Android Menu button.
    fn press_menu(&mut self) -> impl Future<Output = Result<()>>;

    /// Increase volume.
    fn volume_up(&mut self) -> impl Future<Output = Result<()>>;

    /// Decrease volume.
    fn volume_down(&mut self) -> impl Future<Output = Result<()>>;

    /// Mute/unmute volume.
    fn volume_mute(&mut self) -> impl Future<Output = Result<()>>;

    /// Press the power button (screen on/off).
    fn press_power(&mut self) -> impl Future<Output = Result<()>>;

    /// Wake up the device.
    fn wake_up(&mut self) -> impl Future<Output = Result<()>>;

    /// Put the device to sleep.
    fn sleep(&mut self) -> impl Future<Output = Result<()>>;

    /// Launch an app by package name.
    ///
    /// # Arguments
    /// * `package` - The app's package name (e.g., "com.android.settings")
    fn launch_app(&mut self, package: &str) -> impl Future<Output = Result<()>>;

    /// Stop an app by package name.
    ///
    /// # Arguments
    /// * `package` - The app's package name
    fn stop_app(&mut self, package: &str) -> impl Future<Output = Result<()>>;

    /// Perform a swipe gesture.
    ///
    /// # Arguments
    /// * `start` - Starting coordinates (x, y) in screen pixels
    /// * `end` - Ending coordinates (x, y) in screen pixels
    /// * `duration_ms` - Duration of the swipe in milliseconds
    fn swipe(
        &mut self,
        start: (f64, f64),
        end: (f64, f64),
        duration_ms: u64,
    ) -> impl Future<Output = Result<()>>;

    /// Perform a long press at coordinates.
    ///
    /// # Arguments
    /// * `x` - X coordinate in screen pixels
    /// * `y` - Y coordinate in screen pixels
    /// * `duration_ms` - Duration of the long press in milliseconds
    fn long_press(&mut self, x: f64, y: f64, duration_ms: u64) -> impl Future<Output = Result<()>>;

    /// Get the current foreground activity info.
    fn get_current_activity(&self) -> impl Future<Output = Result<String>>;

    /// Open the notification shade.
    fn open_notifications(&mut self) -> impl Future<Output = Result<()>>;

    /// Open quick settings.
    fn open_quick_settings(&mut self) -> impl Future<Output = Result<()>>;
}

impl AndroidExtensions for AndroidAccessibility {
    fn press_back(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::Back as u32).await?;
            Ok(())
        }
    }

    fn press_home(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::Home as u32).await?;
            Ok(())
        }
    }

    fn press_recent_apps(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::AppSwitch as u32).await?;
            Ok(())
        }
    }

    fn press_menu(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::Menu as u32).await?;
            Ok(())
        }
    }

    fn volume_up(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::VolumeUp as u32).await?;
            Ok(())
        }
    }

    fn volume_down(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb
                .key_event(AndroidKeyCode::VolumeDown as u32)
                .await?;
            Ok(())
        }
    }

    fn volume_mute(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb
                .key_event(AndroidKeyCode::VolumeMute as u32)
                .await?;
            Ok(())
        }
    }

    fn press_power(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::Power as u32).await?;
            Ok(())
        }
    }

    fn wake_up(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::Wakeup as u32).await?;
            Ok(())
        }
    }

    fn sleep(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.key_event(AndroidKeyCode::Sleep as u32).await?;
            Ok(())
        }
    }

    fn launch_app(&mut self, package: &str) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.launch_app(package, None).await?;
            Ok(())
        }
    }

    fn stop_app(&mut self, package: &str) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.stop_app(package).await?;
            Ok(())
        }
    }

    fn swipe(
        &mut self,
        start: (f64, f64),
        end: (f64, f64),
        duration_ms: u64,
    ) -> impl Future<Output = Result<()>> {
        async move {
            self.adb.swipe(start, end, duration_ms).await?;
            Ok(())
        }
    }

    fn long_press(&mut self, x: f64, y: f64, duration_ms: u64) -> impl Future<Output = Result<()>> {
        async move {
            // Long press is a swipe with same start and end
            self.adb.swipe((x, y), (x, y), duration_ms).await?;
            Ok(())
        }
    }

    fn get_current_activity(&self) -> impl Future<Output = Result<String>> {
        async move { self.adb.get_current_activity().await }
    }

    fn open_notifications(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb
                .shell(&["cmd", "statusbar", "expand-notifications"])
                .await?;
            Ok(())
        }
    }

    fn open_quick_settings(&mut self) -> impl Future<Output = Result<()>> {
        async move {
            self.adb
                .shell(&["cmd", "statusbar", "expand-settings"])
                .await?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bounds() {
        let bounds = parse_bounds("[0,0][1080,1920]").unwrap();
        assert_eq!(bounds.origin.x, 0.0);
        assert_eq!(bounds.origin.y, 0.0);
        assert_eq!(bounds.size.width, 1080.0);
        assert_eq!(bounds.size.height, 1920.0);

        let bounds = parse_bounds("[100,200][500,600]").unwrap();
        assert_eq!(bounds.origin.x, 100.0);
        assert_eq!(bounds.origin.y, 200.0);
        assert_eq!(bounds.size.width, 400.0);
        assert_eq!(bounds.size.height, 400.0);

        assert!(parse_bounds("invalid").is_none());
        assert!(parse_bounds("[0,0]").is_none());
        assert!(parse_bounds("").is_none());
    }

    #[test]
    fn test_role_mapping() {
        assert_eq!(
            map_android_class_to_role("android.widget.Button"),
            Role::Button
        );
        assert_eq!(
            map_android_class_to_role("android.widget.EditText"),
            Role::TextInput
        );
        assert_eq!(
            map_android_class_to_role("android.widget.TextView"),
            Role::Label
        );
        assert_eq!(
            map_android_class_to_role("android.widget.CheckBox"),
            Role::CheckBox
        );
        assert_eq!(
            map_android_class_to_role("android.widget.Switch"),
            Role::Switch
        );
        assert_eq!(
            map_android_class_to_role("android.widget.ListView"),
            Role::List
        );
        assert_eq!(
            map_android_class_to_role("android.widget.ScrollView"),
            Role::ScrollView
        );
        assert_eq!(
            map_android_class_to_role("android.widget.LinearLayout"),
            Role::GenericContainer
        );
        assert_eq!(
            map_android_class_to_role("android.widget.SeekBar"),
            Role::Slider
        );
        assert_eq!(
            map_android_class_to_role("android.widget.Spinner"),
            Role::ComboBox
        );
        assert_eq!(
            map_android_class_to_role("android.widget.ImageView"),
            Role::Image
        );
        assert_eq!(
            map_android_class_to_role("android.webkit.WebView"),
            Role::Document
        );
    }

    #[test]
    fn test_escape_shell_text() {
        assert_eq!(
            accessibility_android_sys::escape_shell_text("hello"),
            "hello"
        );
        assert_eq!(
            accessibility_android_sys::escape_shell_text("hello world"),
            "hello%sworld"
        );
        assert_eq!(
            accessibility_android_sys::escape_shell_text("test$var"),
            "test\\$var"
        );
        assert_eq!(accessibility_android_sys::escape_shell_text("a&b"), "a\\&b");
    }

    #[test]
    fn test_android_keycode_mapping() {
        assert_eq!(
            AndroidKeyCode::from_code(Code::KeyA),
            Some(AndroidKeyCode::A)
        );
        assert_eq!(
            AndroidKeyCode::from_code(Code::Enter),
            Some(AndroidKeyCode::Enter)
        );
        assert_eq!(
            AndroidKeyCode::from_code(Code::Backspace),
            Some(AndroidKeyCode::Del)
        );
        assert_eq!(
            AndroidKeyCode::from_code(Code::ArrowUp),
            Some(AndroidKeyCode::DpadUp)
        );
        assert_eq!(
            AndroidKeyCode::from_code(Code::F1),
            Some(AndroidKeyCode::F1)
        );
    }

    #[test]
    fn test_parse_ui_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <hierarchy rotation="0">
          <node index="0" text="Hello" class="android.widget.TextView"
                bounds="[0,0][1080,100]" clickable="false" enabled="true" />
          <node index="1" text="" class="android.widget.Button"
                content-desc="Submit" bounds="[0,100][1080,200]"
                clickable="true" enabled="true">
            <node index="0" text="Submit" class="android.widget.TextView"
                  bounds="[10,110][1070,190]" clickable="false" enabled="true" />
          </node>
        </hierarchy>"#;

        let root = parse_ui_xml(xml).unwrap();
        assert_eq!(root.class, "hierarchy");
        assert_eq!(root.children.len(), 2);

        let text_view = &root.children[0];
        assert_eq!(text_view.class, "android.widget.TextView");
        assert_eq!(text_view.text.as_deref(), Some("Hello"));
        assert!(!text_view.clickable);

        let button = &root.children[1];
        assert_eq!(button.class, "android.widget.Button");
        assert_eq!(button.content_desc.as_deref(), Some("Submit"));
        assert!(button.clickable);
        assert_eq!(button.children.len(), 1);
    }
}
