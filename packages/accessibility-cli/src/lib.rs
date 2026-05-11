//! Unified Accessibility CLI
//!
//! Cross-platform accessibility CLI for macOS, Windows, iOS Simulator, Linux, and Android.
//!
//! Usage:
//!
//! ```text
//! cargo run -p accessibility-cli --bin accessibility-cli -- --platform mac [OPTIONS]       # macOS accessibility
//! cargo run -p accessibility-cli --bin accessibility-cli -- --platform win [OPTIONS]       # Windows accessibility
//! cargo run -p accessibility-cli --bin accessibility-cli -- --platform ios [OPTIONS]       # iOS Simulator accessibility
//! cargo run -p accessibility-cli --bin accessibility-cli -- --platform linux [OPTIONS]     # Linux accessibility (AT-SPI)
//! cargo run -p accessibility-cli --bin accessibility-cli -- --platform android [OPTIONS]   # Android device/emulator (ADB)
//! ```
//!
//! Examples:
//!
//! ```text
//! accessibility-cli --platform mac --pid 123 --llm           # Query specific macOS app
//! accessibility-cli --platform mac --pid 123 --mouse-click 300,240  # Targeted macOS click
//! accessibility-cli --platform win --pid 123 --llm           # Query specific Windows app
//! accessibility-cli --platform ios --udid ABC --annotate     # Annotated iOS screenshot
//! accessibility-cli --platform ios --hid-tap 100,200         # HID tap on iOS Simulator
//! accessibility-cli --platform linux --pid 123 --llm         # Query specific Linux app
//! accessibility-cli --platform android --serial ABC --llm    # Query Android device
//! accessibility-cli --platform android --adb-back            # Press Android back button
//! accessibility-cli --platform android --adb-swipe 100,200,100,800  # Swipe on Android
//! ```

use accessibility_core::accessibility::{
    AccessibilityEvent, AccessibilityEventType, ListenerConfig, TargetedAccessibility, TreeFilter,
};
use accessibility_core::api::{
    JsonPrinter, LlmPrinter, LlmQueryPrinter, Printer, TreePrinter, annotate_elements,
    decode_screenshot, draw_grid_overlay, format_role_short, print_element_summary,
    print_formatted, print_statistics, truncate,
};
use clap::{Args, Parser, ValueEnum};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use accessibility_core::input::MouseButton;
use accessibility_core::platform::AndroidExtensions;
use accessibility_core::platform::android::AndroidAccessibility;
#[cfg(target_os = "macos")]
use accessibility_core::platform::ios_simulator::IOSSimulatorAccessibility;
#[cfg(target_os = "macos")]
use accessibility_core::platform::macos::MacOSAccessibility;

/// Result of an operation that may need to be retried with timeout polling.
#[derive(Debug, Clone, PartialEq)]
enum OperationResult {
    /// Operation completed successfully (no retry needed)
    Success,
    /// Element not found - retry if timeout is set
    NotFound(String),
    /// Operation completed but didn't need to find an element (e.g., printing tree)
    Completed,
    /// Fatal error - exit immediately
    Error(String),
}

/// Generate a random screenshot path in the system temp directory.
fn screenshot_path() -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u32)
        .unwrap_or(0);
    let random_id = std::process::id() ^ timestamp;
    std::env::temp_dir().join(format!("{:x}.png", random_id))
}

/// Handle screenshot-screen command.
async fn handle_screenshot_screen(adapter: &TargetedAccessibility, args: &CommonArgs) {
    println!("Capturing full screen screenshot...");
    match adapter.capture_screen() {
        Ok(screenshot) => {
            if args.overlay {
                match adapter.get_screen_bounds().await {
                    Ok(bounds) => {
                        let mut img = decode_screenshot(&screenshot);
                        draw_grid_overlay(
                            &mut img,
                            args.grid_size,
                            &bounds,
                            screenshot.width,
                            screenshot.height,
                        );
                        let filename = screenshot_path();
                        if let Err(e) = img.save(&filename) {
                            eprintln!("Failed to save image to {}: {}", filename.display(), e);
                            std::process::exit(1);
                        }
                        println!(
                            "Saved overlay screenshot to {} ({}x{} pixels, grid size: {})",
                            filename.display(),
                            screenshot.width,
                            screenshot.height,
                            args.grid_size
                        );
                    }
                    Err(e) => {
                        eprintln!("Failed to get screen bounds: {}", e);
                        // Save raw screenshot without overlay as fallback
                        let filename = screenshot_path();
                        if let Err(e) = std::fs::write(&filename, &screenshot.data) {
                            eprintln!("Failed to save screenshot to {}: {}", filename.display(), e);
                            std::process::exit(1);
                        }
                        println!(
                            "Saved screenshot to {} ({}x{})",
                            filename.display(),
                            screenshot.width,
                            screenshot.height
                        );
                    }
                }
            } else {
                let filename = screenshot_path();
                if let Err(e) = std::fs::write(&filename, &screenshot.data) {
                    eprintln!("Failed to write {}: {}", filename.display(), e);
                    std::process::exit(1);
                }
                println!(
                    "Saved full screen screenshot to {} ({}x{} pixels)",
                    filename.display(),
                    screenshot.width,
                    screenshot.height
                );
            }
        }
        Err(e) => {
            eprintln!("Failed to capture screen: {}", e);
            std::process::exit(1);
        }
    }
}

/// Handle annotate command.
async fn handle_annotate(
    adapter: &TargetedAccessibility,
    tree: &accessibility_core::accessibility::ElementTree,
    args: &CommonArgs,
) {
    // Resolve elements (query or interactive with bounds)
    let (elements, description) = match adapter.find_elements(tree, args.query.as_deref(), true) {
        Ok(elems) => (
            elems,
            if args.query.is_some() {
                "matching"
            } else {
                "interactive"
            },
        ),
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    if elements.is_empty() {
        println!("No elements to annotate.");
        return;
    }

    println!(
        "Found {} {} elements with bounds",
        elements.len(),
        description
    );

    // Capture screenshot
    let screenshot = match adapter.capture_screen() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to capture screenshot: {}", e);
            std::process::exit(1);
        }
    };

    let screen_bounds = match adapter.get_screen_bounds().await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to get screen bounds: {}", e);
            std::process::exit(1);
        }
    };

    // Decode and annotate
    let mut img = decode_screenshot(&screenshot);
    annotate_elements(&mut img, &elements, &screen_bounds, &screenshot, args.label);

    if args.overlay {
        draw_grid_overlay(
            &mut img,
            args.grid_size,
            &screen_bounds,
            screenshot.width,
            screenshot.height,
        );
    }

    let filename = screenshot_path();
    if let Err(e) = img.save(&filename) {
        eprintln!(
            "Failed to save annotated image to {}: {}",
            filename.display(),
            e
        );
        std::process::exit(1);
    }
    println!(
        "Saved annotated screenshot to {} ({} elements marked)",
        filename.display(),
        elements.len()
    );

    if args.label {
        println!("\nElement labels:");
        let scale_x = screenshot.width as f64 / screen_bounds.size.width;
        let scale_y = screenshot.height as f64 / screen_bounds.size.height;
        let mut label_num = 1u32;
        for elem in &elements {
            if let Some(bounds) = &elem.bounds {
                let px = ((bounds.origin.x - screen_bounds.origin.x) * scale_x) as i32;
                let py = ((bounds.origin.y - screen_bounds.origin.y) * scale_y) as i32;
                if px >= 0 && py >= 0 && px < img.width() as i32 && py < img.height() as i32 {
                    let role_str = format_role_short(elem.role);
                    println!(
                        "  {}: [{}] {} \"{}\"",
                        label_num,
                        elem.id,
                        role_str,
                        truncate(&elem.display_label(), 30)
                    );
                    label_num += 1;
                }
            }
        }
    }
}

/// Check if elements matching a query exist in the tree.
/// Returns Ok(true) if matches found, Ok(false) if no matches, Err on query error.
fn query_has_matches(
    adapter: &TargetedAccessibility,
    tree: &accessibility_core::accessibility::ElementTree,
    query: &str,
) -> Result<bool, String> {
    match adapter.find_elements(tree, Some(query), false) {
        Ok(elements) => Ok(!elements.is_empty()),
        Err(e) => Err(e.to_string()),
    }
}

/// Helper for element action operations (click, focus, blur).
/// Returns OperationResult based on whether the action succeeded.
/// Perform a click action on an element.
async fn perform_element_action(
    adapter: &mut TargetedAccessibility,
    tree: &accessibility_core::accessibility::ElementTree,
    target: &str,
    action_name: &str,
) -> OperationResult {
    // Pre-check if element exists
    match query_has_matches(adapter, tree, target) {
        Ok(false) => return OperationResult::NotFound(format!("No element found for: {}", target)),
        Err(e) => return OperationResult::Error(e),
        Ok(true) => {}
    }

    match adapter.click_element(target, tree).await {
        Ok(id) => {
            if let Some(elem) = adapter.get_element(id) {
                println!(
                    "Clicked element [{}] {:?} \"{}\"",
                    id,
                    elem.role,
                    elem.display_label()
                );
            } else {
                println!("Clicked element [{}]", id);
            }
            OperationResult::Success
        }
        Err(e) => OperationResult::Error(format!("{} failed: {}", action_name, e)),
    }
}

/// Perform a focus action on an element.
async fn perform_element_action_focus(
    adapter: &mut TargetedAccessibility,
    tree: &accessibility_core::accessibility::ElementTree,
    target: &str,
) -> OperationResult {
    // Pre-check if element exists
    match query_has_matches(adapter, tree, target) {
        Ok(false) => return OperationResult::NotFound(format!("No element found for: {}", target)),
        Err(e) => return OperationResult::Error(e),
        Ok(true) => {}
    }

    match adapter.focus_element(target, tree).await {
        Ok(id) => {
            if let Some(elem) = adapter.get_element(id) {
                println!(
                    "Focused element [{}] {:?} \"{}\"",
                    id,
                    elem.role,
                    elem.display_label()
                );
            } else {
                println!("Focused element [{}]", id);
            }
            OperationResult::Success
        }
        Err(e) => OperationResult::Error(format!("Focus failed: {}", e)),
    }
}

/// Perform a blur action on an element.
async fn perform_element_action_blur(
    adapter: &mut TargetedAccessibility,
    tree: &accessibility_core::accessibility::ElementTree,
    target: &str,
) -> OperationResult {
    // Pre-check if element exists
    match query_has_matches(adapter, tree, target) {
        Ok(false) => return OperationResult::NotFound(format!("No element found for: {}", target)),
        Err(e) => return OperationResult::Error(e),
        Ok(true) => {}
    }

    match adapter.blur_element(target, tree).await {
        Ok(id) => {
            if let Some(elem) = adapter.get_element(id) {
                println!(
                    "Blurred element [{}] {:?} \"{}\"",
                    id,
                    elem.role,
                    elem.display_label()
                );
            } else {
                println!("Blurred element [{}]", id);
            }
            OperationResult::Success
        }
        Err(e) => OperationResult::Error(format!("Blur failed: {}", e)),
    }
}

/// Handle common CLI operations.
/// Returns OperationResult to indicate whether the operation succeeded,
/// needs retry (element not found), or had a fatal error.
async fn handle_common_operations(
    adapter: &mut TargetedAccessibility,
    args: &CommonArgs,
    tree: &accessibility_core::accessibility::ElementTree,
    _filter: &TreeFilter,
) -> OperationResult {
    // Handle click
    if let Some(ref target) = args.click {
        return perform_element_action(adapter, tree, target, "click").await;
    }

    // Handle focus
    if let Some(ref target) = args.focus {
        return perform_element_action_focus(adapter, tree, target).await;
    }

    // Handle blur
    if let Some(ref target) = args.blur {
        return perform_element_action_blur(adapter, tree, target).await;
    }

    // Handle type/set value
    if let Some(ref type_args) = args.type_value {
        if type_args.len() == 2 {
            let target = &type_args[0];
            let text = &type_args[1];
            // Pre-check if element exists
            match query_has_matches(adapter, tree, target) {
                Ok(false) => {
                    return OperationResult::NotFound(format!("No element found for: {}", target));
                }
                Err(e) => return OperationResult::Error(e),
                Ok(true) => {}
            }
            match adapter.set_element_value(target, text, tree).await {
                Ok(id) => {
                    println!("Set value on [{}] to \"{}\"", id, text);
                    return OperationResult::Success;
                }
                Err(e) => {
                    return OperationResult::Error(format!("Set value failed: {}", e));
                }
            }
        }
        return OperationResult::Completed;
    }

    // Handle keystroke (requires element selector to focus first)
    if let Some(ref key_args) = args.key {
        if !adapter.supports_keystroke() {
            return OperationResult::Error(format!(
                "Error: Keystroke injection is not supported on {}.",
                adapter.platform_name()
            ));
        }
        let key_spec = &key_args[0];
        let target = &key_args[1];

        // Pre-check if element exists
        match query_has_matches(adapter, tree, target) {
            Ok(false) => {
                return OperationResult::NotFound(format!("No element found for: {}", target));
            }
            Err(e) => return OperationResult::Error(e),
            Ok(true) => {}
        }

        // Focus the target element first
        match adapter.focus_element(target, tree).await {
            Ok(id) => {
                if let Some(elem) = adapter.get_element(id) {
                    println!(
                        "Focused element [{}] {:?} \"{}\"",
                        id,
                        elem.role,
                        elem.display_label()
                    );
                }
            }
            Err(e) => {
                return OperationResult::Error(format!("Focus failed: {}", e));
            }
        }
        // Small delay to allow focus to take effect
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send the keystroke using the new method
        match adapter.send_keystroke(key_spec).await {
            Ok((key, modifiers)) => {
                if !modifiers.is_empty() {
                    println!("Sent keystroke: {:?}+{:?}", modifiers, key);
                } else {
                    println!("Sent keystroke: {:?}", key);
                }
                return OperationResult::Success;
            }
            Err(e) => {
                return OperationResult::Error(format!(
                    "Keystroke failed: {}\nExamples: enter, space, cmd+c, ctrl+shift+a",
                    e
                ));
            }
        }
    }

    // Handle screenshot mode
    if args.screenshot {
        handle_screenshot_elements(adapter, tree, args.query.as_deref()).await;
        return OperationResult::Completed;
    }

    // Handle annotate mode
    if args.annotate {
        handle_annotate(adapter, tree, args).await;
        return OperationResult::Completed;
    }

    // Handle query mode
    if let Some(query) = &args.query {
        match adapter.find_elements(tree, Some(query.as_str()), false) {
            Ok(elements) => {
                if elements.is_empty() {
                    return OperationResult::NotFound(format!(
                        "No matches found for query: {}",
                        query
                    ));
                }
                println!(
                    "Found {} match{}:",
                    elements.len(),
                    if elements.len() == 1 { "" } else { "es" }
                );
                for elem in elements {
                    print_element_summary(elem);
                }
                return OperationResult::Success;
            }
            Err(e) => {
                return OperationResult::Error(e.to_string());
            }
        }
    }

    // Create the appropriate printer based on args
    let printer: Box<dyn Printer> = if args.json {
        Box::new(JsonPrinter)
    } else if args.llm_query {
        Box::new(LlmQueryPrinter::new(args.structure))
    } else if args.llm {
        Box::new(LlmPrinter::new(args.structure))
    } else {
        Box::new(TreePrinter)
    };

    let is_tree_mode = !args.json && !args.llm && !args.llm_query;

    // For Tree mode, print additional context
    if is_tree_mode {
        println!("=== {} Accessibility Tree ===", adapter.platform_name());
        println!("App: {:?}", tree.app_name);
        println!("PID: {:?}", tree.pid);
        println!("Version: {}", tree.version);
        println!("Element Count: {}", tree.element_count);
        println!();
    }

    // Print the tree using the selected printer
    print_formatted(tree, printer.as_ref());

    // For Tree mode, print additional statistics and interactive elements
    if is_tree_mode {
        println!();
        print_statistics(&tree.root);

        // Show interactive elements
        if let Ok(interactive) = adapter.find_elements(tree, None, false)
            && !interactive.is_empty()
        {
            println!();
            println!("=== Interactive Elements ({}) ===", interactive.len());
            for elem in interactive.iter().take(20) {
                println!("  [{}] {:?}: {}", elem.id, elem.role, elem.display_label());
                if !elem.actions.is_empty() {
                    println!("       Actions: {}", elem.actions.join(", "));
                }
            }
            if interactive.len() > 20 {
                println!("  ... and {} more", interactive.len() - 20);
            }
        }
    }

    OperationResult::Completed
}

/// Handle screenshot of individual elements.
async fn handle_screenshot_elements(
    adapter: &TargetedAccessibility,
    tree: &accessibility_core::accessibility::ElementTree,
    query: Option<&str>,
) {
    // Resolve elements (query or interactive with bounds)
    let (elements, description) = match adapter.find_elements(tree, query, true) {
        Ok(elems) => (
            elems,
            if query.is_some() {
                "matching"
            } else {
                "interactive"
            },
        ),
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };
    println!(
        "Found {} {} elements with bounds",
        elements.len(),
        description
    );

    let screenshot = match adapter.capture_screen() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to capture screen: {}", e);
            std::process::exit(1);
        }
    };

    let screen_bounds = match adapter.get_screen_bounds().await {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to get screen bounds: {}", e);
            std::process::exit(1);
        }
    };

    for (i, elem) in elements.iter().enumerate() {
        if let Some(bounds) = &elem.bounds {
            match screenshot.crop(bounds, &screen_bounds) {
                Ok(cropped) => {
                    let filename = screenshot_path();
                    if let Err(e) = std::fs::write(&filename, &cropped.data) {
                        eprintln!("Failed to write {}: {}", filename.display(), e);
                        std::process::exit(1);
                    }
                    println!(
                        "  [{}] {} {:?} \"{}\" -> {} ({}x{})",
                        i + 1,
                        elem.id,
                        elem.role,
                        truncate(&elem.display_label(), 30),
                        filename.display(),
                        cropped.width,
                        cropped.height
                    );
                }
                Err(e) => {
                    eprintln!("  [{}] {} {:?} -> ERROR: {}", i + 1, elem.id, elem.role, e);
                }
            }
        }
    }
}

/// Handle hit test.
async fn handle_hit_test(adapter: &mut TargetedAccessibility, x: f64, y: f64) {
    if !adapter.supports_hit_test() {
        eprintln!("Hit test is not supported on {}", adapter.platform_name());
        std::process::exit(1);
    }

    match adapter.hit_test(x, y).await {
        Ok(Some(id)) => {
            if let Some(elem) = adapter.get_element(id) {
                println!(
                    "Hit at ({}, {}): [{}] {:?} \"{}\"",
                    x,
                    y,
                    id,
                    elem.role,
                    elem.display_label()
                );
            } else {
                println!("Hit at ({}, {}): element [{}]", x, y, id);
            }
        }
        Ok(None) => println!("No element at ({}, {})", x, y),
        Err(e) => {
            eprintln!("Hit test failed: {}", e);
            std::process::exit(1);
        }
    }
}

async fn handle_mouse_click(adapter: &mut TargetedAccessibility, click: &MouseClickParams) {
    if !adapter.supports_mouse_click() {
        eprintln!(
            "Mouse clicks are not supported on {}",
            adapter.platform_name()
        );
        std::process::exit(1);
    }

    match adapter.mouse_click_at(click.x, click.y, click.button).await {
        Ok(()) => {
            println!(
                "Clicked at ({}, {}) with {} button",
                click.x, click.y, click.button
            );
        }
        Err(e) => {
            eprintln!("Mouse click failed: {}", e);
            std::process::exit(1);
        }
    }
}

/// Parse event type filter strings to AccessibilityEventType.
fn parse_event_type(s: &str) -> Option<AccessibilityEventType> {
    match s.to_lowercase().as_str() {
        "focus" | "focus-changed" => Some(AccessibilityEventType::FocusChanged),
        "value" | "value-changed" => Some(AccessibilityEventType::ValueChanged),
        "title" | "title-changed" => Some(AccessibilityEventType::TitleChanged),
        "structure" | "structure-changed" => Some(AccessibilityEventType::StructureChanged),
        "window-created" => Some(AccessibilityEventType::WindowCreated),
        "window-destroyed" => Some(AccessibilityEventType::WindowDestroyed),
        "window-focus" | "window-focus-changed" => Some(AccessibilityEventType::WindowFocusChanged),
        "text-selected" | "selected-text-changed" => {
            Some(AccessibilityEventType::SelectedTextChanged)
        }
        "element-destroyed" => Some(AccessibilityEventType::ElementDestroyed),
        _ => None,
    }
}

/// Handle event listening mode.
async fn handle_event_listening(
    adapter: &mut TargetedAccessibility,
    args: &CommonArgs,
    target_pid: Option<u32>,
) {
    if !adapter.supports_event_listening() {
        eprintln!(
            "Event listening is not supported on {}",
            adapter.platform_name()
        );
        std::process::exit(1);
    }

    // Build config with optional event type filter
    let mut config = ListenerConfig::new().with_buffer_size(256);

    // Honor --pid: start_listening reads the PID from ListenerConfig, not the
    // adapter's target PID, so without this every process' events would stream in.
    if let Some(pid) = target_pid {
        config = config.with_pid(pid);
    }

    if let Some(filter_strs) = &args.listen_filter {
        let event_types: Vec<AccessibilityEventType> = filter_strs
            .iter()
            .filter_map(|s| {
                let parsed = parse_event_type(s);
                if parsed.is_none() {
                    eprintln!("Warning: Unknown event type '{}', ignoring", s);
                }
                parsed
            })
            .collect();

        if !event_types.is_empty() {
            config = config.with_event_types(event_types.clone());
            println!("Filtering events: {:?}", event_types);
        }
    }

    println!(
        "Starting accessibility event listener on {}...",
        adapter.platform_name()
    );
    println!("Press Ctrl+C to stop.\n");

    // Set up Ctrl+C handler
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    if let Err(e) = ctrlc::set_handler(move || {
        println!("\nStopping event listener...");
        running_clone.store(false, Ordering::SeqCst);
    }) {
        eprintln!("Failed to set Ctrl+C handler: {}", e);
        std::process::exit(1);
    }

    // Track event counts
    let event_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let event_count_clone = event_count.clone();
    let running_for_callback = running.clone();

    // Start listening with callback that prints events
    let handle = adapter.start_listening(
        config,
        Box::new(move |event| {
            // Check if we should stop
            if !running_for_callback.load(Ordering::SeqCst) {
                return;
            }

            let count = event_count_clone.fetch_add(1, Ordering::SeqCst) + 1;

            match &event {
                AccessibilityEvent::FocusChanged { element, pid, .. } => {
                    let elem_info = element
                        .as_ref()
                        .map(|e| format!("[{}] {:?} \"{}\"", e.id, e.role, e.display_label()))
                        .unwrap_or_else(|| "None".to_string());
                    println!(
                        "[{}] FOCUS_CHANGED pid={:?} element={}",
                        count, pid, elem_info
                    );
                    if let Some(elem) = element
                        && let Some(bounds) = &elem.bounds
                    {
                        println!(
                            "       bounds: ({:.0}, {:.0}) {}x{}",
                            bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height
                        );
                    }
                }
                AccessibilityEvent::ValueChanged {
                    element,
                    old_value,
                    new_value,
                    ..
                } => {
                    let elem_info = element
                        .as_ref()
                        .map(|e| format!("[{}] {:?} \"{}\"", e.id, e.role, e.display_label()))
                        .unwrap_or_else(|| "None".to_string());
                    println!("[{}] VALUE_CHANGED element={}", count, elem_info);
                    if old_value.is_some() || new_value.is_some() {
                        println!(
                            "       old=\"{}\" new=\"{}\"",
                            old_value.as_deref().unwrap_or(""),
                            new_value.as_deref().unwrap_or("")
                        );
                    }
                }
                AccessibilityEvent::TitleChanged {
                    element,
                    old_title,
                    new_title,
                    ..
                } => {
                    let elem_info = element
                        .as_ref()
                        .map(|e| format!("[{}] {:?}", e.id, e.role))
                        .unwrap_or_else(|| "None".to_string());
                    println!("[{}] TITLE_CHANGED element={}", count, elem_info);
                    println!(
                        "       old=\"{}\" new=\"{}\"",
                        old_title.as_deref().unwrap_or(""),
                        new_title.as_deref().unwrap_or("")
                    );
                }
                AccessibilityEvent::StructureChanged {
                    parent_element,
                    change_type,
                    ..
                } => {
                    let parent_info = parent_element
                        .as_ref()
                        .map(|e| format!("[{}] {:?} \"{}\"", e.id, e.role, e.display_label()))
                        .unwrap_or_else(|| "None".to_string());
                    println!(
                        "[{}] STRUCTURE_CHANGED type={:?} parent={}",
                        count, change_type, parent_info
                    );
                }
                AccessibilityEvent::WindowCreated { element, pid, .. } => {
                    let elem_info = element
                        .as_ref()
                        .map(|e| format!("[{}] {:?} \"{}\"", e.id, e.role, e.display_label()))
                        .unwrap_or_else(|| "None".to_string());
                    println!(
                        "[{}] WINDOW_CREATED pid={:?} element={}",
                        count, pid, elem_info
                    );
                }
                AccessibilityEvent::WindowDestroyed { window_id, pid, .. } => {
                    println!(
                        "[{}] WINDOW_DESTROYED pid={:?} window_id={:?}",
                        count, pid, window_id
                    );
                }
                AccessibilityEvent::WindowFocusChanged { element, pid, .. } => {
                    let elem_info = element
                        .as_ref()
                        .map(|e| format!("[{}] {:?} \"{}\"", e.id, e.role, e.display_label()))
                        .unwrap_or_else(|| "None".to_string());
                    println!(
                        "[{}] WINDOW_FOCUS_CHANGED pid={:?} element={}",
                        count, pid, elem_info
                    );
                }
                AccessibilityEvent::SelectedTextChanged {
                    element,
                    selected_text,
                    ..
                } => {
                    let elem_info = element
                        .as_ref()
                        .map(|e| format!("[{}] {:?}", e.id, e.role))
                        .unwrap_or_else(|| "None".to_string());
                    println!(
                        "[{}] SELECTED_TEXT_CHANGED element={} text=\"{}\"",
                        count,
                        elem_info,
                        selected_text.as_deref().unwrap_or("")
                    );
                }
                AccessibilityEvent::ElementDestroyed { element_id, .. } => {
                    println!("[{}] ELEMENT_DESTROYED id={:?}", count, element_id);
                }
                AccessibilityEvent::Error { message, .. } => {
                    eprintln!("[{}] ERROR: {}", count, message);
                }
                AccessibilityEvent::Stopped { reason, .. } => {
                    println!("[{}] STOPPED reason={:?}", count, reason);
                }
            }
        }),
    );

    let handle = match handle {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Failed to start event listener: {}", e);
            std::process::exit(1);
        }
    };

    // Wait for stop signal using async sleep
    while running.load(Ordering::SeqCst) && handle.is_running() {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Stop the listener
    handle.stop().await;

    let total = event_count.load(Ordering::SeqCst);
    println!("\nEvent listener stopped. Total events received: {}", total);
}

/// Check if this operation type supports timeout polling.
/// Only element-targeting operations (query, click, focus, blur, type, key) support polling.
fn operation_supports_timeout(args: &CommonArgs) -> bool {
    args.query.is_some()
        || args.click.is_some()
        || args.focus.is_some()
        || args.blur.is_some()
        || args.type_value.is_some()
        || args.key.is_some()
}

/// Unified entry point for running the CLI with TargetedAccessibility.
/// Handles screenshot, annotate, hit test, and common operations.
async fn run_platform(
    adapter: &mut TargetedAccessibility,
    args: &CommonArgs,
    filter: &TreeFilter,
    hit_test_coords: Option<(f64, f64)>,
    target_pid: Option<u32>,
) {
    // Handle event listening mode
    if args.listen {
        handle_event_listening(adapter, args, target_pid).await;
        return;
    }

    // Handle screenshot-screen or overlay-only mode
    if args.screenshot_screen || (args.overlay && !args.annotate) {
        handle_screenshot_screen(adapter, args).await;
        return;
    }

    if let Some(click) = &args.mouse_click {
        handle_mouse_click(adapter, click).await;
        return;
    }

    // Determine if we should use timeout polling
    let use_polling = args.timeout > 0 && operation_supports_timeout(args);

    if use_polling {
        // Polling loop: repeatedly refresh tree and try operation until success or timeout
        let timeout_ms = args.timeout;
        let poll_interval_ms = args.poll_interval;
        let start = std::time::Instant::now();

        loop {
            // Clear cache and get fresh tree. Transient tree-build failures
            // are normal during animations / redraws — retry until timeout
            // rather than exit, since the whole point of polling is to wait
            // for the UI to stabilize.
            adapter.clear_cache();
            let tree = match adapter.get_tree(filter).await {
                Ok(t) => t,
                Err(e) => {
                    let elapsed = start.elapsed().as_millis() as u64;
                    if elapsed >= timeout_ms {
                        eprintln!("Failed to get accessibility tree after {}ms: {}", elapsed, e);
                        std::process::exit(1);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(poll_interval_ms)).await;
                    continue;
                }
            };

            // Try the operation
            let result = handle_common_operations(adapter, args, &tree, filter).await;

            match result {
                OperationResult::Success | OperationResult::Completed => {
                    // Operation succeeded, we're done
                    return;
                }
                OperationResult::NotFound(msg) => {
                    // Check if we've exceeded timeout
                    let elapsed = start.elapsed().as_millis() as u64;
                    if elapsed >= timeout_ms {
                        eprintln!("Timeout after {}ms: {}", elapsed, msg);
                        std::process::exit(1);
                    }
                    // Sleep and retry
                    tokio::time::sleep(std::time::Duration::from_millis(poll_interval_ms)).await;
                }
                OperationResult::Error(e) => {
                    eprintln!("{}", e);
                    std::process::exit(1);
                }
            }
        }
    } else {
        // Non-polling path: original behavior
        // Get tree
        let tree = match adapter.get_tree(filter).await {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Failed to get accessibility tree: {}", e);
                std::process::exit(1);
            }
        };

        // Handle hit test if coordinates provided
        if let Some((x, y)) = hit_test_coords {
            handle_hit_test(adapter, x, y).await;
            return;
        }

        // Handle annotate mode (with tree)
        if args.annotate || args.screenshot {
            handle_annotate(adapter, &tree, args).await;
            return;
        }

        // Handle common operations (click, focus, blur, type, key, query, default output)
        let result = handle_common_operations(adapter, args, &tree, filter).await;

        // Handle the result for non-polling path
        match result {
            OperationResult::Success | OperationResult::Completed => {
                // Success, nothing more to do
            }
            OperationResult::NotFound(msg) => {
                // No timeout set, just print the message and exit successfully
                // (for query this is normal behavior - just prints "No matches found")
                println!("{}", msg);
            }
            OperationResult::Error(e) => {
                eprintln!("{}", e);
                std::process::exit(1);
            }
        }
    }
}

#[derive(Parser)]
#[command(name = "accessibility-cli")]
#[command(
    about = r###"Cross-platform accessibility CLI for macOS, Windows, iOS Simulator, Linux, and Android.
Usage:
    accessibility-cli --platform mac [OPTIONS]       # macOS accessibility
    accessibility-cli --platform win [OPTIONS]       # Windows accessibility
    accessibility-cli --platform ios [OPTIONS]       # iOS Simulator accessibility
    accessibility-cli --platform linux [OPTIONS]     # Linux accessibility (AT-SPI)
    accessibility-cli --platform android [OPTIONS]   # Android device/emulator (ADB)

Examples:
    accessibility-cli --platform mac --pid 123 --llm                    # Query specific macOS app
    accessibility-cli --platform mac --pid 123 --mouse-click 300,240    # Background pixel click on macOS
    accessibility-cli --platform mac --key "cmd+c" "[title=Username]"   # Send Cmd+C to username field
    accessibility-cli --platform win --pid 123 --llm                    # Query specific Windows app
    accessibility-cli --platform win --key "ctrl+c" "[title=Username]"  # Send Ctrl+C to username field
    accessibility-cli --platform ios --udid ABC --annotate              # Annotated iOS screenshot
    accessibility-cli --platform linux --pid 123 --llm                  # Query specific Linux app
    accessibility-cli --platform android --serial ABC --llm             # Query Android device
    accessibility-cli --platform android --adb-back                     # Press Android back button
    accessibility-cli --platform android --adb-launch com.example.app   # Launch Android app
"###
)]
#[command(version)]
pub struct Cli {
    /// Target platform (defaults to current OS)
    #[arg(long, short = 'p', value_enum, default_value_t = PlatformType::default())]
    pub platform: PlatformType,

    /// Target application by process ID (default: focused app)
    /// Used for mac, win, linux platforms
    #[arg(long)]
    pub pid: Option<u32>,

    /// Target simulator by UDID (default: first booted)
    /// Used for ios platform
    #[arg(long)]
    pub udid: Option<String>,

    /// Target Android device by serial (default: only connected device)
    /// Used for android platform. Use `adb devices` to list connected devices.
    #[arg(long)]
    pub serial: Option<String>,

    /// Hit test at screen coordinates (x,y)
    /// Used for mac, win, linux platforms
    #[arg(long, value_parser = parse_coords)]
    pub hit: Option<(f64, f64)>,

    /// Test framework loading only (iOS only)
    #[arg(long)]
    pub test_load: bool,

    /// Press element by ID (iOS accessibility)
    #[arg(long)]
    pub press: Option<u64>,

    /// Tap at coordinates via accessibility (x,y) (iOS only)
    #[arg(long, value_parser = parse_coords)]
    pub tap: Option<(f64, f64)>,

    #[command(flatten)]
    pub common: CommonArgs,

    #[cfg(target_os = "macos")]
    #[command(flatten)]
    pub hid: HIDArgs,

    #[command(flatten)]
    pub adb: AndroidArgs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum PlatformType {
    /// macOS accessibility (uses AXUIElement API)
    #[value(name = "mac")]
    MacOS,

    /// Windows accessibility (uses UI Automation API)
    #[value(name = "win")]
    Windows,

    /// iOS Simulator accessibility (uses AccessibilityPlatformTranslation)
    #[value(name = "ios")]
    IOS,

    /// Linux accessibility (uses AT-SPI via D-Bus)
    #[value(name = "linux")]
    Linux,

    /// Android device/emulator (uses ADB)
    #[value(name = "android")]
    Android,
}

impl Default for PlatformType {
    fn default() -> Self {
        #[cfg(target_os = "macos")]
        return PlatformType::MacOS;
        #[cfg(target_os = "windows")]
        return PlatformType::Windows;
        #[cfg(target_os = "linux")]
        return PlatformType::Linux;
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        return PlatformType::Linux; // Fallback
    }
}

#[cfg(target_os = "macos")]
#[derive(Args)]
pub struct HIDArgs {
    /// HID tap at coordinates (x,y)
    #[arg(long, value_parser = parse_coords)]
    hid_tap: Option<(f64, f64)>,

    /// HID swipe from start to end (x1,y1,x2,y2[,duration_ms])
    #[arg(long, value_parser = parse_swipe)]
    hid_swipe: Option<SwipeParams>,

    /// Press Home button
    #[arg(long)]
    hid_home: bool,

    /// Press Lock button
    #[arg(long)]
    hid_lock: bool,

    /// Press Siri button
    #[arg(long)]
    hid_siri: bool,

    /// Press Side button
    #[arg(long)]
    hid_side: bool,
}

/// Android-specific arguments for ADB operations.
#[derive(Args)]
pub struct AndroidArgs {
    /// Press Android Back button
    #[arg(long)]
    adb_back: bool,

    /// Press Android Home button
    #[arg(long)]
    adb_home: bool,

    /// Press Android Recent Apps button
    #[arg(long)]
    adb_recent: bool,

    /// Press Android Menu button
    #[arg(long)]
    adb_menu: bool,

    /// Increase volume
    #[arg(long)]
    adb_volume_up: bool,

    /// Decrease volume
    #[arg(long)]
    adb_volume_down: bool,

    /// ADB tap at coordinates (x,y)
    #[arg(long, value_parser = parse_coords)]
    adb_tap: Option<(f64, f64)>,

    /// ADB swipe from start to end (x1,y1,x2,y2[,duration_ms])
    #[arg(long, value_parser = parse_swipe_coords)]
    adb_swipe: Option<SwipeParams>,

    /// ADB long press at coordinates (x,y,duration_ms)
    #[arg(long, value_parser = parse_long_press)]
    adb_long_press: Option<(f64, f64, u64)>,

    /// Launch Android app by package name
    #[arg(long)]
    adb_launch: Option<String>,

    /// Stop Android app by package name
    #[arg(long)]
    adb_stop: Option<String>,

    /// Open notification shade
    #[arg(long)]
    adb_notifications: bool,

    /// Open quick settings
    #[arg(long)]
    adb_quick_settings: bool,

    /// Wake up the device
    #[arg(long)]
    adb_wake: bool,

    /// Put the device to sleep
    #[arg(long)]
    adb_sleep: bool,
}

#[derive(Args)]
pub struct CommonArgs {
    /// Maximum tree depth
    #[arg(long)]
    depth: Option<usize>,

    /// Only show interactive elements
    #[arg(long)]
    interactive: bool,

    /// Only show visible elements
    #[arg(long)]
    visible: bool,

    /// Output as JSON
    #[arg(long)]
    json: bool,

    /// Compact LLM-friendly output (concise format)
    #[arg(long)]
    llm: bool,

    /// Verbose LLM output with CSS-like selectors (detailed format)
    #[arg(long)]
    llm_query: bool,

    /// Structure-only output (with --llm or --llm-query)
    #[arg(long)]
    structure: bool,

    /// Capture screenshots of interactive elements
    #[arg(long)]
    screenshot: bool,

    /// Capture full screen screenshot
    #[arg(long)]
    screenshot_screen: bool,

    /// Annotated screenshot with element boxes
    #[arg(long)]
    annotate: bool,

    /// Add numbered labels (with --annotate)
    #[arg(long)]
    label: bool,

    /// Add coordinate grid overlay
    #[arg(long)]
    overlay: bool,

    /// Grid cell size in points (default: 100)
    #[arg(long, default_value = "100")]
    grid_size: u32,

    /// CSS-like query
    #[arg(short, long)]
    query: Option<String>,

    /// Click/activate element by query
    /// Examples: --click "Button", --click "[title=Submit]", --click "Link[title=Login]"
    #[arg(long)]
    click: Option<String>,

    /// Focus element by query
    /// Examples: --focus "TextField", --focus "[title=Search]"
    #[arg(long)]
    focus: Option<String>,

    /// Blur (remove focus from) element by query
    /// Examples: --blur "TextField", --blur "[title=Search]"
    #[arg(long)]
    blur: Option<String>,

    /// Set value on element by query
    /// Usage: `--type <QUERY> <TEXT>`
    /// Examples: --type "TextField" "hello", --type "[title=Email]" "user@example.com"
    #[arg(long = "type", num_args = 2)]
    type_value: Option<Vec<String>>,

    /// Send keystroke to a focused element
    /// Usage: `--key <KEYSTROKE> <SELECTOR>`
    /// Examples: --key enter "TextField", --key cmd+c "[title=Username]", --key enter "Link[title=Submit]"
    #[arg(long, num_args = 2)]
    key: Option<Vec<String>>,

    /// Click at absolute screen coordinates.
    /// Usage: --mouse-click x,y[,button]
    /// Buttons: left, right, middle
    #[arg(long, value_parser = parse_mouse_click)]
    mouse_click: Option<MouseClickParams>,

    /// Listen for and log accessibility events in real-time
    /// Press Ctrl+C to stop listening
    #[arg(long)]
    listen: bool,

    /// Filter event types when using --listen (comma-separated)
    /// Available: focus,value,title,structure,window-created,window-destroyed,window-focus,text-selected
    /// Example: --listen-filter focus,value
    #[arg(long, value_delimiter = ',')]
    listen_filter: Option<Vec<String>>,

    /// Timeout in milliseconds for query operations (polls until element found or timeout)
    /// When specified with --query, --click, --focus, --blur, --type, or --key,
    /// the CLI will repeatedly refresh the accessibility tree until a match is found.
    /// Set to 0 to disable polling and return immediately if not found.
    #[arg(long, default_value = "30000", value_name = "MS")]
    timeout: u64,

    /// Poll interval in milliseconds when using --timeout (default: 100)
    #[arg(long, default_value = "100", value_name = "MS")]
    poll_interval: u64,
}

/// Parameters for a swipe gesture.
#[derive(Clone, Debug)]
pub struct SwipeParams {
    pub start: (f64, f64),
    pub end: (f64, f64),
    pub duration_ms: u64,
}

/// Parameters for a mouse click.
#[derive(Clone, Debug)]
pub struct MouseClickParams {
    pub x: f64,
    pub y: f64,
    pub button: MouseButton,
}

fn parse_coords(s: &str) -> Result<(f64, f64), String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 2 {
        return Err("Expected format: x,y".to_string());
    }
    let x = parts[0]
        .trim()
        .parse()
        .map_err(|_| "Invalid x coordinate")?;
    let y = parts[1]
        .trim()
        .parse()
        .map_err(|_| "Invalid y coordinate")?;
    Ok((x, y))
}

fn parse_mouse_click(s: &str) -> Result<MouseClickParams, String> {
    let parts: Vec<&str> = s.split(',').collect();
    if !(2..=3).contains(&parts.len()) {
        return Err("Expected format: x,y[,button]".to_string());
    }

    let x = parts[0]
        .trim()
        .parse()
        .map_err(|_| "Invalid x coordinate")?;
    let y = parts[1]
        .trim()
        .parse()
        .map_err(|_| "Invalid y coordinate")?;
    let button = parts
        .get(2)
        .map(|button| {
            MouseButton::from_name(button.trim())
                .ok_or_else(|| "Invalid button; expected left, right, or middle".to_string())
        })
        .transpose()?
        .unwrap_or_default();

    Ok(MouseClickParams { x, y, button })
}

/// Parse swipe parameters from a string.
fn parse_swipe_coords(s: &str) -> Result<SwipeParams, String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() < 4 {
        return Err("Expected format: x1,y1,x2,y2[,duration_ms]".to_string());
    }
    let x1 = parts[0].trim().parse().map_err(|_| "Invalid x1")?;
    let y1 = parts[1].trim().parse().map_err(|_| "Invalid y1")?;
    let x2 = parts[2].trim().parse().map_err(|_| "Invalid x2")?;
    let y2 = parts[3].trim().parse().map_err(|_| "Invalid y2")?;
    let duration_ms: u64 = match parts.get(4) {
        Some(s) => s
            .trim()
            .parse()
            .map_err(|_| "Invalid duration_ms".to_string())?,
        None => 300,
    };
    Ok(SwipeParams {
        start: (x1, y1),
        end: (x2, y2),
        duration_ms,
    })
}

/// Parse swipe parameters (alias for iOS HID).
#[cfg(target_os = "macos")]
fn parse_swipe(s: &str) -> Result<SwipeParams, String> {
    parse_swipe_coords(s)
}

/// Parse long press parameters (x,y,duration_ms).
fn parse_long_press(s: &str) -> Result<(f64, f64, u64), String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() < 2 {
        return Err("Expected format: x,y[,duration_ms]".to_string());
    }
    let x: f64 = parts[0]
        .trim()
        .parse()
        .map_err(|_| "Invalid x coordinate")?;
    let y: f64 = parts[1]
        .trim()
        .parse()
        .map_err(|_| "Invalid y coordinate")?;
    let duration_ms: u64 = match parts.get(2) {
        Some(s) => s
            .trim()
            .parse()
            .map_err(|_| "Invalid duration_ms".to_string())?,
        None => 1000,
    };
    Ok((x, y, duration_ms))
}

/// Run the CLI using process arguments.
pub fn run() {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("Failed to create tokio runtime: {}", e);
            std::process::exit(1);
        }
    };
    runtime.block_on(run_cli(&cli));
}

/// Build a TreeFilter from CommonArgs
fn build_filter(common: &CommonArgs) -> TreeFilter {
    TreeFilter {
        max_depth: common.depth,
        max_elements: Some(1000),
        interactive_only: common.interactive,
        visible_only: common.visible,
        within_bounds: None,
        roles: None,
    }
}

/// Reject platform-specific flags that don't match `--platform`.
///
/// Without this, e.g. `--platform mac --tap 100,100` silently dumps the tree
/// instead of running the requested tap — the iOS/HID/ADB flags are only
/// consumed inside their respective platform arms.
fn validate_platform_flags(cli: &Cli) -> Result<(), String> {
    let ios_only_set = cli.test_load || cli.press.is_some() || cli.tap.is_some();
    #[cfg(target_os = "macos")]
    let hid_set = cli.hid.hid_tap.is_some()
        || cli.hid.hid_swipe.is_some()
        || cli.hid.hid_home
        || cli.hid.hid_lock
        || cli.hid.hid_siri
        || cli.hid.hid_side;
    #[cfg(not(target_os = "macos"))]
    let hid_set = false;

    let adb = &cli.adb;
    let adb_set = adb.adb_back
        || adb.adb_home
        || adb.adb_recent
        || adb.adb_menu
        || adb.adb_volume_up
        || adb.adb_volume_down
        || adb.adb_tap.is_some()
        || adb.adb_swipe.is_some()
        || adb.adb_long_press.is_some()
        || adb.adb_launch.is_some()
        || adb.adb_stop.is_some()
        || adb.adb_notifications
        || adb.adb_quick_settings
        || adb.adb_wake
        || adb.adb_sleep;

    if (ios_only_set || hid_set) && cli.platform != PlatformType::IOS {
        return Err(
            "iOS-only flags (--tap, --press, --test-load, --hid-*) require --platform ios".into(),
        );
    }
    if adb_set && cli.platform != PlatformType::Android {
        return Err("--adb-* flags require --platform android".into());
    }
    Ok(())
}

pub async fn run_cli(cli: &Cli) {
    if let Err(msg) = validate_platform_flags(cli) {
        eprintln!("error: {}", msg);
        std::process::exit(2);
    }

    // Handle iOS test-load early (doesn't need adapter)
    #[cfg(target_os = "macos")]
    if cli.platform == PlatformType::IOS && cli.test_load {
        println!("Testing framework loading...");
        match accessibility_core::platform::ios_simulator::load_frameworks() {
            Ok(()) => {
                println!("Frameworks loaded successfully!");
                println!("  - AccessibilityPlatformTranslation.framework: OK");
                println!("  - CoreSimulator.framework: OK");
            }
            Err(e) => {
                eprintln!("Failed to load frameworks: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }

    let filter = build_filter(&cli.common);

    match cli.platform {
        #[cfg(target_os = "macos")]
        PlatformType::MacOS => {
            // Check accessibility permissions
            if !MacOSAccessibility::is_process_trusted() {
                eprintln!("Error: Accessibility permissions not granted.");
                eprintln!();
                eprintln!("Please enable accessibility access for this terminal/app:");
                eprintln!("  1. Open System Preferences > Privacy & Security > Accessibility");
                eprintln!("  2. Click the lock icon to make changes");
                eprintln!("  3. Add and enable your terminal app (Terminal, iTerm2, etc.)");
                std::process::exit(1);
            }

            let mut adapter = match TargetedAccessibility::new_macos(cli.pid) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("Failed to create macOS adapter: {}", e);
                    std::process::exit(1);
                }
            };
            run_platform(&mut adapter, &cli.common, &filter, cli.hit, cli.pid).await;
        }

        #[cfg(target_os = "macos")]
        PlatformType::IOS => {
            // For iOS-specific commands (HID, tap, press), use the raw adapter
            // Then create TargetedAccessibility for common operations
            let mut ios_adapter = match IOSSimulatorAccessibility::new(cli.udid.as_deref()) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("Failed to create iOS Simulator adapter: {}", e);
                    eprintln!();
                    eprintln!("Make sure:");
                    eprintln!("  1. iOS Simulator is running");
                    eprintln!("  2. A simulator is booted (not just the Simulator.app window)");
                    eprintln!("  3. An app is open and in focus in the simulator");
                    eprintln!("  4. Xcode is installed (for CoreSimulator framework)");
                    std::process::exit(1);
                }
            };

            if !cli.common.llm && !cli.common.json {
                println!("Connected to simulator: {}", ios_adapter.device_udid());
            }

            // Handle iOS-specific commands (HID, tap, press) before common operations.
            if handle_ios_specific(&mut ios_adapter, cli) {
                return;
            }

            // For common operations, use TargetedAccessibility
            let mut adapter = match TargetedAccessibility::new_ios(cli.udid.as_deref()) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("Failed to create iOS adapter: {}", e);
                    std::process::exit(1);
                }
            };
            run_platform(&mut adapter, &cli.common, &filter, None, None).await;
        }

        #[cfg(target_os = "windows")]
        PlatformType::Windows => {
            let mut adapter = match TargetedAccessibility::new_windows(cli.pid) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("Failed to create Windows adapter: {}", e);
                    std::process::exit(1);
                }
            };
            run_platform(&mut adapter, &cli.common, &filter, cli.hit, cli.pid).await;
        }

        #[cfg(target_os = "linux")]
        PlatformType::Linux => {
            let mut adapter = match TargetedAccessibility::new_linux(cli.pid).await {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("Failed to create Linux adapter: {}", e);
                    eprintln!();
                    eprintln!("Make sure:");
                    eprintln!("  1. AT-SPI2 is running (accessibility services enabled)");
                    eprintln!("  2. The target application supports accessibility");
                    std::process::exit(1);
                }
            };
            run_platform(&mut adapter, &cli.common, &filter, cli.hit, cli.pid).await;
        }

        // Android works on all host platforms via ADB
        PlatformType::Android => {
            // Create raw AndroidAccessibility for Android-specific commands
            let mut android_adapter = match AndroidAccessibility::new(cli.serial.as_deref()) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("Failed to create Android adapter: {}", e);
                    eprintln!();
                    eprintln!("Make sure:");
                    eprintln!("  1. ADB is installed and in your PATH");
                    eprintln!("  2. An Android device/emulator is connected (`adb devices`)");
                    eprintln!("  3. USB debugging is enabled on the device");
                    std::process::exit(1);
                }
            };

            if !cli.common.llm && !cli.common.json {
                println!(
                    "Connected to Android device{}",
                    cli.serial
                        .as_ref()
                        .map(|s| format!(" ({})", s))
                        .unwrap_or_default()
                );
            }

            // Handle Android-specific commands first
            if handle_android_specific(&mut android_adapter, cli).await {
                return;
            }

            // For common operations, use TargetedAccessibility
            let mut adapter = match TargetedAccessibility::new_android(cli.serial.as_deref()) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("Failed to create Android adapter: {}", e);
                    std::process::exit(1);
                }
            };
            run_platform(&mut adapter, &cli.common, &filter, None, None).await;
        }

        // Unsupported platform combinations
        #[cfg(not(target_os = "macos"))]
        PlatformType::MacOS => {
            eprintln!("Error: macOS platform is only supported on macOS");
            std::process::exit(1);
        }
        #[cfg(not(target_os = "windows"))]
        PlatformType::Windows => {
            eprintln!("Error: Windows platform is only supported on Windows");
            std::process::exit(1);
        }
        #[cfg(not(target_os = "macos"))]
        PlatformType::IOS => {
            eprintln!("Error: iOS platform is only supported on macOS (via Simulator)");
            std::process::exit(1);
        }
        #[cfg(not(target_os = "linux"))]
        PlatformType::Linux => {
            eprintln!("Error: Linux platform is only supported on Linux");
            std::process::exit(1);
        }
    }
}

/// Handle iOS-specific commands (HID, tap, press). Returns true if a command was handled.
#[cfg(target_os = "macos")]
fn handle_ios_specific(adapter: &mut IOSSimulatorAccessibility, cli: &Cli) -> bool {
    // Handle HID tap
    if let Some((x, y)) = cli.hid.hid_tap {
        println!("HID tap at ({}, {})...", x, y);
        match adapter.hid_tap(x, y) {
            Ok(()) => println!("HID tap successful!"),
            Err(e) => {
                eprintln!("HID tap failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    // Handle HID swipe
    if let Some(ref swipe) = cli.hid.hid_swipe {
        println!(
            "HID swipe from ({},{}) to ({},{}) over {}ms...",
            swipe.start.0, swipe.start.1, swipe.end.0, swipe.end.1, swipe.duration_ms
        );
        match adapter.hid_swipe(swipe.start, swipe.end, swipe.duration_ms) {
            Ok(()) => println!("HID swipe successful!"),
            Err(e) => {
                eprintln!("HID swipe failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    // Handle HID buttons
    if cli.hid.hid_home {
        handle_hid_button(
            adapter,
            accessibility_core::platform::ios_simulator::HardwareButton::Home,
            "Home",
        );
        return true;
    }
    if cli.hid.hid_lock {
        handle_hid_button(
            adapter,
            accessibility_core::platform::ios_simulator::HardwareButton::Lock,
            "Lock",
        );
        return true;
    }
    if cli.hid.hid_siri {
        handle_hid_button(
            adapter,
            accessibility_core::platform::ios_simulator::HardwareButton::Siri,
            "Siri",
        );
        return true;
    }
    if cli.hid.hid_side {
        handle_hid_button(
            adapter,
            accessibility_core::platform::ios_simulator::HardwareButton::SideButton,
            "Side",
        );
        return true;
    }

    // Handle iOS-specific accessibility tap
    if let Some((x, y)) = cli.tap {
        println!("Tapping at ({}, {})...", x, y);
        match adapter.tap(x, y) {
            Ok(()) => println!("Tap successful!"),
            Err(e) => {
                eprintln!("Tap failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    // Handle press by ID
    if let Some(id) = cli.press {
        println!("Pressing element {}...", id);
        let key = accessibility_core::accessibility::ElementKey::from_ffi(id);
        match adapter.press(key) {
            Ok(()) => println!("Press successful!"),
            Err(e) => {
                eprintln!("Press failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    false
}

#[cfg(target_os = "macos")]
fn handle_hid_button(
    adapter: &mut IOSSimulatorAccessibility,
    button: accessibility_core::platform::ios_simulator::HardwareButton,
    name: &str,
) {
    println!("HID {} button press...", name);
    match adapter.hid_button(button, 0) {
        Ok(()) => println!("HID {} button press successful!", name),
        Err(e) => {
            eprintln!("HID {} button press failed: {}", name, e);
            std::process::exit(1);
        }
    }
}

/// Handle Android-specific commands (buttons, tap, swipe, launch, etc.). Returns true if a command was handled.
async fn handle_android_specific(adapter: &mut AndroidAccessibility, cli: &Cli) -> bool {
    // Handle Android button presses
    if cli.adb.adb_back {
        println!("Pressing Back button...");
        match adapter.press_back().await {
            Ok(()) => println!("Back button pressed!"),
            Err(e) => {
                eprintln!("Back button failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    if cli.adb.adb_home {
        println!("Pressing Home button...");
        match adapter.press_home().await {
            Ok(()) => println!("Home button pressed!"),
            Err(e) => {
                eprintln!("Home button failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    if cli.adb.adb_recent {
        println!("Pressing Recent Apps button...");
        match adapter.press_recent_apps().await {
            Ok(()) => println!("Recent Apps button pressed!"),
            Err(e) => {
                eprintln!("Recent Apps button failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    if cli.adb.adb_menu {
        println!("Pressing Menu button...");
        match adapter.press_menu().await {
            Ok(()) => println!("Menu button pressed!"),
            Err(e) => {
                eprintln!("Menu button failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    if cli.adb.adb_volume_up {
        println!("Pressing Volume Up...");
        match adapter.volume_up().await {
            Ok(()) => println!("Volume Up pressed!"),
            Err(e) => {
                eprintln!("Volume Up failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    if cli.adb.adb_volume_down {
        println!("Pressing Volume Down...");
        match adapter.volume_down().await {
            Ok(()) => println!("Volume Down pressed!"),
            Err(e) => {
                eprintln!("Volume Down failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    if cli.adb.adb_wake {
        println!("Waking device...");
        match adapter.wake_up().await {
            Ok(()) => println!("Device woken up!"),
            Err(e) => {
                eprintln!("Wake up failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    if cli.adb.adb_sleep {
        println!("Putting device to sleep...");
        match adapter.sleep().await {
            Ok(()) => println!("Device put to sleep!"),
            Err(e) => {
                eprintln!("Sleep failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    if cli.adb.adb_notifications {
        println!("Opening notification shade...");
        match adapter.open_notifications().await {
            Ok(()) => println!("Notification shade opened!"),
            Err(e) => {
                eprintln!("Open notifications failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    if cli.adb.adb_quick_settings {
        println!("Opening quick settings...");
        match adapter.open_quick_settings().await {
            Ok(()) => println!("Quick settings opened!"),
            Err(e) => {
                eprintln!("Open quick settings failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    // Handle ADB tap
    if let Some((x, y)) = cli.adb.adb_tap {
        println!("Tapping at ({}, {})...", x, y);
        match adapter.adb().tap(x, y) {
            Ok(()) => println!("Tap successful!"),
            Err(e) => {
                eprintln!("Tap failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    // Handle ADB swipe
    if let Some(ref swipe) = cli.adb.adb_swipe {
        println!(
            "Swiping from ({},{}) to ({},{}) over {}ms...",
            swipe.start.0, swipe.start.1, swipe.end.0, swipe.end.1, swipe.duration_ms
        );
        match adapter
            .swipe(swipe.start, swipe.end, swipe.duration_ms)
            .await
        {
            Ok(()) => println!("Swipe successful!"),
            Err(e) => {
                eprintln!("Swipe failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    // Handle ADB long press
    if let Some((x, y, duration_ms)) = cli.adb.adb_long_press {
        println!("Long pressing at ({}, {}) for {}ms...", x, y, duration_ms);
        match adapter.long_press(x, y, duration_ms).await {
            Ok(()) => println!("Long press successful!"),
            Err(e) => {
                eprintln!("Long press failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    // Handle app launch
    if let Some(ref package) = cli.adb.adb_launch {
        println!("Launching {}...", package);
        match adapter.launch_app(package).await {
            Ok(()) => println!("App launched!"),
            Err(e) => {
                eprintln!("Launch failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    // Handle app stop
    if let Some(ref package) = cli.adb.adb_stop {
        println!("Stopping {}...", package);
        match adapter.stop_app(package).await {
            Ok(()) => println!("App stopped!"),
            Err(e) => {
                eprintln!("Stop failed: {}", e);
                std::process::exit(1);
            }
        }
        return true;
    }

    false
}
