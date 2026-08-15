//! Read and write `~/.ssh/known_hosts` in OpenSSH format.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::RemoteError;

/// Returns the default path to `~/.ssh/known_hosts`, or `None` if the home
/// directory cannot be determined.
pub fn default_known_hosts_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ssh").join("known_hosts"))
}

pub(crate) fn host_key(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// Check if a host key is already in the given `known_hosts` file.
///
/// Returns:
/// - `Some(true)` if the key matches
/// - `Some(false)` if the host exists but key differs (MITM warning)
/// - `None` if the host is unknown
pub fn check_known_host(
    known_hosts_file: &Path,
    host: &str,
    port: u16,
    server_key: &ssh_key::PublicKey,
) -> Option<bool> {
    let contents = fs::read_to_string(known_hosts_file).ok()?;
    let lookup = host_key(host, port);
    let server_key_str = server_key.to_openssh().ok()?;
    let server_key_data = key_data_from_openssh(&server_key_str);

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(4, ' ');
        let Some(hostnames) = parts.next() else {
            continue;
        };
        let Some(key_type) = parts.next() else {
            continue;
        };
        let Some(key_b64) = parts.next() else {
            continue;
        };

        let host_matches = hostnames.split(',').any(|h| h == lookup);
        if !host_matches {
            continue;
        }

        let existing_data = format!("{key_type} {key_b64}");
        if existing_data == server_key_data {
            return Some(true);
        } else {
            return Some(false);
        }
    }

    None
}

/// Add a host key to the given `known_hosts` file.
pub fn add_known_host(
    known_hosts_file: &Path,
    host: &str,
    port: u16,
    server_key: &ssh_key::PublicKey,
) -> Result<(), RemoteError> {
    if let Some(parent) = known_hosts_file.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| RemoteError::KnownHosts(format!("cannot create ~/.ssh: {e}")))?;
    }

    let key_str = server_key
        .to_openssh()
        .map_err(|e| RemoteError::KnownHosts(format!("cannot encode public key: {e}")))?;
    let key_data = key_data_from_openssh(&key_str);
    let hostname = host_key(host, port);
    let line = format!("{hostname} {key_data}\n");

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(known_hosts_file)
        .map_err(|e| RemoteError::KnownHosts(format!("cannot open known_hosts: {e}")))?;

    file.write_all(line.as_bytes())
        .map_err(|e| RemoteError::KnownHosts(format!("cannot write known_hosts: {e}")))?;

    Ok(())
}

pub(crate) fn key_data_from_openssh(openssh_str: &str) -> String {
    let mut parts = openssh_str.splitn(3, ' ');
    let key_type = parts.next().unwrap_or("");
    let b64 = parts.next().unwrap_or("");
    format!("{key_type} {b64}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_key_standard_port() {
        assert_eq!(host_key("example.com", 22), "example.com");
    }

    #[test]
    fn host_key_custom_port() {
        assert_eq!(host_key("example.com", 2222), "[example.com]:2222");
    }

    #[test]
    fn key_data_strips_comment() {
        let input = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest user@host";
        let data = key_data_from_openssh(input);
        assert_eq!(data, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest");
    }

    #[test]
    fn key_data_no_comment() {
        let input = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest";
        let data = key_data_from_openssh(input);
        assert_eq!(data, "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITest");
    }

    #[test]
    fn default_known_hosts_path_is_under_ssh_dir() {
        if let Some(path) = default_known_hosts_path() {
            assert!(path.ends_with(".ssh/known_hosts"));
        }
    }
}
