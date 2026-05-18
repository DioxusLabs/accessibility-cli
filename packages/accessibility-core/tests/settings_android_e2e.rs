//! End-to-end tests for accessibility-core Android accessibility APIs using Settings.
//!
//! These tests expect an Android device or emulator to already be connected through ADB.
//! CI provides that with `reactivecircus/android-emulator-runner`.
//!
//! Run with:
//! ```sh
//! cargo test -p accessibility-core --test settings_android_e2e -- --ignored --nocapture --test-threads=1
//! ```

use accessibility_core::accessibility::{Element, ElementTree};
use accessibility_core::api::{App, AppConfig, Platform};
use accessibility_core::platform::android::{AdbClient, AndroidAccessibility, AndroidExtensions};
use accesskit::Role;
use anyhow::{Context, Result, bail};
use serial_test::serial;
use std::ops::Deref;
use std::time::{Duration, Instant};

const SETTINGS_PACKAGE: &str = "com.android.settings";
const SETTINGS_ACTION: &str = "android.settings.SETTINGS";
const DEVICE_BOOT_TIMEOUT: Duration = Duration::from_secs(180);
const SETTINGS_PROCESS_TIMEOUT: Duration = Duration::from_secs(30);
const UI_READY_TIMEOUT: Duration = Duration::from_secs(90);
const POLL_INTERVAL: Duration = Duration::from_millis(1_500);
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

struct DeviceGuard {
    adb: AdbClient,
}

impl DeviceGuard {
    fn new() -> Result<Self> {
        let adb = AdbClient::new(None);
        adb.command(&["wait-for-device"])
            .context("Failed waiting for Android device")?;
        adb.check_connection()?;
        wait_for_boot(&adb)?;
        stabilize_device(&adb);
        Ok(Self { adb })
    }
}

fn wait_for_boot(adb: &AdbClient) -> Result<()> {
    let start = Instant::now();
    loop {
        if let Ok(output) = adb.shell(&["getprop", "sys.boot_completed"])
            && output.trim() == "1"
        {
            return Ok(());
        }

        if start.elapsed() >= DEVICE_BOOT_TIMEOUT {
            bail!(
                "Android device did not finish booting within {} seconds",
                DEVICE_BOOT_TIMEOUT.as_secs()
            );
        }

        std::thread::sleep(Duration::from_secs(2));
    }
}

fn stabilize_device(adb: &AdbClient) {
    let _ = adb.shell(&["input", "keyevent", "224"]);
    let _ = adb.shell(&["wm", "dismiss-keyguard"]);

    for setting in [
        "window_animation_scale",
        "transition_animation_scale",
        "animator_duration_scale",
    ] {
        let _ = adb.shell(&["settings", "put", "global", setting, "0"]);
    }
}

fn launch_settings(adb: &AdbClient) -> Result<()> {
    adb.shell(&[
        "am",
        "start",
        "-W",
        "-a",
        SETTINGS_ACTION,
        "-p",
        SETTINGS_PACKAGE,
    ])
    .context("Failed to launch Android Settings")?;
    Ok(())
}

fn wait_for_settings_process(adb: &AdbClient, timeout: Duration) -> Result<()> {
    let start = Instant::now();

    loop {
        let observation = match adb.shell(&["pidof", SETTINGS_PACKAGE]) {
            Ok(pid) => {
                let pid = pid.trim();
                if !pid.is_empty() {
                    return Ok(());
                }
                "pidof returned no Settings process".to_string()
            }
            Err(pidof_error) => match adb.shell(&["ps", "-A"]) {
                Ok(processes) => {
                    if processes
                        .lines()
                        .any(|line| line.contains(SETTINGS_PACKAGE))
                    {
                        return Ok(());
                    }
                    format!("Settings process was not listed by ps: {pidof_error}")
                }
                Err(ps_error) => format!("pidof failed: {pidof_error}; ps failed: {ps_error}"),
            },
        };

        if start.elapsed() >= timeout {
            bail!(
                "Android Settings process did not start within {} seconds: {}",
                timeout.as_secs(),
                observation
            );
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}

struct AndroidSettingsGuard {
    app: App,
    device: DeviceGuard,
}

impl AndroidSettingsGuard {
    async fn launch() -> Result<Self> {
        let device = DeviceGuard::new()?;
        let mut accessibility = AndroidAccessibility::new(None)
            .context("Failed to create Android accessibility reader")?;

        reset_settings(&mut accessibility).await?;

        let config = AppConfig::new()
            .with_platform(Platform::Android)
            .with_android_device()
            .with_timeout(UI_READY_TIMEOUT)
            .with_poll_interval(POLL_INTERVAL);
        let app = App::with_config(config)
            .await
            .context("Failed to connect to Android accessibility adapter")?;

        let tree = wait_for_settings_tree(&app, UI_READY_TIMEOUT).await?;
        println!(
            "Settings tree ready: {} elements, {} labels",
            tree.element_count,
            count_role(&tree.root, Role::Label)
        );

        Ok(Self { app, device })
    }
}

impl Drop for AndroidSettingsGuard {
    fn drop(&mut self) {
        let _ = self.device.adb.stop_app(SETTINGS_PACKAGE);
    }
}

impl Deref for AndroidSettingsGuard {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.app
    }
}

async fn reset_settings(accessibility: &mut AndroidAccessibility) -> Result<()> {
    let _ = accessibility.adb().stop_app(SETTINGS_PACKAGE);
    let _ = accessibility.wake_up().await;
    let _ = accessibility.press_home().await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    launch_settings(accessibility.adb())?;
    wait_for_settings_process(accessibility.adb(), SETTINGS_PROCESS_TIMEOUT)?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    Ok(())
}

async fn wait_for_settings_tree(app: &App, timeout: Duration) -> Result<ElementTree> {
    let start = Instant::now();
    let mut last_observation: String;

    loop {
        match app.fresh_tree().await {
            Ok(tree) => {
                let label_count = count_role(&tree.root, Role::Label);
                if tree.element_count > 0 && label_count > 0 {
                    return Ok(tree);
                }
                last_observation = format!(
                    "tree had {} elements and {} labels",
                    tree.element_count, label_count
                );
            }
            Err(error) => {
                last_observation = error.to_string();
            }
        }

        if start.elapsed() >= timeout {
            bail!(
                "Settings UI did not become ready within {} seconds: {}",
                timeout.as_secs(),
                last_observation
            );
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn count_role(element: &Element, role: Role) -> usize {
    usize::from(element.role == role)
        + element
            .children
            .iter()
            .map(|child| count_role(child, role))
            .sum::<usize>()
}

#[tokio::test]
#[serial]
#[ignore = "Requires Android device/emulator with ADB"]
async fn test_android_device_input_smoke() -> Result<()> {
    let device = DeviceGuard::new()?;
    let mut accessibility =
        AndroidAccessibility::new(None).context("Failed to create Android accessibility reader")?;

    accessibility.wake_up().await?;
    accessibility.press_home().await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    accessibility.press_recent_apps().await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    accessibility.press_home().await?;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let (width, height) = accessibility
        .refresh_screen_size()
        .context("Failed to get Android screen size")?;
    assert!(width > 0);
    assert!(height > 0);

    launch_settings(accessibility.adb())?;
    wait_for_settings_process(accessibility.adb(), SETTINGS_PROCESS_TIMEOUT)?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let center_x = width as f64 / 2.0;
    let start_y = height as f64 * 0.7;
    let end_y = height as f64 * 0.3;
    accessibility
        .swipe((center_x, start_y), (center_x, end_y), 300)
        .await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    accessibility
        .swipe((center_x, end_y), (center_x, start_y), 300)
        .await?;

    let _ = device.adb.stop_app(SETTINGS_PACKAGE);
    Ok(())
}

#[tokio::test]
#[serial]
#[ignore = "Requires Android device/emulator with ADB"]
async fn test_android_settings_smoke() -> Result<()> {
    let settings = AndroidSettingsGuard::launch().await?;
    let tree = settings.fresh_tree().await?;
    let label_count = count_role(&tree.root, Role::Label);

    println!(
        "Settings smoke tree: {} elements, {} labels",
        tree.element_count, label_count
    );

    assert!(tree.element_count > 0);
    assert!(label_count > 0);

    let labels = settings.locator("Label").no_wait().all().await?;
    assert!(!labels.is_empty());

    let first_label = settings.locator("Label").no_wait().first().get().await?;
    assert!(first_label.is_some());

    let screenshot = settings.screenshot().await?;
    println!(
        "Screenshot dimensions: {}x{}, {} bytes",
        screenshot.width,
        screenshot.height,
        screenshot.data.len()
    );

    assert!(screenshot.width > 0);
    assert!(screenshot.height > 0);
    assert!(screenshot.data.len() >= PNG_SIGNATURE.len());
    assert_eq!(&screenshot.data[..PNG_SIGNATURE.len()], PNG_SIGNATURE);

    Ok(())
}
