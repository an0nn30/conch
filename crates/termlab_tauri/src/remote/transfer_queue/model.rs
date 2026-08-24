use serde::{Deserialize, Serialize};
use termlab_remote::transfer::SourceFingerprint;
use ts_rs::TS;
use uuid::Uuid;

pub const TRANSFER_STORE_VERSION: u32 = 1;
pub const TRANSFER_HISTORY_LIMIT: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum TransferEndpoint {
    Configured {
        server_entry_id: String,
        label: String,
    },
    AdHoc {
        host: String,
        port: u16,
        user: String,
        proxy_command: Option<String>,
        proxy_jump: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TransferProtocol {
    Sftp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TransferDirection {
    Upload,
    Download,
}

/// Build the scheduler's serialization key for the destination of a transfer.
///
/// The connection scope prevents independent endpoints from blocking each
/// other, while lexical path normalization makes equivalent paths contend for
/// the same destination slot without requiring the destination to exist yet.
pub fn build_destination_key(
    host_key: &str,
    direction: &TransferDirection,
    local_path: &str,
    remote_path: &str,
) -> String {
    let destination = match direction {
        TransferDirection::Upload => normalize_destination_path(remote_path),
        TransferDirection::Download => normalize_local_destination_path(local_path),
    };
    format!("{host_key}:{destination}")
}

pub(super) fn normalize_destination_path(path: &str) -> String {
    let absolute = path.starts_with('/');
    let mut components: Vec<&str> = Vec::new();

    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => match components.last() {
                Some(previous) if *previous != ".." => {
                    components.pop();
                }
                _ if !absolute => components.push(component),
                _ => {}
            },
            _ => components.push(component),
        }
    }

    if absolute {
        if components.is_empty() {
            "/".into()
        } else {
            format!("/{}", components.join("/"))
        }
    } else if components.is_empty() {
        ".".into()
    } else {
        components.join("/")
    }
}

pub(super) fn normalize_local_destination_path(path: &str) -> String {
    if uses_windows_path_semantics(path) {
        normalize_windows_path(path)
    } else {
        normalize_destination_path(path)
    }
}

/// Select local path rules without depending on the host running recovery.
///
/// Native Windows builds use Windows rules for every local path. Other hosts
/// recognize drive-prefixed and backslash-UNC forms so persisted Windows jobs
/// retain the same identity during inspection or recovery. All other forms use
/// the existing POSIX lexical rules, including literal backslashes on Unix.
pub(super) fn uses_windows_path_semantics(path: &str) -> bool {
    cfg!(windows) || is_windows_drive_path(path) || path.starts_with(r"\\")
}

pub(super) fn is_windows_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn normalize_windows_path(path: &str) -> String {
    let path = path.replace('/', r"\");
    let mut absolute = false;
    let mut prefix = String::new();
    let components: Vec<&str>;

    if let Some(rest) = path.strip_prefix(r"\\") {
        absolute = true;
        let mut parts = rest.split('\\').filter(|part| !part.is_empty());
        prefix.push_str(r"\\");
        if let Some(server) = parts.next() {
            prefix.push_str(server);
        }
        if let Some(share) = parts.next() {
            prefix.push('\\');
            prefix.push_str(share);
        }
        components = parts.collect();
    } else if is_windows_drive_path(&path) {
        let drive = (path.as_bytes()[0] as char).to_ascii_uppercase();
        prefix.push(drive);
        prefix.push(':');
        let rest = &path[2..];
        absolute = rest.starts_with('\\');
        if absolute {
            prefix.push('\\');
        }
        components = rest.split('\\').filter(|part| !part.is_empty()).collect();
    } else if let Some(rest) = path.strip_prefix('\\') {
        absolute = true;
        prefix.push('\\');
        components = rest.split('\\').filter(|part| !part.is_empty()).collect();
    } else {
        components = path.split('\\').filter(|part| !part.is_empty()).collect();
    }

    let mut normalized = Vec::new();
    for component in components {
        match component {
            "." => {}
            ".." => match normalized.last() {
                Some(previous) if *previous != ".." => {
                    normalized.pop();
                }
                _ if !absolute => normalized.push(component),
                _ => {}
            },
            _ => normalized.push(component),
        }
    }

    if normalized.is_empty() {
        if prefix.is_empty() {
            ".".into()
        } else {
            prefix
        }
    } else if prefix.is_empty() {
        normalized.join(r"\")
    } else if prefix.ends_with('\\') || !absolute {
        format!("{prefix}{}", normalized.join(r"\"))
    } else {
        format!("{prefix}\\{}", normalized.join(r"\"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum TransferOrigin {
    FilesPanel,
    Editor,
    Other { name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TransferPriority {
    Interactive,
    Normal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum ConflictPolicy {
    Ask,
    Overwrite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum ConflictResolution {
    Resume,
    Overwrite,
    Rename { destination: String },
    Skip,
    Restart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum AttentionReason {
    DestinationConflict {
        resume_available: bool,
    },
    SourceChanged {
        expected: SourceFingerprint,
        actual: SourceFingerprint,
    },
    SourceCannotResume,
    MissingPartial,
    CommitRecovery {
        message: String,
    },
    Cleanup {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CompletionResult {
    Transferred,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CommitPhase {
    None,
    Prepared,
    BackupMoved,
    PartialPromoted,
    CleanupPending,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagedArtifacts {
    pub partial_path: String,
    pub backup_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
#[ts(export)]
pub enum TransferJobState {
    Queued,
    Connecting,
    Checking,
    Running,
    Paused,
    NeedsConnection {
        message: String,
    },
    NeedsAttention {
        reason: AttentionReason,
    },
    RetryWaiting {
        attempt: u8,
        #[ts(as = "f64")]
        next_retry_at_ms: u64,
    },
    Completed {
        result: CompletionResult,
    },
    Failed {
        error: String,
    },
    Cancelled {
        cleanup_error: Option<String>,
    },
}

impl TransferJobState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled { .. }
        )
    }

    /// Whether this state owns scheduler capacity until its transfer actor
    /// reports a new state.
    pub fn holds_lease(&self) -> bool {
        matches!(self, Self::Connecting | Self::Checking | Self::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueueSettings {
    pub global_limit: usize,
    pub per_host_limit: usize,
}

impl Default for QueueSettings {
    fn default() -> Self {
        Self {
            global_limit: 3,
            per_host_limit: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TransferJob {
    pub id: Uuid,
    pub protocol: TransferProtocol,
    pub direction: TransferDirection,
    pub origin: TransferOrigin,
    pub endpoint: TransferEndpoint,
    pub local_path: String,
    pub remote_path: String,
    pub file_name: String,
    pub batch_id: Option<Uuid>,
    pub priority: TransferPriority,
    #[ts(as = "f64")]
    pub queue_order: u64,
    pub host_key: String,
    pub destination_key: String,
    pub state: TransferJobState,
    pub source_fingerprint: Option<SourceFingerprint>,
    #[ts(as = "f64")]
    pub durable_checkpoint: u64,
    #[ts(as = "f64")]
    pub bytes_transferred: u64,
    #[ts(as = "f64")]
    pub total_bytes: u64,
    #[ts(as = "f64")]
    pub speed_bytes_per_second: u64,
    #[ts(as = "Option<f64>")]
    pub eta_seconds: Option<u64>,
    pub retry_attempt: u8,
    pub max_attempts: u8,
    pub conflict_policy: ConflictPolicy,
    pub artifacts: Option<ManagedArtifacts>,
    pub commit_phase: CommitPhase,
    #[ts(as = "f64")]
    pub created_at_ms: u64,
    #[ts(as = "f64")]
    pub updated_at_ms: u64,
    #[ts(as = "Option<f64>")]
    pub started_at_ms: Option<u64>,
    #[ts(as = "Option<f64>")]
    pub finished_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferQueueDocument {
    pub version: u32,
    pub revision: u64,
    pub queue_paused: bool,
    pub settings: QueueSettings,
    pub jobs: Vec<TransferJob>,
    #[serde(default)]
    pub recovery_error: Option<String>,
}

impl Default for TransferQueueDocument {
    fn default() -> Self {
        Self {
            version: TRANSFER_STORE_VERSION,
            revision: 0,
            queue_paused: true,
            settings: QueueSettings::default(),
            jobs: Vec::new(),
            recovery_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewTransferJob {
    pub id: Uuid,
    pub protocol: TransferProtocol,
    pub direction: TransferDirection,
    pub origin: TransferOrigin,
    pub endpoint: TransferEndpoint,
    pub local_path: String,
    pub remote_path: String,
    pub file_name: String,
    pub batch_id: Option<Uuid>,
    pub conflict_policy: ConflictPolicy,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TransferQueueSummary {
    pub queued: usize,
    pub running: usize,
    pub paused: usize,
    pub attention: usize,
    pub failed: usize,
    pub active: usize,
    pub history: usize,
    pub queue_paused: bool,
}

impl TransferQueueSummary {
    pub fn from_jobs(jobs: &[TransferJob], queue_paused: bool) -> Self {
        let mut summary = Self {
            queued: 0,
            running: 0,
            paused: 0,
            attention: 0,
            failed: 0,
            active: 0,
            history: 0,
            queue_paused,
        };

        for job in jobs {
            if job.state.is_terminal() {
                summary.history += 1;
            } else {
                summary.active += 1;
            }
            match job.state {
                TransferJobState::Queued | TransferJobState::RetryWaiting { .. } => {
                    summary.queued += 1;
                }
                TransferJobState::Connecting
                | TransferJobState::Checking
                | TransferJobState::Running => summary.running += 1,
                TransferJobState::Paused => summary.paused += 1,
                TransferJobState::NeedsConnection { .. }
                | TransferJobState::NeedsAttention { .. } => summary.attention += 1,
                TransferJobState::Failed { .. } => summary.failed += 1,
                TransferJobState::Completed { .. } | TransferJobState::Cancelled { .. } => {}
            }
        }

        summary
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TransferQueueSnapshot {
    #[ts(as = "f64")]
    pub revision: u64,
    pub queue_paused: bool,
    pub settings: QueueSettings,
    pub jobs: Vec<TransferJob>,
    pub summary: TransferQueueSummary,
    pub recovery_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn sample_job(state: TransferJobState) -> TransferJob {
        TransferJob {
            id: Uuid::nil(),
            protocol: TransferProtocol::Sftp,
            direction: TransferDirection::Upload,
            origin: TransferOrigin::FilesPanel,
            endpoint: TransferEndpoint::Configured {
                server_entry_id: "server-1".into(),
                label: "Production".into(),
            },
            local_path: "/tmp/report.csv".into(),
            remote_path: "/srv/report.csv".into(),
            file_name: "report.csv".into(),
            batch_id: None,
            priority: TransferPriority::Normal,
            queue_order: 4,
            host_key: "configured:server-1".into(),
            destination_key: "configured:server-1:/srv/report.csv".into(),
            state,
            source_fingerprint: None,
            durable_checkpoint: 0,
            bytes_transferred: 0,
            total_bytes: 0,
            speed_bytes_per_second: 0,
            eta_seconds: None,
            retry_attempt: 0,
            max_attempts: 3,
            conflict_policy: ConflictPolicy::Ask,
            artifacts: None,
            commit_phase: CommitPhase::None,
            created_at_ms: 10,
            updated_at_ms: 10,
            started_at_ms: None,
            finished_at_ms: None,
        }
    }

    #[test]
    fn document_defaults_are_versioned_and_suspended() {
        let doc = TransferQueueDocument::default();
        assert_eq!(doc.version, TRANSFER_STORE_VERSION);
        assert!(doc.queue_paused);
        assert_eq!(
            doc.settings,
            QueueSettings {
                global_limit: 3,
                per_host_limit: 2,
            }
        );
        assert!(doc.jobs.is_empty());
    }

    #[test]
    fn serialized_job_uses_tagged_camel_case_without_secrets() {
        let json = serde_json::to_string(&sample_job(TransferJobState::NeedsConnection {
            message: "Reconnect explicitly".into(),
        }))
        .unwrap();

        assert!(json.contains("\"state\":{\"kind\":\"needsConnection\""));
        for forbidden in [
            "password",
            "passphrase",
            "privateKey",
            "sshHandle",
            "sessionHandle",
        ] {
            assert!(
                !json.contains(forbidden),
                "serialized job leaked field {forbidden}"
            );
        }
    }

    #[test]
    fn summary_counts_terminal_jobs_as_history_not_queue_work() {
        let jobs = vec![
            sample_job(TransferJobState::Queued),
            sample_job(TransferJobState::Completed {
                result: CompletionResult::Transferred,
            }),
            sample_job(TransferJobState::Cancelled {
                cleanup_error: None,
            }),
        ];

        let summary = TransferQueueSummary::from_jobs(&jobs, false);

        assert_eq!(summary.queued, 1);
        assert_eq!(summary.active, 1);
        assert_eq!(summary.history, 2);
    }

    #[test]
    fn download_destination_key_normalizes_windows_drive_and_backslash_paths() {
        let canonical = build_destination_key(
            "configured:server",
            &TransferDirection::Download,
            r"C:\build\app.tar",
            "/unused",
        );
        let equivalent = build_destination_key(
            "configured:server",
            &TransferDirection::Download,
            "C:/build/output/../app.tar",
            "/unused",
        );

        assert_eq!(canonical, r"configured:server:C:\build\app.tar");
        assert_eq!(equivalent, canonical);
    }

    #[test]
    fn download_destination_key_normalizes_windows_unc_paths() {
        let canonical = build_destination_key(
            "configured:server",
            &TransferDirection::Download,
            r"\\server\share\releases\app.tar",
            "/unused",
        );
        let equivalent = build_destination_key(
            "configured:server",
            &TransferDirection::Download,
            r"\\server\share\releases\staging\..\app.tar",
            "/unused",
        );

        assert_eq!(
            canonical,
            r"configured:server:\\server\share\releases\app.tar"
        );
        assert_eq!(equivalent, canonical);
    }

    #[test]
    fn upload_destination_key_keeps_windows_like_sftp_names_posix_literal() {
        let key = build_destination_key(
            "configured:server",
            &TransferDirection::Upload,
            "/unused",
            r"C:\build\output\..\app.tar",
        );

        assert_eq!(key, r"configured:server:C:\build\output\..\app.tar");
    }
}
