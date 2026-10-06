use super::*;
use spreadsheet_domain::workbook::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn hint(
    document: DocumentId,
    update: &HeadUpdate,
) -> Result<v1::DocumentHeadChanged, Box<dyn std::error::Error>> {
    let decoded = v1::ServerEnvelope::decode(head_hint(document, update).as_slice())?;
    match decoded.payload {
        Some(v1::server_envelope::Payload::DocumentHeadChanged(hint)) => Ok(hint),
        _ => Err("unexpected head frame".into()),
    }
}

#[test]
fn history_scheduling_preserves_unknown_user_and_system_semantics() -> TestResult {
    let document = DocumentId::from_bytes([1; 16])?;
    let actor = ActorId::from_bytes([5; 16])?;
    let create = Operation::CreateWorkbook {
        sheet: SheetSeed {
            id: SheetId::from_bytes([2; 16])?,
            name: "Sheet1".into(),
            rows: vec![RowId::from_bytes([3; 16])?],
            columns: vec![ColumnId::from_bytes([4; 16])?],
        },
    };
    let book = Workbook::empty(document).candidate(&create, DocumentVersion::new(1)?)?;
    let operation = spreadsheet_domain::axis_edit::prepare_axis_edit(
        &book,
        &Operation::InsertRowsRebased {
            sheet: SheetId::from_bytes([2; 16])?,
            before: RowId::from_bytes([3; 16])?,
            rows: vec![RowId::from_bytes([9; 16])?],
        },
        DocumentVersion::new(2)?,
    )?;
    let book = book.candidate(&operation, DocumentVersion::new(2)?)?;
    // This durable user event has no wire-representable canonical patch.
    let event = Arc::new(CommittedEvent {
        document_id: document,
        operation_id: OperationId::from_bytes([6; 16])?,
        principal: EventPrincipal::User(actor),
        version: book.version(),
        operation,
        state_hash: book.hash(),
    });
    let update = HeadUpdate {
        version: book.version(),
        event: Some(event.clone()),
    };
    let user = hint(document, &update)?;
    assert_eq!(user.head_event_kind, v1::HeadEventKind::User as i32);
    assert_eq!(user.head_actor_id, actor.as_bytes());
    let latest = user
        .latest_operation
        .as_ref()
        .ok_or("missing user envelope")?;
    assert!(latest.canonical_operation.is_none());
    let fallback =
        v1::ServerEnvelope::decode(head_hint_with_operation(document, &update, false).as_slice())?;
    let Some(v1::server_envelope::Payload::DocumentHeadChanged(fallback)) = fallback.payload else {
        return Err("unexpected fallback frame".into());
    };
    assert_eq!(fallback.head_event_kind, v1::HeadEventKind::User as i32);
    assert_eq!(fallback.head_actor_id, actor.as_bytes());
    assert!(fallback.latest_operation.is_none());
    assert_eq!(latest.actor_id, actor.as_bytes());
    let context = CalculationContext::new(
        CalculationContextId::from_bytes([7; 16])?,
        1,
        1_700_000_000_000,
        180,
        19_676,
        [8; 32],
        "pxel-formula-0.16.0".into(),
        "pxel-excel-subset-v1".into(),
        CalculationReason::Explicit,
    )?;
    let operation = Operation::AdvanceCalculationEpoch { context };
    let next = book.candidate(&operation, DocumentVersion::new(3)?)?;
    let system = hint(
        document,
        &HeadUpdate {
            version: next.version(),
            event: Some(Arc::new(CommittedEvent {
                document_id: document,
                operation_id: OperationId::from_bytes([8; 16])?,
                principal: EventPrincipal::System,
                version: next.version(),
                operation,
                state_hash: next.hash(),
            })),
        },
    )?;
    assert_eq!(system.head_event_kind, v1::HeadEventKind::System as i32);
    assert!(system.head_actor_id.is_empty());
    assert!(system.latest_operation.is_none());
    for update in [
        HeadUpdate {
            version: next.version(),
            event: None,
        },
        HeadUpdate {
            version: next.version(),
            event: Some(event),
        },
    ] {
        let coalesced = hint(document, &update)?;
        assert_eq!(
            coalesced.head_event_kind,
            v1::HeadEventKind::Unspecified as i32
        );
        assert!(coalesced.head_actor_id.is_empty());
        assert!(coalesced.latest_operation.is_none());
    }
    Ok(())
}
