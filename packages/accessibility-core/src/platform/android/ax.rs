use anyhow::{Result, anyhow};
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use crate::accessibility::{
    AccessibilityReader, AndroidTarget, Element, ElementTree, Point, Rect, Size, Target, TreeFilter,
};

use super::AndroidAccessibility;

const COVERAGE_COLUMNS: usize = 32;
const COVERAGE_ROWS: usize = 64;
const BACKDROP_AREA: f64 = 0.9;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct NormalizedRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl NormalizedRect {
    fn from_screen(rect: &Rect, screen: &Rect) -> Option<Self> {
        if screen.size.width <= 0.0 || screen.size.height <= 0.0 {
            return None;
        }
        Some(Self {
            x: (rect.origin.x - screen.origin.x) / screen.size.width,
            y: (rect.origin.y - screen.origin.y) / screen.size.height,
            width: rect.size.width / screen.size.width,
            height: rect.size.height / screen.size.height,
        })
    }

    fn is_backdrop(self) -> bool {
        self.width * self.height >= BACKDROP_AREA
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Discovery {
    Recursive,
    PointGrid,
}

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
    pub depth: u32,
    pub selector: String,
    pub discovery: Discovery,
}

#[derive(Debug, Clone, Serialize)]
pub struct AxSnapshot {
    pub app_name: Option<String>,
    pub pid: Option<u32>,
    pub elements: Vec<ElementDetail>,
    pub coverage: f64,
    pub coverage_after_scan: Option<f64>,
    pub probes: Option<usize>,
    pub is_landscape: bool,
}

pub enum AxCommand {
    Snapshot {
        scan: bool,
        reply: oneshot::Sender<Result<AxSnapshot>>,
    },
    HitTest {
        x: f64,
        y: f64,
        reply: oneshot::Sender<Result<Option<ElementDetail>>>,
    },
}

pub async fn spawn_ax_worker(serial: &str) -> Result<mpsc::UnboundedSender<AxCommand>> {
    let adb = super::AdbClient::discover(Some(serial));
    let mut reader = AndroidAccessibility::with_adb_path(Some(serial), &adb.adb_path).await?;
    let target = Target::Android(AndroidTarget::Serial(serial.to_string()));
    let (commands, mut command_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut screen = None;
        while let Some(command) = command_rx.recv().await {
            match command {
                AxCommand::Snapshot { scan, reply } => {
                    let result = snapshot(&mut reader, &target, scan).await;
                    if let Ok((_, bounds)) = &result {
                        screen = Some(*bounds);
                    }
                    let _ = reply.send(result.map(|(snapshot, _)| snapshot));
                }
                AxCommand::HitTest { x, y, reply } => {
                    let result = hit_test(&mut reader, screen.as_ref(), x, y).await;
                    let _ = reply.send(result);
                }
            }
        }
    });
    Ok(commands)
}

async fn snapshot(
    reader: &mut AndroidAccessibility,
    target: &Target,
    _scan: bool,
) -> Result<(AxSnapshot, Rect)> {
    let tree = reader.get_tree(target, &TreeFilter::default()).await?;
    let screen =
        screen_bounds(&tree).ok_or_else(|| anyhow!("Android tree has no screen bounds"))?;
    let mut elements = Vec::with_capacity(tree.element_count);
    flatten(&tree.root, &screen, 0, &mut elements);
    elements.retain(|element| {
        element.bounds.is_some_and(|bounds| {
            !bounds.is_backdrop() && bounds.width > 0.0 && bounds.height > 0.0
        })
    });
    let coverage = coverage(&elements);
    Ok((
        AxSnapshot {
            app_name: tree.app_name,
            pid: tree.pid,
            elements,
            coverage,
            coverage_after_scan: None,
            probes: None,
            is_landscape: screen.size.width > screen.size.height,
        },
        screen,
    ))
}

async fn hit_test(
    reader: &mut AndroidAccessibility,
    screen: Option<&Rect>,
    x: f64,
    y: f64,
) -> Result<Option<ElementDetail>> {
    let screen =
        screen.ok_or_else(|| anyhow!("no accessibility snapshot yet; fetch the tree first"))?;
    let point_x = screen.origin.x + x * screen.size.width;
    let point_y = screen.origin.y + y * screen.size.height;
    let Some(id) = reader.hit_test(point_x, point_y).await? else {
        return Ok(None);
    };
    Ok(reader
        .get_element(id)
        .map(|element| to_detail(element, screen, 0)))
}

fn screen_bounds(tree: &ElementTree) -> Option<Rect> {
    let mut max_x = 0.0f64;
    let mut max_y = 0.0f64;
    let mut stack = vec![&tree.root];
    while let Some(element) = stack.pop() {
        if let Some(bounds) = &element.bounds {
            max_x = max_x.max(bounds.origin.x + bounds.size.width);
            max_y = max_y.max(bounds.origin.y + bounds.size.height);
        }
        stack.extend(element.children.iter());
    }
    (max_x > 0.0 && max_y > 0.0).then(|| Rect::new(Point::new(0.0, 0.0), Size::new(max_x, max_y)))
}

fn flatten(element: &Element, screen: &Rect, depth: u32, out: &mut Vec<ElementDetail>) {
    out.push(to_detail(element, screen, depth));
    for child in &element.children {
        flatten(child, screen, depth + 1, out);
    }
}

fn to_detail(element: &Element, screen: &Rect, depth: u32) -> ElementDetail {
    let role = format!("{:?}", element.role);
    ElementDetail {
        id: element.id.to_string(),
        role: role.clone(),
        label: element.title.clone().filter(|value| !value.is_empty()),
        value: element.value.clone().filter(|value| !value.is_empty()),
        identifier: element.identifier.clone().filter(|value| !value.is_empty()),
        enabled: element.enabled,
        focused: element.focused,
        actions: element.actions.clone(),
        bounds: element
            .bounds
            .as_ref()
            .and_then(|bounds| NormalizedRect::from_screen(bounds, screen)),
        depth,
        selector: selector_for(element, &role),
        discovery: Discovery::Recursive,
    }
}

fn selector_for(element: &Element, role: &str) -> String {
    if let Some(identifier) = element
        .identifier
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return format!("#{identifier}");
    }
    if let Some(label) = element.title.as_deref().filter(|value| !value.is_empty()) {
        return format!("{role}[label=\"{}\"]", label.replace('"', "\\\""));
    }
    role.to_string()
}

fn coverage(elements: &[ElementDetail]) -> f64 {
    let mut cells = [false; COVERAGE_COLUMNS * COVERAGE_ROWS];
    for bounds in elements.iter().filter_map(|element| element.bounds) {
        let left = (bounds.x.clamp(0.0, 1.0) * COVERAGE_COLUMNS as f64).floor() as usize;
        let right = (((bounds.x + bounds.width).clamp(0.0, 1.0) * COVERAGE_COLUMNS as f64).ceil()
            as usize)
            .min(COVERAGE_COLUMNS);
        let top = (bounds.y.clamp(0.0, 1.0) * COVERAGE_ROWS as f64).floor() as usize;
        let bottom = (((bounds.y + bounds.height).clamp(0.0, 1.0) * COVERAGE_ROWS as f64).ceil()
            as usize)
            .min(COVERAGE_ROWS);
        for row in top..bottom {
            for column in left..right {
                cells[row * COVERAGE_COLUMNS + column] = true;
            }
        }
    }
    cells.iter().filter(|filled| **filled).count() as f64 / cells.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accessibility::Element;
    use accesskit::Role;

    #[test]
    fn normalizes_android_pixel_bounds() {
        let screen = Rect::new(Point::new(0.0, 0.0), Size::new(1080.0, 2400.0));
        let bounds = Rect::new(Point::new(108.0, 240.0), Size::new(540.0, 1200.0));
        let normalized = NormalizedRect::from_screen(&bounds, &screen).unwrap();
        assert_eq!(normalized.x, 0.1);
        assert_eq!(normalized.y, 0.1);
        assert_eq!(normalized.width, 0.5);
        assert_eq!(normalized.height, 0.5);
    }

    #[test]
    fn coverage_counts_element_area() {
        let mut element = Element::new(crate::accessibility::ElementKey::default(), Role::Button);
        element.bounds = Some(Rect::new(Point::new(0.0, 0.0), Size::new(100.0, 100.0)));
        let detail = to_detail(
            &element,
            &Rect::new(Point::new(0.0, 0.0), Size::new(200.0, 200.0)),
            0,
        );
        assert_eq!(coverage(&[detail]), 0.25);
    }
}
