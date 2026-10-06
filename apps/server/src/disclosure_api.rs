use crate::{AppState, auth, documents::ApiError};
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use spreadsheet_domain::workbook::DocumentId;
use spreadsheet_storage::{
    DisclosureAction, DisclosureAuditPage, DisclosureError, DisclosurePolicy, DisclosureReceipt,
    DisclosureRules, DisclosureStamp,
};
use std::sync::Arc;
use uuid::Uuid;

pub fn error(error: DisclosureError) -> ApiError {
    match error {
        DisclosureError::Unauthorized => ApiError(StatusCode::FORBIDDEN, "unauthorized"),
        DisclosureError::Conflict => ApiError(StatusCode::CONFLICT, "policy_changed"),
        DisclosureError::InvalidInput => ApiError(StatusCode::BAD_REQUEST, "invalid_policy"),
        _ => ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "disclosure_audit_unavailable",
        ),
    }
}
fn identifier(value: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(value)
        .ok()
        .filter(|id| id.get_version_num() == 7)
        .ok_or(ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))
}
async fn member(state: &AppState, headers: &HeaderMap) -> Result<auth::Principal, ApiError> {
    state
        .auth
        .authenticate(headers)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"))
}
fn document(value: &str) -> Result<DocumentId, ApiError> {
    DocumentId::from_bytes(*identifier(value)?.as_bytes())
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, "invalid_identity"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyQuery {
    link_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyUpdate {
    schema_version: u32,
    expected_revision: u64,
    rules: DisclosureRules,
}
pub async fn get_policy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<PolicyQuery>,
) -> Result<Json<DisclosurePolicy>, ApiError> {
    let principal = member(&state, &headers).await?;
    Ok(Json(
        state
            .database
            .disclosure_policy(
                principal.identity,
                document(&id)?,
                query.link_id.as_deref().map(identifier).transpose()?,
            )
            .await
            .map_err(error)?,
    ))
}
pub async fn set_policy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<PolicyQuery>,
    Json(input): Json<PolicyUpdate>,
) -> Result<Json<DisclosurePolicy>, ApiError> {
    if !auth::valid_origin(&headers, &state.origins) {
        return Err(ApiError(StatusCode::FORBIDDEN, "origin_denied"));
    }
    if input.schema_version != 1 {
        return Err(ApiError(StatusCode::BAD_REQUEST, "unknown_policy_version"));
    }
    let principal = member(&state, &headers).await?;
    let doc = document(&id)?;
    let gate = state.delivery.get(doc).await;
    let _write = tokio::time::timeout(std::time::Duration::from_secs(3), gate.lock.write())
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "send_gate_busy"))?;
    let policy = state
        .database
        .set_disclosure_policy(
            principal.identity,
            doc,
            query.link_id.as_deref().map(identifier).transpose()?,
            input.expected_revision,
            input.rules,
        )
        .await
        .map_err(error)?;
    gate.committed_epoch(policy.access_epoch);
    Ok(Json(policy))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditQuery {
    cursor: Option<String>,
}
pub async fn audit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<DisclosureAuditPage>, ApiError> {
    let principal = member(&state, &headers).await?;
    Ok(Json(
        state
            .database
            .disclosure_audit(
                principal.identity,
                document(&id)?,
                query.cursor.as_deref().map(identifier).transpose()?,
            )
            .await
            .map_err(error)?,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipboardRequest {
    schema_version: u32,
    action: DisclosureAction,
    stamp: DisclosureStamp,
    #[serde(default)]
    text: Option<String>,
}
pub async fn clipboard(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ClipboardRequest>,
) -> Result<Response, ApiError> {
    let principal = member(&state, &headers).await?;
    clipboard_response(&state, &headers, principal, document(&id)?, None, input).await
}
pub async fn guest_clipboard(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(link): Path<String>,
    Json(input): Json<ClipboardRequest>,
) -> Result<Response, ApiError> {
    let link = identifier(&link)?;
    let principal = state
        .auth
        .authenticate_share(&headers, link)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
        .ok_or(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"))?;
    let auth::PrincipalScope::ShareGuest { session, .. } = &principal.scope else {
        return Err(ApiError(StatusCode::FORBIDDEN, "unauthorized"));
    };
    let doc = session.document_id;
    clipboard_response(&state, &headers, principal, doc, Some(link), input).await
}
async fn clipboard_response(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    principal: auth::Principal,
    doc: DocumentId,
    link: Option<Uuid>,
    input: ClipboardRequest,
) -> Result<Response, ApiError> {
    if !auth::valid_origin(headers, &state.origins) {
        return Err(ApiError(StatusCode::FORBIDDEN, "origin_denied"));
    }
    if input.schema_version != 1
        || matches!(
            input.action,
            DisclosureAction::ExportXlsx
                | DisclosureAction::ExportXlsxAnonymized
                | DisclosureAction::ExportPxel
                | DisclosureAction::ExportPxelAnonymized
        )
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "invalid_clipboard_request",
        ));
    }
    if !crate::rate_allowed(state).await {
        return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "rate_limit"));
    }
    // Actor authorization reads take a document UPDATE lock. Prepare the safe
    // projection before the final SHARE delivery lease to avoid lock inversion.
    let needs_safe = input.action == DisclosureAction::CopyAnonymized
        || match state
            .database
            .requires_anonymization(principal.identity, doc, link)
            .await
        {
            Ok(value) => value,
            Err(DisclosureError::Unauthorized) => false,
            Err(failure) => return Err(error(failure)),
        };
    let initial = if needs_safe {
        Some(
            state
                .database
                .authorize_disclosure(
                    principal.identity,
                    doc,
                    link,
                    input.action,
                    Some(input.stamp),
                )
                .await
                .map_err(error)?,
        )
    } else {
        None
    };
    let receipt = initial.as_ref();
    let projection = if receipt.is_some_and(|r| r.allowed && r.anonymized) {
        let view = match crate::privacy_api::safe_view(state, principal.identity, doc, link).await {
            Ok(view) => view,
            Err(failure) => {
                state
                    .database
                    .record_disclosure_failure(
                        principal.identity,
                        doc,
                        link,
                        input.action,
                        input.stamp,
                    )
                    .await
                    .map_err(error)?;
                return Err(failure);
            }
        };
        if view.stamp() != input.stamp {
            return Err(ApiError(StatusCode::CONFLICT, "document_changed"));
        }
        Some(view)
    } else {
        None
    };
    let text = if receipt.is_some_and(|r| r.allowed && r.anonymized) {
        if let Some(text) = input.text {
            let policy = state
                .database
                .privacy_policy(principal.identity, doc, link)
                .await
                .map_err(error)?;
            Some(crate::privacy_api::text_mask(&text, &policy.rules)?)
        } else {
            None
        }
    } else {
        None
    };
    let (receipt, lease, guard) = prepare_delivery(
        state,
        &principal,
        doc,
        link,
        input.action,
        Some(input.stamp),
    )
    .await?;
    if receipt.allowed
        && receipt.anonymized
        && projection
            .as_ref()
            .is_none_or(|view| view.stamp() != receipt.stamp)
    {
        return Err(ApiError(StatusCode::CONFLICT, "document_changed"));
    }
    #[derive(serde::Serialize)]
    struct ClipboardReceipt {
        #[serde(flatten)]
        receipt: DisclosureReceipt,
        #[serde(skip_serializing_if = "Option::is_none")]
        projection: Option<crate::documents::RedactedView>,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    }
    let allowed = receipt.allowed;
    let body = serde_json::to_vec(&ClipboardReceipt {
        receipt,
        projection,
        text,
    })
    .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    protected_response(body, "application/json", None, lease, guard, allowed)
}
pub async fn prepare_delivery(
    state: &Arc<AppState>,
    principal: &auth::Principal,
    doc: DocumentId,
    link: Option<Uuid>,
    action: DisclosureAction,
    stamp: Option<DisclosureStamp>,
) -> Result<
    (
        DisclosureReceipt,
        spreadsheet_storage::DocumentAccessLease,
        tokio::sync::OwnedRwLockReadGuard<()>,
    ),
    ApiError,
> {
    let gate = state.delivery.get(doc).await;
    let guard = gate.lock.clone().read_owned().await;
    if !state
        .auth
        .active(principal)
        .await
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?
    {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "unauthorized"));
    }
    // The durable document SHARE lock fences policy/ACL changes through body delivery.
    let lease_result = state
        .database
        .lease_delivery_access(principal.identity, doc)
        .await;
    let lease = match lease_result {
        Ok(lease) => lease,
        Err(spreadsheet_application::journal::JournalError::Unauthorized) => {
            // Authenticated attempts with a removed ACL still receive a durable refusal.
            state
                .database
                .authorize_disclosure(principal.identity, doc, link, action, stamp)
                .await
                .map_err(error)?;
            return Err(ApiError(StatusCode::FORBIDDEN, "unauthorized"));
        }
        Err(_) => {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "disclosure_audit_unavailable",
            ));
        }
    };
    let receipt = state
        .database
        .authorize_disclosure(principal.identity, doc, link, action, stamp)
        .await
        .map_err(error)?;
    Ok((receipt, lease, guard))
}
pub fn protected_response(
    bytes: Vec<u8>,
    content_type: &'static str,
    disposition: Option<&'static str>,
    lease: spreadsheet_storage::DocumentAccessLease,
    guard: tokio::sync::OwnedRwLockReadGuard<()>,
    allowed: bool,
) -> Result<Response, ApiError> {
    let stream = futures_util::stream::unfold(
        (Some(bytes), lease, guard),
        |(bytes, lease, guard)| async move {
            bytes.map(|bytes| {
                (
                    Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(bytes)),
                    (None, lease, guard),
                )
            })
        },
    );
    let mut response = Response::builder()
        .status(if allowed {
            StatusCode::OK
        } else {
            StatusCode::FORBIDDEN
        })
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .header(axum::http::header::CACHE_CONTROL, "no-store");
    if let Some(value) = disposition {
        response = response.header(axum::http::header::CONTENT_DISPOSITION, value);
    }
    response
        .body(Body::from_stream(stream))
        .map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))
}
