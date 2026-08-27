use std::collections::VecDeque;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};

use async_lsp::LanguageClient;
use async_lsp::lsp_types as lsp;
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::Notify;
use tokio::sync::mpsc;

use super::types::{
    CompletionItem, CompletionResponse, CompletionTextEdit, CompletionUnsupportedEffect,
    DefinitionResponse, Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity,
    EditorLocation, EditorPosition, EditorRange, EditorTextEdit, HoverBlock, HoverResponse,
    LspCapabilities, NegotiatedTriggers, SignatureHelpResponse, SignatureInformation,
    SignatureParameter,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProgressPayload {
    Begin {
        title: String,
        cancellable: Option<bool>,
        message: Option<String>,
        percentage: Option<u32>,
    },
    Report {
        cancellable: Option<bool>,
        message: Option<String>,
        percentage: Option<u32>,
    },
    End {
        message: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ClientEvent {
    Diagnostics {
        uri: String,
        version: Option<i32>,
        diagnostics: Vec<Diagnostic>,
    },
    Message {
        kind: String,
        message: String,
    },
    Progress {
        token: String,
        progress: ProgressPayload,
    },
    RegistrationsChanged(Vec<DynamicRegistration>),
    CapabilitiesChanged(LspCapabilities),
    Overflow {
        dropped: u64,
    },
    ProtocolExited(Option<String>),
    ProcessExited {
        success: bool,
        code: Option<i32>,
    },
    /// A bounded stderr chunk was drained. Raw bytes deliberately never cross
    /// this boundary, preventing server output from leaking document source.
    Stderr {
        bytes: u32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DynamicRegistration {
    pub id: String,
    pub method: String,
    pub options: DynamicRegistrationOptions,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DynamicRegistrationOptions {
    DidOpen(lsp::TextDocumentRegistrationOptions),
    DidClose(lsp::TextDocumentRegistrationOptions),
    DidChange(lsp::TextDocumentChangeRegistrationOptions),
    DidSave(lsp::TextDocumentSaveRegistrationOptions),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SyncPolicy {
    pub change: lsp::TextDocumentSyncKind,
    pub open: bool,
    pub close: bool,
    pub save: bool,
    pub save_include_text: bool,
}

impl Default for SyncPolicy {
    fn default() -> Self {
        Self {
            change: lsp::TextDocumentSyncKind::NONE,
            open: false,
            close: false,
            save: false,
            save_include_text: false,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SessionCapabilityState {
    static_sync: SyncPolicy,
    features: LspCapabilities,
    /// Set once, from the initialize result. Dynamic (un)registration changes
    /// which features exist, never which characters open them, so the
    /// `CapabilitiesChanged` path deliberately leaves these alone.
    triggers: NegotiatedTriggers,
    registrations: Vec<DynamicRegistration>,
}

impl Default for SessionCapabilityState {
    fn default() -> Self {
        Self {
            static_sync: SyncPolicy::default(),
            features: LspCapabilities {
                completion: false,
                hover: false,
                signature_help: false,
                definition: false,
                diagnostics: false,
            },
            triggers: NegotiatedTriggers::default(),
            registrations: Vec::new(),
        }
    }
}

impl SessionCapabilityState {
    pub(crate) fn set_static_sync(&mut self, sync: SyncPolicy) {
        self.static_sync = sync;
    }

    pub(crate) fn set_features(&mut self, features: LspCapabilities) {
        self.features = features;
    }

    pub(crate) fn features(&self) -> LspCapabilities {
        self.features
    }

    pub(crate) fn set_triggers(&mut self, triggers: NegotiatedTriggers) {
        self.triggers = triggers;
    }

    pub(crate) fn triggers(&self) -> NegotiatedTriggers {
        self.triggers.clone()
    }

    pub(crate) fn sync_policy(&self) -> SyncPolicy {
        let mut sync = self.static_sync;
        for registration in &self.registrations {
            match &registration.options {
                DynamicRegistrationOptions::DidOpen(_) => sync.open = true,
                DynamicRegistrationOptions::DidClose(_) => sync.close = true,
                DynamicRegistrationOptions::DidChange(options) => {
                    sync.change = if options.sync_kind == 1 {
                        lsp::TextDocumentSyncKind::FULL
                    } else {
                        lsp::TextDocumentSyncKind::INCREMENTAL
                    };
                }
                DynamicRegistrationOptions::DidSave(options) => {
                    sync.save = true;
                    sync.save_include_text = options.include_text.unwrap_or(false);
                }
            }
        }
        sync
    }

    fn registrations(&self) -> Vec<DynamicRegistration> {
        self.registrations.clone()
    }
}

const EVENT_MAILBOX_CAPACITY: usize = 32;

#[derive(Clone)]
pub(crate) struct EventSink {
    mailbox: Arc<EventMailbox>,
}

struct EventMailbox {
    pending: Mutex<PendingEvents>,
    ready: Notify,
}

#[derive(Default)]
struct PendingEvents {
    queue: VecDeque<ClientEvent>,
    dropped: u64,
}

impl EventSink {
    pub(crate) fn new(target: mpsc::Sender<ClientEvent>) -> Self {
        let mailbox = Arc::new(EventMailbox {
            pending: Mutex::new(PendingEvents::default()),
            ready: Notify::new(),
        });
        let forwarder = mailbox.clone();
        tokio::spawn(async move {
            loop {
                forwarder.ready.notified().await;
                loop {
                    let event = forwarder.pop();
                    let Some(event) = event else { break };
                    if target.send(event).await.is_err() {
                        return;
                    }
                }
            }
        });
        Self { mailbox }
    }

    pub(crate) fn send(&self, event: ClientEvent) {
        self.mailbox.push(event);
    }
}

impl EventMailbox {
    fn push(&self, event: ClientEvent) {
        let mut pending = self.pending.lock().expect("LSP event mailbox poisoned");
        if let Some(existing) = pending
            .queue
            .iter_mut()
            .find(|existing| same_state_key(existing, &event))
        {
            *existing = event;
            self.ready.notify_one();
            return;
        }
        if pending.queue.len() == EVENT_MAILBOX_CAPACITY {
            pending.dropped = pending.dropped.saturating_add(1);
            let incoming_priority = event_priority(&event);
            let removable = pending
                .queue
                .iter()
                .position(|queued| match incoming_priority {
                    EventPriority::Terminal => event_priority(queued) != EventPriority::Terminal,
                    EventPriority::State => event_priority(queued) == EventPriority::Ordinary,
                    EventPriority::Ordinary => false,
                });
            let Some(remove) = removable else {
                self.ready.notify_one();
                return;
            };
            pending.queue.remove(remove);
        }
        pending.queue.push_back(event);
        drop(pending);
        self.ready.notify_one();
    }

    fn pop(&self) -> Option<ClientEvent> {
        let mut pending = self.pending.lock().expect("LSP event mailbox poisoned");
        if pending.dropped > 0 {
            let dropped = std::mem::take(&mut pending.dropped);
            return Some(ClientEvent::Overflow { dropped });
        }
        pending.queue.pop_front()
    }
}

fn same_state_key(left: &ClientEvent, right: &ClientEvent) -> bool {
    match (left, right) {
        (
            ClientEvent::Diagnostics { uri: left, .. },
            ClientEvent::Diagnostics { uri: right, .. },
        ) => left == right,
        (ClientEvent::RegistrationsChanged(_), ClientEvent::RegistrationsChanged(_)) => true,
        (ClientEvent::CapabilitiesChanged(_), ClientEvent::CapabilitiesChanged(_)) => true,
        (ClientEvent::Progress { token: left, .. }, ClientEvent::Progress { token: right, .. }) => {
            left == right
        }
        (ClientEvent::ProtocolExited(_), ClientEvent::ProtocolExited(_)) => true,
        (ClientEvent::ProcessExited { .. }, ClientEvent::ProcessExited { .. }) => true,
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EventPriority {
    Ordinary,
    State,
    Terminal,
}

fn event_priority(event: &ClientEvent) -> EventPriority {
    match event {
        ClientEvent::ProtocolExited(_) | ClientEvent::ProcessExited { .. } => {
            EventPriority::Terminal
        }
        ClientEvent::Diagnostics { .. }
        | ClientEvent::RegistrationsChanged(_)
        | ClientEvent::CapabilitiesChanged(_)
        | ClientEvent::Progress { .. } => EventPriority::State,
        ClientEvent::Message { .. } | ClientEvent::Overflow { .. } | ClientEvent::Stderr { .. } => {
            EventPriority::Ordinary
        }
    }
}

pub(crate) struct ClientState {
    events: EventSink,
    workspace_configuration: Value,
    capabilities: Arc<Mutex<SessionCapabilityState>>,
}

impl ClientState {
    pub(crate) fn new(
        events: EventSink,
        workspace_configuration: Value,
        capabilities: Arc<Mutex<SessionCapabilityState>>,
    ) -> Self {
        Self {
            events,
            workspace_configuration,
            capabilities,
        }
    }

    fn send(&self, event: ClientEvent) {
        self.events.send(event);
    }
}

impl LanguageClient for ClientState {
    type Error = async_lsp::ResponseError;
    type NotifyResult = ControlFlow<async_lsp::Result<()>>;

    fn configuration(
        &mut self,
        params: lsp::ConfigurationParams,
    ) -> BoxFuture<'static, Result<Vec<Value>, Self::Error>> {
        let configuration = self.workspace_configuration.clone();
        let sections = params
            .items
            .iter()
            .map(|item| item.section.as_deref())
            .collect::<Vec<_>>();
        let values = resolve_workspace_configuration(&configuration, sections);
        Box::pin(async move { Ok(values) })
    }

    fn work_done_progress_create(
        &mut self,
        _params: lsp::WorkDoneProgressCreateParams,
    ) -> BoxFuture<'static, Result<(), Self::Error>> {
        Box::pin(async { Ok(()) })
    }

    fn register_capability(
        &mut self,
        params: lsp::RegistrationParams,
    ) -> BoxFuture<'static, Result<(), Self::Error>> {
        let parsed = params
            .registrations
            .into_iter()
            .map(parse_registration)
            .collect::<Result<Vec<_>, _>>();
        let result = parsed.and_then(|registrations| {
            let snapshot = {
                let mut capabilities = self
                    .capabilities
                    .lock()
                    .expect("LSP capability state poisoned");
                if registrations.iter().any(|registration| {
                    capabilities
                        .registrations
                        .iter()
                        .any(|existing| existing.id == registration.id)
                }) {
                    return Err(invalid_registration("duplicate registration id"));
                }
                capabilities.registrations.extend(registrations);
                capabilities.registrations()
            };
            self.send(ClientEvent::RegistrationsChanged(snapshot));
            let features = self
                .capabilities
                .lock()
                .expect("LSP capability state poisoned")
                .features();
            self.send(ClientEvent::CapabilitiesChanged(features));
            Ok(())
        });
        Box::pin(async move { result })
    }

    fn unregister_capability(
        &mut self,
        params: lsp::UnregistrationParams,
    ) -> BoxFuture<'static, Result<(), Self::Error>> {
        let result = {
            let mut capabilities = self
                .capabilities
                .lock()
                .expect("LSP capability state poisoned");
            let valid = params.unregisterations.iter().all(|unregistration| {
                supported_registration_method(&unregistration.method)
                    && capabilities.registrations.iter().any(|registration| {
                        registration.id == unregistration.id
                            && registration.method == unregistration.method
                    })
            });
            if !valid {
                Err(invalid_registration(
                    "unknown or unsupported dynamic unregistration",
                ))
            } else {
                for unregistration in params.unregisterations {
                    capabilities.registrations.retain(|registration| {
                        registration.id != unregistration.id
                            || registration.method != unregistration.method
                    });
                }
                let snapshot = capabilities.registrations();
                drop(capabilities);
                self.send(ClientEvent::RegistrationsChanged(snapshot));
                let features = self
                    .capabilities
                    .lock()
                    .expect("LSP capability state poisoned")
                    .features();
                self.send(ClientEvent::CapabilitiesChanged(features));
                Ok(())
            }
        };
        Box::pin(async move { result })
    }

    fn publish_diagnostics(&mut self, params: lsp::PublishDiagnosticsParams) -> Self::NotifyResult {
        if params.uri.scheme() != "file" {
            return ControlFlow::Continue(());
        }
        self.send(ClientEvent::Diagnostics {
            uri: params.uri.to_string(),
            version: params.version,
            diagnostics: normalize_diagnostics(&params.uri, params.diagnostics),
        });
        ControlFlow::Continue(())
    }

    fn show_message(&mut self, params: lsp::ShowMessageParams) -> Self::NotifyResult {
        self.send(ClientEvent::Message {
            kind: message_kind(params.typ),
            message: params.message,
        });
        ControlFlow::Continue(())
    }

    fn log_message(&mut self, params: lsp::LogMessageParams) -> Self::NotifyResult {
        self.send(ClientEvent::Message {
            kind: message_kind(params.typ),
            message: params.message,
        });
        ControlFlow::Continue(())
    }

    fn progress(&mut self, params: lsp::ProgressParams) -> Self::NotifyResult {
        self.send(ClientEvent::Progress {
            token: match params.token {
                lsp::NumberOrString::Number(value) => value.to_string(),
                lsp::NumberOrString::String(value) => value,
            },
            progress: normalize_progress(params.value),
        });
        ControlFlow::Continue(())
    }
}

fn parse_registration(
    registration: lsp::Registration,
) -> Result<DynamicRegistration, async_lsp::ResponseError> {
    let options = registration.register_options.unwrap_or_else(|| {
        serde_json::json!({
            "documentSelector": null
        })
    });
    let options = match registration.method.as_str() {
        "textDocument/didOpen" => DynamicRegistrationOptions::DidOpen(
            serde_json::from_value(options).map_err(invalid_registration)?,
        ),
        "textDocument/didClose" => DynamicRegistrationOptions::DidClose(
            serde_json::from_value(options).map_err(invalid_registration)?,
        ),
        "textDocument/didChange" => {
            let options: lsp::TextDocumentChangeRegistrationOptions =
                serde_json::from_value(options).map_err(invalid_registration)?;
            if !matches!(options.sync_kind, 1 | 2) {
                return Err(invalid_registration("unsupported didChange syncKind"));
            }
            DynamicRegistrationOptions::DidChange(options)
        }
        "textDocument/didSave" => DynamicRegistrationOptions::DidSave(
            serde_json::from_value(options).map_err(invalid_registration)?,
        ),
        _ => {
            return Err(invalid_registration(
                "unsupported dynamic registration method",
            ));
        }
    };
    let selector_is_supported = match &options {
        DynamicRegistrationOptions::DidOpen(options)
        | DynamicRegistrationOptions::DidClose(options) => options.document_selector.is_none(),
        DynamicRegistrationOptions::DidChange(options) => options.document_selector.is_none(),
        DynamicRegistrationOptions::DidSave(options) => options
            .text_document_registration_options
            .document_selector
            .is_none(),
    };
    if !selector_is_supported {
        return Err(invalid_registration(
            "dynamic document selectors are unsupported",
        ));
    }
    Ok(DynamicRegistration {
        id: registration.id,
        method: registration.method,
        options,
    })
}

fn supported_registration_method(method: &str) -> bool {
    matches!(
        method,
        "textDocument/didOpen"
            | "textDocument/didClose"
            | "textDocument/didChange"
            | "textDocument/didSave"
    )
}

fn invalid_registration(message: impl std::fmt::Display) -> async_lsp::ResponseError {
    async_lsp::ResponseError::new(async_lsp::ErrorCode::INVALID_PARAMS, message)
}

fn message_kind(kind: lsp::MessageType) -> String {
    if kind == lsp::MessageType::ERROR {
        "error"
    } else if kind == lsp::MessageType::WARNING {
        "warning"
    } else if kind == lsp::MessageType::INFO {
        "info"
    } else {
        "log"
    }
    .to_owned()
}

pub(crate) fn normalize_completion(
    document_id: &str,
    source_version: i32,
    generation: u64,
    response: Option<lsp::CompletionResponse>,
) -> (CompletionResponse, Vec<(String, lsp::CompletionItem)>) {
    let (is_incomplete, items) = match response {
        None => (false, Vec::new()),
        Some(lsp::CompletionResponse::Array(items)) => (false, items),
        Some(lsp::CompletionResponse::List(list)) => (list.is_incomplete, list.items),
    };
    let mut originals = Vec::with_capacity(items.len());
    let items = items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            let id = format!("{document_id}:{source_version}:{generation}:{index}");
            let normalized = normalize_completion_item(id.clone(), &item);
            originals.push((id, item));
            normalized
        })
        .collect();
    (
        CompletionResponse {
            document_id: document_id.to_owned(),
            source_version,
            is_incomplete,
            items,
        },
        originals,
    )
}

pub(crate) fn normalize_resolved_completion(
    id: String,
    item: &lsp::CompletionItem,
) -> CompletionItem {
    normalize_completion_item(id, item)
}

fn normalize_completion_item(id: String, item: &lsp::CompletionItem) -> CompletionItem {
    CompletionItem {
        id,
        label: item.label.clone(),
        detail: item.detail.clone(),
        kind: item.kind.map(completion_kind),
        documentation: item
            .documentation
            .as_ref()
            .map(normalize_documentation)
            .unwrap_or_default(),
        sort_text: item.sort_text.clone(),
        filter_text: item.filter_text.clone(),
        insert_text: item.insert_text.clone(),
        is_snippet: item.insert_text_format == Some(lsp::InsertTextFormat::SNIPPET),
        text_edit: item.text_edit.as_ref().map(|edit| match edit {
            lsp::CompletionTextEdit::Edit(edit) => CompletionTextEdit::TextEdit {
                range: editor_range(edit.range),
                new_text: edit.new_text.clone(),
            },
            lsp::CompletionTextEdit::InsertAndReplace(edit) => {
                CompletionTextEdit::InsertReplaceEdit {
                    insert: editor_range(edit.insert),
                    replace: editor_range(edit.replace),
                    new_text: edit.new_text.clone(),
                }
            }
        }),
        additional_text_edits: item
            .additional_text_edits
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|edit| EditorTextEdit {
                range: editor_range(edit.range),
                new_text: edit.new_text,
            })
            .collect(),
        commit_characters: item.commit_characters.clone().unwrap_or_default(),
        deprecated: item.deprecated.unwrap_or(false)
            || item
                .tags
                .as_ref()
                .is_some_and(|tags| tags.contains(&lsp::CompletionItemTag::DEPRECATED)),
        unsupported_effects: item
            .command
            .as_ref()
            .map(|_| vec![CompletionUnsupportedEffect::Command])
            .unwrap_or_default(),
    }
}

pub(crate) fn resolve_workspace_configuration<'a>(
    configuration: &Value,
    sections: impl IntoIterator<Item = Option<&'a str>>,
) -> Vec<Value> {
    sections
        .into_iter()
        .map(|section| {
            let Some(section) = section else {
                return configuration.clone();
            };
            section
                .split('.')
                .try_fold(configuration, |value, key| value.get(key))
                .cloned()
                .unwrap_or(Value::Null)
        })
        .collect()
}

pub(crate) fn normalize_progress(value: lsp::ProgressParamsValue) -> ProgressPayload {
    match value {
        lsp::ProgressParamsValue::WorkDone(lsp::WorkDoneProgress::Begin(value)) => {
            ProgressPayload::Begin {
                title: value.title,
                cancellable: value.cancellable,
                message: value.message,
                percentage: value.percentage,
            }
        }
        lsp::ProgressParamsValue::WorkDone(lsp::WorkDoneProgress::Report(value)) => {
            ProgressPayload::Report {
                cancellable: value.cancellable,
                message: value.message,
                percentage: value.percentage,
            }
        }
        lsp::ProgressParamsValue::WorkDone(lsp::WorkDoneProgress::End(value)) => {
            ProgressPayload::End {
                message: value.message,
            }
        }
    }
}

fn completion_kind(kind: lsp::CompletionItemKind) -> String {
    let value = if kind == lsp::CompletionItemKind::METHOD {
        "method"
    } else if kind == lsp::CompletionItemKind::FUNCTION {
        "function"
    } else if kind == lsp::CompletionItemKind::CONSTRUCTOR {
        "constructor"
    } else if kind == lsp::CompletionItemKind::FIELD {
        "field"
    } else if kind == lsp::CompletionItemKind::VARIABLE {
        "variable"
    } else if kind == lsp::CompletionItemKind::CLASS {
        "class"
    } else if kind == lsp::CompletionItemKind::INTERFACE {
        "interface"
    } else if kind == lsp::CompletionItemKind::MODULE {
        "module"
    } else if kind == lsp::CompletionItemKind::PROPERTY {
        "property"
    } else if kind == lsp::CompletionItemKind::FILE {
        "file"
    } else if kind == lsp::CompletionItemKind::FOLDER {
        "folder"
    } else if kind == lsp::CompletionItemKind::KEYWORD {
        "keyword"
    } else if kind == lsp::CompletionItemKind::SNIPPET {
        "snippet"
    } else {
        "text"
    };
    value.to_owned()
}

pub(crate) fn normalize_hover(
    document_id: &str,
    source_version: i32,
    hover: Option<lsp::Hover>,
) -> HoverResponse {
    let (range, blocks) = hover.map_or((None, Vec::new()), |hover| {
        let blocks = match hover.contents {
            lsp::HoverContents::Scalar(value) => vec![normalize_marked_string(value)],
            lsp::HoverContents::Array(values) => {
                values.into_iter().map(normalize_marked_string).collect()
            }
            lsp::HoverContents::Markup(value) => vec![normalize_markup(value)],
        };
        (hover.range.map(editor_range), blocks)
    });
    HoverResponse {
        document_id: document_id.to_owned(),
        source_version,
        range,
        blocks,
    }
}

fn normalize_marked_string(value: lsp::MarkedString) -> HoverBlock {
    match value {
        lsp::MarkedString::String(value) => HoverBlock {
            markdown: true,
            value,
        },
        lsp::MarkedString::LanguageString(value) => HoverBlock {
            markdown: true,
            value: format!("```{}\n{}\n```", value.language, value.value),
        },
    }
}

fn normalize_markup(value: lsp::MarkupContent) -> HoverBlock {
    HoverBlock {
        markdown: value.kind == lsp::MarkupKind::Markdown,
        value: value.value,
    }
}

fn normalize_documentation(value: &lsp::Documentation) -> Vec<HoverBlock> {
    vec![match value {
        lsp::Documentation::String(value) => HoverBlock {
            markdown: false,
            value: value.clone(),
        },
        lsp::Documentation::MarkupContent(value) => normalize_markup(value.clone()),
    }]
}

pub(crate) fn normalize_signature_help(
    document_id: &str,
    source_version: i32,
    help: Option<lsp::SignatureHelp>,
) -> SignatureHelpResponse {
    let help = help.unwrap_or(lsp::SignatureHelp {
        signatures: Vec::new(),
        active_signature: None,
        active_parameter: None,
    });
    SignatureHelpResponse {
        document_id: document_id.to_owned(),
        source_version,
        signatures: help
            .signatures
            .into_iter()
            .map(|signature| SignatureInformation {
                label: signature.label.clone(),
                documentation: signature
                    .documentation
                    .as_ref()
                    .map(normalize_documentation)
                    .unwrap_or_default(),
                parameters: signature
                    .parameters
                    .unwrap_or_default()
                    .into_iter()
                    .map(|parameter| match parameter.label {
                        lsp::ParameterLabel::Simple(label) => SignatureParameter {
                            label,
                            label_start_utf16: None,
                            label_end_utf16: None,
                            documentation: parameter
                                .documentation
                                .as_ref()
                                .map(normalize_documentation)
                                .unwrap_or_default(),
                        },
                        lsp::ParameterLabel::LabelOffsets([start, end]) => SignatureParameter {
                            label: utf16_slice(&signature.label, start, end),
                            label_start_utf16: Some(start),
                            label_end_utf16: Some(end),
                            documentation: parameter
                                .documentation
                                .as_ref()
                                .map(normalize_documentation)
                                .unwrap_or_default(),
                        },
                    })
                    .collect(),
                active_parameter: signature.active_parameter,
            })
            .collect(),
        active_signature: help.active_signature,
        active_parameter: help.active_parameter,
    }
}

pub(crate) fn normalize_definition(
    document_id: &str,
    source_version: i32,
    response: Option<lsp::GotoDefinitionResponse>,
) -> DefinitionResponse {
    let locations = match response {
        None => Vec::new(),
        Some(lsp::GotoDefinitionResponse::Scalar(location)) => vec![location],
        Some(lsp::GotoDefinitionResponse::Array(locations)) => locations,
        Some(lsp::GotoDefinitionResponse::Link(links)) => links
            .into_iter()
            .map(|link| lsp::Location::new(link.target_uri, link.target_selection_range))
            .collect(),
    }
    .into_iter()
    .filter_map(normalize_location)
    .collect();
    DefinitionResponse {
        document_id: document_id.to_owned(),
        source_version,
        locations,
    }
}

fn normalize_location(location: lsp::Location) -> Option<EditorLocation> {
    (location.uri.scheme() == "file").then(|| EditorLocation {
        uri: location.uri.to_string(),
        range: editor_range(location.range),
    })
}

pub(crate) fn normalize_diagnostics(
    uri: &lsp::Url,
    diagnostics: Vec<lsp::Diagnostic>,
) -> Vec<Diagnostic> {
    diagnostics
        .into_iter()
        .enumerate()
        .map(|(index, diagnostic)| Diagnostic {
            id: format!(
                "{}:{}:{}:{}",
                uri, diagnostic.range.start.line, diagnostic.range.start.character, index
            ),
            uri: uri.to_string(),
            range: editor_range(diagnostic.range),
            severity: match diagnostic.severity {
                Some(value) if value == lsp::DiagnosticSeverity::ERROR => DiagnosticSeverity::Error,
                Some(value) if value == lsp::DiagnosticSeverity::INFORMATION => {
                    DiagnosticSeverity::Information
                }
                Some(value) if value == lsp::DiagnosticSeverity::HINT => DiagnosticSeverity::Hint,
                _ => DiagnosticSeverity::Warning,
            },
            code: diagnostic.code.map(|code| match code {
                lsp::NumberOrString::Number(value) => value.to_string(),
                lsp::NumberOrString::String(value) => value,
            }),
            source: diagnostic.source,
            message: diagnostic.message,
            related_information: diagnostic
                .related_information
                .unwrap_or_default()
                .into_iter()
                .filter_map(|related| {
                    normalize_location(related.location).map(|location| {
                        DiagnosticRelatedInformation {
                            location,
                            message: related.message,
                        }
                    })
                })
                .collect(),
        })
        .collect()
}

pub(crate) fn editor_range(range: lsp::Range) -> EditorRange {
    EditorRange {
        start: EditorPosition {
            line: range.start.line,
            character: range.start.character,
        },
        end: EditorPosition {
            line: range.end.line,
            character: range.end.character,
        },
    }
}

fn utf16_slice(value: &str, start: u32, end: u32) -> String {
    let value = value.encode_utf16().collect::<Vec<_>>();
    let range = start as usize..end as usize;
    String::from_utf16_lossy(value.get(range).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use async_lsp::lsp_types as lsp;
    use serde_json::json;

    use super::{
        ProgressPayload, normalize_completion, normalize_definition, normalize_diagnostics,
        normalize_hover, normalize_progress, normalize_resolved_completion,
        normalize_signature_help, resolve_workspace_configuration,
    };
    use crate::lsp::types::{CompletionTextEdit, CompletionUnsupportedEffect, DiagnosticSeverity};

    #[test]
    fn completion_normalizes_edits_snippets_and_unsupported_command_effects() {
        let raw: lsp::CompletionResponse = serde_json::from_value(json!({
            "isIncomplete": true,
            "items": [{
                "label": "console.log",
                "kind": 3,
                "documentation": { "kind": "markdown", "value": "**log**" },
                "insertText": "log(${1:value})",
                "insertTextFormat": 2,
                "textEdit": {
                    "newText": "console.log(${1:value})",
                    "insert": {
                        "start": { "line": 2, "character": 4 },
                        "end": { "line": 2, "character": 7 }
                    },
                    "replace": {
                        "start": { "line": 2, "character": 1 },
                        "end": { "line": 2, "character": 7 }
                    }
                },
                "additionalTextEdits": [{
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 0, "character": 0 }
                    },
                    "newText": "import console;\n"
                }],
                "commitCharacters": [".", "("],
                "command": { "title": "Run import", "command": "editor.runImport" }
            }]
        }))
        .unwrap();

        let (response, originals) = normalize_completion("doc", 7, 12, Some(raw));

        assert!(response.is_incomplete);
        assert_eq!(response.items[0].kind.as_deref(), Some("function"));
        assert!(response.items[0].is_snippet);
        assert_eq!(response.items[0].commit_characters, [".", "("]);
        assert_eq!(response.items[0].additional_text_edits.len(), 1);
        assert_eq!(
            response.items[0].unsupported_effects,
            [CompletionUnsupportedEffect::Command]
        );
        assert!(matches!(
            response.items[0].text_edit,
            Some(CompletionTextEdit::InsertReplaceEdit { .. })
        ));
        assert_eq!(originals[0].0, "doc:7:12:0");

        let mut resolved = originals[0].1.clone();
        resolved.command = Some(lsp::Command {
            title: "Run resolved action".into(),
            command: "editor.runResolved".into(),
            arguments: None,
        });
        assert_eq!(
            normalize_resolved_completion(originals[0].0.clone(), &resolved).unsupported_effects,
            [CompletionUnsupportedEffect::Command]
        );
    }

    #[test]
    fn workspace_configuration_resolves_ordered_dot_sections_and_missing_values() {
        let configuration = json!({
            "typescript": {
                "preferences": { "quoteStyle": "single" }
            },
            "javascript": { "format": true }
        });

        assert_eq!(
            resolve_workspace_configuration(
                &configuration,
                [
                    None,
                    Some("typescript.preferences"),
                    Some("missing.section"),
                    Some("javascript"),
                ]
            ),
            vec![
                configuration,
                json!({ "quoteStyle": "single" }),
                json!(null),
                json!({ "format": true }),
            ]
        );
    }

    #[test]
    fn progress_is_normalized_to_typed_begin_report_and_end_payloads() {
        let begin: lsp::ProgressParamsValue = serde_json::from_value(json!({
            "kind": "begin",
            "title": "Indexing",
            "cancellable": true,
            "message": "starting",
            "percentage": 1
        }))
        .unwrap();
        let report: lsp::ProgressParamsValue = serde_json::from_value(json!({
            "kind": "report",
            "message": "halfway",
            "percentage": 50
        }))
        .unwrap();
        let end: lsp::ProgressParamsValue =
            serde_json::from_value(json!({ "kind": "end", "message": "done" })).unwrap();

        assert_eq!(
            normalize_progress(begin),
            ProgressPayload::Begin {
                title: "Indexing".into(),
                cancellable: Some(true),
                message: Some("starting".into()),
                percentage: Some(1),
            }
        );
        assert_eq!(
            normalize_progress(report),
            ProgressPayload::Report {
                cancellable: None,
                message: Some("halfway".into()),
                percentage: Some(50),
            }
        );
        assert_eq!(
            normalize_progress(end),
            ProgressPayload::End {
                message: Some("done".into()),
            }
        );
    }

    #[test]
    fn hover_and_signature_preserve_markup_and_active_parameter_offsets() {
        let hover: lsp::Hover = serde_json::from_value(json!({
            "contents": [
                "**markdown**",
                { "language": "typescript", "value": "const answer = 42" }
            ]
        }))
        .unwrap();
        let hover = normalize_hover("doc", 4, Some(hover));
        assert_eq!(hover.blocks[0].value, "**markdown**");
        assert_eq!(
            hover.blocks[1].value,
            "```typescript\nconst answer = 42\n```"
        );
        assert!(hover.blocks.iter().all(|block| block.markdown));

        let signature: lsp::SignatureHelp = serde_json::from_value(json!({
            "signatures": [{
                "label": "log(value: unknown)",
                "documentation": "plain docs",
                "parameters": [{
                    "label": [4, 18],
                    "documentation": { "kind": "markdown", "value": "the **value**" }
                }],
                "activeParameter": 0
            }],
            "activeSignature": 0,
            "activeParameter": 0
        }))
        .unwrap();
        let signature = normalize_signature_help("doc", 4, Some(signature));
        assert_eq!(signature.active_signature, Some(0));
        assert_eq!(signature.active_parameter, Some(0));
        assert_eq!(
            signature.signatures[0].parameters[0].label,
            "value: unknown"
        );
        assert_eq!(
            signature.signatures[0].parameters[0].label_start_utf16,
            Some(4)
        );
        assert!(!signature.signatures[0].documentation[0].markdown);
        assert!(signature.signatures[0].parameters[0].documentation[0].markdown);
    }

    #[test]
    fn definitions_and_diagnostic_related_locations_reject_non_file_uris() {
        let definition: lsp::GotoDefinitionResponse = serde_json::from_value(json!([
            {
                "targetUri": "file:///project/main.ts",
                "targetRange": {
                    "start": { "line": 1, "character": 0 },
                    "end": { "line": 1, "character": 8 }
                },
                "targetSelectionRange": {
                    "start": { "line": 1, "character": 2 },
                    "end": { "line": 1, "character": 6 }
                }
            },
            {
                "targetUri": "https://example.invalid/source.ts",
                "targetRange": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 1 }
                },
                "targetSelectionRange": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 1 }
                }
            }
        ]))
        .unwrap();
        let definition = normalize_definition("doc", 9, Some(definition));
        assert_eq!(definition.locations.len(), 1);
        assert_eq!(definition.locations[0].range.start.character, 2);

        let uri = lsp::Url::parse("file:///project/main.ts").unwrap();
        let diagnostics: Vec<lsp::Diagnostic> = serde_json::from_value(json!([{
            "range": {
                "start": { "line": 3, "character": 1 },
                "end": { "line": 3, "character": 5 }
            },
            "severity": 1,
            "code": 2322,
            "source": "typescript",
            "message": "not assignable",
            "relatedInformation": [
                {
                    "location": {
                        "uri": "file:///project/types.ts",
                        "range": {
                            "start": { "line": 8, "character": 0 },
                            "end": { "line": 8, "character": 4 }
                        }
                    },
                    "message": "declared here"
                },
                {
                    "location": {
                        "uri": "untitled:buffer",
                        "range": {
                            "start": { "line": 0, "character": 0 },
                            "end": { "line": 0, "character": 1 }
                        }
                    },
                    "message": "unsupported"
                }
            ]
        }]))
        .unwrap();
        let diagnostics = normalize_diagnostics(&uri, diagnostics);
        assert_eq!(diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diagnostics[0].code.as_deref(), Some("2322"));
        assert_eq!(diagnostics[0].source.as_deref(), Some("typescript"));
        assert_eq!(diagnostics[0].related_information.len(), 1);
    }
}
