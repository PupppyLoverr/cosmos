use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::EngineConfig;

#[derive(Serialize)]
struct Handshake<'a> {
    version: u8,
    app: &'static str,
    rpc: String,
    bin: &'a Path,
    tools_key_ref: String,
    updated_at: String,
    capabilities: [&'static str; 1],
}

pub fn publish_handshake(config: &EngineConfig) {
    if let Err(error) = publish(config) {
        tracing::debug!(%error, "sister-app discovery unavailable");
    }
}

fn publish(config: &EngineConfig) -> Result<()> {
    let home = std::env::var_os("HOME").context("HOME unavailable")?;
    let directory = discovery_directory(&PathBuf::from(home));
    let binary = std::env::current_exe()?;
    write_handshake(&directory, &config.data_dir, config.ipc_port, &binary)
}

fn discovery_directory(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    return home.join("Library/Application Support/Cosmos");
    #[cfg(not(target_os = "macos"))]
    home.join(".config/cosmos")
}

fn write_handshake(directory: &Path, data: &Path, port: u16, binary: &Path) -> Result<()> {
    let data = data.canonicalize()?;
    let handshake = Handshake {
        version: 1,
        app: "cosmos",
        rpc: format!("127.0.0.1:{port}"),
        bin: binary,
        tools_key_ref: format!(
            "file:{}#provider=composio",
            data.join("tools.json").display()
        ),
        updated_at: chrono::Utc::now().to_rfc3339(),
        capabilities: ["tools_reference"],
    };
    std::fs::create_dir_all(directory)?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer_pretty(&mut file, &handshake)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(directory.join("handshake.json"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_is_atomic_and_exports_only_a_reference() -> Result<()> {
        let root = tempfile::tempdir()?;
        let data = root.path().join("profile");
        std::fs::create_dir(&data)?;
        std::fs::write(data.join("tools.json"), "private-test-sentinel")?;
        let directory = root.path().join("discovery");
        write_handshake(
            &directory,
            &data,
            27654,
            Path::new("/Applications/Cosmos.app"),
        )?;
        write_handshake(
            &directory,
            &data,
            27655,
            Path::new("/Applications/Cosmos.app"),
        )?;
        let text = std::fs::read_to_string(directory.join("handshake.json"))?;
        let document: serde_json::Value = serde_json::from_str(&text)?;
        assert_eq!(document["app"], "cosmos");
        assert_eq!(document["version"], 1);
        assert_eq!(document["rpc"], "127.0.0.1:27655");
        assert!(
            document["tools_key_ref"]
                .as_str()
                .unwrap()
                .ends_with("/tools.json#provider=composio")
        );
        assert!(!text.contains("private-test-sentinel"));
        assert_eq!(std::fs::read_dir(&directory)?.count(), 1);
        Ok(())
    }
}
