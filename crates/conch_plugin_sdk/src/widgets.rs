//! Declarative widget types for plugin UI.
//!
//! Plugins describe their UI as a tree of `Widget` values. Native plugins
//! serialize this tree to JSON and pass it to `HostApi::set_widgets`. Lua
//! plugins use `ui.panel_*` helpers that build the same tree internally.
//!
//! The host parses the widget tree once, caches it, and renders it every frame
//! using egui. Plugins only push a new tree when their state changes.

use serde::{Deserialize, Serialize};

/// A single widget in the declarative UI tree.
///
/// Widgets are composable — layout widgets (`Horizontal`, `Vertical`,
/// `SplitPane`, `ScrollArea`, `Tabs`) contain child widgets.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Widget {
    // -- Layout Containers -------------------------------------------------

    /// Horizontal row of child widgets.
    Horizontal {
        id: Option<String>,
        children: Vec<Widget>,
        spacing: Option<f32>,
        /// When true, the row is centered within its parent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        centered: Option<bool>,
    },

    /// Vertical column of child widgets.
    Vertical {
        id: Option<String>,
        children: Vec<Widget>,
        spacing: Option<f32>,
    },

    /// Two-pane split (horizontal or vertical) with adjustable ratio.
    SplitPane {
        id: String,
        /// "horizontal" or "vertical".
        direction: SplitDirection,
        /// Split ratio (0.0–1.0). 0.5 = equal.
        ratio: f32,
        /// Whether the user can drag to resize.
        resizable: bool,
        left: Box<Widget>,
        right: Box<Widget>,
    },

    /// Scrollable container.
    ScrollArea {
        id: Option<String>,
        /// Max height in points. None = fill available space.
        max_height: Option<f32>,
        children: Vec<Widget>,
    },

    /// Tab container — each child is a tab pane.
    Tabs {
        id: String,
        /// Index of the active tab (0-based).
        active: usize,
        tabs: Vec<TabPane>,
    },

    // -- Data Display ------------------------------------------------------

    /// Section heading (larger, bold text).
    Heading {
        text: String,
    },

    /// Standard label text.
    Label {
        text: String,
        /// Optional: "secondary", "muted", "accent", "warn", "error".
        style: Option<TextStyle>,
    },

    /// Monospace text (for code, paths, log output).
    Text {
        text: String,
    },

    /// Scrollable monospace text area that sticks to the bottom.
    /// Useful for live log output.
    ScrollText {
        id: String,
        text: String,
        /// Max height in points.
        max_height: Option<f32>,
    },

    /// Key-value pair display (label on left, value on right).
    KeyValue {
        key: String,
        value: String,
    },

    /// Visual separator line.
    Separator,

    /// Flexible or fixed-size spacer.
    Spacer {
        /// Size in points. None = flexible fill.
        size: Option<f32>,
    },

    /// Icon + label combination.
    IconLabel {
        icon: String,
        text: String,
        /// Optional: "secondary", "muted".
        style: Option<TextStyle>,
    },

    /// Small status badge (e.g., "connected", "error").
    Badge {
        text: String,
        /// "info", "success", "warn", "error".
        variant: BadgeVariant,
    },

    /// Progress bar with optional label.
    Progress {
        id: String,
        /// 0.0–1.0.
        fraction: f32,
        /// Text shown alongside the bar.
        label: Option<String>,
    },

    /// Inline image by path or embedded data.
    Image {
        id: Option<String>,
        /// File path or "data:..." URI.
        src: String,
        /// Width in points. None = auto.
        width: Option<f32>,
        /// Height in points. None = auto.
        height: Option<f32>,
    },

    // -- Interactive Widgets -----------------------------------------------

    /// Clickable button.
    Button {
        id: String,
        label: String,
        /// Optional icon name.
        icon: Option<String>,
        enabled: Option<bool>,
    },

    /// Single-line text input.
    TextInput {
        id: String,
        /// Current value.
        value: String,
        /// Placeholder text.
        hint: Option<String>,
        /// Submit on Enter (generates `text_input_submit` event).
        submit_on_enter: Option<bool>,
        /// If true, request keyboard focus on this input this frame.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_focus: Option<bool>,
    },

    /// Multi-line text editor.
    TextEdit {
        id: String,
        value: String,
        hint: Option<String>,
        /// Height in lines.
        lines: Option<u32>,
    },

    /// Checkbox toggle.
    Checkbox {
        id: String,
        label: String,
        checked: bool,
    },

    /// Dropdown selection.
    ComboBox {
        id: String,
        /// Currently selected value.
        selected: String,
        /// Available options.
        options: Vec<ComboBoxOption>,
    },

    // -- Complex Widgets ---------------------------------------------------

    /// Horizontal button/icon toolbar.
    Toolbar {
        id: Option<String>,
        items: Vec<ToolbarItem>,
    },

    /// Clickable breadcrumb path bar.
    PathBar {
        id: String,
        /// Path segments (e.g., ["~", "projects", "conch"]).
        segments: Vec<String>,
    },

    /// Collapsible tree view with icons, badges, and context menus.
    TreeView {
        id: String,
        nodes: Vec<TreeNode>,
        /// ID of the currently selected node.
        selected: Option<String>,
    },

    /// Data table with sortable columns, selectable rows, and context menus.
    Table {
        id: String,
        columns: Vec<TableColumn>,
        rows: Vec<TableRow>,
        /// Column ID currently sorted by.
        sort_column: Option<String>,
        sort_ascending: Option<bool>,
        /// ID of the selected row.
        selected_row: Option<String>,
    },

    /// Drag-and-drop target area.
    DropZone {
        id: String,
        /// Label shown when empty / waiting for drop.
        label: String,
        /// Child widgets rendered inside the zone.
        children: Vec<Widget>,
    },

    /// Context menu definition attached to a parent widget.
    ///
    /// Wrap another widget to give it a right-click menu.
    ContextMenu {
        /// The widget this menu attaches to.
        child: Box<Widget>,
        items: Vec<ContextMenuItem>,
    },
}

// -- Supporting Types ------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabPane {
    pub label: String,
    pub icon: Option<String>,
    pub children: Vec<Widget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextStyle {
    Normal,
    Secondary,
    Muted,
    Accent,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BadgeVariant {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComboBoxOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolbarItem {
    Button {
        id: String,
        icon: Option<String>,
        label: Option<String>,
        tooltip: Option<String>,
        enabled: Option<bool>,
    },
    Separator,
    Spacer,
    TextInput {
        id: String,
        value: String,
        hint: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    /// Color hint for the icon (e.g., "blue", "muted"). Maps to theme colors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
    /// Render the label in bold (e.g., for active/connected items).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bold: Option<bool>,
    /// Optional status badge (e.g., "connected").
    pub badge: Option<String>,
    /// Whether this node is expanded (only meaningful if `children` is non-empty).
    pub expanded: Option<bool>,
    pub children: Vec<TreeNode>,
    /// Right-click menu items for this node.
    pub context_menu: Option<Vec<ContextMenuItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableColumn {
    pub id: String,
    pub label: String,
    /// Whether clicking the header sorts by this column.
    pub sortable: Option<bool>,
    /// Column width in points. None = auto.
    pub width: Option<f32>,
    /// Whether this column is visible. None or Some(true) = visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableRow {
    pub id: String,
    pub cells: Vec<TableCell>,
    /// Right-click menu items for this row.
    pub context_menu: Option<Vec<ContextMenuItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TableCell {
    /// Plain text cell.
    Text(String),
    /// Rich cell with icon and/or badge.
    Rich {
        text: String,
        icon: Option<String>,
        badge: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMenuItem {
    pub id: String,
    pub label: String,
    pub icon: Option<String>,
    pub enabled: Option<bool>,
    /// Keyboard shortcut hint text (display only, not functional).
    pub shortcut: Option<String>,
}

// -- Widget Events ---------------------------------------------------------

/// Events generated by interactive widgets and sent to the plugin.
///
/// The host serializes these to JSON and delivers them via
/// `conch_plugin_event()` on the plugin's thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WidgetEvent {
    /// A button was clicked.
    ButtonClick { id: String },

    /// A tree node was selected (single click).
    TreeSelect { id: String, node_id: String },

    /// A tree node was double-clicked (e.g., connect to server).
    TreeActivate { id: String, node_id: String },

    /// A tree node was expanded or collapsed.
    TreeToggle {
        id: String,
        node_id: String,
        expanded: bool,
    },

    /// A context menu action on a tree node.
    TreeContextMenu {
        id: String,
        node_id: String,
        action: String,
    },

    /// Text input value changed (debounced).
    TextInputChanged { id: String, value: String },

    /// Text input submitted (Enter pressed).
    TextInputSubmit { id: String, value: String },

    /// Text edit value changed.
    TextEditChanged { id: String, value: String },

    /// A table row was selected.
    TableSelect { id: String, row_id: String },

    /// A table row was double-clicked.
    TableActivate { id: String, row_id: String },

    /// A table column header was clicked (sort).
    TableSort {
        id: String,
        column: String,
        ascending: bool,
    },

    /// A context menu action on a table row.
    TableContextMenu {
        id: String,
        row_id: String,
        action: String,
    },

    /// A right-click on a table column header (for column visibility toggles, etc.).
    TableHeaderContextMenu {
        id: String,
        column: String,
    },

    /// A tab was switched.
    TabChanged { id: String, active: usize },

    /// A checkbox was toggled.
    CheckboxChanged { id: String, checked: bool },

    /// A combobox selection changed.
    ComboBoxChanged { id: String, value: String },

    /// A path bar segment was clicked.
    PathBarNavigate {
        id: String,
        /// Index of the clicked segment (0-based).
        segment_index: usize,
    },

    /// Items were dropped onto a drop zone.
    Drop {
        id: String,
        /// Source widget ID (if from within the plugin).
        source: Option<String>,
        /// Dropped items (file paths, node IDs, etc.).
        items: Vec<String>,
    },

    /// A context menu action (standalone context menu widget).
    ContextMenuAction { action: String },

    /// A toolbar text input submitted.
    ToolbarInputSubmit { id: String, value: String },

    /// A toolbar text input changed.
    ToolbarInputChanged { id: String, value: String },
}

// -- Plugin Event Envelope -------------------------------------------------

/// Top-level event delivered to `conch_plugin_event()`.
///
/// This wraps both widget events and system events (IPC, lifecycle).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginEvent {
    /// A widget interaction in one of the plugin's panels.
    Widget(WidgetEvent),

    /// A menu item registered by this plugin was triggered.
    MenuAction { action: String },

    /// An IPC event from the message bus (another plugin published this).
    BusEvent {
        event_type: String,
        data: serde_json::Value,
    },

    /// A direct query from another plugin (this plugin is the service provider).
    BusQuery {
        /// Caller-assigned request ID for correlating the response.
        request_id: String,
        method: String,
        args: serde_json::Value,
    },

    /// The host theme changed (dark/light switch, color scheme change).
    ThemeChanged { theme_json: String },

    /// The plugin is being shut down. Clean up resources.
    Shutdown,
}

// -- Builder Helpers -------------------------------------------------------

/// Convenience helpers for building widget trees in Rust.
impl Widget {
    pub fn button(id: impl Into<String>, label: impl Into<String>) -> Self {
        Widget::Button {
            id: id.into(),
            label: label.into(),
            icon: None,
            enabled: None,
        }
    }

    pub fn label(text: impl Into<String>) -> Self {
        Widget::Label {
            text: text.into(),
            style: None,
        }
    }

    pub fn heading(text: impl Into<String>) -> Self {
        Widget::Heading { text: text.into() }
    }

    pub fn text_input(id: impl Into<String>, value: impl Into<String>) -> Self {
        Widget::TextInput {
            id: id.into(),
            value: value.into(),
            hint: None,
            submit_on_enter: Some(true),
            request_focus: None,
        }
    }

    pub fn separator() -> Self {
        Widget::Separator
    }

    pub fn horizontal(children: Vec<Widget>) -> Self {
        Widget::Horizontal {
            id: None,
            children,
            spacing: None,
            centered: None,
        }
    }

    pub fn vertical(children: Vec<Widget>) -> Self {
        Widget::Vertical {
            id: None,
            children,
            spacing: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialize to JSON and deserialize back, assert they match.
    fn roundtrip(widget: &Widget) -> Widget {
        let json = serde_json::to_string(widget).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    fn roundtrip_event(event: &WidgetEvent) -> WidgetEvent {
        let json = serde_json::to_string(event).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    fn roundtrip_plugin_event(event: &PluginEvent) -> PluginEvent {
        let json = serde_json::to_string(event).unwrap();
        serde_json::from_str(&json).unwrap()
    }

    // -- Widget serde roundtrips --

    #[test]
    fn widget_separator_roundtrip() {
        let w = Widget::Separator;
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains("\"type\":\"separator\""));
        let _: Widget = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn widget_heading_roundtrip() {
        let w = Widget::heading("Hello");
        if let Widget::Heading { text } = roundtrip(&w) {
            assert_eq!(text, "Hello");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_label_roundtrip() {
        let w = Widget::Label { text: "test".into(), style: Some(TextStyle::Accent) };
        if let Widget::Label { text, style } = roundtrip(&w) {
            assert_eq!(text, "test");
            assert!(matches!(style, Some(TextStyle::Accent)));
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_text_roundtrip() {
        let w = Widget::Text { text: "monospace".into() };
        if let Widget::Text { text } = roundtrip(&w) {
            assert_eq!(text, "monospace");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_button_roundtrip() {
        let w = Widget::Button {
            id: "btn".into(),
            label: "Click".into(),
            icon: Some("plus".into()),
            enabled: Some(false),
        };
        if let Widget::Button { id, label, icon, enabled } = roundtrip(&w) {
            assert_eq!(id, "btn");
            assert_eq!(label, "Click");
            assert_eq!(icon.as_deref(), Some("plus"));
            assert_eq!(enabled, Some(false));
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_text_input_roundtrip() {
        let w = Widget::TextInput {
            id: "search".into(),
            value: "foo".into(),
            hint: Some("Search...".into()),
            submit_on_enter: Some(true),
            request_focus: None,
        };
        if let Widget::TextInput { id, value, hint, submit_on_enter, .. } = roundtrip(&w) {
            assert_eq!(id, "search");
            assert_eq!(value, "foo");
            assert_eq!(hint.as_deref(), Some("Search..."));
            assert_eq!(submit_on_enter, Some(true));
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_checkbox_roundtrip() {
        let w = Widget::Checkbox { id: "cb".into(), label: "Enable".into(), checked: true };
        if let Widget::Checkbox { id, checked, .. } = roundtrip(&w) {
            assert_eq!(id, "cb");
            assert!(checked);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_progress_roundtrip() {
        let w = Widget::Progress { id: "p".into(), fraction: 0.75, label: Some("75%".into()) };
        if let Widget::Progress { fraction, .. } = roundtrip(&w) {
            assert!((fraction - 0.75).abs() < 0.001);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_horizontal_with_children() {
        let w = Widget::horizontal(vec![
            Widget::label("A"),
            Widget::separator(),
            Widget::button("b", "B"),
        ]);
        if let Widget::Horizontal { children, .. } = roundtrip(&w) {
            assert_eq!(children.len(), 3);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_vertical_with_children() {
        let w = Widget::vertical(vec![Widget::heading("Title"), Widget::separator()]);
        if let Widget::Vertical { children, .. } = roundtrip(&w) {
            assert_eq!(children.len(), 2);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_tree_view_roundtrip() {
        let w = Widget::TreeView {
            id: "tree".into(),
            nodes: vec![
                TreeNode {
                    id: "root".into(),
                    label: "Root".into(),
                    icon: Some("folder".into()),
                    icon_color: None,
                    bold: None,
                    badge: None,
                    expanded: Some(true),
                    children: vec![
                        TreeNode {
                            id: "child".into(),
                            label: "Child".into(),
                            icon: None,
                            icon_color: None,
                            bold: None,
                            badge: Some("new".into()),
                            expanded: None,
                            children: vec![],
                            context_menu: None,
                        },
                    ],
                    context_menu: Some(vec![
                        ContextMenuItem {
                            id: "delete".into(),
                            label: "Delete".into(),
                            icon: None,
                            enabled: Some(true),
                            shortcut: None,
                        },
                    ]),
                },
            ],
            selected: Some("child".into()),
        };
        if let Widget::TreeView { id, nodes, selected } = roundtrip(&w) {
            assert_eq!(id, "tree");
            assert_eq!(nodes.len(), 1);
            assert_eq!(nodes[0].children.len(), 1);
            assert_eq!(nodes[0].children[0].badge.as_deref(), Some("new"));
            assert_eq!(selected.as_deref(), Some("child"));
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_table_roundtrip() {
        let w = Widget::Table {
            id: "files".into(),
            columns: vec![
                TableColumn { id: "name".into(), label: "Name".into(), sortable: Some(true), width: None, visible: None },
                TableColumn { id: "size".into(), label: "Size".into(), sortable: Some(true), width: Some(80.0), visible: None },
            ],
            rows: vec![
                TableRow {
                    id: "r1".into(),
                    cells: vec![
                        TableCell::Text("file.txt".into()),
                        TableCell::Rich { text: "1.2 KB".into(), icon: None, badge: None },
                    ],
                    context_menu: None,
                },
            ],
            sort_column: Some("name".into()),
            sort_ascending: Some(true),
            selected_row: None,
        };
        if let Widget::Table { columns, rows, sort_column, .. } = roundtrip(&w) {
            assert_eq!(columns.len(), 2);
            assert_eq!(rows.len(), 1);
            assert_eq!(sort_column.as_deref(), Some("name"));
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_split_pane_roundtrip() {
        let w = Widget::SplitPane {
            id: "split".into(),
            direction: SplitDirection::Horizontal,
            ratio: 0.3,
            resizable: true,
            left: Box::new(Widget::label("Left")),
            right: Box::new(Widget::label("Right")),
        };
        if let Widget::SplitPane { ratio, resizable, direction, .. } = roundtrip(&w) {
            assert!((ratio - 0.3).abs() < 0.001);
            assert!(resizable);
            assert!(matches!(direction, SplitDirection::Horizontal));
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_tabs_roundtrip() {
        let w = Widget::Tabs {
            id: "tabs".into(),
            active: 1,
            tabs: vec![
                TabPane { label: "Tab A".into(), icon: None, children: vec![Widget::label("A")] },
                TabPane { label: "Tab B".into(), icon: Some("star".into()), children: vec![] },
            ],
        };
        if let Widget::Tabs { active, tabs, .. } = roundtrip(&w) {
            assert_eq!(active, 1);
            assert_eq!(tabs.len(), 2);
            assert_eq!(tabs[1].icon.as_deref(), Some("star"));
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_toolbar_roundtrip() {
        let w = Widget::Toolbar {
            id: Some("tb".into()),
            items: vec![
                ToolbarItem::Button {
                    id: "add".into(), icon: Some("plus".into()),
                    label: None, tooltip: Some("Add".into()), enabled: None,
                },
                ToolbarItem::Separator,
                ToolbarItem::Spacer,
                ToolbarItem::TextInput {
                    id: "search".into(), value: "".into(), hint: Some("Search".into()),
                },
            ],
        };
        if let Widget::Toolbar { items, .. } = roundtrip(&w) {
            assert_eq!(items.len(), 4);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_combobox_roundtrip() {
        let w = Widget::ComboBox {
            id: "sort".into(),
            selected: "name".into(),
            options: vec![
                ComboBoxOption { value: "name".into(), label: "Name".into() },
                ComboBoxOption { value: "size".into(), label: "Size".into() },
            ],
        };
        if let Widget::ComboBox { selected, options, .. } = roundtrip(&w) {
            assert_eq!(selected, "name");
            assert_eq!(options.len(), 2);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_path_bar_roundtrip() {
        let w = Widget::PathBar {
            id: "path".into(),
            segments: vec!["~".into(), "projects".into(), "conch".into()],
        };
        if let Widget::PathBar { segments, .. } = roundtrip(&w) {
            assert_eq!(segments, vec!["~", "projects", "conch"]);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_badge_roundtrip() {
        let w = Widget::Badge { text: "ok".into(), variant: BadgeVariant::Success };
        if let Widget::Badge { text, variant } = roundtrip(&w) {
            assert_eq!(text, "ok");
            assert!(matches!(variant, BadgeVariant::Success));
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_spacer_roundtrip() {
        let w = Widget::Spacer { size: Some(10.0) };
        if let Widget::Spacer { size } = roundtrip(&w) {
            assert_eq!(size, Some(10.0));
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_key_value_roundtrip() {
        let w = Widget::KeyValue { key: "Host".into(), value: "example.com".into() };
        if let Widget::KeyValue { key, value } = roundtrip(&w) {
            assert_eq!(key, "Host");
            assert_eq!(value, "example.com");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_scroll_area_roundtrip() {
        let w = Widget::ScrollArea {
            id: Some("scroll".into()),
            max_height: Some(200.0),
            children: vec![Widget::label("item")],
        };
        if let Widget::ScrollArea { max_height, children, .. } = roundtrip(&w) {
            assert_eq!(max_height, Some(200.0));
            assert_eq!(children.len(), 1);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_drop_zone_roundtrip() {
        let w = Widget::DropZone {
            id: "dz".into(),
            label: "Drop files here".into(),
            children: vec![],
        };
        if let Widget::DropZone { label, .. } = roundtrip(&w) {
            assert_eq!(label, "Drop files here");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn widget_context_menu_roundtrip() {
        let w = Widget::ContextMenu {
            child: Box::new(Widget::label("Right-click me")),
            items: vec![
                ContextMenuItem {
                    id: "copy".into(), label: "Copy".into(),
                    icon: None, enabled: None, shortcut: Some("Cmd+C".into()),
                },
            ],
        };
        if let Widget::ContextMenu { items, .. } = roundtrip(&w) {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].shortcut.as_deref(), Some("Cmd+C"));
        } else {
            panic!("Wrong variant");
        }
    }

    // -- Widget list roundtrip (what set_widgets actually sends) --

    #[test]
    fn widget_vec_roundtrip() {
        let widgets = vec![
            Widget::heading("Sessions"),
            Widget::separator(),
            Widget::button("add", "Add Server"),
        ];
        let json = serde_json::to_string(&widgets).unwrap();
        let parsed: Vec<Widget> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 3);
    }

    // -- WidgetEvent serde roundtrips --

    #[test]
    fn event_button_click() {
        let e = WidgetEvent::ButtonClick { id: "btn".into() };
        if let WidgetEvent::ButtonClick { id } = roundtrip_event(&e) {
            assert_eq!(id, "btn");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn event_tree_select() {
        let e = WidgetEvent::TreeSelect { id: "tree".into(), node_id: "n1".into() };
        if let WidgetEvent::TreeSelect { node_id, .. } = roundtrip_event(&e) {
            assert_eq!(node_id, "n1");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn event_tree_context_menu() {
        let e = WidgetEvent::TreeContextMenu {
            id: "tree".into(), node_id: "n1".into(), action: "delete".into(),
        };
        if let WidgetEvent::TreeContextMenu { action, .. } = roundtrip_event(&e) {
            assert_eq!(action, "delete");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn event_text_input_submit() {
        let e = WidgetEvent::TextInputSubmit { id: "search".into(), value: "query".into() };
        if let WidgetEvent::TextInputSubmit { value, .. } = roundtrip_event(&e) {
            assert_eq!(value, "query");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn event_table_header_context_menu() {
        let e = WidgetEvent::TableHeaderContextMenu { id: "files".into(), column: "ext".into() };
        if let WidgetEvent::TableHeaderContextMenu { id, column } = roundtrip_event(&e) {
            assert_eq!(id, "files");
            assert_eq!(column, "ext");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn event_table_sort() {
        let e = WidgetEvent::TableSort { id: "files".into(), column: "size".into(), ascending: false };
        if let WidgetEvent::TableSort { column, ascending, .. } = roundtrip_event(&e) {
            assert_eq!(column, "size");
            assert!(!ascending);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn event_checkbox_changed() {
        let e = WidgetEvent::CheckboxChanged { id: "cb".into(), checked: true };
        if let WidgetEvent::CheckboxChanged { checked, .. } = roundtrip_event(&e) {
            assert!(checked);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn event_combobox_changed() {
        let e = WidgetEvent::ComboBoxChanged { id: "sort".into(), value: "size".into() };
        if let WidgetEvent::ComboBoxChanged { value, .. } = roundtrip_event(&e) {
            assert_eq!(value, "size");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn event_tab_changed() {
        let e = WidgetEvent::TabChanged { id: "tabs".into(), active: 2 };
        if let WidgetEvent::TabChanged { active, .. } = roundtrip_event(&e) {
            assert_eq!(active, 2);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn event_path_bar_navigate() {
        let e = WidgetEvent::PathBarNavigate { id: "path".into(), segment_index: 1 };
        if let WidgetEvent::PathBarNavigate { segment_index, .. } = roundtrip_event(&e) {
            assert_eq!(segment_index, 1);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn event_drop() {
        let e = WidgetEvent::Drop {
            id: "dz".into(),
            source: Some("local".into()),
            items: vec!["/tmp/file.txt".into()],
        };
        if let WidgetEvent::Drop { items, source, .. } = roundtrip_event(&e) {
            assert_eq!(items.len(), 1);
            assert_eq!(source.as_deref(), Some("local"));
        } else {
            panic!("Wrong variant");
        }
    }

    // -- PluginEvent serde roundtrips --

    #[test]
    fn plugin_event_widget_roundtrip() {
        let e = PluginEvent::Widget(WidgetEvent::ButtonClick { id: "add_server".into() });
        let json = serde_json::to_string(&e).unwrap();
        // Verify no "type" collision — "kind" tags the outer, "type" tags the inner.
        assert!(json.contains("\"kind\":\"widget\""), "expected kind tag, got: {json}");
        assert!(json.contains("\"type\":\"button_click\""), "expected type tag, got: {json}");
        if let PluginEvent::Widget(WidgetEvent::ButtonClick { id }) = roundtrip_plugin_event(&e) {
            assert_eq!(id, "add_server");
        } else {
            panic!("Wrong variant after roundtrip");
        }
    }

    #[test]
    fn plugin_event_menu_action() {
        let e = PluginEvent::MenuAction { action: "ssh.connect".into() };
        if let PluginEvent::MenuAction { action } = roundtrip_plugin_event(&e) {
            assert_eq!(action, "ssh.connect");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn plugin_event_bus_event() {
        let e = PluginEvent::BusEvent {
            event_type: "ssh.session_ready".into(),
            data: serde_json::json!({"session_id": 42}),
        };
        if let PluginEvent::BusEvent { event_type, data } = roundtrip_plugin_event(&e) {
            assert_eq!(event_type, "ssh.session_ready");
            assert_eq!(data["session_id"], 42);
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn plugin_event_bus_query() {
        let e = PluginEvent::BusQuery {
            request_id: "req-1".into(),
            method: "exec".into(),
            args: serde_json::json!({"command": "ls"}),
        };
        if let PluginEvent::BusQuery { method, args, .. } = roundtrip_plugin_event(&e) {
            assert_eq!(method, "exec");
            assert_eq!(args["command"], "ls");
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn plugin_event_shutdown() {
        let e = PluginEvent::Shutdown;
        let json = serde_json::to_string(&e).unwrap();
        let parsed: PluginEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, PluginEvent::Shutdown));
    }

    #[test]
    fn plugin_event_theme_changed() {
        let e = PluginEvent::ThemeChanged { theme_json: "{}".into() };
        if let PluginEvent::ThemeChanged { theme_json } = roundtrip_plugin_event(&e) {
            assert_eq!(theme_json, "{}");
        } else {
            panic!("Wrong variant");
        }
    }

    // -- JSON from external source (what a C/Go plugin would produce) --

    #[test]
    fn parse_widget_from_raw_json() {
        let json = r#"{"type":"button","id":"ok","label":"OK","icon":null,"enabled":true}"#;
        let w: Widget = serde_json::from_str(json).unwrap();
        assert!(matches!(w, Widget::Button { .. }));
    }

    #[test]
    fn parse_widget_list_from_raw_json() {
        let json = r#"[
            {"type":"heading","text":"Title"},
            {"type":"separator"},
            {"type":"label","text":"Hello","style":"muted"}
        ]"#;
        let widgets: Vec<Widget> = serde_json::from_str(json).unwrap();
        assert_eq!(widgets.len(), 3);
    }

    #[test]
    fn parse_event_from_raw_json() {
        let json = r#"{"type":"button_click","id":"connect"}"#;
        let e: WidgetEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(e, WidgetEvent::ButtonClick { .. }));
    }

    // -- Builder helpers --

    #[test]
    fn builder_button() {
        if let Widget::Button { id, label, icon, enabled } = Widget::button("ok", "OK") {
            assert_eq!(id, "ok");
            assert_eq!(label, "OK");
            assert!(icon.is_none());
            assert!(enabled.is_none());
        } else {
            panic!("Wrong variant");
        }
    }

    #[test]
    fn builder_text_input() {
        if let Widget::TextInput { submit_on_enter, .. } = Widget::text_input("s", "") {
            assert_eq!(submit_on_enter, Some(true));
        } else {
            panic!("Wrong variant");
        }
    }
}
