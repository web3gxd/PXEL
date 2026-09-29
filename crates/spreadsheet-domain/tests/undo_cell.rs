use spreadsheet_domain::{
    formula::{AST_SCHEMA_VERSION, BoundAst, Formula, Node, NodeId},
    workbook::{
        ActorId, CellAddress, CellContent, CellValue, CellWriteKind, ColumnId, CommittedEvent,
        DocumentId, DocumentVersion, DomainError, EventPrincipal, Operation, OperationId, RowId,
        SheetId, SheetSeed, Workbook,
    },
};

fn seed() -> Result<(Workbook, CellAddress), DomainError> {
    let document_id = DocumentId::from_bytes([1; 16])?;
    let address = CellAddress {
        sheet: SheetId::from_bytes([2; 16])?,
        row: RowId::from_bytes([3; 16])?,
        column: ColumnId::from_bytes([4; 16])?,
    };
    let workbook = Workbook::empty(document_id).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: address.sheet,
                name: "Sheet".into(),
                rows: vec![address.row],
                columns: vec![address.column],
            },
        },
        DocumentVersion::new(1)?,
    )?;
    Ok((workbook, address))
}

fn authored_write(
    address: CellAddress,
    content: Option<CellContent>,
    previous: Option<CellContent>,
    kind: CellWriteKind,
) -> Operation {
    Operation::CellWrite {
        address,
        content,
        previous,
        kind: Some(kind),
        undoes: None,
        undoes_version: None,
    }
}

#[test]
fn guarded_write_and_compensation_replay_to_same_hash() -> Result<(), DomainError> {
    let (mut original, address) = seed()?;
    let authored_id = OperationId::from_bytes([5; 16])?;
    let value = CellContent::Value(CellValue::Text("first".into()));
    let authored = authored_write(address, Some(value.clone()), None, CellWriteKind::SetValue);
    let after_write = original.candidate(&authored, DocumentVersion::new(2)?)?;
    original.apply_event(&CommittedEvent {
        document_id: original.document_id(),
        operation_id: authored_id,
        principal: EventPrincipal::User(ActorId::from_bytes([6; 16])?),
        version: DocumentVersion::new(2)?,
        operation: authored,
        state_hash: after_write.hash(),
    })?;
    assert_eq!(original.hash(), after_write.hash());
    let compensation = Operation::CellWrite {
        address,
        content: None,
        previous: Some(value),
        kind: None,
        undoes: Some(authored_id),
        undoes_version: Some(DocumentVersion::new(2)?),
    };
    let after_undo = original.candidate(&compensation, DocumentVersion::new(3)?)?;
    original.apply_event(&CommittedEvent {
        document_id: original.document_id(),
        operation_id: OperationId::from_bytes([7; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([6; 16])?),
        version: DocumentVersion::new(3)?,
        operation: compensation,
        state_hash: after_undo.hash(),
    })?;
    assert_eq!(original.hash(), after_undo.hash());
    assert_eq!(original.cell_content(address)?, None);
    Ok(())
}

#[test]
fn domain_preimage_guard_needs_journal_history_check_for_equal_value_write()
-> Result<(), DomainError> {
    let (book, address) = seed()?;
    let value = CellContent::Value(CellValue::Number(9.0));
    let book = book.candidate(
        &authored_write(address, Some(value.clone()), None, CellWriteKind::SetValue),
        DocumentVersion::new(2)?,
    )?;
    let foreign = authored_write(
        address,
        Some(value.clone()),
        Some(value.clone()),
        CellWriteKind::SetValue,
    );
    let book = book.candidate(&foreign, DocumentVersion::new(3)?)?;
    // The domain guard checks content only. The journal's event-history guard
    // must additionally detect this equal-value foreign write before undo.
    assert_eq!(book.cell_content(address)?, Some(&value));
    let apparent_undo = Operation::CellWrite {
        address,
        content: None,
        previous: Some(value),
        kind: None,
        undoes: Some(OperationId::from_bytes([5; 16])?),
        undoes_version: Some(DocumentVersion::new(2)?),
    };
    assert!(
        book.candidate(&apparent_undo, DocumentVersion::new(4)?)
            .is_ok()
    );
    let stale = authored_write(address, None, None, CellWriteKind::Clear);
    assert_eq!(
        book.candidate(&stale, DocumentVersion::new(4)?).err(),
        Some(DomainError::PreimageConflict)
    );
    Ok(())
}

#[test]
fn clearing_formula_records_preimage_but_undo_cannot_restore_it_yet() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    let formula = Formula::new(
        "=42".into(),
        "=42".into(),
        BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Number(42.0)])
            .map_err(|_| DomainError::InvalidFormula)?,
        address,
        book.version(),
    )
    .map_err(|_| DomainError::InvalidFormula)?;
    let formula_content = CellContent::Formula(formula.clone());
    let book = book.candidate(
        &Operation::SetCellFormula { address, formula },
        DocumentVersion::new(2)?,
    )?;
    let clear = authored_write(
        address,
        None,
        Some(formula_content.clone()),
        CellWriteKind::Clear,
    );
    let book = book.candidate(&clear, DocumentVersion::new(3)?)?;
    assert!(book.cell_content(address)?.is_none());
    let undo = Operation::CellWrite {
        address,
        content: Some(formula_content),
        previous: None,
        kind: None,
        undoes: Some(OperationId::from_bytes([5; 16])?),
        undoes_version: Some(DocumentVersion::new(3)?),
    };
    assert_eq!(
        book.candidate(&undo, DocumentVersion::new(4)?).err(),
        Some(DomainError::UnsupportedUndoFormula)
    );
    Ok(())
}

#[test]
fn authored_kind_and_compensation_metadata_are_coherent() -> Result<(), DomainError> {
    let (book, address) = seed()?;
    let bad_clear = authored_write(
        address,
        Some(CellContent::Value(CellValue::Number(1.0))),
        None,
        CellWriteKind::Clear,
    );
    assert_eq!(
        book.candidate(&bad_clear, DocumentVersion::new(2)?).err(),
        Some(DomainError::InvalidValue)
    );
    let missing_target_version = Operation::CellWrite {
        address,
        content: None,
        previous: None,
        kind: None,
        undoes: Some(OperationId::from_bytes([5; 16])?),
        undoes_version: None,
    };
    assert_eq!(
        book.candidate(&missing_target_version, DocumentVersion::new(2)?)
            .err(),
        Some(DomainError::VersionConflict)
    );
    Ok(())
}
