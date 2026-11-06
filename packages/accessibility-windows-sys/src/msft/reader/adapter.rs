use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::{ERROR_TIMEOUT, GetLastError, WPARAM};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GA_ROOTOWNER, GetWindowThreadProcessId, IsHungAppWindow, IsWindowVisible,
    SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_NULL,
};
use windows::core::BOOL;

use super::events::{EventCallback, run_windows_event_loop};
use super::*;
use crate::msft::input::{code_to_vk, send_key_event};

impl WindowsAccessibility {
    pub async fn get_tree(&mut self, pid: u32, filter: &TreeFilter) -> Result<ElementTree> {
        // Clear previous state
        self.clear_cache();

        let root_element = self.find_root_for_pid(pid)?;
        self.partial_view = (self.process_may_stall(pid)
            && visible_windows()
                .iter()
                .filter(|window| window.pid == pid)
                .count()
                > 1)
        .then(|| {
            let name = unsafe { root_element.CurrentName() }
                .map(|name| name.to_string())
                .unwrap_or_default();
            if !name.is_empty() {
                return name;
            }
            unsafe { root_element.CurrentClassName() }
                .map(|class| class.to_string())
                .unwrap_or_default()
        });
        let root_element = unsafe { root_element.BuildUpdatedCache(&self.walk_request)? };
        self.walk_started = std::time::Instant::now();

        // Get app name
        let app_name: Option<String> = unsafe {
            root_element
                .CurrentName()
                .ok()
                .map(|b| b.to_string())
                .filter(|s| !s.is_empty())
        };

        let mut element_count = 0;
        self.walk_cut_short = false;
        let root = self
            .build_element(&root_element, 0, filter, &mut element_count)?
            .ok_or_else(|| anyhow::anyhow!("Failed to build root element"))?;
        if self.walk_cut_short {
            element_count = element_count.max(filter.max_elements.unwrap_or(element_count));
        }

        Ok(ElementTree {
            version: self.cache.version(),
            pid: Some(pid),
            app_name,
            root,
            element_count,
        })
    }

    pub fn get_element(&self, id: ElementKey) -> Option<&Element> {
        self.cache.get(id)
    }

    pub async fn perform_action(&mut self, id: ElementKey, action: Action) -> Result<()> {
        self.begin_action(id);
        let native = self
            .native_elements
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Element not found: {}", id))?;
        match action {
            Action::Click => {
                // Try Invoke pattern first
                if let Ok(invoke_pattern) = unsafe {
                    native.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                } {
                    self.pending_action =
                        dispatch(invoke_pattern, |pattern| unsafe { pattern.Invoke() })?;
                    return Ok(());
                }
                // Ribbon toggles, tabs and list items have no Invoke.
                if let Ok(toggle) = unsafe {
                    native.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                } {
                    self.pending_action = dispatch(toggle, |pattern| unsafe { pattern.Toggle() })?;
                    return Ok(());
                }
                if let Ok(selection) = unsafe {
                    native.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    )
                } {
                    self.pending_action =
                        dispatch(selection, |pattern| unsafe { pattern.Select() })?;
                    return Ok(());
                }
                if let Ok(expander) = unsafe {
                    native.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                        UIA_ExpandCollapsePatternId,
                    )
                } {
                    let state = unsafe { expander.CurrentExpandCollapseState()? };
                    let expand = state == ExpandCollapseState_Collapsed
                        || state == ExpandCollapseState_PartiallyExpanded;
                    self.pending_action = dispatch(expander, move |pattern| unsafe {
                        if expand {
                            pattern.Expand()
                        } else {
                            pattern.Collapse()
                        }
                    })?;
                    return Ok(());
                }
                bail!("Element does not support click/invoke action");
            }
            Action::Focus => {
                unsafe { native.SetFocus()? };
                Ok(())
            }
            Action::SetValue => {
                bail!("SetValue action requires using set_value() method");
            }
            Action::Increment | Action::Decrement => {
                let range = unsafe {
                    native.GetCurrentPatternAs::<IUIAutomationRangeValuePattern>(
                        UIA_RangeValuePatternId,
                    )
                }
                .map_err(|_| anyhow::anyhow!("Element does not support the RangeValue pattern"))?;
                let step = unsafe { range.CurrentSmallChange()? };
                let step = if step > 0.0 { step } else { 1.0 };
                let delta = if action == Action::Increment {
                    step
                } else {
                    -step
                };
                let next = unsafe { range.CurrentValue()? } + delta;
                let next = next.clamp(unsafe { range.CurrentMinimum()? }, unsafe {
                    range.CurrentMaximum()?
                });
                self.pending_action =
                    dispatch(range, move |pattern| unsafe { pattern.SetValue(next) })?;
                Ok(())
            }
            Action::ShowContextMenu => {
                let element = native
                    .cast::<IUIAutomationElement3>()
                    .map_err(|_| anyhow::anyhow!("Element has no native context menu"))?;
                self.pending_action =
                    dispatch(element, |pattern| unsafe { pattern.ShowContextMenu() })?;
                Ok(())
            }
            _ => bail!("Action {:?} not implemented for Windows", action),
        }
    }

    pub async fn set_value(&mut self, id: ElementKey, value: &str) -> Result<()> {
        self.begin_action(id);
        let native = self
            .native_elements
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Element not found: {}", id))?;
        let value_pattern =
            unsafe { native.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) }
                .map_err(|_| anyhow::anyhow!("Element does not support the Value pattern"))?;

        let value = value.to_string();
        self.pending_action = dispatch(value_pattern, move |pattern| unsafe {
            pattern.SetValue(&BSTR::from(value.as_str()))
        })?;
        Ok(())
    }

    /// Whether the last action's call into the app was still running when it was left behind.
    pub fn action_pending(&self) -> bool {
        self.pending_action.is_some()
    }

    /// The window a walk was limited to because a pattern call is still running.
    pub fn partial_view(&self) -> Option<&str> {
        self.partial_view.as_deref()
    }

    fn park_pending_action(&mut self) {
        let pid = self.pending_action_pid;
        self.running_actions
            .extend(self.pending_action.take().map(|action| (pid, action)));
    }

    fn begin_action(&mut self, id: ElementKey) {
        self.park_pending_action();
        self.pending_action_pid = self
            .native_elements
            .get(id)
            .and_then(|native| unsafe { native.CurrentProcessId() }.ok())
            .map_or(0, |pid| pid as u32);
    }

    /// Whether some windows of this process may not answer, so that one view of
    /// it is not the whole application.
    fn process_may_stall(&self, pid: u32) -> bool {
        let running = |action: &Receiver<windows::core::Result<()>>| {
            matches!(action.try_recv(), Err(TryRecvError::Empty))
        };
        (self.pending_action_pid == pid && self.pending_action.as_ref().is_some_and(running))
            || self
                .running_actions
                .iter()
                .any(|(action_pid, action)| *action_pid == pid && running(action))
            || visible_windows().iter().any(|window| {
                window.pid == pid && unsafe { IsHungAppWindow(window.hwnd) }.as_bool()
            })
    }

    /// Whether requests touching every window, like listing the desktop, may
    /// stall: on a window waiting on a pattern call, or on one that stopped pumping.
    pub(super) fn desktop_may_stall(&self) -> bool {
        self.action_running() || visible_windows().iter().any(|window| is_hung(window.hwnd))
    }

    pub(super) fn action_running(&self) -> bool {
        self.pending_action
            .iter()
            .chain(self.running_actions.iter().map(|(_, action)| action))
            .any(|action| matches!(action.try_recv(), Err(TryRecvError::Empty)))
    }

    /// Releasing elements can wait on a window handling a pattern call, so hand
    /// them to a thread that may wait instead.
    fn release_native_elements(&mut self) {
        self.park_pending_action();
        self.running_actions
            .retain(|(_, action)| matches!(action.try_recv(), Err(TryRecvError::Empty)));
        if self.running_actions.is_empty() {
            self.native_elements.clear();
            return;
        }
        let elements = ComPointer(std::mem::take(&mut self.native_elements));
        std::thread::spawn(move || {
            let elements = elements;
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            drop(elements);
            unsafe { CoUninitialize() };
        });
    }

    /// While a pattern call runs, this process's requests to the windows waiting
    /// on it stall until it returns, and they ignore the transaction timeout,
    /// while a dialog the call opened still answers. Listing the desktop stalls
    /// too, so find the windows through Win32 and use the topmost that answers.
    pub(super) fn find_answering_window(&self, pid: u32) -> Result<IUIAutomationElement> {
        let foreground = unsafe { GetForegroundWindow() };
        let selected = self
            .selected_window
            .as_ref()
            .filter(|(selected_pid, _)| *selected_pid == pid)
            .map(|(_, title)| title.as_str());
        let (answer_tx, answer_rx) = std::sync::mpsc::channel();
        let mut foreground_dialog_probed = false;
        let windows = visible_windows();
        let windows = windows
            .iter()
            .filter(|window| window.pid == pid)
            .take(MAX_PROBED_WINDOWS);
        for (index, window) in windows.enumerate() {
            let handle = window.hwnd.0 as isize;
            let stalled = Arc::clone(&self.stalled_probes);
            // A window that has not answered its last probe will not answer another.
            if !stalled
                .lock()
                .expect("probe set lock poisoned")
                .insert(handle)
            {
                continue;
            }
            let title = common::window_title(window.hwnd);
            let is_selected =
                selected.is_some_and(|selected| title.to_lowercase().contains(selected));
            let answer = WindowAnswer {
                selected: is_selected,
                // A dialog on top of the selected window is what can be acted on,
                // but one on top of another window must not hide the selected one.
                immediate: window.hwnd == foreground
                    && selected.is_none_or(|selected| {
                        let owner = unsafe { GetAncestor(window.hwnd, GA_ROOTOWNER) };
                        owner != window.hwnd
                            && common::window_title(owner)
                                .to_lowercase()
                                .contains(selected)
                    }),
                titled: !title.is_empty(),
                index,
            };
            foreground_dialog_probed |= answer.immediate;
            let automation = ComPointer(self.automation.clone());
            let answer_tx = answer_tx.clone();
            std::thread::spawn(move || {
                let automation = automation;
                let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                let hwnd = HWND(handle as *mut _);
                // A window that answers one request is not waiting on the pattern call.
                if let Ok(element) = unsafe { automation.0.ElementFromHandle(hwnd) }
                    && unsafe { element.CurrentName() }.is_ok()
                {
                    let _ = answer_tx.send((answer, ComPointer(element)));
                }
                stalled
                    .lock()
                    .expect("probe set lock poisoned")
                    .remove(&handle);
                drop(automation);
                unsafe { CoUninitialize() };
            });
        }
        drop(answer_tx);

        let deadline = std::time::Instant::now() + WINDOW_PROBE_WAIT;
        let mut answers = Vec::new();
        while let Ok(answer) =
            answer_rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        {
            if answer.0.immediate || (answer.0.selected && !foreground_dialog_probed) {
                return Ok(answer.1.0);
            }
            answers.push(answer);
        }
        answers.sort_by_key(|(answer, _)| (!answer.selected, !answer.titled, answer.index));
        match answers.into_iter().next() {
            Some((_, element)) => Ok(element.0),
            None => bail!(
                "blocked: this application is not answering accessibility requests, often because it is still handling your last action — do not repeat the action; wait, then inspect again"
            ),
        }
    }

    /// Refuse a disabled element or one inside a disabled window. UI Automation
    /// patterns ignore both: Toggle on a ribbon button changes the document
    /// underneath an open modal dialog, which disables its owner window.
    pub fn ensure_actionable(&self, id: ElementKey) -> Result<()> {
        let native = self
            .native_elements
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Element not found: {}", id))?;
        if !unsafe { native.CurrentIsEnabled()? }.as_bool() {
            bail!("unsupported_operation: this element is disabled");
        }
        let walker = unsafe { self.automation.RawViewWalker()? };
        // UIA nests an owned dialog under its owner, which the dialog itself
        // disables, so only ancestors within the same top-level HWND count.
        let is_top_level = |element: &IUIAutomationElement| {
            unsafe { element.CurrentNativeWindowHandle() }.is_ok_and(|handle| {
                let hwnd = HWND(handle.0 as *mut _);
                !hwnd.is_invalid() && unsafe { GetAncestor(hwnd, GA_ROOT) } == hwnd
            })
        };
        let mut current = native.clone();
        while !is_top_level(&current)
            && let Ok(parent) = unsafe { walker.GetParentElement(&current) }
        {
            if !unsafe { parent.CurrentIsEnabled()? }.as_bool() {
                bail!(
                    "blocked: this element's window is disabled, usually because a modal dialog is open — inspect, then close the dialog first"
                );
            }
            current = parent;
        }
        Ok(())
    }

    /// Raise the element's window and return its center for a pointer click.
    /// Errors carry the provider's controlled prefixes: a click that would land
    /// on another control must be refused, not sent.
    pub fn prepare_pointer_target(&self, id: ElementKey, pid: u32) -> Result<(f64, f64)> {
        let native = self
            .native_elements
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Element not found: {}", id))?;
        self.focus_window(pid)?;
        if unsafe { native.CurrentIsOffscreen()? }.as_bool()
            && let Ok(scroll) = unsafe {
                native
                    .GetCurrentPatternAs::<IUIAutomationScrollItemPattern>(UIA_ScrollItemPatternId)
            }
        {
            let _ = unsafe { scroll.ScrollIntoView() };
        }
        let rect = unsafe { native.CurrentBoundingRectangle()? };
        if rect.right <= rect.left
            || rect.bottom <= rect.top
            || unsafe { native.CurrentIsOffscreen()? }.as_bool()
        {
            bail!(
                "unsupported_operation: this element has no native action and is not on screen to click — scroll to it or open its parent first"
            );
        }
        let center = POINT {
            x: rect.left + (rect.right - rect.left) / 2,
            y: rect.top + (rect.bottom - rect.top) / 2,
        };
        // Off every monitor, SendInput still succeeds but the click lands nowhere.
        if unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONULL) }.is_invalid() {
            bail!(
                "unsupported_operation: this element's center is off every monitor, so a pointer click cannot reach it — move its window on screen first"
            );
        }
        if let Ok(hit) = unsafe { self.automation.ElementFromPoint(center) }
            && !self.is_same_branch(native, &hit)
        {
            let covering = unsafe { hit.CurrentName() }
                .map(|name| name.to_string())
                .unwrap_or_default();
            bail!(
                "blocked: {covering:?} covers this element, so a click would land on it instead — close or dismiss it, then inspect again"
            );
        }
        Ok((center.x as f64, center.y as f64))
    }

    /// Whether `hit` is `target`, inside it, or an ancestor of it below the desktop.
    fn is_same_branch(&self, target: &IUIAutomationElement, hit: &IUIAutomationElement) -> bool {
        const MAX_ANCESTORS: usize = 128;
        let same = |a: &IUIAutomationElement, b: &IUIAutomationElement| {
            unsafe { self.automation.CompareElements(a, b) }
                .map(|same| same.as_bool())
                .unwrap_or(false)
        };
        let Ok(walker) = (unsafe { self.automation.RawViewWalker() }) else {
            return true;
        };
        let Ok(desktop) = (unsafe { self.automation.GetRootElement() }) else {
            return true;
        };
        let reaches = |from: &IUIAutomationElement, goal: &IUIAutomationElement| {
            let mut current = from.clone();
            for _ in 0..MAX_ANCESTORS {
                if same(&current, goal) {
                    return true;
                }
                if same(&current, &desktop) {
                    return false;
                }
                match unsafe { walker.GetParentElement(&current) } {
                    Ok(parent) => current = parent,
                    Err(_) => return false,
                }
            }
            false
        };
        reaches(hit, target) || (!same(hit, &desktop) && reaches(target, hit))
    }

    pub async fn hit_test(&mut self, x: f64, y: f64) -> Result<Option<ElementKey>> {
        let point = POINT {
            x: x as i32,
            y: y as i32,
        };
        let element = unsafe { self.automation.ElementFromPoint(point)? };

        // Get the name and control type of the hit element for comparison
        let hit_name: String = unsafe {
            element
                .CurrentName()
                .map(|b| b.to_string())
                .unwrap_or_default()
        };
        let hit_control_type = unsafe { element.CurrentControlType().ok() };

        // Check if this element is already in our cache by comparing properties
        for (key, native) in &self.native_elements {
            let native_name: String = unsafe {
                native
                    .CurrentName()
                    .map(|b| b.to_string())
                    .unwrap_or_default()
            };
            let native_control_type = unsafe { native.CurrentControlType().ok() };

            // Match by name and control type
            if native_name == hit_name && native_control_type == hit_control_type {
                // Also compare bounding rectangles for more accuracy
                if let (Ok(native_rect), Ok(hit_rect)) = unsafe {
                    (
                        native.CurrentBoundingRectangle(),
                        element.CurrentBoundingRectangle(),
                    )
                } && native_rect.left == hit_rect.left
                    && native_rect.top == hit_rect.top
                    && native_rect.right == hit_rect.right
                    && native_rect.bottom == hit_rect.bottom
                {
                    return Ok(Some(key));
                }
            }
        }

        let element = unsafe { element.BuildUpdatedCache(&self.walk_request)? };
        self.walk_started = std::time::Instant::now();
        let mut element_count = 0;
        let Some(element) =
            self.build_element(&element, 0, &TreeFilter::default(), &mut element_count)?
        else {
            return Ok(None);
        };

        Ok(Some(element.id))
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.release_native_elements();
    }

    pub fn snapshot_version(&self) -> u64 {
        self.cache.version()
    }

    // Platform adapter methods (merged from WindowsAdapter)

    pub fn capture_screen_for_pid(&self, pid: u32) -> Result<Screenshot> {
        if let Ok(screenshot) = WindowsAccessibility::capture_window(self, pid) {
            return Ok(screenshot);
        }
        WindowsAccessibility::capture_screen(self)
    }

    pub async fn get_screen_bounds_for_pid(&self, pid: u32) -> Result<Rect> {
        if let Some(bounds) = self.get_window_bounds_for_pid(pid) {
            return Ok(bounds);
        }
        Ok(Self::get_screen_bounds())
    }

    pub fn platform_name(&self) -> &'static str {
        "Windows"
    }

    pub async fn keystroke(&mut self, key: Code, modifiers: Modifiers) -> Result<()> {
        self.keystroke_internal(key, modifiers)
    }

    pub async fn mouse_click_at(&mut self, x: f64, y: f64, button: MouseButton) -> Result<()> {
        // Send move + down + up as one atomic `SendInput` batch with absolute
        // coordinates on every event. Separate calls are flaky on UWP hosts
        // because the OS can coalesce or reorder them, dispatching the down
        // event before the cursor-tracking state has caught up.
        let screen_width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) } as f64;
        let screen_height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) } as f64;
        let screen_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) } as f64;
        let screen_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) } as f64;
        if screen_width <= 0.0 || screen_height <= 0.0 {
            bail!(
                "Virtual desktop reports non-positive dimensions ({} x {})",
                screen_width,
                screen_height
            );
        }

        let norm_x = ((x - screen_x) * 65535.0 / screen_width) as i32;
        let norm_y = ((y - screen_y) * 65535.0 / screen_height) as i32;
        let abs_flags = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        let (down_flag, up_flag) = match button {
            MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        };

        let make = |flags| INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: norm_x,
                    dy: norm_y,
                    mouseData: 0,
                    dwFlags: flags | abs_flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        let inputs = [make(MOUSEEVENTF_MOVE), make(down_flag), make(up_flag)];

        let inserted = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        if inserted as usize != inputs.len() {
            bail!(
                "SendInput inserted {}/{} mouse events",
                inserted,
                inputs.len()
            );
        }
        Ok(())
    }

    pub async fn press_key(&mut self, key: Code) -> Result<()> {
        let vk = code_to_vk(key);
        send_key_event(vk, false)
    }

    pub async fn release_key(&mut self, key: Code) -> Result<()> {
        let vk = code_to_vk(key);
        send_key_event(vk, true)
    }

    pub async fn mouse_move(&mut self, x: f64, y: f64) -> Result<()> {
        // Get screen dimensions for absolute positioning
        let screen_width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) } as f64;
        let screen_height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) } as f64;
        let screen_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) } as f64;
        let screen_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) } as f64;

        // Convert to normalized coordinates (0-65535)
        let norm_x = ((x - screen_x) * 65535.0 / screen_width) as i32;
        let norm_y = ((y - screen_y) * 65535.0 / screen_height) as i32;

        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: norm_x,
                    dy: norm_y,
                    mouseData: 0,
                    dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        let inserted = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
        if inserted != 1 {
            bail!("SendInput failed to insert mouse move event");
        }
        Ok(())
    }

    pub async fn mouse_click(&mut self, button: MouseButton) -> Result<()> {
        self.mouse_click_internal(button)
    }

    pub async fn mouse_double_click(&mut self, button: MouseButton) -> Result<()> {
        self.mouse_click_internal(button)?;
        self.mouse_click_internal(button)
    }

    pub async fn mouse_scroll(&mut self, _delta_x: f64, delta_y: f64) -> Result<()> {
        // WHEEL_DELTA is 120. The mouseData field is interpreted as a signed value
        let wheel_delta_signed = (delta_y * 120.0) as i32;
        let wheel_delta = u32::from_ne_bytes(wheel_delta_signed.to_ne_bytes());

        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: wheel_delta,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };

        let inserted = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
        if inserted != 1 {
            bail!("SendInput failed to insert scroll event");
        }
        Ok(())
    }

    pub fn supports_keystroke(&self) -> bool {
        true
    }

    pub fn supports_mouse_click(&self) -> bool {
        true
    }

    pub fn supports_hit_test(&self) -> bool {
        true
    }

    // Event listening implementation

    pub fn run_event_loop(
        config: ListenerConfig,
        callback: Box<dyn FnMut(AccessibilityEvent) + Send + 'static>,
        stop_flag: Arc<AtomicBool>,
    ) -> Result<()> {
        // Determine target PID - must be specified in config
        let target_pid = config.pid.ok_or_else(|| {
            anyhow::anyhow!(
                "No target PID specified for event listening (set pid in ListenerConfig)"
            )
        })?;

        // Wrap callback in Arc<Mutex> for thread-safe access
        let callback: Arc<Mutex<EventCallback>> = Arc::new(Mutex::new(callback));

        run_windows_event_loop(target_pid, config, callback, stop_flag);
        Ok(())
    }

    pub fn supports_event_listening(&self) -> bool {
        true
    }

    pub fn supported_event_types(&self) -> Vec<AccessibilityEventType> {
        vec![
            AccessibilityEventType::FocusChanged,
            AccessibilityEventType::ValueChanged,
            AccessibilityEventType::TitleChanged,
            AccessibilityEventType::StructureChanged,
            AccessibilityEventType::WindowCreated,
            AccessibilityEventType::WindowDestroyed,
        ]
    }
}

/// How long an action waits for the app before carrying on without it.
const ACTION_WAIT: Duration = Duration::from_secs(5);

struct WindowAnswer {
    selected: bool,
    immediate: bool,
    titled: bool,
    index: usize,
}

pub(super) struct VisibleWindow {
    pub(super) hwnd: HWND,
    pub(super) pid: u32,
}

/// Visible top-level windows in z-order, without a UI Automation request.
/// The "Not Responding" stand-in Windows shows over a hung window, owned by
/// another process.
pub(super) const GHOST_CLASS: &str = "Ghost";

const PUMP_CHECK_TIMEOUT_MS: u32 = 250;
const PUMP_CHECK_MEMORY: std::time::Duration = std::time::Duration::from_secs(1);

thread_local! {
    static SILENT_THREADS: std::cell::RefCell<std::collections::HashMap<u32, std::time::Instant>> =
        std::cell::RefCell::default();
}

/// Whether the window's thread stopped pumping. `IsHungAppWindow` alone misses a
/// suspended process and anything else that has no input waiting, so ask the
/// window too. A silent thread is remembered briefly: scans visit its every window.
pub(super) fn is_hung(hwnd: HWND) -> bool {
    if unsafe { IsHungAppWindow(hwnd) }.as_bool() {
        return true;
    }
    let thread = unsafe { GetWindowThreadProcessId(hwnd, None) };
    let remembered = SILENT_THREADS.with_borrow_mut(|silent| {
        silent.retain(|_, since| since.elapsed() < PUMP_CHECK_MEMORY);
        silent.contains_key(&thread)
    });
    if remembered {
        return true;
    }
    let answered = unsafe {
        SendMessageTimeoutW(
            hwnd,
            WM_NULL,
            WPARAM(0),
            LPARAM(0),
            SMTO_BLOCK | SMTO_ABORTIFHUNG,
            PUMP_CHECK_TIMEOUT_MS,
            None,
        )
    };
    let silent = answered.0 == 0 && unsafe { GetLastError() } == ERROR_TIMEOUT;
    if silent {
        SILENT_THREADS.with_borrow_mut(|threads| {
            threads.insert(thread, std::time::Instant::now());
        });
    }
    silent
}

pub(super) fn visible_windows() -> Vec<VisibleWindow> {
    unsafe extern "system" fn collect(hwnd: HWND, list: LPARAM) -> BOOL {
        let list = unsafe { &mut *(list.0 as *mut Vec<VisibleWindow>) };
        if unsafe { IsWindowVisible(hwnd) }.as_bool() && common::window_class(hwnd) != GHOST_CLASS {
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
            list.push(VisibleWindow { hwnd, pid });
        }
        BOOL(1)
    }
    let mut list = Vec::new();
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut list as *mut _ as isize)) };
    list
}

struct ComPointer<T>(T);
// SAFETY: created in the multithreaded apartment and used only from another MTA thread.
unsafe impl<T> Send for ComPointer<T> {}

/// How long reads wait for windows to answer while a pattern call runs.
const WINDOW_PROBE_WAIT: Duration = Duration::from_secs(2);
const MAX_PROBED_WINDOWS: usize = 8;

/// Run a pattern call on its own thread, returning its pending result if it
/// outlives the wait: these calls wait for the app's handler, ignoring the
/// transaction timeout, and a handler that opens a modal dialog can hold them
/// for a minute.
fn dispatch<P: 'static>(
    pattern: P,
    call: impl FnOnce(&P) -> windows::core::Result<()> + Send + 'static,
) -> Result<Option<Receiver<windows::core::Result<()>>>> {
    let pattern = ComPointer(pattern);
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let pattern = pattern;
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let result = call(&pattern.0);
        drop(pattern);
        let _ = result_tx.send(result);
        unsafe { CoUninitialize() };
    });
    match result_rx.recv_timeout(ACTION_WAIT) {
        Ok(result) => result.map(|()| None).map_err(Into::into),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(Some(result_rx)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            bail!("the action's thread ended without a result")
        }
    }
}
