//! Configuration types for the API.

use std::time::Duration;

#[cfg(target_os = "macos")]
use crate::accessibility::IosSimulatorTarget;
use crate::accessibility::{AndroidTarget, Target};

/// Target platform for accessibility operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Platform {
    /// macOS (uses AXUIElement API)
    #[default]
    #[cfg(target_os = "macos")]
    MacOS,

    /// Windows (uses UI Automation API)
    #[cfg(target_os = "windows")]
    #[default]
    Windows,

    /// Linux (uses AT-SPI via D-Bus)
    #[cfg(target_os = "linux")]
    #[default]
    Linux,

    /// iOS Simulator (macOS only)
    #[cfg(target_os = "macos")]
    IOSSimulator,

    /// Android (via ADB - works on any host OS)
    Android,
}

impl Platform {
    /// Get the platform name as a string.
    pub fn name(&self) -> &'static str {
        match self {
            #[cfg(target_os = "macos")]
            Platform::MacOS => "macOS",
            #[cfg(target_os = "windows")]
            Platform::Windows => "Windows",
            #[cfg(target_os = "linux")]
            Platform::Linux => "Linux",
            #[cfg(target_os = "macos")]
            Platform::IOSSimulator => "iOS Simulator",
            Platform::Android => "Android",
        }
    }
}

/// Configuration for connecting to an application.
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Target platform.
    pub platform: Platform,

    /// Explicit target for the connection.
    pub target: Target,

    /// Default timeout for locator operations.
    pub default_timeout: Duration,

    /// Default polling interval for retry loops.
    pub default_poll_interval: Duration,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            platform: Platform::default(),
            target: Target::System,
            default_timeout: Duration::from_secs(30),
            default_poll_interval: Duration::from_millis(100),
        }
    }
}

impl AppConfig {
    /// Create a new config for the default platform.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the target PID.
    pub fn with_pid(mut self, pid: u32) -> Self {
        self.target = Target::Pid(pid);
        self
    }

    /// Set the explicit target.
    pub fn with_target(mut self, target: Target) -> Self {
        self.target = target;
        self
    }

    /// Set the default timeout for locator operations.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Set the default polling interval.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.default_poll_interval = interval;
        self
    }

    /// Set the target platform.
    pub fn with_platform(mut self, platform: Platform) -> Self {
        self.platform = platform;
        self
    }

    /// Set the simulator UDID (iOS only).
    #[cfg(target_os = "macos")]
    pub fn with_udid(mut self, udid: impl Into<String>) -> Self {
        self.target = Target::IosSimulator(IosSimulatorTarget::Udid(udid.into()));
        self
    }

    /// Use the first booted iOS Simulator.
    #[cfg(target_os = "macos")]
    pub fn with_booted_ios_simulator(mut self) -> Self {
        self.target = Target::IosSimulator(IosSimulatorTarget::Booted);
        self
    }

    /// Use the default connected Android device.
    pub fn with_android_device(mut self) -> Self {
        self.target = Target::Android(AndroidTarget::DefaultDevice);
        self
    }

    /// Set the Android device serial (from `adb devices`).
    pub fn with_android_serial(mut self, serial: impl Into<String>) -> Self {
        self.target = Target::Android(AndroidTarget::Serial(serial.into()));
        self
    }
}

/// Options for locator operations.
#[derive(Debug, Clone)]
pub struct LocatorOptions {
    /// Timeout for this specific operation (overrides app default).
    pub timeout: Option<Duration>,

    /// Polling interval for this specific operation (overrides app default).
    pub poll_interval: Option<Duration>,

    /// Whether to use strict mode (error on multiple matches).
    pub strict: bool,
}

impl Default for LocatorOptions {
    fn default() -> Self {
        Self {
            timeout: None,
            poll_interval: None,
            strict: true,
        }
    }
}

impl LocatorOptions {
    /// Create new options with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the timeout for this operation.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set the polling interval for this operation.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = Some(interval);
        self
    }

    /// Set strict mode (error on multiple matches).
    pub fn strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// Disable strict mode (take first match when multiple found).
    pub fn first(mut self) -> Self {
        self.strict = false;
        self
    }

    /// Disable timeout (return immediately if not found).
    pub fn no_wait(mut self) -> Self {
        self.timeout = Some(Duration::ZERO);
        self
    }
}
