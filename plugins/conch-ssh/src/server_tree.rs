//! Server tree widget builder — constructs the widget tree for the Sessions panel.

use std::collections::HashMap;

use conch_plugin_sdk::icons;
use conch_plugin_sdk::widgets::*;

use crate::config::{ServerEntry, SshConfig};
use crate::session_backend::SshBackendState;

/// Check if a server entry matches a filter string (case-insensitive).
/// Matches against label, host, and user.
fn entry_matches_filter(entry: &ServerEntry, filter: &str) -> bool {
    let f = filter.to_lowercase();
    entry.label.to_lowercase().contains(&f)
        || entry.host.to_lowercase().contains(&f)
        || entry.user.to_lowercase().contains(&f)
}

/// Return the ID of the first server that matches the filter, searching
/// folders, ungrouped, then ssh config entries in display order.
pub fn first_matching_server_id(
    config: &SshConfig,
    ssh_config_entries: &[ServerEntry],
    filter: &str,
) -> Option<String> {
    if filter.is_empty() {
        return None;
    }
    for folder in &config.folders {
        for entry in &folder.entries {
            if entry_matches_filter(entry, filter) {
                return Some(entry.id.clone());
            }
        }
    }
    for entry in &config.ungrouped {
        if entry_matches_filter(entry, filter) {
            return Some(entry.id.clone());
        }
    }
    for entry in ssh_config_entries {
        if entry_matches_filter(entry, filter) {
            return Some(entry.id.clone());
        }
    }
    None
}

/// Collect all server entries matching a filter into a flat list (no folders).
/// Returns `(id, label, subtitle)` for each match.
pub fn matching_servers<'a>(
    config: &'a SshConfig,
    ssh_config_entries: &'a [ServerEntry],
    filter: &str,
) -> Vec<&'a ServerEntry> {
    let mut results = Vec::new();
    for folder in &config.folders {
        for entry in &folder.entries {
            if entry_matches_filter(entry, filter) {
                results.push(entry);
            }
        }
    }
    for entry in &config.ungrouped {
        if entry_matches_filter(entry, filter) {
            results.push(entry);
        }
    }
    for entry in ssh_config_entries {
        if entry_matches_filter(entry, filter) {
            results.push(entry);
        }
    }
    results
}

/// Build the full widget tree for the SSH Sessions panel.
///
/// When `quick_connect_value` is non-empty, shows a flat filtered list
/// instead of the folder tree.  `search_selected_index` highlights one
/// entry in the filtered list (navigable with arrow keys).
pub fn build_server_tree(
    config: &SshConfig,
    ssh_config_entries: &[ServerEntry],
    sessions: &HashMap<u64, Box<SshBackendState>>,
    selected: Option<&str>,
    quick_connect_value: &str,
    focus_quick_connect: bool,
    search_selected_index: Option<usize>,
) -> Vec<Widget> {
    let mut widgets = Vec::new();
    let filtering = !quick_connect_value.is_empty();

    // -- Toolbar with New Folder button (hidden when filtering) --
    if !filtering {
        widgets.push(Widget::Toolbar {
            id: Some("session_toolbar".to_string()),
            items: vec![
                ToolbarItem::Spacer,
                ToolbarItem::Button {
                    id: "add_folder".to_string(),
                    icon: Some(icons::FOLDER_NEW.to_string()),
                    label: None,
                    tooltip: Some("New folder".to_string()),
                    enabled: None,
                },
            ],
        });
    }

    // -- Quick Connect search bar --
    widgets.push(Widget::TextInput {
        id: "quick_connect".to_string(),
        value: quick_connect_value.to_string(),
        hint: Some("Search sessions or user@host:port...".to_string()),
        submit_on_enter: Some(true),
        request_focus: if focus_quick_connect { Some(true) } else { None },
    });

    if filtering {
        // -- Flat filtered list (no folders) --
        let matches = matching_servers(config, ssh_config_entries, quick_connect_value);
        let selected_idx = search_selected_index.unwrap_or(0);

        let mut tree_nodes = Vec::new();
        for (i, entry) in matches.iter().enumerate() {
            let is_connected = sessions.values().any(|s| s.host() == entry.host);
            let is_selected = i == selected_idx;
            tree_nodes.push(TreeNode {
                id: entry.id.clone(),
                label: entry.label.clone(),
                icon: Some(icons::COMPUTER.to_string()),
                icon_color: None,
                bold: if is_connected || is_selected { Some(true) } else { None },
                badge: None,
                expanded: None,
                children: Vec::new(),
                context_menu: None,
            });
        }

        if tree_nodes.is_empty() {
            widgets.push(Widget::Label {
                text: "No matching connections".to_string(),
                style: Some(conch_plugin_sdk::widgets::TextStyle::Muted),
            });
        } else {
            widgets.push(Widget::TreeView {
                id: "server_tree".to_string(),
                nodes: tree_nodes,
                selected: matches.get(selected_idx).map(|e| e.id.clone()),
            });
        }
    } else {
        // -- Full server tree with folders --
        let mut tree_nodes = Vec::new();

        // User-created folders.
        for folder in &config.folders {
            let children: Vec<TreeNode> = folder.entries.iter()
                .map(|entry| server_to_tree_node(entry, sessions))
                .collect();

            tree_nodes.push(TreeNode {
                id: folder.id.clone(),
                label: folder.name.clone(),
                icon: Some(icons::FOLDER.to_string()),
                icon_color: Some("blue".to_string()),
                bold: None,
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

        // Ungrouped servers.
        for entry in &config.ungrouped {
            tree_nodes.push(server_to_tree_node(entry, sessions));
        }

        // ~/.ssh/config folder.
        if !ssh_config_entries.is_empty() {
            let children: Vec<TreeNode> = ssh_config_entries.iter()
                .map(|entry| server_to_tree_node(entry, sessions))
                .collect();

            tree_nodes.push(TreeNode {
                id: "sshconfig_folder".to_string(),
                label: "~/.ssh/config".to_string(),
                icon: Some(icons::FOLDER.to_string()),
                icon_color: Some("blue".to_string()),
                bold: None,
                badge: None,
                expanded: Some(true),
                children,
                context_menu: None,
            });
        }

        widgets.push(Widget::TreeView {
            id: "server_tree".to_string(),
            nodes: tree_nodes,
            selected: selected.map(String::from),
        });

        // -- Footer --
        widgets.push(Widget::Separator);
        widgets.push(Widget::Button {
            id: "add_server".to_string(),
            label: "+ New Connection".to_string(),
            icon: Some(icons::COMPUTER.to_string()),
            enabled: None,
        });
    }

    widgets
}

/// Convert a ServerEntry to a tree node with monitor icon.
/// Connected servers are shown in bold.
fn server_to_tree_node(
    entry: &ServerEntry,
    sessions: &HashMap<u64, Box<SshBackendState>>,
) -> TreeNode {
    let is_connected = sessions.values().any(|s| s.host() == entry.host);

    TreeNode {
        id: entry.id.clone(),
        label: entry.label.clone(),
        icon: Some(icons::COMPUTER.to_string()),
        icon_color: None,
        bold: if is_connected { Some(true) } else { None },
        badge: None,
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
    use crate::config::{ServerFolder, SshConfig};

    fn make_entry(id: &str, host: &str, user: &str) -> ServerEntry {
        ServerEntry {
            id: id.to_string(),
            label: format!("{} ({})", id, host),
            host: host.to_string(),
            port: 22,
            user: user.to_string(),
            auth_method: "key".to_string(),
            key_path: None,
            proxy_command: None,
            proxy_jump: None,
        }
    }

    fn empty_sessions() -> HashMap<u64, Box<SshBackendState>> {
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
            tunnels: Vec::new(),
        }
    }

    fn make_ssh_config_entries() -> Vec<ServerEntry> {
        vec![
            make_entry("sshconfig_k8s-cp-1", "10.0.1.1", "admin"),
            make_entry("sshconfig_k8s-cp-2", "10.0.1.2", "admin"),
        ]
    }

    #[test]
    fn empty_config_produces_toolbar_input_tree_and_footer() {
        let cfg = SshConfig::default();
        let widgets = build_server_tree(&cfg, &[], &empty_sessions(), None, "", false, None);
        // Toolbar + TextInput + TreeView + Separator + Button
        assert_eq!(widgets.len(), 5);
        assert!(matches!(&widgets[0], Widget::Toolbar { .. }));
        assert!(matches!(&widgets[1], Widget::TextInput { .. }));
        assert!(matches!(&widgets[2], Widget::TreeView { .. }));
        assert!(matches!(&widgets[3], Widget::Separator));
        assert!(matches!(&widgets[4], Widget::Button { .. }));
    }

    #[test]
    fn quick_connect_input_has_correct_props() {
        let cfg = SshConfig::default();
        let widgets = build_server_tree(&cfg, &[], &empty_sessions(), None, "", false, None);
        match &widgets[1] {
            Widget::TextInput { id, hint, submit_on_enter, .. } => {
                assert_eq!(id, "quick_connect");
                assert_eq!(hint.as_deref(), Some("Search sessions or user@host:port..."));
                assert_eq!(*submit_on_enter, Some(true));
            }
            _ => panic!("expected text input"),
        }
    }

    #[test]
    fn tree_contains_folder_and_ungrouped() {
        let cfg = make_config_with_folder();
        let widgets = build_server_tree(&cfg, &[], &empty_sessions(), None, "", false, None);
        match &widgets[2] {
            Widget::TreeView { nodes, .. } => {
                assert_eq!(nodes.len(), 2); // 1 folder + 1 ungrouped
                assert_eq!(nodes[0].id, "folder_0");
                assert_eq!(nodes[0].icon_color.as_deref(), Some("blue"));
                assert_eq!(nodes[1].id, "srv2");
                assert_eq!(nodes[1].icon.as_deref(), Some(icons::COMPUTER));
            }
            _ => panic!("expected tree view"),
        }
    }

    #[test]
    fn ssh_config_entries_appear_as_muted_folder() {
        let cfg = SshConfig::default();
        let ssh_entries = make_ssh_config_entries();
        let widgets = build_server_tree(&cfg, &ssh_entries, &empty_sessions(), None, "", false, None);
        match &widgets[2] {
            Widget::TreeView { nodes, .. } => {
                assert_eq!(nodes.len(), 1); // just the ssh config folder
                let folder = &nodes[0];
                assert_eq!(folder.id, "sshconfig_folder");
                assert_eq!(folder.label, "~/.ssh/config");
                assert_eq!(folder.icon_color.as_deref(), Some("blue"));
                assert_eq!(folder.children.len(), 2);
            }
            _ => panic!("expected tree view"),
        }
    }

    #[test]
    fn connected_server_is_bold_not_badged() {
        let cfg = make_config_with_folder();
        let entry = &cfg.ungrouped[0];
        let backend = SshBackendState::new_preallocated(entry.host.clone(), entry.user.clone(), entry.port);
        let mut sessions = HashMap::new();
        sessions.insert(1, backend);

        let widgets = build_server_tree(&cfg, &[], &sessions, None, "", false, None);
        match &widgets[2] {
            Widget::TreeView { nodes, .. } => {
                let server = &nodes[1]; // srv2 at 10.0.0.1
                assert_eq!(server.bold, Some(true));
                assert!(server.badge.is_none());
            }
            _ => panic!("expected tree view"),
        }
    }

    #[test]
    fn no_active_sessions_section() {
        let cfg = make_config_with_folder();
        let entry = &cfg.ungrouped[0];
        let backend = SshBackendState::new_preallocated(entry.host.clone(), entry.user.clone(), entry.port);
        let mut sessions = HashMap::new();
        sessions.insert(42, backend);

        let widgets = build_server_tree(&cfg, &[], &sessions, None, "", false, None);
        // Toolbar + TextInput + TreeView + Separator + Button, no extra Active Sessions widgets.
        assert_eq!(widgets.len(), 5);
    }

    #[test]
    fn selected_passed_to_tree() {
        let cfg = make_config_with_folder();
        let widgets = build_server_tree(&cfg, &[], &empty_sessions(), Some("srv2"), "", false, None);
        match &widgets[2] {
            Widget::TreeView { selected, .. } => {
                assert_eq!(selected.as_deref(), Some("srv2"));
            }
            _ => panic!("expected tree view"),
        }
    }

    #[test]
    fn user_folders_and_ssh_config_coexist() {
        let cfg = make_config_with_folder();
        let ssh_entries = make_ssh_config_entries();
        let widgets = build_server_tree(&cfg, &ssh_entries, &empty_sessions(), None, "", false, None);
        match &widgets[2] {
            Widget::TreeView { nodes, .. } => {
                // folder_0, srv2 (ungrouped), sshconfig_folder
                assert_eq!(nodes.len(), 3);
                assert_eq!(nodes[0].id, "folder_0");
                assert_eq!(nodes[0].icon_color.as_deref(), Some("blue"));
                assert_eq!(nodes[1].id, "srv2");
                assert_eq!(nodes[2].id, "sshconfig_folder");
                assert_eq!(nodes[2].icon_color.as_deref(), Some("blue"));
            }
            _ => panic!("expected tree view"),
        }
    }

    #[test]
    fn filter_narrows_to_flat_matching_entries() {
        let cfg = make_config_with_folder();
        let ssh_entries = make_ssh_config_entries();
        // Filter by "prod" — should match only "prod.example.com" as a flat entry.
        // When filtering: toolbar hidden, layout is TextInput(0) + TreeView(1).
        let widgets = build_server_tree(&cfg, &ssh_entries, &empty_sessions(), None, "prod", false, None);
        match &widgets[1] {
            Widget::TreeView { nodes, .. } => {
                // Flat list: one matching server, no folder nesting.
                assert_eq!(nodes.len(), 1);
                assert_eq!(nodes[0].id, "srv1");
                assert!(nodes[0].children.is_empty());
            }
            _ => panic!("expected tree view"),
        }
    }

    #[test]
    fn filter_empty_string_shows_all() {
        let cfg = make_config_with_folder();
        let widgets = build_server_tree(&cfg, &[], &empty_sessions(), None, "", false, None);
        match &widgets[2] {
            Widget::TreeView { nodes, .. } => {
                assert_eq!(nodes.len(), 2); // folder + ungrouped
            }
            _ => panic!("expected tree view"),
        }
    }
}
