use spreadsheet_domain::workbook::*;
use spreadsheet_formula::{
    binder::bind,
    calculation::{CalculationLimits, CalculationObserver, CalculationProjection},
};
use std::{collections::BTreeMap, sync::atomic::AtomicBool};
#[derive(Default)]
struct Observer {
    starts: Vec<CellAddress>,
    work: BTreeMap<CellAddress, (u64, bool)>,
}
impl CalculationObserver for Observer {
    fn formula_started(&mut self, address: CellAddress) {
        self.starts.push(address);
    }
    fn formula_finished(&mut self, address: CellAddress, work: u64, blocked: bool) {
        self.work.insert(address, (work, blocked));
    }
}
#[test]
fn observed_calculation_preserves_results_head_and_context_including_cycles()
-> Result<(), Box<dyn std::error::Error>> {
    let sheet = SheetId::from_bytes([1; 16])?;
    let row = RowId::from_bytes([2; 16])?;
    let columns: Vec<_> = (3..8)
        .map(|id| ColumnId::from_bytes([id; 16]))
        .collect::<Result<_, _>>()?;
    let mut book = Workbook::empty(DocumentId::from_bytes([9; 16])?);
    book = book.candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Profile".into(),
                rows: vec![row],
                columns: columns.clone(),
            },
        },
        DocumentVersion::new(1)?,
    )?;
    for (column, source) in [
        (0, "=2"),
        (1, "=A1*3"),
        (2, "=IF(TRUE,B1,99)"),
        (3, "=E1"),
        (4, "=D1"),
    ] {
        let address = CellAddress {
            sheet,
            row,
            column: columns[column],
        };
        let formula = bind(source, &book, address)?;
        book = book.candidate(
            &Operation::SetCellFormula { address, formula },
            DocumentVersion::new(book.version().get() + 1)?,
        )?;
    }
    let hash = book.hash();
    let version = book.version();
    let context = book.calculation_context().cloned();
    let limits = CalculationLimits::default();
    let mut ordinary = CalculationProjection::from_workbook(&book, limits)?;
    let mut observed = CalculationProjection::from_workbook(&book, limits)?;
    ordinary.calculate(&book, limits, &AtomicBool::new(false))?;
    let mut observer = Observer::default();
    let patch =
        observed.calculate_observed(&book, limits, &AtomicBool::new(false), &mut observer)?;
    assert_eq!(ordinary.result_hash()?, observed.result_hash()?);
    assert_eq!(ordinary.results(), observed.results());
    assert_eq!(observer.starts.len(), 5);
    assert_eq!(observer.work.len(), 5);
    assert_eq!(
        observer
            .work
            .values()
            .filter(|(_, blocked)| *blocked)
            .count(),
        2
    );
    assert_eq!(
        observer.work.values().map(|(work, _)| work).sum::<u64>(),
        patch.evaluation_work_used
    );
    assert_eq!(book.hash(), hash);
    assert_eq!(book.version(), version);
    assert_eq!(book.calculation_context(), context.as_ref());
    Ok(())
}

#[test]
fn potential_graph_includes_both_branches_and_explicitly_truncates_ranges()
-> Result<(), Box<dyn std::error::Error>> {
    let sheet = SheetId::from_bytes([1; 16])?;
    let columns: Vec<_> = (3..6)
        .map(|id| ColumnId::from_bytes([id; 16]))
        .collect::<Result<_, _>>()?;
    let rows: Vec<_> = (0u32..1000)
        .map(|index| {
            let mut id = [2; 16];
            id[..4].copy_from_slice(&index.to_le_bytes());
            RowId::from_bytes(id)
        })
        .collect::<Result<_, _>>()?;
    let mut book = Workbook::empty(DocumentId::from_bytes([9; 16])?);
    book = book.candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Graph".into(),
                rows: rows.clone(),
                columns: columns.clone(),
            },
        },
        DocumentVersion::new(1)?,
    )?;
    let address = CellAddress {
        sheet,
        row: rows[0],
        column: columns[1],
    };
    book = book.candidate(
        &Operation::SetCellFormula {
            address,
            formula: bind("=IF(TRUE,A1,C1)", &book, address)?,
        },
        DocumentVersion::new(2)?,
    )?;
    let graph = spreadsheet_formula::profile_graph::graph(&book, &[address])?;
    assert_eq!(graph.nodes.len(), 3);
    assert_eq!(graph.edges.len(), 2);
    assert!(!graph.truncated);
    book = book.candidate(
        &Operation::SetCellFormula {
            address,
            formula: bind("=SUM(A1:A1000)", &book, address)?,
        },
        DocumentVersion::new(3)?,
    )?;
    let graph = spreadsheet_formula::profile_graph::graph(&book, &[address])?;
    assert_eq!(graph.nodes.len(), 500);
    assert_eq!(graph.edges.len(), 499);
    assert!(graph.truncated);
    assert_eq!(book.version().get(), 3);
    Ok(())
}

#[test]
fn dense_potential_graph_caps_edges_at_two_thousand() -> Result<(), Box<dyn std::error::Error>> {
    let sheet = SheetId::from_bytes([1; 16])?;
    let column = ColumnId::from_bytes([3; 16])?;
    let rows: Vec<_> = (0u32..100)
        .map(|index| {
            let mut id = [2; 16];
            id[..4].copy_from_slice(&index.to_le_bytes());
            RowId::from_bytes(id)
        })
        .collect::<Result<_, _>>()?;
    let mut book = Workbook::empty(DocumentId::from_bytes([9; 16])?);
    book = book.candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Dense".into(),
                rows: rows.clone(),
                columns: vec![column],
            },
        },
        DocumentVersion::new(1)?,
    )?;
    let mut roots = Vec::new();
    for row in rows.iter().take(25) {
        let address = CellAddress {
            sheet,
            row: *row,
            column,
        };
        book = book.candidate(
            &Operation::SetCellFormula {
                address,
                formula: bind("=SUM($A$1:$A$100)", &book, address)?,
            },
            DocumentVersion::new(book.version().get() + 1)?,
        )?;
        roots.push(address);
    }
    let graph = spreadsheet_formula::profile_graph::graph(&book, &roots)?;
    assert_eq!(graph.edges.len(), 2000);
    assert_eq!(graph.nodes.len(), 100);
    assert!(graph.truncated);
    Ok(())
}
