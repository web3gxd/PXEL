//! Standalone v1 file DTO and authenticated envelope; no wire/persisted DTOs.
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    XChaCha20Poly1305,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

pub const MAX_PAYLOAD: usize = 32 * 1024 * 1024;
pub const MAX_FILE: usize = MAX_PAYLOAD + 72;
const MAGIC: &[u8; 8] = b"PXEL\r\n\x1a\n";
const HEADER: usize = 56;
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("invalid PXEL file")]
    Invalid,
    #[error("unsupported PXEL file version")]
    Version,
    #[error("PXEL file limit exceeded")]
    Limit,
    #[error("invalid PXEL password length")]
    Password,
    #[error("wrong password or damaged PXEL file")]
    Authentication,
    #[error("PXEL encryption unavailable")]
    Unavailable,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Book {
    pub schema_version: u32,
    pub engine_version: String,
    pub compatibility_profile: String,
    pub title: String,
    pub anonymized: bool,
    pub sheets: Vec<Sheet>,
    pub settings: Settings,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sheet {
    pub name: String,
    pub rows: u32,
    pub columns: u32,
    pub frozen_rows: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<SheetViewSettings>,
    pub cells: Vec<Cell>,
    pub styles: Vec<Style>,
    pub row_heights: Vec<Size>,
    pub column_widths: Vec<Size>,
    pub merges: Vec<Merge>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SheetViewSettings {
    pub hidden: bool,
    pub default_column_width: u32,
    pub tab_color: Option<u32>,
    pub hidden_rows: Vec<u32>,
    pub hidden_columns: Vec<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    pub row: u32,
    pub column: u32,
    pub content: Content,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Content {
    Blank,
    Number(f64),
    Text(String),
    Boolean(bool),
    Formula(String),
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Style {
    pub row: u32,
    pub column: u32,
    pub properties: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Size {
    pub position: u32,
    pub size_css_px: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Merge {
    pub first_row: u32,
    pub last_row: u32,
    pub first_column: u32,
    pub last_column: u32,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub filters: Vec<Filter>,
    pub hidden_hardcode_warnings: Vec<Location>,
    pub recognition_enabled: bool,
    pub recognition_types: Vec<String>,
    pub ignored_sensitive_cells: Vec<SensitiveLocation>,
    pub omitted_stale_filters: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub sheet: u32,
    pub row: u32,
    pub column: u32,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensitiveLocation {
    pub location: Location,
    pub kind: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub sheet: u32,
    pub first_row: u32,
    pub last_row: u32,
    pub first_column: u32,
    pub last_column: u32,
    pub header_row: Option<u32>,
    pub conditions: Vec<Condition>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub column: u32,
    pub predicate: spreadsheet_storage::FilterPredicate,
}

pub fn password(secret: &str) -> Result<(), FileError> {
    if !(12..=128).contains(&secret.chars().count()) {
        Err(FileError::Password)
    } else {
        Ok(())
    }
}
fn derive(secret: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, FileError> {
    password(secret)?;
    let params = Params::new(65536, 3, 1, Some(32)).map_err(|_| FileError::Unavailable)?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(secret.as_bytes(), salt, &mut *key)
        .map_err(|_| FileError::Unavailable)?;
    Ok(key)
}
pub fn encrypt(book: &Book, secret: &str) -> Result<Vec<u8>, FileError> {
    password(secret)?;
    let plain = Zeroizing::new(serde_json::to_vec(book).map_err(|_| FileError::Invalid)?);
    if plain.len() > MAX_PAYLOAD {
        return Err(FileError::Limit);
    }
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 24];
    getrandom::fill(&mut salt).map_err(|_| FileError::Unavailable)?;
    getrandom::fill(&mut nonce).map_err(|_| FileError::Unavailable)?;
    let key = derive(secret, &salt)?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    let mut header = Vec::with_capacity(HEADER);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&1u16.to_be_bytes());
    header.extend_from_slice(&[1, 0]);
    header.extend_from_slice(&(plain.len() as u32).to_be_bytes());
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce);
    let encrypted = cipher
        .encrypt(
            (&nonce).into(),
            Payload {
                msg: &plain,
                aad: &header,
            },
        )
        .map_err(|_| FileError::Unavailable)?;
    header.extend_from_slice(&encrypted);
    Ok(header)
}
pub fn decrypt(file: &[u8], secret: &str) -> Result<Book, FileError> {
    password(secret)?;
    if file.len() > MAX_FILE {
        return Err(FileError::Limit);
    }
    if file.len() < HEADER + 16 || file.get(..8) != Some(MAGIC.as_slice()) {
        return Err(FileError::Invalid);
    }
    if file.get(8..10) != Some(1u16.to_be_bytes().as_slice()) || file[10] != 1 || file[11] != 0 {
        return Err(FileError::Version);
    }
    let length =
        u32::from_be_bytes(file[12..16].try_into().map_err(|_| FileError::Invalid)?) as usize;
    if length > MAX_PAYLOAD {
        return Err(FileError::Limit);
    }
    if length + HEADER + 16 != file.len() {
        return Err(FileError::Invalid);
    }
    let key = derive(secret, &file[16..32])?;
    let cipher = XChaCha20Poly1305::new((&*key).into());
    let nonce: &[u8; 24] = file[32..56].try_into().map_err(|_| FileError::Invalid)?;
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                nonce.into(),
                Payload {
                    msg: &file[HEADER..],
                    aad: &file[..HEADER],
                },
            )
            .map_err(|_| FileError::Authentication)?,
    );
    let book: Book = serde_json::from_slice(&plain).map_err(|_| FileError::Invalid)?;
    if !matches!(book.schema_version, 1 | 2) {
        return Err(FileError::Version);
    }
    Ok(book)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unique_salt_nonce_unicode_password_integrity_and_version() -> Result<(), FileError> {
        let book = Book {
            schema_version: 1,
            engine_version: "engine".into(),
            compatibility_profile: "profile".into(),
            title: "Private fixture".into(),
            anonymized: false,
            sheets: vec![],
            settings: Settings::default(),
        };
        let secret = "  пароль 🔑 é  ";
        let first = encrypt(&book, secret)?;
        let second = encrypt(&book, secret)?;
        assert_ne!(&first[16..32], &second[16..32]);
        assert_ne!(&first[32..56], &second[32..56]);
        assert_eq!(decrypt(&first, secret)?.title, book.title);
        assert!(matches!(
            decrypt(&first, "  пароль 🔑 é  "),
            Err(FileError::Authentication)
        ));
        assert!(matches!(
            decrypt(&first, "wrong password 12"),
            Err(FileError::Authentication)
        ));
        let mut damaged = first.clone();
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        assert!(matches!(
            decrypt(&damaged, secret),
            Err(FileError::Authentication)
        ));
        let mut version = first.clone();
        version[9] = 2;
        assert!(matches!(decrypt(&version, secret), Err(FileError::Version)));
        let mut header = first.clone();
        header[20] ^= 1;
        assert!(matches!(
            decrypt(&header, secret),
            Err(FileError::Authentication)
        ));
        assert!(password(&"😀".repeat(12)).is_ok());
        assert!(password(&"я".repeat(128)).is_ok());
        assert!(password(&"x".repeat(11)).is_err());
        assert!(password(&"x".repeat(129)).is_err());
        assert!(
            !first
                .windows(book.title.len())
                .any(|s| s == book.title.as_bytes())
        );
        assert!(matches!(
            decrypt(&first[..first.len() - 1], secret),
            Err(FileError::Invalid)
        ));
        Ok(())
    }
}
