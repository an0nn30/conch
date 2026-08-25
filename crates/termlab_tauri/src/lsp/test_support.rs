use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, watch};

use super::catalog::{AdapterDescriptor, BundledServerCatalog, ResolvedServerCommand};
use super::root::LanguageId;
use super::session::{LaunchedServer, ProcessExit, ProcessHandle, ServerLauncher, SessionError};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerScript {
    FullFeature,
    FullSync,
    NoSync,
    OpenCloseFalse,
    DynamicSync,
    NonFileDiagnostics,
    CompletionGenerations,
    EventFlood,
    ConfigurationSections,
    HangingCompletion,
    IgnoresExit,
    DelayedKillAfterExit,
    DelayedKillAfterShutdownTimeout,
    MalformedCompletion,
    HangingInitialize,
    InitializeFailureDelayedReap,
    HangingInitializeDelayedReap,
    InitializedNotificationFailureDelayedReap,
    ExitAfterInitialize,
    HangingShutdown,
    StderrFlood,
}

pub(crate) fn full_feature_script() -> ServerScript {
    ServerScript::FullFeature
}

pub(crate) fn root() -> PathBuf {
    std::env::temp_dir()
        .canonicalize()
        .expect("canonical temp root")
}

pub(crate) fn test_descriptor() -> &'static AdapterDescriptor {
    BundledServerCatalog::new().descriptor(LanguageId::TypeScript)
}

pub(crate) fn test_command() -> ResolvedServerCommand {
    ResolvedServerCommand {
        adapter_id: "typescript",
        resource_root: root(),
        program: PathBuf::from("/bin/true"),
        args: Vec::new(),
        resource_files: Vec::new(),
    }
}

#[derive(Clone)]
pub(crate) struct ObservedProtocol {
    methods: Arc<Mutex<Vec<String>>>,
    messages: Arc<Mutex<Vec<Value>>>,
    client_responses: Arc<Mutex<Vec<Value>>>,
    kill_observed: Arc<AtomicBool>,
}

impl ObservedProtocol {
    pub(crate) fn assert_order(&self, expected: &[&str]) {
        let methods = self.methods.lock().expect("observed protocol lock");
        let positions = expected
            .iter()
            .map(|expected| {
                methods
                    .iter()
                    .position(|actual| actual == expected)
                    .unwrap_or_else(|| panic!("missing {expected}; observed {methods:?}"))
            })
            .collect::<Vec<_>>();
        assert!(
            positions.windows(2).all(|pair| pair[0] < pair[1]),
            "expected {expected:?} in order; observed {methods:?}"
        );
    }

    pub(crate) fn did_change_content_changes(&self) -> Option<Vec<Value>> {
        self.messages
            .lock()
            .expect("observed messages lock")
            .iter()
            .find(|message| message["method"] == "textDocument/didChange")
            .and_then(|message| message["params"]["contentChanges"].as_array().cloned())
    }

    pub(crate) fn assert_incremental_changes_descend(&self) {
        assert_eq!(
            self.did_change_content_changes(),
            Some(vec![
                json!({
                    "range": {
                        "start": { "line": 0, "character": 2 },
                        "end": { "line": 0, "character": 3 }
                    },
                    "text": "N"
                }),
                json!({
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 0, "character": 1 }
                    },
                    "text": "C"
                }),
            ])
        );
    }

    pub(crate) fn assert_saved_text(&self, expected: &str) {
        let messages = self.messages.lock().expect("observed messages lock");
        let save = messages
            .iter()
            .find(|message| message["method"] == "textDocument/didSave")
            .unwrap_or_else(|| panic!("missing didSave; observed {messages:?}"));
        assert_eq!(save["params"]["text"], expected);
    }

    pub(crate) fn assert_configuration_and_unsupported_request_responses(&self) {
        let responses = self.client_responses.lock().expect("client responses lock");
        let by_id = |id| {
            responses
                .iter()
                .find(|response| response["id"] == id)
                .unwrap_or_else(|| panic!("missing response {id}; observed {responses:?}"))
        };
        assert_eq!(by_id(100)["result"], json!([{}]));
        assert_eq!(by_id(101)["result"], Value::Null);
        assert_eq!(by_id(102)["result"], Value::Null);
        assert_eq!(by_id(103)["result"], Value::Null);
        assert_eq!(by_id(104)["error"]["code"], -32601);
        assert_eq!(by_id(105)["error"]["code"], -32602);
    }

    pub(crate) fn assert_cancelled_request_id(&self, request_id: i32) {
        let messages = self.messages.lock().expect("observed messages lock");
        assert!(
            messages.iter().any(|message| {
                message["method"] == "$/cancelRequest" && message["params"]["id"] == request_id
            }),
            "missing cancellation for {request_id}; observed {messages:?}"
        );
    }

    pub(crate) fn method_count(&self, method: &str) -> usize {
        self.methods
            .lock()
            .expect("observed protocol lock")
            .iter()
            .filter(|observed| observed.as_str() == method)
            .count()
    }

    pub(crate) fn has_client_response(&self, id: i64) -> bool {
        self.client_responses
            .lock()
            .expect("client responses lock")
            .iter()
            .any(|response| response["id"] == id)
    }

    pub(crate) fn assert_configuration_sections(&self) {
        let responses = self.client_responses.lock().expect("client responses lock");
        let response = responses
            .iter()
            .find(|response| response["id"] == 130)
            .unwrap_or_else(|| panic!("missing configuration response; observed {responses:?}"));
        assert_eq!(
            response["result"],
            json!([
                {
                    "typescript": {
                        "preferences": { "quoteStyle": "single" }
                    },
                    "javascript": { "format": true }
                },
                { "quoteStyle": "single" },
                null,
                { "format": true }
            ])
        );
    }

    pub(crate) fn kill_observed(&self) -> bool {
        self.kill_observed.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
pub(crate) struct MockServerLauncher {
    script: ServerScript,
    observed: ObservedProtocol,
}

impl MockServerLauncher {
    pub(crate) fn scripted(script: ServerScript) -> (Self, ObservedProtocol) {
        let observed = ObservedProtocol {
            methods: Arc::new(Mutex::new(Vec::new())),
            messages: Arc::new(Mutex::new(Vec::new())),
            client_responses: Arc::new(Mutex::new(Vec::new())),
            kill_observed: Arc::new(AtomicBool::new(false)),
        };
        (
            Self {
                script,
                observed: observed.clone(),
            },
            observed,
        )
    }
}

#[async_trait]
impl ServerLauncher for MockServerLauncher {
    async fn launch(
        &self,
        _command: ResolvedServerCommand,
        _canonical_project_root: PathBuf,
    ) -> Result<LaunchedServer, SessionError> {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (stderr_read, mut stderr_write) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client);
        let (server_read, server_write) = tokio::io::split(server);
        let (kill_tx, kill_rx) = oneshot::channel();
        let (exit_tx, exit_rx) = watch::channel(None);
        let script = self.script;
        if script == ServerScript::StderrFlood {
            tokio::spawn(async move {
                let source_like_secret = vec![b's'; 256 * 1024];
                let _ = stderr_write.write_all(&source_like_secret).await;
                let _ = stderr_write.shutdown().await;
            });
        } else {
            drop(stderr_write);
        }
        let observed = self.observed.clone();
        let lifecycle_observed = observed.clone();
        tokio::spawn(async move {
            let success = tokio::select! {
                result = run_script(script, server_read, server_write, observed) => result.is_ok(),
                _ = kill_rx => {
                    lifecycle_observed.kill_observed.store(true, Ordering::SeqCst);
                    if matches!(
                        script,
                        ServerScript::DelayedKillAfterExit
                            | ServerScript::DelayedKillAfterShutdownTimeout
                            | ServerScript::InitializeFailureDelayedReap
                            | ServerScript::HangingInitializeDelayedReap
                            | ServerScript::InitializedNotificationFailureDelayedReap
                    ) {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    false
                },
            };
            let _ = exit_tx.send(Some(ProcessExit {
                success,
                code: Some(if success { 0 } else { 1 }),
            }));
        });
        Ok(LaunchedServer {
            stdout: Box::new(client_read),
            stderr: Box::new(stderr_read),
            stdin: Box::new(client_write),
            process: ProcessHandle::new(kill_tx, exit_rx),
        })
    }
}

async fn run_script(
    script: ServerScript,
    reader: impl AsyncRead + Unpin,
    mut writer: impl AsyncWrite + Unpin,
    observed: ObservedProtocol,
) -> Result<(), String> {
    let mut reader = BufReader::new(reader);
    let mut completion_generation = 0_u32;
    let mut dynamic_unregistered = false;
    while let Some(message) = read_message(&mut reader).await? {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            observed
                .client_responses
                .lock()
                .expect("client responses lock")
                .push(message);
            continue;
        };
        observed
            .methods
            .lock()
            .expect("observed methods lock")
            .push(method.to_owned());
        observed
            .messages
            .lock()
            .expect("observed messages lock")
            .push(message.clone());
        let id = message.get("id").cloned();
        match (script, method) {
            (
                ServerScript::HangingInitialize | ServerScript::HangingInitializeDelayedReap,
                "initialize",
            ) => {}
            (ServerScript::InitializeFailureDelayedReap, "initialize") => {
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id.unwrap(),
                        "error": { "code": -32002, "message": "initialize rejected" }
                    }),
                )
                .await?;
            }
            (_, "initialize") => {
                let capabilities = &message["params"]["capabilities"];
                assert_eq!(capabilities["workspace"], json!({ "configuration": true }));
                assert_eq!(capabilities["window"], json!({ "workDoneProgress": true }));
                assert_eq!(
                    capabilities["general"],
                    json!({
                        "positionEncodings": ["utf-16"],
                        "staleRequestSupport": {
                            "cancel": true,
                            "retryOnContentModified": []
                        }
                    })
                );
                let text_document = capabilities["textDocument"].as_object().unwrap();
                assert_eq!(
                    text_document
                        .keys()
                        .cloned()
                        .collect::<std::collections::BTreeSet<_>>(),
                    [
                        "completion",
                        "definition",
                        "diagnostic",
                        "hover",
                        "publishDiagnostics",
                        "signatureHelp",
                        "synchronization",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
                );
                assert_eq!(
                    capabilities["textDocument"]["completion"]["completionItem"]["snippetSupport"],
                    true
                );
                assert_eq!(
                    capabilities["textDocument"]["definition"]["linkSupport"],
                    true
                );
                for feature in [
                    "completion",
                    "hover",
                    "signatureHelp",
                    "definition",
                    "diagnostic",
                ] {
                    assert!(
                        capabilities["textDocument"][feature]
                            .get("dynamicRegistration")
                            .is_none()
                    );
                }
                let text_document_sync = match script {
                    ServerScript::FullSync => Some(json!({
                        "openClose": true,
                        "change": 1
                    })),
                    ServerScript::NoSync | ServerScript::DynamicSync => None,
                    ServerScript::OpenCloseFalse => Some(json!({
                        "openClose": false,
                        "change": 2
                    })),
                    _ => Some(json!({
                        "openClose": true,
                        "change": 2,
                        "save": { "includeText": true }
                    })),
                };
                let mut server_capabilities = json!({
                    "completionProvider": { "resolveProvider": true },
                    "hoverProvider": true,
                    "signatureHelpProvider": { "triggerCharacters": ["(", ","] },
                    "definitionProvider": true,
                    "diagnosticProvider": {
                        "interFileDependencies": true,
                        "workspaceDiagnostics": false
                    }
                });
                if let Some(sync) = text_document_sync {
                    server_capabilities["textDocumentSync"] = sync;
                }
                write_response(
                    &mut writer,
                    id.unwrap(),
                    json!({
                        "capabilities": server_capabilities
                    }),
                )
                .await?;
            }
            (ServerScript::FullFeature, "initialized") => {
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": 100,
                        "method": "workspace/configuration",
                        "params": { "items": [{ "section": "typescript" }] }
                    }),
                )
                .await?;
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": 101,
                        "method": "window/workDoneProgress/create",
                        "params": { "token": "index" }
                    }),
                )
                .await?;
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": 102,
                        "method": "client/registerCapability",
                        "params": { "registrations": [{
                            "id": "save-1",
                            "method": "textDocument/didSave",
                            "registerOptions": {
                                "documentSelector": null,
                                "includeText": true
                            }
                        }] }
                    }),
                )
                .await?;
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": 103,
                        "method": "client/unregisterCapability",
                        "params": { "unregisterations": [{
                            "id": "save-1",
                            "method": "textDocument/didSave"
                        }] }
                    }),
                )
                .await?;
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": 105,
                        "method": "client/registerCapability",
                        "params": { "registrations": [{
                            "id": "watch-unsupported",
                            "method": "workspace/didChangeWatchedFiles",
                            "registerOptions": {}
                        }] }
                    }),
                )
                .await?;
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": 104,
                        "method": "workspace/applyEdit",
                        "params": { "edit": { "changes": {} } }
                    }),
                )
                .await?;
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "window/logMessage",
                        "params": { "type": 3, "message": "mock initialized" }
                    }),
                )
                .await?;
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "window/showMessage",
                        "params": { "type": 2, "message": "mock warning" }
                    }),
                )
                .await?;
            }
            (ServerScript::DynamicSync, "initialized") => {
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": 120,
                        "method": "client/registerCapability",
                        "params": { "registrations": [
                            {
                                "id": "open-dynamic",
                                "method": "textDocument/didOpen",
                                "registerOptions": { "documentSelector": null }
                            },
                            {
                                "id": "change-dynamic",
                                "method": "textDocument/didChange",
                                "registerOptions": {
                                    "documentSelector": null,
                                    "syncKind": 2
                                }
                            },
                            {
                                "id": "close-dynamic",
                                "method": "textDocument/didClose",
                                "registerOptions": { "documentSelector": null }
                            }
                        ] }
                    }),
                )
                .await?;
            }
            (ServerScript::ConfigurationSections, "initialized") => {
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": 130,
                        "method": "workspace/configuration",
                        "params": { "items": [
                            {},
                            { "section": "typescript.preferences" },
                            { "section": "missing.section" },
                            { "section": "javascript" }
                        ] }
                    }),
                )
                .await?;
            }
            (ServerScript::EventFlood, "initialized") => {
                for request in [
                    json!({
                        "jsonrpc": "2.0", "id": 140,
                        "method": "client/registerCapability",
                        "params": { "registrations": [{
                            "id": "save-a", "method": "textDocument/didSave",
                            "registerOptions": { "documentSelector": null }
                        }] }
                    }),
                    json!({
                        "jsonrpc": "2.0", "id": 141,
                        "method": "client/unregisterCapability",
                        "params": { "unregisterations": [{
                            "id": "save-a", "method": "textDocument/didSave"
                        }] }
                    }),
                    json!({
                        "jsonrpc": "2.0", "id": 142,
                        "method": "client/registerCapability",
                        "params": { "registrations": [{
                            "id": "save-b", "method": "textDocument/didSave",
                            "registerOptions": { "documentSelector": null }
                        }] }
                    }),
                ] {
                    write_message(&mut writer, &request).await?;
                }
            }
            (ServerScript::ExitAfterInitialize, "initialized") => return Ok(()),
            (ServerScript::NonFileDiagnostics, "textDocument/didOpen") => {
                for (uri, version, message_text) in [
                    ("untitled:buffer", 1, "untitled"),
                    ("https://example.invalid/main.ts", 2, "https"),
                    ("vscode-remote://ssh-remote+host/main.ts", 3, "remote"),
                    (
                        message["params"]["textDocument"]["uri"].as_str().unwrap(),
                        4,
                        "local latest",
                    ),
                ] {
                    write_message(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "method": "textDocument/publishDiagnostics",
                            "params": {
                                "uri": uri,
                                "version": version,
                                "diagnostics": [{
                                    "range": {
                                        "start": { "line": 0, "character": 0 },
                                        "end": { "line": 0, "character": 1 }
                                    },
                                    "message": message_text
                                }]
                            }
                        }),
                    )
                    .await?;
                }
            }
            (ServerScript::EventFlood, "textDocument/didOpen") => {
                let uri = &message["params"]["textDocument"]["uri"];
                for version in 1..=3 {
                    write_message(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "method": "textDocument/publishDiagnostics",
                            "params": {
                                "uri": uri,
                                "version": version,
                                "diagnostics": [{
                                    "range": {
                                        "start": { "line": 0, "character": 0 },
                                        "end": { "line": 0, "character": 1 }
                                    },
                                    "message": format!("diagnostic-{version}")
                                }]
                            }
                        }),
                    )
                    .await?;
                }
                for index in 0..48 {
                    write_message(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "method": "window/logMessage",
                            "params": { "type": 3, "message": format!("flood-{index}") }
                        }),
                    )
                    .await?;
                }
                for value in [
                    json!({ "kind": "begin", "title": "Index" }),
                    json!({ "kind": "report", "message": "latest report", "percentage": 99 }),
                    json!({ "kind": "end", "message": "complete" }),
                ] {
                    write_message(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "method": "$/progress",
                            "params": { "token": "flood", "value": value }
                        }),
                    )
                    .await?;
                }
            }
            (_, "textDocument/didOpen") => {
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/publishDiagnostics",
                        "params": {
                            "uri": message["params"]["textDocument"]["uri"],
                            "version": 1,
                            "diagnostics": [{
                                "range": {
                                    "start": { "line": 0, "character": 0 },
                                    "end": { "line": 0, "character": 3 }
                                },
                                "severity": 2,
                                "code": "demo",
                                "source": "mock",
                                "message": "demonstration warning"
                            }]
                        }
                    }),
                )
                .await?;
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "method": "$/progress",
                        "params": {
                            "token": "index",
                            "value": { "kind": "end", "message": "ready" }
                        }
                    }),
                )
                .await?;
            }
            (ServerScript::HangingCompletion, "textDocument/completion") => {}
            (ServerScript::MalformedCompletion, "textDocument/completion") => {
                write_response(&mut writer, id.unwrap(), json!({ "items": "invalid" })).await?;
            }
            (ServerScript::CompletionGenerations, "textDocument/completion") => {
                completion_generation += 1;
                write_response(
                    &mut writer,
                    id.unwrap(),
                    json!({
                        "isIncomplete": false,
                        "items": [{
                            "label": format!("generation-{completion_generation}"),
                            "data": { "generation": completion_generation }
                        }]
                    }),
                )
                .await?;
            }
            (_, "textDocument/completion") => {
                write_response(
                    &mut writer,
                    id.unwrap(),
                    json!({
                        "isIncomplete": false,
                        "items": [{
                            "label": "console",
                            "kind": 6,
                            "detail": "global console",
                            "insertText": "console",
                            "insertTextFormat": 1,
                            "commitCharacters": ["."],
                            "data": { "resolve": 1 }
                        }]
                    }),
                )
                .await?;
            }
            (_, "completionItem/resolve") => {
                let mut item = message["params"].clone();
                item["documentation"] = if script == ServerScript::CompletionGenerations {
                    json!({
                        "kind": "markdown",
                        "value": format!("resolved generation {}", item["data"]["generation"])
                    })
                } else {
                    json!({
                        "kind": "markdown",
                        "value": "Resolved **console**"
                    })
                };
                write_response(&mut writer, id.unwrap(), item).await?;
            }
            (_, "textDocument/hover") => {
                write_response(
                    &mut writer,
                    id.unwrap(),
                    json!({
                        "contents": { "kind": "markdown", "value": "`console`" },
                        "range": {
                            "start": { "line": 0, "character": 0 },
                            "end": { "line": 0, "character": 3 }
                        }
                    }),
                )
                .await?;
            }
            (_, "textDocument/signatureHelp") => {
                write_response(
                    &mut writer,
                    id.unwrap(),
                    json!({
                        "signatures": [{
                            "label": "log(value: unknown)",
                            "parameters": [{ "label": [4, 18] }],
                            "activeParameter": 0
                        }],
                        "activeSignature": 0,
                        "activeParameter": 0
                    }),
                )
                .await?;
            }
            (_, "textDocument/definition") => {
                write_response(
                    &mut writer,
                    id.unwrap(),
                    json!([{
                        "uri": message["params"]["textDocument"]["uri"],
                        "range": {
                            "start": { "line": 0, "character": 0 },
                            "end": { "line": 0, "character": 3 }
                        }
                    }]),
                )
                .await?;
            }
            (_, "textDocument/diagnostic") => {
                write_response(
                    &mut writer,
                    id.unwrap(),
                    json!({
                        "kind": "full",
                        "resultId": "diagnostics-1",
                        "items": [{
                            "range": {
                                "start": { "line": 0, "character": 0 },
                                "end": { "line": 0, "character": 3 }
                            },
                            "severity": 1,
                            "message": "pull error"
                        }]
                    }),
                )
                .await?;
            }
            (
                ServerScript::HangingShutdown | ServerScript::DelayedKillAfterShutdownTimeout,
                "shutdown",
            ) => {}
            (ServerScript::DynamicSync, "textDocument/didChange") if !dynamic_unregistered => {
                dynamic_unregistered = true;
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": 121,
                        "method": "client/unregisterCapability",
                        "params": { "unregisterations": [
                            { "id": "open-dynamic", "method": "textDocument/didOpen" },
                            { "id": "change-dynamic", "method": "textDocument/didChange" },
                            { "id": "close-dynamic", "method": "textDocument/didClose" }
                        ] }
                    }),
                )
                .await?;
            }
            (_, "shutdown") => write_response(&mut writer, id.unwrap(), Value::Null).await?,
            (ServerScript::IgnoresExit | ServerScript::DelayedKillAfterExit, "exit") => {}
            (_, "exit") => return Ok(()),
            _ => {}
        }
    }
    Err("client transport closed before exit".into())
}

async fn read_message(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
) -> Result<Option<Value>, String> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(None);
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| error.to_string())?,
            );
        }
    }
    let length = content_length.ok_or_else(|| "missing Content-Length".to_owned())?;
    let mut body = vec![0_u8; length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| error.to_string())
}

async fn write_response(
    writer: &mut (impl AsyncWrite + Unpin),
    id: Value,
    result: Value,
) -> Result<(), String> {
    write_message(
        writer,
        &json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    )
    .await
}

async fn write_message(
    writer: &mut (impl AsyncWrite + Unpin),
    message: &Value,
) -> Result<(), String> {
    let body = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&body)
        .await
        .map_err(|error| error.to_string())?;
    writer.flush().await.map_err(|error| error.to_string())
}
