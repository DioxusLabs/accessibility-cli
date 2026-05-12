//! Safe low-level wrappers around Windows UI Automation, GDI, and input APIs.

#[cfg(target_os = "windows")]
pub use windows;
