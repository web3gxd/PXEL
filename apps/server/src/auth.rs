use axum::http::{HeaderMap, header};
use sha2::{Digest, Sha256};
use spreadsheet_application::journal::AuthenticatedActor;
#[cfg(feature = "dev-auth")]
use spreadsheet_domain::workbook::{ActorId, WorkspaceId};
use spreadsheet_storage::{DatabaseError, PgDatabase};
use std::time::{Duration, Instant};
#[cfg(feature = "dev-auth")]
use uuid::Uuid;

#[derive(Clone)]
pub struct Principal {
    pub identity: AuthenticatedActor,
    pub expires_at: Instant,
    session_token_hash: [u8; 32],
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
        if !self.enabled {
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
        let session = self
            .database
            .issue_development_account_session(token_hash(&token), actor, workspace, account)
            .await?;
        Ok(Some(cookie(&token, session.remaining_seconds)))
    }

    pub async fn authenticate(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<Principal>, DatabaseError> {
        if !self.enabled {
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
            }))
    }

    pub async fn active(&self, principal: &Principal) -> Result<bool, DatabaseError> {
        if Instant::now() >= principal.expires_at {
            return Ok(false);
        }
        Ok(self
            .database
            .find_development_session(principal.session_token_hash)
            .await?
            .is_some_and(|session| session.identity == principal.identity))
    }
}

#[cfg(feature = "dev-auth")]
fn cookie(token: &str, remaining: u64) -> String {
    format!("pxel_dev_session={token}; HttpOnly; SameSite=Strict; Path=/api; Max-Age={remaining}")
}

fn token_hash(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
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
