//! Playwright-like fluent API for accessibility-core accessibility automation.
//!
//! This module provides a high-level, ergonomic API for interacting with
//! applications through their accessibility trees. It's designed to be
//! similar to Playwright's API for web automation.
//!
//! # Example
//!
//! ```ignore
//! use accessibility_core::api::{App, Platform};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Connect to a running application
//!     let app = App::connect(pid, Platform::MacOS)?;
//!
//!     // Use locators to find and interact with elements
//!     app.locator("Button[title='5']").click().await?;
//!     app.locator("Button[title='+']").click().await?;
//!     app.locator("Button[title='3']").click().await?;
//!     app.locator("Button[title='=']").click().await?;
//!
//!     // Wait for results
//!     let result = app.wait_for_locator("StaticText[value*='8']").await?;
//!     assert!(result.value().unwrap().contains("8"));
//!
//!     Ok(())
//! }
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
pub use error::{SkyVMError, SkyVMResult};
pub use locator::Locator;
pub use output::{
    JsonPrinter, LlmPrinter, LlmQueryPrinter, Printer, TreePrinter, format_element_selector,
    format_role_short, print_element_summary, print_formatted, print_statistics, print_tree,
    truncate,
};
pub use screenshot::{
    AnnotatedScreenshot, annotate_elements, decode_screenshot, draw_grid_overlay, draw_rect_border,
};
