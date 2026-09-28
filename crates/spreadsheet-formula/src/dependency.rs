//! Deterministic direct dependency graph; cell identity never uses display coordinates.
use spreadsheet_domain::workbook::CellAddress;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("dependency traversal budget exceeded")]
pub struct DependencyLimitExceeded;

#[derive(Clone, Debug, Default)]
pub struct DependencyGraph {
    forward: BTreeMap<CellAddress, BTreeSet<CellAddress>>,
    reverse: BTreeMap<CellAddress, BTreeSet<CellAddress>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecalculationOrder {
    pub ready: Vec<CellAddress>,
    /// Cycles and their blocked downstream nodes, NOT an exact set of SCC members.
    /// Conservative static-cycle semantics apply even to unselected IF branches.
    pub blocked: BTreeSet<CellAddress>,
}

fn charge(remaining: &mut usize) -> Result<(), DependencyLimitExceeded> {
    *remaining = remaining.checked_sub(1).ok_or(DependencyLimitExceeded)?;
    Ok(())
}

impl DependencyGraph {
    /// Replaces all edges of a formula. Call only after formula admission succeeds.
    pub fn replace(&mut self, cell: CellAddress, references: BTreeSet<CellAddress>) {
        self.remove(cell);
        for target in &references {
            self.reverse.entry(*target).or_default().insert(cell);
        }
        self.forward.insert(cell, references);
    }

    /// Removes a formula's outgoing edges, preserving formulas which reference this cell.
    pub fn remove(&mut self, cell: CellAddress) {
        if let Some(previous) = self.forward.remove(&cell) {
            for target in previous {
                if let Some(users) = self.reverse.get_mut(&target) {
                    users.remove(&cell);
                    if users.is_empty() {
                        self.reverse.remove(&target);
                    }
                }
            }
        }
    }

    pub fn dependencies(&self, cell: CellAddress) -> Option<&BTreeSet<CellAddress>> {
        self.forward.get(&cell)
    }

    /// One-hop reverse edges for a combined direct/range dirty traversal.
    pub fn dependents(&self, cell: &CellAddress) -> Option<&BTreeSet<CellAddress>> {
        self.reverse.get(cell)
    }

    /// Returns changed cells and their transitive dependents without scanning unrelated formulas.
    pub fn dirty(
        &self,
        changed: impl IntoIterator<Item = CellAddress>,
        remaining: &mut usize,
    ) -> Result<BTreeSet<CellAddress>, DependencyLimitExceeded> {
        let mut dirty = BTreeSet::new();
        let mut queue = VecDeque::new();
        for cell in changed {
            charge(remaining)?;
            if dirty.insert(cell) {
                queue.push_back(cell);
            }
        }
        while let Some(cell) = queue.pop_front() {
            if let Some(users) = self.reverse.get(&cell) {
                for user in users {
                    charge(remaining)?;
                    if dirty.insert(*user) {
                        queue.push_back(*user);
                    }
                }
            }
        }
        Ok(dirty)
    }

    /// Kahn traversal restricted to dirty formulas. Non-dirty dependencies use existing cache.
    pub fn schedule(
        &self,
        dirty: &BTreeSet<CellAddress>,
        remaining: &mut usize,
    ) -> Result<RecalculationOrder, DependencyLimitExceeded> {
        let mut incoming = BTreeMap::new();
        for cell in dirty {
            charge(remaining)?;
            if let Some(references) = self.forward.get(cell) {
                let mut count: usize = 0;
                for reference in references {
                    charge(remaining)?;
                    if dirty.contains(reference) && self.forward.contains_key(reference) {
                        count += 1;
                    }
                }
                incoming.insert(*cell, count);
            }
        }
        let mut ready: BTreeSet<_> = incoming
            .iter()
            .filter_map(|(cell, count)| (*count == 0).then_some(*cell))
            .collect();
        let mut ordered = Vec::with_capacity(incoming.len());
        while let Some(cell) = ready.pop_first() {
            charge(remaining)?;
            incoming.remove(&cell);
            ordered.push(cell);
            if let Some(users) = self.reverse.get(&cell) {
                for user in users {
                    charge(remaining)?;
                    if let Some(count) = incoming.get_mut(user) {
                        // Each edge is a set member and is visited exactly once.
                        *count = count.checked_sub(1).ok_or(DependencyLimitExceeded)?;
                        if *count == 0 {
                            ready.insert(*user);
                        }
                    }
                }
            }
        }
        Ok(RecalculationOrder {
            ready: ordered,
            blocked: incoming.into_keys().collect(),
        })
    }
}
