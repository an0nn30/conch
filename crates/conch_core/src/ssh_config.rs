use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::models::ServerEntry;

/// Parsed SSH config host block.
#[derive(Debug, Clone, Default)]
struct HostBlock {
    host_pattern: String,
    hostname: Option<String>,
    port: Option<u16>,
    user: Option<String>,
    identity_file: Option<String>,
    proxy_command: Option<String>,
    proxy_jump: Option<String>,
}

/// Parse ~/.ssh/config and return ServerEntry list for non-wildcard hosts.
pub fn parse_ssh_config() -> Result<Vec<ServerEntry>> {
    let path = ssh_config_path();
    if !path.exists() {
        return Ok(Vec::new());
    }

    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    Ok(parse_ssh_config_str(&contents))
}

fn ssh_config_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".ssh")
        .join("config")
}

fn parse_ssh_config_str(contents: &str) -> Vec<ServerEntry> {
    let mut blocks: Vec<HostBlock> = Vec::new();
    let mut current: Option<HostBlock> = None;
    // Track global defaults from Host *
    let mut defaults = HashMap::<String, String>::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Split on first whitespace or =
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.trim().to_lowercase(), v.trim().to_string()),
            None => continue,
        };

        if key == "host" {
            if let Some(block) = current.take() {
                blocks.push(block);
            }
            current = Some(HostBlock {
                host_pattern: value.clone(),
                ..Default::default()
            });
        } else if let Some(ref mut block) = current {
            apply_directive(block, &key, &value);
        } else {
            // Before any Host block — treat as global
            defaults.insert(key, value);
        }
    }
    if let Some(block) = current.take() {
        blocks.push(block);
    }

    // Extract defaults from Host * block
    let star_blocks: Vec<_> = blocks.iter()
        .filter(|b| b.host_pattern == "*")
        .cloned()
        .collect();

    blocks
        .into_iter()
        .filter(|b| !b.host_pattern.contains('*') && !b.host_pattern.contains('?'))
        .map(|b| {
            let default_user = star_blocks.iter().find_map(|s| s.user.clone())
                .or_else(|| defaults.get("user").cloned());
            ServerEntry {
                name: b.host_pattern.clone(),
                host: b.hostname.unwrap_or_else(|| b.host_pattern.clone()),
                port: b.port.unwrap_or(22),
                user: b.user.or(default_user).unwrap_or_default(),
                identity_file: b.identity_file,
                proxy_command: b.proxy_command,
                proxy_jump: b.proxy_jump,
                startup_command: None,
                session_key: None,
                from_ssh_config: true,
            }
        })
        .collect()
}

fn apply_directive(block: &mut HostBlock, key: &str, value: &str) {
    match key {
        "hostname" => block.hostname = Some(value.to_string()),
        "port" => block.port = value.parse().ok(),
        "user" => block.user = Some(value.to_string()),
        "identityfile" => {
            let expanded = shellexpand_tilde(value);
            block.identity_file = Some(expanded);
        }
        "proxycommand" => block.proxy_command = Some(value.to_string()),
        "proxyjump" => block.proxy_jump = Some(value.to_string()),
        _ => {} // Ignore unsupported directives
    }
}

fn shellexpand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic() {
        let input = r#"
Host web-01
    HostName 192.168.1.10
    User admin
    Port 2222
    IdentityFile ~/.ssh/id_ed25519

Host db-01
    HostName 10.0.0.5
    User root

Host *
    User default_user
"#;
        let entries = parse_ssh_config_str(input);
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].name, "web-01");
        assert_eq!(entries[0].host, "192.168.1.10");
        assert_eq!(entries[0].port, 2222);
        assert_eq!(entries[0].user, "admin");

        assert_eq!(entries[1].name, "db-01");
        assert_eq!(entries[1].host, "10.0.0.5");
        assert_eq!(entries[1].user, "root");
        assert_eq!(entries[1].port, 22);
    }

    #[test]
    fn test_wildcards_excluded() {
        let input = "Host *\n    User foo\n";
        let entries = parse_ssh_config_str(input);
        assert!(entries.is_empty());
    }
}
