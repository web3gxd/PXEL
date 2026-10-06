use super::*;
use spreadsheet_domain::workbook::{ActorId, WorkspaceId};

fn identity() -> Result<AuthenticatedActor, DomainError> {
    Ok(AuthenticatedActor {
        actor_id: ActorId::from_bytes([1; 16])?,
        workspace_id: WorkspaceId::from_bytes([2; 16])?,
    })
}
fn state(version: u64) -> Result<Arc<CalculatedState>, DomainError> {
    Ok(Arc::new(CalculatedState {
        document_id: DocumentId::from_bytes([3; 16])?,
        input_version: DocumentVersion::new(version)?,
        context: None,
        base_input_version: None,
        outcome: calculation::CalculationOutcome::Failed(
            spreadsheet_formula::calculation::CalculationError::InvalidProjection,
        ),
    }))
}
fn head_update(version: u64) -> Result<HeadUpdate, DomainError> {
    Ok(HeadUpdate {
        version: DocumentVersion::new(version)?,
        event: None,
    })
}

#[tokio::test]
async fn head_authorizes_without_requiring_a_calculated_result()
-> Result<(), Box<dyn std::error::Error>> {
    let identity = identity()?;
    let (sender, mut requests) = mpsc::channel(1);
    let (_output, calculated) = watch::channel(None);
    let (_head_sender, head_updates) = watch::channel(head_update(0)?);
    let session = DocumentSession {
        sender,
        calculated,
        head_updates,
    };
    let query = tokio::spawn(async move { session.head(identity).await });
    let Some(Request::ReadHead {
        identity: received,
        reply,
    }) = requests.recv().await
    else {
        return Err("missing head authorization request".into());
    };
    assert_eq!(received.actor_id, identity.actor_id);
    assert_eq!(received.workspace_id, identity.workspace_id);
    assert!(reply.send(Ok(DocumentVersion::new(7)?)).is_ok());
    assert_eq!(query.await??.get(), 7);
    Ok(())
}

#[tokio::test]
async fn watch_registers_before_authorization_and_keeps_current_result()
-> Result<(), Box<dyn std::error::Error>> {
    let identity = identity()?;
    let (sender, mut requests) = mpsc::channel(1);
    let (output, calculated) = watch::channel(Some(state(1)?));
    let (_head_sender, head_updates) = watch::channel(head_update(0)?);
    let session = DocumentSession {
        sender,
        calculated,
        head_updates,
    };
    let registration = tokio::spawn(async move { session.watch_calculation(identity).await });
    let Some(Request::ReadHead {
        identity: received,
        reply,
    }) = requests.recv().await
    else {
        return Err("missing authorization request".into());
    };
    assert_eq!(received.actor_id, identity.actor_id);
    assert_eq!(received.workspace_id, identity.workspace_id);
    // Completion races authorization. No new worker or polling loop is necessary.
    output.send_replace(Some(state(2)?));
    assert!(reply.send(Ok(DocumentVersion::new(2)?)).is_ok());
    let mut receiver = registration.await??;
    assert_eq!(
        receiver.borrow().as_ref().map(|s| s.input_version.get()),
        Some(2)
    );
    tokio::time::timeout(Duration::from_secs(1), receiver.changed()).await??;
    output.send_replace(Some(state(3)?));
    tokio::time::timeout(Duration::from_secs(1), receiver.changed()).await??;
    assert_eq!(
        receiver.borrow().as_ref().map(|s| s.input_version.get()),
        Some(3)
    );
    Ok(())
}

#[tokio::test]
async fn watch_never_returns_cached_content_when_authorization_fails()
-> Result<(), Box<dyn std::error::Error>> {
    let identity = identity()?;
    let (sender, mut requests) = mpsc::channel(1);
    let (_output, calculated) = watch::channel(Some(state(1)?));
    let (_head_sender, head_updates) = watch::channel(head_update(0)?);
    let session = DocumentSession {
        sender,
        calculated,
        head_updates,
    };
    let registration = tokio::spawn(async move { session.watch_calculation(identity).await });
    let Some(Request::ReadHead { reply, .. }) = requests.recv().await else {
        return Err("missing authorization request".into());
    };
    assert!(
        reply
            .send(Err(SessionError::Journal(JournalError::Unauthorized)))
            .is_ok()
    );
    assert!(matches!(
        registration.await?,
        Err(SessionError::Journal(JournalError::Unauthorized))
    ));
    Ok(())
}

#[tokio::test]
async fn head_watcher_registers_before_authorization_and_coalesces_versions()
-> Result<(), Box<dyn std::error::Error>> {
    let identity = identity()?;
    let (sender, mut requests) = mpsc::channel(1);
    let (_output, calculated) = watch::channel(None);
    let (head_sender, head_updates) = watch::channel(head_update(0)?);
    let session = DocumentSession {
        sender,
        calculated,
        head_updates,
    };
    let registration = tokio::spawn(async move { session.watch_head(identity).await });
    let Some(Request::ReadHead { reply, .. }) = requests.recv().await else {
        return Err("missing authorization request".into());
    };
    head_sender.send_replace(head_update(2)?);
    head_sender.send_replace(head_update(3)?);
    assert!(reply.send(Ok(DocumentVersion::new(3)?)).is_ok());
    let mut updates = registration.await??;
    tokio::time::timeout(Duration::from_secs(1), updates.changed()).await??;
    assert_eq!(updates.borrow_and_update().version.get(), 3);
    head_sender.send_replace(head_update(4)?);
    tokio::time::timeout(Duration::from_secs(1), updates.changed()).await??;
    assert_eq!(updates.borrow_and_update().version.get(), 4);
    Ok(())
}
