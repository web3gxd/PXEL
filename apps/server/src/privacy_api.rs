//! Recognition and value-only redaction on a separate fixed-version projection.
use crate::{AppState, documents::ApiError};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use spreadsheet_application::journal::AuthenticatedActor;
use spreadsheet_collab::SessionDirectory;
use spreadsheet_domain::{
    privacy::{self, SensitiveMatch},
    workbook::{CellAddress, CellContent, CellValue, DocumentId, Workbook},
};
use spreadsheet_formula::{
    Value,
    calculation::{CalculationLimits, CalculationProjection},
};
use spreadsheet_storage::{PrivacyPolicy, PrivacyRules};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, atomic::AtomicBool},
};
use uuid::Uuid;

fn limited() -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, "privacy_analysis_limit")
}
pub(crate) fn text_mask(text: &str, rules: &PrivacyRules) -> Result<String, ApiError> {
    let kinds = rules.kinds().map_err(crate::disclosure_api::error)?;
    Ok(privacy::mask(
        text,
        &privacy::recognize(text, &kinds).map_err(|_| limited())?,
    ))
}
#[derive(Serialize)]
pub(crate) struct Finding {
    pub sheet_id: String,
    pub row_id: String,
    pub column_id: String,
    pub row: u32,
    pub column: u32,
    pub kind: &'static str,
    pub fingerprint: String,
}
pub(crate) struct Analysis {
    pub values: BTreeMap<CellAddress, CellValue>,
    pub findings: Vec<Finding>,
    pub affected: usize,
}
fn scalar(value: &Value) -> CellValue {
    match value {
        Value::Blank => CellValue::Blank,
        Value::Number(n) => CellValue::Number(*n),
        Value::Text(t) => CellValue::Text(t.clone()),
        Value::Boolean(b) => CellValue::Boolean(*b),
        Value::Error(e) => CellValue::Text(e.code().into()),
        Value::DateTime { serial, .. } => CellValue::Number(*serial),
        Value::Array { .. } => CellValue::Text("[МАССИВ]".into()),
    }
}
pub(crate) fn analyze(book: &Workbook, rules: &PrivacyRules) -> Result<Analysis, ApiError> {
    let kinds = rules.kinds().map_err(crate::disclosure_api::error)?;
    if !rules.enabled || kinds.is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "privacy_recognition_disabled",
        ));
    }
    if book.calculation_context().is_none() && book.sheets().iter().flat_map(|s|s.cells().values()).any(|v| matches!(v,CellContent::Formula(f) if f.ast().nodes().iter().any(|node|matches!(node,spreadsheet_domain::formula::Node::Call{name,..} if matches!(name.as_str(),"RAND"|"NOW"|"TODAY"))))) { return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY,"privacy_context_missing")); }
    let limits = CalculationLimits::default();
    let mut calc = CalculationProjection::from_workbook(book, limits).map_err(|_| limited())?;
    calc.calculate(book, limits, &AtomicBool::new(false))
        .map_err(|_| limited())?;
    let mut values = BTreeMap::new();
    let mut matches = BTreeMap::new();
    let mut findings = Vec::new();
    let mut bytes = 0usize;
    for sheet in book.sheets() {
        let mut passport_rows = BTreeSet::new();
        let mut passport_columns = BTreeSet::new();
        let mut context_bytes = 0usize;
        if sheet.cells().len() > 100_000 {
            return Err(limited());
        }
        for (address, content) in sheet.cells() {
            let value = match content {
                CellContent::Value(v) => v.clone(),
                CellContent::Formula(_) => scalar(calc.results().get(address).ok_or_else(limited)?),
            };
            if let CellValue::Text(text) = value {
                context_bytes = context_bytes
                    .checked_add(text.len())
                    .filter(|n| *n <= 16 * 1024 * 1024)
                    .ok_or_else(limited)?;
                if privacy::passport_context(&text) {
                    passport_rows.insert(address.row);
                    passport_columns.insert(address.column);
                }
            }
        }
        for (address, content) in sheet.cells() {
            if values.len() >= 100_000 {
                return Err(limited());
            }
            let value = match content {
                CellContent::Value(v) => v.clone(),
                CellContent::Formula(_) => scalar(calc.results().get(address).ok_or_else(limited)?),
            };
            let mut text = match &value {
                CellValue::Text(t) => t.clone(),
                CellValue::Number(n) => n.to_string(),
                _ => String::new(),
            };
            let formula = matches!(content, CellContent::Formula(_));
            if let CellContent::Formula(source) = content {
                text.push('\n');
                text.push_str(source.source());
            }
            bytes = bytes
                .checked_add(text.len())
                .filter(|n| *n <= 16 * 1024 * 1024)
                .ok_or_else(limited)?;
            let mut scope = Vec::with_capacity(48);
            scope.extend_from_slice(address.sheet.as_bytes());
            scope.extend_from_slice(address.row.as_bytes());
            scope.extend_from_slice(address.column.as_bytes());
            let detected: Vec<SensitiveMatch> = privacy::recognize_with_passport_context(
                &text,
                &kinds,
                passport_rows.contains(&address.row) || passport_columns.contains(&address.column),
            )
            .map_err(|_| limited())?
            .into_iter()
            .filter(|m| {
                !rules
                    .exceptions
                    .contains(&privacy::fingerprint(&scope, &text, m.kind))
            })
            .collect();
            for m in &detected {
                if findings.len() >= 100_000 {
                    return Err(limited());
                }
                findings.push(Finding {
                    sheet_id: Uuid::from_bytes(*address.sheet.as_bytes()).to_string(),
                    row_id: Uuid::from_bytes(*address.row.as_bytes()).to_string(),
                    column_id: Uuid::from_bytes(*address.column.as_bytes()).to_string(),
                    row: sheet.row_position(address.row).ok_or_else(limited)?,
                    column: sheet.column_position(address.column).ok_or_else(limited)?,
                    kind: m.kind.name(),
                    fingerprint: privacy::fingerprint(&scope, &text, m.kind),
                });
            }
            if !detected.is_empty() {
                matches.insert(
                    *address,
                    if formula {
                        ("[СКРЫТО]".into(), vec![])
                    } else {
                        (text, detected)
                    },
                );
            }
            // Arrays cannot be represented by this scalar view; retain no array data.
            if matches!(content, CellContent::Formula(_))
                && matches!(calc.results().get(address), Some(Value::Array { .. }))
            {
                matches
                    .entry(*address)
                    .or_insert_with(|| ("[СКРЫТО]".into(), vec![]));
            }
            values.insert(*address, value);
        }
    }
    let affected = calc
        .dependent_closure(book, matches.keys().copied(), 1_000_000)
        .map_err(|_| limited())?;
    for address in &affected {
        let value = if let Some((text, found)) = matches.get(address) {
            if found.is_empty() {
                "[СКРЫТО]".into()
            } else {
                privacy::mask(text, found)
            }
        } else {
            "[СКРЫТО: ЗАВИСИМОСТЬ]".into()
        };
        values.insert(*address, CellValue::Text(value));
    }
    Ok(Analysis {
        values,
        findings,
        affected: affected.len(),
    })
}
pub(crate) async fn safe_view(
    state: &AppState,
    identity: AuthenticatedActor,
    doc: DocumentId,
    link: Option<Uuid>,
) -> Result<crate::documents::RedactedView, ApiError> {
    let policy = state
        .database
        .privacy_policy(identity, doc, link)
        .await
        .map_err(crate::disclosure_api::error)?;
    let session = state
        .sessions
        .session(identity, doc)
        .await
        .map_err(crate::documents::session_api_error)?;
    let book = session
        .read_confirmed(identity)
        .await
        .map_err(crate::documents::session_api_error)?;
    let (copy, export, _, stamp) = state
        .database
        .disclosure_capabilities(identity, doc, link)
        .await
        .map_err(crate::disclosure_api::error)?;
    if stamp.document_version != book.version().get() {
        return Err(ApiError(StatusCode::CONFLICT, "document_changed"));
    }
    let role = if link.is_some() {
        false
    } else {
        state
            .database
            .document_role_name(identity, doc)
            .await
            .map_err(|_| limited())?
            .as_deref()
            == Some("owner")
    };
    let comment = if let Some(link) = link {
        state
            .database
            .share_guest_role(identity, doc, link)
            .await
            .map_err(crate::disclosure_api::error)?
    } else {
        false
    };
    let slot = state
        .history_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "analysis_busy"))?;
    let pxel = state
        .database
        .can_export_pxel(identity, doc, link)
        .await
        .map_err(crate::disclosure_api::error)?;
    let csv = state
        .database
        .can_export_csv(identity, doc, link)
        .await
        .map_err(crate::disclosure_api::error)?;
    tokio::task::spawn_blocking(move || {
        let _slot = slot;
        let analysis = analyze(&book, &policy.rules)?;
        crate::documents::redacted_view(
            &book,
            &policy,
            &analysis,
            crate::documents::RedactedCapabilities {
                can_copy: copy,
                can_export: export,
                can_export_pxel: pxel,
                can_export_csv: csv,
                can_comment: comment,
                can_share: role,
                is_guest: link.is_some(),
                stamp,
            },
        )
    })
    .await
    .map_err(|_| limited())?
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyWrite {
    pub schema_version: u32,
    pub document_version: String,
    pub expected_revision: u64,
    pub rules: PrivacyRules,
}
pub async fn get_policy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let principal = state
        .auth
        .authenticate(&headers)
        .await
        .map_err(|_| limited())?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let doc = DocumentId::from_bytes(crate::documents::parse_id(&id)?).map_err(|_| limited())?;
    if state
        .database
        .document_role_name(principal.identity, doc)
        .await
        .map_err(|_| limited())?
        .as_deref()
        != Some("owner")
    {
        return Err(ApiError(StatusCode::FORBIDDEN, "owner_required"));
    }
    let policy = state
        .database
        .privacy_policy(principal.identity, doc, None)
        .await
        .map_err(crate::disclosure_api::error)?;
    crate::analysis_api::deliver(&state, &principal, doc, policy).await
}
pub async fn set_policy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<PolicyWrite>,
) -> Result<Json<PrivacyPolicy>, ApiError> {
    let request = crate::analysis_api::AnalysisRequest {
        schema_version: input.schema_version,
        document_version: input.document_version,
    };
    let (principal, doc, book) =
        crate::analysis_api::read_book(&state, &headers, &id, &request, true).await?;
    let gate = state.delivery.get(doc).await;
    let _guard = tokio::time::timeout(std::time::Duration::from_secs(3), gate.lock.write())
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "send_gate_busy"))?;
    let policy = state
        .database
        .set_privacy_policy(
            principal.identity,
            doc,
            book.version().get(),
            input.expected_revision,
            input.rules,
        )
        .await
        .map_err(crate::disclosure_api::error)?;
    gate.committed_epoch(policy.access_epoch);
    Ok(Json(policy))
}
#[derive(Serialize)]
struct AnalysisView {
    schema_version: u32,
    document_version: String,
    policy_revision: u64,
    affected: usize,
    findings: Vec<Finding>,
    projection: crate::documents::RedactedView,
}
pub async fn analyze_owner(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<crate::analysis_api::AnalysisRequest>,
) -> Result<Response, ApiError> {
    let (principal, doc, book) =
        crate::analysis_api::read_book(&state, &headers, &id, &input, true).await?;
    let policy = state
        .database
        .privacy_policy(principal.identity, doc, None)
        .await
        .map_err(crate::disclosure_api::error)?;
    let (copy, export, _, stamp) = state
        .database
        .disclosure_capabilities(principal.identity, doc, None)
        .await
        .map_err(crate::disclosure_api::error)?;
    let slot = state
        .history_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "analysis_busy"))?;
    let pxel = state
        .database
        .can_export_pxel(principal.identity, doc, None)
        .await
        .map_err(crate::disclosure_api::error)?;
    let csv = state
        .database
        .can_export_csv(principal.identity, doc, None)
        .await
        .map_err(crate::disclosure_api::error)?;
    let view = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        let analysis = analyze(&book, &policy.rules)?;
        let projection = crate::documents::redacted_view(
            &book,
            &policy,
            &analysis,
            crate::documents::RedactedCapabilities {
                can_copy: copy,
                can_export: export,
                can_export_pxel: pxel,
                can_export_csv: csv,
                can_comment: false,
                can_share: true,
                is_guest: false,
                stamp,
            },
        )?;
        Ok::<_, ApiError>(AnalysisView {
            schema_version: 1,
            document_version: book.version().get().to_string(),
            policy_revision: policy.revision,
            affected: analysis.affected,
            findings: analysis.findings,
            projection,
        })
    })
    .await
    .map_err(|_| limited())??;
    crate::analysis_api::deliver(&state, &principal, doc, view).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_domain::workbook::*;
    #[test]
    fn passport_labels_apply_only_to_their_sheet_axes_and_exceptions_keep_originals()
    -> Result<(), Box<dyn std::error::Error>> {
        let sheet = SheetId::from_bytes([1; 16])?;
        let rows = (2..6)
            .map(|n| RowId::from_bytes([n; 16]))
            .collect::<Result<Vec<_>, _>>()?;
        let columns = (6..9)
            .map(|n| ColumnId::from_bytes([n; 16]))
            .collect::<Result<Vec<_>, _>>()?;
        let mut book = Workbook::empty(DocumentId::from_bytes([9; 16])?).candidate(
            &Operation::CreateWorkbook {
                sheet: SheetSeed {
                    id: sheet,
                    name: "Тест".into(),
                    rows: rows.clone(),
                    columns: columns.clone(),
                },
            },
            DocumentVersion::new(1)?,
        )?;
        let cell = |r: usize, c: usize| CellAddress {
            sheet,
            row: rows[r],
            column: columns[c],
        };
        for (r, c, text) in [
            (0, 0, "Паспорт"),
            (1, 0, "4501123456"),
            (1, 1, "4501123456"),
            (1, 2, "4501123456"),
            (2, 1, "Серия и номер"),
            (2, 2, "45 01 345678"),
            (3, 0, "450112345"),
        ] {
            book = book.candidate(
                &Operation::SetCellValue {
                    address: cell(r, c),
                    value: CellValue::Text(text.into()),
                },
                DocumentVersion::new(book.version().get() + 1)?,
            )?;
        }
        let hash = book.hash();
        let mut rules = PrivacyRules {
            enabled: true,
            types: vec!["passport".into()],
            exceptions: vec![],
        };
        let analysis = analyze(&book, &rules).map_err(|e| e.1)?;
        assert_eq!(analysis.findings.len(), 3);
        assert_eq!(
            analysis.values[&cell(1, 0)],
            CellValue::Text("[СКРЫТО]".into())
        );
        assert_eq!(
            analysis.values[&cell(2, 2)],
            CellValue::Text("[СКРЫТО]".into())
        );
        assert_eq!(
            analysis.values[&cell(1, 1)],
            CellValue::Text("[СКРЫТО]".into())
        );
        assert_eq!(
            analysis.values[&cell(1, 2)],
            CellValue::Text("4501123456".into())
        );
        assert_eq!(
            analysis.values[&cell(3, 0)],
            CellValue::Text("450112345".into())
        );
        rules.exceptions = analysis
            .findings
            .iter()
            .map(|f| f.fingerprint.clone())
            .collect();
        let excluded = analyze(&book, &rules).map_err(|e| e.1)?;
        assert!(excluded.findings.is_empty());
        assert_eq!(
            excluded.values[&cell(1, 0)],
            CellValue::Text("4501123456".into())
        );
        assert_eq!(book.hash(), hash);
        Ok(())
    }
    #[test]
    fn masks_potential_dependencies_literals_and_metadata_without_mutating_the_book()
    -> Result<(), Box<dyn std::error::Error>> {
        let sheet = SheetId::from_bytes([1; 16])?;
        let row = RowId::from_bytes([2; 16])?;
        let columns = (3..8)
            .map(|n| ColumnId::from_bytes([n; 16]))
            .collect::<Result<Vec<_>, _>>()?;
        let mut book = Workbook::empty(DocumentId::from_bytes([8; 16])?).candidate(
            &Operation::CreateWorkbook {
                sheet: SheetSeed {
                    id: sheet,
                    name: "test@example.com".into(),
                    rows: vec![row],
                    columns: columns.clone(),
                },
            },
            DocumentVersion::new(1)?,
        )?;
        let cell = |i: usize| CellAddress {
            sheet,
            row,
            column: columns[i],
        };
        book = book.candidate(
            &Operation::SetCellValue {
                address: cell(0),
                value: CellValue::Text("Связь: test@example.com".into()),
            },
            DocumentVersion::new(2)?,
        )?;
        for (i, source) in [
            (1, "=LEN(A1)"),
            (2, "=IF(FALSE,A1,\"public\")"),
            (3, "=LEN(\"other@example.org\")"),
            (4, "=2+3"),
        ] {
            book = book.candidate(
                &Operation::SetCellFormula {
                    address: cell(i),
                    formula: spreadsheet_formula::binder::bind(source, &book, cell(i))?,
                },
                DocumentVersion::new(book.version().get() + 1)?,
            )?;
        }
        let rules = PrivacyRules {
            enabled: true,
            types: vec!["email".into()],
            exceptions: vec![],
        };
        let hash = book.hash();
        let analysis = analyze(&book, &rules).map_err(|e| e.1)?;
        assert_eq!(analysis.affected, 4);
        assert_eq!(
            analysis.values[&cell(0)],
            CellValue::Text("Связь: [СКРЫТО]".into())
        );
        assert_eq!(
            analysis.values[&cell(1)],
            CellValue::Text("[СКРЫТО: ЗАВИСИМОСТЬ]".into())
        );
        assert_eq!(analysis.values[&cell(2)], analysis.values[&cell(1)]);
        assert_eq!(
            analysis.values[&cell(3)],
            CellValue::Text("[СКРЫТО]".into())
        );
        assert_eq!(analysis.values[&cell(4)], CellValue::Number(5.));
        let policy = PrivacyPolicy {
            schema_version: 1,
            revision: 1,
            rules: rules.clone(),
            access_epoch: 1,
        };
        let view = crate::documents::redacted_view(
            &book,
            &policy,
            &analysis,
            crate::documents::RedactedCapabilities {
                can_copy: true,
                can_export: false,
                can_export_pxel: false,
                can_export_csv: false,
                can_comment: true,
                can_share: false,
                is_guest: true,
                stamp: spreadsheet_storage::DisclosureStamp {
                    document_version: book.version().get(),
                    book_revision: 1,
                    link_revision: 1,
                },
            },
        )
        .map_err(|e| e.1)?;
        let text = serde_json::to_string(&view)?;
        for forbidden in [
            "test@example.com",
            "other@example.org",
            "bound_formula",
            "calculation_context",
            "random_seed",
            "state_hash",
            "=LEN",
            "source",
            "history",
        ] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
        assert_eq!(book.hash(), hash);
        assert!(book.calculation_context().is_none());
        let mut except = rules;
        except.exceptions.push(
            analysis
                .findings
                .iter()
                .find(|f| f.column_id == Uuid::from_bytes(*columns[0].as_bytes()).to_string())
                .ok_or("missing finding")?
                .fingerprint
                .clone(),
        );
        let excluded = analyze(&book, &except).map_err(|e| e.1)?;
        assert_eq!(excluded.affected, 1);
        assert_eq!(excluded.values[&cell(1)], CellValue::Number(23.));
        assert!(analyze(&book, &PrivacyRules::default()).is_err());
        Ok(())
    }
}
