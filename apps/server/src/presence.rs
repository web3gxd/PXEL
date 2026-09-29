use spreadsheet_domain::workbook::{ActorId, DocumentId};
use spreadsheet_wire_protocol::v1;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, watch};
use uuid::Uuid;

const MAX_PEERS: usize = 64;
const BROADCAST_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Default)]
pub struct PresenceDirectory {
    documents: Arc<Mutex<HashMap<DocumentId, PresenceDocument>>>,
}

struct PresenceDocument {
    peers: BTreeMap<Uuid, v1::PresencePeer>,
    changes: watch::Sender<Vec<v1::PresencePeer>>,
    instance: Uuid,
    last_broadcast: Instant,
    pending: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PresenceError {
    #[error("document presence capacity reached")]
    Full,
    #[error("session is not subscribed to document")]
    NotSubscribed,
}

impl PresenceDirectory {
    fn changed(&self, document: DocumentId, entry: &mut PresenceDocument) {
        if entry.last_broadcast.elapsed() >= BROADCAST_INTERVAL {
            entry
                .changes
                .send_replace(entry.peers.values().cloned().collect());
            entry.last_broadcast = Instant::now();
            entry.pending = false;
            return;
        }
        if entry.pending {
            return;
        }
        entry.pending = true;
        let delay = BROADCAST_INTERVAL.saturating_sub(entry.last_broadcast.elapsed());
        let instance = entry.instance;
        let documents = self.documents.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let mut documents = documents.lock().await;
            if let Some(entry) = documents.get_mut(&document)
                && entry.instance == instance
                && entry.pending
            {
                entry
                    .changes
                    .send_replace(entry.peers.values().cloned().collect());
                entry.last_broadcast = Instant::now();
                entry.pending = false;
            }
        });
    }

    pub async fn join(
        &self,
        document: DocumentId,
        session: Uuid,
        actor: ActorId,
    ) -> Result<watch::Receiver<Vec<v1::PresencePeer>>, PresenceError> {
        let mut documents = self.documents.lock().await;
        let entry = documents.entry(document).or_insert_with(|| {
            let (changes, _) = watch::channel(Vec::new());
            PresenceDocument {
                peers: BTreeMap::new(),
                changes,
                instance: Uuid::new_v4(),
                last_broadcast: Instant::now() - BROADCAST_INTERVAL,
                pending: false,
            }
        });
        if entry.peers.len() >= MAX_PEERS && !entry.peers.contains_key(&session) {
            return Err(PresenceError::Full);
        }
        entry.peers.insert(
            session,
            v1::PresencePeer {
                session_id: session.as_bytes().to_vec(),
                actor_id: actor.as_bytes().to_vec(),
                anchor: None,
                focus: None,
            },
        );
        let receiver = entry.changes.subscribe();
        if entry.peers.values().any(|peer| peer.anchor.is_some()) {
            self.changed(document, entry);
        }
        Ok(receiver)
    }

    pub async fn update(
        &self,
        document: DocumentId,
        session: Uuid,
        anchor: Option<v1::CellAddress>,
        focus: Option<v1::CellAddress>,
    ) -> Result<(), PresenceError> {
        let mut documents = self.documents.lock().await;
        let entry = documents
            .get_mut(&document)
            .ok_or(PresenceError::NotSubscribed)?;
        let peer = entry
            .peers
            .get_mut(&session)
            .ok_or(PresenceError::NotSubscribed)?;
        peer.anchor = anchor;
        peer.focus = focus;
        self.changed(document, entry);
        Ok(())
    }

    pub async fn leave(&self, document: DocumentId, session: Uuid) {
        let mut documents = self.documents.lock().await;
        let Some(entry) = documents.get_mut(&document) else {
            return;
        };
        let Some(departed) = entry.peers.remove(&session) else {
            return;
        };
        if entry.peers.is_empty() {
            documents.remove(&document);
        } else if departed.anchor.is_some() {
            self.changed(document, entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn presence_is_coalesced_bounded_and_removed_on_leave()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = PresenceDirectory::default();
        let document = DocumentId::from_bytes([1; 16])?;
        let actor = ActorId::from_bytes([2; 16])?;
        let first = Uuid::from_bytes([3; 16]);
        let mut updates = directory.join(document, first, actor).await?;
        let address = v1::CellAddress {
            sheet_id: vec![4; 16],
            row_id: vec![5; 16],
            column_id: vec![6; 16],
        };
        directory
            .update(document, first, Some(address.clone()), Some(address))
            .await?;
        assert!(updates.changed().await.is_ok());
        assert_eq!(
            updates.borrow_and_update()[0]
                .anchor
                .as_ref()
                .map(|v| &v.row_id),
            Some(&vec![5; 16])
        );
        let next = v1::CellAddress {
            sheet_id: vec![4; 16],
            row_id: vec![7; 16],
            column_id: vec![6; 16],
        };
        let final_cell = v1::CellAddress {
            row_id: vec![8; 16],
            ..next.clone()
        };
        directory
            .update(document, first, Some(next.clone()), Some(next))
            .await?;
        directory
            .update(document, first, Some(final_cell.clone()), Some(final_cell))
            .await?;
        tokio::time::timeout(Duration::from_millis(250), updates.changed()).await??;
        assert_eq!(
            updates.borrow_and_update()[0]
                .anchor
                .as_ref()
                .map(|v| &v.row_id),
            Some(&vec![8; 16])
        );
        for index in 0..63u8 {
            directory
                .join(document, Uuid::from_bytes([index + 10; 16]), actor)
                .await?;
        }
        assert_eq!(
            directory
                .join(document, Uuid::from_bytes([90; 16]), actor)
                .await
                .err(),
            Some(PresenceError::Full)
        );
        directory.leave(document, first).await;
        tokio::time::timeout(Duration::from_millis(250), updates.changed()).await??;
        assert_eq!(updates.borrow().len(), 63);
        assert_eq!(
            directory.update(document, first, None, None).await,
            Err(PresenceError::NotSubscribed)
        );
        Ok(())
    }
}
