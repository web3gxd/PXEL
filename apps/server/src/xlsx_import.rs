//! Parent-side conversion from validated XLSX IR into a durable import journal.
//! The local CLI is feature-gated until the untrusted worker has an OS sandbox.

use spreadsheet_domain::{
    formula::{FormulaSource, Node},
    workbook::{
        ActorId, CellAddress, CellValue, CellWrite, ColumnId, CommittedEvent, DocumentId,
        DocumentVersion, DomainError, EventPrincipal, MAX_COLUMNS, MAX_ROWS, Operation,
        OperationId, RowId, SheetId, SheetSeed, Workbook,
    },
};
use spreadsheet_formula::binder;
use spreadsheet_xlsx::{NormalizedCellValue, NormalizedWorkbook};
use thiserror::Error;
use uuid::Uuid;

const MAX_IMPORT_CELLS: usize = 1_000;

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("invalid XLSX intermediate representation")]
    InvalidIr,
    #[error("XLSX workbook exceeds the supported import profile")]
    UnsupportedProfile,
    #[error("XLSX formula cannot be imported in the supported profile")]
    InvalidFormula,
    #[error("imported workbook violates domain limits")]
    Domain(#[from] DomainError),
}

fn event(
    document: DocumentId,
    actor: ActorId,
    previous: &Workbook,
    operation: Operation,
) -> Result<(CommittedEvent, Workbook), ImportError> {
    let version = DocumentVersion::new(
        previous
            .version()
            .get()
            .checked_add(1)
            .ok_or(DomainError::LimitExceeded)?,
    )?;
    let candidate = previous.candidate(&operation, version)?;
    Ok((
        CommittedEvent {
            document_id: document,
            operation_id: OperationId::from_bytes(*Uuid::now_v7().as_bytes())?,
            principal: EventPrincipal::User(actor),
            version,
            operation,
            state_hash: candidate.hash(),
        },
        candidate,
    ))
}

pub fn prepare(
    imported: &NormalizedWorkbook,
    document: DocumentId,
    actor: ActorId,
) -> Result<Vec<CommittedEvent>, ImportError> {
    imported.validate().map_err(|_| ImportError::InvalidIr)?;
    let sheet = imported.sheets.first().ok_or(ImportError::InvalidIr)?;
    if imported.sheets.len() != 1 || sheet.cells.len() > MAX_IMPORT_CELLS {
        return Err(ImportError::UnsupportedProfile);
    }
    let row_extent = sheet
        .cells
        .last()
        .map_or(1000_usize, |cell| cell.row as usize)
        .max(1000);
    let column_extent = sheet
        .cells
        .iter()
        .map(|cell| cell.column as usize)
        .max()
        .unwrap_or(26)
        .max(26);
    if row_extent > MAX_ROWS || column_extent > MAX_COLUMNS {
        return Err(ImportError::UnsupportedProfile);
    }
    let seed = SheetSeed {
        id: SheetId::from_bytes(*Uuid::now_v7().as_bytes())?,
        name: sheet.name.clone(),
        rows: (0..row_extent)
            .map(|_| RowId::from_bytes(*Uuid::now_v7().as_bytes()))
            .collect::<Result<_, _>>()?,
        columns: (0..column_extent)
            .map(|_| ColumnId::from_bytes(*Uuid::now_v7().as_bytes()))
            .collect::<Result<_, _>>()?,
    };
    let mut events = Vec::with_capacity(sheet.cells.len().saturating_add(2));
    let (created, mut current) = event(
        document,
        actor,
        &Workbook::empty(document),
        Operation::CreateWorkbook {
            sheet: seed.clone(),
        },
    )?;
    events.push(created);
    let mut values = Vec::new();
    let mut formulas = Vec::new();
    for cell in &sheet.cells {
        let address = CellAddress {
            sheet: seed.id,
            row: *seed
                .rows
                .get((cell.row - 1) as usize)
                .ok_or(ImportError::UnsupportedProfile)?,
            column: *seed
                .columns
                .get((cell.column - 1) as usize)
                .ok_or(ImportError::UnsupportedProfile)?,
        };
        match &cell.value {
            NormalizedCellValue::Number(value) => values.push(CellWrite {
                address,
                value: CellValue::Number(*value),
            }),
            NormalizedCellValue::Text(value) => values.push(CellWrite {
                address,
                value: CellValue::Text(value.clone()),
            }),
            NormalizedCellValue::Boolean(value) => values.push(CellWrite {
                address,
                value: CellValue::Boolean(*value),
            }),
            NormalizedCellValue::Formula(raw) => {
                let source = FormulaSource::new(format!("={raw}"))
                    .map_err(|_| ImportError::InvalidFormula)?;
                let formula = binder::bind(source.as_str(), &current, address)
                    .map_err(|_| ImportError::InvalidFormula)?;
                if formula
                    .ast()
                    .nodes()
                    .iter()
                    .any(|node| matches!(node, Node::InvalidReference(_)))
                {
                    return Err(ImportError::InvalidFormula);
                }
                formulas.push((address, formula));
            }
        }
    }
    if !values.is_empty() {
        let (saved, next) = event(
            document,
            actor,
            &current,
            Operation::SetCellValues { writes: values },
        )?;
        events.push(saved);
        current = next;
    }
    for (address, formula) in formulas {
        let (saved, next) = event(
            document,
            actor,
            &current,
            Operation::SetCellFormula { address, formula },
        )?;
        events.push(saved);
        current = next;
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_xlsx::{NormalizedCell, NormalizedSheet};

    #[test]
    fn normalized_values_and_formula_become_replayable_events()
    -> Result<(), Box<dyn std::error::Error>> {
        let input = NormalizedWorkbook {
            sheets: vec![NormalizedSheet {
                name: "Data".into(),
                cells: vec![
                    NormalizedCell {
                        row: 1,
                        column: 1,
                        value: NormalizedCellValue::Number(7.0),
                    },
                    NormalizedCell {
                        row: 1,
                        column: 2,
                        value: NormalizedCellValue::Formula("A1*2".into()),
                    },
                ],
            }],
        };
        let document = DocumentId::from_bytes([1; 16])?;
        let actor = ActorId::from_bytes([2; 16])?;
        let events = prepare(&input, document, actor)?;
        assert_eq!(events.len(), 3);
        let mut replay = Workbook::empty(document);
        for event in &events {
            replay.apply_event(event)?;
        }
        let sheet = replay.sheets().first().ok_or("missing sheet")?;
        let a1 = CellAddress {
            sheet: sheet.id(),
            row: sheet.rows()[0],
            column: sheet.columns()[0],
        };
        let b1 = CellAddress {
            sheet: sheet.id(),
            row: sheet.rows()[0],
            column: sheet.columns()[1],
        };
        assert_eq!(
            replay.cell_content(a1)?,
            Some(&spreadsheet_domain::workbook::CellContent::Value(
                CellValue::Number(7.0)
            ))
        );
        assert!(matches!(
            replay.cell_content(b1)?,
            Some(spreadsheet_domain::workbook::CellContent::Formula(_))
        ));
        Ok(())
    }

    #[test]
    fn unsupported_workbook_fails_before_any_event() -> Result<(), Box<dyn std::error::Error>> {
        let document = DocumentId::from_bytes([1; 16])?;
        let actor = ActorId::from_bytes([2; 16])?;
        let input = NormalizedWorkbook {
            sheets: vec![NormalizedSheet {
                name: "Data".into(),
                cells: vec![NormalizedCell {
                    row: 1,
                    column: 1,
                    value: NormalizedCellValue::Formula("'Missing'!A1".into()),
                }],
            }],
        };
        assert!(matches!(
            prepare(&input, document, actor),
            Err(ImportError::InvalidFormula)
        ));
        Ok(())
    }
}
