//! Accessibility tree reading and control for computer use AI.
//!
//! This module provides a platform-agnostic interface for:
//! - Querying the accessibility tree of running applications
//! - Caching elements with sequential IDs for LLM interaction
//! - Performing actions on elements (click, focus, set value, etc.)
//! - Hit testing to find elements at screen coordinates
//! - Event-driven accessibility listening for real-time UI changes

mod cache;
pub mod query;
pub mod roles;
mod targeted;
mod types;

pub use cache::ElementCache;
pub use query::{AccessibilityPseudoClass, Selector, find_matches, parse as parse_query};
#[cfg(target_os = "macos")]
pub use targeted::IosSimulatorTarget;
pub use targeted::{AndroidTarget, Target, TargetedAccessibility};
pub use types::*;

use crate::input::{Code, Modifiers, MouseButton};
use accesskit::Action;
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::task::JoinHandle;

/// Platform-agnostic trait for reading and controlling the accessibility tree.
///
/// Implementations are provided for each platform:
/// - macOS: Uses AXUIElement API
/// - Windows: Uses UI Automation
/// - Linux: Uses AT-SPI via D-Bus
pub trait AccessibilityReader {
    /// Snapshot the accessibility tree for a target.
    ///
    /// The `filter` controls tree depth, element count limits, and filtering.
    ///
    /// Returns an `ElementTree` with all elements assigned sequential IDs.
    /// These IDs can be used with other methods until `clear_cache()` is called.
    fn get_tree(
        &mut self,
        target: &Target,
        filter: &TreeFilter,
    ) -> impl std::future::Future<Output = Result<ElementTree>>;

    /// Get a cached element by its ID.
    ///
    /// Returns None if the ID is not in the cache (call `get_tree()` first).
    fn get_element(&self, id: ElementKey) -> Option<&Element>;

    /// Perform an accessibility action on an element.
    ///
    /// Common actions include:
    /// - `Action::Click` - Click/activate the element
    /// - `Action::Focus` - Set keyboard focus to element
    /// - `Action::Blur` - Remove keyboard focus from element
    /// - `Action::Expand` / `Action::Collapse` - For expandable items
    /// - `Action::ScrollIntoView` - Scroll element into visible area
    fn perform_action(
        &mut self,
        id: ElementKey,
        action: Action,
    ) -> impl std::future::Future<Output = Result<()>>;

    /// Set the value of an element (text fields, sliders, etc.).
    fn set_value(
        &mut self,
        id: ElementKey,
        value: &str,
    ) -> impl std::future::Future<Output = Result<()>>;

    /// Find the element at the given screen coordinates.
    ///
    /// Returns the ElementKey if an element is found and cached.
    fn hit_test(
        &mut self,
        x: f64,
        y: f64,
    ) -> impl std::future::Future<Output = Result<Option<ElementKey>>>;

    /// Clear the element cache.
    ///
    /// Call this before taking a new snapshot to invalidate old IDs.
    fn clear_cache(&mut self);

    /// Get the current snapshot version.
    ///
    /// Increments each time `clear_cache()` is called.
    fn snapshot_version(&self) -> u64;

    // Platform adapter methods (merged from PlatformAdapter trait)

    /// Capture a screenshot for a target.
    fn capture_screen(&self, _target: &Target) -> Result<Screenshot> {
        anyhow::bail!("Screenshot not supported on this platform")
    }

    /// Get bounds for coordinate conversion.
    fn get_screen_bounds(
        &self,
        _target: &Target,
    ) -> impl std::future::Future<Output = Result<Rect>> {
        async { anyhow::bail!("Screen bounds not supported on this platform") }
    }

    /// Start a live video capture session, pushing encoded frames to `sink`.
    ///
    /// This is the streaming counterpart to [`Self::capture_screen`]. Only the
    /// iOS Simulator implements it today.
    fn start_video_capture(
        &self,
        _config: &crate::video::VideoConfig,
        _sink: crate::video::FrameSink,
    ) -> Result<Box<dyn crate::video::VideoCapture>> {
        crate::video::unsupported(self.platform_name())
    }

    /// Whether [`Self::start_video_capture`] is expected to succeed.
    fn supports_video_capture(&self) -> bool {
        false
    }

    /// Get the platform name (e.g., "macOS", "Windows", "Linux", "iOS").
    fn platform_name(&self) -> &'static str {
        "Unknown"
    }

    /// Send a keystroke with optional modifiers.
    fn keystroke(
        &mut self,
        _target: &Target,
        _key: Code,
        _modifiers: Modifiers,
    ) -> impl std::future::Future<Output = Result<()>> {
        async { anyhow::bail!("Keystroke not supported on this platform") }
    }

    /// Type raw text using keystroke simulation.
    fn type_raw(
        &mut self,
        _target: &Target,
        _text: &str,
    ) -> impl std::future::Future<Output = Result<()>> {
        async { anyhow::bail!("Type raw not supported on this platform") }
    }

    /// Click mouse at screen coordinates.
    fn mouse_click_at(
        &mut self,
        _target: &Target,
        _x: f64,
        _y: f64,
        _button: MouseButton,
    ) -> impl std::future::Future<Output = Result<()>> {
        async { anyhow::bail!("Mouse click not supported on this platform") }
    }

    /// Press a key down (without releasing).
    ///
    /// Use `release_key` to release it later. Useful for holding modifiers.
    fn press_key(
        &mut self,
        _target: &Target,
        _key: Code,
    ) -> impl std::future::Future<Output = Result<()>> {
        async { anyhow::bail!("Press key not supported on this platform") }
    }

    /// Release a previously pressed key.
    fn release_key(
        &mut self,
        _target: &Target,
        _key: Code,
    ) -> impl std::future::Future<Output = Result<()>> {
        async { anyhow::bail!("Release key not supported on this platform") }
    }

    /// Move the mouse to absolute screen coordinates.
    fn mouse_move(
        &mut self,
        _target: &Target,
        _x: f64,
        _y: f64,
    ) -> impl std::future::Future<Output = Result<()>> {
        async { anyhow::bail!("Mouse move not supported on this platform") }
    }

    /// Click a mouse button at the current position.
    fn mouse_click(
        &mut self,
        _target: &Target,
        _button: MouseButton,
    ) -> impl std::future::Future<Output = Result<()>> {
        async { anyhow::bail!("Mouse click not supported on this platform") }
    }

    /// Double-click a mouse button at the current position.
    fn mouse_double_click(
        &mut self,
        _target: &Target,
        _button: MouseButton,
    ) -> impl std::future::Future<Output = Result<()>> {
        async { anyhow::bail!("Mouse double click not supported on this platform") }
    }

    /// Scroll the mouse wheel.
    ///
    /// Positive delta scrolls up/left, negative scrolls down/right.
    fn mouse_scroll(
        &mut self,
        _target: &Target,
        _delta_x: f64,
        _delta_y: f64,
    ) -> impl std::future::Future<Output = Result<()>> {
        async { anyhow::bail!("Mouse scroll not supported on this platform") }
    }

    // Capability detection methods

    /// Returns true if this platform supports keystroke injection.
    fn supports_keystroke(&self) -> bool {
        false
    }

    /// Returns true if this platform supports mouse click injection.
    fn supports_mouse_click(&self) -> bool {
        false
    }

    /// Returns true if this platform supports hit testing.
    fn supports_hit_test(&self) -> bool {
        false
    }

    /// Returns true if this platform supports terminal display (viuer).
    fn supports_terminal_display(&self) -> bool {
        false
    }

    // Event listening methods

    /// Start listening for accessibility events with a callback.
    ///
    /// The listener runs in a background task and invokes the callback for each event.
    /// The callback is called from a background thread, so it must be `Send`.
    ///
    /// # Arguments
    /// * `config` - Configuration for event filtering
    /// * `callback` - Boxed function called for each accessibility event
    ///
    /// # Returns
    /// * `ListenerHandle` - Handle to stop the listener and check its status
    ///
    /// # Example
    /// ```ignore
    /// let config = ListenerConfig::new()
    ///     .with_event_types(vec![AccessibilityEventType::FocusChanged]);
    ///
    /// let handle = reader.start_listening(config, Box::new(|event| {
    ///     match event {
    ///         AccessibilityEvent::FocusChanged { element, .. } => {
    ///             println!("Focus changed: {:?}", element);
    ///         }
    ///         AccessibilityEvent::Stopped { .. } => {
    ///             println!("Listener stopped");
    ///         }
    ///         _ => {}
    ///     }
    /// }))?;
    ///
    /// // Do other work...
    ///
    /// handle.stop().await;
    /// ```
    fn start_listening(
        &mut self,
        _config: ListenerConfig,
        _callback: Box<dyn FnMut(AccessibilityEvent) + Send + 'static>,
    ) -> Result<ListenerHandle> {
        anyhow::bail!("Event listening not supported on this platform")
    }

    /// Check if this platform supports event listening.
    fn supports_event_listening(&self) -> bool {
        false
    }

    /// Get the list of event types supported on this platform.
    fn supported_event_types(&self) -> Vec<AccessibilityEventType> {
        Vec::new()
    }
}

/// Handle to control and monitor an accessibility event listener.
///
/// This handle is returned by `start_listening()` and can be used to:
/// - Stop the listener gracefully
/// - Check if the listener is still running
///
/// The listener is automatically stopped when this handle is dropped.
pub struct ListenerHandle {
    /// Atomic flag to signal the listener to stop.
    stop_flag: Arc<AtomicBool>,
    /// Handle to the background task running the listener.
    task_handle: Option<JoinHandle<()>>,
}

impl ListenerHandle {
    /// Create a new listener handle.
    ///
    /// # Arguments
    /// * `stop_flag` - Atomic flag shared with the listener to signal stop
    /// * `task_handle` - JoinHandle for the background listener task
    pub fn new(stop_flag: Arc<AtomicBool>, task_handle: JoinHandle<()>) -> Self {
        Self {
            stop_flag,
            task_handle: Some(task_handle),
        }
    }

    /// Stop the listener and wait for it to complete.
    ///
    /// This sets the stop flag and waits for the background task to finish.
    /// The callback will receive a `Stopped` event before the listener exits.
    pub async fn stop(mut self) {
        // Set stop flag
        self.stop_flag.store(true, Ordering::SeqCst);

        // Wait for task to complete
        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }
    }

    /// Stop the listener synchronously (blocking).
    ///
    /// This sets the stop flag and blocks until the background task finishes.
    /// Use this when you need to stop from a non-async context.
    pub fn stop_blocking(mut self) {
        // Set stop flag
        self.stop_flag.store(true, Ordering::SeqCst);

        // Wait for task to complete by polling until finished
        if let Some(handle) = self.task_handle.take() {
            while !handle.is_finished() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    /// Check if the listener is still running.
    ///
    /// Returns `true` if the background task is still active.
    pub fn is_running(&self) -> bool {
        self.task_handle.as_ref().is_some_and(|h| !h.is_finished())
    }

    /// Get a clone of the stop flag for use in listener implementations.
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        self.stop_flag.clone()
    }
}

impl Drop for ListenerHandle {
    fn drop(&mut self) {
        // Signal stop on drop to ensure cleanup
        self.stop_flag.store(true, Ordering::SeqCst);
        // Note: We set the stop flag but don't join the thread here since Drop is sync
        // and blocking could cause deadlocks. The thread will observe the stop flag
        // and exit gracefully. Dropping the JoinHandle detaches the thread.
    }
}
