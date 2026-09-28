mod auth;
mod calculation_stream;
mod config;
mod documents;

use axum::{
    Json, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{CloseFrame, Message, WebSocket},
    },
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use prost::Message as ProstMessage;
use serde::Serialize;
use spreadsheet_collab::{DocumentSession, SessionDirectory};
use spreadsheet_domain::workbook::{DocumentId, DocumentVersion};
use spreadsheet_storage::{LocalFilesystemBlobStore, PgDatabase};
use spreadsheet_wire_protocol::{self as wire, v1};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore, watch};
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;

struct AppState {
    database: PgDatabase,
    blobs: LocalFilesystemBlobStore,
    auth: auth::Auth,
    origins: Vec<String>,
    sockets: Arc<Semaphore>,
    calculation_encoders: Arc<Semaphore>,
    rate: Mutex<(Instant, u32)>,
    sessions: spreadsheet_collab::InProcessSessionDirectory<PgDatabase>,
}

#[derive(Serialize)]
struct Status {
    status: &'static str,
}

struct HeadSubscription {
    document: DocumentId,
    session: DocumentSession,
    updates: watch::Receiver<DocumentVersion>,
    last_sent: DocumentVersion,
}

async fn next_head(subscription: &mut Option<HeadSubscription>) -> Option<DocumentVersion> {
    let Some(active) = subscription.as_mut() else {
        return std::future::pending().await;
    };
    if active.updates.changed().await.is_err() {
        *subscription = None;
        return None;
    }
    Some(*active.updates.borrow_and_update())
}

fn head_hint(document: DocumentId, version: DocumentVersion) -> Vec<u8> {
    v1::ServerEnvelope {
        protocol_version: wire::PROTOCOL_VERSION,
        payload: Some(v1::server_envelope::Payload::DocumentHeadChanged(
            v1::DocumentHeadChanged {
                document_id: document.as_bytes().to_vec(),
                head_version: version.get(),
            },
        )),
    }
    .encode_to_vec()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
    let blobs = LocalFilesystemBlobStore::new(&config.blob_root, 64 * 1024 * 1024).await?;
    let state = Arc::new(AppState {
        auth: auth::Auth::new(config.dev_auth, database.clone()),
        sessions: spreadsheet_collab::InProcessSessionDirectory::new(database.clone()),
        database,
        blobs,
        origins: config.allowed_origins,
        sockets: Arc::new(Semaphore::new(128)),
        calculation_encoders: Arc::new(Semaphore::new(2)),
        rate: Mutex::new((Instant::now(), 0)),
    });
    let app = Router::new()
        .route(
            "/healthz",
            get(|| async { Json(Status { status: "alive" }) }),
        )
        .route("/readyz", get(ready))
        .route("/api/ws", get(websocket))
        .route(
            "/api/documents/{id}/calculation",
            get(documents::get_calculation),
        )
        .route(
            "/api/dev/session",
            post(dev_session).delete(revoke_dev_session),
        )
        .route("/api/documents", post(documents::create))
        .route("/api/documents/{id}", get(documents::get_document))
        .route(
            "/api/documents/{id}/recalculate",
            post(documents::recalculate),
        )
        .fallback_service(
            ServeDir::new("apps/web/dist")
                .not_found_service(ServeFile::new("apps/web/dist/index.html")),
        )
        .layer(tower_http::limit::RequestBodyLimitLayer::new(1024 * 1024))
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

async fn connection(mut socket: WebSocket, principal: auth::Principal, state: Arc<AppState>) {
    let first = tokio::time::timeout(Duration::from_secs(5), socket.recv()).await;
    let result = match first {
        Ok(Some(Ok(Message::Binary(bytes)))) => wire::decode_hello(&bytes),
        _ => Err(wire::DecodeError::InvalidMessage),
    };
    if let Err(error) = result {
        let _ = send(
            &mut socket,
            Message::Binary(wire::error_message(error).into()),
        )
        .await;
        let _ = send(
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
    if !send(&mut socket, Message::Binary(welcome.into())).await {
        return;
    }
    tracing::info!(event = "session_connected", %session_id);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_pong = Instant::now();
    let mut calculation = None;
    let mut head_subscription: Option<HeadSubscription> = None;
    loop {
        tokio::select! {
            version = next_head(&mut head_subscription) => {
                let Some(version) = version else { continue; };
                let Some(active) = head_subscription.as_mut() else { continue; };
                if version <= active.last_sent { continue; }
                let current = match active.session.head(principal.identity).await {
                    Ok(current) => current,
                    Err(_) => break,
                };
                if !matches!(state.auth.active(&principal).await, Ok(true)) { break; }
                if current > active.last_sent {
                    if !send(&mut socket, Message::Binary(head_hint(active.document, current).into())).await { break; }
                    active.last_sent = current;
                }
            }
            frame = calculation_stream::next(&mut calculation) => {
                let Some(frame) = frame else { calculation = None; continue; };
                // Queueing is not authorization: check the current head/access again at send.
                let allowed = matches!(frame.session.head(principal.identity).await,
                    Ok(current) if current == frame.version);
                if !allowed || !matches!(state.auth.active(&principal).await, Ok(true)) { break; }
                if !send(&mut socket, Message::Binary(frame.bytes.into())).await { break; }
            }
            _ = heartbeat.tick() => {
                if last_pong.elapsed() >= Duration::from_secs(45)
                    || !matches!(state.auth.active(&principal).await, Ok(true)) { break; }
                if !send(&mut socket, Message::Ping(Vec::new().into())).await { break; }
            }
            message = socket.recv() => match message {
                Some(Ok(Message::Pong(_))) => last_pong = Instant::now(),
                Some(Ok(Message::Ping(bytes))) => { if !send(&mut socket, Message::Pong(bytes)).await { break; } },
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(Message::Binary(bytes))) => {
                    if !matches!(state.auth.active(&principal).await, Ok(true)) { break; }
                    match wire::decode_request(&bytes) {
                        Ok(wire::ClientRequest::Command(command)) => {
                            let result = documents::handle_command(&state, &principal, *command).await;
                            if !send(&mut socket, Message::Binary(result.encode_to_vec().into())).await { break; }
                        }
                        Ok(wire::ClientRequest::Calculation(request)) => {
                            if !rate_allowed(&state).await { break; }
                            drop(calculation.take());
                            match calculation_stream::start(state.clone(), principal.clone(), request).await {
                                Ok(stream) => calculation = Some(stream),
                                Err(()) => {
                                    let error = v1::ServerEnvelope { protocol_version: wire::PROTOCOL_VERSION,
                                        payload: Some(v1::server_envelope::Payload::Error(v1::ProtocolError {
                                            code: v1::ErrorCode::AuthRequired.into(), supported_protocol_version: wire::PROTOCOL_VERSION,
                                        })) }.encode_to_vec();
                                    let _ = send(&mut socket, Message::Binary(error.into())).await;
                                    break;
                                }
                            }
                        }
                        Ok(wire::ClientRequest::Subscribe(request)) => {
                            if !rate_allowed(&state).await { break; }
                            let document = match request.document_id.as_slice().try_into()
                                .ok().and_then(|bytes| DocumentId::from_bytes(bytes).ok()) {
                                Some(document) => document,
                                None => break,
                            };
                            let session = match state.sessions.session(principal.identity, document).await {
                                Ok(session) => session,
                                Err(_) => break,
                            };
                            let updates = match session.watch_head(principal.identity).await {
                                Ok(updates) => updates,
                                Err(_) => break,
                            };
                            let current = match session.head(principal.identity).await {
                                Ok(current) => current,
                                Err(_) => break,
                            };
                            if request.last_seen_version > current.get() { break; }
                            if current.get() > request.last_seen_version
                                && !send(&mut socket, Message::Binary(head_hint(document, current).into())).await { break; }
                            head_subscription = Some(HeadSubscription {
                                document,
                                session,
                                updates,
                                last_sent: current,
                            });
                        }
                        Err(error) => {
                            let _ = send(&mut socket, Message::Binary(wire::error_message(error).into())).await;
                            break;
                        }
                    }
                }
                _ => {
                    let _ = send(&mut socket, Message::Binary(wire::error_message(wire::DecodeError::InvalidMessage).into())).await;
                    break;
                }
            }
        }
    }
    let _ = send(&mut socket, Message::Close(None)).await;
    tracing::info!(event = "session_disconnected", %session_id);
}

async fn send(socket: &mut WebSocket, message: Message) -> bool {
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
