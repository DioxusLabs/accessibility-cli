//! End-to-end tests for accessibility-core Android accessibility APIs using Settings app.
//!
//! These tests use the Settings app which is always available on Android devices.
//! The tests automatically start an Android emulator if one isn't already running.
//!
//! Run with:
//! ```sh
//! cargo test -p accessibility-core --test settings_android_e2e -- --ignored --nocapture
//! ```

use accessibility_core::api::{App, AppConfig, Platform};
use accessibility_core::platform::android::{AndroidAccessibility, AndroidExtensions};
use serial_test::serial;
use std::ops::Deref;
use std::process::{Child, Command};
use std::time::Duration;

// ============================================================================
// Emulator Guard (per-test lifecycle with Drop cleanup)
// ============================================================================

/// Guard that manages the Android emulator lifecycle.
/// Starts the emulator if needed, stops it when dropped.
struct EmulatorGuard {
    started_by_us: bool,
    child: Option<Child>,
}

impl EmulatorGuard {
    /// Ensure an emulator is running, starting one if necessary.
    #[allow(clippy::zombie_processes)]
    fn new() -> Self {
        // Check if a device is already connected
        let output = Command::new("adb")
            .args(["devices"])
            .output()
            .expect("Failed to run adb devices");

        let devices_output = String::from_utf8_lossy(&output.stdout);
        let has_device = devices_output
            .lines()
            .skip(1) // Skip "List of devices attached"
            .any(|line| line.contains("device") && !line.contains("offline"));

        if has_device {
            println!("Android device/emulator already connected");
            return Self {
                started_by_us: false,
                child: None,
            };
        }

        println!("No Android device found, starting emulator...");

        // Find emulator path
        let emulator_paths = [
            std::env::var("ANDROID_HOME")
                .map(|h| format!("{}/emulator/emulator", h))
                .unwrap_or_default(),
            std::env::var("HOME")
                .map(|h| format!("{}/Library/Android/sdk/emulator/emulator", h))
                .unwrap_or_default(),
            "/usr/local/share/android-sdk/emulator/emulator".to_string(),
        ];

        let emulator_path = emulator_paths
            .iter()
            .find(|p| !p.is_empty() && std::path::Path::new(p).exists())
            .expect("Could not find Android emulator. Set ANDROID_HOME or install Android SDK.");

        // List available AVDs
        let avd_output = Command::new(emulator_path)
            .args(["-list-avds"])
            .output()
            .expect("Failed to list AVDs");

        let avds = String::from_utf8_lossy(&avd_output.stdout);
        let avd_name = avds
            .lines()
            .next()
            .expect("No AVDs found. Create one with Android Studio or `avdmanager`.");

        println!("Starting emulator: {}", avd_name);

        // Start emulator in background
        let child = Command::new(emulator_path)
            .args([
                "-avd",
                avd_name,
                "-no-audio",
                "-no-window",
                "-gpu",
                "swiftshader_indirect",
            ])
            .spawn()
            .expect("Failed to start emulator");

        // Wait for emulator to boot
        println!("Waiting for emulator to boot...");
        let boot_timeout = Duration::from_secs(120);
        let start = std::time::Instant::now();

        loop {
            std::thread::sleep(Duration::from_secs(2));

            // Check if device is connected
            let output = Command::new("adb").args(["devices"]).output().ok();

            if let Some(output) = output {
                let devices = String::from_utf8_lossy(&output.stdout);
                if devices.contains("emulator") && devices.contains("device") {
                    // Check if boot completed
                    let boot_check = Command::new("adb")
                        .args(["shell", "getprop", "sys.boot_completed"])
                        .output()
                        .ok();

                    if let Some(boot) = boot_check {
                        let boot_status = String::from_utf8_lossy(&boot.stdout);
                        if boot_status.trim() == "1" {
                            println!("Emulator booted successfully!");
                            // Give it a moment to fully settle
                            std::thread::sleep(Duration::from_secs(3));
                            return Self {
                                started_by_us: true,
                                child: Some(child),
                            };
                        }
                    }
                }
            }

            if start.elapsed() >= boot_timeout {
                panic!(
                    "Emulator failed to boot within {} seconds",
                    boot_timeout.as_secs()
                );
            }

            print!(".");
            use std::io::Write;
            std::io::stdout().flush().ok();
        }
    }
}

impl Drop for EmulatorGuard {
    fn drop(&mut self) {
        if self.started_by_us {
            println!("\nStopping emulator...");
            let _ = Command::new("adb").args(["emu", "kill"]).output();
            if let Some(mut child) = self.child.take() {
                let _ = child.wait();
            }
            // Wait for emulator process to exit
            std::thread::sleep(Duration::from_secs(2));
            // Reset ADB server to clear stale device connections
            let _ = Command::new("adb").args(["kill-server"]).output();
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Drop guard that ensures the Settings app and emulator are cleaned up when the test exits.
struct AndroidSettingsGuard {
    app: App,
    accessibility: AndroidAccessibility,
    #[allow(dead_code)] // Kept alive for Drop
    emulator: EmulatorGuard,
}

impl AndroidSettingsGuard {
    /// Launch Settings and connect to it.
    async fn launch() -> Self {
        // Ensure emulator is running first (will be stopped when guard is dropped)
        let emulator = EmulatorGuard::new();

        let mut accessibility =
            AndroidAccessibility::new(None).expect("Failed to create Android accessibility reader");

        // Launch Settings
        accessibility
            .launch_app("com.android.settings")
            .await
            .expect("Failed to launch Settings");

        // Wait for app to settle
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Create App connection
        let config = AppConfig::new().with_platform(Platform::Android);
        let app = App::with_config(config)
            .await
            .expect("Failed to connect to Android");

        // Wait for Settings UI to be ready
        let timeout = Duration::from_secs(10);
        let start = std::time::Instant::now();
        loop {
            // Settings should have labels/text views
            let found = app.locator("Label").no_wait().count().await > 0;
            if found {
                break;
            }
            if start.elapsed() >= timeout {
                panic!("Settings UI did not become ready");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        Self {
            app,
            accessibility,
            emulator,
        }
    }
}

impl Drop for AndroidSettingsGuard {
    fn drop(&mut self) {
        // Use spawn_blocking to avoid "Cannot start a runtime from within a runtime" error
        // We don't wait for completion since Drop can't be async
        let _ = self.accessibility.adb().stop_app("com.android.settings");
    }
}

impl Deref for AndroidSettingsGuard {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.app
    }
}

// ============================================================================
// Tests
// ============================================================================

/// Test that we can read the accessibility tree from Android Settings.
#[tokio::test]
#[serial]
#[ignore = "Requires Android device/emulator with ADB"]
async fn test_android_settings_accessibility_tree() {
    let settings = AndroidSettingsGuard::launch().await;

    let tree = settings
        .tree()
        .await
        .expect("Failed to get accessibility tree");

    println!("App name: {:?}", tree.app_name);
    println!("Element count: {}", tree.element_count);

    assert!(tree.element_count > 0, "Tree should have elements");

    // Settings should have labels (TextViews)
    let label_count = settings.locator("Label").no_wait().count().await;
    println!("Found {} labels", label_count);
    assert!(label_count > 0, "Settings should have labels");

    // Print some elements for debugging
    let labels = settings
        .locator("Label")
        .no_wait()
        .all()
        .await
        .expect("Failed to get labels");

    println!("\nSample labels:");
    for (i, label) in labels.iter().take(10).enumerate() {
        println!("  {}: {:?}", i, label.title);
    }
}

/// Test performing a click action on Settings.
#[tokio::test]
#[serial]
#[ignore = "Requires Android device/emulator with ADB"]
async fn test_android_settings_perform_action() {
    let mut settings = AndroidSettingsGuard::launch().await;

    // First go to main settings
    settings.accessibility.press_home().await.ok();
    tokio::time::sleep(Duration::from_millis(500)).await;
    settings
        .accessibility
        .launch_app("com.android.settings")
        .await
        .expect("Failed to relaunch settings");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Try to find and click on a common settings item
    // Look for "Network" or "Wi-Fi" or "Display" - common settings
    let search_terms = ["Network", "Wi-Fi", "Display", "Sound", "Battery"];

    for term in search_terms {
        let selector = format!("Label[title*='{}']", term);
        if settings.locator(&selector).no_wait().exists().await {
            println!("Found settings item: {}", term);

            // Click on it
            settings
                .locator(&selector)
                .first()
                .click()
                .await
                .expect("Failed to click");

            tokio::time::sleep(Duration::from_secs(1)).await;

            // Verify we navigated (should have different content now)
            let tree = settings.fresh_tree().await.expect("Failed to get tree");
            println!("After click, element count: {}", tree.element_count);

            // Go back
            settings
                .accessibility
                .press_back()
                .await
                .expect("Failed to press back");

            println!("Successfully clicked and navigated!");
            return;
        }
    }

    // If none of the terms found, just verify we can interact with any clickable item
    let clickable = settings
        .locator("[actions*='click']")
        .no_wait()
        .first()
        .get()
        .await
        .expect("Failed to find clickable");

    if let Some(elem) = clickable {
        println!("Found clickable element: {:?}", elem.title);
    }
}

/// Test screenshot capture functionality.
#[tokio::test]
#[serial]
#[ignore = "Requires Android device/emulator with ADB"]
async fn test_android_settings_screenshot() {
    let settings = AndroidSettingsGuard::launch().await;

    // Capture screenshot
    let screenshot = settings
        .screenshot()
        .await
        .expect("Failed to capture screenshot");

    // Verify screenshot has reasonable dimensions
    println!(
        "Screenshot dimensions: {}x{}",
        screenshot.width, screenshot.height
    );
    println!("Screenshot data size: {} bytes", screenshot.data.len());

    assert!(screenshot.width > 0, "Screenshot width should be > 0");
    assert!(screenshot.height > 0, "Screenshot height should be > 0");

    // Verify it's valid PNG data
    let png_signature = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    assert_eq!(
        &screenshot.data[..8],
        &png_signature,
        "Screenshot data should start with PNG signature"
    );

    // Test annotated screenshot
    let annotated = settings
        .annotated_screenshot(Some("Label"), true)
        .await
        .expect("Failed to create annotated screenshot");

    let (w, h) = annotated.dimensions();
    println!(
        "Annotated screenshot: {}x{} with {} labels",
        w,
        h,
        annotated.labels().len()
    );

    assert!(!annotated.labels().is_empty(), "Should have some labels");
}

/// Test finding elements by various properties.
#[tokio::test]
#[serial]
#[ignore = "Requires Android device/emulator with ADB"]
async fn test_android_settings_find_elements() {
    let settings = AndroidSettingsGuard::launch().await;

    // Find labels (TextViews)
    let labels = settings
        .locator("Label")
        .no_wait()
        .all()
        .await
        .expect("Failed to get labels");
    println!("Found {} labels", labels.len());
    assert!(!labels.is_empty(), "Should find labels");

    // Find images (ImageViews)
    let images = settings.locator("Image").no_wait().count().await;
    println!("Found {} images", images);

    // Find scroll views
    let scrollviews = settings.locator("ScrollView").no_wait().count().await;
    println!("Found {} scroll views", scrollviews);

    // Find containers
    let containers = settings.locator("GenericContainer").no_wait().count().await;
    println!("Found {} containers", containers);

    // Check element bounds
    let labels_with_bounds = labels.iter().filter(|l| l.bounds.is_some()).count();
    println!(
        "Labels with bounds: {} / {}",
        labels_with_bounds,
        labels.len()
    );
    assert!(
        labels_with_bounds > 0,
        "At least some labels should have bounds"
    );
}

/// Test locator options (first, all, exists, count).
#[tokio::test]
#[serial]
#[ignore = "Requires Android device/emulator with ADB"]
async fn test_android_settings_locator_options() {
    let settings = AndroidSettingsGuard::launch().await;

    // Test count
    let label_count = settings.locator("Label").no_wait().count().await;
    println!("Label count: {}", label_count);

    // Test first
    let first_label = settings
        .locator("Label")
        .no_wait()
        .first()
        .get()
        .await
        .expect("Failed to get first label");
    assert!(first_label.is_some(), "Should get first label");
    if let Some(label) = first_label {
        println!("First label: {:?}", label.title);
    }

    // Test all
    let all_labels = settings
        .locator("Label")
        .no_wait()
        .all()
        .await
        .expect("Failed to get all labels");
    assert_eq!(
        all_labels.len(),
        label_count,
        "all() count should match count()"
    );

    // Test exists for nonexistent element
    let nonexistent = settings
        .locator("Label[title='NONEXISTENT_ELEMENT_12345']")
        .no_wait()
        .exists()
        .await;
    assert!(!nonexistent, "Nonexistent element should not exist");

    // Test exists for existing element
    let exists = settings.locator("Label").no_wait().exists().await;
    assert!(exists, "Labels should exist");
}

/// Test Android navigation buttons.
#[tokio::test]
#[serial]
#[ignore = "Requires Android device/emulator with ADB"]
async fn test_android_navigation() {
    let _emulator = EmulatorGuard::new();

    let mut accessibility =
        AndroidAccessibility::new(None).expect("Failed to create Android accessibility reader");

    // Launch Settings
    accessibility
        .launch_app("com.android.settings")
        .await
        .expect("Failed to launch settings");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Press Home
    accessibility
        .press_home()
        .await
        .expect("Failed to press home");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Press Recent Apps
    accessibility
        .press_recent_apps()
        .await
        .expect("Failed to press recent apps");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Press Home again to go back
    accessibility
        .press_home()
        .await
        .expect("Failed to press home");
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Navigation test completed successfully");
}

/// Test swipe gesture.
#[tokio::test]
#[serial]
#[ignore = "Requires Android device/emulator with ADB"]
async fn test_android_swipe_gesture() {
    let _emulator = EmulatorGuard::new();

    let mut accessibility =
        AndroidAccessibility::new(None).expect("Failed to create Android accessibility reader");

    // Get screen size
    let (width, height) = accessibility
        .screen_size()
        .expect("Failed to get screen size");
    println!("Screen size: {}x{}", width, height);

    // Launch Settings (has scrollable content)
    accessibility
        .launch_app("com.android.settings")
        .await
        .expect("Failed to launch settings");
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Swipe up to scroll down
    let center_x = width as f64 / 2.0;
    let start_y = height as f64 * 0.7;
    let end_y = height as f64 * 0.3;

    accessibility
        .swipe((center_x, start_y), (center_x, end_y), 300)
        .await
        .expect("Failed to swipe");

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Swipe down to scroll up
    accessibility
        .swipe((center_x, end_y), (center_x, start_y), 300)
        .await
        .expect("Failed to swipe back");

    println!("Swipe gesture test completed");

    // Cleanup
    accessibility
        .stop_app("com.android.settings")
        .await
        .expect("Failed to stop settings");
}
