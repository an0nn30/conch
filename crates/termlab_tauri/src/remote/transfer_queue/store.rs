use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::remote::transfer_queue::model::{
    TRANSFER_HISTORY_LIMIT, TRANSFER_STORE_VERSION, TransferJobState, TransferQueueDocument,
};

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Json(serde_json::Error),
    Migration(MigrationError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationError {
    UnsupportedVersion { version: u32 },
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "transfer queue store I/O error: {error}"),
            Self::Json(error) => write!(formatter, "transfer queue store JSON error: {error}"),
            Self::Migration(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion { version } => {
                write!(
                    formatter,
                    "unsupported transfer queue schema version {version}"
                )
            }
        }
    }
}

impl std::error::Error for MigrationError {}

pub enum LoadOutcome {
    Loaded(TransferQueueDocument),
    Quarantined {
        document: TransferQueueDocument,
        path: PathBuf,
    },
}

impl LoadOutcome {
    pub fn into_document(self) -> TransferQueueDocument {
        match self {
            Self::Loaded(document) | Self::Quarantined { document, .. } => document,
        }
    }
}

pub struct TransferStore {
    path: PathBuf,
}

impl TransferStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn save(&self, document: &TransferQueueDocument) -> Result<(), StoreError> {
        validate_document(document)?;
        let contents = serde_json::to_vec_pretty(document)?;
        write_atomically(&self.path, &contents)?;
        Ok(())
    }

    pub fn load(&self) -> Result<LoadOutcome, StoreError> {
        let contents = match fs::read(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(LoadOutcome::Loaded(TransferQueueDocument::default()));
            }
            Err(error) => return Err(error.into()),
        };

        let mut document = match serde_json::from_slice(&contents) {
            Ok(document) => document,
            Err(_) => return self.quarantine(),
        };
        validate_document(&document)?;
        recover_for_startup(&mut document);
        Ok(LoadOutcome::Loaded(document))
    }

    fn quarantine(&self) -> Result<LoadOutcome, StoreError> {
        let quarantine_path = quarantine_path(&self.path)?;
        fs::rename(&self.path, &quarantine_path)?;
        let mut document = TransferQueueDocument::default();
        let quarantine_name = quarantine_path
            .file_name()
            .unwrap_or(quarantine_path.as_os_str())
            .to_string_lossy();
        document.recovery_error = Some(format!(
            "Transfer queue file was quarantined as {quarantine_name}"
        ));
        Ok(LoadOutcome::Quarantined {
            document,
            path: quarantine_path,
        })
    }
}

fn validate_document(document: &TransferQueueDocument) -> Result<(), StoreError> {
    if document.version != TRANSFER_STORE_VERSION {
        return Err(StoreError::Migration(MigrationError::UnsupportedVersion {
            version: document.version,
        }));
    }
    Ok(())
}

fn quarantine_path(path: &Path) -> io::Result<PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "transfer queue store path has no parent directory",
        )
    })?;
    let stem = path.file_stem().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "transfer queue store path has no file stem",
        )
    })?;
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let suffix = if extension.is_empty() {
        String::new()
    } else {
        format!(".{extension}")
    };

    for collision in 0..1000 {
        let separator = if collision == 0 {
            String::new()
        } else {
            format!("-{collision}")
        };
        let candidate = parent.join(format!(
            "{}.corrupt-{timestamp}{separator}{suffix}",
            stem.to_string_lossy()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a transfer queue quarantine path",
    ))
}

/// Normalize persisted queue state before exposing it to a new application
/// process. This function performs no I/O and never starts work.
pub fn recover_for_startup(document: &mut TransferQueueDocument) {
    let mut changed = false;

    if !document.queue_paused {
        document.queue_paused = true;
        changed = true;
    }

    for job in &mut document.jobs {
        if matches!(
            job.state,
            TransferJobState::Connecting | TransferJobState::Checking | TransferJobState::Running
        ) {
            job.state = TransferJobState::Paused;
            changed = true;
        }
        if job.speed_bytes_per_second != 0 {
            job.speed_bytes_per_second = 0;
            changed = true;
        }
        if job.eta_seconds.is_some() {
            job.eta_seconds = None;
            changed = true;
        }
    }

    let terminal_count = document
        .jobs
        .iter()
        .filter(|job| job.state.is_terminal())
        .count();
    if terminal_count > TRANSFER_HISTORY_LIMIT {
        let remove_count = terminal_count - TRANSFER_HISTORY_LIMIT;
        let mut terminal_indices: Vec<_> = document
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, job)| job.state.is_terminal())
            .map(|(index, job)| {
                (
                    index,
                    (
                        job.finished_at_ms.unwrap_or(job.created_at_ms),
                        job.created_at_ms,
                        index,
                    ),
                )
            })
            .collect();
        terminal_indices.sort_unstable_by_key(|(_, sort_key)| *sort_key);

        let mut remove = vec![false; document.jobs.len()];
        for (index, _) in terminal_indices.into_iter().take(remove_count) {
            remove[index] = true;
        }
        document.jobs = std::mem::take(&mut document.jobs)
            .into_iter()
            .enumerate()
            .filter_map(|(index, job)| (!remove[index]).then_some(job))
            .collect();
        changed = true;
    }

    if changed {
        document.revision += 1;
    }
}

fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "transfer queue store path has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;

    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "transfer queue store path has no file name",
        )
    })?;
    let temporary_path = parent.join(format!(
        ".{}.{}.tmp",
        file_name.to_string_lossy(),
        uuid::Uuid::new_v4()
    ));

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let write_result = (|| -> io::Result<()> {
        let mut file = options.open(&temporary_path)?;
        file.write_all(contents)?;
        file.flush()?;
        file.sync_all()?;
        replace_atomically(&temporary_path, path)?;
        sync_parent_directory(parent)?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result
}

#[cfg(not(windows))]
fn replace_atomically(temporary_path: &Path, path: &Path) -> io::Result<()> {
    fs::rename(temporary_path, path)
}

#[cfg(windows)]
fn replace_atomically(temporary_path: &Path, path: &Path) -> io::Result<()> {
    use std::{iter::once, os::windows::ffi::OsStrExt};
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let temporary_wide: Vec<u16> = temporary_path
        .as_os_str()
        .encode_wide()
        .chain(once(0))
        .collect();
    let target_wide: Vec<u16> = path.as_os_str().encode_wide().chain(once(0)).collect();

    // Both paths are siblings, so Windows can atomically replace the target
    // without the delete-then-rename durability gap.
    if unsafe {
        MoveFileExW(
            temporary_wide.as_ptr(),
            target_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } != 0
    {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::transfer_queue::model::*;
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

    fn document_with(job: TransferJob) -> TransferQueueDocument {
        TransferQueueDocument {
            jobs: vec![job],
            ..TransferQueueDocument::default()
        }
    }

    fn temp_store() -> (tempfile::TempDir, TransferStore, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        (dir, TransferStore::new(path.clone()), path)
    }

    #[test]
    fn save_roundtrips_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        let store = TransferStore::new(path.clone());

        store
            .save(&document_with(sample_job(TransferJobState::Paused)))
            .unwrap();

        let loaded = store.load().unwrap().into_document();
        assert_eq!(loaded.jobs.len(), 1);
        assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn second_save_replaces_the_previous_document() {
        let (_dir, store, _path) = temp_store();
        store.save(&TransferQueueDocument::default()).unwrap();

        store
            .save(&document_with(sample_job(TransferJobState::Paused)))
            .unwrap();

        let loaded = store.load().unwrap().into_document();
        assert_eq!(loaded.jobs.len(), 1);
        assert_eq!(loaded.jobs[0].state, TransferJobState::Paused);
    }

    #[cfg(unix)]
    #[test]
    fn new_store_is_user_only() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, store, path) = temp_store();
        store.save(&TransferQueueDocument::default()).unwrap();

        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn malformed_file_is_quarantined_without_changing_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        let original = b"{ definitely not valid JSON";
        std::fs::write(&path, original).unwrap();

        let outcome = TransferStore::new(path.clone()).load().unwrap();
        let LoadOutcome::Quarantined {
            document,
            path: quarantine_path,
        } = outcome
        else {
            panic!("malformed queue must be quarantined");
        };

        assert!(!path.exists());
        assert_eq!(std::fs::read(&quarantine_path).unwrap(), original);
        let quarantine_name = quarantine_path.file_name().unwrap().to_string_lossy();
        assert!(quarantine_name.starts_with("transfers.corrupt-"));
        assert!(quarantine_name.ends_with(".json"));
        assert!(document.queue_paused);
        assert!(document.jobs.is_empty());
        assert!(
            document
                .recovery_error
                .as_deref()
                .unwrap()
                .contains(quarantine_name.as_ref())
        );
    }

    #[test]
    fn startup_recovery_pauses_inflight_jobs_and_preserves_other_active_states() {
        let states = vec![
            TransferJobState::Connecting,
            TransferJobState::Checking,
            TransferJobState::Running,
            TransferJobState::Queued,
            TransferJobState::Paused,
            TransferJobState::NeedsConnection {
                message: "Reconnect explicitly".into(),
            },
            TransferJobState::NeedsAttention {
                reason: AttentionReason::MissingPartial,
            },
            TransferJobState::RetryWaiting {
                attempt: 2,
                next_retry_at_ms: 42,
            },
        ];
        let mut document = TransferQueueDocument {
            revision: 12,
            queue_paused: false,
            jobs: states.into_iter().map(sample_job).collect(),
            ..TransferQueueDocument::default()
        };
        document.jobs[2].speed_bytes_per_second = 1_024;
        document.jobs[2].eta_seconds = Some(15);

        recover_for_startup(&mut document);

        assert!(document.queue_paused);
        assert_eq!(document.revision, 13);
        assert_eq!(document.jobs[0].state, TransferJobState::Paused);
        assert_eq!(document.jobs[1].state, TransferJobState::Paused);
        assert_eq!(document.jobs[2].state, TransferJobState::Paused);
        assert_eq!(document.jobs[3].state, TransferJobState::Queued);
        assert_eq!(document.jobs[4].state, TransferJobState::Paused);
        assert!(matches!(
            document.jobs[5].state,
            TransferJobState::NeedsConnection { .. }
        ));
        assert!(matches!(
            document.jobs[6].state,
            TransferJobState::NeedsAttention { .. }
        ));
        assert!(matches!(
            document.jobs[7].state,
            TransferJobState::RetryWaiting { .. }
        ));
        assert_eq!(document.jobs[2].speed_bytes_per_second, 0);
        assert_eq!(document.jobs[2].eta_seconds, None);
    }

    #[test]
    fn startup_recovery_compacts_only_the_oldest_terminal_history() {
        let mut jobs: Vec<_> = (0..502)
            .map(|index| {
                let mut job = sample_job(TransferJobState::Completed {
                    result: CompletionResult::Transferred,
                });
                job.id = Uuid::from_u128(index as u128 + 1);
                job.created_at_ms = index;
                job.finished_at_ms = Some(index);
                job
            })
            .collect();
        jobs.push(sample_job(TransferJobState::Queued));

        let mut document = TransferQueueDocument {
            jobs,
            ..TransferQueueDocument::default()
        };
        recover_for_startup(&mut document);

        assert_eq!(document.jobs.len(), 501);
        assert!(!document.jobs.iter().any(|job| job.id == Uuid::from_u128(1)));
        assert!(!document.jobs.iter().any(|job| job.id == Uuid::from_u128(2)));
        assert!(document.jobs.iter().any(|job| job.id == Uuid::from_u128(3)));
        assert!(
            document
                .jobs
                .iter()
                .any(|job| matches!(job.state, TransferJobState::Queued))
        );
    }

    #[test]
    fn version_one_document_without_recovery_error_loads_with_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transfers.json");
        let mut value = serde_json::to_value(TransferQueueDocument::default()).unwrap();
        value.as_object_mut().unwrap().remove("recoveryError");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();

        let document = TransferStore::new(path).load().unwrap().into_document();

        assert_eq!(document.recovery_error, None);
    }

    #[test]
    fn save_rejects_unsupported_schema_with_a_typed_migration_error() {
        let (_dir, store, _path) = temp_store();
        let mut document = TransferQueueDocument::default();
        document.version = 0;

        let error = store.save(&document).unwrap_err();

        assert!(matches!(
            error,
            StoreError::Migration(MigrationError::UnsupportedVersion { version: 0 })
        ));
    }

    #[test]
    fn load_rejects_unsupported_schema_without_moving_the_original_file() {
        for version in [0, TRANSFER_STORE_VERSION + 1] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("transfers.json");
            let mut document = TransferQueueDocument::default();
            document.version = version;
            let original = serde_json::to_vec(&document).unwrap();
            std::fs::write(&path, &original).unwrap();

            let error = match TransferStore::new(path.clone()).load() {
                Err(error) => error,
                Ok(_) => panic!("unsupported schema version {version} must be rejected"),
            };

            assert!(matches!(
                error,
                StoreError::Migration(MigrationError::UnsupportedVersion {
                    version: rejected_version
                }) if rejected_version == version
            ));
            assert_eq!(std::fs::read(&path).unwrap(), original);
            assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".corrupt-")
            }));
        }
    }
}
