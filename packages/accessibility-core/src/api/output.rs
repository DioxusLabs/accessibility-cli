//! Output formatting utilities.

use crate::accessibility::{Element, ElementTree};
use accesskit::Role;
use std::collections::HashMap;

/// Trait for printing accessibility trees in different formats.
pub trait Printer {
    /// Print the accessibility tree.
    fn print(&self, tree: &ElementTree);
}

/// Output format for accessibility trees and element lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// Human-readable tree output.
    #[default]
    Tree,
    /// JSON output.
    Json,
    /// Compact LLM-friendly output.
    Llm,
    /// Queryable LLM-friendly selector output.
    LlmQuery,
}

/// Printer that renders a tree using a selected output format.
#[derive(Debug, Clone, Copy, Default)]
pub struct OutputPrinter {
    /// Output format to render.
    pub format: OutputFormat,
    /// Only print structure for LLM formats.
    pub structure_only: bool,
}

impl OutputPrinter {
    /// Create a new output printer.
    pub fn new(format: OutputFormat, structure_only: bool) -> Self {
        Self {
            format,
            structure_only,
        }
    }
}

impl Printer for OutputPrinter {
    fn print(&self, tree: &ElementTree) {
        match self.format {
            OutputFormat::Tree => TreePrinter.print(tree),
            OutputFormat::Json => JsonPrinter.print(tree),
            OutputFormat::Llm => LlmPrinter::new(self.structure_only).print(tree),
            OutputFormat::LlmQuery => LlmQueryPrinter::new(self.structure_only).print(tree),
        }
    }
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

fn format_role_query_name(role: Role) -> &'static str {
    match role {
        Role::Application => "Application",
        Role::Window => "Window",
        Role::Dialog => "Dialog",
        Role::Button => "Button",
        Role::Link => "Link",
        Role::TextInput => "TextInput",
        Role::MultilineTextInput => "MultilineTextInput",
        Role::CheckBox => "CheckBox",
        Role::RadioButton => "RadioButton",
        Role::ComboBox => "ComboBox",
        Role::Slider => "Slider",
        Role::Tab => "Tab",
        Role::TabList => "TabList",
        Role::MenuItem => "MenuItem",
        Role::MenuBar => "MenuBar",
        Role::Menu => "Menu",
        Role::MenuItemCheckBox => "MenuItemCheckBox",
        Role::MenuItemRadio => "MenuItemRadio",
        Role::Switch => "Switch",
        Role::SpinButton => "SpinButton",
        Role::ProgressIndicator => "ProgressIndicator",
        Role::Image => "Image",
        Role::TextRun => "TextRun",
        Role::Label => "Label",
        Role::Group => "Group",
        Role::List => "List",
        Role::ListItem => "ListItem",
        Role::Cell => "Cell",
        Role::Row => "Row",
        Role::Table => "Table",
        Role::ScrollView => "ScrollView",
        Role::Toolbar => "Toolbar",
        Role::Article => "Article",
        Role::Navigation => "Navigation",
        Role::Region => "Region",
        Role::Banner => "Banner",
        Role::Complementary => "Complementary",
        Role::ContentInfo => "ContentInfo",
        Role::Main => "Main",
        Role::Search => "Search",
        Role::Form => "Form",
        Role::Section => "Section",
        Role::Document => "Document",
        Role::WebView => "WebView",
        Role::Heading => "Heading",
        _ => "*",
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

fn format_attr_selector(name: &str, value: &str) -> String {
    let mut serialized = String::new();
    cssparser::serialize_string(value, &mut serialized)
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

/// Print an element list using the selected output format.
pub fn print_elements_formatted(elements: &[&Element], format: OutputFormat) {
    match format {
        OutputFormat::Tree => {
            println!(
                "Found {} match{}:",
                elements.len(),
                if elements.len() == 1 { "" } else { "es" }
            );
            for elem in elements {
                print_element_summary(elem);
            }
        }
        OutputFormat::Json => match serde_json::to_string_pretty(elements) {
            Ok(json) => println!("{}", json),
            Err(e) => eprintln!("Failed to serialize elements: {}", e),
        },
        OutputFormat::Llm => {
            for elem in elements {
                println!("{}", format_element_concise_line(elem));
            }
        }
        OutputFormat::LlmQuery => {
            for elem in elements {
                println!("{}", format_element_selector(elem));
            }
        }
    }
}

/// Format an element as a CSS selector string.
pub fn format_element_selector(elem: &Element) -> String {
    let role_str = format_role_query_name(elem.role);
    let mut attrs: Vec<String> = Vec::new();

    attrs.push(format_attr_selector("data-id", &elem.id.to_string()));
    attrs.push(format_attr_selector("role", &format!("{:?}", elem.role)));

    if let Some(title) = elem.title.as_ref().filter(|s| !s.is_empty()) {
        attrs.push(format_attr_selector("title", title));
    }

    if let Some(desc) = elem.description.as_ref().filter(|s| !s.is_empty())
        && elem.title.as_deref() != Some(desc.as_str())
    {
        attrs.push(format_attr_selector("description", desc));
    }

    if let Some(value) = elem.value.as_ref().filter(|s| !s.is_empty())
        && elem.title.as_deref() != Some(value.as_str())
    {
        attrs.push(format_attr_selector("value", value));
    }

    if let Some(url) = elem.url.as_ref().filter(|s| !s.is_empty()) {
        attrs.push(format_attr_selector("url", url));
    }

    if let Some(help) = elem.help.as_ref().filter(|s| !s.is_empty()) {
        attrs.push(format_attr_selector("help", help));
    }

    if let Some(identifier) = elem.identifier.as_ref().filter(|s| !s.is_empty()) {
        attrs.push(format_attr_selector("identifier", identifier));
    }

    if let Some(role_description) = elem.role_description.as_ref().filter(|s| !s.is_empty()) {
        attrs.push(format_attr_selector("role-description", role_description));
    }

    let actions = format_actions_query_value(&elem.actions);
    if !actions.is_empty() {
        attrs.push(format_attr_selector("actions", &actions));
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
    println!("{}", format_element_concise_line(elem));
}

fn format_element_concise_line(elem: &Element) -> String {
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
        format!("[{}] {} {}", elem.id, role_str, pos)
    } else {
        format!(
            "[{}] {} \"{}\" {}",
            elem.id,
            role_str,
            truncate(label, 40),
            pos
        )
    }
}

/// Print verbose LLM format with CSS-like selectors.
fn print_llm_query_format(
    root: &Element,
    _app_name: Option<&str>,
    _pid: Option<u32>,
    structure_only: bool,
) {
    for line in format_llm_query_lines(root, structure_only) {
        println!("{}", line);
    }
}

fn format_llm_query_lines(root: &Element, structure_only: bool) -> Vec<String> {
    let mut lines = vec![format_element_selector(root), String::new()];
    if structure_only {
        for child in &root.children {
            collect_structure_node_lines(child, 0, &mut lines);
        }
        return lines;
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
        collect_window_llm_lines(window, &mut lines);
        lines.push(String::new());
    }

    if let Some(mb) = menubar {
        collect_menubar_llm_lines(mb, &mut lines);
        lines.push(String::new());
    }

    if !other_interactive.is_empty() {
        for elem in other_interactive {
            lines.push(format_element_llm_line(elem, 0));
        }
    }

    lines
}

fn print_structure_node(element: &Element, indent: usize) {
    let mut lines = Vec::new();
    collect_structure_node_lines(element, indent, &mut lines);
    for line in lines {
        println!("{}", line);
    }
}

fn collect_structure_node_lines(element: &Element, indent: usize, lines: &mut Vec<String>) {
    let prefix = "  ".repeat(indent);
    let is_structural = is_structural_node(element);

    if is_structural || indent == 0 {
        lines.push(format!("{}{}", prefix, format_element_selector(element)));

        if !element.children.is_empty() {
            for child in &element.children {
                if is_structural_node(child) || has_structural_descendants(child) {
                    collect_structure_node_lines(child, indent + 1, lines);
                }
            }
        }
    } else if has_structural_descendants(element) {
        for child in &element.children {
            collect_structure_node_lines(child, indent, lines);
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

fn collect_window_llm_lines(window: &Element, lines: &mut Vec<String>) {
    let mut all_interactive: Vec<&Element> = Vec::new();
    for child in &window.children {
        collect_interactive(child, &mut all_interactive);
    }

    lines.push(format_element_selector(window));

    if !all_interactive.is_empty() {
        for child in &window.children {
            collect_element_hierarchical_lines(child, 1, lines);
        }
    }
}

fn collect_element_hierarchical_lines(element: &Element, indent: usize, lines: &mut Vec<String>) {
    let capped_indent = indent.min(8);
    let is_container = is_meaningful_container(element);
    let interactive_children = count_interactive_descendants(element);

    if is_container && interactive_children > 0 {
        push_container_header_line(element, capped_indent, lines);

        let child_indent = if has_printable_label(element) {
            capped_indent + 1
        } else {
            capped_indent
        };

        for child in &element.children {
            collect_element_hierarchical_lines(child, child_indent, lines);
        }
    } else if is_llm_relevant(element) {
        lines.push(format_element_llm_line(element, capped_indent));
    } else {
        for child in &element.children {
            collect_element_hierarchical_lines(child, capped_indent, lines);
        }
    }
}

fn has_printable_label(elem: &Element) -> bool {
    elem.title.as_ref().is_some_and(|t| !t.is_empty())
        || elem.description.as_ref().is_some_and(|d| !d.is_empty())
}

fn push_container_header_line(elem: &Element, indent: usize, lines: &mut Vec<String>) {
    let prefix = "  ".repeat(indent);

    lines.push(format!("{}{}", prefix, format_element_selector(elem)));
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

fn collect_menubar_llm_lines(menubar: &Element, lines: &mut Vec<String>) {
    lines.push(format_element_selector(menubar));
    for item in &menubar.children {
        if item.role == Role::MenuItem {
            lines.push(format_element_llm_line(item, 1));
        }
    }
}

fn format_element_llm_line(elem: &Element, indent: usize) -> String {
    let prefix = "  ".repeat(indent);
    let selector = format_element_selector(elem);
    format!("{}{}", prefix, selector)
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

fn format_actions_query_value(actions: &[String]) -> String {
    actions
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
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accessibility::{ElementKey, find_matches, parse_query};

    fn make_output_round_trip_tree() -> ElementTree {
        let mut root = Element::new(ElementKey::from_ffi(1), Role::Application);
        root.title = Some("Test App".to_string());

        let mut window = Element::new(ElementKey::from_ffi(2), Role::Window);
        window.title = Some("Main Window".to_string());

        let mut group = Element::new(ElementKey::from_ffi(3), Role::Group);
        group.title = Some("Primary Controls".to_string());

        let mut list = Element::new(ElementKey::from_ffi(4), Role::List);
        list.title = Some("Actions".to_string());

        let mut button = Element::new(ElementKey::from_ffi(5), Role::Button);
        button.title = Some("Run".to_string());
        button.actions = vec!["AXPress".to_string()];

        let mut text = Element::new(ElementKey::from_ffi(6), Role::TextRun);
        text.value = Some("Status: ready".to_string());

        list.children.push(button);
        group.children.push(list);
        group.children.push(text);
        window.children.push(group);

        let mut menubar = Element::new(ElementKey::from_ffi(7), Role::MenuBar);
        let mut apple = Element::new(ElementKey::from_ffi(8), Role::MenuItem);
        apple.title = Some("Apple".to_string());
        apple.actions = vec!["AXPress".to_string(), "AXPick".to_string()];
        let mut edit = Element::new(ElementKey::from_ffi(9), Role::MenuItem);
        edit.title = Some("Edit".to_string());
        edit.actions = vec!["AXPress".to_string()];
        menubar.children.push(apple);
        menubar.children.push(edit);

        let mut link = Element::new(ElementKey::from_ffi(10), Role::Link);
        link.title = Some("Docs".to_string());
        link.url = Some("https://example.test/docs?q=\"roundtrip\"".to_string());

        root.children.push(window);
        root.children.push(menubar);
        root.children.push(link);

        ElementTree {
            version: 1,
            pid: Some(123),
            app_name: Some("Test App".to_string()),
            root,
            element_count: 10,
        }
    }

    fn assert_llm_query_output_round_trips(tree: &ElementTree, structure_only: bool) {
        for raw_line in format_llm_query_lines(&tree.root, structure_only) {
            let line = raw_line.trim();
            if line.is_empty() {
                continue;
            }

            let parsed = parse_query(line).unwrap_or_else(|err| panic!("{line}: {err}"));
            let matches = find_matches(&parsed, tree);
            assert_eq!(matches.len(), 1, "{line}");
        }
    }

    #[test]
    fn llm_menu_item_line_uses_query_selector_syntax() {
        let mut item = Element::new(ElementKey::from_ffi(4_294_967_299), Role::MenuItem);
        item.title = Some("Apple".to_string());
        item.actions = vec![
            "AXCancel".to_string(),
            "AXPress".to_string(),
            "AXPick".to_string(),
        ];

        assert_eq!(
            format_element_llm_line(&item, 1),
            "  MenuItem[data-id=\"4294967299\"][role=\"MenuItem\"][title=\"Apple\"][actions=\"cancel click pick\"]"
        );
    }

    #[test]
    fn formatted_selector_round_trips_full_escaped_attributes() {
        let mut button = Element::new(ElementKey::from_ffi(42), Role::Button);
        button.title =
            Some("Say \"hi\"\\again with enough text to prove it is not truncated".to_string());
        button.description = Some("A \"quoted\" description".to_string());
        button.help = Some("Help text".to_string());
        button.identifier = Some("primary-button".to_string());
        button.role_description = Some("button".to_string());
        button.actions = vec!["AXPress".to_string()];

        let selector = format_element_selector(&button);
        let parsed = parse_query(&selector).unwrap();
        let tree = ElementTree {
            version: 1,
            pid: None,
            app_name: None,
            root: button,
            element_count: 1,
        };

        let matches = find_matches(&parsed, &tree);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, ElementKey::from_ffi(42));
    }

    #[test]
    fn formatted_selectors_round_trip_for_roles_emitted_by_llm_query() {
        let roles = [
            Role::Application,
            Role::Window,
            Role::Dialog,
            Role::Button,
            Role::Link,
            Role::TextInput,
            Role::MultilineTextInput,
            Role::CheckBox,
            Role::RadioButton,
            Role::ComboBox,
            Role::Slider,
            Role::Tab,
            Role::TabList,
            Role::MenuItem,
            Role::MenuBar,
            Role::Menu,
            Role::MenuItemCheckBox,
            Role::MenuItemRadio,
            Role::Switch,
            Role::SpinButton,
            Role::ProgressIndicator,
            Role::Image,
            Role::TextRun,
            Role::Label,
            Role::Group,
            Role::List,
            Role::ListItem,
            Role::Cell,
            Role::Row,
            Role::Table,
            Role::ScrollView,
            Role::Toolbar,
            Role::Article,
            Role::Navigation,
            Role::Region,
            Role::Banner,
            Role::Complementary,
            Role::ContentInfo,
            Role::Main,
            Role::Search,
            Role::Form,
            Role::Section,
            Role::Document,
            Role::WebView,
            Role::Heading,
            Role::Unknown,
        ];

        for (index, role) in roles.into_iter().enumerate() {
            let id = ElementKey::from_ffi(index as u64 + 1);
            let element = Element::new(id, role);
            let selector = format_element_selector(&element);
            let parsed = parse_query(&selector).unwrap_or_else(|err| panic!("{selector}: {err}"));
            let tree = ElementTree {
                version: 1,
                pid: None,
                app_name: None,
                root: element,
                element_count: 1,
            };

            let matches = find_matches(&parsed, &tree);
            assert_eq!(matches.len(), 1, "{selector}");
            assert_eq!(matches[0].id, id, "{selector}");
        }
    }

    #[test]
    fn every_llm_query_output_line_round_trips() {
        let tree = make_output_round_trip_tree();
        assert_llm_query_output_round_trips(&tree, false);
        assert_llm_query_output_round_trips(&tree, true);
    }
}
