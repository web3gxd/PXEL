use criterion::{Criterion, criterion_group, criterion_main};
use spreadsheet_domain::workbook::{
    ActorId, CellAddress, CellValue, ColumnId, CommittedEvent, DocumentId, DocumentVersion,
    DomainError, EventPrincipal, Operation, OperationId, RowId, SheetId, SheetSeed, SnapshotImage,
    Workbook,
};
use std::hint::black_box;

const PREFIX_EVENTS: u64 = 10_000;
const TAIL_EVENTS: u64 = 10_000;

struct Fixture {
    document_id: DocumentId,
    genesis: CommittedEvent,
    events: Vec<CommittedEvent>,
    image: SnapshotImage,
    snapshot_hash: [u8; 32],
    final_hash: [u8; 32],
}

fn fixture() -> Result<Fixture, DomainError> {
    let document_id = DocumentId::from_bytes([1; 16])?;
    let sheet = SheetId::from_bytes([2; 16])?;
    let row = RowId::from_bytes([3; 16])?;
    let column = ColumnId::from_bytes([4; 16])?;
    let actor = ActorId::from_bytes([5; 16])?;
    let genesis_operation = Operation::CreateWorkbook {
        sheet: SheetSeed {
            id: sheet,
            name: "Benchmark".into(),
            rows: vec![row],
            columns: vec![column],
        },
    };
    let mut book =
        Workbook::empty(document_id).candidate(&genesis_operation, DocumentVersion::new(1)?)?;
    let genesis = CommittedEvent {
        document_id,
        operation_id: OperationId::from_bytes([6; 16])?,
        principal: EventPrincipal::User(actor),
        version: book.version(),
        operation: genesis_operation,
        state_hash: book.hash(),
    };
    let mut events = Vec::with_capacity((PREFIX_EVENTS + TAIL_EVENTS) as usize);
    let mut checkpoint = (book.snapshot_image()?, book.hash());
    for number in 1..=PREFIX_EVENTS + TAIL_EVENTS {
        let operation = Operation::SetCellValue {
            address: CellAddress { sheet, row, column },
            value: CellValue::Number(number as f64),
        };
        let version = DocumentVersion::new(number + 1)?;
        book = book.candidate(&operation, version)?;
        events.push(CommittedEvent {
            document_id,
            operation_id: OperationId::from_bytes((number as u128 + 6).to_be_bytes())?,
            principal: EventPrincipal::User(actor),
            version,
            operation,
            state_hash: book.hash(),
        });
        if number == PREFIX_EVENTS {
            checkpoint = (book.snapshot_image()?, book.hash());
        }
    }
    let (image, snapshot_hash) = checkpoint;
    let fixture = Fixture {
        document_id,
        genesis,
        events,
        image,
        snapshot_hash,
        final_hash: book.hash(),
    };
    let restored = replay_snapshot(&fixture)?;
    let full = replay_full(&fixture)?;
    if restored.hash() != fixture.final_hash || full.hash() != fixture.final_hash {
        return Err(DomainError::HashMismatch);
    }
    Ok(fixture)
}

fn replay_snapshot(fixture: &Fixture) -> Result<Workbook, DomainError> {
    let mut book = Workbook::restore_snapshot(fixture.image.clone(), fixture.snapshot_hash)?;
    for event in &fixture.events[PREFIX_EVENTS as usize..] {
        book.apply_event(event)?;
    }
    Ok(book)
}

fn replay_full(fixture: &Fixture) -> Result<Workbook, DomainError> {
    let mut book = Workbook::empty(fixture.document_id);
    book.apply_event(&fixture.genesis)?;
    for event in &fixture.events {
        book.apply_event(event)?;
    }
    Ok(book)
}

fn benches(c: &mut Criterion) {
    let fixture = match fixture() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("recovery benchmark fixture invalid: {error}");
            std::process::exit(1);
        }
    };
    c.bench_function("snapshot_plus_10000_tail_events_domain", |bench| {
        bench.iter(|| black_box(replay_snapshot(black_box(&fixture))))
    });
    c.bench_function("full_replay_20001_events_domain", |bench| {
        bench.iter(|| black_box(replay_full(black_box(&fixture))))
    });
}

criterion_group!(recovery, benches);
criterion_main!(recovery);
