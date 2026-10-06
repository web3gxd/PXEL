//! CSV download shares the audited and fenced disclosure boundary.
use crate::{AppState, auth, catalog_api, csv_api, csv_export, documents::ApiError};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use spreadsheet_collab::SessionDirectory;
use spreadsheet_domain::workbook::{DocumentId, SheetId};
use spreadsheet_storage::DisclosureAction;
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportQuery {
    schema_version: u32,
    sheet_id: String,
    document_version: String,
    #[serde(default)]
    delimiter: csv_api::Delimiter,
    #[serde(default = "safe_default")]
    safe_text: bool,
    #[serde(default)]
    anonymized: bool,
}
fn safe_default() -> bool {
    true
}
pub async fn export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let principal = catalog_api::member(&state, &headers, false).await?;
    let doc = DocumentId::from_bytes(*catalog_api::uuid(&id)?.as_bytes())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))?;
    export_for(state, principal, doc, None, query).await
}
pub async fn shared_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    if !crate::rate_allowed(&state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    let link = catalog_api::uuid(&id)?;
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
    export_for(state, principal, doc, Some(link), query).await
}
async fn export_for(
    state: Arc<AppState>,
    principal: auth::Principal,
    doc: DocumentId,
    link: Option<uuid::Uuid>,
    query: ExportQuery,
) -> Result<Response, ApiError> {
    if query.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "csv_version"));
    }
    let sheet = SheetId::from_bytes(*catalog_api::uuid(&query.sheet_id)?.as_bytes())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))?;
    let action = if query.anonymized {
        DisclosureAction::ExportCsvAnonymized
    } else {
        DisclosureAction::ExportCsv
    };
    let start = state
        .database
        .authorize_disclosure(principal.identity, doc, link, action, None)
        .await
        .map_err(crate::disclosure_api::error)?;
    if !start.allowed {
        return Err(ApiError(StatusCode::FORBIDDEN, "disclosure_denied"));
    }
    if query.document_version != start.stamp.document_version.to_string() {
        return Err(ApiError(StatusCode::CONFLICT, "version_changed"));
    }
    let permit = state
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "export_busy"))?;
    let book = state
        .sessions
        .session(principal.identity, doc)
        .await
        .map_err(crate::documents::session_api_error)?
        .read_confirmed(principal.identity)
        .await
        .map_err(crate::documents::session_api_error)?;
    if book.version().get() != start.stamp.document_version {
        return Err(ApiError(StatusCode::CONFLICT, "version_changed"));
    }
    let privacy = if start.anonymized {
        Some(
            state
                .database
                .privacy_policy(principal.identity, doc, link)
                .await
                .map_err(crate::disclosure_api::error)?
                .rules,
        )
    } else {
        None
    };
    let result = tokio::task::spawn_blocking(move || {
        let analysis = privacy
            .as_ref()
            .map(|p| crate::privacy_api::analyze(&book, p))
            .transpose()
            .map_err(|error| (error, true))?;
        let bytes = csv_export::encode(
            &book,
            sheet,
            query.delimiter.byte(),
            query.safe_text,
            analysis.as_ref().map(|a| &a.values),
        )
        .map_err(|e| {
            (
                ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    match e {
                        csv_export::CsvExportError::Limit => "csv_limit",
                        csv_export::CsvExportError::Invalid => "csv_export_invalid",
                        csv_export::CsvExportError::Calculation => "csv_calculation",
                    },
                ),
                false,
            )
        })?;
        Ok::<_, (ApiError, bool)>((bytes, permit))
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "csv_unavailable"))?;
    let (bytes, _permit) = match result {
        Ok(result) => result,
        Err((failure, privacy_failed)) => {
            let audit = if privacy_failed {
                state
                    .database
                    .record_disclosure_failure(principal.identity, doc, link, action, start.stamp)
                    .await
            } else {
                state
                    .database
                    .record_export_preparation_failure(
                        principal.identity,
                        doc,
                        link,
                        action,
                        start.stamp,
                    )
                    .await
            };
            audit.map_err(crate::disclosure_api::error)?;
            return Err(failure);
        }
    };
    let (receipt, lease, guard) = crate::disclosure_api::prepare_delivery(
        &state,
        &principal,
        doc,
        link,
        action,
        Some(start.stamp),
    )
    .await?;
    if !receipt.allowed {
        return Err(ApiError(StatusCode::FORBIDDEN, "disclosure_changed"));
    }
    crate::disclosure_api::protected_response(
        bytes,
        "text/csv; charset=utf-8; header=absent",
        Some("attachment; filename=PXEL.csv"),
        lease,
        guard,
        true,
    )
}
