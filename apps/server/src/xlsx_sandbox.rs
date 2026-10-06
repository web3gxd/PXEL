//! Operator-owned artifact pins; never accepts runtime arguments from requests.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;

#[derive(Debug, thiserror::Error)]
#[error("XLSX sandbox configuration is invalid or unavailable")]
pub struct SandboxError;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    runtime_version: String,
    runtime_path: PathBuf,
    runtime_sha256: String,
    module_path: PathBuf,
    module_sha256: String,
}

pub struct Sandbox {
    runtime: PathBuf,
    module: PathBuf,
    _runtime_guard: File,
    _module_guard: File,
}

fn open_artifact(
    path: &Path,
    digest: &str,
    maximum: u64,
    wasm: bool,
) -> Result<File, SandboxError> {
    if !path.is_absolute()
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(SandboxError);
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| SandboxError)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() == 0
        || metadata.len() > maximum
    {
        return Err(SandboxError);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(SandboxError);
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1); // FILE_SHARE_READ: keep write/delete denied.
    }
    let mut file = options.open(path).map_err(|_| SandboxError)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0_u64;
    if wasm {
        let mut header = [0_u8; 8];
        file.read_exact(&mut header).map_err(|_| SandboxError)?;
        if &header != b"\0asm\x01\0\0\0" {
            return Err(SandboxError);
        }
        hash.update(header);
        size = 8;
    }
    loop {
        let count = file.read(&mut buffer).map_err(|_| SandboxError)?;
        if count == 0 {
            break;
        }
        size = size.checked_add(count as u64).ok_or(SandboxError)?;
        if size > maximum {
            return Err(SandboxError);
        }
        hash.update(&buffer[..count]);
    }
    let actual: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if size == 0 || actual != digest {
        return Err(SandboxError);
    }
    Ok(file)
}

fn read_manifest(path: &Path) -> Result<Sandbox, SandboxError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| SandboxError)?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err(SandboxError);
    }
    let file = File::open(path).map_err(|_| SandboxError)?;
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SandboxError)?;
    if bytes.len() > 64 * 1024 {
        return Err(SandboxError);
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| SandboxError)?;
    if manifest.schema_version != 1 || manifest.runtime_version != "49.0.2" {
        return Err(SandboxError);
    }
    let runtime_guard = open_artifact(
        &manifest.runtime_path,
        &manifest.runtime_sha256,
        256 * 1024 * 1024,
        false,
    )?;
    let module_guard = open_artifact(
        &manifest.module_path,
        &manifest.module_sha256,
        32 * 1024 * 1024,
        true,
    )?;
    Ok(Sandbox {
        runtime: manifest.runtime_path,
        module: manifest.module_path,
        _runtime_guard: runtime_guard,
        _module_guard: module_guard,
    })
}

fn clean_command(path: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(path);
    command
        .env_clear()
        .current_dir(std::env::temp_dir())
        .kill_on_drop(true);
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", root);
    }
    command
}

impl Sandbox {
    pub async fn load(path: PathBuf) -> Result<Self, SandboxError> {
        let sandbox = tokio::task::spawn_blocking(move || read_manifest(&path))
            .await
            .map_err(|_| SandboxError)??;
        let mut command = clean_command(&sandbox.runtime);
        command
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|_| SandboxError)?;
        let stdout = child.stdout.take().ok_or(SandboxError)?;
        let mut version = Vec::new();
        let status = tokio::time::timeout(Duration::from_secs(2), async {
            stdout
                .take(513)
                .read_to_end(&mut version)
                .await
                .map_err(|_| SandboxError)?;
            child.wait().await.map_err(|_| SandboxError)
        })
        .await
        .map_err(|_| SandboxError)??;
        if !status.success() || version.len() > 512 || !version.starts_with(b"wasmtime 49.0.2 ") {
            return Err(SandboxError);
        }
        Ok(sandbox)
    }

    pub fn command(&self) -> tokio::process::Command {
        let mut command = clean_command(&self.runtime);
        command.args([
            "run",
            "-C",
            "cache=n",
            "-W",
            "fuel=500000000",
            "-W",
            "max-memory-size=268435456",
            "-W",
            "timeout=5s",
            "-S",
            "inherit-env=n",
            "-S",
            "inherit-network=n",
            "-S",
            "tcp=n",
            "-S",
            "udp=n",
            "-S",
            "http=n",
        ]);
        command.arg(&self.module);
        command
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn verified_artifact_blocks_modification_until_guard_is_released()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("module.wasm");
        let bytes = b"\0asm\x01\0\0\0";
        std::fs::write(&path, bytes)?;
        let digest: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let guard = open_artifact(&path, &digest, 1024, true)?;
        assert!(OpenOptions::new().write(true).open(&path).is_err());
        assert!(std::fs::remove_file(&path).is_err());
        drop(guard);
        assert!(OpenOptions::new().write(true).open(&path).is_ok());
        std::fs::remove_file(&path)?;
        Ok(())
    }
}
