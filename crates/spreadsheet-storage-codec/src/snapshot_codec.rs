use super::{
    CodecError, addr, decode_addr, decode_content, decode_operation, encode_content, id, operation,
    v1,
};
use prost::Message;
use spreadsheet_domain::workbook::{
    CalculationContext, ColumnId, DocumentId, DocumentVersion, MAX_CELLS, MAX_COLUMNS,
    MAX_RETIRED_COLUMNS, MAX_RETIRED_ROWS, MAX_ROWS, Operation, RowId, SheetId, SheetSeed,
    SnapshotImage, Workbook,
};

/// The first reader is deliberately bounded; a larger snapshot needs a new
/// streaming decoder or a measured increase to this cap before activation.
pub const MAX_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;
const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn encode_context(context: &CalculationContext) -> Result<v1::AdvanceCalculationEpoch, CodecError> {
    match operation(&Operation::AdvanceCalculationEpoch {
        context: context.clone(),
    })?
    .kind
    {
        Some(v1::operation::Kind::AdvanceCalculationEpoch(encoded)) => Ok(encoded),
        _ => Err(CodecError::Invalid),
    }
}

fn decode_context(encoded: v1::AdvanceCalculationEpoch) -> Result<CalculationContext, CodecError> {
    match decode_operation(v1::Operation {
        kind: Some(v1::operation::Kind::AdvanceCalculationEpoch(encoded)),
    })? {
        Operation::AdvanceCalculationEpoch { context } => Ok(context),
        _ => Err(CodecError::Invalid),
    }
}

pub fn encode_snapshot(
    workbook: &Workbook,
    writer_engine_version: &str,
    writer_compatibility_profile: &str,
) -> Result<Vec<u8>, CodecError> {
    if !label(writer_engine_version) || !label(writer_compatibility_profile) {
        return Err(CodecError::Invalid);
    }
    let image = workbook.snapshot_image().map_err(|_| CodecError::Invalid)?;
    if image.calculation_context.as_ref().is_some_and(|context| {
        context.engine_version() != writer_engine_version
            || context.compatibility_profile() != writer_compatibility_profile
    }) {
        return Err(CodecError::Invalid);
    }
    let envelope = v1::SnapshotEnvelope {
        snapshot_schema_version: SNAPSHOT_SCHEMA_VERSION,
        document_id: image.document_id.as_bytes().to_vec(),
        document_version: image.version.get(),
        last_axis_change_version: image.last_axis_change_version.get(),
        sheet: Some(v1::CreateWorkbook {
            sheet_id: image.sheet.id.as_bytes().to_vec(),
            name: image.sheet.name,
            rows: image
                .sheet
                .rows
                .iter()
                .map(|id| id.as_bytes().to_vec())
                .collect(),
            columns: image
                .sheet
                .columns
                .iter()
                .map(|id| id.as_bytes().to_vec())
                .collect(),
        }),
        cells: image
            .cells
            .iter()
            .map(|(address, content)| {
                Ok(v1::SnapshotCell {
                    address: Some(addr(address)),
                    content: Some(encode_content(content)?),
                })
            })
            .collect::<Result<_, CodecError>>()?,
        retired_rows: image
            .retired_rows
            .iter()
            .map(|id| id.as_bytes().to_vec())
            .collect(),
        retired_columns: image
            .retired_columns
            .iter()
            .map(|id| id.as_bytes().to_vec())
            .collect(),
        calculation_context: image
            .calculation_context
            .as_ref()
            .map(encode_context)
            .transpose()?,
        state_hash: workbook.hash().to_vec(),
        writer_engine_version: writer_engine_version.into(),
        writer_compatibility_profile: writer_compatibility_profile.into(),
    };
    if envelope.encoded_len() > MAX_SNAPSHOT_BYTES {
        return Err(CodecError::SnapshotTooLarge);
    }
    Ok(envelope.encode_to_vec())
}

/// Expected metadata comes from an authorized, ACTIVE PostgreSQL record.
/// The blob byte checksum is verified by BlobStore before this decoder runs.
#[allow(clippy::too_many_arguments)]
pub fn decode_snapshot(
    bytes: &[u8],
    expected_document: DocumentId,
    expected_version: DocumentVersion,
    expected_hash: [u8; 32],
    expected_engine_version: &str,
    expected_compatibility_profile: &str,
) -> Result<Workbook, CodecError> {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(CodecError::SnapshotTooLarge);
    }
    let envelope = v1::SnapshotEnvelope::decode(bytes).map_err(|_| CodecError::Invalid)?;
    if envelope.snapshot_schema_version != SNAPSHOT_SCHEMA_VERSION {
        return Err(CodecError::UnsupportedSchema);
    }
    if envelope.document_id.as_slice() != expected_document.as_bytes()
        || envelope.document_version != expected_version.get()
        || envelope.state_hash.as_slice() != expected_hash
        || envelope.writer_engine_version != expected_engine_version
        || envelope.writer_compatibility_profile != expected_compatibility_profile
        || !label(&envelope.writer_engine_version)
        || !label(&envelope.writer_compatibility_profile)
        || envelope.cells.len() > MAX_CELLS
        || envelope.retired_rows.len() > MAX_RETIRED_ROWS
        || envelope.retired_columns.len() > MAX_RETIRED_COLUMNS
    {
        return Err(CodecError::Invalid);
    }
    let sheet = envelope.sheet.ok_or(CodecError::Invalid)?;
    if sheet.rows.is_empty()
        || sheet.rows.len() > MAX_ROWS
        || sheet.columns.is_empty()
        || sheet.columns.len() > MAX_COLUMNS
    {
        return Err(CodecError::Invalid);
    }
    let image = SnapshotImage {
        document_id: expected_document,
        version: expected_version,
        last_axis_change_version: DocumentVersion::new(envelope.last_axis_change_version)
            .map_err(|_| CodecError::Invalid)?,
        sheet: SheetSeed {
            id: SheetId::from_bytes(id(&sheet.sheet_id)?).map_err(|_| CodecError::Invalid)?,
            name: sheet.name,
            rows: sheet
                .rows
                .iter()
                .map(|value| RowId::from_bytes(id(value)?).map_err(|_| CodecError::Invalid))
                .collect::<Result<_, _>>()?,
            columns: sheet
                .columns
                .iter()
                .map(|value| ColumnId::from_bytes(id(value)?).map_err(|_| CodecError::Invalid))
                .collect::<Result<_, _>>()?,
        },
        cells: envelope
            .cells
            .into_iter()
            .map(|cell| {
                let address = decode_addr(cell.address.ok_or(CodecError::Invalid)?)?;
                let content = decode_content(cell.content.ok_or(CodecError::Invalid)?, address)?;
                Ok((address, content))
            })
            .collect::<Result<_, CodecError>>()?,
        retired_rows: envelope
            .retired_rows
            .iter()
            .map(|value| RowId::from_bytes(id(value)?).map_err(|_| CodecError::Invalid))
            .collect::<Result<_, _>>()?,
        retired_columns: envelope
            .retired_columns
            .iter()
            .map(|value| ColumnId::from_bytes(id(value)?).map_err(|_| CodecError::Invalid))
            .collect::<Result<_, _>>()?,
        calculation_context: envelope
            .calculation_context
            .map(decode_context)
            .transpose()?,
    };
    if image.calculation_context.as_ref().is_some_and(|context| {
        context.engine_version() != expected_engine_version
            || context.compatibility_profile() != expected_compatibility_profile
    }) {
        return Err(CodecError::Invalid);
    }
    Workbook::restore_snapshot(image, expected_hash).map_err(|_| CodecError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spreadsheet_domain::{
        formula::{AST_SCHEMA_VERSION, BoundAst, Formula, Node, NodeId},
        workbook::{
            ActorId, CalculationContextId, CalculationReason, CellAddress, CellValue,
            CommittedEvent, EventPrincipal, OperationId,
        },
    };

    #[test]
    fn binary_snapshot_roundtrip_tail_and_bad_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let document = DocumentId::from_bytes([1; 16])?;
        let sheet = SheetId::from_bytes([2; 16])?;
        let row = RowId::from_bytes([3; 16])?;
        let removed = RowId::from_bytes([4; 16])?;
        let column = ColumnId::from_bytes([5; 16])?;
        let formula_column = ColumnId::from_bytes([6; 16])?;
        let mut book = Workbook::empty(document).candidate(
            &Operation::CreateWorkbook {
                sheet: SheetSeed {
                    id: sheet,
                    name: "Sheet".into(),
                    rows: vec![row, removed],
                    columns: vec![column, formula_column],
                },
            },
            DocumentVersion::new(1)?,
        )?;
        let source = CellAddress { sheet, row, column };
        book = book.candidate(
            &Operation::SetCellValue {
                address: source,
                value: CellValue::Number(7.0),
            },
            DocumentVersion::new(2)?,
        )?;
        let inserted = RowId::from_bytes([7; 16])?;
        book = book.candidate(
            &Operation::InsertRows {
                sheet,
                before: removed,
                rows: vec![inserted],
            },
            DocumentVersion::new(3)?,
        )?;
        book = book.candidate(
            &Operation::DeleteRows {
                sheet,
                rows: vec![removed],
            },
            DocumentVersion::new(4)?,
        )?;
        let context = CalculationContext::new(
            CalculationContextId::from_bytes([8; 16])?,
            1,
            86_400_000,
            0,
            1,
            [9; 32],
            "engine-v1".into(),
            "pxel-excel-subset-v1".into(),
            CalculationReason::Explicit,
        )?;
        book = book.candidate(
            &Operation::AdvanceCalculationEpoch { context },
            DocumentVersion::new(5)?,
        )?;
        let formula_address = CellAddress {
            sheet,
            row: inserted,
            column: formula_column,
        };
        book = book.candidate(
            &Operation::SetCellFormula {
                address: formula_address,
                formula: Formula::new(
                    "=42".into(),
                    "=42".into(),
                    BoundAst::new(AST_SCHEMA_VERSION, NodeId(0), vec![Node::Number(42.0)])?,
                    formula_address,
                    DocumentVersion::new(5)?,
                )?,
            },
            DocumentVersion::new(6)?,
        )?;
        let bytes = encode_snapshot(&book, "engine-v1", "pxel-excel-subset-v1")?;
        assert!(matches!(
            encode_snapshot(&book, "engine-v2", "pxel-excel-subset-v1"),
            Err(CodecError::Invalid)
        ));
        let expected_hash = book.hash();
        let mut restored = decode_snapshot(
            &bytes,
            document,
            DocumentVersion::new(6)?,
            expected_hash,
            "engine-v1",
            "pxel-excel-subset-v1",
        )?;
        assert_eq!(restored.hash(), expected_hash);
        assert_eq!(restored.last_axis_change_version().get(), 4);
        assert_eq!(
            restored.cell_content(formula_address)?,
            book.cell_content(formula_address)?
        );
        let tail = Operation::SetCellValue {
            address: source,
            value: CellValue::Number(13.0),
        };
        let direct = book.candidate(&tail, DocumentVersion::new(7)?)?;
        restored.apply_event(&CommittedEvent {
            document_id: document,
            operation_id: OperationId::from_bytes([10; 16])?,
            principal: EventPrincipal::User(ActorId::from_bytes([11; 16])?),
            version: DocumentVersion::new(7)?,
            operation: tail,
            state_hash: direct.hash(),
        })?;
        assert_eq!(restored.hash(), direct.hash());

        let mut envelope = v1::SnapshotEnvelope::decode(bytes.as_slice())?;
        envelope.snapshot_schema_version = 2;
        assert!(matches!(
            decode_snapshot(
                &envelope.encode_to_vec(),
                document,
                book.version(),
                expected_hash,
                "engine-v1",
                "pxel-excel-subset-v1"
            ),
            Err(CodecError::UnsupportedSchema)
        ));
        envelope.snapshot_schema_version = 1;
        if let Some(context) = envelope.calculation_context.as_mut() {
            context.engine_version = "engine-v2".into();
        }
        assert!(matches!(
            decode_snapshot(
                &envelope.encode_to_vec(),
                document,
                book.version(),
                expected_hash,
                "engine-v1",
                "pxel-excel-subset-v1"
            ),
            Err(CodecError::Invalid)
        ));
        if let Some(context) = envelope.calculation_context.as_mut() {
            context.engine_version = "engine-v1".into();
        }
        envelope.retired_rows.push(removed.as_bytes().to_vec());
        assert!(matches!(
            decode_snapshot(
                &envelope.encode_to_vec(),
                document,
                book.version(),
                expected_hash,
                "engine-v1",
                "pxel-excel-subset-v1"
            ),
            Err(CodecError::Invalid)
        ));
        assert!(matches!(
            decode_snapshot(
                &bytes,
                document,
                book.version(),
                expected_hash,
                "engine-v2",
                "pxel-excel-subset-v1"
            ),
            Err(CodecError::Invalid)
        ));
        assert!(matches!(
            decode_snapshot(
                &vec![0; MAX_SNAPSHOT_BYTES + 1],
                document,
                book.version(),
                expected_hash,
                "engine-v1",
                "pxel-excel-subset-v1"
            ),
            Err(CodecError::SnapshotTooLarge)
        ));
        assert!(matches!(
            decode_snapshot(
                &bytes,
                document,
                book.version(),
                [0; 32],
                "engine-v1",
                "pxel-excel-subset-v1"
            ),
            Err(CodecError::Invalid)
        ));
        Ok(())
    }
}
