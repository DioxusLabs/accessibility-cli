//! Linux accessibility implementation using AT-SPI via D-Bus.
//!
//! This module provides access to the Linux AT-SPI2 accessibility tree
//! for reading UI element information and performing actions.

use crate::accessibility::{
    AccessibilityEvent, AccessibilityEventType, AccessibilityReader, Element, ElementCache,
    ElementKey, ElementTree, ListenerConfig, ListenerHandle, Point, Rect, Screenshot, Size,
    StopReason, StructureChangeType, Target, TreeFilter,
};
use accessibility_linux_sys::atspi::proxy::accessible::AccessibleProxy;
use accessibility_linux_sys::atspi::proxy::action::ActionProxy;
use accessibility_linux_sys::atspi::proxy::component::ComponentProxy;
use accessibility_linux_sys::atspi::proxy::editable_text::EditableTextProxy;
use accessibility_linux_sys::atspi::proxy::text::TextProxy;
use accessibility_linux_sys::atspi::proxy::value::ValueProxy;
use accessibility_linux_sys::atspi::{
    InterfaceSet, Role as AtspiRole, connection::AccessibilityConnection,
};
use accessibility_linux_sys::atspi_common::CoordType;
use accessibility_linux_sys::zbus::fdo::DBusProxy;
use accessibility_linux_sys::zbus::proxy::CacheProperties;
use accessibility_linux_sys::{atspi, x11rb, zbus};
use accesskit::{Action, Role};
use anyhow::{Result, anyhow, bail};
use slotmap::SecondaryMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

/// Macro to generate D-Bus proxy factory functions with consistent error handling.
macro_rules! create_proxy_fn {
    ($fn_name:ident, $proxy_type:ident) => {
        async fn $fn_name<'a>(
            conn: &'a zbus::Connection,
            bus_name: &'a str,
            object_path: &'a str,
        ) -> Result<$proxy_type<'a>> {
            $proxy_type::builder(conn)
                .destination(bus_name)?
                .path(object_path)?
                .build()
                .await
                .map_err(|e| anyhow!("Failed to create {}: {}", stringify!($proxy_type), e))
        }
    };
}

/// Native handle for AT-SPI accessible objects.
///
/// Stores the D-Bus bus name and object path needed to recreate proxies.
#[derive(Clone, Debug)]
struct NativeHandle {
    bus_name: String,
    object_path: String,
}

/// Linux accessibility reader using AT-SPI via D-Bus.
pub struct LinuxAccessibility {
    /// Cache of elements with their platform handles.
    cache: ElementCache,

    /// Map from ElementKey to native handle for performing actions.
    /// Uses SecondaryMap which is automatically synchronized with the primary SlotMap in cache.
    handles: SecondaryMap<ElementKey, NativeHandle>,

    /// AT-SPI connection to the accessibility bus.
    connection: AccessibilityConnection,
}

impl LinuxAccessibility {
    /// Create a new Linux accessibility reader.
    ///
    /// This must be called from within an async context (tokio runtime).
    /// Returns an error if AT-SPI bus is not available.
    pub async fn new() -> Result<Self> {
        let connection = AccessibilityConnection::new().await.map_err(|e| {
            anyhow!(
                "Failed to connect to AT-SPI bus. Is AT-SPI2 available? Error: {}",
                e
            )
        })?;

        Ok(Self {
            cache: ElementCache::new(),
            handles: SecondaryMap::new(),
            connection,
        })
    }

    /// Get the PID of a D-Bus bus name owner.
    async fn get_pid_for_bus_name(conn: &zbus::Connection, bus_name: &str) -> Option<u32> {
        use accessibility_linux_sys::zbus::names::BusName;
        let dbus_proxy = DBusProxy::new(conn).await.ok()?;
        let bus_name = BusName::try_from(bus_name).ok()?;
        dbus_proxy
            .get_connection_unix_process_id(bus_name)
            .await
            .ok()
    }

    /// Create an AccessibleProxy from bus name and object path.
    async fn create_accessible_proxy<'a>(
        conn: &'a zbus::Connection,
        bus_name: &'a str,
        object_path: &'a str,
    ) -> Result<AccessibleProxy<'a>> {
        AccessibleProxy::builder(conn)
            .destination(bus_name)?
            .path(object_path)?
            // Disable property caching - AT-SPI objects may have incomplete D-Bus interfaces
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .map_err(|e| anyhow!("Failed to create AccessibleProxy: {}", e))
    }

    /// Create a TextProxy from bus name and object path (needs custom cache settings).
    async fn create_text_proxy<'a>(
        conn: &'a zbus::Connection,
        bus_name: &'a str,
        object_path: &'a str,
    ) -> Result<TextProxy<'a>> {
        TextProxy::builder(conn)
            .destination(bus_name)?
            .path(object_path)?
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .map_err(|e| anyhow!("Failed to create TextProxy: {}", e))
    }

    create_proxy_fn!(create_component_proxy, ComponentProxy);
    create_proxy_fn!(create_action_proxy, ActionProxy);
    create_proxy_fn!(create_editable_text_proxy, EditableTextProxy);
    create_proxy_fn!(create_value_proxy, ValueProxy);

    /// Map AT-SPI Role to accesskit Role.
    fn map_role(atspi_role: AtspiRole) -> Role {
        match atspi_role {
            AtspiRole::Button | AtspiRole::PushButtonMenu => Role::Button,
            AtspiRole::CheckBox => Role::CheckBox,
            AtspiRole::RadioButton => Role::RadioButton,
            AtspiRole::Entry | AtspiRole::PasswordText => Role::TextInput,
            AtspiRole::Text | AtspiRole::Terminal => Role::MultilineTextInput,
            AtspiRole::Label | AtspiRole::Static => Role::Label,
            AtspiRole::ComboBox => Role::ComboBox,
            AtspiRole::Slider | AtspiRole::SpinButton => Role::Slider,
            AtspiRole::Menu | AtspiRole::PopupMenu => Role::Menu,
            AtspiRole::MenuItem => Role::MenuItem,
            AtspiRole::CheckMenuItem => Role::MenuItemCheckBox,
            AtspiRole::RadioMenuItem => Role::MenuItemRadio,
            AtspiRole::List | AtspiRole::ListBox => Role::List,
            AtspiRole::ListItem | AtspiRole::TreeItem => Role::ListItem,
            AtspiRole::Table => Role::Table,
            AtspiRole::TableCell | AtspiRole::TableRow => Role::Cell,
            AtspiRole::Link => Role::Link,
            AtspiRole::Image | AtspiRole::Icon => Role::Image,
            AtspiRole::Window | AtspiRole::Frame | AtspiRole::Dialog => Role::Window,
            AtspiRole::ToolBar => Role::Toolbar,
            AtspiRole::MenuBar => Role::MenuBar,
            AtspiRole::ScrollBar => Role::ScrollBar,
            AtspiRole::ScrollPane => Role::ScrollView,
            AtspiRole::StatusBar => Role::Tooltip,
            AtspiRole::Panel | AtspiRole::Filler => Role::Group,
            AtspiRole::Application => Role::Application,
            AtspiRole::DocumentFrame | AtspiRole::DocumentWeb => Role::WebView,
            AtspiRole::PageTabList => Role::TabList,
            AtspiRole::PageTab => Role::Tab,
            AtspiRole::ProgressBar => Role::ProgressIndicator,
            AtspiRole::Separator => Role::Splitter,
            AtspiRole::Tree | AtspiRole::TreeTable => Role::Tree,
            AtspiRole::ToggleButton => Role::Switch,
            AtspiRole::Heading => Role::Heading,
            AtspiRole::Paragraph => Role::Paragraph,
            AtspiRole::Form => Role::Form,
            AtspiRole::Alert => Role::Alert,
            AtspiRole::Canvas => Role::Canvas,
            AtspiRole::Animation => Role::Figure,
            AtspiRole::ColumnHeader | AtspiRole::TableColumnHeader => Role::ColumnHeader,
            AtspiRole::RowHeader | AtspiRole::TableRowHeader => Role::RowHeader,
            AtspiRole::DesktopFrame | AtspiRole::DesktopIcon => Role::Unknown,
            _ => Role::Unknown,
        }
    }

    /// Build an Element from an AT-SPI accessible proxy (non-recursive helper).
    async fn build_single_element(
        conn: &zbus::Connection,
        proxy: &AccessibleProxy<'_>,
        handle: &NativeHandle,
        interfaces: InterfaceSet,
        id: ElementKey,
    ) -> Option<Element> {
        // Get role
        let atspi_role = proxy.get_role().await.ok()?;
        let role = Self::map_role(atspi_role);

        // Build element
        let mut element = Element::new(id, role);

        // Get basic properties
        element.title = proxy.name().await.ok().filter(|s| !s.is_empty());
        element.description = proxy.description().await.ok().filter(|s| !s.is_empty());
        element.help = proxy.help_text().await.ok().filter(|s| !s.is_empty());
        element.identifier = proxy.accessible_id().await.ok().filter(|s| !s.is_empty());
        element.role_description = proxy.get_localized_role_name().await.ok();

        // Get states
        if let Ok(states) = proxy.get_state().await {
            element.enabled =
                !states.contains(atspi::State::Sensitive) || states.contains(atspi::State::Enabled);
            element.focused = states.contains(atspi::State::Focused);
        }

        // Get bounds from Component interface if available
        if interfaces.contains(atspi::Interface::Component)
            && let Ok(component) =
                Self::create_component_proxy(conn, &handle.bus_name, &handle.object_path).await
            && let Ok((x, y, width, height)) = component.get_extents(CoordType::Screen).await
        {
            element.bounds = Some(Rect::new(
                Point::new(x as f64, y as f64),
                Size::new(width as f64, height as f64),
            ));
        }

        // Get actions from Action interface if available
        // NOTE: Some older GTK applications (like Ubuntu 20.04's gnome-calculator) have
        // a buggy GetActions implementation that crashes when called. We use NActions
        // and GetName instead which work correctly.
        if interfaces.contains(atspi::Interface::Action)
            && let Ok(action_proxy) =
                Self::create_action_proxy(conn, &handle.bus_name, &handle.object_path).await
            && let Ok(n_actions) = action_proxy.nactions().await
        {
            // Use nactions + get_name instead of get_actions for compatibility
            let mut actions = Vec::new();
            for i in 0..n_actions {
                if let Ok(name) = action_proxy.get_name(i).await {
                    actions.push(name);
                }
            }
            element.actions = actions;
        }

        // Get value if Value interface is available
        if interfaces.contains(atspi::Interface::Value)
            && let Ok(value_proxy) =
                Self::create_value_proxy(conn, &handle.bus_name, &handle.object_path).await
            && let Ok(value) = value_proxy.current_value().await
        {
            element.value = Some(value.to_string());
        }

        // Get text content if Text interface is available (for text inputs)
        // Only read if we don't already have a value from Value interface
        if element.value.is_none()
            && interfaces.contains(atspi::Interface::Text)
            && let Ok(text_proxy) =
                Self::create_text_proxy(conn, &handle.bus_name, &handle.object_path).await
            && let Ok(char_count) = text_proxy.character_count().await
            && char_count > 0
            && let Ok(text) = text_proxy.get_text(0, char_count).await
            && !text.is_empty()
        {
            element.value = Some(text);
        }

        Some(element)
    }

    /// Build the accessibility tree iteratively using a stack.
    async fn build_tree_async(
        &mut self,
        root_handle: NativeHandle,
        filter: &TreeFilter,
    ) -> Option<Element> {
        // Temporary ID type used within async block (just an index, not a real slotmap ID)
        type TempId = u32;

        // Stack entry: (handle, parent_temp_id, depth)
        // parent_temp_id is None for the root element
        struct StackEntry {
            handle: NativeHandle,
            interfaces: InterfaceSet,
            parent_temp_id: Option<TempId>,
            depth: usize,
        }

        // Clone the connection for use in async block
        let conn = self.connection.connection().clone();

        // Collect results and handles in async block using temporary IDs
        let async_result: Option<_> = async {
            // Use temporary indices that will be remapped to real slotmap IDs later
            let mut results: HashMap<TempId, (Element, Option<TempId>, usize)> = HashMap::new();
            let mut handles_to_insert: Vec<(TempId, NativeHandle)> = Vec::new();
            let mut element_count = 0usize;
            let mut root_temp_id: Option<TempId> = None;
            let mut next_temp_id: TempId = 1;

            // Initialize stack with root
            let root_proxy = Self::create_accessible_proxy(
                &conn,
                &root_handle.bus_name,
                &root_handle.object_path,
            )
            .await
            .ok()?;
            let root_interfaces = root_proxy.get_interfaces().await.ok()?;

            let mut stack = vec![StackEntry {
                handle: root_handle,
                interfaces: root_interfaces,
                parent_temp_id: None,
                depth: 0,
            }];

            while let Some(entry) = stack.pop() {
                // Check element count limit
                if let Some(max) = filter.max_elements
                    && element_count >= max
                {
                    continue;
                }

                // Allocate temporary ID (will be remapped later)
                let temp_id = next_temp_id;
                next_temp_id += 1;
                if root_temp_id.is_none() {
                    root_temp_id = Some(temp_id);
                }

                // Create proxy for this element
                let proxy = match Self::create_accessible_proxy(
                    &conn,
                    &entry.handle.bus_name,
                    &entry.handle.object_path,
                )
                .await
                {
                    Ok(p) => p,
                    Err(_) => continue,
                };

                // Build element with a placeholder ID (will be replaced with real ID later)
                let placeholder_key = ElementKey::from_ffi(1);
                let element = match Self::build_single_element(
                    &conn,
                    &proxy,
                    &entry.handle,
                    entry.interfaces,
                    placeholder_key,
                )
                .await
                {
                    Some(e) => e,
                    None => continue,
                };

                // Store handle for later insertion
                handles_to_insert.push((temp_id, entry.handle.clone()));

                // Get children if we should recurse
                let should_recurse = filter.max_depth.is_none_or(|max| entry.depth < max);
                if should_recurse && let Ok(children) = proxy.get_children().await {
                    // Push children to stack in reverse order so first child is processed first
                    for child_ref in children.into_iter().rev() {
                        let child_handle = NativeHandle {
                            bus_name: child_ref.name_as_str().unwrap_or_default().to_string(),
                            object_path: child_ref.path_as_str().to_string(),
                        };

                        if let Ok(child_proxy) = Self::create_accessible_proxy(
                            &conn,
                            &child_handle.bus_name,
                            &child_handle.object_path,
                        )
                        .await
                            && let Ok(child_interfaces) = child_proxy.get_interfaces().await
                        {
                            stack.push(StackEntry {
                                handle: child_handle,
                                interfaces: child_interfaces,
                                parent_temp_id: Some(temp_id),
                                depth: entry.depth + 1,
                            });
                        }
                    }
                }

                // Store result with depth for later processing
                results.insert(temp_id, (element, entry.parent_temp_id, entry.depth));
                element_count += 1;
            }

            Some((results, handles_to_insert, root_temp_id))
        }
        .await;

        let (mut results, handles_to_insert, root_temp_id) = async_result?;
        let root_temp_id = root_temp_id?;

        // Build temp_id -> handle mapping for later use
        let handles_map: HashMap<TempId, NativeHandle> = handles_to_insert.into_iter().collect();

        // Build children map using temporary IDs
        let mut children_map: HashMap<TempId, Vec<TempId>> = HashMap::new();
        for (&temp_id, (_, parent_temp_id, _)) in &results {
            if let Some(pid) = parent_temp_id {
                children_map.entry(*pid).or_default().push(temp_id);
            }
        }

        // Build tree recursively, storing elements and handles as we go
        fn build_tree_from_results(
            temp_id: u32,
            results: &mut HashMap<u32, (Element, Option<u32>, usize)>,
            children_map: &HashMap<u32, Vec<u32>>,
            handles_map: &HashMap<u32, NativeHandle>,
            filter: &TreeFilter,
            cache: &mut ElementCache,
            handles: &mut SecondaryMap<ElementKey, NativeHandle>,
        ) -> Option<Element> {
            let (element, _, depth) = results.remove(&temp_id)?;

            // Recursively build children first
            let mut children_elements = Vec::new();
            if let Some(child_temp_ids) = children_map.get(&temp_id) {
                for &child_temp_id in child_temp_ids {
                    if let Some(child) = build_tree_from_results(
                        child_temp_id,
                        results,
                        children_map,
                        handles_map,
                        filter,
                        cache,
                        handles,
                    ) {
                        children_elements.push(child);
                    }
                }
            }

            // Check if element passes filter or has children
            let passes_filter = filter.should_include(&element, depth);
            if !passes_filter && children_elements.is_empty() {
                return None;
            }

            // Store in cache with the final ID
            let (id, stored_element) = cache.store_with_clone(|id| Element {
                id,
                role: element.role,
                title: element.title,
                description: element.description,
                value: element.value,
                url: element.url,
                help: element.help,
                role_description: element.role_description,
                identifier: element.identifier,
                bounds: element.bounds,
                enabled: element.enabled,
                focused: element.focused,
                actions: element.actions,
                children: children_elements,
            });

            // Store the handle with the real ID
            if let Some(handle) = handles_map.get(&temp_id) {
                handles.insert(id, handle.clone());
            }

            Some(stored_element)
        }

        build_tree_from_results(
            root_temp_id,
            &mut results,
            &children_map,
            &handles_map,
            filter,
            &mut self.cache,
            &mut self.handles,
        )
    }

    /// Find the target application by PID.
    async fn find_app_by_pid(
        conn: &zbus::Connection,
        root: &AccessibleProxy<'_>,
        target_pid: u32,
    ) -> Option<(NativeHandle, u32)> {
        let children = root.get_children().await.ok()?;

        for child_ref in children {
            let bus_name = child_ref.name_as_str().unwrap_or_default().to_string();

            // Get PID from D-Bus
            if let Some(pid) = Self::get_pid_for_bus_name(conn, &bus_name).await
                && pid == target_pid
            {
                return Some((
                    NativeHandle {
                        bus_name,
                        object_path: child_ref.path_as_str().to_string(),
                    },
                    pid,
                ));
            }
        }

        None
    }

    /// Find the focused application.
    async fn find_focused_app(
        conn: &zbus::Connection,
        root: &AccessibleProxy<'_>,
    ) -> Option<(NativeHandle, u32)> {
        let children = root.get_children().await.ok()?;

        // First pass: look for focused/active application
        for child_ref in &children {
            let handle = NativeHandle {
                bus_name: child_ref.name_as_str().unwrap_or_default().to_string(),
                object_path: child_ref.path_as_str().to_string(),
            };

            if let Ok(proxy) =
                Self::create_accessible_proxy(conn, &handle.bus_name, &handle.object_path).await
                && let Ok(states) = proxy.get_state().await
            {
                // Check for Active or Focused state
                if states.contains(atspi::State::Active) || states.contains(atspi::State::Focused) {
                    let pid = Self::get_pid_for_bus_name(conn, &handle.bus_name)
                        .await
                        .unwrap_or(0);
                    return Some((handle, pid));
                }
            }
        }

        // Fallback: return first application with a valid PID
        for child_ref in &children {
            let bus_name = child_ref.name_as_str().unwrap_or_default().to_string();
            if let Some(pid) = Self::get_pid_for_bus_name(conn, &bus_name).await
                && pid > 0
            {
                return Some((
                    NativeHandle {
                        bus_name,
                        object_path: child_ref.path_as_str().to_string(),
                    },
                    pid,
                ));
            }
        }

        None
    }

    // Screenshot Support (not yet implemented on Linux)

    /// Capture the entire screen.
    ///
    /// Not yet implemented on Linux.
    pub fn capture_screen(&self) -> Result<crate::accessibility::Screenshot> {
        bail!("Screenshot capture is not yet implemented on Linux")
    }

    /// Capture a specific window by PID.
    ///
    /// Not yet implemented on Linux.
    pub fn capture_window(&self, _pid: u32) -> Result<crate::accessibility::Screenshot> {
        bail!("Screenshot capture is not yet implemented on Linux")
    }

    /// List all accessible applications with their PIDs.
    ///
    /// Returns a list of (pid, app_name, window_title, is_focused) for each application.
    pub async fn list_windows(&self) -> Vec<(u32, String, String, bool)> {
        let mut windows = Vec::new();
        let conn = self.connection.connection();

        let root = match self.connection.root_accessible_on_registry().await {
            Ok(r) => r,
            Err(_) => return windows,
        };

        let children = match root.get_children().await {
            Ok(c) => c,
            Err(_) => return windows,
        };

        // First, find the focused app to determine is_focused
        let focused_pid = Self::find_focused_app(conn, &root)
            .await
            .map(|(_, pid)| pid);

        for child_ref in children {
            let bus_name = child_ref.name_as_str().unwrap_or_default().to_string();
            let object_path = child_ref.path_as_str().to_string();

            // Get PID
            let pid = match Self::get_pid_for_bus_name(conn, &bus_name).await {
                Some(p) if p > 0 => p,
                _ => continue,
            };

            // Get app name via proxy
            let app_name = if let Ok(proxy) =
                Self::create_accessible_proxy(conn, &bus_name, &object_path).await
            {
                proxy.name().await.unwrap_or_else(|_| "Unknown".to_string())
            } else {
                "Unknown".to_string()
            };

            // Skip empty names
            if app_name.is_empty() || app_name == "Unknown" {
                continue;
            }

            let is_focused = focused_pid == Some(pid);

            // Use app_name as both app_name and title (AT-SPI doesn't always have window titles)
            windows.push((pid, app_name.clone(), app_name, is_focused));
        }

        windows
    }

    /// Get window bounds via AT-SPI Component interface.
    ///
    /// This works on both Wayland and X11.
    pub async fn get_window_bounds_for_pid_via_atspi(&self, target_pid: u32) -> Option<Rect> {
        let conn = self.connection.connection();
        let root = self.connection.root_accessible_on_registry().await.ok()?;
        let children = root.get_children().await.ok()?;

        for child_ref in children {
            let bus_name = child_ref.name_as_str().unwrap_or_default().to_string();

            // Check PID
            if let Some(pid) = Self::get_pid_for_bus_name(conn, &bus_name).await
                && pid == target_pid
            {
                // Get the application's first window with bounds
                let handle = NativeHandle {
                    bus_name: bus_name.clone(),
                    object_path: child_ref.path_as_str().to_string(),
                };

                if let Ok(proxy) =
                    Self::create_accessible_proxy(conn, &handle.bus_name, &handle.object_path).await
                {
                    // Try to find a window child with bounds
                    if let Ok(app_children) = proxy.get_children().await {
                        for win_ref in app_children {
                            let win_handle = NativeHandle {
                                bus_name: win_ref.name_as_str().unwrap_or_default().to_string(),
                                object_path: win_ref.path_as_str().to_string(),
                            };

                            if let Ok(component) = Self::create_component_proxy(
                                conn,
                                &win_handle.bus_name,
                                &win_handle.object_path,
                            )
                            .await
                                && let Ok((x, y, width, height)) =
                                    component.get_extents(CoordType::Screen).await
                                && width > 0
                                && height > 0
                            {
                                return Some(Rect::new(
                                    Point::new(x as f64, y as f64),
                                    Size::new(width as f64, height as f64),
                                ));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Get the bounds of the screen.
    ///
    /// Returns screen coordinates and dimensions.
    pub fn get_global_screen_bounds() -> Result<Rect> {
        use accessibility_linux_sys::x11rb::connection::Connection;

        // Connect to X11 display
        let (conn, screen_num) =
            x11rb::connect(None).map_err(|e| anyhow!("Failed to connect to X11: {}", e))?;

        let screen = &conn.setup().roots[screen_num];

        Ok(Rect::new(
            Point::new(0.0, 0.0),
            Size::new(
                screen.width_in_pixels as f64,
                screen.height_in_pixels as f64,
            ),
        ))
    }

    /// Get window bounds for a specific PID.
    ///
    /// Searches through X11 windows to find one matching the PID.
    pub fn get_window_bounds_for_pid(pid: u32) -> Option<Rect> {
        use accessibility_linux_sys::x11rb::connection::Connection;
        use accessibility_linux_sys::x11rb::protocol::xproto::ConnectionExt as _;

        let (conn, screen_num) = x11rb::connect(None).ok()?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;

        // Get _NET_WM_PID atom
        let pid_atom = conn
            .intern_atom(false, b"_NET_WM_PID")
            .ok()?
            .reply()
            .ok()?
            .atom;

        // Search through all windows
        Self::find_window_by_pid_recursive(&conn, root, pid_atom, pid)
    }

    /// Recursively search for a window with matching PID.
    fn find_window_by_pid_recursive(
        conn: &impl x11rb::connection::Connection,
        window: u32,
        pid_atom: u32,
        target_pid: u32,
    ) -> Option<Rect> {
        use accessibility_linux_sys::x11rb::protocol::xproto::ConnectionExt as _;

        // Check if this window has the target PID
        if let Ok(reply) = conn
            .get_property(
                false,
                window,
                pid_atom,
                x11rb::protocol::xproto::AtomEnum::CARDINAL,
                0,
                1,
            )
            .ok()?
            .reply()
            && reply.value_len == 1
            && reply.format == 32
        {
            let window_pid = u32::from_ne_bytes([
                reply.value[0],
                reply.value[1],
                reply.value[2],
                reply.value[3],
            ]);
            if window_pid == target_pid {
                // Get window geometry
                if let Ok(geom) = conn.get_geometry(window).ok()?.reply() {
                    // Translate coordinates to root window
                    if let Ok(trans) = conn
                        .translate_coordinates(window, conn.setup().roots[0].root, 0, 0)
                        .ok()?
                        .reply()
                    {
                        return Some(Rect::new(
                            Point::new(trans.dst_x as f64, trans.dst_y as f64),
                            Size::new(geom.width as f64, geom.height as f64),
                        ));
                    }
                }
            }
        }

        // Check children
        if let Ok(reply) = conn.query_tree(window).ok()?.reply() {
            for child in reply.children {
                if let Some(rect) =
                    Self::find_window_by_pid_recursive(conn, child, pid_atom, target_pid)
                {
                    return Some(rect);
                }
            }
        }

        None
    }
}

impl AccessibilityReader for LinuxAccessibility {
    async fn get_tree(&mut self, target: &Target, filter: &TreeFilter) -> Result<ElementTree> {
        // Clear previous cache
        self.clear_cache();

        let version = self.cache.version();

        // Clone connection for use in async block
        let conn = self.connection.connection().clone();

        // First, find the target application
        let root = self
            .connection
            .root_accessible_on_registry()
            .await
            .map_err(|e| anyhow!("Failed to get root accessible: {}", e))?;

        // Find target application
        let target_pid = target.require_pid("Linux", "accessibility tree queries")?;
        let (app_handle, actual_pid) = Self::find_app_by_pid(&conn, &root, target_pid)
            .await
            .ok_or_else(|| anyhow!("Application with PID {} not found", target_pid))?;

        // Get app name
        let app_proxy =
            Self::create_accessible_proxy(&conn, &app_handle.bus_name, &app_handle.object_path)
                .await?;
        let app_name = app_proxy.name().await.ok();

        // Build the tree
        let root_element = self
            .build_tree_async(app_handle, filter)
            .await
            .ok_or_else(|| anyhow!("Failed to build accessibility tree"))?;

        let element_count = self.cache.len();

        Ok(ElementTree {
            version,
            pid: Some(actual_pid),
            app_name,
            root: root_element,
            element_count,
        })
    }

    fn get_element(&self, id: ElementKey) -> Option<&Element> {
        self.cache.get(id)
    }

    async fn perform_action(&mut self, id: ElementKey, action: Action) -> Result<()> {
        let handle = self
            .handles
            .get(id)
            .ok_or_else(|| anyhow!("Element {} not found in cache", id))?
            .clone();

        let conn = self.connection.connection().clone();

        // Handle Focus action specially using Component interface
        if action == Action::Focus {
            let component =
                Self::create_component_proxy(&conn, &handle.bus_name, &handle.object_path).await?;
            let success = component
                .grab_focus()
                .await
                .map_err(|e| anyhow!("Failed to grab focus: {}", e))?;
            if !success {
                bail!("Failed to grab focus on element");
            }
            return Ok(());
        }

        // Use Action interface for other actions
        let action_proxy =
            Self::create_action_proxy(&conn, &handle.bus_name, &handle.object_path).await?;

        // Map accesskit Action to AT-SPI action index
        let action_index = match action {
            Action::Click => 0, // Primary action is usually index 0
            Action::ShowContextMenu => {
                // Find "showContextMenu" or similar action by name
                let actions = action_proxy
                    .get_actions()
                    .await
                    .map_err(|e| anyhow!("Failed to get actions: {}", e))?;
                actions
                    .iter()
                    .position(|a| {
                        let name = a.name.to_lowercase();
                        name.contains("context") || name.contains("menu")
                    })
                    .map(|i| i as i32)
                    .unwrap_or(0)
            }
            Action::Increment => {
                // Find "increment" action
                let actions = action_proxy
                    .get_actions()
                    .await
                    .map_err(|e| anyhow!("Failed to get actions: {}", e))?;
                actions
                    .iter()
                    .position(|a| a.name.to_lowercase().contains("increment"))
                    .map(|i| i as i32)
                    .unwrap_or(-1)
            }
            Action::Decrement => {
                // Find "decrement" action
                let actions = action_proxy
                    .get_actions()
                    .await
                    .map_err(|e| anyhow!("Failed to get actions: {}", e))?;
                actions
                    .iter()
                    .position(|a| a.name.to_lowercase().contains("decrement"))
                    .map(|i| i as i32)
                    .unwrap_or(-1)
            }
            _ => bail!("Action {:?} not supported on Linux", action),
        };

        if action_index < 0 {
            bail!("Action {:?} not available on this element", action);
        }

        let success = action_proxy
            .do_action(action_index)
            .await
            .map_err(|e| anyhow!("Failed to perform action: {}", e))?;

        if !success {
            bail!("Action {:?} failed", action);
        }

        Ok(())
    }

    async fn set_value(&mut self, id: ElementKey, value: &str) -> Result<()> {
        let handle = self
            .handles
            .get(id)
            .ok_or_else(|| anyhow!("Element {} not found in cache", id))?
            .clone();

        let conn = self.connection.connection().clone();
        let value = value.to_string();

        // Try EditableText interface first (for text fields)
        if let Ok(editable) =
            Self::create_editable_text_proxy(&conn, &handle.bus_name, &handle.object_path).await
            && editable.set_text_contents(&value).await.is_ok()
        {
            return Ok(());
        }

        // Fallback to Value interface (for sliders, spin buttons)
        if let Ok(value_proxy) =
            Self::create_value_proxy(&conn, &handle.bus_name, &handle.object_path).await
            && let Ok(numeric_value) = value.parse::<f64>()
        {
            value_proxy
                .set_current_value(numeric_value)
                .await
                .map_err(|e| anyhow!("Failed to set value: {}", e))?;
            return Ok(());
        }

        bail!("Element does not support setting value")
    }

    async fn hit_test(&mut self, x: f64, y: f64) -> Result<Option<ElementKey>> {
        let conn = self.connection.connection().clone();

        // Get root accessible from registry
        let root = self
            .connection
            .root_accessible_on_registry()
            .await
            .map_err(|e| anyhow!("Failed to get root accessible: {}", e))?;

        // Get children (applications)
        let children = root
            .get_children()
            .await
            .map_err(|e| anyhow!("Failed to get children: {}", e))?;

        // Try each application's component interface
        for child_ref in children {
            let handle = NativeHandle {
                bus_name: child_ref.name_as_str().unwrap_or_default().to_string(),
                object_path: child_ref.path_as_str().to_string(),
            };

            if let Ok(component) =
                Self::create_component_proxy(&conn, &handle.bus_name, &handle.object_path).await
                && let Ok(accessible_ref) = component
                    .get_accessible_at_point(x as i32, y as i32, CoordType::Screen)
                    .await
            {
                // Check if we got a valid object (not null path)
                if accessible_ref.path_as_str() != "/org/a11y/atspi/null" {
                    let hit_handle = NativeHandle {
                        bus_name: accessible_ref.name_as_str().unwrap_or_default().to_string(),
                        object_path: accessible_ref.path_as_str().to_string(),
                    };

                    if let Ok(proxy) = Self::create_accessible_proxy(
                        &conn,
                        &hit_handle.bus_name,
                        &hit_handle.object_path,
                    )
                    .await
                        && let Ok(interfaces) = proxy.get_interfaces().await
                    {
                        // Build element with placeholder ID (will be assigned when stored)
                        let placeholder_key = ElementKey::from_ffi(1);
                        if let Some(element) = Self::build_single_element(
                            &conn,
                            &proxy,
                            &hit_handle,
                            interfaces,
                            placeholder_key,
                        )
                        .await
                        {
                            // Store in cache using store_with_clone to assign proper ID
                            let (id, _) = self.cache.store_with_clone(|id| Element {
                                id,
                                role: element.role,
                                title: element.title.clone(),
                                description: element.description.clone(),
                                value: element.value.clone(),
                                url: element.url.clone(),
                                help: element.help.clone(),
                                role_description: element.role_description.clone(),
                                identifier: element.identifier.clone(),
                                bounds: element.bounds,
                                enabled: element.enabled,
                                focused: element.focused,
                                actions: element.actions.clone(),
                                children: vec![], // hit_test returns a single element without children
                            });

                            // Store the handle
                            self.handles.insert(id, hit_handle);
                            return Ok(Some(id));
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    fn clear_cache(&mut self) {
        self.cache.clear();
        self.handles.clear();
    }

    fn snapshot_version(&self) -> u64 {
        self.cache.version()
    }

    // Platform adapter methods (merged from LinuxAdapter)

    #[allow(clippy::manual_async_fn)]
    fn capture_screen(
        &self,
        target: &Target,
    ) -> impl std::future::Future<Output = Result<Screenshot>> {
        async move {
            let pid = match target {
                Target::Pid(pid) => Some(*pid),
                Target::System => None,
                _ => bail!("Linux screenshot requires Target::Pid or Target::System"),
            };
            if let Some(pid) = pid
                && let Ok(screenshot) = self.capture_window(pid)
            {
                return Ok(screenshot);
            }
            LinuxAccessibility::capture_screen(self)
        }
    }

    async fn get_screen_bounds(&self, target: &Target) -> Result<Rect> {
        let pid = match target {
            Target::Pid(pid) => Some(*pid),
            Target::System => None,
            _ => bail!("Linux screen bounds requires Target::Pid or Target::System"),
        };
        if let Some(pid) = pid {
            if let Some(bounds) = self.get_window_bounds_for_pid_via_atspi(pid).await {
                if bounds.origin.x == 0.0
                    && bounds.origin.y == 0.0
                    && std::env::var("WAYLAND_DISPLAY").is_ok()
                {
                    return Ok(Rect::new(Point::new(0.0, 0.0), bounds.size));
                }
                return Ok(bounds);
            }
            if let Some(bounds) = LinuxAccessibility::get_window_bounds_for_pid(pid) {
                return Ok(bounds);
            }
        }
        // Static method that doesn't require async
        Self::get_global_screen_bounds()
    }

    fn platform_name(&self) -> &'static str {
        "Linux"
    }

    fn supports_hit_test(&self) -> bool {
        true
    }

    // Event listening implementation

    fn start_listening(
        &mut self,
        config: ListenerConfig,
        callback: Box<dyn FnMut(AccessibilityEvent) + Send + 'static>,
    ) -> Result<ListenerHandle> {
        let Some(target_pid) = config.pid else {
            return Err(anyhow!("Linux event listening requires a target pid"));
        };
        let target_pid = Some(target_pid);

        // Create stop flag
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_clone = stop_flag.clone();

        // Wrap callback in Arc<Mutex> for thread-safe access
        let callback: Arc<Mutex<EventCallback>> = Arc::new(Mutex::new(callback));

        // Clone config for the spawned task
        let config_clone = config.clone();

        // Clone the connection for the spawned task
        let conn = self.connection.connection().clone();

        // Spawn async task for AT-SPI D-Bus event loop
        let task_handle = tokio::spawn(async move {
            run_linux_event_loop(conn, target_pid, config_clone, callback, stop_flag_clone).await;
        });

        Ok(ListenerHandle::new(stop_flag, task_handle))
    }

    fn supports_event_listening(&self) -> bool {
        true
    }

    fn supported_event_types(&self) -> Vec<AccessibilityEventType> {
        vec![
            AccessibilityEventType::FocusChanged,
            AccessibilityEventType::ValueChanged,
            AccessibilityEventType::StructureChanged,
            AccessibilityEventType::WindowCreated,
            AccessibilityEventType::WindowDestroyed,
        ]
    }
}

/// Type alias for the boxed callback trait object.
type EventCallback = Box<dyn FnMut(AccessibilityEvent) + Send>;

/// Get the current timestamp in milliseconds since UNIX epoch.
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Build a minimal Element from AT-SPI event data.
async fn build_element_from_event(
    conn: &zbus::Connection,
    bus_name: &str,
    object_path: &str,
) -> Option<Element> {
    let proxy = LinuxAccessibility::create_accessible_proxy(conn, bus_name, object_path)
        .await
        .ok()?;

    let atspi_role = proxy.get_role().await.ok()?;
    let role = LinuxAccessibility::map_role(atspi_role);

    // Use a placeholder key since we're not caching this element
    let placeholder_key = ElementKey::from_ffi(1);

    let mut element = Element::new(placeholder_key, role);
    element.title = proxy.name().await.ok().filter(|s| !s.is_empty());
    element.description = proxy.description().await.ok().filter(|s| !s.is_empty());
    element.identifier = proxy.accessible_id().await.ok().filter(|s| !s.is_empty());

    // Get states
    if let Ok(states) = proxy.get_state().await {
        element.enabled =
            !states.contains(atspi::State::Sensitive) || states.contains(atspi::State::Enabled);
        element.focused = states.contains(atspi::State::Focused);
    }

    // Try to get bounds from Component interface
    if let Ok(interfaces) = proxy.get_interfaces().await
        && interfaces.contains(atspi::Interface::Component)
        && let Ok(component) =
            LinuxAccessibility::create_component_proxy(conn, bus_name, object_path).await
        && let Ok((x, y, width, height)) = component.get_extents(CoordType::Screen).await
    {
        element.bounds = Some(Rect::new(
            Point::new(x as f64, y as f64),
            Size::new(width as f64, height as f64),
        ));
    }

    Some(element)
}

async fn event_matches_target_pid(
    conn: &zbus::Connection,
    bus_name: &str,
    target_pid: Option<u32>,
) -> bool {
    match target_pid {
        Some(pid) => match LinuxAccessibility::get_pid_for_bus_name(conn, bus_name).await {
            Some(event_pid) => event_pid == pid,
            None => true,
        },
        None => true,
    }
}

/// Run the Linux event loop using AT-SPI D-Bus signals.
///
/// This function runs as an async task and subscribes to AT-SPI events
/// using the atspi crate's event stream.
async fn run_linux_event_loop(
    conn: zbus::Connection,
    target_pid: Option<u32>,
    config: ListenerConfig,
    callback: Arc<Mutex<EventCallback>>,
    stop_flag: Arc<AtomicBool>,
) {
    use accessibility_linux_sys::futures_lite::StreamExt;

    // Create a new accessibility connection for event listening
    let atspi_conn = match AccessibilityConnection::new().await {
        Ok(c) => c,
        Err(e) => {
            if let Ok(mut cb) = callback.lock() {
                cb(AccessibilityEvent::Error {
                    message: format!("Failed to create AT-SPI connection: {}", e),
                    timestamp: current_timestamp(),
                });
            }
            return;
        }
    };

    // Register for events we care about
    let mut event_types_to_register = Vec::new();

    if config.should_capture(AccessibilityEventType::FocusChanged) {
        event_types_to_register.push("focus");
    }
    if config.should_capture(AccessibilityEventType::ValueChanged) {
        event_types_to_register.push("object:text-changed");
        event_types_to_register.push("object:property-change:accessible-value");
    }
    if config.should_capture(AccessibilityEventType::StructureChanged) {
        event_types_to_register.push("object:children-changed");
    }
    if config.should_capture(AccessibilityEventType::WindowCreated) {
        event_types_to_register.push("window:create");
    }
    if config.should_capture(AccessibilityEventType::WindowDestroyed) {
        event_types_to_register.push("window:destroy");
    }

    // Register for focus events if enabled
    if config.should_capture(AccessibilityEventType::FocusChanged)
        && let Err(e) = atspi_conn
            .register_event::<atspi::events::focus::FocusEvent>()
            .await
    {
        eprintln!("Warning: Failed to register for focus events: {}", e);
    }

    // Register for object events
    if config.should_capture(AccessibilityEventType::StructureChanged)
        && let Err(e) = atspi_conn
            .register_event::<atspi::events::object::ChildrenChangedEvent>()
            .await
    {
        eprintln!(
            "Warning: Failed to register for children changed events: {}",
            e
        );
    }

    if config.should_capture(AccessibilityEventType::ValueChanged)
        && let Err(e) = atspi_conn
            .register_event::<atspi::events::object::TextChangedEvent>()
            .await
    {
        eprintln!("Warning: Failed to register for text changed events: {}", e);
    }

    // Register for window events
    if config.should_capture(AccessibilityEventType::WindowCreated)
        && let Err(e) = atspi_conn
            .register_event::<atspi::events::window::CreateEvent>()
            .await
    {
        eprintln!(
            "Warning: Failed to register for window create events: {}",
            e
        );
    }

    if config.should_capture(AccessibilityEventType::WindowDestroyed)
        && let Err(e) = atspi_conn
            .register_event::<atspi::events::window::DestroyEvent>()
            .await
    {
        eprintln!(
            "Warning: Failed to register for window destroy events: {}",
            e
        );
    }

    // Get the event stream
    let event_stream = atspi_conn.event_stream();
    tokio::pin!(event_stream);

    // Main event loop
    loop {
        // Check for stop signal
        if stop_flag.load(AtomicOrdering::SeqCst) {
            break;
        }

        // Use a short timeout so we can check the stop flag periodically
        let event =
            tokio::time::timeout(std::time::Duration::from_millis(100), event_stream.next()).await;

        let event = match event {
            Ok(Some(Ok(e))) => e,
            Ok(Some(Err(_))) => continue, // Error receiving event, continue
            Ok(None) => break,            // Stream ended
            Err(_) => continue,           // Timeout, check stop flag again
        };

        // Convert AT-SPI event to our AccessibilityEvent
        let accessibility_event = match &event {
            atspi::Event::Focus(atspi::events::FocusEvents::Focus(focus_event)) => {
                // Check PID if filtering
                let bus_name = focus_event.item.name_as_str().unwrap_or_default();
                if !event_matches_target_pid(&conn, bus_name, target_pid).await {
                    continue;
                }

                let element =
                    build_element_from_event(&conn, bus_name, focus_event.item.path_as_str()).await;

                let event_pid = LinuxAccessibility::get_pid_for_bus_name(&conn, bus_name).await;

                AccessibilityEvent::FocusChanged {
                    element,
                    pid: event_pid,
                    timestamp: current_timestamp(),
                }
            }

            atspi::Event::Object(atspi::events::ObjectEvents::ChildrenChanged(children_event)) => {
                let bus_name = children_event.item.name_as_str().unwrap_or_default();
                if !event_matches_target_pid(&conn, bus_name, target_pid).await {
                    continue;
                }

                let parent =
                    build_element_from_event(&conn, bus_name, children_event.item.path_as_str())
                        .await;

                // Determine change type based on operation
                let change_type = match children_event.operation {
                    atspi::Operation::Insert => StructureChangeType::ChildrenAdded,
                    atspi::Operation::Delete => StructureChangeType::ChildrenRemoved,
                };

                AccessibilityEvent::StructureChanged {
                    parent_element: parent,
                    change_type,
                    timestamp: current_timestamp(),
                }
            }

            atspi::Event::Object(atspi::events::ObjectEvents::TextChanged(text_event)) => {
                let bus_name = text_event.item.name_as_str().unwrap_or_default();
                if !event_matches_target_pid(&conn, bus_name, target_pid).await {
                    continue;
                }

                let element =
                    build_element_from_event(&conn, bus_name, text_event.item.path_as_str()).await;

                AccessibilityEvent::ValueChanged {
                    element,
                    old_value: None,
                    new_value: Some(text_event.text.clone()),
                    timestamp: current_timestamp(),
                }
            }

            atspi::Event::Window(atspi::events::WindowEvents::Create(create_event)) => {
                let bus_name = create_event.item.name_as_str().unwrap_or_default();
                if !event_matches_target_pid(&conn, bus_name, target_pid).await {
                    continue;
                }

                let element =
                    build_element_from_event(&conn, bus_name, create_event.item.path_as_str())
                        .await;

                let event_pid = LinuxAccessibility::get_pid_for_bus_name(&conn, bus_name).await;

                AccessibilityEvent::WindowCreated {
                    element,
                    pid: event_pid,
                    timestamp: current_timestamp(),
                }
            }

            atspi::Event::Window(atspi::events::WindowEvents::Destroy(destroy_event)) => {
                let bus_name = destroy_event.item.name_as_str().unwrap_or_default();
                if !event_matches_target_pid(&conn, bus_name, target_pid).await {
                    continue;
                }

                let event_pid = LinuxAccessibility::get_pid_for_bus_name(&conn, bus_name).await;

                AccessibilityEvent::WindowDestroyed {
                    window_id: Some(destroy_event.item.path_as_str().to_string()),
                    pid: event_pid,
                    timestamp: current_timestamp(),
                }
            }

            _ => continue, // Ignore other events
        };

        // Send the event via callback
        if let Ok(mut cb) = callback.lock() {
            cb(accessibility_event);
        }
    }

    // Send stopped event
    if let Ok(mut cb) = callback.lock() {
        cb(AccessibilityEvent::Stopped {
            reason: StopReason::UserRequested,
            timestamp: current_timestamp(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_mapping() {
        assert_eq!(
            LinuxAccessibility::map_role(AtspiRole::Button),
            Role::Button
        );
        assert_eq!(
            LinuxAccessibility::map_role(AtspiRole::Entry),
            Role::TextInput
        );
        assert_eq!(
            LinuxAccessibility::map_role(AtspiRole::CheckBox),
            Role::CheckBox
        );
        assert_eq!(LinuxAccessibility::map_role(AtspiRole::Link), Role::Link);
        assert_eq!(
            LinuxAccessibility::map_role(AtspiRole::Invalid),
            Role::Unknown
        );
    }
}
