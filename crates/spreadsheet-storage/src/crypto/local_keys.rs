use super::physical_key_id;
use chacha20poly1305::{
    XChaCha20Poly1305,
    aead::{Aead, KeyInit, Payload},
};
use serde::{Deserialize, Deserializer, de::Visitor};
use spreadsheet_application::keys::{KeyProvider, KeyProviderError};
use std::{collections::BTreeMap, ffi::OsString, fmt, fs::OpenOptions, io::Read, path::Path};
use zeroize::Zeroizing;

const MAX_KEYRING_BYTES: usize = 64 * 1024;
const MAX_KEYS: usize = 32;

#[cfg(windows)]
fn private_file_acl(path: &Path) -> Result<(), KeyProviderError> {
    use std::os::windows::process::CommandExt;
    // The path is an environment value, never interpolated into shell code.
    // Native ACL access would require unsafe code; use the OS PowerShell API.
    let script = r#"$ErrorActionPreference='Stop'; try {
      $acl=Get-Acl -LiteralPath $env:PXEL_KEYRING_ACL_PATH;
      $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value;
      $allowed=@($sid,'S-1-5-18','S-1-5-32-544');
      $owner=$acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value;
      if ($allowed -notcontains $owner) { exit 1 };
      foreach($rule in $acl.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])) {
        if ($rule.AccessControlType -eq [System.Security.AccessControl.AccessControlType]::Allow -and $allowed -notcontains $rule.IdentityReference.Value) { exit 1 }
      };
      exit 0
    } catch { exit 1 }"#;
    let executable = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .ok_or(KeyProviderError::ProviderUnavailable)?
        .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let status = std::process::Command::new(executable)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(0x0800_0000)
        .env_remove("PSModulePath")
        .env("PXEL_KEYRING_ACL_PATH", path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| KeyProviderError::ProviderUnavailable)?;
    if !status.success() {
        return Err(KeyProviderError::InvalidConfiguration);
    }
    Ok(())
}

/// Private explicit local KEK configuration. This type deliberately has no
/// Debug or serialization implementation, and never falls back to a new key.
pub struct LocalKeyProvider {
    active: String,
    keys: BTreeMap<String, Zeroizing<[u8; 32]>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Keyring {
    schema_version: u32,
    active_kek_id: String,
    keys: Vec<KeyEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyEntry {
    id: String,
    key_hex: SecretHex,
}

struct SecretHex(Zeroizing<String>);

impl<'de> Deserialize<'de> for SecretHex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HexVisitor;
        impl Visitor<'_> for HexVisitor {
            type Value = SecretHex;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a hexadecimal encryption key")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(SecretHex(Zeroizing::new(value.to_owned())))
            }

            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(SecretHex(Zeroizing::new(value)))
            }
        }
        deserializer.deserialize_string(HexVisitor)
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn decode_key(value: &str) -> Result<Zeroizing<[u8; 32]>, KeyProviderError> {
    if value.len() != 64 {
        return Err(KeyProviderError::InvalidConfiguration);
    }
    let mut key = Zeroizing::new([0; 32]);
    for (position, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let nibble = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        };
        let high = nibble(pair[0]).ok_or(KeyProviderError::InvalidConfiguration)?;
        let low = nibble(pair[1]).ok_or(KeyProviderError::InvalidConfiguration)?;
        key[position] = high * 16 + low;
    }
    // A zero KEK is an obvious configuration accident, not a usable secret.
    if key.iter().all(|byte| *byte == 0) {
        return Err(KeyProviderError::InvalidConfiguration);
    }
    Ok(key)
}

impl LocalKeyProvider {
    pub fn from_keyring_json(json: &str) -> Result<Self, KeyProviderError> {
        // ASCII IDs and hexadecimal secrets need no JSON escaping. Reject
        // escapes before parsing so serde_json never copies secret characters
        // into its own unzeroized string-decoding scratch buffer.
        if json.is_empty() || json.len() > MAX_KEYRING_BYTES || json.contains('\\') {
            return Err(KeyProviderError::InvalidConfiguration);
        }
        let input = Zeroizing::new(json.to_owned());
        let keyring: Keyring =
            serde_json::from_str(&input).map_err(|_| KeyProviderError::InvalidConfiguration)?;
        if keyring.schema_version != 1
            || !valid_id(&keyring.active_kek_id)
            || keyring.keys.is_empty()
            || keyring.keys.len() > MAX_KEYS
        {
            return Err(KeyProviderError::InvalidConfiguration);
        }
        let mut keys = BTreeMap::new();
        for entry in keyring.keys {
            if !valid_id(&entry.id)
                || keys
                    .insert(entry.id, decode_key(&entry.key_hex.0)?)
                    .is_some()
            {
                return Err(KeyProviderError::InvalidConfiguration);
            }
        }
        if !keys.contains_key(&keyring.active_kek_id) {
            return Err(KeyProviderError::InvalidConfiguration);
        }
        Ok(Self {
            active: keyring.active_kek_id,
            keys,
        })
    }

    pub fn from_file(path: &Path) -> Result<Self, KeyProviderError> {
        let metadata =
            std::fs::symlink_metadata(path).map_err(|_| KeyProviderError::InvalidConfiguration)?;
        if !metadata.is_file() || metadata.len() > MAX_KEYRING_BYTES as u64 {
            return Err(KeyProviderError::InvalidConfiguration);
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Keep a stable readable handle while ACL and bytes are checked.
            options.share_mode(1);
        }
        let file = options
            .open(path)
            .map_err(|_| KeyProviderError::ProviderUnavailable)?;
        let opened = file
            .metadata()
            .map_err(|_| KeyProviderError::ProviderUnavailable)?;
        if !opened.is_file() || opened.len() > MAX_KEYRING_BYTES as u64 {
            return Err(KeyProviderError::InvalidConfiguration);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if opened.permissions().mode() & 0o077 != 0
                || opened.dev() != metadata.dev()
                || opened.ino() != metadata.ino()
            {
                return Err(KeyProviderError::InvalidConfiguration);
            }
        }
        #[cfg(windows)]
        private_file_acl(path)?;
        let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_KEYRING_BYTES + 1));
        file.take(MAX_KEYRING_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| KeyProviderError::ProviderUnavailable)?;
        if bytes.len() > MAX_KEYRING_BYTES {
            return Err(KeyProviderError::InvalidConfiguration);
        }
        let input =
            std::str::from_utf8(&bytes).map_err(|_| KeyProviderError::InvalidConfiguration)?;
        Self::from_keyring_json(input)
    }

    /// Exactly one configuration source must be present, including on restart.
    pub fn from_environment() -> Result<Self, KeyProviderError> {
        Self::from_sources(
            std::env::var_os("PXEL_KEYRING_JSON"),
            std::env::var_os("PXEL_KEYRING_FILE"),
        )
    }

    pub(super) fn from_sources(
        inline: Option<OsString>,
        file: Option<OsString>,
    ) -> Result<Self, KeyProviderError> {
        // Guard even malformed or contradictory sources before branching.
        // This transfers the OS string's encoded allocation into the scrubbed
        // buffer without dropping an unguarded invalid-Unicode error payload.
        let inline = inline.map(|json| Zeroizing::new(json.into_encoded_bytes()));
        match (inline, file) {
            (Some(json), None) => {
                let json = std::str::from_utf8(&json)
                    .map_err(|_| KeyProviderError::InvalidConfiguration)?;
                Self::from_keyring_json(json)
            }
            (None, Some(path)) if !path.is_empty() => Self::from_file(Path::new(&path)),
            _ => Err(KeyProviderError::InvalidConfiguration),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Result<Self, KeyProviderError> {
        let mut keys = BTreeMap::new();
        keys.insert("test-kek".to_owned(), super::random_key()?);
        Ok(Self {
            active: "test-kek".to_owned(),
            keys,
        })
    }

    fn key(&self, id: &str) -> Result<&[u8; 32], KeyProviderError> {
        self.keys
            .get(id)
            .map(|material| &**material)
            .ok_or(KeyProviderError::KeyUnavailable)
    }
}

impl KeyProvider for LocalKeyProvider {
    fn active_kek_id(&self) -> &str {
        &self.active
    }

    fn physical_key_id(&self, kek_id: &str) -> Result<[u8; 32], KeyProviderError> {
        Ok(physical_key_id(self.key(kek_id)?))
    }

    fn wrap_key(
        &self,
        kek_id: &str,
        nonce: &[u8; 24],
        aad: &[u8],
        dek: &[u8; 32],
    ) -> Result<Vec<u8>, KeyProviderError> {
        XChaCha20Poly1305::new(self.key(kek_id)?.into())
            .encrypt(nonce.into(), Payload { msg: dek, aad })
            .map_err(|_| KeyProviderError::AuthenticationFailed)
    }

    fn unwrap_key(
        &self,
        kek_id: &str,
        nonce: &[u8; 24],
        aad: &[u8],
        wrapped: &[u8],
    ) -> Result<Zeroizing<[u8; 32]>, KeyProviderError> {
        if wrapped.len() != 48 {
            return Err(KeyProviderError::AuthenticationFailed);
        }
        let bytes = Zeroizing::new(
            XChaCha20Poly1305::new(self.key(kek_id)?.into())
                .decrypt(nonce.into(), Payload { msg: wrapped, aad })
                .map_err(|_| KeyProviderError::AuthenticationFailed)?,
        );
        let mut key = Zeroizing::new([0; 32]);
        if bytes.len() != key.len() {
            return Err(KeyProviderError::AuthenticationFailed);
        }
        key.copy_from_slice(&bytes);
        Ok(key)
    }
}
