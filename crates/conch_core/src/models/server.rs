use serde::{Deserialize, Serialize};

/// A single SSH server entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerEntry {
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub identity_file: Option<String>,
    #[serde(default)]
    pub proxy_command: Option<String>,
    #[serde(default)]
    pub proxy_jump: Option<String>,
    #[serde(default)]
    pub startup_command: Option<String>,
    /// Unique key for session restore (e.g. "user@host:port")
    #[serde(default)]
    pub session_key: Option<String>,
    /// Whether this came from ~/.ssh/config (not persisted)
    #[serde(skip)]
    pub from_ssh_config: bool,
}

fn default_port() -> u16 {
    22
}

impl ServerEntry {
    pub fn session_key(&self) -> String {
        self.session_key.clone().unwrap_or_else(|| {
            format!("{}@{}:{}", self.user, self.host, self.port)
        })
    }

    pub fn display_name(&self) -> &str {
        &self.name
    }
}
