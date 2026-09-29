use spreadsheet_domain::workbook::{
    ActorId, CellAddress, CellValue, ColumnId, CommittedEvent, ContentWrite, DocumentId,
    DocumentVersion, EventPrincipal, Operation, OperationId, RowId, SheetId, SheetSeed, Workbook,
};
use std::{collections::BTreeMap, time::Instant};

const CLIENTS: usize = 3;

fn bytes(seed: u64, tag: u64) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&seed.to_be_bytes());
    bytes[8..].copy_from_slice(&tag.to_be_bytes());
    bytes
}

fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

struct Replica {
    book: Workbook,
    pending: BTreeMap<u64, CommittedEvent>,
}

impl Replica {
    fn receive(&mut self, event: &CommittedEvent) -> Result<(), Box<dyn std::error::Error>> {
        if event.version <= self.book.version() {
            return Ok(());
        }
        self.pending.insert(event.version.get(), event.clone());
        while let Some(next) = self.pending.remove(&(self.book.version().get() + 1)) {
            self.book.apply_event(&next)?;
        }
        Ok(())
    }

    fn replay(&mut self, log: &[CommittedEvent]) -> Result<(), Box<dyn std::error::Error>> {
        for event in log.iter().skip(self.book.version().get() as usize) {
            self.receive(event)?;
        }
        Ok(())
    }
}

fn run_convergence(
    scenarios: u64,
    commands_per_scenario: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    for seed in 1..=scenarios {
        let document = DocumentId::from_bytes(bytes(seed, 1))?;
        let sheet = SheetId::from_bytes(bytes(seed, 2))?;
        let actor = ActorId::from_bytes(bytes(seed, 3))?;
        let rows = (0..8)
            .map(|index| RowId::from_bytes(bytes(seed, 100 + index)))
            .collect::<Result<Vec<_>, _>>()?;
        let columns = (0..8)
            .map(|index| ColumnId::from_bytes(bytes(seed, 200 + index)))
            .collect::<Result<Vec<_>, _>>()?;
        let create = Operation::CreateWorkbook {
            sheet: SheetSeed {
                id: sheet,
                name: "Sheet".into(),
                rows: rows.clone(),
                columns: columns.clone(),
            },
        };
        let mut authority =
            Workbook::empty(document).candidate(&create, DocumentVersion::new(1)?)?;
        let mut log = vec![CommittedEvent {
            document_id: document,
            operation_id: OperationId::from_bytes(bytes(seed, 1_000))?,
            principal: EventPrincipal::User(actor),
            version: DocumentVersion::new(1)?,
            operation: create,
            state_hash: authority.hash(),
        }];
        let mut replicas: Vec<_> = (0..CLIENTS)
            .map(|_| Replica {
                book: authority.clone(),
                pending: BTreeMap::new(),
            })
            .collect();
        let mut random = seed ^ 0x9e37_79b9_7f4a_7c15;
        let mut inserts = 0;
        let mut deletes = 0;
        let mut column_inserts = 0;
        let mut column_deletes = 0;
        let mut copies = 0;
        for sequence in 1..=commands_per_scenario {
            let live_rows = authority.sheets()[0].rows();
            let live_columns = authority.sheets()[0].columns();
            let row = (next_random(&mut random) % live_rows.len() as u64) as usize;
            let column = (next_random(&mut random) % live_columns.len() as u64) as usize;
            let address = CellAddress {
                sheet,
                row: live_rows[row],
                column: live_columns[column],
            };
            let operation = if sequence.is_multiple_of(131) && live_rows.len() > rows.len() {
                Operation::DeleteRows {
                    sheet,
                    rows: vec![live_rows[row]],
                }
            } else if sequence.is_multiple_of(97) {
                Operation::InsertRows {
                    sheet,
                    before: live_rows[row],
                    rows: vec![RowId::from_bytes(bytes(seed, 10_000 + sequence))?],
                }
            } else if sequence.is_multiple_of(223) && live_columns.len() > columns.len() {
                Operation::DeleteColumns {
                    sheet,
                    columns: vec![live_columns[column]],
                }
            } else if sequence.is_multiple_of(211) {
                Operation::InsertColumnsRebased {
                    sheet,
                    before: live_columns[column],
                    columns: vec![ColumnId::from_bytes(bytes(seed, 20_000 + sequence))?],
                }
            } else if sequence.is_multiple_of(53) {
                let source_column =
                    (next_random(&mut random) % (live_columns.len() - 1) as u64) as usize;
                let destination_column =
                    (next_random(&mut random) % (live_columns.len() - 1) as u64) as usize;
                let destination_row = (next_random(&mut random) % live_rows.len() as u64) as usize;
                let writes = (0..2)
                    .map(|offset| {
                        let source = CellAddress {
                            sheet,
                            row: live_rows[row],
                            column: live_columns[source_column + offset],
                        };
                        Ok(ContentWrite {
                            address: CellAddress {
                                sheet,
                                row: live_rows[destination_row],
                                column: live_columns[destination_column + offset],
                            },
                            content: authority.cell_content(source)?.cloned(),
                        })
                    })
                    .collect::<Result<Vec<_>, spreadsheet_domain::workbook::DomainError>>()?;
                Operation::CopyRange { writes }
            } else if sequence.is_multiple_of(17) {
                Operation::ClearCell { address }
            } else {
                Operation::SetCellValue {
                    address,
                    value: CellValue::Number((seed * commands_per_scenario + sequence) as f64),
                }
            };
            match &operation {
                Operation::InsertRows { .. } => inserts += 1,
                Operation::DeleteRows { .. } => deletes += 1,
                Operation::InsertColumnsRebased { .. } => column_inserts += 1,
                Operation::DeleteColumns { .. } => column_deletes += 1,
                Operation::CopyRange { .. } => copies += 1,
                _ => {}
            }
            let version = DocumentVersion::new(sequence + 1)?;
            let next = authority.candidate(&operation, version)?;
            let event = CommittedEvent {
                document_id: document,
                operation_id: OperationId::from_bytes(bytes(seed, 1_000 + sequence))?,
                principal: EventPrincipal::User(actor),
                version,
                operation,
                state_hash: next.hash(),
            };
            authority = next;
            log.push(event.clone());
            for (index, replica) in replicas.iter_mut().enumerate() {
                // Each client spends one quarter of the run disconnected.
                let disconnected = ((sequence / 100) + index as u64).is_multiple_of(4);
                if !disconnected && !next_random(&mut random).is_multiple_of(4) {
                    replica.receive(&event)?;
                    if next_random(&mut random).is_multiple_of(7) {
                        replica.receive(&event)?;
                    }
                }
                if sequence.is_multiple_of(31) && !disconnected {
                    // A newer event can arrive before an older one; the replica buffers it.
                    replica.receive(&event)?;
                    let older = (next_random(&mut random) % 5) as usize;
                    let old_index = log.len().saturating_sub(older + 1);
                    replica.receive(&log[old_index])?;
                }
                if sequence.is_multiple_of(100) && !disconnected {
                    replica.replay(&log)?;
                }
            }
            if sequence.is_multiple_of(250) {
                let mut restored = Workbook::empty(document);
                for committed in &log {
                    restored.apply_event(committed)?;
                }
                assert_eq!(
                    restored.hash(),
                    authority.hash(),
                    "server replay seed {seed}"
                );
                authority = restored;
            }
        }
        for replica in &mut replicas {
            replica.replay(&log)?;
            assert_eq!(
                replica.book.version(),
                authority.version(),
                "version seed {seed}"
            );
            assert_eq!(replica.book.hash(), authority.hash(), "hash seed {seed}");
        }
        assert!(
            inserts > 0 && deletes > 0 && column_inserts > 0 && column_deletes > 0 && copies > 0,
            "operation mix seed {seed}"
        );
    }
    eprintln!(
        "convergence: {scenarios} seeds × {commands_per_scenario} commands, {CLIENTS} replicas, elapsed {:?}",
        started.elapsed()
    );
    Ok(())
}

#[test]
fn seeded_convergence_smoke() -> Result<(), Box<dyn std::error::Error>> {
    run_convergence(5, 300)
}

#[test]
#[ignore = "full 100,000-command stress; run `just convergence`"]
fn seeded_convergence_full() -> Result<(), Box<dyn std::error::Error>> {
    run_convergence(100, 1_000)
}
