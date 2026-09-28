use spreadsheet_domain::workbook::{CellAddress, ColumnId, DomainError, RowId, SheetId};
use spreadsheet_formula::dependency::DependencyGraph;
use std::collections::BTreeSet;

fn cell(row: u128) -> Result<CellAddress, DomainError> {
    Ok(CellAddress {
        sheet: SheetId::from_bytes(1_u128.to_be_bytes())?,
        row: RowId::from_bytes(row.to_be_bytes())?,
        column: ColumnId::from_bytes(1_u128.to_be_bytes())?,
    })
}

#[test]
fn replacement_and_removal_keep_only_current_dependencies() -> Result<(), Box<dyn std::error::Error>>
{
    let (a, b, c, d) = (cell(1)?, cell(2)?, cell(3)?, cell(4)?);
    let mut graph = DependencyGraph::default();
    graph.replace(b, BTreeSet::from([a]));
    graph.replace(c, BTreeSet::from([b]));
    graph.replace(d, BTreeSet::new());
    assert_eq!(graph.dirty([a], &mut 100)?, BTreeSet::from([a, b, c]));
    graph.replace(b, BTreeSet::from([d]));
    assert_eq!(graph.dirty([a], &mut 100)?, BTreeSet::from([a]));
    graph.remove(b);
    assert_eq!(graph.dirty([d], &mut 100)?, BTreeSet::from([d]));
    assert_eq!(graph.dirty([b], &mut 100)?, BTreeSet::from([b, c]));
    Ok(())
}

#[test]
fn ten_thousand_chain_is_iterative_and_only_dirty_formulas_are_scheduled()
-> Result<(), Box<dyn std::error::Error>> {
    let mut graph = DependencyGraph::default();
    for row in 2..=10_001 {
        graph.replace(cell(row)?, BTreeSet::from([cell(row - 1)?]));
    }
    graph.replace(cell(20_000)?, BTreeSet::new());
    let mut budget = 100_000;
    let dirty = graph.dirty([cell(1)?], &mut budget)?;
    let order = graph.schedule(&dirty, &mut budget)?;
    assert_eq!(order.ready.len(), 10_000);
    assert_eq!(order.ready.first(), Some(&cell(2)?));
    assert_eq!(order.ready.last(), Some(&cell(10_001)?));
    assert!(order.blocked.is_empty());
    assert!(!order.ready.contains(&cell(20_000)?));
    Ok(())
}

#[test]
fn cycle_blocks_downstream_but_not_independent_work() -> Result<(), Box<dyn std::error::Error>> {
    let (a, b, c, d) = (cell(1)?, cell(2)?, cell(3)?, cell(4)?);
    let mut graph = DependencyGraph::default();
    graph.replace(a, BTreeSet::from([b]));
    graph.replace(b, BTreeSet::from([a]));
    graph.replace(c, BTreeSet::from([b]));
    graph.replace(d, BTreeSet::new());
    let dirty = graph.dirty([a, d], &mut 100)?;
    let order = graph.schedule(&dirty, &mut 100)?;
    assert_eq!(order.ready, vec![d]);
    assert_eq!(order.blocked, BTreeSet::from([a, b, c]));
    assert!(graph.dirty([a], &mut 1).is_err());
    assert!(graph.schedule(&dirty, &mut 1).is_err());
    Ok(())
}
