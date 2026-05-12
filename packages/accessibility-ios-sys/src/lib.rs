//! Safe low-level wrappers around iOS Simulator private accessibility and HID APIs.

pub use block2;

#[cfg(target_os = "macos")]
pub use libc;
#[cfg(target_os = "macos")]
pub use objc2;
#[cfg(target_os = "macos")]
pub use objc2_core_foundation;
#[cfg(target_os = "macos")]
pub use objc2_foundation;

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{CStr, CString, c_char, c_void};

    use anyhow::{Result, anyhow};

    /// Load the AccessibilityPlatformTranslation private framework.
    pub fn load_axp_framework() -> Result<()> {
        let path = b"/System/Library/PrivateFrameworks/AccessibilityPlatformTranslation.framework/AccessibilityPlatformTranslation\0";

        let handle = unsafe {
            libc::dlopen(
                path.as_ptr() as *const c_char,
                libc::RTLD_NOW | libc::RTLD_GLOBAL,
            )
        };
        if handle.is_null() {
            let error = unsafe { CStr::from_ptr(libc::dlerror()) };
            return Err(anyhow!(
                "Failed to load AccessibilityPlatformTranslation: {}",
                error.to_string_lossy()
            ));
        }
        Ok(())
    }

    /// Load the CoreSimulator private framework.
    pub fn load_coresimulator_framework() -> Result<()> {
        let paths: &[&[u8]] = &[
            b"/Library/Developer/PrivateFrameworks/CoreSimulator.framework/CoreSimulator\0",
            b"/Applications/Xcode.app/Contents/Developer/Library/PrivateFrameworks/CoreSimulator.framework/CoreSimulator\0",
        ];

        for path in paths {
            let handle = unsafe {
                libc::dlopen(
                    path.as_ptr() as *const c_char,
                    libc::RTLD_NOW | libc::RTLD_GLOBAL,
                )
            };
            if !handle.is_null() {
                return Ok(());
            }
        }

        let error = unsafe { CStr::from_ptr(libc::dlerror()) };
        Err(anyhow!(
            "Failed to load CoreSimulator framework: {}",
            error.to_string_lossy()
        ))
    }

    /// Load the SimulatorKit framework from Xcode.
    pub fn load_simulatorkit_framework() -> Result<*mut c_void> {
        let mut paths_to_try: Vec<String> = Vec::new();

        if let Ok(output) = std::process::Command::new("xcode-select")
            .arg("-p")
            .output()
            && output.status.success()
        {
            let dev_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            paths_to_try.push(format!(
                "{}/Library/PrivateFrameworks/SimulatorKit.framework/SimulatorKit",
                dev_path
            ));
        }

        paths_to_try.extend([
            "/Applications/Xcode.app/Contents/Developer/Library/PrivateFrameworks/SimulatorKit.framework/SimulatorKit".to_string(),
            "/Applications/Xcode-beta.app/Contents/Developer/Library/PrivateFrameworks/SimulatorKit.framework/SimulatorKit".to_string(),
        ]);

        for path in &paths_to_try {
            let c_path = CString::new(path.as_str()).unwrap();
            let handle =
                unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
            if !handle.is_null() {
                return Ok(handle);
            }
        }

        let error = unsafe { CStr::from_ptr(libc::dlerror()) };
        Err(anyhow!(
            "Failed to load SimulatorKit framework: {}. Tried paths: {:?}",
            error.to_string_lossy(),
            paths_to_try
        ))
    }

    /// Load all required private frameworks.
    pub fn load_frameworks() -> Result<()> {
        load_axp_framework()?;
        load_coresimulator_framework()?;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub use macos::{
    load_axp_framework, load_coresimulator_framework, load_frameworks, load_simulatorkit_framework,
};
