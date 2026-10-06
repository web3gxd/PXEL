//! Current-workspace library; metadata/prefs never enter the workbook journal.
use crate::{AppState, auth, documents::ApiError};
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use spreadsheet_domain::workbook::DocumentId;
use spreadsheet_storage::{
    CatalogError, LayoutPreference, LibraryLayout, LibraryPreference, Metadata,
};
use std::sync::Arc;
use uuid::Uuid;
pub(crate) fn error(e: CatalogError) -> ApiError {
    ApiError(
        match e {
            CatalogError::Unauthorized => StatusCode::FORBIDDEN,
            CatalogError::Conflict => StatusCode::CONFLICT,
            CatalogError::Invalid => StatusCode::BAD_REQUEST,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        },
        match e {
            CatalogError::Unauthorized => "catalog_unauthorized",
            CatalogError::Conflict => "catalog_changed",
            CatalogError::Invalid => "catalog_invalid",
            _ => "catalog_unavailable",
        },
    )
}
pub(crate) async fn member(
    state: &AppState,
    headers: &HeaderMap,
    write: bool,
) -> Result<auth::Principal, ApiError> {
    if write && !auth::valid_origin(headers, &state.origins) {
        return Err(ApiError(StatusCode::FORBIDDEN, "origin_denied"));
    }
    if !crate::rate_allowed(state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    state
        .auth
        .authenticate(headers)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"))
}
pub(crate) fn uuid(value: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value)
        .ok()
        .filter(|id| id.get_version_num() == 7)
        .ok_or(ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))
}
fn doc(value: &str) -> Result<DocumentId, ApiError> {
    DocumentId::from_bytes(*uuid(value)?.as_bytes())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    #[serde(default)]
    before: Option<Uuid>,
    #[serde(default = "schema")]
    schema_version: u32,
}
fn schema() -> u32 {
    1
}
pub async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(input): Query<PageQuery>,
) -> Result<Response, ApiError> {
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "catalog_version"));
    }
    let principal = member(&state, &headers, false).await?;
    let (page, lease) = state
        .database
        .catalog_page(principal.identity, input.before)
        .await
        .map_err(error)?;
    if !matches!(state.auth.active(&principal).await, Ok(true)) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let bytes = serde_json::to_vec(&page)
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    let stream = futures_util::stream::unfold((Some(bytes), lease), |(bytes, lease)| async move {
        bytes.map(|b| {
            (
                Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(b)),
                (None, lease),
            )
        })
    });
    Response::builder()
        .header("Content-Type", "application/json")
        .header("Cache-Control", "no-store")
        .body(Body::from_stream(stream))
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))
}
pub async fn metadata(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let principal = member(&state, &headers, false).await?;
    let doc = doc(&id)?;
    let value = state
        .database
        .document_metadata(principal.identity, doc)
        .await
        .map_err(error)?;
    crate::analysis_api::deliver(&state, &principal, doc, value).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataWrite {
    schema_version: u32,
    expected_revision: u64,
    title: String,
}
pub async fn set_metadata(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<MetadataWrite>,
) -> Result<Json<Metadata>, ApiError> {
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "catalog_version"));
    }
    let principal = member(&state, &headers, true).await?;
    Ok(Json(
        state
            .database
            .set_document_metadata(
                principal.identity,
                doc(&id)?,
                input.expected_revision,
                input.title,
            )
            .await
            .map_err(error)?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FavoriteWrite {
    schema_version: u32,
    expected_revision: u64,
    favorite: bool,
}
pub async fn favorite(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<FavoriteWrite>,
) -> Result<Json<LibraryPreference>, ApiError> {
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "catalog_version"));
    }
    let principal = member(&state, &headers, true).await?;
    Ok(Json(
        state
            .database
            .set_library_preference(
                principal.identity,
                doc(&id)?,
                Some(input.expected_revision),
                Some(input.favorite),
            )
            .await
            .map_err(error)?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Touch {
    schema_version: u32,
}
pub async fn touch(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<Touch>,
) -> Result<Json<LibraryPreference>, ApiError> {
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "catalog_version"));
    }
    let principal = member(&state, &headers, true).await?;
    Ok(Json(
        state
            .database
            .set_library_preference(principal.identity, doc(&id)?, None, None)
            .await
            .map_err(error)?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutWrite {
    schema_version: u32,
    expected_revision: u64,
    layout: LibraryLayout,
}
pub async fn layout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<LayoutWrite>,
) -> Result<Json<LayoutPreference>, ApiError> {
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "catalog_version"));
    }
    let principal = member(&state, &headers, true).await?;
    Ok(Json(
        state
            .database
            .set_library_layout(principal.identity, input.expected_revision, input.layout)
            .await
            .map_err(error)?,
    ))
}
