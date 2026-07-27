//! Safe low-level wrappers around iOS Simulator private accessibility and HID APIs.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "macos")]
pub(crate) mod frameworks {
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
        const SUFFIX: &str = "Library/PrivateFrameworks/CoreSimulator.framework/CoreSimulator";

        // The system-wide copy is canonical and exists whenever any Xcode is
        // installed, so it goes first.
        let mut paths = vec![format!(
            "/Library/Developer/PrivateFrameworks/CoreSimulator.framework/CoreSimulator"
        )];

        if let Some(dev_path) = developer_dir() {
            paths.push(format!("{dev_path}/{SUFFIX}"));
        }
        paths.push(format!(
            "/Applications/Xcode.app/Contents/Developer/{SUFFIX}"
        ));

        for path in &paths {
            let c_path = CString::new(path.as_str())?;
            let handle =
                unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
            if !handle.is_null() {
                return Ok(());
            }
        }

        let error = unsafe { CStr::from_ptr(libc::dlerror()) };
        Err(anyhow!(
            "Failed to load CoreSimulator framework: {}. Tried paths: {:?}",
            error.to_string_lossy(),
            paths
        ))
    }

    /// Candidate SimulatorKit locations for a given developer directory.
    ///
    /// Xcode 27 moved SimulatorKit out of `Developer/Library/PrivateFrameworks`
    /// and into `Contents/SharedFrameworks` (a sibling of `Developer`), so both
    /// layouts have to be probed.
    fn simulatorkit_candidates(developer_dir: &str) -> [String; 2] {
        [
            format!("{developer_dir}/../SharedFrameworks/SimulatorKit.framework/SimulatorKit"),
            format!(
                "{developer_dir}/Library/PrivateFrameworks/SimulatorKit.framework/SimulatorKit"
            ),
        ]
    }

    /// Resolve the active developer directory, preferring `DEVELOPER_DIR`.
    ///
    /// Falls back to `xcode-select -p`, which can point at a Command Line Tools
    /// install that has no simulator frameworks at all — callers still probe the
    /// well-known Xcode.app locations afterwards.
    pub fn developer_dir() -> Option<String> {
        if let Ok(dir) = std::env::var("DEVELOPER_DIR")
            && !dir.is_empty()
        {
            return Some(dir);
        }

        let output = std::process::Command::new("xcode-select")
            .arg("-p")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!dir.is_empty()).then_some(dir)
    }

    /// Load the SimulatorKit framework from Xcode.
    pub fn load_simulatorkit_framework() -> Result<*mut c_void> {
        let mut paths_to_try: Vec<String> = Vec::new();

        if let Some(dev_path) = developer_dir() {
            paths_to_try.extend(simulatorkit_candidates(&dev_path));
        }

        for app in [
            "/Applications/Xcode.app/Contents/Developer",
            "/Applications/Xcode-beta.app/Contents/Developer",
        ] {
            paths_to_try.extend(simulatorkit_candidates(app));
        }

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
mod macos;

#[cfg(target_os = "macos")]
pub use frameworks::{
    developer_dir, load_axp_framework, load_coresimulator_framework, load_frameworks,
    load_simulatorkit_framework,
};

#[cfg(target_os = "macos")]
pub use macos::*;
