use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::fs;
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::time::Duration;

use async_lsp::concurrency::ConcurrencyLayer;
use async_lsp::lsp_types as lsp;
use async_lsp::panic::CatchUnwindLayer;
use async_lsp::router::Router;
use async_lsp::{LanguageServer, MainLoop, ServerSocket};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::process::Command;
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio::time::timeout;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;
use tower::ServiceBuilder;

use super::catalog::{AdapterDescriptor, ResolvedServerCommand};
use super::client::{
    ClientEvent, ClientState, EventSink, SessionCapabilityState, SyncPolicy, normalize_completion,
    normalize_definition, normalize_diagnostics, normalize_hover, normalize_resolved_completion,
    normalize_signature_help,
};
use super::document::{DocumentError, VersionedDocument};
use super::types::{
    CompletionItem, CompletionResponse, DefinitionResponse, Diagnostic, HoverResponse,
    LspCapabilities, LspChangeBatch, NegotiatedTriggers, SignatureHelpResponse, normalize_triggers,
};

const SHORT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const DEFINITION_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(120);
const MIN_INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REVALIDATED_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_COMPLETION_GENERATIONS: usize = 32;
const MAX_COMPLETION_ITEMS: usize = 2048;

type BoxReader = Box<dyn AsyncRead + Unpin + Send>;
type BoxWriter = Box<dyn AsyncWrite + Unpin + Send>;
type BoxRequest<T> = Pin<Box<dyn Future<Output = async_lsp::Result<T>> + Send>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessExit {
    pub success: bool,
    pub code: Option<i32>,
}

#[derive(Clone)]
pub(crate) struct ProcessHandle {
    kill: Arc<std::sync::Mutex<Option<oneshot::Sender<()>>>>,
    exit: watch::Receiver<Option<ProcessExit>>,
}

impl ProcessHandle {
    pub(crate) fn new(
        kill: oneshot::Sender<()>,
        exit: watch::Receiver<Option<ProcessExit>>,
    ) -> Self {
        Self {
            kill: Arc::new(std::sync::Mutex::new(Some(kill))),
            exit,
        }
    }

    pub(crate) fn kill(&self) {
        if let Some(kill) = self.kill.lock().expect("process kill lock poisoned").take() {
            let _ = kill.send(());
        }
    }

    pub(crate) async fn wait(&self) -> ProcessExit {
        let mut exit = self.exit.clone();
        loop {
            if let Some(status) = exit.borrow().clone() {
                return status;
            }
            if exit.changed().await.is_err() {
                return ProcessExit {
                    success: false,
                    code: None,
                };
            }
        }
    }
}

pub(crate) struct LaunchedServer {
    pub stdout: BoxReader,
    pub stderr: BoxReader,
    pub stdin: BoxWriter,
    pub process: ProcessHandle,
}

#[async_trait]
pub(crate) trait ServerLauncher: Send + Sync + 'static {
    async fn launch(
        &self,
        command: ResolvedServerCommand,
        canonical_project_root: PathBuf,
    ) -> Result<LaunchedServer, SessionError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct ProcessServerLauncher;

#[async_trait]
impl ServerLauncher for ProcessServerLauncher {
    async fn launch(
        &self,
        command: ResolvedServerCommand,
        canonical_project_root: PathBuf,
    ) -> Result<LaunchedServer, SessionError> {
        let validated_command = command.clone();
        let validated_root = canonical_project_root.clone();
        tokio::task::spawn_blocking(move || {
            revalidate_server_command(&validated_command, &validated_root)
        })
        .await
        .map_err(|error| SessionError::Launch(format!("validation task failed: {error}")))??;

        // There is an unavoidable residual path-based exec race after this immediate
        // validation. The Task 7 ruling explicitly retains it; fd-based execution is
        // outside the POC plan.
        let mut child = sanitized_command(&command, &canonical_project_root)
            .spawn()
            .map_err(|error| SessionError::Launch(error.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SessionError::Launch("language server stdout was not piped".into()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SessionError::Launch("language server stdin was not piped".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| SessionError::Launch("language server stderr was not piped".into()))?;
        let (kill_tx, kill_rx) = oneshot::channel();
        let (exit_tx, exit_rx) = watch::channel(None);
        tokio::spawn(async move {
            let status = tokio::select! {
                status = child.wait() => status,
                _ = kill_rx => {
                    let _ = child.kill().await;
                    child.wait().await
                }
            };
            let exit = status.map_or(
                ProcessExit {
                    success: false,
                    code: None,
                },
                |status| ProcessExit {
                    success: status.success(),
                    code: status.code(),
                },
            );
            let _ = exit_tx.send(Some(exit));
        });
        Ok(LaunchedServer {
            stdout: Box::new(stdout),
            stderr: Box::new(stderr),
            stdin: Box::new(stdin),
            process: ProcessHandle::new(kill_tx, exit_rx),
        })
    }
}

fn sanitized_command(command: &ResolvedServerCommand, root: &Path) -> Command {
    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin:/usr/sbin:/sbin".into()),
        );
    for key in [
        "HOME", "TMPDIR", "LANG", "LC_ALL", "LC_CTYPE", "USER", "LOGNAME",
    ] {
        if let Some(value) = std::env::var_os(key) {
            process.env(key, value);
        }
    }
    process
}

fn revalidate_server_command(
    command: &ResolvedServerCommand,
    canonical_project_root: &Path,
) -> Result<(), SessionError> {
    require_canonical_directory(canonical_project_root, "project root")?;
    require_canonical_directory(&command.resource_root, "resource root")?;
    if !command.program.is_absolute() {
        return Err(SessionError::Integrity("program is not absolute".into()));
    }

    let mut identified = HashSet::new();
    for resource in &command.resource_files {
        if !identified.insert(resource.path.clone()) {
            return Err(SessionError::Integrity(format!(
                "duplicate resource identity for {}",
                resource.path.display()
            )));
        }
        revalidate_resource(&command.resource_root, resource)?;
    }
    if !identified.contains(&command.program) {
        return Err(SessionError::Integrity(
            "program has no catalog identity".into(),
        ));
    }
    Ok(())
}

fn require_canonical_directory(path: &Path, label: &str) -> Result<(), SessionError> {
    if !path.is_absolute()
        || fs::symlink_metadata(path)
            .map(|metadata| !metadata.file_type().is_dir() || metadata.file_type().is_symlink())
            .unwrap_or(true)
        || path.canonicalize().ok().as_deref() != Some(path)
    {
        return Err(SessionError::Integrity(format!(
            "{label} is not a canonical directory"
        )));
    }
    Ok(())
}

fn revalidate_resource(
    root: &Path,
    resource: &super::catalog::ResolvedResourceFile,
) -> Result<(), SessionError> {
    if !resource.path.is_absolute() || !resource.path.starts_with(root) {
        return Err(SessionError::Integrity(format!(
            "resource {} is outside the catalog root",
            resource.path.display()
        )));
    }
    let metadata = fs::symlink_metadata(&resource.path)
        .map_err(|error| SessionError::Integrity(error.to_string()))?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() != resource.identity.size
        || metadata.len() == 0
        || metadata.len() > MAX_REVALIDATED_FILE_BYTES
        || resource.path.canonicalize().ok().as_deref() != Some(resource.path.as_path())
    {
        return Err(SessionError::Integrity(format!(
            "resource {} changed after catalog resolution",
            resource.path.display()
        )));
    }
    let mut file = open_without_following_symlink(&resource.path)
        .map_err(|error| SessionError::Integrity(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| SessionError::Integrity(error.to_string()))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .filter(|total| *total <= resource.identity.size)
            .ok_or_else(|| SessionError::Integrity("resource grew while hashing".into()))?;
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if total != resource.identity.size || actual != resource.identity.sha256 {
        return Err(SessionError::Integrity(format!(
            "resource {} failed catalog identity validation",
            resource.path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn open_without_following_symlink(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_without_following_symlink(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new().read(true).open(path)
}

#[derive(Debug)]
pub(crate) enum SessionError {
    Launch(String),
    Integrity(String),
    Protocol(String),
    Timeout(&'static str),
    Cancelled,
    UnknownDocument(String),
    UnknownCompletion(String),
    NonLocalUri(String),
    Document(DocumentError),
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Launch(message) => {
                write!(formatter, "could not launch language server: {message}")
            }
            Self::Integrity(message) => write!(
                formatter,
                "language server integrity check failed: {message}"
            ),
            Self::Protocol(message) => {
                write!(formatter, "language server protocol failed: {message}")
            }
            Self::Timeout(operation) => write!(formatter, "language server {operation} timed out"),
            Self::Cancelled => formatter.write_str("language server startup was cancelled"),
            Self::UnknownDocument(document) => write!(formatter, "unknown LSP document {document}"),
            Self::UnknownCompletion(item) => write!(formatter, "unknown completion item {item}"),
            Self::NonLocalUri(uri) => write!(formatter, "non-file LSP URI is unsupported: {uri}"),
            Self::Document(error) => write!(formatter, "invalid document change: {error:?}"),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<DocumentError> for SessionError {
    fn from(error: DocumentError) -> Self {
        Self::Document(error)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SessionDocument {
    pub document_id: String,
    pub uri: lsp::Url,
    pub language_id: String,
    pub version: i32,
    pub text: String,
}

struct OpenDocument {
    uri: lsp::Url,
    text: VersionedDocument,
}

struct CachedCompletion {
    document_id: String,
    source_version: i32,
    generation: u64,
    item: lsp::CompletionItem,
}

#[derive(Default)]
struct CompletionCache {
    items: HashMap<String, CachedCompletion>,
    generations: VecDeque<(String, u64, Vec<String>)>,
}

impl CompletionCache {
    fn insert_generation(
        &mut self,
        document_id: &str,
        source_version: i32,
        generation: u64,
        items: Vec<(String, lsp::CompletionItem)>,
    ) {
        let mut ids = Vec::with_capacity(items.len());
        for (id, item) in items {
            ids.push(id.clone());
            self.items.insert(
                id,
                CachedCompletion {
                    document_id: document_id.to_owned(),
                    source_version,
                    generation,
                    item,
                },
            );
        }
        self.generations
            .push_back((document_id.to_owned(), generation, ids));
        while self.generations.len() > MAX_COMPLETION_GENERATIONS
            || self.items.len() > MAX_COMPLETION_ITEMS
        {
            let Some((_, _, expired)) = self.generations.pop_front() else {
                break;
            };
            for id in expired {
                self.items.remove(&id);
            }
        }
    }

    fn purge_document(&mut self, document_id: &str) {
        self.items.retain(|_, item| item.document_id != document_id);
        self.generations
            .retain(|(cached_document, _, _)| cached_document != document_id);
    }
}

struct SessionInner {
    server: ServerSocket,
    process: ProcessHandle,
    documents: Mutex<HashMap<String, OpenDocument>>,
    completion_items: Mutex<CompletionCache>,
    capabilities: Arc<std::sync::Mutex<SessionCapabilityState>>,
    next_completion_generation: AtomicU64,
    request_gate: Mutex<()>,
    next_request_id: AtomicI32,
    shutdown_timeout: Duration,
}

impl Drop for SessionInner {
    fn drop(&mut self) {
        self.process.kill();
    }
}

#[derive(Clone)]
pub(crate) struct LspSession {
    inner: Arc<SessionInner>,
}

trait InitializedNotifier: Send + Sync {
    fn notify(&self, server: &ServerSocket) -> Result<(), SessionError>;
}

#[derive(Debug, Default, Clone, Copy)]
struct ProtocolInitializedNotifier;

impl InitializedNotifier for ProtocolInitializedNotifier {
    fn notify(&self, server: &ServerSocket) -> Result<(), SessionError> {
        server
            .notify::<lsp::notification::Initialized>(lsp::InitializedParams {})
            .map_err(|error| SessionError::Protocol(error.to_string()))
    }
}

async fn clean_up_failed_startup(
    process: &ProcessHandle,
    mainloop_task: tokio::task::JoinHandle<()>,
    stderr_task: tokio::task::JoinHandle<()>,
) {
    process.kill();
    // Kill acknowledgement is not an exit fact. Keep both pipe-draining tasks
    // alive until the process is reaped, then consume their task handles so a
    // failed start cannot leak detached startup work.
    let _ = process.wait().await;
    mainloop_task.abort();
    stderr_task.abort();
    let _ = mainloop_task.await;
    let _ = stderr_task.await;
}

impl LspSession {
    pub(crate) async fn start<L: ServerLauncher>(
        descriptor: &'static AdapterDescriptor,
        command: ResolvedServerCommand,
        canonical_project_root: PathBuf,
        launcher: L,
        events: mpsc::Sender<ClientEvent>,
    ) -> Result<Self, SessionError> {
        Self::start_with_cancellation(
            descriptor,
            command,
            canonical_project_root,
            launcher,
            events,
            CancellationToken::new(),
        )
        .await
    }

    pub(crate) async fn start_with_cancellation<L: ServerLauncher>(
        descriptor: &'static AdapterDescriptor,
        command: ResolvedServerCommand,
        canonical_project_root: PathBuf,
        launcher: L,
        events: mpsc::Sender<ClientEvent>,
        cancellation: CancellationToken,
    ) -> Result<Self, SessionError> {
        Self::start_with_initialized_notifier_and_cancellation(
            descriptor,
            command,
            canonical_project_root,
            launcher,
            events,
            ProtocolInitializedNotifier,
            cancellation,
        )
        .await
    }

    async fn start_with_initialized_notifier<L: ServerLauncher, N: InitializedNotifier>(
        descriptor: &'static AdapterDescriptor,
        command: ResolvedServerCommand,
        canonical_project_root: PathBuf,
        launcher: L,
        events: mpsc::Sender<ClientEvent>,
        initialized_notifier: N,
    ) -> Result<Self, SessionError> {
        Self::start_with_initialized_notifier_and_cancellation(
            descriptor,
            command,
            canonical_project_root,
            launcher,
            events,
            initialized_notifier,
            CancellationToken::new(),
        )
        .await
    }

    async fn start_with_initialized_notifier_and_cancellation<
        L: ServerLauncher,
        N: InitializedNotifier,
    >(
        descriptor: &'static AdapterDescriptor,
        command: ResolvedServerCommand,
        canonical_project_root: PathBuf,
        launcher: L,
        events: mpsc::Sender<ClientEvent>,
        initialized_notifier: N,
        cancellation: CancellationToken,
    ) -> Result<Self, SessionError> {
        // Validate static adapter data before a child exists. From this point
        // onward every fallible startup edge shares the kill-and-reap path.
        let workspace_configuration = serde_json::from_str(descriptor.workspace_configuration_json)
            .map_err(|error| SessionError::Protocol(error.to_string()))?;
        let launched = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(SessionError::Cancelled),
            launched = launcher.launch(command, canonical_project_root.clone()) => launched?,
        };
        let event_sink = EventSink::new(events);
        let stderr_events = event_sink.clone();
        let mut stderr = launched.stderr;
        let stderr_task = tokio::spawn(async move {
            let mut chunk = [0_u8; 4096];
            loop {
                match stderr.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(bytes) => stderr_events.send(ClientEvent::Stderr {
                        bytes: bytes.try_into().unwrap_or(u32::MAX),
                    }),
                }
            }
        });
        let capabilities = Arc::new(std::sync::Mutex::new(SessionCapabilityState::default()));
        let router = Router::from_language_client(ClientState::new(
            event_sink.clone(),
            workspace_configuration,
            capabilities.clone(),
        ));
        let (mainloop, mut server) = MainLoop::new_client(|_| {
            ServiceBuilder::new()
                .layer(CatchUnwindLayer::default())
                .layer(ConcurrencyLayer::default())
                .service(router)
        });
        let protocol_events = event_sink.clone();
        let mainloop_task = tokio::spawn(async move {
            let result = mainloop
                .run_buffered(launched.stdout.compat(), launched.stdin.compat_write())
                .await;
            protocol_events.send(ClientEvent::ProtocolExited(
                result.err().map(|error| error.to_string()),
            ));
        });

        let init_timeout = descriptor
            .timeouts
            .initialize
            .max(MIN_INITIALIZE_TIMEOUT)
            .min(MAX_INITIALIZE_TIMEOUT);
        let startup = async {
            let initialize =
                server.initialize(initialize_params(descriptor, &canonical_project_root));
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => Err(SessionError::Cancelled),
                result = timeout(init_timeout, initialize) => match result {
                    Ok(Ok(result)) => Ok(result),
                    Ok(Err(error)) => Err(SessionError::Protocol(error.to_string())),
                    Err(_) => {
                        let _ = server.notify::<lsp::notification::Cancel>(lsp::CancelParams {
                            id: lsp::NumberOrString::Number(0),
                        });
                        Err(SessionError::Timeout("initialize"))
                    }
                },
            }?;
            let features = normalized_capabilities(&result.capabilities);
            let triggers = negotiated_triggers(&result.capabilities);
            let sync = sync_policy(result.capabilities.text_document_sync);
            {
                let mut capabilities = capabilities.lock().expect("LSP capability state poisoned");
                capabilities.set_static_sync(sync);
                capabilities.set_features(features);
                capabilities.set_triggers(triggers);
            }
            event_sink.send(ClientEvent::CapabilitiesChanged(features));
            if cancellation.is_cancelled() {
                return Err(SessionError::Cancelled);
            }
            initialized_notifier.notify(&server)?;
            if cancellation.is_cancelled() {
                return Err(SessionError::Cancelled);
            }
            Ok(())
        }
        .await;
        if let Err(error) = startup {
            clean_up_failed_startup(&launched.process, mainloop_task, stderr_task).await;
            return Err(error);
        }
        // Only successful sessions get an independent process reporter. A
        // failed start above owns and consumes the sole exit wait instead.
        let process_events = event_sink.clone();
        let process_for_report = launched.process.clone();
        tokio::spawn(async move {
            let status = process_for_report.wait().await;
            process_events.send(ClientEvent::ProcessExited {
                success: status.success,
                code: status.code,
            });
        });
        Ok(Self {
            inner: Arc::new(SessionInner {
                server,
                process: launched.process,
                documents: Mutex::new(HashMap::new()),
                completion_items: Mutex::new(CompletionCache::default()),
                capabilities,
                next_completion_generation: AtomicU64::new(1),
                request_gate: Mutex::new(()),
                next_request_id: AtomicI32::new(1),
                shutdown_timeout: descriptor.timeouts.shutdown.min(Duration::from_secs(3)),
            }),
        })
    }

    pub(crate) async fn did_open(&self, document: SessionDocument) -> Result<(), SessionError> {
        if document.uri.scheme() != "file" {
            return Err(SessionError::NonLocalUri(document.uri.to_string()));
        }
        let sync = self
            .inner
            .capabilities
            .lock()
            .expect("LSP capability state poisoned")
            .sync_policy();
        let item = lsp::TextDocumentItem {
            uri: document.uri.clone(),
            language_id: document.language_id.clone(),
            version: document.version,
            text: document.text.clone(),
        };
        {
            let mut documents = self.inner.documents.lock().await;
            documents.insert(
                document.document_id.clone(),
                OpenDocument {
                    uri: document.uri,
                    text: VersionedDocument::new(
                        &document.document_id,
                        &document.text,
                        document.version,
                    )?,
                },
            );
        }
        self.inner
            .completion_items
            .lock()
            .await
            .purge_document(&document.document_id);
        if sync.open {
            self.inner
                .server
                .notify::<lsp::notification::DidOpenTextDocument>(lsp::DidOpenTextDocumentParams {
                    text_document: item,
                })
                .map_err(|error| SessionError::Protocol(error.to_string()))?;
        }
        Ok(())
    }

    pub(crate) fn capabilities(&self) -> LspCapabilities {
        self.inner
            .capabilities
            .lock()
            .expect("LSP capability state poisoned")
            .features()
    }

    pub(crate) fn trigger_characters(&self) -> NegotiatedTriggers {
        self.inner
            .capabilities
            .lock()
            .expect("LSP capability state poisoned")
            .triggers()
    }

    pub(crate) async fn did_change(&self, batch: LspChangeBatch) -> Result<i32, SessionError> {
        let sync = self
            .inner
            .capabilities
            .lock()
            .expect("LSP capability state poisoned")
            .sync_policy();
        let document_id = batch.document_id.clone();
        let (uri, version, changes) = {
            let mut documents = self.inner.documents.lock().await;
            let document = documents
                .get_mut(&batch.document_id)
                .ok_or_else(|| SessionError::UnknownDocument(batch.document_id.clone()))?;
            let applied = document.text.apply_batch(batch)?;
            let changes = if sync.change == lsp::TextDocumentSyncKind::FULL {
                vec![lsp::TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: document.text.text(),
                }]
            } else {
                applied.changes
            };
            (document.uri.clone(), applied.version, changes)
        };
        self.inner
            .completion_items
            .lock()
            .await
            .purge_document(&document_id);
        if sync.change != lsp::TextDocumentSyncKind::NONE {
            self.inner
                .server
                .notify::<lsp::notification::DidChangeTextDocument>(
                    lsp::DidChangeTextDocumentParams {
                        text_document: lsp::VersionedTextDocumentIdentifier { uri, version },
                        content_changes: changes,
                    },
                )
                .map_err(|error| SessionError::Protocol(error.to_string()))?;
        }
        Ok(version)
    }

    pub(crate) async fn did_save(&self, document_id: &str) -> Result<(), SessionError> {
        let sync = self
            .inner
            .capabilities
            .lock()
            .expect("LSP capability state poisoned")
            .sync_policy();
        let (uri, text) = {
            let documents = self.inner.documents.lock().await;
            let document = documents
                .get(document_id)
                .ok_or_else(|| SessionError::UnknownDocument(document_id.to_owned()))?;
            (document.uri.clone(), document.text.text())
        };
        if sync.save {
            self.inner
                .server
                .notify::<lsp::notification::DidSaveTextDocument>(lsp::DidSaveTextDocumentParams {
                    text_document: lsp::TextDocumentIdentifier { uri },
                    text: sync.save_include_text.then_some(text),
                })
                .map_err(|error| SessionError::Protocol(error.to_string()))?;
        }
        Ok(())
    }

    pub(crate) async fn did_close(&self, document_id: &str) -> Result<(), SessionError> {
        let sync = self
            .inner
            .capabilities
            .lock()
            .expect("LSP capability state poisoned")
            .sync_policy();
        let document = self
            .inner
            .documents
            .lock()
            .await
            .remove(document_id)
            .ok_or_else(|| SessionError::UnknownDocument(document_id.to_owned()))?;
        self.inner
            .completion_items
            .lock()
            .await
            .purge_document(document_id);
        if sync.close {
            self.inner
                .server
                .notify::<lsp::notification::DidCloseTextDocument>(
                    lsp::DidCloseTextDocumentParams {
                        text_document: lsp::TextDocumentIdentifier { uri: document.uri },
                    },
                )
                .map_err(|error| SessionError::Protocol(error.to_string()))?;
        }
        Ok(())
    }

    pub(crate) async fn completion(
        &self,
        document_id: &str,
        position: lsp::Position,
    ) -> Result<CompletionResponse, SessionError> {
        let (uri, version) = self.document_snapshot(document_id).await?;
        let params = lsp::CompletionParams {
            text_document_position: text_document_position(uri, position),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: None,
        };
        let response = self
            .request(SHORT_REQUEST_TIMEOUT, "completion", move |server| {
                server.completion(params)
            })
            .await?;
        let generation = self
            .inner
            .next_completion_generation
            .fetch_add(1, Ordering::Relaxed);
        let (response, originals) =
            normalize_completion(document_id, version, generation, response);
        self.inner.completion_items.lock().await.insert_generation(
            document_id,
            version,
            generation,
            originals,
        );
        Ok(response)
    }

    pub(crate) async fn resolve_completion_item(
        &self,
        item_id: &str,
    ) -> Result<CompletionItem, SessionError> {
        let (document_id, source_version, generation, item) = {
            let cache = self.inner.completion_items.lock().await;
            let cached = cache
                .items
                .get(item_id)
                .ok_or_else(|| SessionError::UnknownCompletion(item_id.to_owned()))?;
            (
                cached.document_id.clone(),
                cached.source_version,
                cached.generation,
                cached.item.clone(),
            )
        };
        let current_version = self.document_snapshot(&document_id).await?.1;
        if current_version != source_version {
            return Err(SessionError::UnknownCompletion(item_id.to_owned()));
        }
        let resolved = self
            .request(SHORT_REQUEST_TIMEOUT, "completion resolve", move |server| {
                server.completion_item_resolve(item)
            })
            .await?;
        if let Some(cached) = self
            .inner
            .completion_items
            .lock()
            .await
            .items
            .get_mut(item_id)
            .filter(|cached| {
                cached.generation == generation && cached.source_version == source_version
            })
        {
            cached.item = resolved.clone();
        }
        Ok(normalize_resolved_completion(item_id.to_owned(), &resolved))
    }

    pub(crate) async fn hover(
        &self,
        document_id: &str,
        position: lsp::Position,
    ) -> Result<HoverResponse, SessionError> {
        let (uri, version) = self.document_snapshot(document_id).await?;
        let params = lsp::HoverParams {
            text_document_position_params: text_document_position(uri, position),
            work_done_progress_params: Default::default(),
        };
        let response = self
            .request(SHORT_REQUEST_TIMEOUT, "hover", move |server| {
                server.hover(params)
            })
            .await?;
        Ok(normalize_hover(document_id, version, response))
    }

    pub(crate) async fn signature_help(
        &self,
        document_id: &str,
        position: lsp::Position,
    ) -> Result<SignatureHelpResponse, SessionError> {
        let (uri, version) = self.document_snapshot(document_id).await?;
        let params = lsp::SignatureHelpParams {
            context: None,
            text_document_position_params: text_document_position(uri, position),
            work_done_progress_params: Default::default(),
        };
        let response = self
            .request(SHORT_REQUEST_TIMEOUT, "signature help", move |server| {
                server.signature_help(params)
            })
            .await?;
        Ok(normalize_signature_help(document_id, version, response))
    }

    pub(crate) async fn definition(
        &self,
        document_id: &str,
        position: lsp::Position,
    ) -> Result<DefinitionResponse, SessionError> {
        let (uri, version) = self.document_snapshot(document_id).await?;
        let params = lsp::GotoDefinitionParams {
            text_document_position_params: text_document_position(uri, position),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let response = self
            .request(DEFINITION_TIMEOUT, "definition", move |server| {
                server.definition(params)
            })
            .await?;
        Ok(normalize_definition(document_id, version, response))
    }

    pub(crate) async fn pull_diagnostics(
        &self,
        document_id: &str,
        previous_result_id: Option<String>,
    ) -> Result<(Option<String>, Vec<Diagnostic>), SessionError> {
        let (uri, _) = self.document_snapshot(document_id).await?;
        let params = lsp::DocumentDiagnosticParams {
            text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
            identifier: None,
            previous_result_id,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let response = self
            .request(SHORT_REQUEST_TIMEOUT, "diagnostics", move |server| {
                server.document_diagnostic(params)
            })
            .await?;
        match response {
            lsp::DocumentDiagnosticReportResult::Report(lsp::DocumentDiagnosticReport::Full(
                report,
            )) => Ok((
                report.full_document_diagnostic_report.result_id,
                normalize_diagnostics(&uri, report.full_document_diagnostic_report.items),
            )),
            lsp::DocumentDiagnosticReportResult::Report(
                lsp::DocumentDiagnosticReport::Unchanged(report),
            ) => Ok((
                Some(report.unchanged_document_diagnostic_report.result_id),
                Vec::new(),
            )),
            lsp::DocumentDiagnosticReportResult::Partial(_) => Err(SessionError::Protocol(
                "partial document diagnostic response without a full report".into(),
            )),
        }
    }

    pub(crate) async fn shutdown(&self) -> Result<(), SessionError> {
        let deadline = tokio::time::Instant::now() + self.inner.shutdown_timeout;
        let result = self
            .request(self.inner.shutdown_timeout, "shutdown", |server| {
                server.shutdown(())
            })
            .await;
        if result.is_err() {
            self.inner.process.kill();
            // A kill acknowledgement is not a reap. Keep the generation live
            // until the process watcher has observed the OS child exit.
            let _ = self.inner.process.wait().await;
            return result;
        }
        let _ = self.inner.server.notify::<lsp::notification::Exit>(());
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() || timeout(remaining, self.inner.process.wait()).await.is_err() {
            self.inner.process.kill();
            let _ = self.inner.process.wait().await;
        }
        result
    }

    async fn document_snapshot(&self, document_id: &str) -> Result<(lsp::Url, i32), SessionError> {
        let documents = self.inner.documents.lock().await;
        let document = documents
            .get(document_id)
            .ok_or_else(|| SessionError::UnknownDocument(document_id.to_owned()))?;
        Ok((document.uri.clone(), document.text.version()))
    }

    async fn request<T: Send + 'static>(
        &self,
        duration: Duration,
        operation: &'static str,
        make_request: impl FnOnce(&mut ServerSocket) -> BoxRequest<T>,
    ) -> Result<T, SessionError> {
        let (request_id, request) = {
            let gate = self.inner.request_gate.lock().await;
            let request_id = self.inner.next_request_id.fetch_add(1, Ordering::Relaxed);
            let mut server = self.inner.server.clone();
            let request = make_request(&mut server);
            drop(gate);
            (request_id, request)
        };
        match timeout(duration, request).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => Err(SessionError::Protocol(error.to_string())),
            Err(_) => {
                let _ = self
                    .inner
                    .server
                    .notify::<lsp::notification::Cancel>(lsp::CancelParams {
                        id: lsp::NumberOrString::Number(request_id),
                    });
                Err(SessionError::Timeout(operation))
            }
        }
    }
}

fn text_document_position(
    uri: lsp::Url,
    position: lsp::Position,
) -> lsp::TextDocumentPositionParams {
    lsp::TextDocumentPositionParams {
        text_document: lsp::TextDocumentIdentifier { uri },
        position,
    }
}

#[allow(deprecated)]
fn initialize_params(descriptor: &AdapterDescriptor, root: &Path) -> lsp::InitializeParams {
    let capabilities: lsp::ClientCapabilities = serde_json::from_value(serde_json::json!({
        "workspace": {
            "configuration": true
        },
        "textDocument": {
            "synchronization": {
                "dynamicRegistration": true,
                "didSave": true
            },
            "completion": {
                "contextSupport": true,
                "completionItem": {
                    "snippetSupport": true,
                    "commitCharactersSupport": true,
                    "documentationFormat": ["markdown", "plaintext"],
                    "deprecatedSupport": true,
                    "insertReplaceSupport": true,
                    "resolveSupport": {
                        "properties": ["documentation", "detail", "additionalTextEdits"]
                    }
                }
            },
            "hover": {
                "contentFormat": ["markdown", "plaintext"]
            },
            "signatureHelp": {
                "signatureInformation": {
                    "documentationFormat": ["markdown", "plaintext"],
                    "parameterInformation": { "labelOffsetSupport": true },
                    "activeParameterSupport": true
                },
                "contextSupport": true
            },
            "definition": {
                "linkSupport": true
            },
            "publishDiagnostics": {
                "relatedInformation": true,
                "versionSupport": true
            },
            "diagnostic": {
                "relatedDocumentSupport": false
            }
        },
        "window": {
            "workDoneProgress": true
        },
        "general": {
            "positionEncodings": ["utf-16"],
            "staleRequestSupport": {
                "cancel": true,
                "retryOnContentModified": []
            }
        }
    }))
    .expect("static client capabilities are valid");
    let root_uri = lsp::Url::from_file_path(root).expect("canonical project root is a file URI");
    lsp::InitializeParams {
        process_id: Some(std::process::id()),
        root_path: None,
        root_uri: Some(root_uri.clone()),
        initialization_options: serde_json::from_str(descriptor.initialization_options_json).ok(),
        capabilities,
        trace: Some(lsp::TraceValue::Off),
        workspace_folders: Some(vec![lsp::WorkspaceFolder {
            uri: root_uri,
            name: root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("project")
                .to_owned(),
        }]),
        client_info: Some(lsp::ClientInfo {
            name: "TermLab".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
        locale: None,
        work_done_progress_params: Default::default(),
    }
}

fn sync_policy(capability: Option<lsp::TextDocumentSyncCapability>) -> SyncPolicy {
    match capability {
        None => SyncPolicy {
            change: lsp::TextDocumentSyncKind::NONE,
            open: false,
            close: false,
            save: false,
            save_include_text: false,
        },
        Some(lsp::TextDocumentSyncCapability::Kind(change)) => SyncPolicy {
            change,
            open: true,
            close: true,
            save: true,
            save_include_text: false,
        },
        Some(lsp::TextDocumentSyncCapability::Options(options)) => {
            let (save, save_include_text) = match options.save {
                None => (false, false),
                Some(lsp::TextDocumentSyncSaveOptions::Supported(save)) => (save, false),
                Some(lsp::TextDocumentSyncSaveOptions::SaveOptions(options)) => {
                    (true, options.include_text.unwrap_or(false))
                }
            };
            SyncPolicy {
                change: options.change.unwrap_or(lsp::TextDocumentSyncKind::NONE),
                open: options.open_close.unwrap_or(false),
                close: options.open_close.unwrap_or(false),
                save,
                save_include_text,
            }
        }
    }
}

/// The trigger characters the server advertised, normalized (empties and
/// duplicates dropped) but not yet merged with the catalog's curated
/// completion list — that merge needs the document's language, which only the
/// manager knows.
fn negotiated_triggers(capabilities: &lsp::ServerCapabilities) -> NegotiatedTriggers {
    let completion = capabilities
        .completion_provider
        .as_ref()
        .and_then(|provider| provider.trigger_characters.clone())
        .unwrap_or_default();
    let (signature_help, signature_help_retrigger) = capabilities
        .signature_help_provider
        .as_ref()
        .map(|provider| {
            (
                provider.trigger_characters.clone().unwrap_or_default(),
                provider.retrigger_characters.clone().unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    NegotiatedTriggers {
        completion: normalize_triggers(&completion),
        signature_help: normalize_triggers(&signature_help),
        signature_help_retrigger: normalize_triggers(&signature_help_retrigger),
    }
}

fn normalized_capabilities(capabilities: &lsp::ServerCapabilities) -> LspCapabilities {
    let hover = capabilities
        .hover_provider
        .as_ref()
        .is_some_and(|provider| !matches!(provider, lsp::HoverProviderCapability::Simple(false)));
    let definition = capabilities
        .definition_provider
        .as_ref()
        .is_some_and(|provider| !matches!(provider, lsp::OneOf::Left(false)));
    LspCapabilities {
        completion: capabilities.completion_provider.is_some(),
        hover,
        signature_help: capabilities.signature_help_provider.is_some(),
        definition,
        diagnostics: capabilities.diagnostic_provider.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::time::Duration;

    use async_lsp::lsp_types::Position;
    #[cfg(unix)]
    use sha2::{Digest, Sha256};
    #[cfg(unix)]
    use tempfile::TempDir;

    use super::{LspSession, ProcessServerLauncher, ServerLauncher, SessionDocument, SessionError};
    use crate::lsp::catalog::{AdapterDescriptor, ResolvedServerCommand};
    #[cfg(unix)]
    use crate::lsp::catalog::{ResolvedFileIdentity, ResolvedResourceFile};
    use crate::lsp::client::{ClientEvent, ProgressPayload};
    use crate::lsp::test_support::{
        MockServerLauncher, ObservedProtocol, ServerScript, full_feature_script, root,
        test_command, test_descriptor,
    };
    use crate::lsp::types::{LspChangeBatch, LspTextChange};

    #[test]
    fn negotiated_triggers_reads_both_providers_and_normalizes_them() {
        let capabilities = async_lsp::lsp_types::ServerCapabilities {
            completion_provider: Some(async_lsp::lsp_types::CompletionOptions {
                trigger_characters: Some(vec![
                    ".".to_owned(),
                    ".".to_owned(),
                    String::new(),
                    "::".to_owned(),
                ]),
                ..Default::default()
            }),
            signature_help_provider: Some(async_lsp::lsp_types::SignatureHelpOptions {
                trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
                retrigger_characters: Some(vec![",".to_owned()]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let triggers = super::negotiated_triggers(&capabilities);

        assert_eq!(triggers.completion, vec![".".to_owned(), "::".to_owned()]);
        assert_eq!(
            triggers.signature_help,
            vec!["(".to_owned(), ",".to_owned()]
        );
        assert_eq!(triggers.signature_help_retrigger, vec![",".to_owned()]);
    }

    #[test]
    fn a_server_advertising_no_providers_negotiates_no_triggers() {
        let triggers =
            super::negotiated_triggers(&async_lsp::lsp_types::ServerCapabilities::default());

        assert_eq!(triggers, crate::lsp::types::NegotiatedTriggers::default());
    }

    fn descriptor_with_configuration(configuration: &'static str) -> &'static AdapterDescriptor {
        Box::leak(Box::new(AdapterDescriptor {
            workspace_configuration_json: configuration,
            ..*test_descriptor()
        }))
    }

    fn session_document(document_id: &str, uri: async_lsp::lsp_types::Url) -> SessionDocument {
        SessionDocument {
            document_id: document_id.into(),
            uri,
            language_id: "typescript".into(),
            version: 1,
            text: "con".into(),
        }
    }

    async fn wait_for_startup_kill(observed: &ObservedProtocol) {
        for _ in 0..100 {
            if observed.kill_observed() {
                tokio::task::yield_now().await;
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("failed startup never killed its launched process");
    }

    #[tokio::test]
    async fn session_round_trips_editor_features() {
        let (launcher, observed) = MockServerLauncher::scripted(full_feature_script());
        let (sink, mut events) = tokio::sync::mpsc::channel(16);
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        assert_eq!(
            session.capabilities(),
            crate::lsp::types::LspCapabilities {
                completion: true,
                hover: true,
                signature_help: true,
                definition: true,
                diagnostics: true,
            }
        );
        session
            .did_open(SessionDocument {
                document_id: "doc-1".into(),
                uri: async_lsp::lsp_types::Url::from_file_path(root().join("main.ts")).unwrap(),
                language_id: "typescript".into(),
                version: 1,
                text: "con".into(),
            })
            .await
            .unwrap();
        session
            .did_change(LspChangeBatch {
                document_id: "doc-1".into(),
                base_version: 1,
                next_version: 2,
                changes: vec![
                    LspTextChange {
                        from_utf16: 0,
                        to_utf16: 1,
                        inserted_text: "C".into(),
                    },
                    LspTextChange {
                        from_utf16: 2,
                        to_utf16: 3,
                        inserted_text: "N".into(),
                    },
                ],
            })
            .await
            .unwrap();
        session.did_save("doc-1").await.unwrap();
        let completion = session
            .completion("doc-1", Position::new(0, 3))
            .await
            .unwrap();
        assert_eq!(completion.items[0].label, "console");
        let completion_id = completion.items[0].id.clone();
        assert!(
            session
                .resolve_completion_item(&completion_id)
                .await
                .unwrap()
                .documentation[0]
                .markdown
        );
        assert_eq!(
            session
                .hover("doc-1", Position::new(0, 3))
                .await
                .unwrap()
                .blocks[0]
                .value,
            "`console`"
        );
        assert_eq!(
            session
                .signature_help("doc-1", Position::new(0, 3))
                .await
                .unwrap()
                .signatures[0]
                .active_parameter,
            Some(0)
        );
        assert!(
            !session
                .definition("doc-1", Position::new(0, 3))
                .await
                .unwrap()
                .locations
                .is_empty()
        );
        let (result_id, diagnostics) = session.pull_diagnostics("doc-1", None).await.unwrap();
        assert_eq!(result_id.as_deref(), Some("diagnostics-1"));
        assert_eq!(diagnostics[0].message, "pull error");
        session.did_close("doc-1").await.unwrap();
        session.shutdown().await.unwrap();
        let mut received_events = Vec::new();
        while let Ok(event) = events.try_recv() {
            received_events.push(event);
        }
        assert!(
            received_events
                .iter()
                .any(|event| matches!(event, ClientEvent::Diagnostics { .. }))
        );
        assert!(
            received_events
                .iter()
                .any(|event| matches!(event, ClientEvent::Progress { .. }))
        );
        assert!(received_events
            .iter()
            .any(|event| matches!(event, ClientEvent::Message { kind, message } if kind == "warning" && message == "mock warning")));
        assert!(received_events.iter().any(|event| {
            matches!(event, ClientEvent::RegistrationsChanged(registrations) if registrations.is_empty())
        }));
        observed.assert_order(&[
            "initialize",
            "initialized",
            "textDocument/didOpen",
            "textDocument/didChange",
            "textDocument/didSave",
            "textDocument/completion",
            "completionItem/resolve",
            "textDocument/hover",
            "textDocument/signatureHelp",
            "textDocument/definition",
            "textDocument/diagnostic",
            "textDocument/didClose",
            "shutdown",
            "exit",
        ]);
        observed.assert_incremental_changes_descend();
        observed.assert_saved_text("CoN");
        observed.assert_configuration_and_unsupported_request_responses();
    }

    #[tokio::test]
    async fn stderr_is_drained_as_bounded_redacted_facts() {
        let (launcher, _observed) = MockServerLauncher::scripted(ServerScript::StderrFlood);
        let (sink, mut events) = tokio::sync::mpsc::channel(64);
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        let mut chunks = 0_usize;
        let mut bytes = 0_u64;
        while bytes < 256 * 1024 {
            let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
                .await
                .unwrap()
                .unwrap();
            if let ClientEvent::Stderr { bytes: chunk } = event {
                chunks += 1;
                bytes += u64::from(chunk);
                assert!(chunk <= 4096);
                assert!(!format!("{event:?}").contains("ssssssss"));
            }
        }
        assert!(chunks <= 64);
        session.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn workspace_configuration_sections_round_trip_in_requested_order() {
        let (launcher, observed) =
            MockServerLauncher::scripted(ServerScript::ConfigurationSections);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        let session = LspSession::start(
            descriptor_with_configuration(
                r#"{"typescript":{"preferences":{"quoteStyle":"single"}},"javascript":{"format":true}}"#,
            ),
            test_command(),
            root(),
            launcher,
            sink,
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !observed.has_client_response(130) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        session.shutdown().await.unwrap();
        observed.assert_configuration_sections();
    }

    #[tokio::test]
    async fn supported_dynamic_sync_registration_and_unregistration_change_behavior() {
        let (launcher, observed) = MockServerLauncher::scripted(ServerScript::DynamicSync);
        let (sink, mut events) = tokio::sync::mpsc::channel(16);
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let Some(ClientEvent::RegistrationsChanged(registrations)) = events.recv().await
                else {
                    continue;
                };
                if registrations.len() == 3
                    && registrations.iter().any(|registration| {
                        registration.id == "change-dynamic"
                            && registration.method == "textDocument/didChange"
                    })
                {
                    break;
                }
            }
        })
        .await
        .unwrap();
        session
            .did_open(session_document(
                "doc-dynamic",
                async_lsp::lsp_types::Url::from_file_path(root().join("dynamic.ts")).unwrap(),
            ))
            .await
            .unwrap();
        session
            .did_change(LspChangeBatch {
                document_id: "doc-dynamic".into(),
                base_version: 1,
                next_version: 2,
                changes: vec![LspTextChange {
                    from_utf16: 0,
                    to_utf16: 1,
                    inserted_text: "C".into(),
                }],
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(events.recv().await, Some(ClientEvent::RegistrationsChanged(registrations)) if registrations.is_empty()) {
                    break;
                }
            }
        })
        .await
        .unwrap();
        session
            .did_change(LspChangeBatch {
                document_id: "doc-dynamic".into(),
                base_version: 2,
                next_version: 3,
                changes: vec![LspTextChange {
                    from_utf16: 1,
                    to_utf16: 2,
                    inserted_text: "O".into(),
                }],
            })
            .await
            .unwrap();
        session.did_close("doc-dynamic").await.unwrap();
        session.shutdown().await.unwrap();
        assert_eq!(observed.method_count("textDocument/didOpen"), 1);
        assert_eq!(observed.method_count("textDocument/didChange"), 1);
        assert_eq!(observed.method_count("textDocument/didClose"), 0);
    }

    #[tokio::test]
    async fn false_and_absent_open_close_sync_keep_local_documents_without_lifecycle_notifications()
    {
        for (script, expected_change_count) in
            [(ServerScript::OpenCloseFalse, 1), (ServerScript::NoSync, 0)]
        {
            let (launcher, observed) = MockServerLauncher::scripted(script);
            let (sink, _events) = tokio::sync::mpsc::channel(16);
            let session =
                LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
                    .await
                    .unwrap();
            session
                .did_open(session_document(
                    "doc-local-only",
                    async_lsp::lsp_types::Url::from_file_path(root().join("local-only.ts"))
                        .unwrap(),
                ))
                .await
                .unwrap();
            assert_eq!(
                session
                    .did_change(LspChangeBatch {
                        document_id: "doc-local-only".into(),
                        base_version: 1,
                        next_version: 2,
                        changes: vec![LspTextChange {
                            from_utf16: 0,
                            to_utf16: 1,
                            inserted_text: "C".into(),
                        }],
                    })
                    .await
                    .unwrap(),
                2
            );
            session.did_close("doc-local-only").await.unwrap();
            session.shutdown().await.unwrap();
            assert_eq!(observed.method_count("textDocument/didOpen"), 0);
            assert_eq!(
                observed.method_count("textDocument/didChange"),
                expected_change_count
            );
            assert_eq!(observed.method_count("textDocument/didClose"), 0);
        }
    }

    #[tokio::test]
    async fn non_file_documents_are_rejected_before_session_mutation_or_notification() {
        let (launcher, observed) = MockServerLauncher::scripted(ServerScript::FullSync);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        for (index, uri) in [
            "untitled:buffer",
            "https://example.invalid/main.ts",
            "vscode-remote://ssh-remote+host/main.ts",
        ]
        .into_iter()
        .enumerate()
        {
            let document_id = format!("remote-{index}");
            assert!(matches!(
                session
                    .did_open(session_document(
                        &document_id,
                        async_lsp::lsp_types::Url::parse(uri).unwrap(),
                    ))
                    .await,
                Err(SessionError::NonLocalUri(rejected)) if rejected == uri
            ));
            assert!(matches!(
                session.completion(&document_id, Position::new(0, 0)).await,
                Err(SessionError::UnknownDocument(_))
            ));
        }
        session.shutdown().await.unwrap();
        assert_eq!(observed.method_count("textDocument/didOpen"), 0);
    }

    #[tokio::test]
    async fn non_file_publish_diagnostics_are_dropped_without_stopping_the_protocol() {
        let (launcher, _observed) = MockServerLauncher::scripted(ServerScript::NonFileDiagnostics);
        let (sink, mut events) = tokio::sync::mpsc::channel(16);
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        session
            .did_open(session_document(
                "doc-diagnostics",
                async_lsp::lsp_types::Url::from_file_path(root().join("diagnostics.ts")).unwrap(),
            ))
            .await
            .unwrap();
        let local = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(ClientEvent::Diagnostics {
                    uri,
                    version: Some(4),
                    diagnostics,
                }) = events.recv().await
                {
                    break (uri, diagnostics);
                }
            }
        })
        .await
        .unwrap();
        assert!(local.0.starts_with("file:"));
        assert_eq!(local.1[0].message, "local latest");
        tokio::task::yield_now().await;
        while let Ok(event) = events.try_recv() {
            if let ClientEvent::Diagnostics { uri, .. } = event {
                assert!(
                    uri.starts_with("file:"),
                    "leaked non-file diagnostics: {uri}"
                );
            }
        }
        session.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn same_version_completion_generations_resolve_originals_and_purge_on_change_and_close() {
        let (launcher, _observed) =
            MockServerLauncher::scripted(ServerScript::CompletionGenerations);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        session
            .did_open(session_document(
                "doc-generations",
                async_lsp::lsp_types::Url::from_file_path(root().join("generations.ts")).unwrap(),
            ))
            .await
            .unwrap();
        let first = session
            .completion("doc-generations", Position::new(0, 3))
            .await
            .unwrap();
        let second = session
            .completion("doc-generations", Position::new(0, 3))
            .await
            .unwrap();
        assert_ne!(first.items[0].id, second.items[0].id);
        assert_eq!(
            session
                .resolve_completion_item(&first.items[0].id)
                .await
                .unwrap()
                .documentation[0]
                .value,
            "resolved generation 1"
        );
        assert_eq!(
            session
                .resolve_completion_item(&second.items[0].id)
                .await
                .unwrap()
                .documentation[0]
                .value,
            "resolved generation 2"
        );
        let stale_after_change = session
            .completion("doc-generations", Position::new(0, 3))
            .await
            .unwrap()
            .items[0]
            .id
            .clone();
        session
            .did_change(LspChangeBatch {
                document_id: "doc-generations".into(),
                base_version: 1,
                next_version: 2,
                changes: vec![LspTextChange {
                    from_utf16: 0,
                    to_utf16: 1,
                    inserted_text: "C".into(),
                }],
            })
            .await
            .unwrap();
        assert!(matches!(
            session.resolve_completion_item(&stale_after_change).await,
            Err(SessionError::UnknownCompletion(_))
        ));
        let stale_after_close = session
            .completion("doc-generations", Position::new(0, 3))
            .await
            .unwrap()
            .items[0]
            .id
            .clone();
        session.did_close("doc-generations").await.unwrap();
        assert!(matches!(
            session.resolve_completion_item(&stale_after_close).await,
            Err(SessionError::UnknownCompletion(_))
        ));
        session.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn capacity_one_event_backpressure_retains_latest_state_and_reports_overflow() {
        let (launcher, observed) = MockServerLauncher::scripted(ServerScript::EventFlood);
        let (sink, mut events) = tokio::sync::mpsc::channel(1);
        sink.send(ClientEvent::Message {
            kind: "test-blocker".into(),
            message: "block forwarding".into(),
        })
        .await
        .unwrap();
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        session
            .did_open(session_document(
                "doc-flood",
                async_lsp::lsp_types::Url::from_file_path(root().join("flood.ts")).unwrap(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !observed.has_client_response(142) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            matches!(events.recv().await, Some(ClientEvent::Message { kind, .. }) if kind == "test-blocker")
        );
        let mut latest_diagnostics = false;
        let mut latest_registration = false;
        let mut latest_progress = false;
        let mut overflow = false;
        tokio::time::timeout(Duration::from_secs(1), async {
            while !(latest_diagnostics && latest_registration && latest_progress && overflow) {
                match events.recv().await {
                    Some(ClientEvent::Diagnostics {
                        version: Some(3),
                        diagnostics,
                        ..
                    }) => {
                        latest_diagnostics = diagnostics[0].message == "diagnostic-3";
                    }
                    Some(ClientEvent::RegistrationsChanged(registrations)) => {
                        latest_registration =
                            registrations.len() == 1 && registrations[0].id == "save-b";
                    }
                    Some(ClientEvent::Progress {
                        progress: ProgressPayload::End { message },
                        ..
                    }) => latest_progress = message.as_deref() == Some("complete"),
                    Some(ClientEvent::Overflow { dropped }) => overflow = dropped > 0,
                    Some(_) => {}
                    None => panic!("event forwarder stopped"),
                }
            }
        })
        .await
        .unwrap();
        session.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn terminal_lifecycle_survives_capacity_one_event_flood() {
        let (launcher, observed) = MockServerLauncher::scripted(ServerScript::EventFlood);
        let (sink, mut events) = tokio::sync::mpsc::channel(1);
        sink.send(ClientEvent::Message {
            kind: "test-blocker".into(),
            message: "block forwarding".into(),
        })
        .await
        .unwrap();
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        session
            .did_open(session_document(
                "doc-terminal-flood",
                async_lsp::lsp_types::Url::from_file_path(root().join("terminal-flood.ts"))
                    .unwrap(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !observed.has_client_response(142) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        session.shutdown().await.unwrap();
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }

        assert!(
            matches!(events.recv().await, Some(ClientEvent::Message { kind, .. }) if kind == "test-blocker")
        );
        let mut protocol_exit = None;
        let mut process_exit = None;
        tokio::time::timeout(Duration::from_secs(1), async {
            while protocol_exit.is_none() || process_exit.is_none() {
                match events.recv().await {
                    Some(ClientEvent::ProtocolExited(error)) => protocol_exit = Some(error),
                    Some(ClientEvent::ProcessExited { success, code }) => {
                        process_exit = Some((success, code));
                    }
                    Some(_) => {}
                    None => panic!("event forwarder stopped"),
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(
            protocol_exit,
            Some(Some("the underlying channel reached EOF".into()))
        );
        assert_eq!(process_exit, Some((true, Some(0))));
    }

    #[tokio::test]
    async fn full_sync_sends_the_complete_document_and_absent_sync_sends_no_change() {
        for (script, expected_changes) in [
            (
                ServerScript::FullSync,
                Some(vec![serde_json::json!({ "text": "cON" })]),
            ),
            (ServerScript::NoSync, None),
        ] {
            let (launcher, observed) = MockServerLauncher::scripted(script);
            let (sink, _events) = tokio::sync::mpsc::channel(16);
            let session =
                LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
                    .await
                    .unwrap();
            session
                .did_open(SessionDocument {
                    document_id: "doc-1".into(),
                    uri: async_lsp::lsp_types::Url::from_file_path(root().join("main.ts")).unwrap(),
                    language_id: "typescript".into(),
                    version: 1,
                    text: "con".into(),
                })
                .await
                .unwrap();
            session
                .did_change(LspChangeBatch {
                    document_id: "doc-1".into(),
                    base_version: 1,
                    next_version: 2,
                    changes: vec![LspTextChange {
                        from_utf16: 1,
                        to_utf16: 3,
                        inserted_text: "ON".into(),
                    }],
                })
                .await
                .unwrap();
            session.shutdown().await.unwrap();
            assert_eq!(observed.did_change_content_changes(), expected_changes);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_request_is_cancelled_by_its_protocol_id() {
        let (launcher, observed) = MockServerLauncher::scripted(ServerScript::HangingCompletion);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        session
            .did_open(SessionDocument {
                document_id: "doc-1".into(),
                uri: async_lsp::lsp_types::Url::from_file_path(root().join("main.ts")).unwrap(),
                language_id: "typescript".into(),
                version: 1,
                text: "con".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            session.completion("doc-1", Position::new(0, 3)).await,
            Err(SessionError::Timeout("completion"))
        ));
        session.shutdown().await.unwrap();
        observed.assert_cancelled_request_id(1);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_kills_a_server_that_does_not_exit() {
        let (launcher, _observed) = MockServerLauncher::scripted(ServerScript::IgnoresExit);
        let (sink, mut events) = tokio::sync::mpsc::channel(16);
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        session.shutdown().await.unwrap();
        loop {
            if let Some(ClientEvent::ProcessExited { success, code }) = events.recv().await {
                assert!(!success);
                assert_eq!(code, Some(1));
                break;
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_deadline_includes_waiting_for_the_shutdown_response() {
        let (launcher, _observed) = MockServerLauncher::scripted(ServerScript::HangingShutdown);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        let started = tokio::time::Instant::now();
        assert!(matches!(
            session.shutdown().await,
            Err(SessionError::Timeout("shutdown"))
        ));
        assert_eq!(started.elapsed(), Duration::from_secs(3));
    }

    #[tokio::test(start_paused = true)]
    async fn graceful_shutdown_timeout_kills_and_reaps_before_completion() {
        let (launcher, _observed) =
            MockServerLauncher::scripted(ServerScript::DelayedKillAfterExit);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        let shutdown = tokio::spawn(async move { session.shutdown().await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::task::yield_now().await;
        assert!(
            !shutdown.is_finished(),
            "kill acknowledgement is not process reap"
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        shutdown.await.unwrap().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn failed_shutdown_request_kills_and_reaps_before_returning_error() {
        let (launcher, _observed) =
            MockServerLauncher::scripted(ServerScript::DelayedKillAfterShutdownTimeout);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        let shutdown = tokio::spawn(async move { session.shutdown().await });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::task::yield_now().await;
        assert!(
            !shutdown.is_finished(),
            "shutdown timeout must still reap after kill"
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(matches!(
            shutdown.await.unwrap(),
            Err(SessionError::Timeout("shutdown"))
        ));
    }

    #[tokio::test]
    async fn malformed_completion_response_fails_only_that_request() {
        let (launcher, _observed) = MockServerLauncher::scripted(ServerScript::MalformedCompletion);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        let session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        session
            .did_open(SessionDocument {
                document_id: "doc-1".into(),
                uri: async_lsp::lsp_types::Url::from_file_path(root().join("main.ts")).unwrap(),
                language_id: "typescript".into(),
                version: 1,
                text: "con".into(),
            })
            .await
            .unwrap();
        assert!(matches!(
            session.completion("doc-1", Position::new(0, 3)).await,
            Err(SessionError::Protocol(_))
        ));
        session.did_save("doc-1").await.unwrap();
        session.shutdown().await.unwrap();
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn launcher_rejects_changed_program_and_resource_argument_before_spawn() {
        for changed_index in [0_usize, 1] {
            let directory = TempDir::new().unwrap();
            let program = directory.path().join("server");
            let cli = directory.path().join("cli.mjs");
            fs::write(&program, b"original program").unwrap();
            fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
            fs::write(&cli, b"original cli").unwrap();
            let root = directory.path().canonicalize().unwrap();
            let program = program.canonicalize().unwrap();
            let cli = cli.canonicalize().unwrap();
            let resource_files = [&program, &cli]
                .into_iter()
                .map(|path| {
                    let bytes = fs::read(path).unwrap();
                    ResolvedResourceFile {
                        path: path.clone(),
                        identity: ResolvedFileIdentity {
                            size: bytes.len() as u64,
                            sha256: format!("{:x}", Sha256::digest(bytes)),
                        },
                    }
                })
                .collect();
            let command = ResolvedServerCommand {
                adapter_id: "test",
                resource_root: root.clone(),
                program: program.clone(),
                args: vec![cli.clone(), "--stdio".into()],
                resource_files,
            };
            fs::write(
                if changed_index == 0 { &program } else { &cli },
                if changed_index == 0 {
                    b"tampered program".as_slice()
                } else {
                    b"tampered cli".as_slice()
                },
            )
            .unwrap();

            assert!(matches!(
                ProcessServerLauncher.launch(command, root).await,
                Err(SessionError::Integrity(_))
            ));
        }
    }

    #[test]
    #[cfg(unix)]
    fn literal_absolute_arguments_do_not_require_a_resource_identity() {
        let directory = TempDir::new().unwrap();
        let program = directory.path().join("server");
        fs::write(&program, b"server bytes").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let root = directory.path().canonicalize().unwrap();
        let program = program.canonicalize().unwrap();
        let command = ResolvedServerCommand {
            adapter_id: "test",
            resource_root: root.clone(),
            program: program.clone(),
            args: vec![PathBuf::from("/literal-adapter-value")],
            resource_files: vec![ResolvedResourceFile {
                path: program,
                identity: ResolvedFileIdentity {
                    size: 12,
                    sha256: format!("{:x}", Sha256::digest(b"server bytes")),
                },
            }],
        };

        assert!(super::revalidate_server_command(&command, &root).is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn initialize_timeout_returns_a_typed_failure() {
        let (launcher, _observed) = MockServerLauncher::scripted(ServerScript::HangingInitialize);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        assert!(matches!(
            LspSession::start(test_descriptor(), test_command(), root(), launcher, sink,).await,
            Err(SessionError::Timeout("initialize"))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn startup_cancellation_kills_and_reaps_before_returning_cancelled() {
        let (launcher, observed) =
            MockServerLauncher::scripted(ServerScript::HangingInitializeDelayedReap);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        let cancellation = tokio_util::sync::CancellationToken::new();
        let start = tokio::spawn({
            let cancellation = cancellation.clone();
            async move {
                LspSession::start_with_cancellation(
                    test_descriptor(),
                    test_command(),
                    root(),
                    launcher,
                    sink,
                    cancellation,
                )
                .await
            }
        });
        tokio::task::yield_now().await;
        cancellation.cancel();

        wait_for_startup_kill(&observed).await;
        assert!(
            !start.is_finished(),
            "cancellation acknowledgement is not process reap"
        );
        tokio::time::advance(Duration::from_millis(999)).await;
        tokio::task::yield_now().await;
        assert!(
            !start.is_finished(),
            "cancelled startup returned before the exit fact"
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert!(matches!(start.await.unwrap(), Err(SessionError::Cancelled)));
    }

    #[tokio::test(start_paused = true)]
    async fn initialize_request_failure_kills_and_waits_for_reap_before_start_returns() {
        let (launcher, observed) =
            MockServerLauncher::scripted(ServerScript::InitializeFailureDelayedReap);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        let cancellation = tokio_util::sync::CancellationToken::new();
        let start = tokio::spawn({
            let cancellation = cancellation.clone();
            async move {
                LspSession::start_with_cancellation(
                    test_descriptor(),
                    test_command(),
                    root(),
                    launcher,
                    sink,
                    cancellation,
                )
                .await
            }
        });

        wait_for_startup_kill(&observed).await;
        cancellation.cancel();
        assert!(
            !start.is_finished(),
            "kill acknowledgement is not process reap"
        );
        tokio::time::advance(Duration::from_millis(999)).await;
        tokio::task::yield_now().await;
        assert!(
            !start.is_finished(),
            "startup returned before the exit fact"
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert!(matches!(
            start.await.unwrap(),
            Err(SessionError::Protocol(_))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn initialize_timeout_kills_and_waits_for_reap_before_start_returns() {
        let (launcher, observed) =
            MockServerLauncher::scripted(ServerScript::HangingInitializeDelayedReap);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        let cancellation = tokio_util::sync::CancellationToken::new();
        let start = tokio::spawn({
            let cancellation = cancellation.clone();
            async move {
                LspSession::start_with_cancellation(
                    test_descriptor(),
                    test_command(),
                    root(),
                    launcher,
                    sink,
                    cancellation,
                )
                .await
            }
        });
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(60)).await;

        wait_for_startup_kill(&observed).await;
        cancellation.cancel();
        assert!(
            !start.is_finished(),
            "kill acknowledgement is not process reap"
        );
        tokio::time::advance(Duration::from_millis(999)).await;
        tokio::task::yield_now().await;
        assert!(
            !start.is_finished(),
            "startup returned before the exit fact"
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert!(matches!(
            start.await.unwrap(),
            Err(SessionError::Timeout("initialize"))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn initialized_notification_failure_kills_and_waits_for_reap_before_start_returns() {
        struct RejectInitialized;

        impl super::InitializedNotifier for RejectInitialized {
            fn notify(&self, _server: &async_lsp::ServerSocket) -> Result<(), SessionError> {
                Err(SessionError::Protocol(
                    "initialized notification rejected".into(),
                ))
            }
        }

        let (launcher, observed) =
            MockServerLauncher::scripted(ServerScript::InitializedNotificationFailureDelayedReap);
        let (sink, _events) = tokio::sync::mpsc::channel(16);
        let cancellation = tokio_util::sync::CancellationToken::new();
        let start = tokio::spawn({
            let cancellation = cancellation.clone();
            async move {
                LspSession::start_with_initialized_notifier_and_cancellation(
                    test_descriptor(),
                    test_command(),
                    root(),
                    launcher,
                    sink,
                    RejectInitialized,
                    cancellation,
                )
                .await
            }
        });

        wait_for_startup_kill(&observed).await;
        cancellation.cancel();
        assert!(
            !start.is_finished(),
            "kill acknowledgement is not process reap"
        );
        tokio::time::advance(Duration::from_millis(999)).await;
        tokio::task::yield_now().await;
        assert!(
            !start.is_finished(),
            "startup returned before the exit fact"
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert!(matches!(
            start.await.unwrap(),
            Err(SessionError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn process_exit_status_is_reported_without_panicking_the_session() {
        let (launcher, _observed) = MockServerLauncher::scripted(ServerScript::ExitAfterInitialize);
        let (sink, mut events) = tokio::sync::mpsc::channel(16);
        let _session = LspSession::start(test_descriptor(), test_command(), root(), launcher, sink)
            .await
            .unwrap();
        let exit = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(ClientEvent::ProcessExited { success, code }) = events.recv().await {
                    break (success, code);
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(exit, (true, Some(0)));
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn launcher_rejects_symlinks_and_paths_outside_the_catalog_root() {
        let directory = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let real = directory.path().join("real-server");
        fs::write(&real, b"server bytes").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();
        let link = directory.path().join("server");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let root = directory.path().canonicalize().unwrap();
        let identity = ResolvedFileIdentity {
            size: 12,
            sha256: format!("{:x}", Sha256::digest(b"server bytes")),
        };
        let symlink_command = ResolvedServerCommand {
            adapter_id: "test",
            resource_root: root.clone(),
            program: link.clone(),
            args: Vec::new(),
            resource_files: vec![ResolvedResourceFile {
                path: link,
                identity: identity.clone(),
            }],
        };
        assert!(matches!(
            ProcessServerLauncher
                .launch(symlink_command, root.clone())
                .await,
            Err(SessionError::Integrity(_))
        ));

        let escaped = outside.path().join("server");
        fs::write(&escaped, b"server bytes").unwrap();
        fs::set_permissions(&escaped, fs::Permissions::from_mode(0o755)).unwrap();
        let escaped = escaped.canonicalize().unwrap();
        let escaped_command = ResolvedServerCommand {
            adapter_id: "test",
            resource_root: root.clone(),
            program: escaped.clone(),
            args: Vec::new(),
            resource_files: vec![ResolvedResourceFile {
                path: escaped,
                identity,
            }],
        };
        assert!(matches!(
            ProcessServerLauncher.launch(escaped_command, root).await,
            Err(SessionError::Integrity(_))
        ));
    }

    #[test]
    fn production_command_uses_an_explicit_minimal_environment() {
        let command = ResolvedServerCommand {
            adapter_id: "test",
            resource_root: root(),
            program: PathBuf::from("/absolute/bundled/server"),
            args: vec!["--stdio".into()],
            resource_files: Vec::new(),
        };
        let process = super::sanitized_command(&command, &root());
        assert_eq!(process.as_std().get_program(), "/absolute/bundled/server");
        let environment = process
            .as_std()
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            environment.get(std::ffi::OsStr::new("PATH")),
            Some(&Some(std::env::var_os("PATH").unwrap_or_else(|| {
                std::ffi::OsString::from("/usr/bin:/bin:/usr/sbin:/sbin")
            })))
        );
        for dangerous in [
            "NODE_OPTIONS",
            "NODE_PATH",
            "DYLD_LIBRARY_PATH",
            "DYLD_INSERT_LIBRARIES",
            "LD_LIBRARY_PATH",
            "RUSTUP_HOME",
            "CARGO_HOME",
            "PYTHONPATH",
            "CLASSPATH",
            "JAVA_TOOL_OPTIONS",
        ] {
            assert!(!environment.contains_key(std::ffi::OsStr::new(dangerous)));
        }
    }
}
