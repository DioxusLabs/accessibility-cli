//! macOS accessibility implementation using AXUIElement API.
//!
//! This module provides access to the macOS accessibility tree for reading
//! UI element information and performing actions.

#![allow(dead_code)]

use crate::accessibility::{
    AccessibilityEvent, AccessibilityEventType, AccessibilityReader, Element, ElementCache,
    ElementKey, ElementTree, ListenerConfig, ListenerHandle, Point, Rect, Screenshot, Size,
    StopReason, TreeFilter,
};
use crate::input::code_from_char;
use accessibility_macos_sys::{
    AX_SEARCH_KEY_BUTTON, AX_SEARCH_KEY_CHECKBOX, AX_SEARCH_KEY_CONTROL, AX_SEARCH_KEY_GRAPHIC,
    AX_SEARCH_KEY_HEADING, AX_SEARCH_KEY_LINK, AX_SEARCH_KEY_LIST, AX_SEARCH_KEY_RADIO_GROUP,
    AX_SEARCH_KEY_STATIC_TEXT, AX_SEARCH_KEY_TABLE, AX_SEARCH_KEY_TEXT_FIELD, AxElement,
    AxObserver, AxSearchPredicate, ModifierFlags as MacModifierFlags,
    MouseButton as MacMouseButton, MouseEventKind as MacMouseEventKind, RunLoop, RunLoopSource,
    WindowId,
};
use accesskit::{Action, Role};
use anyhow::{Result, anyhow, bail};
use keyboard_types::{Code, Modifiers};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
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
const AX_ENHANCED_USER_INTERFACE: &str = "AXEnhancedUserInterface";
const AX_MANUAL_ACCESSIBILITY: &str = "AXManualAccessibility";
const AX_ENHANCED_USER_INTERFACE_OBSERVER_WAIT: Duration = Duration::from_millis(500);
const AX_FULL_ACCESSIBILITY_PRIME_DEPTH: usize = 8;
const AX_VISIBLE_CHILDREN: &str = "AXVisibleChildren";
const AX_CHILDREN_IN_NAVIGATION_ORDER: &str = "AXChildrenInNavigationOrder";
const AX_CONTENTS: &str = "AXContents";
const AX_ROWS: &str = "AXRows";
const AX_COLUMNS: &str = "AXColumns";
const AX_TABS: &str = "AXTabs";
const AX_TOOLBAR: &str = "AXToolbar";
const AX_SPLITTERS: &str = "AXSplitters";
const AX_SELECTED_CHILDREN: &str = "AXSelectedChildren";
const AX_SELECTED_ROWS: &str = "AXSelectedRows";
const AX_SELECTED_COLUMNS: &str = "AXSelectedColumns";
const AX_WEB_SEARCH_RESULTS_LIMIT: i32 = 2000;
const AX_CREATED_NOTIFICATION: &str = "AXCreated";
const AX_LOAD_COMPLETE_NOTIFICATION: &str = "AXLoadComplete";
const AX_LAYOUT_COMPLETE_NOTIFICATION: &str = "AXLayoutComplete";
const AX_CHILDREN_CHANGED_NOTIFICATION: &str = "AXChildrenChanged";
const AX_VALUE_CHANGED_NOTIFICATION: &str = "AXValueChanged";
const AX_TITLE_CHANGED_NOTIFICATION: &str = "AXTitleChanged";
const AX_WINDOW_CREATED_NOTIFICATION: &str = "AXWindowCreated";
const AX_MAIN_WINDOW_CHANGED_NOTIFICATION: &str = "AXMainWindowChanged";
const AX_FOCUSED_WINDOW_CHANGED_NOTIFICATION: &str = "AXFocusedWindowChanged";
const AX_FOCUSED_UI_ELEMENT_CHANGED_NOTIFICATION: &str = "AXFocusedUIElementChanged";
const AX_ROW_COUNT_CHANGED_NOTIFICATION: &str = "AXRowCountChanged";
const AX_SELECTED_CHILDREN_CHANGED_NOTIFICATION: &str = "AXSelectedChildrenChanged";
const AX_LIVE_REGION_CREATED_NOTIFICATION: &str = "AXLiveRegionCreated";
const AX_LIVE_REGION_CHANGED_NOTIFICATION: &str = "AXLiveRegionChanged";

const AX_CHILD_ATTRIBUTES: &[&str] = &[
    AX_CHILDREN,
    AX_VISIBLE_CHILDREN,
    AX_CHILDREN_IN_NAVIGATION_ORDER,
    AX_CONTENTS,
    AX_ROWS,
    AX_COLUMNS,
    AX_TABS,
    AX_TOOLBAR,
    AX_SPLITTERS,
    AX_SELECTED_CHILDREN,
    AX_SELECTED_ROWS,
    AX_SELECTED_COLUMNS,
];

const AX_WEB_SEARCH_KEYS: &[&str] = &[
    AX_SEARCH_KEY_CONTROL,
    AX_SEARCH_KEY_BUTTON,
    AX_SEARCH_KEY_LINK,
    AX_SEARCH_KEY_TEXT_FIELD,
    AX_SEARCH_KEY_CHECKBOX,
    AX_SEARCH_KEY_RADIO_GROUP,
    AX_SEARCH_KEY_STATIC_TEXT,
    AX_SEARCH_KEY_HEADING,
    AX_SEARCH_KEY_LIST,
    AX_SEARCH_KEY_TABLE,
    AX_SEARCH_KEY_GRAPHIC,
];

const AX_MATERIALIZATION_NOTIFICATIONS: &[&str] = &[
    AX_CREATED_NOTIFICATION,
    AX_LOAD_COMPLETE_NOTIFICATION,
    AX_LAYOUT_COMPLETE_NOTIFICATION,
    AX_CHILDREN_CHANGED_NOTIFICATION,
    AX_VALUE_CHANGED_NOTIFICATION,
    AX_TITLE_CHANGED_NOTIFICATION,
    AX_WINDOW_CREATED_NOTIFICATION,
    AX_MAIN_WINDOW_CHANGED_NOTIFICATION,
    AX_FOCUSED_WINDOW_CHANGED_NOTIFICATION,
    AX_FOCUSED_UI_ELEMENT_CHANGED_NOTIFICATION,
    AX_ROW_COUNT_CHANGED_NOTIFICATION,
    AX_SELECTED_CHILDREN_CHANGED_NOTIFICATION,
    AX_LIVE_REGION_CREATED_NOTIFICATION,
    AX_LIVE_REGION_CHANGED_NOTIFICATION,
];

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

#[derive(Clone, Copy, Debug)]
struct ChildDiscovery {
    include_search_descendants: bool,
}

impl ChildDiscovery {
    const STRUCTURAL_ONLY: Self = Self {
        include_search_descendants: false,
    };
    const ENRICHED: Self = Self {
        include_search_descendants: true,
    };

    fn discover(self, element: &AxElement) -> Vec<AxElement> {
        let mut children = self.structural_children(element);
        if !self.should_include_search_descendants(element) {
            return children;
        }

        let mut seen = HashSet::new();
        self.collect_structural_signatures(element, &mut seen, 0);
        for child in self.search_predicate_children(element) {
            MacOSAccessibility::push_unique_element(&mut children, &mut seen, child);
        }

        children
    }

    fn structural_children(self, element: &AxElement) -> Vec<AxElement> {
        let mut children = Vec::new();
        let mut seen = HashSet::new();
        let attribute_names = element.attribute_names();

        for attribute in AX_CHILD_ATTRIBUTES {
            if !attribute_names.is_empty() && !attribute_names.iter().any(|name| name == attribute)
            {
                continue;
            }

            for child in element.attribute_elements(attribute) {
                MacOSAccessibility::push_unique_element(&mut children, &mut seen, child);
            }
        }

        children
    }

    fn should_include_search_descendants(self, element: &AxElement) -> bool {
        if !self.include_search_descendants {
            return false;
        }

        MacOSAccessibility::get_string_attribute(element, AX_ROLE).as_deref() == Some(ROLE_WEB_AREA)
            && element.supports_ui_elements_for_search_predicate()
    }

    fn search_predicate_children(self, element: &AxElement) -> Vec<AxElement> {
        element.ui_elements_for_search_predicate(AxSearchPredicate::new(
            AX_WEB_SEARCH_KEYS,
            AX_WEB_SEARCH_RESULTS_LIMIT,
        ))
    }

    fn collect_structural_signatures(
        self,
        element: &AxElement,
        seen: &mut HashSet<String>,
        depth: usize,
    ) {
        let mut stack = vec![(element.clone(), depth)];
        while let Some((current, current_depth)) = stack.pop() {
            if current_depth > 24 {
                continue;
            }

            for child in self.structural_children(&current).into_iter().rev() {
                if seen.insert(MacOSAccessibility::element_signature(&child)) {
                    stack.push((child, current_depth + 1));
                }
            }
        }
    }
}

struct MaterializationObserver {
    _observer: AxObserver,
    run_loop: RunLoop,
    source: RunLoopSource,
    notified: Box<AtomicBool>,
}

impl MaterializationObserver {
    fn start(pid: u32, app: &AxElement) -> Option<Self> {
        let observer = AxObserver::new(pid).ok()?;
        let run_loop = RunLoop::current()?;
        let notified = Box::new(AtomicBool::new(false));

        observer.add_notifications(app, AX_MATERIALIZATION_NOTIFICATIONS, &notified);
        for window in MacOSAccessibility::get_application_windows(app) {
            observer.add_notifications(&window, AX_MATERIALIZATION_NOTIFICATIONS, &notified);
        }

        let source = observer.run_loop_source();
        run_loop.add_default_source(&source);

        Some(Self {
            _observer: observer,
            run_loop,
            source,
            notified,
        })
    }

    fn take_notified(&self) -> bool {
        self.notified.swap(false, Ordering::SeqCst)
    }
}

impl Drop for MaterializationObserver {
    fn drop(&mut self) {
        self.run_loop.remove_default_source(&self.source);
    }
}

/// macOS accessibility reader using AXUIElement API.
pub struct MacOSAccessibility {
    /// Cache of elements with their platform handles.
    cache: ElementCache,

    /// Map from ElementKey to AX element handle for performing actions.
    handles: HashMap<ElementKey, AxElement>,

    /// PID from the most recent tree build, used to keep cached actions targeted.
    last_tree_pid: Option<u32>,

    /// System-wide accessibility element (for hit testing and focus queries).
    system_wide: AxElement,
}

fn sys_point(point: accessibility_macos_sys::Point) -> Point {
    Point::new(point.x, point.y)
}

fn sys_rect(rect: accessibility_macos_sys::Rect) -> Rect {
    Rect::new(
        sys_point(rect.origin),
        Size::new(rect.size.width, rect.size.height),
    )
}

fn sys_screenshot(image: accessibility_macos_sys::PngImage) -> Screenshot {
    Screenshot {
        data: image.data,
        width: image.width,
        height: image.height,
    }
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

        Ok(Self {
            cache: ElementCache::new(),
            handles: HashMap::new(),
            last_tree_pid: None,
            system_wide: AxElement::system_wide(),
        })
    }

    fn empty_replacement() -> Self {
        Self {
            cache: ElementCache::new(),
            handles: HashMap::new(),
            last_tree_pid: None,
            system_wide: AxElement::system_wide(),
        }
    }

    async fn run_with_blocking_state<T, F>(&mut self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Self) -> Result<T> + Send + 'static,
    {
        let mut reader = std::mem::replace(self, Self::empty_replacement());

        let (reader, result) = if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle
                .spawn_blocking(move || {
                    let result = f(&mut reader);
                    (reader, result)
                })
                .await
                .map_err(|error| anyhow!("macOS accessibility blocking task failed: {error}"))?
        } else {
            let result = f(&mut reader);
            (reader, result)
        };

        *self = reader;
        result
    }

    async fn run_blocking_task<T, F>(f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
    {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle
                .spawn_blocking(f)
                .await
                .map_err(|error| anyhow!("macOS accessibility blocking task failed: {error}"))?
        } else {
            f()
        }
    }

    /// Check if the process has accessibility permissions.
    pub fn is_process_trusted() -> bool {
        accessibility_macos_sys::is_process_trusted()
    }

    /// Snapshot the accessibility tree synchronously for a target application.
    ///
    /// The async trait method delegates here; the sys wrapper bounds individual
    /// remote AX messages with AXUIElementSetMessagingTimeout so a bad target
    /// cannot wedge the caller indefinitely.
    fn get_tree_blocking_for_pid(
        &mut self,
        pid: Option<u32>,
        filter: &TreeFilter,
    ) -> Result<ElementTree> {
        let (app_element, actual_pid) = if let Some(pid) = pid {
            (AxElement::application(pid), pid)
        } else {
            let focused_pid = Self::get_frontmost_app_pid()
                .or_else(|| self.get_focused_app_pid_ax())
                .ok_or_else(|| anyhow!("No focused application found"))?;
            (AxElement::application(focused_pid), focused_pid)
        };
        let app_name = Self::get_string_attribute(&app_element, AX_TITLE);

        self.prepare_and_build_tree(actual_pid, &app_element, app_name, filter)
    }

    /// Return the main display's bounds in global screen coordinates.
    fn main_display_bounds() -> Rect {
        sys_rect(accessibility_macos_sys::main_display_bounds())
    }

    /// Capture the main display and encode it as PNG.
    fn capture_main_display() -> Result<Screenshot> {
        accessibility_macos_sys::capture_main_display().map(sys_screenshot)
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

    fn modifier_flags(modifiers: Modifiers) -> MacModifierFlags {
        MacModifierFlags {
            shift: modifiers.contains(Modifiers::SHIFT),
            control: modifiers.contains(Modifiers::CONTROL),
            alt: modifiers.contains(Modifiers::ALT),
            meta: modifiers.contains(Modifiers::META),
        }
    }

    fn post_key_event(
        pid: Option<u32>,
        code: Code,
        modifiers: Modifiers,
        key_down: bool,
    ) -> Result<()> {
        let key_code = Self::key_code(code)
            .ok_or_else(|| anyhow!("Key {:?} is not supported on macOS", code))?;
        // Even with SkyLight per-PID delivery, AppKit-based apps drop key
        // events that arrive while they are not frontmost — that's an
        // OS-level policy we can't override. Callers driving a backgrounded
        // app should invoke the equivalent action (e.g. click the Equals
        // button) rather than send a key like Return.
        accessibility_macos_sys::post_keyboard_event(
            pid,
            key_code,
            Self::modifier_flags(modifiers),
            key_down,
        )
    }

    fn post_keystroke(pid: Option<u32>, code: Code, modifiers: Modifiers) -> Result<()> {
        Self::post_key_event(pid, code, modifiers, true)?;
        std::thread::sleep(Duration::from_millis(10));
        Self::post_key_event(pid, code, modifiers, false)
    }

    fn mac_mouse_button(button: crate::input::MouseButton) -> MacMouseButton {
        match button {
            crate::input::MouseButton::Left => MacMouseButton::Left,
            crate::input::MouseButton::Right => MacMouseButton::Right,
            crate::input::MouseButton::Middle => MacMouseButton::Middle,
        }
    }

    fn post_mouse_event(
        pid: Option<u32>,
        point: Point,
        kind: MacMouseEventKind,
        button: crate::input::MouseButton,
        click_state: i64,
        pressure: f64,
    ) -> Result<()> {
        let window_id = pid.and_then(Self::get_window_id_for_pid);
        accessibility_macos_sys::post_mouse_event(
            pid,
            window_id,
            point.x,
            point.y,
            kind,
            Self::mac_mouse_button(button),
            click_state,
            pressure,
        )
    }

    fn post_chromium_activation_primer(pid: Option<u32>) -> Result<()> {
        if pid.is_none() {
            return Ok(());
        }

        Self::post_mouse_event(
            pid,
            Point::new(-1.0, -1.0),
            MacMouseEventKind::Down,
            crate::input::MouseButton::Left,
            1,
            1.0,
        )?;
        std::thread::sleep(Duration::from_millis(2));
        Self::post_mouse_event(
            pid,
            Point::new(-1.0, -1.0),
            MacMouseEventKind::Up,
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

        let point = Point::new(x, y);
        // Hover the target before clicking. Chromium's pointer-event pipeline
        // tracks hit-test state across moves; React's synthetic onClick won't
        // fire on a button if the renderer never observed a MouseMoved landing
        // on it. The chromium primer wakes the renderer but lands at (-1, -1),
        // so without this move the pointer state stays off-screen.
        Self::post_mouse_event(pid, point, MacMouseEventKind::Move, button, 0, 0.0)?;
        std::thread::sleep(Duration::from_millis(10));
        Self::post_mouse_event(
            pid,
            point,
            MacMouseEventKind::Down,
            button,
            click_state,
            1.0,
        )?;
        std::thread::sleep(Duration::from_millis(10));
        Self::post_mouse_event(pid, point, MacMouseEventKind::Up, button, click_state, 0.0)
    }

    fn current_mouse_location() -> Result<accessibility_macos_sys::Point> {
        accessibility_macos_sys::current_mouse_location()
    }

    fn get_pid_for_element(element: &AxElement) -> Option<u32> {
        element.pid()
    }

    fn flatten_elements(element: &Element, elements: &mut Vec<Element>) {
        let mut stack = vec![element];
        while let Some(current) = stack.pop() {
            elements.push(current.clone());
            for child in current.children.iter().rev() {
                stack.push(child);
            }
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

    fn has_attribute_name(element: &AxElement, attribute: &str) -> bool {
        element.has_attribute(attribute)
    }

    /// Ask the target application to expose its full accessibility interface.
    ///
    /// Chromium uses AXEnhancedUserInterface as the macOS assistive-technology
    /// signal. Electron apps additionally honor AXManualAccessibility. These are
    /// one-way enable requests from our side; toggling them back to false can
    /// make Chromium debounce and delay rebuilding the web accessibility cache.
    fn enable_full_accessibility(element: &AxElement) -> bool {
        let _ = element.attribute_string(AX_ROLE);
        let manual = element.set_bool_attribute(AX_MANUAL_ACCESSIBILITY, true);
        let enhanced = element.set_bool_attribute(AX_ENHANCED_USER_INTERFACE, true);
        manual || enhanced
    }

    fn enable_full_accessibility_for_app(app: &AxElement) -> bool {
        let mut seen = std::collections::HashSet::new();
        let mut requested = Self::enable_full_accessibility_for_subtree(
            app,
            AX_FULL_ACCESSIBILITY_PRIME_DEPTH,
            &mut seen,
        );

        for window in Self::get_application_windows(app) {
            requested |= Self::enable_full_accessibility_for_subtree(
                &window,
                AX_FULL_ACCESSIBILITY_PRIME_DEPTH,
                &mut seen,
            );
        }

        requested
    }

    fn enable_full_accessibility_for_subtree(
        element: &AxElement,
        remaining_depth: usize,
        seen: &mut std::collections::HashSet<String>,
    ) -> bool {
        if !seen.insert(Self::element_signature(element)) {
            return false;
        }

        let mut requested = Self::enable_full_accessibility(element);
        if remaining_depth == 0 {
            return requested;
        }

        for child in Self::discover_children(element, ChildDiscovery::STRUCTURAL_ONLY) {
            requested |=
                Self::enable_full_accessibility_for_subtree(&child, remaining_depth - 1, seen);
        }

        requested
    }

    fn prime_accessibility_roots(app: &AxElement) {
        let _ = app.attribute_string(AX_FOCUSED_UI_ELEMENT);
        let _ = Self::discover_children(app, ChildDiscovery::STRUCTURAL_ONLY);

        for window in Self::get_application_windows(app) {
            let _ = Self::discover_children(&window, ChildDiscovery::STRUCTURAL_ONLY);
            let _ = window.attribute_string(AX_FOCUSED_UI_ELEMENT);
        }
    }

    fn observe_materialization_notifications(
        observer: &AxObserver,
        element: &AxElement,
        notified: &AtomicBool,
    ) {
        observer.add_notifications(element, AX_MATERIALIZATION_NOTIFICATIONS, notified);
    }

    fn has_full_accessibility_request(app: &AxElement) -> bool {
        Self::has_attribute_name(app, AX_ENHANCED_USER_INTERFACE)
            || Self::get_application_windows(app)
                .iter()
                .any(|window| Self::has_attribute_name(window, AX_ENHANCED_USER_INTERFACE))
    }

    fn prepare_and_build_tree(
        &mut self,
        pid: u32,
        app: &AxElement,
        app_name: Option<String>,
        filter: &TreeFilter,
    ) -> Result<ElementTree> {
        let observer = MaterializationObserver::start(pid, app);
        let requested = Self::enable_full_accessibility_for_app(app);
        Self::prime_accessibility_roots(app);

        if !requested && !Self::has_full_accessibility_request(app) {
            return self.build_tree_snapshot(pid, app, app_name, filter);
        }

        let deadline = std::time::Instant::now() + AX_ENHANCED_USER_INTERFACE_OBSERVER_WAIT;

        loop {
            let tree = self.build_tree_snapshot(pid, app, app_name.clone(), filter)?;
            if Self::tree_has_webview_content(&tree) || std::time::Instant::now() >= deadline {
                return Ok(tree);
            }

            accessibility_macos_sys::run_default_loop_slice(0.05, true);
            if observer
                .as_ref()
                .is_some_and(|observer| observer.take_notified())
            {
                Self::prime_accessibility_roots(app);
            }
        }
    }

    fn build_tree_snapshot(
        &mut self,
        pid: u32,
        app: &AxElement,
        app_name: Option<String>,
        filter: &TreeFilter,
    ) -> Result<ElementTree> {
        self.clear_cache();
        self.last_tree_pid = Some(pid);
        let version = self.cache.version();
        let mut element_count = 0;
        let root = self
            .build_element(app, filter, 0, &mut element_count)
            .ok_or_else(|| anyhow!("Failed to build accessibility tree"))?;

        Ok(ElementTree {
            version,
            pid: Some(pid),
            app_name,
            root,
            element_count,
        })
    }

    fn tree_has_webview_content(tree: &ElementTree) -> bool {
        fn has_accessible_text(element: &Element) -> bool {
            [
                element.title.as_ref(),
                element.description.as_ref(),
                element.value.as_ref(),
            ]
            .iter()
            .flatten()
            .any(|value| !value.trim().is_empty())
        }

        let mut stack = vec![(&tree.root, 0usize, false)];
        while let Some((element, depth, inside_webview)) = stack.pop() {
            if depth > 24 {
                continue;
            }

            if inside_webview && has_accessible_text(element) {
                return true;
            }

            let child_inside_webview =
                inside_webview || (element.role == Role::WebView && !element.children.is_empty());
            for child in element.children.iter().rev() {
                stack.push((child, depth + 1, child_inside_webview));
            }
        }

        false
    }

    fn element_signature(element: &AxElement) -> String {
        fn normalized_attribute(element: &AxElement, attribute: &str) -> Option<String> {
            MacOSAccessibility::get_string_attribute(element, attribute)
                .filter(|value| !value.is_empty())
        }

        let pid = Self::get_pid_for_element(element);
        let role = normalized_attribute(element, AX_ROLE);
        let title = normalized_attribute(element, AX_TITLE);
        let description = normalized_attribute(element, AX_DESCRIPTION);
        let value = normalized_attribute(element, AX_VALUE);
        let bounds = Self::get_bounds(element).map(|bounds| {
            (
                bounds.origin.x.round() as i64,
                bounds.origin.y.round() as i64,
                bounds.size.width.round() as i64,
                bounds.size.height.round() as i64,
            )
        });

        format!("{pid:?}|{role:?}|{title:?}|{description:?}|{value:?}|{bounds:?}")
    }

    fn push_unique_element(
        elements: &mut Vec<AxElement>,
        seen: &mut std::collections::HashSet<String>,
        element: AxElement,
    ) {
        if seen.insert(Self::element_signature(&element)) {
            elements.push(element);
        }
    }

    /// Get a string attribute value.
    fn get_string_attribute(element: &AxElement, attribute: &str) -> Option<String> {
        element.attribute_string(attribute)
    }

    /// Get a boolean attribute value.
    fn get_bool_attribute(element: &AxElement, attribute: &str) -> Option<bool> {
        element.attribute_bool(attribute)
    }

    /// Get the position of an element as a Point.
    fn get_position(element: &AxElement) -> Option<Point> {
        element.attribute_point(AX_POSITION).map(sys_point)
    }

    /// Get the size of an element.
    fn get_size(element: &AxElement) -> Option<(f64, f64)> {
        element
            .attribute_size(AX_SIZE)
            .map(|size| (size.width, size.height))
    }

    /// Get the bounds (position + size) of an element.
    fn get_bounds(element: &AxElement) -> Option<Rect> {
        let position = Self::get_position(element)?;
        let (width, height) = Self::get_size(element)?;

        Some(Rect::new(position, Size::new(width, height)))
    }

    /// Discover children for the requested traversal purpose.
    fn discover_children(element: &AxElement, discovery: ChildDiscovery) -> Vec<AxElement> {
        discovery.discover(element)
    }

    /// Get tree-building children for an element.
    fn get_children(element: &AxElement) -> Vec<AxElement> {
        Self::discover_children(element, ChildDiscovery::ENRICHED)
    }

    /// Get the windows of an application element.
    ///
    /// For a non-frontmost application, `AXChildren` typically omits the visible
    /// windows. Empirically on macOS, `AXWindows` is *also* often empty for
    /// backgrounded apps, but `AXMainWindow` still returns the focused window;
    /// we use both so single-window apps still walk correctly when backgrounded.
    /// The returned list is deduped by window title — macOS hands out fresh
    /// AX element wrappers per call so raw-pointer dedup doesn't work.
    fn get_application_windows(element: &AxElement) -> Vec<AxElement> {
        let mut windows: Vec<AxElement> = Vec::new();
        let mut seen_titles: std::collections::HashSet<String> = std::collections::HashSet::new();

        let push = |w: AxElement,
                    windows: &mut Vec<AxElement>,
                    seen: &mut std::collections::HashSet<String>| {
            let title = Self::get_string_attribute(&w, AX_TITLE).unwrap_or_default();
            if title.is_empty() || seen.insert(title) {
                windows.push(w);
            }
        };

        for window in element.attribute_elements(AX_WINDOWS) {
            push(window, &mut windows, &mut seen_titles);
        }

        for window in element.attribute_elements(AX_MAIN_WINDOW) {
            push(window, &mut windows, &mut seen_titles);
        }

        windows
    }

    /// Get available actions for an element.
    fn get_actions(element: &AxElement) -> Vec<String> {
        element.action_names()
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

    /// Build an Element from an AX element.
    fn build_element(
        &mut self,
        ax_element: &AxElement,
        filter: &TreeFilter,
        depth: usize,
        element_count: &mut usize,
    ) -> Option<Element> {
        struct BuildFrame {
            ax_element: AxElement,
            depth: usize,
            element: Option<Element>,
            self_matches: bool,
            children: Vec<AxElement>,
            next_child: usize,
            retained_children: Vec<Element>,
        }

        impl BuildFrame {
            fn new(ax_element: AxElement, depth: usize) -> Self {
                Self {
                    ax_element,
                    depth,
                    element: None,
                    self_matches: false,
                    children: Vec::new(),
                    next_child: 0,
                    retained_children: Vec::new(),
                }
            }
        }

        let root_depth = depth;
        let mut root = None;
        let mut stack = vec![BuildFrame::new(ax_element.clone(), depth)];

        while !stack.is_empty() {
            let index = stack.len() - 1;

            if stack[index].element.is_none() {
                if let Some(max) = filter.max_elements
                    && *element_count >= max
                    && stack[index].depth != root_depth
                {
                    stack.pop();
                    continue;
                }

                let current_ax = stack[index].ax_element.clone();
                let current_depth = stack[index].depth;
                let ax_role = match Self::get_string_attribute(&current_ax, AX_ROLE) {
                    Some(role) => role,
                    None => {
                        stack.pop();
                        continue;
                    }
                };
                let role = Self::map_role(&ax_role);

                #[allow(deprecated)]
                let id = self.cache.next_id();

                let mut element = Element::new(id, role);
                element.title = Self::get_string_attribute(&current_ax, AX_TITLE);
                element.description = Self::get_string_attribute(&current_ax, AX_DESCRIPTION);
                element.value = Self::get_string_attribute(&current_ax, AX_VALUE);
                element.bounds = Self::get_bounds(&current_ax);
                element.enabled = Self::get_bool_attribute(&current_ax, AX_ENABLED).unwrap_or(true);
                element.focused =
                    Self::get_bool_attribute(&current_ax, AX_FOCUSED).unwrap_or(false);
                element.actions = Self::get_actions(&current_ax);

                let self_matches = filter.should_include(&element, current_depth);
                let mut children = if filter.max_depth.is_none_or(|max| current_depth < max) {
                    Self::get_children(&current_ax)
                } else {
                    Vec::new()
                };

                // For backgrounded apps, AXChildren of the Application typically omits
                // visible windows; AXWindows still returns them. Fall back to AXWindows
                // only when AXChildren produced no Window-role child.
                if role == Role::Application {
                    let has_window_child = children.iter().any(|child| {
                        Self::get_string_attribute(child, AX_ROLE)
                            .map(|role| role == ROLE_WINDOW)
                            .unwrap_or(false)
                    });
                    if !has_window_child {
                        children.extend(Self::get_application_windows(&current_ax));
                    }
                }

                stack[index].element = Some(element);
                stack[index].self_matches = self_matches;
                stack[index].children = children;
                continue;
            }

            if stack[index].next_child < stack[index].children.len() {
                let child = stack[index].children[stack[index].next_child].clone();
                let child_depth = stack[index].depth + 1;
                stack[index].next_child += 1;
                stack.push(BuildFrame::new(child, child_depth));
                continue;
            }

            let mut frame = stack.pop().expect("stack is not empty");
            let Some(mut element) = frame.element.take() else {
                continue;
            };
            element.children = frame.retained_children;

            let keep =
                frame.self_matches || !element.children.is_empty() || frame.depth == root_depth;
            if !keep {
                continue;
            }

            let id = element.id;
            self.handles.insert(id, frame.ax_element);

            #[allow(deprecated)]
            self.cache.store_with_id(id, element.clone());
            *element_count += 1;

            if let Some(parent) = stack.last_mut() {
                parent.retained_children.push(element);
            } else {
                root = Some(element);
            }
        }

        root
    }

    /// Get the focused application's PID using NSWorkspace (most reliable method).
    fn get_frontmost_app_pid() -> Option<u32> {
        accessibility_macos_sys::frontmost_application_pid()
    }

    /// List all visible application windows with their PIDs, app names, window titles, and focus state.
    pub fn list_windows() -> Vec<(u32, String, String, bool)> {
        let mut windows = Vec::new();
        let frontmost_pid = accessibility_macos_sys::frontmost_application_pid();

        for app in accessibility_macos_sys::running_applications() {
            if app.activation_policy != 0 {
                continue;
            }

            let app_name = app.localized_name.unwrap_or_else(|| "Unknown".to_string());
            let window_title =
                Self::get_window_title_for_pid(app.pid).unwrap_or_else(|| app_name.clone());
            let is_focused = frontmost_pid == Some(app.pid);

            windows.push((app.pid, app_name, window_title, is_focused));
        }

        windows
    }

    /// Get the main window for a given PID using accessibility APIs.
    fn get_window_for_pid(pid: u32) -> Option<AxElement> {
        let app = AxElement::application(pid);
        Self::enable_full_accessibility_for_app(&app);

        for window in app.attribute_elements(AX_MAIN_WINDOW) {
            if let Some(bounds) = Self::get_bounds(&window)
                && bounds.size.width > 0.0
                && bounds.size.height > 0.0
            {
                return Some(window);
            }
        }

        for window in app.attribute_elements(AX_WINDOWS) {
            if let Some(bounds) = Self::get_bounds(&window)
                && bounds.size.width > 0.0
                && bounds.size.height > 0.0
            {
                return Some(window);
            }
        }

        None
    }

    /// Get the window title for a given PID using accessibility APIs.
    fn get_window_title_for_pid(pid: u32) -> Option<String> {
        let window = Self::get_window_for_pid(pid)?;
        Self::get_string_attribute(&window, AX_TITLE).filter(|title| !title.is_empty())
    }

    /// Get the main window bounds for a given PID using accessibility APIs.
    fn get_window_bounds_for_pid(pid: u32) -> Option<Rect> {
        let window = Self::get_window_for_pid(pid)?;
        Self::get_bounds(&window)
            .filter(|bounds| bounds.size.width > 0.0 && bounds.size.height > 0.0)
    }

    /// Resolve an AX window to its WindowServer ID using private AX SPI.
    fn get_window_id(window: &AxElement) -> Option<WindowId> {
        window.window_id()
    }

    fn get_window_id_for_pid(pid: u32) -> Option<WindowId> {
        let window = Self::get_window_for_pid(pid)?;
        Self::get_window_id(&window)
    }

    /// Set a target window's WindowServer alpha without hiding or minimizing it.
    ///
    /// This is intentionally narrow and used by macOS integration tests that
    /// need a real, materialized window for AX while keeping it off the user's
    /// screen. Hiding/minimizing Chrome prevents its web accessibility tree from
    /// materializing.
    #[doc(hidden)]
    pub fn set_window_alpha_for_pid(pid: u32, alpha: f32) -> bool {
        Self::get_window_id_for_pid(pid)
            .is_some_and(|window_id| accessibility_macos_sys::set_window_alpha(window_id, alpha))
    }

    /// Move and resize a target window without activating its owning app.
    ///
    /// Used by macOS integration tests to keep Chrome's renderer-backed window
    /// materialized for AX while placing it outside the user's visible display.
    #[doc(hidden)]
    pub fn move_window_for_pid(pid: u32, x: f64, y: f64, width: f64, height: f64) -> bool {
        let Some(window) = Self::get_window_for_pid(pid) else {
            return false;
        };

        let positioned =
            window.set_point_attribute(AX_POSITION, accessibility_macos_sys::Point::new(x, y));
        let sized =
            window.set_size_attribute(AX_SIZE, accessibility_macos_sys::Size::new(width, height));

        positioned.is_ok() && sized.is_ok()
    }

    /// Capture a target window through WindowServer so occluding windows are not included.
    fn capture_window_for_pid(pid: u32) -> Result<Option<Screenshot>> {
        let Some(window_id) = Self::get_window_for_pid(pid)
            .as_ref()
            .and_then(Self::get_window_id)
        else {
            return Ok(None);
        };

        accessibility_macos_sys::capture_window(window_id).map(|image| image.map(sys_screenshot))
    }

    /// Get the focused application's PID (fallback using AX APIs).
    fn get_focused_app_pid_ax(&self) -> Option<u32> {
        self.system_wide
            .attribute_elements(AX_FOCUSED_APPLICATION)
            .into_iter()
            .find_map(|element| element.pid())
            .or_else(|| {
                self.system_wide
                    .attribute_elements(AX_FOCUSED_UI_ELEMENT)
                    .into_iter()
                    .find_map(|element| element.pid())
            })
    }
}

impl AccessibilityReader for MacOSAccessibility {
    fn platform_name(&self) -> &'static str {
        "macOS"
    }

    async fn get_tree(&mut self, pid: Option<u32>, filter: &TreeFilter) -> Result<ElementTree> {
        let filter = filter.clone();
        self.run_with_blocking_state(move |reader| reader.get_tree_blocking_for_pid(pid, &filter))
            .await
    }

    fn get_element(&self, id: ElementKey) -> Option<&Element> {
        self.cache.get(id)
    }

    async fn perform_action(&mut self, id: ElementKey, action: Action) -> Result<()> {
        self.run_with_blocking_state(move |reader| {
            let handle = reader
                .handles
                .get(&id)
                .ok_or_else(|| anyhow!("Element {} not found in cache", id))?;

            // Focus/Blur aren't AX actions on macOS — they're attribute writes.
            if matches!(action, Action::Focus | Action::Blur) {
                let want_focus = matches!(action, Action::Focus);
                let result = handle.set_bool_attribute_result(AX_FOCUSED, want_focus);
                if !result.is_success() {
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
                return Ok(());
            }

            // Route certain clicks through a synthetic mouse event via SkyLight
            // instead of AXPress, when the element has bounds and a target pid:
            //
            // 1. Menu/MenuItem/MenuBar — AXPress on these goes through AppKit's
            //    menu-tracking path which promotes the owning app to key.
            //    Synthetic clicks keep focus put.
            // 2. Chromium-based apps (Electron: Discord/Slack/VS Code; Chrome
            //    itself; Edge/Brave/etc.) — Chromium's AX-to-DOM bridge
            //    silently drops AXPress for many web elements. The AX call
            //    returns success but the renderer never dispatches a DOM
            //    click. Synthetic mouse events hit Chromium's input pipeline
            //    directly and the web element's onClick fires.
            //
            // AXPress remains the path for native AppKit controls (Calculator,
            // Finder, etc.) where it's bulletproof and unaffected by window
            // occlusion — and for elements without bounds.
            if matches!(action, Action::Click)
                && let Some(element) = reader.cache.get(id)
                && let Some(bounds) = element.bounds
                && let Some(pid) = Self::get_pid_for_element(handle)
                && (matches!(element.role, Role::Menu | Role::MenuItem | Role::MenuBar)
                    || accessibility_macos_sys::is_chromium_based_app(pid))
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

            let action_name = Self::map_action(action)
                .ok_or_else(|| anyhow!("Action {:?} not supported on macOS", action))?;

            if let Err(result) = handle.perform_action(action_name) {
                bail!("Failed to perform action {}: {:?}", action_name, result);
            }

            Ok(())
        })
        .await
    }

    async fn set_value(&mut self, id: ElementKey, value: &str) -> Result<()> {
        let value = value.to_string();
        self.run_with_blocking_state(move |reader| {
            let handle = reader
                .handles
                .get(&id)
                .ok_or_else(|| anyhow!("Element {} not found in cache", id))?;

            if let Err(result) = handle.set_string_attribute(AX_VALUE, &value) {
                bail!("Failed to set value: {:?}", result);
            }

            Ok(())
        })
        .await
    }

    async fn hit_test(&mut self, x: f64, y: f64) -> Result<Option<ElementKey>> {
        self.run_with_blocking_state(move |reader| {
            if let Some(ax_element) = reader.system_wide.element_at_position(x, y) {
                let mut count = reader.cache.len();
                let element =
                    reader.build_element(&ax_element, &TreeFilter::default(), 0, &mut count);
                Ok(element.map(|e| e.id))
            } else {
                Ok(None)
            }
        })
        .await
    }

    fn clear_cache(&mut self) {
        self.cache.clear();
        self.handles.clear();
        self.last_tree_pid = None;
    }

    fn snapshot_version(&self) -> u64 {
        self.cache.version()
    }

    async fn keystroke(&mut self, pid: Option<u32>, key: Code, modifiers: Modifiers) -> Result<()> {
        Self::post_keystroke(pid, key, modifiers)
    }

    async fn type_raw(&mut self, pid: Option<u32>, text: &str) -> Result<()> {
        let text = text.to_string();
        Self::run_blocking_task(move || {
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
        })
        .await
    }

    async fn mouse_click_at(
        &mut self,
        pid: Option<u32>,
        x: f64,
        y: f64,
        button: crate::input::MouseButton,
    ) -> Result<()> {
        Self::post_mouse_click_sequence(pid, x, y, button, 1)
    }

    async fn press_key(&mut self, pid: Option<u32>, key: Code) -> Result<()> {
        Self::post_key_event(pid, key, Modifiers::empty(), true)
    }

    async fn release_key(&mut self, pid: Option<u32>, key: Code) -> Result<()> {
        Self::post_key_event(pid, key, Modifiers::empty(), false)
    }

    async fn mouse_move(&mut self, pid: Option<u32>, x: f64, y: f64) -> Result<()> {
        Self::post_mouse_event(
            pid,
            Point::new(x, y),
            MacMouseEventKind::Move,
            crate::input::MouseButton::Left,
            0,
            0.0,
        )
    }

    async fn mouse_click(
        &mut self,
        pid: Option<u32>,
        button: crate::input::MouseButton,
    ) -> Result<()> {
        Self::run_blocking_task(move || {
            let point = Self::current_mouse_location()?;
            Self::post_mouse_click_sequence(pid, point.x, point.y, button, 1)
        })
        .await
    }

    async fn mouse_double_click(
        &mut self,
        pid: Option<u32>,
        button: crate::input::MouseButton,
    ) -> Result<()> {
        Self::run_blocking_task(move || {
            let point = Self::current_mouse_location()?;
            Self::post_mouse_click_sequence(pid, point.x, point.y, button, 1)?;
            std::thread::sleep(Duration::from_millis(40));
            Self::post_mouse_click_sequence(pid, point.x, point.y, button, 2)
        })
        .await
    }

    async fn mouse_scroll(&mut self, pid: Option<u32>, delta_x: f64, delta_y: f64) -> Result<()> {
        accessibility_macos_sys::post_scroll_event(pid, delta_x, delta_y)
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
            && let Some(window_bounds) = Self::get_window_bounds_for_pid(pid)
        {
            let screen_bounds = Self::main_display_bounds();
            if let Ok(cropped) = screenshot.crop(&window_bounds, &screen_bounds) {
                return Ok(cropped);
            }
        }

        Ok(screenshot)
    }

    async fn get_screen_bounds(&self, pid: Option<u32>) -> Result<Rect> {
        Self::run_blocking_task(move || {
            Ok(pid
                .and_then(Self::get_window_bounds_for_pid)
                .unwrap_or_else(Self::main_display_bounds))
        })
        .await
    }

    fn start_listening(
        &mut self,
        config: ListenerConfig,
        callback: Box<dyn FnMut(AccessibilityEvent) + Send + 'static>,
    ) -> Result<ListenerHandle> {
        let pid = config.pid;
        let stop_flag = Arc::new(AtomicBool::new(false));
        let task_stop_flag = stop_flag.clone();

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
            let mut observed_elements: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut first_snapshot = true;
            let materialization_notified = AtomicBool::new(false);
            let mut observer_source = None;
            let mut observer = None;
            let run_loop = RunLoop::current();

            if let (Some(pid), Some(run_loop)) = (pid, run_loop.as_ref())
                && let Ok(ax_observer) = AxObserver::new(pid)
            {
                let app = AxElement::application(pid);
                Self::observe_materialization_notifications(
                    &ax_observer,
                    &app,
                    &materialization_notified,
                );
                for window in Self::get_application_windows(&app) {
                    Self::observe_materialization_notifications(
                        &ax_observer,
                        &window,
                        &materialization_notified,
                    );
                }

                let source = ax_observer.run_loop_source();
                run_loop.add_default_source(&source);
                Self::enable_full_accessibility_for_app(&app);
                Self::prime_accessibility_roots(&app);
                observer_source = Some(source);
                observer = Some(ax_observer);
            }

            while !task_stop_flag.load(Ordering::SeqCst) {
                if run_loop.is_some() {
                    accessibility_macos_sys::run_default_loop_slice(0.05, true);
                }
                if materialization_notified.swap(false, Ordering::SeqCst)
                    && let Some(pid) = pid
                {
                    let app = AxElement::application(pid);
                    Self::enable_full_accessibility_for_app(&app);
                    Self::prime_accessibility_roots(&app);
                }

                match reader.get_tree_blocking_for_pid(pid, &TreeFilter::default()) {
                    Ok(tree) => {
                        let (values, focused) = MacOSAccessibility::listener_snapshots(&tree);
                        if let Some(ax_observer) = observer.as_ref() {
                            for handle in reader.handles.values() {
                                let signature = MacOSAccessibility::element_signature(handle);
                                if observed_elements.insert(signature) {
                                    Self::observe_materialization_notifications(
                                        ax_observer,
                                        handle,
                                        &materialization_notified,
                                    );
                                }
                            }
                        }

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

            if let (Some(run_loop), Some(source)) = (run_loop.as_ref(), observer_source.as_ref()) {
                run_loop.remove_default_source(source);
            }
            drop(observer);

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
