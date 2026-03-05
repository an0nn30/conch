use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A saved SSH tunnel (local port forward).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedTunnel {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub label: String,
    pub session_key: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    #[serde(default)]
    pub auto_start: bool,
}

impl SavedTunnel {
    pub fn description(&self) -> String {
        format!(
            ":{} -> {}:{}",
            self.local_port, self.remote_host, self.remote_port
        )
    }
}
