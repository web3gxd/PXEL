use crate::{
    AppState,
    analysis_api::{AnalysisRequest, deliver, read_book},
    documents::ApiError,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Serialize;
use spreadsheet_domain::workbook::{CellAddress, Workbook};
use spreadsheet_formula::{
    calculation::{CalculationLimits, CalculationObserver, CalculationProjection},
    profile_graph,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use uuid::Uuid;

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}
struct Timing {
    address: CellAddress,
    start_us: u64,
    duration_us: u64,
    work: u64,
    cycle_or_blocked: bool,
}
struct Observer {
    start: Instant,
    active: BTreeMap<CellAddress, (Instant, u64)>,
    timings: Vec<Timing>,
}
impl CalculationObserver for Observer {
    fn formula_started(&mut self, address: CellAddress) {
        let now = Instant::now();
        self.active
            .insert(address, (now, micros(now.duration_since(self.start))));
    }
    fn formula_finished(&mut self, address: CellAddress, work: u64, cycle_or_blocked: bool) {
        if let Some((start, start_us)) = self.active.remove(&address) {
            self.timings.push(Timing {
                address,
                start_us,
                duration_us: micros(start.elapsed()),
                work,
                cycle_or_blocked,
            });
        }
    }
}
#[derive(Serialize)]
struct AddressView {
    sheet_id: String,
    row_id: String,
    column_id: String,
    sheet_name: String,
    row: u32,
    column: u32,
}
fn address(book: &Workbook, cell: CellAddress) -> Result<AddressView, ApiError> {
    let sheet = book
        .sheets()
        .iter()
        .find(|sheet| sheet.id() == cell.sheet)
        .ok_or(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "invalid_projection",
        ))?;
    Ok(AddressView {
        sheet_id: Uuid::from_bytes(*cell.sheet.as_bytes()).to_string(),
        row_id: Uuid::from_bytes(*cell.row.as_bytes()).to_string(),
        column_id: Uuid::from_bytes(*cell.column.as_bytes()).to_string(),
        sheet_name: sheet.name().to_owned(),
        row: sheet.row_position(cell.row).ok_or(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "invalid_projection",
        ))?,
        column: sheet.column_position(cell.column).ok_or(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "invalid_projection",
        ))?,
    })
}
#[derive(Serialize)]
struct FormulaView {
    address: AddressView,
    start_us: u64,
    duration_us: u64,
    work: u64,
    cycle_or_blocked: bool,
}
#[derive(Serialize)]
struct GraphNodeView {
    address: AddressView,
    formula: bool,
}
#[derive(Serialize)]
struct GraphView {
    nodes: Vec<GraphNodeView>,
    edges: Vec<(usize, usize)>,
    truncated: bool,
    potential: bool,
}
#[derive(Serialize)]
struct ProfileView {
    schema_version: u32,
    document_version: String,
    context_id: Option<String>,
    calculation_epoch: Option<String>,
    engine_version: &'static str,
    compatibility_profile: &'static str,
    result_hash: String,
    preparation_us: u64,
    calculation_us: u64,
    total_us: u64,
    preparation_work: usize,
    graph_work: usize,
    evaluation_work: u64,
    formula_count: usize,
    evaluated_cells: usize,
    result_bytes: usize,
    formulas: Vec<FormulaView>,
    formula_rows_truncated: bool,
    graph: GraphView,
}
fn analyze(book: &Workbook) -> Result<ProfileView, ApiError> {
    if book.calculation_context().is_none() && book.sheets().iter().flat_map(|sheet| sheet.cells().values()).any(|content| {
        matches!(content, spreadsheet_domain::workbook::CellContent::Formula(formula) if formula.ast().nodes().iter().any(|node| matches!(node, spreadsheet_domain::formula::Node::Call { name, .. } if matches!(name.as_str(), "RAND" | "NOW" | "TODAY"))))
    }) {
        return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY, "profile_context_missing"));
    }
    let start = Instant::now();
    let limits = CalculationLimits::default();
    let mut projection = CalculationProjection::from_workbook(book, limits)
        .map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "profile_limit"))?;
    let preparation_us = micros(start.elapsed());
    let mut observer = Observer {
        start,
        active: BTreeMap::new(),
        timings: Vec::new(),
    };
    let calculation_start = Instant::now();
    let patch = projection
        .calculate_observed(book, limits, &AtomicBool::new(false), &mut observer)
        .map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "profile_limit"))?;
    let calculation_us = micros(calculation_start.elapsed());
    let result_hash = projection
        .result_hash()
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "profile_failed"))?;
    observer.timings.sort_by(|left, right| {
        right
            .duration_us
            .cmp(&left.duration_us)
            .then(left.address.cmp(&right.address))
    });
    let roots: Vec<_> = observer
        .timings
        .iter()
        .map(|timing| timing.address)
        .collect();
    let graph = profile_graph::graph(book, &roots)
        .map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "profile_limit"))?;
    let graph = GraphView {
        potential: true,
        truncated: graph.truncated,
        edges: graph.edges,
        nodes: graph
            .nodes
            .into_iter()
            .map(|node| {
                Ok(GraphNodeView {
                    address: address(book, node.address)?,
                    formula: node.formula,
                })
            })
            .collect::<Result<_, ApiError>>()?,
    };
    let formula_count = observer.timings.len();
    let formulas = observer
        .timings
        .into_iter()
        .take(1000)
        .map(|timing| {
            Ok(FormulaView {
                address: address(book, timing.address)?,
                start_us: timing.start_us,
                duration_us: timing.duration_us,
                work: timing.work,
                cycle_or_blocked: timing.cycle_or_blocked,
            })
        })
        .collect::<Result<_, ApiError>>()?;
    let total_us = micros(start.elapsed());
    tracing::info!(
        event = "formula_profile",
        formula_count,
        evaluated_cells = patch.evaluated_cells,
        graph_work = patch.graph_work_used,
        evaluation_work = patch.evaluation_work_used,
        preparation_us,
        total_us
    );
    Ok(ProfileView {
        schema_version: 1,
        document_version: book.version().get().to_string(),
        context_id: book
            .calculation_context()
            .map(|context| Uuid::from_bytes(*context.context_id().as_bytes()).to_string()),
        calculation_epoch: book
            .calculation_context()
            .map(|context| context.epoch().to_string()),
        engine_version: spreadsheet_formula::calculation::ENGINE_VERSION,
        compatibility_profile: spreadsheet_formula::calculation::COMPATIBILITY_PROFILE,
        result_hash: result_hash
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        preparation_us,
        calculation_us,
        total_us,
        preparation_work: projection.preparation_work(),
        graph_work: patch.graph_work_used,
        evaluation_work: patch.evaluation_work_used,
        formula_count,
        evaluated_cells: patch.evaluated_cells,
        result_bytes: projection.result_bytes(),
        formulas,
        formula_rows_truncated: formula_count > 1000,
        graph,
    })
}
pub async fn profile(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<AnalysisRequest>,
) -> Result<Response, ApiError> {
    let (principal, document, book) = read_book(&state, &headers, &id, &input, true).await?;
    let slot = state
        .history_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "analysis_busy"))?;
    let report = tokio::task::spawn_blocking(move || {
        let _slot = slot;
        analyze(&book)
    })
    .await
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "profile_failed"))??;
    deliver(&state, &principal, document, report).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_domain::workbook::*;
    #[test]
    fn executor_observer_measures_a_separate_projection_and_missing_context_fails_without_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let sheet = SheetId::from_bytes([1; 16])?;
        let row = RowId::from_bytes([2; 16])?;
        let column = ColumnId::from_bytes([3; 16])?;
        let address = CellAddress { sheet, row, column };
        let mut book = Workbook::empty(DocumentId::from_bytes([4; 16])?);
        book = book.candidate(
            &Operation::CreateWorkbook {
                sheet: SheetSeed {
                    id: sheet,
                    name: "Profile".into(),
                    rows: vec![row],
                    columns: vec![column],
                },
            },
            DocumentVersion::new(1)?,
        )?;
        book = book.candidate(
            &Operation::SetCellFormula {
                address,
                formula: spreadsheet_formula::binder::bind("=RAND()", &book, address)?,
            },
            DocumentVersion::new(2)?,
        )?;
        let hash = book.hash();
        match analyze(&book) {
            Err(ApiError(status, code)) => {
                assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
                assert_eq!(code, "profile_context_missing");
            }
            _ => return Err("expected missing-context rejection".into()),
        }
        assert_eq!(book.hash(), hash);
        assert_eq!(book.version().get(), 2);
        assert!(book.calculation_context().is_none());
        book = book.candidate(
            &Operation::SetCellFormula {
                address,
                formula: spreadsheet_formula::binder::bind("=2+3", &book, address)?,
            },
            DocumentVersion::new(3)?,
        )?;
        let hash = book.hash();
        let report = analyze(&book).map_err(|error| error.1)?;
        assert_eq!(report.formula_count, 1);
        assert_eq!(report.evaluated_cells, 1);
        assert_eq!(report.formulas.len(), 1);
        assert_eq!(report.formulas[0].work, report.evaluation_work);
        assert!(report.total_us >= report.preparation_us);
        assert!(report.total_us >= report.calculation_us);
        assert_eq!(book.hash(), hash);
        assert_eq!(book.version().get(), 3);
        assert!(book.calculation_context().is_none());
        Ok(())
    }
}
