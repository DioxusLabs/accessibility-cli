use accesskit::Role;
use euclid::{Point2D, Rect as EuclidRect, Size2D};
use slotmap::{Key, KeyData, SlotMap};
use windows::Win32::Foundation::{HWND, LPARAM, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GA_ROOT, GetAncestor, GetClassNameW, GetWindowTextW, IsWindowVisible, SW_HIDE,
    ShowWindow, WindowFromPoint,
};
use windows::core::BOOL;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScreenSpace;

pub type Point = Point2D<f64, ScreenSpace>;
pub type Size = Size2D<f64, ScreenSpace>;
pub type Rect = EuclidRect<f64, ScreenSpace>;

slotmap::new_key_type! {
    pub struct ElementKey;
}

impl ElementKey {
    pub fn to_ffi(self) -> u64 {
        self.data().as_ffi()
    }

    pub fn from_ffi(value: u64) -> Self {
        KeyData::from_ffi(value).into()
    }
}

impl std::fmt::Display for ElementKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_ffi())
    }
}

#[derive(Debug, Clone)]
pub struct Screenshot {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct Element {
    pub id: ElementKey,
    pub role: Role,
    pub title: Option<String>,
    pub description: Option<String>,
    pub value: Option<String>,
    pub url: Option<String>,
    pub help: Option<String>,
    pub role_description: Option<String>,
    pub identifier: Option<String>,
    pub bounds: Option<Rect>,
    pub enabled: bool,
    pub focused: bool,
    pub actions: Vec<String>,
    pub children: Vec<Element>,
}

impl Element {
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
}

#[derive(Debug, Clone)]
pub struct ElementTree {
    pub version: u64,
    pub pid: Option<u32>,
    pub app_name: Option<String>,
    pub root: Element,
    pub element_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct TreeFilter {
    pub max_depth: Option<usize>,
    pub max_elements: Option<usize>,
    pub interactive_only: bool,
    pub visible_only: bool,
    pub within_bounds: Option<Rect>,
    pub roles: Option<Vec<Role>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

pub struct WindowBlockerSpec<'a> {
    pub titles: &'a [&'a str],
    pub classes: &'a [&'a str],
}

fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..len])
}

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..len])
}

fn matches_window_blocker(hwnd: HWND, spec: &WindowBlockerSpec<'_>) -> bool {
    spec.titles.iter().any(|title| *title == window_title(hwnd))
        || spec
            .classes
            .iter()
            .any(|class| *class == window_class(hwnd))
}

pub fn hide_top_level_windows_matching(spec: &WindowBlockerSpec<'_>) -> usize {
    struct Ctx<'a> {
        spec: &'a WindowBlockerSpec<'a>,
        hidden: usize,
    }

    let mut ctx = Ctx { spec, hidden: 0 };

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = unsafe { &mut *(lparam.0 as *mut Ctx) };
        if unsafe { IsWindowVisible(hwnd).as_bool() } && matches_window_blocker(hwnd, ctx.spec) {
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            ctx.hidden += 1;
        }
        true.into()
    }

    let lparam = LPARAM(&mut ctx as *mut _ as isize);
    let _ = unsafe { EnumWindows(Some(enum_proc), lparam) };
    ctx.hidden
}

pub fn hide_windows_matching_at_point(x: f64, y: f64, spec: &WindowBlockerSpec<'_>) -> usize {
    let point = POINT {
        x: x as i32,
        y: y as i32,
    };
    let mut hidden = 0;

    for _ in 0..6 {
        let hwnd = unsafe { WindowFromPoint(point) };
        if hwnd.is_invalid() {
            break;
        }

        let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
        let to_hide = if root.is_invalid() { hwnd } else { root };
        if !matches_window_blocker(to_hide, spec) && !matches_window_blocker(hwnd, spec) {
            break;
        }

        let _ = unsafe { ShowWindow(to_hide, SW_HIDE) };
        hidden += 1;
    }

    hidden
}

#[derive(Debug, Clone)]
pub enum AccessibilityEvent {
    FocusChanged {
        element: Option<Element>,
        pid: Option<u32>,
        timestamp: u64,
    },
    ValueChanged {
        element: Option<Element>,
        old_value: Option<String>,
        new_value: Option<String>,
        timestamp: u64,
    },
    TitleChanged {
        element: Option<Element>,
        old_title: Option<String>,
        new_title: Option<String>,
        timestamp: u64,
    },
    StructureChanged {
        parent_element: Option<Element>,
        change_type: StructureChangeType,
        timestamp: u64,
    },
    WindowCreated {
        element: Option<Element>,
        pid: Option<u32>,
        timestamp: u64,
    },
    WindowDestroyed {
        window_id: Option<String>,
        pid: Option<u32>,
        timestamp: u64,
    },
    WindowFocusChanged {
        element: Option<Element>,
        pid: Option<u32>,
        timestamp: u64,
    },
    SelectedTextChanged {
        element: Option<Element>,
        selected_text: Option<String>,
        timestamp: u64,
    },
    ElementDestroyed {
        element_id: Option<ElementKey>,
        timestamp: u64,
    },
    Error {
        message: String,
        timestamp: u64,
    },
    Stopped {
        reason: StopReason,
        timestamp: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureChangeType {
    ChildrenAdded,
    ChildrenRemoved,
    ChildrenReordered,
    Invalidated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    UserRequested,
    ProcessTerminated,
    ConnectionLost,
    PermissionDenied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone)]
pub struct ListenerConfig {
    pub event_types: Option<Vec<AccessibilityEventType>>,
    pub pid: Option<u32>,
    pub buffer_size: usize,
}

impl ListenerConfig {
    pub fn should_capture(&self, event_type: AccessibilityEventType) -> bool {
        match &self.event_types {
            Some(types) => types.contains(&event_type),
            None => true,
        }
    }
}

pub(super) struct ElementCache {
    elements: SlotMap<ElementKey, Element>,
    version: u64,
}

impl ElementCache {
    pub(super) fn new() -> Self {
        Self {
            elements: SlotMap::with_key(),
            version: 1,
        }
    }

    pub(super) fn clear(&mut self) {
        self.elements.clear();
        self.version = self.version.saturating_add(1);
    }

    pub(super) fn get(&self, id: ElementKey) -> Option<&Element> {
        self.elements.get(id)
    }

    pub(super) fn store_with_clone<F>(&mut self, f: F) -> (ElementKey, Element)
    where
        F: FnOnce(ElementKey) -> Element,
    {
        let key = self.elements.insert_with_key(f);
        let element = self.elements[key].clone();
        (key, element)
    }

    pub(super) fn version(&self) -> u64 {
        self.version
    }
}
