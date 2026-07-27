//! Accessibility element inspection for the web UI.
//!
//! Two things are exposed:
//!
//! - A whole-tree snapshot, which the browser uses for instant hover feedback.
//! - A live hit test, which asks the simulator what is actually at a point.
//!
//! The snapshot can go stale between fetches and mis-picks overlapping or
//! transformed views; the hit test is authoritative but costs a round trip.
//! The UI uses the snapshot while the pointer is moving and confirms with the
//! hit test once it settles, which is why both exist.
//!
//! # Coordinate spaces
//!
//! Accessibility frames come back in macOS screen points, positioned wherever
//! the Simulator window happens to be. The browser knows nothing about that,
//! so every rect is converted to a 0..1 fraction of the app's own bounds
//! before it leaves this module:
//!
//! ```text
//! normalized = (ax_rect.origin - app_bounds.origin) / app_bounds.size
//! ```
//!
//! That also makes the values independent of the display scale factor, which
//! is why no pixel/point conversion appears here.

use anyhow::{Result, anyhow};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use accessibility_core::accessibility::{Element, Rect, TreeFilter};

/// A rectangle in normalized display space (0..1 on both axes).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct NormalizedRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl NormalizedRect {
    fn from_screen(rect: &Rect, app_bounds: &Rect) -> Option<Self> {
        if app_bounds.size.width <= 0.0 || app_bounds.size.height <= 0.0 {
            return None;
        }
        Some(Self {
            x: (rect.origin.x - app_bounds.origin.x) / app_bounds.size.width,
            y: (rect.origin.y - app_bounds.origin.y) / app_bounds.size.height,
            width: rect.size.width / app_bounds.size.width,
            height: rect.size.height / app_bounds.size.height,
        })
    }
}

/// One inspectable element, flattened for the browser.
#[derive(Debug, Clone, Serialize)]
pub struct ElementDetail {
    pub id: String,
    pub role: String,
    pub label: Option<String>,
    pub value: Option<String>,
    pub identifier: Option<String>,
    pub enabled: bool,
    pub focused: bool,
    pub actions: Vec<String>,
    pub bounds: Option<NormalizedRect>,
    /// Depth in the tree, used by the UI to prefer the innermost hit.
    pub depth: u32,
    /// A selector that would target this element from the CLI.
    pub selector: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AxSnapshot {
    pub app_name: Option<String>,
    pub pid: Option<u32>,
    pub elements: Vec<ElementDetail>,
}

pub enum AxCommand {
    Snapshot {
        reply: oneshot::Sender<Result<AxSnapshot>>,
    },
    HitTest {
        x: f64,
        y: f64,
        reply: oneshot::Sender<Result<Option<ElementDetail>>>,
    },
}

/// Build the `accessibility-cli` selector that would resolve this element.
///
/// Preferring the identifier keeps the selector stable across copy changes,
/// which is the whole point of exposing it in the inspector.
fn selector_for(element: &Element, role: &str) -> String {
    if let Some(identifier) = element.identifier.as_deref().filter(|s| !s.is_empty()) {
        return format!("#{identifier}");
    }
    if let Some(label) = element.title.as_deref().filter(|s| !s.is_empty()) {
        return format!("{role}[label=\"{}\"]", label.replace('"', "\\\""));
    }
    role.to_string()
}

fn to_detail(element: &Element, app_bounds: &Rect, depth: u32) -> ElementDetail {
    let role = format!("{:?}", element.role);
    ElementDetail {
        id: element.id.to_string(),
        selector: selector_for(element, &role),
        role,
        label: element.title.clone().filter(|s| !s.is_empty()),
        value: element.value.clone().filter(|s| !s.is_empty()),
        identifier: element.identifier.clone().filter(|s| !s.is_empty()),
        enabled: element.enabled,
        focused: element.focused,
        actions: element.actions.clone(),
        bounds: element
            .bounds
            .as_ref()
            .and_then(|bounds| NormalizedRect::from_screen(bounds, app_bounds)),
        depth,
    }
}

fn flatten(element: &Element, app_bounds: &Rect, depth: u32, out: &mut Vec<ElementDetail>) {
    out.push(to_detail(element, app_bounds, depth));
    for child in &element.children {
        flatten(child, app_bounds, depth + 1, out);
    }
}

/// Start the accessibility worker thread and return its command channel.
#[cfg(target_os = "macos")]
pub fn spawn_ax_worker(udid: &str) -> Result<mpsc::UnboundedSender<AxCommand>> {
    use accessibility_core::platform::ios_simulator::IOSSimulatorAccessibility;

    let mut reader = IOSSimulatorAccessibility::new(Some(udid))?;
    let (tx, mut rx) = mpsc::unbounded_channel::<AxCommand>();

    std::thread::Builder::new()
        .name("sim-ax".into())
        .spawn(move || {
            while let Some(command) = rx.blocking_recv() {
                match command {
                    AxCommand::Snapshot { reply } => {
                        let _ = reply.send(snapshot(&mut reader));
                    }
                    AxCommand::HitTest { x, y, reply } => {
                        let _ = reply.send(hit_test(&mut reader, x, y));
                    }
                }
            }
        })?;

    Ok(tx)
}

#[cfg(target_os = "macos")]
fn snapshot(
    reader: &mut accessibility_core::platform::ios_simulator::IOSSimulatorAccessibility,
) -> Result<AxSnapshot> {
    let tree = reader.get_tree(&TreeFilter::default())?;
    // `get_screen_bounds` is only populated once a tree has been read, so it
    // has to be queried after the fetch above.
    let app_bounds = reader.get_screen_bounds()?;

    let mut elements = Vec::with_capacity(tree.element_count);
    flatten(&tree.root, &app_bounds, 0, &mut elements);

    Ok(AxSnapshot {
        app_name: tree.app_name,
        pid: tree.pid,
        elements,
    })
}

#[cfg(target_os = "macos")]
fn hit_test(
    reader: &mut accessibility_core::platform::ios_simulator::IOSSimulatorAccessibility,
    x: f64,
    y: f64,
) -> Result<Option<ElementDetail>> {
    // The hit test wants macOS screen points, so the browser's normalized
    // coordinates are mapped back through the app's bounds.
    let app_bounds = reader
        .get_screen_bounds()
        .map_err(|_| anyhow!("no accessibility snapshot yet; fetch the tree first"))?;

    let screen_x = app_bounds.origin.x + x * app_bounds.size.width;
    let screen_y = app_bounds.origin.y + y * app_bounds.size.height;

    Ok(reader
        .element_at_point(screen_x, screen_y)?
        .map(|element| to_detail(&element, &app_bounds, 0)))
}

#[cfg(not(target_os = "macos"))]
pub fn spawn_ax_worker(_udid: &str) -> Result<mpsc::UnboundedSender<AxCommand>> {
    anyhow::bail!("Simulator accessibility requires macOS")
}
