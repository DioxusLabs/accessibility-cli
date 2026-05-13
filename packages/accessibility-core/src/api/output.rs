//! Output formatting utilities.

use crate::accessibility::{Element, ElementKey, ElementTree};
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
        print_llm_query_format(tree, self.structure_only);
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

/// Print an element list using tree context when a format can benefit from it.
pub fn print_elements_formatted_with_tree(
    elements: &[&Element],
    format: OutputFormat,
    tree: &ElementTree,
) {
    if format != OutputFormat::LlmQuery {
        print_elements_formatted(elements, format);
        return;
    }

    let mut formatter = MinimalQueryFormatter::new(tree);
    for elem in elements {
        println!("{}", formatter.selector_for(elem));
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

struct MinimalQueryFormatter<'a> {
    tree: &'a ElementTree,
    element_ids: Vec<ElementKey>,
    elements_by_id: HashMap<ElementKey, &'a Element>,
    parent_by_id: HashMap<ElementKey, Option<ElementKey>>,
    child_index_by_id: HashMap<ElementKey, usize>,
    match_index: SelectorMatchIndex<'a>,
    match_cache: HashMap<String, Option<ElementKey>>,
}

impl<'a> MinimalQueryFormatter<'a> {
    fn new(tree: &'a ElementTree) -> Self {
        let mut formatter = Self {
            tree,
            element_ids: Vec::new(),
            elements_by_id: HashMap::new(),
            parent_by_id: HashMap::new(),
            child_index_by_id: HashMap::new(),
            match_index: SelectorMatchIndex::default(),
            match_cache: HashMap::new(),
        };
        formatter.index_tree();
        formatter
    }

    fn selector_for(&mut self, elem: &Element) -> String {
        let Some(mut candidate) = self.unique_candidate_for(elem.id) else {
            return self.id_fallback_selector(elem);
        };

        for removal in candidate.removal_candidates() {
            let mut trial = candidate.clone();
            if !trial.remove(removal.part) {
                continue;
            }
            if self.matches_target(&trial, elem.id) {
                candidate = trial;
            }
        }

        let selector = candidate.to_selector();
        if self.unique_candidate_match_id(&candidate) == Some(elem.id) {
            selector
        } else {
            self.id_fallback_selector(elem)
        }
    }

    fn nested_selector_for(&mut self, scope_id: Option<ElementKey>, elem: &Element) -> String {
        let Some(mut candidate) = self.unique_relative_candidate_for(scope_id, elem.id) else {
            return self.id_fallback_selector(elem);
        };

        for removal in candidate.removal_candidates() {
            let mut trial = candidate.clone();
            if !trial.remove(removal.part) {
                continue;
            }
            if self.matches_relative_target(scope_id, &trial, elem.id) {
                candidate = trial;
            }
        }

        let selector = candidate.to_selector();
        if self.unique_relative_candidate_match_id(scope_id, &candidate) == Some(elem.id) {
            selector
        } else {
            self.id_fallback_selector(elem)
        }
    }

    fn unique_candidate_for(&mut self, target_id: ElementKey) -> Option<SelectorCandidate> {
        if let Some(candidate) = self.maximal_candidate(target_id)
            && self.matches_target(&candidate, target_id)
        {
            return Some(candidate);
        }

        if let Some(candidate) = self.positional_candidate(target_id)
            && self.matches_target(&candidate, target_id)
        {
            return Some(candidate);
        }

        None
    }

    fn unique_relative_candidate_for(
        &mut self,
        scope_id: Option<ElementKey>,
        target_id: ElementKey,
    ) -> Option<SelectorCandidate> {
        if let Some(candidate) = self.relative_semantic_candidate(scope_id, target_id)
            && self.matches_relative_target(scope_id, &candidate, target_id)
        {
            return Some(candidate);
        }

        if let Some(candidate) = self.relative_positional_candidate(scope_id, target_id)
            && self.matches_relative_target(scope_id, &candidate, target_id)
        {
            return Some(candidate);
        }

        None
    }

    fn index_tree(&mut self) {
        let mut stack = vec![(&self.tree.root, None, 0usize)];
        while let Some((current, parent_id, child_index)) = stack.pop() {
            self.element_ids.push(current.id);
            self.elements_by_id.insert(current.id, current);
            self.parent_by_id.insert(current.id, parent_id);
            self.child_index_by_id.insert(current.id, child_index);
            self.match_index.add(current);

            for (index, child) in current.children.iter().enumerate().rev() {
                stack.push((child, Some(current.id), index));
            }
        }
    }

    fn maximal_candidate(&self, target_id: ElementKey) -> Option<SelectorCandidate> {
        let path = self.path_to(target_id)?;
        let target_path_index = path.len().checked_sub(1)?;
        let target_has_semantics = element_has_selector_semantics(path[target_path_index]);
        let steps = path
            .into_iter()
            .enumerate()
            .filter(|(path_index, elem)| {
                *path_index == 0
                    || *path_index == target_path_index
                    || is_top_level_selector_context(elem)
                    || element_has_selector_semantics(elem)
                    || (!target_has_semantics && *path_index + 1 == target_path_index)
            })
            .map(|(path_index, elem)| {
                let nth_child = self
                    .parent_by_id
                    .get(&elem.id)
                    .copied()
                    .flatten()
                    .and_then(|_| self.child_index_by_id.get(&elem.id).copied())
                    .map(|child_index| child_index + 1);
                SelectorStep::new(elem, nth_child, path_index)
            })
            .collect();

        Some(SelectorCandidate { steps })
    }

    fn positional_candidate(&self, target_id: ElementKey) -> Option<SelectorCandidate> {
        let steps = self
            .path_to(target_id)?
            .into_iter()
            .enumerate()
            .map(|(path_index, elem)| {
                let nth_child = self
                    .parent_by_id
                    .get(&elem.id)
                    .copied()
                    .flatten()
                    .and_then(|_| self.child_index_by_id.get(&elem.id).copied())
                    .map(|child_index| child_index + 1);
                SelectorStep::new(elem, nth_child, path_index)
            })
            .collect();

        Some(SelectorCandidate { steps })
    }

    fn relative_semantic_candidate(
        &self,
        scope_id: Option<ElementKey>,
        target_id: ElementKey,
    ) -> Option<SelectorCandidate> {
        let path = self.relative_path(scope_id, target_id)?;
        let target_path_index = path.len().checked_sub(1)?;
        let target_has_semantics = element_has_selector_semantics(path[target_path_index]);
        let steps = path
            .into_iter()
            .enumerate()
            .filter(|(path_index, elem)| {
                *path_index == target_path_index
                    || is_top_level_selector_context(elem)
                    || element_has_selector_semantics(elem)
                    || (!target_has_semantics && *path_index + 1 == target_path_index)
            })
            .map(|(path_index, elem)| {
                let nth_child = self
                    .parent_by_id
                    .get(&elem.id)
                    .copied()
                    .flatten()
                    .and_then(|_| self.child_index_by_id.get(&elem.id).copied())
                    .map(|child_index| child_index + 1);
                SelectorStep::new(elem, nth_child, path_index)
            })
            .collect();

        Some(SelectorCandidate { steps })
    }

    fn relative_positional_candidate(
        &self,
        scope_id: Option<ElementKey>,
        target_id: ElementKey,
    ) -> Option<SelectorCandidate> {
        let steps = self
            .relative_path(scope_id, target_id)?
            .into_iter()
            .enumerate()
            .map(|(path_index, elem)| {
                let nth_child = self
                    .parent_by_id
                    .get(&elem.id)
                    .copied()
                    .flatten()
                    .and_then(|_| self.child_index_by_id.get(&elem.id).copied())
                    .map(|child_index| child_index + 1);
                SelectorStep::new(elem, nth_child, path_index)
            })
            .collect();

        Some(SelectorCandidate { steps })
    }

    fn path_to(&self, target_id: ElementKey) -> Option<Vec<&'a Element>> {
        let mut ids = Vec::new();
        let mut current_id = target_id;

        loop {
            ids.push(current_id);
            match self.parent_by_id.get(&current_id).copied().flatten() {
                Some(parent_id) => current_id = parent_id,
                None => break,
            }
        }

        ids.reverse();
        ids.into_iter()
            .map(|id| self.elements_by_id.get(&id).copied())
            .collect()
    }

    fn relative_path(
        &self,
        scope_id: Option<ElementKey>,
        target_id: ElementKey,
    ) -> Option<Vec<&'a Element>> {
        let path = self.path_to(target_id)?;
        let Some(scope_id) = scope_id else {
            return Some(path);
        };

        let scope_index = path.iter().position(|elem| elem.id == scope_id)?;
        if scope_index + 1 >= path.len() {
            return None;
        }

        Some(path.into_iter().skip(scope_index + 1).collect())
    }

    fn matches_target(&mut self, candidate: &SelectorCandidate, target_id: ElementKey) -> bool {
        self.unique_candidate_match_id(candidate) == Some(target_id)
    }

    fn matches_relative_target(
        &mut self,
        scope_id: Option<ElementKey>,
        candidate: &SelectorCandidate,
        target_id: ElementKey,
    ) -> bool {
        self.unique_relative_candidate_match_id(scope_id, candidate) == Some(target_id)
    }

    fn unique_candidate_match_id(&mut self, candidate: &SelectorCandidate) -> Option<ElementKey> {
        let selector = candidate.to_selector();
        if selector.is_empty() {
            return None;
        }

        if let Some(cached) = self.match_cache.get(&selector) {
            return *cached;
        }

        let unique_id = self.direct_unique_match_id(candidate);

        self.match_cache.insert(selector, unique_id);
        unique_id
    }

    fn unique_relative_candidate_match_id(
        &mut self,
        scope_id: Option<ElementKey>,
        candidate: &SelectorCandidate,
    ) -> Option<ElementKey> {
        let selector = format!("{:?}|{}", scope_id, candidate.to_selector());
        if selector.is_empty() {
            return None;
        }

        if let Some(cached) = self.match_cache.get(&selector) {
            return *cached;
        }

        let unique_id = self.direct_unique_relative_match_id(scope_id, candidate);

        self.match_cache.insert(selector, unique_id);
        unique_id
    }

    fn direct_unique_match_id(&self, candidate: &SelectorCandidate) -> Option<ElementKey> {
        let active_steps = candidate.active_steps();
        if active_steps.is_empty() {
            return None;
        }

        let (_, terminal_step) = active_steps[active_steps.len() - 1];
        let candidate_ids = self.terminal_candidate_ids(terminal_step)?;
        let mut matched = None;
        for element_id in candidate_ids {
            if self
                .matches_active_steps(*element_id, &active_steps, active_steps.len() - 1)
                .is_some()
            {
                if matched.is_some() {
                    return None;
                }
                matched = Some(*element_id);
            }
        }

        matched
    }

    fn direct_unique_relative_match_id(
        &self,
        scope_id: Option<ElementKey>,
        candidate: &SelectorCandidate,
    ) -> Option<ElementKey> {
        let active_steps = candidate.active_steps();
        if active_steps.is_empty() {
            return None;
        }

        let mut matched = None;
        for element in self.elements_by_id.values() {
            let Some(first_step_id) =
                self.matches_active_steps(element.id, &active_steps, active_steps.len() - 1)
            else {
                continue;
            };

            if !self.matches_scope(scope_id, first_step_id, active_steps[0].0) {
                continue;
            }

            if matched.is_some() {
                return None;
            }
            matched = Some(element.id);
        }

        matched
    }

    fn terminal_candidate_ids(&self, step: &SelectorStep) -> Option<&[ElementKey]> {
        let mut best = if step.role == "*" {
            self.element_ids.as_slice()
        } else {
            self.match_index.role.get(step.role)?.as_slice()
        };

        for attr in step.attrs.iter().filter(|attr| attr.active) {
            let attr_ids = self.match_index.attr_ids(attr)?;
            if attr_ids.len() < best.len() {
                best = attr_ids;
            }
        }

        Some(best)
    }

    fn matches_active_steps(
        &self,
        element_id: ElementKey,
        active_steps: &[(usize, &SelectorStep)],
        active_index: usize,
    ) -> Option<ElementKey> {
        let Some(element) = self.elements_by_id.get(&element_id).copied() else {
            return None;
        };
        let (step_index, step) = active_steps[active_index];
        if !step.matches(element, self.child_index_by_id.get(&element_id).copied()) {
            return None;
        }

        if active_index == 0 {
            return Some(element_id);
        }

        let (previous_step_index, _) = active_steps[active_index - 1];
        let parent_id = self.parent_by_id.get(&element_id).copied().flatten();
        if previous_step_index + 1 == step_index {
            return parent_id.and_then(|parent_id| {
                self.matches_active_steps(parent_id, active_steps, active_index - 1)
            });
        }

        let mut ancestor_id = parent_id;
        while let Some(id) = ancestor_id {
            if let Some(first_step_id) =
                self.matches_active_steps(id, active_steps, active_index - 1)
            {
                return Some(first_step_id);
            }
            ancestor_id = self.parent_by_id.get(&id).copied().flatten();
        }

        None
    }

    fn matches_scope(
        &self,
        scope_id: Option<ElementKey>,
        first_step_id: ElementKey,
        first_step_index: usize,
    ) -> bool {
        let Some(scope_id) = scope_id else {
            return true;
        };

        if first_step_index == 0 {
            self.parent_by_id.get(&first_step_id).copied().flatten() == Some(scope_id)
        } else {
            self.is_descendant_of(first_step_id, scope_id)
        }
    }

    fn is_descendant_of(&self, element_id: ElementKey, ancestor_id: ElementKey) -> bool {
        let mut current_id = self.parent_by_id.get(&element_id).copied().flatten();
        while let Some(id) = current_id {
            if id == ancestor_id {
                return true;
            }
            current_id = self.parent_by_id.get(&id).copied().flatten();
        }

        false
    }

    fn id_fallback_selector(&mut self, elem: &Element) -> String {
        let selector = format!(
            "{}{}",
            format_role_query_name(elem.role),
            format_attr_selector("data-id", &elem.id.to_string())
        );

        if self.elements_by_id.contains_key(&elem.id) {
            selector
        } else {
            format_element_selector(elem)
        }
    }
}

#[derive(Default)]
struct SelectorMatchIndex<'a> {
    role: HashMap<&'static str, Vec<ElementKey>>,
    title: HashMap<&'a str, Vec<ElementKey>>,
    description: HashMap<&'a str, Vec<ElementKey>>,
    value: HashMap<&'a str, Vec<ElementKey>>,
    url: HashMap<&'a str, Vec<ElementKey>>,
    help: HashMap<&'a str, Vec<ElementKey>>,
    identifier: HashMap<&'a str, Vec<ElementKey>>,
    role_description: HashMap<&'a str, Vec<ElementKey>>,
    actions: HashMap<String, Vec<ElementKey>>,
}

impl<'a> SelectorMatchIndex<'a> {
    fn add(&mut self, elem: &'a Element) {
        push_indexed_value(&mut self.role, format_role_query_name(elem.role), elem.id);

        if let Some(title) = elem.title.as_deref().filter(|s| !s.is_empty()) {
            push_indexed_value(&mut self.title, title, elem.id);
        }
        if let Some(description) = elem.description.as_deref().filter(|s| !s.is_empty()) {
            push_indexed_value(&mut self.description, description, elem.id);
        }
        if let Some(value) = elem.value.as_deref().filter(|s| !s.is_empty()) {
            push_indexed_value(&mut self.value, value, elem.id);
        }
        if let Some(url) = elem.url.as_deref().filter(|s| !s.is_empty()) {
            push_indexed_value(&mut self.url, url, elem.id);
        }
        if let Some(help) = elem.help.as_deref().filter(|s| !s.is_empty()) {
            push_indexed_value(&mut self.help, help, elem.id);
        }
        if let Some(identifier) = elem.identifier.as_deref().filter(|s| !s.is_empty()) {
            push_indexed_value(&mut self.identifier, identifier, elem.id);
        }
        if let Some(role_description) = elem.role_description.as_deref().filter(|s| !s.is_empty()) {
            push_indexed_value(&mut self.role_description, role_description, elem.id);
        }

        let actions = format_actions_query_value(&elem.actions);
        if actions.is_empty() {
            return;
        }

        push_owned_indexed_value(&mut self.actions, actions.clone(), elem.id);
        for action in actions.split_whitespace() {
            if action != actions {
                push_owned_indexed_value(&mut self.actions, action.to_string(), elem.id);
            }
        }
    }

    fn attr_ids(&self, attr: &SelectorAttr) -> Option<&[ElementKey]> {
        let ids = match attr.kind {
            SelectorAttrKind::Title => self.title.get(attr.value.as_str()),
            SelectorAttrKind::Description => self.description.get(attr.value.as_str()),
            SelectorAttrKind::Value => self.value.get(attr.value.as_str()),
            SelectorAttrKind::Url => self.url.get(attr.value.as_str()),
            SelectorAttrKind::Help => self.help.get(attr.value.as_str()),
            SelectorAttrKind::Identifier => self.identifier.get(attr.value.as_str()),
            SelectorAttrKind::RoleDescription => self.role_description.get(attr.value.as_str()),
            SelectorAttrKind::Actions => self.actions.get(attr.value.as_str()),
        }?;

        Some(ids.as_slice())
    }
}

fn push_indexed_value<K>(map: &mut HashMap<K, Vec<ElementKey>>, value: K, id: ElementKey)
where
    K: Eq + std::hash::Hash,
{
    map.entry(value).or_default().push(id);
}

fn push_owned_indexed_value(
    map: &mut HashMap<String, Vec<ElementKey>>,
    value: String,
    id: ElementKey,
) {
    let ids = map.entry(value).or_default();
    if !ids.contains(&id) {
        ids.push(id);
    }
}

fn is_top_level_selector_context(elem: &Element) -> bool {
    matches!(
        elem.role,
        Role::Application | Role::Window | Role::Dialog | Role::MenuBar
    )
}

fn element_has_selector_semantics(elem: &Element) -> bool {
    elem.title.as_ref().is_some_and(|s| !s.is_empty())
        || elem.description.as_ref().is_some_and(|s| !s.is_empty())
        || elem.value.as_ref().is_some_and(|s| !s.is_empty())
        || elem.url.as_ref().is_some_and(|s| !s.is_empty())
        || elem.help.as_ref().is_some_and(|s| !s.is_empty())
        || elem.identifier.as_ref().is_some_and(|s| !s.is_empty())
        || elem
            .role_description
            .as_ref()
            .is_some_and(|s| !s.is_empty())
        || !format_actions_query_value(&elem.actions).is_empty()
        || elem.focused
        || !elem.enabled
}

#[derive(Clone)]
struct SelectorCandidate {
    steps: Vec<SelectorStep>,
}

impl SelectorCandidate {
    fn active_steps(&self) -> Vec<(usize, &SelectorStep)> {
        self.steps
            .iter()
            .filter_map(|step| step.active.then_some((step.path_index, step)))
            .collect()
    }

    fn to_selector(&self) -> String {
        let mut selector = String::new();
        let mut previous_step_index: Option<usize> = None;

        for step in &self.steps {
            if !step.active {
                continue;
            }

            let step_selector = step.to_selector();
            if step_selector.is_empty() {
                continue;
            }

            if let Some(previous_index) = previous_step_index {
                if step.path_index == previous_index + 1 {
                    selector.push_str(" > ");
                } else {
                    selector.push(' ');
                }
            }

            selector.push_str(&step_selector);
            previous_step_index = Some(step.path_index);
        }

        selector
    }

    fn removal_candidates(&self) -> Vec<RemovalCandidate> {
        let target_index = self.steps.len().saturating_sub(1);
        let mut removals = Vec::new();
        let mut order = 0usize;

        for (step_index, step) in self.steps.iter().enumerate() {
            for (pseudo_index, pseudo) in step.pseudos.iter().enumerate() {
                removals.push(RemovalCandidate {
                    part: SelectorPart::Pseudo(step_index, pseudo_index),
                    cost: pseudo.removal_cost(),
                    order,
                });
                order += 1;
            }

            for (attr_index, attr) in step.attrs.iter().enumerate() {
                if attr.always_keep() {
                    continue;
                }
                removals.push(RemovalCandidate {
                    part: SelectorPart::Attr(step_index, attr_index),
                    cost: attr.removal_cost(step_index == target_index),
                    order,
                });
                order += 1;
            }

            if step_index != target_index {
                removals.push(RemovalCandidate {
                    part: SelectorPart::Step(step_index),
                    cost: 600,
                    order,
                });
                order += 1;
            }
        }

        removals.sort_by(|left, right| {
            right
                .cost
                .cmp(&left.cost)
                .then_with(|| left.order.cmp(&right.order))
        });
        removals
    }

    fn remove(&mut self, part: SelectorPart) -> bool {
        match part {
            SelectorPart::Step(index) => {
                let Some(step) = self.steps.get_mut(index) else {
                    return false;
                };
                if !step.active {
                    return false;
                }
                step.active = false;
                true
            }
            SelectorPart::Attr(step_index, attr_index) => {
                let Some(step) = self.steps.get_mut(step_index) else {
                    return false;
                };
                if !step.active {
                    return false;
                }
                let Some(attr) = step.attrs.get_mut(attr_index) else {
                    return false;
                };
                if !attr.active {
                    return false;
                }
                attr.active = false;
                true
            }
            SelectorPart::Pseudo(step_index, pseudo_index) => {
                let Some(step) = self.steps.get_mut(step_index) else {
                    return false;
                };
                if !step.active {
                    return false;
                }
                let Some(pseudo) = step.pseudos.get_mut(pseudo_index) else {
                    return false;
                };
                if !pseudo.active {
                    return false;
                }
                pseudo.active = false;
                true
            }
        }
    }
}

#[derive(Clone)]
struct SelectorStep {
    active: bool,
    path_index: usize,
    role: &'static str,
    attrs: Vec<SelectorAttr>,
    pseudos: Vec<SelectorPseudo>,
}

impl SelectorStep {
    fn new(elem: &Element, nth_child: Option<usize>, path_index: usize) -> Self {
        let mut attrs = Vec::new();

        if let Some(title) = elem.title.as_ref().filter(|s| !s.is_empty()) {
            attrs.push(SelectorAttr::new(SelectorAttrKind::Title, "title", title));
        }

        if let Some(desc) = elem.description.as_ref().filter(|s| !s.is_empty())
            && elem.title.as_deref() != Some(desc.as_str())
        {
            attrs.push(SelectorAttr::new(
                SelectorAttrKind::Description,
                "description",
                desc,
            ));
        }

        if let Some(value) = elem.value.as_ref().filter(|s| !s.is_empty())
            && elem.title.as_deref() != Some(value.as_str())
        {
            attrs.push(SelectorAttr::new(SelectorAttrKind::Value, "value", value));
        }

        if let Some(url) = elem.url.as_ref().filter(|s| !s.is_empty()) {
            attrs.push(SelectorAttr::new(SelectorAttrKind::Url, "url", url));
        }

        if let Some(help) = elem.help.as_ref().filter(|s| !s.is_empty()) {
            attrs.push(SelectorAttr::new(SelectorAttrKind::Help, "help", help));
        }

        if let Some(identifier) = elem.identifier.as_ref().filter(|s| !s.is_empty()) {
            attrs.push(SelectorAttr::new(
                SelectorAttrKind::Identifier,
                "identifier",
                identifier,
            ));
        }

        if let Some(role_description) = elem.role_description.as_ref().filter(|s| !s.is_empty()) {
            attrs.push(SelectorAttr::new(
                SelectorAttrKind::RoleDescription,
                "role-description",
                role_description,
            ));
        }

        let actions = format_actions_query_value(&elem.actions);
        if !actions.is_empty() {
            attrs.push(SelectorAttr::new(
                SelectorAttrKind::Actions,
                "actions",
                &actions,
            ));
        }

        let mut pseudos = Vec::new();
        if elem.focused {
            pseudos.push(SelectorPseudo::new(SelectorPseudoKind::Focused));
        }
        if !elem.enabled {
            pseudos.push(SelectorPseudo::new(SelectorPseudoKind::Disabled));
        }
        if let Some(nth_child) = nth_child {
            pseudos.push(SelectorPseudo::new(SelectorPseudoKind::NthChild(nth_child)));
        }

        Self {
            active: true,
            path_index,
            role: format_role_query_name(elem.role),
            attrs,
            pseudos,
        }
    }

    fn to_selector(&self) -> String {
        let mut selector = self.role.to_string();

        for attr in &self.attrs {
            if attr.active {
                selector.push_str(&format_attr_selector(attr.name, &attr.value));
            }
        }

        for pseudo in &self.pseudos {
            if pseudo.active {
                selector.push_str(&pseudo.to_selector());
            }
        }

        selector
    }

    fn matches(&self, elem: &Element, child_index: Option<usize>) -> bool {
        if self.role != "*" && format_role_query_name(elem.role) != self.role {
            return false;
        }

        self.attrs
            .iter()
            .filter(|attr| attr.active)
            .all(|attr| attr.matches(elem))
            && self
                .pseudos
                .iter()
                .filter(|pseudo| pseudo.active)
                .all(|pseudo| pseudo.matches(elem, child_index))
    }
}

#[derive(Clone)]
struct SelectorAttr {
    active: bool,
    kind: SelectorAttrKind,
    name: &'static str,
    value: String,
}

impl SelectorAttr {
    fn new(kind: SelectorAttrKind, name: &'static str, value: &str) -> Self {
        Self {
            active: true,
            kind,
            name,
            value: value.to_string(),
        }
    }

    fn removal_cost(&self, is_target: bool) -> u16 {
        match self.kind {
            SelectorAttrKind::Actions | SelectorAttrKind::Url => 800,
            SelectorAttrKind::Title
            | SelectorAttrKind::Description
            | SelectorAttrKind::Value
            | SelectorAttrKind::Help
            | SelectorAttrKind::Identifier
            | SelectorAttrKind::RoleDescription => {
                if is_target {
                    100
                } else {
                    650
                }
            }
        }
    }

    fn always_keep(&self) -> bool {
        matches!(
            self.kind,
            SelectorAttrKind::Title
                | SelectorAttrKind::Description
                | SelectorAttrKind::Value
                | SelectorAttrKind::Help
                | SelectorAttrKind::Identifier
                | SelectorAttrKind::RoleDescription
        )
    }

    fn matches(&self, elem: &Element) -> bool {
        let actual = match self.kind {
            SelectorAttrKind::Title => elem.title.as_deref(),
            SelectorAttrKind::Description => elem.description.as_deref(),
            SelectorAttrKind::Value => elem.value.as_deref(),
            SelectorAttrKind::Url => elem.url.as_deref(),
            SelectorAttrKind::Help => elem.help.as_deref(),
            SelectorAttrKind::Identifier => elem.identifier.as_deref(),
            SelectorAttrKind::RoleDescription => elem.role_description.as_deref(),
            SelectorAttrKind::Actions => {
                let actions = format_actions_query_value(&elem.actions);
                return !actions.is_empty()
                    && (actions == self.value
                        || actions
                            .split_whitespace()
                            .any(|action| action == self.value.as_str()));
            }
        };

        actual == Some(self.value.as_str())
    }
}

#[derive(Clone, Copy)]
enum SelectorAttrKind {
    Title,
    Description,
    Value,
    Url,
    Help,
    Identifier,
    RoleDescription,
    Actions,
}

#[derive(Clone)]
struct SelectorPseudo {
    active: bool,
    kind: SelectorPseudoKind,
}

impl SelectorPseudo {
    fn new(kind: SelectorPseudoKind) -> Self {
        Self { active: true, kind }
    }

    fn to_selector(&self) -> String {
        match self.kind {
            SelectorPseudoKind::Focused => ":focused".to_string(),
            SelectorPseudoKind::Disabled => ":disabled".to_string(),
            SelectorPseudoKind::NthChild(index) => format!(":nth-child({})", index),
        }
    }

    fn removal_cost(&self) -> u16 {
        match self.kind {
            SelectorPseudoKind::NthChild(_) => 1_000,
            SelectorPseudoKind::Focused | SelectorPseudoKind::Disabled => 900,
        }
    }

    fn matches(&self, elem: &Element, child_index: Option<usize>) -> bool {
        match self.kind {
            SelectorPseudoKind::Focused => elem.focused,
            SelectorPseudoKind::Disabled => !elem.enabled,
            SelectorPseudoKind::NthChild(index) => {
                child_index.is_some_and(|child_index| child_index + 1 == index)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum SelectorPseudoKind {
    Focused,
    Disabled,
    NthChild(usize),
}

#[derive(Clone, Copy)]
enum SelectorPart {
    Step(usize),
    Attr(usize, usize),
    Pseudo(usize, usize),
}

struct RemovalCandidate {
    part: SelectorPart,
    cost: u16,
    order: usize,
}

/// Print human-readable tree using CSS selector format.
pub fn print_tree(element: &Element, indent: usize) {
    let mut stack = vec![(element, indent)];

    while let Some((current, current_indent)) = stack.pop() {
        let prefix = "  ".repeat(current_indent);
        let selector = format_element_selector(current);

        let mut status = Vec::new();
        if current.focused {
            status.push("FOCUSED");
        }
        if !current.enabled {
            status.push("disabled");
        }
        let status_str = if status.is_empty() {
            String::new()
        } else {
            format!(" [{}]", status.join(", "))
        };

        println!("{}[{}] {}{}", prefix, current.id, selector, status_str);

        for child in current.children.iter().rev() {
            stack.push((child, current_indent + 1));
        }
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
    let mut stack = vec![element];
    while let Some(current) = stack.pop() {
        *counts.entry(current.role).or_insert(0) += 1;
        for child in current.children.iter().rev() {
            stack.push(child);
        }
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
fn print_llm_query_format(tree: &ElementTree, structure_only: bool) {
    for line in format_llm_query_lines(tree, structure_only) {
        println!("{}", line);
    }
}

fn format_llm_query_lines(tree: &ElementTree, structure_only: bool) -> Vec<String> {
    let root = &tree.root;
    let tree_lines = collect_llm_query_tree_lines(root, structure_only);
    let mut formatter = MinimalQueryFormatter::new(tree);
    render_css_tree_lines(&tree_lines, &mut formatter)
}

#[derive(Clone, Copy)]
struct CssTreeLine<'a> {
    element: &'a Element,
    depth: usize,
}

fn collect_llm_query_tree_lines(root: &Element, structure_only: bool) -> Vec<CssTreeLine<'_>> {
    let mut lines = vec![CssTreeLine {
        element: root,
        depth: 0,
    }];

    if structure_only {
        for child in &root.children {
            collect_structure_node_css_lines(child, 1, &mut lines);
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
        collect_window_css_lines(window, 1, &mut lines);
    }

    if let Some(mb) = menubar {
        collect_menubar_css_lines(mb, 1, &mut lines);
    }

    if !other_interactive.is_empty() {
        for elem in other_interactive {
            lines.push(CssTreeLine {
                element: elem,
                depth: 1,
            });
        }
    }

    lines
}

fn render_css_tree_lines(
    tree_lines: &[CssTreeLine<'_>],
    formatter: &mut MinimalQueryFormatter<'_>,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut open_elements: Vec<ElementKey> = Vec::new();

    for (index, tree_line) in tree_lines.iter().enumerate() {
        while open_elements.len() > tree_line.depth {
            let close_indent = "  ".repeat(open_elements.len() - 1);
            lines.push(format!("{}}}", close_indent));
            open_elements.pop();
        }

        let has_children = tree_lines
            .get(index + 1)
            .is_some_and(|next| next.depth > tree_line.depth);
        let indent = "  ".repeat(tree_line.depth);
        let selector = if tree_line.depth == 0 {
            formatter.selector_for(tree_line.element)
        } else {
            let scope_id = open_elements
                .get(tree_line.depth.saturating_sub(1))
                .copied();
            formatter.nested_selector_for(scope_id, tree_line.element)
        };

        if has_children {
            lines.push(format!("{}{} {{", indent, selector));
            open_elements.push(tree_line.element.id);
        } else {
            lines.push(format!("{}{} {{}}", indent, selector));
        }
    }

    while let Some(_) = open_elements.pop() {
        let close_indent = "  ".repeat(open_elements.len());
        lines.push(format!("{}}}", close_indent));
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
    let mut format_selector = format_element_selector;
    collect_structure_node_lines_with(element, indent, lines, &mut format_selector);
}

fn collect_structure_node_lines_with(
    element: &Element,
    indent: usize,
    lines: &mut Vec<String>,
    format_selector: &mut impl FnMut(&Element) -> String,
) {
    let mut stack = vec![(element, indent)];

    while let Some((current, current_indent)) = stack.pop() {
        let is_structural = is_structural_node(current);

        if is_structural || current_indent == 0 {
            let prefix = "  ".repeat(current_indent);
            lines.push(format!("{}{}", prefix, format_selector(current)));

            for child in current.children.iter().rev() {
                if is_structural_node(child) || has_structural_descendants(child) {
                    stack.push((child, current_indent + 1));
                }
            }
        } else if has_structural_descendants(current) {
            for child in current.children.iter().rev() {
                stack.push((child, current_indent));
            }
        }
    }
}

fn collect_structure_node_css_lines<'a>(
    element: &'a Element,
    depth: usize,
    lines: &mut Vec<CssTreeLine<'a>>,
) {
    let mut stack = vec![(element, depth)];

    while let Some((current, current_depth)) = stack.pop() {
        let is_structural = is_structural_node(current);

        if is_structural || current_depth == depth {
            lines.push(CssTreeLine {
                element: current,
                depth: current_depth,
            });

            for child in current.children.iter().rev() {
                if is_structural_node(child) || has_structural_descendants(child) {
                    stack.push((child, current_depth + 1));
                }
            }
        } else if has_structural_descendants(current) {
            for child in current.children.iter().rev() {
                stack.push((child, current_depth));
            }
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
    let mut stack: Vec<&Element> = element.children.iter().collect();
    while let Some(current) = stack.pop() {
        if is_structural_node(current) {
            return true;
        }
        for child in &current.children {
            stack.push(child);
        }
    }
    false
}

fn count_interactive_descendants(element: &Element) -> usize {
    let mut count = 0;
    let mut stack = vec![element];
    while let Some(current) = stack.pop() {
        if is_llm_relevant(current) {
            count += 1;
        }
        for child in current.children.iter().rev() {
            stack.push(child);
        }
    }
    count
}

fn collect_window_css_lines<'a>(
    window: &'a Element,
    depth: usize,
    lines: &mut Vec<CssTreeLine<'a>>,
) {
    let mut all_interactive: Vec<&Element> = Vec::new();
    for child in &window.children {
        collect_interactive(child, &mut all_interactive);
    }

    lines.push(CssTreeLine {
        element: window,
        depth,
    });

    if !all_interactive.is_empty() {
        for child in &window.children {
            collect_element_hierarchical_css_lines(child, depth + 1, lines);
        }
    }
}

fn collect_element_hierarchical_css_lines<'a>(
    element: &'a Element,
    depth: usize,
    lines: &mut Vec<CssTreeLine<'a>>,
) {
    let mut stack = vec![(element, depth)];

    while let Some((current, current_depth)) = stack.pop() {
        let is_container = is_meaningful_container(current);
        let interactive_children = count_interactive_descendants(current);

        if is_container && interactive_children > 0 {
            lines.push(CssTreeLine {
                element: current,
                depth: current_depth,
            });

            for child in current.children.iter().rev() {
                stack.push((child, current_depth + 1));
            }
        } else if is_llm_relevant(current) {
            lines.push(CssTreeLine {
                element: current,
                depth: current_depth,
            });
        } else {
            for child in current.children.iter().rev() {
                stack.push((child, current_depth));
            }
        }
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

fn collect_menubar_css_lines<'a>(
    menubar: &'a Element,
    depth: usize,
    lines: &mut Vec<CssTreeLine<'a>>,
) {
    lines.push(CssTreeLine {
        element: menubar,
        depth,
    });
    for item in &menubar.children {
        if item.role == Role::MenuItem {
            lines.push(CssTreeLine {
                element: item,
                depth: depth + 1,
            });
        }
    }
}

fn collect_interactive<'a>(element: &'a Element, result: &mut Vec<&'a Element>) {
    let mut stack = vec![element];
    while let Some(current) = stack.pop() {
        if is_llm_relevant(current) {
            result.push(current);
        }
        for child in current.children.iter().rev() {
            stack.push(child);
        }
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

    fn make_deep_output_tree(depth: u64) -> ElementTree {
        let mut button = Element::new(ElementKey::from_ffi(depth + 100), Role::Button);
        button.title = Some("Needle".to_string());
        button.actions = vec!["AXPress".to_string()];

        let mut current = button;
        for id in (0..depth).rev() {
            let mut group = Element::new(ElementKey::from_ffi(id + 100), Role::Group);
            group.children.push(current);
            current = group;
        }

        let mut window = Element::new(ElementKey::from_ffi(42), Role::Window);
        window.title = Some("Deep Window".to_string());
        window.children.push(current);

        let mut root = Element::new(ElementKey::from_ffi(1), Role::Application);
        root.title = Some("Deep App".to_string());
        root.children.push(window);

        ElementTree {
            version: 1,
            pid: None,
            app_name: Some("Deep App".to_string()),
            element_count: depth as usize + 3,
            root,
        }
    }

    fn css_line_selector(line: &str) -> Option<&str> {
        let line = line.trim();
        if line.is_empty() || line == "}" {
            return None;
        }

        line.strip_suffix(" {}").or_else(|| line.strip_suffix(" {"))
    }

    fn assert_llm_query_output_selectors_parse(tree: &ElementTree, structure_only: bool) {
        for raw_line in format_llm_query_lines(tree, structure_only) {
            if let Some(selector) = css_line_selector(&raw_line) {
                parse_query(selector).unwrap_or_else(|err| panic!("{selector}: {err}"));
            }
        }
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

    fn minimal_selector_for(tree: &ElementTree, id: u64) -> String {
        let id = ElementKey::from_ffi(id);
        let element = find_element_by_id(&tree.root, id).expect("test element should exist");
        MinimalQueryFormatter::new(tree).selector_for(element)
    }

    #[test]
    fn llm_menu_item_line_uses_query_selector_syntax() {
        let mut root = Element::new(ElementKey::from_ffi(1), Role::Application);
        let mut menubar = Element::new(ElementKey::from_ffi(2), Role::MenuBar);
        let mut item = Element::new(ElementKey::from_ffi(4_294_967_299), Role::MenuItem);
        item.title = Some("Apple".to_string());
        item.actions = vec![
            "AXCancel".to_string(),
            "AXPress".to_string(),
            "AXPick".to_string(),
        ];
        menubar.children.push(item);
        root.children.push(menubar);
        let tree = ElementTree {
            version: 1,
            pid: None,
            app_name: None,
            root,
            element_count: 3,
        };

        assert_eq!(
            minimal_selector_for(&tree, 4_294_967_299),
            "MenuItem[title=\"Apple\"]"
        );
    }

    #[test]
    fn minimal_selector_keeps_text_attrs_even_when_role_is_unique() {
        let tree = make_output_round_trip_tree();
        let selector = minimal_selector_for(&tree, 5);

        assert_eq!(selector, "Button[title=\"Run\"]");
        assert!(!selector.contains("data-id"));
    }

    #[test]
    fn minimal_selector_keeps_ancestor_context_for_duplicate_labels() {
        let mut root = Element::new(ElementKey::from_ffi(1), Role::Application);
        let mut window = Element::new(ElementKey::from_ffi(2), Role::Window);

        let mut editor = Element::new(ElementKey::from_ffi(3), Role::Toolbar);
        editor.title = Some("Editor".to_string());
        let mut editor_save = Element::new(ElementKey::from_ffi(4), Role::Button);
        editor_save.title = Some("Save".to_string());
        editor.children.push(editor_save);

        let mut footer = Element::new(ElementKey::from_ffi(5), Role::Toolbar);
        footer.title = Some("Footer".to_string());
        let mut footer_save = Element::new(ElementKey::from_ffi(6), Role::Button);
        footer_save.title = Some("Save".to_string());
        footer.children.push(footer_save);

        window.children.push(editor);
        window.children.push(footer);
        root.children.push(window);
        let tree = ElementTree {
            version: 1,
            pid: None,
            app_name: None,
            root,
            element_count: 6,
        };

        assert_eq!(
            minimal_selector_for(&tree, 4),
            "Toolbar[title=\"Editor\"] > Button[title=\"Save\"]"
        );
    }

    #[test]
    fn minimal_selector_uses_position_before_id_for_identical_siblings() {
        let mut root = Element::new(ElementKey::from_ffi(1), Role::Application);
        let mut window = Element::new(ElementKey::from_ffi(2), Role::Window);
        for id in 3..=4 {
            let mut button = Element::new(ElementKey::from_ffi(id), Role::Button);
            button.title = Some("Save".to_string());
            window.children.push(button);
        }
        root.children.push(window);
        let tree = ElementTree {
            version: 1,
            pid: None,
            app_name: None,
            root,
            element_count: 4,
        };

        let selector = minimal_selector_for(&tree, 3);
        assert_eq!(selector, "Button[title=\"Save\"]:nth-child(1)");
        assert!(!selector.contains("data-id"));
    }

    #[test]
    fn minimal_selector_uses_position_for_anonymous_elements_before_id() {
        let mut root = Element::new(ElementKey::from_ffi(1), Role::Application);
        let mut window = Element::new(ElementKey::from_ffi(2), Role::Window);
        window
            .children
            .push(Element::new(ElementKey::from_ffi(3), Role::Unknown));
        window
            .children
            .push(Element::new(ElementKey::from_ffi(4), Role::Unknown));
        root.children.push(window);
        let tree = ElementTree {
            version: 1,
            pid: None,
            app_name: None,
            root,
            element_count: 4,
        };

        let selector = minimal_selector_for(&tree, 3);
        assert_eq!(selector, "Window > *:nth-child(1)");
        assert!(!selector.contains("data-id"));
    }

    #[test]
    fn minimal_selector_uses_full_positional_path_before_id() {
        let mut root = Element::new(ElementKey::from_ffi(1), Role::Application);
        let mut window = Element::new(ElementKey::from_ffi(2), Role::Window);

        for group_id in [3, 5] {
            let mut group = Element::new(ElementKey::from_ffi(group_id), Role::Group);
            group.children.push(Element::new(
                ElementKey::from_ffi(group_id + 1),
                Role::Unknown,
            ));
            window.children.push(group);
        }

        root.children.push(window);
        let tree = ElementTree {
            version: 1,
            pid: None,
            app_name: None,
            root,
            element_count: 6,
        };

        let selector = minimal_selector_for(&tree, 4);
        let parsed = parse_query(&selector).unwrap();
        let matches = find_matches(&parsed, &tree);

        assert_eq!(selector, "Group:nth-child(1) > *");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, ElementKey::from_ffi(4));
        assert!(!selector.contains("data-id"));
    }

    #[test]
    fn minimal_selector_escapes_strings_when_attribute_is_needed() {
        let mut root = Element::new(ElementKey::from_ffi(1), Role::Application);
        let mut first = Element::new(ElementKey::from_ffi(2), Role::Button);
        first.title = Some("Say \"hi\"\\again".to_string());
        let mut second = Element::new(ElementKey::from_ffi(3), Role::Button);
        second.title = Some("Other".to_string());
        root.children.push(first);
        root.children.push(second);
        let tree = ElementTree {
            version: 1,
            pid: None,
            app_name: None,
            root,
            element_count: 3,
        };

        let selector = minimal_selector_for(&tree, 2);
        let parsed = parse_query(&selector).unwrap();
        let matches = find_matches(&parsed, &tree);

        assert_eq!(selector, "Button[title=\"Say \\\"hi\\\"\\\\again\"]");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, ElementKey::from_ffi(2));
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
    fn llm_query_output_is_parseable_nested_css() {
        let tree = make_output_round_trip_tree();
        assert_llm_query_output_selectors_parse(&tree, false);
        assert_llm_query_output_selectors_parse(&tree, true);
    }

    #[test]
    fn llm_query_output_uses_nested_css_blocks() {
        let tree = make_output_round_trip_tree();
        let lines = format_llm_query_lines(&tree, false);

        assert_eq!(
            lines.first().map(String::as_str),
            Some("Application[title=\"Test App\"] {")
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "  Window[title=\"Main Window\"] {")
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "    Group[title=\"Primary Controls\"] {")
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "      List[title=\"Actions\"] {")
        );
        assert!(
            lines
                .iter()
                .any(|line| line == "        Button[title=\"Run\"] {}")
        );
        assert!(lines.iter().any(|line| line == "  MenuBar {"));
        assert!(
            lines
                .iter()
                .any(|line| line == "    MenuItem[title=\"Apple\"] {}")
        );
        assert_eq!(lines.last().map(String::as_str), Some("}"));
    }

    #[test]
    fn llm_query_output_handles_deep_tree_iteratively() {
        let tree = make_deep_output_tree(2048);
        let lines = format_llm_query_lines(&tree, false);
        assert!(
            lines
                .iter()
                .any(|line| line.trim() == "Button[title=\"Needle\"] {}"),
            "{lines:?}"
        );
    }
}
