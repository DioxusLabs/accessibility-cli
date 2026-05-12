//! Safe low-level wrappers for the desktop macOS APIs used by accessibility-cli.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::*;
