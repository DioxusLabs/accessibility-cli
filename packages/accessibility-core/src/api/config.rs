//! Configuration types for the SkyVM API.

use std::time::Duration;

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

    /// Process ID to target (None for focused app).
    pub pid: Option<u32>,

    /// Simulator UDID for iOS (macOS only).
    #[cfg(target_os = "macos")]
    pub udid: Option<String>,

    /// Device serial for Android (from `adb devices`).
    pub android_serial: Option<String>,

    /// Default timeout for locator operations.
    pub default_timeout: Duration,

    /// Default polling interval for retry loops.
    pub default_poll_interval: Duration,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            platform: Platform::default(),
            pid: None,
            #[cfg(target_os = "macos")]
            udid: None,
            android_serial: None,
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
        self.pid = Some(pid);
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
        self.udid = Some(udid.into());
        self
    }

    /// Set the Android device serial (from `adb devices`).
    pub fn with_android_serial(mut self, serial: impl Into<String>) -> Self {
        self.android_serial = Some(serial.into());
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
