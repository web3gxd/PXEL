//! Frozen property-level style writes. Durable inverse provenance is checked by the journal.
use crate::{style::*, workbook::*};
use std::collections::BTreeSet;

pub const MAX_STYLE_TARGETS: usize = 1_000;
pub const MAX_STYLED_CELLS: usize = 100_000;
pub const MAX_STYLE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum StyleWriteKind {
    Cell = 1,
    Range = 2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuardedStyleWrite {
    pub address: CellAddress,
    pub previous: Vec<StyleChange>,
    pub changes: Vec<StyleChange>,
}

fn canonical_patch(patch: &[StyleChange]) -> Result<Vec<StyleChange>, DomainError> {
    let style = CellStyle::default()
        .apply(patch)
        .map_err(|_| DomainError::InvalidStyle)?;
    let mut properties = patch.iter().map(StyleChange::property).collect::<Vec<_>>();
    properties.sort();
    Ok(properties.into_iter().map(|p| style.change(p)).collect())
}

fn validate_targets(
    book: &Workbook,
    addresses: &[CellAddress],
    kind: StyleWriteKind,
    require_rectangle: bool,
    max_targets: usize,
) -> Result<(), DomainError> {
    if addresses.is_empty() || addresses.len() > max_targets {
        return Err(DomainError::LimitExceeded);
    }
    if kind == StyleWriteKind::Cell && addresses.len() != 1 {
        return Err(DomainError::InvalidStyle);
    }
    let mut seen = BTreeSet::new();
    let first = addresses[0];
    let sheet = book
        .sheets()
        .iter()
        .find(|s| s.id() == first.sheet)
        .ok_or(DomainError::MissingSheet)?;
    let mut rows = BTreeSet::new();
    let mut columns = BTreeSet::new();
    for address in addresses {
        if address.sheet != first.sheet {
            return Err(DomainError::InvalidStyle);
        }
        sheet.validate_writable_address(*address)?;
        if !seen.insert(*address) {
            return Err(DomainError::InvalidStyle);
        }
        rows.insert(
            sheet
                .row_position(address.row)
                .ok_or(DomainError::MissingRow)?,
        );
        columns.insert(
            sheet
                .column_position(address.column)
                .ok_or(DomainError::MissingColumn)?,
        );
    }
    if require_rectangle
        && (rows.len() * columns.len() != addresses.len()
            || rows
                .iter()
                .zip(rows.iter().skip(1))
                .any(|(a, b)| a.checked_add(1) != Some(*b))
            || columns
                .iter()
                .zip(columns.iter().skip(1))
                .any(|(a, b)| a.checked_add(1) != Some(*b)))
    {
        return Err(DomainError::InvalidStyle);
    }
    Ok(())
}

pub fn prepare_style_write(
    book: &Workbook,
    addresses: &[CellAddress],
    patch: &[StyleChange],
    kind: StyleWriteKind,
) -> Result<Operation, DomainError> {
    validate_targets(book, addresses, kind, true, MAX_STYLE_TARGETS)?;
    let changes = canonical_patch(patch)?;
    let mut writes = Vec::with_capacity(addresses.len());
    for address in addresses {
        let sheet = book
            .sheets()
            .iter()
            .find(|s| s.id() == address.sheet)
            .ok_or(DomainError::MissingSheet)?;
        let default = CellStyle::default();
        let style = sheet.styles().get(address).unwrap_or(&default);
        writes.push(GuardedStyleWrite {
            address: *address,
            previous: changes.iter().map(|c| style.change(c.property())).collect(),
            changes: changes.clone(),
        });
    }
    writes.sort_by_key(|w| w.address);
    Ok(Operation::StyleWrite { kind, writes })
}

pub(crate) fn validate_capacity(book: &Workbook) -> Result<(), DomainError> {
    let mut count = 0usize;
    let mut bytes = 0usize;
    for sheet in book.sheets() {
        if sheet.occupied_count() > MAX_CELLS {
            return Err(DomainError::LimitExceeded);
        }
        count = count
            .checked_add(sheet.styles().len())
            .ok_or(DomainError::LimitExceeded)?;
        for style in sheet.styles().values() {
            bytes = bytes
                .checked_add(512 + style.font_family().len())
                .ok_or(DomainError::LimitExceeded)?;
        }
    }
    if count > MAX_STYLED_CELLS || bytes > MAX_STYLE_BYTES {
        return Err(DomainError::LimitExceeded);
    }
    Ok(())
}

pub(crate) fn candidate(
    book: &Workbook,
    kind: StyleWriteKind,
    writes: &[GuardedStyleWrite],
    version: DocumentVersion,
    require_rectangle: bool,
) -> Result<Workbook, DomainError> {
    bounded_candidate(
        book,
        kind,
        writes,
        version,
        require_rectangle,
        MAX_STYLE_TARGETS,
    )
}

pub(crate) fn sort_candidate(
    book: &Workbook,
    writes: &[GuardedStyleWrite],
    version: DocumentVersion,
) -> Result<Workbook, DomainError> {
    bounded_candidate(
        book,
        StyleWriteKind::Range,
        writes,
        version,
        false,
        crate::sort::MAX_SORT_CELLS as usize,
    )
}

fn bounded_candidate(
    book: &Workbook,
    kind: StyleWriteKind,
    writes: &[GuardedStyleWrite],
    version: DocumentVersion,
    require_rectangle: bool,
    max_targets: usize,
) -> Result<Workbook, DomainError> {
    if writes.is_empty() || writes.len() > max_targets {
        return Err(DomainError::LimitExceeded);
    }
    let addresses = writes.iter().map(|w| w.address).collect::<Vec<_>>();
    // Compensation reuses a frozen stable footprint; later insertions may
    // separate those cells without altering any written property.
    validate_targets(book, &addresses, kind, require_rectangle, max_targets)?;
    if writes.windows(2).any(|w| w[0].address >= w[1].address) {
        return Err(DomainError::InvalidStyle);
    }
    for write in writes {
        if canonical_patch(&write.changes)? != write.changes
            || canonical_patch(&write.previous)? != write.previous
            || !write
                .previous
                .iter()
                .map(StyleChange::property)
                .eq(write.changes.iter().map(StyleChange::property))
        {
            return Err(DomainError::InvalidStyle);
        }
        let sheet = book
            .sheets()
            .iter()
            .find(|s| s.id() == write.address.sheet)
            .ok_or(DomainError::MissingSheet)?;
        let default = CellStyle::default();
        let current = sheet.styles().get(&write.address).unwrap_or(&default);
        if write
            .previous
            .iter()
            .any(|p| current.change(p.property()) != *p)
        {
            return Err(DomainError::PreimageConflict);
        }
    }
    let mut next = book.clone();
    for write in writes {
        let sheet = next
            .sheets
            .iter_mut()
            .find(|s| s.id() == write.address.sheet)
            .ok_or(DomainError::MissingSheet)?;
        let default = CellStyle::default();
        let style = sheet
            .styles
            .get(&write.address)
            .unwrap_or(&default)
            .apply(&write.changes)
            .map_err(|_| DomainError::InvalidStyle)?;
        if style.is_default() {
            sheet.styles.remove(&write.address);
        } else {
            sheet.styles.insert(write.address, style);
        }
    }
    validate_capacity(&next)?;
    next.version = version;
    Ok(next)
}
