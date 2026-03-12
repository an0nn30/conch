//! SSH plugin configuration — server entries, folders, saved state.
//!
//! In v2, this lives in `~/.config/conch/plugins/ssh/servers.toml`,
//! managed via the HostApi config persistence API.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerEntry {
    pub id: String,
    pub label: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// "key" or "password".
    pub auth_method: String,
    pub key_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerFolder {
    pub id: String,
    pub name: String,
    pub expanded: bool,
    pub entries: Vec<ServerEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SshConfig {
    pub folders: Vec<ServerFolder>,
    /// Standalone servers not in any folder.
    pub ungrouped: Vec<ServerEntry>,
}

impl SshConfig {
    pub fn find_server(&self, id: &str) -> Option<&ServerEntry> {
        self.ungrouped.iter().find(|s| s.id == id)
            .or_else(|| {
                self.folders.iter()
                    .flat_map(|f| f.entries.iter())
                    .find(|s| s.id == id)
            })
    }

    pub fn add_server(&mut self, entry: ServerEntry) {
        self.ungrouped.push(entry);
    }

    pub fn add_folder(&mut self, name: &str) {
        self.folders.push(ServerFolder {
            id: format!("folder_{}", self.folders.len()),
            name: name.to_string(),
            expanded: true,
            entries: Vec::new(),
        });
    }

    pub fn remove_server(&mut self, id: &str) {
        self.ungrouped.retain(|s| s.id != id);
        for folder in &mut self.folders {
            folder.entries.retain(|s| s.id != id);
        }
    }

    pub fn set_folder_expanded(&mut self, folder_id: &str, expanded: bool) {
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == folder_id) {
            f.expanded = expanded;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str, host: &str) -> ServerEntry {
        ServerEntry {
            id: id.to_string(),
            label: id.to_string(),
            host: host.to_string(),
            port: 22,
            user: "root".to_string(),
            auth_method: "key".to_string(),
            key_path: None,
        }
    }

    fn make_config() -> SshConfig {
        let mut cfg = SshConfig::default();
        cfg.add_server(make_entry("srv1", "10.0.0.1"));
        cfg.add_server(make_entry("srv2", "10.0.0.2"));
        cfg.add_folder("Production");
        cfg.folders[0].entries.push(make_entry("srv3", "prod.example.com"));
        cfg
    }

    #[test]
    fn default_config_is_empty() {
        let cfg = SshConfig::default();
        assert!(cfg.folders.is_empty());
        assert!(cfg.ungrouped.is_empty());
    }

    #[test]
    fn add_server_appends_to_ungrouped() {
        let mut cfg = SshConfig::default();
        cfg.add_server(make_entry("a", "host-a"));
        assert_eq!(cfg.ungrouped.len(), 1);
        assert_eq!(cfg.ungrouped[0].id, "a");
    }

    #[test]
    fn add_folder_creates_empty_folder() {
        let mut cfg = SshConfig::default();
        cfg.add_folder("Dev");
        assert_eq!(cfg.folders.len(), 1);
        assert_eq!(cfg.folders[0].name, "Dev");
        assert!(cfg.folders[0].expanded);
        assert!(cfg.folders[0].entries.is_empty());
    }

    #[test]
    fn add_folder_ids_are_sequential() {
        let mut cfg = SshConfig::default();
        cfg.add_folder("A");
        cfg.add_folder("B");
        assert_eq!(cfg.folders[0].id, "folder_0");
        assert_eq!(cfg.folders[1].id, "folder_1");
    }

    #[test]
    fn find_server_in_ungrouped() {
        let cfg = make_config();
        let srv = cfg.find_server("srv1").unwrap();
        assert_eq!(srv.host, "10.0.0.1");
    }

    #[test]
    fn find_server_in_folder() {
        let cfg = make_config();
        let srv = cfg.find_server("srv3").unwrap();
        assert_eq!(srv.host, "prod.example.com");
    }

    #[test]
    fn find_server_missing_returns_none() {
        let cfg = make_config();
        assert!(cfg.find_server("nonexistent").is_none());
    }

    #[test]
    fn remove_server_from_ungrouped() {
        let mut cfg = make_config();
        cfg.remove_server("srv1");
        assert!(cfg.find_server("srv1").is_none());
        assert_eq!(cfg.ungrouped.len(), 1);
    }

    #[test]
    fn remove_server_from_folder() {
        let mut cfg = make_config();
        cfg.remove_server("srv3");
        assert!(cfg.find_server("srv3").is_none());
        assert!(cfg.folders[0].entries.is_empty());
    }

    #[test]
    fn remove_nonexistent_server_is_noop() {
        let mut cfg = make_config();
        cfg.remove_server("nope");
        assert_eq!(cfg.ungrouped.len(), 2);
        assert_eq!(cfg.folders[0].entries.len(), 1);
    }

    #[test]
    fn set_folder_expanded_true_to_false() {
        let mut cfg = make_config();
        assert!(cfg.folders[0].expanded);
        cfg.set_folder_expanded(&cfg.folders[0].id.clone(), false);
        assert!(!cfg.folders[0].expanded);
    }

    #[test]
    fn set_folder_expanded_nonexistent_is_noop() {
        let mut cfg = make_config();
        cfg.set_folder_expanded("bad_id", false);
        // no panic, state unchanged
        assert!(cfg.folders[0].expanded);
    }

    #[test]
    fn serde_roundtrip() {
        let cfg = make_config();
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: SshConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.ungrouped.len(), cfg.ungrouped.len());
        assert_eq!(parsed.folders.len(), cfg.folders.len());
        assert_eq!(parsed.folders[0].entries[0].host, "prod.example.com");
    }

    #[test]
    fn server_entry_key_path_optional() {
        let entry = make_entry("x", "host");
        assert!(entry.key_path.is_none());

        let mut entry2 = make_entry("y", "host2");
        entry2.key_path = Some("/home/user/.ssh/id_rsa".to_string());
        let json = serde_json::to_string(&entry2).unwrap();
        let parsed: ServerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.key_path.unwrap(), "/home/user/.ssh/id_rsa");
    }
}
