use crate::{AppState, auth};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spreadsheet_application::journal::{JournalError, OperationJournal};
use spreadsheet_collab::{SessionDirectory, SessionError, Submission};
use spreadsheet_domain::workbook::*;
use spreadsheet_storage::{ExportAuditError, PermissionError, PermissionRole, SnapshotError};
use spreadsheet_wire_protocol::{self as wire, v1};
use spreadsheet_xlsx::{NormalizedCell, NormalizedCellValue, NormalizedSheet, NormalizedWorkbook};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRequest {
    document_id: String,
    operation_id: String,
}

#[derive(Serialize)]
pub struct Created {
    document_id: String,
    version: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecalculateRequest {
    operation_id: String,
}

#[derive(Serialize)]
pub struct RecalculateReceipt {
    operation_id: String,
    committed_version: String,
    current_head_version: String,
    canonical_event_hash: String,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum PermissionChangeRequest {
    Grant {
        change_id: String,
        target_user_id: String,
        role: PermissionRoleInput,
    },
    Revoke {
        change_id: String,
        target_user_id: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRoleInput {
    Editor,
    Viewer,
}

#[derive(Serialize)]
pub struct PermissionChangeView {
    change_id: String,
    access_epoch: u64,
    newly_committed: bool,
}

pub async fn change_permission(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<PermissionChangeRequest>,
) -> Result<Json<PermissionChangeView>, ApiError> {
    if !auth::valid_origin(&headers, &state.origins) {
        return Err(ApiError(StatusCode::FORBIDDEN, "origin_denied"));
    }
    if !crate::rate_allowed(&state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limited"));
    }
    let principal = principal(&state, &headers).await?;
    let document = DocumentId::from_bytes(parse_id(&id)?).map_err(domain_error)?;
    let (change_id, target, role) = match input {
        PermissionChangeRequest::Grant {
            change_id,
            target_user_id,
            role,
        } => (
            change_id,
            target_user_id,
            Some(match role {
                PermissionRoleInput::Editor => PermissionRole::Editor,
                PermissionRoleInput::Viewer => PermissionRole::Viewer,
            }),
        ),
        PermissionChangeRequest::Revoke {
            change_id,
            target_user_id,
        } => (change_id, target_user_id, None),
    };
    let change_id =
        AuthorizationChangeId::from_bytes(parse_id(&change_id)?).map_err(domain_error)?;
    let target = ActorId::from_bytes(parse_id(&target)?).map_err(domain_error)?;
    let gate = state.delivery.get(document).await;
    let _write = tokio::time::timeout(std::time::Duration::from_secs(3), gate.lock.write())
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "send_gate_busy"))?;
    let receipt = state
        .database
        .change_permission(principal.identity, document, target, change_id, role)
        .await
        .map_err(|error| match error {
            PermissionError::Unauthorized | PermissionError::OwnerImmutable => {
                ApiError(StatusCode::FORBIDDEN, "unauthorized")
            }
            PermissionError::NotFound => ApiError(StatusCode::NOT_FOUND, "not_found"),
            PermissionError::IdempotencyConflict => {
                ApiError(StatusCode::CONFLICT, "idempotency_conflict")
            }
            PermissionError::Conflict => ApiError(StatusCode::CONFLICT, "retryable_conflict"),
            PermissionError::Unavailable | PermissionError::Corrupt => {
                ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
            }
        })?;
    if receipt.newly_committed {
        gate.committed_epoch(receipt.access_epoch);
    }
    Ok(Json(PermissionChangeView {
        change_id: id_string(receipt.change_id.as_bytes()),
        access_epoch: receipt.access_epoch,
        newly_committed: receipt.newly_committed,
    }))
}

pub async fn recalculate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<RecalculateRequest>,
) -> Result<Json<RecalculateReceipt>, ApiError> {
    if !auth::valid_origin(&headers, &state.origins) {
        return Err(ApiError(StatusCode::FORBIDDEN, "origin_denied"));
    }
    if !crate::rate_allowed(&state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    let principal = principal(&state, &headers).await?;
    let document_id = DocumentId::from_bytes(parse_id(&id)?).map_err(domain_error)?;
    let operation_id =
        OperationId::from_bytes(parse_id(&input.operation_id)?).map_err(domain_error)?;
    let (_, context) = crate::calculation_context::sample(CalculationReason::Explicit)
        .map_err(session_api_error)?;
    let session = state
        .sessions
        .session(principal.identity, document_id)
        .await
        .map_err(session_api_error)?;
    let receipt = session
        .advance_context(principal.identity, operation_id, context)
        .await
        .map_err(session_api_error)?;
    Ok(Json(RecalculateReceipt {
        operation_id: id_string(receipt.operation_id.as_bytes()),
        committed_version: receipt.committed_version.get().to_string(),
        current_head_version: receipt.current_head_version.get().to_string(),
        canonical_event_hash: receipt
            .canonical_event_hash
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    }))
}

pub struct ApiError(pub StatusCode, pub &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct ErrorBody {
            code: &'static str,
        }
        (self.0, Json(ErrorBody { code: self.1 })).into_response()
    }
}

async fn principal(state: &AppState, headers: &HeaderMap) -> Result<auth::Principal, ApiError> {
    state
        .auth
        .authenticate(headers)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"))
}

fn parse_id(value: &str) -> Result<[u8; 16], ApiError> {
    let id = Uuid::parse_str(value)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))?;
    if id.get_version_num() != 7 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_identity"));
    }
    Ok(*id.as_bytes())
}

fn domain_error(_: DomainError) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "invalid_command")
}
fn journal_error(error: JournalError) -> ApiError {
    match error {
        JournalError::Unauthorized => ApiError(StatusCode::FORBIDDEN, "unauthorized"),
        JournalError::NotFound => ApiError(StatusCode::NOT_FOUND, "not_found"),
        JournalError::Conflict => ApiError(StatusCode::CONFLICT, "retryable_conflict"),
        JournalError::IdempotencyConflict => ApiError(StatusCode::CONFLICT, "idempotency_conflict"),
        _ => ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    }
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<CreateRequest>,
) -> Result<(StatusCode, Json<Created>), ApiError> {
    if !auth::valid_origin(&headers, &state.origins) {
        return Err(ApiError(StatusCode::FORBIDDEN, "origin_denied"));
    }
    if !crate::rate_allowed(&state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    let principal = principal(&state, &headers).await?;
    let document_id =
        DocumentId::from_bytes(parse_id(&input.document_id)?).map_err(domain_error)?;
    let operation_id =
        OperationId::from_bytes(parse_id(&input.operation_id)?).map_err(domain_error)?;
    let sheet = SheetSeed {
        id: SheetId::from_bytes(*Uuid::now_v7().as_bytes()).map_err(domain_error)?,
        name: "Лист 1".into(),
        rows: (0..1000)
            .map(|_| RowId::from_bytes(*Uuid::now_v7().as_bytes()))
            .collect::<Result<_, _>>()
            .map_err(domain_error)?,
        columns: (0..26)
            .map(|_| ColumnId::from_bytes(*Uuid::now_v7().as_bytes()))
            .collect::<Result<_, _>>()
            .map_err(domain_error)?,
    };
    let version = DocumentVersion::new(1).map_err(domain_error)?;
    let operation = Operation::CreateWorkbook { sheet };
    let candidate = Workbook::empty(document_id)
        .candidate(&operation, version)
        .map_err(domain_error)?;
    let event = CommittedEvent {
        document_id,
        operation_id,
        principal: EventPrincipal::User(principal.identity.actor_id),
        version,
        operation,
        state_hash: candidate.hash(),
    };
    let receipt = state
        .database
        .create(principal.identity, event)
        .await
        .map_err(journal_error)?;
    Ok((
        StatusCode::CREATED,
        Json(Created {
            document_id: id_string(document_id.as_bytes()),
            version: receipt.current_head_version.get().to_string(),
        }),
    ))
}

#[derive(Serialize)]
pub struct DocumentView {
    authored_projection_schema_version: u32,
    style_schema_version: u32,
    style_profile_version: u32,
    document_id: String,
    version: String,
    state_hash: String,
    last_axis_change_version: String,
    retired_row_ids: Vec<String>,
    retired_column_ids: Vec<String>,
    row_activation_floors: Vec<RowActivationFloorView>,
    column_activation_floors: Vec<ColumnActivationFloorView>,
    calculation_context: Option<CalculationContextView>,
    sheets: Vec<SheetView>,
}
#[derive(Serialize)]
struct RowActivationFloorView {
    row_id: String,
    version: String,
}
#[derive(Serialize)]
struct ColumnActivationFloorView {
    column_id: String,
    version: String,
}
#[derive(Serialize)]
struct CalculationContextView {
    schema_version: u32,
    context_id: String,
    epoch: String,
    effective_timestamp_utc_ms: String,
    utc_offset_minutes: i16,
    civil_day_since_unix_epoch: i32,
    random_seed: String,
    engine_version: String,
    compatibility_profile: String,
    reason: &'static str,
}
#[derive(Serialize)]
struct SheetView {
    id: String,
    name: String,
    rows: Vec<String>,
    columns: Vec<String>,
    cells: Vec<CellView>,
    styles: Vec<StyledCellView>,
}
#[derive(Serialize)]
struct StyledCellView {
    row_id: String,
    column_id: String,
    style: CellStyleView,
}
#[derive(Serialize)]
struct CellStyleView {
    font_family: String,
    font_size_hundredth_points: u16,
    font_bold: bool,
    font_italic: bool,
    font_underline: UnderlineView,
    font_strike: bool,
    font_color: [u8; 3],
    fill: [u8; 3],
    border_top: Option<BorderView>,
    border_right: Option<BorderView>,
    border_bottom: Option<BorderView>,
    border_left: Option<BorderView>,
    horizontal: HorizontalView,
    vertical: VerticalView,
    wrap: bool,
    number_format: NumberFormatView,
}
#[derive(Serialize)]
struct BorderView {
    line: BorderLineView,
    color: [u8; 3],
}
macro_rules! style_json_enum {
    ($view:ident, $domain:ident, $($variant:ident),+ $(,)?) => {
        #[derive(Serialize)]
        #[serde(rename_all = "snake_case")]
        enum $view { $($variant),+ }
        impl From<spreadsheet_domain::style::$domain> for $view {
            fn from(value: spreadsheet_domain::style::$domain) -> Self {
                match value { $(spreadsheet_domain::style::$domain::$variant => Self::$variant),+ }
            }
        }
    };
}
style_json_enum!(UnderlineView, Underline, None, Single, Double);
style_json_enum!(
    BorderLineView,
    BorderLine,
    Thin,
    Medium,
    Thick,
    Dashed,
    Dotted,
    Double
);
style_json_enum!(
    HorizontalView,
    HorizontalAlignment,
    General,
    Left,
    Center,
    Right
);
style_json_enum!(VerticalView, VerticalAlignment, Top, Center, Bottom);
style_json_enum!(
    NumberFormatView,
    NumberFormat,
    General,
    Integer,
    Decimal2,
    Percent0,
    Percent2,
    Scientific2,
    DateIso
);
impl From<spreadsheet_domain::style::Border> for BorderView {
    fn from(value: spreadsheet_domain::style::Border) -> Self {
        Self {
            line: value.line().into(),
            color: value.color().channels(),
        }
    }
}
impl From<&spreadsheet_domain::style::CellStyle> for CellStyleView {
    fn from(value: &spreadsheet_domain::style::CellStyle) -> Self {
        Self {
            font_family: value.font_family().to_owned(),
            font_size_hundredth_points: value.font_size().get(),
            font_bold: value.font_bold(),
            font_italic: value.font_italic(),
            font_underline: value.font_underline().into(),
            font_strike: value.font_strike(),
            font_color: value.font_color().channels(),
            fill: value.fill().channels(),
            border_top: value.border_top().map(Into::into),
            border_right: value.border_right().map(Into::into),
            border_bottom: value.border_bottom().map(Into::into),
            border_left: value.border_left().map(Into::into),
            horizontal: value.horizontal().into(),
            vertical: value.vertical().into(),
            wrap: value.wrap(),
            number_format: value.number_format().into(),
        }
    }
}
#[derive(Serialize)]
struct CellView {
    row_id: String,
    column_id: String,
    value: JsonValue,
}
#[derive(Serialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
enum JsonValue {
    Blank,
    Number(f64),
    Text(String),
    Boolean(bool),
    Formula(JsonFormula),
}

#[derive(Serialize)]
struct JsonFormula {
    source: String,
    canonical: String,
    current_text: String,
    ast_schema: u32,
    compatibility_profile: &'static str,
    bound_at_version: String,
    bound_formula_wire_hex: String,
}

pub async fn get_document(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<DocumentView>, ApiError> {
    if !crate::rate_allowed(&state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    let principal = principal(&state, &headers).await?;
    let id = DocumentId::from_bytes(parse_id(&id)?).map_err(domain_error)?;
    let session = state
        .sessions
        .session(principal.identity, id)
        .await
        .map_err(session_api_error)?;
    let workbook = session
        .read(principal.identity)
        .await
        .map_err(session_api_error)?;
    document_view(id, &workbook)
}

#[derive(Serialize)]
pub struct UndoHistoryView {
    schema_version: u32,
    document_id: String,
    actor_id: String,
    through_version: String,
    earliest_scanned_version: String,
    undo: Vec<HistoryTargetView>,
    redo: Vec<HistoryTargetView>,
    truncated: bool,
    truncation_reason: Option<&'static str>,
}

#[derive(Serialize)]
struct HistoryTargetView {
    operation_id: String,
    committed_version: String,
}

fn history_target_view(
    target: spreadsheet_application::history::HistoryTarget,
) -> HistoryTargetView {
    HistoryTargetView {
        operation_id: id_string(target.operation_id.as_bytes()),
        committed_version: target.committed_version.get().to_string(),
    }
}

pub async fn undo_history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<UndoHistoryView>, ApiError> {
    if !crate::rate_allowed(&state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    if !query.is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "unsupported_history_query",
        ));
    }
    let principal = principal(&state, &headers).await?;
    let document = DocumentId::from_bytes(parse_id(&id)?).map_err(domain_error)?;
    let _permit = state
        .history_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "history_busy"))?;
    let history = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state.database.undo_history(principal.identity, document),
    )
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "history_timeout"))?
    .map_err(journal_error)?;
    if !state
        .auth
        .active(&principal)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
    {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    use spreadsheet_application::history::HistoryTruncation;
    Ok(Json(UndoHistoryView {
        schema_version: 1,
        document_id: id_string(history.document_id.as_bytes()),
        actor_id: id_string(history.actor_id.as_bytes()),
        through_version: history.through_version.get().to_string(),
        earliest_scanned_version: history.earliest_scanned_version.get().to_string(),
        undo: history.undo.into_iter().map(history_target_view).collect(),
        redo: history.redo.into_iter().map(history_target_view).collect(),
        truncated: history.truncation.is_some(),
        truncation_reason: history.truncation.map(|reason| match reason {
            HistoryTruncation::VersionWindow => "version_window",
            HistoryTruncation::ByteBudget => "byte_budget",
        }),
    }))
}

pub async fn export_xlsx(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    if !crate::rate_allowed(&state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    let principal = principal(&state, &headers).await?;
    let document = DocumentId::from_bytes(parse_id(&id)?).map_err(domain_error)?;
    state
        .database
        .authorize_export(principal.identity, document)
        .await
        .map_err(export_audit_api_error)?;
    let permit = state
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "export_busy"))?;
    let session = state
        .sessions
        .session(principal.identity, document)
        .await
        .map_err(session_api_error)?;
    let workbook = session
        .read(principal.identity)
        .await
        .map_err(session_api_error)?;
    let version = workbook.version();
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let normalized = export_projection(&workbook)?;
        spreadsheet_xlsx::export_scalar_xlsx(&normalized).map_err(|_| {
            ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "export_profile_unsupported",
            )
        })
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "export_unavailable"))??;
    let checksum: [u8; 32] = Sha256::digest(&bytes).into();
    state
        .database
        .record_export(principal.identity, document, version, checksum, bytes.len())
        .await
        .map_err(export_audit_api_error)?;
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=PXEL.xlsx",
            ),
            (axum::http::header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response())
}

fn export_audit_api_error(error: ExportAuditError) -> ApiError {
    match error {
        ExportAuditError::Unauthorized => ApiError(StatusCode::FORBIDDEN, "unauthorized"),
        ExportAuditError::Unavailable => {
            ApiError(StatusCode::SERVICE_UNAVAILABLE, "export_audit_unavailable")
        }
    }
}

fn export_projection(workbook: &Workbook) -> Result<NormalizedWorkbook, ApiError> {
    if workbook
        .sheets()
        .iter()
        .any(|sheet| !sheet.styles().is_empty())
    {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "xlsx_styles_not_supported",
        ));
    }
    let mut sheets = Vec::with_capacity(workbook.sheets().len());
    for sheet in workbook.sheets() {
        let mut cells = Vec::with_capacity(sheet.cells().len());
        for (address, content) in sheet.cells() {
            let row = sheet
                .row_position(address.row)
                .and_then(|position| position.checked_add(1))
                .ok_or(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "export_projection",
                ))?;
            let column = sheet
                .column_position(address.column)
                .and_then(|position| position.checked_add(1))
                .ok_or(ApiError(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "export_projection",
                ))?;
            let value = match content {
                CellContent::Value(CellValue::Blank) => continue,
                CellContent::Value(CellValue::Number(value)) => NormalizedCellValue::Number(*value),
                CellContent::Value(CellValue::Text(value)) => {
                    NormalizedCellValue::Text(value.clone())
                }
                CellContent::Value(CellValue::Boolean(value)) => {
                    NormalizedCellValue::Boolean(*value)
                }
                CellContent::Formula(formula) => {
                    let current = spreadsheet_formula::bound::render_current(formula, workbook)
                        .map_err(|_| {
                            ApiError(
                                StatusCode::UNPROCESSABLE_ENTITY,
                                "export_profile_unsupported",
                            )
                        })?;
                    let source = current.strip_prefix('=').ok_or(ApiError(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "export_projection",
                    ))?;
                    NormalizedCellValue::Formula(source.to_owned())
                }
            };
            cells.push(NormalizedCell { row, column, value });
        }
        cells.sort_unstable_by_key(|cell| (cell.row, cell.column));
        sheets.push(NormalizedSheet {
            name: sheet.name().to_owned(),
            cells,
        });
    }
    let normalized = NormalizedWorkbook {
        sheets,
        issues: vec![],
    };
    normalized.validate().map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "export_profile_unsupported",
        )
    })?;
    Ok(normalized)
}

pub async fn get_document_version(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((id, version)): Path<(String, String)>,
) -> Result<Json<DocumentView>, ApiError> {
    if !crate::rate_allowed(&state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    let principal = principal(&state, &headers).await?;
    let id = DocumentId::from_bytes(parse_id(&id)?).map_err(domain_error)?;
    let version = version
        .parse::<u64>()
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_version"))?;
    let version = DocumentVersion::new(version)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_version"))?;
    let _permit = state
        .history_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "history_busy"))?;
    let recovered = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state
            .database
            .recover_document_at_version(&state.blobs, principal.identity, id, version),
    )
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "history_timeout"))?
    .map_err(snapshot_api_error)?;
    document_view(id, &recovered.workbook)
}

fn snapshot_api_error(error: SnapshotError) -> ApiError {
    match error {
        SnapshotError::VersionNotCommitted => {
            ApiError(StatusCode::NOT_FOUND, "version_not_committed")
        }
        SnapshotError::Journal(error) => journal_error(error),
        _ => ApiError(StatusCode::SERVICE_UNAVAILABLE, "history_unavailable"),
    }
}

fn document_view(id: DocumentId, workbook: &Workbook) -> Result<Json<DocumentView>, ApiError> {
    let sheets = workbook
        .sheets()
        .iter()
        .map(|sheet| -> Result<SheetView, ApiError> {
            Ok(SheetView {
                id: id_string(sheet.id().as_bytes()),
                name: sheet.name().to_owned(),
                rows: sheet
                    .rows()
                    .iter()
                    .map(|id| id_string(id.as_bytes()))
                    .collect(),
                columns: sheet
                    .columns()
                    .iter()
                    .map(|id| id_string(id.as_bytes()))
                    .collect(),
                styles: sheet
                    .styles()
                    .iter()
                    .map(|(address, style)| StyledCellView {
                        row_id: id_string(address.row.as_bytes()),
                        column_id: id_string(address.column.as_bytes()),
                        style: style.into(),
                    })
                    .collect(),
                cells: sheet
                    .cells()
                    .iter()
                    .map(|(address, value)| -> Result<CellView, ApiError> {
                        Ok(CellView {
                            row_id: id_string(address.row.as_bytes()),
                            column_id: id_string(address.column.as_bytes()),
                            value: match value {
                                CellContent::Value(CellValue::Blank) => JsonValue::Blank,
                                CellContent::Value(CellValue::Number(n)) => JsonValue::Number(*n),
                                CellContent::Value(CellValue::Text(s)) => {
                                    JsonValue::Text(s.clone())
                                }
                                CellContent::Value(CellValue::Boolean(b)) => JsonValue::Boolean(*b),
                                CellContent::Formula(formula) => JsonValue::Formula(JsonFormula {
                                    bound_formula_wire_hex: prost::Message::encode_to_vec(
                                        &wire::formula::encode_formula(formula),
                                    )
                                    .iter()
                                    .map(|byte| format!("{byte:02x}"))
                                    .collect(),
                                    source: formula.source().to_owned(),
                                    canonical: formula.canonical().to_owned(),
                                    current_text: spreadsheet_formula::bound::render_current(
                                        formula, workbook,
                                    )
                                    .map_err(|_| {
                                        ApiError(
                                            StatusCode::INTERNAL_SERVER_ERROR,
                                            "formula_projection",
                                        )
                                    })?,
                                    ast_schema: formula.ast().schema_version(),
                                    compatibility_profile: formula.profile().identifier(),
                                    bound_at_version: formula.bound_at_version().get().to_string(),
                                }),
                            },
                        })
                    })
                    .collect::<Result<Vec<_>, ApiError>>()?,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(Json(DocumentView {
        authored_projection_schema_version: 4,
        style_schema_version: wire::style::STYLE_SCHEMA_VERSION,
        style_profile_version: wire::style::STYLE_PROFILE_VERSION,
        document_id: id_string(id.as_bytes()),
        version: workbook.version().get().to_string(),
        last_axis_change_version: workbook.last_axis_change_version().get().to_string(),
        retired_row_ids: workbook
            .retired_row_ids()
            .map(|row| id_string(row.as_bytes()))
            .collect(),
        retired_column_ids: workbook
            .retired_column_ids()
            .map(|column| id_string(column.as_bytes()))
            .collect(),
        row_activation_floors: workbook
            .row_activation_floors()
            .map(|(row, version)| RowActivationFloorView {
                row_id: id_string(row.as_bytes()),
                version: version.get().to_string(),
            })
            .collect(),
        column_activation_floors: workbook
            .column_activation_floors()
            .map(|(column, version)| ColumnActivationFloorView {
                column_id: id_string(column.as_bytes()),
                version: version.get().to_string(),
            })
            .collect(),
        state_hash: workbook
            .hash()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        calculation_context: workbook
            .calculation_context()
            .map(|context| CalculationContextView {
                schema_version: 1,
                context_id: id_string(context.context_id().as_bytes()),
                epoch: context.epoch().to_string(),
                effective_timestamp_utc_ms: context.effective_timestamp_utc_ms().to_string(),
                utc_offset_minutes: context.utc_offset_minutes(),
                civil_day_since_unix_epoch: context.civil_day_since_unix_epoch(),
                random_seed: context
                    .random_seed()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
                engine_version: context.engine_version().to_owned(),
                compatibility_profile: context.compatibility_profile().to_owned(),
                reason: match context.reason() {
                    CalculationReason::Open => "open",
                    CalculationReason::Explicit => "explicit",
                    CalculationReason::Timer => "timer",
                    CalculationReason::VolatileDependency => "volatile_dependency",
                    CalculationReason::EngineUpgrade => "engine_upgrade",
                },
            }),
        sheets,
    }))
}

fn session_api_error(error: SessionError) -> ApiError {
    match error {
        SessionError::Journal(error) => journal_error(error),
        SessionError::QueueFull => ApiError(StatusCode::TOO_MANY_REQUESTS, "queue_full"),
        _ => ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    }
}

#[derive(Serialize)]
struct CalculationView {
    document_id: String,
    input_version: String,
    engine_version: &'static str,
    compatibility_profile: &'static str,
    calculation_epoch: String,
    context_id: String,
    status: &'static str,
    calculated_state_hash: Option<String>,
    values: Vec<CalculatedCellView>,
    error_code: Option<&'static str>,
    evaluated_cells: usize,
}
#[derive(Serialize)]
struct CalculatedCellView {
    sheet_id: String,
    row_id: String,
    column_id: String,
    value: CalculatedValue,
}
#[derive(Serialize)]
#[serde(tag = "type", content = "value", rename_all = "lowercase")]
enum CalculatedValue {
    Blank,
    Number(f64),
    Text(String),
    Boolean(bool),
    Error(&'static str),
    DateTime {
        serial: f64,
        system: &'static str,
    },
    Array {
        rows: u32,
        columns: u32,
        values: Vec<CalculatedValue>,
    },
}
impl From<&spreadsheet_formula::Value> for CalculatedValue {
    fn from(value: &spreadsheet_formula::Value) -> Self {
        use spreadsheet_formula::Value;
        match value {
            Value::Blank => Self::Blank,
            Value::Number(n) => Self::Number(*n),
            Value::Text(text) => Self::Text(text.clone()),
            Value::Boolean(value) => Self::Boolean(*value),
            Value::Error(error) => Self::Error(error.code()),
            Value::DateTime { serial, system } => Self::DateTime {
                serial: *serial,
                system: match system {
                    spreadsheet_formula::DateSystem::Excel1900 => "1900",
                    spreadsheet_formula::DateSystem::Excel1904 => "1904",
                },
            },
            Value::Array {
                rows,
                columns,
                values,
            } => Self::Array {
                rows: *rows,
                columns: *columns,
                values: values.iter().map(Self::from).collect(),
            },
        }
    }
}

pub async fn get_calculation(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    if !crate::rate_allowed(&state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    let principal = principal(&state, &headers).await?;
    let id = DocumentId::from_bytes(parse_id(&id)?).map_err(domain_error)?;
    let session = state
        .sessions
        .session(principal.identity, id)
        .await
        .map_err(session_api_error)?;
    let Some(calculated) = session
        .calculated(principal.identity)
        .await
        .map_err(session_api_error)?
    else {
        return Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"status":"pending"})),
        )
            .into_response());
    };
    // Large result encoding runs outside the async executor. Canonical input is immutable.
    let encoder = state
        .calculation_encoders
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "calculation_encoding_busy"))?;
    let body = tokio::task::spawn_blocking(move || {
        let _encoder = encoder;
        use spreadsheet_collab::calculation::{
            COMPATIBILITY_PROFILE, CalculationOutcome, ENGINE_VERSION,
        };
        let mut view = CalculationView {
            document_id: id_string(calculated.document_id.as_bytes()),
            input_version: calculated.input_version.get().to_string(),
            engine_version: ENGINE_VERSION,
            compatibility_profile: COMPATIBILITY_PROFILE,
            calculation_epoch: calculated
                .context
                .as_ref()
                .map_or(0, CalculationContext::epoch)
                .to_string(),
            context_id: calculated.context.as_ref().map_or_else(
                || id_string(calculated.document_id.as_bytes()),
                |context| id_string(context.context_id().as_bytes()),
            ),
            status: "failed",
            calculated_state_hash: None,
            values: Vec::new(),
            error_code: None,
            evaluated_cells: 0,
        };
        match &calculated.outcome {
            CalculationOutcome::Complete {
                results,
                hash,
                evaluated_cells,
            } => {
                view.status = "complete";
                view.calculated_state_hash =
                    Some(hash.iter().map(|byte| format!("{byte:02x}")).collect());
                view.evaluated_cells = *evaluated_cells;
                view.values = results
                    .iter()
                    .map(|(address, value)| CalculatedCellView {
                        sheet_id: id_string(address.sheet.as_bytes()),
                        row_id: id_string(address.row.as_bytes()),
                        column_id: id_string(address.column.as_bytes()),
                        value: value.into(),
                    })
                    .collect();
            }
            CalculationOutcome::Failed(error) => {
                use spreadsheet_formula::{EvaluationError, calculation::CalculationError};
                view.error_code = Some(match error {
                    CalculationError::DependencyLimit
                    | CalculationError::Evaluation(EvaluationError::CalculationLimitExceeded) => {
                        "CalculationLimitExceeded"
                    }
                    CalculationError::Evaluation(EvaluationError::Cancelled) => "Cancelled",
                    CalculationError::Evaluation(
                        EvaluationError::UnsupportedCompatibilityFeature,
                    ) => "UnsupportedCompatibilityFeature",
                    _ => "CalculationFailed",
                });
            }
        }
        serde_json::to_vec(&view)
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "calculation_unavailable"))?
    .map_err(|_| {
        ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "calculation_encoding_failed",
        )
    })?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response())
}
fn id_string(bytes: &[u8; 16]) -> String {
    Uuid::from_bytes(*bytes).to_string()
}
fn bytes_id(bytes: &[u8]) -> Result<[u8; 16], SessionError> {
    bytes.try_into().map_err(|_| SessionError::InvalidCommand)
}

fn address(input: Option<v1::CellAddress>) -> Result<CellAddress, SessionError> {
    let input = input.ok_or(SessionError::InvalidCommand)?;
    Ok(CellAddress {
        sheet: SheetId::from_bytes(bytes_id(&input.sheet_id)?)?,
        row: RowId::from_bytes(bytes_id(&input.row_id)?)?,
        column: ColumnId::from_bytes(bytes_id(&input.column_id)?)?,
    })
}

fn command(input: v1::DocumentCommand) -> Result<Command, SessionError> {
    if usize::from(input.operation.is_some())
        + usize::from(input.formula_command.is_some())
        + usize::from(input.undo_cell.is_some())
        + usize::from(input.copy_range.is_some())
        + usize::from(input.move_range.is_some())
        + usize::from(input.set_cell_style.is_some())
        + usize::from(input.set_range_style.is_some())
        != 1
    {
        return Err(SessionError::InvalidCommand);
    }
    let operation = if let Some(style) = input.set_cell_style {
        CommandPayload::SetCellStyle {
            address: address(style.address).map_err(|_| SessionError::InvalidCommand)?,
            changes: style_patch(style.patch)?,
        }
    } else if let Some(style) = input.set_range_style {
        if style.addresses.is_empty() || style.addresses.len() > 1000 {
            return Err(SessionError::InvalidCommand);
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut sheet = None;
        let mut addresses = Vec::with_capacity(style.addresses.len());
        for target in style.addresses {
            let target = address(Some(target)).map_err(|_| SessionError::InvalidCommand)?;
            if sheet.is_some_and(|id| id != target.sheet) || !seen.insert(target) {
                return Err(SessionError::InvalidCommand);
            }
            sheet = Some(target.sheet);
            addresses.push(target);
        }
        CommandPayload::SetRangeStyle {
            addresses,
            changes: style_patch(style.patch)?,
        }
    } else if let Some(formula) = input.formula_command {
        CommandPayload::SetCellFormula {
            address: address(formula.address)?,
            source: FormulaSource::new(formula.source).map_err(|_| SessionError::InvalidFormula)?,
        }
    } else if let Some(undo) = input.undo_cell {
        CommandPayload::UndoCell {
            target_operation_id: OperationId::from_bytes(bytes_id(&undo.target_operation_id)?)?,
        }
    } else if let Some(copy) = input.copy_range {
        CommandPayload::CopyRange {
            source: address(copy.source)?,
            destination: address(copy.destination)?,
            rows: copy.rows,
            columns: copy.columns,
        }
    } else if let Some(moved) = input.move_range {
        let source = address(moved.source)?;
        let destination = address(moved.destination)?;
        if source.sheet != destination.sheet
            || source == destination
            || moved.rows == 0
            || moved.columns == 0
            || u64::from(moved.rows) * u64::from(moved.columns) > 1000
        {
            return Err(SessionError::InvalidCommand);
        }
        CommandPayload::MoveRange {
            source,
            destination,
            rows: moved.rows,
            columns: moved.columns,
        }
    } else {
        match input
            .operation
            .and_then(|op| op.kind)
            .ok_or(SessionError::InvalidCommand)?
        {
            v1::canonical_operation::Kind::SetCellValue(set) => {
                let value = match set
                    .value
                    .and_then(|value| value.kind)
                    .ok_or(SessionError::InvalidCommand)?
                {
                    v1::cell_value::Kind::Blank(true) => CellValue::Blank,
                    v1::cell_value::Kind::Blank(false) => return Err(SessionError::InvalidCommand),
                    v1::cell_value::Kind::Number(n) => CellValue::Number(n),
                    v1::cell_value::Kind::Text(s) => CellValue::Text(s),
                    v1::cell_value::Kind::Boolean(b) => CellValue::Boolean(b),
                }
                .normalized()?;
                CommandPayload::SetCellValue {
                    address: address(set.address)?,
                    value,
                }
            }
            v1::canonical_operation::Kind::SetCellValues(batch) => {
                if batch.writes.is_empty() || batch.writes.len() > 1_000 {
                    return Err(SessionError::InvalidCommand);
                }
                let mut seen = std::collections::BTreeSet::new();
                let mut writes = Vec::with_capacity(batch.writes.len());
                for write in batch.writes {
                    let cell = address(write.address)?;
                    if !seen.insert(cell) {
                        return Err(SessionError::InvalidCommand);
                    }
                    let value = match write
                        .value
                        .and_then(|value| value.kind)
                        .ok_or(SessionError::InvalidCommand)?
                    {
                        v1::cell_value::Kind::Blank(true) => CellValue::Blank,
                        v1::cell_value::Kind::Blank(false) => {
                            return Err(SessionError::InvalidCommand);
                        }
                        v1::cell_value::Kind::Number(n) => CellValue::Number(n),
                        v1::cell_value::Kind::Text(s) => CellValue::Text(s),
                        v1::cell_value::Kind::Boolean(b) => CellValue::Boolean(b),
                    }
                    .normalized()?;
                    writes.push(CellWrite {
                        address: cell,
                        value,
                    });
                }
                CommandPayload::SetCellValues { writes }
            }
            v1::canonical_operation::Kind::ClearCell(clear) => CommandPayload::ClearCell {
                address: address(clear.address)?,
            },
            v1::canonical_operation::Kind::ExtendRows(extension) => {
                if extension.rows.is_empty() || extension.rows.len() > 1_000 {
                    return Err(SessionError::InvalidCommand);
                }
                let sheet = SheetId::from_bytes(bytes_id(&extension.sheet_id)?)?;
                let mut rows = Vec::with_capacity(extension.rows.len());
                let mut seen = std::collections::BTreeSet::new();
                for raw in extension.rows {
                    let bytes = bytes_id(&raw)?;
                    if Uuid::from_bytes(bytes).get_version_num() != 7 {
                        return Err(SessionError::InvalidCommand);
                    }
                    let row = RowId::from_bytes(bytes)?;
                    if !seen.insert(row) {
                        return Err(SessionError::InvalidCommand);
                    }
                    rows.push(row);
                }
                CommandPayload::ExtendRows { sheet, rows }
            }
            v1::canonical_operation::Kind::InsertRows(insertion) => {
                if insertion.rows.is_empty() || insertion.rows.len() > 1_000 {
                    return Err(SessionError::InvalidCommand);
                }
                let sheet = SheetId::from_bytes(bytes_id(&insertion.sheet_id)?)?;
                let before = RowId::from_bytes(bytes_id(&insertion.before_row_id)?)?;
                let mut rows = Vec::with_capacity(insertion.rows.len());
                let mut seen = std::collections::BTreeSet::new();
                for raw in insertion.rows {
                    let bytes = bytes_id(&raw)?;
                    if Uuid::from_bytes(bytes).get_version_num() != 7 {
                        return Err(SessionError::InvalidCommand);
                    }
                    let row = RowId::from_bytes(bytes)?;
                    if !seen.insert(row) {
                        return Err(SessionError::InvalidCommand);
                    }
                    rows.push(row);
                }
                CommandPayload::InsertRows {
                    sheet,
                    before,
                    rows,
                }
            }
            v1::canonical_operation::Kind::DeleteRows(deletion) => {
                if deletion.rows.is_empty() || deletion.rows.len() > 1_000 {
                    return Err(SessionError::InvalidCommand);
                }
                let sheet = SheetId::from_bytes(bytes_id(&deletion.sheet_id)?)?;
                let mut rows = Vec::with_capacity(deletion.rows.len());
                let mut seen = std::collections::BTreeSet::new();
                for raw in deletion.rows {
                    let row = RowId::from_bytes(bytes_id(&raw)?)?;
                    if !seen.insert(row) {
                        return Err(SessionError::InvalidCommand);
                    }
                    rows.push(row);
                }
                CommandPayload::DeleteRows { sheet, rows }
            }
            v1::canonical_operation::Kind::InsertColumns(insertion) => {
                if insertion.columns.is_empty() || insertion.columns.len() > 100 {
                    return Err(SessionError::InvalidCommand);
                }
                let sheet = SheetId::from_bytes(bytes_id(&insertion.sheet_id)?)?;
                let before = ColumnId::from_bytes(bytes_id(&insertion.before_column_id)?)?;
                let mut columns = Vec::with_capacity(insertion.columns.len());
                let mut seen = std::collections::BTreeSet::new();
                for raw in insertion.columns {
                    let column = ColumnId::from_bytes(bytes_id(&raw)?)?;
                    if !seen.insert(column) {
                        return Err(SessionError::InvalidCommand);
                    }
                    columns.push(column);
                }
                CommandPayload::InsertColumns {
                    sheet,
                    before,
                    columns,
                }
            }
            v1::canonical_operation::Kind::DeleteColumns(deletion) => {
                if deletion.columns.is_empty() || deletion.columns.len() > 100 {
                    return Err(SessionError::InvalidCommand);
                }
                let sheet = SheetId::from_bytes(bytes_id(&deletion.sheet_id)?)?;
                let mut columns = Vec::with_capacity(deletion.columns.len());
                let mut seen = std::collections::BTreeSet::new();
                for raw in deletion.columns {
                    let column = ColumnId::from_bytes(bytes_id(&raw)?)?;
                    if !seen.insert(column) {
                        return Err(SessionError::InvalidCommand);
                    }
                    columns.push(column);
                }
                CommandPayload::DeleteColumns { sheet, columns }
            }
            v1::canonical_operation::Kind::AddSheet(input) => {
                if input.rows.is_empty()
                    || input.rows.len() > 1_000
                    || input.columns.is_empty()
                    || input.columns.len() > 100
                {
                    return Err(SessionError::InvalidCommand);
                }
                let sheet_id = bytes_id(&input.sheet_id)?;
                if Uuid::from_bytes(sheet_id).get_version_num() != 7 {
                    return Err(SessionError::InvalidCommand);
                }
                let rows = input
                    .rows
                    .iter()
                    .map(|raw| {
                        let bytes = bytes_id(raw)?;
                        if Uuid::from_bytes(bytes).get_version_num() != 7 {
                            return Err(SessionError::InvalidCommand);
                        }
                        RowId::from_bytes(bytes).map_err(SessionError::from)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let columns = input
                    .columns
                    .iter()
                    .map(|raw| {
                        let bytes = bytes_id(raw)?;
                        if Uuid::from_bytes(bytes).get_version_num() != 7 {
                            return Err(SessionError::InvalidCommand);
                        }
                        ColumnId::from_bytes(bytes).map_err(SessionError::from)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                if rows.iter().collect::<std::collections::BTreeSet<_>>().len() != rows.len()
                    || columns
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != columns.len()
                {
                    return Err(SessionError::InvalidCommand);
                }
                CommandPayload::AddSheet {
                    sheet: SheetSeed {
                        id: SheetId::from_bytes(sheet_id)?,
                        name: input.name,
                        rows,
                        columns,
                    },
                }
            }
            v1::canonical_operation::Kind::SetCellFormula(_) => {
                return Err(SessionError::InvalidCommand);
            }
        }
    };
    let operation_id = bytes_id(&input.operation_id)?;
    let document_id = bytes_id(&input.document_id)?;
    if Uuid::from_bytes(operation_id).get_version_num() != 7
        || Uuid::from_bytes(document_id).get_version_num() != 7
    {
        return Err(SessionError::InvalidCommand);
    }
    Ok(Command {
        operation_id: OperationId::from_bytes(operation_id)?,
        document_id: DocumentId::from_bytes(document_id)?,
        actor_id: ActorId::from_bytes(bytes_id(&input.actor_id)?)?,
        base_version: DocumentVersion::new(input.base_version)?,
        client_sequence: input.client_sequence,
        operation,
    })
}

fn style_patch(
    input: Option<v1::StylePatch>,
) -> Result<Vec<spreadsheet_domain::style::StyleChange>, SessionError> {
    wire::style::decode_patch(input.ok_or(SessionError::InvalidCommand)?).map_err(|error| {
        match error {
            wire::style::StyleCodecError::TooLarge => {
                SessionError::Journal(JournalError::PayloadTooLarge)
            }
            wire::style::StyleCodecError::UnsupportedSchema
            | wire::style::StyleCodecError::Invalid => SessionError::InvalidCommand,
        }
    })
}

fn wire_address(value: CellAddress) -> v1::CellAddress {
    v1::CellAddress {
        sheet_id: value.sheet.as_bytes().to_vec(),
        row_id: value.row.as_bytes().to_vec(),
        column_id: value.column.as_bytes().to_vec(),
    }
}
fn canonical(operation: Operation) -> Option<v1::CanonicalOperation> {
    let kind = match operation {
        Operation::RangeWrite {
            writes,
            kind: Some(spreadsheet_domain::workbook::RangeWriteKind::SetValues),
            ..
        } => {
            let writes = writes
                .into_iter()
                .map(|write| {
                    Some(spreadsheet_domain::workbook::CellWrite {
                        address: write.address,
                        value: match write.content {
                            Some(spreadsheet_domain::workbook::CellContent::Value(value)) => value,
                            None => CellValue::Blank,
                            _ => return None,
                        },
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            return canonical(Operation::SetCellValues { writes });
        }
        Operation::CellWrite {
            address, content, ..
        } => {
            let value = match content {
                None => v1::cell_value::Kind::Blank(true),
                Some(spreadsheet_domain::workbook::CellContent::Value(value)) => match value {
                    CellValue::Blank => v1::cell_value::Kind::Blank(true),
                    CellValue::Number(n) => v1::cell_value::Kind::Number(n),
                    CellValue::Text(s) => v1::cell_value::Kind::Text(s),
                    CellValue::Boolean(b) => v1::cell_value::Kind::Boolean(b),
                },
                Some(spreadsheet_domain::workbook::CellContent::Formula(formula)) => {
                    return Some(v1::CanonicalOperation {
                        kind: Some(v1::canonical_operation::Kind::SetCellFormula(Box::new(
                            v1::SetCellFormula {
                                address: Some(wire_address(address)),
                                formula: Some(wire::formula::encode_formula(&formula)),
                            },
                        ))),
                    });
                }
            };
            if matches!(value, v1::cell_value::Kind::Blank(_)) {
                v1::canonical_operation::Kind::ClearCell(v1::ClearCell {
                    address: Some(wire_address(address)),
                })
            } else {
                v1::canonical_operation::Kind::SetCellValue(v1::SetCellValue {
                    address: Some(wire_address(address)),
                    value: Some(v1::CellValue { kind: Some(value) }),
                })
            }
        }
        Operation::SetCellValue { address, value } => {
            v1::canonical_operation::Kind::SetCellValue(v1::SetCellValue {
                address: Some(wire_address(address)),
                value: Some(v1::CellValue {
                    kind: Some(match value {
                        CellValue::Blank => v1::cell_value::Kind::Blank(true),
                        CellValue::Number(n) => v1::cell_value::Kind::Number(n),
                        CellValue::Text(s) => v1::cell_value::Kind::Text(s),
                        CellValue::Boolean(b) => v1::cell_value::Kind::Boolean(b),
                    }),
                }),
            })
        }
        Operation::SetCellValues { writes } => {
            v1::canonical_operation::Kind::SetCellValues(v1::SetCellValues {
                writes: writes
                    .into_iter()
                    .map(|write| v1::SetCellValue {
                        address: Some(wire_address(write.address)),
                        value: Some(v1::CellValue {
                            kind: Some(match write.value {
                                CellValue::Blank => v1::cell_value::Kind::Blank(true),
                                CellValue::Number(n) => v1::cell_value::Kind::Number(n),
                                CellValue::Text(s) => v1::cell_value::Kind::Text(s),
                                CellValue::Boolean(b) => v1::cell_value::Kind::Boolean(b),
                            }),
                        }),
                    })
                    .collect(),
            })
        }
        Operation::ClearCell { address } => {
            v1::canonical_operation::Kind::ClearCell(v1::ClearCell {
                address: Some(wire_address(address)),
            })
        }
        Operation::ExtendRows { sheet, rows } => {
            v1::canonical_operation::Kind::ExtendRows(v1::ExtendRows {
                sheet_id: sheet.as_bytes().to_vec(),
                rows: rows.iter().map(|id| id.as_bytes().to_vec()).collect(),
            })
        }
        Operation::InsertRows {
            sheet,
            before,
            rows,
        }
        | Operation::InsertRowsRebased {
            sheet,
            before,
            rows,
        } => v1::canonical_operation::Kind::InsertRows(v1::InsertRows {
            sheet_id: sheet.as_bytes().to_vec(),
            before_row_id: before.as_bytes().to_vec(),
            rows: rows.iter().map(|id| id.as_bytes().to_vec()).collect(),
        }),
        Operation::DeleteRows { sheet, rows } => {
            v1::canonical_operation::Kind::DeleteRows(v1::DeleteRows {
                sheet_id: sheet.as_bytes().to_vec(),
                rows: rows.iter().map(|id| id.as_bytes().to_vec()).collect(),
            })
        }
        Operation::InsertColumnsRebased {
            sheet,
            before,
            columns,
        } => v1::canonical_operation::Kind::InsertColumns(v1::InsertColumns {
            sheet_id: sheet.as_bytes().to_vec(),
            before_column_id: before.as_bytes().to_vec(),
            columns: columns.iter().map(|id| id.as_bytes().to_vec()).collect(),
        }),
        Operation::DeleteColumns { sheet, columns } => {
            v1::canonical_operation::Kind::DeleteColumns(v1::DeleteColumns {
                sheet_id: sheet.as_bytes().to_vec(),
                columns: columns.iter().map(|id| id.as_bytes().to_vec()).collect(),
            })
        }
        Operation::AddSheet { sheet } => v1::canonical_operation::Kind::AddSheet(v1::AddSheet {
            sheet_id: sheet.id.as_bytes().to_vec(),
            name: sheet.name,
            rows: sheet.rows.iter().map(|id| id.as_bytes().to_vec()).collect(),
            columns: sheet
                .columns
                .iter()
                .map(|id| id.as_bytes().to_vec())
                .collect(),
        }),
        Operation::CreateWorkbook { .. }
        | Operation::AdvanceCalculationEpoch { .. }
        | Operation::CopyRange { .. }
        | Operation::RangeWrite { .. }
        | Operation::MoveRange { .. }
        | Operation::MoveCompensation { .. }
        | Operation::AxisEdit { .. }
        | Operation::AxisCompensation { .. }
        | Operation::StyleWrite { .. }
        | Operation::StyleCompensation { .. } => {
            return None;
        }
        Operation::SetCellFormula { address, formula } => {
            v1::canonical_operation::Kind::SetCellFormula(Box::new(v1::SetCellFormula {
                address: Some(wire_address(address)),
                formula: Some(wire::formula::encode_formula(&formula)),
            }))
        }
    };
    Some(v1::CanonicalOperation { kind: Some(kind) })
}

pub(crate) fn committed_message(event: &CommittedEvent) -> Option<v1::OperationCommitted> {
    let EventPrincipal::User(actor_id) = event.principal else {
        return None;
    };
    // A delayed formula's authored A1 source is not safe to rebind at head.
    // Keep the durable bound AST, but use the existing projection-resync path
    // until wire carries a rendered text view for this exact committed version.
    let formula = match &event.operation {
        Operation::SetCellFormula { formula, .. }
        | Operation::CellWrite {
            content: Some(spreadsheet_domain::workbook::CellContent::Formula(formula)),
            ..
        } => Some(formula),
        _ => None,
    };
    let transformed = formula.is_some_and(|formula| {
        formula.bound_at_version().get() < event.version.get().saturating_sub(1)
    });
    let operation = if transformed {
        None
    } else {
        canonical(event.operation.clone())
    };
    Some(v1::OperationCommitted {
        document_id: event.document_id.as_bytes().to_vec(),
        operation_id: event.operation_id.as_bytes().to_vec(),
        actor_id: actor_id.as_bytes().to_vec(),
        committed_version: event.version.get(),
        canonical_operation: operation,
        confirmed_state_hash: event.state_hash.to_vec(),
        transformed,
    })
}

pub async fn handle_command(
    state: &AppState,
    principal: &auth::Principal,
    input: v1::DocumentCommand,
) -> v1::ServerEnvelope {
    let document_id = input.document_id.clone();
    let operation_id = input.operation_id.clone();
    let result = async {
        let command = command(input)?;
        let session = state
            .sessions
            .session(principal.identity, command.document_id)
            .await?;
        session.submit(principal.identity, command).await
    }
    .await;
    let payload = match result {
        Ok(Submission::Committed { event, .. }) => match committed_message(&event) {
            Some(message) => v1::server_envelope::Payload::Committed(message),
            None => v1::server_envelope::Payload::Error(v1::ProtocolError {
                code: v1::ErrorCode::ServerBusy.into(),
                supported_protocol_version: wire::PROTOCOL_VERSION,
            }),
        },
        Ok(Submission::AlreadyCommitted(receipt)) => {
            v1::server_envelope::Payload::Receipt(v1::OperationReceipt {
                document_id,
                operation_id: receipt.operation_id.as_bytes().to_vec(),
                committed_version: receipt.committed_version.get(),
                canonical_event_hash: receipt.canonical_event_hash.to_vec(),
                current_head_version: receipt.current_head_version.get(),
            })
        }
        Err(error) => {
            let code = match error {
                SessionError::ActorMismatch | SessionError::Journal(JournalError::Unauthorized) => {
                    v1::RejectionCode::Unauthorized
                }
                SessionError::BaseVersionAhead => v1::RejectionCode::BaseVersionAhead,
                SessionError::RebaseWindowExceeded => v1::RejectionCode::RebaseWindowExceeded,
                SessionError::SourceVersionChanged => v1::RejectionCode::SourceVersionChanged,
                SessionError::UnsupportedMoveDependency => {
                    v1::RejectionCode::UnsupportedMoveDependency
                }
                SessionError::InvalidFormula => v1::RejectionCode::InvalidFormula,
                SessionError::Journal(JournalError::NotFound) => v1::RejectionCode::NotFound,
                SessionError::Journal(JournalError::IdempotencyConflict) => {
                    v1::RejectionCode::IdempotencyConflict
                }
                SessionError::Journal(JournalError::UndoConflict) => {
                    v1::RejectionCode::UndoConflict
                }
                SessionError::Journal(JournalError::NotUndoable) => v1::RejectionCode::NotUndoable,
                SessionError::Journal(JournalError::UndoWindowExceeded) => {
                    v1::RejectionCode::UndoWindowExceeded
                }
                SessionError::Journal(JournalError::PayloadTooLarge) => {
                    v1::RejectionCode::PayloadTooLarge
                }
                SessionError::Journal(JournalError::Conflict) => {
                    v1::RejectionCode::RetryableConflict
                }
                SessionError::QueueFull => v1::RejectionCode::QueueFull,
                SessionError::Domain(
                    DomainError::MissingSheet
                    | DomainError::MissingRow
                    | DomainError::MissingColumn,
                ) => v1::RejectionCode::DeletedTarget,
                SessionError::Domain(DomainError::UnsupportedStructuralDependency) => {
                    v1::RejectionCode::UnsupportedStructuralDependency
                }
                SessionError::InvalidCommand | SessionError::Domain(_) => {
                    v1::RejectionCode::InvalidCommand
                }
                _ => v1::RejectionCode::Unavailable,
            };
            v1::server_envelope::Payload::Rejected(v1::OperationRejected {
                document_id,
                operation_id,
                code: code.into(),
                retryable: matches!(
                    code,
                    v1::RejectionCode::RetryableConflict
                        | v1::RejectionCode::QueueFull
                        | v1::RejectionCode::Unavailable
                ),
                resync_required: matches!(
                    code,
                    v1::RejectionCode::BaseVersionAhead
                        | v1::RejectionCode::RetryableConflict
                        | v1::RejectionCode::RebaseWindowExceeded
                        | v1::RejectionCode::SourceVersionChanged
                ),
            })
        }
    };
    v1::ServerEnvelope {
        protocol_version: wire::PROTOCOL_VERSION,
        payload: Some(payload),
    }
}

#[cfg(test)]
mod undo_wire_tests {
    use super::*;

    fn id() -> Vec<u8> {
        Uuid::now_v7().as_bytes().to_vec()
    }

    fn envelope() -> v1::DocumentCommand {
        v1::DocumentCommand {
            operation_id: id(),
            document_id: id(),
            actor_id: id(),
            base_version: 1,
            client_sequence: 1,
            ..Default::default()
        }
    }

    fn bold_patch() -> v1::StylePatch {
        v1::StylePatch {
            schema_version: 1,
            profile_version: 1,
            changes: vec![v1::StyleChange {
                kind: Some(v1::style_change::Kind::FontBold(v1::BooleanStyleChange {
                    action: Some(v1::boolean_style_change::Action::Set(true)),
                })),
            }],
        }
    }

    #[test]
    fn style_commands_use_stable_targets_and_exactly_one_intent() -> Result<(), SessionError> {
        let cell = v1::CellAddress {
            sheet_id: id(),
            row_id: id(),
            column_id: id(),
        };
        let mut request = envelope();
        request.set_cell_style = Some(v1::SetCellStyleCommand {
            address: Some(cell.clone()),
            patch: Some(bold_patch()),
        });
        assert!(matches!(
            command(request.clone())?.operation,
            CommandPayload::SetCellStyle { .. }
        ));
        let mut combined = request.clone();
        combined.undo_cell = Some(v1::UndoCell {
            target_operation_id: id(),
        });
        assert!(matches!(
            command(combined),
            Err(SessionError::InvalidCommand)
        ));
        request.set_range_style = Some(v1::SetRangeStyleCommand {
            addresses: vec![cell.clone()],
            patch: Some(bold_patch()),
        });
        assert!(matches!(
            command(request.clone()),
            Err(SessionError::InvalidCommand)
        ));
        request.set_cell_style = None;
        assert!(matches!(
            command(request.clone())?.operation,
            CommandPayload::SetRangeStyle { .. }
        ));
        request.set_range_style = Some(v1::SetRangeStyleCommand {
            addresses: (0..1000)
                .map(|_| v1::CellAddress {
                    row_id: id(),
                    ..cell.clone()
                })
                .collect(),
            patch: Some(bold_patch()),
        });
        assert!(matches!(command(request.clone())?.operation,
            CommandPayload::SetRangeStyle { addresses, .. } if addresses.len() == 1000));
        for addresses in [
            vec![],
            vec![cell.clone(); 2],
            vec![cell.clone(); 1001],
            vec![
                cell.clone(),
                v1::CellAddress {
                    sheet_id: id(),
                    ..cell.clone()
                },
            ],
            vec![v1::CellAddress {
                row_id: vec![0; 16],
                ..cell.clone()
            }],
        ] {
            request.set_range_style = Some(v1::SetRangeStyleCommand {
                addresses,
                patch: Some(bold_patch()),
            });
            assert!(matches!(
                command(request.clone()),
                Err(SessionError::InvalidCommand)
            ));
        }
        request.set_range_style = Some(v1::SetRangeStyleCommand {
            addresses: vec![cell],
            patch: None,
        });
        assert!(matches!(
            command(request),
            Err(SessionError::InvalidCommand)
        ));
        for patch in [
            None,
            Some(v1::StylePatch {
                schema_version: 2,
                ..bold_patch()
            }),
            Some(v1::StylePatch {
                changes: vec![],
                ..bold_patch()
            }),
        ] {
            let mut invalid = envelope();
            invalid.set_cell_style = Some(v1::SetCellStyleCommand {
                address: Some(v1::CellAddress {
                    sheet_id: id(),
                    row_id: id(),
                    column_id: id(),
                }),
                patch,
            });
            assert!(matches!(
                command(invalid),
                Err(SessionError::InvalidCommand)
            ));
        }
        Ok(())
    }

    #[test]
    fn formula_projection_preserves_bound_ast_after_axis_insertion()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = DocumentId::from_bytes([1; 16])?;
        let sheet = SheetSeed {
            id: SheetId::from_bytes([2; 16])?,
            name: "Sheet1".into(),
            rows: vec![RowId::from_bytes([3; 16])?, RowId::from_bytes([4; 16])?],
            columns: vec![ColumnId::from_bytes([5; 16])?],
        };
        let origin = CellAddress {
            sheet: sheet.id,
            row: sheet.rows[1],
            column: sheet.columns[0],
        };
        let before = sheet.rows[0];
        let mut book = Workbook::empty(document).candidate(
            &Operation::CreateWorkbook { sheet },
            DocumentVersion::new(1)?,
        )?;
        let formula = spreadsheet_formula::binder::bind("=A1+1", &book, origin)?;
        book = book.candidate(
            &Operation::SetCellFormula {
                address: origin,
                formula,
            },
            DocumentVersion::new(2)?,
        )?;
        book = book.candidate(
            &Operation::InsertRowsRebased {
                sheet: origin.sheet,
                before,
                rows: vec![RowId::from_bytes([6; 16])?],
            },
            DocumentVersion::new(3)?,
        )?;
        let view = serde_json::to_value(
            document_view(document, &book)
                .map_err(|_| "projection failed")?
                .0,
        )?;
        let value = &view["sheets"][0]["cells"][0]["value"]["value"];
        let hex = value["bound_formula_wire_hex"]
            .as_str()
            .ok_or("missing bound formula")?;
        let bytes = hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair)?, 16).map_err(Into::into))
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        let wire = <v1::BoundFormula as prost::Message>::decode(bytes.as_slice())?;
        let decoded = wire::formula::decode_formula(wire)?;
        assert_eq!(
            book.cell_content(origin)?,
            Some(&CellContent::Formula(decoded))
        );
        assert_eq!(value["source"], "=A1+1");
        assert_eq!(value["current_text"], "=(A2+1)");
        Ok(())
    }
    #[test]
    fn style_projection_keeps_styled_blanks_and_exposes_authored_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        use spreadsheet_domain::style::{Change, StyleChange};
        let document = DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let sheet = SheetSeed {
            id: SheetId::from_bytes(*Uuid::now_v7().as_bytes())?,
            name: "Лист 1".into(),
            rows: vec![RowId::from_bytes(*Uuid::now_v7().as_bytes())?],
            columns: vec![ColumnId::from_bytes(*Uuid::now_v7().as_bytes())?],
        };
        let cell = CellAddress {
            sheet: sheet.id,
            row: sheet.rows[0],
            column: sheet.columns[0],
        };
        let book = Workbook::empty(document).candidate(
            &Operation::CreateWorkbook { sheet },
            DocumentVersion::new(1)?,
        )?;
        let operation = spreadsheet_domain::style_write::prepare_style_write(
            &book,
            &[cell],
            &[StyleChange::FontBold(Change::Set(true))],
            StyleWriteKind::Cell,
        )?;
        let styled = book.candidate(&operation, DocumentVersion::new(2)?)?;
        assert!(matches!(
            export_projection(&styled),
            Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "xlsx_styles_not_supported"
            ))
        ));
        let view = serde_json::to_value(
            document_view(document, &styled)
                .map_err(|_| "projection failed")?
                .0,
        )?;
        assert_eq!(view["authored_projection_schema_version"], 4);
        assert_eq!(view["style_schema_version"], 1);
        assert_eq!(view["style_profile_version"], 1);
        assert_eq!(view["last_axis_change_version"], "1");
        assert_eq!(view["retired_row_ids"], serde_json::json!([]));
        assert_eq!(view["row_activation_floors"], serde_json::json!([]));
        assert!(view["calculation_context"].is_null());
        assert_eq!(view["sheets"][0]["cells"], serde_json::json!([]));
        assert_eq!(
            view["sheets"][0]["styles"][0]["row_id"],
            id_string(cell.row.as_bytes())
        );
        let style = &view["sheets"][0]["styles"][0]["style"];
        assert_eq!(style["font_bold"], true);
        assert_eq!(style["font_family"], "Arial");
        assert_eq!(style["font_size_hundredth_points"], 1000);
        assert_eq!(style["horizontal"], "general");
        assert_eq!(style["vertical"], "bottom");
        assert_eq!(style["font_underline"], "none");
        assert_eq!(style["number_format"], "general");
        assert_eq!(style["font_color"], serde_json::json!([0, 0, 0]));
        assert_eq!(style["fill"], serde_json::json!([255, 255, 255]));
        assert!(style["border_top"].is_null());
        assert_ne!(book.hash(), styled.hash());
        let cleared = styled.candidate(
            &Operation::ClearCell { address: cell },
            DocumentVersion::new(3)?,
        )?;
        let after = serde_json::to_value(
            document_view(document, &cleared)
                .map_err(|_| "projection failed")?
                .0,
        )?;
        assert_eq!(after["sheets"][0]["styles"], view["sheets"][0]["styles"]);

        let actor = ActorId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let event = CommittedEvent {
            document_id: document,
            operation_id: OperationId::from_bytes(*Uuid::now_v7().as_bytes())?,
            principal: EventPrincipal::User(actor),
            version: styled.version(),
            operation,
            state_hash: styled.hash(),
        };
        let message = committed_message(&event).ok_or("missing style receipt")?;
        assert_eq!(message.actor_id, actor.as_bytes());
        assert!(message.canonical_operation.is_none());
        assert_eq!(message.confirmed_state_hash, styled.hash());
        let Operation::StyleWrite { kind, writes } = event.operation else {
            return Err("expected style write".into());
        };
        let inverse = Operation::StyleCompensation {
            kind,
            writes: writes
                .into_iter()
                .map(|write| GuardedStyleWrite {
                    address: write.address,
                    previous: write.changes,
                    changes: write.previous,
                })
                .collect(),
            undoes: event.operation_id,
            undoes_version: event.version,
        };
        let restored = styled.candidate(&inverse, DocumentVersion::new(3)?)?;
        let restored_view = serde_json::to_value(
            document_view(document, &restored)
                .map_err(|_| "projection failed")?
                .0,
        )?;
        assert_eq!(restored_view["sheets"][0]["styles"], serde_json::json!([]));
        let undo_event = CommittedEvent {
            operation: inverse,
            operation_id: OperationId::from_bytes(*Uuid::now_v7().as_bytes())?,
            version: restored.version(),
            state_hash: restored.hash(),
            ..event
        };
        assert!(
            committed_message(&undo_event)
                .ok_or("missing style inverse receipt")?
                .canonical_operation
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn authored_projection_preserves_retired_and_revived_axis_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        use spreadsheet_domain::axis_edit::{prepare_axis_edit, prepare_axis_inverse};
        let document = DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let actor = ActorId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let sheet = SheetSeed {
            id: SheetId::from_bytes(*Uuid::now_v7().as_bytes())?,
            name: "Лист 1".into(),
            rows: (0..2)
                .map(|_| RowId::from_bytes(*Uuid::now_v7().as_bytes()))
                .collect::<Result<_, _>>()?,
            columns: (0..2)
                .map(|_| ColumnId::from_bytes(*Uuid::now_v7().as_bytes()))
                .collect::<Result<_, _>>()?,
        };
        let row = sheet.rows[0];
        let column = sheet.columns[0];
        let sheet_id = sheet.id;
        let mut book = Workbook::empty(document).candidate(
            &Operation::CreateWorkbook { sheet },
            DocumentVersion::new(1)?,
        )?;
        for operation in [
            Operation::DeleteRows {
                sheet: sheet_id,
                rows: vec![row],
            },
            Operation::DeleteColumns {
                sheet: sheet_id,
                columns: vec![column],
            },
        ] {
            let version = DocumentVersion::new(book.version().get() + 1)?;
            let operation = prepare_axis_edit(&book, &operation, version)?;
            book = book.candidate(&operation, version)?;
            let target = CommittedEvent {
                document_id: document,
                operation_id: OperationId::from_bytes(*Uuid::now_v7().as_bytes())?,
                principal: EventPrincipal::User(actor),
                version,
                operation,
                state_hash: book.hash(),
            };
            let version = DocumentVersion::new(version.get() + 1)?;
            let inverse = prepare_axis_inverse(&book, &target, version)?;
            book = book.candidate(&inverse, version)?;
        }
        let view = serde_json::to_value(
            document_view(document, &book)
                .map_err(|_| "projection failed")?
                .0,
        )?;
        assert_eq!(view["version"], "5");
        assert_eq!(view["last_axis_change_version"], "5");
        assert_eq!(
            view["retired_row_ids"],
            serde_json::json!([id_string(row.as_bytes())])
        );
        assert_eq!(
            view["retired_column_ids"],
            serde_json::json!([id_string(column.as_bytes())])
        );
        assert_eq!(
            view["row_activation_floors"],
            serde_json::json!([{ "row_id": id_string(row.as_bytes()), "version": "3" }])
        );
        assert_eq!(
            view["column_activation_floors"],
            serde_json::json!([{ "column_id": id_string(column.as_bytes()), "version": "5" }])
        );
        assert_eq!(view["sheets"][0]["rows"][0], id_string(row.as_bytes()));
        assert_eq!(
            view["sheets"][0]["columns"][0],
            id_string(column.as_bytes())
        );
        Ok(())
    }

    #[test]
    fn undo_intent_cannot_be_combined_with_a_cell_write() -> Result<(), SessionError> {
        let target = id();
        let mut request = envelope();
        request.undo_cell = Some(v1::UndoCell {
            target_operation_id: target.clone(),
        });
        let parsed = command(request)?;
        assert!(matches!(
            parsed.operation,
            CommandPayload::UndoCell { target_operation_id }
                if target_operation_id.as_bytes() == target.as_slice()
        ));

        let mut forged = envelope();
        forged.undo_cell = Some(v1::UndoCell {
            target_operation_id: target,
        });
        forged.operation = Some(v1::CanonicalOperation {
            kind: Some(v1::canonical_operation::Kind::ClearCell(v1::ClearCell {
                address: None,
            })),
        });
        assert!(matches!(command(forged), Err(SessionError::InvalidCommand)));
        let cell = v1::CellAddress {
            sheet_id: id(),
            row_id: id(),
            column_id: id(),
        };
        let mut copy = envelope();
        copy.copy_range = Some(v1::CopyRangeCommand {
            source: Some(cell.clone()),
            destination: Some(cell),
            rows: 1,
            columns: 1,
        });
        assert!(matches!(
            command(copy.clone())?.operation,
            CommandPayload::CopyRange { .. }
        ));
        copy.undo_cell = Some(v1::UndoCell {
            target_operation_id: id(),
        });
        assert!(matches!(command(copy), Err(SessionError::InvalidCommand)));
        Ok(())
    }

    #[test]
    fn move_wire_rejects_bad_geometry_and_multiple_payloads() -> Result<(), SessionError> {
        let source = v1::CellAddress {
            sheet_id: id(),
            row_id: id(),
            column_id: id(),
        };
        let destination = v1::CellAddress {
            column_id: id(),
            ..source.clone()
        };
        let mut request = envelope();
        request.move_range = Some(v1::MoveRangeCommand {
            source: Some(source.clone()),
            destination: Some(destination.clone()),
            rows: 1,
            columns: 1,
        });
        assert!(matches!(
            command(request.clone())?.operation,
            CommandPayload::MoveRange { .. }
        ));
        for (rows, columns) in [(0, 1), (1, 0), (1001, 1), (u32::MAX, u32::MAX)] {
            let mut malformed = request.clone();
            if let Some(moved) = &mut malformed.move_range {
                moved.rows = rows;
                moved.columns = columns;
            }
            assert!(matches!(
                command(malformed),
                Err(SessionError::InvalidCommand)
            ));
        }
        let mut same = request.clone();
        if let Some(moved) = &mut same.move_range {
            moved.destination = Some(source.clone());
        }
        assert!(matches!(command(same), Err(SessionError::InvalidCommand)));
        request.copy_range = Some(v1::CopyRangeCommand {
            source: Some(source),
            destination: Some(destination),
            rows: 1,
            columns: 1,
        });
        assert!(matches!(
            command(request),
            Err(SessionError::InvalidCommand)
        ));
        Ok(())
    }

    #[test]
    fn add_sheet_wire_validates_ids_and_roundtrips_canonical() -> Result<(), SessionError> {
        let sheet_id = id();
        let row = id();
        let column = id();
        let mut request = envelope();
        request.operation = Some(v1::CanonicalOperation {
            kind: Some(v1::canonical_operation::Kind::AddSheet(v1::AddSheet {
                sheet_id: sheet_id.clone(),
                name: "Лист 2".into(),
                rows: vec![row.clone()],
                columns: vec![column.clone()],
            })),
        });
        let parsed = command(request.clone())?;
        let CommandPayload::AddSheet { sheet } = parsed.operation else {
            return Err(SessionError::InvalidCommand);
        };
        assert_eq!(sheet.id.as_bytes(), sheet_id.as_slice());
        assert_eq!(sheet.rows[0].as_bytes(), row.as_slice());
        assert_eq!(sheet.columns[0].as_bytes(), column.as_slice());
        let wire = canonical(Operation::AddSheet { sheet }).ok_or(SessionError::InvalidCommand)?;
        assert!(matches!(
            wire.kind,
            Some(v1::canonical_operation::Kind::AddSheet(_))
        ));
        if let Some(v1::canonical_operation::Kind::AddSheet(value)) = request
            .operation
            .as_mut()
            .and_then(|operation| operation.kind.as_mut())
        {
            value.rows.push(value.rows[0].clone());
        }
        assert!(matches!(
            command(request),
            Err(SessionError::InvalidCommand)
        ));
        Ok(())
    }
}
