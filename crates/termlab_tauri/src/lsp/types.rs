use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(type = "string")]
pub(crate) struct ReservationId(Uuid);

impl ReservationId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(type = "string")]
pub(crate) struct DocumentId(Uuid);

impl DocumentId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, tag = "kind", rename_all = "camelCase")]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ReserveResult {
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Reserved {
        reservation_id: ReservationId,
        canonical_path: String,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    FocusPending { window_label: String },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    FocusOwner {
        document_id: DocumentId,
        window_label: String,
        pane_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LspTextChange {
    pub from_utf16: u32,
    pub to_utf16: u32,
    pub inserted_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LspChangeBatch {
    pub document_id: String,
    pub base_version: i32,
    pub next_version: i32,
    pub changes: Vec<LspTextChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, tag = "kind", rename_all = "camelCase")]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ApplyChangesResponse {
    Applied {
        version: i32,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    ResyncRequired {
        expected_version: i32,
        received_base_version: i32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditorPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditorRange {
    pub start: EditorPosition,
    pub end: EditorPosition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditorTextEdit {
    pub range: EditorRange,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, tag = "kind", rename_all = "camelCase")]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum CompletionTextEdit {
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    TextEdit {
        range: EditorRange,
        new_text: String,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    InsertReplaceEdit {
        insert: EditorRange,
        replace: EditorRange,
        new_text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HoverBlock {
    /// True only after the backend has accepted and normalized Markdown from
    /// the server. The frontend still sanitizes it before inserting DOM.
    pub markdown: bool,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompletionItem {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    /// A stable frontend category such as `function`, `variable`, or `file`;
    /// never the numeric LSP `CompletionItemKind` value.
    pub kind: Option<String>,
    pub documentation: Vec<HoverBlock>,
    pub sort_text: Option<String>,
    pub filter_text: Option<String>,
    pub insert_text: Option<String>,
    pub is_snippet: bool,
    pub text_edit: Option<CompletionTextEdit>,
    pub additional_text_edits: Vec<EditorTextEdit>,
    pub commit_characters: Vec<String>,
    pub deprecated: bool,
    pub workspace_edit_unsupported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompletionResponse {
    pub document_id: String,
    pub source_version: i32,
    pub is_incomplete: bool,
    pub items: Vec<CompletionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HoverResponse {
    pub document_id: String,
    pub source_version: i32,
    pub range: Option<EditorRange>,
    pub blocks: Vec<HoverBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignatureParameter {
    pub label: String,
    pub label_start_utf16: Option<u32>,
    pub label_end_utf16: Option<u32>,
    pub documentation: Vec<HoverBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignatureInformation {
    pub label: String,
    pub documentation: Vec<HoverBlock>,
    pub parameters: Vec<SignatureParameter>,
    pub active_parameter: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SignatureHelpResponse {
    pub document_id: String,
    pub source_version: i32,
    pub signatures: Vec<SignatureInformation>,
    pub active_signature: Option<u32>,
    pub active_parameter: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditorLocation {
    pub uri: String,
    pub range: EditorRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DefinitionResponse {
    pub document_id: String,
    pub source_version: i32,
    pub locations: Vec<EditorLocation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub(crate) enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticRelatedInformation {
    pub location: EditorLocation,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Diagnostic {
    pub id: String,
    pub uri: String,
    pub range: EditorRange,
    pub severity: DiagnosticSeverity,
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
    pub related_information: Vec<DiagnosticRelatedInformation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticSnapshot {
    pub revision: u32,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectCandidate {
    pub root_uri: String,
    pub canonical_path: String,
    pub display_name: String,
    pub marker: String,
    pub reason: String,
    pub confidence: u8,
    pub is_fallback: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub(crate) enum LspSessionState {
    Disabled,
    ChoosingProject,
    Untrusted,
    Starting,
    Indexing,
    Ready,
    Failed,
    Unavailable,
}

/// A stable reason why a curated language adapter cannot be started. The
/// catalog keeps the detailed filesystem validation error internal, while the
/// manager later exposes this normalized status to the frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, tag = "kind", rename_all = "camelCase")]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum LspUnavailableReason {
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    NotBundledYet { adapter_id: String },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    MissingResource {
        adapter_id: String,
        relative_path: String,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    CorruptResource {
        adapter_id: String,
        relative_path: String,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    UnsupportedPlatform { expected: String, actual: String },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    UnsupportedArchitecture { expected: String, actual: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LspCapabilities {
    pub completion: bool,
    pub hover: bool,
    pub signature_help: bool,
    pub definition: bool,
    pub diagnostics: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LspStatus {
    pub revision: u32,
    pub document_id: Option<String>,
    pub session_id: Option<String>,
    pub adapter_id: Option<String>,
    pub project_root_uri: Option<String>,
    pub state: LspSessionState,
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub unavailable_reason: Option<LspUnavailableReason>,
    pub capabilities: LspCapabilities,
    pub error_count: u32,
    pub warning_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenDocumentResponse {
    pub document_id: String,
    pub version: i32,
    pub project_candidates: Vec<ProjectCandidate>,
    pub status: LspStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResyncDocumentResponse {
    pub document_id: String,
    pub version: i32,
    pub status: LspStatus,
}

#[cfg(test)]
mod tests {
    use super::{
        ApplyChangesResponse, CompletionItem, CompletionTextEdit, Diagnostic, EditorLocation,
        EditorPosition, EditorRange, HoverBlock, LspCapabilities, LspChangeBatch, LspSessionState,
        LspStatus, LspTextChange, LspUnavailableReason, ProjectCandidate, SignatureHelpResponse,
    };
    use ts_rs::TS;

    #[test]
    fn version_mismatch_response_serializes_as_resync_required() {
        let response = ApplyChangesResponse::ResyncRequired {
            expected_version: 7,
            received_base_version: 6,
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            serde_json::json!({
                "kind": "resyncRequired",
                "expectedVersion": 7,
                "receivedBaseVersion": 6,
            })
        );
    }

    #[test]
    fn normalized_boundary_types_export_typescript_declarations() {
        let config = ts_rs::Config::default();
        let completion_declaration = CompletionItem::decl(&config);
        assert!(completion_declaration.contains("sortText"));
        assert!(!completion_declaration.contains("sort_text"));

        for declaration in [
            EditorPosition::decl(&config),
            EditorRange::decl(&config),
            completion_declaration,
            HoverBlock::decl(&config),
            SignatureHelpResponse::decl(&config),
            EditorLocation::decl(&config),
            Diagnostic::decl(&config),
            ProjectCandidate::decl(&config),
            LspStatus::decl(&config),
            ApplyChangesResponse::decl(&config),
        ] {
            assert!(declaration.starts_with("type ") || declaration.starts_with("interface "));
        }
    }

    #[test]
    fn unavailable_reasons_serialize_in_exact_camel_case_and_export_to_typescript() {
        let values = [
            (
                LspUnavailableReason::NotBundledYet {
                    adapter_id: "json".into(),
                },
                serde_json::json!({ "kind": "notBundledYet", "adapterId": "json" }),
            ),
            (
                LspUnavailableReason::MissingResource {
                    adapter_id: "rust".into(),
                    relative_path: "manifest.json".into(),
                },
                serde_json::json!({ "kind": "missingResource", "adapterId": "rust", "relativePath": "manifest.json" }),
            ),
            (
                LspUnavailableReason::CorruptResource {
                    adapter_id: "typescript".into(),
                    relative_path: "node/bin/node".into(),
                },
                serde_json::json!({ "kind": "corruptResource", "adapterId": "typescript", "relativePath": "node/bin/node" }),
            ),
            (
                LspUnavailableReason::UnsupportedPlatform {
                    expected: "macOS".into(),
                    actual: "linux".into(),
                },
                serde_json::json!({ "kind": "unsupportedPlatform", "expected": "macOS", "actual": "linux" }),
            ),
            (
                LspUnavailableReason::UnsupportedArchitecture {
                    expected: "arm64".into(),
                    actual: "x86_64".into(),
                },
                serde_json::json!({ "kind": "unsupportedArchitecture", "expected": "arm64", "actual": "x86_64" }),
            ),
        ];
        for (value, expected) in values {
            assert_eq!(serde_json::to_value(value).unwrap(), expected);
        }
        let declaration = LspUnavailableReason::decl(&ts_rs::Config::default());
        assert!(declaration.contains("notBundledYet"));
        assert!(declaration.contains("relativePath"));
        assert!(LspStatus::decl(&ts_rs::Config::default()).contains("unavailableReason"));
    }

    #[test]
    fn unavailable_reason_is_optional_on_normalized_lsp_status() {
        let status = LspStatus {
            revision: 1,
            document_id: None,
            session_id: None,
            adapter_id: Some("json".into()),
            project_root_uri: None,
            state: LspSessionState::Unavailable,
            message: None,
            unavailable_reason: Some(LspUnavailableReason::NotBundledYet {
                adapter_id: "json".into(),
            }),
            capabilities: LspCapabilities {
                completion: false,
                hover: false,
                signature_help: false,
                definition: false,
                diagnostics: false,
            },
            error_count: 0,
            warning_count: 0,
        };

        assert_eq!(
            serde_json::to_value(status).unwrap(),
            serde_json::json!({
                "revision": 1,
                "documentId": null,
                "sessionId": null,
                "adapterId": "json",
                "projectRootUri": null,
                "state": "unavailable",
                "message": null,
                "unavailableReason": { "kind": "notBundledYet", "adapterId": "json" },
                "capabilities": { "completion": false, "hover": false, "signatureHelp": false, "definition": false, "diagnostics": false },
                "errorCount": 0,
                "warningCount": 0,
            })
        );

        let without_reason = LspStatus {
            revision: 2,
            document_id: None,
            session_id: None,
            adapter_id: None,
            project_root_uri: None,
            state: LspSessionState::Disabled,
            message: None,
            unavailable_reason: None,
            capabilities: LspCapabilities {
                completion: false,
                hover: false,
                signature_help: false,
                definition: false,
                diagnostics: false,
            },
            error_count: 0,
            warning_count: 0,
        };
        let serialized = serde_json::to_value(&without_reason).unwrap();
        assert_eq!(
            serialized,
            serde_json::json!({
                "revision": 2,
                "documentId": null,
                "sessionId": null,
                "adapterId": null,
                "projectRootUri": null,
                "state": "disabled",
                "message": null,
                "capabilities": { "completion": false, "hover": false, "signatureHelp": false, "definition": false, "diagnostics": false },
                "errorCount": 0,
                "warningCount": 0,
            })
        );
        let restored: LspStatus = serde_json::from_value(serialized).unwrap();
        assert_eq!(restored.unavailable_reason, None);

        assert_eq!(
            LspUnavailableReason::decl(&ts_rs::Config::default()),
            "type LspUnavailableReason = { \"kind\": \"notBundledYet\", adapterId: string, } | { \"kind\": \"missingResource\", adapterId: string, relativePath: string, } | { \"kind\": \"corruptResource\", adapterId: string, relativePath: string, } | { \"kind\": \"unsupportedPlatform\", expected: string, actual: string, } | { \"kind\": \"unsupportedArchitecture\", expected: string, actual: string, };"
        );
        assert_eq!(
            LspStatus::decl(&ts_rs::Config::default()),
            "type LspStatus = { revision: number, documentId: string | null, sessionId: string | null, adapterId: string | null, projectRootUri: string | null, state: LspSessionState, message: string | null, unavailableReason?: LspUnavailableReason, capabilities: LspCapabilities, errorCount: number, warningCount: number, };"
        );
    }

    #[test]
    fn change_batch_serializes_frontend_offsets_in_camel_case() {
        let batch = LspChangeBatch {
            document_id: "doc-1".into(),
            base_version: 3,
            next_version: 4,
            changes: vec![LspTextChange {
                from_utf16: 1,
                to_utf16: 2,
                inserted_text: "X".into(),
            }],
        };

        assert_eq!(
            serde_json::to_value(batch).unwrap(),
            serde_json::json!({
                "documentId": "doc-1",
                "baseVersion": 3,
                "nextVersion": 4,
                "changes": [{
                    "fromUtf16": 1,
                    "toUtf16": 2,
                    "insertedText": "X",
                }],
            })
        );
    }

    #[test]
    fn completion_text_edits_preserve_plain_and_insert_replace_ranges() {
        let insert = EditorRange {
            start: EditorPosition {
                line: 2,
                character: 3,
            },
            end: EditorPosition {
                line: 2,
                character: 5,
            },
        };
        let replace = EditorRange {
            start: EditorPosition {
                line: 2,
                character: 3,
            },
            end: EditorPosition {
                line: 2,
                character: 8,
            },
        };

        assert_eq!(
            serde_json::to_value(CompletionTextEdit::TextEdit {
                range: insert,
                new_text: "plain".into(),
            })
            .unwrap(),
            serde_json::json!({
                "kind": "textEdit",
                "range": {
                    "start": { "line": 2, "character": 3 },
                    "end": { "line": 2, "character": 5 },
                },
                "newText": "plain",
            })
        );
        assert_eq!(
            serde_json::to_value(CompletionTextEdit::InsertReplaceEdit {
                insert,
                replace,
                new_text: "wide".into(),
            })
            .unwrap(),
            serde_json::json!({
                "kind": "insertReplaceEdit",
                "insert": {
                    "start": { "line": 2, "character": 3 },
                    "end": { "line": 2, "character": 5 },
                },
                "replace": {
                    "start": { "line": 2, "character": 3 },
                    "end": { "line": 2, "character": 8 },
                },
                "newText": "wide",
            })
        );
    }

    #[test]
    fn completion_text_edit_typescript_keeps_both_insert_replace_ranges() {
        let declaration = CompletionTextEdit::decl(&ts_rs::Config::default());

        assert!(declaration.contains("insert: EditorRange"));
        assert!(declaration.contains("replace: EditorRange"));
        assert!(declaration.contains("newText: string"));
        assert!(!declaration.contains("new_text"));
    }
}
