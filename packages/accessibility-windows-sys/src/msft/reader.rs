use super::common::ElementCache;
use super::input::{code_to_vk, send_key_event};
use super::*;

mod adapter;
mod events;

/// Windows accessibility reader using UI Automation.
pub struct WindowsAccessibility {
    automation: IUIAutomation,
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

        Ok(Self {
            automation,
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
        let _ = unsafe { element.SetFocus() };

        // Then bring window to foreground
        let _ = unsafe { SetForegroundWindow(hwnd) };

        Ok(())
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
            Err(_) => return windows,
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
            _ => Role::Unknown,
        }
    }

    /// Build an Element from a UI Automation element.
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

        // Get element properties
        let control_type = unsafe { native.CurrentControlType()? };
        let role = Self::control_type_to_role(control_type.0);

        let name: String = unsafe {
            native
                .CurrentName()
                .map(|b| b.to_string())
                .unwrap_or_default()
        };

        let automation_id: String = unsafe {
            native
                .CurrentAutomationId()
                .map(|b| b.to_string())
                .unwrap_or_default()
        };

        // Get bounding rectangle
        let rect = unsafe { native.CurrentBoundingRectangle()? };
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

        let enabled = unsafe { native.CurrentIsEnabled()?.as_bool() };

        let has_focus = unsafe { native.CurrentHasKeyboardFocus()?.as_bool() };

        // Collect element properties
        let title = if name.is_empty() { None } else { Some(name) };
        let identifier = if automation_id.is_empty() {
            None
        } else {
            Some(automation_id)
        };

        // Try to get value for text controls
        let mut value = None;
        if matches!(role, Role::TextInput | Role::Label)
            && let Ok(value_pattern) = unsafe {
                native.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            }
            && let Ok(v) = unsafe { value_pattern.CurrentValue() }
        {
            let value_str = v.to_string();
            if !value_str.is_empty() {
                value = Some(value_str);
            }
        }

        // Get children
        let children =
            unsafe { native.FindAll(TreeScope_Children, &self.automation.CreateTrueCondition()?)? };
        let child_count = unsafe { children.Length()? };

        let mut children_elements = Vec::new();
        for i in 0..child_count {
            if let Ok(child_native) = unsafe { children.GetElement(i) }
                && let Ok(Some(child_elem)) =
                    self.build_element(&child_native, depth + 1, filter, element_count)
            {
                children_elements.push(child_elem);
            }
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
            actions: Vec::new(),
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

        let all_windows = unsafe {
            root.FindAll(TreeScope_Children, &condition)
                .map_err(|e| anyhow::anyhow!("FindAll failed: {:?}", e))?
        };

        let count = unsafe { all_windows.Length()? };

        // First pass: try to match by PID directly via window handle
        for i in 0..count {
            if let Ok(window) = unsafe { all_windows.GetElement(i) }
                && let Ok(native_hwnd) = unsafe { window.CurrentNativeWindowHandle() }
            {
                let hwnd = HWND(native_hwnd.0 as *mut _);
                let mut window_pid: u32 = 0;
                unsafe { GetWindowThreadProcessId(hwnd, Some(&mut window_pid)) };
                if window_pid == pid {
                    return Ok(window);
                }
            }
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
