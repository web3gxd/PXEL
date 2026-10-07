//! Authenticated bounded in-memory preview; never publishes a workbook.
use crate::{AppState, catalog_api, documents::ApiError, xlsx_sandbox, xlsx_worker};
use axum::{
    Json,
    extract::{FromRequest, State},
    http::{HeaderMap, StatusCode},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spreadsheet_xlsx::{
    CompatibilityIssue, NormalizedCell, NormalizedCellValue, NormalizedWorkbook,
};
use std::{sync::Arc, time::Duration};
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    schema_version: u32,
    file_base64: String,
    selected_sheets: Option<Vec<u16>>,
}

#[derive(Serialize)]
pub struct Preview {
    schema_version: u32,
    selected_sheets: Vec<u16>,
    sheets: Vec<Sheet>,
    issues: Vec<CompatibilityIssue>,
    preview_digest: String,
}

#[derive(Serialize)]
struct Sheet {
    index: u16,
    name: String,
    cells: usize,
    styles: usize,
    merges: usize,
    sample: Vec<NormalizedCell>,
}

fn options_error() -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "xlsx_options")
}

fn make_preview(
    book: NormalizedWorkbook,
    source: &[u8],
    selected: Option<Vec<u16>>,
) -> Result<Preview, ApiError> {
    let selection = selected.unwrap_or_else(|| (0..book.sheets.len() as u16).collect());
    if selection.is_empty()
        || selection.len() > book.sheets.len()
        || selection
            .iter()
            .any(|index| usize::from(*index) >= book.sheets.len())
        || !selection.windows(2).all(|pair| pair[0] < pair[1])
    {
        return Err(options_error());
    }
    let mut hash = Sha256::new();
    hash.update(b"PXEL-XLSX-PREVIEW-1");
    hash.update((source.len() as u64).to_be_bytes());
    hash.update(source);
    hash.update((selection.len() as u16).to_be_bytes());
    for index in &selection {
        hash.update(index.to_be_bytes());
    }
    let preview_digest = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let sheets = book
        .sheets
        .into_iter()
        .enumerate()
        .map(|(index, sheet)| {
            let sample = sheet
                .cells
                .iter()
                .take(40)
                .cloned()
                .map(|mut cell| {
                    match &mut cell.value {
                        NormalizedCellValue::Text(text) | NormalizedCellValue::Formula(text) => {
                            *text = text.chars().take(256).collect();
                        }
                        _ => {}
                    }
                    cell
                })
                .collect();
            Sheet {
                index: index as u16,
                name: sheet.name,
                cells: sheet.cells.len(),
                styles: sheet.styles.len(),
                merges: sheet.merges.len(),
                sample,
            }
        })
        .collect();
    Ok(Preview {
        schema_version: 1,
        selected_sheets: selection,
        sheets,
        issues: book.issues,
        preview_digest,
    })
}

fn worker_error(error: xlsx_worker::WorkerError) -> ApiError {
    match error {
        xlsx_worker::WorkerError::InvalidInput | xlsx_worker::WorkerError::Rejected => {
            ApiError(StatusCode::UNPROCESSABLE_ENTITY, "xlsx_rejected")
        }
        _ => ApiError(StatusCode::SERVICE_UNAVAILABLE, "xlsx_unavailable"),
    }
}

pub async fn preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Result<Json<Preview>, ApiError> {
    let principal = catalog_api::member(&state, &headers, true).await?;
    let permit = state
        .exports
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "import_busy"))?;
    let path = std::env::var_os("PXEL_XLSX_SANDBOX_MANIFEST").ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "xlsx_unavailable",
    ))?;
    let sandbox = xlsx_sandbox::Sandbox::load(path.into())
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "xlsx_unavailable"))?;
    let Json(input) = tokio::time::timeout(
        Duration::from_secs(15),
        Json::<Request>::from_request(request, &state),
    )
    .await
    .map_err(|_| ApiError(StatusCode::REQUEST_TIMEOUT, "xlsx_request_timeout"))?
    .map_err(|rejection| ApiError(rejection.status(), "xlsx_options"))?;
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "xlsx_version"));
    }
    let maximum = spreadsheet_xlsx::worker_protocol::MAX_SOURCE_BYTES;
    if input.file_base64.len() > maximum.div_ceil(3) * 4 {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "xlsx_limit"));
    }
    let (bytes, selected, permit) = tokio::task::spawn_blocking(move || {
        let bytes = Zeroizing::new(
            STANDARD
                .decode(input.file_base64)
                .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "xlsx_encoding"))?,
        );
        if bytes.is_empty() || bytes.len() > maximum {
            return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "xlsx_limit"));
        }
        Ok((bytes, input.selected_sheets, permit))
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "xlsx_unavailable"))??;
    let book = xlsx_worker::load_isolated(&bytes, &sandbox)
        .await
        .map_err(worker_error)?;
    let (preview, _permit) = tokio::task::spawn_blocking(move || {
        make_preview(book, &bytes, selected).map(|preview| (preview, permit))
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "xlsx_unavailable"))??;
    if !matches!(state.auth.active(&principal).await, Ok(true)) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(Json(preview))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_and_digest_bind_exact_uploaded_content() -> Result<(), Box<dyn std::error::Error>>
    {
        let bytes = include_bytes!("../../../fixtures/xlsx/minimal_scalars.xlsx");
        let book =
            spreadsheet_xlsx::normalize_reader(std::io::Cursor::new(bytes), bytes.len() as u64)?;
        let all = make_preview(book.clone(), bytes, None).map_err(|_| "preview failed")?;
        let selected =
            make_preview(book.clone(), bytes, Some(vec![1])).map_err(|_| "preview failed")?;
        assert_eq!(all.selected_sheets, vec![0, 1]);
        assert_ne!(all.preview_digest, selected.preview_digest);
        for selection in [vec![], vec![1, 0], vec![0, 0], vec![2]] {
            assert!(make_preview(book.clone(), bytes, Some(selection)).is_err());
        }
        assert!(
            serde_json::from_str::<Request>(
                r#"{"schema_version":1,"file_base64":"","runtime_path":"private"}"#
            )
            .is_err()
        );
        Ok(())
    }
}
