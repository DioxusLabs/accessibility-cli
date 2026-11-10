use super::common::ElementCache;
use super::input::{code_to_vk, send_key_event};
use super::*;
use windows::Win32::System::Ole::SafeArrayDestroy;
use windows::Win32::System::Variant::{VARENUM, VT_BOOL, VT_BSTR, VT_I4, VT_R8, VariantClear};
use windows::Win32::UI::Accessibility::{
    IUIAutomationItemContainerPattern, IUIAutomationScrollPattern,
    IUIAutomationVirtualizedItemPattern, ScrollAmount_LargeDecrement, ScrollAmount_LargeIncrement,
    ScrollAmount_NoAmount, UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_FrameworkIdPropertyId, UIA_IsContentElementPropertyId, UIA_IsControlElementPropertyId,
    UIA_ItemContainerPatternId, UIA_NativeWindowHandlePropertyId, UIA_ScrollPatternId,
    UIA_VirtualizedItemPatternId,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_PARENT, GetAncestor, GetDesktopWindow, IsWindowVisible,
};

mod adapter;
mod events;

/// UTF-16 units of a document's text kept as its element value.
const MAX_DOCUMENT_TEXT: i32 = 4000;
/// How long UI Automation waits for an app to answer one request. The default
/// (20 s) lets a hung app consume most of a caller's deadline in a single call.
const TRANSACTION_TIMEOUT_MS: u32 = 10_000;
/// A walk stops here and reports a truncated tree.
const MAX_WALK_TIME: std::time::Duration = std::time::Duration::from_secs(20);
/// Listing a long list in one request can take most of a walk and cannot be
/// interrupted, so lists are read one child at a time up to this many.
const MAX_LIST_CHILDREN: usize = 500;
const UIA_E_TIMEOUT: i32 = 0x80131505_u32 as i32;
const WALK_PROPERTIES: [UIA_PROPERTY_ID; 24] = [
    UIA_ControlTypePropertyId,
    UIA_NamePropertyId,
    UIA_AutomationIdPropertyId,
    UIA_BoundingRectanglePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_HasKeyboardFocusPropertyId,
    UIA_IsOffscreenPropertyId,
    UIA_IsInvokePatternAvailablePropertyId,
    UIA_IsTogglePatternAvailablePropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId,
    UIA_IsValuePatternAvailablePropertyId,
    UIA_IsRangeValuePatternAvailablePropertyId,
    UIA_IsTextPatternAvailablePropertyId,
    UIA_ValueValuePropertyId,
    UIA_ValueIsReadOnlyPropertyId,
    UIA_RangeValueValuePropertyId,
    UIA_ToggleToggleStatePropertyId,
    UIA_SelectionItemIsSelectedPropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_FrameworkIdPropertyId,
    UIA_IsControlElementPropertyId,
    UIA_IsContentElementPropertyId,
    UIA_NativeWindowHandlePropertyId,
];

/// A cached property, freed on drop. Win32's raw VARIANT has no destructor.
struct CachedProperty(VARIANT);

impl CachedProperty {
    fn read(native: &IUIAutomationElement, property: UIA_PROPERTY_ID) -> Option<Self> {
        unsafe { native.GetCachedPropertyValue(property) }
            .ok()
            .map(Self)
    }

    fn kind(&self) -> VARENUM {
        unsafe { self.0.Anonymous.Anonymous.vt }
    }
}

impl Drop for CachedProperty {
    fn drop(&mut self) {
        let _ = unsafe { VariantClear(&mut self.0) };
    }
}

fn cached_bool(native: &IUIAutomationElement, property: UIA_PROPERTY_ID) -> bool {
    CachedProperty::read(native, property).is_some_and(|value| {
        value.kind() == VT_BOOL
            && unsafe { value.0.Anonymous.Anonymous.Anonymous.boolVal }.as_bool()
    })
}

fn cached_i32(native: &IUIAutomationElement, property: UIA_PROPERTY_ID) -> Option<i32> {
    let value = CachedProperty::read(native, property)?;
    (value.kind() == VT_I4).then(|| unsafe { value.0.Anonymous.Anonymous.Anonymous.lVal })
}

fn cached_f64(native: &IUIAutomationElement, property: UIA_PROPERTY_ID) -> Option<f64> {
    let value = CachedProperty::read(native, property)?;
    (value.kind() == VT_R8).then(|| unsafe { value.0.Anonymous.Anonymous.Anonymous.dblVal })
}

fn cached_string(native: &IUIAutomationElement, property: UIA_PROPERTY_ID) -> Option<String> {
    let value = CachedProperty::read(native, property)?;
    if value.kind() != VT_BSTR {
        return None;
    }
    let text = unsafe { &value.0.Anonymous.Anonymous.Anonymous.bstrVal };
    Some(text.to_string()).filter(|text| !text.is_empty())
}

/// FNV-1a of the runtime id, which identifies an element for its lifetime.
fn runtime_id_hash(native: &IUIAutomationElement) -> Option<u64> {
    let array = unsafe { native.GetRuntimeId() }.ok()?;
    if array.is_null() {
        return None;
    }
    let hash = unsafe {
        let parts = (*array).rgsabound[0].cElements as usize;
        let data = (*array).pvData as *const i32;
        let hash = std::slice::from_raw_parts(data, parts)
            .iter()
            .fold(0xcbf29ce484222325_u64, |hash, part| {
                (hash ^ *part as u32 as u64).wrapping_mul(0x100000001b3)
            });
        let _ = SafeArrayDestroy(array);
        hash
    };
    Some(hash)
}

/// Windows accessibility reader using UI Automation.
pub struct WindowsAccessibility {
    automation: IUIAutomation,
    walk_request: IUIAutomationCacheRequest,
    pending_action: Option<std::sync::mpsc::Receiver<windows::core::Result<()>>>,
    pending_action_pid: u32,
    running_actions: Vec<(u32, std::sync::mpsc::Receiver<windows::core::Result<()>>)>,
    partial_view: Option<String>,
    stalled_probes: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<isize>>>,
    all_elements: IUIAutomationCondition,
    walk_started: std::time::Instant,
    walk_cut_short: bool,
    /// The walked process, and the top-level windows the walk has entered.
    walk_pid: u32,
    walk_windows: std::collections::HashSet<i32>,
    /// A pid and title substring that picks which of its windows gets walked.
    selected_window: Option<(u32, String)>,
    cache: ElementCache,
    /// Map from ElementKey to native IUIAutomationElement.
    /// Uses SecondaryMap which is automatically synchronized with the primary SlotMap in cache.
    native_elements: SecondaryMap<ElementKey, IUIAutomationElement>,
}

impl WindowsAccessibility {
    /// Create a new Windows accessibility reader.
    pub fn new() -> Result<Self> {
        // Initialize COM
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }

        // Create UI Automation instance
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)? };

        // UIA reports physical pixels to DPI-aware callers only, and pointer
        // input and screen metrics have to use the same coordinate space.
        unsafe {
            let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        if let Ok(automation2) = automation.cast::<IUIAutomation2>() {
            let _ = unsafe { automation2.SetTransactionTimeout(TRANSACTION_TIMEOUT_MS) };
        }

        // Fetching every property with the child list makes a walk one round
        // trip per parent instead of about ten per element.
        let all_elements = unsafe { automation.CreateTrueCondition()? };
        let walk_request = unsafe { automation.CreateCacheRequest()? };
        unsafe { walk_request.SetTreeFilter(&all_elements)? };
        for property in WALK_PROPERTIES {
            unsafe { walk_request.AddProperty(property)? };
        }

        Ok(Self {
            automation,
            walk_request,
            pending_action: None,
            pending_action_pid: 0,
            running_actions: Vec::new(),
            partial_view: None,
            stalled_probes: Default::default(),
            all_elements,
            walk_started: std::time::Instant::now(),
            walk_cut_short: false,
            walk_pid: 0,
            walk_windows: std::collections::HashSet::new(),
            selected_window: None,
            cache: ElementCache::new(),
            native_elements: SecondaryMap::new(),
        })
    }

    /// Focus the window for a given PID.
    ///
    /// This brings the window to the foreground and gives it keyboard focus.
    /// Required before sending keyboard input.
    pub fn focus_window(&self, pid: u32) -> Result<()> {
        let element = self.find_root_for_pid(pid)?;
        let native_hwnd = unsafe { element.CurrentNativeWindowHandle()? };
        let hwnd = HWND(native_hwnd.0 as *mut _);

        // Set focus via UI Automation first
        if unsafe { IsIconic(hwnd) }.as_bool() {
            let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
        }
        let _ = unsafe { element.SetFocus() };

        // Then bring window to foreground
        let _ = unsafe { SetForegroundWindow(hwnd) };

        Ok(())
    }

    /// Prefer the process's window whose title contains `title` when walking it.
    pub fn select_window(&mut self, pid: u32, title: Option<&str>) {
        self.selected_window = title.map(|title| (pid, title.to_lowercase()));
    }

    /// List all top-level windows with their PIDs.
    ///
    /// Returns a list of (pid, app_name, window_title, is_focused) for each window.
    pub fn list_windows(&self) -> Vec<(u32, String, String, bool)> {
        let mut windows = Vec::new();

        // Get foreground window to determine focus
        let foreground_hwnd = unsafe { GetForegroundWindow() };
        let mut foreground_pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(foreground_hwnd, Some(&mut foreground_pid)) };
        // The focus sits on the stand-in Windows shows for a hung window.
        if common::window_class(foreground_hwnd) == adapter::GHOST_CLASS
            && let Some(hung) = adapter::visible_windows()
                .into_iter()
                .find(|window| adapter::is_hung(window.hwnd))
        {
            foreground_pid = hung.pid;
        }

        let list_through_win32 = || {
            adapter::visible_windows()
                .into_iter()
                .filter_map(|window| {
                    let title = common::window_title(window.hwnd);
                    (!title.is_empty()).then(|| {
                        let class = common::window_class(window.hwnd);
                        (window.pid, class, title, window.pid == foreground_pid)
                    })
                })
                .collect::<Vec<_>>()
        };
        if self.desktop_may_stall() {
            return list_through_win32();
        }

        // Get root element
        let root = match unsafe { self.automation.GetRootElement() } {
            Ok(r) => r,
            Err(_) => return windows,
        };

        // Create condition to find all children
        let condition = match unsafe { self.automation.CreateTrueCondition() } {
            Ok(c) => c,
            Err(_) => return windows,
        };

        // Get all top-level windows
        let all_windows = match unsafe { root.FindAll(TreeScope_Children, &condition) } {
            Ok(w) => w,
            Err(_) => return list_through_win32(),
        };

        let count = unsafe { all_windows.Length().unwrap_or(0) };

        for i in 0..count {
            if let Ok(window) = unsafe { all_windows.GetElement(i) } {
                // Get PID via window handle
                let mut window_pid: u32 = 0;
                if let Ok(native_hwnd) = unsafe { window.CurrentNativeWindowHandle() } {
                    let hwnd = HWND(native_hwnd.0 as *mut _);
                    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut window_pid)) };
                }

                if window_pid == 0 {
                    continue;
                }

                // Get window name/title
                let window_name: String = unsafe {
                    window
                        .CurrentName()
                        .map(|b| b.to_string())
                        .unwrap_or_default()
                };

                // Skip windows without names (typically system/background)
                if window_name.is_empty() {
                    continue;
                }

                // Get class name as app identifier
                let class_name: String = unsafe {
                    window
                        .CurrentClassName()
                        .map(|b| b.to_string())
                        .unwrap_or_else(|_| "Unknown".to_string())
                };

                if class_name == adapter::GHOST_CLASS {
                    continue;
                }

                let is_focused = window_pid == foreground_pid && foreground_pid != 0;

                windows.push((window_pid, class_name, window_name, is_focused));
            }
        }

        windows
    }

    /// Convert a Windows control type ID to an AccessKit Role.
    fn control_type_to_role(control_type: i32) -> Role {
        match control_type {
            x if x == UIA_ButtonControlTypeId.0 => Role::Button,
            x if x == UIA_CheckBoxControlTypeId.0 => Role::CheckBox,
            x if x == UIA_ComboBoxControlTypeId.0 => Role::ComboBox,
            x if x == UIA_EditControlTypeId.0 => Role::TextInput,
            x if x == UIA_HyperlinkControlTypeId.0 => Role::Link,
            x if x == UIA_ImageControlTypeId.0 => Role::Image,
            x if x == UIA_ListControlTypeId.0 => Role::List,
            x if x == UIA_ListItemControlTypeId.0 => Role::ListItem,
            x if x == UIA_MenuControlTypeId.0 => Role::Menu,
            x if x == UIA_MenuBarControlTypeId.0 => Role::MenuBar,
            x if x == UIA_MenuItemControlTypeId.0 => Role::MenuItem,
            x if x == UIA_ProgressBarControlTypeId.0 => Role::ProgressIndicator,
            x if x == UIA_RadioButtonControlTypeId.0 => Role::RadioButton,
            x if x == UIA_ScrollBarControlTypeId.0 => Role::ScrollBar,
            x if x == UIA_SliderControlTypeId.0 => Role::Slider,
            x if x == UIA_SpinnerControlTypeId.0 => Role::SpinButton,
            x if x == UIA_SplitButtonControlTypeId.0 => Role::Button,
            x if x == UIA_StatusBarControlTypeId.0 => Role::Banner,
            x if x == UIA_TabControlTypeId.0 => Role::TabList,
            x if x == UIA_TabItemControlTypeId.0 => Role::Tab,
            x if x == UIA_TableControlTypeId.0 => Role::Table,
            x if x == UIA_TextControlTypeId.0 => Role::Label,
            x if x == UIA_TitleBarControlTypeId.0 => Role::TitleBar,
            x if x == UIA_ToolBarControlTypeId.0 => Role::Toolbar,
            x if x == UIA_ToolTipControlTypeId.0 => Role::Tooltip,
            x if x == UIA_TreeControlTypeId.0 => Role::Tree,
            x if x == UIA_TreeItemControlTypeId.0 => Role::TreeItem,
            x if x == UIA_WindowControlTypeId.0 => Role::Window,
            x if x == UIA_PaneControlTypeId.0 => Role::Pane,
            x if x == UIA_GroupControlTypeId.0 => Role::Group,
            x if x == UIA_DocumentControlTypeId.0 => Role::Document,
            x if x == UIA_DataGridControlTypeId.0 => Role::Grid,
            x if x == UIA_DataItemControlTypeId.0 => Role::Row,
            x if x == UIA_HeaderControlTypeId.0 => Role::Header,
            x if x == UIA_HeaderItemControlTypeId.0 => Role::ColumnHeader,
            x if x == UIA_SeparatorControlTypeId.0 => Role::Splitter,
            _ => Role::Unknown,
        }
    }

    /// A WinForms combo's open dropdown lists the desktop's top-level windows as
    /// its siblings, so the raw view runs into other applications and back into
    /// this window. A child window of another process, like a WebView2's, stays.
    fn leaves_the_walk(&mut self, child: &IUIAutomationElement) -> bool {
        let Some(handle) =
            cached_i32(child, UIA_NativeWindowHandlePropertyId).filter(|handle| *handle != 0)
        else {
            return false;
        };
        let hwnd = HWND(handle as isize as *mut _);
        if unsafe { GetAncestor(hwnd, GA_PARENT) } != unsafe { GetDesktopWindow() } {
            return false;
        }
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        pid != self.walk_pid || !self.walk_windows.insert(handle)
    }

    /// Build an Element from a UI Automation element.
    fn first_list_children(
        &mut self,
        list: &IUIAutomationElement,
    ) -> windows::core::Result<Vec<IUIAutomationElement>> {
        let walker = unsafe { self.automation.RawViewWalker()? };
        let mut children = Vec::new();
        let mut next = unsafe { walker.GetFirstChildElementBuildCache(list, &self.walk_request) };
        loop {
            let child = match next {
                Ok(child) => child,
                // Past the last child the walker fails too, so only a timeout means more remain.
                Err(error) if error.code().0 == UIA_E_TIMEOUT => {
                    if children.is_empty() {
                        return Err(error);
                    }
                    self.walk_cut_short = true;
                    break;
                }
                Err(_) => break,
            };
            if children.len() == MAX_LIST_CHILDREN || self.walk_started.elapsed() > MAX_WALK_TIME {
                self.walk_cut_short = true;
                break;
            }
            next = unsafe { walker.GetNextSiblingElementBuildCache(&child, &self.walk_request) };
            children.push(child);
        }
        Ok(children)
    }

    fn build_element(
        &mut self,
        native: &IUIAutomationElement,
        depth: usize,
        filter: &TreeFilter,
        element_count: &mut usize,
    ) -> Result<Option<Element>> {
        // Check max elements limit
        if let Some(max) = filter.max_elements
            && *element_count >= max
        {
            return Ok(None);
        }

        // Check max depth limit
        if let Some(max_depth) = filter.max_depth
            && depth > max_depth
        {
            return Ok(None);
        }

        // A slow app gets a partial tree rather than an error at the deadline.
        if self.walk_started.elapsed() > MAX_WALK_TIME {
            if let Some(max) = filter.max_elements {
                *element_count = max;
            }
            return Ok(None);
        }

        // `native` was fetched with `walk_request`, so these read locally.
        let control_type = unsafe { native.CachedControlType()? };
        let role = Self::control_type_to_role(control_type.0);

        let name: String = unsafe {
            native
                .CachedName()
                .map(|b| b.to_string())
                .unwrap_or_default()
        };

        let automation_id: String = unsafe {
            native
                .CachedAutomationId()
                .map(|b| b.to_string())
                .unwrap_or_default()
        };

        // Get bounding rectangle
        let rect = unsafe { native.CachedBoundingRectangle()? };
        let bounds = if rect.right > rect.left && rect.bottom > rect.top {
            Some(Rect::new(
                Point::new(rect.left as f64, rect.top as f64),
                Size::new(
                    (rect.right - rect.left) as f64,
                    (rect.bottom - rect.top) as f64,
                ),
            ))
        } else {
            None
        };

        let enabled = unsafe { native.CachedIsEnabled()?.as_bool() };

        let has_focus = unsafe { native.CachedHasKeyboardFocus()?.as_bool() };

        // Collect element properties
        let title = if name.is_empty() { None } else { Some(name) };
        let identifier = if automation_id.is_empty() {
            None
        } else {
            Some(automation_id)
        };

        let has_value = cached_bool(native, UIA_IsValuePatternAvailablePropertyId);
        let has_range = cached_bool(native, UIA_IsRangeValuePatternAvailablePropertyId);
        let has_toggle = cached_bool(native, UIA_IsTogglePatternAvailablePropertyId);
        let has_selection = cached_bool(native, UIA_IsSelectionItemPatternAvailablePropertyId);

        let mut value = has_value
            .then(|| cached_string(native, UIA_ValueValuePropertyId))
            .flatten();
        if value.is_none() && has_range {
            value =
                cached_f64(native, UIA_RangeValueValuePropertyId).map(|range| range.to_string());
        }
        // Rich documents (Word's body) expose their text only through the Text pattern.
        if value.is_none()
            && role == Role::Document
            && cached_bool(native, UIA_IsTextPatternAvailablePropertyId)
            && let Ok(text_pattern) =
                unsafe { native.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
            && let Ok(range) = unsafe { text_pattern.DocumentRange() }
            && let Ok(text) = unsafe { range.GetText(MAX_DOCUMENT_TEXT) }
        {
            let mut text = text.to_string();
            if text.encode_utf16().count() >= MAX_DOCUMENT_TEXT as usize {
                text.push_str("…[truncated]");
            }
            if !text.is_empty() {
                value = Some(text);
            }
        }

        let checked = if has_toggle {
            match cached_i32(native, UIA_ToggleToggleStatePropertyId) {
                Some(state) if state == ToggleState_On.0 => Some(true),
                Some(state) if state == ToggleState_Off.0 => Some(false),
                _ => None,
            }
        } else if has_selection {
            Some(cached_bool(native, UIA_SelectionItemIsSelectedPropertyId))
        } else {
            None
        };

        let mut actions = Vec::new();
        if cached_bool(native, UIA_IsInvokePatternAvailablePropertyId)
            || has_toggle
            || has_selection
            || cached_bool(native, UIA_IsExpandCollapsePatternAvailablePropertyId)
        {
            actions.push("press".to_string());
        }
        if has_value && !cached_bool(native, UIA_ValueIsReadOnlyPropertyId) {
            actions.push("set_text".to_string());
        }
        if has_range {
            actions.push("increment".to_string());
            actions.push("decrement".to_string());
        }

        // Get children
        let listed = if [
            UIA_ListControlTypeId,
            UIA_DataGridControlTypeId,
            UIA_TableControlTypeId,
            UIA_TreeControlTypeId,
        ]
        .contains(&control_type)
        {
            self.first_list_children(native)
        } else {
            unsafe {
                native.FindAllBuildCache(TreeScope_Children, &self.all_elements, &self.walk_request)
            }
            .map(|children| {
                let count = unsafe { children.Length() }.unwrap_or(0);
                (0..count)
                    .filter_map(|i| unsafe { children.GetElement(i) }.ok())
                    .collect()
            })
        };
        let children: Option<Vec<IUIAutomationElement>> = match listed {
            Ok(children) => Some(children),
            // Nothing read yet means the app is not answering; otherwise keep
            // what was read and report it as truncated.
            Err(error) if error.code().0 == UIA_E_TIMEOUT => {
                if *element_count == 0 {
                    return Err(error.into());
                }
                if let Some(max) = filter.max_elements {
                    *element_count = max;
                }
                None
            }
            // The element disappeared during the walk.
            Err(_) => None,
        };

        let mut children_elements = Vec::new();
        if let Some(children) = &children {
            for child_native in children {
                if self.leaves_the_walk(child_native) {
                    continue;
                }
                match self.build_element(child_native, depth + 1, filter, element_count) {
                    Ok(Some(child_elem)) => children_elements.push(child_elem),
                    Ok(None) => {}
                    Err(error) if format!("{error}").contains("0x80131505") => return Err(error),
                    Err(_) => {}
                }
            }
        }

        // WebView2 exposes the views of a browser window it never shows ("Tab bar",
        // "App bar"), some with bounds, none a control or content element. A web
        // page's wrappers aren't either, so only a childless one can go.
        if children_elements.is_empty()
            && cached_string(native, UIA_FrameworkIdPropertyId).as_deref() == Some("Chrome")
            && (bounds.is_none()
                || (!cached_bool(native, UIA_IsControlElementPropertyId)
                    && !cached_bool(native, UIA_IsContentElementPropertyId)))
        {
            return Ok(None);
        }

        // Store in cache with the final ID
        let (id, elem) = self.cache.store_with_clone(|id| Element {
            id,
            role,
            title,
            description: None,
            value,
            url: None,
            help: None,
            role_description: None,
            identifier,
            bounds,
            enabled,
            focused: has_focus,
            checked,
            native_id: runtime_id_hash(native),
            actions,
            children: children_elements,
        });

        // Store native element reference for later actions
        self.native_elements.insert(id, native.clone());
        *element_count += 1;

        Ok(Some(elem))
    }

    /// Find the root element for a specific PID.
    ///
    /// For UWP apps, the PID from tasklist may not directly match the window's process.
    /// This function tries multiple approaches:
    /// 1. Direct PID match via window handle
    /// 2. For ApplicationFrameWindow (UWP host), check if any child element has matching ProcessId
    fn find_root_for_pid(&self, pid: u32) -> Result<IUIAutomationElement> {
        if self.desktop_may_stall() {
            return self.find_answering_window(pid);
        }

        let root = unsafe {
            self.automation
                .GetRootElement()
                .map_err(|e| anyhow::anyhow!("GetRootElement failed: {:?}", e))?
        };

        let condition = unsafe {
            self.automation
                .CreateTrueCondition()
                .map_err(|e| anyhow::anyhow!("CreateTrueCondition failed: {:?}", e))?
        };

        let Ok(all_windows) = (unsafe { root.FindAll(TreeScope_Children, &condition) }) else {
            return self.find_answering_window(pid);
        };

        let count = unsafe { all_windows.Length()? };

        // First pass: match by PID via the window handle. A process can own
        // empty helper windows (Office's border shadows) listed ahead of its
        // real ones, so prefer the selected window, then the foreground one,
        // then a titled one.
        let foreground_hwnd = unsafe { GetForegroundWindow() };
        let selected_title = self
            .selected_window
            .as_ref()
            .filter(|(selected_pid, _)| *selected_pid == pid)
            .map(|(_, title)| title.as_str());
        let mut selected_foreground = None;
        let mut first_selected = None;
        let mut foreground = None;
        let mut first_titled = None;
        let mut first_match = None;
        for i in 0..count {
            if let Ok(window) = unsafe { all_windows.GetElement(i) }
                && let Ok(native_hwnd) = unsafe { window.CurrentNativeWindowHandle() }
            {
                let hwnd = HWND(native_hwnd.0 as *mut _);
                let mut window_pid: u32 = 0;
                unsafe { GetWindowThreadProcessId(hwnd, Some(&mut window_pid)) };
                if window_pid != pid {
                    continue;
                }
                let title = unsafe { window.CurrentName() }
                    .map(|name| name.to_string())
                    .unwrap_or_default();
                let is_foreground = hwnd == foreground_hwnd;
                // An open Win32 popup menu takes the input without taking the
                // foreground, so nothing else reaches it.
                if unsafe { window.CurrentClassName() }
                    .is_ok_and(|class| class == adapter::POPUP_MENU_CLASS)
                    && unsafe { IsWindowVisible(hwnd) }.as_bool()
                {
                    return Ok(window);
                }
                if selected_title.is_some_and(|selected| title.to_lowercase().contains(selected)) {
                    if is_foreground {
                        selected_foreground = Some(window.clone());
                    }
                    first_selected.get_or_insert(window.clone());
                }
                if is_foreground {
                    foreground = Some(window.clone());
                }
                if !title.is_empty() {
                    first_titled.get_or_insert(window.clone());
                }
                first_match.get_or_insert(window);
            }
        }
        if let Some(window) = selected_foreground
            .or(first_selected)
            .or(foreground)
            .or(first_titled)
            .or(first_match)
        {
            return Ok(window);
        }

        // Second pass: for UWP apps hosted in ApplicationFrameWindow,
        // check if any child element has a matching ProcessId
        for i in 0..count {
            if let Ok(window) = unsafe { all_windows.GetElement(i) } {
                let class_name: String = unsafe {
                    window
                        .CurrentClassName()
                        .map(|b| b.to_string())
                        .unwrap_or_default()
                };

                if class_name == "ApplicationFrameWindow" {
                    // Search all descendants for one with matching PID using UI Automation's ProcessId property
                    if let Ok(descendants) = unsafe {
                        window.FindAll(
                            windows::Win32::UI::Accessibility::TreeScope_Subtree,
                            &self.automation.CreateTrueCondition()?,
                        )
                    } {
                        let desc_count = unsafe { descendants.Length().unwrap_or(0) };
                        for j in 0..desc_count {
                            if let Ok(desc) = unsafe { descendants.GetElement(j) } {
                                // Use CurrentProcessId which returns i32 directly
                                if let Ok(desc_pid) = unsafe { desc.CurrentProcessId() }
                                    && desc_pid as u32 == pid
                                {
                                    // Found a descendant with matching PID, return the host window
                                    return Ok(window);
                                }
                            }
                        }
                    }
                }
            }
        }

        bail!(
            "Could not find window for PID {} (found {} top-level windows)",
            pid,
            count
        )
    }

    /// Capture a screenshot of a specific window.
    pub fn capture_window(&self, pid: u32) -> Result<Screenshot> {
        use windows::Win32::Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC,
            DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject,
        };
        use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};

        // Find the window for this PID
        let element = self.find_root_for_pid(pid)?;
        let native_hwnd = unsafe { element.CurrentNativeWindowHandle()? };
        let hwnd = HWND(native_hwnd.0 as *mut _);

        // Get window rect
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect)? };

        let width = (rect.right - rect.left) as u32;
        let height = (rect.bottom - rect.top) as u32;

        if width == 0 || height == 0 {
            bail!("Window has zero size");
        }

        // Create device contexts
        let hdc_screen = unsafe { GetDC(Some(hwnd)) };
        let hdc_mem = unsafe { CreateCompatibleDC(Some(hdc_screen)) };
        let hbitmap = unsafe { CreateCompatibleBitmap(hdc_screen, width as i32, height as i32) };

        unsafe { SelectObject(hdc_mem, hbitmap.into()) };

        // Capture the window using PrintWindow (works with UWP apps)
        // PW_RENDERFULLCONTENT (0x02) captures the full content including DirectComposition
        const PW_RENDERFULLCONTENT: u32 = 0x02;
        let print_result =
            unsafe { PrintWindow(hwnd, hdc_mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)) };

        if !print_result.as_bool() {
            // PrintWindow failed, clean up and return error
            unsafe {
                let _ = DeleteObject(hbitmap.into());
                let _ = DeleteDC(hdc_mem);
                ReleaseDC(Some(hwnd), hdc_screen);
            };
            bail!("PrintWindow failed to capture window content");
        }

        // Create bitmap info
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32), // Negative for top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [Default::default()],
        };

        // Get the bits
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        unsafe {
            GetDIBits(
                hdc_mem,
                hbitmap,
                0,
                height,
                Some(pixels.as_mut_ptr() as *mut _),
                &mut bmi,
                DIB_RGB_COLORS,
            )
        };

        // Cleanup GDI objects
        unsafe {
            let _ = DeleteObject(hbitmap.into());
            let _ = DeleteDC(hdc_mem);
            ReleaseDC(Some(hwnd), hdc_screen);
        };

        // Convert BGRA to RGBA
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.swap(0, 2); // Swap B and R
        }

        // Encode to PNG
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_raw(width, height, pixels)
            .ok_or_else(|| anyhow::anyhow!("Failed to create image buffer"))?;

        let mut png_data = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut png_data);
        img.write_to(&mut cursor, image::ImageFormat::Png)?;

        Ok(Screenshot {
            data: png_data,
            width,
            height,
        })
    }

    /// Get the bounds of the entire virtual screen.
    pub fn get_screen_bounds() -> Rect {
        let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) } as f64;
        let y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) } as f64;
        let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) } as f64;
        let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) } as f64;
        Rect::new(Point::new(x, y), Size::new(width, height))
    }

    /// Get the bounds of the main window for a given PID.
    ///
    /// Returns the window bounds in screen coordinates, or None if no window found.
    pub fn get_window_bounds_for_pid(&self, pid: u32) -> Option<Rect> {
        let element = self.find_root_for_pid(pid).ok()?;
        let native_hwnd = unsafe { element.CurrentNativeWindowHandle().ok()? };
        let hwnd = HWND(native_hwnd.0 as *mut _);

        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd, &mut rect).ok()? };

        Some(Rect::new(
            Point::new(rect.left as f64, rect.top as f64),
            Size::new(
                (rect.right - rect.left) as f64,
                (rect.bottom - rect.top) as f64,
            ),
        ))
    }

    /// Capture the entire screen.
    pub fn capture_screen(&self) -> Result<Screenshot> {
        use windows::Win32::Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap,
            CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits,
            ReleaseDC, SRCCOPY, SelectObject,
        };

        // Get virtual screen dimensions
        let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) } as u32;
        let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) } as u32;

        if width == 0 || height == 0 {
            bail!("Screen has zero size");
        }

        // Create device contexts (None for desktop DC)
        let hdc_screen = unsafe { GetDC(None) };
        let hdc_mem = unsafe { CreateCompatibleDC(Some(hdc_screen)) };
        let hbitmap = unsafe { CreateCompatibleBitmap(hdc_screen, width as i32, height as i32) };

        unsafe { SelectObject(hdc_mem, hbitmap.into()) };

        // Capture the screen
        unsafe {
            BitBlt(
                hdc_mem,
                0,
                0,
                width as i32,
                height as i32,
                Some(hdc_screen),
                x,
                y,
                SRCCOPY,
            )?
        };

        // Create bitmap info
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [Default::default()],
        };

        // Get the bits
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        unsafe {
            GetDIBits(
                hdc_mem,
                hbitmap,
                0,
                height,
                Some(pixels.as_mut_ptr() as *mut _),
                &mut bmi,
                DIB_RGB_COLORS,
            )
        };

        // Cleanup GDI objects
        unsafe {
            let _ = DeleteObject(hbitmap.into());
            let _ = DeleteDC(hdc_mem);
            ReleaseDC(None, hdc_screen);
        };

        // Convert BGRA to RGBA
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }

        // Encode to PNG
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_raw(width, height, pixels)
            .ok_or_else(|| anyhow::anyhow!("Failed to create image buffer"))?;

        let mut png_data = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut png_data);
        img.write_to(&mut cursor, image::ImageFormat::Png)?;

        Ok(Screenshot {
            data: png_data,
            width,
            height,
        })
    }

    /// Internal mouse click implementation at current position.
    fn mouse_click_internal(&mut self, button: MouseButton) -> Result<()> {
        let (down_flag, up_flag) = match button {
            MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        };

        let input_down = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: down_flag,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        let input_up = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: 0,
                    dwFlags: up_flag,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        let down_inserted =
            unsafe { SendInput(&[input_down], std::mem::size_of::<INPUT>() as i32) };
        if down_inserted != 1 {
            bail!("SendInput failed to insert mouse down event");
        }
        let up_inserted = unsafe { SendInput(&[input_up], std::mem::size_of::<INPUT>() as i32) };
        if up_inserted != 1 {
            bail!("SendInput failed to insert mouse up event");
        }
        Ok(())
    }

    /// Internal keystroke implementation.
    fn keystroke_internal(&mut self, key: Code, modifiers: Modifiers) -> Result<()> {
        // Press modifiers
        if modifiers.contains(Modifiers::CONTROL) {
            send_key_event(code_to_vk(Code::ControlLeft), false)?;
        }
        if modifiers.contains(Modifiers::ALT) {
            send_key_event(code_to_vk(Code::AltLeft), false)?;
        }
        if modifiers.contains(Modifiers::SHIFT) {
            send_key_event(code_to_vk(Code::ShiftLeft), false)?;
        }
        if modifiers.contains(Modifiers::META) {
            send_key_event(code_to_vk(Code::MetaLeft), false)?;
        }

        // Press and release the key
        let vk = code_to_vk(key);
        send_key_event(vk, false)?;
        send_key_event(vk, true)?;

        // Release modifiers in reverse order
        if modifiers.contains(Modifiers::META) {
            send_key_event(code_to_vk(Code::MetaLeft), true)?;
        }
        if modifiers.contains(Modifiers::SHIFT) {
            send_key_event(code_to_vk(Code::ShiftLeft), true)?;
        }
        if modifiers.contains(Modifiers::ALT) {
            send_key_event(code_to_vk(Code::AltLeft), true)?;
        }
        if modifiers.contains(Modifiers::CONTROL) {
            send_key_event(code_to_vk(Code::ControlLeft), true)?;
        }

        Ok(())
    }
}
