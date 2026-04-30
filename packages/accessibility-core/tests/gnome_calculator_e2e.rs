//! End-to-end tests for accessibility-core Linux accessibility APIs using GNOME Calculator.
//!
//! These tests demonstrate the Playwright-like API for accessibility automation.
//!
//! They require:
//! - Linux platform with AT-SPI2 enabled
//! - GNOME Calculator application installed (default on Ubuntu/GNOME systems)
//! - Accessibility services running (orca or at-spi2-registryd)
//!
//! Run with:
//! ```sh
//! cargo test gnome_calculator_e2e -- --nocapture
//! ```

#![cfg(target_os = "linux")]

use accessibility_core::accessibility::{
    AccessibilityEvent, AccessibilityEventType, AccessibilityReader, ListenerConfig,
};
use accessibility_core::api::{App, Platform};
use accessibility_core::platform::x11::LinuxAccessibility;
use serial_test::serial;
use std::fs;
use std::process::Command;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use tokio::sync::mpsc;

// ============================================================================
// D-Bus Session Bus Auto-Detection
// ============================================================================

static INIT_DBUS: Once = Once::new();

/// Ensures DBUS_SESSION_BUS_ADDRESS is set, auto-detecting it if necessary.
///
/// In WSL and some headless environments, the D-Bus session bus address may not
/// be set in the environment even though AT-SPI2 is running. This function finds
/// the address by reading the environment of the running at-spi2-registryd process.
fn ensure_dbus_session_bus() {
    INIT_DBUS.call_once(|| {
        // If already set, nothing to do
        if std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok() {
            return;
        }

        // Try to find the D-Bus session bus address from at-spi2-registryd
        if let Some(addr) = find_dbus_from_atspi_registryd() {
            eprintln!(
                "[test setup] Auto-detected DBUS_SESSION_BUS_ADDRESS: {}",
                addr
            );
            // SAFETY: We're in single-threaded test initialization (Once guard),
            // and no other code is reading environment variables concurrently.
            unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", addr) };
            return;
        }

        // Fallback: try dbus-launch
        if let Some(addr) = try_dbus_launch() {
            eprintln!("[test setup] Started new D-Bus session: {}", addr);
            // SAFETY: Same as above - single-threaded initialization.
            unsafe { std::env::set_var("DBUS_SESSION_BUS_ADDRESS", addr) };
            return;
        }

        eprintln!("[test setup] WARNING: Could not determine DBUS_SESSION_BUS_ADDRESS");
    });
}

/// Find DBUS_SESSION_BUS_ADDRESS from the at-spi2-registryd process environment.
fn find_dbus_from_atspi_registryd() -> Option<String> {
    // Find PID of at-spi2-registryd
    let output = Command::new("pgrep")
        .args(["-f", "at-spi2-registryd"])
        .output()
        .ok()?;

    let pids: Vec<&str> = std::str::from_utf8(&output.stdout).ok()?.lines().collect();

    // Try each PID (there may be multiple registryd processes)
    for pid in pids {
        let pid = pid.trim();
        if pid.is_empty() {
            continue;
        }

        // Read /proc/<pid>/environ
        let environ_path = format!("/proc/{}/environ", pid);
        if let Ok(environ) = fs::read(&environ_path) {
            // Parse null-separated environment variables
            for var in environ.split(|&b| b == 0) {
                if let Ok(s) = std::str::from_utf8(var) {
                    if let Some(addr) = s.strip_prefix("DBUS_SESSION_BUS_ADDRESS=") {
                        if !addr.is_empty() {
                            return Some(addr.to_string());
                        }
                    }
                }
            }
        }
    }

    None
}

/// Try to start a new D-Bus session using dbus-launch.
fn try_dbus_launch() -> Option<String> {
    let output = Command::new("dbus-launch")
        .args(["--sh-syntax"])
        .output()
        .ok()?;

    let stdout = std::str::from_utf8(&output.stdout).ok()?;

    // Parse output like: DBUS_SESSION_BUS_ADDRESS='unix:abstract=/tmp/dbus-xxx,guid=yyy';
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("DBUS_SESSION_BUS_ADDRESS=") {
            let addr = rest.trim_matches(|c| c == '\'' || c == '"' || c == ';');
            if !addr.is_empty() {
                return Some(addr.to_string());
            }
        }
    }

    None
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Drop guard that ensures Calculator is closed when the test exits.
///
/// This handles cleanup on both normal completion and panic.
struct CalculatorGuard {
    pid: u32,
    app: App,
}

impl CalculatorGuard {
    /// Launch Calculator and connect to it, waiting for it to be ready.
    async fn launch() -> Self {
        let pid = Self::launch_calculator();

        let app = App::connect(pid, Platform::Linux)
            .await
            .expect("Failed to connect to Calculator");

        // Wait for Calculator to be ready
        app.locator("Button")
            .first()
            .wait()
            .await
            .expect("Calculator should be ready");

        Self { pid, app }
    }

    /// Launch Calculator and connect with input capability (activates window).
    async fn launch_for_input() -> Self {
        let pid = Self::launch_calculator();
        Self::activate_app(pid);

        let app = App::connect(pid, Platform::Linux)
            .await
            .expect("Failed to connect to Calculator");

        // Wait for Calculator to be ready
        app.locator("Button")
            .first()
            .wait()
            .await
            .expect("Calculator should be ready");

        Self { pid, app }
    }

    /// Launch GNOME Calculator app and return its PID.
    fn launch_calculator() -> u32 {
        // Auto-detect D-Bus session bus if not set
        ensure_dbus_session_bus();

        // Get environment variables - these must be passed to child process for AT-SPI to work
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".to_string());
        let dbus = std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default();

        // Kill any existing calculator
        let _ = Command::new("pkill").args(["-9", "gnome-calc"]).status();
        std::thread::sleep(Duration::from_millis(500));

        // Launch calculator - MUST pass DBUS_SESSION_BUS_ADDRESS for AT-SPI to work correctly
        let child = Command::new("gnome-calculator")
            .env("DISPLAY", &display)
            .env("GTK_MODULES", "gail:atk-bridge")
            .env("DBUS_SESSION_BUS_ADDRESS", &dbus)
            .spawn()
            .expect("Failed to launch gnome-calculator");

        let pid = child.id();

        // Wait for it to initialize
        std::thread::sleep(Duration::from_millis(2500));

        pid
    }

    /// Activate (bring to front) an application by PID.
    fn activate_app(pid: u32) {
        let script = format!(
            r#"
            wmctrl -i -a 0x{:x} 2>/dev/null || xdotool search --pid {} windowactivate 2>/dev/null || true
            sleep 0.3
            "#,
            pid, pid
        );
        let _ = Command::new("bash").args(["-c", &script]).status();
    }

    /// Close GNOME Calculator app.
    fn close_calculator() {
        let script = r#"
            pkill -9 gnome-calc 2>/dev/null || true
            sleep 0.5
        "#;
        let _ = Command::new("bash").args(["-c", script]).status();
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

/// Click a button, trying multiple possible titles.
/// Returns an error only if ALL titles fail.
async fn click_button_with_fallback(calc: &App, titles: &[&str]) -> Result<(), String> {
    let mut last_error = String::new();
    for title in titles {
        match calc
            .locator(&format!("Button[title='{}']", title))
            .first()
            .click()
            .await
        {
            Ok(()) => return Ok(()),
            Err(e) => last_error = format!("Button '{}': {}", title, e),
        }
    }
    Err(format!(
        "Failed to click button with any of {:?}. Last error: {}",
        titles, last_error
    ))
}

/// Wait for any text element to contain the expected value.
///
/// This uses the new Locator wait API. GNOME Calculator display values appear
/// in Label or TextInput elements within the accessibility tree.
async fn wait_for_display_value(app: &App, expected: &str) -> Result<String, String> {
    // Try Label elements first (common for GNOME Calculator display)
    if let Ok(elem) = app
        .locator("Label")
        .first()
        .with_timeout(Duration::from_secs(5))
        .wait_for_value(expected)
        .await
    {
        return elem.value.ok_or_else(|| "Element has no value".to_string());
    }

    // Fallback to TextInput elements
    let elem = app
        .locator("TextInput")
        .first()
        .with_timeout(Duration::from_secs(5))
        .wait_for_value(expected)
        .await
        .map_err(|e| e.to_string())?;

    elem.value.ok_or_else(|| "Element has no value".to_string())
}

// ============================================================================
// Tests
// ============================================================================

/// Test that we can read the accessibility tree from GNOME Calculator using the App API.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn test_calculator_accessibility_tree() {
    let calc = CalculatorGuard::launch().await;

    let tree = calc.tree().await.expect("Failed to get accessibility tree");

    assert_eq!(tree.pid, Some(calc.pid), "PID should match");
    assert!(tree.element_count > 0, "Tree should have elements");

    let button_count = calc.locator("Button").no_wait().count().await;
    assert!(
        button_count > 0,
        "Calculator should have buttons, found {}",
        button_count
    );

    // Check that we can find at least some digit buttons
    let mut found_digits = 0;
    for digit in ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"] {
        if calc
            .locator(&format!("Button[title='{}']", digit))
            .no_wait()
            .exists()
            .await
            || calc
                .locator(&format!("Button[description='{}']", digit))
                .no_wait()
                .exists()
                .await
        {
            found_digits += 1;
        }
    }
    assert!(
        found_digits >= 5,
        "Should find at least 5 digit buttons, found {}",
        found_digits
    );

    println!("Successfully read GNOME Calculator accessibility tree:");
    println!("  - App name: {:?}", tree.app_name);
    println!("  - Element count: {}", tree.element_count);
    println!("  - Button count: {}", button_count);
    println!("  - Digit buttons found: {}", found_digits);
}

/// Test performing accessibility actions to do arithmetic using the Playwright-like API.
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn test_calculator_perform_action() {
    let calc = CalculatorGuard::launch_for_input().await;

    // Calculate 5 + 3 = 8 using locators
    // GNOME Calculator buttons have title matching the digit/operator
    click_button_with_fallback(&calc, &["5"])
        .await
        .expect("Failed to click 5");
    click_button_with_fallback(&calc, &["+", "Add"])
        .await
        .expect("Failed to click +");
    click_button_with_fallback(&calc, &["3"])
        .await
        .expect("Failed to click 3");
    click_button_with_fallback(&calc, &["=", "Equals"])
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

/// Test finding elements by various properties using locators.
#[tokio::test(flavor = "multi_thread")]
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
        println!(
            "  Button: title={:?}, desc={:?}, id={:?}",
            btn.title, btn.description, btn.identifier
        );
    }
    if buttons.len() > 10 {
        println!("  ... and {} more buttons", buttons.len() - 10);
    }

    let window_count = calc.locator("Window").no_wait().count().await;
    println!("Found {} windows", window_count);
}

/// Test locator options and filtering.
#[tokio::test(flavor = "multi_thread")]
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
#[tokio::test(flavor = "multi_thread")]
#[serial]
async fn test_calculator_event_listener() {
    let calc = CalculatorGuard::launch_for_input().await;

    let mut accessibility = LinuxAccessibility::new()
        .await
        .expect("Failed to create LinuxAccessibility");

    // Start listening for events
    let config = ListenerConfig::new()
        .with_pid(calc.pid)
        .with_event_types(vec![
            AccessibilityEventType::FocusChanged,
            AccessibilityEventType::ValueChanged,
        ]);

    let (tx, mut rx) = mpsc::channel::<AccessibilityEvent>(64);
    let tx = Arc::new(Mutex::new(tx));

    let handle = accessibility
        .start_listening(
            config,
            Box::new({
                let tx = tx.clone();
                move |event| {
                    if !matches!(event, AccessibilityEvent::Stopped { .. }) {
                        if let Ok(tx) = tx.lock() {
                            let _ = tx.try_send(event);
                        }
                    }
                }
            }),
        )
        .expect("Failed to start event listener");

    println!("Event listener started, performing actions...");

    // Perform calculator operations using locators
    click_button_with_fallback(&calc, &["5"])
        .await
        .expect("Failed to click 5");
    click_button_with_fallback(&calc, &["+", "Add"])
        .await
        .expect("Failed to click +");
    click_button_with_fallback(&calc, &["3"])
        .await
        .expect("Failed to click 3");
    click_button_with_fallback(&calc, &["=", "Equals"])
        .await
        .expect("Failed to click =");

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

    let focus_events = events
        .iter()
        .filter(|e| matches!(e, AccessibilityEvent::FocusChanged { .. }))
        .count();
    let value_events = events
        .iter()
        .filter(|e| matches!(e, AccessibilityEvent::ValueChanged { .. }))
        .count();

    println!("Focus events: {}", focus_events);
    println!("Value events: {}", value_events);

    assert!(
        !events.is_empty(),
        "Should have received at least some events during calculator operations"
    );

    println!("Event listening test completed successfully!");
}
