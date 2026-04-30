//! Error types for the SkyVM API.

use std::fmt;

/// Result type for SkyVM operations.
pub type SkyVMResult<T> = Result<T, SkyVMError>;

/// Errors that can occur when using the SkyVM API.
#[derive(Debug)]
pub enum SkyVMError {
    /// Element was not found matching the selector.
    ElementNotFound {
        /// The selector that didn't match any elements.
        selector: String,
    },

    /// Multiple elements matched when exactly one was expected.
    MultipleMatches {
        /// The selector that matched multiple elements.
        selector: String,
        /// The number of matches found.
        count: usize,
    },

    /// Operation timed out waiting for a condition.
    Timeout {
        /// The operation that timed out.
        operation: String,
        /// The timeout duration in milliseconds.
        timeout_ms: u64,
    },

    /// An action failed on an element.
    ActionFailed {
        /// The action that failed.
        action: String,
        /// A description of the failure.
        message: String,
    },

    /// Invalid selector syntax.
    InvalidSelector {
        /// The invalid selector string.
        selector: String,
        /// Description of the syntax error.
        message: String,
    },

    /// Platform not supported for this operation.
    PlatformNotSupported {
        /// The operation that's not supported.
        operation: String,
        /// The current platform name.
        platform: String,
    },

    /// Screenshot capture failed.
    ScreenshotFailed {
        /// A description of the failure.
        message: String,
    },

    /// Element found but condition not satisfied within timeout.
    ConditionNotMet {
        /// The selector used to find the element.
        selector: String,
        /// Description of the condition that wasn't met.
        condition: String,
        /// The timeout duration in milliseconds.
        timeout_ms: u64,
    },

    /// Connection to the application failed.
    ConnectionFailed {
        /// A description of the failure.
        message: String,
    },

    /// Generic error wrapping an underlying error.
    Other(anyhow::Error),
}

impl fmt::Display for SkyVMError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SkyVMError::ElementNotFound { selector } => {
                write!(f, "Element not found: {}", selector)
            }
            SkyVMError::MultipleMatches { selector, count } => {
                write!(
                    f,
                    "Multiple elements ({}) matched selector: {}",
                    count, selector
                )
            }
            SkyVMError::Timeout {
                operation,
                timeout_ms,
            } => {
                write!(f, "Timeout after {}ms: {}", timeout_ms, operation)
            }
            SkyVMError::ActionFailed { action, message } => {
                write!(f, "{} failed: {}", action, message)
            }
            SkyVMError::InvalidSelector { selector, message } => {
                write!(f, "Invalid selector '{}': {}", selector, message)
            }
            SkyVMError::PlatformNotSupported {
                operation,
                platform,
            } => {
                write!(f, "{} not supported on {}", operation, platform)
            }
            SkyVMError::ScreenshotFailed { message } => {
                write!(f, "Screenshot failed: {}", message)
            }
            SkyVMError::ConditionNotMet {
                selector,
                condition,
                timeout_ms,
            } => {
                write!(
                    f,
                    "Condition '{}' not met for selector '{}' within {}ms",
                    condition, selector, timeout_ms
                )
            }
            SkyVMError::ConnectionFailed { message } => {
                write!(f, "Connection failed: {}", message)
            }
            SkyVMError::Other(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for SkyVMError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SkyVMError::Other(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

impl From<anyhow::Error> for SkyVMError {
    fn from(e: anyhow::Error) -> Self {
        SkyVMError::Other(e)
    }
}
