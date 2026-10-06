use crate::{AppState, auth, documents::ApiError};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use spreadsheet_collab::SessionDirectory;
use spreadsheet_domain::workbook::DocumentId;
use spreadsheet_storage::{PersonalFilters, PersonalView, PersonalViewError};
use std::sync::Arc;
use uuid::Uuid;

fn error(error: PersonalViewError) -> ApiError {
    match error {
        PersonalViewError::Unauthorized => ApiError(StatusCode::FORBIDDEN, "unauthorized"),
        PersonalViewError::Conflict => ApiError(StatusCode::CONFLICT, "view_or_document_changed"),
        PersonalViewError::Invalid => ApiError(StatusCode::BAD_REQUEST, "invalid_personal_view"),
        PersonalViewError::Unavailable => ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    }
}
fn identifier(text: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(text)
        .ok()
        .filter(|id| id.get_version_num() == 7)
        .ok_or(ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewUpdate {
    schema_version: u32,
    expected_revision: u64,
    document_version: u64,
    settings: PersonalFilters,
}
async fn principal(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    guest: bool,
) -> Result<(auth::Principal, DocumentId), ApiError> {
    let id = identifier(id)?;
    let principal = if guest {
        state.auth.authenticate_share(headers, id).await
    } else {
        state.auth.authenticate(headers).await
    }
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
    .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let document = if guest {
        let auth::PrincipalScope::ShareGuest { session, .. } = &principal.scope else {
            return Err(ApiError(StatusCode::FORBIDDEN, "unauthorized"));
        };
        session.document_id
    } else {
        DocumentId::from_bytes(*id.as_bytes())
            .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))?
    };
    Ok((principal, document))
}
async fn deliver(
    state: &Arc<AppState>,
    principal: &auth::Principal,
    document: DocumentId,
    view: PersonalView,
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
    let bytes = serde_json::to_vec(&view)
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    crate::disclosure_api::protected_response(bytes, "application/json", None, lease, guard, true)
}
async fn read(
    state: Arc<AppState>,
    headers: HeaderMap,
    id: String,
    guest: bool,
) -> Result<Response, ApiError> {
    let (principal, document) = principal(&state, &headers, &id, guest).await?;
    let view = state
        .database
        .personal_view(principal.identity, document)
        .await
        .map_err(error)?;
    deliver(&state, &principal, document, view).await
}
async fn update(
    state: Arc<AppState>,
    headers: HeaderMap,
    id: String,
    guest: bool,
    input: ViewUpdate,
) -> Result<Response, ApiError> {
    if !auth::valid_origin(&headers, &state.origins) {
        return Err(ApiError(StatusCode::FORBIDDEN, "origin_denied"));
    }
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "unknown_view_version"));
    }
    if !crate::rate_allowed(&state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    let (principal, document) = principal(&state, &headers, &id, guest).await?;
    let session = state
        .sessions
        .session(principal.identity, document)
        .await
        .map_err(crate::documents::session_api_error)?;
    let book = session
        .read(principal.identity)
        .await
        .map_err(crate::documents::session_api_error)?;
    if book.version().get() != input.document_version {
        return Err(ApiError(StatusCode::CONFLICT, "view_or_document_changed"));
    }
    input.settings.validate_geometry(&book).map_err(error)?;
    let view = state
        .database
        .set_personal_view(
            principal.identity,
            document,
            input.document_version,
            input.expected_revision,
            input.settings,
        )
        .await
        .map_err(error)?;
    deliver(&state, &principal, document, view).await
}
pub async fn get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    read(state, headers, id, false).await
}
pub async fn put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ViewUpdate>,
) -> Result<Response, ApiError> {
    update(state, headers, id, false, input).await
}
pub async fn guest_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    read(state, headers, id, true).await
}
pub async fn guest_put(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ViewUpdate>,
) -> Result<Response, ApiError> {
    update(state, headers, id, true, input).await
}
