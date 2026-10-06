//! Bounded potential dependencies for visual analysis, including scalar inputs.
use crate::{bound::bound_dependencies, calculation::CalculationError};
use spreadsheet_domain::workbook::{CellAddress, CellContent, Workbook};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
pub const MAX_PROFILE_NODES: usize = 500;
pub const MAX_PROFILE_EDGES: usize = 2000;
pub struct ProfileNode {
    pub address: CellAddress,
    pub formula: bool,
}
pub struct ProfileGraph {
    pub nodes: Vec<ProfileNode>,
    pub edges: Vec<(usize, usize)>,
    pub truncated: bool,
}

pub fn graph(book: &Workbook, roots: &[CellAddress]) -> Result<ProfileGraph, CalculationError> {
    let mut output = ProfileGraph {
        nodes: Vec::new(),
        edges: Vec::new(),
        truncated: false,
    };
    let mut indices = BTreeMap::new();
    let mut queue = VecDeque::new();
    let mut visited = BTreeSet::new();
    let mut edges = BTreeSet::new();
    let mut work = 1_000_000usize;
    // Traverse each root's chain before admitting unrelated roots.
    for root in roots {
        if !insert(book, *root, &mut indices, &mut output, &mut queue)? {
            output.truncated = true;
            break;
        }
        while let Some(address) = queue.pop_front() {
            if !visited.insert(address) {
                continue;
            }
            let sheet = book
                .sheets()
                .iter()
                .find(|sheet| sheet.id() == address.sheet)
                .ok_or(CalculationError::InvalidProjection)?;
            let Some(CellContent::Formula(formula)) = sheet.cells().get(&address) else {
                continue;
            };
            work = work
                .checked_sub(formula.ast().nodes().len())
                .ok_or(CalculationError::DependencyLimit)?;
            let dependencies = bound_dependencies(formula, book)?;
            let source = *indices
                .get(&address)
                .ok_or(CalculationError::InvalidProjection)?;
            for target in dependencies.direct {
                if !edge(
                    book,
                    source,
                    target,
                    &mut indices,
                    &mut output,
                    &mut queue,
                    &mut edges,
                    &mut work,
                )? {
                    break;
                }
            }
            for range in dependencies.ranges {
                let target_sheet = book
                    .sheets()
                    .iter()
                    .find(|sheet| sheet.id() == range.sheet)
                    .ok_or(CalculationError::InvalidProjection)?;
                'range: for row in range.first_row..=range.last_row {
                    for column in range.first_column..=range.last_column {
                        let target = CellAddress {
                            sheet: range.sheet,
                            row: *target_sheet
                                .rows()
                                .get(row as usize)
                                .ok_or(CalculationError::InvalidProjection)?,
                            column: *target_sheet
                                .columns()
                                .get(column as usize)
                                .ok_or(CalculationError::InvalidProjection)?,
                        };
                        if !edge(
                            book,
                            source,
                            target,
                            &mut indices,
                            &mut output,
                            &mut queue,
                            &mut edges,
                            &mut work,
                        )? {
                            break 'range;
                        }
                    }
                }
            }
        }
    }
    output.edges = edges.into_iter().collect();
    Ok(output)
}
fn insert(
    book: &Workbook,
    address: CellAddress,
    indices: &mut BTreeMap<CellAddress, usize>,
    output: &mut ProfileGraph,
    queue: &mut VecDeque<CellAddress>,
) -> Result<bool, CalculationError> {
    if indices.contains_key(&address) {
        return Ok(true);
    }
    if indices.len() == MAX_PROFILE_NODES {
        return Ok(false);
    }
    let sheet = book
        .sheets()
        .iter()
        .find(|sheet| sheet.id() == address.sheet)
        .ok_or(CalculationError::InvalidProjection)?;
    if sheet.row_position(address.row).is_none() || sheet.column_position(address.column).is_none()
    {
        return Err(CalculationError::InvalidProjection);
    }
    indices.insert(address, output.nodes.len());
    output.nodes.push(ProfileNode {
        address,
        formula: matches!(sheet.cells().get(&address), Some(CellContent::Formula(_))),
    });
    queue.push_back(address);
    Ok(true)
}
#[allow(clippy::too_many_arguments)]
fn edge(
    book: &Workbook,
    source: usize,
    target: CellAddress,
    indices: &mut BTreeMap<CellAddress, usize>,
    output: &mut ProfileGraph,
    queue: &mut VecDeque<CellAddress>,
    edges: &mut BTreeSet<(usize, usize)>,
    work: &mut usize,
) -> Result<bool, CalculationError> {
    *work = work
        .checked_sub(1)
        .ok_or(CalculationError::DependencyLimit)?;
    if edges.len() == MAX_PROFILE_EDGES || !insert(book, target, indices, output, queue)? {
        output.truncated = true;
        return Ok(false);
    }
    let target = *indices
        .get(&target)
        .ok_or(CalculationError::InvalidProjection)?;
    edges.insert((source, target));
    Ok(true)
}
