//! Phone pairing (WAN remote control): the desktop shows a short-lived
//! 6-char code + QR; the phone redeems it over the device-room relay for a
//! persistent grant token. Relay conns are gated: until a conn proves a grant
//! (`PairPhone`/`AuthorizePhone`) it may only call the bootstrap set plus the
//! `forwardable` peer surface — the physical/computer surface and the rest of
//! the control plane stay closed to unpaired conns.
//!
//! Trust model: the edge already restricts a device room to the account's own
//! identity; pairing adds proof of physical presence at the desktop (you saw
//! the QR on screen) before a phone gets the phone-control surface, and gives
//! the desktop a per-phone handle to revoke.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zeron_rpc::{RpcError, RpcReply, RpcService};

use crate::rpc::forwardable;

/// Codes are 6 chars from an unambiguous alphabet (no 0/O/1/I/L).
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 6;
/// Offers rotate on every issue and die quickly — a QR left on screen is not
/// a standing invitation.
const OFFER_TTL: Duration = Duration::from_secs(5 * 60);
/// Persisted grants next to the device id / session material.
const GRANTS_FILE: &str = "paired-phones.json";
/// Grant tokens are 256-bit random (two uuid4s), hex.
const TOKEN_BYTES: usize = 32;
/// A paired phone stays trusted for six months from the QR scan, then must
/// pair again. Fixed from creation (not sliding) so an always-connected phone
/// still re-proves physical presence at the desktop twice a year.
const GRANT_TTL: Duration = Duration::from_secs(180 * 24 * 60 * 60);

fn grant_expired(grant: &PhoneGrant, now: i64) -> bool {
    now.saturating_sub(grant.created_at) >= GRANT_TTL.as_millis() as i64
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn rand_bytes(n: usize) -> Vec<u8> {
    // uuid4 is the workspace's existing CSPRNG — no new rand dependency.
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        out.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    out.truncate(n);
    out
}

fn mint_code() -> String {
    rand_bytes(CODE_LEN)
        .iter()
        .map(|b| CODE_ALPHABET[(*b as usize) % CODE_ALPHABET.len()] as char)
        .collect()
}

fn mint_token() -> String {
    rand_bytes(TOKEN_BYTES)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A phone that redeemed a code — persisted; revocable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhoneGrant {
    pub id: String,
    /// Bearer the phone presents on every relay connect (`AuthorizePhone`).
    /// Never leaves the device except in this file and the PairPhone reply.
    pub token: String,
    /// Free-form label ("Rav's iPhone" / platform hint the phone sends).
    #[serde(default)]
    pub label: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub last_used_at: Option<i64>,
}

/// The live offer — regenerated on every `issue` (rotate-on-refresh).
#[derive(Debug, Clone)]
struct PairingOffer {
    code: String,
    expires_at_ms: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("pairing code expired — ask the desktop to refresh")]
    Expired,
    #[error("pairing code does not match")]
    Mismatch,
    #[error("unknown phone grant")]
    UnknownGrant,
    #[error("phone pairing expired after six months — scan a fresh desktop QR")]
    GrantExpired,
}

#[derive(Default)]
struct State {
    offer: Option<PairingOffer>,
    grants: Vec<PhoneGrant>,
    /// conn_id → grant_id for currently-authorized relay conns.
    conns: HashMap<String, String>,
}

/// The desktop's phone-pairing authority. Codes are process-local (a restart
/// rotates everything — matching "short-lived"); grants persist under
/// `data_dir` so phones survive desktop restarts.
pub struct PairingStore {
    device_id: String,
    path: PathBuf,
    state: Mutex<State>,
    /// Called after a code is redeemed — the engine tombstones the
    /// `phonePair/<code>` registry row so a used code stops resolving.
    on_redeemed: Mutex<Option<Arc<dyn Fn(&str) + Send + Sync>>>,
}

impl PairingStore {
    pub fn open(device_id: &str, data_dir: &Path) -> Arc<Self> {
        let path = data_dir.join(GRANTS_FILE);
        let grants = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<PhoneGrant>>(&bytes).ok())
            .unwrap_or_default();
        Arc::new(Self {
            device_id: device_id.to_string(),
            path,
            state: Mutex::new(State {
                offer: None,
                grants,
                conns: HashMap::new(),
            }),
            on_redeemed: Mutex::new(None),
        })
    }

    /// Wire the redeem callback (registry row cleanup) after construction —
    /// the store is created before the workspace host it needs.
    pub fn set_on_redeemed(&self, hook: Arc<dyn Fn(&str) + Send + Sync>) {
        *lock(&self.on_redeemed) = Some(hook);
    }

    /// Mint (or rotate) the pairing offer: returns the code + expiry. The
    /// caller publishes the `phonePair` registry row and renders the QR.
    pub fn issue(&self) -> (String, i64) {
        let offer = PairingOffer {
            code: mint_code(),
            expires_at_ms: now_ms() + OFFER_TTL.as_millis() as i64,
        };
        let code = offer.code.clone();
        let exp = offer.expires_at_ms;
        lock(&self.state).offer = Some(offer);
        (code, exp)
    }

    /// The offer currently on screen, for repaint after window focus.
    pub fn current_offer(&self) -> Option<(String, i64)> {
        lock(&self.state)
            .offer
            .as_ref()
            .map(|o| (o.code.clone(), o.expires_at_ms))
    }

    /// Redeem the on-screen code on this relay conn: single-use (consumed on
    /// success AND on expiry), returns the fresh grant.
    pub fn redeem(
        &self,
        code: &str,
        conn_id: &str,
        label: Option<String>,
    ) -> Result<PhoneGrant, PairingError> {
        let mut state = lock(&self.state);
        let offer = state.offer.take().ok_or(PairingError::Mismatch)?;
        if now_ms() >= offer.expires_at_ms {
            return Err(PairingError::Expired);
        }
        if !offer.code.eq_ignore_ascii_case(code.trim()) {
            // Put the offer back only if the caller mistyped — a wrong guess
            // must not burn the QR someone else is scanning.
            state.offer = Some(offer);
            return Err(PairingError::Mismatch);
        }
        let grant = PhoneGrant {
            id: uuid::Uuid::new_v4().to_string(),
            token: mint_token(),
            label,
            created_at: now_ms(),
            last_used_at: Some(now_ms()),
        };
        state.conns.insert(conn_id.to_string(), grant.id.clone());
        state.grants.push(grant.clone());
        self.persist_locked(&state);
        let redeemed_code = offer.code.clone();
        drop(state);
        if let Some(hook) = lock(&self.on_redeemed).clone() {
            hook(&redeemed_code);
        }
        Ok(grant)
    }

    /// Present a stored grant token on a fresh relay conn (reconnect path).
    pub fn authorize(&self, token: &str, conn_id: &str) -> bool {
        self.authorize_checked(token, conn_id).is_ok()
    }

    /// `authorize` with the reason on failure, so the phone can tell an
    /// expired pairing from a revoked one. Expired grants are deleted.
    pub fn authorize_checked(&self, token: &str, conn_id: &str) -> Result<(), PairingError> {
        self.authorize_at(token, conn_id, now_ms())
    }

    fn authorize_at(&self, token: &str, conn_id: &str, now: i64) -> Result<(), PairingError> {
        let mut state = lock(&self.state);
        let Some(idx) = state.grants.iter().position(|g| g.token == token) else {
            return Err(PairingError::UnknownGrant);
        };
        if grant_expired(&state.grants[idx], now) {
            let id = state.grants.remove(idx).id;
            state.conns.retain(|_, g| *g != id);
            self.persist_locked(&state);
            return Err(PairingError::GrantExpired);
        }
        let grant = &mut state.grants[idx];
        grant.last_used_at = Some(now);
        let id = grant.id.clone();
        state.conns.insert(conn_id.to_string(), id);
        self.persist_locked(&state);
        Ok(())
    }

    /// Gate check: is this relay conn carrying an authorized phone?
    pub fn conn_authorized(&self, conn_id: &str) -> bool {
        lock(&self.state).conns.contains_key(conn_id)
    }

    /// Conn teardown: forget the authorization (grants persist).
    pub fn drop_conn(&self, conn_id: &str) {
        lock(&self.state).conns.remove(conn_id);
    }

    /// Revoke a paired phone: kills every live conn holding the grant and
    /// deletes the stored token, so the phone can never re-authorize.
    pub fn revoke(&self, grant_id: &str) -> bool {
        let mut state = lock(&self.state);
        let before = state.grants.len();
        state.grants.retain(|g| g.id != grant_id);
        if state.grants.len() == before {
            return false;
        }
        state.conns.retain(|_, g| g != grant_id);
        self.persist_locked(&state);
        true
    }

    /// Grants for the settings UI — token deliberately excluded.
    pub fn list(&self) -> Vec<serde_json::Value> {
        lock(&self.state)
            .grants
            .iter()
            .map(|g| {
                serde_json::json!({
                    "id": g.id,
                    "label": g.label,
                    "createdAt": g.created_at,
                    "lastUsedAt": g.last_used_at,
                    "expiresAt": g.created_at.saturating_add(GRANT_TTL.as_millis() as i64),
                })
            })
            .collect()
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    fn persist_locked(&self, state: &State) {
        let bytes = match serde_json::to_vec_pretty(&state.grants) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(error = %err, "phone-pairing: grant serialize failed");
                return;
            }
        };
        let tmp = self.path.with_extension("tmp");
        if let Err(err) = std::fs::write(&tmp, &bytes) {
            tracing::warn!(error = %err, "phone-pairing: grant persist failed");
            return;
        }
        // Windows refuses to rename over an existing destination: remove it
        // first, then retry. Other platforms keep the atomic replace.
        if std::fs::rename(&tmp, &self.path).is_err()
            && (std::fs::remove_file(&self.path).is_err()
                || std::fs::rename(&tmp, &self.path).is_err())
        {
            tracing::warn!("phone-pairing: grant persist failed");
        }
    }
}

/// Methods an unpaired relay conn may call: the pairing ceremony itself plus
/// the identity readouts the phone needs to render "found this desktop".
fn is_bootstrap_method(method: &str) -> bool {
    use zeron_rpc::methods::*;
    matches!(
        method,
        PAIR_PHONE | AUTHORIZE_PHONE | ENGINE_INFO | LOCAL_DEVICE | AUTH_STATUS
    )
}

/// Per-conn service wrapper the host relay installs: gates the physical
/// phone-control surface behind phone pairing while leaving the
/// peer-forwardable surface (desktop↔desktop `targetDeviceId` routing)
/// untouched for same-account callers — the edge already enforces account
/// ownership on the room, and a desktop peer carries no phone grant, so
/// `forwardable` must stay open for multi-device routing to work at all.
/// `from` is an opaque per-conn id the client picks, so the gate cannot tell
/// a phone from a peer desktop; account auth is the boundary for that set,
/// QR pairing is the boundary for everything else.
pub struct RelayGate {
    conn_id: String,
    inner: Arc<dyn RpcService>,
    pairing: Arc<PairingStore>,
}

impl RelayGate {
    pub fn new(conn_id: &str, inner: Arc<dyn RpcService>, pairing: Arc<PairingStore>) -> Self {
        Self {
            conn_id: conn_id.to_string(),
            inner,
            pairing,
        }
    }
}

impl Drop for RelayGate {
    fn drop(&mut self) {
        self.pairing.drop_conn(&self.conn_id);
    }
}

#[async_trait]
impl RpcService for RelayGate {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        use zeron_rpc::methods::*;
        match method {
            PAIR_PHONE => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct PairPhoneParams {
                    code: String,
                    #[serde(default)]
                    label: Option<String>,
                }
                let p: PairPhoneParams = zeron_rpc::parse_params(params)?;
                match self.pairing.redeem(&p.code, &self.conn_id, p.label) {
                    Ok(grant) => RpcReply::value(&serde_json::json!({
                        "deviceToken": grant.token,
                        "grantId": grant.id,
                        "deviceId": self.pairing.device_id(),
                    })),
                    Err(err) => Err(RpcError::Failed(err.to_string())),
                }
            }
            AUTHORIZE_PHONE => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct AuthorizePhoneParams {
                    device_token: String,
                }
                let p: AuthorizePhoneParams = zeron_rpc::parse_params(params)?;
                match self.pairing.authorize_checked(&p.device_token, &self.conn_id) {
                    Ok(()) => RpcReply::value(&serde_json::json!({ "ok": true })),
                    Err(PairingError::GrantExpired) => {
                        Err(RpcError::Failed(PairingError::GrantExpired.to_string()))
                    }
                    Err(_) => Err(RpcError::Failed(
                        "phone not paired — scan the desktop QR".into(),
                    )),
                }
            }
            _ => {
                if is_bootstrap_method(method)
                    || forwardable(method)
                    || self.pairing.conn_authorized(&self.conn_id)
                {
                    self.inner.handle(method, params).await
                } else {
                    Err(RpcError::Failed(format!(
                        "{method} needs a paired phone — scan the QR in Settings → Devices on the desktop"
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Arc<PairingStore> {
        let dir = tempfile::tempdir().unwrap();
        // Keep the dir alive for the store's lifetime: leak the tempdir —
        // tests are short-lived and the OS cleans up.
        let path = dir.path().to_path_buf();
        std::mem::forget(dir);
        PairingStore::open("dev-1", &path)
    }

    #[test]
    fn code_redeems_once_then_dies() {
        let store = store();
        let (code, _exp) = store.issue();
        let grant = store.redeem(&code, "conn-1", None).unwrap();
        assert!(store.conn_authorized("conn-1"));
        // Single-use: second redeem of the same code fails.
        assert!(store.redeem(&code, "conn-2", None).is_err());
        // Reconnect path: token authorizes a new conn.
        assert!(store.authorize(&grant.token, "conn-3"));
        assert!(store.conn_authorized("conn-3"));
    }

    #[test]
    fn wrong_code_does_not_burn_offer() {
        let store = store();
        let (code, _) = store.issue();
        assert!(store.redeem("ZZZZZZ", "conn-1", None).is_err());
        assert!(store.redeem(&code, "conn-1", None).is_ok());
    }

    #[test]
    fn revoke_kills_live_conns_and_token() {
        let store = store();
        let (code, _) = store.issue();
        let grant = store.redeem(&code, "conn-1", None).unwrap();
        assert!(store.revoke(&grant.id));
        assert!(!store.conn_authorized("conn-1"));
        assert!(!store.authorize(&grant.token, "conn-2"));
        assert!(!store.revoke(&grant.id));
    }

    #[test]
    fn grants_persist_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let store = PairingStore::open("dev-1", &path);
        let (code, _) = store.issue();
        let grant = store.redeem(&code, "conn-1", None).unwrap();
        drop(store);
        let reopened = PairingStore::open("dev-1", &path);
        assert!(reopened.authorize(&grant.token, "conn-9"));
        assert_eq!(reopened.list().len(), 1);
    }

    #[test]
    fn grant_valid_until_six_months_then_expires() {
        let store = store();
        let (code, _) = store.issue();
        let grant = store.redeem(&code, "conn-1", None).unwrap();
        let ttl = GRANT_TTL.as_millis() as i64;
        assert!(store
            .authorize_at(&grant.token, "conn-2", grant.created_at + ttl - 1)
            .is_ok());
        assert!(matches!(
            store.authorize_at(&grant.token, "conn-3", grant.created_at + ttl),
            Err(PairingError::GrantExpired)
        ));
        // Expiry deletes the grant and its live conns: re-pairing is the only way back.
        assert!(!store.conn_authorized("conn-2"));
        assert!(store.list().is_empty());
        assert!(matches!(
            store.authorize_checked(&grant.token, "conn-4"),
            Err(PairingError::UnknownGrant)
        ));
    }

    #[test]
    fn repeated_reconnects_keep_grant_alive() {
        let store = store();
        let (code, _) = store.issue();
        let grant = store.redeem(&code, "conn-1", None).unwrap();
        for i in 0..5 {
            store.drop_conn(&format!("conn-{i}"));
            assert!(store.authorize(&grant.token, &format!("conn-{}", i + 1)));
        }
        assert_eq!(store.list().len(), 1);
    }
}
