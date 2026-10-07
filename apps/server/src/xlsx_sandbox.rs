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

fn resource_group_with_memory(bytes: u64) -> Result<processkit::ProcessGroup, SandboxError> {
    // Linux pids.max counts threads as well as processes; Wasmtime needs its
    // bounded compiler/epoch threads. Windows counts only live processes.
    let tasks = if cfg!(target_os = "linux") { 32 } else { 1 };
    processkit::ProcessGroup::with_options(
        processkit::ProcessGroupOptions::default()
            .max_memory(bytes)
            .max_processes(tasks)
            .cpu_quota(1.0),
    )
    .map_err(|_| SandboxError)
}

pub fn resource_group() -> Result<processkit::ProcessGroup, SandboxError> {
    resource_group_with_memory(512 * 1024 * 1024)
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
        let group = resource_group()?;
        let mut child = group.spawn(command).map_err(|_| SandboxError)?;
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
    fn host_resource_probe() -> Result<(), Box<dyn std::error::Error>> {
        match std::env::var("PXEL_HOST_RESOURCE_PROBE").as_deref() {
            Ok("allocate") => {
                use std::io::Write;
                println!("PXEL_ALLOCATION_READY");
                std::io::stdout().flush()?;
                let mut bytes = Vec::<u8>::new();
                bytes.try_reserve_exact(128 * 1024 * 1024)?;
                bytes.resize(128 * 1024 * 1024, 1);
                std::hint::black_box(&bytes);
                println!("PXEL_ALLOCATION_COMPLETE");
            }
            Ok("hold") => {
                use std::io::Write;
                println!("PXEL_RESOURCE_READY");
                std::io::stdout().flush()?;
                let mut buffer = [0_u8; 1];
                std::io::stdin().read_exact(&mut buffer)?;
            }
            Ok("busy") => {
                use std::io::Write;
                println!("PXEL_RESOURCE_READY");
                std::io::stdout().flush()?;
                let mut value = 1_u64;
                loop {
                    value = std::hint::black_box(value.wrapping_mul(17).wrapping_add(1));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn probe_command(mode: &str) -> Result<tokio::process::Command, std::io::Error> {
        let mut command = clean_command(&std::env::current_exe()?);
        command
            .args([
                "--exact",
                "xlsx_sandbox::tests::host_resource_probe",
                "--nocapture",
            ])
            .env("PXEL_HOST_RESOURCE_PROBE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        Ok(command)
    }

    #[tokio::test]
    async fn memory_cap_has_an_independent_positive_control()
    -> Result<(), Box<dyn std::error::Error>> {
        for (limit, expected) in [(64, false), (256, true)] {
            let group = resource_group_with_memory(limit * 1024 * 1024)?;
            let child = group.spawn(probe_command("allocate")?)?;
            let output =
                tokio::time::timeout(Duration::from_secs(10), child.wait_with_output()).await??;
            let stdout = std::str::from_utf8(&output.stdout)?;
            assert!(stdout.contains("PXEL_ALLOCATION_READY"));
            assert_eq!(stdout.contains("PXEL_ALLOCATION_COMPLETE"), expected);
            assert_eq!(output.status.success(), expected);
        }
        assert!(resource_group_with_memory(0).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn process_cap_and_container_drop_are_enforced() -> Result<(), Box<dyn std::error::Error>>
    {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let group = resource_group()?;
        let mut child = group.spawn(probe_command("hold")?)?;
        let stdout = child.stdout.take().ok_or("probe stdout unavailable")?;
        let mut reader = BufReader::new(stdout);
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await? == 0 {
                    return Err(std::io::Error::other("probe exited before readiness"));
                }
                if line.trim_end() == "PXEL_RESOURCE_READY" {
                    break;
                }
            }
            Ok::<(), std::io::Error>(())
        })
        .await??;
        assert!(group.spawn(probe_command("allocate")?).is_err());
        assert!(child.try_wait()?.is_none());
        drop(group);
        // Closing a Windows job can terminate with exit code zero. Readiness,
        // prior liveness and pipe EOF establish cancellation independently.
        tokio::time::timeout(Duration::from_secs(10), child.wait()).await??;
        let mut remaining = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), reader.read_to_end(&mut remaining)).await??;
        assert!(remaining.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn cpu_quota_has_a_measured_positive_control() -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut measured = Vec::new();
        for cores in [0.05, 1.0] {
            let group = processkit::ProcessGroup::with_options(
                processkit::ProcessGroupOptions::default()
                    .max_memory(512 * 1024 * 1024)
                    .max_processes(1)
                    .cpu_quota(cores),
            )?;
            let mut child = group.spawn(probe_command("busy")?)?;
            let stdout = child.stdout.take().ok_or("probe stdout unavailable")?;
            let mut reader = BufReader::new(stdout);
            tokio::time::timeout(Duration::from_secs(10), async {
                let mut line = String::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).await? == 0 {
                        return Err(std::io::Error::other("CPU probe exited before readiness"));
                    }
                    if line.trim_end() == "PXEL_RESOURCE_READY" {
                        break;
                    }
                }
                Ok::<(), std::io::Error>(())
            })
            .await??;
            let start = group
                .stats()?
                .total_cpu_time
                .ok_or("CPU accounting unavailable")?;
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let finish = group
                .stats()?
                .total_cpu_time
                .ok_or("CPU accounting unavailable")?;
            measured.push(
                finish
                    .checked_sub(start)
                    .ok_or("CPU counter went backwards")?,
            );
            drop(group);
            tokio::time::timeout(Duration::from_secs(10), child.wait()).await??;
        }
        assert!(measured[0] < Duration::from_millis(300));
        assert!(measured[1] > Duration::from_millis(300));
        assert!(measured[1] > measured[0] * 3);
        Ok(())
    }

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
