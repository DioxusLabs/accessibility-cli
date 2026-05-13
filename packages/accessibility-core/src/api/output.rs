//! Output formatting utilities.

use crate::accessibility::{Element, ElementTree};
use accesskit::Role;
use std::collections::HashMap;

/// Trait for printing accessibility trees in different formats.
pub trait Printer {
    /// Print the accessibility tree.
    fn print(&self, tree: &ElementTree);
}

/// Human-readable tree printer with CSS selectors.
#[derive(Default)]
pub struct TreePrinter;

impl Printer for TreePrinter {
    fn print(&self, tree: &ElementTree) {
        print_tree(&tree.root, 0);
    }
}

/// JSON printer.
#[derive(Default)]
pub struct JsonPrinter;

impl Printer for JsonPrinter {
    fn print(&self, tree: &ElementTree) {
        match serde_json::to_string_pretty(tree) {
            Ok(json) => println!("{}", json),
            Err(e) => eprintln!("Failed to serialize tree: {}", e),
        }
    }
}

/// Concise LLM format printer (compact, one line per element).
#[derive(Default)]
pub struct LlmPrinter {
    /// Only print structure (containers), not individual elements.
    pub structure_only: bool,
}

impl LlmPrinter {
    /// Create a new LLM printer.
    pub fn new(structure_only: bool) -> Self {
        Self { structure_only }
    }
}

impl Printer for LlmPrinter {
    fn print(&self, tree: &ElementTree) {
        print_llm_concise(
            &tree.root,
            tree.app_name.as_deref(),
            tree.pid,
            self.structure_only,
        );
    }
}

/// Verbose LLM format printer with CSS-like selectors and hierarchy.
#[derive(Default)]
pub struct LlmQueryPrinter {
    /// Only print structure (containers), not individual elements.
    pub structure_only: bool,
}

impl LlmQueryPrinter {
    /// Create a new LLM query printer.
    pub fn new(structure_only: bool) -> Self {
        Self { structure_only }
    }
}

impl Printer for LlmQueryPrinter {
    fn print(&self, tree: &ElementTree) {
        print_llm_query_format(
            &tree.root,
            tree.app_name.as_deref(),
            tree.pid,
            self.structure_only,
        );
    }
}

/// Print the accessibility tree using the given printer.
pub fn print_formatted(tree: &ElementTree, printer: &dyn Printer) {
    printer.print(tree);
}

/// Format a role for short display.
pub fn format_role_short(role: Role) -> &'static str {
    match role {
        Role::Application => "App",
        Role::Window => "Window",
        Role::Dialog => "Dialog",
        Role::Button => "Button",
        Role::Link => "Link",
        Role::TextInput => "TextField",
        Role::MultilineTextInput => "TextArea",
        Role::CheckBox => "Checkbox",
        Role::RadioButton => "Radio",
        Role::ComboBox => "Dropdown",
        Role::Slider => "Slider",
        Role::Tab => "Tab",
        Role::TabList => "TabList",
        Role::MenuItem => "MenuItem",
        Role::MenuBar => "MenuBar",
        Role::Menu => "Menu",
        Role::MenuItemCheckBox => "MenuCheck",
        Role::MenuItemRadio => "MenuRadio",
        Role::Switch => "Switch",
        Role::SpinButton => "Spinner",
        Role::ProgressIndicator => "Progress",
        Role::Image => "Image",
        Role::TextRun => "Text",
        Role::Label => "Text",
        Role::Group => "Group",
        Role::List => "List",
        Role::ListItem => "Item",
        Role::Cell => "Cell",
        Role::Row => "Row",
        Role::Table => "Table",
        Role::ScrollView => "ScrollView",
        Role::Toolbar => "Toolbar",
        Role::Article => "Article",
        Role::Navigation => "Nav",
        Role::Region => "Region",
        Role::Banner => "Banner",
        Role::Main => "Main",
        Role::Search => "Search",
        Role::Form => "Form",
        Role::Section => "Section",
        Role::Document => "WebView",
        Role::Heading => "Header",
        _ => "Element",
    }
}

/// Truncate a string with ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!(
            "{}...",
            s.chars().take(max.saturating_sub(3)).collect::<String>()
        )
    }
}

/// Escape special characters.
pub fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn format_attr_selector(name: &str, value: &str, max: usize) -> String {
    let truncated = truncate(value, max);
    let mut serialized = String::new();
    cssparser::serialize_string(&truncated, &mut serialized)
        .expect("serializing a CSS string into String should not fail");
    format!("[{}={}]", name, serialized)
}

/// Print element summary for REPL/query output.
pub fn print_element_summary(elem: &Element) {
    let role_str = format_role_short(elem.role);
    let label = elem.display_label();

    let bounds_str = elem
        .bounds
        .as_ref()
        .map(|b| {
            format!(
                "({:.0},{:.0} {:.0}x{:.0})",
                b.origin.x, b.origin.y, b.size.width, b.size.height
            )
        })
        .unwrap_or_default();

    let flags = [
        if elem.enabled { "" } else { "disabled" },
        if elem.focused { "focused" } else { "" },
    ]
    .iter()
    .filter(|s| !s.is_empty())
    .cloned()
    .collect::<Vec<_>>()
    .join(" ");

    println!(
        "  [{}] {} \"{}\" {} {}",
        elem.id,
        role_str,
        truncate(&label, 40),
        bounds_str,
        flags
    );
}

/// Format an element as a CSS selector string.
pub fn format_element_selector(elem: &Element) -> String {
    let role_str = format_role_short(elem.role);
    let mut attrs: Vec<String> = Vec::new();

    if let Some(title) = elem.title.as_ref().filter(|s| !s.is_empty()) {
        attrs.push(format_attr_selector("title", title, 50));
    }

    if let Some(desc) = elem.description.as_ref().filter(|s| !s.is_empty())
        && elem.title.as_deref() != Some(desc.as_str())
    {
        attrs.push(format_attr_selector("description", desc, 50));
    }

    if let Some(value) = elem.value.as_ref().filter(|s| !s.is_empty())
        && elem.title.as_deref() != Some(value.as_str())
    {
        attrs.push(format_attr_selector("value", value, 40));
    }

    if let Some(url) = elem.url.as_ref().filter(|s| !s.is_empty()) {
        attrs.push(format_attr_selector("url", url, 50));
    }

    format!("{}{}", role_str, attrs.join(""))
}

/// Print human-readable tree using CSS selector format.
pub fn print_tree(element: &Element, indent: usize) {
    let prefix = "  ".repeat(indent);
    let selector = format_element_selector(element);

    let mut status = Vec::new();
    if element.focused {
        status.push("FOCUSED");
    }
    if !element.enabled {
        status.push("disabled");
    }
    let status_str = if status.is_empty() {
        String::new()
    } else {
        format!(" [{}]", status.join(", "))
    };

    println!("{}[{}] {}{}", prefix, element.id, selector, status_str);

    for child in &element.children {
        print_tree(child, indent + 1);
    }
}

/// Print statistics about the tree.
pub fn print_statistics(element: &Element) {
    let mut role_counts: HashMap<Role, usize> = HashMap::new();
    count_roles(element, &mut role_counts);

    let mut counts: Vec<_> = role_counts.into_iter().collect();
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));

    println!("Elements by role:");
    for (role, count) in counts.iter().take(15) {
        println!("  {:?}: {}", role, count);
    }
    if counts.len() > 15 {
        println!("  ... and {} more role types", counts.len() - 15);
    }
}

fn count_roles(element: &Element, counts: &mut HashMap<Role, usize>) {
    *counts.entry(element.role).or_insert(0) += 1;
    for child in &element.children {
        count_roles(child, counts);
    }
}

/// Print concise LLM format (compact, minimal representation).
fn print_llm_concise(
    root: &Element,
    app_name: Option<&str>,
    pid: Option<u32>,
    structure_only: bool,
) {
    println!(
        "# {} (pid: {})",
        app_name.unwrap_or("Unknown"),
        pid.map(|p| p.to_string())
            .unwrap_or_else(|| "?".to_string())
    );

    if structure_only {
        for child in &root.children {
            print_structure_node(child, 0);
        }
        return;
    }

    // Collect all interactive elements with their text labels
    let mut elements: Vec<&Element> = Vec::new();
    for child in &root.children {
        collect_interactive(child, &mut elements);
    }

    // Print compact format: one line per element
    for elem in elements {
        print_element_concise(elem);
    }
}

fn print_element_concise(elem: &Element) {
    let role_str = format_role_short(elem.role);

    // Get the primary label (prefer title, then description, then value)
    let label = if matches!(elem.role, accesskit::Role::TextRun | accesskit::Role::Label) {
        // For text elements, prefer value (actual text content)
        elem.value
            .as_deref()
            .or(elem.title.as_deref())
            .or(elem.description.as_deref())
            .unwrap_or("")
    } else {
        elem.title
            .as_deref()
            .or(elem.description.as_deref())
            .unwrap_or("")
    };

    // Format bounds concisely as (x,y)
    let pos = elem
        .bounds
        .map(|b| format!("({},{})", b.origin.x as i32, b.origin.y as i32))
        .unwrap_or_default();

    // Single line: [id] Role "label" (x,y)
    if label.is_empty() {
        println!("[{}] {} {}", elem.id, role_str, pos);
    } else {
        println!(
            "[{}] {} \"{}\" {}",
            elem.id,
            role_str,
            truncate(label, 40),
            pos
        );
    }
}

/// Print verbose LLM format with CSS-like selectors.
fn print_llm_query_format(
    root: &Element,
    app_name: Option<&str>,
    pid: Option<u32>,
    structure_only: bool,
) {
    println!(
        "# App: {} (pid: {})",
        app_name.unwrap_or("Unknown"),
        pid.map(|p| p.to_string())
            .unwrap_or_else(|| "?".to_string())
    );
    println!();

    if structure_only {
        for child in &root.children {
            print_structure_node(child, 0);
        }
        return;
    }

    let mut windows: Vec<&Element> = Vec::new();
    let mut menubar: Option<&Element> = None;
    let mut other_interactive: Vec<&Element> = Vec::new();

    for child in &root.children {
        match child.role {
            Role::Window | Role::Dialog => windows.push(child),
            Role::MenuBar => menubar = Some(child),
            _ => {
                collect_interactive(child, &mut other_interactive);
            }
        }
    }

    for window in &windows {
        print_window_llm(window);
        println!();
    }

    if let Some(mb) = menubar {
        print_menubar_llm(mb);
        println!();
    }

    if !other_interactive.is_empty() {
        println!("## Other Elements");
        for elem in other_interactive {
            print_element_llm(elem, 0);
        }
    }
}

fn print_structure_node(element: &Element, indent: usize) {
    let prefix = "  ".repeat(indent);
    let total = count_all_descendants(element);
    let interactive = count_interactive_descendants(element);

    let label = element
        .title
        .as_ref()
        .filter(|s| !s.is_empty())
        .or(element.description.as_ref().filter(|s| !s.is_empty()))
        .map(|s| format!(" \"{}\"", truncate(s, 30)))
        .unwrap_or_default();

    let role_str = format_role_short(element.role);
    let is_structural = is_structural_node(element);

    if is_structural || indent == 0 {
        println!(
            "{}[{}] {}{} ({} elements, {} interactive)",
            prefix, element.id, role_str, label, total, interactive
        );

        if !element.children.is_empty() {
            for child in &element.children {
                if is_structural_node(child) || has_structural_descendants(child) {
                    print_structure_node(child, indent + 1);
                }
            }
        }
    } else if has_structural_descendants(element) {
        for child in &element.children {
            print_structure_node(child, indent);
        }
    }
}

fn is_structural_node(elem: &Element) -> bool {
    let is_top_level = matches!(elem.role, Role::Window | Role::Dialog | Role::MenuBar);
    if is_top_level {
        return true;
    }

    let is_grouping = matches!(
        elem.role,
        Role::Group
            | Role::List
            | Role::Toolbar
            | Role::TabList
            | Role::Navigation
            | Role::Form
            | Role::Article
            | Role::Region
            | Role::Banner
            | Role::Main
            | Role::Search
            | Role::Section
            | Role::ScrollView
            | Role::Application
    );

    let has_label = elem.title.as_ref().is_some_and(|t| !t.is_empty())
        || elem.description.as_ref().is_some_and(|d| !d.is_empty());

    is_grouping && has_label
}

fn has_structural_descendants(element: &Element) -> bool {
    for child in &element.children {
        if is_structural_node(child) || has_structural_descendants(child) {
            return true;
        }
    }
    false
}

fn count_all_descendants(element: &Element) -> usize {
    let mut count = 1;
    for child in &element.children {
        count += count_all_descendants(child);
    }
    count
}

fn count_interactive_descendants(element: &Element) -> usize {
    let mut count = 0;
    if is_llm_relevant(element) {
        count += 1;
    }
    for child in &element.children {
        count += count_interactive_descendants(child);
    }
    count
}

fn print_window_llm(window: &Element) {
    let title = window.title.as_deref().unwrap_or("Untitled");
    let bounds_str = window
        .bounds
        .map(|b| format!(" {}x{}", b.size.width as i32, b.size.height as i32))
        .unwrap_or_default();

    let mut all_interactive: Vec<&Element> = Vec::new();
    for child in &window.children {
        collect_interactive(child, &mut all_interactive);
    }

    println!(
        "## [Window] \"{}\"{}  ({} elements)",
        truncate(title, 50),
        bounds_str,
        all_interactive.len()
    );

    if all_interactive.is_empty() {
        println!("  (no interactive elements)");
    } else {
        for child in &window.children {
            print_element_hierarchical(child, 1);
        }
    }
}

fn print_element_hierarchical(element: &Element, indent: usize) {
    let capped_indent = indent.min(8);
    let is_container = is_meaningful_container(element);
    let interactive_children = count_interactive_descendants(element);

    if is_container && interactive_children > 0 {
        print_container_header(element, capped_indent);

        let child_indent = if has_printable_label(element) {
            capped_indent + 1
        } else {
            capped_indent
        };

        for child in &element.children {
            print_element_hierarchical(child, child_indent);
        }
    } else if is_llm_relevant(element) {
        print_element_llm(element, capped_indent);
    } else {
        for child in &element.children {
            print_element_hierarchical(child, capped_indent);
        }
    }
}

fn has_printable_label(elem: &Element) -> bool {
    elem.title.as_ref().is_some_and(|t| !t.is_empty())
        || elem.description.as_ref().is_some_and(|d| !d.is_empty())
}

fn print_container_header(elem: &Element, indent: usize) {
    let prefix = "  ".repeat(indent);

    let role_str = match elem.role {
        Role::Group => "Group",
        Role::List => "List",
        Role::ListItem => "Item",
        Role::Toolbar => "Toolbar",
        Role::TabList => "Tabs",
        Role::Menu => "Menu",
        Role::Dialog => "Dialog",
        Role::Form => "Form",
        Role::Article => "Article",
        Role::Region => "Region",
        Role::Navigation => "Nav",
        Role::Banner => "Banner",
        Role::Complementary => "Aside",
        Role::ContentInfo => "Footer",
        Role::Main => "Main",
        Role::Search => "Search",
        _ => "Section",
    };

    // Collect non-empty attributes as CSS selector syntax
    let mut attrs: Vec<String> = Vec::new();

    if let Some(title) = elem.title.as_ref().filter(|s| !s.is_empty()) {
        attrs.push(format!("[title=\"{}\"]", truncate(title, 40)));
    }

    if let Some(desc) = elem.description.as_ref().filter(|s| !s.is_empty())
        && elem.title.as_deref() != Some(desc.as_str())
    {
        attrs.push(format!("[description=\"{}\"]", truncate(desc, 40)));
    }

    if !attrs.is_empty() {
        let selector = format!("{}{}", role_str, attrs.join(""));
        println!("{}[{}]", prefix, selector);
    }
}

fn is_meaningful_container(elem: &Element) -> bool {
    let is_grouping_role = matches!(
        elem.role,
        Role::Group
            | Role::List
            | Role::ListItem
            | Role::Toolbar
            | Role::TabList
            | Role::Menu
            | Role::Dialog
            | Role::Form
            | Role::Article
            | Role::Region
            | Role::Navigation
            | Role::Banner
            | Role::Complementary
            | Role::ContentInfo
            | Role::Main
            | Role::Search
            | Role::Section
    );

    if !is_grouping_role {
        return false;
    }

    let has_label = elem.title.as_ref().is_some_and(|t| !t.is_empty())
        || elem.description.as_ref().is_some_and(|d| !d.is_empty());

    let interactive_count = count_interactive_descendants(elem);

    has_label || interactive_count >= 2
}

fn print_menubar_llm(menubar: &Element) {
    println!("## [MenuBar]");
    for item in &menubar.children {
        if item.role == Role::MenuItem {
            print_element_llm(item, 1);
        }
    }
}

fn print_element_llm(elem: &Element, indent: usize) {
    println!("{}", format_element_llm_line(elem, indent));
}

fn format_element_llm_line(elem: &Element, indent: usize) -> String {
    let prefix = "  ".repeat(indent);
    let selector = format_element_selector(elem);

    // Position
    let pos_str = elem
        .bounds
        .map(|b| format!(" ({},{})", b.origin.x as i32, b.origin.y as i32))
        .unwrap_or_default();

    // Actions
    let actions = format_actions_short(&elem.actions);

    format!(
        "{}[{}] {}{} {}",
        prefix, elem.id, selector, pos_str, actions
    )
}

fn collect_interactive<'a>(element: &'a Element, result: &mut Vec<&'a Element>) {
    if is_llm_relevant(element) {
        result.push(element);
    }
    for child in &element.children {
        collect_interactive(child, result);
    }
}

fn is_llm_relevant(elem: &Element) -> bool {
    let has_label = elem.title.as_ref().is_some_and(|t| !t.is_empty())
        || elem.description.as_ref().is_some_and(|d| !d.is_empty())
        || elem.help.as_ref().is_some_and(|h| !h.is_empty())
        || elem.identifier.as_ref().is_some_and(|i| !i.is_empty())
        || elem
            .value
            .as_ref()
            .is_some_and(|v| !v.is_empty() && v.len() > 1);

    // Interactive elements with labels
    if elem.is_interactive() && has_label {
        return true;
    }

    // Links with labels
    if elem.role == Role::Link && has_label {
        return true;
    }

    // TextRun/Label elements with meaningful text content
    if matches!(elem.role, Role::TextRun | Role::Label)
        && let Some(value) = &elem.value
    {
        // Show text runs that have actual content (not just whitespace or bullets)
        let trimmed = value.trim();
        if !trimmed.is_empty() && trimmed.len() > 1 && !trimmed.starts_with('•') {
            return true;
        }
    }

    // Elements with clickable actions (AXPress, AXPick, AXConfirm)
    if elem.has_activation_action() && elem.bounds.is_some() {
        return true;
    }

    // Non-interactive elements with description and actions (like Obsidian sidebar buttons)
    // These are often Group elements that act as buttons
    if has_label
        && !elem.actions.is_empty()
        && elem.bounds.is_some()
        && let Some(bounds) = &elem.bounds
    {
        // Only include if it's a reasonable button size (not full window)
        if bounds.size.width < 200.0 && bounds.size.height < 200.0 {
            return true;
        }
    }

    false
}

fn format_actions_short(actions: &[String]) -> String {
    if actions.is_empty() {
        return String::new();
    }

    let short: Vec<&str> = actions
        .iter()
        .filter_map(|a| match a.as_str() {
            "AXPress" => Some("click"),
            "AXConfirm" => Some("confirm"),
            "AXCancel" => Some("cancel"),
            "AXIncrement" => Some("inc"),
            "AXDecrement" => Some("dec"),
            "AXShowMenu" => Some("menu"),
            "AXPick" => Some("pick"),
            "AXRaise" => Some("raise"),
            _ => None,
        })
        .collect();

    if short.is_empty() {
        String::new()
    } else {
        format!("-> {}", short.join(", "))
    }
}
