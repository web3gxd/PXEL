//! Owner journal and fixed historical projections; never installs an actor candidate.
#[cfg(test)]
mod tests;
use crate::{
    AppState, auth,
    documents::{self, ApiError},
};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use spreadsheet_domain::{axis_edit::AxisDelta, workbook::*};
use spreadsheet_formula::{
    Value,
    calculation::{CalculationLimits, CalculationProjection},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, atomic::AtomicBool},
};
use uuid::Uuid;

fn limited() -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, "history_analysis_limit")
}
fn id(bytes: &[u8; 16]) -> String {
    Uuid::from_bytes(*bytes).to_string()
}
async fn owner(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    raw: &str,
) -> Result<(auth::Principal, DocumentId), ApiError> {
    if !crate::rate_allowed(state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    let identity = state
        .auth
        .authenticate(headers)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let uuid = Uuid::parse_str(raw)
        .ok()
        .filter(|v| v.get_version_num() == 7)
        .ok_or(ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))?;
    let doc = DocumentId::from_bytes(*uuid.as_bytes())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))?;
    if state
        .database
        .document_role_name(identity.identity, doc)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .as_deref()
        != Some("owner")
    {
        return Err(ApiError(StatusCode::FORBIDDEN, "owner_required"));
    }
    Ok((identity, doc))
}
async fn deliver<T: Serialize>(
    state: &Arc<AppState>,
    principal: &auth::Principal,
    doc: DocumentId,
    value: T,
) -> Result<Response, ApiError> {
    // A concurrent downgrade remains readable but must no longer deliver owner data.
    if state
        .database
        .document_role_name(principal.identity, doc)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .as_deref()
        != Some("owner")
    {
        return Err(ApiError(StatusCode::FORBIDDEN, "owner_required"));
    }
    let bytes = serde_json::to_vec(&value).map_err(|_| limited())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(limited());
    }
    if !matches!(state.auth.active(principal).await, Ok(true)) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let gate = state.delivery.get(doc).await;
    let guard = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        gate.lock.clone().read_owned(),
    )
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "send_gate_busy"))?;
    let lease = state
        .database
        .lease_owner_delivery_access(principal.identity, doc)
        .await
        .map_err(documents::journal_error)?;
    crate::disclosure_api::protected_response(bytes, "application/json", None, lease, guard, true)
}
fn version(raw: &str) -> Result<DocumentVersion, ApiError> {
    raw.parse::<u64>()
        .ok()
        .and_then(|n| DocumentVersion::new(n).ok())
        .ok_or(ApiError(StatusCode::BAD_REQUEST, "invalid_version"))
}
async fn recover(
    state: &AppState,
    principal: &auth::Principal,
    doc: DocumentId,
    target: DocumentVersion,
) -> Result<Workbook, ApiError> {
    Ok(tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state
            .database
            .recover_document_at_version(&state.blobs, principal.identity, doc, target),
    )
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "history_timeout"))?
    .map_err(documents::snapshot_api_error)?
    .workbook)
}
fn calculated(book: &Workbook) -> Result<BTreeMap<CellAddress, Value>, ApiError> {
    if book.calculation_context().is_none() && book.sheets().iter().flat_map(|s|s.cells().values()).any(|v|matches!(v,CellContent::Formula(f) if f.ast().nodes().iter().any(|n|matches!(n,spreadsheet_domain::formula::Node::Call{name,..} if matches!(name.as_str(),"RAND"|"NOW"|"TODAY"))))) { return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY,"history_context_missing")); }
    let limits = CalculationLimits::default();
    let mut projection =
        CalculationProjection::from_workbook(book, limits).map_err(|_| limited())?;
    projection
        .calculate(book, limits, &AtomicBool::new(false))
        .map_err(|_| limited())?;
    Ok(projection.results().clone())
}
#[derive(Serialize)]
struct HistoricalValue {
    sheet_id: String,
    row_id: String,
    column_id: String,
    value: documents::CalculatedValue,
}
#[derive(Serialize)]
struct HistoricalView {
    schema_version: u32,
    read_only: bool,
    projection: documents::DocumentView,
    calculated: Vec<HistoricalValue>,
}
pub async fn historical(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((raw, target)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let (principal, doc) = owner(&state, &headers, &raw).await?;
    let slot = state
        .history_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "history_busy"))?;
    let book = recover(&state, &principal, doc, version(&target)?).await?;
    let view = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        let results = calculated(&book)?;
        Ok::<_, ApiError>(HistoricalView {
            schema_version: 1,
            read_only: true,
            projection: documents::document_view(doc, &book)?.0,
            calculated: results
                .iter()
                .map(|(a, v)| HistoricalValue {
                    sheet_id: id(a.sheet.as_bytes()),
                    row_id: id(a.row.as_bytes()),
                    column_id: id(a.column.as_bytes()),
                    value: v.into(),
                })
                .collect(),
        })
    })
    .await
    .map_err(|_| limited())??;
    deliver(&state, &principal, doc, view).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryQuery {
    pub schema_version: u32,
    pub through_version: String,
    pub before_version: Option<String>,
}
#[derive(Serialize)]
struct Footprint {
    sheet_id: String,
    addresses: Vec<[String; 2]>,
    row_ids: Vec<String>,
    column_ids: Vec<String>,
}
#[derive(Serialize)]
struct ActivityView {
    operation_id: String,
    actor_id: Option<String>,
    created_at: i64,
    version: String,
    operation_type: &'static str,
    structural: bool,
    footprints: Vec<Footprint>,
}
#[derive(Serialize)]
struct ActivityPage {
    schema_version: u32,
    through_version: String,
    events: Vec<ActivityView>,
    next_before: Option<String>,
}
fn activity(entry: spreadsheet_storage::ActivityEntry) -> ActivityView {
    let event = entry.event;
    let mut addresses = Vec::new();
    let mut axes: Vec<(SheetId, Vec<RowId>, Vec<ColumnId>)> = Vec::new();
    let kind = match &event.operation {
        Operation::CreateWorkbook { sheet } => {
            axes.push((sheet.id, vec![], vec![]));
            "create_workbook"
        }
        Operation::AddSheet { sheet } => {
            axes.push((sheet.id, vec![], vec![]));
            "add_sheet"
        }
        Operation::SheetSettingsWrite { sheet, .. } => {
            axes.push((*sheet, vec![], vec![]));
            "sheet_settings"
        }
        Operation::CopySheet { sheet, .. } => {
            axes.push((sheet.id, vec![], vec![]));
            "copy_sheet"
        }
        Operation::FrozenRowsWrite { sheet, .. } => {
            axes.push((*sheet, vec![], vec![]));
            "frozen_rows"
        }
        Operation::AxisSizeWrite { sheet, writes } => {
            let mut rows = vec![];
            let mut cols = vec![];
            for w in writes {
                match w.target {
                    spreadsheet_domain::axis_size::AxisSizeTarget::Row(r) => rows.push(r),
                    spreadsheet_domain::axis_size::AxisSizeTarget::Column(c) => cols.push(c),
                }
            }
            axes.push((*sheet, rows, cols));
            "axis_size"
        }
        Operation::SetCellValue { address, .. }
        | Operation::SetCellFormula { address, .. }
        | Operation::ClearCell { address }
        | Operation::CellWrite { address, .. } => {
            addresses.push(*address);
            if matches!(
                event.operation,
                Operation::CellWrite {
                    undoes: Some(_),
                    ..
                }
            ) {
                "undo_redo"
            } else {
                "cell_write"
            }
        }
        Operation::SetCellValues { writes } => {
            addresses.extend(writes.iter().map(|w| w.address));
            "range_write"
        }
        Operation::CopyRange { writes } => {
            addresses.extend(writes.iter().map(|w| w.address));
            "copy_range"
        }
        Operation::RangeWrite { writes, undoes, .. } => {
            addresses.extend(writes.iter().map(|w| w.address));
            if undoes.is_some() {
                "undo_redo"
            } else {
                "range_write"
            }
        }
        Operation::MoveRange { writes, .. } | Operation::MoveCompensation { writes, .. } => {
            addresses.extend(writes.iter().map(|w| w.address));
            if matches!(event.operation, Operation::MoveCompensation { .. }) {
                "undo_redo"
            } else {
                "move_range"
            }
        }
        Operation::StyleWrite { writes, .. } | Operation::StyleCompensation { writes, .. } => {
            addresses.extend(writes.iter().map(|w| w.address));
            if matches!(event.operation, Operation::StyleCompensation { .. }) {
                "undo_redo"
            } else {
                "style_write"
            }
        }
        Operation::CellBatchWrite {
            content_writes,
            style_writes,
            intent,
            undoes,
            ..
        } => {
            addresses.extend(content_writes.iter().map(|w| w.address));
            addresses.extend(style_writes.iter().map(|w| w.address));
            if undoes.is_some() {
                "undo_redo"
            } else if matches!(intent, CellBatchIntent::SortRange { .. }) {
                "sort_range"
            } else {
                "cell_batch"
            }
        }
        Operation::MergeWrite {
            sheet,
            change,
            undoes,
            ..
        }
        | Operation::MergeCellsWrite {
            sheet,
            change,
            undoes,
            ..
        } => {
            addresses.extend([
                CellAddress {
                    sheet: *sheet,
                    row: change.range.first_row(),
                    column: change.range.first_column(),
                },
                CellAddress {
                    sheet: *sheet,
                    row: change.range.last_row(),
                    column: change.range.last_column(),
                },
            ]);
            if undoes.is_some() {
                "undo_redo"
            } else {
                "merge_write"
            }
        }
        Operation::ExtendRows { sheet, rows }
        | Operation::InsertRows { sheet, rows, .. }
        | Operation::InsertRowsRebased { sheet, rows, .. }
        | Operation::DeleteRows { sheet, rows } => {
            axes.push((*sheet, rows.clone(), vec![]));
            "rows"
        }
        Operation::InsertColumnsRebased { sheet, columns, .. }
        | Operation::DeleteColumns { sheet, columns } => {
            axes.push((*sheet, vec![], columns.clone()));
            "columns"
        }
        Operation::AxisEdit { change } | Operation::AxisCompensation { change, .. } => {
            match &change.delta {
                AxisDelta::Rows { removed, inserted } => axes.push((
                    change.sheet,
                    removed
                        .iter()
                        .copied()
                        .chain(inserted.iter().map(|p| p.row))
                        .collect(),
                    vec![],
                )),
                AxisDelta::Columns { removed, inserted } => axes.push((
                    change.sheet,
                    vec![],
                    removed
                        .iter()
                        .copied()
                        .chain(inserted.iter().map(|p| p.column))
                        .collect(),
                )),
            }
            addresses.extend(change.writes.iter().map(|w| w.address));
            if matches!(event.operation, Operation::AxisCompensation { .. }) {
                "undo_redo"
            } else {
                "axis_edit"
            }
        }
        Operation::AdvanceCalculationEpoch { .. } => "calculation_epoch",
    };
    let mut grouped: BTreeMap<SheetId, Footprint> = BTreeMap::new();
    for address in addresses.into_iter().collect::<BTreeSet<_>>() {
        grouped
            .entry(address.sheet)
            .or_insert_with(|| Footprint {
                sheet_id: id(address.sheet.as_bytes()),
                addresses: vec![],
                row_ids: vec![],
                column_ids: vec![],
            })
            .addresses
            .push([id(address.row.as_bytes()), id(address.column.as_bytes())]);
    }
    for (sheet, rows, cols) in axes {
        let f = grouped.entry(sheet).or_insert_with(|| Footprint {
            sheet_id: id(sheet.as_bytes()),
            addresses: vec![],
            row_ids: vec![],
            column_ids: vec![],
        });
        f.row_ids.extend(rows.iter().map(|r| id(r.as_bytes())));
        f.column_ids.extend(cols.iter().map(|c| id(c.as_bytes())));
    }
    ActivityView {
        operation_id: id(event.operation_id.as_bytes()),
        actor_id: match event.principal {
            EventPrincipal::User(a) => Some(id(a.as_bytes())),
            EventPrincipal::System => None,
        },
        created_at: entry.created_at,
        version: event.version.get().to_string(),
        operation_type: kind,
        structural: event.operation.changes_axes(),
        footprints: grouped.into_values().collect(),
    }
}
pub async fn history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(query): Query<HistoryQuery>,
) -> Result<Response, ApiError> {
    if query.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "unknown_history_version"));
    }
    let (principal, doc) = owner(&state, &headers, &raw).await?;
    let _slot = state
        .history_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "history_busy"))?;
    let through = version(&query.through_version)?.get();
    let before = query
        .before_version
        .as_deref()
        .map(version)
        .transpose()?
        .map(DocumentVersion::get);
    let page = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        state
            .database
            .activity_page(principal.identity, doc, through, before),
    )
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "history_timeout"))?
    .map_err(documents::journal_error)?;
    let view = ActivityPage {
        schema_version: 1,
        through_version: page.through_version.to_string(),
        events: page.entries.into_iter().map(activity).collect(),
        next_before: page.next_before.map(|v| v.to_string()),
    };
    deliver(&state, &principal, doc, view).await
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd)]
struct Key {
    sheet: String,
    category: &'static str,
    entity: String,
    property: String,
}
type Comparable = BTreeMap<Key, JsonValue>;
fn insert(
    map: &mut Comparable,
    sheet: SheetId,
    category: &'static str,
    entity: String,
    property: &str,
    value: JsonValue,
) -> Result<(), ApiError> {
    if map.len() >= 1_000_000 {
        return Err(limited());
    }
    map.insert(
        Key {
            sheet: id(sheet.as_bytes()),
            category,
            entity,
            property: property.into(),
        },
        value,
    );
    Ok(())
}
fn comparable(book: &Workbook) -> Result<Comparable, ApiError> {
    let results = calculated(book)?;
    let mut map = Comparable::new();
    for (order, sheet) in book.sheets().iter().enumerate() {
        insert(
            &mut map,
            sheet.id(),
            "sheet",
            String::new(),
            "name",
            json!(sheet.name()),
        )?;
        insert(
            &mut map,
            sheet.id(),
            "sheet",
            String::new(),
            "order",
            json!(order),
        )?;
        insert(
            &mut map,
            sheet.id(),
            "layout",
            String::new(),
            "frozen_rows",
            json!(sheet.frozen_rows()),
        )?;
        insert(
            &mut map,
            sheet.id(),
            "sheet",
            String::new(),
            "hidden",
            json!(sheet.settings().hidden),
        )?;
        insert(
            &mut map,
            sheet.id(),
            "layout",
            String::new(),
            "default_column_width",
            json!(sheet.settings().default_column_width),
        )?;
        insert(
            &mut map,
            sheet.id(),
            "sheet",
            String::new(),
            "tab_color",
            json!(sheet.settings().tab_color),
        )?;
        for row in &sheet.settings().hidden_rows {
            insert(
                &mut map,
                sheet.id(),
                "layout",
                id(row.as_bytes()),
                "row_hidden",
                json!(true),
            )?;
        }
        for column in &sheet.settings().hidden_columns {
            insert(
                &mut map,
                sheet.id(),
                "layout",
                id(column.as_bytes()),
                "column_hidden",
                json!(true),
            )?;
        }
        for (position, row) in sheet.rows().iter().enumerate() {
            insert(
                &mut map,
                sheet.id(),
                "structure",
                id(row.as_bytes()),
                "row_position",
                json!(position),
            )?;
        }
        for (position, column) in sheet.columns().iter().enumerate() {
            insert(
                &mut map,
                sheet.id(),
                "structure",
                id(column.as_bytes()),
                "column_position",
                json!(position),
            )?;
        }
        for (row, size) in sheet.row_heights() {
            insert(
                &mut map,
                sheet.id(),
                "size",
                id(row.as_bytes()),
                "row_height",
                json!(size),
            )?;
        }
        for (column, size) in sheet.column_widths() {
            insert(
                &mut map,
                sheet.id(),
                "size",
                id(column.as_bytes()),
                "column_width",
                json!(size),
            )?;
        }
        for merge in sheet.merges() {
            let entity = format!(
                "{}/{}/{}/{}",
                id(merge.first_row().as_bytes()),
                id(merge.last_row().as_bytes()),
                id(merge.first_column().as_bytes()),
                id(merge.last_column().as_bytes())
            );
            insert(
                &mut map,
                sheet.id(),
                "merge",
                entity,
                "membership",
                json!(true),
            )?;
        }
        for (address, content) in sheet.cells() {
            let entity = format!(
                "{}/{}",
                id(address.row.as_bytes()),
                id(address.column.as_bytes())
            );
            match content {
                CellContent::Formula(f) => {
                    let bound = spreadsheet_wire_protocol::formula::encode_formula(f)
                        .ast
                        .ok_or_else(limited)?;
                    let binding_fingerprint = Sha256::digest(prost::Message::encode_to_vec(&bound))
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>();
                    insert(
                        &mut map,
                        sheet.id(),
                        "formula",
                        entity.clone(),
                        "formula",
                        json!({"authored_source":f.source(),"current_text":spreadsheet_formula::bound::render_current(f,book).map_err(|_|limited())?,"binding_fingerprint":binding_fingerprint}),
                    )?;
                    if let Some(v) = results.get(address) {
                        insert(
                            &mut map,
                            sheet.id(),
                            "calculated",
                            entity,
                            "result",
                            serde_json::to_value(documents::CalculatedValue::from(v))
                                .map_err(|_| limited())?,
                        )?;
                    }
                }
                CellContent::Value(v) => {
                    let value = match v {
                        CellValue::Blank => json!({"type":"blank"}),
                        CellValue::Number(n) => json!({"type":"number","value":n}),
                        CellValue::Text(t) => json!({"type":"text","value":t}),
                        CellValue::Boolean(b) => json!({"type":"boolean","value":b}),
                    };
                    insert(&mut map, sheet.id(), "value", entity, "content", value)?;
                }
            }
        }
        for (address, style) in sheet.styles() {
            let entity = format!(
                "{}/{}",
                id(address.row.as_bytes()),
                id(address.column.as_bytes())
            );
            let properties = serde_json::to_value(documents::CellStyleView::from(style))
                .map_err(|_| limited())?;
            for (property, value) in properties.as_object().ok_or_else(limited)? {
                insert(
                    &mut map,
                    sheet.id(),
                    "style",
                    entity.clone(),
                    property,
                    value.clone(),
                )?;
            }
        }
    }
    Ok(map)
}
#[derive(Serialize)]
struct Difference {
    category: &'static str,
    sheet_id: String,
    entity_id: String,
    property: String,
    before: Option<JsonValue>,
    after: Option<JsonValue>,
    structural_context: bool,
}
#[derive(Serialize)]
struct DiffView {
    schema_version: u32,
    document_id: String,
    from_version: String,
    to_version: String,
    structural_context: bool,
    changes: Vec<Difference>,
}
fn diff(before: &Workbook, after: &Workbook) -> Result<DiffView, ApiError> {
    let left = comparable(before)?;
    let right = comparable(after)?;
    let default_style = serde_json::to_value(documents::CellStyleView::from(
        &spreadsheet_domain::style::CellStyle::default(),
    ))
    .map_err(|_| limited())?;
    let keys = left
        .keys()
        .chain(right.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    if keys.len() > 1_000_000 {
        return Err(limited());
    }
    let structural = before.last_axis_change_version() != after.last_axis_change_version();
    let mut changes = vec![];
    for key in keys {
        let default = if key.category == "style" {
            default_style.get(&key.property)
        } else {
            None
        };
        let a = left.get(&key).or(default);
        let b = right.get(&key).or(default);
        if a == b {
            continue;
        }
        if changes.len() >= 100_000 {
            return Err(limited());
        }
        changes.push(Difference {
            category: key.category,
            sheet_id: key.sheet,
            entity_id: key.entity,
            property: key.property,
            before: a.cloned(),
            after: b.cloned(),
            structural_context: structural && key.category == "formula",
        });
    }
    Ok(DiffView {
        schema_version: 1,
        document_id: id(before.document_id().as_bytes()),
        from_version: before.version().get().to_string(),
        to_version: after.version().get().to_string(),
        structural_context: structural,
        changes,
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffQuery {
    schema_version: u32,
    from_version: String,
    to_version: String,
}
pub async fn compare(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    Query(query): Query<DiffQuery>,
) -> Result<Response, ApiError> {
    if query.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "unknown_diff_version"));
    }
    let (principal, doc) = owner(&state, &headers, &raw).await?;
    let slot = state
        .history_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "history_busy"))?;
    let a = recover(&state, &principal, doc, version(&query.from_version)?).await?;
    let b = recover(&state, &principal, doc, version(&query.to_version)?).await?;
    let value = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        diff(&a, &b)
    })
    .await
    .map_err(|_| limited())??;
    deliver(&state, &principal, doc, value).await
}
