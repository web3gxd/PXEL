//! CSV preview and publication share validated encrypted new-book persistence.
use crate::{AppState, catalog_api, csv_import, documents::ApiError, pxel_file, pxel_workbook};
use axum::{
    Json,
    extract::{FromRequest, State},
    http::{HeaderMap, StatusCode},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spreadsheet_domain::workbook::DocumentId;
use std::sync::Arc;
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Delimiter {
    Comma,
    Semicolon,
    Tab,
}
impl Delimiter {
    fn byte(self) -> u8 {
        match self {
            Self::Comma => b',',
            Self::Semicolon => b';',
            Self::Tab => b'\t',
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    schema_version: u32,
    file_base64: String,
    title: String,
    delimiter: Delimiter,
    #[serde(default)]
    column_types: Vec<csv_import::ColumnType>,
    #[serde(default)]
    header: bool,
    document_id: String,
    request_id: String,
    #[serde(default)]
    preview_digest: Option<String>,
}
#[derive(Serialize)]
pub struct Preview {
    schema_version: u32,
    title: String,
    rows: usize,
    columns: usize,
    column_types: Vec<csv_import::ColumnType>,
    sample: Vec<Vec<String>>,
    preview_digest: String,
}
fn error(value: csv_import::CsvError) -> ApiError {
    ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        match value {
            csv_import::CsvError::Syntax => "csv_syntax",
            csv_import::CsvError::Limit => "csv_limit",
            csv_import::CsvError::Type => "csv_type",
        },
    )
}
async fn prepared(
    actor: spreadsheet_domain::workbook::ActorId,
    input: Request,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<
    (
        pxel_workbook::Prepared,
        Preview,
        tokio::sync::OwnedSemaphorePermit,
    ),
    ApiError,
> {
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "csv_version"));
    }
    if !spreadsheet_storage::valid_title(&input.title) || input.column_types.len() > 1000 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "csv_options"));
    }
    if input.file_base64.len() > csv_import::MAX_SOURCE_BYTES.div_ceil(3) * 4 {
        return Err(error(csv_import::CsvError::Limit));
    }
    let document = DocumentId::from_bytes(*catalog_api::uuid(&input.document_id)?.as_bytes())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))?;
    let request = catalog_api::uuid(&input.request_id)?;
    tokio::task::spawn_blocking(move || {
        let bytes = Zeroizing::new(
            STANDARD
                .decode(&input.file_base64)
                .map_err(|_| error(csv_import::CsvError::Syntax))?,
        );
        if bytes.len() > csv_import::MAX_SOURCE_BYTES {
            return Err(error(csv_import::CsvError::Limit));
        }
        let source = std::str::from_utf8(&bytes)
            .map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "csv_encoding"))?;
        let rows = csv_import::parse(source, input.delimiter.byte()).map_err(error)?;
        let width = rows
            .first()
            .ok_or(error(csv_import::CsvError::Syntax))?
            .len();
        let types = if input.column_types.is_empty() {
            vec![csv_import::ColumnType::Text; width]
        } else {
            input.column_types
        };
        let cells = csv_import::convert(&rows, &types, input.header).map_err(error)?;
        let options = serde_json::to_vec(&(&input.title, input.delimiter, &types, input.header))
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "csv_unavailable"))?;
        let mut hash = Sha256::new();
        hash.update(b"PXEL-CSV-IMPORT-1");
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&*bytes);
        hash.update(options);
        let digest = hash.finalize();
        let preview_digest: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        if input
            .preview_digest
            .as_ref()
            .is_some_and(|expected| *expected != preview_digest)
        {
            return Err(ApiError(StatusCode::CONFLICT, "csv_preview_changed"));
        }
        let book = pxel_file::Book {
            schema_version: 1,
            engine_version: spreadsheet_collab::calculation::ENGINE_VERSION.into(),
            compatibility_profile: spreadsheet_collab::calculation::COMPATIBILITY_PROFILE.into(),
            title: input.title.clone(),
            anonymized: false,
            sheets: vec![pxel_file::Sheet {
                name: "Лист 1".into(),
                rows: (rows.len() as u32).max(1000),
                columns: (width as u32).max(26),
                frozen_rows: 0,
                view: None,
                cells,
                styles: vec![],
                row_heights: vec![],
                column_widths: vec![],
                merges: vec![],
            }],
            settings: pxel_file::Settings::default(),
        };
        let prepared = pxel_workbook::prepare(&book, document, actor, request, &digest)
            .map_err(|_| error(csv_import::CsvError::Limit))?;
        let sample = rows
            .iter()
            .take(20)
            .map(|row| {
                row.iter()
                    .take(20)
                    .map(|value| value.chars().take(256).collect())
                    .collect()
            })
            .collect();
        Ok((
            prepared,
            Preview {
                schema_version: 1,
                title: input.title,
                rows: rows.len(),
                columns: width,
                column_types: types,
                sample,
                preview_digest,
            },
            permit,
        ))
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "csv_unavailable"))?
}
async fn input(
    state: &Arc<AppState>,
    request: axum::extract::Request,
) -> Result<(Request, tokio::sync::OwnedSemaphorePermit), ApiError> {
    // Authorization happens in the handler before buffering a large upload.
    let permit = state
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "import_busy"))?;
    let Json(input) = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        Json::<Request>::from_request(request, state),
    )
    .await
    .map_err(|_| ApiError(StatusCode::REQUEST_TIMEOUT, "csv_request_timeout"))?
    .map_err(|rejection| ApiError(rejection.status(), "csv_options"))?;
    Ok((input, permit))
}
pub async fn preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Result<Json<Preview>, ApiError> {
    let principal = catalog_api::member(&state, &headers, true).await?;
    let (input, permit) = input(&state, request).await?;
    let (_, preview, _permit) = prepared(principal.identity.actor_id, input, permit).await?;
    if !matches!(state.auth.active(&principal).await, Ok(true)) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(Json(preview))
}
pub async fn import(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Result<(StatusCode, Json<crate::documents::Created>), ApiError> {
    let principal = catalog_api::member(&state, &headers, true).await?;
    let (input, permit) = input(&state, request).await?;
    if input
        .preview_digest
        .as_ref()
        .is_none_or(|value| value.len() != 64)
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "csv_preview_required"));
    }
    let (prepared, preview, _permit) = prepared(principal.identity.actor_id, input, permit).await?;
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
            &preview.title,
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
