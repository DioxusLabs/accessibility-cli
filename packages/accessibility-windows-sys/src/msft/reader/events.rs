use super::*;

/// Type alias for the boxed callback trait object.
pub(super) type EventCallback = Box<dyn FnMut(AccessibilityEvent) + Send>;

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
    if let Ok(rect) = unsafe { native.CurrentBoundingRectangle() }
        && rect.right > rect.left
        && rect.bottom > rect.top
    {
        element.bounds = Some(Rect::new(
            Point::new(rect.left as f64, rect.top as f64),
            Size::new(
                (rect.right - rect.left) as f64,
                (rect.bottom - rect.top) as f64,
            ),
        ));
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
pub(super) fn run_windows_event_loop(
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
