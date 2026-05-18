use super::events::{EventCallback, run_windows_event_loop};
use super::*;
use crate::msft::input::{code_to_vk, send_key_event};

impl WindowsAccessibility {
    pub async fn get_tree(&mut self, pid: u32, filter: &TreeFilter) -> Result<ElementTree> {
        // Clear previous state
        self.clear_cache();
        self.native_elements.clear();

        let root_element = self.find_root_for_pid(pid)?;

        // Get app name
        let app_name: Option<String> = unsafe {
            root_element
                .CurrentName()
                .ok()
                .map(|b| b.to_string())
                .filter(|s| !s.is_empty())
        };

        let mut element_count = 0;
        let root = self
            .build_element(&root_element, 0, filter, &mut element_count)?
            .ok_or_else(|| anyhow::anyhow!("Failed to build root element"))?;

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
                    unsafe { invoke_pattern.Invoke()? };
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
            _ => bail!("Action {:?} not implemented for Windows", action),
        }
    }

    pub async fn set_value(&mut self, id: ElementKey, value: &str) -> Result<()> {
        let native = self
            .native_elements
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Element not found: {}", id))?;

        let value_pattern =
            unsafe { native.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)? };

        let bstr = BSTR::from(value);
        unsafe { value_pattern.SetValue(&bstr)? };
        Ok(())
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

        Ok(None)
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
        self.native_elements.clear();
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
