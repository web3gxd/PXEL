use spreadsheet_domain::workbook::*;

fn genesis() -> Result<Workbook, DomainError> {
    let document = DocumentId::from_bytes([1; 16])?;
    Workbook::empty(document).candidate(
        &Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: SheetId::from_bytes([2; 16])?,
                name: "Sheet1".into(),
                rows: vec![RowId::from_bytes([3; 16])?],
                columns: vec![ColumnId::from_bytes([4; 16])?],
            },
        },
        DocumentVersion::new(1)?,
    )
}

fn context(id: u8, epoch: u64) -> Result<CalculationContext, DomainError> {
    CalculationContext::new(
        CalculationContextId::from_bytes([id; 16])?,
        epoch,
        1_700_000_000_000,
        180,
        19_676,
        [id; 32],
        "pxel-formula-0.1.0".into(),
        "pxel-excel-subset-v1".into(),
        CalculationReason::Explicit,
    )
}

#[test]
fn epoch_event_is_atomic_versioned_and_replayable() -> Result<(), DomainError> {
    let mut book = genesis()?;
    let legacy_hash = book.hash();
    let operation = Operation::AdvanceCalculationEpoch {
        context: context(5, 1)?,
    };
    let candidate = book.candidate(&operation, DocumentVersion::new(2)?)?;
    assert_ne!(candidate.hash(), legacy_hash);
    assert!(book.calculation_context().is_none());
    let event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([6; 16])?,
        principal: EventPrincipal::System,
        version: candidate.version(),
        operation,
        state_hash: candidate.hash(),
    };
    book.apply_event(&event)?;
    assert_eq!(book.hash(), candidate.hash());
    assert_eq!(
        book.calculation_context().map(CalculationContext::epoch),
        Some(1)
    );
    assert!(matches!(
        book.candidate(
            &Operation::AdvanceCalculationEpoch {
                context: context(7, 1)?
            },
            DocumentVersion::new(3)?,
        ),
        Err(DomainError::InvalidCalculationContext)
    ));
    let next = book.candidate(
        &Operation::AdvanceCalculationEpoch {
            context: context(7, 2)?,
        },
        DocumentVersion::new(3)?,
    )?;
    assert_eq!(
        next.calculation_context().map(CalculationContext::epoch),
        Some(2)
    );
    Ok(())
}

#[test]
fn context_rejects_bad_civil_day_offset_and_principal() -> Result<(), DomainError> {
    let id = CalculationContextId::from_bytes([5; 16])?;
    assert_eq!(
        CalculationContext::new(
            id,
            1,
            1_700_000_000_000,
            180,
            0,
            [5; 32],
            "pxel-formula-0.1.0".into(),
            "pxel-excel-subset-v1".into(),
            CalculationReason::Explicit,
        ),
        Err(DomainError::InvalidCalculationContext),
    );
    assert_eq!(
        CalculationContext::new(
            id,
            1,
            1_700_000_000_000,
            841,
            19_675,
            [5; 32],
            "pxel-formula-0.1.0".into(),
            "pxel-excel-subset-v1".into(),
            CalculationReason::Explicit,
        ),
        Err(DomainError::InvalidCalculationContext),
    );
    let mut book = genesis()?;
    let operation = Operation::AdvanceCalculationEpoch {
        context: context(5, 1)?,
    };
    let candidate = book.candidate(&operation, DocumentVersion::new(2)?)?;
    let event = CommittedEvent {
        document_id: book.document_id(),
        operation_id: OperationId::from_bytes([6; 16])?,
        principal: EventPrincipal::User(ActorId::from_bytes([7; 16])?),
        version: candidate.version(),
        operation,
        state_hash: candidate.hash(),
    };
    assert_eq!(
        book.apply_event(&event),
        Err(DomainError::InvalidCalculationContext)
    );
    assert_eq!(book.version().get(), 1);
    Ok(())
}
