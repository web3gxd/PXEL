//! Key management port. Adapters own KEKs; storage owns durable nonce allocation.

use thiserror::Error;
use zeroize::Zeroizing;

#[derive(Debug, Error, Eq, PartialEq)]
pub enum KeyProviderError {
    #[error("key provider configuration is invalid")]
    InvalidConfiguration,
    #[error("the requested encryption key is unavailable")]
    KeyUnavailable,
    #[error("secure randomness is unavailable")]
    RandomnessUnavailable,
    #[error("wrapped key authentication failed")]
    AuthenticationFailed,
    #[error("key provider is unavailable")]
    ProviderUnavailable,
}

/// A synchronous KEK adapter. Raw KEKs never cross this boundary.
///
/// The caller must durably reserve `nonce` under `physical_key_id` before
/// wrapping. The physical identity must remain equal for equal key material,
/// including when the same KEK is configured under a different logical ID.
/// Wrapped keys contain exactly a 32-byte DEK plus an authentication tag.
pub trait KeyProvider: Send + Sync {
    fn active_kek_id(&self) -> &str;

    fn physical_key_id(&self, kek_id: &str) -> Result<[u8; 32], KeyProviderError>;

    fn wrap_key(
        &self,
        kek_id: &str,
        nonce: &[u8; 24],
        aad: &[u8],
        dek: &[u8; 32],
    ) -> Result<Vec<u8>, KeyProviderError>;

    fn unwrap_key(
        &self,
        kek_id: &str,
        nonce: &[u8; 24],
        aad: &[u8],
        wrapped: &[u8],
    ) -> Result<Zeroizing<[u8; 32]>, KeyProviderError>;
}
