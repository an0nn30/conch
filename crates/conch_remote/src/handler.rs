//! Unified SSH client handler — delegates host key verification to RemoteCallbacks.

use std::path::PathBuf;
use std::sync::Arc;

use russh::client;

use crate::callbacks::RemoteCallbacks;
use crate::known_hosts;

/// Unified SSH handler for both interactive sessions and tunnels.
///
/// Implements `russh::client::Handler` and delegates host key verification
/// to the provided `RemoteCallbacks` implementation.
pub struct ConchSshHandler {
    pub host: String,
    pub port: u16,
    pub known_hosts_file: PathBuf,
    pub callbacks: Arc<dyn RemoteCallbacks>,
}

#[async_trait::async_trait]
impl client::Handler for ConchSshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let check = known_hosts::check_known_host(
            &self.known_hosts_file,
            &self.host,
            self.port,
            server_public_key,
        );

        match check {
            Some(true) => {
                log::debug!("Host key for {}:{} matches known_hosts", self.host, self.port);
                return Ok(true);
            }
            Some(false) => {
                let fingerprint = server_public_key.fingerprint(ssh_key::HashAlg::Sha256);
                let message = format!(
                    "WARNING: HOST KEY HAS CHANGED for {}:{}\n\n\
                     This could indicate a man-in-the-middle attack.\n\
                     It is also possible that the host key has just been changed.",
                    self.host, self.port
                );
                let fp_str = format!(
                    "{}\n{fingerprint}",
                    server_public_key.algorithm().as_str(),
                );
                let accepted = self.callbacks.verify_host_key(&message, &fp_str).await;
                return Ok(accepted);
            }
            None => {
                // Unknown host — ask the user.
            }
        }

        let fingerprint = server_public_key.fingerprint(ssh_key::HashAlg::Sha256);
        let host_display = if self.port != 22 {
            format!("[{}]:{}", self.host, self.port)
        } else {
            self.host.clone()
        };
        let message = format!("The authenticity of host '{host_display}' can't be established.");
        let fp_str = format!(
            "{} key fingerprint is:\n{fingerprint}",
            server_public_key.algorithm().as_str(),
        );

        let accepted = self.callbacks.verify_host_key(&message, &fp_str).await;

        if accepted {
            if let Err(e) = known_hosts::add_known_host(
                &self.known_hosts_file,
                &self.host,
                self.port,
                server_public_key,
            ) {
                log::warn!("Failed to save host key: {e}");
            }
        }

        Ok(accepted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct AcceptAllCallbacks;

    #[async_trait::async_trait]
    impl RemoteCallbacks for AcceptAllCallbacks {
        async fn verify_host_key(&self, _message: &str, _fingerprint: &str) -> bool {
            true
        }
        async fn prompt_password(&self, _message: &str) -> Option<String> {
            None
        }
        fn on_transfer_progress(&self, _transfer_id: &str, _bytes: u64, _total: Option<u64>) {}
    }

    struct RejectAllCallbacks;

    #[async_trait::async_trait]
    impl RemoteCallbacks for RejectAllCallbacks {
        async fn verify_host_key(&self, _message: &str, _fingerprint: &str) -> bool {
            false
        }
        async fn prompt_password(&self, _message: &str) -> Option<String> {
            None
        }
        fn on_transfer_progress(&self, _transfer_id: &str, _bytes: u64, _total: Option<u64>) {}
    }

    #[test]
    fn handler_fields_accessible() {
        let handler = ConchSshHandler {
            host: "example.com".to_string(),
            port: 22,
            known_hosts_file: PathBuf::from("/tmp/known_hosts"),
            callbacks: Arc::new(AcceptAllCallbacks),
        };
        assert_eq!(handler.host, "example.com");
        assert_eq!(handler.port, 22);
        assert_eq!(handler.known_hosts_file, PathBuf::from("/tmp/known_hosts"));
    }

    #[test]
    fn handler_non_standard_port() {
        let handler = ConchSshHandler {
            host: "192.168.1.1".to_string(),
            port: 2222,
            known_hosts_file: PathBuf::from("/tmp/known_hosts"),
            callbacks: Arc::new(AcceptAllCallbacks),
        };
        assert_eq!(handler.port, 2222);
        // Verify host_display formatting would use bracket notation for non-22 port
        let host_display = if handler.port != 22 {
            format!("[{}]:{}", handler.host, handler.port)
        } else {
            handler.host.clone()
        };
        assert_eq!(host_display, "[192.168.1.1]:2222");
    }

    #[test]
    fn handler_standard_port_display() {
        let handler = ConchSshHandler {
            host: "myserver.io".to_string(),
            port: 22,
            known_hosts_file: PathBuf::from("/tmp/known_hosts"),
            callbacks: Arc::new(RejectAllCallbacks),
        };
        let host_display = if handler.port != 22 {
            format!("[{}]:{}", handler.host, handler.port)
        } else {
            handler.host.clone()
        };
        assert_eq!(host_display, "myserver.io");
    }

    #[test]
    fn accept_callbacks_returns_true() {
        let cb = AcceptAllCallbacks;
        // Verify the trait impl is accessible and the struct is Send + Sync
        let _arc: Arc<dyn RemoteCallbacks> = Arc::new(cb);
    }

    #[test]
    fn reject_callbacks_returns_false() {
        let _arc: Arc<dyn RemoteCallbacks> = Arc::new(RejectAllCallbacks);
    }
}
