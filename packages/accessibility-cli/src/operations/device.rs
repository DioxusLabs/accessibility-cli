use crate::cli::{
    ButtonArg, InputMethodArg, LongPressCommand, PlatformCommand, PlatformType, SwipeCommand,
    TapCommand, TargetArgs,
};
use crate::error::{CliError, CliResult};
use crate::parse::{PointArg, SwipeArg};
use crate::target;
use accessibility_core::platform::AndroidExtensions;
use accessibility_core::platform::android::AndroidAccessibility;
#[cfg(target_os = "macos")]
use accessibility_core::platform::ios_simulator::{HardwareButton, IOSSimulatorAccessibility};

trait DeviceActionExt {
    async fn run_tap(&mut self, point: PointArg, method: InputMethodArg) -> CliResult<()>;
    async fn run_swipe(
        &mut self,
        points: SwipeArg,
        duration_ms: u64,
        method: InputMethodArg,
    ) -> CliResult<()>;
    async fn run_button(&mut self, button: ButtonArg) -> CliResult<()>;
}

pub async fn tap(command: &TapCommand) -> CliResult<()> {
    match command.target.platform {
        PlatformType::Android => {
            let mut adapter = target::android_raw_adapter(&command.target)?;
            adapter.run_tap(command.point, command.method).await
        }
        PlatformType::IOS => run_ios_tap(command.point, command.method, &command.target).await,
        _ => Err(CliError::usage("tap is supported only on iOS and Android")),
    }
}

pub async fn swipe(command: &SwipeCommand) -> CliResult<()> {
    match command.target.platform {
        PlatformType::Android => {
            let mut adapter = target::android_raw_adapter(&command.target)?;
            adapter
                .run_swipe(command.points, command.duration, command.method)
                .await
        }
        PlatformType::IOS => {
            run_ios_swipe(
                command.points,
                command.duration,
                command.method,
                &command.target,
            )
            .await
        }
        _ => Err(CliError::usage(
            "swipe is supported only on iOS and Android",
        )),
    }
}

pub async fn long_press(command: &LongPressCommand) -> CliResult<()> {
    target::ensure_platform(&command.target, &[PlatformType::Android], "long-press")?;
    let mut adapter = target::android_raw_adapter(&command.target)?;
    println!(
        "Long pressing at ({}, {}) for {}ms...",
        command.point.x, command.point.y, command.duration
    );
    adapter
        .long_press(command.point.x, command.point.y, command.duration)
        .await
        .map_err(|e| CliError::runtime(format!("Long press failed: {e}")))?;
    println!("Long press successful!");
    Ok(())
}

pub async fn button(target_args: &TargetArgs, button: ButtonArg) -> CliResult<()> {
    match target_args.platform {
        PlatformType::Android => {
            let mut adapter = target::android_raw_adapter(target_args)?;
            adapter.run_button(button).await
        }
        PlatformType::IOS => run_ios_button(target_args, button).await,
        _ => Err(CliError::usage(
            "button is supported only on iOS and Android",
        )),
    }
}

pub async fn launch(command: &PlatformCommand) -> CliResult<()> {
    target::ensure_platform(&command.target, &[PlatformType::Android], "launch")?;
    let mut adapter = target::android_raw_adapter(&command.target)?;
    println!("Launching {}...", command.app_id);
    adapter
        .launch_app(&command.app_id)
        .await
        .map_err(|e| CliError::runtime(format!("Launch failed: {e}")))?;
    println!("App launched!");
    Ok(())
}

pub async fn stop(command: &PlatformCommand) -> CliResult<()> {
    target::ensure_platform(&command.target, &[PlatformType::Android], "stop")?;
    let mut adapter = target::android_raw_adapter(&command.target)?;
    println!("Stopping {}...", command.app_id);
    adapter
        .stop_app(&command.app_id)
        .await
        .map_err(|e| CliError::runtime(format!("Stop failed: {e}")))?;
    println!("App stopped!");
    Ok(())
}

pub async fn notifications(target_args: &TargetArgs) -> CliResult<()> {
    target::ensure_platform(target_args, &[PlatformType::Android], "notifications")?;
    let mut adapter = target::android_raw_adapter(target_args)?;
    println!("Opening notification shade...");
    adapter
        .open_notifications()
        .await
        .map_err(|e| CliError::runtime(format!("Open notifications failed: {e}")))?;
    println!("Notification shade opened!");
    Ok(())
}

pub async fn quick_settings(target_args: &TargetArgs) -> CliResult<()> {
    target::ensure_platform(target_args, &[PlatformType::Android], "quick-settings")?;
    let mut adapter = target::android_raw_adapter(target_args)?;
    println!("Opening quick settings...");
    adapter
        .open_quick_settings()
        .await
        .map_err(|e| CliError::runtime(format!("Open quick settings failed: {e}")))?;
    println!("Quick settings opened!");
    Ok(())
}

pub async fn wake(target_args: &TargetArgs) -> CliResult<()> {
    target::ensure_platform(target_args, &[PlatformType::Android], "wake")?;
    let mut adapter = target::android_raw_adapter(target_args)?;
    println!("Waking device...");
    adapter
        .wake_up()
        .await
        .map_err(|e| CliError::runtime(format!("Wake up failed: {e}")))?;
    println!("Device woken up!");
    Ok(())
}

pub async fn sleep(target_args: &TargetArgs) -> CliResult<()> {
    target::ensure_platform(target_args, &[PlatformType::Android], "sleep")?;
    let mut adapter = target::android_raw_adapter(target_args)?;
    println!("Putting device to sleep...");
    adapter
        .sleep()
        .await
        .map_err(|e| CliError::runtime(format!("Sleep failed: {e}")))?;
    println!("Device put to sleep!");
    Ok(())
}

pub fn test_load(target_args: &TargetArgs) -> CliResult<()> {
    target::ensure_platform(target_args, &[PlatformType::IOS], "test-load")?;
    test_load_ios()
}

impl DeviceActionExt for AndroidAccessibility {
    async fn run_tap(&mut self, point: PointArg, method: InputMethodArg) -> CliResult<()> {
        ensure_method(method, &[InputMethodArg::Auto, InputMethodArg::Adb], "tap")?;
        println!("Tapping at ({}, {})...", point.x, point.y);
        self.adb()
            .tap(point.x, point.y)
            .map_err(|e| CliError::runtime(format!("Tap failed: {e}")))?;
        println!("Tap successful!");
        Ok(())
    }

    async fn run_swipe(
        &mut self,
        points: SwipeArg,
        duration_ms: u64,
        method: InputMethodArg,
    ) -> CliResult<()> {
        ensure_method(
            method,
            &[InputMethodArg::Auto, InputMethodArg::Adb],
            "swipe",
        )?;
        println!(
            "Swiping from ({},{}) to ({},{}) over {}ms...",
            points.start.0, points.start.1, points.end.0, points.end.1, duration_ms
        );
        AndroidExtensions::swipe(self, points.start, points.end, duration_ms)
            .await
            .map_err(|e| CliError::runtime(format!("Swipe failed: {e}")))?;
        println!("Swipe successful!");
        Ok(())
    }

    async fn run_button(&mut self, button: ButtonArg) -> CliResult<()> {
        use ButtonArg::*;

        match button {
            Back => {
                println!("Pressing Back button...");
                self.press_back()
                    .await
                    .map_err(|e| CliError::runtime(format!("Back button failed: {e}")))?;
                println!("Back button pressed!");
            }
            Home => {
                println!("Pressing Home button...");
                self.press_home()
                    .await
                    .map_err(|e| CliError::runtime(format!("Home button failed: {e}")))?;
                println!("Home button pressed!");
            }
            Recent => {
                println!("Pressing Recent Apps button...");
                self.press_recent_apps()
                    .await
                    .map_err(|e| CliError::runtime(format!("Recent Apps button failed: {e}")))?;
                println!("Recent Apps button pressed!");
            }
            Menu => {
                println!("Pressing Menu button...");
                self.press_menu()
                    .await
                    .map_err(|e| CliError::runtime(format!("Menu button failed: {e}")))?;
                println!("Menu button pressed!");
            }
            VolumeUp => {
                println!("Pressing Volume Up...");
                self.volume_up()
                    .await
                    .map_err(|e| CliError::runtime(format!("Volume Up failed: {e}")))?;
                println!("Volume Up pressed!");
            }
            VolumeDown => {
                println!("Pressing Volume Down...");
                self.volume_down()
                    .await
                    .map_err(|e| CliError::runtime(format!("Volume Down failed: {e}")))?;
                println!("Volume Down pressed!");
            }
            Lock | Siri | Side => {
                return Err(CliError::usage(format!(
                    "button {button:?} is not supported on Android"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl DeviceActionExt for IOSSimulatorAccessibility {
    async fn run_tap(&mut self, point: PointArg, method: InputMethodArg) -> CliResult<()> {
        ensure_method(
            method,
            &[
                InputMethodArg::Auto,
                InputMethodArg::Accessibility,
                InputMethodArg::Hid,
            ],
            "tap",
        )?;

        match method {
            InputMethodArg::Auto | InputMethodArg::Accessibility => {
                self.get_tree(&accessibility_core::accessibility::TreeFilter::default())
                    .map_err(|e| {
                        CliError::runtime(format!(
                            "Tap failed: could not register simulator token: {e}"
                        ))
                    })?;
                println!("Tapping at ({}, {})...", point.x, point.y);
                self.tap(point.x, point.y)
                    .map_err(|e| CliError::runtime(format!("Tap failed: {e}")))?;
                println!("Tap successful!");
            }
            InputMethodArg::Hid => {
                println!("HID tap at ({}, {})...", point.x, point.y);
                self.hid_tap(point.x, point.y)
                    .map_err(|e| CliError::runtime(format!("HID tap failed: {e}")))?;
                println!("HID tap successful!");
            }
            InputMethodArg::Adb => unreachable!("validated by ensure_method"),
        }
        Ok(())
    }

    async fn run_swipe(
        &mut self,
        points: SwipeArg,
        duration_ms: u64,
        method: InputMethodArg,
    ) -> CliResult<()> {
        ensure_method(
            method,
            &[InputMethodArg::Auto, InputMethodArg::Hid],
            "swipe",
        )?;
        println!(
            "HID swipe from ({},{}) to ({},{}) over {}ms...",
            points.start.0, points.start.1, points.end.0, points.end.1, duration_ms
        );
        self.hid_swipe(points.start, points.end, duration_ms)
            .map_err(|e| CliError::runtime(format!("HID swipe failed: {e}")))?;
        println!("HID swipe successful!");
        Ok(())
    }

    async fn run_button(&mut self, button: ButtonArg) -> CliResult<()> {
        use ButtonArg::*;

        match button {
            Home => handle_hid_button(self, HardwareButton::Home, "Home"),
            Lock => handle_hid_button(self, HardwareButton::Lock, "Lock"),
            Siri => handle_hid_button(self, HardwareButton::Siri, "Siri"),
            Side => handle_hid_button(self, HardwareButton::SideButton, "Side"),
            Back | Recent | Menu | VolumeUp | VolumeDown => Err(CliError::usage(format!(
                "button {button:?} is not supported on iOS"
            ))),
        }
    }
}

#[cfg(target_os = "macos")]
fn handle_hid_button(
    adapter: &mut IOSSimulatorAccessibility,
    button: HardwareButton,
    name: &str,
) -> CliResult<()> {
    println!("HID {name} button press...");
    adapter
        .hid_button(button, 0)
        .map_err(|e| CliError::runtime(format!("HID {name} button press failed: {e}")))?;
    println!("HID {name} button press successful!");
    Ok(())
}

#[cfg(target_os = "macos")]
async fn run_ios_tap(
    point: PointArg,
    method: InputMethodArg,
    target_args: &TargetArgs,
) -> CliResult<()> {
    let mut adapter = target::ios_raw_adapter(target_args)?;
    adapter.run_tap(point, method).await
}

#[cfg(not(target_os = "macos"))]
async fn run_ios_tap(
    _point: PointArg,
    _method: InputMethodArg,
    _target_args: &TargetArgs,
) -> CliResult<()> {
    Err(CliError::runtime(
        "Error: iOS platform is only supported on macOS (via Simulator)",
    ))
}

#[cfg(target_os = "macos")]
async fn run_ios_swipe(
    points: SwipeArg,
    duration_ms: u64,
    method: InputMethodArg,
    target_args: &TargetArgs,
) -> CliResult<()> {
    let mut adapter = target::ios_raw_adapter(target_args)?;
    adapter.run_swipe(points, duration_ms, method).await
}

#[cfg(not(target_os = "macos"))]
async fn run_ios_swipe(
    _points: SwipeArg,
    _duration_ms: u64,
    _method: InputMethodArg,
    _target_args: &TargetArgs,
) -> CliResult<()> {
    Err(CliError::runtime(
        "Error: iOS platform is only supported on macOS (via Simulator)",
    ))
}

#[cfg(target_os = "macos")]
async fn run_ios_button(target_args: &TargetArgs, button: ButtonArg) -> CliResult<()> {
    let mut adapter = target::ios_raw_adapter(target_args)?;
    adapter.run_button(button).await
}

#[cfg(not(target_os = "macos"))]
async fn run_ios_button(_target_args: &TargetArgs, _button: ButtonArg) -> CliResult<()> {
    Err(CliError::runtime(
        "Error: iOS platform is only supported on macOS (via Simulator)",
    ))
}

#[cfg(target_os = "macos")]
fn test_load_ios() -> CliResult<()> {
    println!("Testing framework loading...");
    accessibility_core::platform::ios_simulator::load_frameworks()
        .map_err(|e| CliError::runtime(format!("Failed to load frameworks: {e}")))?;
    println!("Frameworks loaded successfully!");
    println!("  - AccessibilityPlatformTranslation.framework: OK");
    println!("  - CoreSimulator.framework: OK");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn test_load_ios() -> CliResult<()> {
    Err(CliError::runtime(
        "Error: iOS platform is only supported on macOS (via Simulator)",
    ))
}

fn ensure_method(
    method: InputMethodArg,
    allowed: &[InputMethodArg],
    action: &str,
) -> CliResult<()> {
    if allowed.contains(&method) {
        return Ok(());
    }
    Err(CliError::usage(format!(
        "--method {method:?} is not valid for {action} on this platform"
    )))
}
