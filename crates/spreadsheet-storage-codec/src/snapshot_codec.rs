use super::style_codec;
use super::{
    CodecError, addr, decode_addr, decode_content, decode_merge_range, decode_operation,
    decode_sheet, encode_content, encode_merge_range, encode_sheet, id, operation, v1,
};
use prost::Message;
use spreadsheet_domain::workbook::{
    CalculationContext, ColumnId, DocumentId, DocumentVersion, MAX_CELLS, MAX_RETIRED_COLUMNS,
    MAX_RETIRED_ROWS, Operation, RowId, SnapshotImage, SnapshotSheetImage, Workbook,
};

/// The first reader is deliberately bounded; a larger snapshot needs a new
/// streaming decoder or a measured increase to this cap before activation.
pub const MAX_SNAPSHOT_BYTES: usize = 32 * 1024 * 1024;
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 8;

/// Inspects the bounded persisted envelope before binding its version to AAD.
/// Callers still validate its workbook and expected metadata with decode_snapshot.
pub fn snapshot_schema_version(bytes: &[u8]) -> Result<u32, CodecError> {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(CodecError::SnapshotTooLarge);
    }
    let envelope = v1::SnapshotEnvelope::decode(bytes).map_err(|_| CodecError::Invalid)?;
    if !(1..=SNAPSHOT_SCHEMA_VERSION).contains(&envelope.snapshot_schema_version) {
        return Err(CodecError::UnsupportedSchema);
    }
    Ok(envelope.snapshot_schema_version)
}

fn encode_cells(
    cells: &[(
        spreadsheet_domain::workbook::CellAddress,
        spreadsheet_domain::workbook::CellContent,
    )],
) -> Result<Vec<v1::SnapshotCell>, CodecError> {
    cells
        .iter()
        .map(|(address, content)| {
            Ok(v1::SnapshotCell {
                address: Some(addr(address)),
                content: Some(encode_content(content)?),
            })
        })
        .collect()
}

fn decode_cells(
    cells: Vec<v1::SnapshotCell>,
) -> Result<
    Vec<(
        spreadsheet_domain::workbook::CellAddress,
        spreadsheet_domain::workbook::CellContent,
    )>,
    CodecError,
> {
    cells
        .into_iter()
        .map(|cell| {
            let address = decode_addr(cell.address.ok_or(CodecError::Invalid)?)?;
            let content = decode_content(cell.content.ok_or(CodecError::Invalid)?, address)?;
            Ok((address, content))
        })
        .collect()
}

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
    let envelope =
        v1::SnapshotEnvelope {
            snapshot_schema_version: SNAPSHOT_SCHEMA_VERSION,
            document_id: image.document_id.as_bytes().to_vec(),
            document_version: image.version.get(),
            last_axis_change_version: image.last_axis_change_version.get(),
            sheet: None,
            cells: Vec::new(),
            sheets: image
                .sheets
                .iter()
                .map(|sheet| {
                    Ok(v1::SnapshotSheet {
                        frozen_rows: sheet.frozen_rows,
                        settings: Some(super::sheet_settings_codec::encode(&sheet.settings)),
                        sheet: Some(encode_sheet(&sheet.sheet)),
                        cells: encode_cells(&sheet.cells)?,
                        styles: sheet
                            .styles
                            .iter()
                            .map(|(address, style)| v1::SnapshotStyle {
                                address: Some(addr(address)),
                                style: Some(style_codec::encode_style(style)),
                            })
                            .collect(),
                        axis_sizes: sheet
                            .row_heights
                            .iter()
                            .map(|(id, size)| v1::AxisSizeOverride {
                                axis_kind: 1,
                                axis_id: id.as_bytes().to_vec(),
                                size_css_px: *size,
                            })
                            .chain(sheet.column_widths.iter().map(|(id, size)| {
                                v1::AxisSizeOverride {
                                    axis_kind: 2,
                                    axis_id: id.as_bytes().to_vec(),
                                    size_css_px: *size,
                                }
                            }))
                            .collect(),
                        merges: sheet
                            .merges
                            .iter()
                            .copied()
                            .map(encode_merge_range)
                            .collect(),
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
            row_activation_floors: image
                .row_activation_floors
                .iter()
                .map(|(row, version)| v1::RowActivationFloor {
                    row_id: row.as_bytes().to_vec(),
                    document_version: version.get(),
                })
                .collect(),
            column_activation_floors: image
                .column_activation_floors
                .iter()
                .map(|(column, version)| v1::ColumnActivationFloor {
                    column_id: column.as_bytes().to_vec(),
                    document_version: version.get(),
                })
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
    expected_last_axis_change_version: DocumentVersion,
    expected_hash: [u8; 32],
    expected_engine_version: &str,
    expected_compatibility_profile: &str,
) -> Result<Workbook, CodecError> {
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(CodecError::SnapshotTooLarge);
    }
    let envelope = v1::SnapshotEnvelope::decode(bytes).map_err(|_| CodecError::Invalid)?;
    if !matches!(
        envelope.snapshot_schema_version,
        1 | 2 | 3 | 4 | 5 | 6 | 7 | SNAPSHOT_SCHEMA_VERSION
    ) {
        return Err(CodecError::UnsupportedSchema);
    }
    // Schema 4 is a writer-normalized profile, not arbitrary protobuf ordering.
    if envelope.snapshot_schema_version >= 4 && envelope.encode_to_vec() != bytes {
        return Err(CodecError::Invalid);
    }
    if envelope.document_id.as_slice() != expected_document.as_bytes()
        || envelope.document_version != expected_version.get()
        || envelope.last_axis_change_version != expected_last_axis_change_version.get()
        || envelope.state_hash.as_slice() != expected_hash
        || envelope.writer_engine_version != expected_engine_version
        || envelope.writer_compatibility_profile != expected_compatibility_profile
        || !label(&envelope.writer_engine_version)
        || !label(&envelope.writer_compatibility_profile)
        || envelope.cells.len() > MAX_CELLS
        || envelope.retired_rows.len() > MAX_RETIRED_ROWS
        || envelope.retired_columns.len() > MAX_RETIRED_COLUMNS
        || envelope.row_activation_floors.len() > MAX_RETIRED_ROWS
        || envelope.column_activation_floors.len() > MAX_RETIRED_COLUMNS
        || envelope
            .sheets
            .iter()
            .map(|sheet| sheet.styles.len())
            .sum::<usize>()
            > spreadsheet_domain::style_write::MAX_STYLED_CELLS
        || (envelope.snapshot_schema_version < 3
            && (!envelope.row_activation_floors.is_empty()
                || !envelope.column_activation_floors.is_empty()))
        || (envelope.snapshot_schema_version < 4
            && envelope.sheets.iter().any(|sheet| !sheet.styles.is_empty()))
        || (envelope.snapshot_schema_version < 5
            && envelope
                .sheets
                .iter()
                .any(|sheet| !sheet.axis_sizes.is_empty()))
        || (envelope.snapshot_schema_version < 6
            && envelope.sheets.iter().any(|sheet| !sheet.merges.is_empty()))
        || (envelope.snapshot_schema_version < 7
            && envelope.sheets.iter().any(|sheet| sheet.frozen_rows != 0))
    {
        return Err(CodecError::Invalid);
    }
    let sheets = if envelope.snapshot_schema_version == 1 {
        if !envelope.sheets.is_empty() {
            return Err(CodecError::Invalid);
        }
        vec![SnapshotSheetImage {
            sheet: decode_sheet(envelope.sheet.ok_or(CodecError::Invalid)?)?,
            cells: decode_cells(envelope.cells)?,
            styles: Vec::new(),
            row_heights: Vec::new(),
            column_widths: Vec::new(),
            merges: Vec::new(),
            frozen_rows: 0,
            settings: spreadsheet_domain::workbook::SheetSettings::default(),
        }]
    } else {
        if envelope.sheet.is_some()
            || !envelope.cells.is_empty()
            || envelope.sheets.is_empty()
            || envelope.sheets.len() > 64
        {
            return Err(CodecError::Invalid);
        }
        envelope
            .sheets
            .into_iter()
            .map(|entry| {
                if entry.cells.len() > MAX_CELLS
                    || entry.styles.len() > spreadsheet_domain::style_write::MAX_STYLED_CELLS
                    || entry.merges.len() > spreadsheet_domain::merge::MAX_MERGE_RANGES_PER_SHEET
                {
                    return Err(CodecError::Invalid);
                }
                let seed = decode_sheet(entry.sheet.clone().ok_or(CodecError::Invalid)?)?;
                let settings = if envelope.snapshot_schema_version >= 8 {
                    super::sheet_settings_codec::decode(
                        entry.settings.clone().ok_or(CodecError::Invalid)?,
                        seed.name.clone(),
                    )?
                } else {
                    if entry.settings.is_some() {
                        return Err(CodecError::Invalid);
                    }
                    spreadsheet_domain::workbook::SheetSettings {
                        name: seed.name.clone(),
                        ..Default::default()
                    }
                };
                if envelope.snapshot_schema_version < 8
                    && entry
                        .axis_sizes
                        .iter()
                        .any(|s| s.size_css_px == if s.axis_kind == 1 { 28 } else { 112 })
                {
                    return Err(CodecError::Invalid);
                }
                let mut row_heights = Vec::new();
                let mut column_widths = Vec::new();
                let mut seen_sizes = std::collections::BTreeSet::new();
                for size in entry.axis_sizes {
                    let raw: [u8; 16] = id(&size.axis_id)?;
                    match size.axis_kind {
                        1 => {
                            let axis = RowId::from_bytes(raw).map_err(|_| CodecError::Invalid)?;
                            if !seen_sizes.insert((1u8, raw)) {
                                return Err(CodecError::Invalid);
                            }
                            row_heights.push((axis, size.size_css_px));
                        }
                        2 => {
                            let axis =
                                ColumnId::from_bytes(raw).map_err(|_| CodecError::Invalid)?;
                            if !seen_sizes.insert((2u8, raw)) {
                                return Err(CodecError::Invalid);
                            }
                            column_widths.push((axis, size.size_css_px));
                        }
                        _ => return Err(CodecError::Invalid),
                    }
                }
                Ok(SnapshotSheetImage {
                    frozen_rows: entry.frozen_rows,
                    settings,
                    sheet: decode_sheet(entry.sheet.ok_or(CodecError::Invalid)?)?,
                    cells: decode_cells(entry.cells)?,
                    styles: entry
                        .styles
                        .into_iter()
                        .map(|entry| {
                            Ok((
                                decode_addr(entry.address.ok_or(CodecError::Invalid)?)?,
                                style_codec::decode_style(entry.style.ok_or(CodecError::Invalid)?)?,
                            ))
                        })
                        .collect::<Result<Vec<_>, CodecError>>()?,
                    row_heights,
                    column_widths,
                    merges: entry
                        .merges
                        .into_iter()
                        .map(decode_merge_range)
                        .collect::<Result<Vec<_>, _>>()?,
                })
            })
            .collect::<Result<Vec<_>, CodecError>>()?
    };
    if (envelope.snapshot_schema_version < 4
        && sheets.iter().map(|sheet| sheet.cells.len()).sum::<usize>() > MAX_CELLS)
        || sheets.iter().map(|sheet| sheet.styles.len()).sum::<usize>()
            > spreadsheet_domain::style_write::MAX_STYLED_CELLS
    {
        return Err(CodecError::Invalid);
    }
    let sheets = sheets
        .into_iter()
        .map(|mut s| {
            if s.settings.name.is_empty() {
                s.settings.name = s.sheet.name.clone();
            }
            s
        })
        .collect();
    let image = SnapshotImage {
        document_id: expected_document,
        version: expected_version,
        last_axis_change_version: DocumentVersion::new(envelope.last_axis_change_version)
            .map_err(|_| CodecError::Invalid)?,
        sheets,
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
        row_activation_floors: envelope
            .row_activation_floors
            .iter()
            .map(|floor| {
                Ok((
                    RowId::from_bytes(id(&floor.row_id)?).map_err(|_| CodecError::Invalid)?,
                    DocumentVersion::new(floor.document_version)
                        .map_err(|_| CodecError::Invalid)?,
                ))
            })
            .collect::<Result<_, CodecError>>()?,
        column_activation_floors: envelope
            .column_activation_floors
            .iter()
            .map(|floor| {
                Ok((
                    ColumnId::from_bytes(id(&floor.column_id)?).map_err(|_| CodecError::Invalid)?,
                    DocumentVersion::new(floor.document_version)
                        .map_err(|_| CodecError::Invalid)?,
                ))
            })
            .collect::<Result<_, CodecError>>()?,
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
            CommittedEvent, EventPrincipal, OperationId, SheetId, SheetSeed,
        },
    };

    #[test]
    fn two_sheet_binary_snapshot_preserves_order_and_sparse_cells()
    -> Result<(), Box<dyn std::error::Error>> {
        let document = DocumentId::from_bytes([21; 16])?;
        let seeds = [
            SheetSeed {
                id: SheetId::from_bytes([22; 16])?,
                name: "Budget".into(),
                rows: vec![RowId::from_bytes([23; 16])?],
                columns: vec![spreadsheet_domain::workbook::ColumnId::from_bytes(
                    [24; 16],
                )?],
            },
            SheetSeed {
                id: SheetId::from_bytes([25; 16])?,
                name: "Plan".into(),
                rows: vec![RowId::from_bytes([26; 16])?],
                columns: vec![spreadsheet_domain::workbook::ColumnId::from_bytes(
                    [27; 16],
                )?],
            },
        ];
        let mut book = Workbook::empty(document);
        book = book.candidate(
            &Operation::CreateWorkbook {
                sheet: seeds[0].clone(),
            },
            DocumentVersion::new(1)?,
        )?;
        book = book.candidate(
            &Operation::AddSheet {
                sheet: seeds[1].clone(),
            },
            DocumentVersion::new(2)?,
        )?;
        let address = CellAddress {
            sheet: seeds[1].id,
            row: seeds[1].rows[0],
            column: seeds[1].columns[0],
        };
        book = book.candidate(
            &Operation::SetCellValue {
                address,
                value: CellValue::Number(42.0),
            },
            DocumentVersion::new(3)?,
        )?;
        let bytes = encode_snapshot(&book, "engine-v1", "pxel-excel-subset-v1")?;
        let encoded = v1::SnapshotEnvelope::decode(bytes.as_slice())?;
        assert_eq!(encoded.snapshot_schema_version, SNAPSHOT_SCHEMA_VERSION);
        assert_eq!(encoded.sheets.len(), 2);
        let restored = decode_snapshot(
            &bytes,
            document,
            book.version(),
            book.last_axis_change_version(),
            book.hash(),
            "engine-v1",
            "pxel-excel-subset-v1",
        )?;
        assert_eq!(restored.hash(), book.hash());
        assert_eq!(
            restored
                .sheets()
                .iter()
                .map(|sheet| sheet.name())
                .collect::<Vec<_>>(),
            vec!["Budget", "Plan"]
        );
        assert_eq!(restored.cell_content(address)?, book.cell_content(address)?);
        Ok(())
    }

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
            book.last_axis_change_version(),
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
        let legacy_sheet = envelope
            .sheets
            .first()
            .ok_or("missing snapshot sheet")?
            .clone();
        let mut legacy = envelope.clone();
        legacy.snapshot_schema_version = 1;
        legacy.sheet = legacy_sheet.sheet;
        legacy.cells = legacy_sheet.cells;
        legacy.sheets.clear();
        assert_eq!(
            decode_snapshot(
                &legacy.encode_to_vec(),
                document,
                book.version(),
                book.last_axis_change_version(),
                expected_hash,
                "engine-v1",
                "pxel-excel-subset-v1"
            )?
            .hash(),
            expected_hash
        );
        envelope.snapshot_schema_version = SNAPSHOT_SCHEMA_VERSION + 1;
        assert!(matches!(
            decode_snapshot(
                &envelope.encode_to_vec(),
                document,
                book.version(),
                book.last_axis_change_version(),
                expected_hash,
                "engine-v1",
                "pxel-excel-subset-v1"
            ),
            Err(CodecError::UnsupportedSchema)
        ));
        envelope.snapshot_schema_version = 2;
        for sheet in &mut envelope.sheets {
            sheet.settings = None;
        }
        if let Some(context) = envelope.calculation_context.as_mut() {
            context.engine_version = "engine-v2".into();
        }
        assert!(matches!(
            decode_snapshot(
                &envelope.encode_to_vec(),
                document,
                book.version(),
                book.last_axis_change_version(),
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
                book.last_axis_change_version(),
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
                book.last_axis_change_version(),
                expected_hash,
                "engine-v2",
                "pxel-excel-subset-v1"
            ),
            Err(CodecError::Invalid)
        ));
        assert!(matches!(
            decode_snapshot(
                &bytes,
                document,
                book.version(),
                DocumentVersion::new(3)?,
                expected_hash,
                "engine-v1",
                "pxel-excel-subset-v1"
            ),
            Err(CodecError::Invalid)
        ));
        assert!(matches!(
            decode_snapshot(
                &vec![0; MAX_SNAPSHOT_BYTES + 1],
                document,
                book.version(),
                book.last_axis_change_version(),
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
                book.last_axis_change_version(),
                [0; 32],
                "engine-v1",
                "pxel-excel-subset-v1"
            ),
            Err(CodecError::Invalid)
        ));
        Ok(())
    }
}
