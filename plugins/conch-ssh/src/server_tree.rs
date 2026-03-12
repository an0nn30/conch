//! Server tree widget builder — constructs the JSON widget tree for the
//! Sessions panel.
//!
//! This exercises: Toolbar, TextInput, Button, TreeView, TreeNode,
//! ContextMenuItem, Badge, Separator, Label.

use std::collections::HashMap;

use conch_plugin_sdk::widgets::*;

use crate::config::SshConfig;
use crate::session_backend::SshSessionBackend;

/// Build the full widget tree for the SSH Sessions panel.
pub fn build_server_tree(
    config: &SshConfig,
    sessions: &HashMap<u64, SshSessionBackend>,
    selected: Option<&str>,
) -> Vec<Widget> {
    let mut widgets = Vec::new();

    // -- Toolbar: Add Server, Add Folder, Quick Connect --
    widgets.push(Widget::Toolbar {
        id: Some("ssh_toolbar".to_string()),
        items: vec![
            ToolbarItem::Button {
                id: "add_server".to_string(),
                icon: Some("plus".to_string()),
                label: None,
                tooltip: Some("Add Server".to_string()),
                enabled: None,
            },
            ToolbarItem::Button {
                id: "add_folder".to_string(),
                icon: Some("folder-plus".to_string()),
                label: None,
                tooltip: Some("Add Folder".to_string()),
                enabled: None,
            },
            ToolbarItem::Separator,
            ToolbarItem::TextInput {
                id: "quick_connect".to_string(),
                value: String::new(),
                hint: Some("user@host:port".to_string()),
            },
        ],
    });

    widgets.push(Widget::Separator);

    // -- Server Tree --
    let mut tree_nodes = Vec::new();

    // Folders
    for folder in &config.folders {
        let children: Vec<TreeNode> = folder.entries.iter().map(|entry| {
            server_to_tree_node(entry, sessions)
        }).collect();

        tree_nodes.push(TreeNode {
            id: folder.id.clone(),
            label: folder.name.clone(),
            icon: Some("folder".to_string()),
            badge: None,
            expanded: Some(folder.expanded),
            children,
            context_menu: Some(vec![
                ContextMenuItem {
                    id: "rename".to_string(),
                    label: "Rename Folder".to_string(),
                    icon: Some("edit".to_string()),
                    enabled: None,
                    shortcut: None,
                },
                ContextMenuItem {
                    id: "delete".to_string(),
                    label: "Delete Folder".to_string(),
                    icon: Some("trash".to_string()),
                    enabled: None,
                    shortcut: None,
                },
            ]),
        });
    }

    // Ungrouped servers
    for entry in &config.ungrouped {
        tree_nodes.push(server_to_tree_node(entry, sessions));
    }

    widgets.push(Widget::TreeView {
        id: "server_tree".to_string(),
        nodes: tree_nodes,
        selected: selected.map(String::from),
    });

    // -- Active sessions summary --
    if !sessions.is_empty() {
        widgets.push(Widget::Separator);
        widgets.push(Widget::heading("Active Sessions"));
        for (id, backend) in sessions {
            widgets.push(Widget::horizontal(vec![
                Widget::IconLabel {
                    icon: "terminal".to_string(),
                    text: format!("{}@{}", backend.user(), backend.host()),
                    style: None,
                },
                Widget::Badge {
                    text: "connected".to_string(),
                    variant: BadgeVariant::Success,
                },
            ]));
            let _ = id; // suppress unused warning in stub
        }
    }

    widgets
}

/// Convert a ServerEntry to a tree node, with "connected" badge if active.
fn server_to_tree_node(
    entry: &crate::config::ServerEntry,
    sessions: &HashMap<u64, SshSessionBackend>,
) -> TreeNode {
    let is_connected = sessions.values().any(|s| s.host() == entry.host);

    TreeNode {
        id: entry.id.clone(),
        label: entry.label.clone(),
        icon: Some("server".to_string()),
        badge: if is_connected { Some("connected".to_string()) } else { None },
        expanded: None,
        children: Vec::new(),
        context_menu: Some(vec![
            ContextMenuItem {
                id: "connect".to_string(),
                label: "Connect".to_string(),
                icon: Some("plug".to_string()),
                enabled: Some(!is_connected),
                shortcut: None,
            },
            ContextMenuItem {
                id: "edit".to_string(),
                label: "Edit...".to_string(),
                icon: Some("edit".to_string()),
                enabled: None,
                shortcut: None,
            },
            ContextMenuItem {
                id: "duplicate".to_string(),
                label: "Duplicate".to_string(),
                icon: Some("copy".to_string()),
                enabled: None,
                shortcut: None,
            },
            ContextMenuItem {
                id: "copy_host".to_string(),
                label: "Copy Hostname".to_string(),
                icon: None,
                enabled: None,
                shortcut: Some("Cmd+C".to_string()),
            },
            ContextMenuItem {
                id: "delete".to_string(),
                label: "Delete".to_string(),
                icon: Some("trash".to_string()),
                enabled: None,
                shortcut: None,
            },
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServerEntry, ServerFolder, SshConfig};

    fn make_entry(id: &str, host: &str, user: &str) -> ServerEntry {
        ServerEntry {
            id: id.to_string(),
            label: format!("{} ({})", id, host),
            host: host.to_string(),
            port: 22,
            user: user.to_string(),
            auth_method: "key".to_string(),
            key_path: None,
        }
    }

    fn empty_sessions() -> HashMap<u64, SshSessionBackend> {
        HashMap::new()
    }

    fn make_config_with_folder() -> SshConfig {
        SshConfig {
            folders: vec![ServerFolder {
                id: "folder_0".to_string(),
                name: "Production".to_string(),
                expanded: true,
                entries: vec![make_entry("srv1", "prod.example.com", "deploy")],
            }],
            ungrouped: vec![make_entry("srv2", "10.0.0.1", "root")],
        }
    }

    #[test]
    fn empty_config_produces_toolbar_separator_tree() {
        let cfg = SshConfig::default();
        let widgets = build_server_tree(&cfg, &empty_sessions(), None);
        // Should have: toolbar, separator, tree_view (empty)
        assert_eq!(widgets.len(), 3);
        matches!(&widgets[0], Widget::Toolbar { .. });
        matches!(&widgets[1], Widget::Separator);
        matches!(&widgets[2], Widget::TreeView { .. });
    }

    #[test]
    fn toolbar_has_expected_items() {
        let cfg = SshConfig::default();
        let widgets = build_server_tree(&cfg, &empty_sessions(), None);
        match &widgets[0] {
            Widget::Toolbar { items, .. } => {
                assert_eq!(items.len(), 4); // add_server, add_folder, separator, quick_connect
                match &items[0] {
                    ToolbarItem::Button { id, .. } => assert_eq!(id, "add_server"),
                    _ => panic!("expected button"),
                }
                match &items[1] {
                    ToolbarItem::Button { id, .. } => assert_eq!(id, "add_folder"),
                    _ => panic!("expected button"),
                }
                assert!(matches!(&items[2], ToolbarItem::Separator));
                match &items[3] {
                    ToolbarItem::TextInput { id, hint, .. } => {
                        assert_eq!(id, "quick_connect");
                        assert_eq!(hint.as_deref(), Some("user@host:port"));
                    }
                    _ => panic!("expected text input"),
                }
            }
            _ => panic!("expected toolbar"),
        }
    }

    #[test]
    fn tree_contains_folder_and_ungrouped() {
        let cfg = make_config_with_folder();
        let widgets = build_server_tree(&cfg, &empty_sessions(), None);
        let tree = &widgets[2];
        match tree {
            Widget::TreeView { nodes, .. } => {
                assert_eq!(nodes.len(), 2); // 1 folder + 1 ungrouped
                assert_eq!(nodes[0].id, "folder_0");
                assert_eq!(nodes[0].label, "Production");
                assert_eq!(nodes[0].children.len(), 1);
                assert_eq!(nodes[0].children[0].id, "srv1");
                assert_eq!(nodes[1].id, "srv2");
            }
            _ => panic!("expected tree view"),
        }
    }

    #[test]
    fn folder_nodes_have_context_menu() {
        let cfg = make_config_with_folder();
        let widgets = build_server_tree(&cfg, &empty_sessions(), None);
        match &widgets[2] {
            Widget::TreeView { nodes, .. } => {
                let folder = &nodes[0];
                let menu = folder.context_menu.as_ref().unwrap();
                assert_eq!(menu.len(), 2);
                assert_eq!(menu[0].id, "rename");
                assert_eq!(menu[1].id, "delete");
            }
            _ => panic!("expected tree view"),
        }
    }

    #[test]
    fn server_nodes_have_five_context_items() {
        let cfg = make_config_with_folder();
        let widgets = build_server_tree(&cfg, &empty_sessions(), None);
        match &widgets[2] {
            Widget::TreeView { nodes, .. } => {
                let server = &nodes[1]; // ungrouped srv2
                let menu = server.context_menu.as_ref().unwrap();
                assert_eq!(menu.len(), 5);
                let ids: Vec<&str> = menu.iter().map(|m| m.id.as_str()).collect();
                assert_eq!(ids, vec!["connect", "edit", "duplicate", "copy_host", "delete"]);
            }
            _ => panic!("expected tree view"),
        }
    }

    #[test]
    fn disconnected_server_connect_is_enabled() {
        let cfg = make_config_with_folder();
        let widgets = build_server_tree(&cfg, &empty_sessions(), None);
        match &widgets[2] {
            Widget::TreeView { nodes, .. } => {
                let server = &nodes[1];
                let connect = &server.context_menu.as_ref().unwrap()[0];
                assert_eq!(connect.enabled, Some(true));
            }
            _ => panic!("expected tree view"),
        }
    }

    #[test]
    fn connected_server_shows_badge_and_disables_connect() {
        let cfg = make_config_with_folder();
        let entry = &cfg.ungrouped[0];
        let backend = SshSessionBackend::new_stub(entry);
        let mut sessions = HashMap::new();
        sessions.insert(1, backend);

        let widgets = build_server_tree(&cfg, &sessions, None);
        match &widgets[2] {
            Widget::TreeView { nodes, .. } => {
                let server = &nodes[1]; // srv2 at 10.0.0.1
                assert_eq!(server.badge.as_deref(), Some("connected"));
                let connect = &server.context_menu.as_ref().unwrap()[0];
                assert_eq!(connect.enabled, Some(false)); // disabled when connected
            }
            _ => panic!("expected tree view"),
        }
    }

    #[test]
    fn active_sessions_appends_summary_widgets() {
        let cfg = make_config_with_folder();
        let entry = &cfg.ungrouped[0];
        let backend = SshSessionBackend::new_stub(entry);
        let mut sessions = HashMap::new();
        sessions.insert(42, backend);

        let widgets = build_server_tree(&cfg, &sessions, None);
        // toolbar, separator, tree, separator, heading, horizontal(icon_label + badge)
        assert!(widgets.len() > 3);
        assert!(matches!(&widgets[3], Widget::Separator));
        match &widgets[4] {
            Widget::Heading { text, .. } => assert_eq!(text, "Active Sessions"),
            _ => panic!("expected heading"),
        }
    }

    #[test]
    fn no_sessions_no_summary() {
        let cfg = make_config_with_folder();
        let widgets = build_server_tree(&cfg, &empty_sessions(), None);
        assert_eq!(widgets.len(), 3); // just toolbar, separator, tree
    }

    #[test]
    fn selected_passed_to_tree() {
        let cfg = make_config_with_folder();
        let widgets = build_server_tree(&cfg, &empty_sessions(), Some("srv2"));
        match &widgets[2] {
            Widget::TreeView { selected, .. } => {
                assert_eq!(selected.as_deref(), Some("srv2"));
            }
            _ => panic!("expected tree view"),
        }
    }

    #[test]
    fn no_selection_is_none() {
        let cfg = make_config_with_folder();
        let widgets = build_server_tree(&cfg, &empty_sessions(), None);
        match &widgets[2] {
            Widget::TreeView { selected, .. } => {
                assert!(selected.is_none());
            }
            _ => panic!("expected tree view"),
        }
    }
}
