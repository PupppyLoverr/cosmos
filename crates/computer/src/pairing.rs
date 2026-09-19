//! First-time explicit pairing for remote Cosmos Computer.
//!
//! Same-account discovery already lists devices. Physical desktop control is
//! a separate trust: a caller must `request` trust, the desktop user must
//! `approve`, and later calls carry a pairing token. The raw token is never
//! persisted — only a SHA-256 hex digest — and never logged.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ComputerError;

const PAIRING_FILENAME: &str = "computer-pairing.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrustState {
    None,
    Pending,
    Trusted,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustRecord {
    pub device_id: String,
    pub device_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    pub state: TrustState,
    pub requested_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<chrono::DateTime<chrono::Utc>>,
    /// SHA-256 hex of the caller-held pairing token. Never the token itself.
    token_hash: String,
}

impl TrustRecord {
    pub fn public(&self) -> TrustPublic {
        TrustPublic {
            device_id: self.device_id.clone(),
            device_name: self.device_name.clone(),
            platform: self.platform.clone(),
            state: self.state,
            requested_at: self.requested_at,
            approved_at: self.approved_at,
        }
    }
}

/// Wire view — no token material.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustPublic {
    pub device_id: String,
    pub device_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    pub state: TrustState,
    pub requested_at: chrono::DateTime<chrono::Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnDisk {
    #[serde(default)]
    records: Vec<TrustRecord>,
}

/// Device-local pairing store.
pub struct PairingStore {
    path: PathBuf,
    records: Vec<TrustRecord>,
}

impl PairingStore {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join(PAIRING_FILENAME);
        let records = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<OnDisk>(&bytes).ok())
            .map(|disk| disk.records)
            .unwrap_or_default();
        Self { path, records }
    }

    fn persist(&self) -> Result<(), ComputerError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ComputerError::Internal(e.to_string()))?;
        }
        let body = serde_json::to_vec_pretty(&OnDisk {
            records: self.records.clone(),
        })
        .map_err(|e| ComputerError::Internal(e.to_string()))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, body).map_err(|e| ComputerError::Internal(e.to_string()))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| ComputerError::Internal(e.to_string()))
    }

    pub fn list(&self) -> Vec<TrustPublic> {
        self.records.iter().map(TrustRecord::public).collect()
    }

    pub fn pending(&self) -> Vec<TrustPublic> {
        self.records
            .iter()
            .filter(|r| r.state == TrustState::Pending)
            .map(TrustRecord::public)
            .collect()
    }

    pub fn trusted(&self) -> Vec<TrustPublic> {
        self.records
            .iter()
            .filter(|r| r.state == TrustState::Trusted)
            .map(TrustRecord::public)
            .collect()
    }

    /// Caller-held token is hashed immediately; the raw value is dropped.
    pub fn request(
        &mut self,
        device_id: &str,
        device_name: &str,
        platform: Option<&str>,
        pairing_token: &str,
    ) -> Result<TrustPublic, ComputerError> {
        let device_id = device_id.trim();
        let pairing_token = pairing_token.trim();
        if device_id.is_empty() || pairing_token.len() < 16 {
            return Err(ComputerError::InvalidArgs(
                "pairing request needs deviceId and a pairingToken (≥16 chars)".into(),
            ));
        }
        let token_hash = hash_token(pairing_token);
        let now = chrono::Utc::now();
        if let Some(existing) = self.records.iter_mut().find(|r| r.device_id == device_id) {
            if existing.state == TrustState::Trusted && existing.token_hash == token_hash {
                return Ok(existing.public());
            }
            existing.device_name = device_name.trim().to_string();
            existing.platform = platform.map(|s| s.to_string());
            existing.state = TrustState::Pending;
            existing.requested_at = now;
            existing.approved_at = None;
            existing.token_hash = token_hash;
            let public = existing.public();
            self.persist()?;
            return Ok(public);
        }
        let record = TrustRecord {
            device_id: device_id.to_string(),
            device_name: device_name.trim().to_string(),
            platform: platform.map(|s| s.to_string()),
            state: TrustState::Pending,
            requested_at: now,
            approved_at: None,
            token_hash,
        };
        let public = record.public();
        self.records.push(record);
        self.persist()?;
        Ok(public)
    }

    pub fn approve(&mut self, device_id: &str) -> Result<TrustPublic, ComputerError> {
        let record = self
            .records
            .iter_mut()
            .find(|r| r.device_id == device_id)
            .ok_or_else(|| {
                ComputerError::refused("pairing_unknown", "no pairing request for that device")
            })?;
        if record.state == TrustState::Revoked {
            return Err(ComputerError::refused(
                "pairing_revoked",
                "that device was revoked — it must request trust again",
            ));
        }
        record.state = TrustState::Trusted;
        record.approved_at = Some(chrono::Utc::now());
        let public = record.public();
        self.persist()?;
        Ok(public)
    }

    pub fn revoke(&mut self, device_id: &str) -> Result<TrustPublic, ComputerError> {
        let record = self
            .records
            .iter_mut()
            .find(|r| r.device_id == device_id)
            .ok_or_else(|| {
                ComputerError::refused("pairing_unknown", "no pairing record for that device")
            })?;
        record.state = TrustState::Revoked;
        record.approved_at = None;
        // Rotate the hash so a retained token cannot reconnect.
        record.token_hash = hash_token(&format!("revoked:{}:{}", device_id, chrono::Utc::now()));
        let public = record.public();
        self.persist()?;
        Ok(public)
    }

    /// Verify a caller-held token against a trusted record. Constant-time-ish
    /// compare of hex hashes (equal length).
    pub fn authorize(&self, device_id: &str, pairing_token: &str) -> Result<(), ComputerError> {
        let Some(record) = self.records.iter().find(|r| r.device_id == device_id) else {
            return Err(ComputerError::refused(
                "pairing_required",
                "this desktop has not paired with that device",
            ));
        };
        match record.state {
            TrustState::Trusted => {}
            TrustState::Pending => {
                return Err(ComputerError::refused(
                    "pairing_pending",
                    "approve this device on the Cosmos desktop before remote Computer Use",
                ));
            }
            TrustState::Revoked | TrustState::None => {
                return Err(ComputerError::refused(
                    "pairing_revoked",
                    "pairing was revoked — request trust again",
                ));
            }
        }
        let got = hash_token(pairing_token);
        if got != record.token_hash {
            return Err(ComputerError::refused(
                "pairing_denied",
                "pairing token does not match",
            ));
        }
        Ok(())
    }

    pub fn status_for(&self, device_id: &str) -> TrustState {
        self.records
            .iter()
            .find(|r| r.device_id == device_id)
            .map(|r| r.state)
            .unwrap_or(TrustState::None)
    }
}

pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_approve_authorize_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = PairingStore::load(dir.path());
        let token = "pairing-token-aaaaaaaa";
        let pending = store
            .request("phone-1", "Pixel", Some("android"), token)
            .unwrap();
        assert_eq!(pending.state, TrustState::Pending);
        store.authorize("phone-1", token).unwrap_err();
        let trusted = store.approve("phone-1").unwrap();
        assert_eq!(trusted.state, TrustState::Trusted);
        store.authorize("phone-1", token).unwrap();
        store
            .authorize("phone-1", "wrong-token-bbbbbbbb")
            .unwrap_err();
        store.revoke("phone-1").unwrap();
        store.authorize("phone-1", token).unwrap_err();
        // Reload — hash only, never the raw token.
        let disk = std::fs::read_to_string(dir.path().join(PAIRING_FILENAME)).unwrap();
        assert!(!disk.contains(token));
        assert!(
            disk.contains("tokenHash") || disk.contains("token_hash") || disk.contains("\"token")
        );
    }
}
