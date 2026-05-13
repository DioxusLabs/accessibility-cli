//! CSS-like query parser for accessibility elements using Mozilla's `selectors` crate.
//!
//! Supports:
//! - ID selectors: `#42` (element with ID 42)
//! - Role selectors: `Button`, `TextInput`, `Window`
//! - Attribute selectors: `[title="Save"]`, `[title*="Save"]`, `[enabled]`
//! - Pseudo-classes: `:focused`, `:enabled`, `:interactive`, `:visible`
//! - Combinators: `Window Button` (descendant), `Window > Button` (child)

use super::{Element, ElementTree};
use accesskit::Role;
use cssparser::{CowRcStr, ParseError, SourceLocation, ToCss};
use selectors::{
    Element as SelectorElement, NthIndexCache, OpaqueElement,
    attr::{AttrSelectorOperation, AttrSelectorOperator, CaseSensitivity, NamespaceConstraint},
    context::QuirksMode,
    matching::{
        ElementSelectorFlags, IgnoreNthChildForInvalidation, MatchingContext, MatchingMode,
        NeedsSelectorFlags, matches_selector_list,
    },
    parser::{ParseRelative, SelectorImpl, SelectorList, SelectorParseErrorKind},
};
use std::borrow::Borrow;
use std::fmt;

/// Marker type for our selector implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessibilitySelectors;

/// String wrapper that implements required traits for selectors.
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
pub struct AttrString(pub String);

impl ToCss for AttrString {
    fn to_css<W>(&self, dest: &mut W) -> fmt::Result
    where
        W: fmt::Write,
    {
        cssparser::serialize_string(&self.0, dest)
    }
}

impl<'a> From<&'a str> for AttrString {
    fn from(s: &'a str) -> Self {
        AttrString(s.to_string())
    }
}

impl AsRef<str> for AttrString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for AttrString {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// Role name wrapper (used as "local name" / type selector).
/// Also stores the original string for attribute name matching.
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
pub struct RoleName {
    pub role: Option<Role>,
    pub original: String,
}

impl ToCss for RoleName {
    fn to_css<W>(&self, dest: &mut W) -> fmt::Result
    where
        W: fmt::Write,
    {
        if self.original.is_empty() || self.original == "*" {
            dest.write_str("*")
        } else {
            dest.write_str(&self.original)
        }
    }
}

impl<'a> From<&'a str> for RoleName {
    fn from(s: &'a str) -> Self {
        if s == "*" {
            RoleName {
                role: None,
                original: "*".to_string(),
            }
        } else {
            RoleName {
                role: super::roles::parse_role_name(s),
                original: s.to_string(),
            }
        }
    }
}

/// Namespace URL wrapper (empty, as we don't use namespaces).
#[derive(Debug, Clone, PartialEq, Eq, Default, Hash)]
pub struct NoNamespace;

impl ToCss for NoNamespace {
    fn to_css<W>(&self, _dest: &mut W) -> fmt::Result
    where
        W: fmt::Write,
    {
        Ok(())
    }
}

impl<'a> From<&'a str> for NoNamespace {
    fn from(_s: &'a str) -> Self {
        NoNamespace
    }
}

/// Custom pseudo-classes for accessibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessibilityPseudoClass {
    Focused,
    Enabled,
    Disabled,
    Interactive,
    Visible,
}

impl ToCss for AccessibilityPseudoClass {
    fn to_css<W>(&self, dest: &mut W) -> fmt::Result
    where
        W: fmt::Write,
    {
        match self {
            AccessibilityPseudoClass::Focused => dest.write_str(":focused"),
            AccessibilityPseudoClass::Enabled => dest.write_str(":enabled"),
            AccessibilityPseudoClass::Disabled => dest.write_str(":disabled"),
            AccessibilityPseudoClass::Interactive => dest.write_str(":interactive"),
            AccessibilityPseudoClass::Visible => dest.write_str(":visible"),
        }
    }
}

impl selectors::parser::NonTSPseudoClass for AccessibilityPseudoClass {
    type Impl = AccessibilitySelectors;

    fn is_active_or_hover(&self) -> bool {
        false
    }

    fn is_user_action_state(&self) -> bool {
        false
    }
}

/// Empty pseudo-element (we don't use these).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NeverPseudoElement {}

impl ToCss for NeverPseudoElement {
    fn to_css<W>(&self, _dest: &mut W) -> fmt::Result
    where
        W: fmt::Write,
    {
        match *self {}
    }
}

impl selectors::parser::PseudoElement for NeverPseudoElement {
    type Impl = AccessibilitySelectors;
}

impl SelectorImpl for AccessibilitySelectors {
    type AttrValue = AttrString;
    type Identifier = AttrString;
    type LocalName = RoleName;
    type NamespaceUrl = NoNamespace;
    type NamespacePrefix = NoNamespace;
    type BorrowedLocalName = RoleName;
    type BorrowedNamespaceUrl = NoNamespace;
    type NonTSPseudoClass = AccessibilityPseudoClass;
    type PseudoElement = NeverPseudoElement;
    type ExtraMatchingData<'a> = ();
}

/// Reference to an element with tree context for selector matching.
pub struct ElementRef<'a> {
    element: &'a Element,
    ancestors: Vec<&'a Element>,
    index_in_parent: usize,
    siblings: &'a [Element],
}

impl<'a> ElementRef<'a> {
    /// Create an ElementRef for the root element.
    pub fn root(element: &'a Element) -> Self {
        Self {
            element,
            ancestors: Vec::new(),
            index_in_parent: 0,
            siblings: std::slice::from_ref(element),
        }
    }

    /// Get the underlying element.
    pub fn get(&self) -> &'a Element {
        self.element
    }
}

impl<'a> fmt::Debug for ElementRef<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ElementRef")
            .field("id", &self.element.id)
            .field("role", &self.element.role)
            .finish()
    }
}

impl<'a> Clone for ElementRef<'a> {
    fn clone(&self) -> Self {
        Self {
            element: self.element,
            ancestors: self.ancestors.clone(),
            index_in_parent: self.index_in_parent,
            siblings: self.siblings,
        }
    }
}

impl<'a> PartialEq for ElementRef<'a> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.element, other.element)
    }
}

impl<'a> Eq for ElementRef<'a> {}

impl<'a> SelectorElement for ElementRef<'a> {
    type Impl = AccessibilitySelectors;

    fn opaque(&self) -> OpaqueElement {
        OpaqueElement::new(self.element)
    }

    fn parent_element(&self) -> Option<Self> {
        let parent = *self.ancestors.last()?;
        let mut ancestors = self.ancestors.clone();
        ancestors.pop();

        let (index_in_parent, siblings) = if let Some(grandparent) = ancestors.last() {
            let index = grandparent
                .children
                .iter()
                .position(|child| child.id == parent.id)
                .unwrap_or(0);
            (index, grandparent.children.as_slice())
        } else {
            (0, std::slice::from_ref(parent))
        };

        Some(Self {
            element: parent,
            ancestors,
            index_in_parent,
            siblings,
        })
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        if self.index_in_parent == 0 {
            return None;
        }
        let prev_index = self.index_in_parent - 1;
        self.siblings.get(prev_index).map(|sibling| ElementRef {
            element: sibling,
            ancestors: self.ancestors.clone(),
            index_in_parent: prev_index,
            siblings: self.siblings,
        })
    }

    fn next_sibling_element(&self) -> Option<Self> {
        let next_index = self.index_in_parent + 1;
        self.siblings.get(next_index).map(|sibling| ElementRef {
            element: sibling,
            ancestors: self.ancestors.clone(),
            index_in_parent: next_index,
            siblings: self.siblings,
        })
    }

    fn first_element_child(&self) -> Option<Self> {
        let mut ancestors = self.ancestors.clone();
        ancestors.push(self.element);

        self.element.children.first().map(|child| ElementRef {
            element: child,
            ancestors,
            index_in_parent: 0,
            siblings: &self.element.children,
        })
    }

    fn is_html_element_in_html_document(&self) -> bool {
        false
    }

    fn has_local_name(&self, local_name: &RoleName) -> bool {
        match local_name.original.as_str() {
            "*" | "" => true,
            name => super::roles::role_name_matches(name, self.element.role),
        }
    }

    fn has_namespace(&self, _ns: &NoNamespace) -> bool {
        true // No namespace support
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.element.role == other.element.role
    }

    fn attr_matches(
        &self,
        _ns: &NamespaceConstraint<&NoNamespace>,
        local_name: &RoleName,
        operation: &AttrSelectorOperation<&AttrString>,
    ) -> bool {
        // Use the original string as the attribute name
        let attr_name = &local_name.original;
        if attr_name.is_empty() || attr_name == "*" {
            return false; // Can't match "*" as attribute name
        }

        self.match_attribute(attr_name, operation)
    }

    fn match_non_ts_pseudo_class(
        &self,
        pseudo: &AccessibilityPseudoClass,
        _context: &mut MatchingContext<'_, Self::Impl>,
    ) -> bool {
        match pseudo {
            AccessibilityPseudoClass::Focused => self.element.focused,
            AccessibilityPseudoClass::Enabled => self.element.enabled,
            AccessibilityPseudoClass::Disabled => !self.element.enabled,
            AccessibilityPseudoClass::Interactive => self.element.is_interactive(),
            AccessibilityPseudoClass::Visible => self
                .element
                .bounds
                .as_ref()
                .map(|b| b.size.width > 0.0 && b.size.height > 0.0)
                .unwrap_or(false),
        }
    }

    fn match_pseudo_element(
        &self,
        _pe: &NeverPseudoElement,
        _context: &mut MatchingContext<'_, Self::Impl>,
    ) -> bool {
        false
    }

    fn is_link(&self) -> bool {
        self.element.role == Role::Link
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(&self, id: &AttrString, _case_sensitivity: CaseSensitivity) -> bool {
        // For numeric IDs, we preprocess them to [data-id="N"]
        if let Ok(num_id) = id.0.parse::<u64>() {
            self.element.id.to_ffi() == num_id
        } else {
            false
        }
    }

    fn has_class(&self, _name: &AttrString, _case_sensitivity: CaseSensitivity) -> bool {
        false // No class support for accessibility elements
    }

    fn imported_part(&self, _name: &AttrString) -> Option<AttrString> {
        None
    }

    fn is_part(&self, _name: &AttrString) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        self.element.children.is_empty()
    }

    fn is_root(&self) -> bool {
        self.ancestors.is_empty()
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {
        // No-op for read-only matching
    }
}

/// Apply a string comparison operator to compare actual vs expected values.
/// This is shared between attribute matching and role matching.
fn apply_string_operator(actual: &str, expected: &str, operator: &AttrSelectorOperator) -> bool {
    match operator {
        AttrSelectorOperator::Equal => actual == expected,
        AttrSelectorOperator::Includes => actual.split_whitespace().any(|word| word == expected),
        AttrSelectorOperator::DashMatch => {
            actual == expected || actual.starts_with(&format!("{}-", expected))
        }
        AttrSelectorOperator::Prefix => actual.starts_with(expected),
        AttrSelectorOperator::Suffix => actual.ends_with(expected),
        AttrSelectorOperator::Substring => actual.contains(expected),
    }
}

fn format_actions_query_value(actions: &[String]) -> String {
    actions
        .iter()
        .filter_map(|action| match action.as_str() {
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

impl<'a> ElementRef<'a> {
    fn match_string_attr(
        &self,
        attr_value: Option<&str>,
        operation: &AttrSelectorOperation<&AttrString>,
    ) -> bool {
        match operation {
            AttrSelectorOperation::Exists => {
                attr_value.is_some() && !attr_value.unwrap_or("").is_empty()
            }
            AttrSelectorOperation::WithValue {
                operator,
                case_sensitivity,
                value,
            } => {
                let Some(actual) = attr_value else {
                    return false;
                };

                let (actual, expected) = match case_sensitivity {
                    CaseSensitivity::CaseSensitive => (actual.to_string(), value.0.clone()),
                    CaseSensitivity::AsciiCaseInsensitive => {
                        (actual.to_lowercase(), value.0.to_lowercase())
                    }
                };

                apply_string_operator(&actual, &expected, operator)
            }
        }
    }

    fn match_attribute(
        &self,
        attr_name: &str,
        operation: &AttrSelectorOperation<&AttrString>,
    ) -> bool {
        let attr_value = match attr_name.to_lowercase().as_str() {
            "title" => self.element.title.as_deref(),
            "description" | "desc" => self.element.description.as_deref(),
            "value" | "val" => self.element.value.as_deref(),
            "url" | "href" => self.element.url.as_deref(),
            "help" => self.element.help.as_deref(),
            "identifier" => self.element.identifier.as_deref(),
            "role-description" | "roledescription" => self.element.role_description.as_deref(),
            "action" | "actions" => return self.match_actions_attr(operation),
            "data-id" | "id" => return self.match_id_attr(operation),
            "role" => return self.match_role_attr(operation),
            "enabled" => return self.match_bool_attr(self.element.enabled, operation),
            "focused" => return self.match_bool_attr(self.element.focused, operation),
            _ => None,
        };

        self.match_string_attr(attr_value, operation)
    }

    fn match_actions_attr(&self, operation: &AttrSelectorOperation<&AttrString>) -> bool {
        let actions = format_actions_query_value(&self.element.actions);
        if actions.is_empty() {
            return false;
        }

        match operation {
            AttrSelectorOperation::Exists => true,
            AttrSelectorOperation::WithValue {
                operator: AttrSelectorOperator::Equal,
                value,
                ..
            } => {
                let expected = value.0.as_str();
                actions == expected || actions.split_whitespace().any(|action| action == expected)
            }
            _ => self.match_string_attr(Some(actions.as_str()), operation),
        }
    }

    fn match_id_attr(&self, operation: &AttrSelectorOperation<&AttrString>) -> bool {
        match operation {
            AttrSelectorOperation::Exists => true,
            AttrSelectorOperation::WithValue {
                operator: AttrSelectorOperator::Equal,
                value,
                ..
            } => {
                if let Ok(num_id) = value.0.parse::<u64>() {
                    self.element.id.to_ffi() == num_id
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn match_role_attr(&self, operation: &AttrSelectorOperation<&AttrString>) -> bool {
        let role_name = format!("{:?}", self.element.role);
        match operation {
            AttrSelectorOperation::Exists => true,
            AttrSelectorOperation::WithValue {
                operator,
                case_sensitivity,
                value,
            } => {
                let (actual, expected) = match case_sensitivity {
                    CaseSensitivity::CaseSensitive => (role_name, value.0.clone()),
                    CaseSensitivity::AsciiCaseInsensitive => {
                        (role_name.to_lowercase(), value.0.to_lowercase())
                    }
                };
                apply_string_operator(&actual, &expected, operator)
            }
        }
    }

    fn match_bool_attr(
        &self,
        actual: bool,
        operation: &AttrSelectorOperation<&AttrString>,
    ) -> bool {
        match operation {
            AttrSelectorOperation::Exists => actual,
            AttrSelectorOperation::WithValue {
                operator: AttrSelectorOperator::Equal,
                value,
                ..
            } => match value.0.to_lowercase().as_str() {
                "true" | "1" | "yes" => actual,
                "false" | "0" | "no" => !actual,
                _ => false,
            },
            _ => false,
        }
    }
}

/// Custom parser that handles our pseudo-classes and type selectors.
struct AccessibilityParser;

impl<'i> selectors::parser::Parser<'i> for AccessibilityParser {
    type Impl = AccessibilitySelectors;
    type Error = SelectorParseErrorKind<'i>;

    fn parse_non_ts_pseudo_class(
        &self,
        location: SourceLocation,
        name: CowRcStr<'i>,
    ) -> Result<AccessibilityPseudoClass, ParseError<'i, Self::Error>> {
        match name.as_ref() {
            "focused" => Ok(AccessibilityPseudoClass::Focused),
            "enabled" => Ok(AccessibilityPseudoClass::Enabled),
            "disabled" => Ok(AccessibilityPseudoClass::Disabled),
            "interactive" => Ok(AccessibilityPseudoClass::Interactive),
            "visible" => Ok(AccessibilityPseudoClass::Visible),
            _ => Err(location.new_custom_error(
                SelectorParseErrorKind::UnsupportedPseudoClassOrElement(name),
            )),
        }
    }
}

/// Preprocess a query string to handle numeric IDs.
///
/// The `selectors` crate rejects `#42` (numeric IDs). We transform them to `[data-id="42"]`.
fn preprocess_query(query: &str) -> String {
    let mut result = String::with_capacity(query.len() * 2);
    let mut chars = query.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '#' {
            // Check if followed by digits
            let mut digits = String::new();
            while let Some(&next) = chars.peek() {
                if next.is_ascii_digit() {
                    digits.push(chars.next().unwrap());
                } else {
                    break;
                }
            }

            if !digits.is_empty() {
                // Transform #42 to [data-id="42"]
                result.push_str(&format!("[data-id=\"{}\"]", digits));
            } else {
                // Not a numeric ID, keep as-is
                result.push('#');
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Parsed selector for accessibility queries.
pub type Selector = selectors::parser::Selector<AccessibilitySelectors>;

/// Parse a CSS-like query string into a selector list.
pub fn parse(query: &str) -> Result<SelectorList<AccessibilitySelectors>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("Empty query".to_string());
    }

    let preprocessed = preprocess_query(query);

    let mut parser_input = cssparser::ParserInput::new(&preprocessed);
    let mut parser = cssparser::Parser::new(&mut parser_input);

    SelectorList::parse(&AccessibilityParser, &mut parser, ParseRelative::No)
        .map_err(|e| format!("Parse error: {:?}", e))
}

/// Create a MatchingContext for selector matching.
///
/// This keeps selector matching context initialization in one place.
fn create_matching_context<'a>(
    nth_index_cache: &'a mut NthIndexCache,
) -> MatchingContext<'a, AccessibilitySelectors> {
    MatchingContext::new(
        MatchingMode::Normal,
        None,
        nth_index_cache,
        QuirksMode::NoQuirks,
        NeedsSelectorFlags::No,
        IgnoreNthChildForInvalidation::No,
    )
}

/// Find all elements matching a selector in the tree.
pub fn find_matches<'a>(
    selector_list: &SelectorList<AccessibilitySelectors>,
    tree: &'a ElementTree,
) -> Vec<&'a Element> {
    let mut results = Vec::new();
    let mut stack: Vec<(&'a Element, Vec<&'a Element>)> = vec![(&tree.root, Vec::new())];

    while let Some((element, ancestors)) = stack.pop() {
        let parent = ancestors.last().copied();
        let index_in_parent = parent
            .map(|p| {
                p.children
                    .iter()
                    .position(|c| c.id == element.id)
                    .unwrap_or(0)
            })
            .unwrap_or(0);

        let siblings: &[Element] = parent
            .map(|p| p.children.as_slice())
            .unwrap_or(std::slice::from_ref(element));

        let elem_ref = ElementRef {
            element,
            ancestors: ancestors.clone(),
            index_in_parent,
            siblings,
        };

        let mut nth_index_cache = NthIndexCache::default();
        let mut context = create_matching_context(&mut nth_index_cache);

        if matches_selector_list(selector_list, &elem_ref, &mut context) {
            results.push(element);
        }

        for child in element.children.iter().rev() {
            let mut child_ancestors = ancestors.clone();
            child_ancestors.push(element);
            stack.push((child, child_ancestors));
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accessibility::{ElementKey, Point, Rect, Size};

    fn make_test_tree() -> ElementTree {
        ElementTree {
            version: 1,
            pid: None,
            app_name: None,
            element_count: 6,
            root: Element {
                id: ElementKey::from_ffi(1),
                role: Role::Window,
                title: Some("Test Window".to_string()),
                description: None,
                value: None,
                url: None,
                help: None,
                role_description: None,
                identifier: None,
                bounds: Some(Rect::new(Point::new(0.0, 0.0), Size::new(800.0, 600.0))),
                enabled: true,
                focused: false,
                actions: vec![],
                children: vec![
                    Element {
                        id: ElementKey::from_ffi(2),
                        role: Role::Button,
                        title: Some("Save".to_string()),
                        description: None,
                        value: None,
                        url: None,
                        help: None,
                        role_description: None,
                        identifier: None,
                        bounds: Some(Rect::new(Point::new(10.0, 10.0), Size::new(80.0, 30.0))),
                        enabled: true,
                        focused: true,
                        actions: vec!["Click".to_string()],
                        children: vec![],
                    },
                    Element {
                        id: ElementKey::from_ffi(3),
                        role: Role::TextInput,
                        title: Some("Name".to_string()),
                        description: None,
                        value: Some("Hello".to_string()),
                        url: None,
                        help: None,
                        role_description: None,
                        identifier: None,
                        bounds: Some(Rect::new(Point::new(10.0, 50.0), Size::new(200.0, 25.0))),
                        enabled: false,
                        focused: false,
                        actions: vec![],
                        children: vec![],
                    },
                    Element {
                        id: ElementKey::from_ffi(4),
                        role: Role::Button,
                        title: Some("Cancel".to_string()),
                        description: None,
                        value: None,
                        url: None,
                        help: None,
                        role_description: None,
                        identifier: None,
                        bounds: Some(Rect::new(Point::new(100.0, 10.0), Size::new(80.0, 30.0))),
                        enabled: true,
                        focused: false,
                        actions: vec!["Click".to_string()],
                        children: vec![],
                    },
                    Element {
                        id: ElementKey::from_ffi(5),
                        role: Role::TextRun,
                        title: None,
                        description: None,
                        value: Some("Static text".to_string()),
                        url: None,
                        help: None,
                        role_description: None,
                        identifier: None,
                        bounds: Some(Rect::new(Point::new(10.0, 90.0), Size::new(200.0, 20.0))),
                        enabled: true,
                        focused: false,
                        actions: vec![],
                        children: vec![],
                    },
                    Element {
                        id: ElementKey::from_ffi(6),
                        role: Role::Label,
                        title: Some("Label text".to_string()),
                        description: None,
                        value: None,
                        url: None,
                        help: None,
                        role_description: None,
                        identifier: None,
                        bounds: Some(Rect::new(Point::new(10.0, 120.0), Size::new(200.0, 20.0))),
                        enabled: true,
                        focused: false,
                        actions: vec![],
                        children: vec![],
                    },
                ],
            },
        }
    }

    fn make_deep_test_tree() -> ElementTree {
        let mut window = Element::new(ElementKey::from_ffi(10), Role::Window);
        window.title = Some("Window".to_string());

        let mut group = Element::new(ElementKey::from_ffi(11), Role::Group);
        group.title = Some("Group".to_string());

        let mut region = Element::new(ElementKey::from_ffi(12), Role::Region);
        region.title = Some("Region".to_string());

        let mut button = Element::new(ElementKey::from_ffi(13), Role::Button);
        button.title = Some("Deep Button".to_string());

        region.children.push(button);
        group.children.push(region);
        window.children.push(group);

        ElementTree {
            version: 1,
            pid: None,
            app_name: None,
            element_count: 4,
            root: window,
        }
    }

    fn make_deep_chain_tree(depth: u64) -> ElementTree {
        let mut leaf = Element::new(ElementKey::from_ffi(depth + 100), Role::Button);
        leaf.title = Some("Needle".to_string());

        for id in (0..depth).rev() {
            let mut parent = Element::new(ElementKey::from_ffi(id + 100), Role::Group);
            parent.children.push(leaf);
            leaf = parent;
        }

        ElementTree {
            version: 1,
            pid: None,
            app_name: None,
            element_count: depth as usize + 1,
            root: leaf,
        }
    }

    #[test]
    fn test_preprocess_numeric_id() {
        assert_eq!(preprocess_query("#42"), "[data-id=\"42\"]");
        assert_eq!(preprocess_query("Button#5"), "Button[data-id=\"5\"]");
        assert_eq!(preprocess_query("#123 Button"), "[data-id=\"123\"] Button");
    }

    #[test]
    fn test_parse_attribute_equals() {
        let sel = parse("[title=\"Save\"]").unwrap();
        assert!(!sel.0.is_empty());
    }

    #[test]
    fn test_parse_attribute_contains() {
        let sel = parse("[title*=\"Save\"]").unwrap();
        assert!(!sel.0.is_empty());
    }

    #[test]
    fn test_parse_pseudo() {
        let sel = parse(":focused").unwrap();
        assert!(!sel.0.is_empty());
    }

    #[test]
    fn test_parse_descendant() {
        let sel = parse("Window Button").unwrap();
        assert!(!sel.0.is_empty());
    }

    #[test]
    fn test_parse_child() {
        let sel = parse("Window > Button").unwrap();
        assert!(!sel.0.is_empty());
    }

    #[test]
    fn test_parse_id() {
        let sel = parse("#42").unwrap();
        assert!(!sel.0.is_empty());
    }

    #[test]
    fn test_find_by_role() {
        let tree = make_test_tree();
        let sel = parse("Button").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().all(|e| e.role == Role::Button));
    }

    #[test]
    fn test_find_by_attribute() {
        let tree = make_test_tree();
        let sel = parse("[title=\"Save\"]").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].title.as_deref(), Some("Save"));
    }

    #[test]
    fn test_find_by_action_membership() {
        let mut tree = make_test_tree();
        tree.root.children[0].actions = vec!["AXCancel".to_string(), "AXPress".to_string()];

        let sel = parse("[actions=\"cancel\"]").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, ElementKey::from_ffi(2));

        let sel = parse("[action=\"click\"]").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, ElementKey::from_ffi(2));

        let sel = parse("[actions=\"cancel click\"]").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, ElementKey::from_ffi(2));
    }

    #[test]
    fn test_find_by_pseudo_focused() {
        let tree = make_test_tree();
        let sel = parse(":focused").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 1);
        assert!(matches[0].focused);
    }

    #[test]
    fn test_find_by_pseudo_disabled() {
        let tree = make_test_tree();
        let sel = parse(":disabled").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 1);
        assert!(!matches[0].enabled);
    }

    #[test]
    fn test_find_descendant() {
        let tree = make_test_tree();
        let sel = parse("Window Button").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_find_child() {
        let tree = make_test_tree();
        let sel = parse("Window > Button").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_find_deep_descendant() {
        let tree = make_deep_test_tree();
        let sel = parse("Window Group Region Button").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, ElementKey::from_ffi(13));
    }

    #[test]
    fn test_find_deep_child_chain() {
        let tree = make_deep_test_tree();
        let sel = parse("Window > Group > Region > Button").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, ElementKey::from_ffi(13));
    }

    #[test]
    fn test_find_matches_handles_deep_tree_iteratively() {
        let tree = make_deep_chain_tree(2048);
        let sel = parse("[title=\"Needle\"]").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].title.as_deref(), Some("Needle"));
    }

    #[test]
    fn test_find_by_id() {
        let tree = make_test_tree();
        // Get the actual FFI value of the second element (first child)
        let target_key = ElementKey::from_ffi(2);
        let ffi_value = target_key.to_ffi();

        // Search for the element by its FFI ID
        let selector_str = format!("#{}", ffi_value);
        let sel = parse(&selector_str).unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, target_key);
    }

    #[test]
    fn test_find_text_aliases() {
        let tree = make_test_tree();

        let sel = parse("TextRun").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].role, Role::TextRun);

        let sel = parse("Text").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().any(|e| e.role == Role::TextRun));
        assert!(matches.iter().any(|e| e.role == Role::Label));

        let sel = parse("StaticText").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 2);

        let sel = parse("Label").unwrap();
        let matches = find_matches(&sel, &tree);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].role, Role::Label);
    }
}
