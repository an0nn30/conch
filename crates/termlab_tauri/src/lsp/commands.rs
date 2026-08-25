//! Typed Tauri boundary for the application-wide LSP manager.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{Emitter, Manager, Runtime};

use super::manager::{
    LspManagerHandle, ManagerEvent, ProjectContextChoice, SessionLogEntry, TrustedProject,
};
use super::trust::TrustDecision;
use super::types::{
    ApplyChangesResponse, CompletionResponse, DefinitionResponse, DiagnosticSnapshot, DocumentId,
    EditorPosition, HoverResponse, LspChangeBatch, LspStatus, OpenDocumentResponse,
    ProjectCandidate, ReservationId, ReserveResult, ResyncDocumentResponse, SignatureHelpResponse,
};

pub(crate) const SESSION_STATUS_EVENT: &str = "lsp-session-status";
pub(crate) const DIAGNOSTICS_UPDATED_EVENT: &str = "lsp-diagnostics-updated";
pub(crate) const DOCUMENT_OWNER_FOCUSED_EVENT: &str = "editor-document-owner-focused";

pub(crate) fn invoke_handler<R: Runtime>()
-> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        editor_reserve_document,
        editor_release_document,
        editor_transfer_document,
        lsp_open_document,
        lsp_apply_changes,
        lsp_resync_document,
        lsp_did_save,
        lsp_close_document,
        lsp_close_documents,
        lsp_project_candidates,
        lsp_set_project_context,
        lsp_set_project_trust,
        lsp_completion,
        lsp_hover,
        lsp_signature_help,
        lsp_definition,
        lsp_problems_snapshot,
        lsp_status_snapshot,
        lsp_restart_session,
        lsp_session_logs,
        lsp_trusted_projects,
        lsp_revoke_project_trust,
    ]
}

pub(crate) fn is_lsp_command(name: &str) -> bool {
    name.starts_with("lsp_")
        || matches!(
            name,
            "editor_reserve_document" | "editor_release_document" | "editor_transfer_document"
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommandContract {
    pub name: &'static str,
    pub args: &'static [&'static str],
    pub result: &'static str,
}

/// Auditable invoke surface. This registry is consumed by boundary tests and
/// keeps frontend argument names and normalized result types explicit.
pub(crate) const LSP_COMMAND_CONTRACTS: &[CommandContract] = &[
    CommandContract {
        name: "editor_reserve_document",
        args: &["path", "windowLabel"],
        result: "ReserveResult",
    },
    CommandContract {
        name: "editor_release_document",
        args: &["reservationId"],
        result: "void",
    },
    CommandContract {
        name: "editor_transfer_document",
        args: &["documentId", "targetReservationId", "windowLabel", "paneId"],
        result: "OpenDocumentResponse",
    },
    CommandContract {
        name: "lsp_open_document",
        args: &["reservationId", "paneId", "contents", "languageId"],
        result: "OpenDocumentResponse",
    },
    CommandContract {
        name: "lsp_apply_changes",
        args: &["documentId", "batch"],
        result: "ApplyChangesResponse",
    },
    CommandContract {
        name: "lsp_resync_document",
        args: &["documentId", "version", "contents"],
        result: "ResyncDocumentResponse",
    },
    CommandContract {
        name: "lsp_did_save",
        args: &["documentId"],
        result: "void",
    },
    CommandContract {
        name: "lsp_close_document",
        args: &["documentId"],
        result: "void",
    },
    CommandContract {
        name: "lsp_close_documents",
        args: &["documentIds"],
        result: "void",
    },
    CommandContract {
        name: "lsp_project_candidates",
        args: &["path", "languageId"],
        result: "ProjectCandidate[]",
    },
    CommandContract {
        name: "lsp_set_project_context",
        args: &["documentId", "context"],
        result: "LspStatus",
    },
    CommandContract {
        name: "lsp_set_project_trust",
        args: &["root", "adapterId", "decision"],
        result: "void",
    },
    CommandContract {
        name: "lsp_completion",
        args: &["documentId", "position", "trigger"],
        result: "CompletionResponse",
    },
    CommandContract {
        name: "lsp_hover",
        args: &["documentId", "position"],
        result: "HoverResponse",
    },
    CommandContract {
        name: "lsp_signature_help",
        args: &["documentId", "position", "trigger"],
        result: "SignatureHelpResponse",
    },
    CommandContract {
        name: "lsp_definition",
        args: &["documentId", "position"],
        result: "DefinitionResponse",
    },
    CommandContract {
        name: "lsp_problems_snapshot",
        args: &["root"],
        result: "DiagnosticSnapshot",
    },
    CommandContract {
        name: "lsp_status_snapshot",
        args: &["documentId"],
        result: "LspStatus[]",
    },
    CommandContract {
        name: "lsp_restart_session",
        args: &["adapterId", "root"],
        result: "void",
    },
    CommandContract {
        name: "lsp_session_logs",
        args: &["adapterId", "root"],
        result: "SessionLogEntry[]",
    },
    CommandContract {
        name: "lsp_trusted_projects",
        args: &[],
        result: "TrustedProject[]",
    },
    CommandContract {
        name: "lsp_revoke_project_trust",
        args: &["root", "adapterId"],
        result: "void",
    },
];

#[derive(Clone)]
pub(crate) struct LspState {
    manager: LspManagerHandle,
}

impl LspState {
    pub(crate) fn new(manager: LspManagerHandle) -> Self {
        Self { manager }
    }

    pub(crate) fn manager(&self) -> &LspManagerHandle {
        &self.manager
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DocumentOwnerFocusedPayload {
    document_id: Option<DocumentId>,
    pane_id: Option<String>,
    canonical_path: Option<String>,
    reservation_failed: bool,
}

pub(crate) fn spawn_event_forwarder<R: Runtime>(
    app: tauri::AppHandle<R>,
    mut events: tokio::sync::mpsc::Receiver<ManagerEvent>,
) {
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                ManagerEvent::SessionStatus {
                    window_label,
                    status,
                } => {
                    if let Some(window) = app.get_webview_window(&window_label) {
                        let _ = window.emit(SESSION_STATUS_EVENT, status);
                    }
                }
                ManagerEvent::DiagnosticsUpdated(snapshot) => {
                    let _ = app.emit(DIAGNOSTICS_UPDATED_EVENT, snapshot);
                }
                ManagerEvent::DocumentOwnerFocused {
                    window_label,
                    document_id,
                    pane_id,
                    canonical_path,
                    reservation_failed,
                } => {
                    if let Some(window) = app.get_webview_window(&window_label) {
                        let _ = window.emit(
                            DOCUMENT_OWNER_FOCUSED_EVENT,
                            DocumentOwnerFocusedPayload {
                                document_id,
                                pane_id,
                                canonical_path,
                                reservation_failed,
                            },
                        );
                    }
                }
            }
        }
    });
}

fn command_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn editor_reserve_document(
    path: String,
    window_label: String,
    state: tauri::State<'_, LspState>,
) -> Result<ReserveResult, String> {
    state
        .manager
        .reserve_document(PathBuf::from(path), window_label)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn editor_release_document(
    reservation_id: ReservationId,
    state: tauri::State<'_, LspState>,
) -> Result<(), String> {
    state
        .manager
        .release_document(reservation_id)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn editor_transfer_document(
    document_id: DocumentId,
    target_reservation_id: ReservationId,
    window_label: String,
    pane_id: String,
    state: tauri::State<'_, LspState>,
) -> Result<OpenDocumentResponse, String> {
    state
        .manager
        .transfer_document(document_id, target_reservation_id, window_label, pane_id)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_open_document(
    reservation_id: ReservationId,
    pane_id: String,
    contents: String,
    language_id: String,
    state: tauri::State<'_, LspState>,
) -> Result<OpenDocumentResponse, String> {
    state
        .manager
        .open_document(reservation_id, pane_id, contents, language_id)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_apply_changes(
    document_id: DocumentId,
    batch: LspChangeBatch,
    state: tauri::State<'_, LspState>,
) -> Result<ApplyChangesResponse, String> {
    state
        .manager
        .apply_changes(document_id, batch)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_resync_document(
    document_id: DocumentId,
    version: i32,
    contents: String,
    state: tauri::State<'_, LspState>,
) -> Result<ResyncDocumentResponse, String> {
    state
        .manager
        .resync_document(document_id, version, contents)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_did_save(
    document_id: DocumentId,
    state: tauri::State<'_, LspState>,
) -> Result<(), String> {
    state
        .manager
        .did_save(document_id)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_close_document(
    document_id: DocumentId,
    state: tauri::State<'_, LspState>,
) -> Result<(), String> {
    state
        .manager
        .close_document(document_id)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_close_documents(
    document_ids: Vec<DocumentId>,
    state: tauri::State<'_, LspState>,
) -> Result<(), String> {
    state
        .manager
        .close_documents(document_ids)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_project_candidates(
    path: String,
    language_id: String,
    state: tauri::State<'_, LspState>,
) -> Result<Vec<ProjectCandidate>, String> {
    state
        .manager
        .project_candidates(PathBuf::from(path), language_id)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_set_project_context(
    document_id: DocumentId,
    context: ProjectContextChoice,
    state: tauri::State<'_, LspState>,
) -> Result<LspStatus, String> {
    state
        .manager
        .set_project_context(document_id, context)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_set_project_trust(
    root: String,
    adapter_id: Option<String>,
    decision: TrustDecision,
    state: tauri::State<'_, LspState>,
) -> Result<(), String> {
    state
        .manager
        .set_project_trust(PathBuf::from(root), adapter_id, decision)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_completion(
    document_id: DocumentId,
    position: EditorPosition,
    trigger: Option<String>,
    state: tauri::State<'_, LspState>,
) -> Result<CompletionResponse, String> {
    state
        .manager
        .completion(document_id, position, trigger)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_hover(
    document_id: DocumentId,
    position: EditorPosition,
    state: tauri::State<'_, LspState>,
) -> Result<HoverResponse, String> {
    state
        .manager
        .hover(document_id, position)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_signature_help(
    document_id: DocumentId,
    position: EditorPosition,
    trigger: Option<String>,
    state: tauri::State<'_, LspState>,
) -> Result<SignatureHelpResponse, String> {
    state
        .manager
        .signature_help(document_id, position, trigger)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_definition(
    document_id: DocumentId,
    position: EditorPosition,
    state: tauri::State<'_, LspState>,
) -> Result<DefinitionResponse, String> {
    state
        .manager
        .definition(document_id, position)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_problems_snapshot(
    root: Option<String>,
    state: tauri::State<'_, LspState>,
) -> Result<DiagnosticSnapshot, String> {
    state
        .manager
        .problems_snapshot(root.map(PathBuf::from))
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_status_snapshot(
    document_id: Option<DocumentId>,
    state: tauri::State<'_, LspState>,
) -> Result<Vec<LspStatus>, String> {
    state
        .manager
        .status_snapshot(document_id)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_restart_session(
    adapter_id: String,
    root: String,
    state: tauri::State<'_, LspState>,
) -> Result<(), String> {
    state
        .manager
        .restart_session(adapter_id, PathBuf::from(root))
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_session_logs(
    adapter_id: String,
    root: String,
    state: tauri::State<'_, LspState>,
) -> Result<Vec<SessionLogEntry>, String> {
    state
        .manager
        .session_logs(adapter_id, PathBuf::from(root))
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_trusted_projects(
    state: tauri::State<'_, LspState>,
) -> Result<Vec<TrustedProject>, String> {
    state
        .manager
        .trusted_projects()
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn lsp_revoke_project_trust(
    root: String,
    adapter_id: Option<String>,
    state: tauri::State<'_, LspState>,
) -> Result<(), String> {
    state
        .manager
        .revoke_project_trust(PathBuf::from(root), adapter_id)
        .await
        .map_err(command_error)
}

#[cfg(test)]
mod tests {
    use super::{DocumentOwnerFocusedPayload, LSP_COMMAND_CONTRACTS, LspState};
    use crate::lsp::manager::{
        Enablement, LspManager, ManagerError, ProjectContextChoice, SessionFactory, SessionStart,
    };
    use crate::lsp::trust::TrustDecision;
    use crate::lsp::types::{DocumentId, ReservationId};
    use async_trait::async_trait;
    use std::sync::Arc;
    use ts_rs::TS;

    struct BoundaryFactory;

    #[async_trait]
    impl SessionFactory for BoundaryFactory {
        async fn start(
            self: Arc<Self>,
            _start: SessionStart,
            _events: tokio::sync::mpsc::Sender<crate::lsp::client::ClientEvent>,
            _cancellation: tokio_util::sync::CancellationToken,
        ) -> Result<Arc<dyn crate::lsp::manager::SessionClient>, ManagerError> {
            unreachable!("boundary decoding never starts a session")
        }
    }

    #[test]
    fn project_and_trust_choices_have_stable_camel_case_boundary_shapes() {
        assert_eq!(
            serde_json::to_value(ProjectContextChoice::root("/repo".into())).unwrap(),
            serde_json::json!({ "kind": "root", "root": "/repo" })
        );
        assert_eq!(
            serde_json::to_value(ProjectContextChoice::DeferForSession).unwrap(),
            serde_json::json!({ "kind": "deferForSession" })
        );
        assert_eq!(
            serde_json::to_value(TrustDecision::Trusted).unwrap(),
            serde_json::json!("trusted")
        );
        let declaration = ProjectContextChoice::decl(&ts_rs::Config::default());
        assert!(declaration.contains("deferForSession"));
    }

    #[test]
    fn targeted_owner_event_payload_uses_camel_case_and_opaque_document_id() {
        let document_id = DocumentId::new();
        let value = serde_json::to_value(DocumentOwnerFocusedPayload {
            document_id: Some(document_id),
            pane_id: Some("pane-a".into()),
            canonical_path: Some("/repo/a.ts".into()),
            reservation_failed: false,
        })
        .unwrap();
        assert_eq!(
            value["documentId"],
            serde_json::to_value(document_id).unwrap()
        );
        assert_eq!(value["paneId"], "pane-a");
        assert_eq!(value["canonicalPath"], "/repo/a.ts");
        assert_eq!(value["reservationFailed"], false);
        assert!(value.get("document_id").is_none());
    }

    #[test]
    fn typed_tauri_lsp_state_exists() {
        let _ = std::mem::size_of::<LspState>();
    }

    #[test]
    fn all_twenty_two_invoke_contracts_have_exact_names_args_and_results() {
        let expected = [
            (
                "editor_reserve_document",
                &["path", "windowLabel"][..],
                "ReserveResult",
            ),
            ("editor_release_document", &["reservationId"][..], "void"),
            (
                "editor_transfer_document",
                &["documentId", "targetReservationId", "windowLabel", "paneId"][..],
                "OpenDocumentResponse",
            ),
            (
                "lsp_open_document",
                &["reservationId", "paneId", "contents", "languageId"][..],
                "OpenDocumentResponse",
            ),
            (
                "lsp_apply_changes",
                &["documentId", "batch"][..],
                "ApplyChangesResponse",
            ),
            (
                "lsp_resync_document",
                &["documentId", "version", "contents"][..],
                "ResyncDocumentResponse",
            ),
            ("lsp_did_save", &["documentId"][..], "void"),
            ("lsp_close_document", &["documentId"][..], "void"),
            ("lsp_close_documents", &["documentIds"][..], "void"),
            (
                "lsp_project_candidates",
                &["path", "languageId"][..],
                "ProjectCandidate[]",
            ),
            (
                "lsp_set_project_context",
                &["documentId", "context"][..],
                "LspStatus",
            ),
            (
                "lsp_set_project_trust",
                &["root", "adapterId", "decision"][..],
                "void",
            ),
            (
                "lsp_completion",
                &["documentId", "position", "trigger"][..],
                "CompletionResponse",
            ),
            (
                "lsp_hover",
                &["documentId", "position"][..],
                "HoverResponse",
            ),
            (
                "lsp_signature_help",
                &["documentId", "position", "trigger"][..],
                "SignatureHelpResponse",
            ),
            (
                "lsp_definition",
                &["documentId", "position"][..],
                "DefinitionResponse",
            ),
            ("lsp_problems_snapshot", &["root"][..], "DiagnosticSnapshot"),
            ("lsp_status_snapshot", &["documentId"][..], "LspStatus[]"),
            ("lsp_restart_session", &["adapterId", "root"][..], "void"),
            (
                "lsp_session_logs",
                &["adapterId", "root"][..],
                "SessionLogEntry[]",
            ),
            ("lsp_trusted_projects", &[][..], "TrustedProject[]"),
            (
                "lsp_revoke_project_trust",
                &["root", "adapterId"][..],
                "void",
            ),
        ];
        assert_eq!(LSP_COMMAND_CONTRACTS.len(), 22);
        for (contract, (name, args, result)) in LSP_COMMAND_CONTRACTS.iter().zip(expected) {
            assert_eq!(
                (contract.name, contract.args, contract.result),
                (name, args, result)
            );
            let invoke_args = contract
                .args
                .iter()
                .map(|arg| ((*arg).to_owned(), serde_json::Value::Null))
                .collect::<serde_json::Map<_, _>>();
            assert_eq!(
                invoke_args
                    .keys()
                    .map(String::as_str)
                    .collect::<std::collections::HashSet<_>>(),
                args.iter().copied().collect()
            );
        }
    }

    #[test]
    fn all_twenty_two_commands_decode_and_dispatch_through_the_production_invoke_handler() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let (manager, actor, _events) = LspManager::new(
            Arc::new(BoundaryFactory),
            temp.path().join("config"),
            temp.path().join("cache"),
            Enablement::all(),
        );
        drop(actor);
        let app = tauri::test::mock_builder()
            .manage(LspState::new(manager))
            .invoke_handler(super::invoke_handler())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let document = serde_json::to_value(DocumentId::new()).unwrap();
        let reservation = serde_json::to_value(ReservationId::new()).unwrap();
        let root = root.display().to_string();
        let batch = serde_json::json!({
            "documentId": document,
            "baseVersion": 1,
            "nextVersion": 2,
            "changes": [],
        });
        let position = serde_json::json!({ "line": 0, "character": 0 });
        let cases = [
            (
                "editor_reserve_document",
                serde_json::json!({ "path": root, "windowLabel": "main" }),
            ),
            (
                "editor_release_document",
                serde_json::json!({ "reservationId": reservation }),
            ),
            (
                "editor_transfer_document",
                serde_json::json!({ "documentId": document, "targetReservationId": reservation, "windowLabel": "main", "paneId": "pane" }),
            ),
            (
                "lsp_open_document",
                serde_json::json!({ "reservationId": reservation, "paneId": "pane", "contents": "x", "languageId": "typescript" }),
            ),
            (
                "lsp_apply_changes",
                serde_json::json!({ "documentId": document, "batch": batch }),
            ),
            (
                "lsp_resync_document",
                serde_json::json!({ "documentId": document, "version": 2, "contents": "x" }),
            ),
            (
                "lsp_did_save",
                serde_json::json!({ "documentId": document }),
            ),
            (
                "lsp_close_document",
                serde_json::json!({ "documentId": document }),
            ),
            (
                "lsp_close_documents",
                serde_json::json!({ "documentIds": [document] }),
            ),
            (
                "lsp_project_candidates",
                serde_json::json!({ "path": root, "languageId": "typescript" }),
            ),
            (
                "lsp_set_project_context",
                serde_json::json!({ "documentId": document, "context": { "kind": "disabled" } }),
            ),
            (
                "lsp_set_project_trust",
                serde_json::json!({ "root": root, "adapterId": "typescript", "decision": "revoked" }),
            ),
            (
                "lsp_completion",
                serde_json::json!({ "documentId": document, "position": position, "trigger": null }),
            ),
            (
                "lsp_hover",
                serde_json::json!({ "documentId": document, "position": position }),
            ),
            (
                "lsp_signature_help",
                serde_json::json!({ "documentId": document, "position": position, "trigger": null }),
            ),
            (
                "lsp_definition",
                serde_json::json!({ "documentId": document, "position": position }),
            ),
            ("lsp_problems_snapshot", serde_json::json!({ "root": null })),
            (
                "lsp_status_snapshot",
                serde_json::json!({ "documentId": document }),
            ),
            (
                "lsp_restart_session",
                serde_json::json!({ "adapterId": "typescript", "root": root }),
            ),
            (
                "lsp_session_logs",
                serde_json::json!({ "adapterId": "typescript", "root": root }),
            ),
            ("lsp_trusted_projects", serde_json::json!({})),
            (
                "lsp_revoke_project_trust",
                serde_json::json!({ "root": root, "adapterId": "typescript" }),
            ),
        ];
        assert_eq!(cases.len(), 22);
        for (command, body) in cases {
            let response = tauri::test::get_ipc_response(
                &webview,
                tauri::webview::InvokeRequest {
                    cmd: command.into(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: "http://tauri.localhost".parse().unwrap(),
                    body: tauri::ipc::InvokeBody::Json(body),
                    headers: Default::default(),
                    invoke_key: tauri::test::INVOKE_KEY.into(),
                },
            );
            assert_eq!(
                response.unwrap_err(),
                serde_json::Value::String("the LSP manager is not running".into()),
                "{command} must decode camelCase arguments and dispatch"
            );
        }
    }
}
