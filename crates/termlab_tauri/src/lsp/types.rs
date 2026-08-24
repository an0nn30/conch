use serde::{Deserialize, Serialize};
use ts_rs::TS;

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
    pub text_edit: Option<EditorTextEdit>,
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
        ApplyChangesResponse, CompletionItem, Diagnostic, EditorLocation, EditorPosition,
        EditorRange, HoverBlock, LspChangeBatch, LspStatus, LspTextChange, ProjectCandidate,
        SignatureHelpResponse,
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
}
