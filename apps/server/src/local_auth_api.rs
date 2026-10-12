//! First-run setup and administrator-issued local accounts. Secrets occur only
//! in bounded request bodies; transport errors never serialize their contents.
use crate::{AppState, auth, share_password::PasswordError};
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use spreadsheet_storage::{
    LocalAccount, LocalAccountError, valid_local_password, valid_local_username,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::OwnedSemaphorePermit;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

#[derive(Deserialize)]
#[serde(transparent)]
struct Secret(String);
impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
impl Secret {
    fn take(&mut self) -> Zeroizing<String> {
        Zeroizing::new(std::mem::take(&mut self.0))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginBody {
    schema_version: u32,
    username: String,
    password: Secret,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupBody {
    schema_version: u32,
    bootstrap_token: Secret,
    username: String,
    password: Secret,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PasswordBody {
    schema_version: u32,
    current_password: Secret,
    new_password: Secret,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionBody {
    schema_version: u32,
    action: String,
    password: Option<Secret>,
}

#[derive(Serialize)]
struct AuthStatus {
    schema_version: u32,
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<LocalAccount>,
}
fn account_status(account: LocalAccount) -> Response {
    Json(AuthStatus {
        schema_version: 1,
        mode: if account.must_change_password {
            "password_change"
        } else {
            "authenticated"
        },
        account: Some(account),
    })
    .into_response()
}
fn error(status: StatusCode, code: &'static str) -> Response {
    #[derive(Serialize)]
    struct ErrorBody {
        code: &'static str,
    }
    (status, Json(ErrorBody { code })).into_response()
}
struct AuthError(StatusCode, &'static str);
impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        error(self.0, self.1)
    }
}
fn failure(status: StatusCode, code: &'static str) -> AuthError {
    AuthError(status, code)
}
fn storage_error(value: LocalAccountError) -> Response {
    match value {
        LocalAccountError::InvalidInput => error(StatusCode::BAD_REQUEST, "invalid_input"),
        LocalAccountError::AlreadyInitialized => {
            error(StatusCode::CONFLICT, "setup_already_completed")
        }
        LocalAccountError::ExistingDocuments => error(StatusCode::CONFLICT, "existing_documents"),
        LocalAccountError::Unauthorized => error(StatusCode::UNAUTHORIZED, "unauthorized"),
        LocalAccountError::MasterProtected => error(StatusCode::FORBIDDEN, "master_protected"),
        LocalAccountError::Capacity => error(StatusCode::CONFLICT, "capacity"),
        LocalAccountError::Conflict => error(StatusCode::CONFLICT, "account_exists"),
        LocalAccountError::Unavailable => error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    }
}
fn password_error(value: PasswordError) -> Response {
    match value {
        PasswordError::Busy => error(StatusCode::TOO_MANY_REQUESTS, "busy"),
        PasswordError::Unavailable => error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    }
}
async fn admission(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<OwnedSemaphorePermit, AuthError> {
    if !auth::valid_origin(headers, &state.origins) {
        return Err(failure(StatusCode::FORBIDDEN, "origin"));
    }
    if state.auth.development_enabled() {
        if !crate::rate_allowed(state).await {
            return Err(failure(StatusCode::TOO_MANY_REQUESTS, "busy"));
        }
        return state
            .auth_admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| failure(StatusCode::TOO_MANY_REQUESTS, "busy"));
    }
    let mut rate = state.auth_rate.lock().await;
    let now = Instant::now();
    if now.duration_since(rate.0) >= Duration::from_secs(60) {
        *rate = (now, 0)
    }
    if rate.1 >= 60 {
        return Err(failure(StatusCode::TOO_MANY_REQUESTS, "busy"));
    }
    rate.1 += 1;
    drop(rate);
    state
        .auth_admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| failure(StatusCode::TOO_MANY_REQUESTS, "busy"))
}
async fn body<T: DeserializeOwned>(body: Body) -> Result<T, AuthError> {
    let bytes = tokio::time::timeout(Duration::from_secs(5), to_bytes(body, 4096))
        .await
        .map_err(|_| failure(StatusCode::REQUEST_TIMEOUT, "body_timeout"))?
        .map_err(|_| failure(StatusCode::PAYLOAD_TOO_LARGE, "body_limit"))?;
    let bytes = Zeroizing::new(bytes.to_vec());
    serde_json::from_slice(&bytes).map_err(|_| failure(StatusCode::BAD_REQUEST, "invalid_input"))
}
fn version(value: u32) -> Result<(), AuthError> {
    if value == 1 {
        Ok(())
    } else {
        Err(failure(StatusCode::BAD_REQUEST, "schema_version"))
    }
}
fn new_token() -> Result<Zeroizing<String>, AuthError> {
    use std::fmt::Write;
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(&mut *bytes)
        .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    let mut token = Zeroizing::new(String::with_capacity(64));
    for byte in bytes.iter() {
        write!(&mut *token, "{byte:02x}")
            .map_err(|_| failure(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    }
    Ok(token)
}
fn with_cookie(mut response: Response, token: &str) -> Response {
    match HeaderValue::from_str(&auth::local_cookie(token)) {
        Ok(cookie) => {
            response.headers_mut().insert(header::SET_COOKIE, cookie);
            response
        }
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    }
}
fn local_only(state: &AppState) -> Result<(), AuthError> {
    if state.auth.development_enabled() {
        Err(failure(StatusCode::NOT_FOUND, "unavailable"))
    } else {
        Ok(())
    }
}

pub async fn status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let mode = if state.auth.development_enabled() {
        "development"
    } else {
        match state.auth.local_session(&headers).await {
            Ok(Some(session)) => return account_status(session.account),
            Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
            Ok(None) => {}
        }
        match state.database.local_instance_initialized().await {
            Ok(true) => "login",
            Ok(false) => match state.database.local_setup_available().await {
                Ok(true) if state.bootstrap_token_hash.is_some() => "setup",
                Ok(_) => "unavailable",
                Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
            },
            Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
        }
    };
    Json(AuthStatus {
        schema_version: 1,
        mode,
        account: None,
    })
    .into_response()
}
pub async fn session(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let _permit = match admission(&state, &headers).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if state.auth.development_enabled() {
        #[cfg(feature = "dev-auth")]
        return match state.auth.issue(&headers).await {
            Ok(Some(cookie)) => match HeaderValue::from_str(&cookie) {
                Ok(cookie) => {
                    let mut response = StatusCode::NO_CONTENT.into_response();
                    response.headers_mut().insert(header::SET_COOKIE, cookie);
                    response
                }
                Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
            },
            _ => error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
        };
    }
    match state.auth.authenticate(&headers).await {
        Ok(Some(_)) => StatusCode::NO_CONTENT.into_response(),
        Ok(None) => error(StatusCode::UNAUTHORIZED, "unauthorized"),
        Err(_) => error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    }
}
pub async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let _permit = match admission(&state, &headers).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if state.auth.development_enabled() {
        #[cfg(feature = "dev-auth")]
        return crate::revoke_dev_session(State(state), headers).await;
    }
    if let Some(hash) = auth::local_session_hash(&headers)
        && state.database.revoke_local_session(hash).await.is_err()
    {
        return error(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "pxel_session=; HttpOnly; Secure; SameSite=Strict; Path=/api; Max-Age=0",
        ),
    );
    response
}
pub async fn setup(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Body,
) -> Response {
    let _permit = match admission(&state, &headers).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = local_only(&state) {
        return e.into_response();
    }
    let Some(expected) = state.bootstrap_token_hash else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "setup_unavailable");
    };
    let mut request: SetupBody = match body(request).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = version(request.schema_version) {
        return e.into_response();
    }
    let digest: [u8; 32] = Sha256::digest(request.bootstrap_token.0.as_bytes()).into();
    let difference = digest
        .iter()
        .zip(expected)
        .fold(0u8, |difference, (actual, expected)| {
            difference | (actual ^ expected)
        });
    if difference != 0 {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let username = request.username.to_ascii_lowercase();
    if !valid_local_username(&username) {
        return error(StatusCode::BAD_REQUEST, "invalid_username");
    }
    if !valid_local_password(&request.password.0) {
        return error(StatusCode::BAD_REQUEST, "invalid_password");
    }
    let token = match new_token() {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let permit = match state.passwords.admit() {
        Ok(p) => p,
        Err(e) => return password_error(e),
    };
    let phc = match state.passwords.hash(request.password.take(), permit).await {
        Ok(v) => v,
        Err(e) => return password_error(e),
    };
    match state
        .database
        .bootstrap_local_instance(&username, phc, auth::token_hash(&token))
        .await
    {
        Ok(account) => with_cookie(account_status(account), &token),
        Err(e) => storage_error(e),
    }
}
pub async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Body,
) -> Response {
    let _permit = match admission(&state, &headers).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = local_only(&state) {
        return e.into_response();
    }
    let mut request: LoginBody = match body(request).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = version(request.schema_version) {
        return e.into_response();
    }
    let username = request.username.to_ascii_lowercase();
    let credential = if valid_local_username(&username) && valid_local_password(&request.password.0)
    {
        match state.database.local_credential(&username).await {
            Ok(v) => v,
            Err(e) => return storage_error(e),
        }
    } else {
        None
    };
    let permit = match state.passwords.admit() {
        Ok(p) => p,
        Err(e) => return password_error(e),
    };
    let valid = match state
        .passwords
        .verify(
            request.password.take(),
            credential.as_ref().and_then(|c| c.phc.clone()),
            permit,
        )
        .await
    {
        Ok(v) => v,
        Err(e) => return password_error(e),
    };
    let token = match new_token() {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    match state
        .database
        .complete_local_login(credential.as_ref(), valid, auth::token_hash(&token))
        .await
    {
        Ok(Some(account)) => with_cookie(account_status(account), &token),
        Ok(None) => error(StatusCode::UNAUTHORIZED, "unauthorized"),
        Err(e) => storage_error(e),
    }
}
pub async fn password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Body,
) -> Response {
    let _permit = match admission(&state, &headers).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = local_only(&state) {
        return e.into_response();
    }
    let session = match state.auth.local_session(&headers).await {
        Ok(Some(s)) => s,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "unauthorized"),
        Err(_) => return error(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    };
    let Some(hash) = auth::local_session_hash(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    let mut request: PasswordBody = match body(request).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = version(request.schema_version) {
        return e.into_response();
    }
    if !valid_local_password(&request.new_password.0)
        || request.new_password.0 == request.current_password.0
    {
        return error(StatusCode::BAD_REQUEST, "invalid_password");
    }
    let credential = match state
        .database
        .local_credential(&session.account.username)
        .await
    {
        Ok(Some(c)) => c,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "unauthorized"),
        Err(e) => return storage_error(e),
    };
    let permit = match state.passwords.admit() {
        Ok(p) => p,
        Err(e) => return password_error(e),
    };
    let valid = match state
        .passwords
        .verify(
            request.current_password.take(),
            credential.phc.clone(),
            permit,
        )
        .await
    {
        Ok(v) => v,
        Err(e) => return password_error(e),
    };
    if !valid {
        if let Err(e) = state
            .database
            .complete_local_login(Some(&credential), false, [0; 32])
            .await
        {
            return storage_error(e);
        }
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let token = match new_token() {
        Ok(t) => t,
        Err(e) => return e.into_response(),
    };
    let permit = match state.passwords.admit() {
        Ok(p) => p,
        Err(e) => return password_error(e),
    };
    let phc = match state
        .passwords
        .hash(request.new_password.take(), permit)
        .await
    {
        Ok(v) => v,
        Err(e) => return password_error(e),
    };
    match state
        .database
        .change_local_password(hash, &credential, phc, auth::token_hash(&token))
        .await
    {
        Ok(account) => with_cookie(account_status(account), &token),
        Err(e) => storage_error(e),
    }
}
pub async fn accounts(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(e) = local_only(&state) {
        return e.into_response();
    }
    let Some(hash) = auth::local_session_hash(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    match state.database.list_local_accounts(hash).await {
        Ok(accounts) => {
            #[derive(Serialize)]
            struct Accounts {
                schema_version: u32,
                accounts: Vec<LocalAccount>,
            }
            Json(Accounts {
                schema_version: 1,
                accounts,
            })
            .into_response()
        }
        Err(e) => storage_error(e),
    }
}
pub async fn create_account(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Body,
) -> Response {
    let _permit = match admission(&state, &headers).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = local_only(&state) {
        return e.into_response();
    }
    let Some(hash) = auth::local_session_hash(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    // Reject ordinary users before they can consume a password worker.
    if let Err(e) = state.database.list_local_accounts(hash).await {
        return storage_error(e);
    }
    let mut request: LoginBody = match body(request).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = version(request.schema_version) {
        return e.into_response();
    }
    let username = request.username.to_ascii_lowercase();
    if !valid_local_username(&username) {
        return error(StatusCode::BAD_REQUEST, "invalid_username");
    }
    if !valid_local_password(&request.password.0) {
        return error(StatusCode::BAD_REQUEST, "invalid_password");
    }
    let permit = match state.passwords.admit() {
        Ok(p) => p,
        Err(e) => return password_error(e),
    };
    let phc = match state.passwords.hash(request.password.take(), permit).await {
        Ok(v) => v,
        Err(e) => return password_error(e),
    };
    match state
        .database
        .create_local_account(hash, &username, phc)
        .await
    {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => storage_error(e),
    }
}
pub async fn update_account(
    State(state): State<Arc<AppState>>,
    Path(user): Path<Uuid>,
    headers: HeaderMap,
    request: Body,
) -> Response {
    let _permit = match admission(&state, &headers).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = local_only(&state) {
        return e.into_response();
    }
    let Some(hash) = auth::local_session_hash(&headers) else {
        return error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    if let Err(e) = state.database.list_local_accounts(hash).await {
        return storage_error(e);
    }
    let mut request: ActionBody = match body(request).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = version(request.schema_version) {
        return e.into_response();
    }
    let (enabled, reset) = match request.action.as_str() {
        "disable" if request.password.is_none() => (Some(false), None),
        "enable" if request.password.is_none() => (Some(true), None),
        "reset_password" => {
            let Some(password) = request.password.as_mut() else {
                return error(StatusCode::BAD_REQUEST, "invalid_password");
            };
            if !valid_local_password(&password.0) {
                return error(StatusCode::BAD_REQUEST, "invalid_password");
            }
            let permit = match state.passwords.admit() {
                Ok(p) => p,
                Err(e) => return password_error(e),
            };
            let phc = match state.passwords.hash(password.take(), permit).await {
                Ok(v) => v,
                Err(e) => return password_error(e),
            };
            (None, Some(phc))
        }
        _ => return error(StatusCode::BAD_REQUEST, "invalid_input"),
    };
    match state
        .database
        .update_local_account(hash, user, enabled, reset)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => storage_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn production_cookie_is_always_private_secure_and_absolute() {
        let cookie = auth::local_cookie(&"a".repeat(64));
        assert!(cookie.contains("HttpOnly; Secure; SameSite=Strict; Path=/api; Max-Age=28800"));
    }
    #[tokio::test]
    async fn body_rejects_oversize_and_unknown_schema_fields() {
        assert!(
            body::<LoginBody>(Body::from("a".repeat(4097)))
                .await
                .is_err()
        );
        assert!(
            body::<LoginBody>(Body::from(
                r#"{"schema_version":1,"username":"alice","password":"abcdefghijkl","extra":true}"#
            ))
            .await
            .is_err()
        );
    }
}
