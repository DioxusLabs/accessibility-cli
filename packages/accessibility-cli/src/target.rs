use crate::cli::{PlatformType, TargetArgs};
use crate::error::{CliError, CliResult};
#[cfg(target_os = "macos")]
use accessibility_core::accessibility::IosSimulatorTarget;
use accessibility_core::accessibility::{AndroidTarget, TargetedAccessibility};
use accessibility_core::platform::android::AndroidAccessibility;
#[cfg(target_os = "macos")]
use accessibility_core::platform::ios_simulator::IOSSimulatorAccessibility;
#[cfg(target_os = "macos")]
use accessibility_core::platform::macos::MacOSAccessibility;

pub fn validate_target_flags(target: &TargetArgs) -> CliResult<()> {
    match target.platform {
        PlatformType::MacOS | PlatformType::Windows | PlatformType::Linux => {
            reject(target.udid.is_some(), "--udid requires --platform ios")?;
            reject(
                target.serial.is_some(),
                "--serial requires --platform android",
            )?;
        }
        PlatformType::IOS => {
            reject(
                target.pid.is_some(),
                "--pid is valid only for mac, win, or linux",
            )?;
            reject(
                target.serial.is_some(),
                "--serial requires --platform android",
            )?;
        }
        PlatformType::Android => {
            reject(
                target.pid.is_some(),
                "--pid is valid only for mac, win, or linux",
            )?;
            reject(target.udid.is_some(), "--udid requires --platform ios")?;
        }
    }
    Ok(())
}

pub fn ensure_platform(
    target: &TargetArgs,
    allowed: &[PlatformType],
    action: &str,
) -> CliResult<()> {
    validate_target_flags(target)?;
    if allowed.contains(&target.platform) {
        return Ok(());
    }

    let supported = allowed
        .iter()
        .map(|platform| platform.display_name())
        .collect::<Vec<_>>()
        .join(", ");
    Err(CliError::usage(format!(
        "{action} is supported only on {supported}"
    )))
}

pub async fn targeted_adapter(
    target: &TargetArgs,
    pid_required_for_pid_platforms: bool,
) -> CliResult<TargetedAccessibility> {
    validate_target_flags(target)?;
    if pid_required_for_pid_platforms && target.platform.is_pid_target() && target.pid.is_none() {
        return Err(CliError::usage(format!(
            "{} app operations require --pid; use list-windows to find a target PID",
            target.platform.display_name()
        )));
    }

    match target.platform {
        PlatformType::MacOS => macos_adapter(target.pid),
        PlatformType::Windows => windows_adapter(target.pid),
        PlatformType::Linux => linux_adapter(target.pid).await,
        PlatformType::IOS => ios_targeted_adapter(target.udid.as_deref()),
        PlatformType::Android => android_targeted_adapter(target.serial.as_deref()),
    }
}

pub fn android_raw_adapter(target: &TargetArgs) -> CliResult<AndroidAccessibility> {
    ensure_platform(target, &[PlatformType::Android], "Android device control")?;
    AndroidAccessibility::new(target.serial.as_deref())
        .map_err(|e| CliError::runtime(format_android_adapter_error(e)))
}

#[cfg(target_os = "macos")]
pub fn ios_raw_adapter(target: &TargetArgs) -> CliResult<IOSSimulatorAccessibility> {
    ensure_platform(target, &[PlatformType::IOS], "iOS Simulator control")?;
    IOSSimulatorAccessibility::new(target.udid.as_deref())
        .map_err(|e| CliError::runtime(format_ios_adapter_error(e)))
}

#[cfg(not(target_os = "macos"))]
pub fn ios_raw_adapter(_target: &TargetArgs) -> CliResult<()> {
    Err(CliError::runtime(
        "Error: iOS platform is only supported on macOS (via Simulator)",
    ))
}

fn reject(condition: bool, message: &str) -> CliResult<()> {
    if condition {
        Err(CliError::usage(message))
    } else {
        Ok(())
    }
}

fn android_targeted_adapter(serial: Option<&str>) -> CliResult<TargetedAccessibility> {
    let target = match serial {
        Some(serial) => AndroidTarget::Serial(serial.to_owned()),
        None => AndroidTarget::DefaultDevice,
    };
    TargetedAccessibility::new_android(target)
        .map_err(|e| CliError::runtime(format_android_adapter_error(e)))
}

#[cfg(target_os = "macos")]
fn macos_adapter(pid: Option<u32>) -> CliResult<TargetedAccessibility> {
    if !MacOSAccessibility::is_process_trusted() {
        return Err(CliError::runtime(
            "Error: Accessibility permissions not granted.\n\nPlease enable accessibility access for this terminal/app:\n  1. Open System Preferences > Privacy & Security > Accessibility\n  2. Click the lock icon to make changes\n  3. Add and enable your terminal app (Terminal, iTerm2, etc.)",
        ));
    }

    let adapter = match pid {
        Some(pid) => TargetedAccessibility::new_macos(pid),
        None => TargetedAccessibility::new_macos_system(),
    };
    adapter.map_err(|e| CliError::runtime(format!("Failed to create macOS adapter: {e}")))
}

#[cfg(not(target_os = "macos"))]
fn macos_adapter(_pid: Option<u32>) -> CliResult<TargetedAccessibility> {
    Err(CliError::runtime(
        "Error: macOS platform is only supported on macOS",
    ))
}

#[cfg(target_os = "windows")]
fn windows_adapter(pid: Option<u32>) -> CliResult<TargetedAccessibility> {
    let adapter = match pid {
        Some(pid) => TargetedAccessibility::new_windows(pid),
        None => TargetedAccessibility::new_windows_system(),
    };
    adapter.map_err(|e| CliError::runtime(format!("Failed to create Windows adapter: {e}")))
}

#[cfg(not(target_os = "windows"))]
fn windows_adapter(_pid: Option<u32>) -> CliResult<TargetedAccessibility> {
    Err(CliError::runtime(
        "Error: Windows platform is only supported on Windows",
    ))
}

#[cfg(target_os = "linux")]
async fn linux_adapter(pid: Option<u32>) -> CliResult<TargetedAccessibility> {
    let adapter = match pid {
        Some(pid) => TargetedAccessibility::new_linux(pid).await,
        None => TargetedAccessibility::new_linux_system().await,
    };
    adapter.map_err(|e| {
        CliError::runtime(format!(
            "Failed to create Linux adapter: {e}\n\nMake sure:\n  1. AT-SPI2 is running (accessibility services enabled)\n  2. The target application supports accessibility"
        ))
    })
}

#[cfg(not(target_os = "linux"))]
async fn linux_adapter(_pid: Option<u32>) -> CliResult<TargetedAccessibility> {
    Err(CliError::runtime(
        "Error: Linux platform is only supported on Linux",
    ))
}

#[cfg(target_os = "macos")]
fn ios_targeted_adapter(udid: Option<&str>) -> CliResult<TargetedAccessibility> {
    let target = match udid {
        Some(udid) => IosSimulatorTarget::Udid(udid.to_owned()),
        None => IosSimulatorTarget::Booted,
    };
    TargetedAccessibility::new_ios(target)
        .map_err(|e| CliError::runtime(format!("Failed to create iOS adapter: {e}")))
}

#[cfg(not(target_os = "macos"))]
fn ios_targeted_adapter(_udid: Option<&str>) -> CliResult<TargetedAccessibility> {
    Err(CliError::runtime(
        "Error: iOS platform is only supported on macOS (via Simulator)",
    ))
}

fn format_android_adapter_error(error: impl std::fmt::Display) -> String {
    format!(
        "Failed to create Android adapter: {error}\n\nMake sure:\n  1. ADB is installed and in your PATH\n  2. An Android device/emulator is connected (`adb devices`)\n  3. USB debugging is enabled on the device"
    )
}

#[cfg(target_os = "macos")]
fn format_ios_adapter_error(error: impl std::fmt::Display) -> String {
    format!(
        "Failed to create iOS Simulator adapter: {error}\n\nMake sure:\n  1. iOS Simulator is running\n  2. A simulator is booted (not just the Simulator.app window)\n  3. An app is open and in focus in the simulator\n  4. Xcode is installed (for CoreSimulator framework)"
    )
}
