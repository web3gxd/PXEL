//! Bounded read-only analysis adapters. Formula/kernel code has no clocks or IO.
use crate::{AppState, auth, documents::ApiError};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use spreadsheet_collab::SessionDirectory;
use spreadsheet_domain::workbook::{CellContent, DocumentId, Workbook};
use spreadsheet_formula::diagnostics::{self, SeriesFormula};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisRequest {
    pub schema_version: u32,
    pub document_version: String,
}

pub(crate) async fn read_book(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    id: &str,
    input: &AnalysisRequest,
    owner: bool,
) -> Result<(auth::Principal, DocumentId, Arc<Workbook>), ApiError> {
    if input.schema_version != 1 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "unknown_analysis_version",
        ));
    }
    if !auth::valid_origin(headers, &state.origins) {
        return Err(ApiError(StatusCode::FORBIDDEN, "origin_denied"));
    }
    if !crate::rate_allowed(state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    let uuid = Uuid::parse_str(id)
        .ok()
        .filter(|id| id.get_version_num() == 7)
        .ok_or(ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))?;
    let document = DocumentId::from_bytes(*uuid.as_bytes())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))?;
    let principal = state
        .auth
        .authenticate(headers)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    if owner
        && state
            .database
            .document_role_name(principal.identity, document)
            .await
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
            .as_deref()
            != Some("owner")
    {
        return Err(ApiError(StatusCode::FORBIDDEN, "owner_required"));
    }
    let session = state
        .sessions
        .session(principal.identity, document)
        .await
        .map_err(crate::documents::session_api_error)?;
    let book = session
        .read_confirmed(principal.identity)
        .await
        .map_err(crate::documents::session_api_error)?;
    if input.document_version != book.version().get().to_string() {
        return Err(ApiError(StatusCode::CONFLICT, "document_changed"));
    }
    Ok((principal, document, Arc::new(book)))
}
pub(crate) async fn deliver<T: Serialize>(
    state: &Arc<AppState>,
    principal: &auth::Principal,
    document: DocumentId,
    value: T,
) -> Result<Response, ApiError> {
    if !matches!(state.auth.active(principal).await, Ok(true)) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let gate = state.delivery.get(document).await;
    let guard = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        gate.lock.clone().read_owned(),
    )
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "send_gate_busy"))?;
    let lease = state
        .database
        .lease_delivery_access(principal.identity, document)
        .await
        .map_err(crate::documents::journal_error)?;
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    crate::disclosure_api::protected_response(bytes, "application/json", None, lease, guard, true)
}
#[derive(Serialize)]
struct WarningView {
    sheet_id: String,
    row_id: String,
    column_id: String,
    row: u32,
    column: u32,
    fingerprint: String,
    series_axis: &'static str,
    formulas: usize,
    dominant: usize,
}
#[derive(Serialize)]
struct DiagnosticsView {
    schema_version: u32,
    document_version: String,
    formulas: usize,
    warnings: Vec<WarningView>,
}
fn hardcodes(book: &Workbook) -> Result<DiagnosticsView, ApiError> {
    let mut owned = Vec::new();
    for sheet in book.sheets() {
        for (address, content) in sheet.cells() {
            if let CellContent::Formula(formula) = content {
                let row = sheet.row_position(address.row).ok_or(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "invalid_projection",
                ))?;
                let column = sheet.column_position(address.column).ok_or(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "invalid_projection",
                ))?;
                let source = spreadsheet_formula::bound::render_current(formula, book)
                    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "invalid_projection"))?;
                owned.push((*address, row, column, source));
            }
        }
    }
    let formulas: Vec<_> = owned
        .iter()
        .map(|(address, row, column, source)| SeriesFormula {
            address: *address,
            row: *row,
            column: *column,
            source,
        })
        .collect();
    let warnings = diagnostics::analyze(&formulas)
        .map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "diagnostics_limit"))?;
    let warnings = warnings
        .into_iter()
        .map(|warning| {
            let sheet = book
                .sheets()
                .iter()
                .find(|sheet| sheet.id() == warning.address.sheet)
                .ok_or(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "invalid_projection",
                ))?;
            Ok(WarningView {
                sheet_id: Uuid::from_bytes(*warning.address.sheet.as_bytes()).to_string(),
                row_id: Uuid::from_bytes(*warning.address.row.as_bytes()).to_string(),
                column_id: Uuid::from_bytes(*warning.address.column.as_bytes()).to_string(),
                row: sheet.row_position(warning.address.row).ok_or(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "invalid_projection",
                ))?,
                column: sheet
                    .column_position(warning.address.column)
                    .ok_or(ApiError(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "invalid_projection",
                    ))?,
                fingerprint: warning
                    .fingerprint
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                series_axis: if warning.by_row { "row" } else { "column" },
                formulas: warning.formulas,
                dominant: warning.dominant,
            })
        })
        .collect::<Result<_, ApiError>>()?;
    Ok(DiagnosticsView {
        schema_version: 1,
        document_version: book.version().get().to_string(),
        formulas: formulas.len(),
        warnings,
    })
}
pub async fn diagnostics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<AnalysisRequest>,
) -> Result<Response, ApiError> {
    let (principal, document, book) = read_book(&state, &headers, &id, &input, false).await?;
    let slot = state
        .history_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "analysis_busy"))?;
    let result = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        hardcodes(&book)
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "analysis_failed"))??;
    deliver(&state, &principal, document, result).await
}
