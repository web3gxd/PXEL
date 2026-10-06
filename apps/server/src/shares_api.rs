use crate::{
    AppState,
    auth::{self, PrincipalScope},
    share_password::PasswordError,
};
use axum::{
    Json,
    body::Body,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spreadsheet_application::journal::{AuthenticatedActor, OperationJournal};
use spreadsheet_collab::SessionDirectory;
use spreadsheet_domain::workbook::DocumentId;
use spreadsheet_storage::{
    CommentAddress, CommentError, GuestSessionSeed, ShareAttemptOutcome, ShareError, ShareLifetime,
    ShareOwnerNotification, ShareRole, ShareSecretKind, ShareSecretPhc,
};
use std::sync::Arc;
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    #[serde(default)]
    anonymized: bool,
    request_id: String,
    permission: String,
    ttl: String,
    confirm_indefinite: bool,
    pin: Option<String>,
    password: Option<String>,
}
impl Drop for CreateBody {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        if let Some(value) = &mut self.pin {
            value.zeroize();
        }
        if let Some(value) = &mut self.password {
            value.zeroize();
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnlockBody {
    token: String,
    secret: String,
}
impl Drop for UnlockBody {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.token.zeroize();
        self.secret.zeroize();
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MutationBody {
    request_id: String,
}

#[derive(Serialize)]
struct ShareView {
    anonymized_at_creation: bool,
    link_id: Uuid,
    document_id: Uuid,
    permission: ShareRole,
    secret_kind: ShareSecretKind,
    status: spreadsheet_storage::ShareStatus,
    created_at: i64,
    expires_at: Option<i64>,
}
fn share_view(link: spreadsheet_storage::ShareLinkSummary) -> ShareView {
    ShareView {
        anonymized_at_creation: link.anonymized_at_creation,
        link_id: link.link_id,
        document_id: Uuid::from_bytes(*link.document_id.as_bytes()),
        permission: link.role,
        secret_kind: link.secret_kind,
        status: link.status,
        created_at: link.created_at_unix,
        expires_at: link.expires_at_unix,
    }
}
#[derive(Serialize)]
pub(super) struct SessionView {
    document_id: Uuid,
    actor_id: Uuid,
    permission: ShareRole,
    expires_at: i64,
    admitted_head_version: String,
}
fn session_view(session: &spreadsheet_storage::GuestSession) -> SessionView {
    SessionView {
        document_id: Uuid::from_bytes(*session.document_id.as_bytes()),
        actor_id: Uuid::from_bytes(*session.identity.actor_id.as_bytes()),
        permission: session.role,
        expires_at: chrono_seconds(session.remaining_seconds),
        admitted_head_version: session.admitted_head_version.get().to_string(),
    }
}
fn chrono_seconds(remaining: u64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| value.as_secs());
    now.saturating_add(remaining).min(i64::MAX as u64) as i64
}
#[derive(Serialize)]
pub(super) struct ShareList {
    shares: Vec<ShareView>,
}
#[derive(Serialize)]
pub(super) struct Created {
    share: ShareView,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}
impl Drop for Created {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        if let Some(token) = &mut self.token {
            token.zeroize();
        }
        if let Some(url) = &mut self.url {
            url.zeroize();
        }
    }
}
#[derive(Serialize)]
pub(super) struct Notifications {
    notifications: Vec<NotificationView>,
}
#[derive(Serialize)]
struct NotificationView {
    notification_id: Uuid,
    link_id: Uuid,
    document_id: Uuid,
    kind: &'static str,
    created_at: i64,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(document): Path<String>,
) -> Result<Json<ShareList>, ApiError> {
    let principal = member(&state, &headers).await?;
    let doc = document_id(&document)?;
    let shares = state
        .database
        .list_shares(principal.identity, doc, 100)
        .await
        .map_err(share_error)?;
    Ok(Json(ShareList {
        shares: shares.into_iter().map(share_view).collect(),
    }))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(document): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<Created>), ApiError> {
    let _admission = state
        .share_admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "share_busy"))?;
    let principal = member(&state, &headers).await?;
    let doc = document_id(&document)?;
    require_origin(&state, &headers)?;
    let mut input: CreateBody =
        parse_zeroized(body).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let request = v7(&input.request_id)?;
    let role = match input.permission.as_str() {
        "view" => ShareRole::View,
        "comment" => ShareRole::Comment,
        "edit" => ShareRole::Edit,
        _ => return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_request")),
    };
    let lifetime = match input.ttl.as_str() {
        "hour" => ShareLifetime::Hour,
        "day" => ShareLifetime::Day,
        "week" => ShareLifetime::Week,
        "indefinite" => ShareLifetime::Indefinite {
            confirmed: input.confirm_indefinite,
        },
        _ => return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_request")),
    };
    let pin = input.pin.take().map(Zeroizing::new);
    let password = input.password.take().map(Zeroizing::new);
    let (kind, secret) = match (pin, password) {
        (Some(pin), None)
            if (6..=64).contains(&pin.len()) && pin.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            (ShareSecretKind::Pin, pin)
        }
        (None, Some(password))
            if (10..=128).contains(&password.chars().count()) && password.len() <= 512 =>
        {
            (ShareSecretKind::Password, password)
        }
        _ => return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_request")),
    };
    let permit = state.passwords.admit().map_err(password_error)?;
    let mut intent = Sha256::new();
    intent.update(b"PXEL share secret intent v1\0");
    intent.update(kind.as_str().as_bytes());
    intent.update([0]);
    intent.update(secret.as_bytes());
    let intent: [u8; 32] = intent.finalize().into();
    let phc = state
        .passwords
        .hash(secret, permit)
        .await
        .map_err(password_error)?;
    let phc = ShareSecretPhc::new(kind, phc).map_err(share_error)?;
    let receipt = state
        .database
        .create_share_presented(
            principal.identity,
            doc,
            request,
            role,
            lifetime,
            phc,
            intent,
            input.anonymized,
        )
        .await
        .map_err(share_error)?;
    let share = share_view(receipt.link);
    let (token, url) = if let Some(secret) = receipt.token {
        let token = hex(secret.as_slice());
        let url = format!("/share/{}#{}", share.link_id, token);
        (Some(token), Some(url))
    } else {
        (None, None)
    };
    Ok((StatusCode::CREATED, Json(Created { share, token, url })))
}

pub async fn revoke(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((document, link)): Path<(String, String)>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    require_origin(&state, &headers)?;
    let principal = member(&state, &headers).await?;
    let doc = document_id(&document)?;
    let link = v7(&link)?;
    let input: MutationBody = serde_json::from_slice(&body)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let request = v7(&input.request_id)?;
    let gate = state.delivery.get(doc).await;
    let _write = tokio::time::timeout(std::time::Duration::from_secs(3), gate.lock.write())
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "send_gate_busy"))?;
    let receipt = state
        .database
        .revoke_share(principal.identity, doc, link, request)
        .await
        .map_err(share_error)?;
    if receipt.newly_committed {
        gate.committed_epoch(receipt.access_epoch);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn notifications(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Notifications>, ApiError> {
    let principal = member(&state, &headers).await?;
    let values: Vec<ShareOwnerNotification> = state
        .database
        .owner_notifications(principal.identity, 100)
        .await
        .map_err(share_error)?;
    Ok(Json(Notifications {
        notifications: values
            .into_iter()
            .map(|value| NotificationView {
                notification_id: value.notification_id,
                link_id: value.link_id,
                document_id: Uuid::from_bytes(*value.document_id.as_bytes()),
                kind: "share_disabled",
                created_at: value.created_at_unix,
            })
            .collect(),
    }))
}

pub async fn session_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(link): Path<String>,
) -> Result<Json<SessionView>, ApiError> {
    let link = v7(&link)?;
    let principal = state
        .auth
        .authenticate_share(&headers, link)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "share_unavailable"))?;
    let PrincipalScope::ShareGuest { session, .. } = principal.scope else {
        return Err(ApiError(StatusCode::NOT_FOUND, "share_unavailable"));
    };
    Ok(Json(session_view(&session)))
}

pub async fn session_post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(link): Path<String>,
    body: Bytes,
) -> Response {
    let result: Result<(HeaderMap, Json<SessionView>), Box<Response>> = async {
        require_origin(&state, &headers).map_err(|error| Box::new(error.into_response()))?;
        let _admission = state
            .share_admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                Box::new(
                    ApiError(StatusCode::TOO_MANY_REQUESTS, "share_unavailable").into_response(),
                )
            })?;
        let link = v7(&link).map_err(|_| {
            Box::new(ApiError(StatusCode::NOT_FOUND, "share_unavailable").into_response())
        })?;
        let mut input: UnlockBody = parse_zeroized(body).map_err(|_| {
            Box::new(ApiError(StatusCode::NOT_FOUND, "share_unavailable").into_response())
        })?;
        let token_valid = decode_hex32(&input.token).is_some();
        let mut token = Zeroizing::new(decode_hex32(&input.token).unwrap_or_else(|| {
            let mut random = [0; 32];
            let _ = getrandom::fill(&mut random);
            random
        }));
        let token_hash: [u8; 32] = Sha256::digest(token.as_slice()).into();
        use zeroize::Zeroize;
        token.zeroize();
        let permit = state.passwords.admit().map_err(|_| {
            Box::new(ApiError(StatusCode::TOO_MANY_REQUESTS, "share_unavailable").into_response())
        })?;
        let attempt = state
            .database
            .begin_share_attempt(link, token_hash)
            .await
            .map_err(|_| {
                Box::new(ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response())
            })?;
        let document = attempt.document_id();
        let phc = attempt.phc().map(|value| Zeroizing::new(value.to_owned()));
        let secret = Zeroizing::new(std::mem::take(&mut input.secret));
        let verified = state
            .passwords
            .verify(secret, phc, permit)
            .await
            .map_err(|error| Box::new(password_error(error).into_response()))?
            && token_valid;
        let mut guest_secret = Zeroizing::new([0_u8; 32]);
        getrandom::fill(guest_secret.as_mut()).map_err(|_| {
            Box::new(ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response())
        })?;
        let guest_token = Zeroizing::new(hex(guest_secret.as_slice()));
        let seed = GuestSessionSeed {
            session_id: Uuid::now_v7(),
            actor_id: spreadsheet_domain::workbook::ActorId::from_bytes(*Uuid::now_v7().as_bytes())
                .map_err(|_| {
                    ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response()
                })?,
            token_hash: Sha256::digest(guest_token.as_bytes()).into(),
        };
        let outcome = if let Some(document) = document {
            let gate = state.delivery.get(document).await;
            let _write = tokio::time::timeout(std::time::Duration::from_secs(3), gate.lock.write())
                .await
                .map_err(|_| {
                    Box::new(
                        ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response(),
                    )
                })?;
            let outcome = attempt
                .complete(verified, verified.then_some(seed))
                .await
                .map_err(|error| Box::new(share_error(error).into_response()))?;
            match outcome {
                ShareAttemptOutcome::Invalidated(receipt) => {
                    gate.committed_epoch(receipt.access_epoch);
                    return Err(Box::new(
                        ApiError(StatusCode::NOT_FOUND, "share_unavailable").into_response(),
                    ));
                }
                other => other,
            }
        } else {
            attempt
                .complete(verified, None)
                .await
                .map_err(|error| Box::new(share_error(error).into_response()))?
        };
        match outcome {
            ShareAttemptOutcome::Accepted(session) => {
                let mut response_headers = HeaderMap::new();
                let cookie = Zeroizing::new(state.auth.share_cookie(
                    &guest_token,
                    link,
                    session.remaining_seconds,
                ));
                response_headers.insert(
                    axum::http::header::SET_COOKIE,
                    axum::http::HeaderValue::from_str(&cookie).map_err(|_| {
                        Box::new(
                            ApiError(StatusCode::INTERNAL_SERVER_ERROR, "unavailable")
                                .into_response(),
                        )
                    })?,
                );
                Ok((response_headers, Json(session_view(&session))))
            }
            ShareAttemptOutcome::Blocked { remaining_seconds } => {
                Err(Box::new(locked_response(remaining_seconds)))
            }
            ShareAttemptOutcome::Invalid | ShareAttemptOutcome::Invalidated(_) => Err(Box::new(
                ApiError(StatusCode::NOT_FOUND, "share_unavailable").into_response(),
            )),
        }
    }
    .await;
    match result {
        Ok(value) => value.into_response(),
        Err(response) => *response,
    }
}

#[derive(Serialize)]
struct CommentView {
    comment_id: Uuid,
    sheet_id: Uuid,
    row_id: Uuid,
    column_id: Uuid,
    author_id: Uuid,
    text: String,
    can_delete: bool,
    #[serde(skip)]
    revision: u64,
}
#[derive(Serialize)]
pub(super) struct CommentsView {
    revision: String,
    comments: Vec<CommentView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddComment {
    request_id: String,
    sheet_id: String,
    row_id: String,
    column_id: String,
    text: String,
}
impl Drop for AddComment {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.text.zeroize();
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteComment {
    request_id: String,
}
#[derive(Deserialize)]
pub(super) struct CommentQuery {
    sheet_id: String,
    row_id: String,
    column_id: String,
    cursor: Option<String>,
}

pub async fn list_comments(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(document): Path<String>,
    Query(query): Query<CommentQuery>,
) -> Result<Json<CommentsView>, ApiError> {
    let principal = any_principal(&state, &headers, None).await?;
    let doc = document_id(&document)?;
    let address = address(&query.sheet_id, &query.row_id, &query.column_id)?;
    let cursor = query.cursor.as_deref().map(parse_decimal).transpose()?;
    validate_address(&state, principal.identity, doc, address).await?;
    comments_page(&state, principal.identity, doc, address, cursor).await
}
pub async fn add_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(document): Path<String>,
    body: Bytes,
) -> Result<Json<CommentsView>, ApiError> {
    require_origin(&state, &headers)?;
    let principal = any_principal(&state, &headers, None).await?;
    let doc = document_id(&document)?;
    let mut input: AddComment =
        parse_zeroized(body).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let request = v7(&input.request_id)?;
    let address = address(&input.sheet_id, &input.row_id, &input.column_id)?;
    validate_address(&state, principal.identity, doc, address).await?;
    state
        .database
        .add_comment(
            principal.identity,
            doc,
            request,
            address,
            Zeroizing::new(std::mem::take(&mut input.text)),
        )
        .await
        .map_err(comment_error)?;
    comments_page(&state, principal.identity, doc, address, None).await
}
pub async fn delete_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((document, comment)): Path<(String, String)>,
    body: Bytes,
) -> Result<Json<CommentsView>, ApiError> {
    require_origin(&state, &headers)?;
    let principal = any_principal(&state, &headers, None).await?;
    let doc = document_id(&document)?;
    let comment = v7(&comment)?;
    let input: DeleteComment = serde_json::from_slice(&body)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let request = v7(&input.request_id)?;
    let address = state
        .database
        .delete_comment(principal.identity, doc, request, comment)
        .await
        .map_err(comment_error)?;
    comments_page(&state, principal.identity, doc, address, None).await
}

pub async fn list_guest_comments(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(link): Path<String>,
    Query(query): Query<CommentQuery>,
) -> Result<Response, ApiError> {
    let link = v7(&link)?;
    let principal = guest(&state, &headers, link).await?;
    let session = guest_session(&principal)?;
    let gate = state.delivery.get(session.document_id).await;
    let guard = gate.lock.clone().read_owned().await;
    let address = address(&query.sheet_id, &query.row_id, &query.column_id)?;
    let cursor = query.cursor.as_deref().map(parse_decimal).transpose()?;
    validate_address(&state, principal.identity, session.document_id, address).await?;
    let mut page = comments_page(
        &state,
        principal.identity,
        session.document_id,
        address,
        cursor,
    )
    .await?;
    mask_guest_comments(&state, &principal, session.document_id, link, &mut page.0).await?;
    response_with_guard(&state, &principal, session.document_id, page.0, guard).await
}
pub async fn add_guest_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(link): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    require_origin(&state, &headers)?;
    let link = v7(&link)?;
    let principal = guest(&state, &headers, link).await?;
    let session = guest_session(&principal)?;
    let gate = state.delivery.get(session.document_id).await;
    let guard = gate.lock.clone().read_owned().await;
    let mut input: AddComment =
        parse_zeroized(body).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let request = v7(&input.request_id)?;
    let address = address(&input.sheet_id, &input.row_id, &input.column_id)?;
    validate_address(&state, principal.identity, session.document_id, address).await?;
    state
        .database
        .add_comment(
            principal.identity,
            session.document_id,
            request,
            address,
            Zeroizing::new(std::mem::take(&mut input.text)),
        )
        .await
        .map_err(comment_error)?;
    let mut page = comments_page(
        &state,
        principal.identity,
        session.document_id,
        address,
        None,
    )
    .await?;
    mask_guest_comments(&state, &principal, session.document_id, link, &mut page.0).await?;
    response_with_guard(&state, &principal, session.document_id, page.0, guard).await
}
pub async fn delete_guest_comment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((link, comment)): Path<(String, String)>,
    body: Bytes,
) -> Result<Response, ApiError> {
    require_origin(&state, &headers)?;
    let link = v7(&link)?;
    let principal = guest(&state, &headers, link).await?;
    let session = guest_session(&principal)?;
    let gate = state.delivery.get(session.document_id).await;
    let guard = gate.lock.clone().read_owned().await;
    let comment = v7(&comment)?;
    let input: DeleteComment = serde_json::from_slice(&body)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let request = v7(&input.request_id)?;
    let address = state
        .database
        .delete_comment(principal.identity, session.document_id, request, comment)
        .await
        .map_err(comment_error)?;
    let mut page = comments_page(
        &state,
        principal.identity,
        session.document_id,
        address,
        None,
    )
    .await?;
    mask_guest_comments(&state, &principal, session.document_id, link, &mut page.0).await?;
    response_with_guard(&state, &principal, session.document_id, page.0, guard).await
}

pub async fn shared_document(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(link): Path<String>,
) -> Result<Response, ApiError> {
    let link = v7(&link).map_err(|_| ApiError(StatusCode::NOT_FOUND, "share_unavailable"))?;
    let principal = guest(&state, &headers, link).await?;
    let session = guest_session(&principal)?;
    if state
        .database
        .requires_anonymization(principal.identity, session.document_id, Some(link))
        .await
        .map_err(crate::disclosure_api::error)?
    {
        let view = crate::privacy_api::safe_view(
            &state,
            principal.identity,
            session.document_id,
            Some(link),
        )
        .await?;
        return response_under_gate(&state, &principal, session.document_id, view).await;
    }
    let view = crate::documents::guest_document_view(
        &state,
        principal.identity,
        session.document_id,
        session.role,
        session.link_id,
    )
    .await
    .map_err(|error| ApiError(error.0, error.1))?;
    response_under_gate(&state, &principal, session.document_id, view.0).await
}

async fn response_under_gate<T: Serialize>(
    state: &Arc<AppState>,
    principal: &auth::Principal,
    document: DocumentId,
    value: T,
) -> Result<Response, ApiError> {
    let gate = state.delivery.get(document).await;
    let guard = gate.lock.clone().read_owned().await;
    response_with_guard(state, principal, document, value, guard).await
}
async fn response_with_guard<T: Serialize>(
    state: &Arc<AppState>,
    principal: &auth::Principal,
    document: DocumentId,
    value: T,
    guard: tokio::sync::OwnedRwLockReadGuard<()>,
) -> Result<Response, ApiError> {
    if let auth::PrincipalScope::ShareGuest { session, .. } = &principal.scope
        && session.document_id != document
    {
        return Err(ApiError(StatusCode::NOT_FOUND, "share_unavailable"));
    }
    if !state
        .auth
        .active(principal)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
    {
        return Err(ApiError(StatusCode::NOT_FOUND, "share_unavailable"));
    }
    let lease = state
        .database
        .lease_delivery_access(principal.identity, document)
        .await
        .map_err(|_| ApiError(StatusCode::NOT_FOUND, "share_unavailable"))?;
    let body = serde_json::to_vec(&value)
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
    if body.len() > 4 * 1024 * 1024 {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "response_too_large",
        ));
    }
    let stream = futures_util::stream::unfold(
        (Some(body), guard, lease),
        |(body, guard, lease)| async move {
            body.map(|body| {
                (
                    Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(body)),
                    (None, guard, lease),
                )
            })
        },
    );
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(axum::http::header::CACHE_CONTROL, "no-store")
        .body(Body::from_stream(stream))
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "unavailable"))?;
    Ok(response)
}

pub async fn guest_calculation_context(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(link): Path<String>,
) -> Result<Response, ApiError> {
    let link = v7(&link)?;
    let principal = guest(&state, &headers, link).await?;
    let session = guest_session(&principal)?;
    deny_anonymized_history(&state, &principal, session.document_id, link).await?;
    let context = crate::documents::guest_calculation_context(
        &state,
        principal.identity,
        session.document_id,
    )
    .await
    .map_err(|error| ApiError(error.0, error.1))?;
    response_under_gate(&state, &principal, session.document_id, context.0).await
}

pub async fn guest_undo_history(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(link): Path<String>,
) -> Result<Response, ApiError> {
    let link = v7(&link)?;
    let principal = guest(&state, &headers, link).await?;
    let session = guest_session(&principal)?;
    deny_anonymized_history(&state, &principal, session.document_id, link).await?;
    let gate = state.delivery.get(session.document_id).await;
    let guard = gate.lock.clone().read_owned().await;
    let history = state
        .database
        .undo_history(principal.identity, session.document_id)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    if !state
        .auth
        .active(&principal)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
    {
        return Err(ApiError(StatusCode::NOT_FOUND, "share_unavailable"));
    }
    use spreadsheet_application::history::HistoryTruncation;
    let targets = |items: Vec<spreadsheet_application::history::HistoryTarget>| {
        items.into_iter().map(|target|serde_json::json!({"operation_id":Uuid::from_bytes(*target.operation_id.as_bytes()).to_string(),"committed_version":target.committed_version.get().to_string()})).collect::<Vec<_>>()
    };
    let value = serde_json::json!({"schema_version":1,"document_id":Uuid::from_bytes(*history.document_id.as_bytes()).to_string(),"actor_id":Uuid::from_bytes(*history.actor_id.as_bytes()).to_string(),"through_version":history.through_version.get().to_string(),"earliest_scanned_version":history.earliest_scanned_version.get().to_string(),"undo":targets(history.undo),"redo":targets(history.redo),"truncated":history.truncation.is_some(),"truncation_reason":history.truncation.map(|reason|match reason{HistoryTruncation::VersionWindow=>"version_window",HistoryTruncation::ByteBudget=>"byte_budget"})});
    response_with_guard(&state, &principal, session.document_id, value, guard).await
}

pub async fn guest_recalculate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(link): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    require_origin(&state, &headers)?;
    let link = v7(&link)?;
    let principal = guest(&state, &headers, link).await?;
    let session_seed = guest_session(&principal)?;
    deny_anonymized_history(&state, &principal, session_seed.document_id, link).await?;
    let gate = state.delivery.get(session_seed.document_id).await;
    let guard = gate.lock.clone().read_owned().await;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        operation_id: String,
    }
    let input: Request = serde_json::from_slice(&body)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let operation =
        spreadsheet_domain::workbook::OperationId::from_bytes(*v7(&input.operation_id)?.as_bytes())
            .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let (_, context) = crate::calculation_context::sample(
        spreadsheet_domain::workbook::CalculationReason::Explicit,
    )
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    let session = state
        .sessions
        .session(principal.identity, session_seed.document_id)
        .await
        .map_err(|_| ApiError(StatusCode::NOT_FOUND, "share_unavailable"))?;
    let receipt = session
        .advance_context(principal.identity, operation, context)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    let value = serde_json::json!({"operation_id":Uuid::from_bytes(*receipt.operation_id.as_bytes()).to_string(),"committed_version":receipt.committed_version.get().to_string(),"current_head_version":receipt.current_head_version.get().to_string(),"canonical_event_hash":hex(&receipt.canonical_event_hash)});
    response_with_guard(&state, &principal, session_seed.document_id, value, guard).await
}

async fn comments_page(
    state: &AppState,
    identity: AuthenticatedActor,
    document: DocumentId,
    address: CommentAddress,
    cursor: Option<u64>,
) -> Result<Json<CommentsView>, ApiError> {
    let page = state
        .database
        .list_comments(identity, document, address, cursor, 100)
        .await
        .map_err(comment_error)?;
    let mut response = CommentsView {
        revision: page.revision.to_string(),
        comments: page
            .comments
            .into_iter()
            .map(|value| CommentView {
                comment_id: value.comment_id,
                sheet_id: value.address.sheet_id,
                row_id: value.address.row_id,
                column_id: value.address.column_id,
                author_id: value.author_id,
                text: value.text.to_string(),
                can_delete: value.can_delete,
                revision: value.revision,
            })
            .collect(),
        next_cursor: page.next_cursor.map(|value| value.to_string()),
    };
    while serde_json::to_vec(&response).map_or(true, |bytes| bytes.len() > 512 * 1024)
        && response.comments.len() > 1
    {
        response.comments.pop();
        response.next_cursor = response
            .comments
            .last()
            .map(|comment| comment.revision.to_string());
    }
    Ok(Json(response))
}
async fn deny_anonymized_history(
    state: &AppState,
    principal: &auth::Principal,
    doc: DocumentId,
    link: Uuid,
) -> Result<(), ApiError> {
    if state
        .database
        .requires_anonymization(principal.identity, doc, Some(link))
        .await
        .map_err(crate::disclosure_api::error)?
    {
        return Err(ApiError(StatusCode::FORBIDDEN, "anonymized_read_only"));
    }
    Ok(())
}
async fn mask_guest_comments(
    state: &AppState,
    principal: &auth::Principal,
    doc: DocumentId,
    link: Uuid,
    page: &mut CommentsView,
) -> Result<(), ApiError> {
    if state
        .database
        .requires_anonymization(principal.identity, doc, Some(link))
        .await
        .map_err(crate::disclosure_api::error)?
    {
        let policy = state
            .database
            .privacy_policy(principal.identity, doc, Some(link))
            .await
            .map_err(crate::disclosure_api::error)?;
        if !policy.rules.enabled
            || policy
                .rules
                .kinds()
                .map_err(crate::disclosure_api::error)?
                .is_empty()
        {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "privacy_recognition_disabled",
            ));
        }
        for comment in &mut page.comments {
            comment.text = crate::privacy_api::text_mask(&comment.text, &policy.rules)?;
        }
    }
    Ok(())
}
async fn validate_address(
    state: &AppState,
    identity: AuthenticatedActor,
    document: DocumentId,
    address: CommentAddress,
) -> Result<(), ApiError> {
    let session = state
        .sessions
        .session(identity, document)
        .await
        .map_err(|_| ApiError(StatusCode::NOT_FOUND, "not_found"))?;
    let workbook = session
        .read(identity)
        .await
        .map_err(|_| ApiError(StatusCode::NOT_FOUND, "not_found"))?;
    let sheet = workbook
        .sheets()
        .iter()
        .find(|sheet| sheet.id().as_bytes() == address.sheet_id.as_bytes())
        .ok_or(ApiError(StatusCode::BAD_REQUEST, "invalid_address"))?;
    if !sheet
        .rows()
        .iter()
        .any(|row| row.as_bytes() == address.row_id.as_bytes())
        || !sheet
            .columns()
            .iter()
            .any(|column| column.as_bytes() == address.column_id.as_bytes())
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_address"));
    }
    Ok(())
}
async fn any_principal(
    state: &AppState,
    headers: &HeaderMap,
    link: Option<Uuid>,
) -> Result<auth::Principal, ApiError> {
    if let Some(link) = link {
        return state
            .auth
            .authenticate_share(headers, link)
            .await
            .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
            .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    state
        .auth
        .authenticate(headers)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"))
}
async fn guest(
    state: &AppState,
    headers: &HeaderMap,
    link: Uuid,
) -> Result<auth::Principal, ApiError> {
    state
        .auth
        .authenticate_share(headers, link)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "share_unavailable"))
}
fn guest_session(
    principal: &auth::Principal,
) -> Result<spreadsheet_storage::GuestSession, ApiError> {
    match &principal.scope {
        PrincipalScope::ShareGuest { session, .. } => Ok(*session),
        PrincipalScope::Member => Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized")),
    }
}
fn require_origin(state: &AppState, headers: &HeaderMap) -> Result<(), ApiError> {
    if auth::valid_origin(headers, &state.origins) {
        Ok(())
    } else {
        Err(ApiError(StatusCode::FORBIDDEN, "origin_denied"))
    }
}
fn parse_zeroized<T: serde::de::DeserializeOwned>(body: Bytes) -> Result<T, ()> {
    let bytes = Zeroizing::new(body.as_ref().to_vec());
    serde_json::from_slice(&bytes).map_err(|_| ())
}

async fn member(state: &AppState, headers: &HeaderMap) -> Result<auth::Principal, ApiError> {
    let principal = any_principal(state, headers, None).await?;
    if !matches!(principal.scope, PrincipalScope::Member) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    Ok(principal)
}
fn document_id(value: &str) -> Result<DocumentId, ApiError> {
    let id = v7(value)?;
    DocumentId::from_bytes(*id.as_bytes())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_request"))
}
fn v7(value: &str) -> Result<Uuid, ApiError> {
    let id =
        Uuid::parse_str(value).map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_request"))?;
    if id.get_version_num() != 7 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_request"));
    }
    Ok(id)
}
fn address(sheet: &str, row: &str, column: &str) -> Result<CommentAddress, ApiError> {
    Ok(CommentAddress {
        sheet_id: v7(sheet)?,
        row_id: v7(row)?,
        column_id: v7(column)?,
    })
}
fn parse_decimal(value: &str) -> Result<u64, ApiError> {
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "invalid_cursor"));
    }
    value
        .parse()
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_cursor"))
}
fn decode_hex32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut result = [0; 32];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let hi = hex_digit(pair[0])?;
        let lo = hex_digit(pair[1])?;
        result[index] = (hi << 4) | lo;
    }
    Some(result)
}
fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
fn hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(D[(byte >> 4) as usize] as char);
        output.push(D[(byte & 15) as usize] as char);
    }
    output
}
fn share_error(value: ShareError) -> ApiError {
    match value {
        ShareError::InvalidInput => ApiError(StatusCode::BAD_REQUEST, "invalid_request"),
        ShareError::Unauthorized => ApiError(StatusCode::FORBIDDEN, "unauthorized"),
        ShareError::NotFound => ApiError(StatusCode::NOT_FOUND, "not_found"),
        ShareError::IdempotencyConflict => ApiError(StatusCode::CONFLICT, "idempotency_conflict"),
        ShareError::Conflict => ApiError(StatusCode::CONFLICT, "retryable_conflict"),
        _ => ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    }
}
fn comment_error(value: CommentError) -> ApiError {
    match value {
        CommentError::InvalidInput => ApiError(StatusCode::BAD_REQUEST, "invalid_request"),
        CommentError::Unauthorized => ApiError(StatusCode::FORBIDDEN, "unauthorized"),
        CommentError::NotFound => ApiError(StatusCode::NOT_FOUND, "not_found"),
        CommentError::IdempotencyConflict => ApiError(StatusCode::CONFLICT, "idempotency_conflict"),
        _ => ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    }
}
fn password_error(value: PasswordError) -> ApiError {
    match value {
        PasswordError::Busy => ApiError(StatusCode::TOO_MANY_REQUESTS, "share_unavailable"),
        PasswordError::Unavailable => ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    }
}

pub struct ApiError(StatusCode, &'static str);
impl From<crate::documents::ApiError> for ApiError {
    fn from(error: crate::documents::ApiError) -> Self {
        Self(error.0, error.1)
    }
}
fn locked_response(seconds: u64) -> Response {
    #[derive(Serialize)]
    struct Locked {
        code: &'static str,
        retry_after_seconds: u64,
    }
    (
        StatusCode::TOO_MANY_REQUESTS,
        Json(Locked {
            code: "share_locked",
            retry_after_seconds: seconds.max(1),
        }),
    )
        .into_response()
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct ErrorBody {
            code: &'static str,
            #[serde(skip_serializing_if = "Option::is_none")]
            retry_after_seconds: Option<u64>,
        }
        (
            self.0,
            Json(ErrorBody {
                code: self.1,
                retry_after_seconds: None,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod request_body_tests {
    use super::parse_zeroized;
    use axum::body::Bytes;
    use serde::Deserialize;

    #[derive(Deserialize, Debug, PartialEq)]
    struct Payload {
        value: String,
    }

    #[test]
    fn parses_a_shared_body_buffer() -> Result<(), ()> {
        let body = Bytes::from(r#"{"value":"payload"}"#);
        let _shared_reference = body.clone();
        let parsed: Payload = parse_zeroized(body)?;
        assert_eq!(
            parsed,
            Payload {
                value: "payload".into()
            }
        );
        Ok(())
    }
}
