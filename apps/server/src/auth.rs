use axum::http::{HeaderMap, header};
use sha2::{Digest, Sha256};
use spreadsheet_application::journal::AuthenticatedActor;
#[cfg(feature = "dev-auth")]
use spreadsheet_domain::workbook::{ActorId, WorkspaceId};
use spreadsheet_storage::{DatabaseError, GuestSession, PgDatabase};
use std::time::{Duration, Instant};
#[cfg(feature = "dev-auth")]
use uuid::Uuid;

#[derive(Clone)]
pub struct Principal {
    pub identity: AuthenticatedActor,
    pub expires_at: Instant,
    session_token_hash: [u8; 32],
    pub scope: PrincipalScope,
}

#[derive(Clone)]
pub enum PrincipalScope {
    Member,
    ShareGuest {
        session: GuestSession,
        token_hash: [u8; 32],
    },
}

pub struct Auth {
    enabled: bool,
    database: PgDatabase,
}

impl Auth {
    pub fn new(enabled: bool, database: PgDatabase) -> Self {
        Self {
            enabled: enabled && cfg!(feature = "dev-auth"),
            database,
        }
    }

    pub fn share_cookie(&self, token: &str, link_id: uuid::Uuid, remaining: u64) -> String {
        share_cookie(token, link_id, remaining, !self.enabled)
    }

    pub fn development_enabled(&self) -> bool {
        self.enabled
    }

    pub async fn local_session(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<spreadsheet_storage::LocalSession>, DatabaseError> {
        if self.enabled || contains_share_cookie(headers) {
            return Ok(None);
        }
        let Some(hash) = local_session_hash(headers) else {
            return Ok(None);
        };
        self.database
            .local_session(hash)
            .await
            .map_err(|_| DatabaseError::SchemaNotReady)
    }

    #[cfg(feature = "dev-auth")]
    pub async fn revoke(&self, headers: &HeaderMap) -> Result<(), DatabaseError> {
        if self.enabled
            && let Some(token) = session_token(headers)
        {
            self.database
                .revoke_development_session(token_hash(token))
                .await?;
        }
        Ok(())
    }

    #[cfg(feature = "dev-auth")]
    pub async fn issue(&self, headers: &HeaderMap) -> Result<Option<String>, DatabaseError> {
        if !self.enabled
            || self
                .database
                .local_instance_initialized()
                .await
                .map_err(|_| DatabaseError::SchemaNotReady)?
        {
            return Ok(None);
        }
        let selected_account = headers.get("x-pxel-dev-account");
        if selected_account.is_none()
            && let Some(token) = session_token(headers)
            && let Some(session) = self
                .database
                .find_development_session(token_hash(token))
                .await?
        {
            return Ok(Some(cookie(token, session.remaining_seconds)));
        }
        let token = Uuid::new_v4().simple().to_string();
        let actor = ActorId::from_bytes(*Uuid::now_v7().as_bytes())
            .map_err(|_| DatabaseError::InvalidConfiguration)?;
        let workspace = WorkspaceId::from_bytes(*Uuid::now_v7().as_bytes())
            .map_err(|_| DatabaseError::InvalidConfiguration)?;
        let account = selected_account
            .map(|value| value.to_str())
            .transpose()
            .map_err(|_| DatabaseError::InvalidConfiguration)?
            .unwrap_or("local");
        let requested_workspace = headers
            .get("x-pxel-dev-workspace")
            .map(|value| {
                let text = value
                    .to_str()
                    .map_err(|_| DatabaseError::InvalidConfiguration)?;
                let id = Uuid::parse_str(text).map_err(|_| DatabaseError::InvalidConfiguration)?;
                if id.get_version_num() != 7 {
                    return Err(DatabaseError::InvalidConfiguration);
                }
                WorkspaceId::from_bytes(*id.as_bytes())
                    .map_err(|_| DatabaseError::InvalidConfiguration)
            })
            .transpose()?;
        let session = self
            .database
            .issue_development_account_session_in_workspace(
                token_hash(&token),
                actor,
                workspace,
                account,
                requested_workspace,
            )
            .await?;
        Ok(Some(cookie(&token, session.remaining_seconds)))
    }

    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<Principal>, DatabaseError> {
        if contains_share_cookie(headers) {
            return Ok(None);
        }
        if !self.enabled {
            let Some(session) = self.local_session(headers).await? else {
                return Ok(None);
            };
            if session.account.must_change_password {
                return Ok(None);
            };
            let Some(session_token_hash) = local_session_hash(headers) else {
                return Ok(None);
            };
            return Ok(Some(Principal {
                identity: session.identity,
                expires_at: Instant::now() + Duration::from_secs(session.remaining_seconds),
                session_token_hash,
                scope: PrincipalScope::Member,
            }));
        }
        if self
            .database
            .local_instance_initialized()
            .await
            .map_err(|_| DatabaseError::SchemaNotReady)?
        {
            return Ok(None);
        }
        let Some(token) = session_token(headers) else {
            return Ok(None);
        };
        Ok(self
            .database
            .find_development_session(token_hash(token))
            .await?
            .map(|session| Principal {
                identity: session.identity,
                expires_at: Instant::now() + Duration::from_secs(session.remaining_seconds),
                session_token_hash: token_hash(token),
                scope: PrincipalScope::Member,
            }))
    }

    pub async fn authenticate_share(
        &self,
        headers: &HeaderMap,
        link_id: uuid::Uuid,
    ) -> Result<Option<Principal>, DatabaseError> {
        let Some(token) = share_session_token(headers) else {
            return Ok(None);
        };
        let hash = token_hash(token);
        Ok(self
            .database
            .guest_session(link_id, hash)
            .await
            .map_err(|_| DatabaseError::SchemaNotReady)?
            .map(|session| Principal {
                identity: session.identity,
                expires_at: Instant::now() + Duration::from_millis(session.remaining_millis),
                session_token_hash: hash,
                scope: PrincipalScope::ShareGuest {
                    session,
                    token_hash: hash,
                },
            }))
    }

    pub async fn active(&self, principal: &Principal) -> Result<bool, DatabaseError> {
        if Instant::now() >= principal.expires_at {
            return Ok(false);
        }
        match &principal.scope {
            PrincipalScope::Member if !self.enabled => Ok(self
                .database
                .local_session(principal.session_token_hash)
                .await
                .map_err(|_| DatabaseError::SchemaNotReady)?
                .is_some_and(|session| {
                    session.identity == principal.identity && !session.account.must_change_password
                })),
            PrincipalScope::Member => {
                if self
                    .database
                    .local_instance_initialized()
                    .await
                    .map_err(|_| DatabaseError::SchemaNotReady)?
                {
                    return Ok(false);
                };
                Ok(self
                    .database
                    .find_development_session(principal.session_token_hash)
                    .await?
                    .is_some_and(|session| session.identity == principal.identity))
            }
            PrincipalScope::ShareGuest {
                session,
                token_hash,
            } => Ok(self
                .database
                .guest_session(session.link_id, *token_hash)
                .await
                .is_ok_and(|current| {
                    current.is_some_and(|current| {
                        current.session_id == session.session_id
                            && current.identity == principal.identity
                    })
                })),
        }
    }
}

#[cfg(feature = "dev-auth")]
fn cookie(token: &str, remaining: u64) -> String {
    format!("pxel_dev_session={token}; HttpOnly; SameSite=Strict; Path=/api; Max-Age={remaining}")
}

pub(super) fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

pub(super) fn local_session_hash(headers: &HeaderMap) -> Option<[u8; 32]> {
    let mut values = headers.get_all(header::COOKIE).iter();
    let cookies = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    };
    let mut tokens = cookies
        .split(';')
        .filter_map(|cookie| cookie.trim().strip_prefix("pxel_session="));
    let token = tokens.next()?;
    (tokens.next().is_none() && token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| token_hash(token))
}

pub(super) fn local_cookie(token: &str) -> String {
    format!("pxel_session={token}; HttpOnly; Secure; SameSite=Strict; Path=/api; Max-Age=28800")
}

fn session_token(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(header::COOKIE).iter();
    let cookies = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let mut tokens = cookies
        .split(';')
        .filter_map(|cookie| cookie.trim().strip_prefix("pxel_dev_session="));
    let token = tokens.next()?;
    (tokens.next().is_none() && token.len() == 32 && token.bytes().all(|b| b.is_ascii_hexdigit()))
        .then_some(token)
}

fn share_session_token(headers: &HeaderMap) -> Option<&str> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE).iter() {
        let cookies = value.to_str().ok()?;
        for cookie in cookies.split(';') {
            if let Some(token) = cookie.trim().strip_prefix("pxel_share_session=") {
                if found.is_some()
                    || token.len() != 64
                    || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return None;
                }
                found = Some(token);
            }
        }
    }
    found
}

pub fn contains_share_cookie(headers: &HeaderMap) -> bool {
    headers.get_all(header::COOKIE).iter().any(|value| {
        value.to_str().is_ok_and(|cookies| {
            cookies
                .split(';')
                .any(|cookie| cookie.trim().starts_with("pxel_share_session="))
        })
    })
}

fn share_cookie(token: &str, link_id: uuid::Uuid, remaining: u64, secure: bool) -> String {
    format!(
        "pxel_share_session={token}; HttpOnly{}; SameSite=Strict; Path=/api/shared/{link_id}; Max-Age={remaining}",
        if secure { "; Secure" } else { "" }
    )
}

pub fn valid_origin(headers: &HeaderMap, allowed: &[String]) -> bool {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let Some(value) = origins.next() else {
        return false;
    };
    origins.next().is_none()
        && value
            .to_str()
            .is_ok_and(|origin| allowed.iter().any(|candidate| candidate == origin))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    #[test]
    fn local_cookie_parser_rejects_ambiguous_or_malformed_bearers()
    -> Result<(), Box<dyn std::error::Error>> {
        let token = "a".repeat(64);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("other=1; pxel_session={token}"))?,
        );
        assert_eq!(local_session_hash(&headers), Some(token_hash(&token)));
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("pxel_session={token}; pxel_session={token}"))?,
        );
        assert!(local_session_hash(&headers).is_none());
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("pxel_session={token}"))?,
        );
        headers.append(header::COOKIE, HeaderValue::from_static("other=1"));
        assert!(local_session_hash(&headers).is_none());
        headers.clear();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("pxel_session=short"),
        );
        assert!(local_session_hash(&headers).is_none());
        Ok(())
    }
    #[test]
    fn guest_cookie_is_scoped_and_secure_outside_development() {
        let link = uuid::Uuid::now_v7();
        let production = share_cookie("a".repeat(64).as_str(), link, 120, true);
        let development = share_cookie("a".repeat(64).as_str(), link, 120, false);
        assert!(production.contains("; Secure;"));
        assert!(development.contains("HttpOnly; SameSite=Strict;"));
        assert!(!development.contains("; Secure;"));
        assert!(production.contains(&format!("Path=/api/shared/{link};")));
    }
    #[test]
    fn origin_requires_exact_single_match() {
        let allowed = vec!["http://localhost:5173".into()];
        let mut headers = HeaderMap::new();
        assert!(!valid_origin(&headers, &allowed));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:5173.evil.test"),
        );
        assert!(!valid_origin(&headers, &allowed));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:5173"),
        );
        assert!(valid_origin(&headers, &allowed));
        headers.append(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:5173"),
        );
        assert!(!valid_origin(&headers, &allowed));
    }

    #[tokio::test]
    async fn disabled_auth_cannot_issue_a_principal() -> Result<(), Box<dyn std::error::Error>> {
        let database = PgDatabase::connect(&std::env::var("DATABASE_URL")?, 2).await?;
        let auth = Auth::new(false, database);
        assert!(auth.authenticate(&HeaderMap::new()).await?.is_none());
        #[cfg(feature = "dev-auth")]
        auth.revoke(&HeaderMap::new()).await?;
        Ok(())
    }

    #[cfg(feature = "dev-auth")]
    #[tokio::test]
    async fn reconnect_reuses_session_and_duplicate_cookie_is_rejected()
    -> Result<(), Box<dyn std::error::Error>> {
        let database = PgDatabase::connect(&std::env::var("DATABASE_URL")?, 2).await?;
        let auth = Auth::new(true, database.clone());
        let value = auth
            .issue(&HeaderMap::new())
            .await?
            .ok_or("session missing")?;
        let value = value.split(';').next().ok_or("cookie missing")?;
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_str(value)?);
        let cleanup_headers = headers.clone();
        let result: Result<(), Box<dyn std::error::Error>> = async {
            let principal = auth
                .authenticate(&headers)
                .await?
                .ok_or("principal missing")?;
            let identity = principal.identity;
            if !auth.active(&principal).await? {
                return Err("issued session is not active".into());
            }
            for _ in 0..256 {
                let repeated = auth.issue(&headers).await?.ok_or("session missing")?;
                if repeated.split(';').next() != Some(value) {
                    return Err("session token changed".into());
                }
                if auth
                    .authenticate(&headers)
                    .await?
                    .ok_or("principal missing")?
                    .identity
                    != identity
                {
                    return Err("session identity changed".into());
                }
            }
            let restarted = Auth::new(true, database);
            if restarted
                .authenticate(&headers)
                .await?
                .ok_or("restarted identity missing")?
                .identity
                != identity
            {
                return Err("restart changed identity".into());
            }
            headers.append(header::COOKIE, HeaderValue::from_str(value)?);
            if auth.authenticate(&headers).await?.is_some() {
                return Err("duplicate cookie accepted".into());
            }
            headers.remove(header::COOKIE);
            headers.insert(header::COOKIE, HeaderValue::from_str(value)?);
            auth.revoke(&headers).await?;
            if auth.active(&principal).await? {
                return Err("revoked socket principal remains active".into());
            }
            if auth.authenticate(&headers).await?.is_some() {
                return Err("revoked session accepted".into());
            }
            auth.revoke(&headers).await?;
            Ok(())
        }
        .await;
        let cleanup = auth.revoke(&cleanup_headers).await;
        result?;
        cleanup?;
        Ok(())
    }
}
