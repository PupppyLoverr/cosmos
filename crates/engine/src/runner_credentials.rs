use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use chrono::{DateTime, Utc};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeron_proto::{ByokProvider, HarnessId};

const IDENTITY_FILE: &str = "runner-identity.json";
const CREDENTIALS_FILE: &str = "runner-credentials.json";
const SEAL_VERSION: u8 = 1;
const SEAL_LABEL: &[u8] = b"cosmos-runner-seal-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunnerCredentialKind {
    ApiKey,
    Github,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerCredentialMetadata {
    pub kind: RunnerCredentialKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ByokProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub sealed_at: DateTime<Utc>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SealedRunnerCredential {
    pub sealed: String,
    #[serde(flatten)]
    pub metadata: RunnerCredentialMetadata,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RunnerCredentialFile {
    pub credentials: Vec<SealedRunnerCredential>,
    pub runner_logins: Vec<HarnessId>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerCredentialPlaintext {
    pub kind: RunnerCredentialKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ByokProvider>,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

struct RunnerIdentity {
    secret: StaticSecret,
}

pub struct RunnerCredentialVault {
    root: PathBuf,
    identity: RunnerIdentity,
    file: Mutex<RunnerCredentialFile>,
}

impl RunnerCredentialVault {
    pub fn load(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(|_| "could not create runner credential store")?;
        let identity = load_or_create_identity(&root.join(IDENTITY_FILE))?;
        let credentials_path = root.join(CREDENTIALS_FILE);
        let file = match fs::read(&credentials_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "runner credential store is malformed")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let file = RunnerCredentialFile::default();
                write_private_json(&credentials_path, &file)?;
                file
            }
            Err(_) => return Err("could not read runner credential store".into()),
        };
        Ok(Self {
            root: root.to_path_buf(),
            identity,
            file: Mutex::new(file),
        })
    }

    pub fn public_key(&self) -> [u8; 32] {
        PublicKey::from(&self.identity.secret).to_bytes()
    }

    pub fn public_key_base64(&self) -> String {
        BASE64.encode(self.public_key())
    }

    pub fn credentials(&self) -> Vec<SealedRunnerCredential> {
        self.file.lock().unwrap().credentials.clone()
    }

    pub fn metadata(&self) -> Vec<RunnerCredentialMetadata> {
        self.file
            .lock()
            .unwrap()
            .credentials
            .iter()
            .map(|credential| credential.metadata.clone())
            .collect()
    }

    pub fn runner_logins(&self) -> Vec<HarnessId> {
        self.file.lock().unwrap().runner_logins.clone()
    }

    pub fn store(&self, credential: SealedRunnerCredential) -> Result<(), String> {
        validate_credential_metadata(&credential.metadata)?;
        let sealed = BASE64
            .decode(&credential.sealed)
            .map_err(|_| "sealed runner credential is invalid".to_string())?;
        if sealed.len() < 1 + 32 + 12 + 16 || sealed[0] != SEAL_VERSION {
            return Err("sealed runner credential is invalid".into());
        }
        let mut file = self.file.lock().unwrap();
        file.credentials.retain(|existing| {
            existing.metadata.kind != credential.metadata.kind
                || existing.metadata.provider != credential.metadata.provider
        });
        file.credentials.push(credential);
        self.persist(&file)
    }

    pub fn revoke(
        &self,
        provider: Option<ByokProvider>,
        kind: Option<RunnerCredentialKind>,
    ) -> Result<(), String> {
        if provider.is_none() && kind.is_none() {
            return Err("provider or kind is required".into());
        }
        let mut file = self.file.lock().unwrap();
        file.credentials.retain(|credential| {
            !(provider.is_none_or(|value| credential.metadata.provider == Some(value))
                && kind.is_none_or(|value| credential.metadata.kind == value))
        });
        self.persist(&file)
    }

    pub fn set_login(&self, harness: HarnessId, enabled: bool) -> Result<(), String> {
        let mut file = self.file.lock().unwrap();
        if enabled && !file.runner_logins.contains(&harness) {
            file.runner_logins.push(harness);
        } else if !enabled {
            file.runner_logins.retain(|existing| *existing != harness);
        }
        self.persist(&file)
    }

    pub fn unseal(
        &self,
        credential: &SealedRunnerCredential,
    ) -> Result<RunnerCredentialPlaintext, String> {
        let plaintext = unseal_for_runner(&self.identity.secret, &credential.sealed)?;
        if plaintext.kind != credential.metadata.kind
            || plaintext.provider != credential.metadata.provider
            || plaintext.base_url != credential.metadata.base_url
            || plaintext.expires_at != credential.metadata.expires_at
        {
            return Err("sealed runner credential metadata does not match".into());
        }
        Ok(plaintext)
    }

    fn persist(&self, file: &RunnerCredentialFile) -> Result<(), String> {
        write_private_json(&self.root.join(CREDENTIALS_FILE), file)
    }
}

pub fn seal_for_runner(
    runner_public_key: &[u8; 32],
    plaintext: &RunnerCredentialPlaintext,
) -> Result<SealedRunnerCredential, String> {
    let runner_key = PublicKey::from(*runner_public_key);
    let ephemeral = StaticSecret::random_from_rng(OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral);
    let shared = ephemeral.diffie_hellman(&runner_key);
    if !shared.was_contributory() {
        return Err("runner public key is invalid".into());
    }
    let shared_key = derive_key(
        shared.as_bytes(),
        ephemeral_public.as_bytes(),
        runner_public_key,
    );
    let mut nonce = [0; 12];
    OsRng.fill_bytes(&mut nonce);
    let cipher = ChaCha20Poly1305::new_from_slice(&shared_key)
        .map_err(|_| "could not seal credential".to_string())?;
    let json =
        serde_json::to_vec(plaintext).map_err(|_| "could not encode credential".to_string())?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), json.as_ref())
        .map_err(|_| "could not seal credential".to_string())?;
    let mut blob = Vec::with_capacity(1 + 32 + 12 + ciphertext.len());
    blob.push(SEAL_VERSION);
    blob.extend_from_slice(ephemeral_public.as_bytes());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    Ok(SealedRunnerCredential {
        sealed: BASE64.encode(blob),
        metadata: RunnerCredentialMetadata {
            kind: plaintext.kind,
            provider: plaintext.provider,
            base_url: plaintext.base_url.clone(),
            expires_at: plaintext.expires_at,
            sealed_at: Utc::now(),
        },
    })
}

fn unseal_for_runner(
    runner_secret: &StaticSecret,
    encoded: &str,
) -> Result<RunnerCredentialPlaintext, String> {
    let blob = BASE64
        .decode(encoded)
        .map_err(|_| "sealed runner credential is invalid".to_string())?;
    if blob.len() < 1 + 32 + 12 + 16 || blob[0] != SEAL_VERSION {
        return Err("sealed runner credential is invalid".into());
    }
    let ephemeral_public_bytes: [u8; 32] = blob[1..33]
        .try_into()
        .map_err(|_| "sealed runner credential is invalid".to_string())?;
    let ephemeral_public = PublicKey::from(ephemeral_public_bytes);
    let runner_public = PublicKey::from(runner_secret);
    let shared = runner_secret.diffie_hellman(&ephemeral_public);
    if !shared.was_contributory() {
        return Err("sealed runner credential is invalid".into());
    }
    let key = derive_key(
        shared.as_bytes(),
        &ephemeral_public_bytes,
        runner_public.as_bytes(),
    );
    let nonce = Nonce::from_slice(&blob[33..45]);
    let cipher = ChaCha20Poly1305::new_from_slice(&key)
        .map_err(|_| "sealed runner credential is invalid".to_string())?;
    let json = cipher
        .decrypt(nonce, &blob[45..])
        .map_err(|_| "sealed runner credential could not be decrypted".to_string())?;
    serde_json::from_slice(&json).map_err(|_| "sealed runner credential is malformed".into())
}

fn derive_key(
    shared: &[u8; 32],
    ephemeral_public: &[u8; 32],
    runner_public: &[u8; 32],
) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(SEAL_LABEL);
    digest.update(shared);
    digest.update(ephemeral_public);
    digest.update(runner_public);
    digest.finalize().into()
}

fn validate_credential_metadata(metadata: &RunnerCredentialMetadata) -> Result<(), String> {
    match metadata.kind {
        RunnerCredentialKind::ApiKey if metadata.provider.is_none() => {
            Err("API-key credentials require a provider".into())
        }
        RunnerCredentialKind::Github if metadata.provider.is_some() => {
            Err("GitHub credentials cannot name a provider".into())
        }
        _ => Ok(()),
    }
}

fn load_or_create_identity(path: &Path) -> Result<RunnerIdentity, String> {
    match fs::read(path) {
        Ok(bytes) => {
            let file: IdentityFile = serde_json::from_slice(&bytes)
                .map_err(|_| "runner identity is malformed".to_string())?;
            let secret = BASE64
                .decode(file.secret)
                .map_err(|_| "runner identity is malformed".to_string())?;
            let secret: [u8; 32] = secret
                .try_into()
                .map_err(|_| "runner identity is malformed".to_string())?;
            return Ok(RunnerIdentity {
                secret: StaticSecret::from(secret),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("could not read runner identity".into()),
    }
    let secret = StaticSecret::random_from_rng(OsRng);
    write_private_json(
        path,
        &IdentityFile {
            secret: BASE64.encode(secret.to_bytes()),
        },
    )?;
    Ok(RunnerIdentity { secret })
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdentityFile {
    secret: String,
}

fn write_private_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path.parent().ok_or("runner store path has no parent")?;
    fs::create_dir_all(parent).map_err(|_| "could not create runner credential store")?;
    let bytes = serde_json::to_vec(value)
        .map_err(|_| "could not encode runner credential store".to_string())?;
    let temp_path = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp_path)
        .map_err(|_| "could not write runner credential store")?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "could not write runner credential store")?;
    drop(file);
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).map_err(|_| "could not replace runner credential store")?;
    }
    if fs::rename(&temp_path, path).is_err() {
        let _ = fs::remove_file(temp_path);
        return Err("could not replace runner credential store".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|_| "could not protect runner credential store".to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plaintext() -> RunnerCredentialPlaintext {
        RunnerCredentialPlaintext {
            kind: RunnerCredentialKind::ApiKey,
            provider: Some(ByokProvider::Anthropic),
            key: "secret-test-value".into(),
            base_url: Some("https://api.anthropic.com".into()),
            expires_at: None,
        }
    }

    #[test]
    fn sealed_credentials_round_trip_and_reject_tampering() {
        let root = tempfile::tempdir().unwrap();
        let vault = RunnerCredentialVault::load(root.path()).unwrap();
        let sealed = seal_for_runner(&vault.public_key(), &plaintext()).unwrap();
        vault.store(sealed.clone()).unwrap();
        let decoded = vault.unseal(&sealed).unwrap();
        assert_eq!(decoded.key, "secret-test-value");
        let mut tampered = sealed;
        let mut bytes = BASE64.decode(&tampered.sealed).unwrap();
        let index = bytes.len() - 1;
        bytes[index] ^= 1;
        tampered.sealed = BASE64.encode(bytes);
        assert!(vault.unseal(&tampered).is_err());
    }

    #[test]
    fn runner_store_persists_ciphertext_metadata_and_login_markers_only() {
        let root = tempfile::tempdir().unwrap();
        let vault = RunnerCredentialVault::load(root.path()).unwrap();
        let sealed = seal_for_runner(&vault.public_key(), &plaintext()).unwrap();
        vault.store(sealed).unwrap();
        vault.set_login(HarnessId::Codex, true).unwrap();
        let contents = fs::read_to_string(root.path().join(CREDENTIALS_FILE)).unwrap();
        assert!(!contents.contains("secret-test-value"));
        assert!(contents.contains("runnerLogins"));
        assert!(contents.contains("anthropic"));
    }

    #[cfg(unix)]
    #[test]
    fn runner_secret_files_are_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        RunnerCredentialVault::load(root.path()).unwrap();
        for file in [IDENTITY_FILE, CREDENTIALS_FILE] {
            let mode = fs::metadata(root.path().join(file))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn runner_identity_is_stable_across_vault_reloads() {
        let root = tempfile::tempdir().unwrap();
        let first = RunnerCredentialVault::load(root.path()).unwrap();
        let public_key = first.public_key();
        drop(first);
        let second = RunnerCredentialVault::load(root.path()).unwrap();
        assert_eq!(second.public_key(), public_key);
    }
}
