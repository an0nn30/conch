//! Structured error type for all remote operations.

use thiserror::Error;

/// Errors returned by `conch_remote` public API functions.
#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("SSH connection failed: {0}")]
    Connection(String),
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("SFTP error: {0}")]
    Sftp(String),
    #[error("transfer failed: {0}")]
    Transfer(String),
    #[error("tunnel error: {0}")]
    Tunnel(String),
    #[error("host key error: {0}")]
    KnownHosts(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_connection() {
        let err = RemoteError::Connection("timed out".into());
        assert_eq!(err.to_string(), "SSH connection failed: timed out");
    }

    #[test]
    fn display_auth() {
        let err = RemoteError::Auth("bad password".into());
        assert_eq!(err.to_string(), "authentication failed: bad password");
    }

    #[test]
    fn display_sftp() {
        let err = RemoteError::Sftp("no such file".into());
        assert_eq!(err.to_string(), "SFTP error: no such file");
    }

    #[test]
    fn display_transfer() {
        let err = RemoteError::Transfer("cancelled".into());
        assert_eq!(err.to_string(), "transfer failed: cancelled");
    }

    #[test]
    fn display_tunnel() {
        let err = RemoteError::Tunnel("port in use".into());
        assert_eq!(err.to_string(), "tunnel error: port in use");
    }

    #[test]
    fn display_known_hosts() {
        let err = RemoteError::KnownHosts("key mismatch".into());
        assert_eq!(err.to_string(), "host key error: key mismatch");
    }

    #[test]
    fn display_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = RemoteError::Io(io_err);
        assert_eq!(err.to_string(), "I/O error: file not found");
    }

    #[test]
    fn display_other() {
        let err = RemoteError::Other("something unexpected".into());
        assert_eq!(err.to_string(), "something unexpected");
    }

    #[test]
    fn from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
        let err: RemoteError = io_err.into();
        assert!(matches!(err, RemoteError::Io(_)));
        assert_eq!(err.to_string(), "I/O error: access denied");
    }

    #[test]
    fn implements_std_error() {
        let err = RemoteError::Connection("test".into());
        // Verify it implements std::error::Error by using it as a trait object.
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn debug_output() {
        let err = RemoteError::Auth("invalid key".into());
        let debug = format!("{err:?}");
        assert!(debug.contains("Auth"));
        assert!(debug.contains("invalid key"));
    }
}
