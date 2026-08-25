//! Application-wide ownership, project policy, and language-server routing.
//!
//! Mutable application state lives only in [`LspManager`]. Potentially slow
//! protocol work runs in bounded per-session workers and returns facts through
//! the actor input channel, so the manager never waits for a server response.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_lsp::lsp_types as lsp;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
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
    ApplyChangesResponse, CompletionResponse, DefinitionResponse, Diagnostic, DiagnosticSnapshot,
    DocumentId, EditorPosition, HoverResponse, LspCapabilities, LspChangeBatch, LspSessionState,
    LspStatus, LspUnavailableReason, OpenDocumentResponse, ProjectCandidate, ReservationId,
    ReserveResult, ResyncDocumentResponse, SignatureHelpResponse,
};

const COMMAND_CAPACITY: usize = 64;
const ACTOR_INPUT_CAPACITY: usize = 128;
const SESSION_OPERATION_CAPACITY: usize = 64;
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

    fn enables(&self, adapter_id: &str) -> bool {
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
    DiagnosticsUpdated(DiagnosticSnapshot),
    DocumentOwnerFocused {
        window_label: String,
        document_id: Option<DocumentId>,
        pane_id: Option<String>,
        reservation_failed: bool,
    },
}

#[async_trait]
pub(crate) trait SessionClient: Send + Sync + 'static {
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
        self.send(ManagerCommand::Shutdown { reply }).await?;
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
    Shutdown {
        reply: oneshot::Sender<Result<(), ManagerError>>,
    },
    #[cfg(test)]
    ExpireReservationsForTest {
        reply: oneshot::Sender<Result<(), ManagerError>>,
    },
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
    },
    Shutdown,
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
    ShutdownFinished {
        key: SessionKey,
        generation: u64,
    },
}

struct ReservationRecord {
    canonical_path: PathBuf,
    window_label: String,
    expires_at: Instant,
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
    session_key: Option<SessionKey>,
    selected_root: Option<PathBuf>,
    deferred_for_session: bool,
    pending_batches: Vec<LspChangeBatch>,
    batch_generation: u64,
    pull_result_id: Option<String>,
    status: LspStatus,
}

struct ManagedSession {
    session_id: String,
    language: LanguageId,
    generation: u64,
    worker: Option<mpsc::Sender<SessionOperation>>,
    documents: HashSet<DocumentId>,
    idle_generation: u64,
    crash_timestamps: VecDeque<tokio::time::Instant>,
    automatic_restart_blocked: bool,
    exit_observed: bool,
    logs: VecDeque<SessionLogEntry>,
    next_log_sequence: u64,
}

struct ProjectStore {
    persisted: ProjectTrustStore,
    enablement: Enablement,
}

struct ManagerState {
    ownership: OwnershipRegistry,
    documents: HashMap<DocumentId, ManagedDocument>,
    sessions: HashMap<SessionKey, ManagedSession>,
    diagnostics: DiagnosticStore,
    projects: ProjectStore,
}

pub(crate) struct LspManager {
    state: ManagerState,
    factory: Arc<dyn SessionFactory>,
    cache_root: PathBuf,
    command_rx: mpsc::Receiver<ManagerCommand>,
    input_tx: mpsc::Sender<ActorInput>,
    input_rx: mpsc::Receiver<ActorInput>,
    event_tx: mpsc::Sender<ManagerEvent>,
    reservations: HashMap<ReservationId, ReservationRecord>,
    pending_requests: HashMap<u64, PendingRequest>,
    latest_requests: HashMap<(DocumentId, RequestKind), u64>,
    next_request_id: u64,
    status_revision: u32,
    shutdown_reply: Option<(usize, oneshot::Sender<Result<(), ManagerError>>)>,
}

impl LspManager {
    pub(crate) fn new(
        factory: Arc<dyn SessionFactory>,
        config_dir: PathBuf,
        cache_root: PathBuf,
        enablement: Enablement,
    ) -> (LspManagerHandle, Self, mpsc::Receiver<ManagerEvent>) {
        let (command_tx, command_rx) = mpsc::channel(COMMAND_CAPACITY);
        let (input_tx, input_rx) = mpsc::channel(ACTOR_INPUT_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(PUBLIC_EVENT_CAPACITY);
        let loaded = ProjectTrustStore::load(config_dir);
        if let Some(warning) = loaded.warning {
            log::warn!("LSP project policy was ignored: {}", warning.message);
        }
        let handle = LspManagerHandle {
            commands: command_tx,
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
            cache_root,
            command_rx,
            input_tx,
            input_rx,
            event_tx,
            reservations: HashMap::new(),
            pending_requests: HashMap::new(),
            latest_requests: HashMap::new(),
            next_request_id: 1,
            status_revision: 0,
            shutdown_reply: None,
        };
        (handle, manager, event_rx)
    }

    pub(crate) async fn run(mut self) {
        loop {
            tokio::select! {
                command = self.command_rx.recv() => {
                    let Some(command) = command else { break };
                    self.handle_command(command);
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
            } => {
                let _ = reply.send(self.set_project_context(document_id, context));
            }
            ManagerCommand::SetProjectTrust {
                root,
                adapter_id,
                decision,
                reply,
            } => {
                let _ = reply.send(self.set_project_trust(root, adapter_id, decision));
            }
            ManagerCommand::SetEnablement { enablement, reply } => {
                self.state.projects.enablement = enablement;
                let documents = self.state.documents.keys().copied().collect::<Vec<_>>();
                for document in documents {
                    self.reevaluate_document(document);
                }
                self.stop_disabled_sessions();
                let _ = reply.send(Ok(()));
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
            ManagerCommand::Shutdown { reply } => self.begin_shutdown(reply),
            #[cfg(test)]
            ManagerCommand::ExpireReservationsForTest { reply } => {
                self.cleanup_reservations_at(Instant::now() + RESERVATION_TTL);
                let _ = reply.send(Ok(()));
            }
        }
    }

    fn reserve_document(
        &mut self,
        path: PathBuf,
        window_label: String,
    ) -> Result<ReserveResult, ManagerError> {
        self.cleanup_reservations_at(Instant::now());
        let result = self
            .state
            .ownership
            .reserve(DocumentIdentifier::Local(path.clone()), &window_label)?;
        match &result {
            ReserveResult::Reserved {
                reservation_id,
                canonical_path,
            } => {
                self.reservations.insert(
                    *reservation_id,
                    ReservationRecord {
                        canonical_path: PathBuf::from(canonical_path),
                        window_label,
                        expires_at: Instant::now() + RESERVATION_TTL,
                        has_waiter: false,
                    },
                );
            }
            ReserveResult::FocusPending { window_label } => {
                if let Ok(canonical) = canonicalize_allow_missing(&path)
                    && let Some(reservation) = self
                        .reservations
                        .values_mut()
                        .find(|reservation| reservation.canonical_path == canonical)
                {
                    reservation.has_waiter = true;
                }
                let _ = window_label;
            }
            ReserveResult::FocusOwner {
                document_id,
                window_label,
                pane_id,
            } => {
                self.emit(ManagerEvent::DocumentOwnerFocused {
                    window_label: window_label.clone(),
                    document_id: Some(*document_id),
                    pane_id: Some(pane_id.clone()),
                    reservation_failed: false,
                });
            }
        }
        Ok(result)
    }

    fn release_document(&mut self, reservation_id: ReservationId) -> Result<(), ManagerError> {
        let record = self.reservations.remove(&reservation_id);
        self.state.ownership.release(reservation_id);
        if let Some(record) = record.filter(|record| record.has_waiter) {
            self.emit(ManagerEvent::DocumentOwnerFocused {
                window_label: record.window_label,
                document_id: None,
                pane_id: None,
                reservation_failed: true,
            });
        }
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
            .remove(&reservation_id)
            .ok_or(ManagerError::InvalidReservation)?;
        if Instant::now() >= reservation.expires_at {
            self.state.ownership.release(reservation_id);
            return Err(ManagerError::InvalidReservation);
        }
        let document_id = self.state.ownership.commit(reservation_id, &pane_id)?;
        let path = self
            .state
            .ownership
            .canonical_path(document_id)
            .map(Path::to_path_buf)
            .ok_or(ManagerError::InvalidReservation)?;
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
                return Err(ManagerError::InvalidProjectRoot);
            }
        };
        let id_text = document_id_text(document_id);
        let text = match VersionedDocument::new(&id_text, &contents, 1) {
            Ok(text) => text,
            Err(error) => {
                self.state.ownership.close(document_id);
                return Err(ManagerError::InvalidChange(format!("{error:?}")));
            }
        };
        let candidates = language
            .and_then(|language| discover_project_roots(&path, language).ok())
            .unwrap_or_default();
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
                session_key: None,
                selected_root: None,
                deferred_for_session: false,
                pending_batches: Vec::new(),
                batch_generation: 0,
                pull_result_id: None,
                status,
            },
        );
        if reservation.has_waiter {
            self.emit(ManagerEvent::DocumentOwnerFocused {
                window_label: reservation.window_label,
                document_id: Some(document_id),
                pane_id: Some(pane_id),
                reservation_failed: false,
            });
        }
        self.reevaluate_document(document_id);
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
            .ok_or(ManagerError::InvalidReservation)?;
        if Instant::now() >= reservation.expires_at {
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
        self.flush_document(document_id)?;
        self.detach_document(document_id, false);
        self.state
            .ownership
            .transfer(document_id, target_reservation_id)?;
        let reservation = self
            .reservations
            .remove(&target_reservation_id)
            .ok_or(ManagerError::InvalidReservation)?;
        let language = resolve_language(
            reservation
                .canonical_path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or(""),
            &reservation.canonical_path,
        )
        .ok();
        let catalog = BundledServerCatalog::new();
        let adapter_id =
            language.map(|language| catalog.descriptor(language).adapter_id.to_owned());
        let uri = lsp::Url::from_file_path(&reservation.canonical_path)
            .map_err(|_| ManagerError::InvalidProjectRoot)?
            .to_string();
        let candidates = language
            .and_then(|language| discover_project_roots(&reservation.canonical_path, language).ok())
            .unwrap_or_default();
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
            document.selected_root = None;
            document.deferred_for_session = false;
            document.pull_result_id = None;
        }
        self.reevaluate_document(document_id);
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
        match document.text.apply_batch(batch.clone()) {
            Ok(applied) => {
                document.pending_batches.push(batch);
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
        self.cancel_document_requests(document_id, ManagerError::StaleResponse);
        let document = self
            .state
            .documents
            .get_mut(&document_id)
            .ok_or(ManagerError::UnknownDocument)?;
        document
            .text
            .resync(&contents, version)
            .map_err(|error| ManagerError::InvalidChange(format!("{error:?}")))?;
        document.pending_batches.clear();
        document.batch_generation = document.batch_generation.saturating_add(1);
        if let Some(key) = document.session_key.clone()
            && let Some(worker) = self
                .state
                .sessions
                .get(&key)
                .and_then(|session| session.worker.clone())
        {
            let id = document_id_text(document_id);
            let _ = worker.try_send(SessionOperation::DidClose(id.clone()));
            let _ = worker.try_send(SessionOperation::DidOpen(SessionDocument {
                document_id: id,
                uri: lsp::Url::parse(&document.uri)
                    .map_err(|_| ManagerError::InvalidProjectRoot)?,
                language_id: document
                    .lsp_language_id
                    .clone()
                    .ok_or(ManagerError::SessionUnavailable)?,
                version,
                text: document.text.text(),
            }));
        }
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
            .get(&document_id)
            .ok_or(ManagerError::UnknownDocument)?;
        let Some(key) = document.session_key.clone() else {
            return Ok(());
        };
        let Some(worker) = self
            .state
            .sessions
            .get(&key)
            .and_then(|session| session.worker.clone())
        else {
            return Ok(());
        };
        let id_text = document_id_text(document_id);
        worker
            .try_send(SessionOperation::DidSave(id_text.clone()))
            .map_err(|_| ManagerError::Overloaded)?;
        worker
            .try_send(SessionOperation::PullDiagnostics {
                document_id,
                document_id_text: id_text,
                uri: document.uri.clone(),
                source_version: document.text.version(),
                previous_result_id: document.pull_result_id.clone(),
            })
            .map_err(|_| ManagerError::Overloaded)?;
        Ok(())
    }

    fn close_document(&mut self, document_id: DocumentId) -> Result<(), ManagerError> {
        if !self.state.documents.contains_key(&document_id) {
            self.state.ownership.close(document_id);
            return Ok(());
        }
        self.flush_document(document_id)?;
        self.cancel_document_requests(document_id, ManagerError::Cancelled);
        self.detach_document(document_id, true);
        self.state.documents.remove(&document_id);
        self.state.ownership.close(document_id);
        Ok(())
    }

    fn set_project_context(
        &mut self,
        document_id: DocumentId,
        context: ProjectContextChoice,
    ) -> Result<LspStatus, ManagerError> {
        let (path, parent) = self
            .state
            .documents
            .get(&document_id)
            .map(|document| {
                (
                    document.path.clone(),
                    document.path.parent().map(Path::to_path_buf),
                )
            })
            .ok_or(ManagerError::UnknownDocument)?;
        self.detach_document(document_id, false);
        match context {
            ProjectContextChoice::Root { root } => {
                let root = canonical_directory(Path::new(&root))?;
                if !path.starts_with(&root) {
                    return Err(ManagerError::InvalidProjectRoot);
                }
                self.state
                    .projects
                    .persisted
                    .set_root_binding(&root, RootBinding::Root(root.clone()))
                    .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
                if let Some(document) = self.state.documents.get_mut(&document_id) {
                    document.deferred_for_session = false;
                    document.selected_root = Some(root);
                }
            }
            ProjectContextChoice::Disabled => {
                let scope = parent.ok_or(ManagerError::InvalidProjectRoot)?;
                self.state
                    .projects
                    .persisted
                    .set_root_binding(&scope, RootBinding::Disabled)
                    .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
                if let Some(document) = self.state.documents.get_mut(&document_id) {
                    document.deferred_for_session = false;
                    document.selected_root = None;
                }
            }
            ProjectContextChoice::DeferForSession => {
                if let Some(document) = self.state.documents.get_mut(&document_id) {
                    document.deferred_for_session = true;
                    document.selected_root = None;
                }
            }
        }
        self.state
            .projects
            .persisted
            .save()
            .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
        self.reevaluate_document(document_id);
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
    ) -> Result<(), ManagerError> {
        let root = canonical_directory(&root)?;
        self.state
            .projects
            .persisted
            .set_trust(&root, adapter_id.as_deref(), decision, now_ms())
            .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
        self.state
            .projects
            .persisted
            .save()
            .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
        if decision == TrustDecision::Revoked {
            self.revoke_sessions(&root, adapter_id.as_deref());
            self.remove_disposable_caches(&root, adapter_id.as_deref());
        }
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
        for document in documents {
            self.reevaluate_document(document);
        }
        Ok(())
    }
}

impl LspManager {
    fn reevaluate_document(&mut self, document_id: DocumentId) {
        let Some(document) = self.state.documents.get(&document_id) else {
            return;
        };
        let Some(adapter_id) = document.adapter_id.clone() else {
            self.detach_document(document_id, false);
            self.set_document_status(
                document_id,
                LspSessionState::Disabled,
                None,
                Some("Project features are not available for this language".into()),
                None,
            );
            return;
        };
        let path = document.path.clone();
        let deferred_for_session = document.deferred_for_session;
        let candidates = document.candidates.clone();
        if !self.state.projects.enablement.enables(&adapter_id) {
            self.detach_document(document_id, false);
            self.set_document_status(
                document_id,
                LspSessionState::Disabled,
                None,
                Some("Project features are disabled".into()),
                None,
            );
            return;
        }
        if deferred_for_session {
            self.detach_document(document_id, false);
            self.set_document_status(
                document_id,
                LspSessionState::ChoosingProject,
                None,
                Some("Project selection deferred for this session".into()),
                None,
            );
            return;
        }

        let binding = self.state.projects.persisted.binding_for(&path);
        match binding {
            Some(RootBinding::Disabled) => {
                self.detach_document(document_id, false);
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
                    self.detach_document(document_id, false);
                    self.set_document_status(
                        document_id,
                        LspSessionState::ChoosingProject,
                        None,
                        Some("Choose project context".into()),
                        None,
                    );
                    return;
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
                    self.detach_document(document_id, false);
                    self.set_document_status(
                        document_id,
                        LspSessionState::Untrusted,
                        Some(SessionKey { adapter_id, root }),
                        Some("Trust this project to enable project features".into()),
                        None,
                    );
                    return;
                }
                self.attach_document(document_id, root);
            }
            None => {
                self.detach_document(document_id, false);
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
    }

    fn attach_document(&mut self, document_id: DocumentId, root: PathBuf) {
        let Some(document) = self.state.documents.get(&document_id) else {
            return;
        };
        let (Some(adapter_id), Some(language)) = (document.adapter_id.clone(), document.language)
        else {
            return;
        };
        let key = SessionKey { adapter_id, root };
        let current_key = document.session_key.clone();
        if current_key.as_ref() == Some(&key) {
            return;
        }
        self.detach_document(document_id, false);
        let needs_start = !self.state.sessions.contains_key(&key);
        if needs_start {
            let session_id = format!("{}-{}", key.adapter_id, uuid::Uuid::new_v4());
            self.state.sessions.insert(
                key.clone(),
                ManagedSession {
                    session_id,
                    language,
                    generation: 0,
                    worker: None,
                    documents: HashSet::new(),
                    idle_generation: 0,
                    crash_timestamps: VecDeque::new(),
                    automatic_restart_blocked: false,
                    exit_observed: false,
                    logs: VecDeque::new(),
                    next_log_sequence: 1,
                },
            );
        }
        let (worker, session_state) = {
            let session = self.state.sessions.get_mut(&key).expect("inserted session");
            session.documents.insert(document_id);
            session.idle_generation = session.idle_generation.saturating_add(1);
            (
                session.worker.clone(),
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
        if let Some(worker) = worker {
            let _ = self.enqueue_open(document_id, &worker);
        } else if needs_start {
            self.start_session(key.clone());
        }
        self.set_document_status(document_id, session_state, Some(key), None, None);
    }

    fn detach_document(&mut self, document_id: DocumentId, closing: bool) {
        let key = self
            .state
            .documents
            .get(&document_id)
            .and_then(|document| document.session_key.clone());
        let Some(key) = key else { return };
        if let Some(session) = self.state.sessions.get_mut(&key) {
            if let Some(worker) = session.worker.clone() {
                let _ = worker.try_send(SessionOperation::DidClose(document_id_text(document_id)));
            }
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
    }

    fn start_session(&mut self, key: SessionKey) {
        let Some(session) = self.state.sessions.get_mut(&key) else {
            return;
        };
        if session.documents.is_empty() {
            return;
        }
        session.generation = session.generation.saturating_add(1);
        session.exit_observed = false;
        session.worker = None;
        let generation = session.generation;
        let start = SessionStart {
            key: key.clone(),
            language: session.language,
            session_id: session.session_id.clone(),
        };
        let factory = self.factory.clone();
        let input = self.input_tx.clone();
        tokio::spawn(async move {
            let (client_events, mut client_event_rx) = mpsc::channel(ACTOR_INPUT_CAPACITY);
            let event_input = input.clone();
            let event_key = key.clone();
            tokio::spawn(async move {
                while let Some(event) = client_event_rx.recv().await {
                    if event_input
                        .send(ActorInput::ClientEvent {
                            key: event_key.clone(),
                            generation,
                            event,
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });
            let result = factory.start(start, client_events).await;
            let _ = input
                .send(ActorInput::SessionStarted {
                    key,
                    generation,
                    result,
                })
                .await;
        });
    }

    fn enqueue_open(
        &self,
        document_id: DocumentId,
        worker: &mpsc::Sender<SessionOperation>,
    ) -> Result<(), ManagerError> {
        let document = self
            .state
            .documents
            .get(&document_id)
            .ok_or(ManagerError::UnknownDocument)?;
        let uri = lsp::Url::parse(&document.uri).map_err(|_| ManagerError::InvalidProjectRoot)?;
        worker
            .try_send(SessionOperation::DidOpen(SessionDocument {
                document_id: document_id_text(document_id),
                uri,
                language_id: document
                    .lsp_language_id
                    .clone()
                    .ok_or(ManagerError::SessionUnavailable)?,
                version: document.text.version(),
                text: document.text.text(),
            }))
            .map_err(|_| ManagerError::Overloaded)
    }

    fn flush_document(&mut self, document_id: DocumentId) -> Result<(), ManagerError> {
        let (key, batches) = {
            let document = self
                .state
                .documents
                .get_mut(&document_id)
                .ok_or(ManagerError::UnknownDocument)?;
            document.batch_generation = document.batch_generation.saturating_add(1);
            (
                document.session_key.clone(),
                std::mem::take(&mut document.pending_batches),
            )
        };
        let Some(key) = key else { return Ok(()) };
        let Some(worker) = self
            .state
            .sessions
            .get(&key)
            .and_then(|session| session.worker.clone())
        else {
            return Ok(());
        };
        for batch in batches {
            worker
                .try_send(SessionOperation::DidChange(batch))
                .map_err(|_| ManagerError::Overloaded)?;
        }
        Ok(())
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
        let Some(worker) = session.worker.clone() else {
            reply.send_error(ManagerError::SessionUnavailable);
            return;
        };
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
            version: document.text.version(),
            uri: document.uri.clone(),
            session_key: session_key.clone(),
            session_generation: session.generation,
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
        if worker.try_send(operation).is_err() {
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
        let Some(document) = self.state.documents.get_mut(&document_id) else {
            return;
        };
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
            capabilities: capabilities(state == LspSessionState::Ready),
            error_count: 0,
            warning_count: 0,
        };
        document.status = status.clone();
        let window_label = document.owner_window.clone();
        self.emit(ManagerEvent::SessionStatus {
            window_label,
            status,
        });
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
        let _ = self.event_tx.try_send(event);
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
                    self.push_log(&key, "protocol", format!("{operation} failed: {message}"));
                }
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
                result,
            } => self.pull_diagnostics_completed(
                key,
                generation,
                document_id,
                uri,
                source_version,
                previous_result_id,
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
            ActorInput::ShutdownFinished { key, generation } => {
                let remove = self
                    .state
                    .sessions
                    .get(&key)
                    .is_some_and(|session| session.generation == generation);
                if remove {
                    self.state.sessions.remove(&key);
                }
                if let Some((remaining, _)) = self.shutdown_reply.as_mut() {
                    *remaining = remaining.saturating_sub(1);
                    if *remaining == 0
                        && let Some((_, reply)) = self.shutdown_reply.take()
                    {
                        let _ = reply.send(Ok(()));
                    }
                }
            }
        }
    }

    fn session_started(
        &mut self,
        key: SessionKey,
        generation: u64,
        result: Result<Arc<dyn SessionClient>, ManagerError>,
    ) {
        let current = self
            .state
            .sessions
            .get(&key)
            .is_some_and(|session| session.generation == generation);
        if !current {
            if let Ok(client) = result {
                let input = self.input_tx.clone();
                tokio::spawn(async move {
                    let _ = client.shutdown().await;
                    drop(input);
                });
            }
            return;
        }
        match result {
            Ok(client) => {
                let worker =
                    spawn_session_worker(key.clone(), generation, client, self.input_tx.clone());
                let documents = {
                    let session = self.state.sessions.get_mut(&key).expect("current session");
                    session.worker = Some(worker.clone());
                    session.exit_observed = false;
                    session.documents.iter().copied().collect::<Vec<_>>()
                };
                for document in documents {
                    if self.enqueue_open(document, &worker).is_ok() {
                        if let Some(managed) = self.state.documents.get_mut(&document) {
                            managed.pending_batches.clear();
                            managed.batch_generation = managed.batch_generation.saturating_add(1);
                        }
                        self.set_document_status(
                            document,
                            LspSessionState::Ready,
                            Some(key.clone()),
                            None,
                            None,
                        );
                    }
                }
            }
            Err(error) => {
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
                self.push_log(&key, "startup", error.to_string());
                for document in documents {
                    self.set_document_status(
                        document,
                        state,
                        Some(key.clone()),
                        Some(error.to_string()),
                        unavailable.clone(),
                    );
                }
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
                let stale = self.state.documents.values().any(|document| {
                    document.session_key.as_ref() == Some(&key)
                        && document.uri == uri
                        && version.is_some_and(|incoming| incoming < document.text.version())
                });
                if stale {
                    return;
                }
                let session_id = self
                    .state
                    .sessions
                    .get(&key)
                    .map(|session| session.session_id.clone())
                    .unwrap_or_default();
                if let Some(diagnostic_key) = DiagnosticKey::new(session_id, &uri)
                    && self
                        .state
                        .diagnostics
                        .replace(diagnostic_key, version, diagnostics)
                {
                    self.emit_diagnostics();
                }
            }
            ClientEvent::Message { kind, message } => self.push_log(&key, &kind, message),
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
            ClientEvent::Overflow { dropped } => {
                self.push_log(
                    &key,
                    "protocol",
                    format!("{dropped} lower-priority server events were dropped"),
                );
            }
            ClientEvent::ProtocolExited(error) => {
                if let Some(error) = error {
                    self.push_log(&key, "protocol", error);
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
        result: Result<(Option<String>, Vec<Diagnostic>), String>,
    ) {
        let fresh = self
            .state
            .documents
            .get(&document_id)
            .is_some_and(|document| {
                document.text.version() == source_version
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
        if let Some(diagnostic_key) = DiagnosticKey::new(session_id, &uri)
            && self
                .state
                .diagnostics
                .replace(diagnostic_key, Some(source_version), diagnostics)
        {
            self.emit_diagnostics();
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
            self.emit_diagnostics();
        }
        for document in &documents {
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
    }

    fn restart_session(&mut self, adapter_id: String, root: PathBuf) -> Result<(), ManagerError> {
        let root = canonical_directory(&root)?;
        let key = SessionKey { adapter_id, root };
        let Some(session) = self.state.sessions.get_mut(&key) else {
            return Err(ManagerError::SessionUnavailable);
        };
        if let Some(worker) = session.worker.take() {
            let _ = worker.try_send(SessionOperation::Shutdown);
        }
        session.crash_timestamps.clear();
        session.automatic_restart_blocked = false;
        session.exit_observed = false;
        let documents = session.documents.iter().copied().collect::<Vec<_>>();
        self.start_session(key.clone());
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

    fn emit_diagnostics(&self) {
        self.emit(ManagerEvent::DiagnosticsUpdated(
            self.state.diagnostics.snapshot(None),
        ));
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
        if let Some(worker) = session.worker.take() {
            let _ = worker.try_send(SessionOperation::Shutdown);
        }
        if self.state.diagnostics.clear_session(&session.session_id) {
            self.emit_diagnostics();
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

    fn remove_disposable_caches(&self, root: &Path, adapter_id: Option<&str>) {
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
        for adapter in adapters {
            let Some(language) = language_for_adapter(&adapter) else {
                continue;
            };
            let Ok(paths) =
                BundledServerCatalog::new().cache_paths(language, root, &self.cache_root)
            else {
                continue;
            };
            for path in [paths.cache_dir, paths.data_dir] {
                let _ = remove_owned_cache_directory(&self.cache_root, &adapter, &path);
            }
        }
    }

    fn cleanup_reservations_at(&mut self, now: Instant) {
        self.state.ownership.cleanup_expired_at(now);
        let expired = self
            .reservations
            .iter()
            .filter_map(|(token, record)| (now >= record.expires_at).then_some(*token))
            .collect::<Vec<_>>();
        for token in expired {
            if let Some(record) = self.reservations.remove(&token)
                && record.has_waiter
            {
                self.emit(ManagerEvent::DocumentOwnerFocused {
                    window_label: record.window_label,
                    document_id: None,
                    pane_id: None,
                    reservation_failed: true,
                });
            }
        }
    }

    fn begin_shutdown(&mut self, reply: oneshot::Sender<Result<(), ManagerError>>) {
        if self.shutdown_reply.is_some() {
            let _ = reply.send(Err(ManagerError::ActorStopped));
            return;
        }
        let documents = self.state.documents.keys().copied().collect::<Vec<_>>();
        for document in documents {
            let _ = self.flush_document(document);
            self.cancel_document_requests(document, ManagerError::Cancelled);
            self.detach_document(document, true);
            self.state.ownership.close(document);
        }
        self.state.documents.clear();
        let sessions = self
            .state
            .sessions
            .iter()
            .filter_map(|(key, session)| {
                session
                    .worker
                    .clone()
                    .map(|worker| (key.clone(), session.generation, worker))
            })
            .collect::<Vec<_>>();
        if sessions.is_empty() {
            self.state.sessions.clear();
            let _ = reply.send(Ok(()));
            return;
        }
        self.shutdown_reply = Some((sessions.len(), reply));
        for (_, _, worker) in sessions {
            if worker.try_send(SessionOperation::Shutdown).is_err()
                && let Some((remaining, _)) = self.shutdown_reply.as_mut()
            {
                *remaining = remaining.saturating_sub(1);
            }
        }
        if self
            .shutdown_reply
            .as_ref()
            .is_some_and(|(remaining, _)| *remaining == 0)
            && let Some((_, reply)) = self.shutdown_reply.take()
        {
            let _ = reply.send(Ok(()));
        }
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

fn canonical_directory(path: &Path) -> Result<PathBuf, ManagerError> {
    let canonical = fs::canonicalize(path).map_err(|_| ManagerError::InvalidProjectRoot)?;
    canonical
        .is_dir()
        .then_some(canonical)
        .ok_or(ManagerError::InvalidProjectRoot)
}

fn canonicalize_allow_missing(path: &Path) -> Result<PathBuf, ManagerError> {
    if !path.is_absolute() {
        return Err(ManagerError::InvalidProjectRoot);
    }
    let mut candidate = lexical_normalize(path);
    let mut suffix = Vec::new();
    loop {
        match fs::canonicalize(&candidate) {
            Ok(mut canonical) => {
                for component in suffix.iter().rev() {
                    canonical.push(component);
                }
                return Ok(canonical);
            }
            Err(_) => {
                let name = candidate
                    .file_name()
                    .map(|name| name.to_os_string())
                    .ok_or(ManagerError::InvalidProjectRoot)?;
                suffix.push(name);
                candidate = candidate
                    .parent()
                    .map(Path::to_path_buf)
                    .ok_or(ManagerError::InvalidProjectRoot)?;
            }
        }
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
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

fn remove_owned_cache_directory(
    cache_root: &Path,
    adapter_id: &str,
    target: &Path,
) -> Result<(), ManagerError> {
    if !target.exists() {
        return Ok(());
    }
    let cache_root = fs::canonicalize(cache_root)
        .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
    let target = fs::canonicalize(target)
        .map_err(|error| ManagerError::Infrastructure(error.to_string()))?;
    let owned_adapter_root = cache_root.join("lsp").join(adapter_id);
    let disposable_name = target.file_name().and_then(|name| name.to_str());
    let has_project_key = target
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()));
    if !target.is_dir()
        || !target.starts_with(&owned_adapter_root)
        || !matches!(disposable_name, Some("cache" | "data"))
        || !has_project_key
    {
        return Err(ManagerError::Infrastructure(
            "refused to remove an unresolved or non-TermLab cache path".into(),
        ));
    }
    fs::remove_dir_all(&target).map_err(|error| ManagerError::Infrastructure(error.to_string()))
}

fn spawn_session_worker(
    key: SessionKey,
    generation: u64,
    client: Arc<dyn SessionClient>,
    input: mpsc::Sender<ActorInput>,
) -> mpsc::Sender<SessionOperation> {
    let (operations, mut receiver) = mpsc::channel(SESSION_OPERATION_CAPACITY);
    tokio::spawn(async move {
        while let Some(operation) = receiver.recv().await {
            match operation {
                SessionOperation::DidOpen(document) => {
                    if let Err(message) = client.did_open(document).await {
                        send_operation_error(&input, &key, generation, "didOpen", message).await;
                    }
                }
                SessionOperation::DidChange(batch) => {
                    if let Err(message) = client.did_change(batch).await {
                        send_operation_error(&input, &key, generation, "didChange", message).await;
                    }
                }
                SessionOperation::DidSave(document_id) => {
                    if let Err(message) = client.did_save(&document_id).await {
                        send_operation_error(&input, &key, generation, "didSave", message).await;
                    }
                }
                SessionOperation::DidClose(document_id) => {
                    if let Err(message) = client.did_close(&document_id).await {
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
                } => {
                    let client = client.clone();
                    let input = input.clone();
                    let key = key.clone();
                    tokio::spawn(async move {
                        let result = client
                            .pull_diagnostics(&document_id_text, previous_result_id.clone())
                            .await;
                        let _ = input
                            .send(ActorInput::PullDiagnosticsCompleted {
                                key,
                                generation,
                                document_id,
                                uri,
                                source_version,
                                previous_result_id,
                                result,
                            })
                            .await;
                    });
                }
                SessionOperation::Shutdown => {
                    let _ = client.shutdown().await;
                    let _ = input
                        .send(ActorInput::ShutdownFinished {
                            key: key.clone(),
                            generation,
                        })
                        .await;
                    break;
                }
            }
        }
    });
    operations
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
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use async_lsp::lsp_types as lsp;
    use async_trait::async_trait;
    use tempfile::TempDir;
    use tokio::sync::{Notify, mpsc};

    use super::{
        Enablement, LspManager, LspManagerHandle, ManagerError, ManagerEvent, ProjectContextChoice,
        SessionClient, SessionFactory, SessionKey, SessionStart,
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
            Ok((Some("result-1".into()), Vec::new()))
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
            self.events
                .lock()
                .unwrap()
                .insert(start.key.clone(), events);
            Ok(Arc::new(FakeSession {
                factory: self,
                key: start.key,
            }))
        }
    }

    struct ManagerHarness {
        _temp: TempDir,
        root: PathBuf,
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
                config,
                cache_root.clone(),
                Enablement::all(),
            );
            tokio::spawn(actor.run());
            Self {
                _temp: temp,
                root,
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
                        uri,
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
}
