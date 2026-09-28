use std::{net::SocketAddr, path::PathBuf};

pub struct Config {
    pub database_url: String,
    pub address: SocketAddr,
    pub blob_root: PathBuf,
    pub allowed_origins: Vec<String>,
    pub dev_auth: bool,
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
        Ok(Self {
            database_url,
            address,
            allowed_origins,
            dev_auth,
            blob_root: std::env::var_os("PXEL_BLOB_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".local/blobs")),
        })
    }
}
