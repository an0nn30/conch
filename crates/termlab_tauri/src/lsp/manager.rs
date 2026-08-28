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
use super::session::{LspSession, ProcessServerLauncher, SessionDocument, SessionError};
use super::trust::{ProjectTrustStore, RootBinding, TrustDecision};
use super::types::{
    ApplyChangesResponse, CompletionItem, CompletionResponse, DefinitionResponse, Diagnostic,
    DiagnosticSeverity, DiagnosticSnapshot, DiagnosticUpdate, DocumentId, EditorPosition,
    HoverResponse, LspCapabilities, LspChangeBatch, LspSessionState, LspStatus,
    LspUnavailableReason, NegotiatedTriggers, OpenDocumentResponse, ProjectCandidate,
    ReservationId, ReserveResult, ResyncDocumentResponse, SignatureHelpResponse,
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
    PolicyChanged,
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
            Self::PolicyChanged => {
                formatter.write_str("project policy changed; retry the document close")
            }
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
    /// One session ended. Unlike `SessionStatus`, which belongs to the window
    /// that owns a document, this is app-wide: the Problems tool window
    /// aggregates every project and may be popped out of the owning window, so
    /// it has to hear the stop wherever it is.
    SessionStopped(LspStatus),
    DiagnosticsUpdated(DiagnosticUpdate),
    DocumentOwnerFocused {
        window_label: String,
        document_id: Option<DocumentId>,
        pane_id: Option<String>,
        canonical_path: Option<String>,
        reservation_failed: bool,
    },
}

#[async_trait]
pub(crate) trait SessionClient: Send + Sync + 'static {
    fn capabilities(&self) -> LspCapabilities;
    /// Trigger characters from the initialize result. Defaulted so a client
    /// that has none — every test double, and any future in-process client —
    /// simply advertises none rather than having to restate this.
    fn trigger_characters(&self) -> NegotiatedTriggers {
        NegotiatedTriggers::default()
    }
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
    /// Resolve one completion item the session already handed out. There is
    /// no position and no source version: the item id carries its own
    /// document, version and generation, and the session's cache rejects an
    /// id whose generation it has since purged.
    async fn resolve_completion(&self, item_id: &str) -> Result<CompletionItem, String>;
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
        cancellation: CancellationToken,
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

    fn trigger_characters(&self) -> NegotiatedTriggers {
        self.0.trigger_characters()
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

    async fn resolve_completion(&self, item_id: &str) -> Result<CompletionItem, String> {
        self.0
            .resolve_completion_item(item_id)
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
        cancellation: CancellationToken,
    ) -> Result<Arc<dyn SessionClient>, ManagerError> {
        let resource_root = self.resource_root.clone()?;
        let catalog = BundledServerCatalog::new();
        let descriptor = catalog.descriptor(start.language);
        let command = catalog
            .resolve(start.language, &resource_root)
            .map_err(|error| ManagerError::Unavailable(error.lsp_reason()))?;
        let session = LspSession::start_with_cancellation(
            descriptor,
            command,
            start.key.root,
            ProcessServerLauncher,
            events,
            cancellation,
        )
        .await
        .map_err(|error| match error {
            SessionError::Cancelled => ManagerError::Cancelled,
            error => ManagerError::Infrastructure(error.to_string()),
        })?;
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
    ) -> Result<OpenDocumentResponse, ManagerError> {
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

    pub(crate) async fn close_documents(
        &self,
        document_ids: Vec<DocumentId>,
    ) -> Result<(), ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::CloseDocuments {
            document_ids,
            reply,
        })
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

    pub(crate) async fn resolve_completion_item(
        &self,
        document_id: DocumentId,
        item_id: String,
    ) -> Result<CompletionItem, ManagerError> {
        let (reply, result) = oneshot::channel();
        self.send(ManagerCommand::ResolveCompletionItem {
            document_id,
            item_id,
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
        reply: oneshot::Sender<Result<OpenDocumentResponse, ManagerError>>,
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
    CloseDocuments {
        document_ids: Vec<DocumentId>,
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
    ResolveCompletionItem {
        document_id: DocumentId,
        item_id: String,
        reply: oneshot::Sender<Result<CompletionItem, ManagerError>>,
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

    fn deferred_reply_closed(&self) -> bool {
        match self {
            Self::SetProjectContext { reply, .. } => reply.is_closed(),
            Self::SetProjectTrust { reply, .. } | Self::SetEnablement { reply, .. } => {
                reply.is_closed()
            }
            Self::CloseDocument { reply, .. } => reply.is_closed(),
            Self::CloseDocuments { reply, .. } => reply.is_closed(),
            _ => false,
        }
    }

    fn fail_deferred(self, error: ManagerError) {
        match self {
            Self::SetProjectContext { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            Self::SetProjectTrust { reply, .. } | Self::SetEnablement { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            Self::CloseDocument { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            Self::CloseDocuments { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            _ => unreachable!("only authority commands are deferred"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RequestKind {
    Completion,
    ResolveCompletion,
    Hover,
    SignatureHelp,
    Definition,
}

enum PendingReply {
    Completion(oneshot::Sender<Result<CompletionResponse, ManagerError>>),
    ResolveCompletion(oneshot::Sender<Result<CompletionItem, ManagerError>>),
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
            Self::ResolveCompletion(reply) => {
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
    /// Boxed: a `CompletionItem` is several times the size of every other
    /// result here, and this enum crosses a channel on every request.
    ResolveCompletion(Box<Result<CompletionItem, String>>),
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
    DidClose {
        document_id: String,
        /// Present only for a close that a deferred close group staged. The
        /// token is the group's proof of delivery: nothing else the session
        /// sends for the same document may discharge the group.
        delivery: Option<ReservedCloseDelivery>,
    },
    Request {
        request_id: u64,
        kind: RequestKind,
        document_id: String,
        position: lsp::Position,
        /// Only a `ResolveCompletion` carries one; every other kind is
        /// addressed by position.
        item_id: Option<String>,
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
    /// Placeholder for a reserved close that was delivered ahead of this queue.
    /// It carries no protocol traffic; it only holds the worker slot the policy
    /// reserved so later groups keep queueing behind the same backpressure. It
    /// still names its document, because reaching it is what tells the mirror
    /// the server has stopped believing that document is open.
    ReservedCloseCredit {
        document_id: String,
    },
}

struct SessionWorkerHandle {
    operations: mpsc::Sender<SessionOperation>,
    /// Lane for policy-reserved closes, delivered one at a time. A policy
    /// transition has to prove its closes reached the server before it may
    /// release the documents, and the ordered lane can be arbitrarily deep with
    /// edits for unrelated documents. Each close still spends one ordered slot
    /// (a `ReservedCloseCredit`), so the backpressure the policy reserved is
    /// unchanged - only the acknowledgement stops waiting behind that backlog.
    close_deliveries: mpsc::Sender<(String, ReservedCloseDelivery)>,
    shutdown: Option<oneshot::Sender<()>>,
}

/// Whether an operation changes what the server believes about a document
/// being open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OpenTransition {
    Open,
    Close,
}

enum PipelineEntry {
    /// A reserved-close credit placeholder a deferred policy is waiting on.
    /// Its document's close went out on the delivery lane, so reaching this
    /// entry is when the mirror stops believing that document is open. Keeping
    /// it believed-open until then is the conservative direction: it can only
    /// make a later close be sent that was not strictly needed, never suppress
    /// one that was.
    ReservedCredit { document_id: String },
    /// Ordinary traffic for one document.
    Document {
        document_id: String,
        transition: Option<OpenTransition>,
    },
}

/// A mirror of everything a session has been handed but not yet executed.
///
/// The outbox and the worker's own queue together form one ordered pipeline
/// that the actor cannot otherwise see into: `pump_session` hands operations
/// off and only a `WorkerReady` says one finished. Mirroring the pipeline here
/// is what lets staging answer the two questions it has to get right - does
/// this document still have ordered traffic in flight, and does the server
/// actually believe it is open.
#[derive(Default)]
struct SessionDeliveryProgress {
    pipeline: VecDeque<PipelineEntry>,
    open_on_server: HashSet<String>,
}

/// The document an operation is ordered against, and how it moves the server's
/// idea of that document being open.
fn operation_document(operation: &SessionOperation) -> Option<(&str, Option<OpenTransition>)> {
    match operation {
        SessionOperation::DidOpen(document) => {
            Some((&document.document_id, Some(OpenTransition::Open)))
        }
        SessionOperation::DidChange(batch) => Some((&batch.document_id, None)),
        SessionOperation::DidSave(document_id) => Some((document_id, None)),
        SessionOperation::DidClose { document_id, .. } => {
            Some((document_id, Some(OpenTransition::Close)))
        }
        SessionOperation::Request { document_id, .. } => Some((document_id, None)),
        SessionOperation::PullDiagnostics {
            document_id_text, ..
        } => Some((document_id_text, None)),
        // Deliberately not the credit placeholder's document: a credit is not
        // traffic to order against, and it must never be swept up by the purge
        // that discards a closing document's queued operations.
        SessionOperation::ReservedCloseCredit { .. } => None,
    }
}

impl SessionDeliveryProgress {
    /// Matched on the operation itself, exhaustively and without a wildcard, so
    /// that a new `SessionOperation` variant is a compile error here rather
    /// than something the mirror silently misfiles.
    fn entry_for(operation: &SessionOperation) -> PipelineEntry {
        match operation {
            SessionOperation::ReservedCloseCredit { document_id } => {
                PipelineEntry::ReservedCredit {
                    document_id: document_id.clone(),
                }
            }
            SessionOperation::DidOpen(document) => PipelineEntry::Document {
                document_id: document.document_id.clone(),
                transition: Some(OpenTransition::Open),
            },
            SessionOperation::DidClose { document_id, .. } => PipelineEntry::Document {
                document_id: document_id.clone(),
                transition: Some(OpenTransition::Close),
            },
            SessionOperation::DidChange(batch) => PipelineEntry::Document {
                document_id: batch.document_id.clone(),
                transition: None,
            },
            SessionOperation::DidSave(document_id) => PipelineEntry::Document {
                document_id: document_id.clone(),
                transition: None,
            },
            SessionOperation::Request { document_id, .. } => PipelineEntry::Document {
                document_id: document_id.clone(),
                transition: None,
            },
            SessionOperation::PullDiagnostics {
                document_id_text, ..
            } => PipelineEntry::Document {
                document_id: document_id_text.clone(),
                transition: None,
            },
        }
    }

    fn record_enqueued(&mut self, operations: &[SessionOperation]) {
        for operation in operations {
            self.pipeline.push_back(Self::entry_for(operation));
        }
    }

    /// One operation finished, in pipeline order.
    fn record_completed(&mut self) {
        let Some(entry) = self.pipeline.pop_front() else {
            // The mirror is one-to-one with what the session was handed, so an
            // empty pipeline here means it has desynchronised - and a
            // desynchronised mirror can report a closing document as having
            // nothing in flight when it does, which is exactly the reordering
            // the purge exists to prevent.
            debug_assert!(false, "session delivery mirror ran dry before its session");
            log::warn!("LSP session delivery mirror ran dry before its session");
            return;
        };
        match entry {
            PipelineEntry::ReservedCredit { document_id } => {
                self.open_on_server.remove(&document_id);
            }
            PipelineEntry::Document {
                document_id,
                transition: Some(OpenTransition::Open),
            } => {
                self.open_on_server.insert(document_id);
            }
            PipelineEntry::Document {
                document_id,
                transition: Some(OpenTransition::Close),
            } => {
                self.open_on_server.remove(&document_id);
            }
            PipelineEntry::Document {
                transition: None, ..
            } => {}
        }
    }

    /// Re-mirror the queued tail after operations were dropped from the outbox.
    /// Everything ahead of the outbox is already inside the worker and cannot
    /// be taken back.
    fn rebuild_queued_tail(
        &mut self,
        previous_outbox_len: usize,
        outbox: &VecDeque<SessionOperation>,
    ) {
        // Invariant: pipeline.len() == (operations inside the worker) +
        // outbox.len(). If it ever slips, the tail rebuilt below would keep
        // entries that are no longer queued or drop entries that still are.
        debug_assert!(
            self.pipeline.len() >= previous_outbox_len,
            "session delivery mirror is shorter than the outbox it mirrors"
        );
        if self.pipeline.len() < previous_outbox_len {
            log::warn!("LSP session delivery mirror is shorter than the outbox it mirrors");
        }
        let inside_worker = self.pipeline.len().saturating_sub(previous_outbox_len);
        self.pipeline.truncate(inside_worker);
        for operation in outbox {
            self.pipeline.push_back(Self::entry_for(operation));
        }
    }

    fn credits_outstanding(&self) -> bool {
        self.pipeline
            .iter()
            .any(|entry| matches!(entry, PipelineEntry::ReservedCredit { .. }))
    }

    fn document_in_flight(&self, document_id: &str) -> bool {
        self.pipeline.iter().any(|entry| {
            matches!(entry, PipelineEntry::Document { document_id: queued, .. } if queued == document_id)
        })
    }

    fn believes_open(&self, document_id: &str) -> bool {
        self.open_on_server.contains(document_id)
    }

    /// Whether the server will still be holding this document open once
    /// everything the session has already been handed has run.
    ///
    /// `believes_open` answers for the operations that have executed; this
    /// answers for the ones that are going to. Settlement needs the second
    /// question, because a document can be re-opened after its group's close
    /// was staged and before the group lets go of it.
    fn will_be_open(&self, document_id: &str) -> bool {
        let mut open = self.open_on_server.contains(document_id);
        for entry in &self.pipeline {
            match entry {
                PipelineEntry::ReservedCredit {
                    document_id: credited,
                } if credited == document_id => open = false,
                PipelineEntry::Document {
                    document_id: queued,
                    transition: Some(transition),
                } if queued == document_id => open = *transition == OpenTransition::Open,
                _ => {}
            }
        }
        open
    }
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
    ReservedCloseDeliverySucceeded {
        delivery: ReservedCloseDelivery,
    },
    ReservedCloseDeliveryFailed {
        delivery: ReservedCloseDelivery,
        message: String,
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
    ReservationWaiter(ReservationId, u64),
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
    waiters: Vec<ReservationWaiter>,
}

#[derive(Clone)]
struct ReservationWaiter {
    delivery_key: FocusKey,
    window_label: String,
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
    triggers: NegotiatedTriggers,
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

struct ShutdownReply {
    pending: HashSet<(SessionKey, u64)>,
    replies: Vec<oneshot::Sender<Result<(), ManagerError>>>,
}

#[derive(Clone, Default)]
struct PolicyCapacityReservation {
    operations: usize,
    attachments: usize,
    close_documents: HashSet<DocumentId>,
}

/// Identity of one didClose notification a deferred close group still owes.
///
/// The token binds the notification to the exact group invocation, policy
/// epoch, session and session generation that staged it. Every field is part
/// of the identity on purpose: an ordinary resync close for the same document
/// carries no token at all, a token replayed after its group settled names a
/// group that is no longer in flight, and a token minted for an older
/// generation cannot speak for the generation that is running now.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ReservedCloseDelivery {
    group_id: u64,
    epoch: u64,
    /// Distinguishes one staging attempt from the next. A group whose staging
    /// aborted part way through keeps its group id, so without this an
    /// acknowledgement of the abandoned attempt would discharge the entry the
    /// retry created for the same document.
    attempt: u64,
    key: SessionKey,
    session_generation: u64,
    document_id: DocumentId,
}

/// Where an admitted close group sits between admission and its reply.
enum DeferredClosePhase {
    /// Admitted and holding reserved policy credits, but nothing has been
    /// handed to a session yet. A pending group may still be retried, cancelled
    /// or failed without the server ever having heard about it.
    Pending,
    /// didClose notifications are in flight. The group owns its documents until
    /// every outstanding delivery is acknowledged, so a half-delivered group
    /// never releases ownership and never advances the policy behind it.
    Delivering {
        outstanding: HashMap<DocumentId, ReservedCloseDelivery>,
        /// Non-empty once a delivery failed: the manager can no longer prove
        /// the server closed the document, so it stops every generation the
        /// group reached and waits for all of those stops before settling. A
        /// group that spans two sessions must not report success while the
        /// other session still holds one of its documents open.
        stopping: Vec<(SessionKey, u64)>,
    },
}

struct DeferredCloseGroup {
    group_id: u64,
    epoch: u64,
    document_ids: Vec<DocumentId>,
    required_session_closes: HashMap<SessionKey, HashSet<DocumentId>>,
    /// Members whose undelivered didOpen this group already threw away. The
    /// decision is remembered rather than re-derived because it is only
    /// visible once, in the staging attempt that did the throwing away: a
    /// later attempt would find nothing queued and would send a real didClose
    /// for a buffer the server never opened.
    ///
    /// The memo is group-scoped, which is only safe because a *different* group
    /// can never inherit a stale verdict: every route that destroys a staged
    /// group also destroys or restarts its session. The lost-ground guard fires
    /// only on a dead or restarted generation, and a restart re-enqueues a
    /// didOpen for every attached document - which either flips `believes_open`
    /// or clears the mirror outright. The re-check against `believes_open` at
    /// the use site is what turns that argument into an enforced one.
    discarded_opens: HashSet<DocumentId>,
    phase: DeferredClosePhase,
    reply: oneshot::Sender<Result<(), ManagerError>>,
}

impl DeferredCloseGroup {
    fn reply_closed(&self) -> bool {
        self.reply.is_closed()
    }

    fn is_pending(&self) -> bool {
        matches!(self.phase, DeferredClosePhase::Pending)
    }

    fn fail(self, error: ManagerError) {
        let _ = self.reply.send(Err(error));
    }
}

pub(crate) struct LspManager {
    state: ManagerState,
    factory: Arc<dyn SessionFactory>,
    cache_root: CacheRootAnchor,
    command_rx: mpsc::Receiver<ManagerCommand>,
    shutdown_rx: mpsc::Receiver<oneshot::Sender<Result<(), ManagerError>>>,
    shutdown_channel_open: bool,
    input_tx: mpsc::Sender<ActorInput>,
    input_rx: mpsc::Receiver<ActorInput>,
    event_tx: mpsc::Sender<ManagerEvent>,
    event_dispatch_cancel: CancellationToken,
    focus_deliveries: HashMap<FocusKey, FocusDelivery>,
    focus_order: VecDeque<FocusKey>,
    focus_in_flight: Option<FocusKey>,
    next_focus_waiter_id: u64,
    active_generations: HashSet<(SessionKey, u64)>,
    reservations: HashMap<ReservationId, ReservationRecord>,
    pending_requests: HashMap<u64, PendingRequest>,
    latest_requests: HashMap<(DocumentId, RequestKind), u64>,
    next_request_id: u64,
    pending_policy_commands: VecDeque<ManagerCommand>,
    pending_close_groups: VecDeque<DeferredCloseGroup>,
    /// Groups that were still delivering when a priority shutdown arrived.
    /// Their replies are owed exactly once, after the generations stop.
    settling_close_groups: Vec<DeferredCloseGroup>,
    session_delivery_progress: HashMap<SessionKey, SessionDeliveryProgress>,
    /// Closes owed for documents that were re-opened between a group staging
    /// them and settling them, and which the session had no room for at
    /// settlement. They are retried as the session drains.
    pending_repair_closes: HashMap<SessionKey, Vec<String>>,
    next_close_group_id: u64,
    next_close_delivery_attempt: u64,
    policy_capacity_reservations: HashMap<SessionKey, PolicyCapacityReservation>,
    policy_reservation_epoch: Option<u64>,
    next_policy_reservation_epoch: u64,
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
            shutdown_channel_open: true,
            input_tx,
            input_rx,
            event_tx,
            event_dispatch_cancel,
            focus_deliveries: HashMap::new(),
            focus_order: VecDeque::new(),
            focus_in_flight: None,
            next_focus_waiter_id: 1,
            active_generations: HashSet::new(),
            reservations: HashMap::new(),
            pending_requests: HashMap::new(),
            latest_requests: HashMap::new(),
            next_request_id: 1,
            pending_policy_commands: VecDeque::new(),
            pending_close_groups: VecDeque::new(),
            settling_close_groups: Vec::new(),
            session_delivery_progress: HashMap::new(),
            pending_repair_closes: HashMap::new(),
            next_close_group_id: 1,
            next_close_delivery_attempt: 1,
            policy_capacity_reservations: HashMap::new(),
            policy_reservation_epoch: None,
            next_policy_reservation_epoch: 1,
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
        let mut policy_input_quota_available = true;
        while !self.terminated {
            let policy_needs_input = !self.pending_close_groups.is_empty()
                || !self.pending_policy_commands.is_empty()
                || !self.policy_capacity_reservations.is_empty();
            if policy_needs_input && policy_input_quota_available {
                let can_receive_shutdown = self.shutdown_channel_open
                    && self
                        .shutdown_reply
                        .as_ref()
                        .is_none_or(|shutdown| shutdown.replies.len() < COMMAND_CAPACITY);
                if can_receive_shutdown {
                    match self.shutdown_rx.try_recv() {
                        Ok(reply) => {
                            self.begin_shutdown(reply);
                            continue;
                        }
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            self.shutdown_channel_open = false;
                        }
                        Err(mpsc::error::TryRecvError::Empty) => {}
                    }
                }
                match self.input_rx.try_recv() {
                    Ok(input) => {
                        self.handle_input(input);
                        // Give a ready normal command one turn before taking
                        // another policy-liveness input. This bounds service in
                        // both directions while a policy is pending.
                        policy_input_quota_available = false;
                        continue;
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => break,
                    Err(mpsc::error::TryRecvError::Empty) => {}
                }
            }
            tokio::select! {
                biased;
                shutdown = self.shutdown_rx.recv(), if self.shutdown_channel_open
                    && self.shutdown_reply.as_ref().is_none_or(|shutdown| {
                        shutdown.replies.len() < COMMAND_CAPACITY
                    }) => {
                    match shutdown {
                        Some(reply) => self.begin_shutdown(reply),
                        None => self.shutdown_channel_open = false,
                    }
                }
                _ = std::future::ready(()), if self.retry_policy_requested && !self.shutting_down => {
                    self.retry_policy_requested = false;
                    self.retry_pending_policy_command();
                    policy_input_quota_available = true;
                }
                command = self.command_rx.recv(), if !self.shutting_down => {
                    let Some(command) = command else { break };
                    if !self.shutting_down {
                        self.handle_command(command);
                    }
                    policy_input_quota_available = true;
                }
                input = self.input_rx.recv() => {
                    let Some(input) = input else { break };
                    self.handle_input(input);
                    policy_input_quota_available = false;
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
                self.handle_close_group(vec![document_id], reply);
            }
            ManagerCommand::CloseDocuments {
                document_ids,
                reply,
            } => {
                self.handle_close_group(document_ids, reply);
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
                self.prepare_policy_attempt();
                match self.set_project_context(document_id, context.clone()) {
                    Err(ManagerError::Overloaded) => self.defer_policy_command(
                        ManagerCommand::SetProjectContext {
                            document_id,
                            context,
                            reply,
                        },
                        self.retrying_policy,
                    ),
                    result => {
                        self.finish_policy_attempt();
                        let _ = reply.send(result);
                    }
                }
            }
            ManagerCommand::SetProjectTrust {
                root,
                adapter_id,
                decision,
                reply,
            } => {
                self.prepare_policy_attempt();
                match self.set_project_trust(root.clone(), adapter_id.clone(), decision) {
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
                        self.finish_policy_attempt();
                        let _ = reply.send(Err(error));
                    }
                    Ok(None) => {
                        self.finish_policy_attempt();
                        let _ = reply.send(Ok(()));
                    }
                    Ok(Some(plan)) => {
                        self.finish_policy_attempt();
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
                }
            }
            ManagerCommand::SetEnablement { enablement, reply } => {
                self.prepare_policy_attempt();
                match self.set_enablement_policy(enablement.clone()) {
                    Err(ManagerError::Overloaded) => {
                        self.defer_policy_command(
                            ManagerCommand::SetEnablement { enablement, reply },
                            self.retrying_policy,
                        );
                    }
                    result => {
                        self.finish_policy_attempt();
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
                    None,
                    RequestKind::Completion,
                    PendingReply::Completion(reply),
                );
            }
            ManagerCommand::ResolveCompletionItem {
                document_id,
                item_id,
                reply,
            } => self.start_request(
                document_id,
                EditorPosition {
                    line: 0,
                    character: 0,
                },
                Some(item_id),
                RequestKind::ResolveCompletion,
                PendingReply::ResolveCompletion(reply),
            ),
            ManagerCommand::Hover {
                document_id,
                position,
                reply,
            } => self.start_request(
                document_id,
                position,
                None,
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
                    None,
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
                None,
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
                        waiters: Vec::new(),
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
            ReserveResult::FocusPending {
                window_label: owner_window_label,
            } => {
                let reservation_id = self
                    .state
                    .ownership
                    .reservation_id(DocumentIdentifier::Local(path.clone()))?
                    .ok_or(ManagerError::InvalidReservation)?;
                let waiter_id = self.next_focus_waiter_id;
                self.next_focus_waiter_id = self.next_focus_waiter_id.wrapping_add(1).max(1);
                let delivery_key = FocusKey::ReservationWaiter(reservation_id, waiter_id);
                self.reserve_focus_obligation(delivery_key)?;
                if let Some(reservation) = self.reservations.get_mut(&reservation_id) {
                    reservation.waiters.push(ReservationWaiter {
                        delivery_key,
                        window_label,
                    });
                }
                let _ = owner_window_label;
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
                        canonical_path: self
                            .state
                            .ownership
                            .canonical_path(*document_id)
                            .map(|path| path.to_string_lossy().into_owned()),
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
        self.reevaluate_document_or_degrade(document_id);
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
    ) -> Result<OpenDocumentResponse, ManagerError> {
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
        self.reevaluate_document_or_degrade(document_id);
        let document = self
            .state
            .documents
            .get(&document_id)
            .ok_or(ManagerError::UnknownDocument)?;
        Ok(OpenDocumentResponse {
            document_id: document_id_text(document_id),
            version: document.text.version(),
            project_candidates: document.candidates.clone(),
            status: document.status.clone(),
        })
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
                    SessionOperation::DidClose {
                        document_id: document_id_text(document_id),
                        delivery: None,
                    },
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

    fn close_documents(&mut self, document_ids: Vec<DocumentId>) -> Result<(), ManagerError> {
        let mut seen = HashSet::new();
        let document_ids = document_ids
            .into_iter()
            .filter(|document_id| seen.insert(*document_id))
            .collect::<Vec<_>>();
        if self.retrying_policy {
            // A matching reserved group may contain more already-admitted
            // didChange batches than one outbox can hold together with every
            // didClose. Spend each WorkerReady turn moving only a safe prefix
            // while preserving slots for the whole close group. Ownership is
            // untouched until the final all-or-none preflight below succeeds.
            self.advance_reserved_close_group_batches(&document_ids)?;
        }
        self.ensure_close_documents_capacity(&document_ids)?;
        for document_id in document_ids {
            self.close_document(document_id)?;
        }
        Ok(())
    }

    /// All-or-none preflight: every listed document must fit its session outbox
    /// together with the batches it still owes and any policy reservation.
    fn ensure_close_documents_capacity(
        &self,
        document_ids: &[DocumentId],
    ) -> Result<(), ManagerError> {
        let mut required = HashMap::<SessionKey, usize>::new();
        for document_id in document_ids {
            let Some(document) = self.state.documents.get(document_id) else {
                continue;
            };
            let Some(key) = document.session_key.clone() else {
                continue;
            };
            let close_count = usize::from(
                self.state
                    .sessions
                    .get(&key)
                    .is_some_and(|session| session.protocol_started),
            );
            *required.entry(key).or_default() +=
                document.pending_batches.len().saturating_add(close_count);
        }
        for (key, count) in required {
            let session = self
                .state
                .sessions
                .get(&key)
                .ok_or(ManagerError::SessionUnavailable)?;
            if session
                .outbox
                .len()
                .saturating_add(count)
                .saturating_add(self.reserved_policy_capacity(&key).operations)
                > SESSION_OUTBOX_CAPACITY
            {
                return Err(ManagerError::Overloaded);
            }
        }
        Ok(())
    }

    fn advance_reserved_close_group_batches(
        &mut self,
        document_ids: &[DocumentId],
    ) -> Result<(), ManagerError> {
        let mut close_required = HashMap::<SessionKey, usize>::new();
        for document_id in document_ids {
            let Some(document) = self.state.documents.get(document_id) else {
                continue;
            };
            let Some(key) = document.session_key.clone() else {
                continue;
            };
            let session = self
                .state
                .sessions
                .get(&key)
                .ok_or(ManagerError::SessionUnavailable)?;
            if session.protocol_started {
                *close_required.entry(key).or_default() += 1;
            }
        }

        for (key, close_count) in close_required {
            let outbox_len = self
                .state
                .sessions
                .get(&key)
                .ok_or(ManagerError::SessionUnavailable)?
                .outbox
                .len();
            let mut batch_credit = SESSION_OUTBOX_CAPACITY
                .saturating_sub(outbox_len)
                .saturating_sub(close_count);
            if batch_credit == 0 {
                continue;
            }
            let mut batches = Vec::new();
            for document_id in document_ids {
                if batch_credit == 0 {
                    break;
                }
                let Some(document) = self.state.documents.get_mut(document_id) else {
                    continue;
                };
                if document.session_key.as_ref() != Some(&key) {
                    continue;
                }
                let take = batch_credit.min(document.pending_batches.len());
                if take == 0 {
                    continue;
                }
                batches.extend(document.pending_batches.drain(..take));
                document.batch_generation = document.batch_generation.saturating_add(1);
                batch_credit -= take;
            }
            self.enqueue_session_operations(
                &key,
                batches.into_iter().map(SessionOperation::DidChange),
            )?;
        }
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
        self.discard_cancelled_close_groups();
        self.discard_cancelled_policy_front();
        if self.delivering_close_group_lost_its_ground() {
            self.invalidate_policy_epoch(ManagerError::PolicyChanged);
            self.retry_policy_requested = !self.pending_policy_commands.is_empty();
            return;
        }
        if self.retry_pending_close_group() {
            return;
        }
        // Every group is settled, but the policy still may not run until the
        // worker has spent the slots those groups reserved: that backpressure
        // is exactly what the policy reserved them for.
        if self.reserved_close_credits_outstanding() {
            return;
        }
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
        if command.deferred_reply_closed() {
            if retry_at_front {
                self.invalidate_policy_epoch(ManagerError::PolicyChanged);
                self.retry_policy_requested = !self.pending_policy_commands.is_empty();
            }
            return;
        }
        if self.pending_policy_commands.len() >= COMMAND_CAPACITY {
            command.fail_deferred(ManagerError::Overloaded);
        } else if retry_at_front {
            self.pending_policy_commands.push_front(command);
        } else {
            self.pending_policy_commands.push_back(command);
        }
    }

    fn discard_cancelled_policy_front(&mut self) {
        let mut discarded = false;
        while self
            .pending_policy_commands
            .front()
            .is_some_and(ManagerCommand::deferred_reply_closed)
        {
            self.pending_policy_commands.pop_front();
            discarded = true;
        }
        if discarded {
            self.invalidate_policy_epoch(ManagerError::PolicyChanged);
            if !self.policy_async_in_flight
                && (!self.pending_close_groups.is_empty()
                    || !self.pending_policy_commands.is_empty())
            {
                self.retry_policy_requested = true;
            }
        }
    }

    fn prepare_policy_attempt(&mut self) {
        self.policy_capacity_reservations.clear();
        if !self.retrying_policy {
            self.invalidate_policy_epoch(ManagerError::PolicyChanged);
        }
    }

    fn finish_policy_attempt(&mut self) {
        self.invalidate_policy_epoch(ManagerError::PolicyChanged);
    }

    fn invalidate_policy_epoch(&mut self, error: ManagerError) {
        // Limitation (M7): a group that is already delivering is dropped here
        // together with the pending ones. Its documents stay open and owned, so
        // the caller can retry, but the didClose notifications it already sent
        // cannot be recalled - the retry will send them a second time. Only the
        // epoch guard's own conditions get us here, and every one of them means
        // the session state is no longer trustworthy anyway.
        for group in self.pending_close_groups.drain(..) {
            group.fail(error.clone());
        }
        self.policy_capacity_reservations.clear();
        self.policy_reservation_epoch = None;
    }

    /// True when a group that already handed didClose notifications to a
    /// session can no longer stand behind them.
    ///
    /// Those notifications cannot be taken back or replayed, so the group is
    /// only meaningful while the epoch it was admitted under is still current
    /// and the exact generation it addressed is still running. When either is
    /// gone the whole epoch is untrustworthy and every group in it has to be
    /// retried by its caller. A group that is already waiting for the
    /// controlled stop it asked for is exempt: that stop is the proof it wants.
    fn delivering_close_group_lost_its_ground(&self) -> bool {
        self.pending_close_groups
            .iter()
            .any(|group| match &group.phase {
                DeferredClosePhase::Pending => false,
                DeferredClosePhase::Delivering {
                    outstanding,
                    stopping,
                } => {
                    stopping.is_empty()
                        && (self.policy_reservation_epoch != Some(group.epoch)
                            || outstanding
                                .values()
                                .any(|delivery| !self.delivery_generation_running(delivery)))
                }
            })
    }

    fn delivery_generation_running(&self, delivery: &ReservedCloseDelivery) -> bool {
        self.state
            .sessions
            .get(&delivery.key)
            .is_some_and(|session| {
                session.protocol_started && session.generation == delivery.session_generation
            })
    }

    fn discard_cancelled_close_groups(&mut self) {
        let mut index = 0;
        while index < self.pending_close_groups.len() {
            // A delivering group is kept even when its caller went away: its
            // didClose notifications are already on the wire, so the manager
            // still has to release the documents when they are acknowledged.
            if self.pending_close_groups[index].reply_closed()
                && self.pending_close_groups[index].is_pending()
            {
                self.pending_close_groups.remove(index);
            } else {
                index += 1;
            }
        }
    }

    fn handle_close_group(
        &mut self,
        document_ids: Vec<DocumentId>,
        reply: oneshot::Sender<Result<(), ManagerError>>,
    ) {
        let mut seen = HashSet::new();
        let document_ids = document_ids
            .into_iter()
            .filter(|document_id| seen.insert(*document_id))
            .collect::<Vec<_>>();
        self.discard_cancelled_close_groups();
        self.discard_cancelled_policy_front();
        if self.pending_close_groups.iter().any(|group| {
            group
                .document_ids
                .iter()
                .any(|document_id| seen.contains(document_id))
        }) {
            let _ = reply.send(Err(ManagerError::Overloaded));
            return;
        }
        if !self.pending_close_groups.is_empty() {
            self.admit_close_group(document_ids, reply);
            return;
        }
        match self.close_documents(document_ids.clone()) {
            Err(ManagerError::Overloaded) => self.admit_close_group(document_ids, reply),
            result => {
                let _ = reply.send(result);
            }
        }
    }

    fn admit_close_group(
        &mut self,
        document_ids: Vec<DocumentId>,
        reply: oneshot::Sender<Result<(), ManagerError>>,
    ) {
        if reply.is_closed() {
            return;
        }
        if self.pending_close_groups.len() >= COMMAND_CAPACITY {
            let _ = reply.send(Err(ManagerError::Overloaded));
            return;
        }
        let Some(epoch) = self.policy_reservation_epoch else {
            let _ = reply.send(Err(ManagerError::Overloaded));
            return;
        };
        if !self
            .pending_policy_commands
            .front()
            .is_some_and(ManagerCommand::is_policy)
        {
            let _ = reply.send(Err(ManagerError::PolicyChanged));
            return;
        }
        let Some(required_session_closes) = self.reserved_close_credits(&document_ids) else {
            let _ = reply.send(Err(ManagerError::Overloaded));
            return;
        };
        let group_id = self.next_close_group_id;
        self.next_close_group_id = self.next_close_group_id.saturating_add(1);
        self.pending_close_groups.push_back(DeferredCloseGroup {
            group_id,
            epoch,
            document_ids,
            required_session_closes,
            discarded_opens: HashSet::new(),
            phase: DeferredClosePhase::Pending,
            reply,
        });
        if self.pending_close_groups.len() == 1 {
            self.retry_policy_requested = true;
        }
    }

    /// Advance the front close group by one turn.
    ///
    /// Returns true whenever a close group still owns the front of the policy
    /// lane, whether this turn staged it, retried it, or found it already
    /// delivering. Only a false answer lets the deferred policy command behind
    /// it run.
    fn retry_pending_close_group(&mut self) -> bool {
        let Some(front) = self.pending_close_groups.front() else {
            return false;
        };
        if !front.is_pending() {
            // The front group already handed its didClose notifications to a
            // session. Nothing else may be staged, and the policy may not run,
            // until every one of them is acknowledged.
            return true;
        }
        let mut group = self.pending_close_groups.pop_front().expect("front exists");
        if !self.close_group_matches_epoch(&group) {
            group.fail(ManagerError::PolicyChanged);
            self.invalidate_policy_epoch(ManagerError::PolicyChanged);
            self.retry_policy_requested = !self.pending_policy_commands.is_empty();
            return true;
        }
        self.retrying_policy = true;
        let result = self.stage_close_group(&mut group);
        self.retrying_policy = false;
        match result {
            Err(ManagerError::Overloaded) => self.pending_close_groups.push_front(group),
            Ok(outstanding) => {
                self.consume_close_group_credits(&group);
                let settled = outstanding.is_empty();
                group.phase = DeferredClosePhase::Delivering {
                    outstanding,
                    stopping: Vec::new(),
                };
                self.pending_close_groups.push_front(group);
                if settled {
                    // Nothing needed a notification, so the group is already
                    // proven complete.
                    self.settle_front_close_group(Ok(()));
                } else {
                    self.retry_policy_requested = false;
                }
            }
            Err(error) => {
                group.fail(error);
                self.retry_policy_requested = !self.pending_close_groups.is_empty()
                    || !self.pending_policy_commands.is_empty();
            }
        }
        true
    }

    /// Hand a matching group's didClose notifications to their sessions.
    ///
    /// Ownership, document state and the group's reply are deliberately left
    /// untouched: until the session acknowledges every notification the
    /// manager cannot honestly claim the documents are closed, so they stay
    /// open and owned exactly as they were.
    fn stage_close_group(
        &mut self,
        group: &mut DeferredCloseGroup,
    ) -> Result<HashMap<DocumentId, ReservedCloseDelivery>, ManagerError> {
        let document_ids = group.document_ids.clone();
        // A matching reserved group may contain more already-admitted didChange
        // batches than one outbox can hold together with every didClose. Spend
        // each WorkerReady turn moving only a safe prefix while preserving
        // slots for the whole close group.
        self.advance_reserved_close_group_batches(&document_ids)?;
        self.ensure_close_documents_capacity(&document_ids)?;

        // Phase one resolves and flushes every member. It is the only fallible
        // part, and it runs to completion before anything is discarded, so a
        // failure here leaves the group exactly as retryable as it was.
        let mut members = Vec::new();
        for document_id in document_ids {
            if !self.state.documents.contains_key(&document_id) {
                continue;
            }
            let key = self
                .state
                .documents
                .get(&document_id)
                .and_then(|document| document.session_key.clone());
            let Some(key) = key else {
                continue;
            };
            // Checked before flushing rather than after: `flush_document` is
            // itself fallible on a missing session, and the whole point of this
            // phase is that nothing fails once phase two starts discarding.
            if !self.state.sessions.contains_key(&key) {
                return Err(ManagerError::SessionUnavailable);
            }
            self.flush_document(document_id)?;
            let Some(session_generation) = self
                .state
                .sessions
                .get(&key)
                .filter(|session| session.protocol_started)
                .map(|session| session.generation)
            else {
                continue;
            };
            members.push((document_id, key, session_generation));
        }

        // Phase two commits. `ensure_close_documents_capacity` above reserved
        // one outbox slot per member on top of the batches phase one flushed,
        // and the discards below only ever free more, so the enqueues here
        // cannot overflow. Should that reasoning ever stop holding, the
        // discard is still self-repairing: it marks the document out of sync
        // and cancels its requests, so an abandoned attempt leaves a document
        // the resync machinery knows how to fix rather than a silent phantom.
        let mut outstanding = HashMap::new();
        for (document_id, key, session_generation) in members {
            let id_text = document_id_text(document_id);
            // Nothing still queued for a closing document can matter, and
            // leaving it queued is exactly what would let an ordered didOpen or
            // didChange land *after* the close this group is about to deliver:
            // the server would keep a phantom buffer shadowing the file for the
            // rest of the session.
            let dropped_open = self.discard_queued_document_traffic(&key, document_id);
            let progress = self.session_delivery_progress.get(&key);
            let believes_open = progress.is_some_and(|progress| progress.believes_open(&id_text));
            if dropped_open && !believes_open {
                group.discarded_opens.insert(document_id);
            }
            if !believes_open && group.discarded_opens.contains(&document_id) {
                // The open this close would have been paired with never reached
                // the server and never will now. Telling the server to close a
                // buffer it does not have would be a lie, so the delivery is
                // vacuously satisfied and the group owes nothing for it.
                //
                // The verdict has to be remembered: it is only visible in the
                // attempt that did the discarding, and a retried attempt would
                // find nothing queued and send a real close. It is also
                // re-checked against the server's current belief rather than
                // trusted blindly, so a session that restarted and re-opened
                // the document in between still gets its close.
                continue;
            }
            // Anything still ahead of this document inside the worker's own
            // queue cannot be taken back, so its close has to stay in the
            // ordered lane to land after it.
            let in_flight = progress.is_some_and(|progress| progress.document_in_flight(&id_text));
            self.next_close_delivery_attempt = self.next_close_delivery_attempt.saturating_add(1);
            let delivery = ReservedCloseDelivery {
                group_id: group.group_id,
                epoch: group.epoch,
                attempt: self.next_close_delivery_attempt,
                key: key.clone(),
                session_generation,
                document_id,
            };
            // Reserved closes travel on the delivery lane so a policy
            // transition can prove its closes landed without first outlasting
            // an unrelated document's backlog. Either way one outbox slot is
            // spent, so the credit accounting is identical.
            //
            // Limitation (M8): a lane that is momentarily full silently falls
            // back to the ordered lane. That is correct but slower, and it is
            // invisible from the outside - the group simply waits longer.
            let out_of_band = !in_flight
                && self
                    .state
                    .sessions
                    .get(&key)
                    .and_then(|session| session.worker.as_ref())
                    .is_some_and(|worker| worker.close_deliveries.capacity() > 0);
            let operation = if out_of_band {
                SessionOperation::ReservedCloseCredit {
                    document_id: id_text.clone(),
                }
            } else {
                SessionOperation::DidClose {
                    document_id: id_text.clone(),
                    delivery: Some(delivery.clone()),
                }
            };
            self.enqueue_session_operations(&key, [operation])?;
            if out_of_band
                && let Some(worker) = self
                    .state
                    .sessions
                    .get(&key)
                    .and_then(|session| session.worker.as_ref())
                && let Err(error) = worker
                    .close_deliveries
                    .try_send((id_text, delivery.clone()))
            {
                // The lane was checked for room a moment ago on this same turn,
                // so this only happens when the worker is already gone. The
                // group is then failed by the staleness guard on the next turn.
                log::warn!("LSP reserved close delivery lane rejected a close: {error}");
            }
            outstanding.insert(document_id, delivery);
        }
        Ok(outstanding)
    }

    /// Discard everything a session still has queued for a document this
    /// manager is closing, and report whether an undelivered didOpen was
    /// among it.
    ///
    /// Only the outbox can be emptied: operations already handed to the worker
    /// are beyond recall, which is why the caller keeps such a document's close
    /// in the ordered lane instead of the delivery lane.
    ///
    /// Two loose ends are tied off here rather than on the caller's happy path,
    /// which a mid-loop failure would never reach. Requests are cancelled for
    /// every document this is called for, discard or not - the caller is closing
    /// it either way, so no reply may be left hanging on work that will never
    /// run. A document that actually lost queued operations the server needed is
    /// additionally marked out of sync, so `apply_changes` asks the client to
    /// resynchronise it. Normally the document is removed moments later when the
    /// group settles and neither matters; they matter precisely when it is not.
    fn discard_queued_document_traffic(
        &mut self,
        key: &SessionKey,
        document_id: DocumentId,
    ) -> bool {
        self.cancel_document_requests(document_id, ManagerError::Cancelled);
        let id_text = document_id_text(document_id);
        let Some(session) = self.state.sessions.get_mut(key) else {
            return false;
        };
        let previous_len = session.outbox.len();
        let mut dropped_open = false;
        session
            .outbox
            .retain(|operation| match operation_document(operation) {
                Some((queued_id, transition)) if queued_id == id_text => {
                    dropped_open |= transition == Some(OpenTransition::Open);
                    false
                }
                _ => true,
            });
        if session.outbox.len() == previous_len {
            return false;
        }
        if let Some(progress) = self.session_delivery_progress.get_mut(key) {
            progress.rebuild_queued_tail(previous_len, &session.outbox);
        }
        if let Some(document) = self.state.documents.get_mut(&document_id) {
            document.synchronization_dirty = true;
        }
        dropped_open
    }

    /// Release the front group's documents and answer its caller exactly once.
    ///
    /// A member can be re-opened between staging and settlement. The resync a
    /// client sends in answer to `ResyncRequired` is not a policy command, so it
    /// runs immediately - while the group is still delivering - and enqueues a
    /// didClose/didOpen pair of its own. Staging deliberately leaves document
    /// state alone, so nothing stops it. Releasing the document on the strength
    /// of the group's own close alone would then leave the server holding a
    /// buffer for a file the manager has forgotten, with nothing left that could
    /// ever close it. Settlement therefore asks what the wire will actually look
    /// like and closes the document again if the answer is "still open".
    fn settle_front_close_group(&mut self, result: Result<(), ManagerError>) {
        let Some(group) = self.pending_close_groups.pop_front() else {
            return;
        };
        for document_id in &group.document_ids {
            // The repair is attempted first and separately, because it must not
            // be able to abort the detach. `detach_document` bails on a refused
            // enqueue before it cancels requests, clears diagnostics, drops the
            // document from its session and arms idle shutdown - so a refused
            // repair would leave the session holding a member that exists
            // nowhere else, one that can never be removed and therefore never
            // lets the session go idle.
            if self.server_will_hold_open(*document_id) {
                self.owe_repair_close(*document_id);
            }
            let _ = self.detach_document(*document_id, true, false);
            self.state.documents.remove(document_id);
            self.state.ownership.close(*document_id);
        }
        let _ = group.reply.send(result);
        self.retry_policy_requested =
            !self.pending_close_groups.is_empty() || !self.pending_policy_commands.is_empty();
        // The next group must be able to start on this same turn: a settled
        // group is the only thing that frees the front of the policy lane, and
        // no worker event is guaranteed to follow it.
        self.retry_pending_policy_command();
    }

    /// Queue the didClose that puts the server back in step for a document the
    /// manager is letting go of, or record that it is still owed.
    ///
    /// The repair travels the ordered lane on purpose. It exists to counteract
    /// operations that are themselves queued there - a resync's didOpen, most
    /// of the time - so it is only correct once it lands behind them. The
    /// delivery lane, which exists precisely to overtake that backlog, would
    /// race the very thing this is repairing.
    ///
    /// A refused enqueue is normal, not exceptional: settlement is driven by an
    /// out-of-band acknowledgement that is by design independent of the ordered
    /// backlog, so the outbox can be at capacity at exactly this moment. The
    /// close is parked and retried as the session drains rather than dropped.
    fn owe_repair_close(&mut self, document_id: DocumentId) {
        let Some(key) = self
            .state
            .documents
            .get(&document_id)
            .and_then(|document| document.session_key.clone())
        else {
            return;
        };
        if !self
            .state
            .sessions
            .get(&key)
            .is_some_and(|session| session.protocol_started)
        {
            return;
        }
        let id_text = document_id_text(document_id);
        if self
            .enqueue_session_operations(
                &key,
                [SessionOperation::DidClose {
                    document_id: id_text.clone(),
                    delivery: None,
                }],
            )
            .is_err()
        {
            self.pending_repair_closes
                .entry(key)
                .or_default()
                .push(id_text);
        }
    }

    /// Retry the closes a session owes, as its outbox frees up.
    ///
    /// Each is re-checked against the mirror first: something else may have
    /// closed the document in the meantime, and a second didClose for an
    /// already-closed buffer is exactly the kind of noise this whole path
    /// exists to avoid.
    fn drain_repair_closes(&mut self, key: &SessionKey) {
        let Some(owed) = self.pending_repair_closes.remove(key) else {
            return;
        };
        let mut still_owed = Vec::new();
        for id_text in owed {
            let outstanding = self
                .session_delivery_progress
                .get(key)
                .is_some_and(|progress| progress.will_be_open(&id_text));
            if !outstanding {
                continue;
            }
            if self
                .enqueue_session_operations(
                    key,
                    [SessionOperation::DidClose {
                        document_id: id_text.clone(),
                        delivery: None,
                    }],
                )
                .is_err()
            {
                still_owed.push(id_text);
            }
        }
        if !still_owed.is_empty() {
            self.pending_repair_closes.insert(key.clone(), still_owed);
        }
    }

    /// True when everything the document's session has already been handed
    /// still leaves the server holding it open.
    fn server_will_hold_open(&self, document_id: DocumentId) -> bool {
        let Some(key) = self
            .state
            .documents
            .get(&document_id)
            .and_then(|document| document.session_key.clone())
        else {
            // No session means no server-side buffer to worry about: either it
            // never had one or the generation that held it is gone.
            return false;
        };
        self.session_delivery_progress
            .get(&key)
            .is_some_and(|progress| progress.will_be_open(&document_id_text(document_id)))
    }

    /// Accept one acknowledged reserved close.
    ///
    /// Only the front group can be delivering, and only a token that matches
    /// one of its outstanding entries in every field may discharge it. A stale
    /// generation, a replayed token from an already-settled group, and an
    /// ordinary close carrying no token at all are all inert here.
    fn reserved_close_delivered(&mut self, delivery: ReservedCloseDelivery) {
        let Some(group) = self.pending_close_groups.front_mut() else {
            return;
        };
        if group.group_id != delivery.group_id {
            return;
        }
        let DeferredClosePhase::Delivering {
            outstanding,
            stopping,
        } = &mut group.phase
        else {
            return;
        };
        if !stopping.is_empty() {
            // The group is already waiting for controlled stops to prove the
            // generations are gone. Late acknowledgements change nothing.
            return;
        }
        if outstanding.get(&delivery.document_id) != Some(&delivery) {
            return;
        }
        outstanding.remove(&delivery.document_id);
        if outstanding.is_empty() {
            self.settle_front_close_group(Ok(()));
        }
    }

    /// Handle a reserved close the session could not deliver.
    ///
    /// Two things are true at once here. The delivery lane stops at its first
    /// failure, so this generation can never acknowledge another reserved
    /// close; that holds whether or not the token still belongs to a group, and
    /// a generation left running would strand the next group forever waiting
    /// for an acknowledgement that cannot come. And earlier members of the
    /// token's own group may already have been closed for real, so that group
    /// can neither be abandoned nor completed on the spot.
    ///
    /// So: stop every generation involved, then let the stops settle the group.
    /// A stopped generation is the only honest proof that the documents it held
    /// are closed, which is why a group spanning two sessions stops both rather
    /// than reporting success while the other still has a document open.
    fn reserved_close_delivery_failed(&mut self, delivery: ReservedCloseDelivery, message: String) {
        self.push_log(
            &delivery.key,
            "protocol",
            format!("didClose failed ({} bytes redacted)", message.len()),
        );
        let attributable = self.pending_close_groups.front().is_some_and(|group| {
            group.group_id == delivery.group_id
                && matches!(
                    &group.phase,
                    DeferredClosePhase::Delivering {
                        outstanding,
                        stopping,
                    } if stopping.is_empty()
                        && outstanding.get(&delivery.document_id) == Some(&delivery)
                )
        });
        let mut generations = vec![(delivery.key.clone(), delivery.session_generation)];
        if attributable
            && let Some(DeferredClosePhase::Delivering { outstanding, .. }) =
                self.pending_close_groups.front().map(|group| &group.phase)
        {
            for pending in outstanding.values() {
                let generation = (pending.key.clone(), pending.session_generation);
                if !generations.contains(&generation) {
                    generations.push(generation);
                }
            }
        }
        let mut waiting = Vec::new();
        for (key, generation) in generations {
            let running = self.state.sessions.get(&key).is_some_and(|session| {
                session.generation == generation && session.worker.is_some()
            });
            if running {
                self.stop_session(&key);
                waiting.push((key, generation));
            }
        }
        if !attributable {
            self.retry_pending_policy_command();
            return;
        }
        if let Some(group) = self.pending_close_groups.front_mut()
            && let DeferredClosePhase::Delivering {
                outstanding,
                stopping,
            } = &mut group.phase
        {
            outstanding.clear();
            *stopping = waiting.clone();
        }
        if waiting.is_empty() {
            // Every generation the group reached is already gone, so all of its
            // documents are closed by construction.
            self.settle_front_close_group(Ok(()));
        }
    }

    /// Settle a group once every generation it stopped has finished stopping.
    fn settle_stopped_close_group(&mut self, key: &SessionKey, generation: u64) {
        let Some(group) = self.pending_close_groups.front_mut() else {
            return;
        };
        let DeferredClosePhase::Delivering { stopping, .. } = &mut group.phase else {
            return;
        };
        if stopping.is_empty() {
            return;
        }
        stopping.retain(|(stopping_key, stopping_generation)| {
            stopping_key != key || *stopping_generation != generation
        });
        if stopping.is_empty() {
            self.settle_front_close_group(Ok(()));
        }
    }

    fn reserved_close_credits(
        &self,
        document_ids: &[DocumentId],
    ) -> Option<HashMap<SessionKey, HashSet<DocumentId>>> {
        let mut required = HashMap::<SessionKey, HashSet<DocumentId>>::new();
        for document_id in document_ids.iter().copied() {
            let Some(key) = self
                .state
                .documents
                .get(&document_id)
                .and_then(|document| document.session_key.clone())
            else {
                continue;
            };
            let needs_close = self
                .state
                .sessions
                .get(&key)
                .is_some_and(|session| session.protocol_started);
            if !needs_close {
                continue;
            }
            let reservation = self.policy_capacity_reservations.get(&key)?;
            if !reservation.close_documents.contains(&document_id) {
                return None;
            }
            required.entry(key).or_default().insert(document_id);
        }
        (!required.is_empty()).then_some(required)
    }

    fn close_group_matches_epoch(&self, group: &DeferredCloseGroup) -> bool {
        self.policy_reservation_epoch == Some(group.epoch)
            && self
                .pending_policy_commands
                .front()
                .is_some_and(ManagerCommand::is_policy)
            && self.reserved_close_credits(&group.document_ids).as_ref()
                == Some(&group.required_session_closes)
            && group
                .required_session_closes
                .iter()
                .all(|(key, documents)| {
                    self.policy_capacity_reservations
                        .get(key)
                        .is_some_and(|reservation| reservation.operations >= documents.len())
                })
    }

    fn consume_close_group_credits(&mut self, group: &DeferredCloseGroup) {
        for (key, documents) in &group.required_session_closes {
            if let Some(reservation) = self.policy_capacity_reservations.get_mut(key) {
                reservation.operations = reservation.operations.saturating_sub(documents.len());
                for document_id in documents {
                    reservation.close_documents.remove(document_id);
                }
            }
        }
        self.policy_capacity_reservations.retain(|_, reservation| {
            reservation.operations > 0
                || reservation.attachments > 0
                || !reservation.close_documents.is_empty()
        });
    }

    fn reserved_policy_capacity(&self, key: &SessionKey) -> PolicyCapacityReservation {
        if self.retrying_policy {
            PolicyCapacityReservation::default()
        } else {
            self.policy_capacity_reservations
                .get(key)
                .cloned()
                .unwrap_or_default()
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
            if session
                .outbox
                .len()
                .saturating_add(count)
                .saturating_add(self.reserved_policy_capacity(&key).operations)
                > SESSION_OUTBOX_CAPACITY
            {
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
        &mut self,
        transitions: impl IntoIterator<Item = (DocumentId, Option<SessionKey>)>,
    ) -> Result<(), ManagerError> {
        let mut operations = HashMap::<SessionKey, usize>::new();
        let mut attachments = HashMap::<SessionKey, usize>::new();
        let mut close_documents = HashMap::<SessionKey, HashSet<DocumentId>>::new();
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
                    *operations.entry(current.clone()).or_default() += 1;
                    close_documents
                        .entry(current)
                        .or_default()
                        .insert(document_id);
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
        for key in operations.keys().chain(attachments.keys()) {
            if !self.state.sessions.contains_key(key) {
                return Err(ManagerError::SessionUnavailable);
            }
        }
        let operation_capacity_blocked = operations.iter().any(|(key, count)| {
            self.state.sessions[key].outbox.len().saturating_add(*count) > SESSION_OUTBOX_CAPACITY
        });
        let attachment_capacity_blocked = attachments.iter().any(|(key, count)| {
            self.state.sessions[key]
                .documents
                .len()
                .saturating_add(*count)
                > SESSION_OUTBOX_CAPACITY
        });
        if operation_capacity_blocked || attachment_capacity_blocked {
            let mut reservations = HashMap::<SessionKey, PolicyCapacityReservation>::new();
            for (key, count) in operations {
                reservations.entry(key).or_default().operations = count;
            }
            for (key, count) in attachments {
                reservations.entry(key).or_default().attachments = count;
            }
            for (key, documents) in close_documents {
                reservations.entry(key).or_default().close_documents = documents;
            }
            self.policy_capacity_reservations = reservations;
            if self.policy_reservation_epoch.is_none() {
                let epoch = self.next_policy_reservation_epoch;
                self.next_policy_reservation_epoch =
                    self.next_policy_reservation_epoch.wrapping_add(1);
                if self.next_policy_reservation_epoch == 0 {
                    self.next_policy_reservation_epoch = 1;
                }
                self.policy_reservation_epoch = Some(epoch);
            }
            return Err(ManagerError::Overloaded);
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

    fn reevaluate_document_or_degrade(&mut self, document_id: DocumentId) {
        if self.reevaluate_document(document_id).is_err() {
            let attempted_session = self.state.documents.get(&document_id).and_then(|document| {
                document
                    .selected_root
                    .clone()
                    .zip(document.adapter_id.clone())
                    .map(|(root, adapter_id)| SessionKey { adapter_id, root })
            });
            // Ownership and the editor buffer have already committed. LSP
            // attachment is optional from this point onward, so detach it
            // without requiring another outbox slot and expose degradation as
            // status rather than making the ownership outcome ambiguous.
            let _ = self.detach_document(document_id, false, false);
            if let Some(document) = self.state.documents.get_mut(&document_id) {
                document.synchronization_dirty = true;
            }
            self.set_document_status(
                document_id,
                LspSessionState::Failed,
                attempted_session,
                Some("Language features are unavailable; editing continues".into()),
                None,
            );
        }
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
        let reserved = self.reserved_policy_capacity(&key);
        if let Some(session) = self.state.sessions.get(&key)
            && (session
                .documents
                .len()
                .saturating_add(1)
                .saturating_add(reserved.attachments)
                > SESSION_OUTBOX_CAPACITY
                || (session.worker.is_some()
                    && session
                        .outbox
                        .len()
                        .saturating_add(1)
                        .saturating_add(reserved.operations)
                        > SESSION_OUTBOX_CAPACITY))
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
                    triggers: NegotiatedTriggers::default(),
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
                [SessionOperation::DidClose {
                    document_id: document_id_text(document_id),
                    delivery: None,
                }],
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
                    [SessionOperation::DidClose {
                        document_id: document_id_text(document_id),
                        delivery: None,
                    }],
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
        self.session_delivery_progress.remove(&key);
        self.pending_repair_closes.remove(&key);
        let session = self.state.sessions.get_mut(&key).expect("checked above");
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
            let result = factory
                .start(start, client_events, cancellation.clone())
                .await;
            if cancellation.is_cancelled() {
                // Startup owns every post-launch cleanup. Never drop its
                // future: only publish terminal generation completion after
                // it returns. If success won the cancellation race, this task
                // owns the handed-off client and must shut it down first.
                if let Ok(client) = result {
                    let _ = client.shutdown().await;
                }
                let _ = input
                    .send(ActorInput::ShutdownFinished { key, generation })
                    .await;
                return;
            }
            // FIFO on actor input is the lifecycle barrier: no client event
            // for this generation can overtake SessionStarted.
            if input
                .send(ActorInput::SessionStarted {
                    key: key.clone(),
                    generation,
                    result,
                })
                .await
                .is_err()
            {
                return;
            }
            while let Some(event) = client_event_rx.recv().await {
                if input
                    .send(ActorInput::ClientEvent {
                        key: key.clone(),
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
        let reserved = self.reserved_policy_capacity(&key);
        let available = self
            .state
            .sessions
            .get(&key)
            .map(|session| {
                SESSION_OUTBOX_CAPACITY
                    .saturating_sub(session.outbox.len())
                    .saturating_sub(reserved.operations)
            })
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
        let reserved = self.reserved_policy_capacity(key);
        let session = self
            .state
            .sessions
            .get(key)
            .ok_or(ManagerError::SessionUnavailable)?;
        if session
            .outbox
            .len()
            .saturating_add(operations.len())
            .saturating_add(reserved.operations)
            > SESSION_OUTBOX_CAPACITY
        {
            return Err(ManagerError::Overloaded);
        }
        self.session_delivery_progress
            .entry(key.clone())
            .or_default()
            .record_enqueued(&operations);
        let session = self.state.sessions.get_mut(key).expect("checked above");
        session.outbox.extend(operations);
        self.pump_session(key);
        Ok(())
    }

    /// True while a session still owes the worker slots a policy reserved for
    /// closes that have already been delivered. The group is finished, but the
    /// backpressure the policy paid for has not been spent yet.
    ///
    /// Only a session with a live worker can ever spend them. A crashed or
    /// failed-to-start session drops its outbox on the floor, and waiting on
    /// credits nobody will consume would wedge every deferred policy command.
    fn reserved_close_credits_outstanding(&self) -> bool {
        self.session_delivery_progress
            .iter()
            .any(|(key, progress)| {
                progress.credits_outstanding()
                    && self
                        .state
                        .sessions
                        .get(key)
                        .is_some_and(|session| session.worker.is_some())
            })
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
        item_id: Option<String>,
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
            item_id,
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
        // Completion triggers are the CURATED list merged with the server's,
        // per the adapter's normalization policy, so a server that advertises
        // none still opens on `.`; signature-help triggers are the server's
        // alone, because the catalog curates no list for them.
        let negotiated_triggers = session_key
            .as_ref()
            .and_then(|key| self.state.sessions.get(key))
            .map(|session| {
                let catalog = BundledServerCatalog::new();
                (
                    catalog.normalize_completion_triggers(
                        session.language,
                        &session.triggers.completion,
                    ),
                    session.triggers.signature_help.clone(),
                    session.triggers.signature_help_retrigger.clone(),
                )
            })
            .unwrap_or_default();
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
            // Gated on the same states as the capabilities: a trigger
            // character for a feature the status says is unavailable would
            // only make the frontend ask a session that cannot answer.
            completion_trigger_characters: if matches!(
                state,
                LspSessionState::Ready | LspSessionState::Indexing
            ) {
                negotiated_triggers.0
            } else {
                Vec::new()
            },
            signature_help_trigger_characters: if matches!(
                state,
                LspSessionState::Ready | LspSessionState::Indexing
            ) {
                negotiated_triggers.1
            } else {
                Vec::new()
            },
            signature_help_retrigger_characters: if matches!(
                state,
                LspSessionState::Ready | LspSessionState::Indexing
            ) {
                negotiated_triggers.2
            } else {
                Vec::new()
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
            completion_trigger_characters: Vec::new(),
            signature_help_trigger_characters: Vec::new(),
            signature_help_retrigger_characters: Vec::new(),
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
                self.generation_finished(key.clone(), generation);
                self.settle_stopped_close_group(&key, generation);
                self.retry_pending_policy_command();
            }
            ActorInput::ReservedCloseDeliverySucceeded { delivery } => {
                self.reserved_close_delivered(delivery)
            }
            ActorInput::ReservedCloseDeliveryFailed { delivery, message } => {
                self.reserved_close_delivery_failed(delivery, message)
            }
            ActorInput::WorkerReady { key, generation } => {
                let current = self
                    .state
                    .sessions
                    .get(&key)
                    .is_some_and(|session| session.generation == generation);
                if current {
                    if let Some(progress) = self.session_delivery_progress.get_mut(&key) {
                        progress.record_completed();
                    }
                    // Preserve the order of work already admitted to the
                    // session, then immediately spend newly freed outbox
                    // credit on the front policy reservation. New batches do
                    // not refill that credit while any policy remains ahead.
                    self.pump_session(&key);
                    // Only now is the slot this turn freed actually available.
                    // Draining before the pump would measure the outbox one
                    // operation fuller than it is about to be, refusing the
                    // owed close for no reason - and the batch flush below
                    // would take the slot instead, so under sustained editing
                    // the repair could be starved turn after turn. Nothing
                    // waits on it, so it does not need to go first; it only
                    // needs to go before the flush.
                    self.drain_repair_closes(&key);
                    self.retry_pending_policy_command();
                    if self.pending_close_groups.is_empty()
                        && self.pending_policy_commands.is_empty()
                        && !self.policy_async_in_flight
                    {
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
                    }
                }
            }
            ActorInput::CacheDeletionFinished { reply, result } => {
                if let Err(error) = &result {
                    log::warn!("LSP cache removal failed: {error}");
                }
                let _ = reply.send(result);
                self.policy_async_in_flight = false;
                self.retry_policy_requested = !self.pending_close_groups.is_empty()
                    || !self.pending_policy_commands.is_empty();
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
            && self.state.sessions.get(&key).is_some_and(|session| {
                session.generation == generation
                    && !session.restart_after_stop
                    && !session
                        .startup_cancel
                        .as_ref()
                        .is_some_and(CancellationToken::is_cancelled)
            });
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
                let negotiated_triggers = client.trigger_characters();
                let worker =
                    spawn_session_worker(key.clone(), generation, client, self.input_tx.clone());
                let documents = {
                    let session = self.state.sessions.get_mut(&key).expect("current session");
                    session.startup_cancel = None;
                    session.worker = Some(worker);
                    session.exit_observed = false;
                    session.capabilities = negotiated_capabilities;
                    session.triggers = negotiated_triggers;
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
                self.session_delivery_progress.remove(&key);
                self.pending_repair_closes.remove(&key);
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
        if let Some(shutdown) = self.shutdown_reply.as_mut() {
            shutdown.pending.remove(&(key, generation));
            if shutdown.pending.is_empty() {
                self.state.sessions.clear();
                self.fail_settling_close_groups();
                if let Some(shutdown) = self.shutdown_reply.take() {
                    for reply in shutdown.replies {
                        let _ = reply.send(Ok(()));
                    }
                }
                self.terminated = true;
            }
        }
    }

    fn fail_settling_close_groups(&mut self) {
        for group in std::mem::take(&mut self.settling_close_groups) {
            group.fail(ManagerError::ActorStopped);
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
            // No `source_version` to check: the response is one item, and its
            // freshness is already established by the guard above plus the
            // session's own generation cache.
            (PendingReply::ResolveCompletion(reply), RequestResult::ResolveCompletion(result)) => {
                let _ = reply.send((*result).map_err(ManagerError::Infrastructure));
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
        // The outbox went with the crash, so nothing that was queued on this
        // generation will ever run - including any close still owed for a
        // document the manager already let go of. The generation that held the
        // buffer is gone, so the debt is gone with it.
        self.session_delivery_progress.remove(&key);
        self.pending_repair_closes.remove(&key);
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
        let stopped_session_id = session.session_id.clone();
        self.session_delivery_progress.remove(key);
        self.pending_repair_closes.remove(key);
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
        self.emit_session_stopped(key, &stopped_session_id);
    }

    // The Problems tool window aggregates by project, so it has to hear that a
    // session ENDED — which no per-document status can tell it. The documents
    // may already be closed, and a session whose last state was `failed` or
    // `unavailable` never produces another document status at all, which is
    // exactly how a group was left behind on screen with no session under it.
    //
    // Session-level, so `document_id` is `None` and the per-pane store ignores
    // it; app-wide, because Problems can be popped out of the window that owned
    // the documents.
    fn emit_session_stopped(&mut self, key: &SessionKey, session_id: &str) {
        self.status_revision = self
            .status_revision
            .checked_add(1)
            .expect("LSP status revision exhausted");
        let status = LspStatus {
            revision: self.status_revision,
            document_id: None,
            session_id: Some(session_id.to_owned()),
            adapter_id: Some(key.adapter_id.clone()),
            project_root_uri: lsp::Url::from_file_path(&key.root)
                .ok()
                .map(|url| url.to_string()),
            state: LspSessionState::Stopped,
            message: None,
            unavailable_reason: None,
            capabilities: capabilities(false),
            completion_trigger_characters: Vec::new(),
            signature_help_trigger_characters: Vec::new(),
            signature_help_retrigger_characters: Vec::new(),
            error_count: 0,
            warning_count: 0,
        };
        self.emit(ManagerEvent::SessionStopped(status));
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
        for waiter in record.waiters {
            let event_window_label = if document_id.is_some() {
                record.window_label.clone()
            } else {
                waiter.window_label
            };
            self.fulfill_focus_obligation(
                waiter.delivery_key,
                ManagerEvent::DocumentOwnerFocused {
                    window_label: event_window_label,
                    document_id,
                    pane_id: pane_id.clone(),
                    canonical_path: Some(record.canonical_path.to_string_lossy().into_owned()),
                    reservation_failed: document_id.is_none(),
                },
            );
        }
    }

    fn begin_shutdown(&mut self, reply: oneshot::Sender<Result<(), ManagerError>>) {
        if let Some(shutdown) = self.shutdown_reply.as_mut() {
            if shutdown.replies.len() >= COMMAND_CAPACITY {
                let _ = reply.send(Err(ManagerError::Overloaded));
            } else {
                shutdown.replies.push(reply);
            }
            return;
        }
        self.shutting_down = true;
        for group in self.pending_close_groups.drain(..) {
            // A pending group never reached a session, so it can be failed at
            // once. A delivering group has notifications on the wire: its
            // caller is answered once, after the generations have stopped.
            if group.is_pending() {
                group.fail(ManagerError::ActorStopped);
            } else {
                self.settling_close_groups.push(group);
            }
        }
        for command in self.pending_policy_commands.drain(..) {
            command.fail_deferred(ManagerError::ActorStopped);
        }
        self.retry_policy_requested = false;
        self.policy_capacity_reservations.clear();
        self.policy_reservation_epoch = None;
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
            self.fail_settling_close_groups();
            let _ = reply.send(Ok(()));
            self.terminated = true;
            return;
        }
        self.shutdown_reply = Some(ShutdownReply {
            pending,
            replies: vec![reply],
        });
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
#[derive(Clone, Debug, PartialEq, Eq)]
struct CacheMountIdentity(Vec<u8>);

#[cfg(unix)]
struct CacheDescriptorMetadata {
    stat: libc::stat,
    mount_identity: CacheMountIdentity,
}

#[cfg(unix)]
trait CacheDescriptorMetadataSource {
    fn metadata(&self, fd: std::os::fd::RawFd) -> Result<CacheDescriptorMetadata, ManagerError>;
}

#[cfg(unix)]
struct SystemCacheDescriptorMetadata;

#[cfg(unix)]
impl CacheDescriptorMetadataSource for SystemCacheDescriptorMetadata {
    fn metadata(&self, fd: std::os::fd::RawFd) -> Result<CacheDescriptorMetadata, ManagerError> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } != 0 {
            return Err(ManagerError::Infrastructure(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(CacheDescriptorMetadata {
            stat: unsafe { stat.assume_init() },
            mount_identity: system_cache_mount_identity(fd)?,
        })
    }
}

#[cfg(unix)]
fn system_cache_mount_identity(fd: std::os::fd::RawFd) -> Result<CacheMountIdentity, ManagerError> {
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    {
        let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
        if unsafe { libc::fstatfs(fd, filesystem.as_mut_ptr()) } != 0 {
            return Err(ManagerError::Infrastructure(format!(
                "could not determine the cache mount identity: {}",
                std::io::Error::last_os_error()
            )));
        }
        let filesystem = unsafe { filesystem.assume_init() };
        let fsid = &filesystem.f_fsid;
        #[allow(unused_mut)]
        let mut identity = unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(fsid).cast::<u8>(),
                std::mem::size_of_val(fsid),
            )
            .to_vec()
        };

        // Linux bind mounts share both st_dev and f_fsid with their source.
        // statx's descriptor-relative mount ID distinguishes the mount point
        // itself, so a same-device bind cannot become a recursive boundary.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        {
            let mut statx_buffer = [0_u64; 32];
            let empty_path = b"\0";
            let result = unsafe {
                libc::syscall(
                    libc::SYS_statx,
                    fd,
                    empty_path.as_ptr().cast::<libc::c_char>(),
                    libc::AT_EMPTY_PATH | libc::AT_SYMLINK_NOFOLLOW,
                    libc::STATX_MNT_ID,
                    statx_buffer.as_mut_ptr().cast::<libc::c_void>(),
                )
            };
            if result != 0 {
                return Err(ManagerError::Infrastructure(format!(
                    "could not determine the cache mount identity: {}",
                    std::io::Error::last_os_error()
                )));
            }
            let bytes = statx_buffer.as_ptr().cast::<u8>();
            let mask = unsafe { std::ptr::read_unaligned(bytes.cast::<u32>()) };
            if mask & libc::STATX_MNT_ID == 0 {
                return Err(ManagerError::Infrastructure(
                    "could not determine the cache mount identity".into(),
                ));
            }
            // `stx_mnt_id` is the u64 at byte offset 144 in Linux's stable
            // `struct statx` ABI.
            let mount_id = unsafe { std::ptr::read_unaligned(bytes.add(144).cast::<u64>()) };
            identity.extend_from_slice(&mount_id.to_ne_bytes());
        }

        Ok(CacheMountIdentity(identity))
    }

    #[cfg(not(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "ios",
        target_os = "linux",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "openbsd"
    )))]
    {
        let _ = fd;
        Err(ManagerError::Infrastructure(
            "safe cache mount identity checks are unavailable on this Unix target".into(),
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
    remove_owned_cache_directory_anchored_with_metadata(
        cache_root,
        adapter_id,
        project_hash,
        disposable_name,
        &SystemCacheDescriptorMetadata,
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
    remove_owned_cache_directory_anchored_with_metadata(
        cache_root,
        adapter_id,
        project_hash,
        disposable_name,
        &SystemCacheDescriptorMetadata,
        before_identity_validation,
    )
}

#[cfg(unix)]
fn remove_owned_cache_directory_anchored_with_metadata(
    cache_root: &CacheRootAnchor,
    adapter_id: &str,
    project_hash: &str,
    disposable_name: &str,
    metadata_source: &dyn CacheDescriptorMetadataSource,
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

    fn same_identity(left: &libc::stat, right: &libc::stat) -> bool {
        left.st_dev == right.st_dev && left.st_ino == right.st_ino
    }

    fn require_boundary(
        actual: &CacheDescriptorMetadata,
        expected_device: libc::dev_t,
        expected_mount: &CacheMountIdentity,
    ) -> Result<(), ManagerError> {
        ensure_cache_device(expected_device, actual.stat.st_dev)?;
        if &actual.mount_identity != expected_mount {
            return Err(ManagerError::Infrastructure(
                "refused to cross a cache filesystem mount boundary".into(),
            ));
        }
        Ok(())
    }

    fn remove_contents(
        directory: &File,
        expected_device: libc::dev_t,
        expected_mount: &CacheMountIdentity,
        metadata_source: &dyn CacheDescriptorMetadataSource,
    ) -> Result<(), ManagerError> {
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
            ensure_cache_device(expected_device, before.st_dev)?;
            if before.st_mode & libc::S_IFMT == libc::S_IFDIR {
                let child =
                    open_directory_at(directory.as_raw_fd(), OsStr::from_bytes(name.to_bytes()))?
                        .ok_or_else(|| {
                        ManagerError::Infrastructure("cache entry changed during removal".into())
                    })?;
                let child_metadata = metadata_source.metadata(child.as_raw_fd())?;
                require_boundary(&child_metadata, expected_device, expected_mount)?;
                if !same_identity(&before, &child_metadata.stat) {
                    return Err(ManagerError::Infrastructure(
                        "cache entry changed during removal".into(),
                    ));
                }
                remove_contents(&child, expected_device, expected_mount, metadata_source)?;
                let current = stat_at(directory.as_raw_fd(), &name)?;
                ensure_cache_device(expected_device, current.st_dev)?;
                let child_metadata = metadata_source.metadata(child.as_raw_fd())?;
                require_boundary(&child_metadata, expected_device, expected_mount)?;
                if !same_identity(&current, &child_metadata.stat) {
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
    let root_metadata = metadata_source.metadata(root.as_raw_fd())?;
    let expected_device = root_metadata.stat.st_dev;
    let expected_mount = root_metadata.mount_identity;
    let Some(lsp) = open_directory_at(root.as_raw_fd(), OsStr::new("lsp"))? else {
        return Ok(());
    };
    require_boundary(
        &metadata_source.metadata(lsp.as_raw_fd())?,
        expected_device,
        &expected_mount,
    )?;
    let Some(adapter) = open_directory_at(lsp.as_raw_fd(), OsStr::new(adapter_id))? else {
        return Ok(());
    };
    require_boundary(
        &metadata_source.metadata(adapter.as_raw_fd())?,
        expected_device,
        &expected_mount,
    )?;
    let Some(project) = open_directory_at(adapter.as_raw_fd(), OsStr::new(project_hash))? else {
        return Ok(());
    };
    require_boundary(
        &metadata_source.metadata(project.as_raw_fd())?,
        expected_device,
        &expected_mount,
    )?;
    let Some(target) = open_directory_at(project.as_raw_fd(), OsStr::new(disposable_name))? else {
        return Ok(());
    };
    let target_metadata = metadata_source.metadata(target.as_raw_fd())?;
    require_boundary(&target_metadata, expected_device, &expected_mount)?;
    before_identity_validation();
    let target_name = CString::new(disposable_name).expect("fixed cache component");
    if !same_identity(
        &stat_at(project.as_raw_fd(), &target_name)?,
        &target_metadata.stat,
    ) {
        return Err(ManagerError::Infrastructure(
            "cache target changed before removal".into(),
        ));
    }
    remove_contents(&target, expected_device, &expected_mount, metadata_source)?;
    let target_metadata = metadata_source.metadata(target.as_raw_fd())?;
    require_boundary(&target_metadata, expected_device, &expected_mount)?;
    if !same_identity(
        &stat_at(project.as_raw_fd(), &target_name)?,
        &target_metadata.stat,
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
    let (close_deliveries, mut close_delivery_rx) =
        mpsc::channel::<(String, ReservedCloseDelivery)>(SESSION_OPERATION_CAPACITY);
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
    let delivery_client = client.clone();
    let delivery_input = input.clone();
    let delivery_stopping = stopping.clone();
    tokio::spawn(async move {
        // One close at a time, so a group observes its members settle in the
        // order it staged them and stops at the first failure.
        loop {
            let (document_id, delivery) = tokio::select! {
                biased;
                _ = delivery_stopping.cancelled() => break,
                next = close_delivery_rx.recv() => {
                    let Some(next) = next else { break };
                    next
                }
            };
            let result = tokio::select! {
                _ = delivery_stopping.cancelled() => break,
                result = delivery_client.did_close(&document_id) => result,
            };
            match result {
                Ok(()) => {
                    let _ = delivery_input
                        .send(ActorInput::ReservedCloseDeliverySucceeded { delivery })
                        .await;
                }
                Err(message) => {
                    let _ = delivery_input
                        .send(ActorInput::ReservedCloseDeliveryFailed { delivery, message })
                        .await;
                    break;
                }
            }
        }
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
                SessionOperation::ReservedCloseCredit { .. } => {}
                SessionOperation::DidClose {
                    document_id,
                    delivery,
                } => {
                    let result = tokio::select! {
                        _ = stopping.cancelled() => break,
                        result = client.did_close(&document_id) => result,
                    };
                    // A close that a deferred group staged reports through its
                    // own delivery token so the actor can tell it apart from
                    // every other close of the same document.
                    match (result, delivery) {
                        (Ok(()), None) => {}
                        (Ok(()), Some(delivery)) => {
                            let _ = input
                                .send(ActorInput::ReservedCloseDeliverySucceeded { delivery })
                                .await;
                        }
                        (Err(message), None) => {
                            send_operation_error(&input, &key, generation, "didClose", message)
                                .await;
                        }
                        (Err(message), Some(delivery)) => {
                            let _ = input
                                .send(ActorInput::ReservedCloseDeliveryFailed { delivery, message })
                                .await;
                        }
                    }
                }
                SessionOperation::Request {
                    request_id,
                    kind,
                    document_id,
                    position,
                    item_id,
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
                                RequestKind::ResolveCompletion => RequestResult::ResolveCompletion(
                                    Box::new(match item_id.as_deref() {
                                        Some(id) => client.resolve_completion(id).await,
                                        None => Err("completion resolve without an item".into()),
                                    }),
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
        close_deliveries,
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
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::Duration;

    use async_lsp::lsp_types as lsp;
    use async_trait::async_trait;
    use tempfile::TempDir;
    use tokio::sync::{Notify, Semaphore, mpsc};

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
        ResolveCompletion(String),
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
        step_changes: AtomicBool,
        change_steps: OnceLock<Semaphore>,
        completed_changes: AtomicUsize,
        step_closes: AtomicBool,
        close_steps: OnceLock<Semaphore>,
        fail_next_close: AtomicBool,
        block_starts: AtomicBool,
        starts_released: Notify,
        block_first_pull: AtomicBool,
        first_pull_released: Notify,
        pull_calls: AtomicUsize,
        fail_next_change: AtomicBool,
        immediate_exit: AtomicBool,
        fail_start: AtomicBool,
        hold_startup_cleanup: AtomicBool,
        startup_cleanups_started: AtomicUsize,
        startup_cleanups_completed: AtomicUsize,
        startup_owners_aborted: AtomicUsize,
        startup_cleanups_released: Notify,
    }

    struct FakeStartupOwner {
        factory: Arc<FakeFactory>,
        armed: bool,
    }

    impl Drop for FakeStartupOwner {
        fn drop(&mut self) {
            if self.armed {
                self.factory
                    .startup_owners_aborted
                    .fetch_add(1, Ordering::SeqCst);
                self.factory
                    .startup_cleanups_started
                    .fetch_add(1, Ordering::SeqCst);
            }
        }
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

        fn hold_changes_one_at_a_time(&self) {
            self.step_changes.store(true, Ordering::SeqCst);
        }

        fn release_one_change(&self) {
            self.change_steps
                .get_or_init(|| Semaphore::new(0))
                .add_permits(1);
        }

        fn release_all_changes(&self) {
            self.step_changes.store(false, Ordering::SeqCst);
            self.change_steps
                .get_or_init(|| Semaphore::new(0))
                .add_permits(super::SESSION_OUTBOX_CAPACITY * 2);
            self.release_changes();
        }

        fn hold_closes_one_at_a_time(&self) {
            self.step_closes.store(true, Ordering::SeqCst);
        }

        fn release_one_close(&self) {
            self.close_steps
                .get_or_init(|| Semaphore::new(0))
                .add_permits(1);
        }

        fn fail_next_close(&self) {
            self.fail_next_close.store(true, Ordering::SeqCst);
        }

        fn hold_starts(&self) {
            self.block_starts.store(true, Ordering::SeqCst);
        }

        fn hold_starts_with_delayed_cleanup(&self) {
            self.hold_starts();
            self.hold_startup_cleanup.store(true, Ordering::SeqCst);
        }

        async fn wait_for_startup_cleanup(&self) {
            for _ in 0..100 {
                if self.startup_cleanups_started.load(Ordering::SeqCst) > 0 {
                    return;
                }
                tokio::task::yield_now().await;
            }
            panic!("cancelled startup never entered owned cleanup");
        }

        fn release_startup_cleanup(&self) {
            self.hold_startup_cleanup.store(false, Ordering::SeqCst);
            self.startup_cleanups_released.notify_waiters();
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

        // What a real typescript-language-server advertises. `::` is not in
        // the curated list, which is what makes the merge visible in the
        // status assertions below.
        fn trigger_characters(&self) -> crate::lsp::types::NegotiatedTriggers {
            crate::lsp::types::NegotiatedTriggers {
                completion: vec![".".to_owned(), "::".to_owned()],
                signature_help: vec!["(".to_owned(), ",".to_owned()],
                signature_help_retrigger: vec![",".to_owned()],
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
            if self.factory.step_changes.load(Ordering::SeqCst) {
                self.factory
                    .change_steps
                    .get_or_init(|| Semaphore::new(0))
                    .acquire()
                    .await
                    .expect("change-step semaphore remains open")
                    .forget();
            } else {
                self.factory
                    .wait_if_held(&self.factory.block_changes, &self.factory.changes_released)
                    .await;
            }
            self.factory
                .completed_changes
                .fetch_add(1, Ordering::SeqCst);
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
            if self.factory.step_closes.load(Ordering::SeqCst) {
                self.factory
                    .close_steps
                    .get_or_init(|| Semaphore::new(0))
                    .acquire()
                    .await
                    .expect("close-step semaphore remains open")
                    .forget();
            }
            if self.factory.fail_next_close.swap(false, Ordering::SeqCst) {
                return Err("didClose failed with source text: secret".into());
            }
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

        async fn resolve_completion(
            &self,
            item_id: &str,
        ) -> Result<crate::lsp::types::CompletionItem, String> {
            self.factory
                .record(&self.key, Observation::ResolveCompletion(item_id.into()));
            self.factory
                .wait_if_held(
                    &self.factory.block_requests,
                    &self.factory.requests_released,
                )
                .await;
            Ok(crate::lsp::types::CompletionItem {
                id: item_id.into(),
                label: "resolved".into(),
                detail: Some("resolved detail".into()),
                kind: Some("variable".into()),
                documentation: Vec::new(),
                sort_text: None,
                filter_text: None,
                insert_text: None,
                is_snippet: false,
                text_edit: None,
                additional_text_edits: Vec::new(),
                commit_characters: Vec::new(),
                deprecated: false,
                unsupported_effects: Vec::new(),
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
            cancellation: tokio_util::sync::CancellationToken,
        ) -> Result<Arc<dyn SessionClient>, ManagerError> {
            let mut startup_owner = FakeStartupOwner {
                factory: self.clone(),
                armed: true,
            };
            self.launches.lock().unwrap().push(start.clone());
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    self.startup_cleanups_started.fetch_add(1, Ordering::SeqCst);
                    self.wait_if_held(
                        &self.hold_startup_cleanup,
                        &self.startup_cleanups_released,
                    ).await;
                    self.startup_cleanups_completed.fetch_add(1, Ordering::SeqCst);
                    startup_owner.armed = false;
                    return Err(ManagerError::Cancelled);
                }
                _ = self.wait_if_held(&self.block_starts, &self.starts_released) => {}
            }
            startup_owner.armed = false;
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

    fn fill_normal_command_queue(
        manager: &LspManagerHandle,
    ) -> Vec<tokio::sync::oneshot::Receiver<Result<Vec<crate::lsp::types::LspStatus>, ManagerError>>>
    {
        (0..super::COMMAND_CAPACITY)
            .map(|_| {
                let (reply, result) = tokio::sync::oneshot::channel();
                manager
                    .commands
                    .try_send(super::ManagerCommand::StatusSnapshot {
                        document_id: None,
                        reply,
                    })
                    .expect("the actor command queue must begin empty");
                result
            })
            .collect()
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

    fn fake_managed_session(
        session_id: &str,
        generation: u64,
        document: DocumentId,
    ) -> super::ManagedSession {
        let (operations, operations_rx) = mpsc::channel(super::SESSION_OPERATION_CAPACITY);
        let (close_deliveries, close_rx) = mpsc::channel(super::SESSION_OPERATION_CAPACITY);
        let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
        // The receivers are never read in these actor-only tests; keeping them
        // alive stops the senders from reporting a closed channel.
        std::mem::forget((operations_rx, close_rx, shutdown_rx));
        super::ManagedSession {
            session_id: session_id.into(),
            language: LanguageId::TypeScript,
            generation,
            worker: Some(super::SessionWorkerHandle {
                operations,
                close_deliveries,
                shutdown: Some(shutdown),
            }),
            startup_cancel: None,
            outbox: std::collections::VecDeque::new(),
            restart_after_stop: false,
            documents: [document].into_iter().collect(),
            idle_generation: 0,
            crash_timestamps: std::collections::VecDeque::new(),
            automatic_restart_blocked: false,
            exit_observed: false,
            logs: std::collections::VecDeque::new(),
            next_log_sequence: 1,
            capabilities: super::capabilities(false),
            triggers: Default::default(),
            protocol_started: true,
        }
    }

    struct ReservedCloseFixture {
        harness: ManagerHarness,
        paths: [PathBuf; 4],
        documents: [DocumentId; 4],
        root: PathBuf,
        policy: tokio::task::JoinHandle<Result<(), ManagerError>>,
    }

    async fn reserved_close_fixture(prefix: &str) -> ReservedCloseFixture {
        let harness = ManagerHarness::new();
        let paths = [
            harness.file(&format!("{prefix}-pressure.ts"), "pressure"),
            harness.file(&format!("{prefix}-a.ts"), "a"),
            harness.file(&format!("{prefix}-b.ts"), "b"),
            harness.file(&format!("{prefix}-c.ts"), "c"),
        ];
        let mut documents = Vec::new();
        for (index, path) in paths.iter().enumerate() {
            documents.push(harness.open(path, "main", &format!("pane-{index}")).await);
        }
        let documents: [DocumentId; 4] = documents.try_into().unwrap();
        let root = harness.root.clone();
        harness
            .choose_and_trust(documents[0], &root, "typescript")
            .await;
        spin().await;
        harness.factory.hold_changes_one_at_a_time();
        saturate_session_outbox(&harness, documents[0]).await;
        let policy = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::none()).await }
        });
        spin().await;
        assert!(!policy.is_finished(), "fixture policy must reserve closes");
        ReservedCloseFixture {
            harness,
            paths,
            documents,
            root,
            policy,
        }
    }

    async fn assert_document_owner(harness: &ManagerHarness, path: &Path, expected: DocumentId) {
        assert!(matches!(
            harness
                .manager
                .reserve_document(path.to_path_buf(), "observer".into())
                .await
                .unwrap(),
            ReserveResult::FocusOwner { document_id, .. } if document_id == expected
        ));
    }

    async fn wait_until_finished<T>(task: &tokio::task::JoinHandle<T>) {
        for _ in 0..100 {
            spin().await;
            if task.is_finished() {
                return;
            }
        }
    }

    async fn wait_for_observation_count(
        factory: &FakeFactory,
        session_key: &SessionKey,
        expected: usize,
        matches_observation: impl Fn(&Observation) -> bool,
    ) {
        for _ in 0..300 {
            if factory
                .observations(session_key)
                .iter()
                .filter(|observation| matches_observation(observation))
                .count()
                >= expected
            {
                return;
            }
            spin().await;
        }
        panic!(
            "expected {expected} matching observations, got {:?}",
            factory.observations(session_key)
        );
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
            ManagerEvent::DocumentOwnerFocused { ref window_label, document_id: Some(id), ref pane_id, reservation_failed: false, .. }
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
    async fn pending_focus_failure_returns_to_the_requesting_window_with_retry_path() {
        let harness = ManagerHarness::new();
        let path = harness.root.join("retry-after-owner-open-fails.ts");
        let ReserveResult::Reserved {
            reservation_id,
            canonical_path: expected_path,
        } = harness
            .manager
            .reserve_document(path.clone(), "owner".into())
            .await
            .unwrap()
        else {
            panic!("first request must reserve")
        };
        assert!(matches!(
            harness
                .manager
                .reserve_document(path, "requester".into())
                .await
                .unwrap(),
            ReserveResult::FocusPending { .. }
        ));
        harness
            .manager
            .release_document(reservation_id)
            .await
            .unwrap();
        assert!(matches!(
            harness.next_owner_event().await,
            ManagerEvent::DocumentOwnerFocused {
                ref window_label,
                canonical_path: Some(ref retry_path),
                reservation_failed: true,
                ..
            } if window_label == "requester" && retry_path == &expected_path
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn every_pending_requester_receives_one_failure_for_the_shared_reservation() {
        let harness = ManagerHarness::new();
        let path = harness.root.join("shared-open-failure.ts");
        let ReserveResult::Reserved {
            reservation_id,
            canonical_path,
        } = harness
            .manager
            .reserve_document(path.clone(), "owner".into())
            .await
            .unwrap()
        else {
            panic!("first request must reserve")
        };
        for requester in ["requester-a", "requester-b", "requester-c"] {
            assert!(matches!(
                harness
                    .manager
                    .reserve_document(path.clone(), requester.into())
                    .await
                    .unwrap(),
                ReserveResult::FocusPending { .. }
            ));
        }
        harness
            .manager
            .release_document(reservation_id)
            .await
            .unwrap();

        for _ in 0..10 {
            spin().await;
        }
        let mut recipients = Vec::new();
        let mut events = harness.events.lock().await;
        while let Ok(event) = events.try_recv() {
            if let ManagerEvent::DocumentOwnerFocused {
                window_label,
                canonical_path: Some(event_path),
                reservation_failed: true,
                ..
            } = event
            {
                assert_eq!(event_path, canonical_path);
                recipients.push(window_label);
            }
        }
        recipients.sort();
        assert_eq!(recipients, ["requester-a", "requester-b", "requester-c"]);
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn every_alias_waiter_receives_one_success_for_the_shared_owner() {
        use std::os::unix::fs::symlink;

        let harness = ManagerHarness::new();
        let path = harness.file("shared-open-success.ts", "let shared = true;");
        let ReserveResult::Reserved { reservation_id, .. } = harness
            .manager
            .reserve_document(path.clone(), "owner".into())
            .await
            .unwrap()
        else {
            panic!("first request must reserve")
        };
        for (index, requester) in ["requester-a", "requester-b", "requester-c"]
            .into_iter()
            .enumerate()
        {
            let alias = harness
                .root
                .join(format!("shared-success-alias-{index}.ts"));
            symlink(&path, &alias).unwrap();
            assert!(matches!(
                harness
                    .manager
                    .reserve_document(alias, requester.into())
                    .await
                    .unwrap(),
                ReserveResult::FocusPending { .. }
            ));
        }
        let opened = harness
            .manager
            .open_document(
                reservation_id,
                "owner-pane".into(),
                "let shared = true;".into(),
                "typescript".into(),
            )
            .await
            .unwrap();
        let document_id: DocumentId =
            serde_json::from_value(serde_json::Value::String(opened.document_id)).unwrap();

        for _ in 0..3 {
            assert!(matches!(
                harness.next_owner_event().await,
                ManagerEvent::DocumentOwnerFocused {
                    ref window_label,
                    document_id: Some(owner),
                    ref pane_id,
                    reservation_failed: false,
                    ..
                } if window_label == "owner" && owner == document_id && pane_id.as_deref() == Some("owner-pane")
            ));
        }
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

    // The Problems tool window groups by project, and a group only disappears
    // when its last session stops. Per-document statuses cannot say that: a
    // session whose documents were all closed, or whose last state was
    // `failed`, leaves no document to carry the news. So stopping a session
    // emits one session-level status of its own.
    async fn stopped_statuses(harness: &ManagerHarness) -> Vec<crate::lsp::types::LspStatus> {
        let mut stopped = Vec::new();
        let mut events = harness.events.lock().await;
        while let Ok(event) = events.try_recv() {
            if let ManagerEvent::SessionStopped(status) = event {
                stopped.push(status);
            }
        }
        stopped
    }

    #[tokio::test(start_paused = true)]
    async fn stopping_a_session_emits_one_terminal_session_status() {
        let harness = ManagerHarness::new();
        let path = harness.file("a.ts", "let a = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        let session_id = harness
            .manager
            .status_snapshot(Some(document))
            .await
            .unwrap()[0]
            .session_id
            .clone();
        assert!(session_id.is_some(), "the session started");

        harness
            .manager
            .set_enablement(Enablement::none())
            .await
            .unwrap();
        spin().await;

        let stopped = stopped_statuses(&harness).await;
        assert_eq!(stopped.len(), 1, "exactly one terminal status per stop");
        let status = &stopped[0];
        assert_eq!(status.state, LspSessionState::Stopped);
        assert_eq!(
            status.session_id, session_id,
            "names the session that ended"
        );
        assert!(
            status.document_id.is_none(),
            "session-level, not another per-document status"
        );
        assert_eq!(status.adapter_id.as_deref(), Some("typescript"));
        assert!(
            status.project_root_uri.is_some(),
            "the Problems group is keyed by its root"
        );
        assert!(
            !status.capabilities.definition,
            "a stopped session can do nothing"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn revoking_trust_emits_the_terminal_session_status_too() {
        let harness = ManagerHarness::new();
        let path = harness.file("a.ts", "let a = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        harness
            .manager
            .set_project_trust(root.clone(), None, TrustDecision::Revoked)
            .await
            .unwrap();
        spin().await;

        let stopped = stopped_statuses(&harness).await;
        assert_eq!(
            stopped.len(),
            1,
            "revocation stops the session exactly once"
        );
        assert_eq!(stopped[0].state, LspSessionState::Stopped);
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
    async fn committed_open_degrades_when_lsp_reevaluation_is_overloaded_and_later_close_releases()
    {
        let harness = ManagerHarness::new();
        let root = harness.root.clone();
        let first_path = harness.file("pressure-owner.ts", "let first = 1;");
        let first = harness.open(&first_path, "main", "first").await;
        harness.choose_and_trust(first, &root, "typescript").await;
        let _ = saturate_session_outbox(&harness, first).await;

        let second_path = harness.file("committed-open.ts", "let second = 2;");
        let ReserveResult::Reserved { reservation_id, .. } = harness
            .manager
            .reserve_document(second_path.clone(), "main".into())
            .await
            .unwrap()
        else {
            panic!("new document must reserve")
        };
        let opened = harness
            .manager
            .open_document(
                reservation_id,
                "second".into(),
                "let second = 2;".into(),
                "typescript".into(),
            )
            .await
            .expect("ownership commit must not be reported as an open failure");
        let document: DocumentId =
            serde_json::from_value(serde_json::Value::String(opened.document_id)).unwrap();
        assert_eq!(opened.status.state, LspSessionState::Failed);
        assert!(matches!(
            harness
                .manager
                .reserve_document(second_path.clone(), "popup".into())
                .await
                .unwrap(),
            ReserveResult::FocusOwner { document_id, .. } if document_id == document
        ));

        harness.factory.release_changes();
        for _ in 0..400 {
            spin().await;
        }
        harness.manager.close_document(document).await.unwrap();
        assert!(matches!(
            harness
                .manager
                .reserve_document(second_path, "popup".into())
                .await
                .unwrap(),
            ReserveResult::Reserved { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn committed_transfer_returns_new_project_metadata_when_reevaluation_is_overloaded() {
        let harness = ManagerHarness::new();
        let target_root = harness.root.join("project-b");
        std::fs::create_dir_all(&target_root).unwrap();
        std::fs::write(target_root.join("tsconfig.json"), "{}").unwrap();
        let pressure_path = harness.file("project-b/pressure.ts", "let pressure = 1;");
        let pressure = harness.open(&pressure_path, "main", "pressure").await;
        harness
            .choose_and_trust(pressure, &target_root, "typescript")
            .await;
        let _ = saturate_session_outbox(&harness, pressure).await;

        let source_path = harness.file("source.txt", "plain");
        let source = harness.open(&source_path, "main", "source").await;
        let target_path = harness.file("project-b/transferred.ts", "plain");
        let ReserveResult::Reserved { reservation_id, .. } = harness
            .manager
            .reserve_document(target_path.clone(), "main".into())
            .await
            .unwrap()
        else {
            panic!("target must reserve")
        };
        let transferred = harness
            .manager
            .transfer_document(source, reservation_id, "main".into(), "source".into())
            .await
            .expect("committed transfer must return metadata, not an LSP error");
        assert_eq!(transferred.document_id, super::document_id_text(source));
        assert_eq!(transferred.status.state, LspSessionState::Failed);
        assert!(
            transferred
                .project_candidates
                .iter()
                .any(|candidate| candidate.canonical_path
                    == target_root.canonicalize().unwrap().to_string_lossy())
        );
        assert!(matches!(
            harness
                .manager
                .reserve_document(target_path, "popup".into())
                .await
                .unwrap(),
            ReserveResult::FocusOwner { document_id, .. } if document_id == source
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn overloaded_close_keeps_the_single_owner_until_a_confirmed_retry_releases_it() {
        let harness = ManagerHarness::new();
        let path = harness.file("retry-close.ts", "let value = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        let _ = saturate_session_outbox(&harness, document).await;

        assert_eq!(
            harness.manager.close_document(document).await,
            Err(ManagerError::Overloaded)
        );
        assert!(matches!(
            harness
                .manager
                .reserve_document(path.clone(), "popup".into())
                .await
                .unwrap(),
            ReserveResult::FocusOwner { document_id, .. } if document_id == document
        ));

        harness.factory.release_changes();
        for _ in 0..400 {
            spin().await;
        }
        harness.manager.close_document(document).await.unwrap();
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
    async fn multi_document_close_is_atomic_when_later_close_capacity_is_unavailable() {
        let harness = ManagerHarness::new();
        let first_path = harness.file("atomic-tab-close-a.ts", "let a = 1;");
        let second_path = harness.file("atomic-tab-close-b.ts", "let b = 2;");
        let first = harness.open(&first_path, "main", "pane-a").await;
        let second = harness.open(&second_path, "main", "pane-b").await;
        let root = harness.root.clone();
        harness.choose_and_trust(first, &root, "typescript").await;
        spin().await;
        let _ = saturate_session_outbox(&harness, second).await;

        assert_eq!(
            harness.manager.close_documents(vec![first, second]).await,
            Err(ManagerError::Overloaded)
        );
        for (path, expected) in [(&first_path, first), (&second_path, second)] {
            assert!(matches!(
                harness
                    .manager
                    .reserve_document(path.clone(), "popup".into())
                    .await
                    .unwrap(),
                ReserveResult::FocusOwner { document_id, .. } if document_id == expected
            ));
        }

        harness.factory.release_changes();
        for _ in 0..400 {
            spin().await;
        }
        harness
            .manager
            .close_documents(vec![first, second])
            .await
            .unwrap();
        for path in [first_path, second_path] {
            assert!(matches!(
                harness
                    .manager
                    .reserve_document(path, "popup".into())
                    .await
                    .unwrap(),
                ReserveResult::Reserved { .. }
            ));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn completion_resolve_reaches_the_session_and_is_discarded_when_the_document_moves_on() {
        let harness = ManagerHarness::new();
        let path = harness.file("a.ts", "let value = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;

        let resolved = harness
            .manager
            .resolve_completion_item(document, "item-7".into())
            .await
            .expect("an attached document resolves through its session");
        assert_eq!(resolved.id, "item-7", "the session was asked for that item");
        assert_eq!(resolved.detail.as_deref(), Some("resolved detail"));
        assert!(
            harness
                .factory
                .observations(&key("typescript", &root))
                .iter()
                .any(|observation| matches!(
                    observation,
                    Observation::ResolveCompletion(id) if id == "item-7"
                )),
            "the request really reached the session client"
        );

        // A resolve whose document has moved on is stale, exactly like the
        // positional requests: the popup it belonged to is gone.
        harness.factory.hold_requests();
        let pending = tokio::spawn({
            let manager = harness.manager.clone();
            async move {
                manager
                    .resolve_completion_item(document, "item-7".into())
                    .await
            }
        });
        spin().await;
        harness
            .manager
            .apply_changes(document, batch(document, 1, 2, "z"))
            .await
            .unwrap();
        harness.factory.release_requests();
        assert_eq!(pending.await.unwrap(), Err(ManagerError::StaleResponse));
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
        // Before a project is chosen there is no session, so the status must
        // advertise neither the features nor the characters that would open
        // them: a trigger for a feature nothing can answer only produces a
        // request the frontend has to throw away.
        let unattached = harness
            .manager
            .status_snapshot(Some(document))
            .await
            .unwrap()
            .remove(0);
        assert!(!unattached.capabilities.completion);
        assert!(unattached.completion_trigger_characters.is_empty());
        assert!(unattached.signature_help_trigger_characters.is_empty());
        assert!(unattached.signature_help_retrigger_characters.is_empty());
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
        // Completion triggers are the CURATED list merged with the server's,
        // in that order; signature-help triggers are the server's alone.
        assert_eq!(
            ready.completion_trigger_characters,
            vec![
                ".".to_owned(),
                "'".to_owned(),
                "\"".to_owned(),
                "/".to_owned(),
                "@".to_owned(),
                "<".to_owned(),
                "::".to_owned(),
            ],
            "the curated list opens completion even where the server named nothing"
        );
        assert_eq!(
            ready.signature_help_trigger_characters,
            vec!["(".to_owned(), ",".to_owned()]
        );
        assert_eq!(
            ready.signature_help_retrigger_characters,
            vec![",".to_owned()]
        );
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
    #[test]
    fn cache_deletion_rejects_injected_same_device_mount_identity_change() {
        use std::os::fd::RawFd;
        use std::os::unix::fs::MetadataExt;

        struct InjectedMountBoundary {
            device: libc::dev_t,
            inode: libc::ino_t,
        }

        impl super::CacheDescriptorMetadataSource for InjectedMountBoundary {
            fn metadata(&self, fd: RawFd) -> Result<super::CacheDescriptorMetadata, ManagerError> {
                let mut metadata = super::CacheDescriptorMetadataSource::metadata(
                    &super::SystemCacheDescriptorMetadata,
                    fd,
                )?;
                if metadata.stat.st_dev == self.device && metadata.stat.st_ino == self.inode {
                    metadata.mount_identity = super::CacheMountIdentity(vec![0xa5; 32]);
                }
                Ok(metadata)
            }
        }

        let temp = TempDir::new().unwrap();
        let cache_root = temp.path().join("cache-root");
        let project = temp.path().join("project");
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let paths = BundledServerCatalog::new()
            .cache_paths(LanguageId::TypeScript, &project, &cache_root)
            .unwrap();
        let boundary = paths.cache_dir.join("same-device-mounted-child");
        std::fs::create_dir_all(&boundary).unwrap();
        let preserved = boundary.join("must-survive");
        std::fs::write(&preserved, "keep").unwrap();
        let boundary_metadata = std::fs::metadata(&boundary).unwrap();
        let injected = InjectedMountBoundary {
            device: boundary_metadata.dev() as libc::dev_t,
            inode: boundary_metadata.ino(),
        };
        let project_hash = paths
            .cache_dir
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        let anchor = super::CacheRootAnchor::capture(&cache_root);

        let result = super::remove_owned_cache_directory_anchored_with_metadata(
            &anchor,
            "typescript",
            project_hash,
            "cache",
            &injected,
            || {},
        );

        assert!(matches!(
            result,
            Err(ManagerError::Infrastructure(message)) if message.contains("mount boundary")
        ));
        assert!(preserved.exists());
        assert!(paths.cache_dir.exists());
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
    async fn priority_shutdown_joins_a_generation_already_shutting_down() {
        let harness = ManagerHarness::new();
        let path = harness.file("join-shutdown.ts", "let value = 1;");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .choose_and_trust(document, &root, "typescript")
            .await;
        harness.factory.hold_shutdowns();

        let first = tokio::spawn({
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
        let joining = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.shutdown().await }
        });
        spin().await;
        assert!(
            !joining.is_finished(),
            "priority shutdown returned before the live generation exited"
        );

        harness.factory.release_shutdowns();
        first.await.unwrap().unwrap();
        joining.await.unwrap().unwrap();
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

    #[tokio::test]
    async fn shutdown_waits_for_cancelled_startup_owner_to_reap() {
        let harness = ManagerHarness::new();
        harness.factory.hold_starts_with_delayed_cleanup();
        let path = harness.file("shutdown-startup-cleanup.ts", "x");
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
        assert_eq!(harness.factory.launch_count("typescript", &harness.root), 1);

        let shutdown = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.shutdown().await }
        });
        harness.factory.wait_for_startup_cleanup().await;
        assert!(
            !shutdown.is_finished(),
            "generation shutdown finished while startup cleanup still owned a live process"
        );
        assert_eq!(
            harness
                .factory
                .startup_cleanups_completed
                .load(Ordering::SeqCst),
            0
        );

        harness.factory.release_startup_cleanup();
        shutdown.await.unwrap().unwrap();
        assert_eq!(
            harness
                .factory
                .startup_cleanups_completed
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(
            harness
                .factory
                .startup_owners_aborted
                .load(Ordering::SeqCst),
            0,
            "the manager must not drop the startup cleanup future"
        );
    }

    #[tokio::test]
    async fn manual_restart_waits_for_cancelled_startup_reap_before_replacement_launch() {
        let harness = ManagerHarness::new();
        harness.factory.hold_starts_with_delayed_cleanup();
        let path = harness.file("restart-startup-cleanup.ts", "x");
        let document = harness.open(&path, "main", "pane").await;
        let root = harness.root.clone();
        harness
            .manager
            .set_project_context(document, ProjectContextChoice::root(root.clone()))
            .await
            .unwrap();
        harness
            .manager
            .set_project_trust(
                root.clone(),
                Some("typescript".into()),
                TrustDecision::Trusted,
            )
            .await
            .unwrap();
        spin().await;
        assert_eq!(harness.factory.launch_count("typescript", &root), 1);

        harness
            .manager
            .restart_session("typescript".into(), root.clone())
            .await
            .unwrap();
        harness.factory.wait_for_startup_cleanup().await;
        assert_eq!(
            harness.factory.launch_count("typescript", &root),
            1,
            "replacement launch overlapped the old startup cleanup"
        );

        harness.factory.release_startup_cleanup();
        for _ in 0..100 {
            if harness.factory.launch_count("typescript", &root) == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            harness
                .factory
                .startup_cleanups_completed
                .load(Ordering::SeqCst),
            1,
            "old process reap must precede generation finish"
        );
        assert_eq!(harness.factory.launch_count("typescript", &root), 2);
        harness.factory.release_starts();
        spin().await;
        harness.manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn manual_restart_rejects_a_successful_start_queued_before_the_command_wins() {
        let temp = TempDir::new().unwrap();
        let config_root = temp.path().join("config");
        let cache_root = temp.path().join("cache");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&project_root).unwrap();
        let factory = Arc::new(FakeFactory::default());
        factory.hold_shutdowns();
        let (manager, mut actor, _events) =
            LspManager::new(factory.clone(), config_root, cache_root, Enablement::all());
        let session_key = SessionKey {
            adapter_id: "typescript".into(),
            root: project_root.canonicalize().unwrap(),
        };
        let path = project_root.join("main.ts");
        std::fs::write(&path, "let value = 1;").unwrap();
        let path = path.canonicalize().unwrap();
        let document_id = DocumentId::new();
        let mut status = actor.blank_status(document_id, Some("typescript"));
        status.state = LspSessionState::Starting;
        status.session_id = Some("queued-success-generation".into());
        actor.state.documents.insert(
            document_id,
            super::ManagedDocument {
                owner_window: "main".into(),
                owner_pane: "pane".into(),
                uri: lsp::Url::from_file_path(&path).unwrap().to_string(),
                path,
                language: Some(LanguageId::TypeScript),
                lsp_language_id: Some("typescript".into()),
                adapter_id: Some("typescript".into()),
                text: crate::lsp::document::VersionedDocument::new(
                    &super::document_id_text(document_id),
                    "let value = 1;",
                    1,
                )
                .unwrap(),
                candidates: Vec::new(),
                binding_scope: project_root.clone(),
                session_key: Some(session_key.clone()),
                selected_root: Some(project_root.clone()),
                deferred_for_session: false,
                pending_batches: Vec::new(),
                batch_generation: 0,
                pull_result_id: None,
                pull_generation: 0,
                synchronization_dirty: false,
                status,
            },
        );
        actor.state.sessions.insert(
            session_key.clone(),
            super::ManagedSession {
                session_id: "queued-success-generation".into(),
                language: LanguageId::TypeScript,
                generation: 1,
                worker: None,
                startup_cancel: Some(tokio_util::sync::CancellationToken::new()),
                outbox: std::collections::VecDeque::new(),
                restart_after_stop: false,
                documents: [document_id].into_iter().collect(),
                idle_generation: 0,
                crash_timestamps: std::collections::VecDeque::new(),
                automatic_restart_blocked: false,
                exit_observed: false,
                logs: std::collections::VecDeque::new(),
                next_log_sequence: 1,
                capabilities: super::capabilities(false),
                triggers: Default::default(),
                protocol_started: false,
            },
        );
        actor.active_generations.insert((session_key.clone(), 1));

        // Both lanes are ready before the first actor turn. The biased command
        // lane must cancel generation 1 before its already-successful startup
        // result reaches the input lane.
        let (restart_reply, restart_result) = tokio::sync::oneshot::channel();
        manager
            .commands
            .try_send(super::ManagerCommand::RestartSession {
                adapter_id: "typescript".into(),
                root: project_root.clone(),
                reply: restart_reply,
            })
            .unwrap();
        actor
            .input_tx
            .try_send(super::ActorInput::SessionStarted {
                key: session_key.clone(),
                generation: 1,
                result: Ok(Arc::new(FakeSession {
                    factory: factory.clone(),
                    key: session_key.clone(),
                })),
            })
            .unwrap();
        let actor_task = tokio::spawn(actor.run());

        restart_result.await.unwrap().unwrap();
        spin().await;
        let observations = factory.observations(&session_key);
        assert_eq!(observations, vec![Observation::Shutdown]);
        assert_eq!(
            factory.launch_count("typescript", &project_root),
            0,
            "replacement launch overlapped the cancelled successful client reap"
        );

        factory.release_shutdowns();
        for _ in 0..100 {
            if factory.launch_count("typescript", &project_root) == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(factory.launch_count("typescript", &project_root), 1);
        manager.shutdown().await.unwrap();
        actor_task.await.unwrap();
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
            } if window_label == "popup"
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn pending_multi_close_policy_reserves_worker_credits_from_continuous_edits() {
        let harness = ManagerHarness::new();
        let first_path = harness.file("policy-fairness-a.ts", "a");
        let second_path = harness.file("policy-fairness-b.ts", "b");
        let first = harness.open(&first_path, "main", "a").await;
        let second = harness.open(&second_path, "main", "b").await;
        let root = harness.root.clone();
        harness.choose_and_trust(first, &root, "typescript").await;
        spin().await;
        assert_eq!(
            harness.manager.status_snapshot(Some(second)).await.unwrap()[0].state,
            LspSessionState::Ready
        );

        harness.factory.hold_changes_one_at_a_time();
        saturate_session_outbox(&harness, first).await;
        let mut version = 1;
        let disable = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::none()).await }
        });
        spin().await;
        let later_enable = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::all()).await }
        });
        spin().await;

        version += 1;
        harness
            .manager
            .apply_changes(second, batch(second, version - 1, version, "pending-0"))
            .await
            .unwrap();
        tokio::time::advance(Duration::from_millis(40)).await;
        spin().await;

        let mut ready_cycles = 0;
        while !disable.is_finished() && ready_cycles < 4 {
            let completed = harness.factory.completed_changes.load(Ordering::SeqCst);
            harness.factory.release_one_change();
            for _ in 0..100 {
                spin().await;
                if harness.factory.completed_changes.load(Ordering::SeqCst) > completed {
                    break;
                }
            }
            spin().await;
            ready_cycles += 1;
            assert!(
                disable.is_finished() || !later_enable.is_finished(),
                "a later policy overtook the capacity-blocked front"
            );
            if !disable.is_finished() {
                let next = version + 1;
                harness
                    .manager
                    .apply_changes(second, batch(second, version, next, "steady-edit"))
                    .await
                    .unwrap();
                version = next;
                tokio::time::advance(Duration::from_millis(40)).await;
                spin().await;
            }
        }

        assert!(
            disable.is_finished(),
            "the front two-close policy was starved for {ready_cycles} worker-ready cycles"
        );
        disable.await.unwrap().unwrap();
        later_enable.await.unwrap().unwrap();
        for _ in 0..100 {
            spin().await;
            if harness.manager.status_snapshot(Some(second)).await.unwrap()[0].state
                == LspSessionState::Ready
            {
                break;
            }
        }

        let observations = harness.factory.observations(&key("typescript", &root));
        let changes = observations
            .iter()
            .filter_map(|observation| match observation {
                Observation::Change {
                    document_id,
                    base,
                    next,
                } if document_id == &super::document_id_text(second) => Some((*base, *next)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            changes
                .windows(2)
                .all(|pair| pair[0].1 == pair[1].0 && pair[1].1 == pair[1].0 + 1),
            "admitted edits were reordered: {changes:?}"
        );
        assert!(
            observations.iter().any(|observation| {
                matches!(
                    observation,
                    Observation::Open {
                        document_id,
                        version: reopened,
                    } if document_id == &super::document_id_text(second) && *reopened == version
                )
            }),
            "the latest accepted edit version was not restored after policy convergence: {observations:?}"
        );
    }

    #[tokio::test]
    async fn permanently_ready_normal_commands_cannot_starve_worker_ready_for_front_policy() {
        let temp = TempDir::new().unwrap();
        let config_root = temp.path().join("config");
        let cache_root = temp.path().join("cache");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&project_root).unwrap();
        let factory = Arc::new(FakeFactory::default());
        let (manager, mut actor, _events) =
            LspManager::new(factory, config_root, cache_root, Enablement::all());
        let session_key = SessionKey {
            adapter_id: "typescript".into(),
            root: project_root.canonicalize().unwrap(),
        };
        let path = project_root.join("main.ts");
        std::fs::write(&path, "let value = 1;").unwrap();
        let path = path.canonicalize().unwrap();
        let document_id = DocumentId::new();
        let mut status = actor.blank_status(document_id, Some("typescript"));
        status.state = LspSessionState::Ready;
        status.session_id = Some("fairness-generation".into());
        actor.state.documents.insert(
            document_id,
            super::ManagedDocument {
                owner_window: "main".into(),
                owner_pane: "pane".into(),
                uri: lsp::Url::from_file_path(&path).unwrap().to_string(),
                path: path.clone(),
                language: Some(LanguageId::TypeScript),
                lsp_language_id: Some("typescript".into()),
                adapter_id: Some("typescript".into()),
                text: crate::lsp::document::VersionedDocument::new(
                    &super::document_id_text(document_id),
                    "let value = 1;",
                    1,
                )
                .unwrap(),
                candidates: Vec::new(),
                binding_scope: project_root.clone(),
                session_key: Some(session_key.clone()),
                selected_root: Some(project_root.clone()),
                deferred_for_session: false,
                pending_batches: Vec::new(),
                batch_generation: 0,
                pull_result_id: None,
                pull_generation: 0,
                synchronization_dirty: false,
                status,
            },
        );
        actor.state.sessions.insert(
            session_key.clone(),
            super::ManagedSession {
                session_id: "fairness-generation".into(),
                language: LanguageId::TypeScript,
                generation: 1,
                worker: None,
                startup_cancel: None,
                outbox: std::collections::VecDeque::new(),
                restart_after_stop: false,
                documents: [document_id].into_iter().collect(),
                idle_generation: 0,
                crash_timestamps: std::collections::VecDeque::new(),
                automatic_restart_blocked: false,
                exit_observed: false,
                logs: std::collections::VecDeque::new(),
                next_log_sequence: 1,
                capabilities: super::capabilities(false),
                triggers: Default::default(),
                protocol_started: true,
            },
        );
        let (policy_reply, policy_result) = tokio::sync::oneshot::channel();
        actor
            .pending_policy_commands
            .push_back(super::ManagerCommand::SetEnablement {
                enablement: Enablement::none(),
                reply: policy_reply,
            });
        actor.policy_capacity_reservations.insert(
            session_key.clone(),
            super::PolicyCapacityReservation {
                operations: 1,
                attachments: 0,
                close_documents: [document_id].into_iter().collect(),
            },
        );

        // Both lanes are ready before the actor takes its first turn. The
        // normal lane then stays ready for a full queue quota.
        let normal_replies = fill_normal_command_queue(&manager);
        actor
            .input_tx
            .try_send(super::ActorInput::WorkerReady {
                key: session_key,
                generation: 1,
            })
            .unwrap();
        tokio::spawn(actor.run());

        policy_result
            .await
            .expect("actor must retain the front policy reply")
            .expect("WorkerReady must advance the front policy");
        let mut normal_turns_before_policy = 0;
        for reply in normal_replies {
            let statuses = reply.await.unwrap().unwrap();
            if statuses
                .iter()
                .any(|status| status.state == LspSessionState::Ready)
            {
                normal_turns_before_policy += 1;
            }
        }
        assert!(
            normal_turns_before_policy <= 1,
            "front policy waited behind {normal_turns_before_policy} normal actor turns even though WorkerReady was already queued"
        );
    }

    #[tokio::test]
    async fn stale_and_duplicate_exact_close_success_cannot_advance_another_group() {
        let temp = TempDir::new().unwrap();
        let config_root = temp.path().join("config");
        let cache_root = temp.path().join("cache");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&project_root).unwrap();
        let first_path = project_root.join("first.ts");
        let second_path = project_root.join("second.ts");
        std::fs::write(&first_path, "first").unwrap();
        std::fs::write(&second_path, "second").unwrap();
        let factory = Arc::new(FakeFactory::default());
        let (_manager, mut actor, _events) =
            LspManager::new(factory, config_root, cache_root, Enablement::all());
        let open = |actor: &mut LspManager, path: &Path, pane: &str| {
            let ReserveResult::Reserved { reservation_id, .. } = actor
                .reserve_document(path.to_path_buf(), "main".into())
                .unwrap()
            else {
                panic!("expected reservation")
            };
            let response = actor
                .open_document(
                    reservation_id,
                    pane.into(),
                    pane.into(),
                    "typescript".into(),
                )
                .unwrap();
            serde_json::from_value::<DocumentId>(serde_json::Value::String(response.document_id))
                .unwrap()
        };
        let first = open(&mut actor, &first_path, "first");
        let second = open(&mut actor, &second_path, "second");
        let session_key = SessionKey {
            adapter_id: "typescript".into(),
            root: project_root.canonicalize().unwrap(),
        };
        for document_id in [first, second] {
            actor
                .state
                .documents
                .get_mut(&document_id)
                .unwrap()
                .session_key = Some(session_key.clone());
        }
        actor.state.sessions.insert(
            session_key.clone(),
            super::ManagedSession {
                session_id: "exact-close-generation".into(),
                language: LanguageId::TypeScript,
                generation: 7,
                worker: None,
                startup_cancel: None,
                outbox: std::collections::VecDeque::new(),
                restart_after_stop: false,
                documents: [first, second].into_iter().collect(),
                idle_generation: 0,
                crash_timestamps: std::collections::VecDeque::new(),
                automatic_restart_blocked: false,
                exit_observed: false,
                logs: std::collections::VecDeque::new(),
                next_log_sequence: 1,
                capabilities: super::capabilities(false),
                triggers: Default::default(),
                protocol_started: true,
            },
        );
        let (policy_reply, _policy_result) = tokio::sync::oneshot::channel();
        actor
            .pending_policy_commands
            .push_back(super::ManagerCommand::SetEnablement {
                enablement: Enablement::none(),
                reply: policy_reply,
            });
        actor.policy_reservation_epoch = Some(41);
        actor.policy_capacity_reservations.insert(
            session_key,
            super::PolicyCapacityReservation {
                operations: 2,
                attachments: 0,
                close_documents: [first, second].into_iter().collect(),
            },
        );
        let (first_reply, mut first_result) = tokio::sync::oneshot::channel();
        actor.admit_close_group(vec![first], first_reply);
        let (second_reply, mut second_result) = tokio::sync::oneshot::channel();
        actor.admit_close_group(vec![second], second_reply);
        assert!(actor.retry_pending_close_group());
        let first_delivery = match &actor.pending_close_groups.front().unwrap().phase {
            super::DeferredClosePhase::Delivering { outstanding, .. } => {
                outstanding.values().next().unwrap().clone()
            }
            super::DeferredClosePhase::Pending => panic!("front group was not staged"),
        };

        let mut stale_delivery = first_delivery.clone();
        stale_delivery.session_generation -= 1;
        actor.handle_input(super::ActorInput::ReservedCloseDeliverySucceeded {
            delivery: stale_delivery,
        });
        assert!(matches!(
            first_result.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        assert!(actor.state.documents.contains_key(&first));

        actor.handle_input(super::ActorInput::ReservedCloseDeliverySucceeded {
            delivery: first_delivery.clone(),
        });
        assert_eq!(first_result.try_recv().unwrap(), Ok(()));
        assert!(actor.state.documents.contains_key(&second));
        let second_delivery = match &actor.pending_close_groups.front().unwrap().phase {
            super::DeferredClosePhase::Delivering { outstanding, .. } => {
                outstanding.values().next().unwrap().clone()
            }
            super::DeferredClosePhase::Pending => panic!("second group was not staged"),
        };

        actor.handle_input(super::ActorInput::ReservedCloseDeliverySucceeded {
            delivery: first_delivery,
        });
        assert!(matches!(
            second_result.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        assert!(actor.state.documents.contains_key(&second));

        actor.handle_input(super::ActorInput::ReservedCloseDeliverySucceeded {
            delivery: second_delivery,
        });
        assert_eq!(second_result.try_recv().unwrap(), Ok(()));
        assert!(!actor.state.documents.contains_key(&second));
    }

    #[tokio::test(start_paused = true)]
    async fn explicit_close_consumes_reserved_multi_close_credit_and_advances_policy() {
        let harness = ManagerHarness::new();
        let first_path = harness.file("reserved-close-a.ts", "a");
        let second_path = harness.file("reserved-close-b.ts", "b");
        let first = harness.open(&first_path, "main", "a").await;
        let second = harness.open(&second_path, "main", "b").await;
        let root = harness.root.clone();
        harness.choose_and_trust(first, &root, "typescript").await;
        spin().await;
        assert_eq!(
            harness.manager.status_snapshot(Some(second)).await.unwrap()[0].state,
            LspSessionState::Ready
        );

        harness.factory.hold_changes_one_at_a_time();
        let last_admitted = saturate_session_outbox(&harness, first).await;
        let disable = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::none()).await }
        });
        spin().await;
        assert!(
            !disable.is_finished(),
            "multi-close policy must hold a reservation"
        );

        let close = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_document(second).await }
        });
        spin().await;
        assert!(
            !close.is_finished(),
            "an explicit close that satisfies reserved policy work must defer instead of returning Overloaded"
        );

        harness.factory.release_one_change();
        for _ in 0..100 {
            if close.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
        }
        close.await.unwrap().unwrap();
        assert!(
            !disable.is_finished(),
            "the remaining reserved close still needs its own worker credit"
        );

        for _ in 0..300 {
            if disable.is_finished() {
                break;
            }
            harness.factory.release_one_change();
            spin().await;
        }
        disable.await.unwrap().unwrap();
        harness
            .manager
            .set_enablement(Enablement::all())
            .await
            .unwrap();
        for _ in 0..100 {
            spin().await;
            let observations = harness.factory.observations(&key("typescript", &root));
            if observations.iter().any(|observation| {
                matches!(
                    observation,
                    Observation::Open {
                        document_id,
                        version,
                    } if document_id == &super::document_id_text(first) && *version == last_admitted
                )
            }) {
                break;
            }
        }

        let observations = harness.factory.observations(&key("typescript", &root));
        assert!(
            observations.iter().any(|observation| {
                matches!(
                    observation,
                    Observation::Open {
                        document_id,
                        version,
                    } if document_id == &super::document_id_text(first) && *version == last_admitted
                )
            }),
            "the remaining document lost its latest accepted edit while close advanced policy: {observations:?}"
        );
        assert!(
            harness
                .manager
                .status_snapshot(Some(second))
                .await
                .unwrap()
                .is_empty(),
            "the explicit close must remove the document exactly once"
        );
        assert!(matches!(
            harness
                .manager
                .reserve_document(second_path, "reopened".into())
                .await
                .unwrap(),
            ReserveResult::Reserved { .. }
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn ordinary_resync_close_cannot_complete_a_later_reserved_close_delivery() {
        let harness = ManagerHarness::new();
        let pressure_path = harness.file("reserved-identity-pressure.ts", "pressure");
        let closing_path = harness.file("reserved-identity-close.ts", "close");
        let pressure = harness.open(&pressure_path, "main", "pressure").await;
        let closing = harness.open(&closing_path, "main", "closing").await;
        let root = harness.root.clone();
        let session_key = key("typescript", &root);
        harness
            .choose_and_trust(pressure, &root, "typescript")
            .await;
        harness.factory.hold_closes_one_at_a_time();

        harness
            .manager
            .resync_document(closing, 2, "let resynced = true;".into())
            .await
            .unwrap();
        wait_for_observation_count(&harness.factory, &session_key, 1, |observation| {
            matches!(observation, Observation::Close(document) if document == &super::document_id_text(closing))
        })
        .await;

        saturate_session_outbox(&harness, pressure).await;
        let policy = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::none()).await }
        });
        spin().await;
        let grouped = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_document(closing).await }
        });
        spin().await;
        assert!(!grouped.is_finished());

        // This releases only the older resync didClose. Its document string is
        // identical to the reserved close, but it has no group delivery ID.
        harness.factory.release_one_close();
        harness.factory.release_all_changes();
        wait_for_observation_count(&harness.factory, &session_key, 2, |observation| {
            matches!(observation, Observation::Close(document) if document == &super::document_id_text(closing))
        })
        .await;
        spin().await;
        assert!(
            !grouped.is_finished(),
            "the ordinary resync close discharged the reserved group"
        );
        assert_document_owner(&harness, &closing_path, closing).await;

        harness.factory.release_one_close();
        wait_until_finished(&grouped).await;
        grouped.await.unwrap().unwrap();
        assert!(matches!(
            harness
                .manager
                .reserve_document(closing_path, "reopen".into())
                .await
                .unwrap(),
            ReserveResult::Reserved { .. }
        ));

        let _ = policy.await.unwrap();
        harness.manager.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn reserved_close_failure_after_prior_success_waits_for_controlled_session_stop() {
        let ReservedCloseFixture {
            harness,
            paths,
            documents: [_, first, second, _],
            root,
            policy,
        } = reserved_close_fixture("group-delivery-failure").await;
        let session_key = key("typescript", &root);
        harness.factory.hold_closes_one_at_a_time();
        harness.factory.hold_shutdowns();
        let grouped = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![first, second]).await }
        });
        harness.factory.release_all_changes();

        wait_for_observation_count(&harness.factory, &session_key, 1, |observation| {
            matches!(observation, Observation::Close(_))
        })
        .await;
        assert!(!grouped.is_finished());
        assert_document_owner(&harness, &paths[1], first).await;
        assert_document_owner(&harness, &paths[2], second).await;

        harness.factory.release_one_close();
        wait_for_observation_count(&harness.factory, &session_key, 2, |observation| {
            matches!(observation, Observation::Close(_))
        })
        .await;
        spin().await;
        assert!(
            !grouped.is_finished(),
            "one successful member cannot partially finalize the group"
        );
        assert_document_owner(&harness, &paths[1], first).await;
        assert_document_owner(&harness, &paths[2], second).await;

        harness.factory.fail_next_close();
        harness.factory.release_one_close();
        wait_for_observation_count(&harness.factory, &session_key, 1, |observation| {
            matches!(observation, Observation::Shutdown)
        })
        .await;
        spin().await;
        assert!(
            !grouped.is_finished(),
            "failed didClose must wait for truthful generation shutdown"
        );
        assert_document_owner(&harness, &paths[1], first).await;
        assert_document_owner(&harness, &paths[2], second).await;

        harness.factory.release_shutdowns();
        wait_until_finished(&grouped).await;
        grouped.await.unwrap().unwrap();
        policy.await.unwrap().unwrap();
        for path in [paths[1].clone(), paths[2].clone()] {
            assert!(matches!(
                harness
                    .manager
                    .reserve_document(path, "reopen".into())
                    .await
                    .unwrap(),
                ReserveResult::Reserved { .. }
            ));
        }
        let observations = harness.factory.observations(&session_key);
        assert_eq!(
            observations
                .iter()
                .filter(|observation| matches!(observation, Observation::Close(_)))
                .count(),
            2,
            "controlled recovery must not resend an already-attempted close"
        );
        assert_eq!(
            observations
                .iter()
                .filter(|observation| matches!(observation, Observation::Shutdown))
                .count(),
            1,
            "the affected generation must be stopped exactly once"
        );
        assert_eq!(harness.factory.launch_count("typescript", &root), 1);
        harness.manager.shutdown().await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn priority_shutdown_waits_to_settle_an_inflight_reserved_close_group_once() {
        let ReservedCloseFixture {
            harness,
            paths,
            documents: [_, first, second, _],
            root,
            policy,
        } = reserved_close_fixture("group-delivery-priority-shutdown").await;
        let session_key = key("typescript", &root);
        harness.factory.hold_closes_one_at_a_time();
        harness.factory.hold_shutdowns();
        let grouped = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![first, second]).await }
        });
        harness.factory.release_all_changes();
        wait_for_observation_count(&harness.factory, &session_key, 1, |observation| {
            matches!(observation, Observation::Close(_))
        })
        .await;
        assert!(!grouped.is_finished());
        assert_document_owner(&harness, &paths[1], first).await;
        assert_document_owner(&harness, &paths[2], second).await;

        let shutdown = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.shutdown().await }
        });
        spin().await;
        assert!(!shutdown.is_finished());
        assert!(
            !grouped.is_finished(),
            "priority shutdown must not report a staged group before generation stop"
        );

        harness.factory.release_shutdowns();
        wait_until_finished(&shutdown).await;
        shutdown.await.unwrap().unwrap();
        assert_eq!(grouped.await.unwrap(), Err(ManagerError::ActorStopped));
        assert_eq!(policy.await.unwrap(), Err(ManagerError::ActorStopped));
    }

    #[tokio::test(start_paused = true)]
    async fn reserved_close_drops_queued_traffic_and_skips_a_never_delivered_open() {
        let harness = ManagerHarness::new();
        let pressure_path = harness.file("lane-order-pressure.ts", "pressure");
        let pressure = harness.open(&pressure_path, "main", "pressure").await;
        let root = harness.root.clone();
        let session_key = key("typescript", &root);
        harness
            .choose_and_trust(pressure, &root, "typescript")
            .await;
        spin().await;

        // Wedge the ordered lane on held didChanges and push enough of them in
        // to guarantee anything enqueued next lands in the outbox rather than
        // the worker's own queue.
        harness.factory.hold_changes();
        harness.factory.hold_changes_one_at_a_time();
        let mut version = 1;
        for next in 2..102 {
            harness
                .manager
                .apply_changes(pressure, batch(pressure, next - 1, next, "pressure"))
                .await
                .unwrap();
            tokio::time::advance(Duration::from_millis(40)).await;
            spin().await;
            version = next;
        }

        // A document attached while the lane is wedged has a didOpen queued
        // that the server has never seen.
        let latecomer_path = harness.file("lane-order-latecomer.ts", "late");
        let latecomer = harness.open(&latecomer_path, "main", "late").await;
        spin().await;
        let latecomer_text = super::document_id_text(latecomer);
        assert!(
            !harness
                .factory
                .observations(&session_key)
                .iter()
                .any(|observation| matches!(
                    observation,
                    Observation::Open { document_id, .. } if document_id == &latecomer_text
                )),
            "the latecomer's didOpen must still be queued behind the wedged lane"
        );

        for next in (version + 1)..500 {
            match harness
                .manager
                .apply_changes(pressure, batch(pressure, next - 1, next, "pressure"))
                .await
            {
                Ok(_) => {}
                Err(ManagerError::Overloaded) => break,
                other => panic!("unexpected saturation result: {other:?}"),
            }
            tokio::time::advance(Duration::from_millis(40)).await;
            spin().await;
        }
        let policy = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::none()).await }
        });
        spin().await;
        assert!(!policy.is_finished(), "the policy must reserve closes");
        let grouped = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![latecomer]).await }
        });
        spin().await;
        assert!(!grouped.is_finished());

        for _ in 0..300 {
            if grouped.is_finished() {
                break;
            }
            harness.factory.release_one_change();
            spin().await;
        }
        grouped.await.unwrap().unwrap();

        harness.factory.release_all_changes();
        for _ in 0..300 {
            if policy.is_finished() {
                break;
            }
            harness.factory.release_one_change();
            spin().await;
        }
        let _ = policy.await.unwrap();
        spin().await;

        let observations = harness.factory.observations(&session_key);
        assert!(
            !observations.iter().any(|observation| matches!(
                observation,
                Observation::Close(document_id) if document_id == &latecomer_text
            )),
            "a document the server was never told to open must not be closed"
        );
        assert!(
            !observations.iter().any(|observation| matches!(
                observation,
                Observation::Open { document_id, .. } if document_id == &latecomer_text
            )),
            "the closing document's queued didOpen must be dropped, not delivered"
        );
        harness.manager.shutdown().await.unwrap();
    }

    struct StagedReservedClose {
        actor: LspManager,
        session_key: SessionKey,
        document: DocumentId,
        group_result: tokio::sync::oneshot::Receiver<Result<(), ManagerError>>,
        delivery: super::ReservedCloseDelivery,
        _policy_result: tokio::sync::oneshot::Receiver<Result<(), ManagerError>>,
    }

    /// One actor with one synthetic session holding a single staged, delivering
    /// reserved close group.
    fn staged_reserved_close(temp: &TempDir, name: &str) -> StagedReservedClose {
        let config_root = temp.path().join("config");
        let cache_root = temp.path().join("cache");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&project_root).unwrap();
        let path = project_root.join(format!("{name}.ts"));
        std::fs::write(&path, name).unwrap();
        let factory = Arc::new(FakeFactory::default());
        let (_manager, mut actor, _events) =
            LspManager::new(factory, config_root, cache_root, Enablement::all());
        let ReserveResult::Reserved { reservation_id, .. } =
            actor.reserve_document(path, "main".into()).unwrap()
        else {
            panic!("expected reservation")
        };
        let response = actor
            .open_document(
                reservation_id,
                name.into(),
                name.into(),
                "typescript".into(),
            )
            .unwrap();
        let document =
            serde_json::from_value::<DocumentId>(serde_json::Value::String(response.document_id))
                .unwrap();
        let session_key = SessionKey {
            adapter_id: "typescript".into(),
            root: project_root.canonicalize().unwrap(),
        };
        actor
            .state
            .documents
            .get_mut(&document)
            .unwrap()
            .session_key = Some(session_key.clone());
        actor
            .state
            .sessions
            .insert(session_key.clone(), fake_managed_session(name, 4, document));
        actor.active_generations.insert((session_key.clone(), 4));
        let (policy_reply, _policy_result) = tokio::sync::oneshot::channel();
        actor
            .pending_policy_commands
            .push_back(super::ManagerCommand::SetEnablement {
                enablement: Enablement::none(),
                reply: policy_reply,
            });
        actor.policy_reservation_epoch = Some(19);
        actor.policy_capacity_reservations.insert(
            session_key.clone(),
            super::PolicyCapacityReservation {
                operations: 1,
                attachments: 0,
                close_documents: [document].into_iter().collect(),
            },
        );
        let (group_reply, group_result) = tokio::sync::oneshot::channel();
        actor.admit_close_group(vec![document], group_reply);
        assert!(actor.retry_pending_close_group());
        let delivery = match &actor.pending_close_groups.front().unwrap().phase {
            super::DeferredClosePhase::Delivering { outstanding, .. } => {
                outstanding.values().next().unwrap().clone()
            }
            super::DeferredClosePhase::Pending => panic!("group was not staged"),
        };
        StagedReservedClose {
            actor,
            session_key,
            document,
            group_result,
            delivery,
            _policy_result,
        }
    }

    /// The open/close transitions a session's outbox still holds for one
    /// document, in the order the server will see them.
    fn queued_transitions(
        actor: &LspManager,
        session_key: &SessionKey,
        document_id: DocumentId,
    ) -> Vec<super::OpenTransition> {
        let id_text = super::document_id_text(document_id);
        actor.state.sessions[session_key]
            .outbox
            .iter()
            .filter_map(|operation| match super::operation_document(operation) {
                Some((queued_id, Some(transition))) if queued_id == id_text => Some(transition),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn a_refused_repair_close_cannot_abort_the_detach() {
        let temp = TempDir::new().unwrap();
        let config_root = temp.path().join("config");
        let cache_root = temp.path().join("cache");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&project_root).unwrap();
        let path = project_root.join("refused.ts");
        std::fs::write(&path, "refused").unwrap();
        let factory = Arc::new(FakeFactory::default());
        let (_manager, mut actor, _events) =
            LspManager::new(factory, config_root, cache_root, Enablement::all());
        let ReserveResult::Reserved { reservation_id, .. } =
            actor.reserve_document(path, "main".into()).unwrap()
        else {
            panic!("expected reservation")
        };
        let response = actor
            .open_document(
                reservation_id,
                "refused".into(),
                "refused".into(),
                "typescript".into(),
            )
            .unwrap();
        let document =
            serde_json::from_value::<DocumentId>(serde_json::Value::String(response.document_id))
                .unwrap();
        let session_key = SessionKey {
            adapter_id: "typescript".into(),
            root: project_root.canonicalize().unwrap(),
        };
        actor
            .state
            .documents
            .get_mut(&document)
            .unwrap()
            .session_key = Some(session_key.clone());
        actor.state.sessions.insert(
            session_key.clone(),
            fake_managed_session("refused-repair", 8, document),
        );
        actor.active_generations.insert((session_key.clone(), 8));

        // A request is outstanding for the document when the group settles.
        let (hover_reply, mut hover_result) = tokio::sync::oneshot::channel();
        actor.start_request(
            document,
            crate::lsp::types::EditorPosition {
                line: 0,
                character: 0,
            },
            None,
            super::RequestKind::Hover,
            super::PendingReply::Hover(hover_reply),
        );
        // From here nothing drains, so everything enqueued stays in the outbox.
        actor.state.sessions.get_mut(&session_key).unwrap().worker = None;
        actor
            .session_delivery_progress
            .get_mut(&session_key)
            .unwrap()
            .open_on_server
            .insert(super::document_id_text(document));

        let (policy_reply, _policy_result) = tokio::sync::oneshot::channel();
        actor
            .pending_policy_commands
            .push_back(super::ManagerCommand::SetEnablement {
                enablement: Enablement::all(),
                reply: policy_reply,
            });
        actor.policy_reservation_epoch = Some(37);
        actor.policy_capacity_reservations.insert(
            session_key.clone(),
            super::PolicyCapacityReservation {
                operations: 1,
                attachments: 0,
                close_documents: [document].into_iter().collect(),
            },
        );
        let (group_reply, mut group_result) = tokio::sync::oneshot::channel();
        actor.admit_close_group(vec![document], group_reply);
        assert!(actor.retry_pending_close_group());
        let delivery = match &actor.pending_close_groups.front().unwrap().phase {
            super::DeferredClosePhase::Delivering { outstanding, .. } => {
                outstanding.values().next().unwrap().clone()
            }
            super::DeferredClosePhase::Pending => panic!("group was not staged"),
        };

        // The client answers with a resync, re-opening the closing document,
        // and then keeps typing until the outbox is full again - which is the
        // regime this whole machinery exists for.
        actor
            .resync_document(document, 2, "refused again".into())
            .unwrap();
        while actor
            .enqueue_session_operations(
                &session_key,
                [super::SessionOperation::DidSave("filler".into())],
            )
            .is_ok()
        {}

        actor.handle_input(super::ActorInput::ReservedCloseDeliverySucceeded { delivery });
        assert_eq!(group_result.try_recv().unwrap(), Ok(()));
        assert!(!actor.state.documents.contains_key(&document));

        // The repair close could not be queued. That must not cost the detach.
        let session = &actor.state.sessions[&session_key];
        assert!(
            !session.documents.contains(&document),
            "a refused repair close left the session holding a document that exists nowhere else"
        );
        assert_eq!(
            session.idle_generation, 1,
            "the session lost its last document, so idle shutdown must be armed"
        );
        assert_eq!(
            hover_result.try_recv().unwrap(),
            Err(ManagerError::Cancelled),
            "a request on a detached document must not be left hanging"
        );

        // The repair is owed, not forgotten: it lands as soon as the session
        // has room for it.
        let session = actor.state.sessions.get_mut(&session_key).unwrap();
        for _ in 0..4 {
            session.outbox.pop_back();
        }
        let progress = actor
            .session_delivery_progress
            .get_mut(&session_key)
            .unwrap();
        for _ in 0..4 {
            progress.pipeline.pop_back();
        }
        actor.handle_input(super::ActorInput::WorkerReady {
            key: session_key.clone(),
            generation: 8,
        });
        assert_eq!(
            queued_transitions(&actor, &session_key, document),
            vec![
                super::OpenTransition::Close,
                super::OpenTransition::Close,
                super::OpenTransition::Open,
                super::OpenTransition::Close,
            ],
            "the owed repair close must land exactly once, after the resync's didOpen"
        );
        assert!(
            !actor.session_delivery_progress[&session_key]
                .will_be_open(&super::document_id_text(document)),
            "the wire must end with the document closed"
        );
        assert!(
            actor.state.sessions.contains_key(&session_key),
            "a repair close must never take the session down with it"
        );

        // A repair carries no delivery token, so a failed one reports through
        // the ordinary operation-failure path. It must not reach the
        // unconditional stop that a failed *reserved* close triggers.
        actor.handle_input(super::ActorInput::SessionOperationFailed {
            key: session_key.clone(),
            generation: 8,
            operation: "didClose",
            message: "repair close failed".into(),
        });
        assert!(
            actor.state.sessions.contains_key(&session_key),
            "a failed repair close must be logged, not escalated to a session stop"
        );
    }

    #[tokio::test]
    async fn an_owed_repair_close_uses_the_slot_the_pump_just_freed() {
        let temp = TempDir::new().unwrap();
        let config_root = temp.path().join("config");
        let cache_root = temp.path().join("cache");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&project_root).unwrap();
        let path = project_root.join("owed.ts");
        std::fs::write(&path, "owed").unwrap();
        let factory = Arc::new(FakeFactory::default());
        let (_manager, mut actor, _events) =
            LspManager::new(factory, config_root, cache_root, Enablement::all());
        let ReserveResult::Reserved { reservation_id, .. } =
            actor.reserve_document(path, "main".into()).unwrap()
        else {
            panic!("expected reservation")
        };
        let response = actor
            .open_document(
                reservation_id,
                "owed".into(),
                "owed".into(),
                "typescript".into(),
            )
            .unwrap();
        let document =
            serde_json::from_value::<DocumentId>(serde_json::Value::String(response.document_id))
                .unwrap();
        let id_text = super::document_id_text(document);
        let session_key = SessionKey {
            adapter_id: "typescript".into(),
            root: project_root.canonicalize().unwrap(),
        };
        actor
            .state
            .documents
            .get_mut(&document)
            .unwrap()
            .session_key = Some(session_key.clone());

        // A worker whose queue holds exactly one operation, so the test can
        // free precisely one slot the way a real worker does.
        let (operations, mut operations_rx) = mpsc::channel(1);
        let (close_deliveries, close_rx) = mpsc::channel(super::SESSION_OPERATION_CAPACITY);
        let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
        std::mem::forget((close_rx, shutdown_rx));
        let mut session = fake_managed_session("owed-repair", 9, document);
        session.worker = Some(super::SessionWorkerHandle {
            operations,
            close_deliveries,
            shutdown: Some(shutdown),
        });
        actor.state.sessions.insert(session_key.clone(), session);

        while actor
            .enqueue_session_operations(
                &session_key,
                [super::SessionOperation::DidSave("filler".into())],
            )
            .is_ok()
        {}
        actor
            .session_delivery_progress
            .get_mut(&session_key)
            .unwrap()
            .open_on_server
            .insert(id_text.clone());
        actor
            .pending_repair_closes
            .insert(session_key.clone(), vec![id_text.clone()]);

        // The worker finishes one operation: that is the slot the owed repair
        // is waiting for, and it only exists once the outbox has been pumped.
        operations_rx.try_recv().unwrap();
        actor.handle_input(super::ActorInput::WorkerReady {
            key: session_key.clone(),
            generation: 9,
        });

        assert!(
            !actor.pending_repair_closes.contains_key(&session_key),
            "the owed repair was refused against an outbox the pump was about to drain"
        );
        assert_eq!(
            queued_transitions(&actor, &session_key, document),
            vec![super::OpenTransition::Close],
            "the repair must land on the slot this turn freed"
        );
    }

    #[test]
    fn the_wire_projection_reads_every_kind_of_pending_close() {
        let mut progress = super::SessionDeliveryProgress::default();
        let out_of_band = "out-of-band".to_owned();
        let vacuous = "vacuous".to_owned();
        let reopened = "reopened".to_owned();
        progress.open_on_server.insert(out_of_band.clone());
        progress.open_on_server.insert(reopened.clone());

        // A credit stands for a close already sent on the delivery lane.
        progress
            .pipeline
            .push_back(super::PipelineEntry::ReservedCredit {
                document_id: out_of_band.clone(),
            });
        // A document whose didOpen was discarded was never opened at all.
        // A re-opened document ends the queue open and still owes a close.
        for (document_id, transition) in [
            (reopened.clone(), super::OpenTransition::Close),
            (reopened.clone(), super::OpenTransition::Open),
        ] {
            progress.pipeline.push_back(super::PipelineEntry::Document {
                document_id,
                transition: Some(transition),
            });
        }

        assert!(
            !progress.will_be_open(&out_of_band),
            "an out-of-band close is settled by its queued credit"
        );
        assert!(
            !progress.will_be_open(&vacuous),
            "a document the server was never told to open is not open"
        );
        assert!(
            progress.will_be_open(&reopened),
            "a queued didOpen leaves the document open however it got there"
        );
    }

    #[tokio::test]
    async fn a_resync_during_delivery_cannot_leave_the_server_holding_a_closed_document() {
        let temp = TempDir::new().unwrap();
        let config_root = temp.path().join("config");
        let cache_root = temp.path().join("cache");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&project_root).unwrap();
        let path = project_root.join("resynced.ts");
        std::fs::write(&path, "resynced").unwrap();
        let factory = Arc::new(FakeFactory::default());
        let (_manager, mut actor, _events) =
            LspManager::new(factory, config_root, cache_root, Enablement::all());
        let ReserveResult::Reserved { reservation_id, .. } =
            actor.reserve_document(path, "main".into()).unwrap()
        else {
            panic!("expected reservation")
        };
        let response = actor
            .open_document(
                reservation_id,
                "resync".into(),
                "resync".into(),
                "typescript".into(),
            )
            .unwrap();
        let document =
            serde_json::from_value::<DocumentId>(serde_json::Value::String(response.document_id))
                .unwrap();
        let session_key = SessionKey {
            adapter_id: "typescript".into(),
            root: project_root.canonicalize().unwrap(),
        };
        actor
            .state
            .documents
            .get_mut(&document)
            .unwrap()
            .session_key = Some(session_key.clone());
        actor.state.sessions.insert(
            session_key.clone(),
            fake_managed_session("resync-window", 6, document),
        );
        actor.active_generations.insert((session_key.clone(), 6));

        // The document's didOpen is already inside the worker, so the close has
        // to travel the ordered lane, and a didChange is still queued behind it
        // so the discard fires and marks the document out of sync.
        let uri = actor.state.documents.get(&document).unwrap().uri.clone();
        actor
            .enqueue_session_operations(
                &session_key,
                [super::SessionOperation::DidOpen(super::SessionDocument {
                    document_id: super::document_id_text(document),
                    uri: lsp::Url::parse(&uri).unwrap(),
                    language_id: "typescript".into(),
                    version: 1,
                    text: "resynced".into(),
                })],
            )
            .unwrap();
        for _ in 0..super::SESSION_OPERATION_CAPACITY {
            actor
                .enqueue_session_operations(
                    &session_key,
                    [super::SessionOperation::DidSave("filler".into())],
                )
                .unwrap();
        }
        actor
            .enqueue_session_operations(
                &session_key,
                [super::SessionOperation::DidChange(batch(
                    document, 1, 2, "edit",
                ))],
            )
            .unwrap();
        actor
            .session_delivery_progress
            .get_mut(&session_key)
            .unwrap()
            .open_on_server
            .insert(super::document_id_text(document));

        // A policy has to be queued for a reserved close group to exist at all.
        // This one leaves the session alive so the wire state stays inspectable
        // after the group settles.
        let (policy_reply, _policy_result) = tokio::sync::oneshot::channel();
        actor
            .pending_policy_commands
            .push_back(super::ManagerCommand::SetEnablement {
                enablement: Enablement::all(),
                reply: policy_reply,
            });
        actor.policy_reservation_epoch = Some(31);
        actor.policy_capacity_reservations.insert(
            session_key.clone(),
            super::PolicyCapacityReservation {
                operations: 1,
                attachments: 0,
                close_documents: [document].into_iter().collect(),
            },
        );
        let (group_reply, mut group_result) = tokio::sync::oneshot::channel();
        actor.admit_close_group(vec![document], group_reply);
        assert!(actor.retry_pending_close_group());
        let delivery = match &actor.pending_close_groups.front().unwrap().phase {
            super::DeferredClosePhase::Delivering { outstanding, .. } => {
                outstanding.values().next().unwrap().clone()
            }
            super::DeferredClosePhase::Pending => panic!("group was not staged"),
        };
        assert!(
            actor
                .state
                .documents
                .get(&document)
                .unwrap()
                .synchronization_dirty,
            "the discard must have marked the document out of sync"
        );
        assert_eq!(
            queued_transitions(&actor, &session_key, document),
            vec![super::OpenTransition::Close],
            "the staged close is queued in the ordered lane"
        );

        // The client answers the resync the dirty flag asked for. It is not a
        // policy command, so it runs immediately - while the group is still
        // delivering - and re-opens the document the group is closing.
        actor
            .resync_document(document, 2, "resynced again".into())
            .unwrap();
        assert_eq!(
            queued_transitions(&actor, &session_key, document),
            vec![
                super::OpenTransition::Close,
                super::OpenTransition::Close,
                super::OpenTransition::Open,
            ],
            "the resync re-opened a document the group is closing"
        );

        actor.handle_input(super::ActorInput::ReservedCloseDeliverySucceeded { delivery });
        assert_eq!(group_result.try_recv().unwrap(), Ok(()));
        assert!(!actor.state.documents.contains_key(&document));
        assert!(
            actor.state.sessions.contains_key(&session_key),
            "the session must outlive the group for this assertion to mean anything"
        );

        assert_eq!(
            queued_transitions(&actor, &session_key, document),
            vec![
                super::OpenTransition::Close,
                super::OpenTransition::Close,
                super::OpenTransition::Open,
                super::OpenTransition::Close,
            ],
            "settlement must add exactly one repair close, after the resync's didOpen"
        );
    }

    #[tokio::test]
    async fn reserved_close_staging_survives_an_abandoned_attempt() {
        let temp = TempDir::new().unwrap();
        let config_root = temp.path().join("config");
        let cache_root = temp.path().join("cache");
        let project_root = temp.path().join("project");
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&project_root).unwrap();
        let phantom_path = project_root.join("phantom.ts");
        let live_path = project_root.join("live.ts");
        std::fs::write(&phantom_path, "phantom").unwrap();
        std::fs::write(&live_path, "live").unwrap();
        let factory = Arc::new(FakeFactory::default());
        let (_manager, mut actor, _events) =
            LspManager::new(factory, config_root, cache_root, Enablement::all());
        let open = |actor: &mut LspManager, path: &Path, pane: &str| {
            let ReserveResult::Reserved { reservation_id, .. } = actor
                .reserve_document(path.to_path_buf(), "main".into())
                .unwrap()
            else {
                panic!("expected reservation")
            };
            let response = actor
                .open_document(
                    reservation_id,
                    pane.into(),
                    pane.into(),
                    "typescript".into(),
                )
                .unwrap();
            serde_json::from_value::<DocumentId>(serde_json::Value::String(response.document_id))
                .unwrap()
        };
        let phantom = open(&mut actor, &phantom_path, "phantom");
        let live = open(&mut actor, &live_path, "live");
        let session_key = SessionKey {
            adapter_id: "typescript".into(),
            root: project_root.canonicalize().unwrap(),
        };
        for document_id in [phantom, live] {
            actor
                .state
                .documents
                .get_mut(&document_id)
                .unwrap()
                .session_key = Some(session_key.clone());
        }
        let mut session = fake_managed_session("abandoned-attempt", 2, phantom);
        session.documents.insert(live);
        actor.state.sessions.insert(session_key.clone(), session);

        // Fill the worker's own queue so the outbox keeps everything enqueued
        // from here on: that is what puts an undelivered didOpen and a pending
        // request in front of the staging attempt.
        for _ in 0..super::SESSION_OPERATION_CAPACITY {
            actor
                .enqueue_session_operations(
                    &session_key,
                    [super::SessionOperation::DidSave("filler".into())],
                )
                .unwrap();
        }

        // The server has `live` open and has never heard of `phantom`, whose
        // didOpen is still sitting in the outbox.
        let phantom_uri = actor.state.documents.get(&phantom).unwrap().uri.clone();
        actor
            .enqueue_session_operations(
                &session_key,
                [super::SessionOperation::DidOpen(super::SessionDocument {
                    document_id: super::document_id_text(phantom),
                    uri: lsp::Url::parse(&phantom_uri).unwrap(),
                    language_id: "typescript".into(),
                    version: 1,
                    text: "phantom".into(),
                })],
            )
            .unwrap();
        actor
            .session_delivery_progress
            .get_mut(&session_key)
            .unwrap()
            .open_on_server
            .insert(super::document_id_text(live));

        let (policy_reply, _policy_result) = tokio::sync::oneshot::channel();
        actor
            .pending_policy_commands
            .push_back(super::ManagerCommand::SetEnablement {
                enablement: Enablement::none(),
                reply: policy_reply,
            });
        actor.policy_reservation_epoch = Some(23);
        actor.policy_capacity_reservations.insert(
            session_key.clone(),
            super::PolicyCapacityReservation {
                operations: 2,
                attachments: 0,
                close_documents: [phantom, live].into_iter().collect(),
            },
        );
        let (hover_reply, mut hover_result) = tokio::sync::oneshot::channel();
        actor.start_request(
            phantom,
            crate::lsp::types::EditorPosition {
                line: 0,
                character: 0,
            },
            None,
            super::RequestKind::Hover,
            super::PendingReply::Hover(hover_reply),
        );
        assert!(
            matches!(
                hover_result.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "the request must be pending before staging discards its document"
        );

        let (group_reply, mut group_result) = tokio::sync::oneshot::channel();
        actor.admit_close_group(vec![phantom, live], group_reply);
        let mut group = actor.pending_close_groups.pop_front().unwrap();

        actor.retrying_policy = true;
        let first = actor.stage_close_group(&mut group).unwrap();
        assert!(
            !first.contains_key(&phantom),
            "a document whose didOpen was discarded owes no close"
        );
        assert!(first.contains_key(&live));
        assert!(
            actor
                .state
                .documents
                .get(&phantom)
                .unwrap()
                .synchronization_dirty,
            "discarding queued traffic must mark the document out of sync so an \
             abandoned attempt is repairable"
        );
        assert_eq!(
            hover_result.try_recv().unwrap(),
            Err(ManagerError::Cancelled),
            "a request whose queued work was discarded must not be left to hang"
        );
        let abandoned = first.get(&live).unwrap().clone();

        // The attempt is abandoned and the group retried unchanged. Nothing is
        // left queued for `phantom`, so this attempt can only get the verdict
        // right by remembering it.
        let second = actor.stage_close_group(&mut group).unwrap();
        actor.retrying_policy = false;
        assert!(
            !second.contains_key(&phantom),
            "a retried attempt resurrected a close for a buffer the server never opened"
        );
        let phantom_text = super::document_id_text(phantom);
        assert!(
            !actor.state.sessions[&session_key]
                .outbox
                .iter()
                .any(|operation| matches!(
                    operation,
                    super::SessionOperation::DidClose { document_id, .. }
                        if document_id == &phantom_text
                )),
            "no didClose may be queued for a document the server never opened"
        );
        let live_delivery = second.get(&live).unwrap().clone();
        assert!(
            live_delivery.attempt > abandoned.attempt,
            "the retry must mint a fresh attempt"
        );

        group.phase = super::DeferredClosePhase::Delivering {
            outstanding: second,
            stopping: Vec::new(),
        };
        actor.pending_close_groups.push_front(group);
        actor.handle_input(super::ActorInput::ReservedCloseDeliverySucceeded {
            delivery: abandoned,
        });
        assert!(
            matches!(
                group_result.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "the abandoned attempt's acknowledgement discharged the retry"
        );
        actor.handle_input(super::ActorInput::ReservedCloseDeliverySucceeded {
            delivery: live_delivery,
        });
        assert_eq!(group_result.try_recv().unwrap(), Ok(()));
    }

    #[tokio::test]
    async fn a_spent_reserved_credit_stops_the_mirror_believing_its_document_is_open() {
        let temp = TempDir::new().unwrap();
        let StagedReservedClose {
            mut actor,
            session_key,
            document,
            ..
        } = staged_reserved_close(&temp, "credit-transition");
        let id_text = super::document_id_text(document);
        let progress = actor
            .session_delivery_progress
            .entry(session_key.clone())
            .or_default();
        progress.open_on_server.insert(id_text.clone());
        progress
            .pipeline
            .push_back(super::PipelineEntry::ReservedCredit {
                document_id: id_text.clone(),
            });
        assert!(
            progress.believes_open(&id_text),
            "the close travelled on the delivery lane, so the ordered lane has \
             not reached its credit yet"
        );

        progress.record_completed();
        assert!(
            !progress.believes_open(&id_text),
            "spending the credit is when the mirror learns the document was closed"
        );
        assert!(
            progress.open_on_server.is_empty(),
            "an out-of-band close must not leave its document believed open forever"
        );
    }

    #[tokio::test]
    async fn discarding_a_document_leaves_reserved_credits_alone() {
        let temp = TempDir::new().unwrap();
        let StagedReservedClose {
            mut actor,
            session_key,
            document,
            ..
        } = staged_reserved_close(&temp, "credit-survives");
        let id_text = super::document_id_text(document);
        actor.state.sessions.get_mut(&session_key).unwrap().worker = None;
        actor
            .enqueue_session_operations(
                &session_key,
                [
                    super::SessionOperation::ReservedCloseCredit {
                        document_id: id_text.clone(),
                    },
                    super::SessionOperation::DidChange(batch(document, 1, 2, "edit")),
                ],
            )
            .unwrap();

        actor.discard_queued_document_traffic(&session_key, document);

        let outbox = &actor.state.sessions[&session_key].outbox;
        assert!(
            !outbox.iter().any(|operation| matches!(
                operation,
                super::SessionOperation::DidChange(batch) if batch.document_id == id_text
            )),
            "the closing document's queued traffic must be discarded"
        );
        assert!(
            outbox.iter().any(|operation| matches!(
                operation,
                super::SessionOperation::ReservedCloseCredit { document_id }
                    if document_id == &id_text
            )),
            "a credit is another group's reserved worker slot, not traffic to discard"
        );
        assert!(
            actor.session_delivery_progress[&session_key].credits_outstanding(),
            "discarding a document must not spend a credit the policy is waiting on"
        );
    }

    #[tokio::test]
    async fn reserved_credits_only_count_while_a_worker_can_spend_them() {
        let temp = TempDir::new().unwrap();
        let StagedReservedClose {
            mut actor,
            session_key,
            document,
            ..
        } = staged_reserved_close(&temp, "credit-predicate");
        actor
            .session_delivery_progress
            .entry(session_key.clone())
            .or_default()
            .pipeline
            .push_back(super::PipelineEntry::ReservedCredit {
                document_id: super::document_id_text(document),
            });
        assert!(
            actor.reserved_close_credits_outstanding(),
            "a live worker still owes the slot the policy reserved"
        );

        actor.state.sessions.get_mut(&session_key).unwrap().worker = None;
        assert!(
            !actor.reserved_close_credits_outstanding(),
            "a session with no worker can never spend a reserved credit"
        );
    }

    #[tokio::test]
    async fn reserved_close_from_an_abandoned_staging_attempt_is_inert() {
        let temp = TempDir::new().unwrap();
        let StagedReservedClose {
            mut actor,
            document,
            mut group_result,
            delivery,
            ..
        } = staged_reserved_close(&temp, "attempt");

        // A staging attempt that aborted part way through keeps the group id,
        // the epoch and the generation, so only the attempt number tells an
        // acknowledgement of the abandoned attempt apart from a live one.
        let mut abandoned = delivery.clone();
        abandoned.attempt -= 1;
        actor.handle_input(super::ActorInput::ReservedCloseDeliverySucceeded {
            delivery: abandoned,
        });
        assert!(matches!(
            group_result.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        assert!(actor.state.documents.contains_key(&document));

        actor.handle_input(super::ActorInput::ReservedCloseDeliverySucceeded { delivery });
        assert_eq!(group_result.try_recv().unwrap(), Ok(()));
        assert!(!actor.state.documents.contains_key(&document));
    }

    #[tokio::test]
    async fn unattributable_reserved_close_failure_stops_the_dead_lane_generation() {
        let temp = TempDir::new().unwrap();
        let StagedReservedClose {
            mut actor,
            session_key,
            mut group_result,
            delivery,
            ..
        } = staged_reserved_close(&temp, "orphan");

        // The epoch is invalidated while the close is still in flight, so the
        // token that comes back has no group left to discharge.
        actor.invalidate_policy_epoch(ManagerError::PolicyChanged);
        assert_eq!(
            group_result.try_recv().unwrap(),
            Err(ManagerError::PolicyChanged)
        );

        actor.handle_input(super::ActorInput::ReservedCloseDeliveryFailed {
            delivery,
            message: "didClose failed".into(),
        });
        assert!(
            !actor.state.sessions.contains_key(&session_key),
            "the delivery lane stops at its first failure, so the generation that owns it \
             must be stopped even when the token cannot be attributed to a group"
        );
    }

    #[tokio::test]
    async fn reserved_close_failure_stops_every_session_the_group_touched() {
        let temp = TempDir::new().unwrap();
        let config_root = temp.path().join("config");
        let cache_root = temp.path().join("cache");
        let first_root = temp.path().join("first-project");
        let second_root = temp.path().join("second-project");
        std::fs::create_dir_all(&cache_root).unwrap();
        std::fs::create_dir_all(&first_root).unwrap();
        std::fs::create_dir_all(&second_root).unwrap();
        let first_path = first_root.join("first.ts");
        let second_path = second_root.join("second.ts");
        std::fs::write(&first_path, "first").unwrap();
        std::fs::write(&second_path, "second").unwrap();
        let factory = Arc::new(FakeFactory::default());
        let (_manager, mut actor, _events) =
            LspManager::new(factory, config_root, cache_root, Enablement::all());
        let open = |actor: &mut LspManager, path: &Path, pane: &str| {
            let ReserveResult::Reserved { reservation_id, .. } = actor
                .reserve_document(path.to_path_buf(), "main".into())
                .unwrap()
            else {
                panic!("expected reservation")
            };
            let response = actor
                .open_document(
                    reservation_id,
                    pane.into(),
                    pane.into(),
                    "typescript".into(),
                )
                .unwrap();
            serde_json::from_value::<DocumentId>(serde_json::Value::String(response.document_id))
                .unwrap()
        };
        let first = open(&mut actor, &first_path, "first");
        let second = open(&mut actor, &second_path, "second");
        let first_key = SessionKey {
            adapter_id: "typescript".into(),
            root: first_root.canonicalize().unwrap(),
        };
        let second_key = SessionKey {
            adapter_id: "typescript".into(),
            root: second_root.canonicalize().unwrap(),
        };
        for (document_id, session_key, generation) in [
            (first, first_key.clone(), 3),
            (second, second_key.clone(), 5),
        ] {
            actor
                .state
                .documents
                .get_mut(&document_id)
                .unwrap()
                .session_key = Some(session_key.clone());
            actor.state.sessions.insert(
                session_key.clone(),
                fake_managed_session(
                    &format!("cross-session-{generation}"),
                    generation,
                    document_id,
                ),
            );
            actor
                .active_generations
                .insert((session_key.clone(), generation));
            actor.policy_capacity_reservations.insert(
                session_key,
                super::PolicyCapacityReservation {
                    operations: 1,
                    attachments: 0,
                    close_documents: [document_id].into_iter().collect(),
                },
            );
        }
        let (policy_reply, _policy_result) = tokio::sync::oneshot::channel();
        actor
            .pending_policy_commands
            .push_back(super::ManagerCommand::SetEnablement {
                enablement: Enablement::none(),
                reply: policy_reply,
            });
        actor.policy_reservation_epoch = Some(11);

        let (group_reply, mut group_result) = tokio::sync::oneshot::channel();
        actor.admit_close_group(vec![first, second], group_reply);
        assert!(actor.retry_pending_close_group());
        let deliveries = match &actor.pending_close_groups.front().unwrap().phase {
            super::DeferredClosePhase::Delivering { outstanding, .. } => {
                outstanding.values().cloned().collect::<Vec<_>>()
            }
            super::DeferredClosePhase::Pending => panic!("group was not staged"),
        };
        assert_eq!(deliveries.len(), 2, "both sessions owe a close");
        let failed = deliveries
            .iter()
            .find(|delivery| delivery.key == first_key)
            .unwrap()
            .clone();

        actor.handle_input(super::ActorInput::ReservedCloseDeliveryFailed {
            delivery: failed,
            message: "didClose failed".into(),
        });
        assert!(
            !actor.state.sessions.contains_key(&first_key),
            "the failing session must be stopped"
        );
        assert!(
            !actor.state.sessions.contains_key(&second_key),
            "a group member's other session still holds its document open"
        );
        assert!(matches!(
            group_result.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        assert!(actor.state.documents.contains_key(&second));

        actor.handle_input(super::ActorInput::ShutdownFinished {
            key: first_key,
            generation: 3,
        });
        assert!(
            matches!(
                group_result.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "one stop cannot settle a group that spans two sessions"
        );

        actor.handle_input(super::ActorInput::ShutdownFinished {
            key: second_key,
            generation: 5,
        });
        assert_eq!(group_result.try_recv().unwrap(), Ok(()));
        assert!(!actor.state.documents.contains_key(&first));
        assert!(!actor.state.documents.contains_key(&second));
    }

    #[tokio::test(start_paused = true)]
    async fn crashed_session_reserved_credits_cannot_wedge_deferred_policy() {
        let ReservedCloseFixture {
            harness,
            documents: [_, first, ..],
            root,
            policy,
            ..
        } = reserved_close_fixture("delivery-credit-crash").await;
        let session_key = key("typescript", &root);
        let grouped = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![first]).await }
        });
        for _ in 0..300 {
            if grouped.is_finished() {
                break;
            }
            harness.factory.release_one_change();
            spin().await;
        }
        grouped.await.unwrap().unwrap();

        // The credit placeholder this group left behind is still queued when the
        // session dies. Nothing will ever spend it, so it must not hold the
        // deferred policy command hostage.
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
        wait_until_finished(&policy).await;
        assert!(
            policy.is_finished(),
            "a stranded reserved-close credit wedged the deferred policy lane"
        );
        let _ = policy.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn reserved_close_groups_complete_in_invocation_fifo_before_their_policy() {
        let ReservedCloseFixture {
            harness,
            paths,
            documents,
            root,
            policy,
        } = reserved_close_fixture("group-epoch-fifo").await;
        let [pressure, a, b, c] = documents;
        let first_group = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![a, b]).await }
        });
        spin().await;
        let second_group = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![c]).await }
        });
        spin().await;
        assert!(!first_group.is_finished() && !second_group.is_finished());

        harness.factory.release_one_change();
        spin().await;
        assert!(
            !first_group.is_finished() && !second_group.is_finished(),
            "one credit cannot partially release the two-document first group"
        );
        assert_document_owner(&harness, &paths[1], a).await;
        assert_document_owner(&harness, &paths[2], b).await;
        assert_document_owner(&harness, &paths[3], c).await;

        harness.factory.release_one_change();
        wait_until_finished(&first_group).await;
        assert!(
            first_group.is_finished(),
            "the first group did not use its second credit"
        );
        assert!(
            !second_group.is_finished(),
            "the later group overtook the first"
        );
        assert!(
            !policy.is_finished(),
            "the policy overtook admitted close groups"
        );
        first_group.await.unwrap().unwrap();
        for path in [&paths[1], &paths[2]] {
            assert!(matches!(
                harness
                    .manager
                    .reserve_document(path.clone(), "reopen".into())
                    .await
                    .unwrap(),
                ReserveResult::Reserved { .. }
            ));
        }
        assert_document_owner(&harness, &paths[3], c).await;

        harness.factory.release_one_change();
        wait_until_finished(&second_group).await;
        assert!(
            second_group.is_finished(),
            "the second group did not receive the next credit"
        );
        assert!(
            !policy.is_finished(),
            "the policy overtook the second close group"
        );
        second_group.await.unwrap().unwrap();

        for _ in 0..300 {
            if policy.is_finished() {
                break;
            }
            harness.factory.release_one_change();
            spin().await;
        }
        policy.await.unwrap().unwrap();
        assert_document_owner(&harness, &paths[0], pressure).await;

        for _ in 0..100 {
            spin().await;
            if harness
                .factory
                .observations(&key("typescript", &root))
                .iter()
                .filter(|observation| matches!(observation, Observation::Close(_)))
                .count()
                == 4
            {
                break;
            }
        }
        let closes = harness
            .factory
            .observations(&key("typescript", &root))
            .into_iter()
            .filter_map(|observation| match observation {
                Observation::Close(document_id) => Some(document_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            closes,
            [a, b, c, pressure]
                .into_iter()
                .map(super::document_id_text)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn reserved_close_groups_use_invocation_fifo_when_group_sizes_arrive_reversed() {
        let ReservedCloseFixture {
            harness,
            documents: [_, a, b, c],
            policy,
            ..
        } = reserved_close_fixture("group-epoch-reversed").await;
        let first_group = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![c]).await }
        });
        spin().await;
        let second_group = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![a, b]).await }
        });
        spin().await;

        harness.factory.release_one_change();
        wait_until_finished(&first_group).await;
        assert!(
            first_group.is_finished(),
            "the first one-document group was overtaken"
        );
        assert!(!second_group.is_finished());
        first_group.await.unwrap().unwrap();

        harness.factory.release_one_change();
        spin().await;
        assert!(!second_group.is_finished());
        harness.factory.release_one_change();
        wait_until_finished(&second_group).await;
        assert!(second_group.is_finished());
        assert!(!policy.is_finished());
        second_group.await.unwrap().unwrap();

        for _ in 0..300 {
            if policy.is_finished() {
                break;
            }
            harness.factory.release_one_change();
            spin().await;
        }
        policy.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn overlapping_reserved_close_group_is_rejected_without_partial_ownership_release() {
        let ReservedCloseFixture {
            harness,
            paths,
            documents: [_, a, b, c],
            policy,
            ..
        } = reserved_close_fixture("group-epoch-overlap").await;
        let first_group = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![a, b, a]).await }
        });
        spin().await;
        let overlapping_group = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![b, c]).await }
        });
        spin().await;
        assert!(
            overlapping_group.is_finished(),
            "an overlapping group must be rejected at admission"
        );
        assert_eq!(
            overlapping_group.await.unwrap(),
            Err(ManagerError::Overloaded)
        );
        assert!(!first_group.is_finished());
        assert_document_owner(&harness, &paths[1], a).await;
        assert_document_owner(&harness, &paths[2], b).await;
        assert_document_owner(&harness, &paths[3], c).await;

        harness.manager.shutdown().await.unwrap();
        assert_eq!(first_group.await.unwrap(), Err(ManagerError::ActorStopped));
        assert_eq!(policy.await.unwrap(), Err(ManagerError::ActorStopped));
    }

    #[tokio::test(start_paused = true)]
    async fn cancelled_front_close_group_returns_its_epoch_credits_to_the_next_group() {
        let ReservedCloseFixture {
            harness,
            paths,
            documents: [pressure, a, b, c],
            policy,
            ..
        } = reserved_close_fixture("group-epoch-cancel").await;
        let cancelled_group = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![a, b]).await }
        });
        spin().await;
        let second_group = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![c]).await }
        });
        spin().await;
        cancelled_group.abort();
        assert!(cancelled_group.await.unwrap_err().is_cancelled());

        harness.factory.release_one_change();
        wait_until_finished(&second_group).await;
        second_group.await.unwrap().unwrap();
        assert_document_owner(&harness, &paths[1], a).await;
        assert_document_owner(&harness, &paths[2], b).await;
        assert!(matches!(
            harness
                .manager
                .reserve_document(paths[3].clone(), "reopen".into())
                .await
                .unwrap(),
            ReserveResult::Reserved { .. }
        ));

        for _ in 0..300 {
            if policy.is_finished() {
                break;
            }
            harness.factory.release_one_change();
            spin().await;
        }
        policy.await.unwrap().unwrap();
        assert_document_owner(&harness, &paths[0], pressure).await;
    }

    #[tokio::test(start_paused = true)]
    async fn priority_shutdown_fails_every_reserved_close_group_once() {
        let ReservedCloseFixture {
            harness,
            documents: [_, a, b, c],
            policy,
            ..
        } = reserved_close_fixture("group-epoch-shutdown").await;
        let first_group = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![a, b]).await }
        });
        spin().await;
        let second_group = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![c]).await }
        });
        spin().await;

        harness.manager.shutdown().await.unwrap();
        assert_eq!(first_group.await.unwrap(), Err(ManagerError::ActorStopped));
        assert_eq!(second_group.await.unwrap(), Err(ManagerError::ActorStopped));
        assert_eq!(policy.await.unwrap(), Err(ManagerError::ActorStopped));
    }

    #[tokio::test(start_paused = true)]
    async fn worker_failure_fails_every_group_from_the_stale_policy_epoch_retryably() {
        let ReservedCloseFixture {
            harness,
            documents: [_, a, b, c],
            root,
            policy,
            ..
        } = reserved_close_fixture("group-epoch-worker-failure").await;
        let first_group = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![a, b]).await }
        });
        spin().await;
        let second_group = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![c]).await }
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
        wait_until_finished(&first_group).await;
        wait_until_finished(&second_group).await;
        assert_eq!(first_group.await.unwrap(), Err(ManagerError::PolicyChanged));
        assert_eq!(
            second_group.await.unwrap(),
            Err(ManagerError::PolicyChanged)
        );
        policy.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn grouped_close_consumes_a_matching_subset_one_worker_credit_at_a_time() {
        let harness = ManagerHarness::new();
        let first_path = harness.file("reserved-group-subset-a.ts", "a");
        let second_path = harness.file("reserved-group-subset-b.ts", "b");
        let third_path = harness.file("reserved-group-subset-c.ts", "c");
        let first = harness.open(&first_path, "main", "a").await;
        let second = harness.open(&second_path, "main", "b").await;
        let third = harness.open(&third_path, "main", "c").await;
        let root = harness.root.clone();
        harness.choose_and_trust(first, &root, "typescript").await;
        spin().await;

        harness.factory.hold_changes_one_at_a_time();
        saturate_session_outbox(&harness, first).await;
        let policy = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::none()).await }
        });
        spin().await;
        assert!(!policy.is_finished());

        let grouped = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![second, third]).await }
        });
        spin().await;
        assert!(
            !grouped.is_finished(),
            "a matching close group must retain its reply instead of spuriously overloading"
        );

        harness.factory.release_one_change();
        for _ in 0..100 {
            spin().await;
            if grouped.is_finished() {
                break;
            }
        }
        assert!(
            !grouped.is_finished(),
            "one physical slot cannot partially release a two-document group"
        );
        for (path, expected) in [(&second_path, second), (&third_path, third)] {
            assert!(matches!(
                harness
                    .manager
                    .reserve_document(path.clone(), "observer".into())
                    .await
                    .unwrap(),
                ReserveResult::FocusOwner { document_id, .. } if document_id == expected
            ));
        }

        harness.factory.release_one_change();
        for _ in 0..100 {
            spin().await;
            if grouped.is_finished() {
                break;
            }
        }
        grouped.await.unwrap().unwrap();
        assert!(
            !policy.is_finished(),
            "the front policy still owns one close credit after a matching subset completes"
        );
        for path in [second_path, third_path] {
            assert!(matches!(
                harness
                    .manager
                    .reserve_document(path, "reopen".into())
                    .await
                    .unwrap(),
                ReserveResult::Reserved { .. }
            ));
        }

        for _ in 0..300 {
            if policy.is_finished() {
                break;
            }
            harness.factory.release_one_change();
            spin().await;
        }
        policy.await.unwrap().unwrap();
        assert!(matches!(
            harness
                .manager
                .reserve_document(first_path, "observer".into())
                .await
                .unwrap(),
            ReserveResult::FocusOwner { document_id, .. } if document_id == first
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn grouped_close_consumes_all_matching_policy_credits_atomically() {
        let harness = ManagerHarness::new();
        let first_path = harness.file("reserved-group-all-a.ts", "a");
        let second_path = harness.file("reserved-group-all-b.ts", "b");
        let first = harness.open(&first_path, "main", "a").await;
        let second = harness.open(&second_path, "main", "b").await;
        let root = harness.root.clone();
        harness.choose_and_trust(first, &root, "typescript").await;
        spin().await;

        harness.factory.hold_changes_one_at_a_time();
        saturate_session_outbox(&harness, first).await;
        let policy = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::none()).await }
        });
        spin().await;
        let grouped = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![first, second]).await }
        });
        spin().await;
        assert!(!grouped.is_finished());

        let mut ready_cycles = 0;
        while !grouped.is_finished() && ready_cycles < 300 {
            harness.factory.release_one_change();
            for _ in 0..10 {
                spin().await;
                if grouped.is_finished() {
                    break;
                }
            }
            ready_cycles += 1;
            if !grouped.is_finished() {
                for (path, expected) in [(&first_path, first), (&second_path, second)] {
                    assert!(matches!(
                        harness
                            .manager
                            .reserve_document(path.clone(), "observer".into())
                            .await
                            .unwrap(),
                        ReserveResult::FocusOwner { document_id, .. } if document_id == expected
                    ));
                }
            }
        }
        assert!(
            ready_cycles > 2,
            "the group must also drain the pressured document's admitted didChange batches"
        );
        assert!(
            grouped.is_finished(),
            "the reserved group did not progress after {ready_cycles} WorkerReady turns"
        );
        grouped.await.unwrap().unwrap();
        for _ in 0..100 {
            spin().await;
            if policy.is_finished() {
                break;
            }
        }
        policy.await.unwrap().unwrap();
        for path in [first_path, second_path] {
            assert!(matches!(
                harness
                    .manager
                    .reserve_document(path, "reopen".into())
                    .await
                    .unwrap(),
                ReserveResult::Reserved { .. }
            ));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn grouped_close_enqueues_did_close_in_caller_fifo_once_per_document() {
        let harness = ManagerHarness::new();
        let first_path = harness.file("group-fifo-a.ts", "a");
        let second_path = harness.file("group-fifo-b.ts", "b");
        let first = harness.open(&first_path, "main", "a").await;
        let second = harness.open(&second_path, "main", "b").await;
        let root = harness.root.clone();
        harness.choose_and_trust(first, &root, "typescript").await;
        spin().await;

        harness
            .manager
            .close_documents(vec![second, first, second])
            .await
            .unwrap();
        for _ in 0..100 {
            spin().await;
            let close_count = harness
                .factory
                .observations(&key("typescript", &root))
                .iter()
                .filter(|observation| matches!(observation, Observation::Close(_)))
                .count();
            if close_count == 2 {
                break;
            }
        }
        let closes = harness
            .factory
            .observations(&key("typescript", &root))
            .into_iter()
            .filter_map(|observation| match observation {
                Observation::Close(document_id) => Some(document_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            closes,
            [second, first]
                .into_iter()
                .map(super::document_id_text)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn grouped_close_unrelated_to_the_front_policy_stays_overloaded_and_preserves_owners() {
        let harness = ManagerHarness::new();
        let first_path = harness.file("reserved-unrelated-a.ts", "a");
        let second_path = harness.file("reserved-unrelated-b.ts", "b");
        let first = harness.open(&first_path, "main", "a").await;
        let second = harness.open(&second_path, "main", "b").await;
        let root = harness.root.clone();
        harness.choose_and_trust(first, &root, "typescript").await;
        spin().await;
        saturate_session_outbox(&harness, first).await;

        let context = tokio::spawn({
            let manager = harness.manager.clone();
            async move {
                manager
                    .set_project_context(first, ProjectContextChoice::Disabled)
                    .await
            }
        });
        spin().await;
        assert!(!context.is_finished());
        assert_eq!(
            harness.manager.close_documents(vec![second]).await,
            Err(ManagerError::Overloaded)
        );
        for (path, expected) in [(&first_path, first), (&second_path, second)] {
            assert!(matches!(
                harness
                    .manager
                    .reserve_document(path.clone(), "observer".into())
                    .await
                    .unwrap(),
                ReserveResult::FocusOwner { document_id, .. } if document_id == expected
            ));
        }
        harness.factory.release_changes();
        context.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn deferred_grouped_close_yields_its_reply_to_priority_shutdown() {
        let harness = ManagerHarness::new();
        let first_path = harness.file("reserved-group-shutdown-a.ts", "a");
        let second_path = harness.file("reserved-group-shutdown-b.ts", "b");
        let first = harness.open(&first_path, "main", "a").await;
        let second = harness.open(&second_path, "main", "b").await;
        let root = harness.root.clone();
        harness.choose_and_trust(first, &root, "typescript").await;
        spin().await;
        harness.factory.hold_changes_one_at_a_time();
        saturate_session_outbox(&harness, first).await;

        let policy = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.set_enablement(Enablement::none()).await }
        });
        spin().await;
        let grouped = tokio::spawn({
            let manager = harness.manager.clone();
            async move { manager.close_documents(vec![first, second]).await }
        });
        spin().await;
        assert!(!grouped.is_finished());

        harness.manager.shutdown().await.unwrap();
        assert_eq!(grouped.await.unwrap(), Err(ManagerError::ActorStopped));
        assert_eq!(policy.await.unwrap(), Err(ManagerError::ActorStopped));
    }

    #[tokio::test]
    async fn normal_commands_progress_without_a_pending_policy() {
        let harness = ManagerHarness::new();
        let replies = fill_normal_command_queue(&harness.manager);
        for reply in replies {
            reply
                .await
                .expect("actor remains live")
                .expect("normal command succeeds");
        }
    }

    #[tokio::test]
    async fn priority_shutdown_preempts_a_full_normal_command_queue() {
        let harness = ManagerHarness::new();
        let replies = fill_normal_command_queue(&harness.manager);
        let (shutdown_reply, shutdown_result) = tokio::sync::oneshot::channel();
        harness
            .manager
            .shutdown
            .try_send(shutdown_reply)
            .expect("priority lane begins empty");

        shutdown_result.await.unwrap().unwrap();
        assert!(
            replies.into_iter().any(|mut reply| {
                matches!(
                    reply.try_recv(),
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed)
                )
            }),
            "priority shutdown must close normal admission before draining a full command queue"
        );
    }
}
