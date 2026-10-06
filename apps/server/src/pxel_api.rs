//! Passwords are POST-body-only; authenticated file work precedes atomic publication.
use crate::{AppState, auth, documents::ApiError, pxel_file, pxel_workbook};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spreadsheet_collab::SessionDirectory;
use spreadsheet_domain::workbook::DocumentId;
use spreadsheet_storage::DisclosureAction;
use std::sync::Arc;
use uuid::Uuid;
use zeroize::Zeroizing;

fn error(value: pxel_file::FileError) -> ApiError {
    use pxel_file::FileError::*;
    ApiError(
        if matches!(value, Unavailable) {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::UNPROCESSABLE_ENTITY
        },
        match value {
            Invalid => "pxel_invalid",
            Version => "pxel_version",
            Limit => "pxel_limit",
            Password => "pxel_password_length",
            Authentication => "pxel_authentication",
            Unavailable => "pxel_unavailable",
        },
    )
}
async fn member(state: &AppState, headers: &HeaderMap) -> Result<auth::Principal, ApiError> {
    if !auth::valid_origin(headers, &state.origins) {
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
fn uuid(value: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value)
        .ok()
        .filter(|id| id.get_version_num() == 7)
        .ok_or(ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    schema_version: u32,
    password: String,
    document_version: String,
    #[serde(default)]
    anonymized: bool,
}
impl Drop for ExportRequest {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.password.zeroize();
    }
}
pub async fn export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ExportRequest>,
) -> Result<Response, ApiError> {
    let principal = member(&state, &headers).await?;
    let doc = DocumentId::from_bytes(*uuid(&id)?.as_bytes())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))?;
    export_for(state, principal, doc, None, input).await
}
pub async fn guest_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ExportRequest>,
) -> Result<Response, ApiError> {
    if !auth::valid_origin(&headers, &state.origins) || !crate::rate_allowed(&state).await {
        return Err(ApiError(StatusCode::FORBIDDEN, "origin_or_rate_denied"));
    }
    let link = uuid(&id)?;
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
    export_for(state, principal, doc, Some(link), input).await
}
async fn export_for(
    state: Arc<AppState>,
    principal: auth::Principal,
    doc: DocumentId,
    link: Option<Uuid>,
    mut input: ExportRequest,
) -> Result<Response, ApiError> {
    let secret = Zeroizing::new(std::mem::take(&mut input.password));
    pxel_file::password(&secret).map_err(error)?;
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "pxel_version"));
    }
    let action = if input.anonymized {
        DisclosureAction::ExportPxelAnonymized
    } else {
        DisclosureAction::ExportPxel
    };
    let start = state
        .database
        .authorize_disclosure(principal.identity, doc, link, action, None)
        .await
        .map_err(crate::disclosure_api::error)?;
    if !start.allowed {
        return Err(ApiError(StatusCode::FORBIDDEN, "disclosure_denied"));
    }
    let session = state
        .sessions
        .session(principal.identity, doc)
        .await
        .map_err(crate::documents::session_api_error)?;
    let book = session
        .read_confirmed(principal.identity)
        .await
        .map_err(crate::documents::session_api_error)?;
    if input.document_version != book.version().get().to_string()
        || book.version().get() != start.stamp.document_version
    {
        return Err(ApiError(StatusCode::CONFLICT, "version_changed"));
    }
    let personal = if link.is_none() {
        state
            .database
            .personal_view(principal.identity, doc)
            .await
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
            .settings
    } else {
        Default::default()
    };
    let privacy = state
        .database
        .privacy_policy(principal.identity, doc, link)
        .await
        .map_err(crate::disclosure_api::error)?
        .rules;
    let metadata = state
        .database
        .document_metadata(principal.identity, doc)
        .await
        .map_err(crate::catalog_api::error)?;
    let metadata_revision = metadata.revision;
    let password_permit = state
        .passwords
        .admit()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "password_busy"))?;
    let export_permit = state
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "export_busy"))?;
    let result = tokio::task::spawn_blocking(move || {
        let _p = password_permit;
        let _e = export_permit;
        let file =
            pxel_workbook::export(&book, &personal, &privacy, start.anonymized, metadata.title)?;
        pxel_file::encrypt(&file, &secret)
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "pxel_unavailable"))?;
    let bytes = match result {
        Ok(bytes) => bytes,
        Err(e) => {
            state
                .database
                .record_disclosure_failure(principal.identity, doc, link, action, start.stamp)
                .await
                .map_err(crate::disclosure_api::error)?;
            return Err(error(e));
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
    if state
        .database
        .document_metadata(principal.identity, doc)
        .await
        .map_err(crate::catalog_api::error)?
        .revision
        != metadata_revision
    {
        return Err(ApiError(StatusCode::CONFLICT, "version_changed"));
    }
    crate::disclosure_api::protected_response(
        bytes,
        "application/vnd.pxel.workbook",
        Some("attachment; filename=PXEL.pxel"),
        lease,
        guard,
        true,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    schema_version: u32,
    password: String,
    file_base64: String,
    document_id: String,
    request_id: String,
}
impl Drop for ImportRequest {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.password.zeroize();
    }
}
#[derive(Serialize)]
pub struct Composition {
    schema_version: u32,
    title: String,
    sheets: Vec<CompositionSheet>,
    formulas: usize,
    anonymized: bool,
    features: Vec<&'static str>,
    omitted_filters: u32,
}
#[derive(Serialize)]
struct CompositionSheet {
    name: String,
    rows: u32,
    columns: u32,
    cells: usize,
}
async fn prepared(
    state: &Arc<AppState>,
    principal: &auth::Principal,
    mut input: ImportRequest,
) -> Result<(pxel_workbook::Prepared, Composition), ApiError> {
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "pxel_version"));
    }
    let secret = Zeroizing::new(std::mem::take(&mut input.password));
    pxel_file::password(&secret).map_err(error)?;
    if input.file_base64.len() > pxel_file::MAX_FILE.div_ceil(3) * 4 {
        return Err(error(pxel_file::FileError::Limit));
    }
    let bytes = Zeroizing::new(
        STANDARD
            .decode(&input.file_base64)
            .map_err(|_| error(pxel_file::FileError::Invalid))?,
    );
    let doc = DocumentId::from_bytes(*uuid(&input.document_id)?.as_bytes())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))?;
    let request = uuid(&input.request_id)?;
    let actor = principal.identity.actor_id;
    let permit = state
        .passwords
        .admit()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "password_busy"))?;
    let import_permit = state
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "import_busy"))?;
    tokio::task::spawn_blocking(move || {
        let _p = permit;
        let _i = import_permit;
        let input = pxel_file::decrypt(&bytes, &secret)?;
        let digest = Sha256::digest(&*bytes);
        let prepared = pxel_workbook::prepare(&input, doc, actor, request, &digest)?;
        let composition = Composition {
            schema_version: 1,
            title: input.title,
            sheets: input
                .sheets
                .iter()
                .map(|s| CompositionSheet {
                    name: s.name.clone(),
                    rows: s.rows,
                    columns: s.columns,
                    cells: s.cells.len(),
                })
                .collect(),
            formulas: input
                .sheets
                .iter()
                .flat_map(|s| &s.cells)
                .filter(|c| matches!(c.content, pxel_file::Content::Formula(_)))
                .count(),
            anonymized: input.anonymized,
            features: vec![
                "Формулы и оформление",
                "Объединения и размеры",
                "Закрепления",
                "Личные представления и диагностика",
            ],
            omitted_filters: input.settings.omitted_stale_filters,
        };
        Ok((prepared, composition))
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "pxel_unavailable"))?
    .map_err(error)
}
pub async fn preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<ImportRequest>,
) -> Result<Json<Composition>, ApiError> {
    let principal = member(&state, &headers).await?;
    let (_, composition) = prepared(&state, &principal, input).await?;
    if !matches!(state.auth.active(&principal).await, Ok(true)) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(Json(composition))
}
pub async fn import(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(input): Json<ImportRequest>,
) -> Result<(StatusCode, Json<crate::documents::Created>), ApiError> {
    let principal = member(&state, &headers).await?;
    let (prepared, composition) = prepared(&state, &principal, input).await?;
    if !matches!(state.auth.active(&principal).await, Ok(true)) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    let receipt = state
        .database
        .create_pxel_imported(
            principal.identity,
            &prepared.events,
            &prepared.personal,
            &prepared.privacy,
            &composition.title,
        )
        .await
        .map_err(crate::documents::journal_error)?;
    Ok((
        StatusCode::CREATED,
        Json(crate::documents::Created {
            document_id: Uuid::from_bytes(*prepared.book.document_id().as_bytes()).to_string(),
            version: receipt.current_head_version.get().to_string(),
        }),
    ))
}
