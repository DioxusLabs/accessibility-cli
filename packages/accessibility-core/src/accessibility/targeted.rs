//! Targeted accessibility wrapper that stores an explicit target and provides convenience methods.

use accesskit::Action;
use anyhow::Result;

use super::{
    AccessibilityEvent, AccessibilityEventType, AccessibilityReader, Element, ElementKey,
    ElementTree, ListenerConfig, ListenerHandle, Rect, Screenshot, TreeFilter, find_matches,
    parse_query,
};
use crate::input::{Code, Modifiers, MouseButton, parse_key_code, parse_modifiers};

/// Enum holding platform-specific accessibility reader implementations.
#[allow(clippy::large_enum_variant)]
enum AccessibilityReaderImpl {
    #[cfg(target_os = "macos")]
    MacOS(crate::platform::macos::MacOSAccessibility),
    #[cfg(target_os = "macos")]
    IOSSimulator(crate::platform::ios_simulator::IOSSimulatorAccessibility),
    #[cfg(target_os = "windows")]
    Windows(crate::platform::msft::WindowsAccessibility),
    #[cfg(target_os = "linux")]
    Linux(crate::platform::x11::LinuxAccessibility),
    // Android via ADB (works on all host platforms)
    Android(crate::platform::android::AndroidAccessibility),
}

/// Macro to dispatch method calls to the inner reader implementation via trait.
macro_rules! dispatch {
    ($self:expr, $method:ident $(, $arg:expr)*) => {
        match &$self.inner {
            #[cfg(target_os = "macos")]
            AccessibilityReaderImpl::MacOS(r) => AccessibilityReader::$method(r $(, $arg)*),
            #[cfg(target_os = "macos")]
            AccessibilityReaderImpl::IOSSimulator(r) => AccessibilityReader::$method(r $(, $arg)*),
            #[cfg(target_os = "windows")]
            AccessibilityReaderImpl::Windows(r) => AccessibilityReader::$method(r $(, $arg)*),
            #[cfg(target_os = "linux")]
            AccessibilityReaderImpl::Linux(r) => AccessibilityReader::$method(r $(, $arg)*),
            AccessibilityReaderImpl::Android(r) => AccessibilityReader::$method(r $(, $arg)*),
        }
    };
}

/// Macro to dispatch mutable method calls to the inner reader implementation via trait.
macro_rules! dispatch_mut {
    ($self:expr, $method:ident $(, $arg:expr)*) => {
        match &mut $self.inner {
            #[cfg(target_os = "macos")]
            AccessibilityReaderImpl::MacOS(r) => AccessibilityReader::$method(r $(, $arg)*),
            #[cfg(target_os = "macos")]
            AccessibilityReaderImpl::IOSSimulator(r) => AccessibilityReader::$method(r $(, $arg)*),
            #[cfg(target_os = "windows")]
            AccessibilityReaderImpl::Windows(r) => AccessibilityReader::$method(r $(, $arg)*),
            #[cfg(target_os = "linux")]
            AccessibilityReaderImpl::Linux(r) => AccessibilityReader::$method(r $(, $arg)*),
            AccessibilityReaderImpl::Android(r) => AccessibilityReader::$method(r $(, $arg)*),
        }
    };
}

/// Macro to dispatch async mutable method calls to the inner reader implementation via trait.
macro_rules! dispatch_mut_async {
    ($self:expr, $method:ident $(, $arg:expr)*) => {
        match &mut $self.inner {
            #[cfg(target_os = "macos")]
            AccessibilityReaderImpl::MacOS(r) => AccessibilityReader::$method(r $(, $arg)*).await,
            #[cfg(target_os = "macos")]
            AccessibilityReaderImpl::IOSSimulator(r) => AccessibilityReader::$method(r $(, $arg)*).await,
            #[cfg(target_os = "windows")]
            AccessibilityReaderImpl::Windows(r) => AccessibilityReader::$method(r $(, $arg)*).await,
            #[cfg(target_os = "linux")]
            AccessibilityReaderImpl::Linux(r) => AccessibilityReader::$method(r $(, $arg)*).await,
            AccessibilityReaderImpl::Android(r) => AccessibilityReader::$method(r $(, $arg)*).await,
        }
    };
}

/// Wrapper that stores a target and provides convenience methods.
///
/// This wrapper holds an underlying `AccessibilityReader` implementation and
/// a target. PID-targeted constructors require a PID, so target-app methods do
/// not accept an optional PID at the public wrapper layer.
///
/// On PID-targeted desktop platforms, tree and control operations require an
/// explicit target PID. System targets are only for passive utilities that do
/// not address an app, such as full-screen capture or window discovery.
///
/// # Example
///
/// ```ignore
/// // Create a macOS reader targeting Calculator (PID 1234)
/// let mut reader = TargetedAccessibility::new_macos(1234)?;
///
/// // No need to pass pid on every call
/// let tree = reader.get_tree(&TreeFilter::default())?;
/// let screenshot = reader.capture_screen()?;
/// reader.keystroke(Code::Enter, Modifiers::empty())?;
/// ```
pub struct TargetedAccessibility {
    inner: AccessibilityReaderImpl,
    target: Target,
}

/// Explicit target for a `TargetedAccessibility` reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Desktop application process target.
    Pid(u32),
    /// iOS Simulator target.
    #[cfg(target_os = "macos")]
    IosSimulator(IosSimulatorTarget),
    /// Android target.
    Android(AndroidTarget),
    /// Passive system scope for operations that do not address an app.
    System,
}

/// iOS Simulator target selection.
#[cfg(target_os = "macos")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IosSimulatorTarget {
    /// Use the first booted simulator.
    Booted,
    /// Use a specific simulator UDID.
    Udid(String),
}

#[cfg(target_os = "macos")]
impl IosSimulatorTarget {
    pub(crate) fn udid(&self) -> Option<&str> {
        match self {
            Self::Booted => None,
            Self::Udid(udid) => Some(udid.as_str()),
        }
    }
}

/// Android device target selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AndroidTarget {
    /// Use the default connected device.
    DefaultDevice,
    /// Use a specific device serial from `adb devices`.
    Serial(String),
}

impl AndroidTarget {
    pub(crate) fn serial(&self) -> Option<&str> {
        match self {
            Self::DefaultDevice => None,
            Self::Serial(serial) => Some(serial.as_str()),
        }
    }
}

impl Target {
    pub(crate) fn require_pid(&self, platform: &str, operation: &str) -> Result<u32> {
        match self {
            Self::Pid(pid) => Ok(*pid),
            _ => anyhow::bail!("{platform} {operation} requires Target::Pid"),
        }
    }
}

// Platform-specific constructors
impl TargetedAccessibility {
    /// Create a new macOS accessibility reader targeting a specific process.
    #[cfg(target_os = "macos")]
    pub fn new_macos(pid: u32) -> Result<Self> {
        Ok(Self {
            inner: AccessibilityReaderImpl::MacOS(
                crate::platform::macos::MacOSAccessibility::new()?
            ),
            target: Target::Pid(pid),
        })
    }

    /// Create a macOS accessibility reader for passive system operations.
    ///
    /// This is intentionally separate from `new_macos(pid)` so app-targeting
    /// cannot accidentally omit the PID.
    #[cfg(target_os = "macos")]
    pub fn new_macos_system() -> Result<Self> {
        Ok(Self {
            inner: AccessibilityReaderImpl::MacOS(
                crate::platform::macos::MacOSAccessibility::new()?
            ),
            target: Target::System,
        })
    }

    /// Create a new iOS Simulator accessibility reader.
    #[cfg(target_os = "macos")]
    pub fn new_ios(target: IosSimulatorTarget) -> Result<Self> {
        Ok(Self {
            inner: AccessibilityReaderImpl::IOSSimulator(
                crate::platform::ios_simulator::IOSSimulatorAccessibility::new(target.udid())?,
            ),
            target: Target::IosSimulator(target),
        })
    }

    /// Create a new Windows accessibility reader targeting a specific process.
    #[cfg(target_os = "windows")]
    pub fn new_windows(pid: u32) -> Result<Self> {
        Ok(Self {
            inner: AccessibilityReaderImpl::Windows(
                crate::platform::msft::WindowsAccessibility::new()?,
            ),
            target: Target::Pid(pid),
        })
    }

    /// Create a Windows accessibility reader for passive system operations.
    ///
    /// This is intentionally separate from `new_windows(pid)` so app-targeting
    /// cannot accidentally omit the PID.
    #[cfg(target_os = "windows")]
    pub fn new_windows_system() -> Result<Self> {
        Ok(Self {
            inner: AccessibilityReaderImpl::Windows(
                crate::platform::msft::WindowsAccessibility::new()?,
            ),
            target: Target::System,
        })
    }

    /// Create a new Linux accessibility reader targeting a specific process.
    #[cfg(target_os = "linux")]
    pub async fn new_linux(pid: u32) -> Result<Self> {
        Ok(Self {
            inner: AccessibilityReaderImpl::Linux(
                crate::platform::x11::LinuxAccessibility::new().await?,
            ),
            target: Target::Pid(pid),
        })
    }

    /// Create a Linux accessibility reader for passive system operations.
    ///
    /// This is intentionally separate from `new_linux(pid)` so app-targeting
    /// cannot accidentally omit the PID.
    #[cfg(target_os = "linux")]
    pub async fn new_linux_system() -> Result<Self> {
        Ok(Self {
            inner: AccessibilityReaderImpl::Linux(
                crate::platform::x11::LinuxAccessibility::new().await?,
            ),
            target: Target::System,
        })
    }

    /// Create a new Android accessibility reader.
    pub fn new_android(target: AndroidTarget) -> Result<Self> {
        Ok(Self {
            inner: AccessibilityReaderImpl::Android(
                crate::platform::android::AndroidAccessibility::new(target.serial())?,
            ),
            target: Target::Android(target),
        })
    }

    /// Get the explicit target.
    pub fn target(&self) -> &Target {
        &self.target
    }
}

// Convenience methods that automatically use the stored target.
impl TargetedAccessibility {
    /// Snapshot the accessibility tree for the target process.
    ///
    /// Uses the stored target automatically.
    pub async fn get_tree(&mut self, filter: &TreeFilter) -> Result<ElementTree> {
        dispatch_mut_async!(self, get_tree, &self.target, filter)
    }

    /// Capture a screenshot of the target window.
    ///
    /// Uses the stored target automatically.
    pub fn capture_screen(&self) -> Result<Screenshot> {
        dispatch!(self, capture_screen, &self.target)
    }

    /// Get the bounds of the target window.
    ///
    /// Uses the stored target automatically.
    pub async fn get_screen_bounds(&self) -> Result<Rect> {
        match &self.inner {
            #[cfg(target_os = "macos")]
            AccessibilityReaderImpl::MacOS(r) => {
                AccessibilityReader::get_screen_bounds(r, &self.target).await
            }
            #[cfg(target_os = "macos")]
            AccessibilityReaderImpl::IOSSimulator(r) => {
                AccessibilityReader::get_screen_bounds(r, &self.target).await
            }
            #[cfg(target_os = "windows")]
            AccessibilityReaderImpl::Windows(r) => {
                AccessibilityReader::get_screen_bounds(r, &self.target).await
            }
            #[cfg(target_os = "linux")]
            AccessibilityReaderImpl::Linux(r) => {
                AccessibilityReader::get_screen_bounds(r, &self.target).await
            }
            AccessibilityReaderImpl::Android(r) => {
                AccessibilityReader::get_screen_bounds(r, &self.target).await
            }
        }
    }

    /// Send a keystroke to the target process.
    ///
    /// Uses the stored target automatically.
    pub async fn keystroke(&mut self, key: Code, modifiers: Modifiers) -> Result<()> {
        dispatch_mut_async!(self, keystroke, &self.target, key, modifiers)
    }

    /// Type raw text to the target process.
    ///
    /// Uses the stored target automatically.
    pub async fn type_raw(&mut self, text: &str) -> Result<()> {
        dispatch_mut_async!(self, type_raw, &self.target, text)
    }

    /// Click mouse at coordinates (targeted to process where supported).
    ///
    /// Uses the stored target automatically.
    pub async fn mouse_click_at(&mut self, x: f64, y: f64, button: MouseButton) -> Result<()> {
        dispatch_mut_async!(self, mouse_click_at, &self.target, x, y, button)
    }

    /// Press a key down (without releasing).
    ///
    /// Uses the stored target automatically.
    pub async fn press_key(&mut self, key: Code) -> Result<()> {
        dispatch_mut_async!(self, press_key, &self.target, key)
    }

    /// Release a previously pressed key.
    ///
    /// Uses the stored target automatically.
    pub async fn release_key(&mut self, key: Code) -> Result<()> {
        dispatch_mut_async!(self, release_key, &self.target, key)
    }

    /// Move the mouse to absolute screen coordinates.
    ///
    /// Uses the stored target automatically.
    pub async fn mouse_move(&mut self, x: f64, y: f64) -> Result<()> {
        dispatch_mut_async!(self, mouse_move, &self.target, x, y)
    }

    /// Click a mouse button at the current position.
    ///
    /// Uses the stored target automatically.
    pub async fn mouse_click(&mut self, button: MouseButton) -> Result<()> {
        dispatch_mut_async!(self, mouse_click, &self.target, button)
    }

    /// Double-click a mouse button at the current position.
    ///
    /// Uses the stored target automatically.
    pub async fn mouse_double_click(&mut self, button: MouseButton) -> Result<()> {
        dispatch_mut_async!(self, mouse_double_click, &self.target, button)
    }

    /// Scroll the mouse wheel.
    ///
    /// Uses the stored target automatically.
    pub async fn mouse_scroll(&mut self, delta_x: f64, delta_y: f64) -> Result<()> {
        dispatch_mut_async!(self, mouse_scroll, &self.target, delta_x, delta_y)
    }
}

// Delegation methods (pass-through to inner reader)
impl TargetedAccessibility {
    /// Get a cached element by ID.
    pub fn get_element(&self, id: ElementKey) -> Option<&Element> {
        dispatch!(self, get_element, id)
    }

    /// Perform an action on an element.
    pub async fn perform_action(&mut self, id: ElementKey, action: Action) -> Result<()> {
        dispatch_mut_async!(self, perform_action, id, action)
    }

    /// Set the value of an element.
    pub async fn set_value(&mut self, id: ElementKey, value: &str) -> Result<()> {
        dispatch_mut_async!(self, set_value, id, value)
    }

    /// Hit test at coordinates.
    pub async fn hit_test(&mut self, x: f64, y: f64) -> Result<Option<ElementKey>> {
        dispatch_mut_async!(self, hit_test, x, y)
    }

    /// Clear the element cache.
    pub fn clear_cache(&mut self) {
        dispatch_mut!(self, clear_cache)
    }

    /// Get the snapshot version.
    pub fn snapshot_version(&self) -> u64 {
        dispatch!(self, snapshot_version)
    }

    /// Get the platform name.
    pub fn platform_name(&self) -> &'static str {
        dispatch!(self, platform_name)
    }

    // Capability checks

    /// Returns true if this platform supports keystroke injection.
    pub fn supports_keystroke(&self) -> bool {
        dispatch!(self, supports_keystroke)
    }

    /// Returns true if this platform supports mouse click injection.
    pub fn supports_mouse_click(&self) -> bool {
        dispatch!(self, supports_mouse_click)
    }

    /// Returns true if this platform supports hit testing.
    pub fn supports_hit_test(&self) -> bool {
        dispatch!(self, supports_hit_test)
    }

    /// Returns true if this platform supports terminal display (viuer).
    pub fn supports_terminal_display(&self) -> bool {
        dispatch!(self, supports_terminal_display)
    }

    /// Check if this platform supports event listening.
    pub fn supports_event_listening(&self) -> bool {
        dispatch!(self, supports_event_listening)
    }

    /// Get the list of event types supported on this platform.
    pub fn supported_event_types(&self) -> Vec<AccessibilityEventType> {
        dispatch!(self, supported_event_types)
    }

    /// List all windows/applications.
    ///
    /// Returns a list of (pid, app_name, window_title, is_focused) for each window.
    pub async fn list_windows(&self) -> Vec<(u32, String, String, bool)> {
        match &self.inner {
            #[cfg(target_os = "macos")]
            AccessibilityReaderImpl::MacOS(_) => {
                crate::platform::macos::MacOSAccessibility::list_windows()
            }
            #[cfg(target_os = "macos")]
            AccessibilityReaderImpl::IOSSimulator(_) => {
                // iOS Simulator doesn't have a list_windows concept
                Vec::new()
            }
            #[cfg(target_os = "windows")]
            AccessibilityReaderImpl::Windows(r) => r.list_windows(),
            #[cfg(target_os = "linux")]
            AccessibilityReaderImpl::Linux(r) => r.list_windows().await,
            AccessibilityReaderImpl::Android(_) => {
                // Android doesn't have a list_windows concept
                Vec::new()
            }
        }
    }

    /// Start listening for events.
    ///
    /// Uses `ListenerConfig::pid` when set, otherwise uses the stored target.
    pub fn start_listening(
        &mut self,
        mut config: ListenerConfig,
        callback: Box<dyn FnMut(AccessibilityEvent) + Send + 'static>,
    ) -> Result<ListenerHandle> {
        if config.pid.is_none()
            && let Target::Pid(pid) = &self.target
        {
            config.pid = Some(*pid);
        }
        dispatch_mut!(self, start_listening, config, callback)
    }
}

impl TargetedAccessibility {
    /// Parse and send a keystroke specification like "cmd+c" or "enter".
    ///
    /// Returns the parsed key code and modifiers on success.
    ///
    /// # Examples
    /// ```ignore
    /// adapter.send_keystroke("enter").await?;
    /// adapter.send_keystroke("cmd+c").await?;
    /// adapter.send_keystroke("ctrl+shift+a").await?;
    /// ```
    pub async fn send_keystroke(&mut self, spec: &str) -> Result<(Code, Modifiers)> {
        let parts: Vec<&str> = spec.split('+').collect();

        if parts.is_empty() {
            anyhow::bail!("Empty keystroke specification");
        }

        // Last part is the key
        let key_str = parts[parts.len() - 1];
        let key =
            parse_key_code(key_str).ok_or_else(|| anyhow::anyhow!("Unknown key: {}", key_str))?;

        // Rest are modifiers
        let mut modifiers = Modifiers::empty();
        for part in &parts[..parts.len() - 1] {
            modifiers |= parse_modifiers(part);
        }

        self.keystroke(key, modifiers).await?;
        Ok((key, modifiers))
    }

    /// Resolve an element by ID string or CSS-like query.
    ///
    /// If `strict` is true, returns an error when multiple matches are found.
    /// If `strict` is false, returns the first match and logs the others.
    ///
    /// # Arguments
    /// * `target` - Either a numeric ID (e.g., "42") or a CSS-like query (e.g., "Button[title=Save]")
    /// * `tree` - The element tree to search in
    /// * `strict` - If true, errors when multiple matches found
    ///
    /// # Examples
    /// ```ignore
    /// let elem = adapter.resolve_element("42", &tree, true)?;
    /// let elem = adapter.resolve_element("Button[title=Save]", &tree, false)?;
    /// ```
    pub fn resolve_element<'a>(
        &self,
        target: &str,
        tree: &'a ElementTree,
        strict: bool,
    ) -> Result<&'a Element> {
        // Try to parse as ID first
        if let Ok(ffi_id) = target.parse::<u64>() {
            let key = ElementKey::from_ffi(ffi_id);
            let matches = tree.find_all(|e| e.id == key);
            return matches
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("Element with ID {} not found", ffi_id));
        }

        // Otherwise treat as query
        let selector = parse_query(target)
            .map_err(|e| anyhow::anyhow!("Invalid query '{}': {}", target, e))?;
        let matches = find_matches(&selector, tree);

        if matches.is_empty() {
            anyhow::bail!("No elements match query: {}", target);
        }

        if matches.len() > 1 && strict {
            let mut msg = format!(
                "Query '{}' matched {} elements (expected exactly 1):\n",
                target,
                matches.len()
            );
            for (i, elem) in matches.iter().take(5).enumerate() {
                msg.push_str(&format!(
                    "  {}: [{}] {:?} \"{}\"\n",
                    i + 1,
                    elem.id,
                    elem.role,
                    elem.display_label()
                ));
            }
            if matches.len() > 5 {
                msg.push_str(&format!("  ... and {} more\n", matches.len() - 5));
            }
            msg.push_str("Use a more specific query or specify an element ID directly.");
            anyhow::bail!(msg);
        }

        Ok(matches[0])
    }

    /// Find elements matching a query, optionally requiring bounds.
    ///
    /// If `query` is `None`, returns all interactive elements.
    /// If `require_bounds` is true, only returns elements with bounds defined.
    ///
    /// # Arguments
    /// * `tree` - The element tree to search in
    /// * `query` - Optional CSS-like query (None returns interactive elements)
    /// * `require_bounds` - If true, only return elements with bounds
    pub fn find_elements<'a>(
        &self,
        tree: &'a ElementTree,
        query: Option<&str>,
        require_bounds: bool,
    ) -> Result<Vec<&'a Element>> {
        if let Some(q) = query {
            let selector =
                parse_query(q).map_err(|e| anyhow::anyhow!("Invalid query '{}': {}", q, e))?;
            let matches = find_matches(&selector, tree);
            if require_bounds {
                Ok(matches.into_iter().filter(|e| e.bounds.is_some()).collect())
            } else {
                Ok(matches)
            }
        } else {
            let elements =
                tree.find_all(|e| e.is_interactive() && (!require_bounds || e.bounds.is_some()));
            Ok(elements)
        }
    }

    /// Click an element by ID or query.
    ///
    /// Returns the clicked element's ID on success.
    pub async fn click_element(&mut self, target: &str, tree: &ElementTree) -> Result<ElementKey> {
        let elem = self.resolve_element(target, tree, true)?;
        self.click_resolved_element(elem).await
    }

    /// Click a previously resolved element.
    ///
    /// Element clicks use the element's native accessibility action.
    ///
    /// On macOS this is the no-focus path for AX-addressable controls. Use
    /// `mouse_click_at` for explicit pixel clicks on surfaces without useful AX
    /// actions.
    pub async fn click_resolved_element(&mut self, elem: &Element) -> Result<ElementKey> {
        let id = elem.id;
        self.perform_action(id, Action::Click).await?;
        Ok(id)
    }

    /// Focus an element by ID or query.
    ///
    /// Returns the focused element's ID on success.
    pub async fn focus_element(&mut self, target: &str, tree: &ElementTree) -> Result<ElementKey> {
        let elem = self.resolve_element(target, tree, true)?;
        let id = elem.id;
        self.perform_action(id, Action::Focus).await?;
        Ok(id)
    }

    /// Blur (remove focus from) an element by ID or query.
    ///
    /// Returns the blurred element's ID on success.
    pub async fn blur_element(&mut self, target: &str, tree: &ElementTree) -> Result<ElementKey> {
        let elem = self.resolve_element(target, tree, true)?;
        let id = elem.id;
        self.perform_action(id, Action::Blur).await?;
        Ok(id)
    }

    /// Set value on an element by ID or query.
    ///
    /// Returns the element's ID on success.
    pub async fn set_element_value(
        &mut self,
        target: &str,
        value: &str,
        tree: &ElementTree,
    ) -> Result<ElementKey> {
        let elem = self.resolve_element(target, tree, true)?;
        let id = elem.id;
        self.set_value(id, value).await?;
        Ok(id)
    }
}
