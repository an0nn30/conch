//! Application-wide ownership for canonical local editor documents.
//!
//! This is a synchronous state machine for the future single LSP manager
//! actor. It is deliberately independent of project context, trust, and
//! language-server enablement.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use unicode_casefold::{Locale, UnicodeCaseFold, Variant};
use unicode_normalization::UnicodeNormalization;

use super::types::{DocumentId, ReservationId, ReserveResult};

const RESERVATION_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DocumentIdentifier {
    Local(PathBuf),
    Untitled,
    Remote(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnershipError {
    NonLocalIdentifier,
    CanonicalizationFailed(io::ErrorKind),
    InvalidReservation,
    DocumentNotOwned,
    OwnerMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaseSensitivity {
    Sensitive,
    Insensitive,
}

pub(crate) trait PathIdentityPolicy {
    fn key_for(&self, path: &CanonicalLocalPath) -> Result<PathBuf, OwnershipError>;
}

#[derive(Debug, Default)]
pub(crate) struct SystemPathIdentityPolicy;

impl PathIdentityPolicy for SystemPathIdentityPolicy {
    fn key_for(&self, path: &CanonicalLocalPath) -> Result<PathBuf, OwnershipError> {
        let sensitivity = system_case_sensitivity(&path.volume_probe)?;
        identity_key(&path.io_path, sensitivity)
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct FixedPathIdentityPolicy {
    sensitivity: CaseSensitivity,
}

#[cfg(test)]
impl FixedPathIdentityPolicy {
    pub(crate) fn new(sensitivity: CaseSensitivity) -> Self {
        Self { sensitivity }
    }
}

#[cfg(test)]
impl PathIdentityPolicy for FixedPathIdentityPolicy {
    fn key_for(&self, path: &CanonicalLocalPath) -> Result<PathBuf, OwnershipError> {
        identity_key(&path.io_path, self.sensitivity)
    }
}

#[derive(Debug)]
enum UriLease {
    Reserved {
        token: ReservationId,
        canonical_path: PathBuf,
        window_label: String,
        expires_at: Instant,
    },
    Owned {
        document_id: DocumentId,
        canonical_path: PathBuf,
        window_label: String,
        pane_id: String,
    },
}

pub(crate) struct OwnershipRegistry<P = SystemPathIdentityPolicy> {
    leases: HashMap<PathBuf, UriLease>,
    path_identity: P,
}

impl Default for OwnershipRegistry<SystemPathIdentityPolicy> {
    fn default() -> Self {
        Self::new()
    }
}

impl OwnershipRegistry<SystemPathIdentityPolicy> {
    pub(crate) fn new() -> Self {
        Self::with_policy(SystemPathIdentityPolicy)
    }
}

impl<P: PathIdentityPolicy> OwnershipRegistry<P> {
    pub(crate) fn with_policy(path_identity: P) -> Self {
        Self {
            leases: HashMap::new(),
            path_identity,
        }
    }

    pub(crate) fn reserve(
        &mut self,
        identifier: DocumentIdentifier,
        window_label: &str,
    ) -> Result<ReserveResult, OwnershipError> {
        self.reserve_at(identifier, window_label, Instant::now())
    }

    pub(crate) fn reserve_at(
        &mut self,
        identifier: DocumentIdentifier,
        window_label: &str,
        now: Instant,
    ) -> Result<ReserveResult, OwnershipError> {
        let canonical = match identifier {
            DocumentIdentifier::Local(path) => canonical_local_path(&path)?,
            DocumentIdentifier::Untitled | DocumentIdentifier::Remote(_) => {
                return Err(OwnershipError::NonLocalIdentifier);
            }
        };
        let key = self.path_identity.key_for(&canonical)?;
        self.cleanup_expired_at(now);

        match self.leases.get(&key) {
            Some(UriLease::Reserved { window_label, .. }) => Ok(ReserveResult::FocusPending {
                window_label: window_label.clone(),
            }),
            Some(UriLease::Owned {
                document_id,
                window_label,
                pane_id,
                ..
            }) => Ok(ReserveResult::FocusOwner {
                document_id: *document_id,
                window_label: window_label.clone(),
                pane_id: pane_id.clone(),
            }),
            None => {
                let reservation_id = ReservationId::new();
                let canonical_path = canonical_path_string(&canonical.io_path)?;
                self.leases.insert(
                    key,
                    UriLease::Reserved {
                        token: reservation_id,
                        canonical_path: canonical.io_path,
                        window_label: window_label.to_owned(),
                        expires_at: now + RESERVATION_TTL,
                    },
                );
                Ok(ReserveResult::Reserved {
                    reservation_id,
                    canonical_path,
                })
            }
        }
    }

    pub(crate) fn commit(
        &mut self,
        token: ReservationId,
        pane_id: &str,
    ) -> Result<DocumentId, OwnershipError> {
        self.commit_at(token, pane_id, Instant::now())
    }

    pub(crate) fn commit_at(
        &mut self,
        token: ReservationId,
        pane_id: &str,
        now: Instant,
    ) -> Result<DocumentId, OwnershipError> {
        let (key, canonical_path, window_label) = self
            .leases
            .iter()
            .find_map(|(key, lease)| match lease {
                UriLease::Reserved {
                    token: candidate,
                    canonical_path,
                    window_label,
                    expires_at,
                } if *candidate == token && now < *expires_at => {
                    Some((key.clone(), canonical_path.clone(), window_label.clone()))
                }
                _ => None,
            })
            .ok_or(OwnershipError::InvalidReservation)?;

        let document_id = DocumentId::new();
        self.leases.insert(
            key,
            UriLease::Owned {
                document_id,
                canonical_path,
                window_label,
                pane_id: pane_id.to_owned(),
            },
        );
        Ok(document_id)
    }

    /// Releases only the still-uncommitted reservation identified by `token`.
    /// A stale token, a repeated release, or a token already committed is a no-op.
    pub(crate) fn release(&mut self, token: ReservationId) -> bool {
        let key = self.leases.iter().find_map(|(key, lease)| match lease {
            UriLease::Reserved {
                token: candidate, ..
            } if *candidate == token => Some(key.clone()),
            _ => None,
        });
        key.is_some_and(|key| self.leases.remove(&key).is_some())
    }

    /// Releases a committed local URI after its matching document closes.
    pub(crate) fn close(&mut self, document_id: DocumentId) -> bool {
        let key = self.owned_key(document_id).cloned();
        key.is_some_and(|key| self.leases.remove(&key).is_some())
    }

    /// Models local-to-remote Save As without accepting or storing a remote URI.
    pub(crate) fn release_for_remote_transfer(&mut self, document_id: DocumentId) -> bool {
        self.close(document_id)
    }

    pub(crate) fn transfer(
        &mut self,
        source_document: DocumentId,
        target_token: ReservationId,
    ) -> Result<(), OwnershipError> {
        self.transfer_at(source_document, target_token, Instant::now())
    }

    /// Atomically moves one document from its owned local path to a live target
    /// reservation. All validation completes before either lease is mutated.
    pub(crate) fn transfer_at(
        &mut self,
        source_document: DocumentId,
        target_token: ReservationId,
        now: Instant,
    ) -> Result<(), OwnershipError> {
        let (source_key, source_window, source_pane) = self
            .leases
            .iter()
            .find_map(|(key, lease)| match lease {
                UriLease::Owned {
                    document_id,
                    window_label,
                    pane_id,
                    ..
                } if *document_id == source_document => {
                    Some((key.clone(), window_label.clone(), pane_id.clone()))
                }
                _ => None,
            })
            .ok_or(OwnershipError::DocumentNotOwned)?;

        let (target_key, target_path, target_window) = self
            .leases
            .iter()
            .find_map(|(key, lease)| match lease {
                UriLease::Reserved {
                    token,
                    canonical_path,
                    window_label,
                    expires_at,
                } if *token == target_token && now < *expires_at => {
                    Some((key.clone(), canonical_path.clone(), window_label.clone()))
                }
                _ => None,
            })
            .ok_or(OwnershipError::InvalidReservation)?;

        if source_window != target_window {
            return Err(OwnershipError::OwnerMismatch);
        }

        self.leases.remove(&source_key);
        self.leases.insert(
            target_key,
            UriLease::Owned {
                document_id: source_document,
                canonical_path: target_path,
                window_label: source_window,
                pane_id: source_pane,
            },
        );
        Ok(())
    }

    pub(crate) fn cleanup_expired_at(&mut self, now: Instant) {
        self.leases.retain(|_, lease| match lease {
            UriLease::Reserved { expires_at, .. } => now < *expires_at,
            UriLease::Owned { .. } => true,
        });
    }

    pub(crate) fn canonical_path(&self, document_id: DocumentId) -> Option<&Path> {
        self.leases.values().find_map(|lease| match lease {
            UriLease::Owned {
                document_id: candidate,
                canonical_path,
                ..
            } if *candidate == document_id => Some(canonical_path.as_path()),
            _ => None,
        })
    }

    fn owned_key(&self, document_id: DocumentId) -> Option<&PathBuf> {
        self.leases.iter().find_map(|(key, lease)| match lease {
            UriLease::Owned {
                document_id: candidate,
                ..
            } if *candidate == document_id => Some(key),
            _ => None,
        })
    }
}

#[derive(Debug)]
pub(crate) struct CanonicalLocalPath {
    io_path: PathBuf,
    volume_probe: PathBuf,
}

fn canonical_path_string(path: &Path) -> Result<String, OwnershipError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or(OwnershipError::CanonicalizationFailed(
            io::ErrorKind::InvalidData,
        ))
}

fn identity_key(
    canonical_path: &Path,
    sensitivity: CaseSensitivity,
) -> Result<PathBuf, OwnershipError> {
    let path = canonical_path
        .to_str()
        .ok_or(OwnershipError::CanonicalizationFailed(
            io::ErrorKind::InvalidData,
        ))?;
    // Both case-sensitive and case-insensitive APFS are normalization-insensitive.
    let decomposed: String = path.nfd().collect();

    match sensitivity {
        CaseSensitivity::Sensitive => Ok(PathBuf::from(decomposed.nfc().collect::<String>())),
        CaseSensitivity::Insensitive => {
            // Canonical caseless key order is deliberate: decompose equivalent
            // spellings, apply the Unicode data table's full non-Turkic fold
            // (including multi-scalar mappings), then recompose a stable key.
            // APFS matching is frozen to Unicode 9.0, so unicode-casefold 0.2.0
            // is intentionally pinned; newer folds would create false aliases.
            let folded: String = decomposed
                .case_fold_with(Variant::Full, Locale::NonTurkic)
                .collect();
            Ok(PathBuf::from(folded.nfc().collect::<String>()))
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn system_case_sensitivity(_volume_probe: &Path) -> Result<CaseSensitivity, OwnershipError> {
    Ok(CaseSensitivity::Sensitive)
}

#[cfg(target_os = "macos")]
fn system_case_sensitivity(volume_probe: &Path) -> Result<CaseSensitivity, OwnershipError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    #[repr(C)]
    struct VolumeCapabilitiesBuffer {
        length: u32,
        capabilities: libc::vol_capabilities_attr_t,
    }

    let path = CString::new(volume_probe.as_os_str().as_bytes())
        .map_err(|_| OwnershipError::CanonicalizationFailed(io::ErrorKind::InvalidInput))?;
    let mut attributes = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_CAPABILITIES,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut buffer = VolumeCapabilitiesBuffer {
        length: 0,
        capabilities: libc::vol_capabilities_attr_t {
            capabilities: [0; 4],
            valid: [0; 4],
        },
    };

    // SAFETY: `path` is NUL-terminated, both pointers refer to initialized
    // C-compatible structs for the duration of this read-only syscall, and
    // the supplied buffer size exactly matches `buffer`.
    let result = unsafe {
        libc::getattrlist(
            path.as_ptr(),
            (&mut attributes as *mut libc::attrlist).cast(),
            (&mut buffer as *mut VolumeCapabilitiesBuffer).cast(),
            std::mem::size_of::<VolumeCapabilitiesBuffer>(),
            0,
        )
    };
    if result != 0 {
        return Err(OwnershipError::CanonicalizationFailed(
            io::Error::last_os_error().kind(),
        ));
    }

    let index = libc::VOL_CAPABILITIES_FORMAT;
    let mask = libc::VOL_CAP_FMT_CASE_SENSITIVE;
    if buffer.capabilities.valid[index] & mask == 0 {
        return Err(OwnershipError::CanonicalizationFailed(
            io::ErrorKind::Unsupported,
        ));
    }
    if buffer.capabilities.capabilities[index] & mask == 0 {
        Ok(CaseSensitivity::Insensitive)
    } else {
        Ok(CaseSensitivity::Sensitive)
    }
}

#[derive(Debug)]
enum MissingComponent {
    Normal(OsString),
    Parent,
}

fn deepest_existing_canonical_ancestor(path: &Path) -> Result<PathBuf, OwnershipError> {
    let mut candidate = Some(path);
    while let Some(path) = candidate {
        match fs::canonicalize(path) {
            Ok(canonical) => return Ok(canonical),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                candidate = path.parent();
            }
            Err(error) => return Err(OwnershipError::CanonicalizationFailed(error.kind())),
        }
    }
    Err(OwnershipError::CanonicalizationFailed(
        io::ErrorKind::NotFound,
    ))
}

/// Canonicalizes existing paths through the filesystem. For a prospective Save
/// As target, the deepest existing ancestor is canonicalized first (resolving
/// symlinks), then missing normal/parent components are normalized onto it.
fn canonical_local_path(path: &Path) -> Result<CanonicalLocalPath, OwnershipError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| OwnershipError::CanonicalizationFailed(error.kind()))?
            .join(path)
    };

    match fs::canonicalize(&absolute) {
        Ok(path) => {
            return Ok(CanonicalLocalPath {
                volume_probe: path.clone(),
                io_path: path,
            });
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => {
            return Err(OwnershipError::CanonicalizationFailed(error.kind()));
        }
        Err(_) => {}
    }

    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    let canonical_ancestor = loop {
        match fs::canonicalize(ancestor) {
            Ok(path) => break path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(OwnershipError::CanonicalizationFailed(error.kind())),
        }

        let component =
            ancestor
                .components()
                .next_back()
                .ok_or(OwnershipError::CanonicalizationFailed(
                    io::ErrorKind::NotFound,
                ))?;
        match component {
            Component::Normal(value) => missing.push(MissingComponent::Normal(value.to_owned())),
            Component::ParentDir => missing.push(MissingComponent::Parent),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err(OwnershipError::CanonicalizationFailed(
                    io::ErrorKind::NotFound,
                ));
            }
        }
        ancestor = ancestor
            .parent()
            .ok_or(OwnershipError::CanonicalizationFailed(
                io::ErrorKind::NotFound,
            ))?;
    };

    let mut canonical = canonical_ancestor.clone();
    for component in missing.into_iter().rev() {
        match component {
            MissingComponent::Normal(value) => canonical.push(value),
            MissingComponent::Parent => {
                canonical.pop();
            }
        }
    }
    let volume_probe = deepest_existing_canonical_ancestor(&canonical)?;
    Ok(CanonicalLocalPath {
        io_path: canonical,
        volume_probe,
    })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::{
        CanonicalLocalPath, CaseSensitivity, DocumentIdentifier, FixedPathIdentityPolicy,
        OwnershipError, OwnershipRegistry, PathIdentityPolicy,
    };
    use crate::lsp::types::{DocumentId, ReserveResult};

    fn local(path: impl AsRef<Path>) -> DocumentIdentifier {
        DocumentIdentifier::Local(path.as_ref().to_path_buf())
    }

    fn reserved(result: ReserveResult) -> crate::lsp::types::ReservationId {
        match result {
            ReserveResult::Reserved { reservation_id, .. } => reservation_id,
            other => panic!("expected a new reservation, got {other:?}"),
        }
    }

    fn reservation(result: ReserveResult) -> (crate::lsp::types::ReservationId, PathBuf) {
        match result {
            ReserveResult::Reserved {
                reservation_id,
                canonical_path,
            } => (reservation_id, PathBuf::from(canonical_path)),
            other => panic!("expected a new reservation, got {other:?}"),
        }
    }

    fn fixture_file(temp: &TempDir, name: &str) -> PathBuf {
        let path = temp.path().join(name);
        fs::write(&path, "contents never need to be read to reserve").unwrap();
        path
    }

    #[test]
    fn pending_reservation_focuses_reserving_window_without_issuing_a_second_token() {
        let temp = TempDir::new().unwrap();
        let file = fixture_file(&temp, "pending.ts");
        let now = Instant::now();
        let mut registry = OwnershipRegistry::new();

        let first = reserved(registry.reserve_at(local(&file), "main", now).unwrap());
        assert_eq!(
            registry
                .reserve_at(local(&file), "main", now + Duration::from_secs(1))
                .unwrap(),
            ReserveResult::FocusPending {
                window_label: "main".into(),
            }
        );
        assert_eq!(
            registry
                .reserve_at(local(&file), "popup", now + Duration::from_secs(1))
                .unwrap(),
            ReserveResult::FocusPending {
                window_label: "main".into(),
            }
        );

        assert!(registry.release(first));
    }

    #[test]
    fn committed_owner_is_focused_from_same_or_different_window() {
        let temp = TempDir::new().unwrap();
        let file = fixture_file(&temp, "owned.ts");
        let now = Instant::now();
        let mut registry = OwnershipRegistry::new();
        let reservation = reserved(registry.reserve_at(local(&file), "main", now).unwrap());
        let document_id = registry
            .commit_at(reservation, "pane-1", now + Duration::from_secs(1))
            .unwrap();

        let expected = ReserveResult::FocusOwner {
            document_id,
            window_label: "main".into(),
            pane_id: "pane-1".into(),
        };
        assert_eq!(
            registry
                .reserve_at(local(&file), "main", now + Duration::from_secs(2))
                .unwrap(),
            expected
        );
        assert_eq!(
            registry
                .reserve_at(local(&file), "popup", now + Duration::from_secs(2))
                .unwrap(),
            expected
        );
    }

    #[test]
    fn reservation_is_live_before_thirty_seconds_and_expires_at_the_exact_boundary() {
        let temp = TempDir::new().unwrap();
        let file = fixture_file(&temp, "expiry.rs");
        let start = Instant::now();
        let mut registry = OwnershipRegistry::new();
        let first = reserved(registry.reserve_at(local(&file), "main", start).unwrap());

        assert_eq!(
            registry
                .reserve_at(
                    local(&file),
                    "popup",
                    start + Duration::from_secs(30) - Duration::from_nanos(1),
                )
                .unwrap(),
            ReserveResult::FocusPending {
                window_label: "main".into(),
            }
        );

        let second = reserved(
            registry
                .reserve_at(local(&file), "popup", start + Duration::from_secs(30))
                .unwrap(),
        );
        assert_ne!(first, second);
    }

    #[test]
    fn stale_token_cannot_commit_release_or_expire_a_later_owner() {
        let temp = TempDir::new().unwrap();
        let file = fixture_file(&temp, "aba.rs");
        let start = Instant::now();
        let mut registry = OwnershipRegistry::new();
        let stale = reserved(registry.reserve_at(local(&file), "main", start).unwrap());
        let later = reserved(
            registry
                .reserve_at(local(&file), "popup", start + Duration::from_secs(30))
                .unwrap(),
        );
        let document_id = registry
            .commit_at(later, "pane-new", start + Duration::from_secs(31))
            .unwrap();

        assert_eq!(
            registry.commit_at(stale, "pane-stale", start + Duration::from_secs(31)),
            Err(OwnershipError::InvalidReservation)
        );
        assert!(!registry.release(stale));
        registry.cleanup_expired_at(start + Duration::from_secs(300));
        assert_eq!(
            registry
                .reserve_at(local(&file), "third", start + Duration::from_secs(300))
                .unwrap(),
            ReserveResult::FocusOwner {
                document_id,
                window_label: "popup".into(),
                pane_id: "pane-new".into(),
            }
        );
    }

    #[test]
    fn release_is_idempotent_and_never_releases_committed_ownership() {
        let temp = TempDir::new().unwrap();
        let abandoned = fixture_file(&temp, "abandoned.ts");
        let committed = fixture_file(&temp, "committed.ts");
        let now = Instant::now();
        let mut registry = OwnershipRegistry::new();

        let abandoned_token =
            reserved(registry.reserve_at(local(&abandoned), "main", now).unwrap());
        assert!(registry.release(abandoned_token));
        assert!(!registry.release(abandoned_token));
        reserved(
            registry
                .reserve_at(local(&abandoned), "popup", now)
                .unwrap(),
        );

        let committed_token =
            reserved(registry.reserve_at(local(&committed), "main", now).unwrap());
        let document_id = registry.commit_at(committed_token, "pane", now).unwrap();
        assert!(!registry.release(committed_token));
        assert_eq!(
            registry
                .reserve_at(local(&committed), "popup", now)
                .unwrap(),
            ReserveResult::FocusOwner {
                document_id,
                window_label: "main".into(),
                pane_id: "pane".into(),
            }
        );
    }

    #[test]
    fn close_is_idempotent_and_only_matching_document_releases_ownership() {
        let temp = TempDir::new().unwrap();
        let file = fixture_file(&temp, "close.ts");
        let now = Instant::now();
        let mut registry = OwnershipRegistry::new();
        let token = reserved(registry.reserve_at(local(&file), "main", now).unwrap());
        let document_id = registry.commit_at(token, "pane", now).unwrap();

        assert!(!registry.close(DocumentId::new()));
        assert!(matches!(
            registry.reserve_at(local(&file), "popup", now).unwrap(),
            ReserveResult::FocusOwner { .. }
        ));
        assert!(registry.close(document_id));
        assert!(!registry.close(document_id));
        assert!(matches!(
            registry.reserve_at(local(&file), "popup", now).unwrap(),
            ReserveResult::Reserved { .. }
        ));
    }

    #[test]
    fn transfer_atomically_moves_the_same_document_and_owner_to_reserved_target() {
        let temp = TempDir::new().unwrap();
        let source = fixture_file(&temp, "before.ts");
        let target = temp.path().join("after.ts");
        let now = Instant::now();
        let mut registry = OwnershipRegistry::new();
        let source_token = reserved(registry.reserve_at(local(&source), "main", now).unwrap());
        let document_id = registry.commit_at(source_token, "pane", now).unwrap();
        let target_token = reserved(registry.reserve_at(local(&target), "main", now).unwrap());

        registry
            .transfer_at(document_id, target_token, now + Duration::from_secs(1))
            .unwrap();

        assert!(matches!(
            registry.reserve_at(local(&source), "popup", now).unwrap(),
            ReserveResult::Reserved { .. }
        ));
        assert_eq!(
            registry.reserve_at(local(&target), "popup", now).unwrap(),
            ReserveResult::FocusOwner {
                document_id,
                window_label: "main".into(),
                pane_id: "pane".into(),
            }
        );
    }

    #[test]
    fn target_collision_is_reported_before_transfer_and_preserves_source() {
        let temp = TempDir::new().unwrap();
        let source = fixture_file(&temp, "source.ts");
        let target = fixture_file(&temp, "target.ts");
        let now = Instant::now();
        let mut registry = OwnershipRegistry::new();

        let source_token = reserved(registry.reserve_at(local(&source), "main", now).unwrap());
        let source_document = registry
            .commit_at(source_token, "source-pane", now)
            .unwrap();
        let target_token = reserved(registry.reserve_at(local(&target), "popup", now).unwrap());
        let target_document = registry
            .commit_at(target_token, "target-pane", now)
            .unwrap();

        assert_eq!(
            registry.reserve_at(local(&target), "main", now).unwrap(),
            ReserveResult::FocusOwner {
                document_id: target_document,
                window_label: "popup".into(),
                pane_id: "target-pane".into(),
            }
        );
        assert_eq!(
            registry.reserve_at(local(&source), "popup", now).unwrap(),
            ReserveResult::FocusOwner {
                document_id: source_document,
                window_label: "main".into(),
                pane_id: "source-pane".into(),
            }
        );
    }

    #[test]
    fn failed_transfer_validates_source_and_target_without_mutation() {
        let temp = TempDir::new().unwrap();
        let source = fixture_file(&temp, "source.rs");
        let target = temp.path().join("target.rs");
        let now = Instant::now();
        let mut registry = OwnershipRegistry::new();
        let source_token = reserved(registry.reserve_at(local(&source), "main", now).unwrap());
        let source_document = registry.commit_at(source_token, "pane", now).unwrap();
        let target_token = reserved(registry.reserve_at(local(&target), "main", now).unwrap());

        assert_eq!(
            registry.transfer_at(DocumentId::new(), target_token, now),
            Err(OwnershipError::DocumentNotOwned)
        );
        assert_eq!(
            registry.reserve_at(local(&source), "popup", now).unwrap(),
            ReserveResult::FocusOwner {
                document_id: source_document,
                window_label: "main".into(),
                pane_id: "pane".into(),
            }
        );
        assert_eq!(
            registry.reserve_at(local(&target), "popup", now).unwrap(),
            ReserveResult::FocusPending {
                window_label: "main".into(),
            }
        );
    }

    #[test]
    fn expired_or_cross_window_target_token_cannot_move_source() {
        let temp = TempDir::new().unwrap();
        let source = fixture_file(&temp, "source.rs");
        let expired_target = temp.path().join("expired.rs");
        let other_window_target = temp.path().join("other.rs");
        let start = Instant::now();
        let mut registry = OwnershipRegistry::new();
        let source_token = reserved(registry.reserve_at(local(&source), "main", start).unwrap());
        let source_document = registry.commit_at(source_token, "pane", start).unwrap();
        let expired = reserved(
            registry
                .reserve_at(local(&expired_target), "main", start)
                .unwrap(),
        );
        let cross_window = reserved(
            registry
                .reserve_at(local(&other_window_target), "popup", start)
                .unwrap(),
        );

        assert_eq!(
            registry.transfer_at(source_document, expired, start + Duration::from_secs(30)),
            Err(OwnershipError::InvalidReservation)
        );
        assert_eq!(
            registry.transfer_at(source_document, cross_window, start),
            Err(OwnershipError::OwnerMismatch)
        );
        assert!(matches!(
            registry.reserve_at(local(&source), "popup", start).unwrap(),
            ReserveResult::FocusOwner { .. }
        ));
        assert_eq!(
            registry
                .reserve_at(local(&other_window_target), "third", start)
                .unwrap(),
            ReserveResult::FocusPending {
                window_label: "popup".into(),
            }
        );
    }

    #[test]
    fn local_to_remote_release_is_explicit_and_never_registers_remote_identifier() {
        let temp = TempDir::new().unwrap();
        let source = fixture_file(&temp, "local.ts");
        let now = Instant::now();
        let mut registry = OwnershipRegistry::new();
        let token = reserved(registry.reserve_at(local(&source), "main", now).unwrap());
        let document_id = registry.commit_at(token, "pane", now).unwrap();

        assert!(registry.release_for_remote_transfer(document_id));
        assert!(!registry.release_for_remote_transfer(document_id));
        assert!(matches!(
            registry.reserve_at(local(&source), "popup", now).unwrap(),
            ReserveResult::Reserved { .. }
        ));
        assert_eq!(
            registry.reserve_at(
                DocumentIdentifier::Remote("sftp://host/project/local.ts".into()),
                "main",
                now,
            ),
            Err(OwnershipError::NonLocalIdentifier)
        );
    }

    #[test]
    fn untitled_and_remote_identifiers_cannot_be_reserved() {
        let now = Instant::now();
        let mut registry = OwnershipRegistry::new();

        assert_eq!(
            registry.reserve_at(DocumentIdentifier::Untitled, "main", now),
            Err(OwnershipError::NonLocalIdentifier)
        );
        assert_eq!(
            registry.reserve_at(
                DocumentIdentifier::Remote("ssh://host/tmp/file.rs".into()),
                "main",
                now,
            ),
            Err(OwnershipError::NonLocalIdentifier)
        );
    }

    #[cfg(unix)]
    #[test]
    fn existing_symlink_aliases_resolve_to_the_same_owner() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let real = fixture_file(&temp, "real.ts");
        let alias = temp.path().join("alias.ts");
        symlink(&real, &alias).unwrap();
        let now = Instant::now();
        let mut registry = OwnershipRegistry::new();
        let token = reserved(registry.reserve_at(local(&real), "main", now).unwrap());
        let document_id = registry.commit_at(token, "pane", now).unwrap();

        assert_eq!(
            registry.reserve_at(local(&alias), "popup", now).unwrap(),
            ReserveResult::FocusOwner {
                document_id,
                window_label: "main".into(),
                pane_id: "pane".into(),
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn leaf_symlink_reservation_uses_referent_for_atomic_editor_io_and_committed_identity() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let referent = fixture_file(&temp, "referent.ts");
        let alias = temp.path().join("alias.ts");
        symlink(&referent, &alias).unwrap();
        let now = Instant::now();
        let mut registry = OwnershipRegistry::new();

        let (token, canonical_path) =
            reservation(registry.reserve_at(local(&alias), "main", now).unwrap());
        assert_eq!(canonical_path, fs::canonicalize(&referent).unwrap());
        let document_id = registry.commit_at(token, "pane", now).unwrap();
        assert_eq!(
            registry.canonical_path(document_id),
            Some(canonical_path.as_path())
        );

        crate::editor_fs::write_text_file(canonical_path.to_str().unwrap(), "updated").unwrap();
        assert!(
            fs::symlink_metadata(&alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&referent).unwrap(), "updated");

        let expected = ReserveResult::FocusOwner {
            document_id,
            window_label: "main".into(),
            pane_id: "pane".into(),
        };
        assert_eq!(
            registry.reserve_at(local(&alias), "popup", now).unwrap(),
            expected
        );
        assert_eq!(
            registry.reserve_at(local(&referent), "popup", now).unwrap(),
            expected
        );
    }

    #[test]
    fn injected_case_sensitive_policy_keeps_prospective_case_variants_distinct() {
        let temp = TempDir::new().unwrap();
        let now = Instant::now();
        let mut registry = OwnershipRegistry::with_policy(FixedPathIdentityPolicy::new(
            CaseSensitivity::Sensitive,
        ));

        reserved(
            registry
                .reserve_at(local(temp.path().join("New.ts")), "main", now)
                .unwrap(),
        );
        assert!(matches!(
            registry
                .reserve_at(local(temp.path().join("new.ts")), "popup", now)
                .unwrap(),
            ReserveResult::Reserved { .. }
        ));
        reserved(
            registry
                .reserve_at(local(temp.path().join("sigma-\u{3c3}.ts")), "main", now)
                .unwrap(),
        );
        assert!(matches!(
            registry
                .reserve_at(local(temp.path().join("sigma-\u{3c2}.ts")), "popup", now)
                .unwrap(),
            ReserveResult::Reserved { .. }
        ));
        reserved(
            registry
                .reserve_at(local(temp.path().join("stra\u{df}e.ts")), "main", now)
                .unwrap(),
        );
        assert!(matches!(
            registry
                .reserve_at(local(temp.path().join("strasse.ts")), "popup", now)
                .unwrap(),
            ReserveResult::Reserved { .. }
        ));
    }

    #[test]
    fn case_sensitive_policy_still_collapses_canonically_equivalent_names() {
        let temp = TempDir::new().unwrap();
        let now = Instant::now();
        let mut registry = OwnershipRegistry::with_policy(FixedPathIdentityPolicy::new(
            CaseSensitivity::Sensitive,
        ));

        reserved(
            registry
                .reserve_at(local(temp.path().join("File.ts")), "upper-owner", now)
                .unwrap(),
        );
        assert!(matches!(
            registry
                .reserve_at(local(temp.path().join("file.ts")), "lower-owner", now)
                .unwrap(),
            ReserveResult::Reserved { .. }
        ));

        reserved(
            registry
                .reserve_at(
                    local(temp.path().join("Caf\u{e9}.ts")),
                    "unicode-owner",
                    now,
                )
                .unwrap(),
        );
        assert_eq!(
            registry
                .reserve_at(local(temp.path().join("Cafe\u{301}.ts")), "popup", now)
                .unwrap(),
            ReserveResult::FocusPending {
                window_label: "unicode-owner".into(),
            }
        );
    }

    #[test]
    fn injected_case_insensitive_policy_collapses_case_and_unicode_equivalent_targets() {
        let temp = TempDir::new().unwrap();
        let now = Instant::now();
        let mut registry = OwnershipRegistry::with_policy(FixedPathIdentityPolicy::new(
            CaseSensitivity::Insensitive,
        ));

        reserved(
            registry
                .reserve_at(local(temp.path().join("New.ts")), "case-owner", now)
                .unwrap(),
        );
        assert_eq!(
            registry
                .reserve_at(local(temp.path().join("new.ts")), "popup", now)
                .unwrap(),
            ReserveResult::FocusPending {
                window_label: "case-owner".into(),
            }
        );

        reserved(
            registry
                .reserve_at(
                    local(temp.path().join("Caf\u{e9}.ts")),
                    "unicode-owner",
                    now,
                )
                .unwrap(),
        );
        assert_eq!(
            registry
                .reserve_at(local(temp.path().join("Cafe\u{301}.ts")), "popup", now,)
                .unwrap(),
            ReserveResult::FocusPending {
                window_label: "unicode-owner".into(),
            }
        );

        reserved(
            registry
                .reserve_at(
                    local(temp.path().join("sigma-\u{3c3}.ts")),
                    "sigma-owner",
                    now,
                )
                .unwrap(),
        );
        assert_eq!(
            registry
                .reserve_at(local(temp.path().join("sigma-\u{3c2}.ts")), "popup", now)
                .unwrap(),
            ReserveResult::FocusPending {
                window_label: "sigma-owner".into(),
            }
        );

        reserved(
            registry
                .reserve_at(
                    local(temp.path().join("stra\u{df}e.ts")),
                    "eszett-owner",
                    now,
                )
                .unwrap(),
        );
        assert_eq!(
            registry
                .reserve_at(local(temp.path().join("strasse.ts")), "popup", now)
                .unwrap(),
            ReserveResult::FocusPending {
                window_label: "eszett-owner".into(),
            }
        );
    }

    #[test]
    fn insensitive_policy_preserves_post_unicode_9_georgian_case_pair_for_apfs() {
        let temp = TempDir::new().unwrap();
        let now = Instant::now();
        let mut registry = OwnershipRegistry::with_policy(FixedPathIdentityPolicy::new(
            CaseSensitivity::Insensitive,
        ));

        reserved(
            registry
                .reserve_at(
                    local(temp.path().join("georgian-\u{1c90}.ts")),
                    "mtavruli-owner",
                    now,
                )
                .unwrap(),
        );
        assert!(matches!(
            registry
                .reserve_at(
                    local(temp.path().join("georgian-\u{10d0}.ts")),
                    "mkhedruli-owner",
                    now,
                )
                .unwrap(),
            ReserveResult::Reserved { .. }
        ));
    }

    #[derive(Debug)]
    struct RecordingPathIdentityPolicy {
        probes: Rc<RefCell<Vec<PathBuf>>>,
    }

    impl PathIdentityPolicy for RecordingPathIdentityPolicy {
        fn key_for(&self, path: &CanonicalLocalPath) -> Result<PathBuf, OwnershipError> {
            self.probes.borrow_mut().push(path.volume_probe.clone());
            Ok(path.io_path.clone())
        }
    }

    #[test]
    fn missing_tail_parent_normalization_probes_the_final_existing_ancestor() {
        let temp = TempDir::new().unwrap();
        let original = temp.path().join("original-volume");
        let final_volume = temp.path().join("final-volume");
        fs::create_dir(&original).unwrap();
        fs::create_dir(&final_volume).unwrap();
        let target = original
            .join("missing")
            .join("..")
            .join("..")
            .join("final-volume")
            .join("new.ts");
        let probes = Rc::new(RefCell::new(Vec::new()));
        let mut registry = OwnershipRegistry::with_policy(RecordingPathIdentityPolicy {
            probes: Rc::clone(&probes),
        });

        let (_, canonical_path) = reservation(
            registry
                .reserve_at(local(&target), "main", Instant::now())
                .unwrap(),
        );

        assert_eq!(
            canonical_path,
            fs::canonicalize(&final_volume).unwrap().join("new.ts")
        );
        assert_eq!(
            probes.borrow().as_slice(),
            [fs::canonicalize(&final_volume).unwrap()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn nonexistent_target_canonicalizes_existing_symlink_parent_and_dot_segments() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let real_parent = temp.path().join("real-parent");
        fs::create_dir(&real_parent).unwrap();
        let alias_parent = temp.path().join("alias-parent");
        symlink(&real_parent, &alias_parent).unwrap();
        let direct = real_parent.join("new.ts");
        let aliased = alias_parent.join("future").join("..").join("new.ts");
        let now = Instant::now();
        let mut registry = OwnershipRegistry::new();

        reserved(registry.reserve_at(local(&aliased), "main", now).unwrap());
        assert_eq!(
            registry.reserve_at(local(&direct), "popup", now).unwrap(),
            ReserveResult::FocusPending {
                window_label: "main".into(),
            }
        );
    }
}
