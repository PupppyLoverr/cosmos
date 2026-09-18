//! Optional-component management: download the pinned Cua Driver release
//! archive, verify its SHA-256 against the value baked at pin time, extract
//! it with the OS's own bsdtar (which sniffs zip vs tar.gz — the same trick
//! `zeron-update` uses, no extra crates), and atomically place it under
//! `{data_dir}/computer/cua-driver/versions/<version>-<target>/`.
//!
//! Nothing here runs with elevated privileges; the whole component lives in
//! the user-writable Cosmos data dir.

use std::path::{Path, PathBuf};

use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::{ComputerError, ComputerPaths, InstallState, ReleaseAsset, write_install_state};

/// Progress for the Settings page's install button (a watch channel item —
/// latest wins, so a fast download can't backlog the UI). Serialized onto the
/// `WatchComputer` stream as `{"stage": "downloading", …}`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "stage")]
pub enum InstallProgress {
    /// Bytes received so far; `total` is the asset's published size.
    Downloading {
        received: u64,
        total: u64,
    },
    Verifying,
    Extracting,
    Finished,
    Failed(String),
    Idle,
}

/// Install (or reinstall over) the pinned release. Blocking IO runs on
/// `spawn_blocking`-friendly syscalls only where trivial; download/extract
/// stream under tokio.
pub async fn install(
    paths: &ComputerPaths,
    asset: &ReleaseAsset,
    progress: &tokio::sync::watch::Sender<InstallProgress>,
) -> Result<InstallState, ComputerError> {
    fn fail(stage: &str, e: impl std::fmt::Display) -> ComputerError {
        ComputerError::Install {
            stage: stage.into(),
            message: e.to_string(),
        }
    }
    let report = |p: InstallProgress| {
        let _ = progress.send(p);
    };

    std::fs::create_dir_all(paths.versions_dir()).map_err(|e| fail("prepare", e))?;
    let stage_dir = paths.root.join(format!(
        ".stage-{}-{}",
        crate::CUA_VERSION,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&stage_dir);
    std::fs::create_dir_all(&stage_dir).map_err(|e| fail("prepare", e))?;
    // RAII-style cleanup on every exit path.
    let cleanup = StageCleanup(stage_dir.clone());

    // --- Download ---------------------------------------------------------
    let archive = stage_dir.join(asset.name);
    download(asset, &archive, &report)
        .await
        .map_err(|e| fail("download", e))?;

    // --- Verify -----------------------------------------------------------
    report(InstallProgress::Verifying);
    verify_sha256(&archive, asset.sha256)
        .await
        .map_err(|e| fail("verify", e))?;

    // --- Extract ----------------------------------------------------------
    report(InstallProgress::Extracting);
    let unpacked = stage_dir.join("unpacked");
    std::fs::create_dir_all(&unpacked).map_err(|e| fail("extract", e))?;
    extract(&archive, &unpacked)
        .await
        .map_err(|e| fail("extract", e))?;

    // The macOS archive nests everything under `cua-driver-rs-<ver>-darwin-
    // universal/`; other platforms unpack flat at the root.
    let source = find_component_root(&unpacked)?;
    let dest = paths.install_dir(crate::CUA_VERSION);
    if dest.exists() {
        // Reinstall-over: drop the previous layout first (driver may be
        // running — Windows locks the exe; surface that error to the user).
        std::fs::remove_dir_all(&dest).map_err(|e| {
            fail(
                "install",
                format!(
                    "could not replace the existing install (is the driver still running?): {e}"
                ),
            )
        })?;
    }
    std::fs::rename(&source, &dest).map_err(|e| {
        fail(
            "install",
            format!("could not move the driver into place: {e}"),
        )
    })?;
    if ComputerPaths::binary_in(&dest).is_none() {
        return Err(fail(
            "install",
            "the archive did not contain a cua-driver binary",
        ));
    }

    let state = InstallState {
        version: crate::CUA_VERSION.to_string(),
        tag: crate::CUA_TAG.to_string(),
        asset: asset.name.to_string(),
        sha256: asset.sha256.to_string(),
        install_dir: dest,
        installed_at: chrono::Utc::now(),
    };
    write_install_state(paths, &state)?;
    report(InstallProgress::Finished);
    drop(cleanup);
    Ok(state)
}

/// Remove the installed component + its marker. A running driver holds the
/// binary mapped — Windows refuses the delete, which surfaces honestly.
pub fn uninstall(paths: &ComputerPaths) -> Result<(), ComputerError> {
    let state = paths.installed()?;
    if let Some(state) = state {
        if state.install_dir.exists() {
            std::fs::remove_dir_all(&state.install_dir).map_err(|e| ComputerError::Install {
                stage: "uninstall".into(),
                message: e.to_string(),
            })?;
        }
    }
    // Also sweep any stale version dirs the marker doesn't know about.
    if let Ok(entries) = std::fs::read_dir(paths.versions_dir()) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let _ = std::fs::remove_file(paths.state_file());
    Ok(())
}

/// Does an installed component verify? Marker + binary must both exist.
pub fn verify(paths: &ComputerPaths) -> bool {
    paths
        .installed()
        .ok()
        .flatten()
        .and_then(|state| ComputerPaths::binary_in(&state.install_dir))
        .is_some()
}

/// Total on-disk size of the installed layout (for the Settings disclosure).
pub fn installed_bytes(install_dir: &Path) -> u64 {
    fn walk(dir: &Path) -> u64 {
        let mut total = 0;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    total += walk(&path);
                } else if let Ok(meta) = path.metadata() {
                    total += meta.len();
                }
            }
        }
        total
    }
    walk(install_dir)
}

async fn download(
    asset: &ReleaseAsset,
    dest: &Path,
    report: &impl Fn(InstallProgress),
) -> anyhow::Result<()> {
    let url = asset.url();
    let response = reqwest::get(&url).await?.error_for_status()?;
    let mut file = tokio::fs::File::create(dest).await?;
    let mut received = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        received += chunk.len() as u64;
        report(InstallProgress::Downloading {
            received,
            total: asset.size_bytes,
        });
    }
    file.flush().await?;
    Ok(())
}

async fn verify_sha256(path: &Path, expected: &str) -> anyhow::Result<()> {
    let path = path.to_path_buf();
    let expected = expected.to_string();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        let actual = format!("{:x}", hasher.finalize());
        anyhow::ensure!(
            actual == expected,
            "checksum mismatch: expected {expected}, got {actual}"
        );
        Ok(())
    })
    .await?
}

/// `tar -xf` — bsdtar sniffs zip/tar.gz from the magic bytes on every
/// supported OS (System32 tar on Windows, /usr/bin/tar elsewhere).
async fn extract(archive: &Path, dest: &Path) -> anyhow::Result<()> {
    let output = tokio::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .output()
        .await?;
    anyhow::ensure!(
        output.status.success(),
        "tar exited {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// The directory that contains the driver binary — the unpacked root itself
/// for flat archives, or the single nested `cua-driver-rs-*` dir on macOS.
fn find_component_root(unpacked: &Path) -> Result<PathBuf, ComputerError> {
    if ComputerPaths::binary_in(unpacked).is_some() {
        return Ok(unpacked.to_path_buf());
    }
    let mut nested = None;
    if let Ok(entries) = std::fs::read_dir(unpacked) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && ComputerPaths::binary_in(&path).is_some() {
                nested = Some(path);
                break;
            }
        }
    }
    nested.ok_or_else(|| ComputerError::Install {
        stage: "install".into(),
        message: "no cua-driver binary found in the archive".into(),
    })
}

struct StageCleanup(PathBuf);

impl Drop for StageCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
