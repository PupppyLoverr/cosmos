//! Credentials for bring-your-own-key providers and the ChatGPT sign-in.
//!
//! Lookup order for an API key: the provider's environment variables, then
//! the OS credential store (Windows Credential Manager / macOS Keychain,
//! service `cosmos-cli`), then `COSMOS_CLI_HOME/credentials.json`. Keys are
//! written to the OS store when it works and to the file otherwise. Set
//! `COSMOS_CLI_KEYRING=0` to force the file (tests, headless boxes).
//!
//! The ChatGPT OAuth session (access + refresh token) always lives in the
//! file: its JWTs exceed the Windows credential blob limit, and it mirrors how
//! Codex keeps `auth.json`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const SERVICE: &str = "cosmos-cli";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Env,
    Keychain,
    File,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Env => "environment",
            Source::Keychain => "credential store",
            Source::File => "credentials.json",
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CredFile {
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chatgpt: Option<crate::chatgpt::Tokens>,
}

fn path() -> PathBuf {
    crate::cli_home().join("credentials.json")
}

pub fn load_file() -> CredFile {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_file(f: &CredFile) -> Result<()> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(f)?)?;
    std::fs::rename(&tmp, &p).context("writing credentials.json")?;
    Ok(())
}

fn keyring_enabled() -> bool {
    cfg!(any(windows, target_os = "macos")) && std::env::var("COSMOS_CLI_KEYRING").map(|v| v != "0").unwrap_or(true)
}

#[cfg(any(windows, target_os = "macos"))]
fn keyring_get(id: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, id).ok()?.get_password().ok().filter(|k| !k.is_empty())
}
#[cfg(not(any(windows, target_os = "macos")))]
fn keyring_get(_id: &str) -> Option<String> {
    None
}

#[cfg(any(windows, target_os = "macos"))]
fn keyring_set(id: &str, key: &str) -> bool {
    keyring::Entry::new(SERVICE, id).and_then(|e| e.set_password(key)).is_ok()
}
#[cfg(not(any(windows, target_os = "macos")))]
fn keyring_set(_id: &str, _key: &str) -> bool {
    false
}

#[cfg(any(windows, target_os = "macos"))]
fn keyring_delete(id: &str) {
    if let Ok(e) = keyring::Entry::new(SERVICE, id) {
        let _ = e.delete_credential();
    }
}
#[cfg(not(any(windows, target_os = "macos")))]
fn keyring_delete(_id: &str) {}

/// A stored (not environment) key for `id`.
pub fn stored_key(id: &str) -> Option<(String, Source)> {
    if keyring_enabled() {
        if let Some(k) = keyring_get(id) {
            return Some((k, Source::Keychain));
        }
    }
    load_file().keys.get(id).filter(|k| !k.is_empty()).map(|k| (k.clone(), Source::File))
}

/// Resolve a key: env vars first, then the stores.
pub fn key_for(id: &str, env: &[String]) -> Option<(String, Source)> {
    for var in env {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return Some((v.trim().to_string(), Source::Env));
            }
        }
    }
    stored_key(id)
}

/// Save a key; returns where it went.
pub fn store_key(id: &str, key: &str) -> Result<Source> {
    let key = key.trim();
    anyhow::ensure!(!key.is_empty(), "empty key");
    if keyring_enabled() && keyring_set(id, key) {
        // don't leave a stale copy in the file
        let mut f = load_file();
        if f.keys.remove(id).is_some() {
            save_file(&f)?;
        }
        return Ok(Source::Keychain);
    }
    let mut f = load_file();
    f.keys.insert(id.to_string(), key.to_string());
    save_file(&f)?;
    Ok(Source::File)
}

/// Forget a stored key (environment variables are left alone).
pub fn delete_key(id: &str) -> Result<()> {
    if keyring_enabled() {
        keyring_delete(id);
    }
    let mut f = load_file();
    if f.keys.remove(id).is_some() {
        save_file(&f)?;
    }
    Ok(())
}

/// Show only the ends of a key.
pub fn mask(key: &str) -> String {
    let n = key.chars().count();
    if n <= 10 {
        return "•".repeat(n.max(4));
    }
    let head: String = key.chars().take(4).collect();
    let tail: String = key.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trip_and_env_precedence() {
        let _env = crate::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("COSMOS_CLI_HOME", dir.path());
        std::env::set_var("COSMOS_CLI_KEYRING", "0");
        assert!(key_for("testprov", &["ZCLI_TEST_NOPE".into()]).is_none());
        assert_eq!(store_key("testprov", " sk-abc123 ").unwrap(), Source::File);
        assert_eq!(key_for("testprov", &[]).unwrap(), ("sk-abc123".into(), Source::File));
        std::env::set_var("ZCLI_TEST_KEY_ENV", "from-env");
        assert_eq!(key_for("testprov", &["ZCLI_TEST_KEY_ENV".into()]).unwrap().1, Source::Env);
        delete_key("testprov").unwrap();
        assert!(stored_key("testprov").is_none());
    }

    #[test]
    fn masking() {
        assert_eq!(mask("sk-1234567890abcd"), "sk-1…abcd");
        assert_eq!(mask("short"), "•••••");
    }
}
