//! Windows accessibility implementation using UI Automation.
//!
//! Raw Windows UI Automation, GDI, COM, and `SendInput` calls live in
//! `accessibility-windows-sys`. This module keeps the core API on safe Rust
//! types and owns the public `ElementKey` mapping used by the rest of the crate.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use accessibility_windows_sys as sys;
use accesskit::Action;
use anyhow::{Result, anyhow, bail};
use slotmap::SecondaryMap;

use crate::accessibility::{
    AccessibilityEvent, AccessibilityEventType, AccessibilityReader, Element, ElementCache,
    ElementKey, ElementTree, ListenerConfig, ListenerHandle, Point, Rect, Screenshot, Size,
    StopReason, StructureChangeType, Target, TreeFilter,
};
use crate::input::{Code, Modifiers, MouseButton, code_from_char};

pub use sys::{
    WindowBlockerSpec, get_foreground_pid, hide_top_level_windows_matching,
    hide_windows_matching_at_point,
};

/// Windows accessibility reader using UI Automation.
pub struct WindowsAccessibility {
    inner: sys::WindowsAccessibility,
    cache: ElementCache,
    sys_ids: SecondaryMap<ElementKey, sys::ElementKey>,
    core_ids: HashMap<u64, ElementKey>,
}

impl WindowsAccessibility {
    /// Create a new Windows accessibility reader.
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: sys::WindowsAccessibility::new()?,
            cache: ElementCache::new(),
            sys_ids: SecondaryMap::new(),
            core_ids: HashMap::new(),
        })
    }

    /// Focus the window for a given PID.
    pub fn focus_window(&self, pid: u32) -> Result<()> {
        self.inner.focus_window(pid)
    }

    /// List all top-level windows with their PIDs.
    pub fn list_windows(&self) -> Vec<(u32, String, String, bool)> {
        self.inner.list_windows()
    }

    /// Capture a screenshot of a specific window.
    pub fn capture_window(&self, pid: u32) -> Result<Screenshot> {
        self.inner.capture_window(pid).map(from_sys_screenshot)
    }

    /// Get the bounds of the main window for a given PID.
    pub fn get_window_bounds_for_pid(&self, pid: u32) -> Option<Rect> {
        self.inner
            .get_window_bounds_for_pid(pid)
            .as_ref()
            .map(from_sys_rect)
    }

    /// Get the bounds of the entire virtual screen.
    pub fn get_screen_bounds() -> Rect {
        from_sys_rect(&sys::WindowsAccessibility::get_screen_bounds())
    }

    /// Capture the entire screen.
    pub fn capture_screen(&self) -> Result<Screenshot> {
        self.inner.capture_screen().map(from_sys_screenshot)
    }

    async fn get_tree_for_pid(&mut self, pid: u32, filter: &TreeFilter) -> Result<ElementTree> {
        self.clear_local_cache();

        let sys_tree = self.inner.get_tree(pid, &to_sys_filter(filter)).await?;
        let root = self.map_element(&sys_tree.root);
        let element_count = count_elements(&root);

        Ok(ElementTree {
            version: self.cache.version(),
            pid: sys_tree.pid,
            app_name: sys_tree.app_name,
            root,
            element_count,
        })
    }

    fn clear_local_cache(&mut self) {
        self.cache.clear();
        self.sys_ids.clear();
        self.core_ids.clear();
    }

    fn sys_id(&self, id: ElementKey) -> Result<sys::ElementKey> {
        self.sys_ids
            .get(id)
            .copied()
            .ok_or_else(|| anyhow!("Element not found: {}", id))
    }

    fn map_element(&mut self, sys_element: &sys::Element) -> Element {
        if let Some(existing) = self.core_ids.get(&sys_element.id.to_ffi()).copied()
            && let Some(element) = self.cache.get(existing)
        {
            return element.clone();
        }

        let children = sys_element
            .children
            .iter()
            .map(|child| self.map_element(child))
            .collect();
        let sys_id = sys_element.id;

        let (id, element) = self.cache.store_with_clone(|id| Element {
            id,
            role: sys_element.role,
            title: sys_element.title.clone(),
            description: sys_element.description.clone(),
            value: sys_element.value.clone(),
            url: sys_element.url.clone(),
            help: sys_element.help.clone(),
            role_description: sys_element.role_description.clone(),
            identifier: sys_element.identifier.clone(),
            bounds: sys_element.bounds.as_ref().map(from_sys_rect),
            enabled: sys_element.enabled,
            focused: sys_element.focused,
            actions: sys_element.actions.clone(),
            children,
        });

        self.sys_ids.insert(id, sys_id);
        self.core_ids.insert(sys_id.to_ffi(), id);
        element
    }
}

impl AccessibilityReader for WindowsAccessibility {
    async fn get_tree(&mut self, target: &Target, filter: &TreeFilter) -> Result<ElementTree> {
        let pid = target.require_pid("Windows", "accessibility tree queries")?;
        self.get_tree_for_pid(pid, filter).await
    }

    fn get_element(&self, id: ElementKey) -> Option<&Element> {
        self.cache.get(id)
    }

    async fn perform_action(&mut self, id: ElementKey, action: Action) -> Result<()> {
        let sys_id = self.sys_id(id)?;
        self.inner.perform_action(sys_id, action).await
    }

    async fn set_value(&mut self, id: ElementKey, value: &str) -> Result<()> {
        let sys_id = self.sys_id(id)?;
        self.inner.set_value(sys_id, value).await
    }

    async fn hit_test(&mut self, x: f64, y: f64) -> Result<Option<ElementKey>> {
        let Some(sys_id) = self.inner.hit_test(x, y).await? else {
            return Ok(None);
        };

        if let Some(id) = self.core_ids.get(&sys_id.to_ffi()).copied() {
            return Ok(Some(id));
        }

        let Some(sys_element) = self.inner.get_element(sys_id).cloned() else {
            return Ok(None);
        };

        let element = self.map_element(&sys_element);
        Ok(Some(element.id))
    }

    fn clear_cache(&mut self) {
        self.inner.clear_cache();
        self.clear_local_cache();
    }

    fn snapshot_version(&self) -> u64 {
        self.cache.version()
    }

    fn capture_screen(
        &self,
        target: &Target,
    ) -> impl std::future::Future<Output = Result<Screenshot>> {
        async move {
            let screenshot = match target {
                Target::Pid(pid) => self.inner.capture_screen_for_pid(*pid),
                Target::System => self.inner.capture_screen(),
                _ => bail!("Windows screenshot requires Target::Pid or Target::System"),
            };
            screenshot.map(from_sys_screenshot)
        }
    }

    async fn get_screen_bounds(&self, target: &Target) -> Result<Rect> {
        let bounds = match target {
            Target::Pid(pid) => self.inner.get_screen_bounds_for_pid(*pid).await?,
            Target::System => sys::WindowsAccessibility::get_screen_bounds(),
            _ => bail!("Windows screen bounds require Target::Pid or Target::System"),
        };
        Ok(from_sys_rect(&bounds))
    }

    fn platform_name(&self) -> &'static str {
        "Windows"
    }

    async fn keystroke(&mut self, target: &Target, key: Code, modifiers: Modifiers) -> Result<()> {
        let pid = target.require_pid("Windows", "keystroke")?;
        self.focus_window(pid)?;
        self.inner.keystroke(key, modifiers).await
    }

    async fn type_raw(&mut self, target: &Target, text: &str) -> Result<()> {
        let pid = target.require_pid("Windows", "type_raw")?;
        self.focus_window(pid)?;
        for c in text.chars() {
            if let Some((key, needs_shift)) = code_from_char(c) {
                let modifiers = if needs_shift {
                    Modifiers::SHIFT
                } else {
                    Modifiers::empty()
                };
                self.inner.keystroke(key, modifiers).await?;
            }
        }
        Ok(())
    }

    async fn mouse_click_at(
        &mut self,
        target: &Target,
        x: f64,
        y: f64,
        button: MouseButton,
    ) -> Result<()> {
        let pid = target.require_pid("Windows", "mouse_click_at")?;
        self.focus_window(pid)?;
        self.inner
            .mouse_click_at(x, y, to_sys_mouse_button(button))
            .await
    }

    async fn press_key(&mut self, target: &Target, key: Code) -> Result<()> {
        let pid = target.require_pid("Windows", "press_key")?;
        self.focus_window(pid)?;
        self.inner.press_key(key).await
    }

    async fn release_key(&mut self, target: &Target, key: Code) -> Result<()> {
        let pid = target.require_pid("Windows", "release_key")?;
        self.focus_window(pid)?;
        self.inner.release_key(key).await
    }

    async fn mouse_move(&mut self, target: &Target, x: f64, y: f64) -> Result<()> {
        let pid = target.require_pid("Windows", "mouse_move")?;
        self.focus_window(pid)?;
        self.inner.mouse_move(x, y).await
    }

    async fn mouse_click(&mut self, target: &Target, button: MouseButton) -> Result<()> {
        let pid = target.require_pid("Windows", "mouse_click")?;
        self.focus_window(pid)?;
        self.inner.mouse_click(to_sys_mouse_button(button)).await
    }

    async fn mouse_double_click(&mut self, target: &Target, button: MouseButton) -> Result<()> {
        let pid = target.require_pid("Windows", "mouse_double_click")?;
        self.focus_window(pid)?;
        self.inner
            .mouse_double_click(to_sys_mouse_button(button))
            .await
    }

    async fn mouse_scroll(&mut self, target: &Target, delta_x: f64, delta_y: f64) -> Result<()> {
        let pid = target.require_pid("Windows", "mouse_scroll")?;
        self.focus_window(pid)?;
        self.inner.mouse_scroll(delta_x, delta_y).await
    }

    fn supports_keystroke(&self) -> bool {
        self.inner.supports_keystroke()
    }

    fn supports_mouse_click(&self) -> bool {
        self.inner.supports_mouse_click()
    }

    fn supports_hit_test(&self) -> bool {
        self.inner.supports_hit_test()
    }

    fn start_listening(
        &mut self,
        config: ListenerConfig,
        callback: Box<dyn FnMut(AccessibilityEvent) + Send + 'static>,
    ) -> Result<ListenerHandle> {
        if config.pid.is_none() {
            anyhow::bail!(
                "No target PID specified for event listening (set pid in ListenerConfig)"
            );
        }

        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_clone = stop_flag.clone();
        let sys_config = to_sys_listener_config(&config);
        let mut callback = callback;

        let task_handle = tokio::task::spawn_blocking(move || {
            let sys_callback = Box::new(move |event| {
                callback(from_sys_event(event));
            });

            let _ = sys::WindowsAccessibility::run_event_loop(
                sys_config,
                sys_callback,
                stop_flag_clone,
            );
        });

        Ok(ListenerHandle::new(stop_flag, task_handle))
    }

    fn supports_event_listening(&self) -> bool {
        self.inner.supports_event_listening()
    }

    fn supported_event_types(&self) -> Vec<AccessibilityEventType> {
        self.inner
            .supported_event_types()
            .into_iter()
            .map(from_sys_event_type)
            .collect()
    }
}

fn to_sys_filter(filter: &TreeFilter) -> sys::TreeFilter {
    sys::TreeFilter {
        max_depth: filter.max_depth,
        max_elements: filter.max_elements,
        interactive_only: filter.interactive_only,
        visible_only: filter.visible_only,
        within_bounds: filter.within_bounds.as_ref().map(to_sys_rect),
        roles: filter.roles.clone(),
    }
}

fn to_sys_rect(rect: &Rect) -> sys::Rect {
    sys::Rect::new(
        sys::Point::new(rect.origin.x, rect.origin.y),
        sys::Size::new(rect.size.width, rect.size.height),
    )
}

fn from_sys_rect(rect: &sys::Rect) -> Rect {
    Rect::new(
        Point::new(rect.origin.x, rect.origin.y),
        Size::new(rect.size.width, rect.size.height),
    )
}

fn from_sys_screenshot(screenshot: sys::Screenshot) -> Screenshot {
    Screenshot {
        data: screenshot.data,
        width: screenshot.width,
        height: screenshot.height,
    }
}

fn from_sys_element_standalone(element: sys::Element) -> Element {
    Element {
        id: ElementKey::from_ffi(element.id.to_ffi()),
        role: element.role,
        title: element.title,
        description: element.description,
        value: element.value,
        url: element.url,
        help: element.help,
        role_description: element.role_description,
        identifier: element.identifier,
        bounds: element.bounds.as_ref().map(from_sys_rect),
        enabled: element.enabled,
        focused: element.focused,
        actions: element.actions,
        children: element
            .children
            .into_iter()
            .map(from_sys_element_standalone)
            .collect(),
    }
}

fn count_elements(element: &Element) -> usize {
    1 + element.children.iter().map(count_elements).sum::<usize>()
}

fn to_sys_mouse_button(button: MouseButton) -> sys::MouseButton {
    match button {
        MouseButton::Left => sys::MouseButton::Left,
        MouseButton::Right => sys::MouseButton::Right,
        MouseButton::Middle => sys::MouseButton::Middle,
    }
}

fn to_sys_listener_config(config: &ListenerConfig) -> sys::ListenerConfig {
    sys::ListenerConfig {
        event_types: config
            .event_types
            .as_ref()
            .map(|types| types.iter().copied().map(to_sys_event_type).collect()),
        pid: config.pid,
        buffer_size: config.buffer_size,
    }
}

fn to_sys_event_type(event_type: AccessibilityEventType) -> sys::AccessibilityEventType {
    match event_type {
        AccessibilityEventType::FocusChanged => sys::AccessibilityEventType::FocusChanged,
        AccessibilityEventType::ValueChanged => sys::AccessibilityEventType::ValueChanged,
        AccessibilityEventType::TitleChanged => sys::AccessibilityEventType::TitleChanged,
        AccessibilityEventType::StructureChanged => sys::AccessibilityEventType::StructureChanged,
        AccessibilityEventType::WindowCreated => sys::AccessibilityEventType::WindowCreated,
        AccessibilityEventType::WindowDestroyed => sys::AccessibilityEventType::WindowDestroyed,
        AccessibilityEventType::WindowFocusChanged => {
            sys::AccessibilityEventType::WindowFocusChanged
        }
        AccessibilityEventType::SelectedTextChanged => {
            sys::AccessibilityEventType::SelectedTextChanged
        }
        AccessibilityEventType::ElementDestroyed => sys::AccessibilityEventType::ElementDestroyed,
    }
}

fn from_sys_event_type(event_type: sys::AccessibilityEventType) -> AccessibilityEventType {
    match event_type {
        sys::AccessibilityEventType::FocusChanged => AccessibilityEventType::FocusChanged,
        sys::AccessibilityEventType::ValueChanged => AccessibilityEventType::ValueChanged,
        sys::AccessibilityEventType::TitleChanged => AccessibilityEventType::TitleChanged,
        sys::AccessibilityEventType::StructureChanged => AccessibilityEventType::StructureChanged,
        sys::AccessibilityEventType::WindowCreated => AccessibilityEventType::WindowCreated,
        sys::AccessibilityEventType::WindowDestroyed => AccessibilityEventType::WindowDestroyed,
        sys::AccessibilityEventType::WindowFocusChanged => {
            AccessibilityEventType::WindowFocusChanged
        }
        sys::AccessibilityEventType::SelectedTextChanged => {
            AccessibilityEventType::SelectedTextChanged
        }
        sys::AccessibilityEventType::ElementDestroyed => AccessibilityEventType::ElementDestroyed,
    }
}

fn from_sys_structure_change(change_type: sys::StructureChangeType) -> StructureChangeType {
    match change_type {
        sys::StructureChangeType::ChildrenAdded => StructureChangeType::ChildrenAdded,
        sys::StructureChangeType::ChildrenRemoved => StructureChangeType::ChildrenRemoved,
        sys::StructureChangeType::ChildrenReordered => StructureChangeType::ChildrenReordered,
        sys::StructureChangeType::Invalidated => StructureChangeType::Invalidated,
    }
}

fn from_sys_stop_reason(reason: sys::StopReason) -> StopReason {
    match reason {
        sys::StopReason::UserRequested => StopReason::UserRequested,
        sys::StopReason::ProcessTerminated => StopReason::ProcessTerminated,
        sys::StopReason::ConnectionLost => StopReason::ConnectionLost,
        sys::StopReason::PermissionDenied => StopReason::PermissionDenied,
    }
}

fn from_sys_event(event: sys::AccessibilityEvent) -> AccessibilityEvent {
    match event {
        sys::AccessibilityEvent::FocusChanged {
            element,
            pid,
            timestamp,
        } => AccessibilityEvent::FocusChanged {
            element: element.map(from_sys_element_standalone),
            pid,
            timestamp,
        },
        sys::AccessibilityEvent::ValueChanged {
            element,
            old_value,
            new_value,
            timestamp,
        } => AccessibilityEvent::ValueChanged {
            element: element.map(from_sys_element_standalone),
            old_value,
            new_value,
            timestamp,
        },
        sys::AccessibilityEvent::TitleChanged {
            element,
            old_title,
            new_title,
            timestamp,
        } => AccessibilityEvent::TitleChanged {
            element: element.map(from_sys_element_standalone),
            old_title,
            new_title,
            timestamp,
        },
        sys::AccessibilityEvent::StructureChanged {
            parent_element,
            change_type,
            timestamp,
        } => AccessibilityEvent::StructureChanged {
            parent_element: parent_element.map(from_sys_element_standalone),
            change_type: from_sys_structure_change(change_type),
            timestamp,
        },
        sys::AccessibilityEvent::WindowCreated {
            element,
            pid,
            timestamp,
        } => AccessibilityEvent::WindowCreated {
            element: element.map(from_sys_element_standalone),
            pid,
            timestamp,
        },
        sys::AccessibilityEvent::WindowDestroyed {
            window_id,
            pid,
            timestamp,
        } => AccessibilityEvent::WindowDestroyed {
            window_id,
            pid,
            timestamp,
        },
        sys::AccessibilityEvent::WindowFocusChanged {
            element,
            pid,
            timestamp,
        } => AccessibilityEvent::WindowFocusChanged {
            element: element.map(from_sys_element_standalone),
            pid,
            timestamp,
        },
        sys::AccessibilityEvent::SelectedTextChanged {
            element,
            selected_text,
            timestamp,
        } => AccessibilityEvent::SelectedTextChanged {
            element: element.map(from_sys_element_standalone),
            selected_text,
            timestamp,
        },
        sys::AccessibilityEvent::ElementDestroyed {
            element_id,
            timestamp,
        } => AccessibilityEvent::ElementDestroyed {
            element_id: element_id.map(|id| ElementKey::from_ffi(id.to_ffi())),
            timestamp,
        },
        sys::AccessibilityEvent::Error { message, timestamp } => {
            AccessibilityEvent::Error { message, timestamp }
        }
        sys::AccessibilityEvent::Stopped { reason, timestamp } => AccessibilityEvent::Stopped {
            reason: from_sys_stop_reason(reason),
            timestamp,
        },
    }
}
