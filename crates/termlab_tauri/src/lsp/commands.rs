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
                    reservation_failed,
                } => {
                    if let Some(window) = app.get_webview_window(&window_label) {
                        let _ = window.emit(
                            DOCUMENT_OWNER_FOCUSED_EVENT,
                            DocumentOwnerFocusedPayload {
                                document_id,
                                pane_id,
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
) -> Result<(), String> {
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
    use super::{DocumentOwnerFocusedPayload, LspState};
    use crate::lsp::manager::ProjectContextChoice;
    use crate::lsp::trust::TrustDecision;
    use crate::lsp::types::DocumentId;
    use ts_rs::TS;

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
            reservation_failed: false,
        })
        .unwrap();
        assert_eq!(
            value["documentId"],
            serde_json::to_value(document_id).unwrap()
        );
        assert_eq!(value["paneId"], "pane-a");
        assert_eq!(value["reservationFailed"], false);
        assert!(value.get("document_id").is_none());
    }

    #[test]
    fn typed_tauri_lsp_state_exists() {
        let _ = std::mem::size_of::<LspState>();
    }
}
