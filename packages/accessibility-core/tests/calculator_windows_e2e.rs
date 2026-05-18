//! End-to-end tests for accessibility-core Windows accessibility APIs.
//!
//! These tests demonstrate the Playwright-like API for accessibility automation.
//!
//! They require:
//! - Windows platform
//! - Windows Calculator app available (built-in on Windows)
//!
//! Run with:
//! ```sh
//! cargo test calculator_windows_e2e -- --nocapture
//! ```

#![cfg(target_os = "windows")]

use accessibility_core::accessibility::{
    AccessibilityEvent, AccessibilityReader, ListenerConfig, Target,
};
use accessibility_core::api::{App, Platform};
use accessibility_core::input::MouseButton;
use accessibility_core::platform::msft::{
    WindowBlockerSpec, WindowsAccessibility, hide_top_level_windows_matching,
    hide_windows_matching_at_point,
};
use serial_test::serial;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

// ============================================================================
// CI-only blocker dismissal
// ============================================================================
//
// The windows-11-arm runner image floats OOBE / "Microsoft account" sign-in
// windows above the desktop that intercept synthetic clicks at calculator's
// coordinates. They come from a few processes (Shell_OOBEProxy host,
// UserOOBEWindowClass with empty title, Windows.UI.Core.CoreWindow titled
// "Microsoft account") and uncover each other as we hide them. We hide them
// (rather than killing the host process) so the OS doesn't immediately respawn
// a fresh popup. Two passes — one over all top-level windows, one driven by
// what's actually under the click pixel — to handle the layered z-order.

/// Drop guard that ensures Calculator is closed when the test exits.
///
/// This handles cleanup on both normal completion and panic.
struct CalculatorGuard {
    pid: u32,
    app: App,
}

impl CalculatorGuard {
    /// Connect to Calculator and wait until UI Automation can find its window.
    ///
    /// `Stop-Process` + `Start-Process calculator:` returns as soon as the UWP
    /// host process exists, but the UI Automation root may not yet enumerate the
    /// hosted ApplicationFrameWindow. Retry both `connect` and `wait` until they
    /// succeed or the deadline passes.
    async fn connect_when_ready(pid: u32) -> App {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);

        loop {
            let last_error = match App::connect(pid, Platform::Windows).await {
                Ok(app) => match app
                    .locator("Button")
                    .first()
                    .with_timeout(Duration::from_secs(1))
                    .wait()
                    .await
                {
                    Ok(_) => return app,
                    Err(err) => err.to_string(),
                },
                Err(err) => err.to_string(),
            };

            assert!(
                std::time::Instant::now() < deadline,
                "Calculator should be ready: {}",
                last_error
            );

            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Launch Calculator and connect to it, waiting for it to be ready.
    async fn launch() -> Self {
        let pid = Self::launch_calculator();
        let app = Self::connect_when_ready(pid).await;
        Self { pid, app }
    }

    /// Launch Calculator and connect with input capability (activates window and clears display).
    async fn launch_for_input() -> Self {
        let pid = Self::launch_calculator();
        Self::activate_app();

        let app = Self::connect_when_ready(pid).await;

        // Clear calculator
        app.keystroke("escape")
            .await
            .expect("Failed to send escape");

        Self { pid, app }
    }

    /// Launch Windows Calculator app and return its PID.
    fn launch_calculator() -> u32 {
        // Poll for the process up to ~15s — `Start-Process calculator:` returns
        // immediately while the UWP host warms up, so a fixed sleep is flaky on CI.
        let script = r#"
            Stop-Process -Name CalculatorApp -Force -ErrorAction SilentlyContinue
            Start-Sleep -Seconds 1
            Start-Process calculator:
            for ($i = 0; $i -lt 30; $i++) {
                $proc = Get-Process -Name CalculatorApp -ErrorAction SilentlyContinue | Select-Object -First 1
                if ($proc) { $proc.Id; exit 0 }
                Start-Sleep -Milliseconds 500
            }
        "#;

        let output = Command::new("powershell")
            .args(["-Command", script])
            .output()
            .expect("Failed to run PowerShell");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let trimmed = stdout.trim();
        trimmed.parse().unwrap_or_else(|err| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!(
                "Failed to launch Calculator and read its PID ({err}). \
                 Is the Microsoft Store Calculator (CalculatorApp) installed?\n\
                 PowerShell stdout: {stdout:?}\n\
                 PowerShell stderr: {stderr:?}"
            )
        })
    }

    /// Activate (bring to front) the Calculator using PowerShell.
    fn activate_app() {
        let script = r#"
            (New-Object -ComObject WScript.Shell).AppActivate('Calculator')
            Start-Sleep -Milliseconds 300
        "#;
        let _ = Command::new("powershell")
            .args(["-Command", script])
            .status();
    }

    /// Close Calculator app.
    fn close_calculator() {
        let script = r#"
            Stop-Process -Name CalculatorApp -Force -ErrorAction SilentlyContinue
            Start-Sleep -Milliseconds 300
        "#;
        let _ = Command::new("powershell")
            .args(["-Command", script])
            .status();
    }
}

impl Drop for CalculatorGuard {
    fn drop(&mut self) {
        Self::close_calculator();
    }
}

impl std::ops::Deref for CalculatorGuard {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.app
    }
}

/// Wait for any text element to contain the expected value.
///
/// This uses the new Locator wait API. Windows Calculator display values appear
/// in Label elements within the accessibility tree.
async fn wait_for_display_value(app: &App, expected: &str) -> Result<String, String> {
    // Windows Calculator uses Label elements for display
    // The wait_for API checks all matching elements for the value
    let elem = app
        .locator("Label")
        .first()
        .with_timeout(Duration::from_secs(5))
        .wait_for(
            |e| {
                // Check both value and title fields for the expected content
                let in_value = e
                    .value
                    .as_ref()
                    .map(|v| v.contains(expected))
                    .unwrap_or(false);
                let in_title = e
                    .title
                    .as_ref()
                    .map(|t| t.contains(expected))
                    .unwrap_or(false);
                in_value || in_title
            },
            &format!("display contains '{}'", expected),
        )
        .await
        .map_err(|e| e.to_string())?;

    // Return the value or title, whichever contains the expected content
    elem.value
        .or(elem.title)
        .ok_or_else(|| "Element has no value or title".to_string())
}

/// Test that we can read the accessibility tree from Calculator using the App API.
#[tokio::test]
#[serial]
async fn test_calculator_accessibility_tree() {
    let calc = CalculatorGuard::launch().await;

    let tree = calc.tree().await.expect("Failed to get accessibility tree");

    assert!(tree.pid == Some(calc.pid), "PID should match");
    assert!(tree.element_count > 0, "Tree should have elements");

    let button_count = calc.locator("Button").no_wait().count().await;
    assert!(
        button_count > 0,
        "Calculator should have buttons, found {}",
        button_count
    );

    println!("Successfully read Calculator accessibility tree:");
    println!("  - App name: {:?}", tree.app_name);
    println!("  - Element count: {}", tree.element_count);
    println!("  - Button count: {}", button_count);
}

/// Test performing accessibility actions to do arithmetic using the Playwright-like API.
#[tokio::test]
#[serial]
async fn test_calculator_perform_action() {
    let calc = CalculatorGuard::launch_for_input().await;

    // Calculate 5 + 3 = 8 using locators
    // Windows Calculator uses titles like "Five", "Plus", etc.
    calc.locator("Button[title='Five']")
        .first()
        .click()
        .await
        .expect("Failed to click 5");

    calc.locator("Button[title='Plus']")
        .first()
        .click()
        .await
        .expect("Failed to click +");

    calc.locator("Button[title='Three']")
        .first()
        .click()
        .await
        .expect("Failed to click 3");

    calc.locator("Button[title='Equals']")
        .first()
        .click()
        .await
        .expect("Failed to click =");

    // Wait for result
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

/// Test using keyboard input with the App API.
#[tokio::test]
#[serial]
async fn test_calculator_input_controller() {
    let calc = CalculatorGuard::launch_for_input().await;

    // Calculate 7 * 6 = 42 using mouse clicks on buttons
    calc.locator("Button[title='Seven']")
        .first()
        .click()
        .await
        .expect("Failed to click 7");

    calc.locator("Button[title='Multiply by']")
        .first()
        .click()
        .await
        .expect("Failed to click multiply");

    calc.locator("Button[title='Six']")
        .first()
        .click()
        .await
        .expect("Failed to click 6");

    calc.locator("Button[title='Equals']")
        .first()
        .click()
        .await
        .expect("Failed to click =");

    // Wait for result
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

/// Test using type_text for easier text input.
#[tokio::test]
#[serial]
async fn test_calculator_type_text() {
    let calc = CalculatorGuard::launch_for_input().await;

    // Type "12+8" and press Enter
    calc.type_text("12+8").await.expect("Failed to type text");
    calc.keystroke("enter")
        .await
        .expect("Failed to press enter");

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
#[serial]
async fn test_calculator_screenshot() {
    let calc = CalculatorGuard::launch().await;

    let screenshot = calc
        .screenshot()
        .await
        .expect("Failed to capture screenshot");

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

    let png_signature = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    assert_eq!(
        &screenshot.data[..8],
        &png_signature,
        "Screenshot data should start with PNG signature"
    );

    println!("Successfully captured Calculator screenshot:");
    println!("  - Dimensions: {}x{}", screenshot.width, screenshot.height);
    println!("  - Data size: {} bytes", screenshot.data.len());
}

/// Test capturing the entire screen.
#[tokio::test]
async fn test_screen_screenshot() {
    let app = App::system(Platform::Windows)
        .await
        .expect("Failed to connect to Windows system scope");

    let screenshot = app.screenshot().await.expect("Failed to capture screen");

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
#[serial]
async fn test_calculator_mouse_click() {
    let calc = CalculatorGuard::launch_for_input().await;

    // Wait for and get button "9" coordinates
    let btn_9 = calc
        .locator("Button[title='Nine']")
        .first()
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

        // Use low-level mouse click via AccessibilityReader
        let mut input = WindowsAccessibility::new().expect("Failed to create accessibility reader");

        // Bring Calculator to the foreground so the absolute-coord click lands on it.
        // `activate_app()` runs before connect, but subsequent UIA queries can shuffle
        // focus on Windows-on-ARM CI; force it back via SetForegroundWindow + UIA SetFocus.
        input
            .focus_window(calc.pid)
            .expect("Failed to focus Calculator");

        // The windows-11-arm runner image floats OOBE / "Microsoft account"
        // sign-in windows above the desktop that intercept synthetic clicks at
        // calculator's coordinates. They come in several flavours from a few
        // processes (Shell_OOBEProxy, UserOOBEWindowClass with empty title,
        // Windows.UI.Core.CoreWindow titled "Microsoft account") and uncover
        // each other as we hide them. Match by title OR class to catch the
        // empty-title OOBE frame, and combine an EnumWindows pass with a
        // point-driven pass that hides whatever's actually under the click
        // pixel. ShowWindow(SW_HIDE) keeps the host alive so the OS doesn't
        // respawn a fresh popup.
        let blockers = WindowBlockerSpec {
            titles: &["Microsoft account"],
            classes: &["Shell_OOBEProxy", "UserOOBEWindowClass"],
        };
        let pre_hidden = hide_top_level_windows_matching(&blockers);
        let point_hidden = hide_windows_matching_at_point(center.x, center.y, &blockers);
        if pre_hidden + point_hidden > 0 {
            println!(
                "Hid {} blocker popup(s) before click ({} via enum, {} at click point)",
                pre_hidden + point_hidden,
                pre_hidden,
                point_hidden
            );
        }

        input
            .mouse_click_at(
                &Target::Pid(calc.pid),
                center.x,
                center.y,
                MouseButton::Left,
            )
            .await
            .expect("Failed to click");

        // Wait for display to show "9"
        let value = wait_for_display_value(&calc, "9")
            .await
            .expect("Display should contain '9'");

        println!("Calculator display after clicking 9: {}", value);
        assert!(
            value.contains('9'),
            "Expected display to contain '9', got '{}'",
            value
        );
    } else {
        panic!("Button '9' has no bounds");
    }
}

/// Test finding elements by various properties using locators.
#[tokio::test]
#[serial]
async fn test_calculator_find_elements() {
    let calc = CalculatorGuard::launch().await;

    let button_count = calc.locator("Button").no_wait().count().await;
    println!("Found {} buttons", button_count);
    assert!(button_count > 0, "Should find buttons");

    let buttons = calc
        .locator("Button")
        .no_wait()
        .all()
        .await
        .expect("Failed to get all buttons");

    for btn in &buttons[..std::cmp::min(10, buttons.len())] {
        println!("  Button: title={:?}, id={:?}", btn.title, btn.identifier);
    }

    let window_count = calc.locator("Window").no_wait().count().await;
    println!("Found {} windows", window_count);
}

/// Test locator options and filtering.
#[tokio::test]
#[serial]
async fn test_calculator_locator_options() {
    let calc = CalculatorGuard::launch().await;

    let all_buttons = calc.locator("Button").no_wait().count().await;
    println!("Total buttons: {}", all_buttons);

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

    assert!(
        !calc
            .locator("Button[title='nonexistent']")
            .no_wait()
            .exists()
            .await,
        "Nonexistent button should not exist"
    );
}

/// Test event listening - verifies that accessibility events are received.
#[tokio::test]
#[serial]
async fn test_calculator_event_listening() {
    let calc = CalculatorGuard::launch_for_input().await;

    let mut accessibility =
        WindowsAccessibility::new().expect("Failed to create WindowsAccessibility");

    // Start listening for events
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

    // Perform actions using locators
    calc.locator("Button[title='Five']")
        .first()
        .click()
        .await
        .expect("Failed to click 5");

    calc.locator("Button[title='Plus']")
        .first()
        .click()
        .await
        .expect("Failed to click +");

    calc.locator("Button[title='Three']")
        .first()
        .click()
        .await
        .expect("Failed to click 3");

    calc.keystroke("enter")
        .await
        .expect("Failed to press enter");

    // Wait for display to show result
    let _ = wait_for_display_value(&calc, "8").await;

    // Collect events with timeout
    let mut events = Vec::new();
    let timeout = tokio::time::sleep(Duration::from_secs(2));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            _ = &mut timeout => break,
            event = rx.recv() => {
                match event {
                    Some(AccessibilityEvent::Stopped { .. }) => break,
                    Some(event) => {
                        println!("Received event: {:?}", event);
                        events.push(event);
                    }
                    None => break,
                }
            }
        }
    }

    handle.stop().await;

    println!("\n=== Event Summary ===");
    println!("Total events received: {}", events.len());

    assert!(
        !events.is_empty(),
        "Should receive accessibility events when clicking Calculator buttons"
    );

    println!("Event listening test completed successfully!");
}
