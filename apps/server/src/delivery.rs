use spreadsheet_domain::workbook::DocumentId;
use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};
use tokio::sync::{Mutex, RwLock, watch};

/// Process-local fencing for document payload sends and ACL mutations.
/// A future distributed leader must replace this with cross-process fencing.
pub struct DeliveryGate {
    pub lock: RwLock<()>,
    changes: watch::Sender<u64>,
}

impl DeliveryGate {
    fn new() -> Self {
        let (changes, _) = watch::channel(0);
        Self {
            lock: RwLock::new(()),
            changes,
        }
    }

    pub fn watch(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// The caller holds the write lock until the durable transaction commits.
    pub fn committed_epoch(&self, epoch: u64) {
        self.changes.send_replace(epoch);
    }
}

#[derive(Default)]
pub struct DeliveryGates {
    entries: Mutex<HashMap<DocumentId, Weak<DeliveryGate>>>,
}

impl DeliveryGates {
    pub async fn get(&self, document: DocumentId) -> Arc<DeliveryGate> {
        let mut entries = self.entries.lock().await;
        entries.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = entries.get(&document).and_then(Weak::upgrade) {
            return gate;
        }
        let gate = Arc::new(DeliveryGate::new());
        entries.insert(document, Arc::downgrade(&gate));
        gate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mutation_waits_for_inflight_send_and_notifies_current_subscriber()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = DeliveryGates::default();
        let document = DocumentId::from_bytes([1; 16])?;
        let gate = directory.get(document).await;
        let same = directory.get(document).await;
        assert!(Arc::ptr_eq(&gate, &same));
        let mut changes = gate.watch();
        let read = gate.lock.read().await;
        let write = gate.lock.write();
        tokio::pin!(write);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut write)
                .await
                .is_err()
        );
        drop(read);
        let write = write.await;
        gate.committed_epoch(4);
        assert!(changes.changed().await.is_ok());
        assert_eq!(*changes.borrow(), 4);
        drop(write);
        Ok(())
    }
}
