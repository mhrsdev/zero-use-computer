//! Normalize native accessibility roles and actions into one vocabulary so
//! the model sees the same words on every platform.

/// macOS AX role (+ subrole) → normalized role.
pub fn from_ax(role: &str, subrole: Option<&str>) -> String {
    if let Some(sub) = subrole {
        let s = match sub {
            "AXSecureTextField" => Some("secure text field"),
            "AXSearchField" => Some("search field"),
            "AXCloseButton" => Some("close button"),
            "AXMinimizeButton" => Some("minimize button"),
            "AXZoomButton" => Some("zoom button"),
            "AXFullScreenButton" => Some("full screen button"),
            "AXToggle" => Some("toggle button"),
            "AXSwitch" => Some("switch"),
            "AXTabButton" => Some("tab"),
            "AXDialog" | "AXSystemDialog" => Some("dialog"),
            "AXFloatingWindow" => Some("floating window"),
            "AXOutlineRow" => Some("tree item"),
            "AXTableRow" => Some("row"),
            "AXSortButton" => Some("sort button"),
            "AXTextLink" => Some("link"),
            "AXContentList" | "AXDefinitionList" => Some("list"),
            _ => None,
        };
        if let Some(s) = s {
            return s.to_string();
        }
    }
    let r = match role {
        "AXApplication" => "application",
        "AXWindow" => "window",
        "AXSheet" => "sheet",
        "AXDrawer" => "drawer",
        "AXButton" => "button",
        "AXRadioButton" => "radio button",
        "AXCheckBox" => "checkbox",
        "AXPopUpButton" => "pop up button",
        "AXMenuButton" => "menu button",
        "AXComboBox" => "combo box",
        "AXDisclosureTriangle" => "disclosure triangle",
        "AXTextField" => "text field",
        "AXTextArea" => "text area",
        "AXStaticText" => "text",
        "AXHeading" => "heading",
        "AXLink" => "link",
        "AXImage" => "image",
        "AXSlider" => "slider",
        "AXIncrementor" | "AXStepper" => "stepper",
        "AXProgressIndicator" | "AXBusyIndicator" => "progress indicator",
        "AXLevelIndicator" | "AXRelevanceIndicator" | "AXValueIndicator" => "indicator",
        "AXScrollArea" => "scroll area",
        "AXScrollBar" => "scroll bar",
        "AXSplitGroup" => "split group",
        "AXSplitter" => "splitter",
        "AXGroup" => "group",
        "AXRadioGroup" => "radio group",
        "AXTabGroup" => "tab group",
        "AXToolbar" => "toolbar",
        "AXList" => "list",
        "AXTable" => "table",
        "AXOutline" => "outline",
        "AXBrowser" => "browser",
        "AXRow" => "row",
        "AXColumn" => "column",
        "AXCell" => "cell",
        "AXMenuBar" => "menu bar",
        "AXMenuBarItem" => "menu bar item",
        "AXMenu" => "menu",
        "AXMenuItem" => "menu item",
        "AXWebArea" => "web area",
        "AXColorWell" => "color well",
        "AXDateField" => "date field",
        "AXTimeField" => "time field",
        "AXLayoutArea" => "layout area",
        "AXLayoutItem" => "layout item",
        "AXHandle" => "handle",
        "AXGrowArea" => "grow area",
        "AXMatte" => "matte",
        "AXRuler" | "AXRulerMarker" => "ruler",
        "AXPopover" => "popover",
        "AXHelpTag" => "tooltip",
        "AXUnknown" => "unknown",
        other => return other.strip_prefix("AX").unwrap_or(other).to_lowercase(),
    };
    r.to_string()
}

/// macOS AX action → normalized action name.
pub fn ax_action(native: &str) -> String {
    match native {
        "AXPress" => "press".into(),
        "AXShowMenu" => "show_menu".into(),
        "AXIncrement" => "increment".into(),
        "AXDecrement" => "decrement".into(),
        "AXConfirm" => "confirm".into(),
        "AXCancel" => "cancel".into(),
        "AXRaise" => "raise".into(),
        "AXPick" => "pick".into(),
        "AXShowAlternateUI" => "show_alternate_ui".into(),
        "AXShowDefaultUI" => "show_default_ui".into(),
        "AXScrollToVisible" => "scroll_to_visible".into(),
        "AXOpen" => "open".into(),
        other => snake(other.strip_prefix("AX").unwrap_or(other)),
    }
}

/// AT-SPI role name (as returned by `GetRoleName`) → normalized role.
pub fn from_atspi(role_name: &str) -> String {
    let r = match role_name {
        "frame" => "window",
        "push button" | "button" => "button",
        "toggle button" => "toggle button",
        "check box" => "checkbox",
        "radio button" => "radio button",
        "combo box" => "combo box",
        "entry" | "text" => "text field",
        "password text" => "secure text field",
        "label" | "static" => "text",
        "page tab" => "tab",
        "page tab list" => "tab group",
        "menu item" | "check menu item" | "radio menu item" | "tearoff menu item" => "menu item",
        "tool bar" => "toolbar",
        "scroll pane" => "scroll area",
        "scroll bar" => "scroll bar",
        "spin button" => "stepper",
        "list item" => "list item",
        "table cell" => "cell",
        "tree table" | "tree" => "outline",
        "panel" | "filler" | "grouping" | "section" => "group",
        "split pane" => "split group",
        "progress bar" => "progress indicator",
        "document web" | "document frame" => "web area",
        "status bar" => "status bar",
        "layered pane" | "root pane" | "glass pane" | "viewport" => "group",
        "icon" => "image",
        other => other,
    };
    r.to_string()
}

/// AT-SPI action name → normalized action name.
pub fn atspi_action(native: &str) -> String {
    match native.to_lowercase().as_str() {
        "click" | "press" | "activate" | "jump" => "press".into(),
        "toggle" => "toggle".into(),
        "expand or contract" | "expand or collapse" => "expand_or_collapse".into(),
        "menu" | "popup" | "showmenu" => "show_menu".into(),
        other => snake(other),
    }
}

/// UI Automation control type id → normalized role.
pub fn from_uia(control_type: i32) -> String {
    let r = match control_type {
        50000 => "button",
        50001 => "calendar",
        50002 => "checkbox",
        50003 => "combo box",
        50004 => "text field",
        50005 => "link",
        50006 => "image",
        50007 => "list item",
        50008 => "list",
        50009 => "menu",
        50010 => "menu bar",
        50011 => "menu item",
        50012 => "progress indicator",
        50013 => "radio button",
        50014 => "scroll bar",
        50015 => "slider",
        50016 => "stepper",
        50017 => "status bar",
        50018 => "tab group",
        50019 => "tab",
        50020 => "text",
        50021 => "toolbar",
        50022 => "tooltip",
        50023 => "outline",
        50024 => "tree item",
        50025 => "custom",
        50026 => "group",
        50027 => "thumb",
        50028 => "data grid",
        50029 => "data item",
        50030 => "document",
        50031 => "split button",
        50032 => "window",
        50033 => "pane",
        50034 => "header",
        50035 => "header item",
        50036 => "table",
        50037 => "title bar",
        50038 => "separator",
        50039 => "semantic zoom",
        50040 => "app bar",
        _ => "unknown",
    };
    r.to_string()
}

fn snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        if c == ' ' || c == '-' {
            out.push('_');
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Roles that carry structure worth keeping even without a label.
pub fn is_structural(role: &str) -> bool {
    matches!(
        role,
        "window"
            | "dialog"
            | "sheet"
            | "floating window"
            | "drawer"
            | "popover"
            | "menu bar"
            | "menu"
            | "list"
            | "table"
            | "outline"
            | "data grid"
            | "browser"
            | "tab group"
            | "toolbar"
            | "scroll area"
            | "web area"
            | "document"
            | "radio group"
            | "status bar"
            | "row"
            | "list item"
            | "tree item"
    )
}

/// Roles that are interactive by nature (kept even without actions).
pub fn is_interactive(role: &str) -> bool {
    matches!(
        role,
        "button"
            | "toggle button"
            | "checkbox"
            | "radio button"
            | "switch"
            | "pop up button"
            | "menu button"
            | "combo box"
            | "text field"
            | "secure text field"
            | "search field"
            | "text area"
            | "link"
            | "slider"
            | "stepper"
            | "tab"
            | "menu item"
            | "menu bar item"
            | "cell"
            | "disclosure triangle"
            | "split button"
            | "date field"
            | "time field"
            | "color well"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(from_ax("AXButton", None), "button");
        assert_eq!(
            from_ax("AXTextField", Some("AXSecureTextField")),
            "secure text field"
        );
        assert_eq!(from_ax("AXFancyThing", None), "fancything");
        assert_eq!(ax_action("AXShowMenu"), "show_menu");
        assert_eq!(ax_action("AXZoomWindow"), "zoom_window");
        assert_eq!(from_atspi("push button"), "button");
        assert_eq!(atspi_action("expand or contract"), "expand_or_collapse");
        assert_eq!(from_uia(50000), "button");
        assert_eq!(from_uia(1), "unknown");
    }
}
