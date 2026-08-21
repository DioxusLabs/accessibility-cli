//! End-to-end tests for accessibility-core macOS accessibility APIs.
//!
//! These tests demonstrate the Playwright-like API for accessibility automation.
//!
//! They require:
//! - macOS platform
//! - Accessibility permissions granted to the terminal
//! - Calculator app available (built-in on macOS)
//!
//! Run with:
//! ```sh
//! cargo test calculator_e2e -- --nocapture
//! ```

#![cfg(target_os = "macos")]

use accessibility_core::accessibility::{
    AccessibilityEvent, AccessibilityEventType, AccessibilityReader, Element, ListenerConfig,
    Target, TreeFilter,
};
use accessibility_core::api::{App, Platform};
use accessibility_core::input::MouseButton;
use accessibility_core::platform::macos::MacOSAccessibility;
use accesskit::{Action, Role};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Drop guard that ensures Calculator is closed when the test exits.
///
/// This handles cleanup on both normal completion and panic.
struct CalculatorGuard {
    pid: u32,
    app: App,
    foreground: ForegroundSnapshot,
    close_on_drop: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ForegroundSnapshot {
    name: String,
    pid: u32,
}

impl ForegroundSnapshot {
    fn capture() -> Self {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut last_error;

        loop {
            match Self::try_capture() {
                Ok(snapshot) => return snapshot,
                Err(error) => last_error = error,
            }

            assert!(
                Instant::now() < deadline,
                "Failed to query frontmost process: {last_error}"
            );

            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn try_capture() -> Result<Self, String> {
        let script = r#"
            tell application "System Events"
                set frontmostProcess to first application process whose frontmost is true
                {name of frontmostProcess, unix id of frontmostProcess}
            end tell
        "#;
        let output = Command::new("osascript")
            .args(["-e", script])
            .output()
            .map_err(|e| format!("failed to run osascript: {e}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut parts = stdout.trim().split(", ");
        let Some(name) = parts.next().filter(|name| !name.is_empty()) else {
            return Err("frontmost process name missing".to_string());
        };
        let Some(pid) = parts.next().and_then(|pid| pid.parse().ok()) else {
            return Err("frontmost process PID missing".to_string());
        };

        Ok(Self {
            name: name.to_string(),
            pid,
        })
    }

    fn is_calculator(&self, calculator_pid: u32) -> bool {
        self.pid == calculator_pid || self.name == "Calculator"
    }

    fn assert_calculator_not_promoted(&self, calculator_pid: u32) {
        if self.is_calculator(calculator_pid) {
            return;
        }

        let current = Self::capture();
        assert!(
            !current.is_calculator(calculator_pid),
            "test promoted Calculator to the frontmost app from {:?}",
            self
        );
    }
}

impl CalculatorGuard {
    /// Launch Calculator and connect to it, waiting for it to be ready.
    async fn launch() -> Self {
        let (pid, close_on_drop) = Self::launch_calculator();

        let app = App::connect(pid, Platform::MacOS)
            .await
            .expect("Failed to connect to Calculator");

        // Wait for Calculator to be ready. This must happen before capturing the
        // foreground snapshot: `open -g` returns before the app finishes launching,
        // and a freshly-launched Calculator can grab focus during startup. Polling
        // through initial AX tree errors gives AppKit time to expose the full tree.
        Self::wait_for_calculator_tree(&app).await;

        let foreground = ForegroundSnapshot::capture();

        Self {
            pid,
            app,
            foreground,
            close_on_drop,
        }
    }

    /// Launch Calculator and connect with input capability without foregrounding it.
    async fn launch_for_input() -> Self {
        let guard = Self::launch().await;
        guard.clear_display().await;
        guard.assert_foreground_unchanged();
        guard
    }

    fn assert_foreground_unchanged(&self) {
        self.foreground.assert_calculator_not_promoted(self.pid);
    }

    async fn wait_for_calculator_tree(app: &App) {
        let deadline = Instant::now() + Duration::from_secs(5);

        loop {
            if app.locator("Button").no_wait().count().await > 0 {
                return;
            }

            assert!(
                Instant::now() < deadline,
                "Calculator should expose a ready accessibility tree"
            );

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Launch Calculator app and return its PID.
    fn launch_calculator() -> (u32, bool) {
        let was_running = Self::calculator_pid().is_some();

        let status = Command::new("open")
            .args(["-g", "-a", "Calculator"])
            .status()
            .expect("Failed to launch Calculator");
        assert!(status.success(), "open -g -a Calculator failed");

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(pid) = Self::calculator_pid() {
                return (pid, !was_running);
            }
            assert!(
                Instant::now() < deadline,
                "Timed out waiting for Calculator to launch"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn calculator_pid() -> Option<u32> {
        let script = r#"
            try
                tell application "System Events"
                    unix id of first process whose name is "Calculator"
                end tell
            on error
                return ""
            end try
        "#;

        let output = Command::new("osascript")
            .args(["-e", script])
            .output()
            .expect("Failed to query Calculator PID");

        let pid_str = String::from_utf8_lossy(&output.stdout);
        pid_str.trim().parse().ok()
    }

    async fn clear_display(&self) {
        for _ in 0..2 {
            self.clear_cache().await;
            if self
                .locator("Button[description='All Clear']")
                .no_wait()
                .exists()
                .await
            {
                self.locator("Button[description='All Clear']")
                    .first()
                    .click()
                    .await
                    .expect("Failed to click All Clear");
            } else if self
                .locator("Button[description='Clear']")
                .no_wait()
                .exists()
                .await
            {
                self.locator("Button[description='Clear']")
                    .first()
                    .click()
                    .await
                    .expect("Failed to click Clear");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Close Calculator app.
    fn close_calculator() {
        // Use AppleScript quit with delay built in
        let script = r#"
            try
                tell application "Calculator" to quit
            end try
            delay 0.3
        "#;
        let _ = Command::new("osascript").args(["-e", script]).status();
    }
}

impl Drop for CalculatorGuard {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.assert_foreground_unchanged();
        }
        if self.close_on_drop {
            Self::close_calculator();
        }
    }
}

impl std::ops::Deref for CalculatorGuard {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.app
    }
}

/// Wait for any TextRun element to contain the expected value.
///
/// This uses the new Locator wait API. The Calculator display value appears
/// in TextRun elements within the accessibility tree.
async fn wait_for_display_value(app: &App, expected: &str) -> Result<String, String> {
    // Use the new wait_for_value API on TextRun elements.
    // Since the Calculator has multiple TextRun elements (expression + result),
    // we use wait_for_value which polls until a matching element is found.
    let elem = app
        .locator("TextRun")
        .first()
        .with_timeout(Duration::from_secs(5))
        .wait_for_value(expected)
        .await
        .map_err(|e| e.to_string())?;

    elem.value.ok_or_else(|| "Element has no value".to_string())
}

async fn click_calculator_buttons_fast(calc: &CalculatorGuard, sequence: &[&str]) {
    let mut accessibility = MacOSAccessibility::new().expect("Failed to create MacOSAccessibility");
    let tree = accessibility
        .get_tree(&Target::Pid(calc.pid), &TreeFilter::default())
        .await
        .expect("Failed to get Calculator accessibility tree");

    for desc in sequence {
        let button_id = tree
            .find_all(|element| is_calculator_button(element, desc))
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("Calculator button '{desc}' not found"))
            .id;

        accessibility
            .perform_action(button_id, Action::Click)
            .await
            .unwrap_or_else(|_| panic!("Failed to click {desc}"));
    }

    calc.assert_foreground_unchanged();
}

fn is_calculator_button(element: &Element, description: &str) -> bool {
    element.role == Role::Button && element.description.as_deref() == Some(description)
}

/// Test that we can read the accessibility tree from Calculator using the App API.
#[tokio::test]
#[serial_test::file_serial(calculator)]
async fn test_calculator_accessibility_tree() {
    let calc = CalculatorGuard::launch().await;

    let tree = calc.tree().await.expect("Failed to get accessibility tree");

    assert!(tree.pid == Some(calc.pid), "PID should match");
    assert!(tree.element_count > 0, "Tree should have elements");

    let button_count = calc.locator("Button").no_wait().count().await;
    assert!(
        button_count >= 10,
        "Calculator should have at least 10 buttons (digits), found {}",
        button_count
    );

    assert!(
        calc.locator("Button[description='5']")
            .no_wait()
            .exists()
            .await,
        "Should find button '5'"
    );

    println!("Successfully read Calculator accessibility tree:");
    println!("  - App name: {:?}", tree.app_name);
    println!("  - Element count: {}", tree.element_count);
    println!("  - Button count: {}", button_count);
}

/// Test performing accessibility actions to do arithmetic using the Playwright-like API.
#[tokio::test]
#[serial_test::file_serial(calculator)]
async fn test_calculator_perform_action() {
    let calc = CalculatorGuard::launch_for_input().await;

    // Click button "5" using locator (auto-waits for element)
    calc.locator("Button[description='5']")
        .click()
        .await
        .expect("Failed to click 5");

    // Click button "+" (auto-waits for element)
    calc.locator("Button[description='Add']")
        .first()
        .click()
        .await
        .expect("Failed to click +");

    // Click button "3" (auto-waits for element)
    calc.locator("Button[description='3']")
        .click()
        .await
        .expect("Failed to click 3");

    // Compute via the AX click path in this action-oriented test.
    calc.locator("Button[description='Equals']")
        .click()
        .await
        .expect("Failed to click Equals");

    // Wait for the result to appear in display using poll_until
    let value = wait_for_display_value(&calc, "8")
        .await
        .expect("Display should contain '8'");

    println!("Calculator display: {}", value);
    assert!(
        value.contains('8'),
        "Expected display to contain '8', got '{}'",
        value
    );
}

/// Test using button clicks with the App API.
#[tokio::test]
#[serial_test::file_serial(calculator)]
async fn test_calculator_input_controller() {
    let calc = CalculatorGuard::launch_for_input().await;

    // Resolve Calculator's buttons once, then click the cached AX handles.
    click_calculator_buttons_fast(&calc, &["7", "Multiply", "6", "Equals"]).await;

    // Wait for the result to appear
    let value = wait_for_display_value(&calc, "42")
        .await
        .expect("Display should contain '42'");

    println!("Calculator display after 7*6: {}", value);
    assert!(
        value.contains("42"),
        "Expected display to contain '42', got '{}'",
        value
    );
}

/// Test computing via fast button clicks.
#[tokio::test]
#[serial_test::file_serial(calculator)]
async fn test_calculator_type_text() {
    let calc = CalculatorGuard::launch_for_input().await;

    click_calculator_buttons_fast(&calc, &["1", "2", "Add", "8", "Equals"]).await;

    // Wait for result
    let value = wait_for_display_value(&calc, "20")
        .await
        .expect("Display should contain '20'");

    println!("Calculator display after 12+8: {}", value);
    assert!(
        value.contains("20"),
        "Expected display to contain '20', got '{}'",
        value
    );
}

/// Test screenshot capture functionality using App API.
#[tokio::test]
#[serial_test::file_serial(calculator)]
async fn test_calculator_screenshot() {
    let calc = CalculatorGuard::launch().await;

    // Wait for Calculator to be fully loaded by checking for a button
    calc.locator("Button[description='5']")
        .wait()
        .await
        .expect("Calculator should have button 5");

    // Capture the Calculator window using App API
    let screenshot = calc
        .screenshot()
        .await
        .expect("Failed to capture screenshot");

    // Verify screenshot has reasonable dimensions
    assert!(screenshot.width > 0, "Screenshot width should be > 0");
    assert!(screenshot.height > 0, "Screenshot height should be > 0");
    assert!(
        screenshot.width < 5000,
        "Screenshot width seems too large: {}",
        screenshot.width
    );
    assert!(
        screenshot.height < 5000,
        "Screenshot height seems too large: {}",
        screenshot.height
    );

    // Verify it's valid PNG data
    assert!(
        screenshot.data.len() > 8,
        "Screenshot data too small to be valid PNG"
    );
    let png_signature = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    assert_eq!(
        &screenshot.data[..8],
        &png_signature,
        "Screenshot data should start with PNG signature"
    );

    // Test annotated screenshot
    let annotated = calc
        .annotated_screenshot(Some("Button"), true)
        .await
        .expect("Failed to create annotated screenshot");
    let (w, h) = annotated.dimensions();
    println!(
        "Annotated screenshot: {}x{} with {} labels",
        w,
        h,
        annotated.labels().len()
    );

    println!("Successfully captured Calculator screenshot:");
    println!("  - Dimensions: {}x{}", screenshot.width, screenshot.height);
    println!("  - Data size: {} bytes", screenshot.data.len());
}

/// Test capturing the entire screen.
#[tokio::test]
async fn test_screen_screenshot() {
    let accessibility = MacOSAccessibility::new().expect("Failed to create accessibility reader");

    let screenshot = accessibility
        .capture_screen(&Target::System)
        .await
        .expect("Failed to capture screen");

    // Screen should have reasonable dimensions (at least 800x600)
    assert!(
        screenshot.width >= 800,
        "Screen width should be >= 800, got {}",
        screenshot.width
    );
    assert!(
        screenshot.height >= 600,
        "Screen height should be >= 600, got {}",
        screenshot.height
    );

    // Verify PNG signature
    let png_signature = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    assert_eq!(
        &screenshot.data[..8],
        &png_signature,
        "Screenshot should be valid PNG"
    );

    println!("Successfully captured screen screenshot:");
    println!("  - Dimensions: {}x{}", screenshot.width, screenshot.height);
    println!("  - Data size: {} bytes", screenshot.data.len());
}

/// Test mouse click operations at specific coordinates.
#[tokio::test]
#[serial_test::file_serial(calculator)]
async fn test_calculator_mouse_click() {
    let calc = CalculatorGuard::launch().await;

    // Wait for and get the button using locator
    let btn_9 = calc
        .locator("Button[description='9']")
        .wait()
        .await
        .expect("Button '9' not found");

    if let Some(bounds) = btn_9.bounds {
        let center = bounds.center();
        println!(
            "Button '9' bounds: x={}, y={}, w={}, h={}, center=({}, {})",
            bounds.origin.x,
            bounds.origin.y,
            bounds.size.width,
            bounds.size.height,
            center.x,
            center.y
        );

        calc.mouse_click_at(center.x, center.y, MouseButton::Left)
            .await
            .expect("Failed to click");

        calc.assert_foreground_unchanged();
    } else {
        panic!("Button '9' has no bounds");
    }
}

/// Mouse scroll against a backgrounded app must not steal focus.
///
/// Regression guard for the SkyLight routing fix in `post_event`: scroll
/// events used to bypass `SLEventPostToPid` and go through the public
/// `CGEvent::post_to_pid`, which activates the target.
#[tokio::test]
#[serial_test::file_serial(calculator)]
async fn test_calculator_mouse_scroll_keeps_focus() {
    let calc = CalculatorGuard::launch().await;

    let mut accessibility = MacOSAccessibility::new().expect("Failed to create MacOSAccessibility");

    // A handful of scrolls so a missed SkyLight delivery would have a clear
    // chance to promote Calculator to key.
    for _ in 0..3 {
        accessibility
            .mouse_scroll(&Target::Pid(calc.pid), 0.0, -3.0)
            .await
            .expect("mouse_scroll failed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    calc.assert_foreground_unchanged();
}

/// Test finding elements by various properties using locators.
#[tokio::test]
#[serial_test::file_serial(calculator)]
async fn test_calculator_find_elements() {
    let calc = CalculatorGuard::launch().await;

    // Test finding by role using locator
    let button_count = calc.locator("Button").no_wait().count().await;
    println!("Found {} buttons", button_count);
    assert!(button_count > 0, "Should find buttons");

    // Get all buttons
    let buttons = calc
        .locator("Button")
        .no_wait()
        .all()
        .await
        .expect("Failed to get all buttons");
    println!("Got {} button handles", buttons.len());

    // Print all buttons for debugging
    for btn in &buttons {
        println!(
            "  Button: title={:?}, desc={:?}, id={:?}",
            btn.title, btn.description, btn.id
        );
    }

    // Test finding window
    let window_count = calc.locator("Window").no_wait().count().await;
    println!("Found {} windows", window_count);

    // Test exists for specific elements
    assert!(
        calc.locator("Button[description='5']")
            .no_wait()
            .exists()
            .await,
        "Should find button '5'"
    );

    // Test that elements have proper bounds
    let buttons_with_bounds = buttons.iter().filter(|b| b.bounds.is_some()).count();
    println!(
        "Found {} buttons with bounds (out of {} total)",
        buttons_with_bounds,
        buttons.len()
    );
}

/// Test locator options and filtering.
#[tokio::test]
#[serial_test::file_serial(calculator)]
async fn test_calculator_locator_options() {
    let calc = CalculatorGuard::launch().await;

    // Test count with no_wait
    let all_buttons = calc.locator("Button").no_wait().count().await;
    println!("Total buttons: {}", all_buttons);

    // Test first() to get first match
    let first_button = calc
        .locator("Button")
        .no_wait()
        .first()
        .get()
        .await
        .expect("Failed to get first button");
    assert!(first_button.is_some(), "Should get at least one button");
    if let Some(btn) = first_button {
        println!("First button: {:?} '{}'", btn.role, btn.display_label());
    }

    // Test exists() for nonexistent element
    assert!(
        !calc
            .locator("Button[description='nonexistent']")
            .no_wait()
            .exists()
            .await,
        "Nonexistent button should not exist"
    );

    // Test all() returns correct count
    let all = calc
        .locator("Button")
        .no_wait()
        .all()
        .await
        .expect("Failed to get all buttons");
    assert_eq!(all.len(), all_buttons, "all() count should match count()");
}

/// Test event listening - verifies that accessibility events are received when Calculator changes.
#[tokio::test]
#[serial_test::file_serial(calculator)]
async fn test_calculator_event_listening() {
    let calc = CalculatorGuard::launch_for_input().await;

    let mut accessibility = MacOSAccessibility::new().expect("Failed to create MacOSAccessibility");

    // Verify event listening is supported
    assert!(
        accessibility.supports_event_listening(),
        "macOS should support event listening"
    );

    let supported = accessibility.supported_event_types();
    println!("Supported event types: {:?}", supported);
    assert!(
        supported.contains(&AccessibilityEventType::FocusChanged),
        "Should support FocusChanged events"
    );
    assert!(
        supported.contains(&AccessibilityEventType::ValueChanged),
        "Should support ValueChanged events"
    );

    // Start listening for all events
    let config = ListenerConfig::new()
        .with_pid(calc.pid)
        .with_buffer_size(64);

    let (tx, mut rx) = mpsc::channel::<AccessibilityEvent>(64);
    let tx = Arc::new(Mutex::new(tx));

    let handle = accessibility
        .start_listening(
            config,
            Box::new({
                let tx = tx.clone();
                move |event| {
                    if let Ok(tx) = tx.lock() {
                        let _ = tx.blocking_send(event);
                    }
                }
            }),
        )
        .expect("Failed to start event listening");

    println!("Event listener started, performing actions...");

    // Perform actions using the Playwright-like API (locators auto-wait)
    println!("Clicking button 5...");
    calc.locator("Button[description='5']")
        .click()
        .await
        .expect("Failed to click 5");

    println!("Clicking button +...");
    calc.locator("Button[description='Add']")
        .first()
        .click()
        .await
        .expect("Failed to click +");

    println!("Clicking button 3...");
    calc.locator("Button[description='3']")
        .click()
        .await
        .expect("Failed to click 3");

    println!("Clicking Equals...");
    calc.locator("Button[description='Equals']")
        .click()
        .await
        .expect("Failed to click Equals");

    // Wait for display to show result before collecting events
    let _ = wait_for_display_value(&calc, "8").await;

    // Collect events with a timeout
    let mut events = Vec::new();
    let timeout = tokio::time::sleep(Duration::from_secs(2));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            _ = &mut timeout => {
                println!("Event collection timeout reached");
                break;
            }
            event = rx.recv() => {
                match event {
                    Some(AccessibilityEvent::Stopped { .. }) => {
                        println!("Received Stopped event");
                        break;
                    }
                    Some(event) => {
                        println!("Received event: {:?}", event);
                        events.push(event);
                    }
                    None => {
                        println!("Channel closed");
                        break;
                    }
                }
            }
        }
    }

    handle.stop().await;

    // Analyze collected events
    println!("\n=== Event Summary ===");
    println!("Total events received: {}", events.len());

    let focus_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AccessibilityEvent::FocusChanged { .. }))
        .collect();
    println!("FocusChanged events: {}", focus_events.len());

    let value_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AccessibilityEvent::ValueChanged { .. }))
        .collect();
    println!("ValueChanged events: {}", value_events.len());

    // Verify we received events
    assert!(
        !events.is_empty(),
        "Should receive accessibility events when clicking Calculator buttons"
    );

    // We should receive at least some ValueChanged events from the calculator display
    assert!(
        !value_events.is_empty(),
        "Should receive ValueChanged events when calculator display updates"
    );

    // Check that we eventually see "8" (the result of 5+3) in the values
    let values: Vec<_> = value_events
        .iter()
        .filter_map(|e| {
            if let AccessibilityEvent::ValueChanged { new_value, .. } = e {
                new_value.clone()
            } else {
                None
            }
        })
        .collect();

    println!("\nCalculation values received: {:?}", values);

    let has_result = values.iter().any(|v| v.contains('8'));
    assert!(
        has_result,
        "Should receive the calculation result '8' in ValueChanged events"
    );

    println!("\nEvent listening test completed successfully!");
}

/// Test event listening with specific event type filtering.
#[tokio::test]
#[serial_test::file_serial(calculator)]
async fn test_calculator_event_filtering() {
    let calc = CalculatorGuard::launch_for_input().await;

    // Wait for Calculator to be ready
    calc.locator("Button[description='5']")
        .wait()
        .await
        .expect("Calculator should be ready");

    let mut accessibility = MacOSAccessibility::new().expect("Failed to create MacOSAccessibility");

    // Only listen for value changes.
    let config = ListenerConfig::new()
        .with_pid(calc.pid)
        .with_event_types(vec![AccessibilityEventType::ValueChanged])
        .with_buffer_size(32);

    let (tx, mut rx) = mpsc::channel::<AccessibilityEvent>(32);
    let tx = Arc::new(Mutex::new(tx));

    let handle = accessibility
        .start_listening(
            config,
            Box::new({
                let tx = tx.clone();
                move |event| {
                    if let Ok(tx) = tx.lock() {
                        let _ = tx.blocking_send(event);
                    }
                }
            }),
        )
        .expect("Failed to start filtered event listening");

    println!("Filtered listener started (ValueChanged only)...");

    calc.locator("Button[description='5']")
        .click()
        .await
        .expect("Failed to click 5");

    // Collect events (timeout handles waiting for events to arrive)
    let mut events = Vec::new();
    let timeout = tokio::time::sleep(Duration::from_secs(1));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            _ = &mut timeout => break,
            event = rx.recv() => {
                match event {
                    Some(AccessibilityEvent::Stopped { .. }) => break,
                    Some(e) => {
                        match &e {
                            AccessibilityEvent::ValueChanged { .. } => {
                                println!("Received expected ValueChanged event");
                                events.push(e);
                            }
                            AccessibilityEvent::Error { message, .. } => {
                                println!("Received error event: {}", message);
                                events.push(e);
                            }
                            other => {
                                println!("Unexpected event type received: {:?}", other);
                                events.push(e);
                            }
                        }
                    }
                    None => break,
                }
            }
        }
    }

    handle.stop().await;

    println!("Filtered test received {} events", events.len());

    calc.assert_foreground_unchanged();

    // All non-error events should be ValueChanged
    for event in &events {
        match event {
            AccessibilityEvent::ValueChanged { .. } => {}
            AccessibilityEvent::Error { .. } => {}
            other => panic!("Received unexpected event type with filter: {:?}", other),
        }
    }

    println!("Event filtering test completed successfully!");
}
