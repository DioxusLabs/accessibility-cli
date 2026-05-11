//! macOS accessibility implementation using AXUIElement API.
//!
//! This module provides access to the macOS accessibility tree for reading
//! UI element information and performing actions.

// Rust 2024 requires unsafe blocks inside unsafe fns, but objc2 code uses many unsafe calls
#![allow(unsafe_op_in_unsafe_fn, dead_code)]

use crate::accessibility::{
    AccessibilityEvent, AccessibilityEventType, AccessibilityReader, Element, ElementCache,
    ElementKey, ElementTree, ListenerConfig, ListenerHandle, Point, Rect, Screenshot, StopReason,
    TreeFilter,
};
use crate::input::code_from_char;
use accesskit::{Action, Role};
use anyhow::{Result, anyhow, bail};
use keyboard_types::{Code, Modifiers};
use objc2::{AnyThread, runtime::AnyObject};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSBitmapImageRepPropertyKey};
use objc2_application_services::{AXError, AXIsProcessTrusted, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{CFArray, CFRetained, CFString, CFType, CGRect};
use objc2_core_graphics::{
    CGDisplayBounds, CGEvent, CGEventField, CGEventFlags, CGEventType, CGImage,
    CGMainDisplayID, CGMouseButton, CGScrollEventUnit, CGWindowID, CGWindowImageOption,
    CGWindowListOption,
};
use objc2_foundation::NSDictionary;
use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

type SLEventPostToPidFn = unsafe extern "C-unwind" fn(libc::pid_t, Option<&CGEvent>);
type AXUIElementGetWindowFn = unsafe extern "C-unwind" fn(&AXUIElement, *mut CGWindowID) -> AXError;

fn dlerror_message() -> String {
    unsafe {
        let error = libc::dlerror();
        if error.is_null() {
            "unknown dynamic loader error".to_string()
        } else {
            CStr::from_ptr(error).to_string_lossy().into_owned()
        }
    }
}

fn skylight_handle() -> Option<*mut c_void> {
    static HANDLE: OnceLock<Option<usize>> = OnceLock::new();

    HANDLE
        .get_or_init(|| unsafe {
            let path = b"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight\0";
            let handle = libc::dlopen(
                path.as_ptr() as *const c_char,
                libc::RTLD_NOW | libc::RTLD_GLOBAL,
            );
            if handle.is_null() {
                let _ = dlerror_message();
                None
            } else {
                Some(handle as usize)
            }
        })
        .map(|handle| handle as *mut c_void)
}

fn skylight_event_post_to_pid() -> Option<SLEventPostToPidFn> {
    static SYMBOL: OnceLock<Option<SLEventPostToPidFn>> = OnceLock::new();

    *SYMBOL.get_or_init(|| unsafe {
        let handle = skylight_handle()?;
        let symbol = libc::dlsym(handle, c"SLEventPostToPid".as_ptr());
        if symbol.is_null() {
            let _ = dlerror_message();
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, SLEventPostToPidFn>(
                symbol,
            ))
        }
    })
}

fn ax_ui_element_get_window() -> Option<AXUIElementGetWindowFn> {
    static SYMBOL: OnceLock<Option<AXUIElementGetWindowFn>> = OnceLock::new();

    *SYMBOL.get_or_init(|| unsafe {
        let symbol = libc::dlsym(libc::RTLD_DEFAULT, c"_AXUIElementGetWindow".as_ptr());
        if symbol.is_null() {
            let _ = dlerror_message();
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, AXUIElementGetWindowFn>(
                symbol,
            ))
        }
    })
}

/// macOS accessibility reader using AXUIElement API.
pub struct MacOSAccessibility {
    /// Cache of elements with their platform handles.
    cache: ElementCache,

    /// Map from ElementKey to AXUIElement handle for performing actions.
    handles: HashMap<ElementKey, CFRetained<AXUIElement>>,

    /// PID from the most recent tree build, used to keep cached actions targeted.
    last_tree_pid: Option<u32>,

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
            last_tree_pid: None,
            system_wide,
        })
    }

    /// Check if the process has accessibility permissions.
    pub fn is_process_trusted() -> bool {
        // Safety: AXIsProcessTrusted is a safe C function
        unsafe { AXIsProcessTrusted() }
    }

    /// Return the main display's bounds in global screen coordinates.
    fn main_display_bounds() -> Rect {
        let bounds = CGDisplayBounds(CGMainDisplayID());
        Rect::new(
            Point::new(bounds.origin.x, bounds.origin.y),
            crate::accessibility::Size::new(bounds.size.width, bounds.size.height),
        )
    }

    /// Capture the main display and encode it as PNG.
    fn capture_main_display() -> Result<Screenshot> {
        #[allow(deprecated)]
        let image = objc2_core_graphics::CGDisplayCreateImage(CGMainDisplayID())
            .ok_or_else(|| anyhow!("Failed to capture main display"))?;

        Self::encode_cg_image_as_png(&image)
    }

    /// Convert a CoreGraphics image into the Screenshot format used by the public API.
    fn encode_cg_image_as_png(image: &CGImage) -> Result<Screenshot> {
        let width = CGImage::width(Some(image)) as u32;
        let height = CGImage::height(Some(image)) as u32;
        if width == 0 || height == 0 {
            bail!("Captured image has empty dimensions: {}x{}", width, height);
        }

        let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
        let properties = NSDictionary::<NSBitmapImageRepPropertyKey, AnyObject>::new();
        let data = unsafe {
            bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
        }
        .ok_or_else(|| anyhow!("Failed to encode screenshot as PNG"))?;

        let len = data.length();
        if len == 0 {
            bail!("Encoded screenshot is empty");
        }

        let mut bytes = vec![0; len];
        unsafe {
            data.getBytes_length(
                NonNull::new(bytes.as_mut_ptr().cast::<c_void>())
                    .expect("Vec pointer should be non-null"),
                len,
            );
        }

        Ok(Screenshot {
            data: bytes,
            width,
            height,
        })
    }

    /// Current timestamp in milliseconds since the Unix epoch.
    fn timestamp_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    /// Map keyboard-types codes to macOS virtual key codes.
    fn key_code(code: Code) -> Option<u16> {
        match code {
            Code::KeyA => Some(0),
            Code::KeyS => Some(1),
            Code::KeyD => Some(2),
            Code::KeyF => Some(3),
            Code::KeyH => Some(4),
            Code::KeyG => Some(5),
            Code::KeyZ => Some(6),
            Code::KeyX => Some(7),
            Code::KeyC => Some(8),
            Code::KeyV => Some(9),
            Code::KeyB => Some(11),
            Code::KeyQ => Some(12),
            Code::KeyW => Some(13),
            Code::KeyE => Some(14),
            Code::KeyR => Some(15),
            Code::KeyY => Some(16),
            Code::KeyT => Some(17),
            Code::Digit1 => Some(18),
            Code::Digit2 => Some(19),
            Code::Digit3 => Some(20),
            Code::Digit4 => Some(21),
            Code::Digit6 => Some(22),
            Code::Digit5 => Some(23),
            Code::Equal => Some(24),
            Code::Digit9 => Some(25),
            Code::Digit7 => Some(26),
            Code::Minus => Some(27),
            Code::Digit8 => Some(28),
            Code::Digit0 => Some(29),
            Code::BracketRight => Some(30),
            Code::KeyO => Some(31),
            Code::KeyU => Some(32),
            Code::BracketLeft => Some(33),
            Code::KeyI => Some(34),
            Code::KeyP => Some(35),
            Code::Enter | Code::NumpadEnter => Some(36),
            Code::KeyL => Some(37),
            Code::KeyJ => Some(38),
            Code::Quote => Some(39),
            Code::KeyK => Some(40),
            Code::Semicolon => Some(41),
            Code::Backslash => Some(42),
            Code::Comma => Some(43),
            Code::Slash => Some(44),
            Code::KeyN => Some(45),
            Code::KeyM => Some(46),
            Code::Period => Some(47),
            Code::Tab => Some(48),
            Code::Space => Some(49),
            Code::Backquote => Some(50),
            Code::Backspace => Some(51),
            Code::Escape => Some(53),
            Code::MetaLeft | Code::MetaRight => Some(55),
            Code::ShiftLeft => Some(56),
            Code::CapsLock => Some(57),
            Code::AltLeft => Some(58),
            Code::ControlLeft => Some(59),
            Code::ShiftRight => Some(60),
            Code::AltRight => Some(61),
            Code::ControlRight => Some(62),
            Code::NumpadDecimal => Some(65),
            Code::NumpadMultiply => Some(67),
            Code::NumpadAdd => Some(69),
            Code::NumLock => Some(71),
            Code::NumpadDivide => Some(75),
            Code::NumpadSubtract => Some(78),
            Code::Numpad0 => Some(82),
            Code::Numpad1 => Some(83),
            Code::Numpad2 => Some(84),
            Code::Numpad3 => Some(85),
            Code::Numpad4 => Some(86),
            Code::Numpad5 => Some(87),
            Code::Numpad6 => Some(88),
            Code::Numpad7 => Some(89),
            Code::Numpad8 => Some(91),
            Code::Numpad9 => Some(92),
            Code::F5 => Some(96),
            Code::F6 => Some(97),
            Code::F7 => Some(98),
            Code::F3 => Some(99),
            Code::F8 => Some(100),
            Code::F9 => Some(101),
            Code::F11 => Some(103),
            Code::F13 => Some(105),
            Code::F16 => Some(106),
            Code::F14 => Some(107),
            Code::F10 => Some(109),
            Code::F12 => Some(111),
            Code::F15 => Some(113),
            Code::Insert => Some(114),
            Code::Home => Some(115),
            Code::PageUp => Some(116),
            Code::Delete => Some(117),
            Code::F4 => Some(118),
            Code::End => Some(119),
            Code::F2 => Some(120),
            Code::PageDown => Some(121),
            Code::F1 => Some(122),
            Code::ArrowLeft => Some(123),
            Code::ArrowRight => Some(124),
            Code::ArrowDown => Some(125),
            Code::ArrowUp => Some(126),
            _ => None,
        }
    }

    fn modifier_flags(modifiers: Modifiers) -> CGEventFlags {
        let mut flags = CGEventFlags::empty();
        if modifiers.contains(Modifiers::SHIFT) {
            flags |= CGEventFlags::MaskShift;
        }
        if modifiers.contains(Modifiers::CONTROL) {
            flags |= CGEventFlags::MaskControl;
        }
        if modifiers.contains(Modifiers::ALT) {
            flags |= CGEventFlags::MaskAlternate;
        }
        if modifiers.contains(Modifiers::META) {
            flags |= CGEventFlags::MaskCommand;
        }
        flags
    }

    fn set_event_target_pid(event: &CGEvent, pid: u32) {
        CGEvent::set_integer_value_field(
            Some(event),
            CGEventField::EventTargetUnixProcessID,
            pid as i64,
        );
    }

    /// Deliver a synthetic CGEvent to a specific process via SkyLight.
    ///
    /// SkyLight per-PID delivery is the only public-ish path that doesn't
    /// steal focus. The public CGEvent post APIs silently activate the
    /// target, so falling back to them would mask focus-stealing regressions
    /// — we bail instead. Callers must pass a concrete pid; global delivery
    /// isn't supported here.
    fn post_event(pid: Option<u32>, event: &CGEvent) -> Result<()> {
        let pid = pid.ok_or_else(|| {
            anyhow!("post_event requires a target pid on macOS (SkyLight has no global path)")
        })?;
        if !Self::post_event_to_pid_via_skylight(pid, event) {
            bail!("SkyLight SLEventPostToPid is unavailable; refusing to fall back to a focus-stealing post");
        }
        Ok(())
    }

    fn post_event_to_pid_via_skylight(pid: u32, event: &CGEvent) -> bool {
        let Some(post_to_pid) = skylight_event_post_to_pid() else {
            return false;
        };

        Self::set_event_target_pid(event, pid);
        unsafe {
            post_to_pid(pid as libc::pid_t, Some(event));
        }
        true
    }

    fn post_key_event(
        pid: Option<u32>,
        code: Code,
        modifiers: Modifiers,
        key_down: bool,
    ) -> Result<()> {
        let key_code = Self::key_code(code)
            .ok_or_else(|| anyhow!("Key {:?} is not supported on macOS", code))?;
        let event = CGEvent::new_keyboard_event(None, key_code, key_down)
            .ok_or_else(|| anyhow!("Failed to create keyboard event"))?;
        CGEvent::set_flags(Some(&event), Self::modifier_flags(modifiers));

        // Even with SkyLight per-PID delivery, AppKit-based apps drop key
        // events that arrive while they are not frontmost — that's an
        // OS-level policy we can't override. Callers driving a backgrounded
        // app should invoke the equivalent action (e.g. click the Equals
        // button) rather than send a key like Return.
        Self::post_event(pid, &event)
    }

    fn post_keystroke(pid: Option<u32>, code: Code, modifiers: Modifiers) -> Result<()> {
        Self::post_key_event(pid, code, modifiers, true)?;
        std::thread::sleep(Duration::from_millis(10));
        Self::post_key_event(pid, code, modifiers, false)
    }

    fn cg_mouse_button(button: crate::input::MouseButton) -> CGMouseButton {
        match button {
            crate::input::MouseButton::Left => CGMouseButton::Left,
            crate::input::MouseButton::Right => CGMouseButton::Right,
            crate::input::MouseButton::Middle => CGMouseButton::Center,
        }
    }

    fn mouse_event_types(button: crate::input::MouseButton) -> (CGEventType, CGEventType) {
        match button {
            crate::input::MouseButton::Left => {
                (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp)
            }
            crate::input::MouseButton::Right => {
                (CGEventType::RightMouseDown, CGEventType::RightMouseUp)
            }
            crate::input::MouseButton::Middle => {
                (CGEventType::OtherMouseDown, CGEventType::OtherMouseUp)
            }
        }
    }

    fn mouse_button_number(button: crate::input::MouseButton) -> i64 {
        match button {
            crate::input::MouseButton::Left => 0,
            crate::input::MouseButton::Right => 1,
            crate::input::MouseButton::Middle => 2,
        }
    }

    fn configure_mouse_event(
        event: &CGEvent,
        pid: Option<u32>,
        button: crate::input::MouseButton,
        click_state: i64,
        pressure: f64,
    ) {
        if let Some(pid) = pid {
            Self::set_event_target_pid(event, pid);
            if let Some(window_id) = unsafe { Self::get_window_id_for_pid(pid) } {
                CGEvent::set_integer_value_field(
                    Some(event),
                    CGEventField::MouseEventWindowUnderMousePointer,
                    window_id as i64,
                );
                CGEvent::set_integer_value_field(
                    Some(event),
                    CGEventField::MouseEventWindowUnderMousePointerThatCanHandleThisEvent,
                    window_id as i64,
                );
            }
        }
        CGEvent::set_integer_value_field(
            Some(event),
            CGEventField::MouseEventButtonNumber,
            Self::mouse_button_number(button),
        );
        CGEvent::set_integer_value_field(
            Some(event),
            CGEventField::MouseEventClickState,
            click_state,
        );
        CGEvent::set_integer_value_field(Some(event), CGEventField::MouseEventSubtype, 0);
        CGEvent::set_double_value_field(Some(event), CGEventField::MouseEventPressure, pressure);
    }

    #[allow(clippy::too_many_arguments)]
    fn post_mouse_event(
        pid: Option<u32>,
        x: f64,
        y: f64,
        event_type: CGEventType,
        button: CGMouseButton,
        input_button: crate::input::MouseButton,
        click_state: i64,
        pressure: f64,
    ) -> Result<()> {
        let point = objc2_core_foundation::CGPoint { x, y };
        let event = CGEvent::new_mouse_event(None, event_type, point, button)
            .ok_or_else(|| anyhow!("Failed to create mouse event"))?;
        Self::configure_mouse_event(&event, pid, input_button, click_state, pressure);
        Self::post_event(pid, &event)
    }

    fn post_chromium_activation_primer(pid: Option<u32>) -> Result<()> {
        if pid.is_none() {
            return Ok(());
        }

        Self::post_mouse_event(
            pid,
            -1.0,
            -1.0,
            CGEventType::LeftMouseDown,
            CGMouseButton::Left,
            crate::input::MouseButton::Left,
            1,
            1.0,
        )?;
        std::thread::sleep(Duration::from_millis(2));
        Self::post_mouse_event(
            pid,
            -1.0,
            -1.0,
            CGEventType::LeftMouseUp,
            CGMouseButton::Left,
            crate::input::MouseButton::Left,
            1,
            0.0,
        )?;
        std::thread::sleep(Duration::from_millis(2));
        Ok(())
    }

    fn post_mouse_click_sequence(
        pid: Option<u32>,
        x: f64,
        y: f64,
        button: crate::input::MouseButton,
        click_state: i64,
    ) -> Result<()> {
        if pid.is_some() && button == crate::input::MouseButton::Left && click_state == 1 {
            Self::post_chromium_activation_primer(pid)?;
        }

        let cg_button = Self::cg_mouse_button(button);
        let (down_type, up_type) = Self::mouse_event_types(button);
        Self::post_mouse_event(pid, x, y, down_type, cg_button, button, click_state, 1.0)?;
        std::thread::sleep(Duration::from_millis(10));
        Self::post_mouse_event(pid, x, y, up_type, cg_button, button, click_state, 0.0)
    }

    fn current_mouse_location() -> Result<objc2_core_foundation::CGPoint> {
        let event =
            CGEvent::new(None).ok_or_else(|| anyhow!("Failed to read current mouse location"))?;
        Ok(CGEvent::location(Some(&event)))
    }

    unsafe fn get_pid_for_element(element: &AXUIElement) -> Option<u32> {
        let mut pid: libc::pid_t = 0;
        let pid_ptr = NonNull::new(&mut pid as *mut libc::pid_t).unwrap();
        let result = element.pid(pid_ptr);
        if result == AXError::Success && pid > 0 {
            Some(pid as u32)
        } else {
            None
        }
    }

    fn flatten_elements(element: &Element, elements: &mut Vec<Element>) {
        elements.push(element.clone());
        for child in &element.children {
            Self::flatten_elements(child, elements);
        }
    }

    fn element_event_key(element: &Element) -> String {
        let bounds = element.bounds.map(|bounds| {
            (
                bounds.origin.x.round() as i64,
                bounds.origin.y.round() as i64,
                bounds.size.width.round() as i64,
                bounds.size.height.round() as i64,
            )
        });

        format!(
            "{:?}|{:?}|{:?}|{:?}|{:?}",
            element.role, element.title, element.description, element.identifier, bounds
        )
    }

    fn listener_snapshots(
        tree: &ElementTree,
    ) -> (HashMap<String, Element>, Option<(String, Element)>) {
        let mut elements = Vec::new();
        Self::flatten_elements(&tree.root, &mut elements);

        let mut values = HashMap::new();
        let mut focused = None;
        for element in elements {
            let key = Self::element_event_key(&element);
            if element.value.is_some() {
                values.insert(key.clone(), element.clone());
            }
            if focused.is_none() && element.focused {
                focused = Some((key, element));
            }
        }

        (values, focused)
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

    /// Get the windows of an application element.
    ///
    /// For a non-frontmost application, `AXChildren` typically omits the visible
    /// windows. Empirically on macOS, `AXWindows` is *also* often empty for
    /// backgrounded apps, but `AXMainWindow` still returns the focused window;
    /// we use both so single-window apps still walk correctly when backgrounded.
    /// The returned list is deduped by window title — macOS hands out fresh
    /// `AXUIElement` wrappers per call so raw-pointer dedup doesn't work.
    unsafe fn get_application_windows(element: &AXUIElement) -> Vec<CFRetained<AXUIElement>> {
        let mut windows: Vec<CFRetained<AXUIElement>> = Vec::new();
        let mut seen_titles: std::collections::HashSet<String> = std::collections::HashSet::new();

        let push = |w: CFRetained<AXUIElement>,
                        windows: &mut Vec<CFRetained<AXUIElement>>,
                        seen: &mut std::collections::HashSet<String>| {
            let title =
                unsafe { Self::get_string_attribute(&w, AX_TITLE) }.unwrap_or_default();
            if title.is_empty() || seen.insert(title) {
                windows.push(w);
            }
        };

        if let Ok(value) = unsafe { Self::get_attribute(element, AX_WINDOWS) } {
            let array: CFRetained<CFArray<AXUIElement>> =
                unsafe { CFRetained::cast_unchecked(value) };
            for i in 0..array.len() {
                if let Some(w) = array.get(i) {
                    push(w, &mut windows, &mut seen_titles);
                }
            }
        }

        if let Ok(value) = unsafe { Self::get_attribute(element, AX_MAIN_WINDOW) } {
            let w: CFRetained<AXUIElement> = unsafe { CFRetained::cast_unchecked(value) };
            push(w, &mut windows, &mut seen_titles);
        }

        windows
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

        let self_matches = filter.should_include(&element, depth);

        // Process children (subject to max_depth). We always recurse so that filters
        // like --interactive / --visible don't prune containers whose descendants do
        // match; the container is included below if any child survived.
        let should_recurse = filter.max_depth.is_none_or(|max| depth < max);
        if should_recurse {
            let mut children = Self::get_children(ax_element);

            // For backgrounded apps, AXChildren of the Application typically omits
            // visible windows; AXWindows still returns them. Fall back to AXWindows
            // only when AXChildren produced no Window-role child, since macOS hands
            // out fresh AXUIElement wrappers per call (no cheap pointer dedup) and
            // we want to avoid double-walking the same window.
            if role == Role::Application {
                let has_window_child = children.iter().any(|c| {
                    Self::get_string_attribute(c, AX_ROLE)
                        .map(|r| r == ROLE_WINDOW)
                        .unwrap_or(false)
                });
                if !has_window_child {
                    for window in unsafe { Self::get_application_windows(ax_element) } {
                        children.push(window);
                    }
                }
            }

            for child in children {
                if let Some(child_element) =
                    self.build_element(&child, filter, depth + 1, element_count)
                {
                    element.children.push(child_element);
                }
            }
        }

        // Include this element if it matches the filter itself, has any kept
        // descendants (so we don't drop containers), or is the root (so get_tree
        // always has something to return).
        if !self_matches && element.children.is_empty() && depth != 0 {
            return None;
        }

        // Store handle for actions - convert reference to NonNull for retain
        self.handles
            .insert(id, unsafe { CFRetained::retain(ax_element.into()) });

        // Store in cache
        #[allow(deprecated)]
        self.cache.store_with_id(id, element.clone());
        *element_count += 1;

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

    /// Get the main window for a given PID using accessibility APIs.
    unsafe fn get_window_for_pid(pid: u32) -> Option<CFRetained<AXUIElement>> {
        let app = AXUIElement::new_application(pid as i32);

        if let Ok(main_window) = Self::get_attribute(&app, AX_MAIN_WINDOW) {
            let window: CFRetained<AXUIElement> = CFRetained::cast_unchecked(main_window);
            if let Some(bounds) = Self::get_bounds(&window)
                && bounds.size.width > 0.0
                && bounds.size.height > 0.0
            {
                return Some(window);
            }
        }

        if let Ok(windows_attr) = Self::get_attribute(&app, AX_WINDOWS) {
            let windows: CFRetained<CFArray<AXUIElement>> =
                CFRetained::cast_unchecked(windows_attr);
            for i in 0..windows.len() {
                if let Some(window) = windows.get(i)
                    && let Some(bounds) = Self::get_bounds(&window)
                    && bounds.size.width > 0.0
                    && bounds.size.height > 0.0
                {
                    return Some(window);
                }
            }
        }

        None
    }

    /// Get the window title for a given PID using accessibility APIs.
    unsafe fn get_window_title_for_pid(pid: u32) -> Option<String> {
        let window = Self::get_window_for_pid(pid)?;
        Self::get_string_attribute(&window, AX_TITLE).filter(|title| !title.is_empty())
    }

    /// Get the main window bounds for a given PID using accessibility APIs.
    unsafe fn get_window_bounds_for_pid(pid: u32) -> Option<Rect> {
        let window = Self::get_window_for_pid(pid)?;
        Self::get_bounds(&window)
            .filter(|bounds| bounds.size.width > 0.0 && bounds.size.height > 0.0)
    }

    /// Resolve an AX window to its WindowServer ID using private AX SPI.
    unsafe fn get_window_id(window: &AXUIElement) -> Option<CGWindowID> {
        let get_window = ax_ui_element_get_window()?;
        let mut window_id: CGWindowID = 0;
        let result = get_window(window, &mut window_id);
        if result == AXError::Success && window_id != 0 {
            Some(window_id)
        } else {
            None
        }
    }

    unsafe fn get_window_id_for_pid(pid: u32) -> Option<CGWindowID> {
        let window = Self::get_window_for_pid(pid)?;
        Self::get_window_id(&window)
    }

    /// Capture a target window through WindowServer so occluding windows are not included.
    fn capture_window_for_pid(pid: u32) -> Result<Option<Screenshot>> {
        let window = unsafe { Self::get_window_for_pid(pid) };
        let Some(window_id) = window
            .as_deref()
            .and_then(|window| unsafe { Self::get_window_id(window) })
        else {
            return Ok(None);
        };

        #[allow(deprecated)]
        let image = objc2_core_graphics::CGWindowListCreateImage(
            CGRect::ZERO,
            CGWindowListOption::OptionIncludingWindow,
            window_id,
            CGWindowImageOption::BoundsIgnoreFraming | CGWindowImageOption::BestResolution,
        );

        image
            .as_deref()
            .map(Self::encode_cg_image_as_png)
            .transpose()
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
    fn platform_name(&self) -> &'static str {
        "macOS"
    }

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
            self.last_tree_pid = Some(actual_pid);

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

            // Focus/Blur aren't AX actions on macOS — they're attribute writes.
            if matches!(action, Action::Focus | Action::Blur) {
                let want_focus = matches!(action, Action::Focus);
                unsafe {
                    let attr = CFString::from_str(AX_FOCUSED);
                    let value: &CFType = if want_focus {
                        objc2_core_foundation::kCFBooleanTrue
                            .ok_or_else(|| anyhow!("kCFBooleanTrue unavailable"))?
                            .as_ref()
                    } else {
                        objc2_core_foundation::kCFBooleanFalse
                            .ok_or_else(|| anyhow!("kCFBooleanFalse unavailable"))?
                            .as_ref()
                    };
                    let result = handle.set_attribute_value(&attr, value);
                    if result != AXError::Success {
                        // -25201 (IllegalArgument) and -25205 (AttributeUnsupported) both mean
                        // "this element won't accept the focus write" — usually because the
                        // platform routes blur through a different mechanism (e.g. AppKit
                        // collapses focus when another window becomes key).
                        let verb = if want_focus { "focus" } else { "blur" };
                        bail!(
                            "this element does not support programmatic {} on macOS ({:?})",
                            verb,
                            result
                        );
                    }
                }
                return Ok(());
            }

            // AXPress on a menu goes through AppKit's menu-tracking path and
            // promotes the owning app to key. Deliver a synthetic mouse click
            // via the SkyLight per-PID path instead, which keeps focus put.
            if matches!(action, Action::Click)
                && let Some(element) = self.cache.get(id)
                && matches!(
                    element.role,
                    Role::Menu | Role::MenuItem | Role::MenuBar
                )
                && let Some(bounds) = element.bounds
                && let Some(pid) = unsafe { Self::get_pid_for_element(handle) }
            {
                let x = bounds.origin.x + bounds.size.width / 2.0;
                let y = bounds.origin.y + bounds.size.height / 2.0;
                return Self::post_mouse_click_sequence(
                    Some(pid),
                    x,
                    y,
                    crate::input::MouseButton::Left,
                    1,
                );
            }

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
        self.last_tree_pid = None;
    }

    fn snapshot_version(&self) -> u64 {
        self.cache.version()
    }

    fn keystroke(
        &mut self,
        pid: Option<u32>,
        key: Code,
        modifiers: Modifiers,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result = Self::post_keystroke(pid, key, modifiers);
        std::future::ready(result)
    }

    fn type_raw(
        &mut self,
        pid: Option<u32>,
        text: &str,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result = (|| {
            for ch in text.chars() {
                let (code, needs_shift) = code_from_char(ch)
                    .ok_or_else(|| anyhow!("Character {:?} is not supported on macOS", ch))?;
                let modifiers = if needs_shift {
                    Modifiers::SHIFT
                } else {
                    Modifiers::empty()
                };
                Self::post_keystroke(pid, code, modifiers)?;
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        })();

        std::future::ready(result)
    }

    fn mouse_click_at(
        &mut self,
        pid: Option<u32>,
        x: f64,
        y: f64,
        button: crate::input::MouseButton,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result = Self::post_mouse_click_sequence(pid, x, y, button, 1);

        std::future::ready(result)
    }

    fn press_key(
        &mut self,
        pid: Option<u32>,
        key: Code,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result = Self::post_key_event(pid, key, Modifiers::empty(), true);

        std::future::ready(result)
    }

    fn release_key(
        &mut self,
        pid: Option<u32>,
        key: Code,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result = Self::post_key_event(pid, key, Modifiers::empty(), false);

        std::future::ready(result)
    }

    fn mouse_move(
        &mut self,
        pid: Option<u32>,
        x: f64,
        y: f64,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result = Self::post_mouse_event(
            pid,
            x,
            y,
            CGEventType::MouseMoved,
            CGMouseButton::Left,
            crate::input::MouseButton::Left,
            0,
            0.0,
        );

        std::future::ready(result)
    }

    fn mouse_click(
        &mut self,
        pid: Option<u32>,
        button: crate::input::MouseButton,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result = (|| {
            let point = Self::current_mouse_location()?;
            Self::post_mouse_click_sequence(pid, point.x, point.y, button, 1)
        })();

        std::future::ready(result)
    }

    fn mouse_double_click(
        &mut self,
        pid: Option<u32>,
        button: crate::input::MouseButton,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result = (|| {
            let point = Self::current_mouse_location()?;
            Self::post_mouse_click_sequence(pid, point.x, point.y, button, 1)?;
            std::thread::sleep(Duration::from_millis(40));
            Self::post_mouse_click_sequence(pid, point.x, point.y, button, 2)
        })();

        std::future::ready(result)
    }

    fn mouse_scroll(
        &mut self,
        pid: Option<u32>,
        delta_x: f64,
        delta_y: f64,
    ) -> impl std::future::Future<Output = Result<()>> {
        let result = (|| {
            let event = CGEvent::new_scroll_wheel_event2(
                None,
                CGScrollEventUnit::Pixel,
                2,
                delta_y.round() as i32,
                delta_x.round() as i32,
                0,
            )
            .ok_or_else(|| anyhow!("Failed to create scroll event"))?;
            Self::post_event(pid, &event)
        })();

        std::future::ready(result)
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

    fn capture_screen(&self, pid: Option<u32>) -> Result<Screenshot> {
        if let Some(pid) = pid
            && let Ok(Some(screenshot)) = Self::capture_window_for_pid(pid)
        {
            return Ok(screenshot);
        }

        let screenshot = Self::capture_main_display()?;

        if let Some(pid) = pid
            && let Some(window_bounds) = unsafe { Self::get_window_bounds_for_pid(pid) }
        {
            let screen_bounds = Self::main_display_bounds();
            if let Ok(cropped) = screenshot.crop(&window_bounds, &screen_bounds) {
                return Ok(cropped);
            }
        }

        Ok(screenshot)
    }

    fn get_screen_bounds(
        &self,
        pid: Option<u32>,
    ) -> impl std::future::Future<Output = Result<Rect>> {
        let bounds = pid
            .and_then(|pid| unsafe { Self::get_window_bounds_for_pid(pid) })
            .unwrap_or_else(Self::main_display_bounds);

        std::future::ready(Ok(bounds))
    }

    fn start_listening(
        &mut self,
        config: ListenerConfig,
        callback: Box<dyn FnMut(AccessibilityEvent) + Send + 'static>,
    ) -> Result<ListenerHandle> {
        let pid = config.pid;
        let stop_flag = Arc::new(AtomicBool::new(false));
        let task_stop_flag = stop_flag.clone();

        let runtime_handle = tokio::runtime::Handle::current();
        let task_handle = tokio::task::spawn_blocking(move || {
            let mut callback = callback;
            let mut reader = match MacOSAccessibility::new() {
                Ok(reader) => reader,
                Err(error) => {
                    callback(AccessibilityEvent::Error {
                        message: error.to_string(),
                        timestamp: MacOSAccessibility::timestamp_ms(),
                    });
                    return;
                }
            };

            let mut previous_values: HashMap<String, Element> = HashMap::new();
            let mut previous_focus: Option<String> = None;
            let mut first_snapshot = true;

            while !task_stop_flag.load(Ordering::SeqCst) {
                match runtime_handle.block_on(reader.get_tree(pid, &TreeFilter::default())) {
                    Ok(tree) => {
                        let (values, focused) = MacOSAccessibility::listener_snapshots(&tree);

                        if config.should_capture(AccessibilityEventType::FocusChanged)
                            && let Some((focus_key, element)) = focused
                            && (first_snapshot || previous_focus.as_deref() != Some(&focus_key))
                        {
                            previous_focus = Some(focus_key);
                            callback(AccessibilityEvent::FocusChanged {
                                element: Some(element),
                                pid: tree.pid,
                                timestamp: MacOSAccessibility::timestamp_ms(),
                            });
                        }

                        if config.should_capture(AccessibilityEventType::ValueChanged) {
                            for (key, element) in &values {
                                let old_value =
                                    previous_values.get(key).and_then(|e| e.value.clone());
                                let new_value = element.value.clone();
                                if first_snapshot || old_value != new_value {
                                    callback(AccessibilityEvent::ValueChanged {
                                        element: Some(element.clone()),
                                        old_value,
                                        new_value,
                                        timestamp: MacOSAccessibility::timestamp_ms(),
                                    });
                                }
                            }
                        }

                        previous_values = values;
                        first_snapshot = false;
                    }
                    Err(error) => {
                        callback(AccessibilityEvent::Error {
                            message: error.to_string(),
                            timestamp: MacOSAccessibility::timestamp_ms(),
                        });
                    }
                }

                std::thread::sleep(Duration::from_millis(100));
            }

            callback(AccessibilityEvent::Stopped {
                reason: StopReason::UserRequested,
                timestamp: MacOSAccessibility::timestamp_ms(),
            });
        });

        Ok(ListenerHandle::new(stop_flag, task_handle))
    }

    fn supports_event_listening(&self) -> bool {
        true
    }

    fn supported_event_types(&self) -> Vec<AccessibilityEventType> {
        vec![
            AccessibilityEventType::FocusChanged,
            AccessibilityEventType::ValueChanged,
        ]
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
