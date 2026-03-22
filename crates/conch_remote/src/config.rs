//! SSH server configuration — server entries, folders, persistence.
//!
//! Persisted to a caller-supplied config directory as `servers.json`.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerEntry {
    pub id: String,
    pub label: String,
    pub host: String,
    pub port: u16,
    // Legacy fields — kept for backward compatibility during migration.
    // Will be removed once vault accounts are fully wired (Task 11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// "key" or "password".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
    // New vault reference — links this server to a vault account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_account_id: Option<uuid::Uuid>,
    /// Raw proxy command (e.g., `ssh -W %h:%p bastion`). `%h` and `%p` are expanded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_command: Option<String>,
    /// SSH jump host (converted to `ssh -W %h:%p <jump>` at connect time).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_jump: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerFolder {
    pub id: String,
    pub name: String,
    pub expanded: bool,
    pub entries: Vec<ServerEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedTunnel {
    pub id: Uuid,
    pub label: String,
    /// Legacy field — `user@host:port` identifying the SSH server for this tunnel.
    #[serde(default)]
    pub session_key: String,
    /// New reference to a `ServerEntry` by ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_entry_id: Option<String>,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    #[serde(default)]
    pub auto_start: bool,
}

impl SavedTunnel {
    pub fn make_session_key(user: &str, host: &str, port: u16) -> String {
        format!("{user}@{host}:{port}")
    }

    pub fn parse_session_key(key: &str) -> Option<(String, String, u16)> {
        let (user, rest) = key.split_once('@')?;
        let (host, port_str) = rest.rsplit_once(':')?;
        let port = port_str.parse().ok()?;
        Some((user.to_string(), host.to_string(), port))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SshConfig {
    pub folders: Vec<ServerFolder>,
    pub ungrouped: Vec<ServerEntry>,
    #[serde(default)]
    pub tunnels: Vec<SavedTunnel>,
}

impl SshConfig {
    pub fn find_server(&self, id: &str) -> Option<&ServerEntry> {
        self.ungrouped
            .iter()
            .find(|s| s.id == id)
            .or_else(|| {
                self.folders
                    .iter()
                    .flat_map(|f| f.entries.iter())
                    .find(|s| s.id == id)
            })
    }

    pub fn find_server_by_label(&self, label: &str) -> Option<&ServerEntry> {
        self.ungrouped
            .iter()
            .find(|s| s.label == label)
            .or_else(|| {
                self.folders
                    .iter()
                    .flat_map(|f| f.entries.iter())
                    .find(|s| s.label == label)
            })
    }

    pub fn all_servers(&self) -> impl Iterator<Item = &ServerEntry> {
        self.ungrouped
            .iter()
            .chain(self.folders.iter().flat_map(|f| f.entries.iter()))
    }

    pub fn add_server(&mut self, entry: ServerEntry) {
        self.ungrouped.push(entry);
    }

    pub fn add_folder(&mut self, name: &str) {
        self.folders.push(ServerFolder {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            expanded: true,
            entries: Vec::new(),
        });
    }

    pub fn add_server_to_folder(&mut self, entry: ServerEntry, folder_id: &str) {
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == folder_id) {
            f.entries.push(entry);
        } else {
            self.ungrouped.push(entry);
        }
    }

    pub fn find_server_folder(&self, server_id: &str) -> Option<&str> {
        self.folders
            .iter()
            .find(|f| f.entries.iter().any(|s| s.id == server_id))
            .map(|f| f.id.as_str())
    }

    pub fn remove_server(&mut self, id: &str) {
        self.ungrouped.retain(|s| s.id != id);
        for folder in &mut self.folders {
            folder.entries.retain(|s| s.id != id);
        }
    }

    pub fn remove_folder(&mut self, folder_id: &str) {
        self.folders.retain(|f| f.id != folder_id);
    }

    pub fn set_folder_expanded(&mut self, folder_id: &str, expanded: bool) {
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == folder_id) {
            f.expanded = expanded;
        }
    }

    // -- Tunnel operations --

    pub fn find_tunnel(&self, id: &Uuid) -> Option<&SavedTunnel> {
        self.tunnels.iter().find(|t| t.id == *id)
    }

    pub fn add_tunnel(&mut self, tunnel: SavedTunnel) {
        self.tunnels.push(tunnel);
    }

    pub fn remove_tunnel(&mut self, id: &Uuid) {
        self.tunnels.retain(|t| t.id != *id);
    }

    pub fn update_tunnel(&mut self, tunnel: SavedTunnel) {
        if let Some(existing) = self.tunnels.iter_mut().find(|t| t.id == tunnel.id) {
            *existing = tunnel;
        }
    }

    // -- Migration helpers --

    /// Returns true if any server entry has legacy user/auth fields but no vault
    /// account linked yet.
    pub fn has_legacy_entries(&self) -> bool {
        self.all_servers()
            .any(|e| e.user.is_some() && e.vault_account_id.is_none())
    }

    /// Collect unique (user, key_path) combinations from legacy entries.
    ///
    /// Returns a `Vec` of `(username, key_path, display_name_hint)` tuples.
    /// Entries that share the same user + key_path are de-duplicated so that
    /// exactly one vault account is created per distinct credential.
    pub fn collect_unique_credentials(&self) -> Vec<(String, Option<String>, String)> {
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for entry in self.all_servers() {
            if let Some(user) = &entry.user {
                let key = (user.clone(), entry.key_path.clone());
                if seen.insert(key.clone()) {
                    let hint = format!("{}@{}", user, entry.host);
                    result.push((user.clone(), entry.key_path.clone(), hint));
                }
            }
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Load the SSH config from `config_dir/servers.json`.
/// Returns an empty `SshConfig` if the file does not exist or cannot be parsed.
pub fn load_config(config_dir: &Path) -> SshConfig {
    let path = config_dir.join("servers.json");
    match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => SshConfig::default(),
    }
}

/// Persist the SSH config to `config_dir/servers.json`.
/// Creates `config_dir` if it does not exist.
pub fn save_config(config_dir: &Path, config: &SshConfig) {
    let _ = fs::create_dir_all(config_dir);
    if let Ok(json) = serde_json::to_string_pretty(config) {
        let _ = fs::write(config_dir.join("servers.json"), json);
    }
}

// ---------------------------------------------------------------------------
// Export / Import
// ---------------------------------------------------------------------------

/// Portable export format — contains servers (with folders) and tunnels.
/// Passwords and absolute key paths are intentionally excluded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPayload {
    pub version: u32,
    pub folders: Vec<ServerFolder>,
    pub ungrouped: Vec<ServerEntry>,
    pub tunnels: Vec<SavedTunnel>,
}

impl SshConfig {
    /// Create an export payload, optionally filtered to specific IDs.
    pub fn to_export_filtered(
        &self,
        server_ids: Option<&[String]>,
        tunnel_ids: Option<&[String]>,
    ) -> ExportPayload {
        let (folders, ungrouped) = match server_ids {
            None => (self.folders.clone(), self.ungrouped.clone()),
            Some(ids) => {
                let ungrouped: Vec<ServerEntry> = self
                    .ungrouped
                    .iter()
                    .filter(|s| ids.contains(&s.id))
                    .cloned()
                    .collect();
                let folders: Vec<ServerFolder> = self
                    .folders
                    .iter()
                    .filter_map(|f| {
                        let entries: Vec<ServerEntry> = f
                            .entries
                            .iter()
                            .filter(|s| ids.contains(&s.id))
                            .cloned()
                            .collect();
                        if entries.is_empty() {
                            None
                        } else {
                            Some(ServerFolder {
                                id: f.id.clone(),
                                name: f.name.clone(),
                                expanded: f.expanded,
                                entries,
                            })
                        }
                    })
                    .collect();
                (folders, ungrouped)
            }
        };

        let tunnels = match tunnel_ids {
            None => self.tunnels.clone(),
            Some(ids) => self
                .tunnels
                .iter()
                .filter(|t| ids.contains(&t.id.to_string()))
                .cloned()
                .collect(),
        };

        let mut payload = ExportPayload {
            version: 1,
            folders,
            ungrouped,
            tunnels,
        };

        // Strip vault_account_id from exported entries — vault references are
        // local to this machine and should not leak into portable exports.
        for entry in &mut payload.ungrouped {
            entry.vault_account_id = None;
        }
        for folder in &mut payload.folders {
            for entry in &mut folder.entries {
                entry.vault_account_id = None;
            }
        }

        payload
    }

    /// Merge an import payload into the current config.
    /// Assigns new IDs to avoid collisions. Returns counts of imported items.
    pub fn merge_import(&mut self, payload: ExportPayload) -> (usize, usize, usize) {
        let mut servers = 0usize;
        let mut folders = 0usize;
        let mut tunnels = 0usize;

        for mut folder in payload.folders {
            folder.id = Uuid::new_v4().to_string();
            for entry in &mut folder.entries {
                entry.id = Uuid::new_v4().to_string();
                servers += 1;
            }
            self.folders.push(folder);
            folders += 1;
        }

        for mut entry in payload.ungrouped {
            entry.id = Uuid::new_v4().to_string();
            self.ungrouped.push(entry);
            servers += 1;
        }

        for mut tunnel in payload.tunnels {
            tunnel.id = Uuid::new_v4();
            self.tunnels.push(tunnel);
            tunnels += 1;
        }

        (servers, folders, tunnels)
    }
}

// ---------------------------------------------------------------------------
// ~/.ssh/config import
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "ios"))]
pub fn parse_ssh_config() -> Vec<ServerEntry> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let path = home.join(".ssh").join("config");
    let contents = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    parse_ssh_config_str(&contents)
}

#[cfg(not(target_os = "ios"))]
fn parse_ssh_config_str(contents: &str) -> Vec<ServerEntry> {
    let mut entries = Vec::new();
    let mut current: Option<PartialEntry> = None;

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (key, value) = match line.split_once(char::is_whitespace) {
            Some((k, v)) => (k.to_lowercase(), v.trim().to_string()),
            None => continue,
        };

        match key.as_str() {
            "host" => {
                if let Some(entry) = current.take() {
                    if let Some(se) = entry.into_server_entry() {
                        entries.push(se);
                    }
                }
                if value.contains('*') || value.contains('?') {
                    continue;
                }
                current = Some(PartialEntry {
                    alias: value,
                    hostname: None,
                    user: None,
                    port: None,
                    identity_file: None,
                    proxy_command: None,
                    proxy_jump: None,
                });
            }
            "hostname" => {
                if let Some(ref mut e) = current {
                    e.hostname = Some(value);
                }
            }
            "user" => {
                if let Some(ref mut e) = current {
                    e.user = Some(value);
                }
            }
            "port" => {
                if let Some(ref mut e) = current {
                    e.port = value.parse().ok();
                }
            }
            "identityfile" => {
                if let Some(ref mut e) = current {
                    e.identity_file = Some(value);
                }
            }
            "proxycommand" => {
                if let Some(ref mut e) = current {
                    e.proxy_command = Some(value);
                }
            }
            "proxyjump" => {
                if let Some(ref mut e) = current {
                    e.proxy_jump = Some(value);
                }
            }
            "match" => {
                if let Some(entry) = current.take() {
                    if let Some(se) = entry.into_server_entry() {
                        entries.push(se);
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(entry) = current.take() {
        if let Some(se) = entry.into_server_entry() {
            entries.push(se);
        }
    }

    entries
}

#[cfg(not(target_os = "ios"))]
struct PartialEntry {
    alias: String,
    hostname: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    identity_file: Option<String>,
    proxy_command: Option<String>,
    proxy_jump: Option<String>,
}

#[cfg(not(target_os = "ios"))]
impl PartialEntry {
    fn into_server_entry(self) -> Option<ServerEntry> {
        let host = self.hostname.unwrap_or_else(|| self.alias.clone());
        let user = self.user.or_else(|| {
            std::env::var("USER").ok()
        });

        Some(ServerEntry {
            id: format!("sshconfig_{}", self.alias),
            label: self.alias,
            host,
            port: self.port.unwrap_or(22),
            user,
            auth_method: Some("key".to_string()),
            key_path: self.identity_file,
            vault_account_id: None,
            proxy_command: self.proxy_command,
            proxy_jump: self.proxy_jump,
        })
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
            user: Some("root".to_string()),
            auth_method: Some("key".to_string()),
            key_path: None,
            vault_account_id: None,
            proxy_command: None,
            proxy_jump: None,
        }
    }

    fn make_config() -> SshConfig {
        let mut cfg = SshConfig::default();
        cfg.add_server(make_entry("srv1", "10.0.0.1"));
        cfg.add_server(make_entry("srv2", "10.0.0.2"));
        cfg.add_folder("Production");
        cfg.folders[0]
            .entries
            .push(make_entry("srv3", "prod.example.com"));
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
    fn serde_roundtrip() {
        let cfg = make_config();
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: SshConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.ungrouped.len(), cfg.ungrouped.len());
        assert_eq!(parsed.folders.len(), cfg.folders.len());
        assert_eq!(parsed.folders[0].entries[0].host, "prod.example.com");
    }

    #[test]
    fn all_servers_iterates_everything() {
        let cfg = make_config();
        let all: Vec<_> = cfg.all_servers().collect();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn saved_tunnel_session_key() {
        assert_eq!(
            SavedTunnel::make_session_key("root", "example.com", 22),
            "root@example.com:22"
        );
    }

    #[test]
    fn saved_tunnel_parse_session_key() {
        let (user, host, port) = SavedTunnel::parse_session_key("deploy@10.0.0.1:2222").unwrap();
        assert_eq!(user, "deploy");
        assert_eq!(host, "10.0.0.1");
        assert_eq!(port, 2222);
    }

    #[cfg(not(target_os = "ios"))]
    #[test]
    fn parse_ssh_config_basic() {
        let config = "\
Host server1
    HostName 10.0.0.1
    User deploy
    Port 2222

Host server2
    HostName example.com
    IdentityFile ~/.ssh/id_ed25519
";
        let entries = parse_ssh_config_str(config);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].label, "server1");
        assert_eq!(entries[0].host, "10.0.0.1");
        assert_eq!(entries[0].user.as_deref(), Some("deploy"));
        assert_eq!(entries[0].port, 2222);
        assert_eq!(entries[1].host, "example.com");
    }

    #[cfg(not(target_os = "ios"))]
    #[test]
    fn parse_ssh_config_skip_wildcard() {
        let config = "\
Host *
    ServerAliveInterval 60

Host myserver
    HostName 10.0.0.5
";
        let entries = parse_ssh_config_str(config);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "myserver");
    }

    #[cfg(not(target_os = "ios"))]
    #[test]
    fn parse_ssh_config_proxy_jump() {
        let config = "\
Host bastion-target
    HostName 10.0.0.99
    ProxyJump bastion.example.com
";
        let entries = parse_ssh_config_str(config);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].proxy_jump.as_deref(),
            Some("bastion.example.com")
        );
    }

    #[test]
    fn load_config_from_missing_dir() {
        let cfg = load_config(&std::path::PathBuf::from("/nonexistent/dir"));
        assert!(cfg.folders.is_empty());
        assert!(cfg.ungrouped.is_empty());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = SshConfig::default();
        cfg.add_server(make_entry("s1", "host1"));
        save_config(dir.path(), &cfg);
        let loaded = load_config(dir.path());
        assert_eq!(loaded.ungrouped.len(), 1);
        assert_eq!(loaded.ungrouped[0].host, "host1");
    }

    #[test]
    fn legacy_server_entry_deserializes_with_vault_account_id_none() {
        let json = r#"{
            "id": "s1",
            "label": "My Server",
            "host": "example.com",
            "port": 22,
            "user": "deploy",
            "auth_method": "key",
            "key_path": "/home/user/.ssh/id_ed25519"
        }"#;
        let entry: ServerEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.id, "s1");
        assert_eq!(entry.host, "example.com");
        assert!(entry.vault_account_id.is_none());
    }

    #[test]
    fn server_entry_with_vault_id_roundtrips() {
        let id = uuid::Uuid::new_v4();
        let entry = ServerEntry {
            id: "s1".into(),
            label: "Test".into(),
            host: "example.com".into(),
            port: 22,
            user: None,
            auth_method: None,
            key_path: None,
            vault_account_id: Some(id),
            proxy_command: None,
            proxy_jump: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: ServerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.vault_account_id, Some(id));
    }

    #[test]
    fn export_strips_vault_account_id() {
        let mut cfg = SshConfig::default();
        let mut entry = make_entry("s1", "host1");
        entry.vault_account_id = Some(uuid::Uuid::new_v4());
        cfg.add_server(entry);

        let payload = cfg.to_export_filtered(None, None);
        for server in &payload.ungrouped {
            assert!(server.vault_account_id.is_none());
        }
    }

    #[test]
    fn detect_legacy_entries() {
        let json = r#"{
            "folders": [],
            "ungrouped": [{
                "id": "s1", "label": "Old Server", "host": "example.com",
                "port": 22, "user": "root", "auth_method": "key",
                "key_path": "/home/user/.ssh/id_ed25519"
            }],
            "tunnels": []
        }"#;
        let cfg: SshConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.has_legacy_entries());
    }

    #[test]
    fn collect_unique_credentials_deduplicates() {
        let json = r#"{
            "folders": [],
            "ungrouped": [
                {"id": "s1", "label": "A", "host": "a.com", "port": 22,
                 "user": "deploy", "auth_method": "key", "key_path": "/k1"},
                {"id": "s2", "label": "B", "host": "b.com", "port": 22,
                 "user": "deploy", "auth_method": "key", "key_path": "/k1"},
                {"id": "s3", "label": "C", "host": "c.com", "port": 22,
                 "user": "root", "auth_method": "password"}
            ],
            "tunnels": []
        }"#;
        let cfg: SshConfig = serde_json::from_str(json).unwrap();
        let creds = cfg.collect_unique_credentials();
        assert_eq!(creds.len(), 2, "deploy+/k1 and root+password should be 2 unique credentials");
    }

    #[test]
    fn has_legacy_entries_false_when_vault_account_set() {
        let mut cfg = SshConfig::default();
        let mut entry = make_entry("s1", "host1");
        entry.vault_account_id = Some(uuid::Uuid::new_v4());
        // Entry has vault_account_id so it is NOT legacy.
        cfg.add_server(entry);
        assert!(!cfg.has_legacy_entries());
    }

    #[test]
    fn saved_tunnel_with_server_entry_id_roundtrips() {
        let tunnel = SavedTunnel {
            id: uuid::Uuid::new_v4(),
            label: "DB Tunnel".into(),
            server_entry_id: Some("s1".into()),
            session_key: String::new(),
            local_port: 5432,
            remote_host: "db.internal".into(),
            remote_port: 5432,
            auto_start: false,
        };
        let json = serde_json::to_string(&tunnel).unwrap();
        let deserialized: SavedTunnel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.server_entry_id, Some("s1".into()));
    }
}
