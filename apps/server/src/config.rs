use std::{net::SocketAddr, num::NonZeroU64, path::PathBuf, time::Duration};

pub struct Config {
    pub database_url: String,
    pub address: SocketAddr,
    pub blob_root: PathBuf,
    pub allowed_origins: Vec<String>,
    pub dev_auth: bool,
    pub bootstrap_token_hash: Option<[u8; 32]>,
    pub snapshot_interval: NonZeroU64,
    pub snapshot_cleanup_retention: Duration,
    pub snapshot_active_retention: Duration,
    #[cfg(feature = "s3")]
    pub s3: Option<S3Settings>,
}

#[cfg(feature = "s3")]
pub struct S3Settings {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let database_url = match std::env::var("DATABASE_URL") {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => {
                let path = std::env::var("PXEL_DATABASE_URL_FILE")
                    .unwrap_or_else(|_| ".local/database-url".into());
                std::fs::read_to_string(path)
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "DATABASE_URL or a readable PXEL_DATABASE_URL_FILE is required"
                        )
                    })?
                    .trim()
                    .to_owned()
            }
            Err(_) => anyhow::bail!("DATABASE_URL must be Unicode"),
        };
        if database_url.is_empty() {
            anyhow::bail!("empty database configuration");
        }
        let address: SocketAddr = std::env::var("PXEL_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8080".into())
            .parse()?;
        let dev_auth = std::env::var("PXEL_DEV_AUTH").as_deref() == Ok("1");
        if dev_auth && !cfg!(feature = "dev-auth") {
            anyhow::bail!("development authentication is not compiled in");
        }
        if dev_auth && !address.ip().is_loopback() {
            anyhow::bail!("development authentication requires a loopback bind");
        }
        let bootstrap_token_hash = match std::env::var_os("PXEL_BOOTSTRAP_TOKEN_FILE") {
            None => None,
            Some(path) => Some(read_bootstrap_token(std::path::Path::new(&path))?),
        };
        let origins = std::env::var("PXEL_ALLOWED_ORIGINS").unwrap_or_else(|_|
            "http://localhost:5173,http://127.0.0.1:5173,http://localhost:8080,http://127.0.0.1:8080".into());
        let allowed_origins: Vec<String> = origins
            .split(',')
            .map(str::trim)
            .map(str::to_owned)
            .collect();
        if allowed_origins
            .iter()
            .any(|origin| origin.is_empty() || origin == "*" || origin.ends_with('/'))
        {
            anyhow::bail!(
                "PXEL_ALLOWED_ORIGINS requires exact nonempty origins without trailing slash"
            );
        }
        let raw_interval = match std::env::var("PXEL_SNAPSHOT_INTERVAL") {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => "1000".into(),
            Err(_) => anyhow::bail!("PXEL_SNAPSHOT_INTERVAL must be Unicode"),
        };
        let snapshot_interval = parse_snapshot_interval(&raw_interval)?;
        let cleanup_hours = match std::env::var("PXEL_SNAPSHOT_CLEANUP_HOURS") {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => "24".into(),
            Err(_) => anyhow::bail!("PXEL_SNAPSHOT_CLEANUP_HOURS must be Unicode"),
        };
        let snapshot_cleanup_retention = parse_snapshot_cleanup_hours(&cleanup_hours)?;
        let active_days = match std::env::var("PXEL_SNAPSHOT_ACTIVE_RETENTION_DAYS") {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => "30".into(),
            Err(_) => anyhow::bail!("PXEL_SNAPSHOT_ACTIVE_RETENTION_DAYS must be Unicode"),
        };
        let snapshot_active_retention = parse_snapshot_active_days(&active_days)?;
        #[cfg(feature = "s3")]
        let s3 = match std::env::var("PXEL_S3_ENDPOINT") {
            Ok(endpoint) => Some(S3Settings {
                endpoint,
                bucket: std::env::var("PXEL_S3_BUCKET")
                    .map_err(|_| anyhow::anyhow!("PXEL_S3_BUCKET is required"))?,
                region: std::env::var("PXEL_S3_REGION")
                    .map_err(|_| anyhow::anyhow!("PXEL_S3_REGION is required"))?,
            }),
            Err(std::env::VarError::NotPresent) => None,
            Err(_) => anyhow::bail!("PXEL_S3_ENDPOINT must be Unicode"),
        };
        #[cfg(not(feature = "s3"))]
        if std::env::var_os("PXEL_S3_ENDPOINT").is_some() {
            anyhow::bail!("PXEL_S3_ENDPOINT requires the s3 build feature");
        }
        Ok(Self {
            database_url,
            address,
            allowed_origins,
            dev_auth,
            bootstrap_token_hash,
            snapshot_interval,
            snapshot_cleanup_retention,
            snapshot_active_retention,
            #[cfg(feature = "s3")]
            s3,
            blob_root: std::env::var_os("PXEL_BLOB_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".local/blobs")),
        })
    }
}

fn read_bootstrap_token(path: &std::path::Path) -> anyhow::Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = crate::storage_maintenance::private_secret_file(path).map_err(|_| {
        anyhow::anyhow!("bootstrap token file must be a private regular file with one link")
    })?;
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    file.by_ref()
        .take(66)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("bootstrap token file is unreadable"))?;
    // Exactly the tool's 64 lowercase hexadecimal bytes, with optional LF.
    let token = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    if token.len() != 64
        || !token
            .iter()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    {
        anyhow::bail!("bootstrap token file must contain 64 lowercase hexadecimal characters");
    }
    Ok(Sha256::digest(token).into())
}

fn parse_snapshot_active_days(raw: &str) -> anyhow::Result<Duration> {
    let days = raw.parse::<u64>()?;
    if !(1..=3650).contains(&days) {
        anyhow::bail!("PXEL_SNAPSHOT_ACTIVE_RETENTION_DAYS must be between 1 and 3650");
    }
    Ok(Duration::from_secs(days * 86_400))
}

fn parse_snapshot_cleanup_hours(raw: &str) -> anyhow::Result<Duration> {
    let hours = raw.parse::<u64>()?;
    if !(1..=8760).contains(&hours) {
        anyhow::bail!("PXEL_SNAPSHOT_CLEANUP_HOURS must be between 1 and 8760");
    }
    Ok(Duration::from_secs(hours * 3600))
}

fn parse_snapshot_interval(raw: &str) -> anyhow::Result<NonZeroU64> {
    let interval = raw.parse::<u64>()?;
    if !(1..=100_000).contains(&interval) {
        anyhow::bail!("PXEL_SNAPSHOT_INTERVAL must be between 1 and 100000");
    }
    NonZeroU64::new(interval).ok_or_else(|| anyhow::anyhow!("invalid snapshot interval"))
}

#[cfg(test)]
mod tests {
    use super::{
        parse_snapshot_active_days, parse_snapshot_cleanup_hours, parse_snapshot_interval,
    };

    #[test]
    fn snapshot_interval_has_a_bounded_nonzero_configuration() {
        assert!(matches!(parse_snapshot_interval("1"), Ok(value) if value.get() == 1));
        assert!(matches!(parse_snapshot_interval("100000"), Ok(value) if value.get() == 100_000));
        for value in ["0", "100001", "not-a-number", "-1"] {
            assert!(parse_snapshot_interval(value).is_err());
        }
    }

    #[test]
    fn snapshot_cleanup_retention_has_a_bounded_configuration() {
        assert!(
            matches!(parse_snapshot_cleanup_hours("24"), Ok(value) if value.as_secs() == 86_400)
        );
        for value in ["0", "8761", "invalid"] {
            assert!(parse_snapshot_cleanup_hours(value).is_err());
        }
    }

    #[test]
    fn active_snapshot_retention_has_a_bounded_configuration() {
        assert!(
            matches!(parse_snapshot_active_days("30"), Ok(value) if value.as_secs() == 2_592_000)
        );
        for value in ["0", "3651", "invalid"] {
            assert!(parse_snapshot_active_days(value).is_err());
        }
    }
}
