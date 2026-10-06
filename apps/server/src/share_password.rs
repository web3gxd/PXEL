//! Bounded password work; worker permits survive request cancellation.
use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version};
use std::{str::FromStr, sync::Arc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("password workers are busy")]
    Busy,
    #[error("password processing is unavailable")]
    Unavailable,
}

fn algorithm() -> Result<Argon2<'static>, PasswordError> {
    let params = Params::new(65536, 3, 1, Some(32)).map_err(|_| PasswordError::Unavailable)?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

fn hash(secret: &str) -> Result<Zeroizing<String>, PasswordError> {
    let mut salt = [0; 16];
    getrandom::fill(&mut salt).map_err(|_| PasswordError::Unavailable)?;
    let value: PasswordHash = algorithm()?
        .hash_password_with_salt(secret.as_bytes(), &salt)
        .map_err(|_| PasswordError::Unavailable)?;
    Ok(Zeroizing::new(value.to_string()))
}

pub struct PasswordWorkers {
    permits: Arc<Semaphore>,
    dummy: Zeroizing<String>,
}

impl PasswordWorkers {
    pub async fn new() -> Result<Self, PasswordError> {
        // Startup precedes HTTP admission. The random dummy verifier uses the
        // same parameters and work as an admitted invalid-link attempt.
        let dummy = tokio::task::spawn_blocking(|| hash("PXEL bounded dummy verification v1"))
            .await
            .map_err(|_| PasswordError::Unavailable)??;
        Ok(Self {
            permits: Arc::new(Semaphore::new(2)),
            dummy,
        })
    }
    pub fn admit(&self) -> Result<OwnedSemaphorePermit, PasswordError> {
        self.permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| PasswordError::Busy)
    }
    pub async fn hash(
        &self,
        secret: Zeroizing<String>,
        permit: OwnedSemaphorePermit,
    ) -> Result<Zeroizing<String>, PasswordError> {
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            hash(&secret)
        })
        .await
        .map_err(|_| PasswordError::Unavailable)?
    }
    pub async fn verify(
        &self,
        secret: Zeroizing<String>,
        phc: Option<Zeroizing<String>>,
        permit: OwnedSemaphorePermit,
    ) -> Result<bool, PasswordError> {
        let real = phc.is_some();
        let selected = phc.unwrap_or_else(|| self.dummy.clone());
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let parsed =
                PasswordHash::from_str(&selected).map_err(|_| PasswordError::Unavailable)?;
            // Stored credentials are independently validated on both creation
            // and authenticated decode, so untrusted PHC costs cannot grow here.
            let valid = algorithm()?
                .verify_password(secret.as_bytes(), &parsed)
                .is_ok();
            Ok(real && valid)
        })
        .await
        .map_err(|_| PasswordError::Unavailable)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn exact_parameters_unique_salts_and_no_implicit_secret_changes()
    -> Result<(), PasswordError> {
        let workers = PasswordWorkers::new().await?;
        let secret = "  пароль 🔑  ";
        let first = workers
            .hash(Zeroizing::new(secret.into()), workers.admit()?)
            .await?;
        let second = workers
            .hash(Zeroizing::new(secret.into()), workers.admit()?)
            .await?;
        assert!(first.starts_with("$argon2id$v=19$m=65536,t=3,p=1$"));
        assert!(
            spreadsheet_storage::ShareSecretPhc::new(
                spreadsheet_storage::ShareSecretKind::Pin,
                first.clone(),
            )
            .is_ok()
        );
        assert_ne!(*first, *second);
        assert!(
            workers
                .verify(
                    Zeroizing::new(secret.into()),
                    Some(first.clone()),
                    workers.admit()?
                )
                .await?
        );
        assert!(
            !workers
                .verify(
                    Zeroizing::new(secret.trim().into()),
                    Some(first),
                    workers.admit()?
                )
                .await?
        );
        assert!(
            !workers
                .verify(Zeroizing::new(secret.into()), None, workers.admit()?)
                .await?
        );
        Ok(())
    }
    #[tokio::test]
    async fn cancellation_does_not_release_a_running_worker_permit() -> Result<(), PasswordError> {
        let workers = PasswordWorkers::new().await?;
        let first = workers.admit()?;
        let second = workers.admit()?;
        assert!(matches!(workers.admit(), Err(PasswordError::Busy)));
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let detached = tokio::spawn(async move {
            tokio::task::spawn_blocking(move || {
                let _permit = first;
                let _ = started.send(());
                let _ = wait.recv();
            })
            .await
        });
        ready.await.map_err(|_| PasswordError::Unavailable)?;
        detached.abort();
        assert!(matches!(workers.admit(), Err(PasswordError::Busy)));
        let _ = release.send(());
        drop(second);
        Ok(())
    }
}
