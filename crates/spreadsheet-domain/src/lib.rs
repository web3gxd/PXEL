//! Shared domain values. No persistence, transport, or evaluation dependencies.

pub mod axis_edit;
pub mod axis_history;
pub mod formula;
pub mod lifecycle;
pub mod move_history;
pub mod style;
pub mod workbook;

use std::fmt;
use thiserror::Error;

/// An opaque single-component blob identity, never a filesystem path.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BlobKey(String);

impl BlobKey {
    pub fn parse(value: &str) -> Result<Self, InvalidBlobKey> {
        let is_checksum = value.len() == 64
            && value
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
        let is_uuid = value.len() == 36
            && value.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    byte == b'-'
                } else {
                    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
                }
            });
        if is_checksum || is_uuid {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidBlobKey)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
#[error("blob key must be a canonical lowercase UUID or SHA-256 hex identity")]
pub struct InvalidBlobKey;

/// SHA-256 of the blob body, excluding the storage adapter envelope.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Checksum([u8; 32]);

impl Checksum {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for Checksum {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Checksum(SHA-256)")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlobMetadata {
    pub size_bytes: u64,
    pub checksum: Checksum,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredBlob {
    pub key: BlobKey,
    pub metadata: BlobMetadata,
}

#[cfg(test)]
mod tests {
    use super::BlobKey;

    #[test]
    fn keys_reject_traversal_paths_reserved_names_and_noncanonical_identities() {
        for candidate in [
            "",
            ".",
            "..",
            "../secret",
            "a/b",
            "a\\b",
            "C:\\secret",
            "CON",
            "NUL",
            "01990000-0000-7000-8000-00000000000A",
            "\0",
        ] {
            assert!(BlobKey::parse(candidate).is_err());
        }
        assert!(BlobKey::parse(&"g".repeat(64)).is_err());
        assert!(BlobKey::parse(&"a".repeat(64)).is_ok());
        assert!(BlobKey::parse("01990000-0000-7000-8000-000000000001").is_ok());
    }
}

pub mod style_write;
