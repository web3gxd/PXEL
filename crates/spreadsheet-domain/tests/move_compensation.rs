use spreadsheet_domain::workbook::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn frozen_move_compensation_is_atomic_guarded_and_inverse_eligible() -> TestResult {
    let sheet = SheetId::from_bytes([2; 16])?;
    let row = RowId::from_bytes([3; 16])?;
    let columns = [
        ColumnId::from_bytes([4; 16])?,
        ColumnId::from_bytes([5; 16])?,
    ];
    let at = columns.map(|column| CellAddress { sheet, row, column });
    let book = Workbook::hydrate(
        DocumentId::from_bytes([1; 16])?,
        DocumentVersion::new(5)?,
        SheetSeed {
            id: sheet,
            name: "Sheet".into(),
            rows: vec![row],
            columns: columns.to_vec(),
        },
        [(at[1], CellValue::Number(7.).into())],
    )?;
    let op = Operation::MoveCompensation {
        source: at[0],
        destination: at[1],
        rows: 1,
        columns: 1,
        writes: vec![
            GuardedContentWrite {
                address: at[0],
                content: Some(CellValue::Number(7.).into()),
                previous: None,
            },
            GuardedContentWrite {
                address: at[1],
                content: None,
                previous: Some(CellValue::Number(7.).into()),
            },
        ],
        undoes: OperationId::from_bytes([7; 16])?,
        undoes_version: book.version(),
    };
    assert!(op.is_inverse_target());
    let restored = book.candidate(&op, DocumentVersion::new(6)?)?;
    assert_eq!(
        restored.cell_content(at[0])?,
        Some(&CellValue::Number(7.).into())
    );
    assert!(restored.cell_content(at[1])?.is_none());
    let hash = book.hash();
    for variant in 0..7 {
        let mut bad = op.clone();
        if let Operation::MoveCompensation {
            source,
            destination,
            rows,
            writes,
            undoes_version,
            ..
        } = &mut bad
        {
            match variant {
                0 => {
                    writes.pop();
                }
                1 => writes.push(writes[0].clone()),
                2 => writes[1].previous = None,
                3 => *undoes_version = DocumentVersion::ZERO,
                4 => *undoes_version = DocumentVersion::new(6)?,
                5 => *destination = *source,
                _ => *rows = 0,
            }
        }
        assert!(book.candidate(&bad, DocumentVersion::new(6)?).is_err());
        assert_eq!(book.hash(), hash);
    }
    assert!(book.preview_candidate(&[op], book.version()).is_err());
    Ok(())
}
