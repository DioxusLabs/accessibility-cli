//! Application connection and control.

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::accessibility::{
    Element, ElementTree, Rect, Screenshot, TargetedAccessibility, TreeFilter,
};
use crate::input::MouseButton;

use super::config::{AppConfig, LocatorOptions, Platform};
use super::error::{Error, Result};
use super::locator::Locator;
use super::screenshot::AnnotatedScreenshot;

/// Represents a connection to an application for accessibility automation.
///
/// This is the main entry point for the Playwright-like API. Create an `App`
/// instance by calling `App::connect()` with a process ID and platform.
///
/// # Example
///
/// ```ignore
/// use accessibility_core::api::{App, Platform};
///
/// // Connect to a running application
/// let app = App::connect(12345, Platform::MacOS)?;
///
/// // Interact using locators
/// app.locator("Button[title='Submit']").click().await?;
/// ```
pub struct App {
    /// The underlying accessibility adapter.
    pub(crate) inner: Arc<Mutex<TargetedAccessibility>>,

    /// Configuration for this app connection.
    pub(crate) config: AppConfig,

    /// Cached element tree (refreshed on demand).
    pub(crate) cached_tree: Arc<Mutex<Option<ElementTree>>>,
}

impl App {
    /// Connect to an application by process ID.
    ///
    /// # Arguments
    /// * `pid` - The process ID to connect to
    /// * `platform` - The target platform
    ///
    /// # Example
    /// ```ignore
    /// let app = App::connect(12345, Platform::MacOS).await?;
    /// ```
    pub async fn connect(pid: u32, platform: Platform) -> Result<Self> {
        let config = AppConfig::default().with_pid(pid).with_platform(platform);
        Self::with_config(config).await
    }

    /// Connect to the focused application on the current platform.
    ///
    /// # Example
    /// ```ignore
    /// let app = App::focused().await?;
    /// ```
    pub async fn focused() -> Result<Self> {
        Self::with_config(AppConfig::default()).await
    }

    /// Connect with a custom configuration.
    ///
    /// # Example
    /// ```ignore
    /// let config = AppConfig::new()
    ///     .with_pid(12345)
    ///     .with_timeout(Duration::from_secs(60));
    /// let app = App::with_config(config).await?;
    /// ```
    pub async fn with_config(config: AppConfig) -> Result<Self> {
        let inner = Self::create_adapter(&config).await?;
        // Platform adapters wrap raw OS handles (e.g., AXUIElement, IUIAutomation)
        // that are not Send/Sync. The Arc/Mutex pair is for shared ownership within
        // a single runtime thread, not cross-thread movement.
        #[allow(clippy::arc_with_non_send_sync)]
        let inner = Arc::new(Mutex::new(inner));
        Ok(Self {
            inner,
            config,
            #[allow(clippy::arc_with_non_send_sync)]
            cached_tree: Arc::new(Mutex::new(None)),
        })
    }

    /// Create the platform-specific adapter.
    async fn create_adapter(config: &AppConfig) -> Result<TargetedAccessibility> {
        match config.platform {
            #[cfg(target_os = "macos")]
            Platform::MacOS => {
                TargetedAccessibility::new_macos(config.pid).map_err(|e| Error::ConnectionFailed {
                    message: format!("Failed to create macOS adapter: {}", e),
                })
            }
            #[cfg(target_os = "macos")]
            Platform::IOSSimulator => TargetedAccessibility::new_ios(config.udid.as_deref())
                .map_err(|e| Error::ConnectionFailed {
                    message: format!("Failed to create iOS Simulator adapter: {}", e),
                }),
            #[cfg(target_os = "windows")]
            Platform::Windows => TargetedAccessibility::new_windows(config.pid).map_err(|e| {
                Error::ConnectionFailed {
                    message: format!("Failed to create Windows adapter: {}", e),
                }
            }),
            #[cfg(target_os = "linux")]
            Platform::Linux => TargetedAccessibility::new_linux(config.pid)
                .await
                .map_err(|e| Error::ConnectionFailed {
                    message: format!("Failed to create Linux adapter: {}", e),
                }),
            Platform::Android => {
                TargetedAccessibility::new_android(config.android_serial.as_deref()).map_err(|e| {
                    Error::ConnectionFailed {
                        message: format!("Failed to create Android adapter: {}", e),
                    }
                })
            }
        }
    }

    /// Get the platform name.
    pub async fn platform_name(&self) -> &'static str {
        let inner = self.inner.lock().await;
        inner.platform_name()
    }

    /// Get the target PID.
    pub fn pid(&self) -> Option<u32> {
        self.config.pid
    }

    /// Create a locator for finding elements.
    ///
    /// Locators are lazy - they don't query elements until an action is performed.
    /// They automatically retry with the configured timeout when elements aren't found.
    ///
    /// # Arguments
    /// * `selector` - A CSS-like selector string
    ///
    /// # Example
    /// ```ignore
    /// // Find by role
    /// app.locator("Button").click().await?;
    ///
    /// // Find by role and attribute
    /// app.locator("Button[title='Submit']").click().await?;
    ///
    /// // Find by attribute only
    /// app.locator("[description='Close']").click().await?;
    /// ```
    pub fn locator(&self, selector: &str) -> Locator {
        Locator::new(
            self.inner.clone(),
            selector.to_string(),
            self.config.default_timeout,
            self.config.default_poll_interval,
            LocatorOptions::default(),
        )
    }

    /// Wait for a locator to match an element.
    ///
    /// This is a convenience method equivalent to `locator(selector).wait().await`.
    ///
    /// # Arguments
    /// * `selector` - A CSS-like selector string
    ///
    /// # Example
    /// ```ignore
    /// let element = app.wait_for_locator("StaticText[value*='Result']").await?;
    /// ```
    pub async fn wait_for_locator(&self, selector: &str) -> Result<Element> {
        self.locator(selector).wait().await
    }

    /// Refresh the cached element tree.
    ///
    /// This clears the cache and fetches a fresh tree from the application.
    pub async fn refresh(&self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.clear_cache();
        let tree = inner
            .get_tree(&TreeFilter::default())
            .await
            .map_err(Error::Other)?;
        drop(inner);

        let mut cached = self.cached_tree.lock().await;
        *cached = Some(tree);
        Ok(())
    }

    /// Get the current element tree, refreshing if necessary.
    pub async fn tree(&self) -> Result<ElementTree> {
        // Try to use cached tree first
        {
            let cached = self.cached_tree.lock().await;
            if let Some(tree) = cached.as_ref() {
                return Ok(tree.clone());
            }
        }

        // Fetch fresh tree
        let mut inner = self.inner.lock().await;
        let tree = inner
            .get_tree(&TreeFilter::default())
            .await
            .map_err(Error::Other)?;
        drop(inner);

        // Cache it
        let mut cached = self.cached_tree.lock().await;
        *cached = Some(tree.clone());
        Ok(tree)
    }

    /// Get a fresh element tree without using the cache.
    pub async fn fresh_tree(&self) -> Result<ElementTree> {
        let mut inner = self.inner.lock().await;
        inner.clear_cache();
        inner
            .get_tree(&TreeFilter::default())
            .await
            .map_err(Error::Other)
    }

    /// Clear the cached element tree.
    pub async fn clear_cache(&self) {
        let mut inner = self.inner.lock().await;
        inner.clear_cache();
        drop(inner);

        let mut cached = self.cached_tree.lock().await;
        *cached = None;
    }

    /// Capture a screenshot of the application.
    pub async fn screenshot(&self) -> Result<Screenshot> {
        let inner = self.inner.lock().await;
        inner
            .capture_screen()
            .map_err(|e: anyhow::Error| Error::ScreenshotFailed {
                message: e.to_string(),
            })
    }

    /// Get the screen bounds for the application.
    pub async fn screen_bounds(&self) -> Result<Rect> {
        let inner = self.inner.lock().await;
        inner
            .get_screen_bounds()
            .await
            .map_err(|e: anyhow::Error| Error::ScreenshotFailed {
                message: e.to_string(),
            })
    }

    /// Capture an annotated screenshot with element boxes.
    ///
    /// # Arguments
    /// * `selector` - Optional selector to filter elements (None = all interactive elements)
    /// * `labels` - Whether to draw numbered labels on elements
    pub async fn annotated_screenshot(
        &self,
        selector: Option<&str>,
        labels: bool,
    ) -> Result<AnnotatedScreenshot> {
        let tree = self.fresh_tree().await?;
        let screenshot = self.screenshot().await?;
        let bounds = self.screen_bounds().await?;

        let inner = self.inner.lock().await;
        let elements = inner
            .find_elements(&tree, selector, true)
            .map_err(Error::Other)?;
        drop(inner);

        Ok(AnnotatedScreenshot::new(
            screenshot, bounds, elements, labels,
        ))
    }

    /// Send a keystroke to the application.
    ///
    /// # Arguments
    /// * `key_spec` - A key specification like "enter", "cmd+c", "ctrl+shift+a"
    ///
    /// # Example
    /// ```ignore
    /// app.keystroke("enter").await?;
    /// app.keystroke("cmd+c").await?;
    /// ```
    pub async fn keystroke(&self, key_spec: &str) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner
            .send_keystroke(key_spec)
            .await
            .map_err(|e: anyhow::Error| Error::ActionFailed {
                action: "keystroke".to_string(),
                message: e.to_string(),
            })?;
        Ok(())
    }

    /// Type raw text into the application.
    ///
    /// # Arguments
    /// * `text` - The text to type
    ///
    /// # Example
    /// ```ignore
    /// app.type_text("Hello, World!").await?;
    /// ```
    pub async fn type_text(&self, text: &str) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner
            .type_raw(text)
            .await
            .map_err(|e: anyhow::Error| Error::ActionFailed {
                action: "type_text".to_string(),
                message: e.to_string(),
            })
    }

    /// Click at absolute screen coordinates.
    ///
    /// On macOS, this is routed to the connected PID when possible so the shared
    /// cursor and foreground app are left alone.
    pub async fn mouse_click_at(&self, x: f64, y: f64, button: MouseButton) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner
            .mouse_click_at(x, y, button)
            .await
            .map_err(|e: anyhow::Error| Error::ActionFailed {
                action: "mouse_click_at".to_string(),
                message: e.to_string(),
            })
    }

    /// Poll until a condition is true or timeout is reached.
    ///
    /// This is a low-level utility for implementing custom wait conditions.
    /// Note: The condition closure is sync - use `tree()` and manual polling for async operations.
    ///
    /// # Arguments
    /// * `condition` - A closure that returns `Some(T)` when the condition is met
    /// * `timeout` - Maximum time to wait (None = use default timeout)
    /// * `poll_interval` - Time between checks (None = use default interval)
    pub async fn poll_until<T, F>(
        &self,
        mut condition: F,
        timeout: Option<Duration>,
        poll_interval: Option<Duration>,
    ) -> Result<T>
    where
        F: FnMut(&mut TargetedAccessibility) -> Option<T>,
    {
        let timeout = timeout.unwrap_or(self.config.default_timeout);
        let poll_interval = poll_interval.unwrap_or(self.config.default_poll_interval);
        let start = Instant::now();

        loop {
            // Clear cache and check condition
            {
                let mut inner = self.inner.lock().await;
                inner.clear_cache();
                if let Some(result) = condition(&mut inner) {
                    return Ok(result);
                }
            }

            // Check timeout
            if start.elapsed() >= timeout {
                return Err(Error::Timeout {
                    operation: "poll_until".to_string(),
                    timeout_ms: timeout.as_millis() as u64,
                });
            }

            // Wait before next poll
            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Wait for the UI state to stabilize (no changes for a duration).
    ///
    /// This is useful after performing actions to ensure the UI has settled.
    ///
    /// # Arguments
    /// * `stable_duration` - How long the UI must be stable
    /// * `timeout` - Maximum time to wait for stability
    pub async fn wait_for_stable(
        &self,
        stable_duration: Duration,
        timeout: Duration,
    ) -> Result<()> {
        let start = Instant::now();
        let mut last_version = {
            let inner = self.inner.lock().await;
            inner.snapshot_version()
        };
        let mut stable_since = Instant::now();

        loop {
            tokio::time::sleep(self.config.default_poll_interval).await;

            let current_version = {
                let mut inner = self.inner.lock().await;
                inner.clear_cache();
                let _ = inner.get_tree(&TreeFilter::default()).await;
                inner.snapshot_version()
            };

            if current_version != last_version {
                last_version = current_version;
                stable_since = Instant::now();
            }

            if stable_since.elapsed() >= stable_duration {
                return Ok(());
            }

            if start.elapsed() >= timeout {
                return Err(Error::Timeout {
                    operation: "wait_for_stable".to_string(),
                    timeout_ms: timeout.as_millis() as u64,
                });
            }
        }
    }
}
