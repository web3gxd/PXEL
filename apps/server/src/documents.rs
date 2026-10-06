use crate::{AppState, auth};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spreadsheet_application::journal::{AuthenticatedActor, JournalError, OperationJournal};
use spreadsheet_collab::{SessionDirectory, SessionError, Submission};
use spreadsheet_domain::style::{
    BorderLine, Change, HorizontalAlignment, NumberFormat, StyleChange, Underline,
    VerticalAlignment,
};
use spreadsheet_domain::workbook::*;
use spreadsheet_storage::{ExportAuditError, PermissionError, PermissionRole, SnapshotError};
use spreadsheet_wire_protocol::{self as wire, v1};
use spreadsheet_xlsx::{NormalizedCell, NormalizedCellValue, NormalizedSheet, NormalizedWorkbook};
use std::sync::Arc;
use uuid::Uuid;

const DOCUMENT_STYLE_SCHEMA_VERSION: u32 = 2;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRequest {
    document_id: String,
    operation_id: String,
}

#[derive(Serialize)]
pub struct Created {
    pub(crate) document_id: String,
    pub(crate) version: String,
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

pub(crate) fn parse_id(value: &str) -> Result<[u8; 16], ApiError> {
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
pub(crate) fn journal_error(error: JournalError) -> ApiError {
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
    capabilities: Option<DocumentCapabilitiesView>,
    encryption: EncryptionMetadataView,
}
/// Independently versioned value-only projection. No authored hash or context.
#[derive(Serialize)]
pub(crate) struct RedactedView {
    redacted_projection_schema_version: u32,
    style_schema_version: u32,
    style_profile_version: u32,
    document_id: String,
    version: String,
    privacy_revision: u64,
    sheets: Vec<SheetView>,
    capabilities: DocumentCapabilitiesView,
    encryption: EncryptionMetadataView,
}
impl RedactedView {
    pub(crate) fn stamp(&self) -> spreadsheet_storage::DisclosureStamp {
        self.capabilities.disclosure_stamp
    }
}
pub(crate) struct RedactedCapabilities {
    pub can_copy: bool,
    pub can_export: bool,
    pub can_export_pxel: bool,
    pub can_comment: bool,
    pub can_share: bool,
    pub is_guest: bool,
    pub stamp: spreadsheet_storage::DisclosureStamp,
}
pub(crate) fn redacted_view(
    book: &Workbook,
    policy: &spreadsheet_storage::PrivacyPolicy,
    analysis: &crate::privacy_api::Analysis,
    caps: RedactedCapabilities,
) -> Result<RedactedView, ApiError> {
    let original = document_view(book.document_id(), book)?.0;
    let mut sheets = original.sheets;
    for (sheet, source) in sheets.iter_mut().zip(book.sheets()) {
        sheet.hidden_rows.retain(|id| sheet.rows.contains(id));
        sheet.hidden_columns.retain(|id| sheet.columns.contains(id));
        sheet.name = crate::privacy_api::text_mask(&sheet.name, &policy.rules)?;
        for (cell, (address, _)) in sheet.cells.iter_mut().zip(source.cells()) {
            cell.value = match analysis.values.get(address).ok_or(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "privacy_projection",
            ))? {
                CellValue::Blank => JsonValue::Blank,
                CellValue::Number(n) => JsonValue::Number(*n),
                CellValue::Text(t) => JsonValue::Text(t.clone()),
                CellValue::Boolean(b) => JsonValue::Boolean(*b),
            };
        }
        for cell in &mut sheet.styles {
            cell.style.font_family =
                crate::privacy_api::text_mask(&cell.style.font_family, &policy.rules)?;
        }
    }
    Ok(RedactedView {
        redacted_projection_schema_version: 2,
        style_schema_version: original.style_schema_version,
        style_profile_version: original.style_profile_version,
        document_id: original.document_id,
        version: original.version,
        privacy_revision: policy.revision,
        sheets,
        capabilities: DocumentCapabilitiesView {
            schema_version: 2,
            can_edit: false,
            can_comment: caps.can_comment,
            can_share: caps.can_share,
            can_copy: caps.can_copy,
            can_export: caps.can_export,
            can_export_pxel: caps.can_export_pxel,
            anonymized: true,
            disclosure_stamp: caps.stamp,
            is_guest: caps.is_guest,
        },
        encryption: original.encryption,
    })
}
#[derive(Serialize)]
struct DocumentCapabilitiesView {
    schema_version: u32,
    can_edit: bool,
    can_comment: bool,
    can_share: bool,
    can_export: bool,
    can_export_pxel: bool,
    can_copy: bool,
    anonymized: bool,
    disclosure_stamp: spreadsheet_storage::DisclosureStamp,
    is_guest: bool,
}
#[derive(Serialize)]
struct EncryptionMetadataView {
    encrypted: bool,
    algorithm: &'static str,
    kek_rotated_at: Option<i64>,
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
    hidden: bool,
    default_column_width: u32,
    tab_color: Option<u32>,
    hidden_rows: Vec<String>,
    hidden_columns: Vec<String>,
    frozen_rows: u32,
    id: String,
    name: String,
    rows: Vec<String>,
    columns: Vec<String>,
    cells: Vec<CellView>,
    styles: Vec<StyledCellView>,
    row_heights: Vec<AxisSizeView>,
    column_widths: Vec<AxisSizeView>,
    merges: Vec<MergeRangeView>,
}
#[derive(Serialize)]
struct MergeRangeView {
    first_row_id: String,
    last_row_id: String,
    first_column_id: String,
    last_column_id: String,
}
#[derive(Serialize)]
struct AxisSizeView {
    axis_id: String,
    size_css_px: u32,
}
#[derive(Serialize)]
struct StyledCellView {
    row_id: String,
    column_id: String,
    style: CellStyleView,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CellStyleView {
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
    text_overflow: TextOverflowView,
    number_format: NumberFormatView,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BorderView {
    line: BorderLineView,
    color: [u8; 3],
}
macro_rules! style_json_enum {
    ($view:ident, $domain:ident, $($variant:ident),+ $(,)?) => {
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum $view { $($variant),+ }
        impl From<spreadsheet_domain::style::$domain> for $view {
            fn from(value: spreadsheet_domain::style::$domain) -> Self {
                match value { $(spreadsheet_domain::style::$domain::$variant => Self::$variant),+ }
            }
        }
        impl From<$view> for spreadsheet_domain::style::$domain {
            fn from(value: $view) -> Self { match value { $($view::$variant => Self::$variant),+ } }
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
style_json_enum!(TextOverflowView, TextOverflow, Clip, Wrap, Overflow);
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
            text_overflow: value.text_overflow().into(),
            number_format: value.number_format().into(),
        }
    }
}
impl TryFrom<CellStyleView> for spreadsheet_domain::style::CellStyle {
    type Error = spreadsheet_domain::style::StyleError;
    fn try_from(value: CellStyleView) -> Result<Self, Self::Error> {
        use spreadsheet_domain::style::{Border, CellStyle, FontFamily, FontSize, Rgb};
        let border = |value: Option<BorderView>| {
            value.map_or(Change::Reset, |b| {
                Change::Set(Border::new(b.line.into(), Rgb::new(b.color)))
            })
        };
        CellStyle::default().apply(&[
            StyleChange::FontFamily(Change::Set(FontFamily::new(value.font_family)?)),
            StyleChange::FontSize(Change::Set(FontSize::new(
                value.font_size_hundredth_points,
            )?)),
            StyleChange::FontBold(Change::Set(value.font_bold)),
            StyleChange::FontItalic(Change::Set(value.font_italic)),
            StyleChange::FontUnderline(Change::Set(value.font_underline.into())),
            StyleChange::FontStrike(Change::Set(value.font_strike)),
            StyleChange::FontColor(Change::Set(Rgb::new(value.font_color))),
            StyleChange::Fill(Change::Set(Rgb::new(value.fill))),
            StyleChange::BorderTop(border(value.border_top)),
            StyleChange::BorderRight(border(value.border_right)),
            StyleChange::BorderBottom(border(value.border_bottom)),
            StyleChange::BorderLeft(border(value.border_left)),
            StyleChange::Horizontal(Change::Set(value.horizontal.into())),
            StyleChange::Vertical(Change::Set(value.vertical.into())),
            StyleChange::TextOverflow(Change::Set(value.text_overflow.into())),
            StyleChange::NumberFormat(Change::Set(value.number_format.into())),
        ])
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
    let mut response = document_view(id, &workbook)?;
    let role = state
        .database
        .document_role_name(principal.identity, id)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    let (can_copy, can_export, _requires_anonymization, disclosure_stamp) = state
        .database
        .disclosure_capabilities(principal.identity, id, None)
        .await
        .map_err(crate::disclosure_api::error)?;
    response.0.capabilities = Some(DocumentCapabilitiesView {
        schema_version: 2,
        can_edit: role.as_deref().is_some_and(|role| role != "viewer"),
        can_comment: role.as_deref().is_some_and(|role| role != "viewer"),
        can_share: role.as_deref() == Some("owner"),
        can_export,
        can_export_pxel: state
            .database
            .can_export_pxel(principal.identity, id, None)
            .await
            .map_err(crate::disclosure_api::error)?,
        can_copy,
        anonymized: false,
        disclosure_stamp,
        is_guest: false,
    });
    response.0.encryption.kek_rotated_at = state
        .database
        .key_rotation_timestamp()
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    Ok(response)
}

pub async fn guest_document_view(
    state: &AppState,
    identity: AuthenticatedActor,
    document: DocumentId,
    role: spreadsheet_storage::ShareRole,
    link_id: Uuid,
) -> Result<Json<DocumentView>, ApiError> {
    let session = state
        .sessions
        .session(identity, document)
        .await
        .map_err(session_api_error)?;
    let workbook = session.read(identity).await.map_err(session_api_error)?;
    let mut response = document_view(document, &workbook)?;
    response.0.last_axis_change_version = "0".into();
    response.0.retired_row_ids.clear();
    response.0.retired_column_ids.clear();
    response.0.row_activation_floors.clear();
    response.0.column_activation_floors.clear();
    let (can_copy, can_export, _requires_anonymization, disclosure_stamp) = state
        .database
        .disclosure_capabilities(identity, document, Some(link_id))
        .await
        .map_err(crate::disclosure_api::error)?;
    response.0.capabilities = Some(DocumentCapabilitiesView {
        schema_version: 2,
        can_edit: role == spreadsheet_storage::ShareRole::Edit,
        can_comment: role != spreadsheet_storage::ShareRole::View,
        can_share: false,
        can_export,
        can_export_pxel: state
            .database
            .can_export_pxel(identity, document, Some(link_id))
            .await
            .map_err(crate::disclosure_api::error)?,
        can_copy,
        anonymized: false,
        disclosure_stamp,
        is_guest: true,
    });
    response.0.encryption.kek_rotated_at = state
        .database
        .key_rotation_timestamp()
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    Ok(response)
}

pub async fn guest_calculation_context(
    state: &AppState,
    identity: AuthenticatedActor,
    document: DocumentId,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session = state
        .sessions
        .session(identity, document)
        .await
        .map_err(session_api_error)?;
    let workbook = session.read(identity).await.map_err(session_api_error)?;
    let context=workbook.calculation_context().map(|context|serde_json::json!({
        "schema_version":1,
        "context_id":id_string(context.context_id().as_bytes()),
        "epoch":context.epoch().to_string(),
        "effective_timestamp_utc_ms":context.effective_timestamp_utc_ms().to_string(),
        "utc_offset_minutes":context.utc_offset_minutes(),
        "civil_day_since_unix_epoch":context.civil_day_since_unix_epoch(),
        "random_seed":context.random_seed().iter().map(|byte|format!("{byte:02x}")).collect::<String>(),
        "engine_version":context.engine_version(),
        "compatibility_profile":context.compatibility_profile(),
        "reason":match context.reason(){CalculationReason::Open=>"open",CalculationReason::Explicit=>"explicit",CalculationReason::Timer=>"timer",CalculationReason::VolatileDependency=>"volatile_dependency",CalculationReason::EngineUpgrade=>"engine_upgrade"}
    }));
    Ok(Json(
        serde_json::json!({"document_id":id_string(document.as_bytes()),"calculation_context":context}),
    ))
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

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ExportQuery {
    #[serde(default)]
    anonymized: bool,
}
pub async fn export_xlsx(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    if !crate::rate_allowed(&state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    let principal = principal(&state, &headers).await?;
    let document = DocumentId::from_bytes(parse_id(&id)?).map_err(domain_error)?;
    export_for_principal(state, principal, document, None, query.anonymized).await
}

pub async fn export_shared_xlsx(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(link): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let link = Uuid::from_bytes(parse_id(&link)?);
    let principal = state
        .auth
        .authenticate_share(&headers, link)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let auth::PrincipalScope::ShareGuest { session, .. } = &principal.scope else {
        return Err(ApiError(StatusCode::FORBIDDEN, "unauthorized"));
    };
    let doc = session.document_id;
    export_for_principal(state, principal, doc, Some(link), query.anonymized).await
}

async fn export_for_principal(
    state: Arc<AppState>,
    principal: auth::Principal,
    document: DocumentId,
    link: Option<Uuid>,
    anonymized: bool,
) -> Result<Response, ApiError> {
    let action = if anonymized {
        spreadsheet_storage::DisclosureAction::ExportXlsxAnonymized
    } else {
        spreadsheet_storage::DisclosureAction::ExportXlsx
    };
    let start = if link.is_none() && !anonymized {
        state
            .database
            .authorize_export(principal.identity, document)
            .await
            .map_err(export_audit_api_error)?
    } else {
        state
            .database
            .authorize_disclosure(principal.identity, document, link, action, None)
            .await
            .map_err(crate::disclosure_api::error)?
    };
    if !start.allowed {
        return Err(ApiError(StatusCode::FORBIDDEN, "disclosure_denied"));
    }
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
    if version.get() != start.stamp.document_version {
        return Err(ApiError(StatusCode::CONFLICT, "version_changed"));
    }
    let privacy = if start.anonymized {
        Some(
            state
                .database
                .privacy_policy(principal.identity, document, link)
                .await
                .map_err(crate::disclosure_api::error)?
                .rules,
        )
    } else {
        None
    };
    let prepared = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let analysis = privacy
            .as_ref()
            .map(|rules| crate::privacy_api::analyze(&workbook, rules))
            .transpose()?;
        let normalized =
            export_projection_redacted(&workbook, analysis.as_ref(), privacy.as_ref())?;
        spreadsheet_xlsx::export_scalar_xlsx(&normalized).map_err(|_| {
            ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "export_profile_unsupported",
            )
        })
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "export_unavailable"))?;
    let bytes = match prepared {
        Ok(bytes) => bytes,
        Err(failure) => {
            state
                .database
                .record_disclosure_failure(principal.identity, document, link, action, start.stamp)
                .await
                .map_err(crate::disclosure_api::error)?;
            return Err(failure);
        }
    };
    let (final_decision, lease, guard) = crate::disclosure_api::prepare_delivery(
        &state,
        &principal,
        document,
        link,
        action,
        Some(start.stamp),
    )
    .await?;
    if !final_decision.allowed {
        return Err(ApiError(StatusCode::FORBIDDEN, "disclosure_changed"));
    }
    if link.is_none() {
        let checksum: [u8; 32] = Sha256::digest(&bytes).into();
        state
            .database
            .record_export(principal.identity, document, version, checksum, bytes.len())
            .await
            .map_err(export_audit_api_error)?;
    }
    crate::disclosure_api::protected_response(
        bytes,
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        Some("attachment; filename=PXEL.xlsx"),
        lease,
        guard,
        true,
    )
}

fn export_audit_api_error(error: ExportAuditError) -> ApiError {
    match error {
        ExportAuditError::Unauthorized => ApiError(StatusCode::FORBIDDEN, "unauthorized"),
        ExportAuditError::Unavailable | ExportAuditError::Crypto(_) => {
            ApiError(StatusCode::SERVICE_UNAVAILABLE, "export_audit_unavailable")
        }
    }
}

#[cfg(test)]
fn export_projection(workbook: &Workbook) -> Result<NormalizedWorkbook, ApiError> {
    export_projection_redacted(workbook, None, None)
}
fn export_projection_redacted(
    workbook: &Workbook,
    analysis: Option<&crate::privacy_api::Analysis>,
    privacy: Option<&spreadsheet_storage::PrivacyRules>,
) -> Result<NormalizedWorkbook, ApiError> {
    let mut sheets = Vec::with_capacity(workbook.sheets().len());
    for sheet in workbook.sheets() {
        if sheet.settings().hidden
            || sheet.settings().default_column_width != 112
            || sheet.settings().tab_color.is_some()
            || !sheet.settings().hidden_rows.is_empty()
            || !sheet.settings().hidden_columns.is_empty()
        {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "xlsx_profile_unsupported",
            ));
        }
        let mut cells = Vec::with_capacity(sheet.cells().len());
        let mut styles = Vec::with_capacity(sheet.styles().len());
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
            let redacted = analysis
                .and_then(|a| a.values.get(address))
                .map(|v| CellContent::Value(v.clone()));
            let content = redacted.as_ref().unwrap_or(content);
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
        for (address, style) in sheet.styles() {
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
            let mut normalized = spreadsheet_xlsx::NormalizedStyle {
                row,
                column,
                font_family: None,
                font_size_hundredth_points: None,
                bold: None,
                italic: None,
                underline: None,
                strike: None,
                font_color: None,
                fill: None,
                border_top: None,
                border_right: None,
                border_bottom: None,
                border_left: None,
                horizontal: None,
                vertical: None,
                wrap: None,
                number_format: None,
            };
            for change in style.normalized_changes() {
                match change {
                    StyleChange::FontFamily(Change::Set(value)) => {
                        normalized.font_family = Some(value.as_str().to_owned())
                    }
                    StyleChange::FontSize(Change::Set(value)) => {
                        normalized.font_size_hundredth_points = Some(value.get())
                    }
                    StyleChange::FontBold(Change::Set(value)) => normalized.bold = Some(value),
                    StyleChange::FontItalic(Change::Set(value)) => normalized.italic = Some(value),
                    StyleChange::FontUnderline(Change::Set(value)) => {
                        normalized.underline = Some(match value {
                            Underline::Single => spreadsheet_xlsx::NormalizedUnderline::Single,
                            Underline::Double => spreadsheet_xlsx::NormalizedUnderline::Double,
                            Underline::None => continue,
                        })
                    }
                    StyleChange::FontStrike(Change::Set(value)) => normalized.strike = Some(value),
                    StyleChange::FontColor(Change::Set(value)) => {
                        normalized.font_color = Some(value.channels())
                    }
                    StyleChange::Fill(Change::Set(value)) => {
                        normalized.fill = Some(value.channels())
                    }
                    StyleChange::BorderTop(Change::Set(value)) => {
                        normalized.border_top = Some(normalized_border(value))
                    }
                    StyleChange::BorderRight(Change::Set(value)) => {
                        normalized.border_right = Some(normalized_border(value))
                    }
                    StyleChange::BorderBottom(Change::Set(value)) => {
                        normalized.border_bottom = Some(normalized_border(value))
                    }
                    StyleChange::BorderLeft(Change::Set(value)) => {
                        normalized.border_left = Some(normalized_border(value))
                    }
                    StyleChange::Horizontal(Change::Set(value)) => {
                        normalized.horizontal = Some(match value {
                            HorizontalAlignment::General => {
                                spreadsheet_xlsx::NormalizedHorizontal::General
                            }
                            HorizontalAlignment::Left => {
                                spreadsheet_xlsx::NormalizedHorizontal::Left
                            }
                            HorizontalAlignment::Center => {
                                spreadsheet_xlsx::NormalizedHorizontal::Center
                            }
                            HorizontalAlignment::Right => {
                                spreadsheet_xlsx::NormalizedHorizontal::Right
                            }
                        })
                    }
                    StyleChange::Vertical(Change::Set(value)) => {
                        normalized.vertical = Some(match value {
                            VerticalAlignment::Top => spreadsheet_xlsx::NormalizedVertical::Top,
                            VerticalAlignment::Center => {
                                spreadsheet_xlsx::NormalizedVertical::Center
                            }
                            VerticalAlignment::Bottom => {
                                spreadsheet_xlsx::NormalizedVertical::Bottom
                            }
                        })
                    }
                    StyleChange::TextOverflow(Change::Set(value)) => {
                        normalized.wrap =
                            Some(value == spreadsheet_domain::style::TextOverflow::Wrap)
                    }
                    StyleChange::NumberFormat(Change::Set(value)) => {
                        normalized.number_format = Some(match value {
                            NumberFormat::General => {
                                spreadsheet_xlsx::NormalizedNumberFormat::General
                            }
                            NumberFormat::Integer => {
                                spreadsheet_xlsx::NormalizedNumberFormat::Integer
                            }
                            NumberFormat::Decimal2 => {
                                spreadsheet_xlsx::NormalizedNumberFormat::Decimal2
                            }
                            NumberFormat::Percent0 => {
                                spreadsheet_xlsx::NormalizedNumberFormat::Percent0
                            }
                            NumberFormat::Percent2 => {
                                spreadsheet_xlsx::NormalizedNumberFormat::Percent2
                            }
                            NumberFormat::Scientific2 => {
                                spreadsheet_xlsx::NormalizedNumberFormat::Scientific2
                            }
                            NumberFormat::DateIso => {
                                spreadsheet_xlsx::NormalizedNumberFormat::DateIso
                            }
                        })
                    }
                    _ => {}
                }
            }
            if let Some(rules) = privacy
                && let Some(font) = normalized.font_family.as_mut()
            {
                *font = crate::privacy_api::text_mask(font, rules)?;
            }
            styles.push(normalized);
        }
        styles.sort_unstable();
        let merges = sheet
            .merges()
            .iter()
            .map(|merge| {
                Ok(spreadsheet_xlsx::NormalizedMerge {
                    first_row: sheet
                        .row_position(merge.first_row())
                        .and_then(|index| index.checked_add(1))
                        .ok_or(ApiError(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "export_projection",
                        ))?,
                    last_row: sheet
                        .row_position(merge.last_row())
                        .and_then(|index| index.checked_add(1))
                        .ok_or(ApiError(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "export_projection",
                        ))?,
                    first_column: sheet
                        .column_position(merge.first_column())
                        .and_then(|index| index.checked_add(1))
                        .ok_or(ApiError(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "export_projection",
                        ))?,
                    last_column: sheet
                        .column_position(merge.last_column())
                        .and_then(|index| index.checked_add(1))
                        .ok_or(ApiError(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "export_projection",
                        ))?,
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        sheets.push(NormalizedSheet {
            name: if let Some(rules) = privacy {
                let name = crate::privacy_api::text_mask(sheet.name(), rules)?;
                if name.contains("[СКРЫТО]") {
                    format!("Лист {}", sheets.len() + 1)
                } else {
                    name
                }
            } else {
                sheet.name().to_owned()
            },
            cells,
            styles,
            merges,
        });
    }
    let normalized = NormalizedWorkbook {
        schema_version: 3,
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

fn normalized_border(
    value: spreadsheet_domain::style::Border,
) -> spreadsheet_xlsx::NormalizedBorder {
    spreadsheet_xlsx::NormalizedBorder {
        line: match value.line() {
            BorderLine::Thin => spreadsheet_xlsx::NormalizedBorderLine::Thin,
            BorderLine::Medium => spreadsheet_xlsx::NormalizedBorderLine::Medium,
            BorderLine::Thick => spreadsheet_xlsx::NormalizedBorderLine::Thick,
            BorderLine::Dashed => spreadsheet_xlsx::NormalizedBorderLine::Dashed,
            BorderLine::Dotted => spreadsheet_xlsx::NormalizedBorderLine::Dotted,
            BorderLine::Double => spreadsheet_xlsx::NormalizedBorderLine::Double,
        },
        color: value.color().channels(),
    }
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

pub(crate) fn snapshot_api_error(error: SnapshotError) -> ApiError {
    match error {
        SnapshotError::VersionNotCommitted => {
            ApiError(StatusCode::NOT_FOUND, "version_not_committed")
        }
        SnapshotError::Journal(error) => journal_error(error),
        _ => ApiError(StatusCode::SERVICE_UNAVAILABLE, "history_unavailable"),
    }
}

pub(crate) fn document_view(
    id: DocumentId,
    workbook: &Workbook,
) -> Result<Json<DocumentView>, ApiError> {
    let sheets = workbook
        .sheets()
        .iter()
        .map(|sheet| -> Result<SheetView, ApiError> {
            Ok(SheetView {
                hidden: sheet.settings().hidden,
                default_column_width: sheet.settings().default_column_width,
                tab_color: sheet.settings().tab_color,
                hidden_rows: sheet
                    .settings()
                    .hidden_rows
                    .iter()
                    .map(|id| id_string(id.as_bytes()))
                    .collect(),
                hidden_columns: sheet
                    .settings()
                    .hidden_columns
                    .iter()
                    .map(|id| id_string(id.as_bytes()))
                    .collect(),
                frozen_rows: sheet.frozen_rows(),
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
                row_heights: sheet
                    .row_heights()
                    .iter()
                    .map(|(axis_id, size_css_px)| AxisSizeView {
                        axis_id: id_string(axis_id.as_bytes()),
                        size_css_px: *size_css_px,
                    })
                    .collect(),
                column_widths: sheet
                    .column_widths()
                    .iter()
                    .map(|(axis_id, size_css_px)| AxisSizeView {
                        axis_id: id_string(axis_id.as_bytes()),
                        size_css_px: *size_css_px,
                    })
                    .collect(),
                merges: sheet
                    .merges()
                    .iter()
                    .map(|merge| MergeRangeView {
                        first_row_id: id_string(merge.first_row().as_bytes()),
                        last_row_id: id_string(merge.last_row().as_bytes()),
                        first_column_id: id_string(merge.first_column().as_bytes()),
                        last_column_id: id_string(merge.last_column().as_bytes()),
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
        authored_projection_schema_version: 8,
        style_schema_version: DOCUMENT_STYLE_SCHEMA_VERSION,
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
        capabilities: None,
        encryption: EncryptionMetadataView {
            encrypted: true,
            algorithm: "XChaCha20-Poly1305",
            kek_rotated_at: None,
        },
    }))
}

pub(crate) fn session_api_error(error: SessionError) -> ApiError {
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
pub(crate) enum CalculatedValue {
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
                ..
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

fn decode_wire_merge_range(
    value: v1::MergeRange,
) -> Result<spreadsheet_domain::merge::MergeRange, SessionError> {
    Ok(spreadsheet_domain::merge::MergeRange::new(
        RowId::from_bytes(bytes_id(&value.first_row_id)?)?,
        RowId::from_bytes(bytes_id(&value.last_row_id)?)?,
        ColumnId::from_bytes(bytes_id(&value.first_column_id)?)?,
        ColumnId::from_bytes(bytes_id(&value.last_column_id)?)?,
    ))
}

fn encode_wire_merge_range(value: spreadsheet_domain::merge::MergeRange) -> v1::MergeRange {
    v1::MergeRange {
        first_row_id: value.first_row().as_bytes().to_vec(),
        last_row_id: value.last_row().as_bytes().to_vec(),
        first_column_id: value.first_column().as_bytes().to_vec(),
        last_column_id: value.last_column().as_bytes().to_vec(),
    }
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
        + usize::from(input.repeat_range.is_some())
        + usize::from(input.move_range.is_some())
        + usize::from(input.set_cell_style.is_some())
        + usize::from(input.set_range_style.is_some())
        + usize::from(input.set_axis_sizes.is_some())
        + usize::from(input.set_cell_batch.is_some())
        + usize::from(input.set_merge_range.is_some())
        + usize::from(input.set_frozen_rows.is_some())
        + usize::from(input.sort_range.is_some())
        + usize::from(input.shift_cells.is_some())
        + usize::from(input.sheet_settings.is_some())
        + usize::from(input.copy_sheet.is_some())
        != 1
    {
        return Err(SessionError::InvalidCommand);
    }
    let operation = if let Some(shift) = input.shift_cells {
        if shift.schema_version != 1 || shift.rows == 0 || shift.columns == 0 {
            return Err(SessionError::InvalidCommand);
        }
        CommandPayload::ShiftCells {
            source: address(shift.source)?,
            rows: shift.rows,
            columns: shift.columns,
            axis: match shift.axis_kind {
                1 => spreadsheet_domain::workbook::CellShiftAxis::Row,
                2 => spreadsheet_domain::workbook::CellShiftAxis::Column,
                _ => return Err(SessionError::InvalidCommand),
            },
            insert: shift.insert,
        }
    } else if let Some(settings) = input.sheet_settings {
        if settings.schema_version != 1 || settings.visibility.len() > 1000 {
            return Err(SessionError::InvalidCommand);
        }
        let tab_color = match settings.tab_color {
            None => None,
            Some(color) => Some(match color.action {
                Some(v1::color_style_change::Action::Set(v)) if v.len() == 3 => {
                    Some((u32::from(v[0]) << 16) | (u32::from(v[1]) << 8) | u32::from(v[2]))
                }
                Some(v1::color_style_change::Action::Reset(true)) => None,
                _ => return Err(SessionError::InvalidCommand),
            }),
        };
        CommandPayload::SetSheetSettings {
            sheet: SheetId::from_bytes(bytes_id(&settings.sheet_id)?)?,
            changes: spreadsheet_domain::workbook::SheetSettingsPatch {
                name: settings.name,
                index: settings.index,
                hidden: settings.hidden,
                default_column_width: settings.default_column_width,
                tab_color,
                visibility: settings
                    .visibility
                    .into_iter()
                    .map(|v| {
                        Ok((
                            match v.axis_kind {
                                1 => spreadsheet_domain::workbook::AxisSizeTarget::Row(
                                    RowId::from_bytes(bytes_id(&v.axis_id)?)?,
                                ),
                                2 => spreadsheet_domain::workbook::AxisSizeTarget::Column(
                                    ColumnId::from_bytes(bytes_id(&v.axis_id)?)?,
                                ),
                                _ => return Err(SessionError::InvalidCommand),
                            },
                            v.hidden,
                        ))
                    })
                    .collect::<Result<_, SessionError>>()?,
            },
        }
    } else if let Some(copy) = input.copy_sheet {
        if copy.schema_version != 1 {
            return Err(SessionError::InvalidCommand);
        }
        let seed = copy.sheet.ok_or(SessionError::InvalidCommand)?;
        if std::iter::once(&seed.sheet_id)
            .chain(seed.rows.iter())
            .chain(seed.columns.iter())
            .any(|v| {
                bytes_id(v).is_err()
                    || bytes_id(v).is_ok_and(|b| Uuid::from_bytes(b).get_version_num() != 7)
            })
        {
            return Err(SessionError::InvalidCommand);
        }
        CommandPayload::CopySheet {
            source: SheetId::from_bytes(bytes_id(&copy.source_sheet_id)?)?,
            sheet: SheetSeed {
                id: SheetId::from_bytes(bytes_id(&seed.sheet_id)?)?,
                name: seed.name,
                rows: seed
                    .rows
                    .iter()
                    .map(|v| Ok(RowId::from_bytes(bytes_id(v)?)?))
                    .collect::<Result<_, SessionError>>()?,
                columns: seed
                    .columns
                    .iter()
                    .map(|v| Ok(ColumnId::from_bytes(bytes_id(v)?)?))
                    .collect::<Result<_, SessionError>>()?,
            },
            index: copy.index,
        }
    } else if let Some(sort) = input.sort_range {
        if sort.schema_version != 1 {
            return Err(SessionError::InvalidCommand);
        }
        let range = spreadsheet_domain::sort::SortRange {
            source: address(sort.source)?,
            rows: sort.rows,
            columns: sort.columns,
            header_rows: sort.header_rows,
            keys: sort
                .keys
                .into_iter()
                .map(|key| {
                    Ok(spreadsheet_domain::sort::SortKey {
                        column: ColumnId::from_bytes(bytes_id(&key.column_id)?)?,
                        descending: key.descending,
                    })
                })
                .collect::<Result<_, SessionError>>()?,
        };
        range
            .validate_shape()
            .map_err(|_| SessionError::InvalidCommand)?;
        CommandPayload::SortRange { range }
    } else if let Some(batch) = input.set_cell_batch {
        if batch.writes.is_empty() || batch.writes.len() > 1_000 || batch.styles.len() > 1_000 {
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
                v1::cell_value::Kind::Blank(false) => return Err(SessionError::InvalidCommand),
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
        let mut style_seen = std::collections::BTreeSet::new();
        let mut styles = Vec::with_capacity(batch.styles.len());
        for style in batch.styles {
            let target = address(style.address)?;
            if !seen.contains(&target) || !style_seen.insert(target) {
                return Err(SessionError::InvalidCommand);
            }
            styles.push((target, style_patch(style.patch)?));
        }
        CommandPayload::SetCellBatch { writes, styles }
    } else if let Some(style) = input.set_cell_style {
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
    } else if let Some(frozen) = input.set_frozen_rows {
        if frozen.rows > spreadsheet_domain::axis_size::MAX_FROZEN_ROWS {
            return Err(SessionError::InvalidCommand);
        }
        CommandPayload::SetFrozenRows {
            sheet: SheetId::from_bytes(bytes_id(&frozen.sheet_id)?)?,
            rows: frozen.rows,
        }
    } else if let Some(sizes) = input.set_axis_sizes {
        if sizes.sheet_id.len() != 16 || sizes.writes.is_empty() || sizes.writes.len() > 1000 {
            return Err(SessionError::InvalidCommand);
        }
        let sheet = SheetId::from_bytes(bytes_id(&sizes.sheet_id)?)?;
        let mut seen = std::collections::BTreeSet::new();
        let mut writes = Vec::with_capacity(sizes.writes.len());
        for write in sizes.writes {
            let raw = bytes_id(&write.axis_id)?;
            let target = match write.axis_kind {
                1 => spreadsheet_domain::workbook::AxisSizeTarget::Row(RowId::from_bytes(raw)?),
                2 => {
                    spreadsheet_domain::workbook::AxisSizeTarget::Column(ColumnId::from_bytes(raw)?)
                }
                _ => return Err(SessionError::InvalidCommand),
            };
            if !seen.insert(target)
                || write
                    .size_css_px
                    .is_some_and(|size| !spreadsheet_domain::axis_size::valid_size(target, size))
            {
                return Err(SessionError::InvalidCommand);
            }
            writes.push((target, write.size_css_px));
        }
        CommandPayload::SetAxisSizes { sheet, writes }
    } else if let Some(merge) = input.set_merge_range {
        if merge.sheet_id.len() != 16 {
            return Err(SessionError::InvalidCommand);
        }
        let sheet = SheetId::from_bytes(bytes_id(&merge.sheet_id)?)?;
        let range = decode_wire_merge_range(merge.range.ok_or(SessionError::InvalidCommand)?)?;
        if merge.clear_covered {
            if !merge.merged {
                return Err(SessionError::InvalidCommand);
            }
            CommandPayload::MergeCells { sheet, range }
        } else {
            CommandPayload::SetMergeRange {
                sheet,
                range,
                merged: merge.merged,
            }
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
            repeat: None,
        }
    } else if let Some(repeat) = input.repeat_range {
        let copy = repeat.source_block.ok_or(SessionError::InvalidCommand)?;
        if copy.rows == 0
            || copy.columns == 0
            || repeat.destination_rows == 0
            || repeat.destination_columns == 0
            || repeat.destination_rows % copy.rows != 0
            || repeat.destination_columns % copy.columns != 0
            || u64::from(repeat.destination_rows) * u64::from(repeat.destination_columns) > 1000
        {
            return Err(SessionError::InvalidCommand);
        }
        CommandPayload::CopyRange {
            source: address(copy.source)?,
            destination: address(copy.destination)?,
            rows: copy.rows,
            columns: copy.columns,
            repeat: Some((repeat.destination_rows, repeat.destination_columns)),
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
            v1::canonical_operation::Kind::FrozenRowsWrite(frozen) => {
                if frozen.rows > spreadsheet_domain::axis_size::MAX_FROZEN_ROWS {
                    return Err(SessionError::InvalidCommand);
                }
                CommandPayload::SetFrozenRows {
                    sheet: SheetId::from_bytes(bytes_id(&frozen.sheet_id)?)?,
                    rows: frozen.rows,
                }
            }
            v1::canonical_operation::Kind::AxisSizeWrite(sizes) => {
                if sizes.sheet_id.len() != 16
                    || sizes.writes.is_empty()
                    || sizes.writes.len() > 1000
                {
                    return Err(SessionError::InvalidCommand);
                }
                let sheet = SheetId::from_bytes(bytes_id(&sizes.sheet_id)?)?;
                let mut seen = std::collections::BTreeSet::new();
                let mut writes = Vec::with_capacity(sizes.writes.len());
                for write in sizes.writes {
                    let raw = bytes_id(&write.axis_id)?;
                    let target = match write.axis_kind {
                        1 => AxisSizeTarget::Row(RowId::from_bytes(raw)?),
                        2 => AxisSizeTarget::Column(ColumnId::from_bytes(raw)?),
                        _ => return Err(SessionError::InvalidCommand),
                    };
                    if !seen.insert(target)
                        || write.size_css_px.is_some_and(|size| {
                            !spreadsheet_domain::axis_size::valid_size(target, size)
                        })
                    {
                        return Err(SessionError::InvalidCommand);
                    }
                    writes.push((target, write.size_css_px));
                }
                CommandPayload::SetAxisSizes { sheet, writes }
            }
            v1::canonical_operation::Kind::MergeWrite(merge) => {
                if merge.sheet_id.len() != 16 {
                    return Err(SessionError::InvalidCommand);
                }
                CommandPayload::SetMergeRange {
                    sheet: SheetId::from_bytes(bytes_id(&merge.sheet_id)?)?,
                    range: decode_wire_merge_range(
                        merge.range.ok_or(SessionError::InvalidCommand)?,
                    )?,
                    merged: merge.merged,
                }
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
        Operation::AxisSizeWrite { sheet, writes } => {
            v1::canonical_operation::Kind::AxisSizeWrite(v1::AxisSizeWrite {
                sheet_id: sheet.as_bytes().to_vec(),
                writes: writes
                    .into_iter()
                    .map(|write| {
                        let (axis_kind, axis_id) = match write.target {
                            AxisSizeTarget::Row(id) => (1, id.as_bytes().to_vec()),
                            AxisSizeTarget::Column(id) => (2, id.as_bytes().to_vec()),
                        };
                        v1::AxisSizeSet {
                            axis_kind,
                            axis_id,
                            size_css_px: write.size,
                        }
                    })
                    .collect(),
            })
        }
        Operation::SheetSettingsWrite { .. }
        | Operation::CopySheet { .. }
        | Operation::MergeCellsWrite { .. } => return None,
        Operation::FrozenRowsWrite { sheet, rows, .. } => {
            v1::canonical_operation::Kind::FrozenRowsWrite(v1::FrozenRowsWrite {
                sheet_id: sheet.as_bytes().to_vec(),
                rows,
            })
        }
        Operation::MergeWrite { sheet, change, .. } => {
            v1::canonical_operation::Kind::MergeWrite(v1::MergeWrite {
                sheet_id: sheet.as_bytes().to_vec(),
                range: Some(encode_wire_merge_range(change.range)),
                merged: change.merged,
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
        | Operation::CellBatchWrite { .. }
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
    let parsed = command(input);
    let receipt_command = parsed.as_ref().ok().cloned();
    let result = async {
        let command = parsed?;
        let session = state
            .sessions
            .session(principal.identity, command.document_id)
            .await?;
        session.submit(principal.identity, command).await
    }
    .await;
    let payload = match result {
        Ok(Submission::Committed { event, .. }) => {
            if matches!(&principal.scope, auth::PrincipalScope::ShareGuest { .. }) {
                let receipt = if let Some(command) = receipt_command.as_ref() {
                    state
                        .database
                        .receipt(principal.identity, command)
                        .await
                        .ok()
                        .flatten()
                } else {
                    None
                };
                match receipt {
                    Some(receipt) => v1::server_envelope::Payload::Receipt(v1::OperationReceipt {
                        document_id: event.document_id.as_bytes().to_vec(),
                        operation_id: event.operation_id.as_bytes().to_vec(),
                        committed_version: receipt.committed_version.get(),
                        canonical_event_hash: receipt.canonical_event_hash.to_vec(),
                        current_head_version: receipt.current_head_version.get(),
                    }),
                    None => v1::server_envelope::Payload::Error(v1::ProtocolError {
                        code: v1::ErrorCode::ServerBusy.into(),
                        supported_protocol_version: wire::PROTOCOL_VERSION,
                    }),
                }
            } else {
                match committed_message(&event) {
                    Some(message) => v1::server_envelope::Payload::Committed(message),
                    None => v1::server_envelope::Payload::Error(v1::ProtocolError {
                        code: v1::ErrorCode::ServerBusy.into(),
                        supported_protocol_version: wire::PROTOCOL_VERSION,
                    }),
                }
            }
        }
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
            let error_category = match &error {
                SessionError::QueueFull => "queue_full",
                SessionError::Unavailable => "session_unavailable",
                SessionError::ActorMismatch => "actor_mismatch",
                SessionError::BaseVersionAhead => "base_version_ahead",
                SessionError::InvalidCommand => "invalid_command",
                SessionError::RebaseWindowExceeded => "rebase_window_exceeded",
                SessionError::SourceVersionChanged => "source_version_changed",
                SessionError::CalculationUnavailable => "calculation_unavailable",
                SessionError::UnsupportedMoveDependency => "unsupported_move_dependency",
                SessionError::InvalidFormula => "invalid_formula",
                SessionError::Domain(_) => "domain_rejected",
                SessionError::Journal(_) => "journal_rejected",
            };
            tracing::warn!(event = "document_command_failed", error_category);
            let code = match error {
                SessionError::ActorMismatch | SessionError::Journal(JournalError::Unauthorized) => {
                    v1::RejectionCode::Unauthorized
                }
                SessionError::BaseVersionAhead => v1::RejectionCode::BaseVersionAhead,
                SessionError::RebaseWindowExceeded => v1::RejectionCode::RebaseWindowExceeded,
                SessionError::SourceVersionChanged => v1::RejectionCode::SourceVersionChanged,
                SessionError::CalculationUnavailable => v1::RejectionCode::CalculationUnavailable,
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
            tracing::warn!(
                event = "document_command_rejected",
                rejection_code = code.as_str_name()
            );
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

    #[test]
    fn repeat_wire_rejects_bad_extents_and_competing_intents() -> Result<(), SessionError> {
        let cell = v1::CellAddress {
            sheet_id: id(),
            row_id: id(),
            column_id: id(),
        };
        let block = v1::CopyRangeCommand {
            source: Some(cell.clone()),
            destination: Some(cell),
            rows: 1,
            columns: 2,
        };
        let mut request = envelope();
        request.repeat_range = Some(v1::RepeatRangeCommand {
            source_block: Some(block.clone()),
            destination_rows: 3,
            destination_columns: 4,
        });
        assert!(matches!(
            command(request.clone())?.operation,
            CommandPayload::CopyRange {
                repeat: Some((3, 4)),
                ..
            }
        ));
        for (rows, columns) in [(0, 4), (3, 0), (3, 3), (1001, 2), (u32::MAX, u32::MAX)] {
            let mut invalid = request.clone();
            if let Some(repeat) = &mut invalid.repeat_range {
                repeat.destination_rows = rows;
                repeat.destination_columns = columns;
            }
            assert!(matches!(
                command(invalid),
                Err(SessionError::InvalidCommand)
            ));
        }
        request.copy_range = Some(block);
        assert!(matches!(
            command(request),
            Err(SessionError::InvalidCommand)
        ));
        Ok(())
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
    fn sort_command_validates_its_own_version_keys_and_single_intent() -> Result<(), SessionError> {
        let sort = v1::SortRangeCommand {
            schema_version: 1,
            source: Some(v1::CellAddress {
                sheet_id: id(),
                row_id: id(),
                column_id: id(),
            }),
            rows: 3,
            columns: 2,
            header_rows: 1,
            keys: vec![v1::SortKey {
                column_id: id(),
                descending: true,
            }],
        };
        let mut input = envelope();
        input.sort_range = Some(sort.clone());
        assert!(matches!(
            command(input)?.operation,
            CommandPayload::SortRange { .. }
        ));
        let mut invalid = sort.clone();
        invalid.schema_version = 2;
        let mut input = envelope();
        input.sort_range = Some(invalid);
        assert!(matches!(command(input), Err(SessionError::InvalidCommand)));
        let mut invalid = sort.clone();
        invalid.keys.push(invalid.keys[0].clone());
        let mut input = envelope();
        input.sort_range = Some(invalid);
        assert!(matches!(command(input), Err(SessionError::InvalidCommand)));
        let mut invalid = sort.clone();
        invalid.rows = u32::MAX;
        let mut input = envelope();
        input.sort_range = Some(invalid);
        assert!(matches!(command(input), Err(SessionError::InvalidCommand)));
        let mut input = envelope();
        input.sort_range = Some(sort);
        input.set_frozen_rows = Some(v1::SetFrozenRowsCommand {
            sheet_id: id(),
            rows: 1,
        });
        assert!(matches!(command(input), Err(SessionError::InvalidCommand)));
        Ok(())
    }

    #[test]
    fn xlsx_export_refuses_sheet_metadata_that_profile_cannot_preserve()
    -> Result<(), Box<dyn std::error::Error>> {
        let seed = SheetSeed {
            id: SheetId::from_bytes([2; 16])?,
            name: "Sheet".into(),
            rows: vec![RowId::from_bytes([3; 16])?],
            columns: vec![ColumnId::from_bytes([4; 16])?],
        };
        let original = Workbook::empty(DocumentId::from_bytes([1; 16])?).candidate(
            &Operation::CreateWorkbook {
                sheet: seed.clone(),
            },
            DocumentVersion::new(1)?,
        )?;
        assert!(export_projection(&original).is_ok());
        let op = spreadsheet_domain::sheet_settings::prepare(
            &original,
            seed.id,
            &spreadsheet_domain::workbook::SheetSettingsPatch {
                tab_color: Some(Some(0x123456)),
                default_column_width: Some(160),
                ..Default::default()
            },
        )?;
        let book = original.candidate(&op, DocumentVersion::new(2)?)?;
        assert!(matches!(
            export_projection(&book),
            Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "xlsx_profile_unsupported"
            ))
        ));
        Ok(())
    }

    #[test]
    fn cells_metadata_and_shift_wire_versions_reject_mixed_unknown_and_bad_colour()
    -> Result<(), SessionError> {
        let mut request = envelope();
        request.sheet_settings = Some(v1::SheetSettingsCommand {
            schema_version: 1,
            sheet_id: id(),
            default_column_width: Some(160),
            tab_color: Some(v1::ColorStyleChange {
                action: Some(v1::color_style_change::Action::Set(vec![18, 52, 86])),
            }),
            ..Default::default()
        });
        assert!(
            matches!(command(request.clone())?.operation,CommandPayload::SetSheetSettings{changes,..} if changes.tab_color==Some(Some(0x123456)))
        );
        for schema in [0, 2] {
            let mut invalid = request.clone();
            invalid
                .sheet_settings
                .as_mut()
                .ok_or(SessionError::InvalidCommand)?
                .schema_version = schema;
            assert!(matches!(
                command(invalid),
                Err(SessionError::InvalidCommand)
            ));
        }
        let mut invalid = request.clone();
        invalid
            .sheet_settings
            .as_mut()
            .ok_or(SessionError::InvalidCommand)?
            .tab_color = Some(v1::ColorStyleChange {
            action: Some(v1::color_style_change::Action::Set(vec![18, 52])),
        });
        assert!(matches!(
            command(invalid),
            Err(SessionError::InvalidCommand)
        ));
        let shift = v1::ShiftCellsCommand {
            schema_version: 1,
            source: Some(v1::CellAddress {
                sheet_id: id(),
                row_id: id(),
                column_id: id(),
            }),
            rows: 1,
            columns: 1,
            axis_kind: 1,
            insert: true,
        };
        let mut combined = request;
        combined.shift_cells = Some(shift.clone());
        assert!(matches!(
            command(combined),
            Err(SessionError::InvalidCommand)
        ));
        let mut single = envelope();
        single.shift_cells = Some(shift);
        assert!(matches!(
            command(single.clone())?.operation,
            CommandPayload::ShiftCells { .. }
        ));
        single
            .shift_cells
            .as_mut()
            .ok_or(SessionError::InvalidCommand)?
            .axis_kind = 3;
        assert!(matches!(command(single), Err(SessionError::InvalidCommand)));
        Ok(())
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
        let export = export_projection(&styled).map_err(|_| "export projection failed")?;
        assert_eq!(export.sheets[0].cells.len(), 0);
        assert_eq!(export.sheets[0].styles.len(), 1);
        assert_eq!(export.sheets[0].styles[0].row, 1);
        assert_eq!(export.sheets[0].styles[0].column, 1);
        assert_eq!(export.sheets[0].styles[0].bold, Some(true));
        let view = serde_json::to_value(
            document_view(document, &styled)
                .map_err(|_| "projection failed")?
                .0,
        )?;
        assert_eq!(view["authored_projection_schema_version"], 8);
        assert_eq!(view["style_schema_version"], 2);
        assert_eq!(view["style_profile_version"], 2);
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
