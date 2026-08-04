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
//! Accessibility frames come back in points relative to the Simulator window,
//! so every rect is converted to a 0..1 fraction of the app's own bounds
//! before it leaves this module:
//!
//! ```text
//! normalized = (ax_rect.origin - app_bounds.origin) / app_bounds.size
//! ```
//!
//! That makes the values independent of the display scale factor, which is
//! why no pixel/point conversion appears here.
//!
//! Crucially, these are **logical** coordinates, already rotated by iOS: in
//! landscape the app reports its bounds as 874x402 rather than 402x874, so the
//! normalized rects are upright and need no further rotation to be drawn.
//!
//! This is the opposite of the HID input path, which is in *raw framebuffer*
//! space — the framebuffer never rotates, so pointer coordinates have to be
//! un-rotated before injection. The two spaces coincide in portrait, which is
//! exactly what makes the difference easy to miss.

use anyhow::{Result, anyhow};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use accessibility_core::accessibility::{Element, Rect, TreeFilter};

use crate::coverage::CoverageGrid;

/// A rectangle in normalized display space (0..1 on both axes).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct NormalizedRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Fraction of the screen above which an element is treated as a backdrop
/// rather than something you can point at.
///
/// Every app has an Application node and usually one or more full-bleed
/// container groups. Hit testing an empty region resolves to one of them, and
/// highlighting it paints a box over the entire device, which reads as "the
/// picker is broken" rather than "there is nothing here".
const BACKDROP_AREA: f64 = 0.9;

impl NormalizedRect {
    /// Whether this covers essentially the whole screen.
    pub fn is_backdrop(&self) -> bool {
        self.width * self.height >= BACKDROP_AREA
    }

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

/// How an element was found.
///
/// Worth surfacing: a swept element is a point sample with no parent, no
/// children and no document order, so consumers should not treat it as
/// equivalent to a node the tree walk returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Discovery {
    /// Walked from the application root.
    Recursive,
    /// Found by hit testing a grid point the tree could not explain.
    PointGrid,
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
    pub discovery: Discovery,
}

#[derive(Debug, Clone, Serialize)]
pub struct AxSnapshot {
    pub app_name: Option<String>,
    pub pid: Option<u32>,
    pub elements: Vec<ElementDetail>,
    /// Fraction of the display explained by the tree walk alone, before any
    /// sweep. Low numbers on a busy screen mean out-of-process content.
    pub coverage: f64,
    /// Coverage after sweeping, when a scan was requested.
    pub coverage_after_scan: Option<f64>,
    /// How many points the sweep probed. `None` when no scan was requested.
    pub probes: Option<usize>,
    /// Whether the app reports itself wider than tall.
    ///
    /// Accessibility bounds are in logical space, so this is the one cheap
    /// signal that reveals the device's real orientation. The framebuffer
    /// cannot provide it: it never rotates.
    pub is_landscape: bool,
}

pub enum AxCommand {
    Snapshot {
        /// Also sweep the regions the tree walk cannot explain.
        scan: bool,
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

fn to_detail(
    element: &Element,
    app_bounds: &Rect,
    depth: u32,
    discovery: Discovery,
) -> ElementDetail {
    let role = format!("{:?}", element.role);
    ElementDetail {
        discovery,
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
    out.push(to_detail(element, app_bounds, depth, Discovery::Recursive));
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
                    AxCommand::Snapshot { scan, reply } => {
                        let _ = reply.send(snapshot(&mut reader, scan));
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
    scan: bool,
) -> Result<AxSnapshot> {
    let tree = reader.get_tree(&TreeFilter::default())?;
    // `get_screen_bounds` is only populated once a tree has been read, so it
    // has to be queried after the fetch above.
    let app_bounds = reader.get_screen_bounds()?;

    let mut elements: Vec<ElementDetail> = Vec::with_capacity(tree.element_count);
    flatten(&tree.root, &app_bounds, 0, &mut elements);

    // Drop backdrops and anything with no usable geometry. Both are real parts
    // of the tree but neither can be pointed at, and leaving them in makes the
    // client's containment search pick them constantly.
    elements.retain(|element| {
        element.bounds.is_some_and(|bounds| {
            !bounds.is_backdrop() && bounds.width > 0.0 && bounds.height > 0.0
        })
    });

    // Everything the tree walk explained, so the sweep can skip it.
    let mut coverage = CoverageGrid::new();
    for element in &elements {
        if let Some(bounds) = element.bounds {
            coverage.mark(&bounds);
        }
    }
    let coverage_before = coverage.ratio();

    let mut probes = None;
    let mut coverage_after_scan = None;
    if scan {
        let swept = sweep(reader, &app_bounds, &mut coverage, &elements)?;
        probes = Some(swept.probes);
        elements.extend(swept.elements);
        coverage_after_scan = Some(coverage.ratio());
    }

    Ok(AxSnapshot {
        app_name: tree.app_name,
        pid: tree.pid,
        elements,
        coverage: coverage_before,
        coverage_after_scan,
        probes,
        is_landscape: app_bounds.size.width > app_bounds.size.height,
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

    let Some(element) = reader.element_at_point(screen_x, screen_y)? else {
        return Ok(None);
    };
    let detail = to_detail(&element, &app_bounds, 0, Discovery::Recursive);

    // Nothing pointable here. Reporting the backdrop would highlight the whole
    // device; reporting nothing lets the caller leave the previous selection
    // or clear it.
    if detail.bounds.is_none_or(|bounds| bounds.is_backdrop()) {
        return Ok(None);
    }
    Ok(Some(detail))
}

#[cfg(not(target_os = "macos"))]
pub fn spawn_ax_worker(_udid: &str) -> Result<mpsc::UnboundedSender<AxCommand>> {
    anyhow::bail!("Simulator accessibility requires macOS")
}

/// Spacing between sweep probes, in device points.
///
/// idb uses 50; 40 is a little denser for phone-sized screens, where rows are
/// around 50 points tall and a coarser grid can step straight over one. Every
/// probe is a hit test, so this trades roughly linearly against scan time.
#[cfg(target_os = "macos")]
const SWEEP_STEP_POINTS: f64 = 40.0;

/// Upper bound on probes, so a scan cannot run away on a large display.
#[cfg(target_os = "macos")]
const SWEEP_MAX_PROBES: usize = 400;

#[cfg(target_os = "macos")]
struct SweepResult {
    elements: Vec<ElementDetail>,
    probes: usize,
}

/// Hit test the regions the tree walk could not explain.
///
/// This is the only way to reach content in another process — `WKWebView` and
/// Safari above all — because such content is individually addressable by
/// point but has no traversable hierarchy. The result is a flat set of point
/// samples, which is why each is tagged [`Discovery::PointGrid`].
#[cfg(target_os = "macos")]
fn sweep(
    reader: &mut accessibility_core::platform::ios_simulator::IOSSimulatorAccessibility,
    app_bounds: &Rect,
    coverage: &mut CoverageGrid,
    known: &[ElementDetail],
) -> Result<SweepResult> {
    use std::collections::HashSet;

    // Elements the tree already reported, keyed by position, so a probe that
    // lands on one does not report it twice under a different provenance.
    let mut seen: HashSet<String> = known
        .iter()
        .filter_map(|element| element.bounds.map(frame_key))
        .collect();

    let mut elements = Vec::new();
    let mut probes = 0usize;

    let columns = (app_bounds.size.width / SWEEP_STEP_POINTS).floor().max(1.0) as usize;
    let rows = (app_bounds.size.height / SWEEP_STEP_POINTS)
        .floor()
        .max(1.0) as usize;

    for row in 0..rows {
        for column in 0..columns {
            if probes >= SWEEP_MAX_PROBES {
                return Ok(SweepResult { elements, probes });
            }

            // Probe cell centres so a point never lands exactly on a boundary.
            let x = (column as f64 + 0.5) / columns as f64;
            let y = (row as f64 + 0.5) / rows as f64;

            // Already explained by the tree; nothing to learn here.
            if coverage.is_filled(x, y) {
                continue;
            }
            probes += 1;

            let screen_x = app_bounds.origin.x + x * app_bounds.size.width;
            let screen_y = app_bounds.origin.y + y * app_bounds.size.height;
            let Ok(Some(element)) = reader.element_at_point(screen_x, screen_y) else {
                continue;
            };

            let detail = to_detail(&element, app_bounds, 0, Discovery::PointGrid);
            let Some(bounds) = detail.bounds else {
                continue;
            };
            if bounds.is_backdrop() || bounds.width <= 0.0 || bounds.height <= 0.0 {
                continue;
            }
            // A 40 point grid lands on a large element many times over.
            if !seen.insert(frame_key(bounds)) {
                // Still mark it: further probes inside it are wasted.
                coverage.mark(&bounds);
                continue;
            }

            coverage.mark(&bounds);
            elements.push(detail);
        }
    }

    Ok(SweepResult { elements, probes })
}

/// Position key for deduplication, rounded so float noise does not defeat it.
#[cfg(target_os = "macos")]
fn frame_key(bounds: NormalizedRect) -> String {
    format!(
        "{:.4},{:.4},{:.4},{:.4}",
        bounds.x, bounds.y, bounds.width, bounds.height
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use accessibility_core::accessibility::{Point, Size};

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(Point::new(x, y), Size::new(w, h))
    }

    #[test]
    fn normalizes_against_app_bounds_not_the_screen() {
        // Landscape: the app reports itself 874x402, and accessibility frames
        // are already in that logical space.
        let app = rect(0.0, 0.0, 874.0, 402.0);
        let button = rect(754.5, 82.0, 27.0, 44.0);
        let normalized = NormalizedRect::from_screen(&button, &app).expect("normalizes");
        assert!((normalized.x - 0.863).abs() < 0.001);
        assert!((normalized.y - 0.204).abs() < 0.001);
        assert!((normalized.width - 0.031).abs() < 0.001);
    }

    #[test]
    fn offsets_by_the_app_origin() {
        let app = rect(100.0, 50.0, 400.0, 800.0);
        let element = rect(300.0, 450.0, 200.0, 400.0);
        let normalized = NormalizedRect::from_screen(&element, &app).expect("normalizes");
        assert!((normalized.x - 0.5).abs() < 1e-9);
        assert!((normalized.y - 0.5).abs() < 1e-9);
    }

    #[test]
    fn degenerate_app_bounds_do_not_divide_by_zero() {
        let app = rect(0.0, 0.0, 0.0, 0.0);
        assert!(NormalizedRect::from_screen(&rect(1.0, 1.0, 2.0, 2.0), &app).is_none());
    }

    #[test]
    fn full_screen_containers_are_backdrops() {
        // Every app has these; highlighting one paints over the whole device.
        assert!(
            NormalizedRect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0
            }
            .is_backdrop()
        );
        assert!(
            NormalizedRect {
                x: 0.0,
                y: 0.0,
                width: 0.98,
                height: 0.96
            }
            .is_backdrop()
        );
    }

    #[test]
    fn ordinary_controls_are_not_backdrops() {
        // A full-width settings row is large but perfectly pointable.
        assert!(
            !NormalizedRect {
                x: 0.05,
                y: 0.45,
                width: 0.90,
                height: 0.06
            }
            .is_backdrop()
        );
        // So is a tall sidebar.
        assert!(
            !NormalizedRect {
                x: 0.0,
                y: 0.0,
                width: 0.25,
                height: 1.0
            }
            .is_backdrop()
        );
    }
}
