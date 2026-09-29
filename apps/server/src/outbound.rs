use crate::{AppState, auth, delivery::DeliveryGate};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, stream::SplitSink};
use spreadsheet_application::journal::{DocumentAccess, OperationJournal};
use spreadsheet_domain::workbook::DocumentId;
use std::{sync::Arc, time::Duration};
use tokio::sync::{Semaphore, mpsc, watch};

async fn send_bounded(sink: &mut SplitSink<WebSocket, Message>, message: Message) -> bool {
    match tokio::time::timeout(SEND_TIMEOUT, sink.send(message)).await {
        Ok(Ok(())) => true,
        Ok(Err(_)) => {
            tracing::warn!(event = "outbound_send_failure", reason = "socket_error");
            false
        }
        Err(_) => {
            tracing::warn!(event = "outbound_send_failure", reason = "timeout");
            false
        }
    }
}

// Replay may enqueue 128 canonical events plus its barrier before the writer runs.
const MAX_QUEUED_FRAMES: usize = 160;
const MAX_QUEUED_BYTES: u32 = 2 * 1024 * 1024;
const SEND_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct DocumentDelivery {
    pub document: DocumentId,
    pub gate: Arc<DeliveryGate>,
    pub ticket: DocumentAccess,
}

struct OutboundFrame {
    message: Message,
    delivery: Option<DocumentDelivery>,
    _bytes: tokio::sync::OwnedSemaphorePermit,
}

pub struct Outbound {
    sender: mpsc::Sender<OutboundFrame>,
    bytes: Arc<Semaphore>,
}

impl Outbound {
    pub fn enqueue(&self, message: Message, delivery: Option<DocumentDelivery>) -> bool {
        let size = match &message {
            Message::Binary(bytes) | Message::Ping(bytes) | Message::Pong(bytes) => bytes.len(),
            Message::Text(text) => text.len(),
            Message::Close(_) => 0,
        };
        let Ok(size) = u32::try_from(size.max(1)) else {
            tracing::warn!(event = "outbound_queue_overflow", limit = "frame_size");
            return false;
        };
        let Ok(permit) = self.bytes.clone().try_acquire_many_owned(size) else {
            tracing::warn!(event = "outbound_queue_overflow", limit = "bytes");
            return false;
        };
        match self.sender.try_send(OutboundFrame {
            message,
            delivery,
            _bytes: permit,
        }) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(event = "outbound_queue_overflow", limit = "frames");
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

pub fn start(
    mut sink: SplitSink<WebSocket, Message>,
    state: Arc<AppState>,
    principal: auth::Principal,
) -> (Outbound, watch::Receiver<bool>, tokio::task::JoinHandle<()>) {
    let (sender, mut receiver) = mpsc::channel::<OutboundFrame>(MAX_QUEUED_FRAMES);
    let (closed, changes) = watch::channel(false);
    let outbound = Outbound {
        sender,
        bytes: Arc::new(Semaphore::new(MAX_QUEUED_BYTES as usize)),
    };
    let task = tokio::spawn(async move {
        while let Some(frame) = receiver.recv().await {
            let sent = if let Some(delivery) = frame.delivery {
                let _read = delivery.gate.lock.read().await;
                let access = state
                    .database
                    .access(principal.identity, delivery.document)
                    .await;
                if !matches!(access, Ok(current)
                    if current.document_access_epoch == delivery.ticket.document_access_epoch
                    && current.membership_access_epoch == delivery.ticket.membership_access_epoch)
                    || !matches!(state.auth.active(&principal).await, Ok(true))
                {
                    false
                } else {
                    send_bounded(&mut sink, frame.message).await
                }
            } else {
                send_bounded(&mut sink, frame.message).await
            };
            if !sent {
                break;
            }
        }
        closed.send_replace(true);
    });
    (outbound, changes, task)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_enforces_byte_limit_even_below_frame_count_limit() {
        let (sender, _receiver) = mpsc::channel(MAX_QUEUED_FRAMES);
        let outbound = Outbound {
            sender,
            bytes: Arc::new(Semaphore::new(4)),
        };
        assert!(outbound.enqueue(Message::Binary(vec![1; 3].into()), None));
        assert!(!outbound.enqueue(Message::Binary(vec![2; 2].into()), None));
        assert!(outbound.enqueue(Message::Ping(vec![3].into()), None));
        assert!(!outbound.enqueue(Message::Pong(vec![4].into()), None));
    }

    #[test]
    fn queue_enforces_frame_limit_even_below_byte_limit() {
        let (sender, _receiver) = mpsc::channel(MAX_QUEUED_FRAMES);
        let outbound = Outbound {
            sender,
            bytes: Arc::new(Semaphore::new(MAX_QUEUED_FRAMES + 1)),
        };
        for _ in 0..MAX_QUEUED_FRAMES {
            assert!(outbound.enqueue(Message::Ping(Vec::new().into()), None));
        }
        assert!(!outbound.enqueue(Message::Ping(Vec::new().into()), None));
    }
}
