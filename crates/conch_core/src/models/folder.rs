use serde::{Deserialize, Serialize};

use super::server::ServerEntry;

/// A folder grouping SSH servers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerFolder {
    pub name: String,
    #[serde(default)]
    pub servers: Vec<ServerEntry>,
    #[serde(default)]
    pub subfolders: Vec<ServerFolder>,
}

impl ServerFolder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            servers: Vec::new(),
            subfolders: Vec::new(),
        }
    }
}
