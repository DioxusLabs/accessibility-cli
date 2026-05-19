use crate::cli::OutputArgs;
use crate::error::{CliError, CliResult};
use crate::operations::OperationResult;
use accessibility_core::accessibility::{
    Element, ElementKey, ElementTree, Rect, TargetedAccessibility,
};
use accessibility_core::api::{
    OutputFormat, OutputPrinter, print_formatted, print_statistics, truncate,
};
use std::collections::HashSet;

pub fn print_tree(adapter: &TargetedAccessibility, tree: &ElementTree, output: &OutputArgs) {
    let output_format = output.output_format();
    let printer = OutputPrinter::new(output_format, output.structure);
    let is_tree_mode = output_format == OutputFormat::Tree;

    if is_tree_mode {
        println!("=== {} Accessibility Tree ===", adapter.platform_name());
        println!("App: {:?}", tree.app_name);
        println!("PID: {:?}", tree.pid);
        println!("Version: {}", tree.version);
        println!("Element Count: {}", tree.element_count);
        println!();
    }

    print_formatted(tree, &printer);

    if is_tree_mode {
        println!();
        print_statistics(&tree.root);

        if let Ok(interactive) = adapter.find_elements(tree, None, false)
            && !interactive.is_empty()
        {
            println!();
            println!("=== Interactive Elements ({}) ===", interactive.len());
            for elem in interactive.iter().take(20) {
                println!("  [{}] {:?}: {}", elem.id, elem.role, elem.display_label());
                if !elem.actions.is_empty() {
                    println!("       Actions: {}", elem.actions.join(", "));
                }
            }
            if interactive.len() > 20 {
                println!("  ... and {} more", interactive.len() - 20);
            }
        }
    }
}

pub fn query(
    adapter: &TargetedAccessibility,
    tree: &ElementTree,
    selector: &str,
    output: &OutputArgs,
) -> CliResult<OperationResult> {
    let elements = adapter
        .find_elements(tree, Some(selector), false)
        .map_err(|e| CliError::runtime(e.to_string()))?;
    if elements.is_empty() {
        return Ok(OperationResult::NotFound(format!(
            "No matches found for query: {selector}"
        )));
    }

    let filtered_tree = filter_tree_to_matches(tree, &elements);
    let printer = OutputPrinter::new(output.output_format(), output.structure);
    print_formatted(&filtered_tree, &printer);
    Ok(OperationResult::Success)
}

pub async fn click(
    adapter: &mut TargetedAccessibility,
    tree: &ElementTree,
    selector: &str,
    action_name: &str,
) -> CliResult<OperationResult> {
    match query_has_matches(adapter, tree, selector)? {
        false => Ok(OperationResult::NotFound(format!(
            "No element found for: {selector}"
        ))),
        true => match adapter.click_element(selector, tree).await {
            Ok(id) => {
                print_element_action(adapter, id, "Clicked");
                Ok(OperationResult::Success)
            }
            Err(e) => Err(CliError::runtime(format!("{action_name} failed: {e}"))),
        },
    }
}

pub async fn focus(
    adapter: &mut TargetedAccessibility,
    tree: &ElementTree,
    selector: &str,
) -> CliResult<OperationResult> {
    match query_has_matches(adapter, tree, selector)? {
        false => Ok(OperationResult::NotFound(format!(
            "No element found for: {selector}"
        ))),
        true => match adapter.focus_element(selector, tree).await {
            Ok(id) => {
                print_element_action(adapter, id, "Focused");
                Ok(OperationResult::Success)
            }
            Err(e) => Err(CliError::runtime(format!("Focus failed: {e}"))),
        },
    }
}

pub async fn blur(
    adapter: &mut TargetedAccessibility,
    tree: &ElementTree,
    selector: &str,
) -> CliResult<OperationResult> {
    match query_has_matches(adapter, tree, selector)? {
        false => Ok(OperationResult::NotFound(format!(
            "No element found for: {selector}"
        ))),
        true => match adapter.blur_element(selector, tree).await {
            Ok(id) => {
                print_element_action(adapter, id, "Blurred");
                Ok(OperationResult::Success)
            }
            Err(e) => Err(CliError::runtime(format!("Blur failed: {e}"))),
        },
    }
}

pub async fn type_value(
    adapter: &mut TargetedAccessibility,
    tree: &ElementTree,
    selector: &str,
    text: &str,
) -> CliResult<OperationResult> {
    match query_has_matches(adapter, tree, selector)? {
        false => Ok(OperationResult::NotFound(format!(
            "No element found for: {selector}"
        ))),
        true => match adapter.set_element_value(selector, text, tree).await {
            Ok(id) => {
                println!("Set value on [{id}] to \"{text}\"");
                Ok(OperationResult::Success)
            }
            Err(e) => Err(CliError::runtime(format!("Set value failed: {e}"))),
        },
    }
}

pub async fn key(
    adapter: &mut TargetedAccessibility,
    tree: &ElementTree,
    selector: &str,
    key_spec: &str,
) -> CliResult<OperationResult> {
    if !adapter.supports_keystroke() {
        return Err(CliError::runtime(format!(
            "Error: Keystroke injection is not supported on {}.",
            adapter.platform_name()
        )));
    }

    if !query_has_matches(adapter, tree, selector)? {
        return Ok(OperationResult::NotFound(format!(
            "No element found for: {selector}"
        )));
    }

    match adapter.focus_element(selector, tree).await {
        Ok(id) => {
            if let Some(elem) = adapter.get_element(id) {
                println!(
                    "Focused element [{}] {:?} \"{}\"",
                    id,
                    elem.role,
                    elem.display_label()
                );
            }
        }
        Err(e) => return Err(CliError::runtime(format!("Focus failed: {e}"))),
    }

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    match adapter.send_keystroke(key_spec).await {
        Ok((key, modifiers)) => {
            if modifiers.is_empty() {
                println!("Sent keystroke: {:?}", key);
            } else {
                println!("Sent keystroke: {:?}+{:?}", modifiers, key);
            }
            Ok(OperationResult::Success)
        }
        Err(e) => Err(CliError::runtime(format!(
            "Keystroke failed: {e}\nExamples: enter, space, cmd+c, ctrl+shift+a"
        ))),
    }
}

pub async fn hit_test(adapter: &mut TargetedAccessibility, x: f64, y: f64) -> CliResult<()> {
    if !adapter.supports_hit_test() {
        return Err(CliError::runtime(format!(
            "Hit test is not supported on {}",
            adapter.platform_name()
        )));
    }

    match adapter.hit_test(x, y).await {
        Ok(Some(id)) => {
            if let Some(elem) = adapter.get_element(id) {
                println!(
                    "Hit at ({}, {}): [{}] {:?} \"{}\"",
                    x,
                    y,
                    id,
                    elem.role,
                    elem.display_label()
                );
            } else {
                println!("Hit at ({x}, {y}): element [{id}]");
            }
            Ok(())
        }
        Ok(None) => {
            println!("No element at ({x}, {y})");
            Ok(())
        }
        Err(e) => Err(CliError::runtime(format!("Hit test failed: {e}"))),
    }
}

pub async fn mouse_click(
    adapter: &mut TargetedAccessibility,
    x: f64,
    y: f64,
    button: accessibility_core::input::MouseButton,
) -> CliResult<()> {
    if !adapter.supports_mouse_click() {
        return Err(CliError::runtime(format!(
            "Mouse clicks are not supported on {}",
            adapter.platform_name()
        )));
    }

    match adapter.mouse_click_at(x, y, button).await {
        Ok(()) => {
            println!("Clicked at ({x}, {y}) with {button} button");
            Ok(())
        }
        Err(e) => Err(CliError::runtime(format!("Mouse click failed: {e}"))),
    }
}

pub async fn list_windows(adapter: &TargetedAccessibility, output: &OutputArgs) -> CliResult<()> {
    let windows = adapter.list_windows().await;

    if output.output_format() == OutputFormat::Json {
        let rows = windows
            .iter()
            .map(|(pid, app_name, window_title, focused)| {
                serde_json::json!({
                    "pid": pid,
                    "app_name": app_name,
                    "window_title": window_title,
                    "focused": focused,
                })
            })
            .collect::<Vec<_>>();
        let json = serde_json::to_string_pretty(&rows)
            .map_err(|e| CliError::runtime(format!("Failed to serialize window list: {e}")))?;
        println!("{json}");
        return Ok(());
    }

    if windows.is_empty() {
        println!("No windows found.");
        return Ok(());
    }

    println!("{:<8} {:<8} {:<28} Window", "PID", "Focused", "App");
    for (pid, app_name, window_title, focused) in windows {
        let focused = if focused { "*" } else { "" };
        println!(
            "{:<8} {:<8} {:<28} {}",
            pid,
            focused,
            truncate(&app_name, 28),
            window_title
        );
    }
    Ok(())
}

fn query_has_matches(
    adapter: &TargetedAccessibility,
    tree: &ElementTree,
    selector: &str,
) -> CliResult<bool> {
    adapter
        .find_elements(tree, Some(selector), false)
        .map(|elements| !elements.is_empty())
        .map_err(|e| CliError::runtime(e.to_string()))
}

fn print_element_action(adapter: &TargetedAccessibility, id: ElementKey, verb: &str) {
    if let Some(elem) = adapter.get_element(id) {
        println!(
            "{} element [{}] {:?} \"{}\"",
            verb,
            id,
            elem.role,
            elem.display_label()
        );
    } else {
        println!("{verb} element [{id}]");
    }
}

fn filter_tree_to_matches(tree: &ElementTree, matches: &[&Element]) -> ElementTree {
    let match_ids = unique_query_match_ids(matches);
    let root = prune_tree_to_matches(&tree.root, &match_ids).unwrap_or_else(|| tree.root.clone());
    let element_count = count_tree_elements(&root);

    ElementTree {
        version: tree.version,
        pid: tree.pid,
        app_name: tree.app_name.clone(),
        root,
        element_count,
    }
}

fn unique_query_match_ids(matches: &[&Element]) -> HashSet<ElementKey> {
    let mut seen = HashSet::new();
    let mut ids = HashSet::new();

    for element in matches {
        let Some(key) = query_match_dedupe_key(element) else {
            ids.insert(element.id);
            continue;
        };

        if seen.insert(key) {
            ids.insert(element.id);
        }
    }

    ids
}

fn query_match_dedupe_key(element: &Element) -> Option<String> {
    let bounds = element.bounds?;
    let bounds = (
        bounds.origin.x.round() as i64,
        bounds.origin.y.round() as i64,
        bounds.size.width.round() as i64,
        bounds.size.height.round() as i64,
    );

    Some(format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{:?}",
        element.role,
        element.title,
        element.description,
        element.value,
        element.url,
        element.help,
        element.identifier,
        element.role_description,
        element.enabled,
        element.focused,
        element.actions.join("\x1f"),
        element.children.is_empty(),
        bounds
    ))
}

fn prune_tree_to_matches(root: &Element, match_ids: &HashSet<ElementKey>) -> Option<Element> {
    enum Frame<'a> {
        Enter(&'a Element),
        Exit(&'a Element, usize),
    }

    let mut frames = vec![Frame::Enter(root)];
    let mut kept: Vec<Option<Element>> = Vec::new();

    while let Some(frame) = frames.pop() {
        match frame {
            Frame::Enter(element) => {
                frames.push(Frame::Exit(element, element.children.len()));
                for child in element.children.iter().rev() {
                    frames.push(Frame::Enter(child));
                }
            }
            Frame::Exit(element, child_count) => {
                let mut children = Vec::new();
                for _ in 0..child_count {
                    if let Some(child) = kept.pop().flatten() {
                        children.push(child);
                    }
                }
                children.reverse();

                if element.id == root.id || match_ids.contains(&element.id) || !children.is_empty()
                {
                    let mut element = element.clone();
                    element.children = children;
                    kept.push(Some(element));
                } else {
                    kept.push(None);
                }
            }
        }
    }

    kept.pop().flatten()
}

fn count_tree_elements(root: &Element) -> usize {
    let mut count = 0;
    let mut stack = vec![root];
    while let Some(element) = stack.pop() {
        count += 1;
        for child in element.children.iter().rev() {
            stack.push(child);
        }
    }
    count
}

pub fn element_overlaps_bounds(element: &Element, screen_bounds: &Rect) -> bool {
    let Some(bounds) = &element.bounds else {
        return false;
    };

    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return false;
    }

    let bounds_right = bounds.origin.x + bounds.size.width;
    let bounds_bottom = bounds.origin.y + bounds.size.height;
    let screen_right = screen_bounds.origin.x + screen_bounds.size.width;
    let screen_bottom = screen_bounds.origin.y + screen_bounds.size.height;

    bounds.origin.x < screen_right
        && bounds_right > screen_bounds.origin.x
        && bounds.origin.y < screen_bottom
        && bounds_bottom > screen_bounds.origin.y
}

#[cfg(test)]
mod tests {
    use super::*;
    use accessibility_core::accessibility::roles::parse_role_name;
    use accessibility_core::accessibility::{Point, Size};

    macro_rules! role {
        ($name:expr) => {
            parse_role_name($name).expect("test role should parse")
        };
    }

    fn bounds(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect::new(Point::new(x, y), Size::new(width, height))
    }

    fn find_element_by_id(element: &Element, id: ElementKey) -> Option<&Element> {
        let mut stack = vec![element];
        while let Some(current) = stack.pop() {
            if current.id == id {
                return Some(current);
            }
            for child in current.children.iter().rev() {
                stack.push(child);
            }
        }
        None
    }

    fn count_elements_matching(element: &Element, matches: impl Fn(&Element) -> bool) -> usize {
        let mut count = 0;
        let mut stack = vec![element];
        while let Some(current) = stack.pop() {
            if matches(current) {
                count += 1;
            }
            for child in current.children.iter().rev() {
                stack.push(child);
            }
        }
        count
    }

    fn duplicate_message_branch(container_id: u64, message_id: u64, reply_id: u64) -> Element {
        let mut container = Element::new(ElementKey::from_ffi(container_id), role!("Group"));
        let mut message = Element::new(ElementKey::from_ffi(message_id), role!("Group"));
        message.title = Some("eveeifyeve replying to Evan Almloff , Same message".to_string());
        message.bounds = Some(bounds(265.0, 244.0, 966.0, 70.0));
        message.actions = vec!["AXShowMenu".to_string()];

        let mut reply = Element::new(ElementKey::from_ffi(reply_id), role!("Group"));
        reply.description = Some("eveeifyeve replying to Evan Almloff".to_string());
        reply.bounds = Some(bounds(337.0, 246.0, 870.0, 18.0));
        reply.actions = vec!["AXShowMenu".to_string()];

        message.children.push(reply);
        container.children.push(message);
        container
    }

    #[test]
    fn query_tree_filter_dedupes_visual_duplicate_matches() {
        let mut root = Element::new(ElementKey::from_ffi(1), role!("Application"));
        let mut window = Element::new(ElementKey::from_ffi(2), role!("Window"));
        let mut web_view = Element::new(ElementKey::from_ffi(3), role!("WebView"));

        web_view.children.push(duplicate_message_branch(4, 5, 6));
        web_view.children.push(duplicate_message_branch(7, 8, 9));
        window.children.push(web_view);
        root.children.push(window);

        let tree = ElementTree {
            version: 1,
            pid: None,
            app_name: None,
            root,
            element_count: 9,
        };
        let matches = [6, 9]
            .iter()
            .map(|id| {
                find_element_by_id(&tree.root, ElementKey::from_ffi(*id))
                    .expect("test element should exist")
            })
            .collect::<Vec<_>>();

        let filtered = filter_tree_to_matches(&tree, &matches);

        assert_eq!(
            count_elements_matching(&filtered.root, |element| {
                element.description.as_deref() == Some("eveeifyeve replying to Evan Almloff")
            }),
            1
        );
        assert!(find_element_by_id(&filtered.root, ElementKey::from_ffi(6)).is_some());
        assert!(find_element_by_id(&filtered.root, ElementKey::from_ffi(9)).is_none());
    }

    #[test]
    fn query_match_dedupe_keeps_unbounded_matches_distinct() {
        let mut first = Element::new(ElementKey::from_ffi(1), role!("Group"));
        first.description = Some("same text".to_string());
        let mut second = Element::new(ElementKey::from_ffi(2), role!("Group"));
        second.description = Some("same text".to_string());

        let ids = unique_query_match_ids(&[&first, &second]);

        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&ElementKey::from_ffi(1)));
        assert!(ids.contains(&ElementKey::from_ffi(2)));
    }
}
