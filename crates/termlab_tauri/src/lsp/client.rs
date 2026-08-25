use std::collections::HashSet;
use std::ops::ControlFlow;

use async_lsp::lsp_types as lsp;
use async_lsp::LanguageClient;
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::sync::mpsc;

use super::types::{
    CompletionItem, CompletionResponse, CompletionTextEdit, DefinitionResponse, Diagnostic,
    DiagnosticRelatedInformation, DiagnosticSeverity, EditorLocation, EditorPosition, EditorRange,
    EditorTextEdit, HoverBlock, HoverResponse, SignatureHelpResponse, SignatureInformation,
    SignatureParameter,
};

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
        value: Value,
    },
    RegistrationsChanged(Vec<String>),
    ProtocolExited(Option<String>),
    ProcessExited {
        success: bool,
        code: Option<i32>,
    },
}

pub(crate) struct ClientState {
    events: mpsc::Sender<ClientEvent>,
    workspace_configuration: Value,
    registrations: HashSet<String>,
}

impl ClientState {
    pub(crate) fn new(events: mpsc::Sender<ClientEvent>, workspace_configuration: Value) -> Self {
        Self {
            events,
            workspace_configuration,
            registrations: HashSet::new(),
        }
    }

    fn send(&self, event: ClientEvent) {
        let _ = self.events.try_send(event);
    }

    fn registration_snapshot(&self) -> Vec<String> {
        let mut registrations = self.registrations.iter().cloned().collect::<Vec<_>>();
        registrations.sort();
        registrations
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
        Box::pin(async move {
            Ok(params
                .items
                .into_iter()
                .map(|_| configuration.clone())
                .collect())
        })
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
        for registration in params.registrations {
            self.registrations.insert(registration.id);
        }
        self.send(ClientEvent::RegistrationsChanged(
            self.registration_snapshot(),
        ));
        Box::pin(async { Ok(()) })
    }

    fn unregister_capability(
        &mut self,
        params: lsp::UnregistrationParams,
    ) -> BoxFuture<'static, Result<(), Self::Error>> {
        for registration in params.unregisterations {
            self.registrations.remove(&registration.id);
        }
        self.send(ClientEvent::RegistrationsChanged(
            self.registration_snapshot(),
        ));
        Box::pin(async { Ok(()) })
    }

    fn publish_diagnostics(&mut self, params: lsp::PublishDiagnosticsParams) -> Self::NotifyResult {
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
            value: serde_json::to_value(params.value).unwrap_or(Value::Null),
        });
        ControlFlow::Continue(())
    }
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
            let id = format!("{document_id}:{source_version}:{index}");
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
        workspace_edit_unsupported: item.data.as_ref().is_some_and(|data| {
            data.get("workspaceEdit").is_some() || data.get("documentChanges").is_some()
        }),
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
        normalize_completion, normalize_definition, normalize_diagnostics, normalize_hover,
        normalize_signature_help,
    };
    use crate::lsp::types::{CompletionTextEdit, DiagnosticSeverity};

    #[test]
    fn completion_normalizes_edits_snippets_and_unsupported_workspace_metadata() {
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
                "data": { "workspaceEdit": { "changes": { "file:///other.ts": [] } } }
            }]
        }))
        .unwrap();

        let (response, originals) = normalize_completion("doc", 7, Some(raw));

        assert!(response.is_incomplete);
        assert_eq!(response.items[0].kind.as_deref(), Some("function"));
        assert!(response.items[0].is_snippet);
        assert_eq!(response.items[0].commit_characters, [".", "("]);
        assert_eq!(response.items[0].additional_text_edits.len(), 1);
        assert!(response.items[0].workspace_edit_unsupported);
        assert!(matches!(
            response.items[0].text_edit,
            Some(CompletionTextEdit::InsertReplaceEdit { .. })
        ));
        assert_eq!(originals[0].0, "doc:7:0");
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
