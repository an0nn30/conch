//! Application-wide ownership, project policy, and language-server routing.
//!
//! Mutable application state lives only in [`LspManager`]. Potentially slow
//! protocol work runs in bounded per-session workers and returns facts through
//! the actor input channel, so the manager never waits for a server response.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_lsp::lsp_types as lsp;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

use super::catalog::{BundledServerCatalog, ResourceRootResolver};
use super::client::ClientEvent;
use super::diagnostics::{DiagnosticKey, DiagnosticStore};
use super::document::{DocumentError, VersionedDocument};
use super::ownership::{DocumentIdentifier, OwnershipError, OwnershipRegistry};
use super::root::{LanguageId, discover_project_roots};
use super::session::{LspSession, ProcessServerLauncher, SessionDocument};
use super::trust::{ProjectTrustStore, RootBinding, TrustDecision};
use super::types::{
    ApplyChangesResponse, CompletionResponse, DefinitionResponse, Diagnostic, DiagnosticSeverity,
    DiagnosticSnapshot, DiagnosticUpdate, DocumentId, EditorPosition, HoverResponse,
    LspCapabilities, LspChangeBatch, LspSessionState, LspStatus, LspUnavailableReason,
    OpenDocumentResponse, ProjectCandidate, ReservationId, ReserveResult, ResyncDocumentResponse,
    SignatureHelpResponse,
};

const COMMAND_CAPACITY: usize = 64;
const ACTOR_INPUT_CAPACITY: usize = 128;
const SESSION_OPERATION_CAPACITY: usize = 64;
const SESSION_OUTBOX_CAPACITY: usize = 128;
const DOCUMENT_PENDING_CAPACITY: usize = 128;
const PUBLIC_EVENT_CAPACITY: usize = 128;
const CHANGE_BATCH_DELAY: Duration = Duration::from_millis(40);
const IDLE_SHUTDOWN_DELAY: Duration = Duration::from_secs(120);
const RESERVATION_TTL: Duration = Duration::from_secs(30);
const CRASH_WINDOW: Duration = Duration::from_secs(5 * 60);
const MAX_CRASHES: usize = 3;
const LOG_CAPACITY: usize = 128;
const MAX_LOG_ENTRY_BYTES: usize = 2 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManagerError {
    ActorStopped,
    Overloaded,
    InvalidReservation,
    UnknownDocument,
    OwnerMismatch,
    InvalidProjectRoot,
    InvalidLanguage(String),
    InvalidChange(String),
    SessionUnavailable,
    Cancelled,
    StaleResponse,
    Unavailable(LspUnavailableReason),
    Infrastructure(String),
}

impl fmt::Display for ManagerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ActorStopped => formatter.write_str("the LSP manager is not running"),
            Self::Overloaded => formatter.write_str("the LSP manager is busy"),
            Self::InvalidReservation => {
                formatter.write_str("invalid or expired document reservation")
            }
            Self::UnknownDocument => formatter.write_str("unknown editor document"),
            Self::OwnerMismatch => formatter.write_str("document owner does not match"),
            Self::InvalidProjectRoot => formatter.write_str("invalid project root"),
            Self::InvalidLanguage(language) => {
                write!(formatter, "unsupported language: {language}")
            }
            Self::InvalidChange(message) => write!(formatter, "invalid document change: {message}"),
            Self::SessionUnavailable => formatter.write_str("language server is unavailable"),
            Self::Cancelled => formatter.write_str("request was superseded"),
            Self::StaleResponse => formatter.write_str("language server response is stale"),
            Self::Unavailable(reason) => {
                write!(formatter, "language server unavailable: {reason:?}")
            }
            Self::Infrastructure(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ManagerError {}

impl From<OwnershipError> for ManagerError {
    fn from(error: OwnershipError) -> Self {
        match error {
            OwnershipError::InvalidReservation => Self::InvalidReservation,
            OwnershipError::DocumentNotOwned => Self::UnknownDocument,
            OwnershipError::OwnerMismatch => Self::OwnerMismatch,
            OwnershipError::NonLocalIdentifier => Self::InvalidProjectRoot,
            OwnershipError::CanonicalizationFailed(kind) => {
                Self::Infrastructure(format!("could not canonicalize local path: {kind:?}"))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SessionKey {
    pub adapter_id: String,
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionStart {
    pub key: SessionKey,
    pub language: LanguageId,
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ProjectContextChoice {
    Root { root: String },
    Disabled,
    DeferForSession,
}

impl ProjectContextChoice {
    pub(crate) fn root(path: PathBuf) -> Self {
        Self::Root {
            root: path.display().to_string(),
        }
    }

    fn root_path(&self) -> Option<PathBuf> {
        match self {
            Self::Root { root } => Some(PathBuf::from(root)),
            Self::Disabled | Self::DeferForSession => None,
        }
    }
}

impl From<PathBuf> for ProjectContextChoice {
    fn from(root: PathBuf) -> Self {
        Self::root(root)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Enablement {
    global: bool,
    adapters: HashSet<String>,
}

impl Enablement {
    pub(crate) fn all() -> Self {
        Self {
            global: true,
            adapters: [
                "typescript",
                "json",
                "python",
                "rust",
                "go",
                "clangd",
                "java",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }

    pub(crate) fn none() -> Self {
        Self {
            global: false,
            adapters: HashSet::new(),
        }
    }

    pub(crate) fn from_config(config: &termlab_core::config::LspConfig) -> Self {
        let languages = &config.languages;
        let enabled = [
            ("typescript", languages.typescript),
            ("json", languages.json),
            ("python", languages.python),
            ("rust", languages.rust),
            ("go", languages.go),
            ("clangd", languages.clangd),
            ("java", languages.java),
        ];
        Self {
            global: config.enabled,
            adapters: enabled
                .into_iter()
                .filter_map(|(adapter, enabled)| enabled.then_some(adapter.to_owned()))
                .collect(),
        }
    }

    pub(crate) fn enables(&self, adapter_id: &str) -> bool {
        self.global && self.adapters.contains(adapter_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct SessionLogEntry {
    pub sequence: u64,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub(crate) struct TrustedProject {
    #[ts(type = "string")]
    pub root: PathBuf,
    pub root_uri: String,
    pub adapter_id: Option<String>,
    pub decision: TrustDecision,
    pub updated_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub(crate) enum ManagerEvent {
    SessionStatus {
        window_label: String,
        status: LspStatus,
    },
    DiagnosticsUpdated(DiagnosticUpdate),
    DocumentOwnerFocused {
        window_label: String,
        document_id: Option<DocumentId>,
        pane_id: Option<String>,
        reservation_failed: bool,
    },
}

#[async_trait]
pub(crate) trait SessionClient: Send + Sync + 'static {
    fn capabilities(&self) -> LspCapabilities;
    async fn did_open(&self, document: SessionDocument) -> Result<(), String>;
    async fn did_change(&self, batch: LspChangeBatch) -> Result<i32, String>;
    async fn did_save(&self, document_id: &str) -> Result<(), String>;
    async fn did_close(&self, document_id: &str) -> Result<(), String>;
    async fn completion(
        &self,
        document_id: &str,
        position: lsp::Position,
        source_version: i32,
    ) -> Result<CompletionResponse, String>;
    async fn hover(
        &self,
        document_id: &str,
        position: lsp::Position,
        source_version: i32,
    ) -> Result<HoverResponse, String>;
    async fn signature_help(
        &self,
        document_id: &str,
        position: lsp::Position,
        source_version: i32,
    ) -> Result<SignatureHelpResponse, String>;
    async fn definition(
        &self,
        document_id: &str,
        position: lsp::Position,
        source_version: i32,
    ) -> Result<DefinitionResponse, String>;
    async fn pull_diagnostics(
        &self,
        document_id: &str,
        previous_result_id: Option<String>,
    ) -> Result<(Option<String>, Vec<Diagnostic>), String>;
    async fn shutdown(&self) -> Result<(), String>;
}

#[async_trait]
pub(crate) trait SessionFactory: Send + Sync + 'static {
    async fn start(
        self: Arc<Self>,
        start: SessionStart,
        events: mpsc::Sender<ClientEvent>,
    ) -> Result<Arc<dyn SessionClient>, ManagerError>;
}

pub(crate) struct RealSessionFactory {
    resource_root: Result<PathBuf, ManagerError>,
}

impl RealSessionFactory {
    pub(crate) fn new(packaged_root: Option<PathBuf>) -> Self {
        let resource_root = ResourceRootResolver::resolve_runtime(None, packaged_root.as_deref())
            .map_err(|error| ManagerError::Unavailable(error.lsp_reason()));
        Self { resource_root }
    }
}

struct RealSessionClient(LspSession);

#[async_trait]
impl SessionClient for RealSessionClient {
    fn capabilities(&self) -> LspCapabilities {
        self.0.capabilities()
    }

    async fn did_open(&self, document: SessionDocument) -> Result<(), String> {
        self.0
            .did_open(document)
            .await
            .map_err(|error| error.to_string())
    }

    async fn did_change(&self, batch: LspChangeBatch) -> Result<i32, String> {
        self.0
            .did_change(batch)
            .await
            .map_err(|error| error.to_string())
    }

    async fn did_save(&self, document_id: &str) -> Result<(), String> {
        self.0
            .did_save(document_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn did_close(&self, document_id: &str) -> Result<(), String> {
        self.0
            .did_close(document_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn completion(
        &self,
        document_id: &str,
        position: lsp::Position,
        _source_version: i32,
    ) -> Result<CompletionResponse, String> {
        self.0
            .completion(document_id, position)
            .await
            .map_err(|error| error.to_string())
    }

    async fn hover(
        &self,
        document_id: &str,
        position: lsp::Position,
        _source_version: i32,
    ) -> Result<HoverResponse, String> {
        self.0
            .hover(document_id, position)
            .await
            .map_err(|error| error.to_string())
    }

    async fn signature_help(
        &self,
        document_id: &str,
        position: lsp::Position,
        _source_version: i32,
    ) -> Result<SignatureHelpResponse, String> {
        self.0
            .signature_help(document_id, position)
            .await
            .map_err(|error| error.to_string())
    }

    async fn definition(
        &self,
        document_id: &str,
        position: lsp::Position,
        _source_version: i32,
    ) -> Result<DefinitionResponse, String> {
        self.0
            .definition(document_id, position)
            .await
            .map_err(|error| error.to_string())
    }

    async fn pull_diagnostics(
        &self,
        document_id: &str,
        previous_result_id: Option<String>,
    ) -> Result<(Option<String>, Vec<Diagnostic>), String> {
        self.0
            .pull_diagnostics(document_id, previous_result_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn shutdown(&self) -> Result<(), String> {
        self.0.shutdown().await.map_err(|error| error.to_string())
    }
}

#[async_trait]
impl SessionFactory for RealSessionFactory {
    async fn start(
        self: Arc<Self>,
        start: SessionStart,
        events: mpsc::Sender<ClientEvent>,
    ) -> Result<Arc<dyn SessionClient>, ManagerError> {
        let resource_root = self.resource_root.clone()?;
        let catalog = BundledServerCatalog::new();
        let descriptor = catalog.descriptor(start.language);
        let command = catalog
            .resolve(start.language, &resource_root)
            .map_err(|error| ManagerError::Unavailable(error.lsp_reason()))?;
        let session = LspSession::start(
            descriptor,
            command,
            start.key.root,
            ProcessServerLauncher,
            events,
        )
        .await
        .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
        Ok(Arc::new(RealSessionClient(session)))
    }
}

#[derive(Clone)]
pub(crate) struct LspManagerHandle {
    commands: mpsc::Sender<ManagerCommand>,
    shutdown: mpsc::Sender<oneshot::Sender<Result<(), ManagerError>>>,
}

impl LspManagerHandle {
    async fn send(&self, command: ManagerCommand) -> Result<(), ManagerError> {
        self.commands
            .send(command)
            .await
            .map_err(|_| ManagerError::ActorStopped)
    }

    pub(crate) async fn reserve_document(
        &self,
        path: PathBuf,
        window_label: String,
    ) -> Result<ReserveResult, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::ReserveDocument {
            path,
            window_label,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn release_document(
        &self,
        reservation_id: ReservationId,
    ) -> Result<(), ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::ReleaseDocument {
            reservation_id,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn transfer_document(
        &self,
        document_id: DocumentId,
        target_reservation_id: ReservationId,
        window_label: String,
        pane_id: String,
    ) -> Result<(), ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::TransferDocument {
            document_id,
            target_reservation_id,
            window_label,
            pane_id,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn open_document(
        &self,
        reservation_id: ReservationId,
        pane_id: String,
        contents: String,
        language_id: String,
    ) -> Result<OpenDocumentResponse, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::OpenDocument {
            reservation_id,
            pane_id,
            contents,
            language_id,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn apply_changes(
        &self,
        document_id: DocumentId,
        batch: LspChangeBatch,
    ) -> Result<ApplyChangesResponse, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::ApplyChanges {
            document_id,
            batch,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn resync_document(
        &self,
        document_id: DocumentId,
        version: i32,
        contents: String,
    ) -> Result<ResyncDocumentResponse, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::ResyncDocument {
            document_id,
            version,
            contents,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn did_save(&self, document_id: DocumentId) -> Result<(), ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::DidSave { document_id, reply })
            .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn close_document(&self, document_id: DocumentId) -> Result<(), ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::CloseDocument { document_id, reply })
            .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn project_candidates(
        &self,
        path: PathBuf,
        language_id: String,
    ) -> Result<Vec<ProjectCandidate>, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::ProjectCandidates {
            path,
            language_id,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn set_project_context(
        &self,
        document_id: DocumentId,
        context: ProjectContextChoice,
    ) -> Result<LspStatus, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::SetProjectContext {
            document_id,
            context,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn set_project_trust(
        &self,
        root: PathBuf,
        adapter_id: Option<String>,
        decision: TrustDecision,
    ) -> Result<(), ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::SetProjectTrust {
            root,
            adapter_id,
            decision,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn set_enablement(&self, enablement: Enablement) -> Result<(), ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::SetEnablement { enablement, reply })
            .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn completion(
        &self,
        document_id: DocumentId,
        position: EditorPosition,
        trigger: Option<String>,
    ) -> Result<CompletionResponse, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::Completion {
            document_id,
            position,
            trigger,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn hover(
        &self,
        document_id: DocumentId,
        position: EditorPosition,
    ) -> Result<HoverResponse, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::Hover {
            document_id,
            position,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn signature_help(
        &self,
        document_id: DocumentId,
        position: EditorPosition,
        trigger: Option<String>,
    ) -> Result<SignatureHelpResponse, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::SignatureHelp {
            document_id,
            position,
            trigger,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn definition(
        &self,
        document_id: DocumentId,
        position: EditorPosition,
    ) -> Result<DefinitionResponse, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::Definition {
            document_id,
            position,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn problems_snapshot(
        &self,
        root: Option<PathBuf>,
    ) -> Result<DiagnosticSnapshot, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::ProblemsSnapshot { root, reply })
            .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn status_snapshot(
        &self,
        document_id: Option<DocumentId>,
    ) -> Result<Vec<LspStatus>, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::StatusSnapshot { document_id, reply })
            .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn restart_session(
        &self,
        adapter_id: String,
        root: PathBuf,
    ) -> Result<(), ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::RestartSession {
            adapter_id,
            root,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn session_logs(
        &self,
        adapter_id: String,
        root: PathBuf,
    ) -> Result<Vec<SessionLogEntry>, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::SessionLogs {
            adapter_id,
            root,
            reply,
        })
        .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn trusted_projects(&self) -> Result<Vec<TrustedProject>, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::TrustedProjects { reply }).await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    pub(crate) async fn revoke_project_trust(
        &self,
        root: PathBuf,
        adapter_id: Option<String>,
    ) -> Result<(), ManagerError> {
        self.set_project_trust(root, adapter_id, TrustDecision::Revoked)
            .await
    }

    pub(crate) async fn shutdown(&self) -> Result<(), ManagerError> {
        let (reply, result) = oneshot::channel();
        self.shutdown
            .send(reply)
            .await
            .map_err(|_| ManagerError::ActorStopped)?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }

    #[cfg(test)]
    async fn expire_reservations_for_test(&self) -> Result<(), ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::ExpireReservationsForTest { reply })
            .await?;
        result.await.map_err(|_| ManagerError::ActorStopped)?
    }
}

enum ManagerCommand {
    ReserveDocument {
        path: PathBuf,
        window_label: String,
        reply: oneshot::Sender<Result<ReserveResult, ManagerError>>,
    },
    ReleaseDocument {
        reservation_id: ReservationId,
        reply: oneshot::Sender<Result<(), ManagerError>>,
    },
    TransferDocument {
        document_id: DocumentId,
        target_reservation_id: ReservationId,
        window_label: String,
        pane_id: String,
        reply: oneshot::Sender<Result<(), ManagerError>>,
    },
    OpenDocument {
        reservation_id: ReservationId,
        pane_id: String,
        contents: String,
        language_id: String,
        reply: oneshot::Sender<Result<OpenDocumentResponse, ManagerError>>,
    },
    ApplyChanges {
        document_id: DocumentId,
        batch: LspChangeBatch,
        reply: oneshot::Sender<Result<ApplyChangesResponse, ManagerError>>,
    },
    ResyncDocument {
        document_id: DocumentId,
        version: i32,
        contents: String,
        reply: oneshot::Sender<Result<ResyncDocumentResponse, ManagerError>>,
    },
    DidSave {
        document_id: DocumentId,
        reply: oneshot::Sender<Result<(), ManagerError>>,
    },
    CloseDocument {
        document_id: DocumentId,
        reply: oneshot::Sender<Result<(), ManagerError>>,
    },
    ProjectCandidates {
        path: PathBuf,
        language_id: String,
        reply: oneshot::Sender<Result<Vec<ProjectCandidate>, ManagerError>>,
    },
    SetProjectContext {
        document_id: DocumentId,
        context: ProjectContextChoice,
        reply: oneshot::Sender<Result<LspStatus, ManagerError>>,
    },
    SetProjectTrust {
        root: PathBuf,
        adapter_id: Option<String>,
        decision: TrustDecision,
        reply: oneshot::Sender<Result<(), ManagerError>>,
    },
    SetEnablement {
        enablement: Enablement,
        reply: oneshot::Sender<Result<(), ManagerError>>,
    },
    Completion {
        document_id: DocumentId,
        position: EditorPosition,
        trigger: Option<String>,
        reply: oneshot::Sender<Result<CompletionResponse, ManagerError>>,
    },
    Hover {
        document_id: DocumentId,
        position: EditorPosition,
        reply: oneshot::Sender<Result<HoverResponse, ManagerError>>,
    },
    SignatureHelp {
        document_id: DocumentId,
        position: EditorPosition,
        trigger: Option<String>,
        reply: oneshot::Sender<Result<SignatureHelpResponse, ManagerError>>,
    },
    Definition {
        document_id: DocumentId,
        position: EditorPosition,
        reply: oneshot::Sender<Result<DefinitionResponse, ManagerError>>,
    },
    ProblemsSnapshot {
        root: Option<PathBuf>,
        reply: oneshot::Sender<Result<DiagnosticSnapshot, ManagerError>>,
    },
    StatusSnapshot {
        document_id: Option<DocumentId>,
        reply: oneshot::Sender<Result<Vec<LspStatus>, ManagerError>>,
    },
    RestartSession {
        adapter_id: String,
        root: PathBuf,
        reply: oneshot::Sender<Result<(), ManagerError>>,
    },
    SessionLogs {
        adapter_id: String,
        root: PathBuf,
        reply: oneshot::Sender<Result<Vec<SessionLogEntry>, ManagerError>>,
    },
    TrustedProjects {
        reply: oneshot::Sender<Result<Vec<TrustedProject>, ManagerError>>,
    },
    #[cfg(test)]
    ExpireReservationsForTest {
        reply: oneshot::Sender<Result<(), ManagerError>>,
    },
}

impl ManagerCommand {
    fn is_policy(&self) -> bool {
        matches!(
            self,
            Self::SetProjectContext { .. }
                | Self::SetProjectTrust { .. }
                | Self::SetEnablement { .. }
        )
    }

    fn policy_reply_closed(&self) -> bool {
        match self {
            Self::SetProjectContext { reply, .. } => reply.is_closed(),
            Self::SetProjectTrust { reply, .. } | Self::SetEnablement { reply, .. } => {
                reply.is_closed()
            }
            _ => false,
        }
    }

    fn fail_policy(self, error: ManagerError) {
        match self {
            Self::SetProjectContext { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            Self::SetProjectTrust { reply, .. } | Self::SetEnablement { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            _ => unreachable!("only policy commands are deferred"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RequestKind {
    Completion,
    Hover,
    SignatureHelp,
    Definition,
}

enum PendingReply {
    Completion(oneshot::Sender<Result<CompletionResponse, ManagerError>>),
    Hover(oneshot::Sender<Result<HoverResponse, ManagerError>>),
    SignatureHelp(oneshot::Sender<Result<SignatureHelpResponse, ManagerError>>),
    Definition(oneshot::Sender<Result<DefinitionResponse, ManagerError>>),
}

impl PendingReply {
    fn send_error(self, error: ManagerError) {
        match self {
            Self::Completion(reply) => {
                let _ = reply.send(Err(error));
            }
            Self::Hover(reply) => {
                let _ = reply.send(Err(error));
            }
            Self::SignatureHelp(reply) => {
                let _ = reply.send(Err(error));
            }
            Self::Definition(reply) => {
                let _ = reply.send(Err(error));
            }
        }
    }
}

enum RequestResult {
    Completion(Result<CompletionResponse, String>),
    Hover(Result<HoverResponse, String>),
    SignatureHelp(Result<SignatureHelpResponse, String>),
    Definition(Result<DefinitionResponse, String>),
}

struct PendingRequest {
    document_id: DocumentId,
    version: i32,
    uri: String,
    session_key: SessionKey,
    session_generation: u64,
    kind: RequestKind,
    cancellation: CancellationToken,
    reply: PendingReply,
}

enum SessionOperation {
    DidOpen(SessionDocument),
    DidChange(LspChangeBatch),
    DidSave(String),
    DidClose(String),
    Request {
        request_id: u64,
        kind: RequestKind,
        document_id: String,
        position: lsp::Position,
        source_version: i32,
        cancellation: CancellationToken,
    },
    PullDiagnostics {
        document_id: DocumentId,
        document_id_text: String,
        uri: String,
        source_version: i32,
        previous_result_id: Option<String>,
        pull_generation: u64,
    },
}

struct SessionWorkerHandle {
    operations: mpsc::Sender<SessionOperation>,
    shutdown: Option<oneshot::Sender<()>>,
}

enum ActorInput {
    SessionStarted {
        key: SessionKey,
        generation: u64,
        result: Result<Arc<dyn SessionClient>, ManagerError>,
    },
    ClientEvent {
        key: SessionKey,
        generation: u64,
        event: ClientEvent,
    },
    SessionOperationFailed {
        key: SessionKey,
        generation: u64,
        operation: &'static str,
        message: String,
    },
    RequestCompleted {
        request_id: u64,
        result: RequestResult,
    },
    PullDiagnosticsCompleted {
        key: SessionKey,
        generation: u64,
        document_id: DocumentId,
        uri: String,
        source_version: i32,
        previous_result_id: Option<String>,
        pull_generation: u64,
        result: Result<(Option<String>, Vec<Diagnostic>), String>,
    },
    BatchDue {
        document_id: DocumentId,
        generation: u64,
    },
    IdleExpired {
        key: SessionKey,
        generation: u64,
    },
    RestartDue {
        key: SessionKey,
        generation: u64,
    },
    ReservationExpired {
        reservation_id: ReservationId,
    },
    ShutdownFinished {
        key: SessionKey,
        generation: u64,
    },
    WorkerReady {
        key: SessionKey,
        generation: u64,
    },
    CacheDeletionFinished {
        reply: oneshot::Sender<Result<(), ManagerError>>,
        result: Result<(), ManagerError>,
    },
    FocusDelivered {
        key: FocusKey,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FocusKey {
    Reservation(ReservationId),
    Owner(DocumentId),
}

enum FocusDelivery {
    Awaiting,
    Ready(ManagerEvent),
    Delivering(Option<ManagerEvent>),
}

#[derive(Clone)]
struct ReservationRecord {
    canonical_path: PathBuf,
    window_label: String,
    expires_at: tokio::time::Instant,
    has_waiter: bool,
}

struct ManagedDocument {
    owner_window: String,
    owner_pane: String,
    path: PathBuf,
    uri: String,
    language: Option<LanguageId>,
    lsp_language_id: Option<String>,
    adapter_id: Option<String>,
    text: VersionedDocument,
    candidates: Vec<ProjectCandidate>,
    binding_scope: PathBuf,
    session_key: Option<SessionKey>,
    selected_root: Option<PathBuf>,
    deferred_for_session: bool,
    pending_batches: Vec<LspChangeBatch>,
    batch_generation: u64,
    pull_result_id: Option<String>,
    pull_generation: u64,
    synchronization_dirty: bool,
    status: LspStatus,
}

struct ManagedSession {
    session_id: String,
    language: LanguageId,
    generation: u64,
    worker: Option<SessionWorkerHandle>,
    startup_cancel: Option<CancellationToken>,
    outbox: VecDeque<SessionOperation>,
    restart_after_stop: bool,
    documents: HashSet<DocumentId>,
    idle_generation: u64,
    crash_timestamps: VecDeque<tokio::time::Instant>,
    automatic_restart_blocked: bool,
    exit_observed: bool,
    logs: VecDeque<SessionLogEntry>,
    next_log_sequence: u64,
    capabilities: LspCapabilities,
    protocol_started: bool,
}

struct ProjectStore {
    persisted: ProjectTrustStore,
    enablement: Enablement,
}

#[derive(Clone)]
struct CacheRootAnchor {
    canonical_path: Option<PathBuf>,
    #[cfg(unix)]
    directory: Option<Arc<fs::File>>,
    capture_error: Option<Arc<str>>,
}

impl CacheRootAnchor {
    fn capture(path: &Path) -> Self {
        let canonical_path = match fs::canonicalize(path) {
            Ok(path) if path.is_dir() => path,
            Ok(_) => {
                return Self {
                    canonical_path: None,
                    #[cfg(unix)]
                    directory: None,
                    capture_error: Some("cache root is not a directory".into()),
                };
            }
            Err(error) => {
                return Self {
                    canonical_path: None,
                    #[cfg(unix)]
                    directory: None,
                    capture_error: Some(error.to_string().into()),
                };
            }
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            match fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(&canonical_path)
            {
                Ok(directory) => Self {
                    canonical_path: Some(canonical_path),
                    directory: Some(Arc::new(directory)),
                    capture_error: None,
                },
                Err(error) => Self {
                    canonical_path: Some(canonical_path),
                    directory: None,
                    capture_error: Some(error.to_string().into()),
                },
            }
        }
        #[cfg(not(unix))]
        {
            Self {
                canonical_path: Some(canonical_path),
                capture_error: None,
            }
        }
    }

    fn unavailable(&self) -> ManagerError {
        ManagerError::Infrastructure(format!(
            "cache root could not be anchored: {}",
            self.capture_error.as_deref().unwrap_or("unknown error")
        ))
    }
}

struct CacheDeletionPlan {
    cache_root: CacheRootAnchor,
    project_root: PathBuf,
    targets: Vec<(String, String, &'static str)>,
}

impl CacheDeletionPlan {
    fn remove(self) -> Result<(), ManagerError> {
        let canonical_cache_root = self
            .cache_root
            .canonical_path
            .as_deref()
            .ok_or_else(|| self.cache_root.unavailable())?;
        if canonical_cache_root.starts_with(&self.project_root) {
            return Err(ManagerError::Infrastructure(
                "refused to remove a cache root inside the project".into(),
            ));
        }
        for (adapter, project_hash, disposable) in self.targets {
            remove_owned_cache_directory_anchored(
                &self.cache_root,
                &adapter,
                &project_hash,
                disposable,
            )?;
        }
        Ok(())
    }
}

struct ManagerState {
    ownership: OwnershipRegistry,
    documents: HashMap<DocumentId, ManagedDocument>,
    sessions: HashMap<SessionKey, ManagedSession>,
    diagnostics: DiagnosticStore,
    projects: ProjectStore,
}

type ShutdownReply = (
    HashSet<(SessionKey, u64)>,
    oneshot::Sender<Result<(), ManagerError>>,
);

pub(crate) struct LspManager {
    state: ManagerState,
    factory: Arc<dyn SessionFactory>,
    cache_root: CacheRootAnchor,
    command_rx: mpsc::Receiver<ManagerCommand>,
    shutdown_rx: mpsc::Receiver<oneshot::Sender<Result<(), ManagerError>>>,
    input_tx: mpsc::Sender<ActorInput>,
    input_rx: mpsc::Receiver<ActorInput>,
    event_tx: mpsc::Sender<ManagerEvent>,
    event_dispatch_cancel: CancellationToken,
    focus_deliveries: HashMap<FocusKey, FocusDelivery>,
    focus_order: VecDeque<FocusKey>,
    focus_in_flight: Option<FocusKey>,
    active_generations: HashSet<(SessionKey, u64)>,
    reservations: HashMap<ReservationId, ReservationRecord>,
    pending_requests: HashMap<u64, PendingRequest>,
    latest_requests: HashMap<(DocumentId, RequestKind), u64>,
    next_request_id: u64,
    pending_policy_commands: VecDeque<ManagerCommand>,
    retrying_policy: bool,
    policy_async_in_flight: bool,
    retry_policy_requested: bool,
    status_revision: u32,
    shutdown_reply: Option<ShutdownReply>,
    shutting_down: bool,
    terminated: bool,
}

impl LspManager {
    pub(crate) fn new(
        factory: Arc<dyn SessionFactory>,
        config_dir: PathBuf,
        cache_root: PathBuf,
        enablement: Enablement,
    ) -> (LspManagerHandle, Self, mpsc::Receiver<ManagerEvent>) {
        let (command_tx, command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let (input_tx, input_rx) = mpsc::channel(ACTOR_INPUT_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(PUBLIC_EVENT_CAPACITY);
        let event_dispatch_cancel = CancellationToken::new();
        let loaded = ProjectTrustStore::load(config_dir);
        if let Some(warning) = loaded.warning {
            log::warn!("LSP project policy was ignored: {}", warning.message);
        }
        let handle = LspManagerHandle {
            commands: command_tx,
            shutdown: shutdown_tx,
        };
        let manager = Self {
            state: ManagerState {
                ownership: OwnershipRegistry::new(),
                documents: HashMap::new(),
                sessions: HashMap::new(),
                diagnostics: DiagnosticStore::default(),
                projects: ProjectStore {
                    persisted: loaded.store,
                    enablement,
                },
            },
            factory,
            cache_root: CacheRootAnchor::capture(&cache_root),
            command_rx,
            shutdown_rx,
            input_tx,
            input_rx,
            event_tx,
            event_dispatch_cancel,
            focus_deliveries: HashMap::new(),
            focus_order: VecDeque::new(),
            focus_in_flight: None,
            active_generations: HashSet::new(),
            reservations: HashMap::new(),
            pending_requests: HashMap::new(),
            latest_requests: HashMap::new(),
            next_request_id: 1,
            pending_policy_commands: VecDeque::new(),
            retrying_policy: false,
            policy_async_in_flight: false,
            retry_policy_requested: false,
            status_revision: 0,
            shutdown_reply: None,
            shutting_down: false,
            terminated: false,
        };
        (handle, manager, event_rx)
    }

    pub(crate) async fn run(mut self) {
        while !self.terminated {
            tokio::select! {
                biased;
                shutdown = self.shutdown_rx.recv(), if !self.shutting_down => {
                    if let Some(reply) = shutdown {
                        self.begin_shutdown(reply);
                    }
                }
                _ = std::future::ready(()), if self.retry_policy_requested && !self.shutting_down => {
                    self.retry_policy_requested = false;
                    self.retry_pending_policy_command();
                }
                command = self.command_rx.recv(), if !self.shutting_down => {
                    let Some(command) = command else { break };
                    if !self.shutting_down {
                        self.handle_command(command);
                    }
                }
                input = self.input_rx.recv() => {
                    let Some(input) = input else { break };
                    self.handle_input(input);
                }
            }
        }
        for (_, pending) in self.pending_requests.drain() {
            pending.cancellation.cancel();
            pending.reply.send_error(ManagerError::ActorStopped);
        }
    }
}

impl LspManager {
    fn handle_command(&mut self, command: ManagerCommand) {
        if command.is_policy() && !self.retrying_policy {
            self.discard_cancelled_policy_front();
            if self.policy_async_in_flight || !self.pending_policy_commands.is_empty() {
                self.defer_policy_command(command, false);
                return;
            }
        }
        self.handle_command_now(command);
    }

    fn handle_command_now(&mut self, command: ManagerCommand) {
        match command {
            ManagerCommand::ReserveDocument {
                path,
                window_label,
                reply,
            } => {
                let _ = reply.send(self.reserve_document(path, window_label));
            }
            ManagerCommand::ReleaseDocument {
                reservation_id,
                reply,
            } => {
                let _ = reply.send(self.release_document(reservation_id));
            }
            ManagerCommand::TransferDocument {
                document_id,
                target_reservation_id,
                window_label,
                pane_id,
                reply,
            } => {
                let _ = reply.send(self.transfer_document(
                    document_id,
                    target_reservation_id,
                    window_label,
                    pane_id,
                ));
            }
            ManagerCommand::OpenDocument {
                reservation_id,
                pane_id,
                contents,
                language_id,
                reply,
            } => {
                let _ =
                    reply.send(self.open_document(reservation_id, pane_id, contents, language_id));
            }
            ManagerCommand::ApplyChanges {
                document_id,
                batch,
                reply,
            } => {
                let _ = reply.send(self.apply_changes(document_id, batch));
            }
            ManagerCommand::ResyncDocument {
                document_id,
                version,
                contents,
                reply,
            } => {
                let _ = reply.send(self.resync_document(document_id, version, contents));
            }
            ManagerCommand::DidSave { document_id, reply } => {
                let _ = reply.send(self.did_save(document_id));
            }
            ManagerCommand::CloseDocument { document_id, reply } => {
                let _ = reply.send(self.close_document(document_id));
            }
            ManagerCommand::ProjectCandidates {
                path,
                language_id,
                reply,
            } => {
                let result = resolve_language(&language_id, &path).and_then(|language| {
                    discover_project_roots(&path, language)
                        .map_err(|error| ManagerError::Infrastructure(error.to_string()))
                });
                let _ = reply.send(result);
            }
            ManagerCommand::SetProjectContext {
                document_id,
                context,
                reply,
            } => match self.set_project_context(document_id, context.clone()) {
                Err(ManagerError::Overloaded) => self.defer_policy_command(
                    ManagerCommand::SetProjectContext {
                        document_id,
                        context,
                        reply,
                    },
                    self.retrying_policy,
                ),
                result => {
                    let _ = reply.send(result);
                }
            },
            ManagerCommand::SetProjectTrust {
                root,
                adapter_id,
                decision,
                reply,
            } => match self.set_project_trust(root.clone(), adapter_id.clone(), decision) {
                Err(ManagerError::Overloaded) => {
                    self.defer_policy_command(
                        ManagerCommand::SetProjectTrust {
                            root,
                            adapter_id,
                            decision,
                            reply,
                        },
                        self.retrying_policy,
                    );
                }
                Err(error) => {
                    let _ = reply.send(Err(error));
                }
                Ok(None) => {
                    let _ = reply.send(Ok(()));
                }
                Ok(Some(plan)) => {
                    self.policy_async_in_flight = true;
                    let input = self.input_tx.clone();
                    tokio::spawn(async move {
                        let result = tokio::task::spawn_blocking(move || plan.remove())
                            .await
                            .map_err(|error| ManagerError::Infrastructure(error.to_string()))
                            .and_then(|result| result);
                        let _ = input
                            .send(ActorInput::CacheDeletionFinished { reply, result })
                            .await;
                    });
                }
            },
            ManagerCommand::SetEnablement { enablement, reply } => {
                match self.set_enablement_policy(enablement.clone()) {
                    Err(ManagerError::Overloaded) => {
                        self.defer_policy_command(
                            ManagerCommand::SetEnablement { enablement, reply },
                            self.retrying_policy,
                        );
                    }
                    result => {
                        let _ = reply.send(result);
                    }
                }
            }
            ManagerCommand::Completion {
                document_id,
                position,
                trigger,
                reply,
            } => {
                let _ = trigger;
                self.start_request(
                    document_id,
                    position,
                    RequestKind::Completion,
                    PendingReply::Completion(reply),
                );
            }
            ManagerCommand::Hover {
                document_id,
                position,
                reply,
            } => self.start_request(
                document_id,
                position,
                RequestKind::Hover,
                PendingReply::Hover(reply),
            ),
            ManagerCommand::SignatureHelp {
                document_id,
                position,
                trigger,
                reply,
            } => {
                let _ = trigger;
                self.start_request(
                    document_id,
                    position,
                    RequestKind::SignatureHelp,
                    PendingReply::SignatureHelp(reply),
                );
            }
            ManagerCommand::Definition {
                document_id,
                position,
                reply,
            } => self.start_request(
                document_id,
                position,
                RequestKind::Definition,
                PendingReply::Definition(reply),
            ),
            ManagerCommand::ProblemsSnapshot { root, reply } => {
                let snapshot = self.state.diagnostics.snapshot(root.as_deref());
                let _ = reply.send(Ok(snapshot));
            }
            ManagerCommand::StatusSnapshot { document_id, reply } => {
                let mut statuses = self
                    .state
                    .documents
                    .iter()
                    .filter(|(candidate, _)| document_id.is_none_or(|wanted| wanted == **candidate))
                    .map(|(_, document)| document.status.clone())
                    .collect::<Vec<_>>();
                statuses.sort_by(|left, right| left.document_id.cmp(&right.document_id));
                let _ = reply.send(Ok(statuses));
            }
            ManagerCommand::RestartSession {
                adapter_id,
                root,
                reply,
            } => {
                let _ = reply.send(self.restart_session(adapter_id, root));
            }
            ManagerCommand::SessionLogs {
                adapter_id,
                root,
                reply,
            } => {
                let result = canonical_directory(&root)
                    .ok()
                    .and_then(|root| self.state.sessions.get(&SessionKey { adapter_id, root }))
                    .map(|session| session.logs.iter().cloned().collect())
                    .ok_or(ManagerError::SessionUnavailable);
                let _ = reply.send(result);
            }
            ManagerCommand::TrustedProjects { reply } => {
                let projects = self
                    .state
                    .projects
                    .persisted
                    .records()
                    .iter()
                    .filter_map(|record| {
                        let root_uri = lsp::Url::from_file_path(&record.workspace)
                            .ok()?
                            .to_string();
                        Some(TrustedProject {
                            root: record.workspace.clone(),
                            root_uri,
                            adapter_id: record.adapter_id.clone(),
                            decision: record.decision,
                            updated_at_ms: record.updated_at_ms,
                            last_used_at_ms: record.last_used_at_ms,
                        })
                    })
                    .collect();
                let _ = reply.send(Ok(projects));
            }
            #[cfg(test)]
            ManagerCommand::ExpireReservationsForTest { reply } => {
                self.cleanup_reservations_at(tokio::time::Instant::now() + RESERVATION_TTL);
                let _ = reply.send(Ok(()));
            }
        }
    }

    fn reserve_document(
        &mut self,
        path: PathBuf,
        window_label: String,
    ) -> Result<ReserveResult, ManagerError> {
        self.cleanup_reservations_at(tokio::time::Instant::now());
        let result = self
            .state
            .ownership
            .reserve(DocumentIdentifier::Local(path.clone()), &window_label)?;
        match &result {
            ReserveResult::Reserved {
                reservation_id,
                canonical_path,
            } => {
                let expires_at = tokio::time::Instant::now() + RESERVATION_TTL;
                self.reservations.insert(
                    *reservation_id,
                    ReservationRecord {
                        canonical_path: PathBuf::from(canonical_path),
                        window_label,
                        expires_at,
                        has_waiter: false,
                    },
                );
                let input = self.input_tx.clone();
                let reservation_id = *reservation_id;
                tokio::spawn(async move {
                    tokio::time::sleep_until(expires_at).await;
                    let _ = input
                        .send(ActorInput::ReservationExpired { reservation_id })
                        .await;
                });
            }
            ReserveResult::FocusPending { window_label } => {
                let reservation_id = self
                    .state
                    .ownership
                    .reservation_id(DocumentIdentifier::Local(path))?
                    .ok_or(ManagerError::InvalidReservation)?;
                self.reserve_focus_obligation(FocusKey::Reservation(reservation_id))?;
                if let Some(reservation) = self.reservations.get_mut(&reservation_id) {
                    reservation.has_waiter = true;
                }
                let _ = window_label;
            }
            ReserveResult::FocusOwner {
                document_id,
                window_label,
                pane_id,
            } => {
                self.queue_focus_event(
                    FocusKey::Owner(*document_id),
                    ManagerEvent::DocumentOwnerFocused {
                        window_label: window_label.clone(),
                        document_id: Some(*document_id),
                        pane_id: Some(pane_id.clone()),
                        reservation_failed: false,
                    },
                )?;
            }
        }
        Ok(result)
    }

    fn release_document(&mut self, reservation_id: ReservationId) -> Result<(), ManagerError> {
        self.state.ownership.release(reservation_id);
        self.finalize_reservation(reservation_id, None, None);
        Ok(())
    }

    fn open_document(
        &mut self,
        reservation_id: ReservationId,
        pane_id: String,
        contents: String,
        language_id: String,
    ) -> Result<OpenDocumentResponse, ManagerError> {
        let reservation = self
            .reservations
            .get(&reservation_id)
            .cloned()
            .ok_or(ManagerError::InvalidReservation)?;
        if tokio::time::Instant::now() >= reservation.expires_at {
            self.state.ownership.release(reservation_id);
            self.finalize_reservation(reservation_id, None, None);
            return Err(ManagerError::InvalidReservation);
        }
        let document_id = match self.state.ownership.commit(reservation_id, &pane_id) {
            Ok(document_id) => document_id,
            Err(error) => {
                self.finalize_reservation(reservation_id, None, None);
                return Err(error.into());
            }
        };
        let Some(path) = self
            .state
            .ownership
            .canonical_path(document_id)
            .map(Path::to_path_buf)
        else {
            self.state.ownership.close(document_id);
            self.finalize_reservation(reservation_id, None, None);
            return Err(ManagerError::InvalidReservation);
        };
        let language = resolve_language(&language_id, &path).ok();
        let catalog = BundledServerCatalog::new();
        let adapter_id =
            language.map(|language| catalog.descriptor(language).adapter_id.to_owned());
        let lsp_language_id = language.map(|language| {
            catalog
                .file_binding(&path)
                .map(|binding| binding.lsp_language_id.to_owned())
                .unwrap_or_else(|| lsp_language_id(language).to_owned())
        });
        let uri = match lsp::Url::from_file_path(&path) {
            Ok(uri) => uri.to_string(),
            Err(()) => {
                self.state.ownership.close(document_id);
                self.finalize_reservation(reservation_id, None, None);
                return Err(ManagerError::InvalidProjectRoot);
            }
        };
        let id_text = document_id_text(document_id);
        let text = match VersionedDocument::new(&id_text, &contents, 1) {
            Ok(text) => text,
            Err(error) => {
                self.state.ownership.close(document_id);
                self.finalize_reservation(reservation_id, None, None);
                return Err(ManagerError::InvalidChange(format!("{error:?}")));
            }
        };
        let candidates = language
            .and_then(|language| discover_project_roots(&path, language).ok())
            .unwrap_or_default();
        let binding_scope = match project_binding_scope(&path, &candidates) {
            Ok(scope) => scope,
            Err(error) => {
                self.state.ownership.close(document_id);
                self.finalize_reservation(reservation_id, None, None);
                return Err(error);
            }
        };
        let status = self.blank_status(document_id, adapter_id.as_deref());
        self.state.documents.insert(
            document_id,
            ManagedDocument {
                owner_window: reservation.window_label.clone(),
                owner_pane: pane_id.clone(),
                path,
                uri,
                language,
                lsp_language_id,
                adapter_id,
                text,
                candidates: candidates.clone(),
                binding_scope,
                session_key: None,
                selected_root: None,
                deferred_for_session: false,
                pending_batches: Vec::new(),
                batch_generation: 0,
                pull_result_id: None,
                pull_generation: 0,
                synchronization_dirty: false,
                status,
            },
        );
        self.finalize_reservation(reservation_id, Some(document_id), Some(pane_id));
        self.reevaluate_document(document_id)?;
        let document = self
            .state
            .documents
            .get(&document_id)
            .ok_or(ManagerError::UnknownDocument)?;
        Ok(OpenDocumentResponse {
            document_id: id_text,
            version: 1,
            project_candidates: candidates,
            status: document.status.clone(),
        })
    }

    fn transfer_document(
        &mut self,
        document_id: DocumentId,
        target_reservation_id: ReservationId,
        window_label: String,
        pane_id: String,
    ) -> Result<(), ManagerError> {
        let reservation = self
            .reservations
            .get(&target_reservation_id)
            .cloned()
            .ok_or(ManagerError::InvalidReservation)?;
        if tokio::time::Instant::now() >= reservation.expires_at {
            return Err(ManagerError::InvalidReservation);
        }
        let document = self
            .state
            .documents
            .get(&document_id)
            .ok_or(ManagerError::UnknownDocument)?;
        if document.owner_window != window_label || document.owner_pane != pane_id {
            return Err(ManagerError::OwnerMismatch);
        }
        // Precompute every fallible target value before the atomic ownership
        // transition or any protocol detachment.
        let target_path = reservation.canonical_path.clone();
        let language = resolve_language(
            target_path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or(""),
            &target_path,
        )
        .ok();
        let catalog = BundledServerCatalog::new();
        let adapter_id =
            language.map(|language| catalog.descriptor(language).adapter_id.to_owned());
        let uri = lsp::Url::from_file_path(&target_path)
            .map_err(|_| ManagerError::InvalidProjectRoot)?
            .to_string();
        let candidates = language
            .and_then(|language| discover_project_roots(&target_path, language).ok())
            .unwrap_or_default();
        let binding_scope = project_binding_scope(&target_path, &candidates)?;
        let transfer_time = Instant::now();
        self.state.ownership.validate_transfer_at(
            document_id,
            target_reservation_id,
            transfer_time,
        )?;
        self.flush_document(document_id)?;
        self.enqueue_close(document_id)?;
        self.state
            .ownership
            .transfer_at(document_id, target_reservation_id, transfer_time)?;
        self.detach_document(document_id, false, false)?;
        if let Some(document) = self.state.documents.get_mut(&document_id) {
            document.path = reservation.canonical_path;
            document.uri = uri;
            document.language = language;
            document.adapter_id = adapter_id;
            document.lsp_language_id = language.map(|language| {
                catalog
                    .file_binding(&document.path)
                    .map(|binding| binding.lsp_language_id.to_owned())
                    .unwrap_or_else(|| lsp_language_id(language).to_owned())
            });
            document.candidates = candidates;
            document.binding_scope = binding_scope;
            document.selected_root = None;
            document.deferred_for_session = false;
            document.pull_result_id = None;
            document.pull_generation = document.pull_generation.saturating_add(1);
        }
        self.finalize_reservation(target_reservation_id, Some(document_id), Some(pane_id));
        self.reevaluate_document(document_id)?;
        Ok(())
    }

    fn apply_changes(
        &mut self,
        document_id: DocumentId,
        batch: LspChangeBatch,
    ) -> Result<ApplyChangesResponse, ManagerError> {
        let document = self
            .state
            .documents
            .get_mut(&document_id)
            .ok_or(ManagerError::UnknownDocument)?;
        if document.synchronization_dirty {
            return Ok(ApplyChangesResponse::ResyncRequired {
                expected_version: document.text.version(),
                received_base_version: batch.base_version,
            });
        }
        if document.pending_batches.len() >= DOCUMENT_PENDING_CAPACITY {
            return Err(ManagerError::Overloaded);
        }
        match document.text.apply_batch(batch.clone()) {
            Ok(applied) => {
                let arm_deadline = document.pending_batches.is_empty();
                document.pending_batches.push(batch);
                if arm_deadline {
                    document.batch_generation = document.batch_generation.saturating_add(1);
                    let generation = document.batch_generation;
                    let input = self.input_tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(CHANGE_BATCH_DELAY).await;
                        let _ = input
                            .send(ActorInput::BatchDue {
                                document_id,
                                generation,
                            })
                            .await;
                    });
                }
                self.cancel_document_requests(document_id, ManagerError::StaleResponse);
                Ok(ApplyChangesResponse::Applied {
                    version: applied.version,
                })
            }
            Err(DocumentError::VersionMismatch { expected, actual }) => {
                Ok(ApplyChangesResponse::ResyncRequired {
                    expected_version: expected,
                    received_base_version: actual,
                })
            }
            Err(error) => Err(ManagerError::InvalidChange(format!("{error:?}"))),
        }
    }

    fn resync_document(
        &mut self,
        document_id: DocumentId,
        version: i32,
        contents: String,
    ) -> Result<ResyncDocumentResponse, ManagerError> {
        let (key, open) = {
            let document = self
                .state
                .documents
                .get(&document_id)
                .ok_or(ManagerError::UnknownDocument)?;
            if version <= document.text.version() {
                return Err(ManagerError::InvalidChange(format!(
                    "{:?}",
                    DocumentError::NonMonotonicVersion {
                        base: document.text.version(),
                        next: version,
                    }
                )));
            }
            let replacement =
                VersionedDocument::new(&document_id_text(document_id), &contents, version)
                    .map_err(|error| ManagerError::InvalidChange(format!("{error:?}")))?;
            let open = SessionDocument {
                document_id: document_id_text(document_id),
                uri: lsp::Url::parse(&document.uri)
                    .map_err(|_| ManagerError::InvalidProjectRoot)?,
                language_id: document
                    .lsp_language_id
                    .clone()
                    .ok_or(ManagerError::SessionUnavailable)?,
                version,
                text: replacement.text(),
            };
            (document.session_key.clone(), open)
        };
        if let Some(key) = key {
            self.enqueue_session_operations(
                &key,
                [
                    SessionOperation::DidClose(document_id_text(document_id)),
                    SessionOperation::DidOpen(open),
                ],
            )?;
        }
        self.cancel_document_requests(document_id, ManagerError::StaleResponse);
        let document = self.state.documents.get_mut(&document_id).expect("checked");
        document.text = VersionedDocument::new(&document_id_text(document_id), &contents, version)
            .map_err(|error| ManagerError::InvalidChange(format!("{error:?}")))?;
        document.pending_batches.clear();
        document.batch_generation = document.batch_generation.saturating_add(1);
        document.synchronization_dirty = false;
        Ok(ResyncDocumentResponse {
            document_id: document_id_text(document_id),
            version,
            status: document.status.clone(),
        })
    }

    fn did_save(&mut self, document_id: DocumentId) -> Result<(), ManagerError> {
        self.flush_document(document_id)?;
        let document = self
            .state
            .documents
            .get_mut(&document_id)
            .ok_or(ManagerError::UnknownDocument)?;
        document.pull_generation = document.pull_generation.saturating_add(1);
        let pull_generation = document.pull_generation;
        let Some(key) = document.session_key.clone() else {
            return Ok(());
        };
        if !self.state.sessions.contains_key(&key) {
            return Ok(());
        }
        let id_text = document_id_text(document_id);
        let operation = SessionOperation::PullDiagnostics {
            document_id,
            document_id_text: id_text,
            uri: document.uri.clone(),
            source_version: document.text.version(),
            previous_result_id: document.pull_result_id.clone(),
            pull_generation,
        };
        self.enqueue_session_operations(
            &key,
            [
                SessionOperation::DidSave(document_id_text(document_id)),
                operation,
            ],
        )?;
        Ok(())
    }

    fn close_document(&mut self, document_id: DocumentId) -> Result<(), ManagerError> {
        if !self.state.documents.contains_key(&document_id) {
            self.state.ownership.close(document_id);
            return Ok(());
        }
        self.flush_document(document_id)?;
        self.enqueue_close(document_id)?;
        self.cancel_document_requests(document_id, ManagerError::Cancelled);
        self.detach_document(document_id, true, false)?;
        self.state.documents.remove(&document_id);
        self.state.ownership.close(document_id);
        Ok(())
    }

    fn set_project_context(
        &mut self,
        document_id: DocumentId,
        context: ProjectContextChoice,
    ) -> Result<LspStatus, ManagerError> {
        let (path, binding_scope, candidates) = self
            .state
            .documents
            .get(&document_id)
            .map(|document| {
                (
                    document.path.clone(),
                    document.binding_scope.clone(),
                    document.candidates.clone(),
                )
            })
            .ok_or(ManagerError::UnknownDocument)?;
        let mut staged = self.state.projects.persisted.clone();
        match context {
            ProjectContextChoice::Root { root } => {
                let root = canonical_directory(Path::new(&root))?;
                if !path.starts_with(&root)
                    || !candidates
                        .iter()
                        .any(|candidate| Path::new(&candidate.canonical_path) == root)
                {
                    return Err(ManagerError::InvalidProjectRoot);
                }
                staged
                    .set_root_binding(&binding_scope, RootBinding::Root(root.clone()))
                    .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
                let desired = self.desired_session_key(
                    document_id,
                    &staged,
                    &self.state.projects.enablement,
                    false,
                );
                self.ensure_transition_capacity([(document_id, desired)])?;
                staged
                    .save()
                    .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
                self.state.projects.persisted = staged;
                if let Some(document) = self.state.documents.get_mut(&document_id) {
                    document.deferred_for_session = false;
                    document.selected_root = Some(root);
                }
            }
            ProjectContextChoice::Disabled => {
                staged
                    .set_root_binding(&binding_scope, RootBinding::Disabled)
                    .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
                self.ensure_transition_capacity([(document_id, None)])?;
                staged
                    .save()
                    .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
                self.state.projects.persisted = staged;
                if let Some(document) = self.state.documents.get_mut(&document_id) {
                    document.deferred_for_session = false;
                    document.selected_root = None;
                }
            }
            ProjectContextChoice::DeferForSession => {
                self.ensure_transition_capacity([(document_id, None)])?;
                if let Some(document) = self.state.documents.get_mut(&document_id) {
                    document.deferred_for_session = true;
                    document.selected_root = None;
                }
            }
        }
        self.reevaluate_document(document_id)?;
        self.state
            .documents
            .get(&document_id)
            .map(|document| document.status.clone())
            .ok_or(ManagerError::UnknownDocument)
    }

    fn set_project_trust(
        &mut self,
        root: PathBuf,
        adapter_id: Option<String>,
        decision: TrustDecision,
    ) -> Result<Option<CacheDeletionPlan>, ManagerError> {
        let root = canonical_directory(&root)?;
        let documents = self
            .state
            .documents
            .iter()
            .filter_map(|(id, document)| {
                let applies = document.selected_root.as_deref() == Some(root.as_path())
                    || self
                        .state
                        .projects
                        .persisted
                        .binding_for(&document.path)
                        .is_some_and(|binding| binding == RootBinding::Root(root.clone()));
                let adapter_matches = adapter_id
                    .as_deref()
                    .is_none_or(|adapter| document.adapter_id.as_deref() == Some(adapter));
                (applies && adapter_matches).then_some(*id)
            })
            .collect::<Vec<_>>();
        let mut staged = self.state.projects.persisted.clone();
        if decision == TrustDecision::Revoked && adapter_id.is_none() {
            staged
                .revoke_all_at_root(&root, now_ms())
                .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
        } else {
            staged
                .set_trust(&root, adapter_id.as_deref(), decision, now_ms())
                .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
        }
        let transitions = documents
            .iter()
            .map(|document| {
                (
                    *document,
                    self.desired_session_key(
                        *document,
                        &staged,
                        &self.state.projects.enablement,
                        self.state.documents[document].deferred_for_session,
                    ),
                )
            })
            .collect::<Vec<_>>();
        self.ensure_transition_capacity(transitions)?;
        staged
            .save()
            .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
        self.state.projects.persisted = staged;
        if decision == TrustDecision::Revoked {
            for document in &documents {
                self.detach_document(*document, false, true)?;
            }
            self.revoke_sessions(&root, adapter_id.as_deref());
        }
        for document in documents {
            self.reevaluate_document(document)?;
        }
        Ok((decision == TrustDecision::Revoked)
            .then(|| self.disposable_cache_plan(&root, adapter_id.as_deref())))
    }
}

impl LspManager {
    fn set_enablement_policy(&mut self, enablement: Enablement) -> Result<(), ManagerError> {
        let documents = self.state.documents.keys().copied().collect::<Vec<_>>();
        let transitions = documents
            .iter()
            .map(|document| {
                (
                    *document,
                    self.desired_session_key(
                        *document,
                        &self.state.projects.persisted,
                        &enablement,
                        self.state.documents[document].deferred_for_session,
                    ),
                )
            })
            .collect::<Vec<_>>();
        self.ensure_transition_capacity(transitions)?;
        self.state.projects.enablement = enablement;
        for document in documents {
            self.reevaluate_document(document)?;
        }
        self.stop_disabled_sessions();
        Ok(())
    }

    fn retry_pending_policy_command(&mut self) {
        if self.shutting_down || self.policy_async_in_flight {
            return;
        }
        self.discard_cancelled_policy_front();
        let queued_before = self.pending_policy_commands.len();
        let Some(command) = self.pending_policy_commands.pop_front() else {
            return;
        };
        self.retrying_policy = true;
        self.handle_command_now(command);
        self.retrying_policy = false;
        // An overloaded retry puts the same command back at the front. Do not
        // turn that into a permanently-ready actor branch: the lifecycle or
        // worker event that frees more capacity will retry it. When the front
        // did complete, schedule exactly one following FIFO command.
        let front_progressed = self.pending_policy_commands.len() < queued_before;
        if front_progressed
            && !self.policy_async_in_flight
            && !self.pending_policy_commands.is_empty()
        {
            self.retry_policy_requested = true;
        }
    }

    fn defer_policy_command(&mut self, command: ManagerCommand, retry_at_front: bool) {
        if command.policy_reply_closed() {
            return;
        }
        if self.pending_policy_commands.len() >= COMMAND_CAPACITY {
            command.fail_policy(ManagerError::Overloaded);
        } else if retry_at_front {
            self.pending_policy_commands.push_front(command);
        } else {
            self.pending_policy_commands.push_back(command);
        }
    }

    fn discard_cancelled_policy_front(&mut self) {
        while self
            .pending_policy_commands
            .front()
            .is_some_and(ManagerCommand::policy_reply_closed)
        {
            self.pending_policy_commands.pop_front();
        }
    }

    fn ensure_close_capacity(
        &self,
        documents: impl IntoIterator<Item = DocumentId>,
    ) -> Result<(), ManagerError> {
        let mut required = HashMap::<SessionKey, usize>::new();
        for document_id in documents {
            if let Some(key) = self
                .state
                .documents
                .get(&document_id)
                .and_then(|document| document.session_key.clone())
                && self
                    .state
                    .sessions
                    .get(&key)
                    .is_some_and(|session| session.protocol_started)
            {
                *required.entry(key).or_default() += 1;
            }
        }
        for (key, count) in required {
            let session = self
                .state
                .sessions
                .get(&key)
                .ok_or(ManagerError::SessionUnavailable)?;
            if session.outbox.len().saturating_add(count) > SESSION_OUTBOX_CAPACITY {
                return Err(ManagerError::Overloaded);
            }
        }
        Ok(())
    }

    fn desired_session_key(
        &self,
        document_id: DocumentId,
        persisted: &ProjectTrustStore,
        enablement: &Enablement,
        deferred: bool,
    ) -> Option<SessionKey> {
        let document = self.state.documents.get(&document_id)?;
        let adapter_id = document.adapter_id.clone()?;
        if deferred || !enablement.enables(&adapter_id) {
            return None;
        }
        let RootBinding::Root(root) = persisted.binding_for(&document.path)? else {
            return None;
        };
        if !document.path.starts_with(&root)
            || !persisted
                .trust_for(&root, Some(&adapter_id))
                .is_some_and(|record| record.decision == TrustDecision::Trusted)
        {
            return None;
        }
        Some(SessionKey { adapter_id, root })
    }

    fn ensure_transition_capacity(
        &self,
        transitions: impl IntoIterator<Item = (DocumentId, Option<SessionKey>)>,
    ) -> Result<(), ManagerError> {
        let mut operations = HashMap::<SessionKey, usize>::new();
        let mut attachments = HashMap::<SessionKey, usize>::new();
        for (document_id, desired) in transitions {
            let current = self
                .state
                .documents
                .get(&document_id)
                .ok_or(ManagerError::UnknownDocument)?
                .session_key
                .clone();
            if current == desired {
                continue;
            }
            if let Some(current) = current {
                if self
                    .state
                    .sessions
                    .get(&current)
                    .is_some_and(|session| session.protocol_started)
                {
                    *operations.entry(current).or_default() += 1;
                }
            }
            if let Some(desired) = desired
                && let Some(session) = self.state.sessions.get(&desired)
            {
                *attachments.entry(desired.clone()).or_default() += 1;
                if session.worker.is_some() {
                    *operations.entry(desired).or_default() += 1;
                }
            }
        }
        for (key, count) in operations {
            let session = self
                .state
                .sessions
                .get(&key)
                .ok_or(ManagerError::SessionUnavailable)?;
            if session.outbox.len().saturating_add(count) > SESSION_OUTBOX_CAPACITY {
                return Err(ManagerError::Overloaded);
            }
        }
        for (key, count) in attachments {
            if self.state.sessions[&key]
                .documents
                .len()
                .saturating_add(count)
                > SESSION_OUTBOX_CAPACITY
            {
                return Err(ManagerError::Overloaded);
            }
        }
        Ok(())
    }

    fn reevaluate_document(&mut self, document_id: DocumentId) -> Result<(), ManagerError> {
        let Some(document) = self.state.documents.get(&document_id) else {
            return Ok(());
        };
        let Some(adapter_id) = document.adapter_id.clone() else {
            self.detach_document(document_id, false, true)?;
            self.set_document_status(
                document_id,
                LspSessionState::Disabled,
                None,
                Some("Project features are not available for this language".into()),
                None,
            );
            return Ok(());
        };
        let path = document.path.clone();
        let deferred_for_session = document.deferred_for_session;
        let candidates = document.candidates.clone();
        if !self.state.projects.enablement.enables(&adapter_id) {
            self.detach_document(document_id, false, true)?;
            self.set_document_status(
                document_id,
                LspSessionState::Disabled,
                None,
                Some("Project features are disabled".into()),
                None,
            );
            return Ok(());
        }
        if deferred_for_session {
            self.detach_document(document_id, false, true)?;
            self.set_document_status(
                document_id,
                LspSessionState::ChoosingProject,
                None,
                Some("Project selection deferred for this session".into()),
                None,
            );
            return Ok(());
        }

        let binding = self.state.projects.persisted.binding_for(&path);
        match binding {
            Some(RootBinding::Disabled) => {
                self.detach_document(document_id, false, true)?;
                self.set_document_status(
                    document_id,
                    LspSessionState::Disabled,
                    None,
                    Some("Project features are off".into()),
                    None,
                );
            }
            Some(RootBinding::Root(root)) => {
                if !path.starts_with(&root) {
                    self.detach_document(document_id, false, true)?;
                    self.set_document_status(
                        document_id,
                        LspSessionState::ChoosingProject,
                        None,
                        Some("Choose project context".into()),
                        None,
                    );
                    return Ok(());
                }
                if let Some(document) = self.state.documents.get_mut(&document_id) {
                    document.selected_root = Some(root.clone());
                }
                let trusted = self
                    .state
                    .projects
                    .persisted
                    .trust_for(&root, Some(&adapter_id))
                    .is_some_and(|record| record.decision == TrustDecision::Trusted);
                if !trusted {
                    self.detach_document(document_id, false, true)?;
                    self.set_document_status(
                        document_id,
                        LspSessionState::Untrusted,
                        Some(SessionKey { adapter_id, root }),
                        Some("Trust this project to enable project features".into()),
                        None,
                    );
                    return Ok(());
                }
                self.attach_document(document_id, root)?;
            }
            None => {
                self.detach_document(document_id, false, true)?;
                let non_fallback = candidates
                    .iter()
                    .filter(|candidate| !candidate.is_fallback)
                    .collect::<Vec<_>>();
                let clear_candidate = if non_fallback.len() == 1 {
                    Some(non_fallback[0])
                } else if non_fallback.is_empty() && candidates.len() == 1 {
                    candidates.first()
                } else {
                    None
                };
                if let Some(candidate) = clear_candidate {
                    let root = PathBuf::from(&candidate.canonical_path);
                    if let Some(document) = self.state.documents.get_mut(&document_id) {
                        document.selected_root = Some(root.clone());
                    }
                    self.set_document_status(
                        document_id,
                        LspSessionState::Untrusted,
                        Some(SessionKey { adapter_id, root }),
                        Some("Choose and trust this project to enable project features".into()),
                        None,
                    );
                } else {
                    self.set_document_status(
                        document_id,
                        LspSessionState::ChoosingProject,
                        None,
                        Some("Choose project context".into()),
                        None,
                    );
                }
            }
        }
        Ok(())
    }

    fn attach_document(
        &mut self,
        document_id: DocumentId,
        root: PathBuf,
    ) -> Result<(), ManagerError> {
        let Some(document) = self.state.documents.get(&document_id) else {
            return Ok(());
        };
        let (Some(adapter_id), Some(language)) = (document.adapter_id.clone(), document.language)
        else {
            return Ok(());
        };
        let key = SessionKey { adapter_id, root };
        let current_key = document.session_key.clone();
        if current_key.as_ref() == Some(&key) {
            return Ok(());
        }
        self.ensure_close_capacity([document_id])?;
        let needs_start = !self.state.sessions.contains_key(&key);
        if let Some(session) = self.state.sessions.get(&key)
            && (session.documents.len() >= SESSION_OUTBOX_CAPACITY
                || (session.worker.is_some() && session.outbox.len() >= SESSION_OUTBOX_CAPACITY))
        {
            return Err(ManagerError::Overloaded);
        }
        self.detach_document(document_id, false, true)?;
        if needs_start {
            let session_id = format!("{}-{}", key.adapter_id, uuid::Uuid::new_v4());
            let generation = self
                .active_generations
                .iter()
                .filter_map(|(active_key, generation)| (active_key == &key).then_some(*generation))
                .max()
                .unwrap_or(0);
            self.state.sessions.insert(
                key.clone(),
                ManagedSession {
                    session_id,
                    language,
                    generation,
                    worker: None,
                    startup_cancel: None,
                    outbox: VecDeque::new(),
                    restart_after_stop: false,
                    documents: HashSet::new(),
                    idle_generation: 0,
                    crash_timestamps: VecDeque::new(),
                    automatic_restart_blocked: false,
                    exit_observed: false,
                    logs: VecDeque::new(),
                    next_log_sequence: 1,
                    capabilities: capabilities(false),
                    protocol_started: false,
                },
            );
        }
        let (ready, session_state) = {
            let session = self.state.sessions.get_mut(&key).expect("inserted session");
            session.documents.insert(document_id);
            session.idle_generation = session.idle_generation.saturating_add(1);
            (
                session.worker.is_some(),
                if session.worker.is_some() {
                    LspSessionState::Ready
                } else {
                    LspSessionState::Starting
                },
            )
        };
        if let Some(document) = self.state.documents.get_mut(&document_id) {
            document.session_key = Some(key.clone());
            document.selected_root = Some(key.root.clone());
        }
        if ready {
            self.enqueue_open(document_id, &key)?;
            if let Some(document) = self.state.documents.get_mut(&document_id) {
                document.synchronization_dirty = false;
            }
        } else if needs_start {
            self.start_session(key.clone());
        }
        self.set_document_status(document_id, session_state, Some(key), None, None);
        Ok(())
    }

    fn detach_document(
        &mut self,
        document_id: DocumentId,
        closing: bool,
        queue_close: bool,
    ) -> Result<(), ManagerError> {
        let attachment = self.state.documents.get(&document_id).and_then(|document| {
            document
                .session_key
                .clone()
                .map(|key| (key, document.uri.clone()))
        });
        let Some((key, uri)) = attachment else {
            return Ok(());
        };
        if queue_close
            && self
                .state
                .sessions
                .get(&key)
                .is_some_and(|session| session.protocol_started)
        {
            self.enqueue_session_operations(
                &key,
                [SessionOperation::DidClose(document_id_text(document_id))],
            )?;
        }
        self.cancel_document_requests(document_id, ManagerError::Cancelled);
        if let Some(document) = self.state.documents.get_mut(&document_id) {
            document.pull_generation = document.pull_generation.saturating_add(1);
        }
        let session_id = self
            .state
            .sessions
            .get(&key)
            .map(|session| session.session_id.clone());
        if let Some(session_id) = session_id
            && self.state.diagnostics.clear_session_uri(&session_id, &uri)
        {
            self.emit_diagnostics(Some(session_id), Some(document_id), Some(uri));
        }
        if let Some(session) = self.state.sessions.get_mut(&key) {
            session.documents.remove(&document_id);
            if session.documents.is_empty() {
                session.idle_generation = session.idle_generation.saturating_add(1);
                let generation = session.idle_generation;
                let input = self.input_tx.clone();
                let idle_key = key.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(IDLE_SHUTDOWN_DELAY).await;
                    let _ = input
                        .send(ActorInput::IdleExpired {
                            key: idle_key,
                            generation,
                        })
                        .await;
                });
            }
        }
        if let Some(document) = self.state.documents.get_mut(&document_id) {
            document.session_key = None;
            if closing {
                document.selected_root = None;
            }
        }
        Ok(())
    }

    fn enqueue_close(&mut self, document_id: DocumentId) -> Result<(), ManagerError> {
        let key = self
            .state
            .documents
            .get(&document_id)
            .ok_or(ManagerError::UnknownDocument)?
            .session_key
            .clone();
        if let Some(key) = key {
            if self
                .state
                .sessions
                .get(&key)
                .is_some_and(|session| session.protocol_started)
            {
                self.enqueue_session_operations(
                    &key,
                    [SessionOperation::DidClose(document_id_text(document_id))],
                )?;
            }
        }
        Ok(())
    }

    fn start_session(&mut self, key: SessionKey) {
        if self
            .active_generations
            .iter()
            .any(|(active_key, _)| active_key == &key)
        {
            if let Some(session) = self.state.sessions.get_mut(&key) {
                session.restart_after_stop = true;
            }
            return;
        }
        let Some(session) = self.state.sessions.get_mut(&key) else {
            return;
        };
        if session.documents.is_empty() {
            return;
        }
        session.generation = session.generation.saturating_add(1);
        session.exit_observed = false;
        session.worker = None;
        session.protocol_started = false;
        session.outbox.clear();
        let generation = session.generation;
        self.active_generations.insert((key.clone(), generation));
        let start = SessionStart {
            key: key.clone(),
            language: session.language,
            session_id: session.session_id.clone(),
        };
        let factory = self.factory.clone();
        let input = self.input_tx.clone();
        let cancellation = CancellationToken::new();
        session.startup_cancel = Some(cancellation.clone());
        tokio::spawn(async move {
            let (client_events, mut client_event_rx) = mpsc::channel(ACTOR_INPUT_CAPACITY);
            tokio::select! {
                _ = cancellation.cancelled() => {
                    let _ = input.send(ActorInput::ShutdownFinished { key, generation }).await;
                }
                result = factory.start(start, client_events) => {
                    // FIFO on actor input is the lifecycle barrier: no client
                    // event for this generation can overtake SessionStarted.
                    if input.send(ActorInput::SessionStarted {
                        key: key.clone(), generation, result,
                    }).await.is_err() {
                        return;
                    }
                    while let Some(event) = client_event_rx.recv().await {
                        if input.send(ActorInput::ClientEvent {
                            key: key.clone(), generation, event,
                        }).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    fn enqueue_open(
        &mut self,
        document_id: DocumentId,
        key: &SessionKey,
    ) -> Result<(), ManagerError> {
        let document = self
            .state
            .documents
            .get(&document_id)
            .ok_or(ManagerError::UnknownDocument)?;
        let uri = lsp::Url::parse(&document.uri).map_err(|_| ManagerError::InvalidProjectRoot)?;
        let operation = SessionOperation::DidOpen(SessionDocument {
            document_id: document_id_text(document_id),
            uri,
            language_id: document
                .lsp_language_id
                .clone()
                .ok_or(ManagerError::SessionUnavailable)?,
            version: document.text.version(),
            text: document.text.text(),
        });
        self.enqueue_session_operations(key, [operation])
    }

    fn flush_document(&mut self, document_id: DocumentId) -> Result<(), ManagerError> {
        let (key, batch_count) = {
            let document = self
                .state
                .documents
                .get(&document_id)
                .ok_or(ManagerError::UnknownDocument)?;
            (document.session_key.clone(), document.pending_batches.len())
        };
        let Some(key) = key else {
            if let Some(document) = self.state.documents.get_mut(&document_id) {
                document.pending_batches.clear();
                document.batch_generation = document.batch_generation.saturating_add(1);
            }
            return Ok(());
        };
        let available = self
            .state
            .sessions
            .get(&key)
            .map(|session| SESSION_OUTBOX_CAPACITY.saturating_sub(session.outbox.len()))
            .ok_or(ManagerError::SessionUnavailable)?;
        if batch_count > available {
            return Err(ManagerError::Overloaded);
        }
        let batches = {
            let document = self.state.documents.get_mut(&document_id).expect("checked");
            document.batch_generation = document.batch_generation.saturating_add(1);
            std::mem::take(&mut document.pending_batches)
        };
        self.enqueue_session_operations(
            &key,
            batches.into_iter().map(SessionOperation::DidChange),
        )?;
        Ok(())
    }

    fn enqueue_session_operations(
        &mut self,
        key: &SessionKey,
        operations: impl IntoIterator<Item = SessionOperation>,
    ) -> Result<(), ManagerError> {
        let operations = operations.into_iter().collect::<Vec<_>>();
        let session = self
            .state
            .sessions
            .get_mut(key)
            .ok_or(ManagerError::SessionUnavailable)?;
        if session.outbox.len().saturating_add(operations.len()) > SESSION_OUTBOX_CAPACITY {
            return Err(ManagerError::Overloaded);
        }
        session.outbox.extend(operations);
        self.pump_session(key);
        Ok(())
    }

    fn pump_session(&mut self, key: &SessionKey) {
        let Some(session) = self.state.sessions.get_mut(key) else {
            return;
        };
        let Some(worker) = session.worker.as_ref() else {
            return;
        };
        while let Some(operation) = session.outbox.pop_front() {
            match worker.operations.try_send(operation) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(operation)) => {
                    session.outbox.push_front(operation);
                    break;
                }
                Err(mpsc::error::TrySendError::Closed(operation)) => {
                    session.outbox.push_front(operation);
                    break;
                }
            }
        }
    }

    fn start_request(
        &mut self,
        document_id: DocumentId,
        position: EditorPosition,
        kind: RequestKind,
        reply: PendingReply,
    ) {
        if let Err(error) = self.flush_document(document_id) {
            reply.send_error(error);
            return;
        }
        let Some(document) = self.state.documents.get(&document_id) else {
            reply.send_error(ManagerError::UnknownDocument);
            return;
        };
        let Some(session_key) = document.session_key.clone() else {
            reply.send_error(ManagerError::SessionUnavailable);
            return;
        };
        let Some(session) = self.state.sessions.get(&session_key) else {
            reply.send_error(ManagerError::SessionUnavailable);
            return;
        };
        if session.worker.is_none() {
            reply.send_error(ManagerError::SessionUnavailable);
            return;
        }
        let session_generation = session.generation;
        let version = document.text.version();
        let uri = document.uri.clone();
        if let Some(previous_id) = self.latest_requests.remove(&(document_id, kind))
            && let Some(previous) = self.pending_requests.remove(&previous_id)
        {
            previous.cancellation.cancel();
            previous.reply.send_error(ManagerError::Cancelled);
        }
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        let cancellation = CancellationToken::new();
        let pending = PendingRequest {
            document_id,
            version,
            uri,
            session_key: session_key.clone(),
            session_generation,
            kind,
            cancellation: cancellation.clone(),
            reply,
        };
        let operation = SessionOperation::Request {
            request_id,
            kind,
            document_id: document_id_text(document_id),
            position: lsp::Position::new(position.line, position.character),
            source_version: pending.version,
            cancellation,
        };
        self.pending_requests.insert(request_id, pending);
        self.latest_requests.insert((document_id, kind), request_id);
        if self
            .enqueue_session_operations(&session_key, [operation])
            .is_err()
        {
            self.latest_requests.remove(&(document_id, kind));
            if let Some(pending) = self.pending_requests.remove(&request_id) {
                pending.reply.send_error(ManagerError::Overloaded);
            }
        }
    }

    fn cancel_document_requests(&mut self, document_id: DocumentId, error: ManagerError) {
        let ids = self
            .pending_requests
            .iter()
            .filter_map(|(id, pending)| (pending.document_id == document_id).then_some(*id))
            .collect::<Vec<_>>();
        for id in ids {
            if let Some(pending) = self.pending_requests.remove(&id) {
                self.latest_requests.remove(&(document_id, pending.kind));
                pending.cancellation.cancel();
                pending.reply.send_error(error.clone());
            }
        }
    }

    fn set_document_status(
        &mut self,
        document_id: DocumentId,
        state: LspSessionState,
        session_key: Option<SessionKey>,
        message: Option<String>,
        unavailable_reason: Option<LspUnavailableReason>,
    ) {
        self.status_revision = self
            .status_revision
            .checked_add(1)
            .expect("LSP status revision exhausted");
        let session_id = session_key
            .as_ref()
            .and_then(|key| self.state.sessions.get(key))
            .map(|session| session.session_id.clone());
        let negotiated_capabilities = session_key
            .as_ref()
            .and_then(|key| self.state.sessions.get(key))
            .map(|session| session.capabilities)
            .unwrap_or_else(|| capabilities(false));
        let Some(document) = self.state.documents.get_mut(&document_id) else {
            return;
        };
        let (error_count, warning_count) = self
            .state
            .diagnostics
            .snapshot(None)
            .items
            .iter()
            .filter(|diagnostic| diagnostic.uri == document.uri)
            .fold(
                (0_u32, 0_u32),
                |(errors, warnings), diagnostic| match diagnostic.severity {
                    DiagnosticSeverity::Error => (errors.saturating_add(1), warnings),
                    DiagnosticSeverity::Warning => (errors, warnings.saturating_add(1)),
                    DiagnosticSeverity::Information | DiagnosticSeverity::Hint => {
                        (errors, warnings)
                    }
                },
            );
        let status = LspStatus {
            revision: self.status_revision,
            document_id: Some(document_id_text(document_id)),
            session_id,
            adapter_id: document.adapter_id.clone(),
            project_root_uri: session_key
                .as_ref()
                .and_then(|key| lsp::Url::from_file_path(&key.root).ok())
                .map(|url| url.to_string()),
            state,
            message,
            unavailable_reason,
            capabilities: if matches!(state, LspSessionState::Ready | LspSessionState::Indexing) {
                negotiated_capabilities
            } else {
                capabilities(false)
            },
            error_count,
            warning_count,
        };
        document.status = status.clone();
        let window_label = document.owner_window.clone();
        self.emit(ManagerEvent::SessionStatus {
            window_label,
            status,
        });
    }

    fn refresh_document_status(&mut self, document_id: DocumentId) {
        let Some(document) = self.state.documents.get(&document_id) else {
            return;
        };
        let status = document.status.clone();
        self.set_document_status(
            document_id,
            status.state,
            document.session_key.clone(),
            status.message,
            status.unavailable_reason,
        );
    }

    fn blank_status(&self, document_id: DocumentId, adapter_id: Option<&str>) -> LspStatus {
        LspStatus {
            revision: self.status_revision,
            document_id: Some(document_id_text(document_id)),
            session_id: None,
            adapter_id: adapter_id.map(str::to_owned),
            project_root_uri: None,
            state: LspSessionState::ChoosingProject,
            message: None,
            unavailable_reason: None,
            capabilities: capabilities(false),
            error_count: 0,
            warning_count: 0,
        }
    }

    fn emit(&self, event: ManagerEvent) {
        debug_assert!(!matches!(event, ManagerEvent::DocumentOwnerFocused { .. }));
        let _ = self.event_tx.try_send(event);
    }

    fn reserve_focus_obligation(&mut self, key: FocusKey) -> Result<(), ManagerError> {
        if self.focus_deliveries.contains_key(&key) {
            return Ok(());
        }
        if self.focus_deliveries.len() >= PUBLIC_EVENT_CAPACITY {
            return Err(ManagerError::Overloaded);
        }
        self.focus_deliveries.insert(key, FocusDelivery::Awaiting);
        self.focus_order.push_back(key);
        Ok(())
    }

    fn queue_focus_event(
        &mut self,
        key: FocusKey,
        event: ManagerEvent,
    ) -> Result<(), ManagerError> {
        match self.focus_deliveries.get_mut(&key) {
            Some(FocusDelivery::Delivering(queued)) => *queued = Some(event),
            Some(delivery) => *delivery = FocusDelivery::Ready(event),
            None => {
                if self.focus_deliveries.len() >= PUBLIC_EVENT_CAPACITY {
                    return Err(ManagerError::Overloaded);
                }
                self.focus_deliveries
                    .insert(key, FocusDelivery::Ready(event));
                self.focus_order.push_back(key);
            }
        }
        self.pump_focus_deliveries();
        Ok(())
    }

    fn fulfill_focus_obligation(&mut self, key: FocusKey, event: ManagerEvent) {
        if let Some(delivery) = self.focus_deliveries.get_mut(&key) {
            *delivery = FocusDelivery::Ready(event);
            self.pump_focus_deliveries();
        }
    }

    fn pump_focus_deliveries(&mut self) {
        if self.focus_in_flight.is_some() || self.shutting_down {
            return;
        }
        let Some(position) = self.focus_order.iter().position(|key| {
            matches!(
                self.focus_deliveries.get(key),
                Some(FocusDelivery::Ready(_))
            )
        }) else {
            return;
        };
        let key = self.focus_order.remove(position).expect("position exists");
        let event = match self.focus_deliveries.remove(&key) {
            Some(FocusDelivery::Ready(event)) => event,
            _ => unreachable!("ready focus delivery selected"),
        };
        self.focus_deliveries
            .insert(key, FocusDelivery::Delivering(None));
        self.focus_in_flight = Some(key);
        let public_events = self.event_tx.clone();
        let input = self.input_tx.clone();
        let cancellation = self.event_dispatch_cancel.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = cancellation.cancelled() => {}
                _ = public_events.send(event) => {}
            }
            let _ = input.send(ActorInput::FocusDelivered { key }).await;
        });
    }
}

impl LspManager {
    fn handle_input(&mut self, input: ActorInput) {
        match input {
            ActorInput::SessionStarted {
                key,
                generation,
                result,
            } => self.session_started(key, generation, result),
            ActorInput::ClientEvent {
                key,
                generation,
                event,
            } => self.client_event(key, generation, event),
            ActorInput::SessionOperationFailed {
                key,
                generation,
                operation,
                message,
            } => {
                if self
                    .state
                    .sessions
                    .get(&key)
                    .is_some_and(|session| session.generation == generation)
                {
                    self.push_log(
                        &key,
                        "protocol",
                        format!("{operation} failed ({} bytes redacted)", message.len()),
                    );
                    if matches!(operation, "didOpen" | "didChange" | "didSave" | "didClose") {
                        let documents = self
                            .state
                            .sessions
                            .get(&key)
                            .map(|session| session.documents.iter().copied().collect::<Vec<_>>())
                            .unwrap_or_default();
                        for document in documents {
                            if let Some(document) = self.state.documents.get_mut(&document) {
                                document.synchronization_dirty = true;
                            }
                        }
                    }
                }
                self.retry_pending_policy_command();
            }
            ActorInput::RequestCompleted { request_id, result } => {
                self.request_completed(request_id, result)
            }
            ActorInput::PullDiagnosticsCompleted {
                key,
                generation,
                document_id,
                uri,
                source_version,
                previous_result_id,
                pull_generation,
                result,
            } => self.pull_diagnostics_completed(
                key,
                generation,
                document_id,
                uri,
                source_version,
                previous_result_id,
                pull_generation,
                result,
            ),
            ActorInput::BatchDue {
                document_id,
                generation,
            } => {
                if self
                    .state
                    .documents
                    .get(&document_id)
                    .is_some_and(|document| document.batch_generation == generation)
                {
                    let _ = self.flush_document(document_id);
                }
            }
            ActorInput::IdleExpired { key, generation } => {
                let expires = self.state.sessions.get(&key).is_some_and(|session| {
                    session.idle_generation == generation && session.documents.is_empty()
                });
                if expires {
                    self.stop_session(&key);
                }
            }
            ActorInput::RestartDue { key, generation } => {
                if self.active_generations.contains(&(key.clone(), generation)) {
                    if let Some(session) = self.state.sessions.get_mut(&key)
                        && session.generation == generation
                        && !session.automatic_restart_blocked
                    {
                        session.restart_after_stop = true;
                    }
                    return;
                }
                let restart = self.state.sessions.get(&key).is_some_and(|session| {
                    session.generation == generation
                        && session.worker.is_none()
                        && !session.documents.is_empty()
                        && !session.automatic_restart_blocked
                });
                if restart {
                    self.start_session(key);
                }
            }
            ActorInput::ReservationExpired { reservation_id } => {
                let expired = self
                    .reservations
                    .get(&reservation_id)
                    .is_some_and(|record| tokio::time::Instant::now() >= record.expires_at);
                if expired {
                    self.state.ownership.release(reservation_id);
                    self.finalize_reservation(reservation_id, None, None);
                }
            }
            ActorInput::ShutdownFinished { key, generation } => {
                self.generation_finished(key, generation);
                self.retry_pending_policy_command();
            }
            ActorInput::WorkerReady { key, generation } => {
                let current = self
                    .state
                    .sessions
                    .get(&key)
                    .is_some_and(|session| session.generation == generation);
                if current {
                    self.pump_session(&key);
                    let documents = self
                        .state
                        .sessions
                        .get(&key)
                        .map(|session| session.documents.iter().copied().collect::<Vec<_>>())
                        .unwrap_or_default();
                    for document in documents {
                        let _ = self.flush_document(document);
                    }
                    self.pump_session(&key);
                    self.retry_pending_policy_command();
                }
            }
            ActorInput::CacheDeletionFinished { reply, result } => {
                if let Err(error) = &result {
                    log::warn!("LSP cache removal failed: {error}");
                }
                let _ = reply.send(result);
                self.policy_async_in_flight = false;
                self.retry_policy_requested = !self.pending_policy_commands.is_empty();
            }
            ActorInput::FocusDelivered { key } => {
                self.focus_in_flight = None;
                match self.focus_deliveries.remove(&key) {
                    Some(FocusDelivery::Delivering(Some(event))) => {
                        self.focus_deliveries
                            .insert(key, FocusDelivery::Ready(event));
                        self.focus_order.push_back(key);
                    }
                    Some(FocusDelivery::Delivering(None)) | None => {}
                    Some(other) => {
                        self.focus_deliveries.insert(key, other);
                    }
                }
                self.pump_focus_deliveries();
            }
        }
    }

    fn session_started(
        &mut self,
        key: SessionKey,
        generation: u64,
        result: Result<Arc<dyn SessionClient>, ManagerError>,
    ) {
        let current = !self.shutting_down
            && self
                .state
                .sessions
                .get(&key)
                .is_some_and(|session| session.generation == generation);
        if !current {
            if let Ok(client) = result {
                let input = self.input_tx.clone();
                let shutdown_key = key.clone();
                tokio::spawn(async move {
                    let _ = client.shutdown().await;
                    let _ = input
                        .send(ActorInput::ShutdownFinished {
                            key: shutdown_key,
                            generation,
                        })
                        .await;
                });
            } else {
                self.generation_finished(key, generation);
            }
            return;
        }
        match result {
            Ok(client) => {
                let negotiated_capabilities = client.capabilities();
                let worker =
                    spawn_session_worker(key.clone(), generation, client, self.input_tx.clone());
                let documents = {
                    let session = self.state.sessions.get_mut(&key).expect("current session");
                    session.startup_cancel = None;
                    session.worker = Some(worker);
                    session.exit_observed = false;
                    session.capabilities = negotiated_capabilities;
                    session.protocol_started = true;
                    session.documents.iter().copied().collect::<Vec<_>>()
                };
                for document in documents {
                    if self.enqueue_open(document, &key).is_ok() {
                        if let Some(managed) = self.state.documents.get_mut(&document) {
                            managed.pending_batches.clear();
                            managed.batch_generation = managed.batch_generation.saturating_add(1);
                            managed.synchronization_dirty = false;
                        }
                        self.set_document_status(
                            document,
                            LspSessionState::Ready,
                            Some(key.clone()),
                            None,
                            None,
                        );
                    } else {
                        if let Some(managed) = self.state.documents.get_mut(&document) {
                            managed.synchronization_dirty = true;
                        }
                        self.set_document_status(
                            document,
                            LspSessionState::Failed,
                            Some(key.clone()),
                            Some("Language server synchronization requires a resync".into()),
                            None,
                        );
                    }
                }
                self.pump_session(&key);
            }
            Err(error) => {
                self.active_generations.remove(&(key.clone(), generation));
                let (state, unavailable) = match &error {
                    ManagerError::Unavailable(reason) => {
                        (LspSessionState::Unavailable, Some(reason.clone()))
                    }
                    _ => (LspSessionState::Failed, None),
                };
                let documents = self
                    .state
                    .sessions
                    .get(&key)
                    .map(|session| session.documents.iter().copied().collect::<Vec<_>>())
                    .unwrap_or_default();
                self.push_log(&key, "startup", "session startup failed".into());
                if let Some(session) = self.state.sessions.get_mut(&key) {
                    session.startup_cancel = None;
                    session.worker = None;
                    session.protocol_started = false;
                    session.outbox.clear();
                }
                for document in documents {
                    if let Some(document) = self.state.documents.get_mut(&document) {
                        document.synchronization_dirty = true;
                    }
                    self.set_document_status(
                        document,
                        state,
                        Some(key.clone()),
                        Some(match &error {
                            ManagerError::Unavailable(_) => {
                                "Bundled language server is unavailable".into()
                            }
                            _ => "Language server could not start".into(),
                        }),
                        unavailable.clone(),
                    );
                }
                self.retry_pending_policy_command();
            }
        }
    }

    fn client_event(&mut self, key: SessionKey, generation: u64, event: ClientEvent) {
        if !self
            .state
            .sessions
            .get(&key)
            .is_some_and(|session| session.generation == generation)
        {
            return;
        }
        match event {
            ClientEvent::Diagnostics {
                uri,
                version,
                diagnostics,
            } => {
                let Some((document_id, current_version)) =
                    self.state
                        .documents
                        .iter()
                        .find_map(|(document_id, document)| {
                            (document.session_key.as_ref() == Some(&key) && document.uri == uri)
                                .then_some((*document_id, document.text.version()))
                        })
                else {
                    return;
                };
                if version.is_some_and(|incoming| incoming < current_version) {
                    return;
                }
                let session_id = self
                    .state
                    .sessions
                    .get(&key)
                    .map(|session| session.session_id.clone())
                    .unwrap_or_default();
                if let Some(diagnostic_key) = DiagnosticKey::new(session_id.clone(), &uri)
                    && self
                        .state
                        .diagnostics
                        .replace(diagnostic_key, version, diagnostics)
                {
                    self.emit_diagnostics(Some(session_id), Some(document_id), Some(uri.clone()));
                    self.refresh_document_status(document_id);
                }
            }
            ClientEvent::Message { kind, message } => self.push_log(
                &key,
                &kind,
                format!("server message redacted ({} bytes)", message.len()),
            ),
            ClientEvent::Progress { progress, .. } => {
                let state = if matches!(progress, super::client::ProgressPayload::End { .. }) {
                    LspSessionState::Ready
                } else {
                    LspSessionState::Indexing
                };
                let documents = self
                    .state
                    .sessions
                    .get(&key)
                    .map(|session| session.documents.iter().copied().collect::<Vec<_>>())
                    .unwrap_or_default();
                for document in documents {
                    self.set_document_status(document, state, Some(key.clone()), None, None);
                }
            }
            ClientEvent::RegistrationsChanged(_) => {}
            ClientEvent::CapabilitiesChanged(capabilities) => {
                let documents = if let Some(session) = self.state.sessions.get_mut(&key) {
                    session.capabilities = capabilities;
                    session.documents.iter().copied().collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                for document in documents {
                    self.refresh_document_status(document);
                }
            }
            ClientEvent::Stderr { bytes } => {
                self.push_log(&key, "stderr", format!("{bytes} bytes redacted"));
            }
            ClientEvent::Overflow { dropped } => {
                self.push_log(
                    &key,
                    "protocol",
                    format!("{dropped} lower-priority server events were dropped"),
                );
            }
            ClientEvent::ProtocolExited(error) => {
                if let Some(error) = error {
                    self.push_log(
                        &key,
                        "protocol",
                        format!("protocol exited with {} error bytes redacted", error.len()),
                    );
                }
                self.session_crashed(key, generation);
            }
            ClientEvent::ProcessExited { success, code } => {
                self.push_log(
                    &key,
                    "process",
                    format!("server exited (success={success}, code={code:?})"),
                );
                self.session_crashed(key, generation);
            }
        }
    }

    fn generation_finished(&mut self, key: SessionKey, generation: u64) {
        self.active_generations.remove(&(key.clone(), generation));
        let restart =
            self.state.sessions.get(&key).is_some_and(|session| {
                session.generation == generation && session.restart_after_stop
            });
        if restart && !self.shutting_down {
            if let Some(session) = self.state.sessions.get_mut(&key) {
                session.restart_after_stop = false;
                session.worker = None;
                session.startup_cancel = None;
            }
            self.start_session(key.clone());
        }
        if let Some((pending, _)) = self.shutdown_reply.as_mut() {
            pending.remove(&(key, generation));
            if pending.is_empty() {
                self.state.sessions.clear();
                if let Some((_, reply)) = self.shutdown_reply.take() {
                    let _ = reply.send(Ok(()));
                }
                self.terminated = true;
            }
        }
    }

    fn request_completed(&mut self, request_id: u64, result: RequestResult) {
        let Some(pending) = self.pending_requests.remove(&request_id) else {
            return;
        };
        self.latest_requests
            .remove(&(pending.document_id, pending.kind));
        let current = self.state.documents.get(&pending.document_id);
        let fresh = current.is_some_and(|document| {
            document.text.version() == pending.version
                && document.uri == pending.uri
                && document.session_key.as_ref() == Some(&pending.session_key)
                && self
                    .state
                    .sessions
                    .get(&pending.session_key)
                    .is_some_and(|session| session.generation == pending.session_generation)
        });
        if !fresh {
            pending.reply.send_error(ManagerError::StaleResponse);
            return;
        }
        match (pending.reply, result) {
            (PendingReply::Completion(reply), RequestResult::Completion(result)) => {
                let result = result
                    .map_err(ManagerError::Infrastructure)
                    .and_then(|response| {
                        (response.source_version == pending.version)
                            .then_some(response)
                            .ok_or(ManagerError::StaleResponse)
                    });
                let _ = reply.send(result);
            }
            (PendingReply::Hover(reply), RequestResult::Hover(result)) => {
                let result = result
                    .map_err(ManagerError::Infrastructure)
                    .and_then(|response| {
                        (response.source_version == pending.version)
                            .then_some(response)
                            .ok_or(ManagerError::StaleResponse)
                    });
                let _ = reply.send(result);
            }
            (PendingReply::SignatureHelp(reply), RequestResult::SignatureHelp(result)) => {
                let result = result
                    .map_err(ManagerError::Infrastructure)
                    .and_then(|response| {
                        (response.source_version == pending.version)
                            .then_some(response)
                            .ok_or(ManagerError::StaleResponse)
                    });
                let _ = reply.send(result);
            }
            (PendingReply::Definition(reply), RequestResult::Definition(result)) => {
                let result = result
                    .map_err(ManagerError::Infrastructure)
                    .and_then(|response| {
                        (response.source_version == pending.version)
                            .then_some(response)
                            .ok_or(ManagerError::StaleResponse)
                    });
                let _ = reply.send(result);
            }
            (reply, _) => reply.send_error(ManagerError::Infrastructure(
                "language server returned the wrong response kind".into(),
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn pull_diagnostics_completed(
        &mut self,
        key: SessionKey,
        generation: u64,
        document_id: DocumentId,
        uri: String,
        source_version: i32,
        previous_result_id: Option<String>,
        pull_generation: u64,
        result: Result<(Option<String>, Vec<Diagnostic>), String>,
    ) {
        let fresh = self
            .state
            .documents
            .get(&document_id)
            .is_some_and(|document| {
                document.text.version() == source_version
                    && document.pull_generation == pull_generation
                    && document.uri == uri
                    && document.session_key.as_ref() == Some(&key)
                    && self
                        .state
                        .sessions
                        .get(&key)
                        .is_some_and(|session| session.generation == generation)
            });
        if !fresh {
            return;
        }
        let Ok((result_id, diagnostics)) = result else {
            return;
        };
        let unchanged = diagnostics.is_empty()
            && previous_result_id.is_some()
            && result_id == previous_result_id;
        if let Some(document) = self.state.documents.get_mut(&document_id) {
            document.pull_result_id = result_id;
        }
        if unchanged {
            return;
        }
        let session_id = self
            .state
            .sessions
            .get(&key)
            .map(|session| session.session_id.clone())
            .unwrap_or_default();
        if let Some(diagnostic_key) = DiagnosticKey::new(session_id.clone(), &uri)
            && self
                .state
                .diagnostics
                .replace(diagnostic_key, Some(source_version), diagnostics)
        {
            self.emit_diagnostics(Some(session_id), Some(document_id), Some(uri.clone()));
            self.refresh_document_status(document_id);
        }
    }

    fn session_crashed(&mut self, key: SessionKey, generation: u64) {
        let now = tokio::time::Instant::now();
        let (documents, session_id, blocked, restart_generation, backoff) = {
            let Some(session) = self.state.sessions.get_mut(&key) else {
                return;
            };
            if session.generation != generation || session.exit_observed {
                return;
            }
            session.exit_observed = true;
            session.worker = None;
            session.protocol_started = false;
            session.outbox.clear();
            session
                .crash_timestamps
                .retain(|timestamp| now.duration_since(*timestamp) <= CRASH_WINDOW);
            session.crash_timestamps.push_back(now);
            session.automatic_restart_blocked = session.crash_timestamps.len() >= MAX_CRASHES;
            let crash_number = session.crash_timestamps.len();
            let backoff = Duration::from_millis(
                250_u64
                    .saturating_mul(1_u64 << crash_number.saturating_sub(1).min(7))
                    .min(30_000),
            );
            (
                session.documents.iter().copied().collect::<Vec<_>>(),
                session.session_id.clone(),
                session.automatic_restart_blocked,
                session.generation,
                backoff,
            )
        };
        if self.state.diagnostics.clear_session(&session_id) {
            self.emit_diagnostics(Some(session_id.clone()), None, None);
        }
        for document in &documents {
            self.cancel_document_requests(*document, ManagerError::SessionUnavailable);
            self.set_document_status(
                *document,
                LspSessionState::Failed,
                Some(key.clone()),
                Some(if blocked {
                    "Language server crashed repeatedly; use Restart to try again".into()
                } else {
                    "Language server stopped unexpectedly".into()
                }),
                None,
            );
        }
        if !blocked && !documents.is_empty() && self.session_is_still_trusted(&key) {
            let input = self.input_tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(backoff).await;
                let _ = input
                    .send(ActorInput::RestartDue {
                        key,
                        generation: restart_generation,
                    })
                    .await;
            });
        }
        self.retry_pending_policy_command();
    }

    fn restart_session(&mut self, adapter_id: String, root: PathBuf) -> Result<(), ManagerError> {
        let root = canonical_directory(&root)?;
        let key = SessionKey { adapter_id, root };
        let Some(session) = self.state.sessions.get_mut(&key) else {
            return Err(ManagerError::SessionUnavailable);
        };
        session.crash_timestamps.clear();
        session.automatic_restart_blocked = false;
        session.exit_observed = false;
        session.restart_after_stop = true;
        let mut waiting = false;
        if let Some(mut worker) = session.worker.take()
            && let Some(shutdown) = worker.shutdown.take()
        {
            let _ = shutdown.send(());
            waiting = true;
        }
        if let Some(cancellation) = session.startup_cancel.take() {
            cancellation.cancel();
            waiting = true;
        }
        let documents = session.documents.iter().copied().collect::<Vec<_>>();
        if !waiting {
            session.restart_after_stop = false;
            self.start_session(key.clone());
        }
        for document in documents {
            self.set_document_status(
                document,
                LspSessionState::Starting,
                Some(key.clone()),
                None,
                None,
            );
        }
        Ok(())
    }

    fn session_is_still_trusted(&self, key: &SessionKey) -> bool {
        self.state.projects.enablement.enables(&key.adapter_id)
            && self
                .state
                .projects
                .persisted
                .trust_for(&key.root, Some(&key.adapter_id))
                .is_some_and(|record| record.decision == TrustDecision::Trusted)
    }

    fn push_log(&mut self, key: &SessionKey, kind: &str, message: String) {
        let Some(session) = self.state.sessions.get_mut(key) else {
            return;
        };
        let message = truncate_log(message);
        session.logs.push_back(SessionLogEntry {
            sequence: session.next_log_sequence,
            kind: kind.to_owned(),
            message,
        });
        session.next_log_sequence = session.next_log_sequence.saturating_add(1);
        while session.logs.len() > LOG_CAPACITY {
            session.logs.pop_front();
        }
    }

    fn emit_diagnostics(
        &self,
        session_id: Option<String>,
        document_id: Option<DocumentId>,
        uri: Option<String>,
    ) {
        let snapshot = self.state.diagnostics.snapshot(None);
        self.emit(ManagerEvent::DiagnosticsUpdated(DiagnosticUpdate {
            revision: snapshot.revision,
            session_id,
            document_id: document_id.map(document_id_text),
            uri,
            snapshot,
        }));
    }
}

impl LspManager {
    fn stop_disabled_sessions(&mut self) {
        let keys = self
            .state
            .sessions
            .keys()
            .filter(|key| !self.state.projects.enablement.enables(&key.adapter_id))
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            self.stop_session(&key);
        }
    }

    fn stop_session(&mut self, key: &SessionKey) {
        let Some(mut session) = self.state.sessions.remove(key) else {
            return;
        };
        if let Some(mut worker) = session.worker.take()
            && let Some(shutdown) = worker.shutdown.take()
        {
            let _ = shutdown.send(());
        }
        if let Some(cancellation) = session.startup_cancel.take() {
            cancellation.cancel();
        }
        if self.state.diagnostics.clear_session(&session.session_id) {
            self.emit_diagnostics(Some(session.session_id.clone()), None, None);
        }
        for document in session.documents {
            if let Some(managed) = self.state.documents.get_mut(&document)
                && managed.session_key.as_ref() == Some(key)
            {
                managed.session_key = None;
            }
        }
    }

    fn revoke_sessions(&mut self, root: &Path, adapter_id: Option<&str>) {
        let keys = self
            .state
            .sessions
            .keys()
            .filter(|key| {
                key.root == root && adapter_id.is_none_or(|adapter| adapter == key.adapter_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            let documents = self
                .state
                .sessions
                .get(&key)
                .map(|session| session.documents.iter().copied().collect::<Vec<_>>())
                .unwrap_or_default();
            self.stop_session(&key);
            for document in documents {
                self.set_document_status(
                    document,
                    LspSessionState::Untrusted,
                    Some(key.clone()),
                    Some("Project trust was revoked".into()),
                    None,
                );
            }
        }
    }

    fn disposable_cache_plan(&self, root: &Path, adapter_id: Option<&str>) -> CacheDeletionPlan {
        let adapters = adapter_id
            .map(|adapter| vec![adapter.to_owned()])
            .unwrap_or_else(|| {
                [
                    "typescript",
                    "json",
                    "python",
                    "rust",
                    "go",
                    "clangd",
                    "java",
                ]
                .into_iter()
                .map(str::to_owned)
                .collect()
            });
        let mut targets = Vec::new();
        for adapter in adapters {
            if language_for_adapter(&adapter).is_none() {
                continue;
            }
            let project_hash = cache_project_key(&adapter, root);
            targets.push((adapter.clone(), project_hash.clone(), "cache"));
            targets.push((adapter, project_hash, "data"));
        }
        CacheDeletionPlan {
            cache_root: self.cache_root.clone(),
            project_root: root.to_path_buf(),
            targets,
        }
    }

    fn cleanup_reservations_at(&mut self, now: tokio::time::Instant) {
        self.state.ownership.cleanup_expired_at(Instant::now());
        let expired = self
            .reservations
            .iter()
            .filter_map(|(token, record)| (now >= record.expires_at).then_some(*token))
            .collect::<Vec<_>>();
        for token in expired {
            self.state.ownership.release(token);
            self.finalize_reservation(token, None, None);
        }
    }

    fn finalize_reservation(
        &mut self,
        reservation_id: ReservationId,
        document_id: Option<DocumentId>,
        pane_id: Option<String>,
    ) {
        let Some(record) = self.reservations.remove(&reservation_id) else {
            return;
        };
        if record.has_waiter {
            self.fulfill_focus_obligation(
                FocusKey::Reservation(reservation_id),
                ManagerEvent::DocumentOwnerFocused {
                    window_label: record.window_label,
                    document_id,
                    pane_id,
                    reservation_failed: document_id.is_none(),
                },
            );
        }
    }

    fn begin_shutdown(&mut self, reply: oneshot::Sender<Result<(), ManagerError>>) {
        if self.shutdown_reply.is_some() {
            let _ = reply.send(Err(ManagerError::ActorStopped));
            return;
        }
        self.shutting_down = true;
        for command in self.pending_policy_commands.drain(..) {
            command.fail_policy(ManagerError::ActorStopped);
        }
        self.retry_policy_requested = false;
        self.event_dispatch_cancel.cancel();
        self.command_rx.close();
        let documents = self.state.documents.keys().copied().collect::<Vec<_>>();
        for document in documents {
            self.cancel_document_requests(document, ManagerError::Cancelled);
            self.state.ownership.close(document);
        }
        self.state.documents.clear();
        let pending = self.active_generations.clone();
        for session in self.state.sessions.values_mut() {
            if let Some(mut worker) = session.worker.take()
                && let Some(shutdown) = worker.shutdown.take()
            {
                let _ = shutdown.send(());
            } else if let Some(cancellation) = session.startup_cancel.take() {
                cancellation.cancel();
            }
        }
        if pending.is_empty() {
            self.state.sessions.clear();
            let _ = reply.send(Ok(()));
            self.terminated = true;
            return;
        }
        self.shutdown_reply = Some((pending, reply));
    }
}

fn capabilities(ready: bool) -> LspCapabilities {
    LspCapabilities {
        completion: ready,
        hover: ready,
        signature_help: ready,
        definition: ready,
        diagnostics: ready,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn document_id_text(document_id: DocumentId) -> String {
    serde_json::to_value(document_id)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .expect("DocumentId always serializes as a UUID string")
}

fn resolve_language(language_id: &str, path: &Path) -> Result<LanguageId, ManagerError> {
    let normalized = language_id.to_ascii_lowercase();
    let language = match normalized.as_str() {
        "javascript" | "javascriptreact" | "js" | "jsx" => LanguageId::JavaScript,
        "typescript" | "typescriptreact" | "ts" | "tsx" => LanguageId::TypeScript,
        "json" | "jsonc" => LanguageId::Json,
        "python" | "py" => LanguageId::Python,
        "rust" | "rs" => LanguageId::Rust,
        "go" => LanguageId::Go,
        "c" => LanguageId::C,
        "cpp" | "c++" | "cc" | "cxx" => LanguageId::Cpp,
        "java" => LanguageId::Java,
        _ => {
            return BundledServerCatalog::new()
                .file_binding(path)
                .map(|binding| binding.language)
                .ok_or_else(|| ManagerError::InvalidLanguage(language_id.to_owned()));
        }
    };
    Ok(language)
}

fn lsp_language_id(language: LanguageId) -> &'static str {
    match language {
        LanguageId::JavaScript => "javascript",
        LanguageId::TypeScript => "typescript",
        LanguageId::Json => "json",
        LanguageId::Python => "python",
        LanguageId::Rust => "rust",
        LanguageId::Go => "go",
        LanguageId::C => "c",
        LanguageId::Cpp => "cpp",
        LanguageId::Java => "java",
    }
}

fn language_for_adapter(adapter_id: &str) -> Option<LanguageId> {
    match adapter_id {
        "typescript" => Some(LanguageId::TypeScript),
        "json" => Some(LanguageId::Json),
        "python" => Some(LanguageId::Python),
        "rust" => Some(LanguageId::Rust),
        "go" => Some(LanguageId::Go),
        "clangd" => Some(LanguageId::Cpp),
        "java" => Some(LanguageId::Java),
        _ => None,
    }
}

fn cache_project_key(adapter_id: &str, canonical_project_root: &Path) -> String {
    let mut hash = Sha256::new();
    hash.update(adapter_id.as_bytes());
    hash.update([0]);
    hash.update(canonical_project_root.as_os_str().as_encoded_bytes());
    format!("{:x}", hash.finalize())
}

fn canonical_directory(path: &Path) -> Result<PathBuf, ManagerError> {
    let canonical = fs::canonicalize(path).map_err(|_| ManagerError::InvalidProjectRoot)?;
    canonical
        .is_dir()
        .then_some(canonical)
        .ok_or(ManagerError::InvalidProjectRoot)
}

fn project_binding_scope(
    document_path: &Path,
    candidates: &[ProjectCandidate],
) -> Result<PathBuf, ManagerError> {
    if let Some(candidate) = candidates
        .iter()
        .max_by_key(|candidate| Path::new(&candidate.canonical_path).components().count())
    {
        return canonical_directory(Path::new(&candidate.canonical_path));
    }
    document_path
        .parent()
        .ok_or(ManagerError::InvalidProjectRoot)
        .and_then(canonical_directory)
}

fn truncate_log(mut message: String) -> String {
    message = message.replace(['\r', '\n'], " ");
    if message.len() <= MAX_LOG_ENTRY_BYTES {
        return message;
    }
    let mut boundary = MAX_LOG_ENTRY_BYTES;
    while !message.is_char_boundary(boundary) {
        boundary -= 1;
    }
    message.truncate(boundary);
    message.push('…');
    message
}

#[cfg(unix)]
fn ensure_cache_device(expected: libc::dev_t, actual: libc::dev_t) -> Result<(), ManagerError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ManagerError::Infrastructure(
            "refused to cross a cache filesystem mount boundary".into(),
        ))
    }
}

#[cfg(unix)]
fn remove_owned_cache_directory_anchored(
    cache_root: &CacheRootAnchor,
    adapter_id: &str,
    project_hash: &str,
    disposable_name: &str,
) -> Result<(), ManagerError> {
    remove_owned_cache_directory_anchored_with_hook(
        cache_root,
        adapter_id,
        project_hash,
        disposable_name,
        || {},
    )
}

#[cfg(unix)]
fn remove_owned_cache_directory_anchored_with_hook(
    cache_root: &CacheRootAnchor,
    adapter_id: &str,
    project_hash: &str,
    disposable_name: &str,
    before_identity_validation: impl FnOnce(),
) -> Result<(), ManagerError> {
    use std::ffi::{CStr, CString, OsStr};
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    fn c_name(name: &OsStr) -> Result<CString, ManagerError> {
        CString::new(name.as_bytes())
            .map_err(|_| ManagerError::Infrastructure("refused cache path containing NUL".into()))
    }

    fn open_directory_at(parent: i32, name: &OsStr) -> Result<Option<File>, ManagerError> {
        let name = c_name(name)?;
        let fd = unsafe {
            libc::openat(
                parent,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd >= 0 {
            // SAFETY: `open`/`openat` returned a new owned descriptor.
            return Ok(Some(unsafe { File::from_raw_fd(fd) }));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(ManagerError::Infrastructure(format!(
                "refused symlinked or invalid cache directory: {error}"
            )))
        }
    }

    fn stat_at(parent: i32, name: &CStr) -> Result<libc::stat, ManagerError> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            libc::fstatat(
                parent,
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result == 0 {
            Ok(unsafe { stat.assume_init() })
        } else {
            Err(ManagerError::Infrastructure(
                std::io::Error::last_os_error().to_string(),
            ))
        }
    }

    fn stat_fd(fd: i32) -> Result<libc::stat, ManagerError> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } == 0 {
            Ok(unsafe { stat.assume_init() })
        } else {
            Err(ManagerError::Infrastructure(
                std::io::Error::last_os_error().to_string(),
            ))
        }
    }

    fn same_identity(left: &libc::stat, right: &libc::stat) -> bool {
        left.st_dev == right.st_dev && left.st_ino == right.st_ino
    }

    fn require_device(actual: &libc::stat, expected: libc::dev_t) -> Result<(), ManagerError> {
        ensure_cache_device(expected, actual.st_dev)
    }

    fn remove_contents(directory: &File, expected_device: libc::dev_t) -> Result<(), ManagerError> {
        let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
        if duplicate < 0 {
            return Err(ManagerError::Infrastructure(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let stream = unsafe { libc::fdopendir(duplicate) };
        if stream.is_null() {
            unsafe { libc::close(duplicate) };
            return Err(ManagerError::Infrastructure(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let mut names = Vec::<CString>::new();
        loop {
            let entry = unsafe { libc::readdir(stream) };
            if entry.is_null() {
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if name.to_bytes() != b"." && name.to_bytes() != b".." {
                names.push(name.to_owned());
            }
        }
        unsafe { libc::closedir(stream) };

        for name in names {
            let before = stat_at(directory.as_raw_fd(), &name)?;
            require_device(&before, expected_device)?;
            if before.st_mode & libc::S_IFMT == libc::S_IFDIR {
                let child =
                    open_directory_at(directory.as_raw_fd(), OsStr::from_bytes(name.to_bytes()))?
                        .ok_or_else(|| {
                        ManagerError::Infrastructure("cache entry changed during removal".into())
                    })?;
                let child_stat = stat_fd(child.as_raw_fd())?;
                require_device(&child_stat, expected_device)?;
                if !same_identity(&before, &child_stat) {
                    return Err(ManagerError::Infrastructure(
                        "cache entry changed during removal".into(),
                    ));
                }
                remove_contents(&child, expected_device)?;
                let current = stat_at(directory.as_raw_fd(), &name)?;
                require_device(&current, expected_device)?;
                if !same_identity(&current, &stat_fd(child.as_raw_fd())?) {
                    return Err(ManagerError::Infrastructure(
                        "cache directory changed during removal".into(),
                    ));
                }
                if unsafe {
                    libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
                } != 0
                {
                    return Err(ManagerError::Infrastructure(
                        std::io::Error::last_os_error().to_string(),
                    ));
                }
            } else if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
                return Err(ManagerError::Infrastructure(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        Ok(())
    }

    if language_for_adapter(adapter_id).is_none()
        || project_hash.len() != 64
        || !project_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !matches!(disposable_name, "cache" | "data")
    {
        return Err(ManagerError::Infrastructure(
            "refused to remove an unresolved or non-TermLab cache path".into(),
        ));
    }
    let root = cache_root
        .directory
        .as_deref()
        .ok_or_else(|| cache_root.unavailable())?;
    let root_stat = stat_fd(root.as_raw_fd())?;
    let expected_device = root_stat.st_dev;
    let Some(lsp) = open_directory_at(root.as_raw_fd(), OsStr::new("lsp"))? else {
        return Ok(());
    };
    require_device(&stat_fd(lsp.as_raw_fd())?, expected_device)?;
    let Some(adapter) = open_directory_at(lsp.as_raw_fd(), OsStr::new(adapter_id))? else {
        return Ok(());
    };
    require_device(&stat_fd(adapter.as_raw_fd())?, expected_device)?;
    let Some(project) = open_directory_at(adapter.as_raw_fd(), OsStr::new(project_hash))? else {
        return Ok(());
    };
    require_device(&stat_fd(project.as_raw_fd())?, expected_device)?;
    let Some(target) = open_directory_at(project.as_raw_fd(), OsStr::new(disposable_name))? else {
        return Ok(());
    };
    require_device(&stat_fd(target.as_raw_fd())?, expected_device)?;
    before_identity_validation();
    let target_name = CString::new(disposable_name).expect("fixed cache component");
    if !same_identity(
        &stat_at(project.as_raw_fd(), &target_name)?,
        &stat_fd(target.as_raw_fd())?,
    ) {
        return Err(ManagerError::Infrastructure(
            "cache target changed before removal".into(),
        ));
    }
    remove_contents(&target, expected_device)?;
    if !same_identity(
        &stat_at(project.as_raw_fd(), &target_name)?,
        &stat_fd(target.as_raw_fd())?,
    ) {
        return Err(ManagerError::Infrastructure(
            "cache target changed during removal".into(),
        ));
    }
    if unsafe {
        libc::unlinkat(
            project.as_raw_fd(),
            target_name.as_ptr(),
            libc::AT_REMOVEDIR,
        )
    } != 0
    {
        return Err(ManagerError::Infrastructure(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn remove_owned_cache_directory_anchored(
    _cache_root: &CacheRootAnchor,
    _adapter_id: &str,
    _project_hash: &str,
    _disposable_name: &str,
) -> Result<(), ManagerError> {
    Err(ManagerError::Infrastructure(
        "safe cache removal is unavailable on this platform".into(),
    ))
}

fn spawn_session_worker(
    key: SessionKey,
    generation: u64,
    client: Arc<dyn SessionClient>,
    input: mpsc::Sender<ActorInput>,
) -> SessionWorkerHandle {
    let (operations, mut receiver) = mpsc::channel(SESSION_OPERATION_CAPACITY);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let stopping = CancellationToken::new();
    let shutdown_client = client.clone();
    let shutdown_input = input.clone();
    let shutdown_key = key.clone();
    let shutdown_stopping = stopping.clone();
    tokio::spawn(async move {
        let _ = shutdown_rx.await;
        shutdown_stopping.cancel();
        let _ = shutdown_client.shutdown().await;
        let _ = shutdown_input
            .send(ActorInput::ShutdownFinished {
                key: shutdown_key,
                generation,
            })
            .await;
    });
    tokio::spawn(async move {
        loop {
            let operation = tokio::select! {
                biased;
                _ = stopping.cancelled() => break,
                operation = receiver.recv() => {
                    let Some(operation) = operation else { break };
                    operation
                }
            };
            match operation {
                SessionOperation::DidOpen(document) => {
                    let result = tokio::select! {
                        _ = stopping.cancelled() => break,
                        result = client.did_open(document) => result,
                    };
                    if let Err(message) = result {
                        send_operation_error(&input, &key, generation, "didOpen", message).await;
                    }
                }
                SessionOperation::DidChange(batch) => {
                    let result = tokio::select! {
                        _ = stopping.cancelled() => break,
                        result = client.did_change(batch) => result,
                    };
                    if let Err(message) = result {
                        send_operation_error(&input, &key, generation, "didChange", message).await;
                    }
                }
                SessionOperation::DidSave(document_id) => {
                    let result = tokio::select! {
                        _ = stopping.cancelled() => break,
                        result = client.did_save(&document_id) => result,
                    };
                    if let Err(message) = result {
                        send_operation_error(&input, &key, generation, "didSave", message).await;
                    }
                }
                SessionOperation::DidClose(document_id) => {
                    let result = tokio::select! {
                        _ = stopping.cancelled() => break,
                        result = client.did_close(&document_id) => result,
                    };
                    if let Err(message) = result {
                        send_operation_error(&input, &key, generation, "didClose", message).await;
                    }
                }
                SessionOperation::Request {
                    request_id,
                    kind,
                    document_id,
                    position,
                    source_version,
                    cancellation,
                } => {
                    let client = client.clone();
                    let input = input.clone();
                    let stopping = stopping.clone();
                    tokio::spawn(async move {
                        let request = async {
                            match kind {
                                RequestKind::Completion => RequestResult::Completion(
                                    client
                                        .completion(&document_id, position, source_version)
                                        .await,
                                ),
                                RequestKind::Hover => RequestResult::Hover(
                                    client.hover(&document_id, position, source_version).await,
                                ),
                                RequestKind::SignatureHelp => RequestResult::SignatureHelp(
                                    client
                                        .signature_help(&document_id, position, source_version)
                                        .await,
                                ),
                                RequestKind::Definition => RequestResult::Definition(
                                    client
                                        .definition(&document_id, position, source_version)
                                        .await,
                                ),
                            }
                        };
                        tokio::select! {
                            _ = stopping.cancelled() => {}
                            _ = cancellation.cancelled() => {}
                            result = request => {
                                let _ = input.send(ActorInput::RequestCompleted { request_id, result }).await;
                            }
                        }
                    });
                }
                SessionOperation::PullDiagnostics {
                    document_id,
                    document_id_text,
                    uri,
                    source_version,
                    previous_result_id,
                    pull_generation,
                } => {
                    let client = client.clone();
                    let input = input.clone();
                    let key = key.clone();
                    let stopping = stopping.clone();
                    tokio::spawn(async move {
                        let result = tokio::select! {
                            _ = stopping.cancelled() => return,
                            result = client.pull_diagnostics(&document_id_text, previous_result_id.clone()) => result,
                        };
                        let _ = input
                            .send(ActorInput::PullDiagnosticsCompleted {
                                key,
                                generation,
                                document_id,
                                uri,
                                source_version,
                                previous_result_id,
                                pull_generation,
                                result,
                            })
                            .await;
                    });
                }
            }
            let _ = input
                .send(ActorInput::WorkerReady {
                    key: key.clone(),
                    generation,
                })
                .await;
        }
    });
    SessionWorkerHandle {
        operations,
        shutdown: Some(shutdown_tx),
    }
}

async fn send_operation_error(
    input: &mpsc::Sender<ActorInput>,
    key: &SessionKey,
    generation: u64,
    operation: &'static str,
    message: String,
) {
    let _ = input
        .send(ActorInput::SessionOperationFailed {
            key: key.clone(),
            generation,
            operation,
            message,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_lsp::lsp_types as lsp;
    use async_trait::async_trait;
    use tempfile::TempDir;
    use tokio::sync::{Notify, mpsc};

    use super::{
        Enablement, LspManager, LspManagerHandle, ManagerError, ManagerEvent,
        PUBLIC_EVENT_CAPACITY, ProjectContextChoice, RESERVATION_TTL, SessionClient,
        SessionFactory, SessionKey, SessionStart,
    };
    use crate::lsp::catalog::BundledServerCatalog;
    use crate::lsp::client::ClientEvent;
    use crate::lsp::root::LanguageId;
    use crate::lsp::trust::TrustDecision;
    use crate::lsp::types::{
        ApplyChangesResponse, CompletionResponse, Diagnostic, DiagnosticSeverity, DocumentId,
        EditorPosition, EditorRange, HoverResponse, LspChangeBatch, LspSessionState, LspTextChange,
        ReserveResult,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Observation {
        Open {
            document_id: String,
            version: i32,
        },
        Change {
            document_id: String,
            base: i32,
            next: i32,
        },
        Save(String),
        Close(String),
        Completion(String),
        Hover(String),
        Signature(String),
        Definition(String),
        PullDiagnostics(String),
        Shutdown,
    }

    #[derive(Default)]
    struct FakeFactory {
        launches: Mutex<Vec<SessionStart>>,
        observations: Mutex<HashMap<SessionKey, Vec<Observation>>>,
        events: Mutex<HashMap<SessionKey, mpsc::Sender<ClientEvent>>>,
        block_requests: AtomicBool,
        requests_released: Notify,
        block_shutdown: AtomicBool,
        shutdowns_released: Notify,
        block_changes: AtomicBool,
        changes_released: Notify,
        block_starts: AtomicBool,
        starts_released: Notify,
        block_first_pull: AtomicBool,
        first_pull_released: Notify,
        pull_calls: AtomicUsize,
        fail_next_change: AtomicBool,
        immediate_exit: AtomicBool,
        fail_start: AtomicBool,
    }

    impl FakeFactory {
        fn launch_count(&self, adapter: &str, root: &Path) -> usize {
            let root = root.canonicalize().unwrap();
            self.launches
                .lock()
                .unwrap()
                .iter()
                .filter(|launch| launch.key.adapter_id == adapter && launch.key.root == root)
                .count()
        }

        fn observations(&self, key: &SessionKey) -> Vec<Observation> {
            self.observations
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .unwrap_or_default()
        }

        async fn emit(&self, key: &SessionKey, event: ClientEvent) {
            let sender = self.events.lock().unwrap().get(key).cloned().unwrap();
            sender.send(event).await.unwrap();
        }

        fn hold_requests(&self) {
            self.block_requests.store(true, Ordering::SeqCst);
        }

        fn release_requests(&self) {
            self.block_requests.store(false, Ordering::SeqCst);
            self.requests_released.notify_waiters();
        }

        fn hold_shutdowns(&self) {
            self.block_shutdown.store(true, Ordering::SeqCst);
        }

        fn release_shutdowns(&self) {
            self.block_shutdown.store(false, Ordering::SeqCst);
            self.shutdowns_released.notify_waiters();
        }

        fn hold_changes(&self) {
            self.block_changes.store(true, Ordering::SeqCst);
        }

        fn release_changes(&self) {
            self.block_changes.store(false, Ordering::SeqCst);
            self.changes_released.notify_waiters();
        }

        fn hold_starts(&self) {
            self.block_starts.store(true, Ordering::SeqCst);
        }

        fn release_starts(&self) {
            self.block_starts.store(false, Ordering::SeqCst);
            self.starts_released.notify_waiters();
        }

        fn hold_first_pull(&self) {
            self.block_first_pull.store(true, Ordering::SeqCst);
        }

        fn release_first_pull(&self) {
            self.block_first_pull.store(false, Ordering::SeqCst);
            self.first_pull_released.notify_waiters();
        }

        async fn wait_if_held(&self, held: &AtomicBool, notify: &Notify) {
            while held.load(Ordering::SeqCst) {
                notify.notified().await;
            }
        }

        fn record(&self, key: &SessionKey, observation: Observation) {
            self.observations
                .lock()
                .unwrap()
                .entry(key.clone())
                .or_default()
                .push(observation);
        }
    }

    struct FakeSession {
        factory: Arc<FakeFactory>,
        key: SessionKey,
    }

    #[async_trait]
    impl SessionClient for FakeSession {
        fn capabilities(&self) -> crate::lsp::types::LspCapabilities {
            crate::lsp::types::LspCapabilities {
                completion: true,
                hover: true,
                signature_help: true,
                definition: true,
                diagnostics: true,
            }
        }

        async fn did_open(
            &self,
            document: crate::lsp::session::SessionDocument,
        ) -> Result<(), String> {
            self.factory.record(
                &self.key,
                Observation::Open {
                    document_id: document.document_id,
                    version: document.version,
                },
            );
            Ok(())
        }

        async fn did_change(&self, batch: LspChangeBatch) -> Result<i32, String> {
            self.factory.record(
                &self.key,
                Observation::Change {
                    document_id: batch.document_id,
                    base: batch.base_version,
                    next: batch.next_version,
                },
            );
            self.factory
                .wait_if_held(&self.factory.block_changes, &self.factory.changes_released)
                .await;
            if self.factory.fail_next_change.swap(false, Ordering::SeqCst) {
                return Err("synchronization failed with source text: secret".into());
            }
            Ok(batch.next_version)
        }

        async fn did_save(&self, document_id: &str) -> Result<(), String> {
            self.factory
                .record(&self.key, Observation::Save(document_id.into()));
            Ok(())
        }

        async fn did_close(&self, document_id: &str) -> Result<(), String> {
            self.factory
                .record(&self.key, Observation::Close(document_id.into()));
            Ok(())
        }

        async fn completion(
            &self,
            document_id: &str,
            _position: lsp::Position,
            source_version: i32,
        ) -> Result<CompletionResponse, String> {
            self.factory
                .record(&self.key, Observation::Completion(document_id.into()));
            self.factory
                .wait_if_held(
                    &self.factory.block_requests,
                    &self.factory.requests_released,
                )
                .await;
            Ok(CompletionResponse {
                document_id: document_id.into(),
                source_version,
                is_incomplete: false,
                items: Vec::new(),
            })
        }

        async fn hover(
            &self,
            document_id: &str,
            _position: lsp::Position,
            source_version: i32,
        ) -> Result<HoverResponse, String> {
            self.factory
                .record(&self.key, Observation::Hover(document_id.into()));
            self.factory
                .wait_if_held(
                    &self.factory.block_requests,
                    &self.factory.requests_released,
                )
                .await;
            Ok(HoverResponse {
                document_id: document_id.into(),
                source_version,
                range: None,
                blocks: Vec::new(),
            })
        }

        async fn signature_help(
            &self,
            document_id: &str,
            _position: lsp::Position,
            source_version: i32,
        ) -> Result<crate::lsp::types::SignatureHelpResponse, String> {
            self.factory
                .record(&self.key, Observation::Signature(document_id.into()));
            Ok(crate::lsp::types::SignatureHelpResponse {
                document_id: document_id.into(),
                source_version,
                signatures: Vec::new(),
                active_signature: None,
                active_parameter: None,
            })
        }

        async fn definition(
            &self,
            document_id: &str,
            _position: lsp::Position,
            source_version: i32,
        ) -> Result<crate::lsp::types::DefinitionResponse, String> {
            self.factory
                .record(&self.key, Observation::Definition(document_id.into()));
            Ok(crate::lsp::types::DefinitionResponse {
                document_id: document_id.into(),
                source_version,
                locations: Vec::new(),
            })
        }

        async fn pull_diagnostics(
            &self,
            document_id: &str,
            _previous_result_id: Option<String>,
        ) -> Result<(Option<String>, Vec<Diagnostic>), String> {
            self.factory
                .record(&self.key, Observation::PullDiagnostics(document_id.into()));
            let call = self.factory.pull_calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 && self.factory.block_first_pull.load(Ordering::SeqCst) {
                self.factory.first_pull_released.notified().await;
            }
            Ok((
                Some(format!("result-{call}")),
                vec![diagnostic(if call == 0 { "old pull" } else { "new pull" })],
            ))
        }

        async fn shutdown(&self) -> Result<(), String> {
            self.factory.record(&self.key, Observation::Shutdown);
            self.factory
                .wait_if_held(
                    &self.factory.block_shutdown,
                    &self.factory.shutdowns_released,
                )
                .await;
            Ok(())
        }
    }

    #[async_trait]
    impl SessionFactory for FakeFactory {
        async fn start(
            self: Arc<Self>,
            start: SessionStart,
            events: mpsc::Sender<ClientEvent>,
        ) -> Result<Arc<dyn SessionClient>, ManagerError> {
            self.launches.lock().unwrap().push(start.clone());
            self.wait_if_held(&self.block_starts, &self.starts_released)
                .await;
            if self.fail_start.load(Ordering::SeqCst) {
                return Err(ManagerError::Infrastructure(
                    "server echoed source: const SECRET = true".into(),
                ));
            }
            self.events
                .lock()
                .unwrap()
                .insert(start.key.clone(), events);
            if self.immediate_exit.load(Ordering::SeqCst) {
                let sender = {
                    self.events
                        .lock()
                        .unwrap()
                        .get(&start.key)
                        .cloned()
                        .unwrap()
                };
                sender
                    .send(ClientEvent::ProcessExited {
                        success: false,
                        code: Some(1),
                    })
                    .await
                    .unwrap();
            }
            Ok(Arc::new(FakeSession {
                factory: self,
                key: start.key,
            }))
        }
    }

    struct ManagerHarness {
        _temp: TempDir,
        root: PathBuf,
        config_root: PathBuf,
        cache_root: PathBuf,
        manager: LspManagerHandle,
        factory: Arc<FakeFactory>,
        events: tokio::sync::Mutex<mpsc::Receiver<ManagerEvent>>,
    }

    impl ManagerHarness {
        fn new() -> Self {
            let temp = TempDir::new().unwrap();
            let root = temp.path().join("repo");
            let config = temp.path().join("config");
            let cache_root = temp.path().join("cache");
            std::fs::create_dir_all(&root).unwrap();
            std::fs::create_dir_all(&cache_root).unwrap();
            let factory = Arc::new(FakeFactory::default());
            let (manager, actor, events) = LspManager::new(
                factory.clone(),
                config.clone(),
                cache_root.clone(),
                Enablement::all(),
            );
            tokio::spawn(actor.run());
            Self {
                _temp: temp,
                root,
                config_root: config,
                cache_root,
                manager,
                factory,
                events: tokio::sync::Mutex::new(events),
            }
        }

        fn file(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
            path
        }

        async fn open(&self, path: &Path, window: &str, pane: &str) -> DocumentId {
            let reservation = self
                .manager
                .reserve_document(path.into(), window.into())
                .await
                .unwrap();
            let ReserveResult::Reserved { reservation_id, .. } = reservation else {
                panic!("expected reservation, got {reservation:?}")
            };
            let response = self
                .manager
                .open_document(
                    reservation_id,
                    pane.into(),
                    "let value = 1;".into(),
                    language(path),
                )
                .await
                .unwrap();
            serde_json::from_value(serde_json::Value::String(response.document_id)).unwrap()
        }

        async fn choose_and_trust(&self, document: DocumentId, root: &Path, adapter: &str) {
            self.manager
                .set_project_context(document, ProjectContextChoice::from(root.to_path_buf()))
                .await
                .unwrap();
            self.manager
                .set_project_trust(root.into(), Some(adapter.into()), TrustDecision::Trusted)
                .await
                .unwrap();
            spin().await;
        }

        async fn next_event(&self) -> ManagerEvent {
            self.events.lock().await.recv().await.unwrap()
        }

        async fn next_owner_event(&self) -> ManagerEvent {
            loop {
                let event = self.next_event().await;
                if matches!(event, ManagerEvent::DocumentOwnerFocused { .. }) {
                    return event;
                }
            }
        }
    }

    fn language(path: &Path) -> String {
        match path.extension().and_then(|value| value.to_str()) {
            Some("rs") => "rust",
            _ => "typescript",
        }
        .into()
    }

    fn batch(document_id: DocumentId, base: i32, next: i32, inserted: &str) -> LspChangeBatch {
        LspChangeBatch {
            document_id: serde_json::to_value(document_id)
                .unwrap()
                .as_str()
                .unwrap()
                .into(),
            base_version: base,
            next_version: next,
            changes: vec![LspTextChange {
                from_utf16: 0,
                to_utf16: 0,
                inserted_text: inserted.into(),
            }],
        }
    }

    fn diagnostic(message: &str) -> Diagnostic {
        Diagnostic {
            id: format!("diagnostic-{message}"),
            uri: "file:///placeholder".into(),
            range: EditorRange {
                start: EditorPosition {
                    line: 0,
                    character: 0,
                },
                end: EditorPosition {
                    line: 0,
                    character: 1,
                },
            },
            severity: DiagnosticSeverity::Warning,
            code: None,
            source: Some("fake".into()),
            message: message.into(),
            related_information: Vec::new(),
        }
    }

    fn key(adapter: &str, root: &Path) -> SessionKey {
        SessionKey {
            adapter_id: adapter.into(),
            root: root.canonicalize().unwrap(),
        }
    }

    async fn spin() {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
    }

    async fn saturate_session_outbox(harness: &ManagerHarness, document: DocumentId) -> i32 {
        harness.factory.hold_changes();
        let mut version = 1;
        for next in 2..500 {
            match harness
                .manager
                .apply_changes(document, batch(document, next - 1, next, "pressure"))
                .await
            {
                Ok(ApplyChangesResponse::Applied { version: admitted }) => version = admitted,
                Err(ManagerError::Overloaded) => return version,
                other => panic!("unexpected saturation result: {other:?}"),
            }
            tokio::time::advance(Duration::from_millis(40)).await;
            spin().await;
        }
        panic!("session outbox did not exert bounded backpressure")
    }

    #[tokio::test(start_paused = true)]
    async fn reservation_commits_focuses_duplicates_and_close_releases_ownership() {
        let harness = ManagerHarness::new();
        let path = harness.file("a.ts", "let value = 1;");
        let first = harness
            .manager
            .reserve_document(path.clone(), "main".into())
            .await
            .unwrap();
        let ReserveResult::Reserved { reservation_id, .. } = first else {
            panic!("expected reservation")
        };
        assert_eq!(
            harness
                .manager
                .reserve_document(path.clone(), "popup".into())
                .await
                .unwrap(),
            ReserveResult::FocusPending {
                window_label: "main".into()
            }
        );

        let opened = harness
            .manager
            .open_document(
                reservation_id,
                "pane-a".into(),
                "let value = 1;".into(),
                "typescript".into(),
            )
            .await
            .unwrap();
        let document_id: DocumentId =
            serde_json::from_value(serde_json::Value::String(opened.document_id)).unwrap();
        assert!(matches!(
            harness.next_owner_event().await,
            ManagerEvent::DocumentOwnerFocused { ref window_label, document_id: Some(id), ref pane_id, reservation_failed: false }
                if window_label == "main" && id == document_id && pane_id.as_deref() == Some("pane-a")
        ));
        assert!(matches!(
            harness
                .manager
                .reserve_document(path.clone(), "popup".into())
                .await
                .unwrap(),
            ReserveResult::FocusOwner { document_id: id, .. } if id == document_id
        ));
        assert!(matches!(
            harness.next_owner_event().await,
            ManagerEvent::DocumentOwnerFocused { document_id: Some(id), .. } if id == document_id
        ));

        harness.manager.close_document(document_id).await.unwrap();
        assert!(matches!(
            harness
                .manager
                .reserve_document(path, "popup".into())
                .await
                .unwrap(),
            ReserveResult::Reserved { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn unsupported_local_documents_keep_app_wide_ownership_without_starting_lsp() {
        let harness = ManagerHarness::new();
        let path = harness.file("notes.txt", "plain notes");
        let reserved = harness
            .manager
            .reserve_document(path.clone(), "main".into())
            .await
            .unwrap();
        let ReserveResult::Reserved { reservation_id, .. } = reserved else {
            panic!("expected a fresh reservation")
        };
        let opened = harness
            .manager
            .open_document(
                reservation_id,
                "notes-pane".into(),
                "plain notes".into(),
                "plaintext".into(),
            )
            .await
            .unwrap();
        let document_id: DocumentId =
            serde_json::from_value(serde_json::Value::String(opened.document_id.clone())).unwrap();

        assert_eq!(opened.status.state, LspSessionState::Disabled);
        assert_eq!(opened.status.adapter_id, None);
        assert!(matches!(
            harness
                .manager
                .reserve_document(path, "popup".into())
                .await
                .unwrap(),
            ReserveResult::FocusOwner { document_id: owner, .. } if owner == document_id
        ));
        assert!(harness.factory.launches.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn expired_reused_save_as_reservation_rejects_the_aba_token() {
        let harness = ManagerHarness::new();
        let source = harness.file("source.ts", "let value = 1;");
        let target = harness.root.join("target.ts");
        let document = harness.open(&source, "main", "pane-a").await;
        let first = harness
            .manager
            .reserve_document(target.clone(), "main".into())
            .await
            .unwrap();
        let ReserveResult::Reserved {
            reservation_id: expired,
            ..
        } = first
        else {
            panic!()
        };
        harness
            .manager
            .expire_reservations_for_test()
            .await
            .unwrap();
        let second = harness
            .manager
            .reserve_document(target.clone(), "main".into())
            .await
            .unwrap();
        let ReserveResult::Reserved {
            reservation_id: current,
            ..
        } = second
        else {
            panic!()
        };

        assert_eq!(
            harness
                .manager
                .transfer_document(document, expired, "main".into(), "pane-a".into())
                .await,
            Err(ManagerError::InvalidReservation)
        );
        harness
            .manager
            .transfer_document(document, current, "main".into(), "pane-a".into())
            .await
            .unwrap();
        assert!(matches!(
            harness
                .manager
                .reserve_document(source, "popup".into())
                .await
                .unwrap(),
            ReserveResult::Reserved { .. }
        ));
        assert!(matches!(
            harness
                .manager
                .reserve_document(target, "popup".into())
                .await
                .unwrap(),
            ReserveResult::FocusOwner { document_id, .. } if document_id == document
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn pending_focus_gets_timer_failure_and_successful_transfer_finalization() {
        let harness = ManagerHarness::new();
        let expiring = harness.root.join("expiring.ts");
        let first = harness
            .manager
            .reserve_document(expiring.clone(), "main".into())
            .await
            .unwrap();
        assert!(matches!(first, ReserveResult::Reserved { .. }));
        assert!(matches!(
            harness
                .manager
                .reserve_document(expiring, "popup".into())
                .await
                .unwrap(),
            ReserveResult::FocusPending { .. }
        ));
        tokio::time::advance(RESERVATION_TTL).await;
        spin().await;
        assert!(matches!(
            harness.next_owner_event().await,
            ManagerEvent::DocumentOwnerFocused {
                document_id: None,
                reservation_failed: true,
                ..
            }
        ));

        let source = harness.file("source.ts", "let value = 1;");
        let document = harness.open(&source, "main", "pane").await;
        let target = harness.root.join("target.ts");
        let target_reservation = harness
            .manager
            .reserve_document(target.clone(), "main".into())
            .await
            .unwrap();
        let ReserveResult::Reserved { reservation_id, .. } = target_reservation else {
            panic!()
        };
        assert!(matches!(
            harness
                .manager
                .reserve_document(target, "popup".into())
                .await
                .unwrap(),
            ReserveResult::FocusPending { .. }
        ));
        harness
            .manager
            .transfer_document(document, reservation_id, "main".into(), "pane".into())
            .await
            .unwrap();
        assert!(matches!(
            harness.next_owner_event().await,
            ManagerEvent::DocumentOwnerFocused {
                document_id: Some(owner),
                reservation_failed: false,
                ..
            } if owner == document
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn owner_focus_is_not_dropped_when_the_public_event_channel_is_full() {
        let harness = ManagerHarness::new();
        for index in 0..=PUBLIC_EVENT_CAPACITY {
            let path = harness.file(&format!("plain-{index}.txt"), "plain");
            let _ = harness.open(&path, "main", &format!("pane-{index}")).await;
        }
        let path = harness.root.join("pending.ts");
        let reserved = harness
            .manager
            .reserve_document(path.clone(), "main".into())
            .await
            .unwrap();
        let ReserveResult::Reserved { reservation_id, .. } = reserved else {
            panic!()
        };
        let _ = harness
            .manager
            .reserve_document(path, "popup".into())
            .await
            .unwrap();
        let opened = harness
            .manager
            .open_document(
                reservation_id,
                "owner".into(),
                "let value = 1;".into(),
                "typescript".into(),
            )
            .await
            .unwrap();
        let document: DocumentId =
            serde_json::from_value(serde_json::Value::String(opened.document_id)).unwrap();

        assert!(matches!(
            harness.next_owner_event().await,
            ManagerEvent::DocumentOwnerFocused {
                document_id: Some(owner),
                reservation_failed: false,
                ..
            } if owner == document
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn a_remembered_exact_root_and_trust_share_one_session() {
        let harness = ManagerHarness::new();
        let root = harness.root.clone();
        std::fs::write(root.join("tsconfig.json"), "{}").unwrap();
        let first = harness.file("a.ts", "let a = 1;");
        let second = harness.file("b.ts", "let b = 2;");
        let first_id = harness.open(&first, "main", "pane-a").await;
        assert_eq!(harness.factory.launch_count("typescript", &root), 0);
        harness
            .choose_and_trust(first_id, &root, "typescript")
            .await;
        let _second_id = harness.open(&second, "popup", "pane-b").await;
        spin().await;

        assert_eq!(harness.factory.launch_count("typescript", &root), 1);
        assert_eq!(
            harness
                .factory
                .observations(&key("typescript", &root))
                .iter()
                .filter(|event| matches!(event, Observation::Open { .. }))
                .count(),
            2
        );
    }

    #[tokio::test(start_paused = true)]
    async fn root_wide_revocation_overrides_and_removes_adapter_specific_trust() {
        let harness = ManagerHarness::new();
        let root = harness.root.clone();
        let path = harness.file("a.ts", "let a = 1;");
        let document = harness.open(&path, "main", "pane").await;
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        assert_eq!(harness.factory.launch_count("typescript", &root), 1);

        harness
            .manager
            .set_project_trust(root.clone(), None, TrustDecision::Revoked)
            .await
            .unwrap();
        spin().await;

        assert_eq!(harness.factory.launch_count("typescript", &root), 1);
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Untrusted
        );
        assert!(
            !harness
                .manager
                .trusted_projects()
                .await
                .unwrap()
                .iter()
                .any(|record| record.root == root
                    && record.adapter_id.as_deref() == Some("typescript")
                    && record.decision == TrustDecision::Trusted)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn nested_roots_and_different_adapters_get_separate_sessions() {
        let harness = ManagerHarness::new();
        let outer = harness.root.clone();
        let nested = outer.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let outer_ts = harness.file("outer.ts", "let a = 1;");
        let nested_ts = harness.file("nested/inner.ts", "let b = 2;");
        let outer_rs = harness.file("outer.rs", "fn main() {}");
        let a = harness.open(&outer_ts, "main", "a").await;
        harness.choose_and_trust(a, &outer, "typescript").await;
        let b = harness.open(&nested_ts, "main", "b").await;
        harness.choose_and_trust(b, &nested, "typescript").await;
        let c = harness.open(&outer_rs, "main", "c").await;
        harness.choose_and_trust(c, &outer, "rust").await;

        assert_eq!(harness.factory.launch_count("typescript", &outer), 1);
        assert_eq!(harness.factory.launch_count("typescript", &nested), 1);
        assert_eq!(harness.factory.launch_count("rust", &outer), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn project_context_distinguishes_ambiguous_deferred_disabled_and_remembered() {
        let harness = ManagerHarness::new();
        std::fs::write(harness.root.join("tsconfig.json"), "{}").unwrap();
        let nested = harness.root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("package.json"), "{}").unwrap();
        let path = harness.file("nested/a.ts", "let a = 1;");
        let document = harness.open(&path, "main", "pane").await;
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::ChoosingProject
        );
        harness
            .manager
            .set_project_context(document, ProjectContextChoice::DeferForSession)
            .await
            .unwrap();
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::ChoosingProject
        );
        harness
            .manager
            .set_project_context(document, ProjectContextChoice::Disabled)
            .await
            .unwrap();
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Disabled
        );
        harness
            .manager
            .set_project_context(document, ProjectContextChoice::from(nested.clone()))
            .await
            .unwrap();
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Untrusted
        );
        assert_eq!(harness.factory.launch_count("typescript", &nested), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn project_context_rejects_an_ancestor_that_was_not_discovered() {
        let harness = ManagerHarness::new();
        std::fs::write(harness.root.join("tsconfig.json"), "{}").unwrap();
        let path = harness.file("src/a.ts", "let a = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let undiscovered = harness.root.parent().unwrap().to_path_buf();

        assert_eq!(
            harness
                .manager
                .set_project_context(document, ProjectContextChoice::from(undiscovered))
                .await,
            Err(ManagerError::InvalidProjectRoot)
        );
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Untrusted
        );
    }

    #[tokio::test(start_paused = true)]
    async fn chosen_outer_root_replaces_the_same_document_scope_disabled_record() {
        let harness = ManagerHarness::new();
        std::fs::write(harness.root.join("tsconfig.json"), "{}").unwrap();
        let nested = harness.root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("package.json"), "{}").unwrap();
        let path = harness.file("nested/shadow.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        harness
            .manager
            .set_project_context(document, ProjectContextChoice::Disabled)
            .await
            .unwrap();
        harness
            .manager
            .set_project_context(document, ProjectContextChoice::root(harness.root.clone()))
            .await
            .unwrap();
        harness
            .manager
            .set_project_trust(
                harness.root.clone(),
                Some("typescript".into()),
                TrustDecision::Trusted,
            )
            .await
            .unwrap();
        spin().await;
        assert_eq!(harness.factory.launch_count("typescript", &harness.root), 1);
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Ready
        );
    }

    #[tokio::test(start_paused = true)]
    async fn failed_context_save_cannot_leave_in_memory_launch_authority() {
        let harness = ManagerHarness::new();
        std::fs::write(harness.root.join("tsconfig.json"), "{}").unwrap();
        let path = harness.file("a.ts", "let a = 1;");
        let document = harness.open(&path, "main", "pane").await;
        std::fs::write(&harness.config_root, "not a directory").unwrap();

        assert!(matches!(
            harness
                .manager
                .set_project_context(document, ProjectContextChoice::from(harness.root.clone()))
                .await,
            Err(ManagerError::Infrastructure(_))
        ));
        std::fs::remove_file(&harness.config_root).unwrap();
        std::fs::create_dir(&harness.config_root).unwrap();
        harness
            .manager
            .set_project_trust(
                harness.root.clone(),
                Some("typescript".into()),
                TrustDecision::Trusted,
            )
            .await
            .unwrap();
        spin().await;

        assert_eq!(harness.factory.launch_count("typescript", &harness.root), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_trust_save_cannot_grant_in_memory_launch_authority() {
        let harness = ManagerHarness::new();
        std::fs::write(harness.root.join("tsconfig.json"), "{}").unwrap();
        let path = harness.file("trust-save.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        harness
            .manager
            .set_project_context(document, ProjectContextChoice::root(harness.root.clone()))
            .await
            .unwrap();
        let store = harness.config_root.join("lsp-projects.toml");
        std::fs::remove_file(&store).unwrap();
        std::fs::create_dir(&store).unwrap();
        assert!(matches!(
            harness
                .manager
                .set_project_trust(
                    harness.root.clone(),
                    Some("typescript".into()),
                    TrustDecision::Trusted,
                )
                .await,
            Err(ManagerError::Infrastructure(_))
        ));
        spin().await;
        assert_eq!(harness.factory.launch_count("typescript", &harness.root), 0);
        assert!(
            !harness
                .manager
                .trusted_projects()
                .await
                .unwrap()
                .iter()
                .any(|record| record.decision == TrustDecision::Trusted)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_single_fallback_is_a_clear_but_still_unremembered_root() {
        let harness = ManagerHarness::new();
        let isolated = harness.root.join("isolated");
        std::fs::create_dir_all(&isolated).unwrap();
        let path = harness.file("isolated/a.ts", "let a = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let status = harness
            .manager
            .status_snapshot(Some(document))
            .await
            .unwrap()
            .remove(0);

        assert_eq!(status.state, LspSessionState::Untrusted);
        assert_eq!(
            status.project_root_uri,
            Some(
                lsp::Url::from_file_path(isolated.canonicalize().unwrap())
                    .unwrap()
                    .to_string()
            )
        );
        assert_eq!(harness.factory.launch_count("typescript", &isolated), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn disablement_detaches_and_stops_without_deleting_trust() {
        let harness = ManagerHarness::new();
        let path = harness.file("a.ts", "let a = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        harness
            .manager
            .set_enablement(Enablement::none())
            .await
            .unwrap();
        spin().await;

        assert!(
            harness
                .factory
                .observations(&key("typescript", &root))
                .contains(&Observation::Shutdown)
        );
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Disabled
        );
        assert!(
            harness
                .manager
                .trusted_projects()
                .await
                .unwrap()
                .iter()
                .any(|record| record.root == root.canonicalize().unwrap())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn per_language_disablement_does_not_disable_other_adapters() {
        let harness = ManagerHarness::new();
        let ts_path = harness.file("a.ts", "let a = 1;");
        let rs_path = harness.file("a.rs", "fn main() {}");
        let ts = harness.open(&ts_path, "main", "ts").await;
        let rs = harness.open(&rs_path, "main", "rs").await;
        let root = harness.root.clone();
        let mut enablement = Enablement::all();
        enablement.adapters.remove("typescript");
        harness.manager.set_enablement(enablement).await.unwrap();
        harness.choose_and_trust(ts, &root, "typescript").await;
        harness.choose_and_trust(rs, &root, "rust").await;

        assert_eq!(harness.factory.launch_count("typescript", &root), 0);
        assert_eq!(harness.factory.launch_count("rust", &root), 1);
        assert_eq!(
            harness.manager.status_snapshot(Some(ts)).await.unwrap()[0].state,
            LspSessionState::Disabled
        );
    }

    #[tokio::test(start_paused = true)]
    async fn changes_batch_for_at_most_40ms_and_save_flushes_in_order() {
        let harness = ManagerHarness::new();
        let path = harness.file("a.ts", "let value = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        let session = key("typescript", &root);

        assert_eq!(
            harness
                .manager
                .apply_changes(document, batch(document, 1, 2, "x"))
                .await
                .unwrap(),
            ApplyChangesResponse::Applied { version: 2 }
        );
        tokio::time::advance(std::time::Duration::from_millis(39)).await;
        spin().await;
        assert!(
            !harness
                .factory
                .observations(&session)
                .iter()
                .any(|op| matches!(op, Observation::Change { .. }))
        );
        tokio::time::advance(std::time::Duration::from_millis(1)).await;
        spin().await;
        assert!(
            harness
                .factory
                .observations(&session)
                .iter()
                .any(|op| matches!(
                    op,
                    Observation::Change {
                        base: 1,
                        next: 2,
                        ..
                    }
                ))
        );

        assert_eq!(
            harness
                .manager
                .apply_changes(document, batch(document, 1, 3, "bad"))
                .await
                .unwrap(),
            ApplyChangesResponse::ResyncRequired {
                expected_version: 2,
                received_base_version: 1
            }
        );
        let resynced = harness
            .manager
            .resync_document(document, 7, "let value = 7;".into())
            .await
            .unwrap();
        assert_eq!(resynced.version, 7);
        spin().await;
        let resync_operations = harness.factory.observations(&session);
        let close = resync_operations
            .iter()
            .rposition(|operation| matches!(operation, Observation::Close(_)))
            .unwrap();
        let reopen = resync_operations
            .iter()
            .rposition(|operation| matches!(operation, Observation::Open { version: 7, .. }))
            .unwrap();
        assert!(close < reopen);

        harness
            .manager
            .apply_changes(document, batch(document, 7, 8, "y"))
            .await
            .unwrap();
        harness.manager.did_save(document).await.unwrap();
        spin().await;
        let operations = harness.factory.observations(&session);
        let change = operations
            .iter()
            .position(|op| matches!(op, Observation::Change { next: 8, .. }))
            .unwrap();
        let save = operations
            .iter()
            .position(|op| matches!(op, Observation::Save(_)))
            .unwrap();
        assert!(change < save);
    }

    #[tokio::test(start_paused = true)]
    async fn continuous_edits_do_not_reset_the_original_forty_millisecond_deadline() {
        let harness = ManagerHarness::new();
        let path = harness.file("deadline.ts", "let value = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        let session = key("typescript", &root);

        harness
            .manager
            .apply_changes(document, batch(document, 1, 2, "a"))
            .await
            .unwrap();
        tokio::time::advance(Duration::from_millis(39)).await;
        harness
            .manager
            .apply_changes(document, batch(document, 2, 3, "b"))
            .await
            .unwrap();
        tokio::time::advance(Duration::from_millis(1)).await;
        spin().await;

        let changes = harness
            .factory
            .observations(&session)
            .into_iter()
            .filter(|operation| matches!(operation, Observation::Change { .. }))
            .collect::<Vec<_>>();
        assert_eq!(changes.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn save_as_owner_mismatch_leaves_old_session_attachment_untouched() {
        let harness = ManagerHarness::new();
        let source = harness.file("source.ts", "let value = 1;");
        let target = harness.root.join("target.ts");
        let document = harness.open(&source, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        let session = key("typescript", &root);
        let baseline = harness.factory.observations(&session).len();
        let reserved = harness
            .manager
            .reserve_document(target, "popup".into())
            .await
            .unwrap();
        let ReserveResult::Reserved { reservation_id, .. } = reserved else {
            panic!("expected target reservation")
        };

        assert_eq!(
            harness
                .manager
                .transfer_document(document, reservation_id, "main".into(), "pane".into())
                .await,
            Err(ManagerError::OwnerMismatch)
        );
        spin().await;
        assert!(
            !harness.factory.observations(&session)[baseline..]
                .iter()
                .any(|operation| matches!(operation, Observation::Close(_)))
        );
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Ready
        );
    }

    #[tokio::test(start_paused = true)]
    async fn interactive_requests_flush_cancel_same_kind_and_discard_stale_results() {
        let harness = ManagerHarness::new();
        let path = harness.file("a.ts", "let value = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        harness.factory.hold_requests();
        harness
            .manager
            .apply_changes(document, batch(document, 1, 2, "x"))
            .await
            .unwrap();

        let first = tokio::spawn({
            let manager = harness.manager.clone();
            async move {
                manager
                    .completion(
                        document,
                        EditorPosition {
                            line: 0,
                            character: 0,
                        },
                        None,
                    )
                    .await
            }
        });
        spin().await;
        let second = tokio::spawn({
            let manager = harness.manager.clone();
            async move {
                manager
                    .completion(
                        document,
                        EditorPosition {
                            line: 0,
                            character: 1,
                        },
                        None,
                    )
                    .await
            }
        });
        spin().await;
        assert_eq!(first.await.unwrap(), Err(ManagerError::Cancelled));
        harness.factory.release_requests();
        assert_eq!(second.await.unwrap().unwrap().source_version, 2);

        harness.factory.hold_requests();
        let hover = tokio::spawn({
            let manager = harness.manager.clone();
            async move {
                manager
                    .hover(
                        document,
                        EditorPosition {
                            line: 0,
                            character: 0,
                        },
                    )
                    .await
            }
        });
        spin().await;
        harness
            .manager
            .apply_changes(document, batch(document, 2, 3, "z"))
            .await
            .unwrap();
        harness.factory.release_requests();
        assert_eq!(hover.await.unwrap(), Err(ManagerError::StaleResponse));
    }

    #[tokio::test(start_paused = true)]
    async fn last_document_idles_for_two_minutes_before_shutdown() {
        let harness = ManagerHarness::new();
        let path = harness.file("a.ts", "let value = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        let session = key("typescript", &root);
        harness.manager.close_document(document).await.unwrap();
        tokio::time::advance(std::time::Duration::from_secs(119)).await;
        spin().await;
        assert!(
            !harness
                .factory
                .observations(&session)
                .contains(&Observation::Shutdown)
        );
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        spin().await;
        assert!(
            harness
                .factory
                .observations(&session)
                .contains(&Observation::Shutdown)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn crash_restarts_with_backoff_then_three_crashes_require_manual_restart() {
        let harness = ManagerHarness::new();
        let path = harness.file("a.ts", "let value = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        let session = key("typescript", &root);

        for expected_launches in 2..=3 {
            harness
                .factory
                .emit(
                    &session,
                    ClientEvent::ProcessExited {
                        success: false,
                        code: Some(1),
                    },
                )
                .await;
            for _ in 0..20 {
                if harness
                    .manager
                    .status_snapshot(Some(document))
                    .await
                    .unwrap()[0]
                    .state
                    == LspSessionState::Failed
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
            tokio::time::advance(std::time::Duration::from_secs(1)).await;
            spin().await;
            assert_eq!(
                harness.factory.launch_count("typescript", &root),
                expected_launches
            );
        }
        harness
            .factory
            .emit(
                &session,
                ClientEvent::ProtocolExited(Some("broken pipe".into())),
            )
            .await;
        tokio::time::advance(std::time::Duration::from_secs(60)).await;
        spin().await;
        assert_eq!(harness.factory.launch_count("typescript", &root), 3);
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Failed
        );

        harness
            .manager
            .restart_session("typescript".into(), root.clone())
            .await
            .unwrap();
        spin().await;
        assert_eq!(harness.factory.launch_count("typescript", &root), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn same_version_pull_generation_discards_aba_and_crash_cancels_promptly() {
        let harness = ManagerHarness::new();
        let path = harness.file("pull.ts", "let value = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        harness.factory.hold_first_pull();

        harness.manager.did_save(document).await.unwrap();
        spin().await;
        harness.manager.did_save(document).await.unwrap();
        spin().await;
        let snapshot = harness.manager.problems_snapshot(None).await.unwrap();
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.items[0].message, "new pull");
        harness.factory.release_first_pull();
        spin().await;
        let snapshot = harness.manager.problems_snapshot(None).await.unwrap();
        assert_eq!(snapshot.items[0].message, "new pull");

        harness.factory.hold_requests();
        let hover = tokio::spawn({
            let manager = harness.manager.clone();
            async move {
                manager
                    .hover(
                        document,
                        EditorPosition {
                            line: 0,
                            character: 0,
                        },
                    )
                    .await
            }
        });
        spin().await;
        harness
            .factory
            .emit(
                &key("typescript", &root),
                ClientEvent::ProcessExited {
                    success: false,
                    code: Some(1),
                },
            )
            .await;
        spin().await;
        assert_eq!(hover.await.unwrap(), Err(ManagerError::SessionUnavailable));
    }

    #[tokio::test(start_paused = true)]
    async fn bounded_outbox_retains_admitted_changes_and_resync_recovers_dirty_state() {
        let harness = ManagerHarness::new();
        let path = harness.file("pressure.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        harness.factory.hold_changes();

        let mut last_admitted = 1;
        let mut overloaded_at = None;
        for next in 2..400 {
            let result = harness
                .manager
                .apply_changes(document, batch(document, next - 1, next, "x"))
                .await;
            match result {
                Ok(ApplyChangesResponse::Applied { version }) => last_admitted = version,
                Err(ManagerError::Overloaded) => {
                    overloaded_at = Some(next);
                    break;
                }
                other => panic!("unexpected admission outcome: {other:?}"),
            }
            tokio::time::advance(Duration::from_millis(40)).await;
            spin().await;
        }
        assert!(
            overloaded_at.is_some(),
            "bounded admission must exert backpressure"
        );
        assert_eq!(
            harness.manager.close_document(document).await,
            Err(ManagerError::Overloaded),
            "a close is not acknowledged while its preceding changes cannot be admitted"
        );
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()
                .len(),
            1,
            "failed close must leave ownership and document state intact"
        );
        harness.factory.release_changes();
        for _ in 0..400 {
            spin().await;
        }
        let changes = harness
            .factory
            .observations(&key("typescript", &root))
            .into_iter()
            .filter_map(|observation| match observation {
                Observation::Change { base, next, .. } => Some((base, next)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(changes.len(), (last_admitted - 1) as usize);
        assert!(changes.windows(2).all(|pair| pair[0].1 == pair[1].0));

        harness
            .factory
            .fail_next_change
            .store(true, Ordering::SeqCst);
        harness
            .manager
            .apply_changes(
                document,
                batch(document, last_admitted, last_admitted + 1, "y"),
            )
            .await
            .unwrap();
        tokio::time::advance(Duration::from_millis(40)).await;
        spin().await;
        assert!(matches!(
            harness
                .manager
                .apply_changes(
                    document,
                    batch(document, last_admitted + 1, last_admitted + 2, "z"),
                )
                .await
                .unwrap(),
            ApplyChangesResponse::ResyncRequired { .. }
        ));
        harness
            .manager
            .resync_document(document, last_admitted + 2, "canonical".into())
            .await
            .unwrap();
        assert!(matches!(
            harness
                .manager
                .apply_changes(
                    document,
                    batch(document, last_admitted + 2, last_admitted + 3, "ok"),
                )
                .await
                .unwrap(),
            ApplyChangesResponse::Applied { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn diagnostics_are_authoritative_revisioned_and_cleared_on_crash() {
        let harness = ManagerHarness::new();
        let path = harness.file("a.ts", "let value = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        let session = key("typescript", &root);
        let uri = lsp::Url::from_file_path(path.canonicalize().unwrap())
            .unwrap()
            .to_string();
        harness
            .factory
            .emit(
                &session,
                ClientEvent::Diagnostics {
                    uri: uri.clone(),
                    version: Some(1),
                    diagnostics: vec![Diagnostic {
                        id: "d1".into(),
                        uri: uri.clone(),
                        range: EditorRange {
                            start: EditorPosition {
                                line: 0,
                                character: 0,
                            },
                            end: EditorPosition {
                                line: 0,
                                character: 1,
                            },
                        },
                        severity: DiagnosticSeverity::Warning,
                        code: None,
                        source: Some("fake".into()),
                        message: "warning".into(),
                        related_information: Vec::new(),
                    }],
                },
            )
            .await;
        spin().await;
        let status = harness
            .manager
            .status_snapshot(Some(document))
            .await
            .unwrap();
        assert_eq!(status[0].warning_count, 1);
        let update = loop {
            let event = harness.next_event().await;
            if let ManagerEvent::DiagnosticsUpdated(update) = event {
                break update;
            }
        };
        assert!(update.revision > 0);
        assert_eq!(
            update.document_id,
            serde_json::to_value(document)
                .unwrap()
                .as_str()
                .map(str::to_owned)
        );
        assert_eq!(update.uri.as_deref(), Some(uri.as_str()));
        assert!(
            update
                .session_id
                .as_deref()
                .is_some_and(|id| id.starts_with("typescript-"))
        );
        assert_eq!(update.snapshot.revision, update.revision);
        let before = harness.manager.problems_snapshot(None).await.unwrap();
        assert_eq!(before.items.len(), 1);
        harness
            .factory
            .emit(
                &session,
                ClientEvent::ProcessExited {
                    success: false,
                    code: None,
                },
            )
            .await;
        spin().await;
        let after = harness.manager.problems_snapshot(None).await.unwrap();
        assert!(after.revision > before.revision);
        assert!(after.items.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn closing_one_shared_document_clears_its_uri_and_rejects_late_pushes() {
        let harness = ManagerHarness::new();
        let first_path = harness.file("a.ts", "let a = 1;");
        let second_path = harness.file("b.ts", "let b = 1;");
        let first = harness.open(&first_path, "main", "a").await;
        let root = harness.root.clone();
        harness.choose_and_trust(first, &root, "typescript").await;
        let _second = harness.open(&second_path, "main", "b").await;
        let session = key("typescript", &root);
        let uri = lsp::Url::from_file_path(first_path.canonicalize().unwrap())
            .unwrap()
            .to_string();
        let publication = || ClientEvent::Diagnostics {
            uri: uri.clone(),
            version: Some(1),
            diagnostics: vec![Diagnostic {
                id: "closed".into(),
                uri: uri.clone(),
                range: EditorRange {
                    start: EditorPosition {
                        line: 0,
                        character: 0,
                    },
                    end: EditorPosition {
                        line: 0,
                        character: 1,
                    },
                },
                severity: DiagnosticSeverity::Error,
                code: None,
                source: Some("fake".into()),
                message: "closed document".into(),
                related_information: Vec::new(),
            }],
        };
        harness.factory.emit(&session, publication()).await;
        spin().await;
        assert_eq!(
            harness
                .manager
                .problems_snapshot(None)
                .await
                .unwrap()
                .items
                .len(),
            1
        );

        harness.manager.close_document(first).await.unwrap();
        spin().await;
        assert!(
            harness
                .manager
                .problems_snapshot(None)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        harness.factory.emit(&session, publication()).await;
        spin().await;
        assert!(
            harness
                .manager
                .problems_snapshot(None)
                .await
                .unwrap()
                .items
                .is_empty()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn session_logs_are_bounded_memory_only_and_exclude_document_source() {
        let harness = ManagerHarness::new();
        let secret = "SOURCE_SECRET_9f8b";
        let path = harness.file("a.ts", secret);
        let reservation = harness
            .manager
            .reserve_document(path, "main".into())
            .await
            .unwrap();
        let ReserveResult::Reserved { reservation_id, .. } = reservation else {
            panic!()
        };
        let opened = harness
            .manager
            .open_document(
                reservation_id,
                "pane".into(),
                secret.into(),
                "typescript".into(),
            )
            .await
            .unwrap();
        let document: DocumentId =
            serde_json::from_value(serde_json::Value::String(opened.document_id)).unwrap();
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        let session = key("typescript", &root);
        for index in 0..200 {
            harness
                .factory
                .emit(
                    &session,
                    ClientEvent::Message {
                        kind: "log".into(),
                        message: format!("entry-{index}"),
                    },
                )
                .await;
        }
        spin().await;
        let logs = harness
            .manager
            .session_logs("typescript".into(), root)
            .await
            .unwrap();
        assert_eq!(logs.len(), 128);
        assert!(!format!("{logs:?}").contains(secret));
    }

    #[tokio::test(start_paused = true)]
    async fn startup_failures_redact_arbitrary_server_text_from_status_and_logs() {
        let harness = ManagerHarness::new();
        harness.factory.fail_start.store(true, Ordering::SeqCst);
        let path = harness.file("failure.ts", "const SECRET = true");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        let status = harness
            .manager
            .status_snapshot(Some(document))
            .await
            .unwrap();
        assert_eq!(status[0].state, LspSessionState::Failed);
        assert!(!format!("{status:?}").contains("SECRET"));
        let logs = harness
            .manager
            .session_logs("typescript".into(), root)
            .await
            .unwrap();
        assert!(!format!("{logs:?}").contains("SECRET"));
    }

    #[tokio::test(start_paused = true)]
    async fn status_tracks_negotiated_and_dynamic_capability_snapshots() {
        let harness = ManagerHarness::new();
        let path = harness.file("capabilities.ts", "let value = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        let ready = harness
            .manager
            .status_snapshot(Some(document))
            .await
            .unwrap()
            .remove(0);
        assert!(ready.capabilities.completion);
        let changed = crate::lsp::types::LspCapabilities {
            completion: false,
            hover: true,
            signature_help: false,
            definition: true,
            diagnostics: false,
        };
        harness
            .factory
            .emit(
                &key("typescript", &root),
                ClientEvent::CapabilitiesChanged(changed),
            )
            .await;
        spin().await;
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .capabilities,
            changed
        );
    }

    #[tokio::test(start_paused = true)]
    async fn revocation_stops_session_clears_diagnostics_and_only_removes_owned_cache_dirs() {
        let harness = ManagerHarness::new();
        let path = harness.file("a.ts", "let value = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        let paths = BundledServerCatalog::new()
            .cache_paths(LanguageId::TypeScript, &root, &harness.cache_root)
            .unwrap();
        std::fs::create_dir_all(&paths.cache_dir).unwrap();
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        let unrelated = harness.cache_root.join("keep.txt");
        std::fs::write(&unrelated, "keep").unwrap();

        harness
            .manager
            .set_project_trust(
                root.clone(),
                Some("typescript".into()),
                TrustDecision::Revoked,
            )
            .await
            .unwrap();
        spin().await;
        assert!(!paths.cache_dir.exists());
        assert!(!paths.data_dir.exists());
        assert!(unrelated.exists());
        assert!(
            harness
                .factory
                .observations(&key("typescript", &root))
                .contains(&Observation::Shutdown)
        );
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Untrusted
        );
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn cache_deletion_rejects_final_symlink_without_blocking_actor() {
        use std::os::unix::fs::symlink;

        let harness = ManagerHarness::new();
        let root_a = harness.root.clone();
        let root_b = harness.root.parent().unwrap().join("other-repo");
        std::fs::create_dir_all(&root_b).unwrap();
        let path = harness.file("symlink.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        harness
            .choose_and_trust(document, &root_a, "typescript")
            .await;
        let catalog = BundledServerCatalog::new();
        let paths_a = catalog
            .cache_paths(LanguageId::TypeScript, &root_a, &harness.cache_root)
            .unwrap();
        let paths_b = catalog
            .cache_paths(LanguageId::TypeScript, &root_b, &harness.cache_root)
            .unwrap();
        std::fs::create_dir_all(&paths_b.cache_dir).unwrap();
        std::fs::write(paths_b.cache_dir.join("owned-by-b"), "keep").unwrap();
        std::fs::create_dir_all(paths_a.cache_dir.parent().unwrap()).unwrap();
        symlink(&paths_b.cache_dir, &paths_a.cache_dir).unwrap();

        let revoke = harness.manager.set_project_trust(
            root_a,
            Some("typescript".into()),
            TrustDecision::Revoked,
        );
        let status = harness.manager.status_snapshot(Some(document));
        let (revoke, status) = tokio::join!(revoke, status);
        assert!(matches!(revoke, Err(ManagerError::Infrastructure(_))));
        assert_eq!(status.unwrap()[0].state, LspSessionState::Untrusted);
        assert!(paths_b.cache_dir.join("owned-by-b").exists());
    }

    #[cfg(unix)]
    #[test]
    fn anchored_cache_deletion_rejects_a_deterministic_target_swap() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let cache_root = temp.path().join("cache-root");
        let project_a = temp.path().join("project-a");
        let project_b = temp.path().join("project-b");
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&project_a).unwrap();
        std::fs::create_dir_all(&project_b).unwrap();
        let catalog = BundledServerCatalog::new();
        let paths_a = catalog
            .cache_paths(LanguageId::TypeScript, &project_a, &cache_root)
            .unwrap();
        let paths_b = catalog
            .cache_paths(LanguageId::TypeScript, &project_b, &cache_root)
            .unwrap();
        std::fs::create_dir_all(&paths_a.cache_dir).unwrap();
        std::fs::write(paths_a.cache_dir.join("owned-by-a"), "keep-a").unwrap();
        std::fs::create_dir_all(&paths_b.cache_dir).unwrap();
        std::fs::write(paths_b.cache_dir.join("owned-by-b"), "keep-b").unwrap();
        let project_hash = paths_a
            .cache_dir
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let moved = paths_a.cache_dir.with_file_name("cache-moved");
        let cache_anchor = super::CacheRootAnchor::capture(&cache_root);
        let result = super::remove_owned_cache_directory_anchored_with_hook(
            &cache_anchor,
            "typescript",
            &project_hash,
            "cache",
            || {
                std::fs::rename(&paths_a.cache_dir, &moved).unwrap();
                symlink(&paths_b.cache_dir, &paths_a.cache_dir).unwrap();
            },
        );
        assert!(matches!(result, Err(ManagerError::Infrastructure(_))));
        assert!(moved.join("owned-by-a").exists());
        assert!(paths_b.cache_dir.join("owned-by-b").exists());
    }

    #[cfg(unix)]
    #[test]
    fn cache_deletion_mount_policy_accepts_only_the_anchored_device() {
        assert!(super::ensure_cache_device(7, 7).is_ok());
        assert!(matches!(
            super::ensure_cache_device(7, 8),
            Err(ManagerError::Infrastructure(message)) if message.contains("mount boundary")
        ));
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn cache_deletion_remains_bound_to_the_root_opened_at_manager_creation() {
        let harness = ManagerHarness::new();
        let path = harness.file("anchored-root.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;

        let original_paths = BundledServerCatalog::new()
            .cache_paths(LanguageId::TypeScript, &root, &harness.cache_root)
            .unwrap();
        let project_hash = original_paths
            .cache_dir
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_owned();
        std::fs::create_dir_all(&original_paths.cache_dir).unwrap();
        std::fs::write(original_paths.cache_dir.join("original"), "remove").unwrap();
        let anchored_root = harness.cache_root.with_file_name("cache-root-opened");
        std::fs::rename(&harness.cache_root, &anchored_root).unwrap();
        std::fs::create_dir_all(&harness.cache_root).unwrap();
        let relative_target = PathBuf::from("lsp")
            .join("typescript")
            .join(project_hash)
            .join("cache");
        let replacement_target = harness.cache_root.join(&relative_target);
        std::fs::create_dir_all(&replacement_target).unwrap();
        std::fs::write(replacement_target.join("outside"), "preserve").unwrap();

        harness
            .manager
            .set_project_trust(root, Some("typescript".into()), TrustDecision::Revoked)
            .await
            .unwrap();

        let anchored_target = anchored_root.join(relative_target);
        assert!(!anchored_target.exists());
        assert!(replacement_target.join("outside").exists());
    }

    #[tokio::test(start_paused = true)]
    async fn app_shutdown_starts_all_process_ceilings_concurrently() {
        let harness = ManagerHarness::new();
        let ts_path = harness.file("a.ts", "let value = 1;");
        let rs_path = harness.file("a.rs", "fn main() {}");
        let ts = harness.open(&ts_path, "main", "ts").await;
        let rs = harness.open(&rs_path, "main", "rs").await;
        let root = harness.root.clone();
        harness.choose_and_trust(ts, &root, "typescript").await;
        harness.choose_and_trust(rs, &root, "rust").await;
        harness.factory.hold_shutdowns();
        let shutdown = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.shutdown().await }
        });
        spin().await;
        assert!(
            harness
                .factory
                .observations(&key("typescript", &root))
                .contains(&Observation::Shutdown)
        );
        assert!(
            harness
                .factory
                .observations(&key("rust", &root))
                .contains(&Observation::Shutdown)
        );
        harness.factory.release_shutdowns();
        shutdown.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_cancels_inflight_start_and_closes_actor_and_public_events() {
        let harness = ManagerHarness::new();
        harness.factory.hold_starts();
        let path = harness.file("starting.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .manager
            .set_project_context(document, ProjectContextChoice::root(root.clone()))
            .await
            .unwrap();
        harness
            .manager
            .set_project_trust(root, Some("typescript".into()), TrustDecision::Trusted)
            .await
            .unwrap();
        spin().await;

        harness.manager.shutdown().await.unwrap();
        assert_eq!(
            harness.manager.status_snapshot(None).await,
            Err(ManagerError::ActorStopped)
        );
        let mut events = harness.events.lock().await;
        while events.try_recv().is_ok() {}
        assert!(events.recv().await.is_none());
        harness.factory.release_starts();
    }

    #[tokio::test(start_paused = true)]
    async fn immediate_startup_exit_never_resurrects_a_ready_generation() {
        let harness = ManagerHarness::new();
        harness.factory.immediate_exit.store(true, Ordering::SeqCst);
        let path = harness.file("immediate.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        spin().await;
        let status = harness
            .manager
            .status_snapshot(Some(document))
            .await
            .unwrap();
        assert_eq!(status[0].state, LspSessionState::Failed);
    }

    #[tokio::test(start_paused = true)]
    async fn saturated_context_change_keeps_attachment_and_authority_until_close_is_admitted() {
        let harness = ManagerHarness::new();
        let path = harness.file("context-pressure.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        saturate_session_outbox(&harness, document).await;

        let transition = tokio::spawn({
            let manager = harness.manager.clone();
            async move {
                manager
                    .set_project_context(document, ProjectContextChoice::Disabled)
                    .await
            }
        });
        spin().await;
        assert!(!transition.is_finished());
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Ready
        );
        harness.factory.release_changes();
        transition.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn saturated_global_and_adapter_disables_are_atomic_across_attached_documents() {
        for enablement in [
            Enablement::none(),
            Enablement {
                global: true,
                adapters: ["rust".to_owned()].into_iter().collect(),
            },
        ] {
            let harness = ManagerHarness::new();
            let ts_path = harness.file("enablement-pressure.ts", "x");
            let rs_path = harness.file("enablement-pressure.rs", "fn main() {}");
            let ts = harness.open(&ts_path, "main", "ts").await;
            let rs = harness.open(&rs_path, "main", "rs").await;
            let root = harness.root.clone();
            harness.choose_and_trust(ts, &root, "typescript").await;
            harness.choose_and_trust(rs, &root, "rust").await;
            saturate_session_outbox(&harness, ts).await;

            let transition = tokio::spawn({
                let manager = harness.manager.clone();
                async move { manager.set_enablement(enablement).await }
            });
            spin().await;
            assert!(!transition.is_finished());
            let statuses = harness.manager.status_snapshot(None).await.unwrap();
            assert!(
                statuses
                    .iter()
                    .all(|status| status.state == LspSessionState::Ready)
            );
            assert!(
                !harness
                    .factory
                    .observations(&key("typescript", &root))
                    .contains(&Observation::Shutdown)
            );
            harness.factory.release_changes();
            transition.await.unwrap().unwrap();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn saturated_revoke_preserves_trust_and_attachment_until_all_closes_are_admitted() {
        let harness = ManagerHarness::new();
        let path = harness.file("revoke-pressure.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        saturate_session_outbox(&harness, document).await;

        let revoke = tokio::spawn({
            let manager = harness.manager.clone();
            let root = root.clone();
            async move {
                manager
                    .set_project_trust(root, Some("typescript".into()), TrustDecision::Revoked)
                    .await
            }
        });
        spin().await;
        assert!(!revoke.is_finished());
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Ready
        );
        let projects = harness.manager.trusted_projects().await.unwrap();
        let canonical_root = root.canonicalize().unwrap();
        assert!(
            projects.iter().any(|record| {
                record.root == canonical_root
                    && record.adapter_id.as_deref() == Some("typescript")
                    && record.decision == TrustDecision::Trusted
            }),
            "persisted authority changed after rejected revoke: {projects:?}"
        );
        harness.factory.release_changes();
        revoke.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn deferred_policy_commands_commit_in_invocation_fifo_order() {
        let harness = ManagerHarness::new();
        let path = harness.file("policy-fifo.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        saturate_session_outbox(&harness, document).await;

        let disable = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::none()).await }
        });
        spin().await;
        let enable = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::all()).await }
        });
        spin().await;
        assert!(!disable.is_finished());
        assert!(
            !enable.is_finished(),
            "a later inverse policy must not overtake deferred authority"
        );
        assert!(
            harness.manager.status_snapshot(None).await.is_ok(),
            "a deferred policy queue must not starve unrelated commands"
        );

        harness.factory.release_changes();
        disable.await.unwrap().unwrap();
        enable.await.unwrap().unwrap();
        for _ in 0..100 {
            spin().await;
            if harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state
                == LspSessionState::Ready
            {
                break;
            }
        }
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Ready
        );
    }

    #[tokio::test(start_paused = true)]
    async fn deferred_policy_queue_is_bounded_and_cancelled_callers_do_not_block_the_front() {
        let harness = ManagerHarness::new();
        let path = harness.file("policy-capacity.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        saturate_session_outbox(&harness, document).await;

        let mut pending = Vec::new();
        for _ in 0..=super::COMMAND_CAPACITY {
            pending.push(tokio::spawn({
                let manager = harness.manager.clone();
                async move { manager.set_enablement(Enablement::none()).await }
            }));
        }
        for _ in 0..100 {
            spin().await;
            if pending.iter().any(tokio::task::JoinHandle::is_finished) {
                break;
            }
        }
        let overload = pending
            .iter()
            .position(tokio::task::JoinHandle::is_finished)
            .expect("bounded pending-policy admission must reject overload");
        assert_eq!(
            pending.swap_remove(overload).await.unwrap(),
            Err(ManagerError::Overloaded)
        );
        for command in pending {
            command.abort();
        }
        let successor = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::all()).await }
        });
        assert!(harness.manager.status_snapshot(None).await.is_ok());
        harness.factory.release_changes();
        successor.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn startup_terminal_failure_reconciles_outbox_and_completes_deferred_policy() {
        let harness = ManagerHarness::new();
        let path = harness.file("policy-startup-failure.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        saturate_session_outbox(&harness, document).await;
        let disable = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::none()).await }
        });
        spin().await;
        harness.factory.fail_start.store(true, Ordering::SeqCst);
        harness
            .factory
            .emit(
                &key("typescript", &root),
                ClientEvent::ProcessExited {
                    success: false,
                    code: Some(1),
                },
            )
            .await;
        tokio::time::advance(Duration::from_millis(250)).await;
        spin().await;

        assert!(
            disable.is_finished(),
            "terminal startup failure must not strand a deferred policy reply"
        );
        disable.await.unwrap().unwrap();
        assert_eq!(
            harness
                .manager
                .status_snapshot(Some(document))
                .await
                .unwrap()[0]
                .state,
            LspSessionState::Disabled
        );
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_waits_for_generation_already_stopping_after_a_crash() {
        let harness = ManagerHarness::new();
        let path = harness.file("crash-shutdown.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        harness.factory.hold_shutdowns();
        harness
            .factory
            .emit(
                &key("typescript", &root),
                ClientEvent::ProcessExited {
                    success: false,
                    code: Some(1),
                },
            )
            .await;
        spin().await;

        let shutdown = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.shutdown().await }
        });
        spin().await;
        assert!(!shutdown.is_finished());
        harness.factory.release_shutdowns();
        shutdown.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn duplicate_exit_facts_never_restart_before_the_old_generation_finishes() {
        let harness = ManagerHarness::new();
        let path = harness.file("double-exit.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        harness.factory.hold_shutdowns();
        let session_key = key("typescript", &root);
        harness
            .factory
            .emit(&session_key, ClientEvent::ProtocolExited(None))
            .await;
        harness
            .factory
            .emit(
                &session_key,
                ClientEvent::ProcessExited {
                    success: false,
                    code: Some(1),
                },
            )
            .await;
        spin().await;
        tokio::time::advance(Duration::from_millis(250)).await;
        spin().await;
        assert_eq!(harness.factory.launch_count("typescript", &root), 1);
        harness.factory.release_shutdowns();
        for _ in 0..20 {
            spin().await;
            if harness.factory.launch_count("typescript", &root) == 2 {
                break;
            }
        }
        assert_eq!(harness.factory.launch_count("typescript", &root), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn focus_waiter_admission_is_bounded_and_every_admitted_terminal_event_arrives() {
        let harness = ManagerHarness::new();
        let mut reservations = Vec::new();
        for index in 0..PUBLIC_EVENT_CAPACITY {
            let path = harness.root.join(format!("focus-{index}.ts"));
            let ReserveResult::Reserved { reservation_id, .. } = harness
                .manager
                .reserve_document(path.clone(), "main".into())
                .await
                .unwrap()
            else {
                panic!("fresh path must reserve")
            };
            assert!(matches!(
                harness.manager.reserve_document(path, "alias".into()).await,
                Ok(ReserveResult::FocusPending { .. })
            ));
            reservations.push(reservation_id);
        }
        let overflow = harness.root.join("focus-overflow.ts");
        let ReserveResult::Reserved { reservation_id, .. } = harness
            .manager
            .reserve_document(overflow.clone(), "main".into())
            .await
            .unwrap()
        else {
            panic!("fresh path must reserve")
        };
        assert_eq!(
            harness
                .manager
                .reserve_document(overflow, "alias".into())
                .await,
            Err(ManagerError::Overloaded)
        );
        harness
            .manager
            .release_document(reservation_id)
            .await
            .unwrap();

        for reservation in reservations {
            harness.manager.release_document(reservation).await.unwrap();
        }
        spin().await;
        let mut events = harness.events.lock().await;
        let mut count = 0;
        while let Ok(event) = events.try_recv() {
            if matches!(event, ManagerEvent::DocumentOwnerFocused { .. }) {
                count += 1;
            }
        }
        assert_eq!(count, PUBLIC_EVENT_CAPACITY);
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn focus_waiter_uses_reservation_identity_across_path_aliases() {
        use std::os::unix::fs::symlink;

        let harness = ManagerHarness::new();
        let path = harness.file("focus-identity.ts", "x");
        let alias = harness.root.join("focus-alias.ts");
        symlink(&path, &alias).unwrap();
        let ReserveResult::Reserved { reservation_id, .. } = harness
            .manager
            .reserve_document(path, "main".into())
            .await
            .unwrap()
        else {
            panic!("fresh path must reserve")
        };
        assert_eq!(
            harness
                .manager
                .reserve_document(alias, "popup".into())
                .await
                .unwrap(),
            ReserveResult::FocusPending {
                window_label: "main".into()
            }
        );
        harness
            .manager
            .release_document(reservation_id)
            .await
            .unwrap();
        assert!(matches!(
            harness.next_owner_event().await,
            ManagerEvent::DocumentOwnerFocused {
                ref window_label,
                reservation_failed: true,
                ..
            } if window_label == "main"
        ));
    }
}
