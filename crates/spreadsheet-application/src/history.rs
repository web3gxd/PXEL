//! Content-free discovery of a user's bounded durable undo/redo targets.
//! This reducer never authorizes an inverse; command admission does that again.
use crate::journal::JournalError;
use spreadsheet_domain::workbook::*;
use std::collections::BTreeMap;

pub const MAX_HISTORY_TARGETS: usize = 100;
pub const MAX_HISTORY_VERSIONS: u64 = 10_000;
pub const MAX_HISTORY_RECORDS: usize = 10_001;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryTarget {
    pub operation_id: OperationId,
    pub committed_version: DocumentVersion,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryEntryKind {
    Original,
    Compensation { target: HistoryTarget },
    Barrier,
    Ignore,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryEntry {
    pub target: HistoryTarget,
    pub kind: HistoryEntryKind,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryTruncation {
    VersionWindow,
    ByteBudget,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryWindow {
    pub document_id: DocumentId,
    pub actor_id: ActorId,
    pub through_version: DocumentVersion,
    pub earliest_scanned_version: DocumentVersion,
    /// Ascending contiguous suffix, including foreign and system events.
    pub entries: Vec<HistoryEntry>,
    pub truncation: Option<HistoryTruncation>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UndoHistory {
    pub document_id: DocumentId,
    pub actor_id: ActorId,
    pub through_version: DocumentVersion,
    pub earliest_scanned_version: DocumentVersion,
    /// Most recent target at the end; neither stack promises current eligibility.
    pub undo: Vec<HistoryTarget>,
    pub redo: Vec<HistoryTarget>,
    pub truncation: Option<HistoryTruncation>,
}

pub fn describe(event: &CommittedEvent, actor: ActorId) -> Result<HistoryEntry, JournalError> {
    if event.version == DocumentVersion::ZERO {
        return Err(JournalError::Corrupt);
    }
    let target = HistoryTarget {
        operation_id: event.operation_id,
        committed_version: event.version,
    };
    if event.principal != EventPrincipal::User(actor) {
        return Ok(HistoryEntry {
            target,
            kind: HistoryEntryKind::Ignore,
        });
    }
    let compensation = match &event.operation {
        Operation::CellWrite {
            kind,
            undoes,
            undoes_version,
            ..
        } => match (kind, undoes, undoes_version) {
            (None, Some(id), Some(version)) => Some((*id, *version)),
            (_, None, None) => None,
            _ => return Err(JournalError::Corrupt),
        },
        Operation::RangeWrite {
            kind,
            undoes,
            undoes_version,
            ..
        } => match (kind, undoes, undoes_version) {
            (None, Some(id), Some(version)) => Some((*id, *version)),
            (_, None, None) => None,
            _ => return Err(JournalError::Corrupt),
        },
        Operation::MoveCompensation {
            undoes,
            undoes_version,
            ..
        }
        | Operation::StyleCompensation {
            undoes,
            undoes_version,
            ..
        }
        | Operation::AxisCompensation {
            undoes,
            undoes_version,
            ..
        } => Some((*undoes, *undoes_version)),
        _ => None,
    };
    let kind = if let Some((id, version)) = compensation {
        if version == DocumentVersion::ZERO || version >= event.version || id == event.operation_id
        {
            return Err(JournalError::Corrupt);
        }
        HistoryEntryKind::Compensation {
            target: HistoryTarget {
                operation_id: id,
                committed_version: version,
            },
        }
    } else if event.operation.is_inverse_target() {
        HistoryEntryKind::Original
    } else {
        HistoryEntryKind::Barrier
    };
    Ok(HistoryEntry { target, kind })
}

pub fn reconstruct(window: HistoryWindow) -> Result<UndoHistory, JournalError> {
    let head = window.through_version.get();
    let earliest = window.earliest_scanned_version.get();
    let floor = head.saturating_sub(MAX_HISTORY_VERSIONS).max(1);
    if head == 0 {
        if earliest != 0 || !window.entries.is_empty() || window.truncation.is_some() {
            return Err(JournalError::Corrupt);
        }
    } else {
        if earliest < floor
            || earliest > head
            || window.entries.is_empty()
            || window.entries.len() > MAX_HISTORY_RECORDS
            || (head - earliest + 1) != window.entries.len() as u64
        {
            return Err(JournalError::Corrupt);
        }
        match window.truncation {
            None if earliest != 1 => return Err(JournalError::Corrupt),
            Some(_) if earliest == 1 => return Err(JournalError::Corrupt),
            Some(HistoryTruncation::VersionWindow) if earliest != floor => {
                return Err(JournalError::Corrupt);
            }
            _ => {}
        }
    }
    let mut undo = Vec::with_capacity(MAX_HISTORY_TARGETS);
    let mut redo = Vec::with_capacity(MAX_HISTORY_TARGETS);
    // Keep identities for the entire bounded suffix, including discarded stack
    // entries, so a wrong version cannot masquerade as an unknown inverse.
    let mut seen = BTreeMap::new();
    for (offset, entry) in window.entries.iter().enumerate() {
        let expected = earliest
            .checked_add(offset as u64)
            .ok_or(JournalError::Corrupt)?;
        if entry.target.committed_version.get() != expected
            || seen
                .insert(entry.target.operation_id, entry.target.committed_version)
                .is_some()
        {
            return Err(JournalError::Corrupt);
        }
    }
    for entry in window.entries {
        match entry.kind {
            HistoryEntryKind::Ignore => {}
            HistoryEntryKind::Barrier => {
                undo.clear();
                redo.clear();
            }
            HistoryEntryKind::Original => {
                push(&mut undo, entry.target);
                redo.clear();
            }
            HistoryEntryKind::Compensation { target } => {
                if target.committed_version == DocumentVersion::ZERO
                    || target.committed_version >= entry.target.committed_version
                    || seen
                        .get(&target.operation_id)
                        .is_some_and(|version| *version != target.committed_version)
                    || (target.committed_version.get() >= earliest
                        && !seen.contains_key(&target.operation_id))
                {
                    return Err(JournalError::Corrupt);
                }
                if let Some(position) = undo.iter().position(|candidate| *candidate == target) {
                    undo.remove(position);
                    push(&mut redo, entry.target);
                } else if let Some(position) =
                    redo.iter().position(|candidate| *candidate == target)
                {
                    redo.remove(position);
                    push(&mut undo, entry.target);
                } else {
                    // A compensation outside this suffix/branch/cap is itself a
                    // durable authored change and remains an inverse candidate.
                    push(&mut undo, entry.target);
                    redo.clear();
                }
            }
        }
    }
    Ok(UndoHistory {
        document_id: window.document_id,
        actor_id: window.actor_id,
        through_version: window.through_version,
        earliest_scanned_version: window.earliest_scanned_version,
        undo,
        redo,
        truncation: window.truncation,
    })
}
fn push(stack: &mut Vec<HistoryTarget>, target: HistoryTarget) {
    if stack.len() == MAX_HISTORY_TARGETS {
        stack.remove(0);
    }
    stack.push(target);
}
