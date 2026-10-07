//! Authenticated bounded preview and atomic, retry-stable XLSX publication.
use crate::{
    AppState, catalog_api, documents::ApiError, pxel_workbook, xlsx_candidate, xlsx_sandbox,
    xlsx_worker,
};
use axum::{
    Json,
    extract::{FromRequest, State},
    http::{HeaderMap, StatusCode},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spreadsheet_domain::workbook::DocumentId;
use spreadsheet_xlsx::{
    CompatibilityIssue, NormalizedCell, NormalizedCellValue, NormalizedWorkbook,
};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    schema_version: u32,
    file_base64: String,
    selected_sheets: Option<Vec<u16>>,
    title: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publication {
    schema_version: u32,
    file_base64: String,
    selected_sheets: Vec<u16>,
    title: String,
    document_id: String,
    request_id: String,
    preview_digest: String,
    accept_reported: bool,
}

#[derive(Serialize)]
pub struct Preview {
    schema_version: u32,
    selected_sheets: Vec<u16>,
    sheets: Vec<Sheet>,
    issues: Vec<CompatibilityIssue>,
    formula_issues: Vec<xlsx_candidate::FormulaIssue>,
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
        formula_issues: vec![],
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

struct HashWriter<'a>(&'a mut Sha256);
impl std::io::Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn with_title(mut preview: Preview, file: &crate::pxel_file::Book) -> Result<Preview, ApiError> {
    if !spreadsheet_storage::valid_title(&file.title) {
        return Err(options_error());
    }
    let mut hash = Sha256::new();
    hash.update(b"PXEL-XLSX-PUBLICATION-PREVIEW-1");
    hash.update(preview.preview_digest.as_bytes());
    serde_json::to_writer(
        HashWriter(&mut hash),
        &(file, &preview.issues, &preview.formula_issues),
    )
    .map_err(|_| unavailable())?;
    preview.preview_digest = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(preview)
}

pub async fn preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Result<Json<Preview>, ApiError> {
    let principal = catalog_api::member(&state, &headers, true).await?;
    let (input, sandbox, permit) = input::<Request>(&state, request).await?;
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "xlsx_version"));
    }
    if input
        .title
        .as_ref()
        .is_some_and(|title| !spreadsheet_storage::valid_title(title))
    {
        return Err(options_error());
    }
    let (bytes, selected, title, permit) = tokio::task::spawn_blocking(move || {
        Ok::<_, ApiError>((
            decode(&input.file_base64)?,
            input.selected_sheets,
            input.title,
            permit,
        ))
    })
    .await
    .map_err(|_| unavailable())??;
    let book = xlsx_worker::load_isolated(&bytes, &sandbox)
        .await
        .map_err(worker_error)?;
    let actor = principal.identity.actor_id;
    let (preview, _permit) = tokio::task::spawn_blocking(move || {
        let mut preview = make_preview(book.clone(), &bytes, selected)?;
        let bind_candidate = title.is_some();
        let file = xlsx_candidate::convert(
            &book,
            &preview.selected_sheets,
            title.unwrap_or_else(|| "Импорт XLSX".into()),
        )
        .map_err(candidate_error)?;
        preview.formula_issues = xlsx_candidate::formula_issues(&file, &preview.selected_sheets)
            .map_err(candidate_error)?;
        if bind_candidate {
            preview = with_title(preview, &file)?;
        }
        let request = Uuid::now_v7();
        let document = DocumentId::from_bytes(*request.as_bytes()).map_err(|_| options_error())?;
        pxel_workbook::prepare_xlsx(
            &file,
            document,
            actor,
            request,
            preview.preview_digest.as_bytes(),
        )
        .map_err(candidate_error)?;
        Ok::<_, ApiError>((preview, permit))
    })
    .await
    .map_err(|_| unavailable())??;
    if !matches!(state.auth.active(&principal).await, Ok(true)) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(Json(preview))
}

fn unavailable() -> ApiError {
    ApiError(StatusCode::SERVICE_UNAVAILABLE, "xlsx_unavailable")
}
fn candidate_error(error: crate::pxel_file::FileError) -> ApiError {
    ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        match error {
            crate::pxel_file::FileError::Limit => "xlsx_profile_limit",
            _ => "xlsx_profile",
        },
    )
}

async fn input<T: serde::de::DeserializeOwned>(
    state: &Arc<AppState>,
    request: axum::extract::Request,
) -> Result<(T, xlsx_sandbox::Sandbox, tokio::sync::OwnedSemaphorePermit), ApiError> {
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
        Json::<T>::from_request(request, state),
    )
    .await
    .map_err(|_| ApiError(StatusCode::REQUEST_TIMEOUT, "xlsx_request_timeout"))?
    .map_err(|rejection| ApiError(rejection.status(), "xlsx_options"))?;
    Ok((input, sandbox, permit))
}

fn decode(encoded: &str) -> Result<Zeroizing<Vec<u8>>, ApiError> {
    let maximum = spreadsheet_xlsx::worker_protocol::MAX_SOURCE_BYTES;
    if encoded.len() > maximum.div_ceil(3) * 4 {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "xlsx_limit"));
    }
    let bytes = Zeroizing::new(
        STANDARD
            .decode(encoded)
            .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "xlsx_encoding"))?,
    );
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "xlsx_limit"));
    }
    Ok(bytes)
}

pub async fn import(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: axum::extract::Request,
) -> Result<(StatusCode, Json<crate::documents::Created>), ApiError> {
    let principal = catalog_api::member(&state, &headers, true).await?;
    let (input, sandbox, permit) = input::<Publication>(&state, request).await?;
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "xlsx_version"));
    }
    if !spreadsheet_storage::valid_title(&input.title)
        || input.preview_digest.len() != 64
        || !input
            .preview_digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(options_error());
    }
    let document = DocumentId::from_bytes(*catalog_api::uuid(&input.document_id)?.as_bytes())
        .map_err(|_| options_error())?;
    let request_id = catalog_api::uuid(&input.request_id)?;
    let (bytes, input, permit) = tokio::task::spawn_blocking(move || {
        Ok::<_, ApiError>((decode(&input.file_base64)?, input, permit))
    })
    .await
    .map_err(|_| unavailable())??;
    let book = xlsx_worker::load_isolated(&bytes, &sandbox)
        .await
        .map_err(worker_error)?;
    let actor = principal.identity.actor_id;
    let (prepared, title, _permit) = tokio::task::spawn_blocking(move || {
        let mut preview = make_preview(book.clone(), &bytes, Some(input.selected_sheets))?;
        let file = xlsx_candidate::convert(&book, &preview.selected_sheets, input.title.clone())
            .map_err(candidate_error)?;
        preview.formula_issues = xlsx_candidate::formula_issues(&file, &preview.selected_sheets)
            .map_err(candidate_error)?;
        preview = with_title(preview, &file)?;
        if preview.preview_digest != input.preview_digest {
            return Err(ApiError(StatusCode::CONFLICT, "xlsx_preview_changed"));
        }
        if !input.accept_reported
            && (!preview.formula_issues.is_empty()
                || book.issues.iter().any(|issue| {
                    issue
                        .sheet_index
                        .is_none_or(|index| preview.selected_sheets.contains(&index))
                }))
        {
            return Err(ApiError(StatusCode::CONFLICT, "xlsx_loss_acceptance"));
        }
        let mut hash = Sha256::new();
        hash.update(b"PXEL-XLSX-PUBLICATION-1");
        hash.update(preview.preview_digest.as_bytes());
        hash.update(
            serde_json::to_vec(&(
                &input.title,
                input.accept_reported,
                spreadsheet_collab::calculation::ENGINE_VERSION,
                spreadsheet_collab::calculation::COMPATIBILITY_PROFILE,
            ))
            .map_err(|_| unavailable())?,
        );
        let prepared =
            pxel_workbook::prepare_xlsx(&file, document, actor, request_id, &hash.finalize())
                .map_err(candidate_error)?;
        Ok::<_, ApiError>((prepared, input.title, permit))
    })
    .await
    .map_err(|_| unavailable())??;
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
            &title,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_digest_binds_candidate_and_compatibility_report()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = include_bytes!("../../../fixtures/xlsx/minimal_scalars.xlsx");
        let ir =
            spreadsheet_xlsx::normalize_reader(std::io::Cursor::new(bytes), bytes.len() as u64)?;
        let mut file = xlsx_candidate::convert(&ir, &[0, 1], "Импорт".into())?;
        let digest = |file: &crate::pxel_file::Book,
                      reported: bool|
         -> Result<String, Box<dyn std::error::Error>> {
            let mut preview =
                make_preview(ir.clone(), bytes, None).map_err(|_| "preview failed")?;
            if reported {
                preview.formula_issues.push(xlsx_candidate::FormulaIssue {
                    sheet_index: 0,
                    function: "UNKNOWN".into(),
                    cells: 1,
                });
            }
            Ok(with_title(preview, file)
                .map_err(|_| "digest failed")?
                .preview_digest)
        };
        let original = digest(&file, false)?;
        assert_eq!(original, digest(&file, false)?);
        assert_ne!(original, digest(&file, true)?);
        file.title = "Другая книга".into();
        assert_ne!(original, digest(&file, false)?);
        file.title = "Импорт".into();
        file.sheets[0].rows += 1;
        assert_ne!(original, digest(&file, false)?);
        Ok(())
    }

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
