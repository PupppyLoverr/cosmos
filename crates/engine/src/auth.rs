//! Auth — the engine owns the cloud session for its device (feature-inventory §3.7,
//! ARCHITECTURE §5).
//!
//! Production identity is **Woozlit** (Firebase on woozlit.com), the same login
//! the Woozlit desktop app uses: open `/login?desktop=1&callback=…`, Google or
//! Discord on the website, then a custom-token bounce back to this machine.
//! The engine exchanges that custom token for a Firebase ID token + refresh
//! token via Google's public Identity Toolkit (no Firebase SDK, no WorkOS).
//! The edge verifies those ID tokens against Google's JWKS.
//!
//! Two modes:
//! - **Dev** (no cloud-auth client id configured, or the edge reports `auth: "dev"`):
//!   always signed in; the bearer IS the configured user id.
//! - **Woozlit**: headed devices bind a loopback callback and open woozlit.com;
//!   the website returns `token` (Firebase custom token) + `userId`/`email`.
//!   A personal workspace is implied (`org_id` = Firebase uid) so there is no
//!   org picker. Refresh tokens are persisted 0600 in the data dir; access
//!   tokens are cached with dual-clock expiry (see [`AccessEntry`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

use crate::EngineError;

const SIGN_IN_TTL: Duration = Duration::from_secs(15 * 60);
/// Refresh when the cached token has less than this much life left.
const TOKEN_SLACK: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Public Woozlit / Firebase client values (they ship in Woozlit's web bundle).
pub const WOOZLIT_ORIGIN: &str = "https://www.woozlit.com";
pub const FIREBASE_API_KEY: &str = "AIzaSyBbJtkBoJrKM2hjW2H3o9pgtcyiGge9yOo";
pub const FIREBASE_PROJECT_ID: &str = "nexiloop-c0456";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Wire types (feature-inventory §2 AuthRpc)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgMembership {
    pub id: String,
    pub organization_id: String,
    pub name: String,
}

/// AuthStatus stream payload (`SignedOut | NeedsOrganization{user} |
/// SignedIn{user, orgId?}`). Serializes as the canonical [`zeron_proto::AuthState`]
/// wire shape (`{"state": "signedIn", …}`) so every client parses one form.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    SignedOut,
    NeedsOrganization {
        user: AuthUser,
    },
    SignedIn {
        user: AuthUser,
        org_id: Option<String>,
    },
}

impl AuthState {
    pub fn is_signed_in(&self) -> bool {
        matches!(self, AuthState::SignedIn { .. })
    }

    pub fn org_id(&self) -> Option<&str> {
        match self {
            AuthState::SignedIn { org_id, .. } => org_id.as_deref(),
            _ => None,
        }
    }

    pub fn user(&self) -> Option<&AuthUser> {
        match self {
            AuthState::SignedIn { user, .. } | AuthState::NeedsOrganization { user } => Some(user),
            AuthState::SignedOut => None,
        }
    }

    /// The proto wire twin — the one shape the engine emits over AuthStatus.
    pub fn to_proto(&self) -> zeron_proto::AuthState {
        let profile = |user: &AuthUser| zeron_proto::UserProfile {
            id: user.id.clone(),
            email: user.email.clone(),
            name: user.name.clone(),
        };
        match self {
            AuthState::SignedOut => zeron_proto::AuthState::SignedOut,
            AuthState::NeedsOrganization { user } => zeron_proto::AuthState::NeedsOrganization {
                user: profile(user),
            },
            AuthState::SignedIn { user, org_id } => zeron_proto::AuthState::SignedIn {
                user: profile(user),
                org_id: org_id.clone(),
            },
        }
    }
}

impl Serialize for AuthState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_proto().serialize(serializer)
    }
}

// ---------------------------------------------------------------------------
// Config + construction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Edge base URL (`/auth/*` routes).
    pub edge_url: String,
    /// Data dir for the persisted session (`session.json`, 0600).
    pub data_dir: PathBuf,
    /// Cloud-auth enable flag; `None` = dev mode. Historically a WorkOS client
    /// id; any non-empty value now turns on Woozlit login.
    pub workos_client_id: Option<String>,
    /// WorkOS API base (unused for Woozlit; kept so existing env/tests compile).
    pub workos_api_base: String,
    /// Woozlit origin that hosts `/login` (override for staging).
    pub woozlit_origin: String,
    /// Firebase web API key used for custom-token exchange + refresh.
    pub firebase_api_key: String,
    /// Override Identity Toolkit `signInWithCustomToken` URL (tests).
    pub firebase_custom_token_url: String,
    /// Override Secure Token refresh URL (tests).
    pub firebase_refresh_url: String,
    /// Dev-mode bearer/user id (mirrors the old `ZERON_EDGE_TOKEN` behavior).
    pub dev_user_id: String,
    /// Loopback callback port; `None` = ephemeral.
    pub callback_port: Option<u16>,
}

impl AuthConfig {
    pub fn new(edge_url: impl Into<String>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            edge_url: edge_url.into(),
            data_dir: data_dir.into(),
            workos_client_id: None,
            workos_api_base: "https://api.workos.com".into(),
            woozlit_origin: WOOZLIT_ORIGIN.into(),
            firebase_api_key: FIREBASE_API_KEY.into(),
            firebase_custom_token_url: String::new(),
            firebase_refresh_url: String::new(),
            dev_user_id: "dev-user".into(),
            callback_port: None,
        }
    }

    fn custom_token_url(&self) -> String {
        if !self.firebase_custom_token_url.trim().is_empty() {
            return self.firebase_custom_token_url.clone();
        }
        format!(
            "https://identitytoolkit.googleapis.com/v1/accounts:signInWithCustomToken?key={}",
            url_encode(&self.firebase_api_key)
        )
    }

    fn refresh_url(&self) -> String {
        if !self.firebase_refresh_url.trim().is_empty() {
            return self.firebase_refresh_url.clone();
        }
        format!(
            "https://securetoken.googleapis.com/v1/token?key={}",
            url_encode(&self.firebase_api_key)
        )
    }
}

/// The persisted session (refresh token + user + last org scope).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    refresh_token: String,
    user: AuthUser,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    org_id: Option<String>,
}

/// Access-token cache. Expiry ages the token's own lifetime (`exp - iat`) by
/// BOTH clocks, pessimistically. Monotonic alone (`Instant`) freezes across
/// system sleep (macOS `mach_absolute_time` and Linux `CLOCK_MONOTONIC` both
/// exclude suspend), so a laptop waking from hours of sleep presented a
/// wall-expired token that still read "fresh" — every room/relay redial got a
/// 401 with the same stale bearer and sync never recovered (user report).
/// Wall clock alone breaks under skewed device clocks (`exp` vs local time);
/// the elapsed-since-issue reading is skew-immune, and a BACKWARD wall step
/// (NTP correction) degrades harmlessly to the monotonic reading.
struct AccessEntry {
    token: String,
    ttl: Duration,
    got_at: Instant,
    got_wall: std::time::SystemTime,
}

impl AccessEntry {
    fn fresh(token: String) -> Self {
        let ttl = jwt_claims(&token)
            .and_then(|c| match (c.exp, c.iat) {
                (Some(exp), Some(iat)) if exp > iat => {
                    Some(Duration::from_secs((exp - iat) as u64))
                }
                _ => None,
            })
            .unwrap_or(Duration::from_secs(240));
        Self {
            token,
            ttl,
            got_at: Instant::now(),
            got_wall: std::time::SystemTime::now(),
        }
    }

    fn remaining(&self) -> Duration {
        let monotonic = self.got_at.elapsed();
        let wall = std::time::SystemTime::now()
            .duration_since(self.got_wall)
            .unwrap_or(Duration::ZERO);
        self.ttl.saturating_sub(monotonic.max(wall))
    }
}

struct AuthInner {
    config: AuthConfig,
    /// `Some(client_id)` = WorkOS mode; `None` = dev mode.
    workos: Option<String>,
    /// Whether construction loaded a parseable WorkOS session. This is an
    /// immutable startup fact: refresh or sign-out must not rewrite it.
    loaded_workos_session: bool,
    http: reqwest::Client,
    state_tx: watch::Sender<AuthState>,
    token_tx: watch::Sender<u64>,
    stored: Mutex<Option<StoredSession>>,
    access: Mutex<Option<AccessEntry>>,
    /// Pending OAuth states plus the cancellation generation that fences code
    /// exchanges already in flight when sign-out occurs.
    sign_in: Mutex<SignInLifecycle>,
    /// Single-flight refresh: WorkOS refresh tokens are single-use (rotated per
    /// exchange); two concurrent refreshes would race and could revoke the session.
    refresh_gate: tokio::sync::Mutex<()>,
    /// Loopback callback listener port, bound lazily on the first headed sign-in.
    loopback: tokio::sync::Mutex<Option<u16>>,
}

#[derive(Default)]
struct SignInLifecycle {
    generation: u64,
    pending: HashMap<String, Instant>,
}

/// The auth service — cheap to clone by `Arc`.
#[derive(Clone)]
pub struct Auth {
    inner: Arc<AuthInner>,
}

impl Auth {
    /// Build from config: dev mode unless a WorkOS client id is configured.
    pub fn new(config: AuthConfig) -> Self {
        let workos = config
            .workos_client_id
            .clone()
            .filter(|s| !s.trim().is_empty());
        let session_file = config.data_dir.join("session.json");
        let stored: Option<StoredSession> = if workos.is_some() {
            std::fs::read_to_string(&session_file)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
        } else {
            None
        };
        let initial = match (&workos, &stored) {
            (None, _) => AuthState::SignedIn {
                user: AuthUser {
                    id: config.dev_user_id.clone(),
                    email: config.dev_user_id.clone(),
                    name: None,
                },
                org_id: None,
            },
            (Some(_), Some(session)) => state_for(session.user.clone(), session.org_id.clone()),
            (Some(_), None) => AuthState::SignedOut,
        };
        let loaded_workos_session = workos.is_some() && stored.is_some();
        let (state_tx, _) = watch::channel(initial);
        let (token_tx, _) = watch::channel(0);
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            inner: Arc::new(AuthInner {
                config,
                workos,
                loaded_workos_session,
                http,
                state_tx,
                token_tx,
                stored: Mutex::new(stored),
                access: Mutex::new(None),
                sign_in: Mutex::new(SignInLifecycle::default()),
                refresh_gate: tokio::sync::Mutex::new(()),
                loopback: tokio::sync::Mutex::new(None),
            }),
        }
    }

    /// Like [`Auth::new`], but additionally probes `{edge}/health`: an edge running in
    /// dev auth mode forces dev mode even when a client id is configured (matching the
    /// edge's "bearer = user id" verification).
    pub async fn detect(mut config: AuthConfig) -> Self {
        if config.workos_client_id.is_some() {
            #[derive(Deserialize)]
            struct Health {
                auth: Option<String>,
            }
            let url = format!("{}/health", config.edge_url.trim_end_matches('/'));
            let probe = async {
                reqwest::Client::new()
                    .get(&url)
                    .timeout(Duration::from_secs(3))
                    .send()
                    .await
                    .ok()?
                    .json::<Health>()
                    .await
                    .ok()
            };
            if let Some(health) = probe.await
                && health.auth.as_deref() == Some("dev")
            {
                tracing::info!("auth: edge is in dev mode — using dev bearer");
                config.workos_client_id = None;
            }
        }
        Self::new(config)
    }

    pub fn workos_enabled(&self) -> bool {
        self.inner.workos.is_some()
    }

    /// True when construction loaded a parseable persisted WorkOS session.
    /// The value stays true even if a later refresh revokes that session.
    pub fn loaded_workos_session(&self) -> bool {
        self.inner.loaded_workos_session
    }

    /// Live auth status (current value + changes).
    pub fn watch_state(&self) -> watch::Receiver<AuthState> {
        self.inner.state_tx.subscribe()
    }

    pub fn state(&self) -> AuthState {
        self.inner.state_tx.borrow().clone()
    }

    /// The signed-in user id — the identity that scopes workspace rooms
    /// (`ws3/{orgId}/{userId}`) and local storage (`orgs/{org}/{user}/`).
    /// Dev mode mirrors the edge's dev-bearer parsing (`user@org` → `user`,
    /// a bare token IS the user id). `None` = signed out (WorkOS only).
    pub fn user_id(&self) -> Option<String> {
        if self.inner.workos.is_none() {
            let dev = &self.inner.config.dev_user_id;
            return Some(dev.split('@').next().unwrap_or(dev).to_string());
        }
        self.state().user().map(|u| u.id.clone())
    }

    /// Current bearer for edge rooms / the device relay — `None` when signed out.
    /// Dev mode: the configured user id. WorkOS: cached access token, refreshed when
    /// it has under 30s left.
    pub async fn access_token(&self) -> Option<String> {
        if self.inner.workos.is_none() {
            return Some(self.inner.config.dev_user_id.clone());
        }
        if let Some(entry) = &*lock(&self.inner.access)
            && entry.remaining() > TOKEN_SLACK
        {
            return Some(entry.token.clone());
        }
        match self.refresh(None).await {
            Ok(token) => token,
            Err(err) => {
                tracing::warn!(error = %err, "auth: refresh failed");
                None
            }
        }
    }

    /// Sleep-until-near-expiry refresh loop so long-lived dials (relay, rooms) always
    /// have a live token to present on reconnect. No-op task in dev mode.
    pub fn spawn_refresh_loop(&self) -> tokio::task::JoinHandle<()> {
        let auth = self.clone();
        tokio::spawn(async move {
            if auth.inner.workos.is_none() {
                return;
            }
            let mut state_rx = auth.watch_state();
            let mut wake = zeron_sync::wake::subscribe();
            let mut online = zeron_sync::wake::subscribe_online();
            loop {
                if !state_rx.borrow().is_signed_in() {
                    if state_rx.changed().await.is_err() {
                        return;
                    }
                    continue;
                }
                let remaining = lock(&auth.inner.access)
                    .as_ref()
                    .map(AccessEntry::remaining)
                    .unwrap_or(Duration::ZERO);
                let wait = remaining.saturating_sub(Duration::from_secs(60));
                if wait > Duration::ZERO {
                    // Re-evaluate at least once a minute rather than parking
                    // on one long timer: tokio timers ride the monotonic
                    // clock, which excludes system suspend — a laptop waking
                    // from sleep would otherwise wait the WHOLE original
                    // duration again before noticing the (wall-expired) token.
                    let wait = wait.min(Duration::from_secs(60));
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => { continue; }
                        changed = state_rx.changed() => {
                            if changed.is_err() { return; }
                            continue;
                        }
                        // Wake: the cached token is almost certainly
                        // wall-expired — refresh NOW so the reconnecting
                        // rooms/relays dial with live credentials instead of
                        // discovering staleness one 401 at a time.
                        _ = wake.recv() => {}
                    }
                }
                if let Err(err) = auth.refresh(None).await {
                    tracing::warn!(error = %err, "auth: background refresh failed");
                    // A failed refresh is usually the network, not WorkOS —
                    // retry the moment connectivity returns (online bus)
                    // instead of always waiting out the full pause.
                    while online.try_recv().is_ok() {}
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                        _ = wake.recv() => {}
                        _ = online.recv() => {}
                    }
                }
            }
        })
    }

    // -- sign-in flows ------------------------------------------------------

    /// Begin a headed sign-in: returns the Woozlit login URL whose `callback`
    /// is our loopback server (bound lazily on an ephemeral port).
    pub async fn start_sign_in(&self) -> Result<String, EngineError> {
        if self.inner.workos.is_none() {
            return Ok(String::new()); // dev mode: nothing to do
        }
        let port = self.ensure_loopback().await?;
        Ok(self.begin_woozlit_sign_in(&format!("http://127.0.0.1:{port}/callback")))
    }

    /// Same Woozlit login URL as headed sign-in. CLI uses the loopback callback
    /// (open the URL on this machine); there is no paste-code page anymore.
    pub fn start_headless_sign_in(&self) -> String {
        if self.inner.workos.is_none() {
            return String::new();
        }
        // Headless without a bound loopback can't receive the bounce; callers
        // that can open a browser should use `start_sign_in`. This helper is
        // kept for tests that inject `state.token` via `complete_sign_in`.
        self.begin_woozlit_sign_in("http://127.0.0.1/callback")
    }

    /// Finish a sign-in with `state.token` (Firebase custom token). The state
    /// half must match a sign-in started HERE.
    pub async fn complete_sign_in(&self, pasted: &str) -> Result<(), EngineError> {
        if self.inner.workos.is_none() {
            return Ok(());
        }
        let trimmed = pasted.trim();
        let (state, token) = trimmed.split_once('.').unwrap_or(("", ""));
        if state.is_empty() || token.is_empty() {
            return Err(EngineError::Other(
                "invalid or expired sign-in — start again from Cosmos".into(),
            ));
        }
        let Some(generation) = self.take_pending(state) else {
            return Err(EngineError::Other(
                "invalid or expired sign-in — start again from Cosmos".into(),
            ));
        };
        let result = self.exchange_woozlit_token(token, None, None, None).await?;
        self.finish_sign_in(result, generation)
    }

    pub fn sign_out(&self) {
        let mut sign_in = lock(&self.inner.sign_in);
        sign_in.generation = sign_in.generation.wrapping_add(1);
        sign_in.pending.clear();
        *lock(&self.inner.stored) = None;
        *lock(&self.inner.access) = None;
        self.persist::<&StoredSession>(None);
        self.inner.state_tx.send_replace(AuthState::SignedOut);
        self.inner
            .token_tx
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    // -- organizations ------------------------------------------------------

    pub async fn list_orgs(&self) -> Result<Vec<OrgMembership>, EngineError> {
        if self.inner.workos.is_none() {
            return Ok(Vec::new());
        }
        // Woozlit accounts are personal: one workspace, id = Firebase uid.
        Ok(self
            .state()
            .user()
            .map(|user| {
                vec![OrgMembership {
                    id: user.id.clone(),
                    organization_id: user.id.clone(),
                    name: user.name.clone().unwrap_or_else(|| user.email.clone()),
                }]
            })
            .unwrap_or_default())
    }

    /// Create an org — Woozlit has no org directory; the personal workspace
    /// is created implicitly at sign-in.
    pub async fn create_org(&self, _name: &str) -> Result<(), EngineError> {
        Ok(())
    }

    /// Scope the session to an org. Woozlit personal workspaces are already
    /// scoped to the Firebase uid; anything else is a no-op success.
    pub async fn select_org(&self, _organization_id: &str) -> Result<(), EngineError> {
        Ok(())
    }

    // -- internals ----------------------------------------------------------

    fn begin_woozlit_sign_in(&self, callback_base: &str) -> String {
        let state = uuid::Uuid::new_v4().to_string();
        {
            let mut sign_in = lock(&self.inner.sign_in);
            let cutoff = Instant::now();
            sign_in
                .pending
                .retain(|_, at| cutoff.duration_since(*at) < SIGN_IN_TTL);
            sign_in.pending.insert(state.clone(), cutoff);
        }
        let sep = if callback_base.contains('?') {
            '&'
        } else {
            '?'
        };
        let callback = format!("{callback_base}{sep}state={state}");
        format!(
            "{}/login?desktop=1&callback={}",
            self.inner.config.woozlit_origin.trim_end_matches('/'),
            url_encode(&callback)
        )
    }

    /// Consume a pending sign-in state and capture its cancellation generation.
    /// `None` means unknown/expired (CSRF check).
    fn take_pending(&self, state: &str) -> Option<u64> {
        let mut sign_in = lock(&self.inner.sign_in);
        let now = Instant::now();
        sign_in
            .pending
            .retain(|_, at| now.duration_since(*at) < SIGN_IN_TTL);
        sign_in.pending.remove(state)?;
        Some(sign_in.generation)
    }

    async fn exchange_woozlit_token(
        &self,
        token: &str,
        user_id: Option<&str>,
        email: Option<&str>,
        display_name: Option<&str>,
    ) -> Result<SignInResult, EngineError> {
        let (access_token, refresh_token, local_id) = if is_firebase_id_token(token) {
            let sub = jwt_claims(token).and_then(|c| c.sub).unwrap_or_default();
            (token.to_string(), String::new(), sub)
        } else {
            self.sign_in_with_custom_token(token).await?
        };
        let claims = jwt_claims(&access_token);
        let id = user_id
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .or_else(|| claims.as_ref().and_then(|c| c.sub.clone()))
            .or_else(|| (!local_id.is_empty()).then_some(local_id))
            .unwrap_or_else(|| "woozlit-user".into());
        let email = email
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .or_else(|| claims.as_ref().and_then(|c| c.email.clone()))
            .unwrap_or_else(|| format!("{id}@woozlit.com"));
        let name = display_name.map(str::to_string).filter(|s| !s.is_empty());
        Ok(SignInResult {
            user: AuthUser { id, email, name },
            access_token,
            refresh_token,
        })
    }

    async fn sign_in_with_custom_token(
        &self,
        custom_token: &str,
    ) -> Result<(String, String, String), EngineError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            token: &'a str,
            return_secure_token: bool,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Reply {
            id_token: String,
            refresh_token: String,
            #[serde(default)]
            local_id: String,
        }
        let url = self.inner.config.custom_token_url();
        let res = self
            .inner
            .http
            .post(&url)
            .json(&Body {
                token: custom_token,
                return_secure_token: true,
            })
            .send()
            .await
            .map_err(|e| EngineError::Other(format!("could not reach Woozlit auth: {e}")))?;
        if !res.status().is_success() {
            return Err(EngineError::Other(format!(
                "Woozlit token exchange failed ({})",
                res.status().as_u16()
            )));
        }
        let body: Reply = res
            .json()
            .await
            .map_err(|e| EngineError::Other(format!("malformed Woozlit auth response: {e}")))?;
        Ok((body.id_token, body.refresh_token, body.local_id))
    }

    fn finish_sign_in(&self, result: SignInResult, generation: u64) -> Result<(), EngineError> {
        // Serialize the final commit with sign-out. A callback can consume its
        // OAuth state and spend time exchanging the token; if cancellation wins
        // during that await, its old generation must never restore credentials.
        let sign_in = lock(&self.inner.sign_in);
        if sign_in.generation != generation {
            return Err(EngineError::Other(
                "sign-in was canceled — start again from Cosmos".into(),
            ));
        }
        // Woozlit has no org directory: the Firebase uid IS the workspace.
        let org_id = jwt_claims(&result.access_token)
            .and_then(|c| c.org_id)
            .or_else(|| Some(result.user.id.clone()));
        *lock(&self.inner.access) = Some(AccessEntry::fresh(result.access_token));
        let session = StoredSession {
            refresh_token: result.refresh_token,
            user: result.user.clone(),
            org_id: org_id.clone(),
        };
        self.persist(Some(&session));
        *lock(&self.inner.stored) = Some(session);
        tracing::info!(email = %result.user.email, org = org_id.as_deref().unwrap_or("<none>"),
            "auth: signed in");
        self.inner
            .state_tx
            .send_replace(state_for(result.user, org_id));
        self.inner
            .token_tx
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        Ok(())
    }

    /// Refresh the session (single-flight). `organization_id` migrates the WorkOS
    /// session to that org; routine refreshes keep the current scope. Returns the new
    /// access token, `None` when signed out / the refresh could not run.
    async fn refresh(&self, organization_id: Option<&str>) -> Result<Option<String>, EngineError> {
        let _gate = self.inner.refresh_gate.lock().await;
        // Re-check under the gate: the refresh we queued behind may have done the work.
        if organization_id.is_none()
            && let Some(entry) = &*lock(&self.inner.access)
            && entry.remaining() > TOKEN_SLACK
        {
            return Ok(Some(entry.token.clone()));
        }
        let Some(refresh_token) = lock(&self.inner.stored)
            .as_ref()
            .map(|s| s.refresh_token.clone())
            .filter(|s| !s.is_empty())
        else {
            return Ok(None);
        };
        let url = self.inner.config.refresh_url();
        let res = self
            .inner
            .http
            .post(&url)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(format!(
                "grant_type=refresh_token&refresh_token={}",
                url_encode(&refresh_token)
            ))
            .send()
            .await;
        let res = match res {
            Ok(res) => res,
            Err(err) => {
                // Network failure is transient: keep the session, but surface the
                // error so the background loop applies its retry delay.
                return Err(EngineError::Other(format!(
                    "could not reach Woozlit auth during refresh: {err}"
                )));
            }
        };
        let status = res.status().as_u16();
        if (400..500).contains(&status) && organization_id.is_none() {
            // A definitive 4xx means the refresh token itself is dead.
            tracing::warn!(
                status,
                "auth: refresh rejected — session revoked; signing out"
            );
            self.sign_out();
            return Ok(None);
        }
        if !res.status().is_success() {
            return Err(EngineError::Other(format!("refresh failed ({status})")));
        }
        #[derive(Deserialize)]
        struct Tokens {
            #[serde(default, alias = "idToken", alias = "id_token")]
            id_token: Option<String>,
            #[serde(default, alias = "accessToken", alias = "access_token")]
            access_token: Option<String>,
            #[serde(default, alias = "refreshToken", alias = "refresh_token")]
            refresh_token: Option<String>,
        }
        let tokens: Tokens = res
            .json()
            .await
            .map_err(|e| EngineError::Other(format!("malformed refresh response: {e}")))?;
        let access_token = tokens
            .id_token
            .or(tokens.access_token)
            .ok_or_else(|| EngineError::Other("refresh returned no access token".into()))?;
        let new_refresh = tokens.refresh_token.unwrap_or(refresh_token);
        let org_id = jwt_claims(&access_token)
            .and_then(|c| c.org_id)
            .or_else(|| {
                lock(&self.inner.stored)
                    .as_ref()
                    .and_then(|s| s.org_id.clone())
            });
        let entry = AccessEntry::fresh(access_token.clone());
        tracing::info!(ttl_s = entry.ttl.as_secs(), "auth: access token refreshed");
        *lock(&self.inner.access) = Some(entry);
        let (user, org_changed) = {
            let mut stored = lock(&self.inner.stored);
            match stored.as_mut() {
                Some(session) => {
                    let changed = session.org_id != org_id;
                    session.refresh_token = new_refresh;
                    session.org_id = org_id.clone();
                    (session.user.clone(), changed)
                }
                None => return Ok(None), // signed out mid-refresh
            }
        };
        self.persist(lock(&self.inner.stored).as_ref());
        if org_changed {
            self.inner.state_tx.send_replace(state_for(user, org_id));
        }
        self.inner
            .token_tx
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        Ok(Some(access_token))
    }

    fn session_file(&self) -> PathBuf {
        self.inner.config.data_dir.join("session.json")
    }

    /// Persist (0600) or remove the stored session. Never panics: a disk error degrades
    /// to a logged warning, not a crash mid-refresh.
    fn persist<S: std::borrow::Borrow<StoredSession>>(&self, session: Option<S>) {
        let path = self.session_file();
        let outcome = match session {
            Some(session) => serde_json::to_vec(session.borrow())
                .map_err(std::io::Error::other)
                .and_then(|bytes| write_private(&path, &bytes)),
            None => match std::fs::remove_file(&path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
                _ => Ok(()),
            },
        };
        if let Err(err) = outcome {
            tracing::warn!(error = %err, "auth: failed to persist session");
        }
    }

    #[allow(dead_code)]
    async fn authed_json<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, EngineError> {
        let token = self
            .access_token()
            .await
            .ok_or_else(|| EngineError::Other("not signed in".into()))?;
        let url = format!(
            "{}{}",
            self.inner.config.edge_url.trim_end_matches('/'),
            path
        );
        let mut req = self.inner.http.request(method, &url).bearer_auth(token);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req
            .send()
            .await
            .map_err(|e| EngineError::Other(format!("the edge is unreachable: {e}")))?;
        if !res.status().is_success() {
            return Err(EngineError::Other(format!(
                "workspace request failed ({})",
                res.status().as_u16()
            )));
        }
        res.json::<T>()
            .await
            .map_err(|e| EngineError::Other(format!("malformed response: {e}")))
    }

    // -- loopback callback server ------------------------------------------

    /// Bind the loopback callback listener (idempotent); returns its port.
    async fn ensure_loopback(&self) -> Result<u16, EngineError> {
        let mut slot = self.inner.loopback.lock().await;
        if let Some(port) = *slot {
            return Ok(port);
        }
        let requested = self.inner.config.callback_port.unwrap_or(0);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", requested))
            .await
            .map_err(|e| EngineError::Other(format!("sign-in callback bind failed: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| EngineError::Other(format!("sign-in callback addr: {e}")))?
            .port();
        *slot = Some(port);
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(loopback_loop(listener, weak));
        tracing::info!(port, "auth: sign-in callback listening");
        Ok(port)
    }
}

struct SignInResult {
    user: AuthUser,
    access_token: String,
    refresh_token: String,
}

fn state_for(user: AuthUser, org_id: Option<String>) -> AuthState {
    // Every user must belong to an organization before the product opens up; an org-less
    // session is `NeedsOrganization`, which the UI gates on.
    match org_id {
        Some(org_id) => AuthState::SignedIn {
            user,
            org_id: Some(org_id),
        },
        None => AuthState::NeedsOrganization { user },
    }
}

/// The relay/room token seam: `Auth` IS a [`zeron_rpc::TokenSource`], so the host relay
/// and link cache always dial with a fresh bearer after refreshes.
#[async_trait::async_trait]
impl zeron_rpc::TokenSource for Auth {
    async fn token(&self) -> Option<String> {
        if self.inner.workos.is_some() && !self.state().is_signed_in() {
            return None;
        }
        self.access_token().await
    }

    fn subscribe(&self) -> Option<watch::Receiver<u64>> {
        Some(self.inner.token_tx.subscribe())
    }
}

// ---------------------------------------------------------------------------
// Loopback HTTP (hand-rolled: no HTTP server dependency in the engine)
// ---------------------------------------------------------------------------

async fn loopback_loop(listener: tokio::net::TcpListener, inner: Weak<AuthInner>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            break;
        };
        let Some(inner) = inner.upgrade() else { break };
        tokio::spawn(async move {
            if let Err(err) = handle_loopback_conn(stream, Auth { inner }).await {
                tracing::debug!(error = %err, "auth: callback connection failed");
            }
        });
    }
}

async fn handle_loopback_conn(
    mut stream: tokio::net::TcpStream,
    auth: Auth,
) -> Result<(), std::io::Error> {
    // Read the request head (bounded; we only need the request line).
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "header read"))??;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let request_line = head.lines().next().unwrap_or_default();
    let target = request_line.split_whitespace().nth(1).unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    let (status, body) = if path != "/callback" {
        ("404 Not Found", page("Not found."))
    } else {
        let params: HashMap<String, String> = query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_string(), url_decode(v)))
            .collect();
        let token = params.get("token").or_else(|| params.get("code"));
        let state = params.get("state");
        let user_id = params.get("userId").map(String::as_str);
        let email = params.get("email").map(String::as_str);
        let display_name = params.get("displayName").map(String::as_str);
        let invalid_callback = || {
            (
                "400 Bad Request",
                page("Invalid or expired sign-in link. Start again from Cosmos."),
            )
        };
        match (token, state) {
            (Some(token), Some(state)) => match auth.take_pending(state) {
                Some(generation) => match auth
                    .exchange_woozlit_token(token, user_id, email, display_name)
                    .await
                {
                    Ok(result) => match auth.finish_sign_in(result, generation) {
                        Ok(()) => (
                            "200 OK",
                            page("Signed in. You can close this tab and return to Cosmos."),
                        ),
                        Err(err) => {
                            tracing::info!(error = %err, "auth: discarded canceled callback exchange");
                            (
                                "409 Conflict",
                                page(
                                    "This sign-in was canceled. Start again from Cosmos if you still want to enable sync.",
                                ),
                            )
                        }
                    },
                    Err(err) => {
                        tracing::warn!(error = %err, "auth: loopback code exchange failed");
                        (
                            "502 Bad Gateway",
                            page("Sign-in failed during token exchange — check the Cosmos logs."),
                        )
                    }
                },
                None => invalid_callback(),
            },
            _ => invalid_callback(),
        }
    };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

fn page(message: &str) -> String {
    format!("<html><body style='font-family:sans-serif;padding:2rem'>{message}</body></html>")
}

// ---------------------------------------------------------------------------
// Small utilities (JWT claims, base64url, URL encoding, 0600 writes)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct JwtClaims {
    #[serde(default)]
    exp: Option<i64>,
    #[serde(default)]
    iat: Option<i64>,
    #[serde(default)]
    org_id: Option<String>,
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    iss: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

fn is_firebase_id_token(token: &str) -> bool {
    jwt_claims(token)
        .and_then(|c| c.iss)
        .is_some_and(|iss| iss.contains("securetoken.google.com"))
}

/// Decode (without verifying — the edge verifies) the JWT payload claims. Total: a
/// malformed token yields `None`, never a panic.
fn jwt_claims(token: &str) -> Option<JwtClaims> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64url_decode(payload)?;
    serde_json::from_slice(&bytes).ok()
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4 + 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Write a file readable only by the owner (0600). On non-unix targets a plain write.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        // An existing file keeps its old mode through OpenOptions — enforce 0600 anyway.
        file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        file.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_round_trips_jwt_payload() {
        let payload = br#"{"exp":100,"iat":40,"org_id":"org_1"}"#;
        // Standard base64url without padding (as JWTs use).
        let encoded = {
            const ALPHABET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut out = String::new();
            for chunk in payload.chunks(3) {
                let b = [
                    chunk[0],
                    *chunk.get(1).unwrap_or(&0),
                    *chunk.get(2).unwrap_or(&0),
                ];
                let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
                out.push(ALPHABET[(n >> 18) as usize & 63] as char);
                out.push(ALPHABET[(n >> 12) as usize & 63] as char);
                if chunk.len() > 1 {
                    out.push(ALPHABET[(n >> 6) as usize & 63] as char);
                }
                if chunk.len() > 2 {
                    out.push(ALPHABET[n as usize & 63] as char);
                }
            }
            out
        };
        assert_eq!(
            base64url_decode(&encoded).as_deref(),
            Some(payload.as_slice())
        );
        let token = format!("h.{encoded}.sig");
        let claims = jwt_claims(&token).expect("claims decode");
        assert_eq!(claims.exp, Some(100));
        assert_eq!(claims.iat, Some(40));
        assert_eq!(claims.org_id.as_deref(), Some("org_1"));
    }

    #[test]
    fn url_coding_round_trips() {
        let raw = "http://127.0.0.1:1234/callback?x=a b&y=%";
        assert_eq!(url_decode(&url_encode(raw)), raw);
        assert_eq!(url_encode("a b"), "a%20b");
    }

    #[test]
    fn auth_state_serializes_as_proto_shape() {
        let user = AuthUser {
            id: "u1".into(),
            email: "u@x".into(),
            name: None,
        };
        let signed_in = AuthState::SignedIn {
            user: user.clone(),
            org_id: Some("org_1".into()),
        };
        let value = serde_json::to_value(&signed_in).expect("json");
        assert_eq!(
            value,
            serde_json::json!({
                "state": "signedIn",
                "user": {"id": "u1", "email": "u@x", "name": null},
                "orgId": "org_1",
            })
        );
        // The proto type itself round-trips the emitted value.
        let parsed: zeron_proto::AuthState = serde_json::from_value(value).expect("proto parse");
        assert!(matches!(parsed, zeron_proto::AuthState::SignedIn { .. }));
        assert_eq!(
            serde_json::to_value(AuthState::SignedOut).expect("json"),
            serde_json::json!({"state": "signedOut"})
        );
        assert_eq!(
            serde_json::to_value(AuthState::NeedsOrganization { user }).expect("json"),
            serde_json::json!({
                "state": "needsOrganization",
                "user": {"id": "u1", "email": "u@x", "name": null},
            })
        );
    }
}

#[async_trait::async_trait]
impl zeron_preview::signaling::TokenSource for Auth {
    async fn token(&self) -> Option<String> {
        self.access_token().await
    }
}
