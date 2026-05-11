//! Playwright-like fluent API for accessibility automation.
//!
//! This module provides a high-level, ergonomic API for interacting with
//! applications through their accessibility trees. It's designed to be
//! similar to Playwright's API for web automation.
//!
//! # Stability
//!
//! `accessibility-core` is pre-1.0 and the API surface in this module
//! (`App`, `Locator`, `Error`, `Result`, `AppConfig`, `LocatorOptions`,
//! `Platform`, `Element`) is considered the supported public API. Anything
//! reachable only through `crate::accessibility::*` or `crate::platform::*`
//! is implementation detail and may change between minor versions.
//!
//! Breaking changes within the supported API will be called out in the
//! `CHANGELOG.md`.
//!
//! # Example
//!
//! ```no_run
//! use accessibility_core::api::{App, Platform};
//! use std::time::Duration;
//!
//! # async fn run(pid: u32) -> Result<(), accessibility_core::api::Error> {
//! // Connect to a running application by pid.
//! let app = App::connect(pid, Platform::MacOS).await?;
//!
//! // Locators are lazy and auto-retry until the configured timeout.
//! app.locator("Button[title='5']").click().await?;
//! app.locator("Button[title='+']").click().await?;
//! app.locator("Button[title='3']").click().await?;
//! app.locator("Button[title='=']").click().await?;
//!
//! // Wait for the result label to settle.
//! let result = app
//!     .locator("StaticText[value*='8']")
//!     .with_timeout(Duration::from_secs(5))
//!     .wait()
//!     .await?;
//! # let _ = result;
//! # Ok(())
//! # }
//! ```
//!
//! # Filling and waiting
//!
//! ```no_run,ignore
//! use accessibility_core::api::{App, Platform};
//!
//! # async fn run(pid: u32) -> Result<(), accessibility_core::api::Error> {
//! let app = App::connect(pid, Platform::Windows).await?;
//!
//! // Fill a text field, then submit.
//! app.locator("TextField[title='Email']")
//!     .fill("user@example.com")
//!     .await?;
//! app.locator("Button[title='Sign in']").click().await?;
//!
//! // Block until a "Welcome" label appears.
//! let _ = app.wait_for_locator("StaticText[value^='Welcome']").await?;
//! # Ok(())
//! # }
//! ```

mod app;
mod config;
mod error;
mod locator;
mod output;
mod screenshot;

pub use crate::accessibility::Element;
pub use app::App;
pub use config::{AppConfig, LocatorOptions, Platform};
pub use error::{Error, Result};
pub use locator::Locator;
pub use output::{
    JsonPrinter, LlmPrinter, LlmQueryPrinter, Printer, TreePrinter, format_element_selector,
    format_role_short, print_element_summary, print_formatted, print_statistics, print_tree,
    truncate,
};
pub use screenshot::{
    AnnotatedScreenshot, annotate_elements, decode_screenshot, draw_grid_overlay, draw_rect_border,
};
