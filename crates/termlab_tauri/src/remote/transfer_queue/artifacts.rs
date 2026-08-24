use std::{fmt, path::Path};

use uuid::Uuid;

use super::model::{
    CommitPhase, ManagedArtifacts, is_windows_drive_path, normalize_destination_path,
    normalize_local_destination_path, uses_windows_path_semantics,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactPathError {
    InvalidRemoteDestination,
    InvalidLocalDestination,
}

impl fmt::Display for ArtifactPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidRemoteDestination => "remote destination must name a final file",
            Self::InvalidLocalDestination => "local destination must name a UTF-8 final file",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ArtifactPathError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactInventory {
    pub final_exists: bool,
    pub partial_exists: bool,
    pub backup_exists: bool,
}

impl fmt::Display for ArtifactInventory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "final={}, partial={}, backup={}",
            self.final_exists, self.partial_exists, self.backup_exists
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    ResumeCopy,
    MoveFinalToBackup,
    PromotePartial,
    RestoreBackup,
    DeleteBackupAndComplete,
    Complete,
    NeedsAttention { message: String },
}

pub fn recovery_action(
    phase: CommitPhase,
    backup_expected: Option<bool>,
    inventory: ArtifactInventory,
) -> RecoveryAction {
    use CommitPhase::{BackupMoved, CleanupPending, Complete, PartialPromoted, Prepared};

    let layout = (
        inventory.final_exists,
        inventory.partial_exists,
        inventory.backup_exists,
    );
    match (phase, backup_expected, layout) {
        (CommitPhase::None, _, (_, _, false)) => RecoveryAction::ResumeCopy,

        // A fresh destination has no authoritative backup. The final-only
        // layout means promotion succeeded before its next durable phase write.
        (Prepared | BackupMoved, Some(false), (false, true, false)) => {
            RecoveryAction::PromotePartial
        }
        (BackupMoved | PartialPromoted | CleanupPending, Some(false), (true, false, false)) => {
            RecoveryAction::Complete
        }

        // An overwrite promised an authoritative backup. Never reinterpret a
        // missing backup as a fresh destination: that would make an ambiguous
        // final eligible for destructive cleanup.
        (Prepared, Some(true), (true, true, false)) => RecoveryAction::MoveFinalToBackup,
        (Prepared | BackupMoved, Some(true), (false, true, true)) => RecoveryAction::PromotePartial,
        (
            Prepared | BackupMoved | PartialPromoted | CleanupPending,
            Some(true),
            (false, false, true),
        ) => RecoveryAction::RestoreBackup,
        (BackupMoved | PartialPromoted | CleanupPending, Some(true), (true, false, true)) => {
            RecoveryAction::DeleteBackupAndComplete
        }

        // Legacy v1 jobs have no destination provenance. A managed backup is
        // positive evidence that an old final existed, so those layouts remain
        // recoverable. Backup-free commit layouts are intentionally ambiguous.
        (Prepared | BackupMoved, None, (false, true, true)) => RecoveryAction::PromotePartial,
        (Prepared | BackupMoved | PartialPromoted | CleanupPending, None, (false, false, true)) => {
            RecoveryAction::RestoreBackup
        }
        (BackupMoved | PartialPromoted | CleanupPending, None, (true, false, true)) => {
            RecoveryAction::DeleteBackupAndComplete
        }

        (Complete, _, (true, false, false)) => RecoveryAction::Complete,
        _ => RecoveryAction::NeedsAttention {
            message: format!(
                "managed artifact inventory does not match persisted {phase:?} commit phase \
                 with backup_expected={backup_expected:?} (final={}, partial={}, backup={}); \
                 preserve all artifacts and resolve explicitly",
                inventory.final_exists, inventory.partial_exists, inventory.backup_exists
            ),
        },
    }
}

impl ManagedArtifacts {
    pub fn for_destination(id: Uuid, destination: &str) -> Result<Self, ArtifactPathError> {
        if destination.is_empty()
            || destination.ends_with('/')
            || destination.contains('\0')
            || matches!(destination.rsplit('/').next(), Some("." | ".."))
        {
            return Err(ArtifactPathError::InvalidRemoteDestination);
        }

        let destination = normalize_destination_path(destination);
        let (mut parent, file_name) = destination
            .rsplit_once('/')
            .map_or(("", destination.as_str()), |(parent, file_name)| {
                (parent, file_name)
            });
        if parent.is_empty() && destination.starts_with('/') {
            parent = "/";
        }
        if file_name.is_empty() || matches!(file_name, "." | "..") {
            return Err(ArtifactPathError::InvalidRemoteDestination);
        }

        let partial_name = managed_name(file_name, "part", id);
        let backup_name = managed_name(file_name, "backup", id);
        Ok(Self {
            partial_path: remote_sibling(parent, &partial_name),
            backup_path: remote_sibling(parent, &backup_name),
        })
    }

    pub fn for_local_destination(id: Uuid, destination: &Path) -> Result<Self, ArtifactPathError> {
        let destination_text = destination
            .as_os_str()
            .to_str()
            .ok_or(ArtifactPathError::InvalidLocalDestination)?;
        let windows_path = uses_windows_path_semantics(destination_text);
        if destination_text.is_empty()
            || has_trailing_local_separator(destination_text, windows_path)
        {
            return Err(ArtifactPathError::InvalidLocalDestination);
        }

        if matches!(
            local_final_component(destination_text, windows_path),
            "." | ".."
        ) {
            return Err(ArtifactPathError::InvalidLocalDestination);
        }

        let destination = normalize_local_destination_path(destination_text);
        let file_name = local_final_component(&destination, windows_path);
        if windows_path && !windows_path_has_final_component(&destination) {
            return Err(ArtifactPathError::InvalidLocalDestination);
        }
        let file_name = (!file_name.is_empty())
            .then_some(file_name)
            .ok_or(ArtifactPathError::InvalidLocalDestination)?;
        let prefix = &destination[..destination.len() - file_name.len()];

        Ok(Self {
            partial_path: format!("{prefix}{}", managed_name(file_name, "part", id)),
            backup_path: format!("{prefix}{}", managed_name(file_name, "backup", id)),
        })
    }
}

fn managed_name(file_name: &str, role: &str, id: Uuid) -> String {
    format!(".{file_name}.termlab-{role}-{id}")
}

fn remote_sibling(parent: &str, file_name: &str) -> String {
    match parent {
        "" => file_name.to_owned(),
        "/" => format!("/{file_name}"),
        _ => format!("{parent}/{file_name}"),
    }
}

fn has_trailing_local_separator(path: &str, windows_path: bool) -> bool {
    path.ends_with('/') || (windows_path && path.ends_with('\\'))
}

fn local_final_component(path: &str, windows_path: bool) -> &str {
    let component = if windows_path {
        path.rsplit(['/', '\\']).next().unwrap_or(path)
    } else {
        path.rsplit('/').next().unwrap_or(path)
    };
    if windows_path && component == path && is_windows_drive_path(path) {
        &path[2..]
    } else {
        component
    }
}

fn windows_path_has_final_component(path: &str) -> bool {
    if let Some(rest) = path.strip_prefix(r"\\") {
        rest.split('\\').filter(|part| !part.is_empty()).count() >= 3
    } else if is_windows_drive_path(path) {
        !path[2..].trim_start_matches('\\').is_empty()
    } else {
        !local_final_component(path, true).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use uuid::Uuid;

    use super::super::model::{CommitPhase, ManagedArtifacts};
    use super::{ArtifactInventory, RecoveryAction, recovery_action};

    const JOB_ID: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

    #[test]
    fn artifacts_are_unique_hidden_siblings_of_the_destination() {
        let id = Uuid::parse_str(JOB_ID).unwrap();

        let artifacts = ManagedArtifacts::for_destination(id, "/srv/releases/app.tar").unwrap();

        assert_eq!(
            artifacts.partial_path,
            "/srv/releases/.app.tar.termlab-part-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        );
        assert_eq!(
            artifacts.backup_path,
            "/srv/releases/.app.tar.termlab-backup-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        );
        assert_ne!(artifacts.partial_path, artifacts.backup_path);
    }

    #[test]
    fn root_destination_keeps_artifacts_at_the_root() {
        let id = Uuid::parse_str(JOB_ID).unwrap();

        let artifacts = ManagedArtifacts::for_destination(id, "/file").unwrap();

        assert_eq!(
            artifacts.partial_path,
            "/.file.termlab-part-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        );
        assert_eq!(
            artifacts.backup_path,
            "/.file.termlab-backup-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        );
    }

    #[test]
    fn remote_artifacts_use_the_canonical_destination_sibling() {
        let id = Uuid::parse_str(JOB_ID).unwrap();

        let artifacts = ManagedArtifacts::for_destination(id, "/srv/releases/../app.tar").unwrap();

        assert_eq!(
            artifacts.partial_path,
            "/srv/.app.tar.termlab-part-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        );
        assert_eq!(
            artifacts.backup_path,
            "/srv/.app.tar.termlab-backup-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        );
    }

    #[test]
    fn remote_destination_requires_a_final_file_name() {
        let id = Uuid::parse_str(JOB_ID).unwrap();

        for invalid in [
            "",
            "/",
            "/srv/releases/",
            "/srv/releases/.",
            "/srv/releases/..",
            ".",
            "..",
            "app.tar\0suffix",
        ] {
            assert!(
                ManagedArtifacts::for_destination(id, invalid).is_err(),
                "accepted invalid remote destination {invalid:?}"
            );
        }
    }

    #[test]
    fn local_artifacts_use_platform_paths_beside_the_destination() {
        let id = Uuid::parse_str(JOB_ID).unwrap();
        let destination = Path::new("build").join("output").join("app.tar");

        let artifacts = ManagedArtifacts::for_local_destination(id, &destination).unwrap();

        let expected_partial = Path::new("build")
            .join("output")
            .join(format!(".app.tar.termlab-part-{JOB_ID}"));
        let expected_backup = Path::new("build")
            .join("output")
            .join(format!(".app.tar.termlab-backup-{JOB_ID}"));
        assert_eq!(artifacts.partial_path, expected_partial.to_str().unwrap());
        assert_eq!(artifacts.backup_path, expected_backup.to_str().unwrap());
    }

    #[test]
    fn equivalent_local_destinations_produce_the_same_artifacts() {
        let id = Uuid::parse_str(JOB_ID).unwrap();
        let canonical = Path::new("build").join("app.tar");
        let equivalent = Path::new("build").join("output").join("..").join("app.tar");

        let canonical_artifacts = ManagedArtifacts::for_local_destination(id, &canonical).unwrap();
        let equivalent_artifacts =
            ManagedArtifacts::for_local_destination(id, &equivalent).unwrap();

        assert_eq!(equivalent_artifacts, canonical_artifacts);
    }

    #[test]
    fn windows_drive_destinations_share_artifacts_across_separator_forms() {
        let id = Uuid::parse_str(JOB_ID).unwrap();
        let canonical = Path::new(r"C:\build\app.tar");
        let equivalent = Path::new("C:/build/output/../app.tar");

        let canonical_artifacts = ManagedArtifacts::for_local_destination(id, canonical).unwrap();
        let equivalent_artifacts = ManagedArtifacts::for_local_destination(id, equivalent).unwrap();

        assert_eq!(equivalent_artifacts, canonical_artifacts);
        assert_eq!(
            canonical_artifacts.partial_path,
            r"C:\build\.app.tar.termlab-part-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        );
        assert_eq!(
            canonical_artifacts.backup_path,
            r"C:\build\.app.tar.termlab-backup-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        );
    }

    #[test]
    fn windows_unc_destinations_normalize_to_job_owned_siblings() {
        let id = Uuid::parse_str(JOB_ID).unwrap();
        let canonical = Path::new(r"\\server\share\releases\app.tar");
        let equivalent = Path::new(r"\\server\share\releases\staging\..\app.tar");

        let canonical_artifacts = ManagedArtifacts::for_local_destination(id, canonical).unwrap();
        let equivalent_artifacts = ManagedArtifacts::for_local_destination(id, equivalent).unwrap();

        assert_eq!(equivalent_artifacts, canonical_artifacts);
        assert_eq!(
            canonical_artifacts.partial_path,
            r"\\server\share\releases\.app.tar.termlab-part-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        );
        assert_eq!(
            canonical_artifacts.backup_path,
            r"\\server\share\releases\.app.tar.termlab-backup-aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        );
    }

    #[test]
    fn local_destination_requires_a_final_file_name() {
        let id = Uuid::parse_str(JOB_ID).unwrap();

        for invalid in [Path::new(""), Path::new("/"), Path::new("build/output/")] {
            assert!(
                ManagedArtifacts::for_local_destination(id, invalid).is_err(),
                "accepted invalid local destination {invalid:?}"
            );
        }
    }

    #[test]
    fn artifact_names_are_owned_by_one_job_id() {
        let first = Uuid::parse_str(JOB_ID).unwrap();
        let second = Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap();

        let first_artifacts = ManagedArtifacts::for_destination(first, "/srv/app.tar").unwrap();
        let second_artifacts = ManagedArtifacts::for_destination(second, "/srv/app.tar").unwrap();

        assert_ne!(first_artifacts.partial_path, second_artifacts.partial_path);
        assert_ne!(first_artifacts.backup_path, second_artifacts.backup_path);
    }

    #[derive(Debug, Clone, Copy)]
    enum ExpectedAction {
        ResumeCopy,
        MoveFinalToBackup,
        PromotePartial,
        RestoreBackup,
        DeleteBackupAndComplete,
        Complete,
        NeedsAttention,
    }

    #[test]
    fn recovery_action_is_exhaustive_for_every_phase_and_inventory() {
        use CommitPhase::{
            BackupMoved, CleanupPending, Complete, None as NoCommitPhase, PartialPromoted, Prepared,
        };
        use ExpectedAction::{
            Complete as MarkComplete, DeleteBackupAndComplete, NeedsAttention, PromotePartial,
            RestoreBackup, ResumeCopy,
        };

        let cases = [
            // None: no backup operation has begun, so copying can start or resume.
            (NoCommitPhase, (false, false, false), ResumeCopy),
            (NoCommitPhase, (false, false, true), NeedsAttention),
            (NoCommitPhase, (false, true, false), ResumeCopy),
            (NoCommitPhase, (false, true, true), NeedsAttention),
            (NoCommitPhase, (true, false, false), ResumeCopy),
            (NoCommitPhase, (true, false, true), NeedsAttention),
            (NoCommitPhase, (true, true, false), ResumeCopy),
            (NoCommitPhase, (true, true, true), NeedsAttention),
            // Prepared: the complete partial can move forward, including after a
            // final-to-backup rename that crashed before its phase barrier.
            (Prepared, (false, false, false), NeedsAttention),
            (Prepared, (false, false, true), RestoreBackup),
            (Prepared, (false, true, false), NeedsAttention),
            (Prepared, (false, true, true), PromotePartial),
            (Prepared, (true, false, false), NeedsAttention),
            (Prepared, (true, false, true), NeedsAttention),
            (Prepared, (true, true, false), NeedsAttention),
            (Prepared, (true, true, true), NeedsAttention),
            // BackupMoved: either promote the partial, restore the only
            // authoritative backup, or finish a promotion that already landed.
            (BackupMoved, (false, false, false), NeedsAttention),
            (BackupMoved, (false, false, true), RestoreBackup),
            (BackupMoved, (false, true, false), NeedsAttention),
            (BackupMoved, (false, true, true), PromotePartial),
            (BackupMoved, (true, false, false), NeedsAttention),
            (BackupMoved, (true, false, true), DeleteBackupAndComplete),
            (BackupMoved, (true, true, false), NeedsAttention),
            (BackupMoved, (true, true, true), NeedsAttention),
            // PartialPromoted: a final without a partial is the promoted file;
            // retain or restore the backup if the final is unexpectedly absent.
            (PartialPromoted, (false, false, false), NeedsAttention),
            (PartialPromoted, (false, false, true), RestoreBackup),
            (PartialPromoted, (false, true, false), NeedsAttention),
            (PartialPromoted, (false, true, true), NeedsAttention),
            (PartialPromoted, (true, false, false), NeedsAttention),
            (
                PartialPromoted,
                (true, false, true),
                DeleteBackupAndComplete,
            ),
            (PartialPromoted, (true, true, false), NeedsAttention),
            (PartialPromoted, (true, true, true), NeedsAttention),
            // CleanupPending has the same authoritative layouts but records
            // that removing the managed backup still needs to be retried.
            (CleanupPending, (false, false, false), NeedsAttention),
            (CleanupPending, (false, false, true), RestoreBackup),
            (CleanupPending, (false, true, false), NeedsAttention),
            (CleanupPending, (false, true, true), NeedsAttention),
            (CleanupPending, (true, false, false), NeedsAttention),
            (CleanupPending, (true, false, true), DeleteBackupAndComplete),
            (CleanupPending, (true, true, false), NeedsAttention),
            (CleanupPending, (true, true, true), NeedsAttention),
            // Complete is valid only when the final is the sole artifact.
            (Complete, (false, false, false), NeedsAttention),
            (Complete, (false, false, true), NeedsAttention),
            (Complete, (false, true, false), NeedsAttention),
            (Complete, (false, true, true), NeedsAttention),
            (Complete, (true, false, false), MarkComplete),
            (Complete, (true, false, true), NeedsAttention),
            (Complete, (true, true, false), NeedsAttention),
            (Complete, (true, true, true), NeedsAttention),
        ];

        assert_eq!(cases.len(), 6 * 8);
        for (phase, (final_exists, partial_exists, backup_exists), expected) in cases {
            let inventory = ArtifactInventory {
                final_exists,
                partial_exists,
                backup_exists,
            };
            let action = recovery_action(phase, None, inventory);

            assert_expected_action(phase, inventory, action, expected);
        }
    }

    #[test]
    fn recovery_distinguishes_fresh_destination_commit_crash_windows() {
        use CommitPhase::{BackupMoved, CleanupPending, Complete, PartialPromoted, Prepared};
        use ExpectedAction::{Complete as MarkComplete, NeedsAttention, PromotePartial};

        let cases = [
            // Prepared proves only the pre-promotion layout. A promoted final
            // becomes recoverable only after the BackupMoved barrier.
            (Prepared, (false, true, false), PromotePartial),
            (Prepared, (true, false, false), NeedsAttention),
            (Prepared, (true, false, true), NeedsAttention),
            (BackupMoved, (false, true, false), PromotePartial),
            (BackupMoved, (true, false, false), MarkComplete),
            (PartialPromoted, (true, false, false), MarkComplete),
            (CleanupPending, (true, false, false), MarkComplete),
            (Complete, (true, false, false), MarkComplete),
            // A backup is impossible for a destination observed as fresh.
            (BackupMoved, (false, true, true), NeedsAttention),
            (PartialPromoted, (true, false, true), NeedsAttention),
        ];

        for (phase, (final_exists, partial_exists, backup_exists), expected) in cases {
            let inventory = ArtifactInventory {
                final_exists,
                partial_exists,
                backup_exists,
            };
            let action = recovery_action(phase, Some(false), inventory);
            assert_expected_action(phase, inventory, action, expected);
        }
    }

    #[test]
    fn recovery_never_treats_a_missing_overwrite_backup_as_fresh() {
        use CommitPhase::{BackupMoved, CleanupPending, PartialPromoted, Prepared};
        use ExpectedAction::{
            DeleteBackupAndComplete, MoveFinalToBackup, NeedsAttention, PromotePartial,
        };

        let cases = [
            (Prepared, (true, true, false), MoveFinalToBackup),
            (Prepared, (false, true, true), PromotePartial),
            (BackupMoved, (false, true, true), PromotePartial),
            (
                PartialPromoted,
                (true, false, true),
                DeleteBackupAndComplete,
            ),
            (CleanupPending, (true, false, true), DeleteBackupAndComplete),
            // These layouts would be safe for a fresh destination, but an
            // overwrite promised an authoritative backup and must stop.
            (Prepared, (false, true, false), NeedsAttention),
            (Prepared, (true, false, false), NeedsAttention),
            (Prepared, (true, false, true), NeedsAttention),
            (BackupMoved, (false, true, false), NeedsAttention),
            (BackupMoved, (true, false, false), NeedsAttention),
            (PartialPromoted, (true, false, false), NeedsAttention),
            (CleanupPending, (true, false, false), NeedsAttention),
        ];

        for (phase, (final_exists, partial_exists, backup_exists), expected) in cases {
            let inventory = ArtifactInventory {
                final_exists,
                partial_exists,
                backup_exists,
            };
            let action = recovery_action(phase, Some(true), inventory);
            assert_expected_action(phase, inventory, action, expected);
        }
    }

    fn assert_expected_action(
        phase: CommitPhase,
        inventory: ArtifactInventory,
        actual: RecoveryAction,
        expected: ExpectedAction,
    ) {
        let matches = matches!(
            (&actual, expected),
            (RecoveryAction::ResumeCopy, ExpectedAction::ResumeCopy)
                | (
                    RecoveryAction::MoveFinalToBackup,
                    ExpectedAction::MoveFinalToBackup
                )
                | (
                    RecoveryAction::PromotePartial,
                    ExpectedAction::PromotePartial
                )
                | (RecoveryAction::RestoreBackup, ExpectedAction::RestoreBackup)
                | (
                    RecoveryAction::DeleteBackupAndComplete,
                    ExpectedAction::DeleteBackupAndComplete
                )
                | (RecoveryAction::Complete, ExpectedAction::Complete)
                | (
                    RecoveryAction::NeedsAttention { .. },
                    ExpectedAction::NeedsAttention
                )
        );
        assert!(
            matches,
            "phase {phase:?}, inventory {inventory:?}: expected {expected:?}, got {actual:?}"
        );

        if let RecoveryAction::NeedsAttention { message } = actual {
            assert!(!message.trim().is_empty());
        }
    }
}
