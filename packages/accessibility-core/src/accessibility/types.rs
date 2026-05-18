//! Core types for accessibility tree representation.

use accesskit::Role;
use euclid::{Point2D, Rect as EuclidRect, Size2D};
use serde::{Deserialize, Serialize};
use slotmap::{Key, KeyData};

// Define the key type for the SlotMap
slotmap::new_key_type! {
    /// Unique identifier for a cached accessibility element.
    ///
    /// This is a slotmap key type that includes both a slot index and a generation
    /// counter. This means that after `clear_cache()`, attempting to look up an old
    /// key will return `None` automatically (stale-key detection).
    ///
    /// The type is serializable via slotmap's native serde support (object with `idx`
    /// and `version` fields). For CLI parsing and display, use `to_ffi()` and `from_ffi()`.
    pub struct ElementKey;
}

impl ElementKey {
    /// Convert this ElementKey to a u64 FFI representation.
    ///
    /// This is useful for CLI parsing and display purposes.
    /// The u64 encoding is: lower 32 bits for the slot index, upper 32 bits for generation.
    pub fn to_ffi(self) -> u64 {
        self.data().as_ffi()
    }

    /// Create an ElementKey from a u64 FFI representation.
    ///
    /// This is useful for CLI parsing.
    pub fn from_ffi(value: u64) -> Self {
        KeyData::from_ffi(value).into()
    }
}

impl std::fmt::Display for ElementKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_ffi())
    }
}

/// Marker type for screen coordinate space.
///
/// This empty type is used with euclid's typed units to ensure that
/// screen coordinates are not accidentally mixed with other coordinate spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScreenSpace;

/// A point in screen coordinates.
pub type Point = Point2D<f64, ScreenSpace>;

/// A size in screen coordinates.
pub type Size = Size2D<f64, ScreenSpace>;

/// A rectangle in screen coordinates.
pub type Rect = EuclidRect<f64, ScreenSpace>;

/// A captured screenshot with PNG-encoded image data.
#[derive(Debug, Clone)]
pub struct Screenshot {
    /// PNG-encoded image bytes.
    pub data: Vec<u8>,
    /// Width of the image in pixels.
    pub width: u32,
    /// Height of the image in pixels.
    pub height: u32,
}

impl Screenshot {
    /// Crop a region from this screenshot based on screen coordinates.
    ///
    /// # Arguments
    /// * `bounds` - The bounds to crop, in screen coordinates
    /// * `screen_bounds` - The screen bounds that this screenshot represents
    ///
    /// # Returns
    /// A new Screenshot containing only the cropped region
    pub fn crop(&self, bounds: &Rect, screen_bounds: &Rect) -> anyhow::Result<Screenshot> {
        use image::ImageReader;
        use std::io::Cursor;

        // Calculate scale factor (screen coords -> pixels)
        let scale_x = self.width as f64 / screen_bounds.size.width;
        let scale_y = self.height as f64 / screen_bounds.size.height;

        // Convert bounds to pixel coordinates
        let px = ((bounds.origin.x - screen_bounds.origin.x) * scale_x).round() as u32;
        let py = ((bounds.origin.y - screen_bounds.origin.y) * scale_y).round() as u32;
        let pw = (bounds.size.width * scale_x).round() as u32;
        let ph = (bounds.size.height * scale_y).round() as u32;

        // Clamp to image bounds
        let px = px.min(self.width);
        let py = py.min(self.height);
        let pw = pw.min(self.width.saturating_sub(px));
        let ph = ph.min(self.height.saturating_sub(py));

        if pw == 0 || ph == 0 {
            anyhow::bail!("Crop region is empty or outside screenshot bounds");
        }

        // Decode PNG
        let img = ImageReader::new(Cursor::new(&self.data))
            .with_guessed_format()?
            .decode()?;

        // Crop the image
        let cropped = img.crop_imm(px, py, pw, ph);

        // Encode back to PNG
        let mut output = Cursor::new(Vec::new());
        cropped.write_to(&mut output, image::ImageFormat::Png)?;

        Ok(Screenshot {
            data: output.into_inner(),
            width: pw,
            height: ph,
        })
    }
}

/// An accessibility element with its properties.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Element {
    /// Unique identifier assigned during tree traversal.
    pub id: ElementKey,

    /// The role/type of this element (button, text field, etc.).
    pub role: Role,

    /// The element's title/label (e.g., button text).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Additional description for accessibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The current value (for text fields, sliders, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,

    /// The URL/href for link elements.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Help text for the element (AXHelp on macOS).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,

    /// Human-readable role description (AXRoleDescription on macOS).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role_description: Option<String>,

    /// Accessibility identifier (AXIdentifier on macOS).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,

    /// Screen bounds of the element.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Rect>,

    /// Whether the element is enabled for interaction.
    pub enabled: bool,

    /// Whether the element currently has keyboard focus.
    pub focused: bool,

    /// Available actions on this element.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,

    /// Child elements (for tree structure).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Element>,
}

impl Element {
    /// Create a new element with the given ID and role.
    pub fn new(id: ElementKey, role: Role) -> Self {
        Self {
            id,
            role,
            title: None,
            description: None,
            value: None,
            url: None,
            help: None,
            role_description: None,
            identifier: None,
            bounds: None,
            enabled: true,
            focused: false,
            actions: Vec::new(),
            children: Vec::new(),
        }
    }

    /// Get a display label for this element.
    ///
    /// Returns title, description, help, identifier, value, or role name in order of preference.
    /// Skips empty strings when looking for a label.
    pub fn display_label(&self) -> String {
        self.title
            .as_ref()
            .filter(|s| !s.is_empty())
            .or(self.description.as_ref().filter(|s| !s.is_empty()))
            .or(self.help.as_ref().filter(|s| !s.is_empty()))
            .or(self.identifier.as_ref().filter(|s| !s.is_empty()))
            .or(self.value.as_ref().filter(|s| !s.is_empty()))
            .cloned()
            .unwrap_or_else(|| format!("{:?}", self.role))
    }

    /// Check if this element is interactive (can receive actions).
    pub fn is_interactive(&self) -> bool {
        matches!(
            self.role,
            Role::Button
                | Role::Link
                | Role::TextInput
                | Role::MultilineTextInput
                | Role::CheckBox
                | Role::RadioButton
                | Role::ComboBox
                | Role::Slider
                | Role::Tab
                | Role::MenuItem
                | Role::MenuItemCheckBox
                | Role::MenuItemRadio
                | Role::Switch
                | Role::SpinButton
        ) || self.has_activation_action()
    }

    /// Check if this element exposes a platform activation action.
    pub fn has_activation_action(&self) -> bool {
        self.actions
            .iter()
            .any(|action| matches!(action.as_str(), "AXPress" | "AXPick" | "AXConfirm"))
    }

    /// Recursively find all elements matching a predicate.
    pub fn find_all<F>(&self, predicate: &F) -> Vec<&Element>
    where
        F: Fn(&Element) -> bool,
    {
        let mut results = Vec::new();
        let mut stack = vec![self];

        while let Some(element) = stack.pop() {
            if predicate(element) {
                results.push(element);
            }

            for child in element.children.iter().rev() {
                stack.push(child);
            }
        }

        results
    }
}

/// The complete accessibility tree with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementTree {
    /// Snapshot version (increments on each `clear_cache()`).
    pub version: u64,

    /// Process ID of the application (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,

    /// Application name (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,

    /// Root element of the tree.
    pub root: Element,

    /// Total number of elements in the tree.
    pub element_count: usize,
}

impl ElementTree {
    /// Find all elements matching a predicate.
    pub fn find_all<F>(&self, predicate: F) -> Vec<&Element>
    where
        F: Fn(&Element) -> bool,
    {
        self.root.find_all(&predicate)
    }
}

/// Filter options for accessibility tree traversal.
#[derive(Debug, Clone, Default)]
pub struct TreeFilter {
    /// Maximum depth to traverse (None = unlimited).
    pub max_depth: Option<usize>,

    /// Maximum number of elements to return (None = unlimited).
    pub max_elements: Option<usize>,

    /// Only include interactive elements (buttons, text fields, etc.).
    pub interactive_only: bool,

    /// Only include visible elements.
    pub visible_only: bool,

    /// Only include elements within these screen bounds.
    pub within_bounds: Option<Rect>,

    /// Filter to specific roles.
    pub roles: Option<Vec<Role>>,
}

impl TreeFilter {
    /// Create a filter for interactive elements only.
    pub fn interactive() -> Self {
        Self {
            interactive_only: true,
            ..Default::default()
        }
    }

    /// Create a filter with a maximum depth.
    pub fn with_max_depth(depth: usize) -> Self {
        Self {
            max_depth: Some(depth),
            ..Default::default()
        }
    }

    /// Create a filter with a maximum element count.
    pub fn with_max_elements(count: usize) -> Self {
        Self {
            max_elements: Some(count),
            ..Default::default()
        }
    }

    /// Check if an element should be included based on this filter.
    pub fn should_include(&self, element: &Element, depth: usize) -> bool {
        // Check depth
        if let Some(max_depth) = self.max_depth
            && depth > max_depth
        {
            return false;
        }

        // Check interactive only
        if self.interactive_only && !element.is_interactive() {
            return false;
        }

        // Check visible only (element must have bounds with non-zero area)
        if self.visible_only {
            match &element.bounds {
                Some(b) if b.size.width > 0.0 && b.size.height > 0.0 => {}
                _ => return false,
            }
        }

        // Check roles filter
        if let Some(ref roles) = self.roles
            && !roles.contains(&element.role)
        {
            return false;
        }

        // Check bounds filter
        if let Some(ref filter_bounds) = self.within_bounds
            && let Some(ref elem_bounds) = element.bounds
            && !filter_bounds.contains(elem_bounds.center())
        {
            return false;
        }

        true
    }
}

/// Accessibility event emitted when UI changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AccessibilityEvent {
    /// Focus changed to a new element.
    FocusChanged {
        element: Option<Element>,
        pid: Option<u32>,
        timestamp: u64,
    },
    /// Value of an element changed (text fields, sliders, etc.).
    ValueChanged {
        element: Option<Element>,
        old_value: Option<String>,
        new_value: Option<String>,
        timestamp: u64,
    },
    /// Title of an element changed.
    TitleChanged {
        element: Option<Element>,
        old_title: Option<String>,
        new_title: Option<String>,
        timestamp: u64,
    },
    /// Structure of the accessibility tree changed.
    StructureChanged {
        parent_element: Option<Element>,
        change_type: StructureChangeType,
        timestamp: u64,
    },
    /// A new window was created.
    WindowCreated {
        element: Option<Element>,
        pid: Option<u32>,
        timestamp: u64,
    },
    /// A window was destroyed.
    WindowDestroyed {
        window_id: Option<String>,
        pid: Option<u32>,
        timestamp: u64,
    },
    /// Window focus changed.
    WindowFocusChanged {
        element: Option<Element>,
        pid: Option<u32>,
        timestamp: u64,
    },
    /// Selected text changed in an element.
    SelectedTextChanged {
        element: Option<Element>,
        selected_text: Option<String>,
        timestamp: u64,
    },
    /// An element was destroyed.
    ElementDestroyed {
        element_id: Option<ElementKey>,
        timestamp: u64,
    },
    /// An error occurred during event listening.
    Error { message: String, timestamp: u64 },
    /// Event listening was stopped.
    Stopped { reason: StopReason, timestamp: u64 },
}

/// Type of structure change in the accessibility tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StructureChangeType {
    /// Children were added to a parent element.
    ChildrenAdded,
    /// Children were removed from a parent element.
    ChildrenRemoved,
    /// Children were reordered within a parent element.
    ChildrenReordered,
    /// The subtree was invalidated and needs to be re-queried.
    Invalidated,
}

/// Reason why event listening was stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    /// Stopped by user request via ListenerHandle::stop().
    UserRequested,
    /// The target process terminated.
    ProcessTerminated,
    /// Connection to the accessibility system was lost.
    ConnectionLost,
    /// Permission to access accessibility was denied.
    PermissionDenied,
}

/// Types of accessibility events that can be subscribed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccessibilityEventType {
    FocusChanged,
    ValueChanged,
    TitleChanged,
    StructureChanged,
    WindowCreated,
    WindowDestroyed,
    WindowFocusChanged,
    SelectedTextChanged,
    ElementDestroyed,
}

/// Configuration for event listening.
#[derive(Debug, Clone)]
pub struct ListenerConfig {
    /// Event types to subscribe to. `None` means all events.
    pub event_types: Option<Vec<AccessibilityEventType>>,
    /// Target PID for raw listeners. `TargetedAccessibility` fills this from
    /// its stored PID when omitted.
    pub pid: Option<u32>,
    /// Size of the event channel buffer. Default: 256.
    pub buffer_size: usize,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            event_types: None,
            pid: None,
            buffer_size: 256,
        }
    }
}

impl ListenerConfig {
    /// Create a new config with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe to specific event types only.
    pub fn with_event_types(mut self, types: Vec<AccessibilityEventType>) -> Self {
        self.event_types = Some(types);
        self
    }

    /// Target a specific process by PID.
    pub fn with_pid(mut self, pid: u32) -> Self {
        self.pid = Some(pid);
        self
    }

    /// Set the channel buffer size.
    pub fn with_buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size;
        self
    }

    /// Check if an event type should be captured.
    pub fn should_capture(&self, event_type: AccessibilityEventType) -> bool {
        match &self.event_types {
            Some(types) => types.contains(&event_type),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use accesskit::Role;

    #[test]
    fn action_bearing_group_is_interactive() {
        let mut element = Element::new(ElementKey::from_ffi(1), Role::Group);
        element.actions.push("AXPress".to_string());

        assert!(element.is_interactive());
    }

    #[test]
    fn menu_only_group_is_not_interactive() {
        let mut element = Element::new(ElementKey::from_ffi(1), Role::Group);
        element.actions.push("AXShowMenu".to_string());
        element.actions.push("AXScrollToVisible".to_string());

        assert!(!element.is_interactive());
    }
}
