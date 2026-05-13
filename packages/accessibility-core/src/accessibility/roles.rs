//! Shared role mapping utilities for accessibility elements.

use accesskit::Role;

/// Parse a user-friendly role name (for CSS selectors) to an AccessKit Role.
pub fn parse_role_name(name: &str) -> Option<Role> {
    match name.to_lowercase().as_str() {
        "button" => Some(Role::Button),
        "link" => Some(Role::Link),
        "textinput" | "textfield" | "input" => Some(Role::TextInput),
        "textarea" | "multilinetextinput" => Some(Role::MultilineTextInput),
        "checkbox" => Some(Role::CheckBox),
        "radiobutton" | "radio" => Some(Role::RadioButton),
        "combobox" | "dropdown" | "select" => Some(Role::ComboBox),
        "slider" => Some(Role::Slider),
        "switch" | "toggle" => Some(Role::Switch),
        "tab" => Some(Role::Tab),
        "tablist" => Some(Role::TabList),
        "menuitem" => Some(Role::MenuItem),
        "menuitemcheckbox" | "menucheck" => Some(Role::MenuItemCheckBox),
        "menuitemradio" | "menuradio" => Some(Role::MenuItemRadio),
        "menubar" => Some(Role::MenuBar),
        "menu" => Some(Role::Menu),
        "window" => Some(Role::Window),
        "dialog" => Some(Role::Dialog),
        "image" | "img" => Some(Role::Image),
        "group" => Some(Role::Group),
        "list" => Some(Role::List),
        "listitem" | "item" => Some(Role::ListItem),
        "toolbar" => Some(Role::Toolbar),
        "table" => Some(Role::Table),
        "row" => Some(Role::Row),
        "cell" => Some(Role::Cell),
        "heading" | "header" => Some(Role::Heading),
        "application" | "app" => Some(Role::Application),
        "scrollbar" => Some(Role::ScrollBar),
        "label" => Some(Role::Label),
        "textrun" => Some(Role::TextRun),
        "text" | "statictext" => Some(Role::TextRun),
        "scrollview" => Some(Role::ScrollView),
        "genericcontainer" | "container" | "div" => Some(Role::GenericContainer),
        "progressbar" | "progress" | "progressindicator" => Some(Role::ProgressIndicator),
        "spinbutton" | "spinner" => Some(Role::SpinButton),
        "navigation" | "nav" => Some(Role::Navigation),
        "region" => Some(Role::Region),
        "banner" => Some(Role::Banner),
        "complementary" | "aside" => Some(Role::Complementary),
        "contentinfo" | "footer" => Some(Role::ContentInfo),
        "main" => Some(Role::Main),
        "search" => Some(Role::Search),
        "form" => Some(Role::Form),
        "section" => Some(Role::Section),
        "document" => Some(Role::Document),
        "webview" => Some(Role::WebView),
        "article" => Some(Role::Article),
        "*" => None, // Universal selector
        _ => None,
    }
}

/// Return whether a user-facing role name should match an AccessKit role.
pub fn role_name_matches(name: &str, role: Role) -> bool {
    match name.to_lowercase().as_str() {
        "text" | "statictext" => matches!(role, Role::TextRun | Role::Label),
        "label" => role == Role::Label,
        "textrun" => role == Role::TextRun,
        _ => parse_role_name(name) == Some(role),
    }
}

/// Map an AX role string (macOS) to an AccessKit Role.
///
/// For iOS, use `map_ax_role_ios()` which handles platform-specific differences.
pub fn map_ax_role(ax_role: &str) -> Role {
    let role = ax_role.strip_prefix("AX").unwrap_or(ax_role);
    match role {
        "Application" => Role::Application,
        "Window" => Role::Window,
        "Button" => Role::Button,
        "TextField" => Role::TextInput,
        "TextArea" => Role::MultilineTextInput,
        "StaticText" => Role::TextRun,
        "CheckBox" => Role::CheckBox,
        "RadioButton" => Role::RadioButton,
        "PopUpButton" | "ComboBox" => Role::ComboBox,
        "Slider" => Role::Slider,
        "Table" => Role::Table,
        "List" => Role::List,
        "Outline" => Role::Tree,
        "Sheet" => Role::Dialog,
        "Menu" => Role::Menu,
        "MenuItem" | "MenuBarItem" => Role::MenuItem,
        "MenuBar" => Role::MenuBar,
        "WebArea" => Role::WebView,
        "Group" => Role::Group,
        "Image" => Role::Image,
        "Link" => Role::Link,
        "ScrollArea" => Role::ScrollView,
        "Toolbar" => Role::Toolbar,
        "TabGroup" => Role::TabList,
        "Tab" => Role::Tab,
        "ProgressIndicator" => Role::ProgressIndicator,
        "SplitGroup" | "Splitter" => Role::Splitter,
        "Row" => Role::Row,
        "Column" => Role::ListItem,
        "Cell" => Role::Cell,
        _ => Role::Unknown,
    }
}

/// Map an AX role string to an AccessKit Role with iOS-specific overrides.
///
/// Falls back to `map_ax_role()` for roles without iOS-specific behavior.
pub fn map_ax_role_ios(ax_role: &str) -> Role {
    let role = ax_role.strip_prefix("AX").unwrap_or(ax_role);
    match role {
        "StaticText" | "Label" => Role::Label,
        "SearchField" => Role::TextInput,
        "NavigationBar" => Role::Navigation,
        "Picker" | "PickerView" => Role::ListBox,
        "Switch" | "Toggle" => Role::Switch,
        "Alert" => Role::Dialog,
        "Header" => Role::Heading,
        "WebArea" | "WebView" => Role::Document,
        "TabBar" => Role::TabList,
        "ScrollView" => Role::ScrollView,
        "TextView" => Role::MultilineTextInput,
        "Outline" => Role::Group,
        _ => map_ax_role(ax_role),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_role_name() {
        assert_eq!(parse_role_name("Button"), Some(Role::Button));
        assert_eq!(parse_role_name("button"), Some(Role::Button));
        assert_eq!(parse_role_name("BUTTON"), Some(Role::Button));
        assert_eq!(parse_role_name("TextInput"), Some(Role::TextInput));
        assert_eq!(parse_role_name("input"), Some(Role::TextInput));
        assert_eq!(parse_role_name("TextRun"), Some(Role::TextRun));
        assert_eq!(parse_role_name("MenuCheck"), Some(Role::MenuItemCheckBox));
        assert_eq!(parse_role_name("MenuRadio"), Some(Role::MenuItemRadio));
        assert_eq!(parse_role_name("Item"), Some(Role::ListItem));
        assert_eq!(parse_role_name("Nav"), Some(Role::Navigation));
        assert_eq!(parse_role_name("Header"), Some(Role::Heading));
        assert_eq!(parse_role_name("Document"), Some(Role::Document));
        assert_eq!(parse_role_name("WebView"), Some(Role::WebView));
        assert_eq!(parse_role_name("Aside"), Some(Role::Complementary));
        assert_eq!(parse_role_name("Footer"), Some(Role::ContentInfo));
        assert_eq!(parse_role_name("*"), None);
        assert_eq!(parse_role_name("unknown"), None);
    }

    #[test]
    fn test_role_name_matches_text_aliases() {
        assert!(role_name_matches("Text", Role::TextRun));
        assert!(role_name_matches("Text", Role::Label));
        assert!(role_name_matches("StaticText", Role::TextRun));
        assert!(role_name_matches("StaticText", Role::Label));
        assert!(role_name_matches("TextRun", Role::TextRun));
        assert!(!role_name_matches("TextRun", Role::Label));
        assert!(role_name_matches("Label", Role::Label));
        assert!(!role_name_matches("Label", Role::TextRun));
    }

    #[test]
    fn test_map_ax_role() {
        assert_eq!(map_ax_role("AXButton"), Role::Button);
        assert_eq!(map_ax_role("AXTextField"), Role::TextInput);
        assert_eq!(map_ax_role("AXStaticText"), Role::TextRun);
        assert_eq!(map_ax_role("AXWebArea"), Role::WebView);
        assert_eq!(map_ax_role("AXUnknownRole"), Role::Unknown);
    }

    #[test]
    fn test_map_ax_role_ios() {
        // iOS-specific overrides
        assert_eq!(map_ax_role_ios("AXStaticText"), Role::Label);
        assert_eq!(map_ax_role_ios("AXLabel"), Role::Label);
        assert_eq!(map_ax_role_ios("AXWebArea"), Role::Document);
        assert_eq!(map_ax_role_ios("AXNavigationBar"), Role::Navigation);
        assert_eq!(map_ax_role_ios("AXSearchField"), Role::TextInput);
        assert_eq!(map_ax_role_ios("AXTextView"), Role::MultilineTextInput);
        assert_eq!(map_ax_role_ios("AXOutline"), Role::Group);
        assert_eq!(map_ax_role_ios("AXPicker"), Role::ListBox);
        assert_eq!(map_ax_role_ios("AXSwitch"), Role::Switch);
        assert_eq!(map_ax_role_ios("AXAlert"), Role::Dialog);
        // Shared mappings still work
        assert_eq!(map_ax_role_ios("AXButton"), Role::Button);
        assert_eq!(map_ax_role_ios("AXTextField"), Role::TextInput);
    }
}
