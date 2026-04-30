//! Platform-specific implementations of the AccessibilityReader trait.
//!
//! Each platform has its own module with a concrete implementation:
//! - `macos`: Uses AXUIElement API for macOS desktop apps
//! - `ios_simulator`: Uses AccessibilityPlatformTranslation for iOS Simulator apps
//! - `msft`: Uses Windows UI Automation
//! - `x11`: Uses AT-SPI via D-Bus
//! - `android`: Uses ADB (Android Debug Bridge) - works on any host OS

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub mod ios_simulator;

#[cfg(target_os = "windows")]
pub mod msft;

#[cfg(target_os = "linux")]
pub mod x11;

// Android via ADB works on any host platform (macOS, Linux, Windows)
pub mod android;
pub use android::AndroidExtensions;

// Re-export the platform-appropriate implementation as `PlatformAccessibility`
#[cfg(target_os = "macos")]
pub use macos::MacOSAccessibility as PlatformAccessibility;

#[cfg(target_os = "windows")]
pub use msft::WindowsAccessibility as PlatformAccessibility;

#[cfg(target_os = "linux")]
pub use x11::LinuxAccessibility as PlatformAccessibility;

/// Create a new accessibility reader for the current platform.
#[cfg(target_os = "macos")]
pub fn create_accessibility_reader() -> anyhow::Result<macos::MacOSAccessibility> {
    macos::MacOSAccessibility::new()
}

#[cfg(target_os = "windows")]
pub fn create_accessibility_reader() -> anyhow::Result<msft::WindowsAccessibility> {
    msft::WindowsAccessibility::new()
}

#[cfg(target_os = "linux")]
pub async fn create_accessibility_reader() -> anyhow::Result<x11::LinuxAccessibility> {
    x11::LinuxAccessibility::new().await
}

// Re-export IOSAdapter trait for iOS-specific HID methods
#[cfg(target_os = "macos")]
pub use macos::IOSAdapter;
