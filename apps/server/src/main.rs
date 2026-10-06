mod analysis_api;
mod auth;
mod calculation_context;
mod calculation_stream;
mod catalog_api;
mod config;
mod csv_api;
mod csv_export;
mod csv_export_api;
mod csv_import;
mod delivery;
mod disclosure_api;
mod documents;
#[cfg(test)]
mod head_history_tests;
mod history_api;
mod outbound;
mod presence;
mod privacy_api;
mod profile_api;
mod pxel_api;
mod pxel_file;
mod pxel_workbook;
mod share_password;
mod shares_api;
mod storage_maintenance;
#[cfg(feature = "legacy-migration")]
mod storage_migration;
mod storage_restore;
mod templates_api;
mod views_api;
#[cfg(feature = "dev-auth")]
mod xlsx_import;
mod xlsx_worker;

use axum::{
    Json, Router,
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::StreamExt;
use prost::Message as ProstMessage;
use serde::Serialize;
use spreadsheet_application::journal::{DocumentAccess, JournalError, OperationJournal};
use spreadsheet_collab::{
    DocumentSession, HeadUpdate, SessionDirectory,
    calculation::{COMPATIBILITY_PROFILE, ENGINE_VERSION},
};
use spreadsheet_domain::workbook::{DocumentId, DocumentVersion, SnapshotId, Workbook};
#[cfg(feature = "s3")]
use spreadsheet_storage::S3BlobStore;
use spreadsheet_storage::{
    ConfiguredBlobStore, LocalFilesystemBlobStore, PgDatabase, SnapshotError,
};
use spreadsheet_wire_protocol::{self as wire, v1};
use std::{
    num::NonZeroU16,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore, broadcast, mpsc, watch};
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;

struct AppState {
    database: PgDatabase,
    blobs: ConfiguredBlobStore,
    auth: auth::Auth,
    origins: Vec<String>,
    sockets: Arc<Semaphore>,
    calculation_encoders: Arc<Semaphore>,
    exports: Arc<Semaphore>,
    history_reads: Arc<Semaphore>,
    rate: Mutex<(Instant, u32)>,
    passwords: share_password::PasswordWorkers,
    share_admission: Arc<Semaphore>,
    sessions: spreadsheet_collab::InProcessSessionDirectory<PgDatabase>,
    delivery: delivery::DeliveryGates,
    access_events: broadcast::Sender<AccessEpochEvent>,
    presence: presence::PresenceDirectory,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccessEpochEvent {
    Document { document: DocumentId, epoch: u64 },
    Reconnect,
}

async fn access_epoch_worker(
    state: Arc<AppState>,
    mut listener: spreadsheet_storage::AccessEpochListener,
) {
    loop {
        match listener.recv().await {
            Ok(change) => {
                state
                    .delivery
                    .get(change.document)
                    .await
                    .committed_epoch(change.epoch);
                let _ = state.access_events.send(AccessEpochEvent::Document {
                    document: change.document,
                    epoch: change.epoch,
                });
            }
            Err(_) => {
                // Fail closed while LISTEN is unavailable. Any later access
                // change may have happened in the reconnect gap.
                let _ = state.access_events.send(AccessEpochEvent::Reconnect);
                tracing::warn!(event = "access_epoch_listener_disconnected");
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    match state.database.listen_access_epochs().await {
                        Ok(reconnected) => {
                            listener = reconnected;
                            break;
                        }
                        Err(_) => tracing::warn!(event = "access_epoch_listener_reconnect_failed"),
                    }
                }
            }
        }
    }
}

#[derive(Serialize)]
struct Status {
    status: &'static str,
}

async fn create_blob_store(config: &config::Config) -> anyhow::Result<ConfiguredBlobStore> {
    #[cfg(feature = "s3")]
    if let Some(settings) = &config.s3 {
        return Ok(ConfiguredBlobStore::S3(
            S3BlobStore::new(
                &settings.endpoint,
                &settings.bucket,
                &settings.region,
                64 * 1024 * 1024,
            )
            .await?,
        ));
    }
    Ok(ConfiguredBlobStore::Local(
        LocalFilesystemBlobStore::new(&config.blob_root, 64 * 1024 * 1024).await?,
    ))
}

async fn snapshot_worker(
    database: PgDatabase,
    blobs: ConfiguredBlobStore,
    mut receiver: mpsc::Receiver<Arc<Workbook>>,
) {
    while let Some(workbook) = receiver.recv().await {
        let Ok(snapshot_id) = SnapshotId::from_bytes(*Uuid::now_v7().as_bytes()) else {
            tracing::error!(event = "snapshot_id_generation_failed");
            continue;
        };
        let version = workbook.version().get();
        if database
            .publish_snapshot(
                &blobs,
                snapshot_id,
                workbook,
                ENGINE_VERSION,
                COMPATIBILITY_PROFILE,
            )
            .await
            .is_err()
        {
            tracing::warn!(event = "snapshot_publication_failed", version);
        }
    }
}

async fn snapshot_cleanup_worker(
    database: PgDatabase,
    blobs: ConfiguredBlobStore,
    incomplete_retention: Duration,
    active_retention: Duration,
) {
    let Some(batch) = NonZeroU16::new(100) else {
        return;
    };
    let mut ticker = tokio::time::interval(Duration::from_secs(3600));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match database
            .retire_old_active_snapshots(active_retention, batch)
            .await
        {
            Ok(retired) => tracing::info!(event = "snapshot_active_retirement", retired),
            Err(_) => tracing::warn!(event = "snapshot_active_retirement_failed"),
        }
        match database
            .cleanup_incomplete_snapshots(&blobs, incomplete_retention, batch)
            .await
        {
            Ok(report) => tracing::info!(
                event = "snapshot_cleanup",
                rejected_records = report.rejected_records,
                delete_attempts = report.delete_attempts,
            ),
            Err(_) => tracing::warn!(event = "snapshot_cleanup_failed"),
        }
    }
}

async fn share_expiry_worker(state: Arc<AppState>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        let documents = match state.database.expired_share_documents(100).await {
            Ok(documents) => documents,
            Err(_) => {
                tracing::warn!(event = "share_expiry_scan_failed");
                continue;
            }
        };
        for document in documents {
            let gate = state.delivery.get(document).await;
            let Ok(_write) = tokio::time::timeout(Duration::from_secs(3), gate.lock.write()).await
            else {
                continue;
            };
            match state.database.expire_document_shares(document).await {
                Ok(Some(receipt)) if receipt.newly_committed => {
                    gate.committed_epoch(receipt.access_epoch)
                }
                Ok(_) => {}
                Err(_) => tracing::warn!(event = "share_expiry_commit_failed"),
            }
        }
    }
}

struct HeadSubscription {
    document: DocumentId,
    session: DocumentSession,
    gate: Arc<delivery::DeliveryGate>,
    ticket: DocumentAccess,
    access_changes: watch::Receiver<u64>,
    updates: watch::Receiver<HeadUpdate>,
    peers: watch::Receiver<Vec<v1::PresencePeer>>,
    last_sent: DocumentVersion,
}

use outbound::DocumentDelivery;

enum SubscriptionEvent {
    Head(HeadUpdate),
    Presence(Vec<v1::PresencePeer>),
    AccessChanged,
    Closed,
}

async fn next_subscription_event(
    subscription: &mut Option<HeadSubscription>,
) -> Option<SubscriptionEvent> {
    let Some(active) = subscription.as_mut() else {
        return std::future::pending().await;
    };
    let event = tokio::select! {
        changed = active.access_changes.changed() => {
            if changed.is_ok() { SubscriptionEvent::AccessChanged } else { SubscriptionEvent::Closed }
        }
        changed = active.updates.changed() => {
            if changed.is_ok() { SubscriptionEvent::Head(active.updates.borrow_and_update().clone()) }
            else { SubscriptionEvent::Closed }
        }
        changed = active.peers.changed() => {
            if changed.is_ok() { SubscriptionEvent::Presence(active.peers.borrow_and_update().clone()) }
            else { SubscriptionEvent::Closed }
        }
    };
    if matches!(event, SubscriptionEvent::Closed) {
        return Some(SubscriptionEvent::Closed);
    }
    Some(event)
}

fn send_document_payload(
    outbound: &outbound::Outbound,
    delivery: &DocumentDelivery,
    payload: Vec<u8>,
) -> bool {
    outbound.enqueue(Message::Binary(payload.into()), Some(delivery.clone()))
}

const MAX_REPLAY_EVENTS: u64 = 128;
const MAX_REPLAY_BYTES: usize = 1024 * 1024;

async fn send_replay(
    outbound: &outbound::Outbound,
    state: &AppState,
    principal: &auth::Principal,
    session: &DocumentSession,
    delivery: &DocumentDelivery,
    after: DocumentVersion,
    barrier: DocumentVersion,
) -> Result<bool, ()> {
    let gap = barrier.get().checked_sub(after.get()).ok_or(())?;
    if gap > MAX_REPLAY_EVENTS {
        return send_replay_fallback(outbound, principal, session, delivery, barrier, None).await;
    }
    let mut cursor = after.get();
    let mut sent_bytes = 0usize;
    while cursor < barrier.get() {
        let limit = barrier.get().saturating_sub(cursor).min(8) as u32;
        let events = state
            .database
            .load_events(
                principal.identity,
                delivery.document,
                DocumentVersion::new(cursor).map_err(|_| ())?,
                limit,
            )
            .await
            .map_err(|_| ())?;
        if events.is_empty() {
            return Err(());
        }
        for event in events {
            cursor = cursor.checked_add(1).ok_or(())?;
            if event.version.get() != cursor || cursor > barrier.get() {
                return Err(());
            }
            if documents::committed_message(&event)
                .is_none_or(|message| message.canonical_operation.is_none())
            {
                let exact_head = (event.version == barrier).then(|| Arc::new(event));
                return send_replay_fallback(
                    outbound, principal, session, delivery, barrier, exact_head,
                )
                .await;
            }
            let payload = head_hint(
                delivery.document,
                &HeadUpdate {
                    version: event.version,
                    event: Some(Arc::new(event)),
                },
            );
            // Bound encoded bytes in addition to each journal read batch.
            sent_bytes = sent_bytes.checked_add(payload.len()).ok_or(())?;
            if sent_bytes > MAX_REPLAY_BYTES {
                return send_replay_fallback(outbound, principal, session, delivery, barrier, None)
                    .await;
            }
            if !matches!(session.head(principal.identity).await, Ok(head) if head >= barrier)
                || !send_document_payload(outbound, delivery, payload)
            {
                return Err(());
            }
        }
    }
    Ok(false)
}

async fn send_replay_fallback(
    outbound: &outbound::Outbound,
    principal: &auth::Principal,
    session: &DocumentSession,
    delivery: &DocumentDelivery,
    barrier: DocumentVersion,
    exact_head: Option<Arc<spreadsheet_domain::workbook::CommittedEvent>>,
) -> Result<bool, ()> {
    if !matches!(session.head(principal.identity).await, Ok(head) if head >= barrier)
        || !send_document_payload(
            outbound,
            delivery,
            head_hint_with_operation(
                delivery.document,
                &HeadUpdate {
                    version: barrier,
                    event: exact_head,
                },
                false,
            ),
        )
    {
        return Err(());
    }
    Ok(true)
}

async fn send_replay_complete(
    outbound: &outbound::Outbound,
    state: &AppState,
    principal: &auth::Principal,
    session: &DocumentSession,
    delivery: &DocumentDelivery,
    barrier: DocumentVersion,
    resync_required: bool,
) -> bool {
    if !matches!(session.head(principal.identity).await, Ok(head) if head >= barrier)
        || !matches!(state.auth.active(principal).await, Ok(true))
    {
        return false;
    }
    let payload = v1::ServerEnvelope {
        protocol_version: wire::PROTOCOL_VERSION,
        payload: Some(v1::server_envelope::Payload::ReplayComplete(
            v1::ReplayComplete {
                document_id: delivery.document.as_bytes().to_vec(),
                barrier_version: barrier.get(),
                resync_required,
            },
        )),
    }
    .encode_to_vec();
    send_document_payload(outbound, delivery, payload)
}

fn head_hint(document: DocumentId, update: &HeadUpdate) -> Vec<u8> {
    head_hint_with_operation(document, update, true)
}

fn guest_head_hint(document: DocumentId, version: DocumentVersion) -> Vec<u8> {
    v1::ServerEnvelope {
        protocol_version: wire::PROTOCOL_VERSION,
        payload: Some(v1::server_envelope::Payload::DocumentHeadChanged(
            v1::DocumentHeadChanged {
                document_id: document.as_bytes().to_vec(),
                head_version: version.get(),
                latest_operation: None,
                head_event_kind: v1::HeadEventKind::Unspecified as i32,
                head_actor_id: Vec::new(),
            },
        )),
    }
    .encode_to_vec()
}

fn head_hint_with_operation(
    document: DocumentId,
    update: &HeadUpdate,
    include_operation: bool,
) -> Vec<u8> {
    let event = update
        .event
        .as_ref()
        .filter(|event| event.document_id == document && event.version == update.version);
    let latest_operation = event
        .filter(|_| include_operation)
        .and_then(|event| documents::committed_message(event));
    let (head_event_kind, head_actor_id) = match event.map(|event| event.principal) {
        Some(spreadsheet_domain::workbook::EventPrincipal::User(actor)) => {
            (v1::HeadEventKind::User as i32, actor.as_bytes().to_vec())
        }
        Some(spreadsheet_domain::workbook::EventPrincipal::System) => {
            (v1::HeadEventKind::System as i32, Vec::new())
        }
        None => (v1::HeadEventKind::Unspecified as i32, Vec::new()),
    };
    v1::ServerEnvelope {
        protocol_version: wire::PROTOCOL_VERSION,
        payload: Some(v1::server_envelope::Payload::DocumentHeadChanged(
            v1::DocumentHeadChanged {
                document_id: document.as_bytes().to_vec(),
                head_version: update.version.get(),
                latest_operation,
                head_event_kind,
                head_actor_id,
            },
        )),
    }
    .encode_to_vec()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut arguments = std::env::args_os();
    let _program = arguments.next();
    let command = arguments.next();
    if command.as_deref() == Some(std::ffi::OsStr::new("storage")) {
        #[cfg(feature = "legacy-migration")]
        {
            let args: Vec<_> = std::env::args_os().skip(1).collect();
            return storage_migration::run(&args)
                .await
                .map_err(|error| anyhow::anyhow!("{error}"));
        }
        #[cfg(not(feature = "legacy-migration"))]
        anyhow::bail!("storage maintenance requires the legacy-migration build feature");
    }
    if command.as_deref() == Some(std::ffi::OsStr::new("backup")) {
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if args.get(1).is_some_and(|arg| arg == "wal-restore") {
            return storage_restore::run_wal_restore(&args)
                .await
                .map_err(|error| anyhow::anyhow!("{error}"));
        }
        if args
            .get(1)
            .is_some_and(|arg| arg == "restore" || arg == "recovery-check")
        {
            return storage_restore::run(&args)
                .await
                .map_err(|error| anyhow::anyhow!("{error}"));
        }
        storage_maintenance::run(&args)
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        return Ok(());
    }
    let key_rotation_target = if command.as_deref() == Some(std::ffi::OsStr::new("keys")) {
        if arguments.next().as_deref() != Some(std::ffi::OsStr::new("rotate"))
            || arguments.next().as_deref() != Some(std::ffi::OsStr::new("--to"))
        {
            anyhow::bail!("usage: pxel-server keys rotate --to <key_id>");
        }
        let target = arguments
            .next()
            .and_then(|value| value.into_string().ok())
            .ok_or_else(|| anyhow::anyhow!("key rotation requires a key ID"))?;
        if arguments.next().is_some() {
            anyhow::bail!("unexpected key rotation argument");
        }
        Some(target)
    } else {
        None
    };
    if command.as_deref() == Some(std::ffi::OsStr::new("xlsx-worker")) {
        if arguments.next().is_some() {
            anyhow::bail!("XLSX worker accepts no command-line arguments");
        }
        xlsx_worker::run()?;
        return Ok(());
    }
    if command.as_deref() == Some(std::ffi::OsStr::new("xlsx-preview")) {
        let path = arguments
            .next()
            .ok_or_else(|| anyhow::anyhow!("XLSX command requires a file path"))?;
        if arguments.next().is_some() {
            anyhow::bail!("XLSX command accepts one file path");
        }
        let counts = xlsx_worker::preview(std::path::Path::new(&path)).await?;
        println!(
            "XLSX preview: {} sheets, {} cells",
            counts.sheets, counts.cells
        );
        return Ok(());
    }
    #[cfg(not(feature = "dev-auth"))]
    if command.as_deref() == Some(std::ffi::OsStr::new("xlsx-import")) {
        anyhow::bail!("XLSX import is unavailable without the development authentication feature");
    }
    #[cfg(feature = "dev-auth")]
    if command.as_deref() == Some(std::ffi::OsStr::new("xlsx-import")) {
        let path = arguments
            .next()
            .ok_or_else(|| anyhow::anyhow!("XLSX import requires a file path"))?;
        let workspace = arguments
            .next()
            .ok_or_else(|| anyhow::anyhow!("XLSX import requires a workspace ID"))?;
        let actor = arguments
            .next()
            .ok_or_else(|| anyhow::anyhow!("XLSX import requires an actor ID"))?;
        let loss_policy = match arguments.next() {
            None => xlsx_import::LossPolicy::Reject,
            Some(flag) if flag == std::ffi::OsStr::new("--allow-loss") => {
                xlsx_import::LossPolicy::AcceptReported
            }
            Some(_) => anyhow::bail!("XLSX import accepts only --allow-loss as an optional flag"),
        };
        if arguments.next().is_some() {
            anyhow::bail!("XLSX import accepts at most four arguments");
        }
        let config = config::Config::load()?;
        if !config.dev_auth {
            anyhow::bail!("XLSX import requires local development authentication");
        }
        let parse_id = |raw: std::ffi::OsString| -> anyhow::Result<[u8; 16]> {
            let text = raw
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("import ID is not Unicode"))?;
            Ok(*Uuid::parse_str(text)?.as_bytes())
        };
        let identity = spreadsheet_application::journal::AuthenticatedActor {
            workspace_id: spreadsheet_domain::workbook::WorkspaceId::from_bytes(parse_id(
                workspace,
            )?)?,
            actor_id: spreadsheet_domain::workbook::ActorId::from_bytes(parse_id(actor)?)?,
        };
        let normalized = xlsx_worker::load(std::path::Path::new(&path)).await?;
        if !normalized.issues.is_empty() {
            for issue in &normalized.issues {
                eprintln!(
                    "XLSX compatibility issue: {:?} (sheet index {:?})",
                    issue.code, issue.sheet_index
                );
            }
            if loss_policy == xlsx_import::LossPolicy::Reject {
                anyhow::bail!("XLSX import would lose unsupported features");
            }
        }
        let document = DocumentId::from_bytes(*Uuid::now_v7().as_bytes())?;
        let events = xlsx_import::prepare(&normalized, document, identity.actor_id, loss_policy)?;
        let database = PgDatabase::connect(&config.database_url, 2).await?;
        database.readiness().await?;
        let receipt = database.create_imported(identity, &events).await?;
        database.close().await;
        println!(
            "Imported document {} at version {} ({} committed events)",
            Uuid::from_bytes(*document.as_bytes()),
            receipt.committed_version.get(),
            events.len()
        );
        return Ok(());
    }
    tracing_subscriber::fmt()
        .json()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "pxel_server=info".into()),
        )
        .init();
    let config = config::Config::load()?;
    let database = PgDatabase::connect(&config.database_url, 8)
        .await
        .map_err(|_| anyhow::anyhow!("PostgreSQL connection failed; credentials are not logged"))?;
    database
        .migrate()
        .await
        .map_err(|_| anyhow::anyhow!("database migrations failed"))?;
    database
        .readiness()
        .await
        .map_err(|_| anyhow::anyhow!("database schema or durability settings are not ready"))?;
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        tracing::info!(event = "migrations_complete");
        database.close().await;
        return Ok(());
    }
    database
        .encryption_readiness()
        .await
        .map_err(|_| anyhow::anyhow!("encrypted storage or retained keys are not ready"))?;
    if let Some(target) = key_rotation_target {
        let report = database.rotate_keys(&target).await.map_err(|_| {
            anyhow::anyhow!(
                "key rotation failed; retained keys and maintenance availability must be checked"
            )
        })?;
        tracing::info!(event="key_rotation_complete", rewrapped=report.rewrapped, verified=report.verified, active_kek_id=%report.active_kek_id);
        database.close().await;
        return Ok(());
    }
    let access_listener = database
        .listen_access_epochs()
        .await
        .map_err(|_| anyhow::anyhow!("document access listener could not start"))?;
    let blobs = create_blob_store(&config).await?;
    let recovery_database = database.clone();
    let recovery_blobs = blobs.clone();
    let historical_database = database.clone();
    let historical_blobs = blobs.clone();
    let (snapshot_sender, snapshot_receiver) = mpsc::channel(1);
    tokio::spawn(snapshot_worker(
        database.clone(),
        blobs.clone(),
        snapshot_receiver,
    ));
    tokio::spawn(snapshot_cleanup_worker(
        database.clone(),
        blobs.clone(),
        config.snapshot_cleanup_retention,
        config.snapshot_active_retention,
    ));
    let snapshot_interval = config.snapshot_interval;
    let (access_events, _) = broadcast::channel(256);
    let state = Arc::new(AppState {
        auth: auth::Auth::new(config.dev_auth, database.clone()),
        sessions: spreadsheet_collab::InProcessSessionDirectory::with_recovery(
            database.clone(),
            move |identity, document| {
                let database = recovery_database.clone();
                let blobs = recovery_blobs.clone();
                async move {
                    let started = Instant::now();
                    let recovered = database.recover_document(&blobs, identity, document).await;
                    if let Ok(result) = &recovered {
                        tracing::info!(
                            event = "document_recovery",
                            snapshot_version =
                                result.snapshot_version.map_or(0, |version| version.get()),
                            replayed_events = result.replayed_events,
                            recovery_duration_ms = started.elapsed().as_millis(),
                        );
                    }
                    recovered
                        .map(|result| result.workbook)
                        .map_err(|error| match error {
                            SnapshotError::Journal(error) => error,
                            SnapshotError::Corrupt
                            | SnapshotError::Codec(_)
                            | SnapshotError::SourceMismatch
                            | SnapshotError::TooLarge
                            | SnapshotError::VersionNotCommitted
                            | SnapshotError::PublicationRetired => JournalError::Corrupt,
                            SnapshotError::Blob(_) | SnapshotError::Database(_) => {
                                JournalError::Unavailable
                            }
                            SnapshotError::WorkerFailed | SnapshotError::Crypto(_) => {
                                JournalError::Unavailable
                            }
                        })
                }
            },
        )
        .with_historical_recovery(move |identity, document, version| {
            let database = historical_database.clone();
            let blobs = historical_blobs.clone();
            async move {
                database
                    .recover_document_at_version(&blobs, identity, document, version)
                    .await
                    .map(|result| result.workbook)
                    .map_err(|error| match error {
                        SnapshotError::Journal(error) => error,
                        SnapshotError::Blob(_)
                        | SnapshotError::Database(_)
                        | SnapshotError::WorkerFailed => JournalError::Unavailable,
                        _ => JournalError::Corrupt,
                    })
            }
        })
        .with_context_sampler(calculation_context::sample)
        .with_snapshot_enqueue(snapshot_interval, move |workbook| {
            if snapshot_sender.try_send(workbook).is_err() {
                tracing::warn!(event = "snapshot_queue_full_or_closed");
            }
        }),
        delivery: delivery::DeliveryGates::default(),
        access_events,
        presence: presence::PresenceDirectory::default(),
        database,
        blobs,
        origins: config.allowed_origins,
        sockets: Arc::new(Semaphore::new(128)),
        calculation_encoders: Arc::new(Semaphore::new(2)),
        exports: Arc::new(Semaphore::new(1)),
        history_reads: Arc::new(Semaphore::new(2)),
        rate: Mutex::new((Instant::now(), 0)),
        passwords: share_password::PasswordWorkers::new().await?,
        share_admission: Arc::new(Semaphore::new(2)),
    });
    tokio::spawn(share_expiry_worker(state.clone()));
    tokio::spawn(access_epoch_worker(state.clone(), access_listener));
    let app = Router::new()
        .route(
            "/healthz",
            get(|| async { Json(Status { status: "alive" }) }),
        )
        .route("/readyz", get(ready))
        .route("/api/ws", get(websocket))
        .route("/api/shared/{link}/ws", get(shared_websocket))
        .route(
            "/api/documents/{id}/calculation",
            get(documents::get_calculation),
        )
        .route(
            "/api/dev/session",
            post(dev_session).delete(revoke_dev_session),
        )
        .route("/api/dev/identity", get(dev_identity))
        .route("/api/notifications", get(shares_api::notifications))
        .route(
            "/api/documents",
            get(catalog_api::list).post(documents::create),
        )
        .route(
            "/api/library/layout",
            axum::routing::put(catalog_api::layout),
        )
        .route(
            "/api/documents/{id}/metadata",
            get(catalog_api::metadata).put(catalog_api::set_metadata),
        )
        .route(
            "/api/documents/{id}/favorite",
            axum::routing::put(catalog_api::favorite),
        )
        .route("/api/documents/{id}/opened", post(catalog_api::touch))
        .route("/api/library/templates", post(templates_api::create))
        .route("/api/documents/{id}", get(documents::get_document))
        .route("/api/documents/{id}/profile", post(profile_api::profile))
        .route("/api/documents/{id}/history", get(history_api::history))
        .route(
            "/api/documents/{id}/history/{version}",
            get(history_api::historical),
        )
        .route("/api/documents/{id}/diff", get(history_api::compare))
        .route(
            "/api/documents/{id}/privacy",
            get(privacy_api::get_policy).put(privacy_api::set_policy),
        )
        .route(
            "/api/documents/{id}/privacy/analyze",
            post(privacy_api::analyze_owner),
        )
        .route(
            "/api/documents/{id}/diagnostics",
            post(analysis_api::diagnostics),
        )
        .route(
            "/api/documents/{id}/shares",
            get(shares_api::list).post(shares_api::create),
        )
        .route(
            "/api/documents/{id}/shares/{link}",
            axum::routing::delete(shares_api::revoke),
        )
        .route(
            "/api/documents/{id}/comments",
            get(shares_api::list_comments).post(shares_api::add_comment),
        )
        .route(
            "/api/documents/{id}/comments/{comment}",
            axum::routing::delete(shares_api::delete_comment),
        )
        .route(
            "/api/shared/{link}/session",
            get(shares_api::session_get).post(shares_api::session_post),
        )
        .route(
            "/api/shared/{link}/document",
            get(shares_api::shared_document),
        )
        .route(
            "/api/shared/{link}/calculation-context",
            get(shares_api::guest_calculation_context),
        )
        .route(
            "/api/shared/{link}/recalculate",
            post(shares_api::guest_recalculate),
        )
        .route(
            "/api/shared/{link}/undo-history",
            get(shares_api::guest_undo_history),
        )
        .route(
            "/api/shared/{link}/comments",
            get(shares_api::list_guest_comments).post(shares_api::add_guest_comment),
        )
        .route(
            "/api/shared/{link}/comments/{comment}",
            axum::routing::delete(shares_api::delete_guest_comment),
        )
        .route(
            "/api/documents/{id}/undo-history",
            get(documents::undo_history),
        )
        .route(
            "/api/documents/{id}/export.xlsx",
            get(documents::export_xlsx),
        )
        .route(
            "/api/documents/{id}/export.csv",
            get(csv_export_api::export),
        )
        .route(
            "/api/documents/{id}/disclosure-policy",
            get(disclosure_api::get_policy).put(disclosure_api::set_policy),
        )
        .route("/api/documents/{id}/audit", get(disclosure_api::audit))
        .route(
            "/api/documents/{id}/personal-view",
            get(views_api::get).put(views_api::put),
        )
        .route(
            "/api/shared/{link}/personal-view",
            get(views_api::guest_get).put(views_api::guest_put),
        )
        .route(
            "/api/documents/{id}/clipboard/authorize",
            post(disclosure_api::clipboard),
        )
        .route(
            "/api/shared/{link}/clipboard/authorize",
            post(disclosure_api::guest_clipboard),
        )
        .route(
            "/api/shared/{link}/export.xlsx",
            get(documents::export_shared_xlsx),
        )
        .route(
            "/api/shared/{link}/export.csv",
            get(csv_export_api::shared_export),
        )
        .route(
            "/api/documents/{id}/versions/{version}",
            get(documents::get_document_version),
        )
        .route(
            "/api/documents/{id}/permissions",
            post(documents::change_permission),
        )
        .route(
            "/api/documents/{id}/recalculate",
            post(documents::recalculate),
        )
        .route("/api/documents/{id}/export.pxel", post(pxel_api::export))
        .route("/api/shared/{id}/export.pxel", post(pxel_api::guest_export))
        .fallback_service(
            ServeDir::new("apps/web/dist")
                .not_found_service(ServeFile::new("apps/web/dist/index.html")),
        )
        .layer(tower_http::limit::RequestBodyLimitLayer::new(1024 * 1024))
        .merge(
            Router::new()
                .route("/api/imports/pxel/preview", post(pxel_api::preview))
                .route("/api/imports/pxel", post(pxel_api::import))
                .layer(axum::extract::DefaultBodyLimit::max(48 * 1024 * 1024))
                .layer(tower_http::limit::RequestBodyLimitLayer::new(
                    48 * 1024 * 1024,
                )),
        )
        .merge(
            Router::new()
                .route("/api/imports/csv/preview", post(csv_api::preview))
                .route("/api/imports/csv", post(csv_api::import))
                .layer(axum::extract::DefaultBodyLimit::max(24 * 1024 * 1024))
                .layer(tower_http::limit::RequestBodyLimitLayer::new(
                    24 * 1024 * 1024,
                )),
        )
        .layer(middleware::from_fn(security_headers))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(config.address).await?;
    tracing::info!(event = "server_started", address = %config.address, development_auth = config.dev_auth);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

async fn ready(State(state): State<Arc<AppState>>) -> Response {
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        state.database.readiness().await.map_err(|_| ())?;
        state.blobs.readiness().await.map_err(|_| ())
    })
    .await;
    match result {
        Ok(Ok(())) => Json(Status { status: "ready" }).into_response(),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(Status {
                status: "unavailable",
            }),
        )
            .into_response(),
    }
}

async fn dev_identity(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !auth::valid_origin(&headers, &state.origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match state.auth.authenticate(&headers).await {
        Ok(Some(principal)) => {
            #[derive(Serialize)]
            struct IdentityView {
                actor_id: String,
                workspace_id: String,
            }
            Json(IdentityView {
                actor_id: Uuid::from_bytes(*principal.identity.actor_id.as_bytes()).to_string(),
                workspace_id: Uuid::from_bytes(*principal.identity.workspace_id.as_bytes())
                    .to_string(),
            })
            .into_response()
        }
        Ok(None) => StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn rate_allowed(state: &AppState) -> bool {
    let mut rate = state.rate.lock().await;
    let now = Instant::now();
    if now.duration_since(rate.0) >= Duration::from_secs(1) {
        *rate = (now, 0);
    }
    if rate.1 >= 64 {
        return false;
    }
    rate.1 += 1;
    true
}

async fn dev_session(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !auth::valid_origin(&headers, &state.origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !rate_allowed(&state).await {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    #[cfg(feature = "dev-auth")]
    {
        let Ok(Some(cookie)) = state.auth.issue(&headers).await else {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };
        let Ok(cookie) = HeaderValue::from_str(&cookie) else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert(header::SET_COOKIE, cookie);
        response
    }
    #[cfg(not(feature = "dev-auth"))]
    StatusCode::NOT_FOUND.into_response()
}

async fn revoke_dev_session(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !auth::valid_origin(&headers, &state.origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    #[cfg(feature = "dev-auth")]
    {
        if state.auth.revoke(&headers).await.is_err() {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert(
            header::SET_COOKIE,
            HeaderValue::from_static(
                "pxel_dev_session=; HttpOnly; SameSite=Strict; Path=/api; Max-Age=0",
            ),
        );
        response
    }
    #[cfg(not(feature = "dev-auth"))]
    StatusCode::NOT_FOUND.into_response()
}

async fn websocket(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !auth::valid_origin(&headers, &state.origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !rate_allowed(&state).await {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let principal = match state.auth.authenticate(&headers).await {
        Ok(Some(principal)) => principal,
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let Ok(permit) = state.sockets.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    upgrade
        .max_message_size(wire::MAX_MESSAGE_BYTES)
        .max_frame_size(wire::MAX_MESSAGE_BYTES)
        .read_buffer_size(4096)
        .write_buffer_size(0)
        .max_write_buffer_size(wire::MAX_MESSAGE_BYTES + 4096)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            connection(socket, principal, state).await;
        })
}

async fn shared_websocket(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(link): Path<String>,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !auth::valid_origin(&headers, &state.origins) || !rate_allowed(&state).await {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(link) = Uuid::parse_str(&link) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if link.get_version_num() != 7 {
        return StatusCode::NOT_FOUND.into_response();
    }
    let principal = match state.auth.authenticate_share(&headers, link).await {
        Ok(Some(principal)) => principal,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let Ok(permit) = state.sockets.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    upgrade
        .max_message_size(wire::MAX_MESSAGE_BYTES)
        .max_frame_size(wire::MAX_MESSAGE_BYTES)
        .read_buffer_size(4096)
        .write_buffer_size(0)
        .max_write_buffer_size(wire::MAX_MESSAGE_BYTES + 4096)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            connection(socket, principal, state).await;
        })
}

async fn connection(mut socket: WebSocket, principal: auth::Principal, state: Arc<AppState>) {
    let anonymized_guest =
        if let auth::PrincipalScope::ShareGuest { session, .. } = &principal.scope {
            match state
                .database
                .requires_anonymization(
                    principal.identity,
                    session.document_id,
                    Some(session.link_id),
                )
                .await
            {
                Ok(value) => value,
                Err(_) => return,
            }
        } else {
            false
        };
    let mut access_events = state.access_events.subscribe();
    // Watch receivers do not retain the sender; keep the weak-directory gate socket-owned.
    let guest_delivery_gate =
        if let auth::PrincipalScope::ShareGuest { session, .. } = &principal.scope {
            Some(state.delivery.get(session.document_id).await)
        } else {
            None
        };
    let mut guest_access_changes = guest_delivery_gate.as_ref().map(|gate| gate.watch());
    if !matches!(state.auth.active(&principal).await, Ok(true)) {
        return;
    }
    let first = tokio::time::timeout(Duration::from_secs(5), socket.recv()).await;
    let result = match first {
        Ok(Some(Ok(Message::Binary(bytes)))) => wire::decode_hello(&bytes),
        _ => Err(wire::DecodeError::InvalidMessage),
    };
    if let Err(error) = result {
        let _ = send_direct(
            &mut socket,
            Message::Binary(wire::error_message(error).into()),
        )
        .await;
        let _ = send_direct(
            &mut socket,
            Message::Close(Some(CloseFrame {
                code: 1002,
                reason: "invalid handshake".into(),
            })),
        )
        .await;
        return;
    }
    let session_id = Uuid::now_v7();
    let welcome = v1::ServerEnvelope {
        protocol_version: wire::PROTOCOL_VERSION,
        payload: Some(v1::server_envelope::Payload::Welcome(v1::ServerWelcome {
            session_id: session_id.as_bytes().to_vec(),
            actor_id: principal.identity.actor_id.as_bytes().to_vec(),
            heartbeat_interval_ms: 15_000,
            max_message_bytes: wire::MAX_MESSAGE_BYTES as u32,
        })),
    }
    .encode_to_vec();
    if !send_direct(&mut socket, Message::Binary(welcome.into())).await {
        return;
    }
    let (sink, mut source) = socket.split();
    let (outbound, mut outbound_closed, mut writer) =
        outbound::start(sink, state.clone(), principal.clone());
    tracing::info!(event = "session_connected", %session_id);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_pong = Instant::now();
    let mut calculation = None;
    let mut head_subscription: Option<HeadSubscription> = None;
    let mut last_presence = Instant::now() - Duration::from_millis(50);
    let expiry = tokio::time::Instant::from_std(principal.expires_at);
    let expiry_timer = tokio::time::sleep_until(expiry);
    tokio::pin!(expiry_timer);
    let mut guest_access_closed = false;
    loop {
        tokio::select! {
            _ = &mut expiry_timer => {
                guest_access_closed = matches!(&principal.scope, auth::PrincipalScope::ShareGuest { .. });
                break;
            },
            event = access_events.recv() => match event {
                Ok(AccessEpochEvent::Document { document, .. }) => {
                    let guest_matches = matches!(
                        &principal.scope,
                        auth::PrincipalScope::ShareGuest { session, .. }
                            if session.document_id == document
                    );
                    let subscription_matches = head_subscription
                        .as_ref()
                        .is_some_and(|active| active.document == document);
                    if guest_matches || subscription_matches {
                        guest_access_closed |= guest_matches;
                        break;
                    }
                }
                Ok(AccessEpochEvent::Reconnect) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    guest_access_closed |= matches!(&principal.scope, auth::PrincipalScope::ShareGuest { .. });
                    if guest_access_closed || head_subscription.is_some() { break; }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            changed = async { match guest_access_changes.as_mut() { Some(receiver)=>receiver.changed().await, None=>std::future::pending().await } } => {
                if guest_access_changes.is_some() || changed.is_err() {
                    guest_access_closed = true;
                    break;
                }
            }
            changed = outbound_closed.changed() => {
                if changed.is_err() || *outbound_closed.borrow() { break; }
            }
            event = next_subscription_event(&mut head_subscription) => {
                let Some(event) = event else { continue; };
                let update = match event {
                    SubscriptionEvent::Head(update) => update,
                    SubscriptionEvent::Presence(peers) => {
                        if matches!(&principal.scope, auth::PrincipalScope::ShareGuest { .. }) { continue; }
                        let Some(active) = head_subscription.as_ref() else { continue; };
                        let payload = v1::ServerEnvelope {
                            protocol_version: wire::PROTOCOL_VERSION,
                            payload: Some(v1::server_envelope::Payload::PresenceSnapshot(
                                v1::PresenceSnapshot {
                                    document_id: active.document.as_bytes().to_vec(),
                                    peers,
                                },
                            )),
                        }.encode_to_vec();
                        if !send_document_payload(
                            &outbound,
                            &DocumentDelivery { document: active.document, gate: active.gate.clone(), ticket: active.ticket },
                            payload,
                        ) { break; }
                        continue;
                    }
                    SubscriptionEvent::AccessChanged => break,
                    SubscriptionEvent::Closed => break,
                };
                let Some(active) = head_subscription.as_mut() else { continue; };
                if update.version <= active.last_sent { continue; }
                let current = match active.session.head(principal.identity).await {
                    Ok(current) => current,
                    Err(_) => break,
                };
                if !matches!(state.auth.active(&principal).await, Ok(true)) { break; }
                if current > active.last_sent {
                    let delivery = DocumentDelivery {
                        document: active.document,
                        gate: active.gate.clone(),
                        ticket: active.ticket,
                    };
                    if matches!(&principal.scope, auth::PrincipalScope::ShareGuest { .. }) {
                        if !send_document_payload(&outbound, &delivery, guest_head_hint(active.document,current)) { break; }
                    } else {
                        let replay = send_replay(
                            &outbound, &state, &principal, &active.session, &delivery,
                            active.last_sent, current,
                        ).await;
                        if replay.is_err() { break; }
                    }
                    active.last_sent = current;
                }
            }
            frame = calculation_stream::next(&mut calculation) => {
                let Some(frame) = frame else { calculation = None; continue; };
                // Queueing is not authorization: check the current head/access again at send.
                let allowed = matches!(frame.session.head(principal.identity).await,
                    Ok(current) if current == frame.version);
                if !allowed || !send_document_payload(
                    &outbound,
                    &DocumentDelivery { document: frame.document, gate: frame.gate, ticket: frame.ticket },
                    frame.bytes,
                ) { break; }
            }
            _ = heartbeat.tick() => {
                if last_pong.elapsed() >= Duration::from_secs(45) { break; }
                match state.auth.active(&principal).await {
                    Ok(true) => {}
                    Ok(false) => {
                        guest_access_closed |= matches!(&principal.scope, auth::PrincipalScope::ShareGuest { .. });
                        break;
                    }
                    Err(_) => break,
                }
                if !outbound.enqueue(Message::Ping(Vec::new().into()), None) { break; }
                if !anonymized_guest && let Some(active) = &head_subscription
                    && active.session.request_timer(principal.identity).is_err() {
                    tracing::debug!(event = "calculation_timer_admission_skipped");
                }
            }
            message = source.next() => match message {
                Some(Ok(Message::Pong(_))) => last_pong = Instant::now(),
                Some(Ok(Message::Ping(bytes))) => { if !outbound.enqueue(Message::Pong(bytes), None) { break; } },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(Message::Binary(bytes))) => {
                    match state.auth.active(&principal).await {
                        Ok(true) => {}
                        Ok(false) => {
                            guest_access_closed |= matches!(&principal.scope, auth::PrincipalScope::ShareGuest { .. });
                            break;
                        }
                        Err(_) => break,
                    }
                    match wire::decode_request(&bytes) {
                        Ok(wire::ClientRequest::Command(command)) => {
                            let document = command.document_id.as_slice().try_into().ok()
                                .and_then(|bytes| DocumentId::from_bytes(bytes).ok());
                            let authorization = if let Some(document) = document {
                                state.database.access(principal.identity, document).await.ok()
                            } else { None };
                            let result = documents::handle_command(&state, &principal, *command).await;
                            let safe_without_access = matches!(
                                result.payload.as_ref(),
                                Some(v1::server_envelope::Payload::Rejected(_)
                                    | v1::server_envelope::Payload::Error(_))
                            );
                            let payload = result.encode_to_vec();
                            let delivered = if let (Some(document), Some(ticket)) = (document, authorization) {
                                let gate = state.delivery.get(document).await;
                                send_document_payload(
                                    &outbound,
                                    &DocumentDelivery { document, gate, ticket }, payload,
                                )
                            } else if safe_without_access {
                                outbound.enqueue(Message::Binary(payload.into()), None)
                            } else {
                                false
                            };
                            if !delivered { break; }
                        }
                        Ok(wire::ClientRequest::Calculation(request)) => {
                            if anonymized_guest { continue; }
                            if !rate_allowed(&state).await { break; }
                            drop(calculation.take());
                            match calculation_stream::start(state.clone(), principal.clone(), request).await {
                                Ok(stream) => calculation = Some(stream),
                                Err(()) => {
                                    let error = v1::ServerEnvelope { protocol_version: wire::PROTOCOL_VERSION,
                                        payload: Some(v1::server_envelope::Payload::Error(v1::ProtocolError {
                                            code: v1::ErrorCode::AuthRequired.into(), supported_protocol_version: wire::PROTOCOL_VERSION,
                                        })) }.encode_to_vec();
                                    let _ = outbound.enqueue(Message::Binary(error.into()), None);
                                    break;
                                }
                            }
                        }
                        Ok(wire::ClientRequest::Presence(request)) => {
                            if matches!(&principal.scope, auth::PrincipalScope::ShareGuest { .. }) { continue; }
                            let Some(active) = head_subscription.as_ref() else { break; };
                            if request.document_id.as_slice() != active.document.as_bytes()
                                || !matches!(state.auth.active(&principal).await, Ok(true)) { break; }
                            if last_presence.elapsed() < Duration::from_millis(50) { continue; }
                            let _read = active.gate.lock.read().await;
                            let authorized = matches!(state.database.access(principal.identity, active.document).await,
                                Ok(current) if current.document_access_epoch == active.ticket.document_access_epoch
                                && current.membership_access_epoch == active.ticket.membership_access_epoch);
                            if !authorized { break; }
                            if state.presence.update(active.document, session_id, request.anchor, request.focus).await.is_err() { break; }
                            last_presence = Instant::now();
                        }
                        Ok(wire::ClientRequest::Subscribe(request)) => {
                            if !rate_allowed(&state).await { break; }
                            let document = match request.document_id.as_slice().try_into()
                                .ok().and_then(|bytes| DocumentId::from_bytes(bytes).ok()) {
                                Some(document) => document,
                                None => break,
                            };
                            if let auth::PrincipalScope::ShareGuest { session: guest, .. } = &principal.scope
                                && guest.document_id != document { break; }
                            let session = match state.sessions.session(principal.identity, document).await {
                                Ok(session) => session,
                                Err(_) => break,
                            };
                            let after = match DocumentVersion::new(request.last_seen_version) {
                                Ok(version) => version,
                                Err(_) => break,
                            };
                            let authorized_head = match session.head(principal.identity).await {
                                Ok(head) => head,
                                Err(_) => break,
                            };
                            if after > authorized_head { break; }
                            if !matches!(&principal.scope, auth::PrincipalScope::ShareGuest { .. })
                                && head_subscription.as_ref().is_none_or(|active| active.document != document)
                                && session.opened(principal.identity).await.is_err() {
                                break;
                            }
                            let updates = match session.watch_head(principal.identity).await {
                                Ok(updates) => updates,
                                Err(_) => break,
                            };
                            let current = match session.head(principal.identity).await {
                                Ok(current) => current,
                                Err(_) => break,
                            };
                            let gate = state.delivery.get(document).await;
                            let ticket = match state.database.access(principal.identity, document).await {
                                Ok(ticket) => ticket,
                                Err(_) => break,
                            };
                            let access_changes = gate.watch();
                            if request.last_seen_version > current.get() { break; }
                            let resync_required = if matches!(&principal.scope, auth::PrincipalScope::ShareGuest { .. }) {
                                true
                            } else {
                                match send_replay(
                                    &outbound, &state, &principal, &session,
                                    &DocumentDelivery { document, gate: gate.clone(), ticket },
                                    after, current,
                                ).await {
                                    Ok(value) => value,
                                    Err(()) => break,
                                }
                            };
                            if !send_replay_complete(
                                &outbound, &state, &principal, &session,
                                &DocumentDelivery { document, gate: gate.clone(), ticket },
                                current, resync_required,
                            ).await { break; }
                            if let Some(previous) = head_subscription.take() {
                                state.presence.leave(previous.document, session_id).await;
                            }
                            let peers = match state.presence.join(document, session_id, principal.identity.actor_id).await {
                                Ok(peers) => peers,
                                Err(_) => break,
                            };
                            head_subscription = Some(HeadSubscription {
                                document,
                                session,
                                gate,
                                ticket,
                                access_changes,
                                updates,
                                peers,
                                last_sent: current,
                            });
                        }
                        Err(error) => {
                            let _ = outbound.enqueue(Message::Binary(wire::error_message(error).into()), None);
                            break;
                        }
                    }
                }
                _ => {
                    let _ = outbound.enqueue(Message::Binary(wire::error_message(wire::DecodeError::InvalidMessage).into()), None);
                    break;
                }
            }
        }
    }
    if let Some(active) = head_subscription.take() {
        state.presence.leave(active.document, session_id).await;
    }
    let close = if guest_access_closed {
        Message::Close(Some(CloseFrame {
            code: 4003,
            reason: "Access closed".into(),
        }))
    } else {
        Message::Close(None)
    };
    let _ = outbound.enqueue(close, None);
    drop(outbound);
    if tokio::time::timeout(Duration::from_secs(2), &mut writer)
        .await
        .is_err()
    {
        writer.abort();
        let _ = writer.await;
    }
    tracing::info!(event = "session_disconnected", %session_id);
}

async fn send_direct(socket: &mut WebSocket, message: Message) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(2), socket.send(message)).await,
        Ok(Ok(()))
    )
}

async fn security_headers(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"));
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    response
}

async fn shutdown() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    if result.is_err() { tracing::error!(event = "shutdown_signal_failed"); }
                }
                _ = terminate.recv() => {}
            },
            Err(_) => tracing::error!(event = "shutdown_signal_failed"),
        }
    }
    #[cfg(not(unix))]
    if tokio::signal::ctrl_c().await.is_err() {
        tracing::error!(event = "shutdown_signal_failed");
    }
}
