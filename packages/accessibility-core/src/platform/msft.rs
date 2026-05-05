//! Windows accessibility implementation using UI Automation.
//!
//! This module provides access to the Windows UI Automation accessibility tree
//! for reading UI element information and performing actions.

use crate::accessibility::{
    AccessibilityEvent, AccessibilityEventType, AccessibilityReader, Element, ElementCache,
    ElementKey, ElementTree, ListenerConfig, ListenerHandle, Point, Rect, Screenshot, Size,
    StopReason, TreeFilter,
};
use crate::input::{Code, Modifiers, MouseButton, code_from_char};
use accesskit::{Action, Role};
use anyhow::{Result, bail};
use slotmap::SecondaryMap;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationValuePattern, TreeScope_Children, UIA_ButtonControlTypeId,
    UIA_CheckBoxControlTypeId, UIA_ComboBoxControlTypeId, UIA_DocumentControlTypeId,
    UIA_EditControlTypeId, UIA_GroupControlTypeId, UIA_HyperlinkControlTypeId,
    UIA_ImageControlTypeId, UIA_InvokePatternId, UIA_ListControlTypeId, UIA_ListItemControlTypeId,
    UIA_MenuBarControlTypeId, UIA_MenuControlTypeId, UIA_MenuItemControlTypeId,
    UIA_PaneControlTypeId, UIA_ProgressBarControlTypeId, UIA_RadioButtonControlTypeId,
    UIA_ScrollBarControlTypeId, UIA_SliderControlTypeId, UIA_SpinnerControlTypeId,
    UIA_SplitButtonControlTypeId, UIA_StatusBarControlTypeId, UIA_TabControlTypeId,
    UIA_TabItemControlTypeId, UIA_TableControlTypeId, UIA_TextControlTypeId,
    UIA_TitleBarControlTypeId, UIA_ToolBarControlTypeId, UIA_ToolTipControlTypeId,
    UIA_TreeControlTypeId, UIA_TreeItemControlTypeId, UIA_ValuePatternId, UIA_WindowControlTypeId,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
    MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_BACK, VK_CANCEL, VK_CAPITAL, VK_CONTROL, VK_DELETE,
    VK_DIVIDE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F2, VK_F3, VK_F4, VK_F5, VK_F6, VK_F7, VK_F8,
    VK_F9, VK_F10, VK_F11, VK_F12, VK_F13, VK_F14, VK_F15, VK_F16, VK_F17, VK_F18, VK_F19, VK_F20,
    VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE,
    VK_MEDIA_PREV_TRACK, VK_MEDIA_STOP, VK_MENU, VK_NEXT, VK_NUMLOCK, VK_NUMPAD0, VK_NUMPAD1,
    VK_NUMPAD2, VK_NUMPAD3, VK_NUMPAD4, VK_NUMPAD5, VK_NUMPAD6, VK_NUMPAD7, VK_NUMPAD8, VK_NUMPAD9,
    VK_OEM_1, VK_OEM_2, VK_OEM_3, VK_OEM_4, VK_OEM_5, VK_OEM_6, VK_OEM_7, VK_OEM_COMMA,
    VK_OEM_MINUS, VK_OEM_PERIOD, VK_OEM_PLUS, VK_PRIOR, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU,
    VK_SCROLL, VK_SHIFT, VK_SNAPSHOT, VK_SPACE, VK_TAB, VK_UP, VK_VOLUME_DOWN, VK_VOLUME_MUTE,
    VK_VOLUME_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SetForegroundWindow,
};
use windows::core::BSTR;

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
        if let Some(max) = filter.max_elements {
            if *element_count >= max {
                return Ok(None);
            }
        }

        // Check max depth limit
        if let Some(max_depth) = filter.max_depth {
            if depth > max_depth {
                return Ok(None);
            }
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
        if matches!(role, Role::TextInput | Role::Label) {
            if let Ok(value_pattern) = unsafe {
                native.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            } {
                if let Ok(v) = unsafe { value_pattern.CurrentValue() } {
                    let value_str = v.to_string();
                    if !value_str.is_empty() {
                        value = Some(value_str);
                    }
                }
            }
        }

        // Get children
        let children =
            unsafe { native.FindAll(TreeScope_Children, &self.automation.CreateTrueCondition()?)? };
        let child_count = unsafe { children.Length()? };

        let mut children_elements = Vec::new();
        for i in 0..child_count {
            if let Ok(child_native) = unsafe { children.GetElement(i) } {
                if let Ok(Some(child_elem)) =
                    self.build_element(&child_native, depth + 1, filter, element_count)
                {
                    children_elements.push(child_elem);
                }
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
            if let Ok(window) = unsafe { all_windows.GetElement(i) } {
                if let Ok(native_hwnd) = unsafe { window.CurrentNativeWindowHandle() } {
                    let hwnd = HWND(native_hwnd.0 as *mut _);
                    let mut window_pid: u32 = 0;
                    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut window_pid)) };
                    if window_pid == pid {
                        return Ok(window);
                    }
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
                                if let Ok(desc_pid) = unsafe { desc.CurrentProcessId() } {
                                    if desc_pid as u32 == pid {
                                        // Found a descendant with matching PID, return the host window
                                        return Ok(window);
                                    }
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

impl AccessibilityReader for WindowsAccessibility {
    async fn get_tree(&mut self, pid: Option<u32>, filter: &TreeFilter) -> Result<ElementTree> {
        // Clear previous state
        self.clear_cache();
        self.native_elements.clear();

        // Get root element
        let root_element = if let Some(pid) = pid {
            self.find_root_for_pid(pid)?
        } else {
            // Get focused element's top-level window
            let focused = unsafe { self.automation.GetFocusedElement()? };
            focused
        };

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
            pid,
            app_name,
            root,
            element_count,
        })
    }

    fn get_element(&self, id: ElementKey) -> Option<&Element> {
        self.cache.get(id)
    }

    async fn perform_action(&mut self, id: ElementKey, action: Action) -> Result<()> {
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

    async fn set_value(&mut self, id: ElementKey, value: &str) -> Result<()> {
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

    async fn hit_test(&mut self, x: f64, y: f64) -> Result<Option<ElementKey>> {
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
                } {
                    if native_rect.left == hit_rect.left
                        && native_rect.top == hit_rect.top
                        && native_rect.right == hit_rect.right
                        && native_rect.bottom == hit_rect.bottom
                    {
                        return Ok(Some(key));
                    }
                }
            }
        }

        Ok(None)
    }

    fn clear_cache(&mut self) {
        self.cache.clear();
        self.native_elements.clear();
    }

    fn snapshot_version(&self) -> u64 {
        self.cache.version()
    }

    // Platform adapter methods (merged from WindowsAdapter)

    fn capture_screen(&self, pid: Option<u32>) -> Result<Screenshot> {
        if let Some(pid) = pid {
            if let Ok(screenshot) = WindowsAccessibility::capture_window(self, pid) {
                return Ok(screenshot);
            }
        }
        WindowsAccessibility::capture_screen(self)
    }

    async fn get_screen_bounds(&self, pid: Option<u32>) -> Result<Rect> {
        if let Some(pid) = pid {
            if let Some(bounds) = self.get_window_bounds_for_pid(pid) {
                return Ok(bounds);
            }
        }
        Ok(Self::get_screen_bounds())
    }

    fn platform_name(&self) -> &'static str {
        "Windows"
    }

    async fn keystroke(
        &mut self,
        _pid: Option<u32>,
        key: Code,
        modifiers: Modifiers,
    ) -> Result<()> {
        // Windows doesn't support process-targeted input like macOS, so pid is ignored
        self.keystroke_internal(key, modifiers)
    }

    async fn type_raw(&mut self, _pid: Option<u32>, text: &str) -> Result<()> {
        // Windows doesn't support process-targeted input like macOS, so pid is ignored
        for c in text.chars() {
            if let Some((key, needs_shift)) = code_from_char(c) {
                let mods = if needs_shift {
                    Modifiers::SHIFT
                } else {
                    Modifiers::empty()
                };
                self.keystroke_internal(key, mods)?;
            }
        }
        Ok(())
    }

    async fn mouse_click_at(
        &mut self,
        _pid: Option<u32>,
        x: f64,
        y: f64,
        button: MouseButton,
    ) -> Result<()> {
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

    async fn press_key(&mut self, _pid: Option<u32>, key: Code) -> Result<()> {
        let vk = code_to_vk(key);
        send_key_event(vk, false)
    }

    async fn release_key(&mut self, _pid: Option<u32>, key: Code) -> Result<()> {
        let vk = code_to_vk(key);
        send_key_event(vk, true)
    }

    async fn mouse_move(&mut self, _pid: Option<u32>, x: f64, y: f64) -> Result<()> {
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

    async fn mouse_click(&mut self, _pid: Option<u32>, button: MouseButton) -> Result<()> {
        self.mouse_click_internal(button)
    }

    async fn mouse_double_click(&mut self, _pid: Option<u32>, button: MouseButton) -> Result<()> {
        self.mouse_click_internal(button)?;
        self.mouse_click_internal(button)
    }

    async fn mouse_scroll(&mut self, _pid: Option<u32>, _delta_x: f64, delta_y: f64) -> Result<()> {
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

    fn supports_keystroke(&self) -> bool {
        true
    }

    fn supports_mouse_click(&self) -> bool {
        true
    }

    fn supports_hit_test(&self) -> bool {
        true
    }

    // Event listening implementation

    fn start_listening(
        &mut self,
        config: ListenerConfig,
        callback: Box<dyn FnMut(AccessibilityEvent) + Send + 'static>,
    ) -> Result<ListenerHandle> {
        // Determine target PID - must be specified in config
        let target_pid = config.pid.ok_or_else(|| {
            anyhow::anyhow!(
                "No target PID specified for event listening (set pid in ListenerConfig)"
            )
        })?;

        // Create stop flag
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_clone = stop_flag.clone();

        // Wrap callback in Arc<Mutex> for thread-safe access
        let callback: Arc<Mutex<EventCallback>> = Arc::new(Mutex::new(callback));

        // Clone config for the spawned task
        let config_clone = config.clone();

        // Spawn the listener task using spawn_blocking
        // because Windows COM event handlers need to run on a thread with message pump
        let task_handle = tokio::task::spawn_blocking(move || {
            run_windows_event_loop(target_pid, config_clone, callback, stop_flag_clone);
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
            AccessibilityEventType::TitleChanged,
            AccessibilityEventType::StructureChanged,
            AccessibilityEventType::WindowCreated,
            AccessibilityEventType::WindowDestroyed,
        ]
    }
}

/// Convert a keyboard-types Code to a Windows virtual key code.
fn code_to_vk(key: Code) -> VIRTUAL_KEY {
    match key {
        Code::KeyA => VIRTUAL_KEY(0x41),
        Code::KeyB => VIRTUAL_KEY(0x42),
        Code::KeyC => VIRTUAL_KEY(0x43),
        Code::KeyD => VIRTUAL_KEY(0x44),
        Code::KeyE => VIRTUAL_KEY(0x45),
        Code::KeyF => VIRTUAL_KEY(0x46),
        Code::KeyG => VIRTUAL_KEY(0x47),
        Code::KeyH => VIRTUAL_KEY(0x48),
        Code::KeyI => VIRTUAL_KEY(0x49),
        Code::KeyJ => VIRTUAL_KEY(0x4A),
        Code::KeyK => VIRTUAL_KEY(0x4B),
        Code::KeyL => VIRTUAL_KEY(0x4C),
        Code::KeyM => VIRTUAL_KEY(0x4D),
        Code::KeyN => VIRTUAL_KEY(0x4E),
        Code::KeyO => VIRTUAL_KEY(0x4F),
        Code::KeyP => VIRTUAL_KEY(0x50),
        Code::KeyQ => VIRTUAL_KEY(0x51),
        Code::KeyR => VIRTUAL_KEY(0x52),
        Code::KeyS => VIRTUAL_KEY(0x53),
        Code::KeyT => VIRTUAL_KEY(0x54),
        Code::KeyU => VIRTUAL_KEY(0x55),
        Code::KeyV => VIRTUAL_KEY(0x56),
        Code::KeyW => VIRTUAL_KEY(0x57),
        Code::KeyX => VIRTUAL_KEY(0x58),
        Code::KeyY => VIRTUAL_KEY(0x59),
        Code::KeyZ => VIRTUAL_KEY(0x5A),
        Code::Digit0 => VIRTUAL_KEY(0x30),
        Code::Digit1 => VIRTUAL_KEY(0x31),
        Code::Digit2 => VIRTUAL_KEY(0x32),
        Code::Digit3 => VIRTUAL_KEY(0x33),
        Code::Digit4 => VIRTUAL_KEY(0x34),
        Code::Digit5 => VIRTUAL_KEY(0x35),
        Code::Digit6 => VIRTUAL_KEY(0x36),
        Code::Digit7 => VIRTUAL_KEY(0x37),
        Code::Digit8 => VIRTUAL_KEY(0x38),
        Code::Digit9 => VIRTUAL_KEY(0x39),
        Code::F1 => VK_F1,
        Code::F2 => VK_F2,
        Code::F3 => VK_F3,
        Code::F4 => VK_F4,
        Code::F5 => VK_F5,
        Code::F6 => VK_F6,
        Code::F7 => VK_F7,
        Code::F8 => VK_F8,
        Code::F9 => VK_F9,
        Code::F10 => VK_F10,
        Code::F11 => VK_F11,
        Code::F12 => VK_F12,
        Code::F13 => VK_F13,
        Code::F14 => VK_F14,
        Code::F15 => VK_F15,
        Code::F16 => VK_F16,
        Code::F17 => VK_F17,
        Code::F18 => VK_F18,
        Code::F19 => VK_F19,
        Code::F20 => VK_F20,
        Code::Enter => VK_RETURN,
        Code::Tab => VK_TAB,
        Code::Space => VK_SPACE,
        Code::Backspace => VK_BACK,
        Code::Escape => VK_ESCAPE,
        Code::Delete => VK_DELETE,
        Code::Insert => VK_INSERT,
        Code::Home => VK_HOME,
        Code::End => VK_END,
        Code::PageUp => VK_PRIOR,
        Code::PageDown => VK_NEXT,
        Code::ArrowUp => VK_UP,
        Code::ArrowDown => VK_DOWN,
        Code::ArrowLeft => VK_LEFT,
        Code::ArrowRight => VK_RIGHT,
        Code::ShiftLeft | Code::ShiftRight => VK_SHIFT,
        Code::ControlLeft | Code::ControlRight => VK_CONTROL,
        Code::AltLeft | Code::AltRight => VK_MENU,
        Code::MetaLeft | Code::MetaRight => VK_LWIN,
        Code::Minus => VK_OEM_MINUS,
        Code::Equal => VK_OEM_PLUS,
        Code::BracketLeft => VK_OEM_4,
        Code::BracketRight => VK_OEM_6,
        Code::Backslash => VK_OEM_5,
        Code::Semicolon => VK_OEM_1,
        Code::Quote => VK_OEM_7,
        Code::Backquote => VK_OEM_3,
        Code::Comma => VK_OEM_COMMA,
        Code::Period => VK_OEM_PERIOD,
        Code::Slash => VK_OEM_2,
        Code::Numpad0 => VK_NUMPAD0,
        Code::Numpad1 => VK_NUMPAD1,
        Code::Numpad2 => VK_NUMPAD2,
        Code::Numpad3 => VK_NUMPAD3,
        Code::Numpad4 => VK_NUMPAD4,
        Code::Numpad5 => VK_NUMPAD5,
        Code::Numpad6 => VK_NUMPAD6,
        Code::Numpad7 => VK_NUMPAD7,
        Code::Numpad8 => VK_NUMPAD8,
        Code::Numpad9 => VK_NUMPAD9,
        Code::NumpadDecimal => VIRTUAL_KEY(0x6E),
        Code::NumpadMultiply => VIRTUAL_KEY(0x6A),
        Code::NumpadAdd => VIRTUAL_KEY(0x6B),
        Code::NumpadSubtract => VIRTUAL_KEY(0x6D),
        Code::NumpadDivide => VIRTUAL_KEY(0x6F),
        Code::NumpadEnter => VK_RETURN, // Same as regular return
        Code::CapsLock => VK_CAPITAL,
        Code::NumLock => VK_NUMLOCK,
        Code::ScrollLock => VK_SCROLL,
        Code::AudioVolumeUp => VK_VOLUME_UP,
        Code::AudioVolumeDown => VK_VOLUME_DOWN,
        Code::AudioVolumeMute => VK_VOLUME_MUTE,
        Code::MediaPlayPause => VK_MEDIA_PLAY_PAUSE,
        Code::MediaStop => VK_MEDIA_STOP,
        Code::MediaTrackNext => VK_MEDIA_NEXT_TRACK,
        Code::MediaTrackPrevious => VK_MEDIA_PREV_TRACK,
        Code::PrintScreen => VK_SNAPSHOT,
        _ => VK_CANCEL, // Unsupported key, return cancel
    }
}

/// Check if a virtual key is an extended key.
/// Extended keys include: arrows, Insert, Delete, Home, End, Page Up, Page Down,
/// Num Lock, Break, Print Screen, and right-hand Alt/Ctrl.
fn is_extended_key(vk: VIRTUAL_KEY) -> bool {
    matches!(
        vk,
        VK_UP | VK_DOWN | VK_LEFT | VK_RIGHT |
        VK_INSERT | VK_DELETE | VK_HOME | VK_END |
        VK_PRIOR | VK_NEXT |  // Page Up / Page Down
        VK_NUMLOCK | VK_CANCEL | VK_SNAPSHOT |  // Num Lock, Break, Print Screen
        VK_DIVIDE |  // Numpad divide
        VK_RCONTROL | VK_RMENU // Right Ctrl, Right Alt
    )
}

/// Send a keyboard event.
fn send_key_event(vk: VIRTUAL_KEY, key_up: bool) -> Result<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{MAP_VIRTUAL_KEY_TYPE, MapVirtualKeyW};

    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    if is_extended_key(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }

    // MAPVK_VK_TO_VSC = 0
    let scan_code = unsafe { MapVirtualKeyW(vk.0 as u32, MAP_VIRTUAL_KEY_TYPE(0)) as u16 };

    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan_code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let inserted = unsafe { SendInput(&[input], std::mem::size_of::<INPUT>() as i32) };
    if inserted != 1 {
        bail!("SendInput failed to insert keyboard event");
    }
    Ok(())
}

/// Get the PID of the foreground window.
pub fn get_foreground_pid() -> Option<u32> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let mut pid: u32 = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 { None } else { Some(pid) }
}

/// Type alias for the boxed callback trait object.
type EventCallback = Box<dyn FnMut(AccessibilityEvent) + Send>;

/// Get the current timestamp in milliseconds since UNIX epoch.
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build a minimal Element from a UI Automation element for event reporting.
fn build_element_from_uia(native: &IUIAutomationElement) -> Option<Element> {
    let control_type = unsafe { native.CurrentControlType().ok()? };
    let role = WindowsAccessibility::control_type_to_role(control_type.0);

    // Use a placeholder key since we're not caching this element
    let placeholder_key = ElementKey::from_ffi(1);

    let mut element = Element::new(placeholder_key, role);

    element.title = unsafe {
        native
            .CurrentName()
            .ok()
            .map(|b| b.to_string())
            .filter(|s| !s.is_empty())
    };

    element.identifier = unsafe {
        native
            .CurrentAutomationId()
            .ok()
            .map(|b| b.to_string())
            .filter(|s| !s.is_empty())
    };

    // Get bounds
    if let Ok(rect) = unsafe { native.CurrentBoundingRectangle() } {
        if rect.right > rect.left && rect.bottom > rect.top {
            element.bounds = Some(Rect::new(
                Point::new(rect.left as f64, rect.top as f64),
                Size::new(
                    (rect.right - rect.left) as f64,
                    (rect.bottom - rect.top) as f64,
                ),
            ));
        }
    }

    element.enabled = unsafe {
        native
            .CurrentIsEnabled()
            .ok()
            .map(|b| b.as_bool())
            .unwrap_or(true)
    };
    element.focused = unsafe {
        native
            .CurrentHasKeyboardFocus()
            .ok()
            .map(|b| b.as_bool())
            .unwrap_or(false)
    };

    Some(element)
}

/// Run the Windows event loop with UI Automation event handlers.
///
/// This function runs on a dedicated thread with COM initialization and uses
/// UI Automation's event subscription mechanism.
///
/// Note: Full COM event handler implementation would require implementing
/// IUIAutomationEventHandler, IUIAutomationFocusChangedEventHandler, etc.
/// as COM objects. This simplified implementation uses polling with focus
/// tracking to provide basic event functionality.
fn run_windows_event_loop(
    target_pid: u32,
    config: ListenerConfig,
    callback: Arc<Mutex<EventCallback>>,
    stop_flag: Arc<AtomicBool>,
) {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, TranslateMessage,
    };

    // Initialize COM for this thread (apartment-threaded for message pump)
    let com_result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if com_result.is_err() {
        if let Ok(mut cb) = callback.lock() {
            cb(AccessibilityEvent::Error {
                message: format!("Failed to initialize COM: {:?}", com_result),
                timestamp: current_timestamp(),
            });
        }
        return;
    }

    // Create UI Automation instance
    let automation: IUIAutomation =
        match unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
            Ok(a) => a,
            Err(e) => {
                if let Ok(mut cb) = callback.lock() {
                    cb(AccessibilityEvent::Error {
                        message: format!("Failed to create UI Automation: {:?}", e),
                        timestamp: current_timestamp(),
                    });
                }
                unsafe { CoUninitialize() };
                return;
            }
        };

    // Track previous focus for change detection
    let mut _last_focus_name: Option<String> = None; // Kept for potential future use
    let mut last_focus_rect: Option<RECT> = None;

    // Track focused element's title for TitleChanged events
    let mut last_focused_title: Option<String> = None;
    // Track focused element's value for ValueChanged events
    let mut last_focused_value: Option<String> = None;

    // Main event loop
    loop {
        // Check for stop signal
        if stop_flag.load(AtomicOrdering::SeqCst) {
            break;
        }

        // Process Windows messages (required for COM)
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE).as_bool() {
                if GetMessageW(&mut msg, None, 0, 0).0 <= 0 {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // Poll for focus changes if configured
        if let Ok(focused) = unsafe { automation.GetFocusedElement() } {
            // Check if this element belongs to our target process using UIA's ProcessId property
            // This works for UWP elements that don't have their own window handles
            if let Ok(element_pid) = unsafe { focused.CurrentProcessId() } {
                let element_pid = element_pid as u32;

                if element_pid == target_pid || target_pid == 0 {
                    // Get current focus info
                    let current_name: Option<String> =
                        unsafe { focused.CurrentName().ok().map(|b| b.to_string()) };
                    let current_rect = unsafe { focused.CurrentBoundingRectangle().ok() };

                    // Get current title and value for change detection
                    let current_title: Option<String> = current_name.clone();
                    // For value, we use the element's name/title since that's what Calculator
                    // updates when displaying results (e.g., "Display is 8")
                    let current_value: Option<String> = current_title.clone();

                    // Check if focus changed to a DIFFERENT element
                    // Use bounding rect as element identity (same position = same element)
                    // This allows detecting title changes on the same element separately
                    let focus_changed_to_different_element = current_rect != last_focus_rect;

                    if focus_changed_to_different_element {
                        // Focus moved to a different element
                        last_focused_title = current_title.clone();
                        last_focused_value = current_value.clone();
                        _last_focus_name = current_name.clone();
                        last_focus_rect = current_rect;

                        if config.should_capture(AccessibilityEventType::FocusChanged) {
                            let element = build_element_from_uia(&focused);
                            if let Ok(mut cb) = callback.lock() {
                                cb(AccessibilityEvent::FocusChanged {
                                    element,
                                    pid: Some(element_pid),
                                    timestamp: current_timestamp(),
                                });
                            }
                        }
                    } else {
                        // Focus didn't change - check for title/value changes on the same element

                        // Check for title change
                        if config.should_capture(AccessibilityEventType::TitleChanged)
                            && current_title != last_focused_title
                        {
                            let old_title = last_focused_title.take();
                            last_focused_title = current_title.clone();
                            _last_focus_name = current_name.clone(); // Keep name in sync

                            let element = build_element_from_uia(&focused);
                            if let Ok(mut cb) = callback.lock() {
                                cb(AccessibilityEvent::TitleChanged {
                                    element,
                                    old_title,
                                    new_title: current_title,
                                    timestamp: current_timestamp(),
                                });
                            }
                        }

                        // Check for value change
                        if config.should_capture(AccessibilityEventType::ValueChanged)
                            && current_value != last_focused_value
                        {
                            let old_value = last_focused_value.take();
                            last_focused_value = current_value.clone();

                            let element = build_element_from_uia(&focused);
                            if let Ok(mut cb) = callback.lock() {
                                cb(AccessibilityEvent::ValueChanged {
                                    element,
                                    old_value,
                                    new_value: current_value,
                                    timestamp: current_timestamp(),
                                });
                            }
                        }
                    }
                }
            }
        }

        // Sleep briefly to avoid busy-waiting
    }

    // Send stopped event
    if let Ok(mut cb) = callback.lock() {
        cb(AccessibilityEvent::Stopped {
            reason: StopReason::UserRequested,
            timestamp: current_timestamp(),
        });
    }

    // Cleanup COM
    unsafe { CoUninitialize() };
}
