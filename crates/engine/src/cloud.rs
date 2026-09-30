//! Cloud / GitHub agent mode — seat config, BYOK keys, probes, exec, GitHub
//! API, automations, and the remote-run ledger.
//!
//! Layout on disk (inside the engine data dir):
//! - `cloud.json` — non-secret config: seats, providers (has_key flags only),
//!   automations, the run ledger.
//! - `cloud-keys.json` — secrets only: provider API keys, Box tokens, GitHub
//!   PATs. Written 0600 on Unix; never logged, never returned by RPC (the UI
//!   receives `has_*` booleans).
//!
//! The desktop never hosts the seat: SSH seats exec over the user's own
//! `~/.ssh/config` alias (agent forwarding; private keys never leave the
//! agent), GitHub seats ride the REST API, Box seats ride their token API
//! when an endpoint is configured.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use zeron_proto::{
    ByokProvider, CloudAutomation, CloudRun, CloudSeatBinding, CloudSeatKind, SeatCapabilities,
    SeatProbe,
};

use crate::EngineError;

const CONFIG_FILE: &str = "cloud.json";
const KEYS_FILE: &str = "cloud-keys.json";
const RUNS_FILE: &str = "cloud-runs.json";
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// `docker exec` container prefix (spec): `cosmos-cloud-<workspace>`.
pub const CONTAINER_PREFIX: &str = "cosmos-cloud";

// ---------------------------------------------------------------------------
// Stored shapes
// ---------------------------------------------------------------------------

/// One configured seat (non-secret). Keys live in [`SeatSecrets`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SeatConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<CloudSeatKind>,
    pub label: String,
    /// SSH config alias (BYO VPS) — the private key stays in the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_alias: Option<String>,
    /// Container name on the SSH host; default `cosmos-cloud-<id>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    /// Workspace path inside the container / on the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
    /// `owner/name` (GitHub seats; SSH seats may bind one for the PR pane).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Box provider API endpoint (https://…); empty = token-only seat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_endpoint: Option<String>,
    /// Remote `cua-driver` HTTP endpoint stamped on the seat — enables the
    /// Computer pane's observe/act (`zeron_computer::remote`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cua_endpoint: Option<String>,
    /// An emulator/stream endpoint the seat advertises (Android simulator on
    /// Linux seats); the Simulator pane stays disabled without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub simulator_endpoint: Option<String>,
    /// The harness CLI to run on the seat (`claude -p`, `opencode run`, …).
    /// Empty = the seat only exposes Files/Changes/Shell/PRs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_command: Option<String>,
    /// Launch the harness detached (`nohup`/`docker exec -d` into a run log)
    /// so the run survives this app closing. Streaming mode tails the log —
    /// the transcript backfills on reconnect.
    #[serde(default)]
    pub detached: bool,
    /// Boat provider sandbox id (`bx_…`) once a box exists — distinct from the
    /// seat's own id; `None` until the first run/probe creates one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub box_id: Option<String>,
    /// Public `host:port` the provider stamped for direct SSH (Boat
    /// `sshEndpoint`); enables the Shell pane's real PTY command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_endpoint: Option<String>,
    /// Railway trial deadline — the seat dies here (60-minute build window);
    /// the header chip and banner count down to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Railway's "keep it" link (`railway.com/ssh-signup?code=…`) — opens in
    /// the user's browser, never embedded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_url: Option<String>,
    /// A URL the seat serves for previews (Railway's `*.up.railway.app`
    /// preview; Boat's hosted-port URL) — the Open-preview action's target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_url: Option<String>,
    /// When the seat's own keep-prompt was last dismissed ("Keep this
    /// computer" → Boat) — persists "asked once" across restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_prompted_at: Option<DateTime<Utc>>,
    /// Local path of the workspace snapshot pulled off this seat before a
    /// Railway trial died (`seat-backups/<id>.tar.gz` under the data dir) —
    /// a later seat restores it so trial work is never lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_at: Option<DateTime<Utc>>,
    /// Rate-limits snapshot retries — a failed pull retries every ~2 minutes
    /// until the seat dies at its deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup_attempted_at: Option<DateTime<Utc>>,
    /// Stamped when this seat ingested a snapshot from a previous seat —
    /// one restore per seat so manual edits aren't clobbered on reruns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_probe: Option<SeatProbe>,
}

impl SeatConfig {
    /// The seat's container name — `cosmos-cloud-<id>` per spec when unset.
    pub fn container_name(&self) -> String {
        self.container
            .clone()
            .filter(|c| !c.trim().is_empty())
            .unwrap_or_else(|| format!("{CONTAINER_PREFIX}-{}", self.id))
    }

    /// Seat working directory: explicit `workspace_path`, else `/workspace`
    /// inside the container.
    pub fn workspace(&self) -> String {
        self.workspace_path
            .clone()
            .filter(|w| !w.trim().is_empty())
            .unwrap_or_else(|| "/workspace".to_string())
    }

    /// The seat as one chat sees it: a chat bound to a repo works in that
    /// repo's checkout on the seat instead of the seat's default workspace.
    pub fn scoped(mut self, workspace: Option<&str>, repo: Option<&str>) -> Self {
        if let Some(workspace) = workspace.map(str::trim).filter(|w| !w.is_empty()) {
            self.workspace_path = Some(workspace.to_string());
        }
        if let Some(repo) = repo.map(str::trim).filter(|r| !r.is_empty()) {
            self.repo = Some(repo.to_string());
        }
        self
    }

    /// A Railway trial seat past its 60-minute build window — the seat is
    /// dead (or in the 24h claim grace); runs must not dispatch to it.
    pub fn expired(&self) -> bool {
        self.kind == Some(CloudSeatKind::Railway)
            && self
                .expires_at
                .is_some_and(|deadline| deadline <= Utc::now())
    }

    /// The seat's honest capability surface — every pane gates on this.
    pub fn capabilities(&self) -> SeatCapabilities {
        match self.kind {
            Some(CloudSeatKind::Railway) => {
                let alive = !self.expired();
                SeatCapabilities {
                    exec: alive,
                    shell: alive,
                    files: alive,
                    git: alive,
                    pull_requests: self.repo.is_some(),
                    computer: self.cua_endpoint.is_some(),
                    simulator: false,
                    // Detached (`nohup`) runs outlive the app — bounded by
                    // the trial's own expiry, which the countdown covers.
                    keeps_running: alive && self.detached,
                }
            }
            Some(CloudSeatKind::Ssh) => SeatCapabilities {
                exec: self.ssh_alias.is_some(),
                shell: self.ssh_alias.is_some(),
                files: self.ssh_alias.is_some(),
                git: self.ssh_alias.is_some(),
                pull_requests: self.repo.is_some(),
                computer: self.cua_endpoint.is_some(),
                simulator: self.simulator_endpoint.is_some(),
                keeps_running: self.ssh_alias.is_some(),
            },
            Some(CloudSeatKind::GitHub) => SeatCapabilities {
                exec: false,
                shell: false,
                files: self.repo.is_some(),
                git: self.repo.is_some(),
                pull_requests: self.repo.is_some(),
                computer: self.cua_endpoint.is_some(),
                simulator: false,
                // A dispatched workflow keeps running after the app closes.
                keeps_running: true,
            },
            Some(CloudSeatKind::Box) => SeatCapabilities {
                // Real exec once a sandbox exists (`/commands`); the token
                // alone is enough for the first run to provision one.
                exec: self.box_id.is_some(),
                // Direct ssh lands on the sandbox once the provider stamps an
                // endpoint; otherwise the Shell pane is a command log.
                shell: self.ssh_endpoint.is_some(),
                files: self.box_id.is_some(),
                git: self.box_id.is_some(),
                pull_requests: self.repo.is_some(),
                computer: self.cua_endpoint.is_some(),
                simulator: false,
                // A hosted box outlives the desktop by definition.
                keeps_running: true,
            },
            None => SeatCapabilities::default(),
        }
    }
}

/// Ensure a remote seat's workspace exists before running a command there.
pub fn command_in_workspace(workspace: &str, command: &str) -> String {
    let workspace = shell_word(workspace);
    format!("mkdir -p {workspace} && cd {workspace} && {command}")
}

/// Shell prelude that makes `repo` (`owner/name`) the current checkout: a
/// first run clones it through `$GITHUB_TOKEN`, later runs reuse it. The
/// token never lands in `.git/config` — the credential helper reads it from
/// the run's environment, so the agent can push while the run lives.
pub fn repo_checkout_prelude(repo: &str) -> String {
    let url = shell_word(&format!("https://github.com/{repo}.git"));
    let helper =
        shell_word("!f() { echo username=x-access-token; echo \"password=$GITHUB_TOKEN\"; }; f");
    format!(
        "if [ ! -d .git ]; then git -c credential.helper= -c credential.helper={helper} clone --quiet {url} . || exit 1; fi; \
git config credential.helper {helper}"
    )
}

/// Per-provider row (non-secret); the key itself lives in `cloud-keys.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderConfig {
    pub provider: Option<ByokProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// "Use this key for Cloud sessions" vs "use local CLI login".
    #[serde(default)]
    pub use_for_cloud: bool,
    /// Set at read time — never stored (the key file is the truth), but the
    /// RPC view does serialize it so the UI can render "key saved".
    #[serde(skip_deserializing)]
    pub has_key: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct CloudConfigFile {
    seats: Vec<SeatConfig>,
    providers: Vec<ProviderConfig>,
    automations: Vec<CloudAutomation>,
    default_seat: Option<String>,
}

/// Secrets — `cloud-keys.json`, 0600 on Unix. Everything optional; a missing
/// file just means nothing is configured yet.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct CloudKeysFile {
    /// provider id → api key / access token.
    provider_keys: HashMap<String, String>,
    /// seat id → secrets (box token / github pat / cua bearer).
    seat_tokens: HashMap<String, SeatSecrets>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SeatSecrets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    box_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github_pat: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cua_token: Option<String>,
}

/// The run ledger file — `cloud-runs.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RunsFile {
    runs: Vec<CloudRun>,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CloudStore {
    inner: Arc<CloudStoreInner>,
}

struct CloudStoreInner {
    dir: PathBuf,
    state: RwLock<CloudState>,
}

#[derive(Default)]
struct CloudState {
    config: CloudConfigFile,
    keys: CloudKeysFile,
    runs: RunsFile,
    load_error: Option<String>,
}

impl CloudStore {
    pub fn load(dir: &Path) -> Self {
        let mut errors = Vec::new();
        let read = |name: &str, errors: &mut Vec<String>| -> Option<serde_json::Value> {
            let path = dir.join(name);
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
                Err(error) => {
                    errors.push(format!("{name}: {error}"));
                    return None;
                }
            };
            match serde_json::from_slice(&bytes) {
                Ok(value) => Some(value),
                Err(error) => {
                    errors.push(format!("{name}: malformed JSON ({error})"));
                    None
                }
            }
        };
        let mut config = read(CONFIG_FILE, &mut errors)
            .and_then(|v| match serde_json::from_value(v) {
                Ok(value) => Some(value),
                Err(error) => {
                    errors.push(format!("{CONFIG_FILE}: invalid shape ({error})"));
                    None
                }
            })
            .unwrap_or_default();
        if dedupe_railway_seats(&mut config)
            && let Err(error) = Self::write(dir, CONFIG_FILE, &config, false)
        {
            errors.push(error);
        }
        let keys = read(KEYS_FILE, &mut errors)
            .and_then(|v| match serde_json::from_value(v) {
                Ok(value) => Some(value),
                Err(error) => {
                    errors.push(format!("{KEYS_FILE}: invalid shape ({error})"));
                    None
                }
            })
            .unwrap_or_default();
        let runs = read(RUNS_FILE, &mut errors)
            .and_then(|v| match serde_json::from_value(v) {
                Ok(value) => Some(value),
                Err(error) => {
                    errors.push(format!("{RUNS_FILE}: invalid shape ({error})"));
                    None
                }
            })
            .unwrap_or_default();
        let load_error = (!errors.is_empty()).then(|| errors.join("; "));
        if let Some(error) = &load_error {
            tracing::error!(error, "cloud store load failed");
        }
        Self {
            inner: Arc::new(CloudStoreInner {
                dir: dir.to_path_buf(),
                state: RwLock::new(CloudState {
                    config,
                    keys,
                    runs,
                    load_error,
                }),
            }),
        }
    }

    fn write<T: ?Sized + Serialize>(
        dir: &Path,
        name: &str,
        value: &T,
        _secret: bool,
    ) -> Result<(), String> {
        let path = dir.join(name);
        let bytes = serde_json::to_vec_pretty(value)
            .map_err(|e| format!("cloud store encode {name}: {e}"))?;
        let staging = path.with_extension("partial");
        let result = (|| {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(if _secret { 0o600 } else { 0o644 });
            }
            let mut file = options.open(&staging)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&staging, &path)
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_file(&staging);
            return Err(format!("cloud store write {name}: {e}"));
        }
        Ok(())
    }

    fn persist_config(&self) -> Result<(), String> {
        let state = self.inner.state.read().unwrap();
        Self::write(&self.inner.dir, CONFIG_FILE, &state.config, false)
    }

    fn persist_keys(&self) -> Result<(), String> {
        let state = self.inner.state.read().unwrap();
        Self::write(&self.inner.dir, KEYS_FILE, &state.keys, true)
    }

    fn persist_runs(&self) -> Result<(), String> {
        let state = self.inner.state.read().unwrap();
        Self::write(&self.inner.dir, RUNS_FILE, &state.runs, false)
    }

    // ── seats ────────────────────────────────────────────────────────────

    pub fn seats(&self) -> Vec<SeatConfig> {
        self.inner.state.read().unwrap().config.seats.clone()
    }

    pub fn seat(&self, id: &str) -> Option<SeatConfig> {
        self.seats().into_iter().find(|s| s.id == id)
    }

    /// Find the seat a chat binding names; tolerant of stale ids.
    pub fn seat_for(&self, binding: &CloudSeatBinding) -> Option<SeatConfig> {
        self.seat(&binding.seat_id)
            .map(|seat| seat.scoped(binding.workspace_path.as_deref(), binding.repo.as_deref()))
    }

    pub fn default_seat(&self) -> Option<SeatConfig> {
        let state = self.inner.state.read().unwrap();
        state
            .config
            .default_seat
            .as_deref()
            .and_then(|id| state.config.seats.iter().find(|s| s.id == id).cloned())
            .or_else(|| state.config.seats.first().cloned())
    }

    pub fn save_seat(&self, seat: SeatConfig) -> Result<(), String> {
        let mut state = self.inner.state.write().unwrap();
        match state.config.seats.iter_mut().find(|s| s.id == seat.id) {
            Some(existing) => {
                let probe = existing.last_probe.take();
                *existing = seat;
                existing.last_probe = probe.or(existing.last_probe.take());
            }
            None => state.config.seats.push(seat),
        }
        drop(state);
        self.persist_config()
    }

    pub fn remove_seat(&self, id: &str) -> Result<(), String> {
        let mut state = self.inner.state.write().unwrap();
        state.config.seats.retain(|s| s.id != id);
        state.keys.seat_tokens.remove(id);
        if state.config.default_seat.as_deref() == Some(id) {
            state.config.default_seat = None;
        }
        drop(state);
        self.persist_config()?;
        self.persist_keys()
    }

    pub fn record_probe(&self, seat_id: &str, probe: SeatProbe) -> Result<(), String> {
        let mut state = self.inner.state.write().unwrap();
        if let Some(seat) = state.config.seats.iter_mut().find(|s| s.id == seat_id) {
            seat.last_probe = Some(probe);
        }
        drop(state);
        self.persist_config()
    }

    // ── seat secrets ─────────────────────────────────────────────────────

    pub fn set_seat_token(
        &self,
        seat_id: &str,
        field: &str,
        value: Option<String>,
    ) -> Result<(), String> {
        let mut state = self.inner.state.write().unwrap();
        let secrets = state
            .keys
            .seat_tokens
            .entry(seat_id.to_string())
            .or_default();
        match field {
            "boxToken" | "box_token" => secrets.box_token = value.filter(|v| !v.is_empty()),
            "githubPat" | "github_pat" => secrets.github_pat = value.filter(|v| !v.is_empty()),
            "cuaToken" | "cua_token" => secrets.cua_token = value.filter(|v| !v.is_empty()),
            _ => {}
        }
        // An all-empty secrets row drops entirely so the file stays clean.
        if secrets.box_token.is_none()
            && secrets.github_pat.is_none()
            && secrets.cua_token.is_none()
        {
            state.keys.seat_tokens.remove(seat_id);
        }
        drop(state);
        self.persist_keys()
    }

    /// The shared GitHub PAT lives under a reserved seat key — "the existing
    /// GitHub integration" the repo picker + GitHub seats fall back to.
    /// Never serialized beyond the keys file.
    pub const SHARED_GITHUB_KEY: &'static str = "_github";

    pub fn github_pat(&self, seat_id: &str) -> Option<String> {
        let state = self.inner.state.read().unwrap();
        state
            .keys
            .seat_tokens
            .get(seat_id)
            .and_then(|s| s.github_pat.clone())
            .or_else(|| {
                state
                    .keys
                    .seat_tokens
                    .get(Self::SHARED_GITHUB_KEY)
                    .and_then(|s| s.github_pat.clone())
            })
    }

    pub fn box_token(&self, seat_id: &str) -> Option<String> {
        self.inner
            .state
            .read()
            .unwrap()
            .keys
            .seat_tokens
            .get(seat_id)
            .and_then(|s| s.box_token.clone())
    }

    pub fn cua_token(&self, seat_id: &str) -> Option<String> {
        self.inner
            .state
            .read()
            .unwrap()
            .keys
            .seat_tokens
            .get(seat_id)
            .and_then(|s| s.cua_token.clone())
    }

    fn seat_has_token(&self, seat_id: &str, field: &str) -> bool {
        let secrets = self
            .inner
            .state
            .read()
            .unwrap()
            .keys
            .seat_tokens
            .get(seat_id)
            .cloned();
        match field {
            "box" => secrets.and_then(|s| s.box_token).is_some(),
            "github" => {
                secrets.and_then(|s| s.github_pat.clone()).is_some()
                    || self.github_pat_shared().is_some()
            }
            _ => false,
        }
    }

    fn github_pat_shared(&self) -> Option<String> {
        self.inner
            .state
            .read()
            .unwrap()
            .keys
            .seat_tokens
            .get(Self::SHARED_GITHUB_KEY)
            .and_then(|s| s.github_pat.clone())
    }

    // ── providers ────────────────────────────────────────────────────────

    pub fn providers(&self) -> Vec<ProviderConfig> {
        let state = self.inner.state.read().unwrap();
        state
            .config
            .providers
            .iter()
            .cloned()
            .map(|mut p| {
                p.has_key = p
                    .provider
                    .map(|provider| state.keys.provider_keys.contains_key(provider.id()))
                    .unwrap_or(false);
                p
            })
            .collect()
    }

    pub fn provider(&self, provider: ByokProvider) -> Option<ProviderConfig> {
        self.providers()
            .into_iter()
            .find(|p| p.provider == Some(provider))
    }

    pub fn provider_key(&self, provider: ByokProvider) -> Option<String> {
        self.inner
            .state
            .read()
            .unwrap()
            .keys
            .provider_keys
            .get(provider.id())
            .cloned()
    }

    /// Save the non-secret row + (when `key` is `Some`) the key itself.
    pub fn save_provider(
        &self,
        mut config: ProviderConfig,
        key: Option<String>,
    ) -> Result<(), String> {
        let mut state = self.inner.state.write().unwrap();
        if let Some(key) = key.filter(|k| !k.trim().is_empty())
            && let Some(provider) = config.provider
        {
            state
                .keys
                .provider_keys
                .insert(provider.id().to_string(), key);
        }
        config.has_key = false;
        match state
            .config
            .providers
            .iter_mut()
            .find(|p| p.provider == config.provider)
        {
            Some(existing) => *existing = config,
            None => state.config.providers.push(config),
        }
        drop(state);
        self.persist_config()?;
        self.persist_keys()
    }

    pub fn remove_provider(&self, provider: ByokProvider) -> Result<(), String> {
        let mut state = self.inner.state.write().unwrap();
        state
            .config
            .providers
            .retain(|p| p.provider != Some(provider));
        state.keys.provider_keys.remove(provider.id());
        drop(state);
        self.persist_config()?;
        self.persist_keys()
    }

    /// Provider keys a cloud run should export to the seat — only rows the
    /// user toggled "use this key for Cloud sessions" on. With no cloud
    /// Anthropic key, a live Claude subscription login is forwarded as
    /// `CLAUDE_CODE_OAUTH_TOKEN` so `claude` on the seat runs under the
    /// user's existing account — no separate BYOK setup needed.
    pub fn cloud_env(&self) -> Vec<(String, String)> {
        let state = self.inner.state.read().unwrap();
        let mut env: Vec<(String, String)> = state
            .config
            .providers
            .iter()
            .filter(|p| p.use_for_cloud)
            .filter_map(|p| {
                let provider = p.provider?;
                let key = state.keys.provider_keys.get(provider.id())?;
                let env = match provider {
                    ByokProvider::Anthropic => "ANTHROPIC_API_KEY",
                    ByokProvider::OpenAi => "OPENAI_API_KEY",
                    ByokProvider::Xai => "XAI_API_KEY",
                    ByokProvider::OpenRouter => "OPENROUTER_API_KEY",
                    ByokProvider::DeepSeek => "DEEPSEEK_API_KEY",
                    ByokProvider::Xiaomi => "XIAOMI_API_KEY",
                    ByokProvider::ChatgptSub => "OPENAI_API_KEY",
                    ByokProvider::DevinSub => "DEVIN_API_KEY",
                    ByokProvider::Custom => "CUSTOM_API_KEY",
                };
                Some((env.to_string(), key.clone()))
            })
            .collect();
        let has_anthropic = env.iter().any(|(k, _)| k == "ANTHROPIC_API_KEY");
        drop(state);
        if !has_anthropic && let Some(token) = claude_oauth_token() {
            env.push(("CLAUDE_CODE_OAUTH_TOKEN".into(), token));
        }
        env
    }

    // ── automations ──────────────────────────────────────────────────────

    pub fn automations(&self) -> Vec<CloudAutomation> {
        self.inner.state.read().unwrap().config.automations.clone()
    }

    pub fn save_automation(&self, automation: CloudAutomation) {
        let mut state = self.inner.state.write().unwrap();
        match state
            .config
            .automations
            .iter_mut()
            .find(|a| a.id == automation.id)
        {
            Some(existing) => *existing = automation,
            None => state.config.automations.push(automation),
        }
        drop(state);
        self.persist_config();
    }

    pub fn remove_automation(&self, id: &str) {
        let mut state = self.inner.state.write().unwrap();
        state.config.automations.retain(|a| a.id != id);
        drop(state);
        self.persist_config();
    }

    /// Due automations are claimed in one mutation: each gets `next_run_at`
    /// advanced past `now` before the caller dispatches, so a crash can't
    /// double-fire the same tick.
    pub fn claim_due(&self, now: DateTime<Utc>) -> Vec<CloudAutomation> {
        let mut state = self.inner.state.write().unwrap();
        let mut due = Vec::new();
        for automation in state.config.automations.iter_mut() {
            if !automation.enabled {
                continue;
            }
            let due_at = automation.next_run_at.unwrap_or(now);
            if due_at > now {
                continue;
            }
            automation.last_run_at = Some(now);
            automation.next_run_at = next_run(&automation.schedule, now);
            due.push(automation.clone());
        }
        if !due.is_empty() {
            drop(state);
            self.persist_config();
        }
        due
    }

    pub fn mark_automation(&self, id: &str, status: &str, chat_id: Option<&str>) {
        let mut state = self.inner.state.write().unwrap();
        if let Some(automation) = state.config.automations.iter_mut().find(|a| a.id == id) {
            automation.last_status = Some(status.to_string());
            if let Some(chat_id) = chat_id {
                automation.last_chat_id = Some(chat_id.to_string());
            }
        }
        drop(state);
        self.persist_config();
    }

    // ── run ledger ───────────────────────────────────────────────────────

    pub fn runs(&self) -> Vec<CloudRun> {
        self.inner.state.read().unwrap().runs.runs.clone()
    }

    pub fn run_for_chat(&self, chat_id: &str) -> Option<CloudRun> {
        self.runs()
            .into_iter()
            .filter(|r| r.chat_id == chat_id)
            .max_by_key(|r| r.updated_at)
    }

    pub fn upsert_run(&self, run: CloudRun) {
        let mut state = self.inner.state.write().unwrap();
        match state.runs.runs.iter_mut().find(|r| r.id == run.id) {
            Some(existing) => *existing = run,
            None => state.runs.runs.push(run),
        }
        drop(state);
        self.persist_runs();
    }

    pub fn update_run(&self, run_id: &str, f: impl FnOnce(&mut CloudRun)) {
        let mut state = self.inner.state.write().unwrap();
        if let Some(run) = state.runs.runs.iter_mut().find(|r| r.id == run_id) {
            f(run);
            run.updated_at = Utc::now();
        }
        drop(state);
        self.persist_runs();
    }

    /// A redacted config view for RPC: secrets collapse to `has_*` booleans.
    pub fn view(&self) -> serde_json::Value {
        let state = self.inner.state.read().unwrap();
        let seats: Vec<serde_json::Value> = state
            .config
            .seats
            .iter()
            .map(|seat| {
                let mut value = serde_json::to_value(seat).unwrap_or_default();
                if let Some(object) = value.as_object_mut() {
                    object.insert(
                        "hasBoxToken".into(),
                        serde_json::json!(self.seat_has_token(&seat.id, "box")),
                    );
                    object.insert(
                        "hasGithubPat".into(),
                        serde_json::json!(self.seat_has_token(&seat.id, "github")),
                    );
                    object.insert(
                        "capabilities".into(),
                        serde_json::to_value(seat.capabilities()).unwrap_or_default(),
                    );
                }
                value
            })
            .collect();
        let providers: Vec<serde_json::Value> = state
            .config
            .providers
            .iter()
            .cloned()
            .map(|mut p| {
                p.has_key = p
                    .provider
                    .map(|provider| state.keys.provider_keys.contains_key(provider.id()))
                    .unwrap_or(false);
                serde_json::to_value(p).unwrap_or_default()
            })
            .collect();
        let has_shared_github = self.github_pat_shared().is_some();
        let load_error = state.load_error.clone();
        drop(state);
        serde_json::json!({
            "seats": seats,
            "providers": providers,
            "automations": self.automations(),
            "defaultSeat": self.default_seat().map(|s| s.id),
            "runs": self.runs(),
            // Whether the shared GitHub PAT exists — the repo picker's
            // "token present" flag (never the token itself).
            "hasGithubPat": has_shared_github,
            "loadError": load_error,
        })
    }

    /// Persist a mutation on one seat's config (probe folds, keep-prompt
    /// stamps, box-id binding) without touching its secrets.
    pub fn update_seat(&self, seat_id: &str, f: impl FnOnce(&mut SeatConfig)) {
        let mut state = self.inner.state.write().unwrap();
        if let Some(seat) = state.config.seats.iter_mut().find(|s| s.id == seat_id) {
            f(seat);
        }
        drop(state);
        let _ = self.persist_config();
    }

    pub fn try_update_seat(
        &self,
        seat_id: &str,
        f: impl FnOnce(&mut SeatConfig),
    ) -> Result<(), String> {
        let mut state = self.inner.state.write().unwrap();
        if let Some(seat) = state.config.seats.iter_mut().find(|s| s.id == seat_id) {
            f(seat);
        }
        drop(state);
        self.persist_config()
    }

    /// `<data>/seat-backups/` — workspace snapshots pulled off dying seats
    /// (the Railway trial safeguard). Local only; never synced.
    pub fn backups_dir(&self) -> PathBuf {
        let dir = self.inner.dir.join("seat-backups");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// The newest Railway workspace snapshot on this device — what a fresh
    /// seat (Boat) restores so the trial's work carries over.
    pub fn latest_railway_backup(&self) -> Option<PathBuf> {
        self.seats()
            .into_iter()
            .filter(|s| s.kind == Some(CloudSeatKind::Railway) && s.backup_path.is_some())
            .max_by_key(|s| s.backup_at)
            .and_then(|s| s.backup_path.map(PathBuf::from))
            .filter(|p| p.exists())
    }
}

fn dedupe_railway_seats(config: &mut CloudConfigFile) -> bool {
    let mut railway_seats: Vec<(usize, SeatConfig)> = config
        .seats
        .iter()
        .enumerate()
        .filter(|(_, seat)| seat.kind == Some(CloudSeatKind::Railway))
        .map(|(index, seat)| (index, seat.clone()))
        .collect();
    if railway_seats.len() <= 1 {
        return false;
    }

    let now = Utc::now();
    railway_seats.sort_by_key(|(index, seat)| {
        let has_backup_metadata = seat.backup_path.is_some()
            || seat.backup_at.is_some()
            || seat.backup_attempted_at.is_some()
            || seat.restored_at.is_some();
        (
            seat.expires_at.is_some() || seat.claim_url.is_some() || has_backup_metadata,
            seat.expires_at.is_some_and(|expires_at| expires_at > now),
            seat.expires_at,
            seat.last_probe.as_ref().map(|probe| probe.probed_at),
            *index,
        )
    });
    let keep_index = railway_seats.last().map(|(index, _)| *index).unwrap();
    let keep_id = config.seats[keep_index].id.clone();
    let removed_default = config.default_seat.as_deref().is_some_and(|default_id| {
        railway_seats
            .iter()
            .any(|(index, seat)| *index != keep_index && seat.id == default_id)
    });
    let dropped: Vec<SeatConfig> = railway_seats
        .into_iter()
        .rev()
        .filter_map(|(index, seat)| (index != keep_index).then_some(seat))
        .collect();
    let kept = &mut config.seats[keep_index];
    if kept
        .harness_command
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        kept.harness_command = dropped.iter().find_map(|seat| {
            seat.harness_command
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        });
    }
    if kept
        .claim_url
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        kept.claim_url = dropped.iter().find_map(|seat| {
            seat.claim_url
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        });
    }
    if kept
        .preview_url
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        kept.preview_url = dropped.iter().find_map(|seat| {
            seat.preview_url
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        });
    }
    if kept
        .backup_path
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        kept.backup_path = dropped.iter().find_map(|seat| {
            seat.backup_path
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        });
    }
    if kept.backup_at.is_none() {
        kept.backup_at = dropped.iter().find_map(|seat| seat.backup_at);
    }
    if kept.backup_attempted_at.is_none() {
        kept.backup_attempted_at = dropped.iter().find_map(|seat| seat.backup_attempted_at);
    }
    if kept.restored_at.is_none() {
        kept.restored_at = dropped.iter().find_map(|seat| seat.restored_at);
    }
    config.seats = config
        .seats
        .drain(..)
        .enumerate()
        .filter_map(|(index, seat)| {
            (seat.kind != Some(CloudSeatKind::Railway) || index == keep_index).then_some(seat)
        })
        .collect();
    if removed_default {
        config.default_seat = Some(keep_id);
    }
    true
}

/// The local Claude Code subscription token (`claudeAiOauth.accessToken`
/// in `$CLAUDE_CONFIG_DIR/.credentials.json` / `~/.claude/.credentials.json`),
/// skipped when expired. macOS keychain-held logins miss here — fine, the
/// env var just isn't injected. Never stored; read fresh per dispatch.
fn claude_oauth_token() -> Option<String> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs_like_home().join(".claude"));
    let text = std::fs::read_to_string(dir.join(".credentials.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let oauth = v.get("claudeAiOauth")?;
    if let Some(expires_ms) = oauth.get("expiresAt").and_then(|e| e.as_i64()) {
        if expires_ms <= Utc::now().timestamp_millis() {
            return None;
        }
    }
    oauth
        .get("accessToken")
        .and_then(|t| t.as_str())
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
}

fn dirs_like_home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// (local file, remote path) pairs a seat's harness needs — the user's
/// existing CLI logins, staged before the run so the seat stays
/// authenticated with the laptop closed. Unknown commands stage nothing;
/// `cloud_env`'s API keys may still cover them.
pub fn harness_credentials(command: &str) -> Vec<(PathBuf, &'static str)> {
    let home = dirs_like_home();
    let env_dir = |name: &str| {
        std::env::var_os(name)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    };
    let xdg = env_dir("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"));
    let bin = command
        .split_whitespace()
        .next()
        .unwrap_or("")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim_end_matches(".exe")
        .to_lowercase();
    let pair = |local: PathBuf, remote: &'static str| local.exists().then_some((local, remote));
    if bin.contains("claude") {
        return pair(
            env_dir("CLAUDE_CONFIG_DIR")
                .unwrap_or_else(|| home.join(".claude"))
                .join(".credentials.json"),
            "~/.claude/.credentials.json",
        )
        .into_iter()
        .collect();
    }
    if bin.contains("codex") {
        return pair(
            env_dir("CODEX_HOME")
                .unwrap_or_else(|| home.join(".codex"))
                .join("auth.json"),
            "~/.codex/auth.json",
        )
        .into_iter()
        .collect();
    }
    if bin.contains("cursor") {
        return pair(
            home.join(".cursor").join("sdk").join("auth.json"),
            "~/.cursor/sdk/auth.json",
        )
        .into_iter()
        .collect();
    }
    if bin.contains("grok") {
        return pair(
            env_dir("GROK_HOME")
                .unwrap_or_else(|| home.join(".grok"))
                .join("auth.json"),
            "~/.grok/auth.json",
        )
        .into_iter()
        .collect();
    }
    if bin.contains("devin") {
        let primary = xdg.join("devin").join("credentials.toml");
        let local = if primary.exists() {
            primary
        } else {
            env_dir("APPDATA")
                .map(|a| a.join("devin").join("credentials.toml"))
                .filter(|f| f.exists())
                .unwrap_or(primary)
        };
        return pair(local, "~/.local/share/devin/credentials.toml")
            .into_iter()
            .collect();
    }
    if bin.contains("opencode") {
        return pair(
            xdg.join("opencode").join("auth.json"),
            "~/.local/share/opencode/auth.json",
        )
        .into_iter()
        .collect();
    }
    if bin == "pi" {
        return pair(
            env_dir("PI_CODING_AGENT_DIR")
                .unwrap_or_else(|| home.join(".pi").join("agent"))
                .join("auth.json"),
            "~/.pi/agent/auth.json",
        )
        .into_iter()
        .collect();
    }
    if bin.contains("hermes") {
        let local = env_dir("HERMES_HOME")
            .map(|h| h.join("auth.json"))
            .or_else(|| {
                env_dir("LOCALAPPDATA")
                    .map(|l| l.join("hermes").join("auth.json"))
                    .filter(|f| f.exists())
            })
            .unwrap_or_else(|| home.join(".hermes").join("auth.json"));
        return pair(local, "~/.hermes/auth.json").into_iter().collect();
    }
    Vec::new()
}

/// Push one credential file onto the seat — base64 over exec (the exec
/// surface has no stdin). Written 0600, never logged; failures are the
/// caller's warn so a staging miss never blocks the run itself.
pub async fn stage_credential(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
    local: &Path,
    remote: &str,
) -> Result<(), String> {
    const MAX: usize = 1024 * 1024;
    let bytes = std::fs::read(local).map_err(|e| format!("read {}: {e}", local.display()))?;
    if bytes.len() > MAX {
        return Err(format!("{} too large to stage", local.display()));
    }
    use base64::Engine as _;
    let dir = remote.rsplit_once('/').map(|(d, _)| d).unwrap_or("~");
    let out = match seat.kind {
        Some(CloudSeatKind::Railway) | Some(CloudSeatKind::Ssh) => {
            let alias = seat
                .ssh_alias
                .clone()
                .unwrap_or_else(|| crate::railway::RAILWAY_HOST.into());
            let cmd = format!(
                "mkdir -p {} && base64 -d > {} && chmod 600 {}",
                shell_word(dir),
                shell_word(remote),
                shell_word(remote)
            );
            let remote_cmd = if seat.kind == Some(CloudSeatKind::Ssh) {
                format!(
                    "docker exec {} sh -lc {}",
                    shell_word(&seat.container_name()),
                    shell_word(&cmd)
                )
            } else {
                cmd
            };
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            ssh_exec_stdin(&alias, &remote_cmd, b64.as_bytes(), Duration::from_secs(30)).await?
        }
        Some(CloudSeatKind::Box) => {
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let temp = format!("/tmp/cosmos-credential-{}.b64", crate::new_id());
            let cmd = format!(
                "trap 'rm -f {}' EXIT && printf '%s' {} > {} && base64 -d {} > {} && chmod 600 {}",
                shell_word(&temp),
                shell_word(&b64),
                shell_word(&temp),
                shell_word(&temp),
                shell_word(remote),
                shell_word(remote),
            );
            seat_exec(http, store, seat, &cmd, Duration::from_secs(30)).await?
        }
        _ => return Err("seat cannot stage credentials".into()),
    };
    if out.code == 0 {
        Ok(())
    } else {
        Err(out
            .stderr
            .lines()
            .next()
            .unwrap_or("credential write failed")
            .to_string())
    }
}

pub async fn cleanup_staged_credentials(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
    remotes: &[&str],
) {
    if !matches!(
        seat.kind,
        Some(CloudSeatKind::Railway) | Some(CloudSeatKind::Box)
    ) {
        return;
    }
    for remote in remotes {
        let _ = seat_exec(
            http,
            store,
            seat,
            &format!("rm -f {}", shell_word(remote)),
            Duration::from_secs(20),
        )
        .await;
    }
}

/// Pull `seat`'s workspace to `seat-backups/<id>.tar.gz` — the Railway
/// trial safeguard the scheduler fires ~10 minutes before `expires_at`.
/// Heavy build dirs are excluded; a cap guards memory (`output()` buffers).
pub async fn snapshot_workspace(store: &CloudStore, seat: &SeatConfig) -> Result<PathBuf, String> {
    let path = store.backups_dir().join(format!("{}.tar.gz", seat.id));
    let tar = command_in_workspace(
        &seat.workspace(),
        "tar czf - --exclude=node_modules --exclude=target --exclude=dist \
         --exclude=.next --exclude=.cache --exclude='*.log' .",
    );
    let output = match seat.kind {
        Some(CloudSeatKind::Railway) => {
            let alias = seat
                .ssh_alias
                .clone()
                .unwrap_or_else(|| crate::railway::RAILWAY_HOST.into());
            tokio::time::timeout(
                Duration::from_secs(240),
                tokio::process::Command::new("ssh")
                    .args([
                        "-o",
                        "BatchMode=yes",
                        "-o",
                        "ConnectTimeout=15",
                        "-o",
                        "StrictHostKeyChecking=accept-new",
                        alias.as_str(),
                        tar.as_str(),
                    ])
                    .output(),
            )
            .await
            .map_err(|_| "workspace snapshot timed out".to_string())?
            .map_err(|e| format!("could not run ssh: {e}"))?
        }
        _ => return Err("snapshots only support Railway seats".into()),
    };
    if !output.status.success() {
        return Err(format!(
            "snapshot failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next()
                .unwrap_or("ssh error")
        ));
    }
    const MAX_SNAPSHOT: usize = 512 * 1024 * 1024;
    if output.stdout.is_empty() {
        return Err("snapshot was empty".into());
    }
    if output.stdout.len() > MAX_SNAPSHOT {
        return Err("workspace too large to snapshot (>512 MB)".into());
    }
    std::fs::write(&path, &output.stdout).map_err(|e| format!("write snapshot: {e}"))?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Provider verification (tiny models/list or 1-token ping — save only on
// success is the caller's rule; this fn just answers).
// ---------------------------------------------------------------------------

/// Verify a BYOK row: GET the provider's light endpoint with the credential.
/// The returned string is a human status line (model count / plan label).
pub async fn verify_provider(
    http: &reqwest::Client,
    provider: ByokProvider,
    key: &str,
    base_url: Option<&str>,
) -> Result<String, String> {
    if key.trim().is_empty() {
        return Err("Key is empty".into());
    }
    let base = base_url
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| provider.default_base_url())
        .trim_end_matches('/');
    if base.is_empty() {
        return Err("Base URL is required for a custom provider".into());
    }
    let request = match provider {
        ByokProvider::Anthropic => http
            .get(format!("{base}/v1/models?limit=1"))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        ByokProvider::ChatgptSub => http
            .get(format!("{base}/models"))
            .bearer_auth(key)
            .header("chatgpt-account-id", ""),
        ByokProvider::DevinSub => http.get(format!("{base}/v1/whoami")).bearer_auth(key),
        _ => http.get(format!("{base}/models")).bearer_auth(key),
    };
    let response = request
        .timeout(PROBE_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(format!("authentication failed ({status})"));
    }
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.get("message").cloned())
                    .or_else(|| v.get("message").cloned())
            })
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| body.chars().take(120).collect());
        return Err(format!("HTTP {status}: {message}"));
    }
    let count = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("data").or_else(|| v.get("models")).cloned())
        .and_then(|v| v.as_array().map(|a| a.len()));
    Ok(match count {
        Some(n) => format!("Key works — {n} models visible"),
        None => "Key works".to_string(),
    })
}

// ---------------------------------------------------------------------------
// Seat probes
// ---------------------------------------------------------------------------

/// Probe a seat: never throws — the answer IS the probe result.
pub async fn probe_seat(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
) -> SeatProbe {
    let mut probe = SeatProbe {
        seat_id: seat.id.clone(),
        online: false,
        detail: None,
        containers: Vec::new(),
        image_digest: None,
        controllable: false,
        keeps_running: false,
        sleeping: false,
        probed_at: Utc::now(),
    };
    let capabilities = seat.capabilities();
    probe.keeps_running = capabilities.keeps_running;
    match seat.kind {
        Some(CloudSeatKind::Railway) => {
            if seat.expired() {
                probe.detail = Some("trial hour ended — claim the box or start a new trial".into());
                return probe;
            }
            match crate::railway::probe_with_existing_deadline(seat.expires_at.is_some()).await {
                Ok(trial) => {
                    probe.online = true;
                    probe.detail = trial.detail.or_else(|| Some("railway.new trial VM".into()));
                    // A fresh manifest can extend or clarify the deadline and
                    // claim link — fold them into the stored seat.
                    if trial.expires_at.is_some() || trial.claim_url.is_some() {
                        let mut seat = seat.clone();
                        seat.expires_at = trial.expires_at.or(seat.expires_at);
                        seat.claim_url = trial.claim_url.or(seat.claim_url);
                        seat.preview_url = trial.preview_url.or(seat.preview_url);
                        let _ = store.save_seat(seat);
                    }
                }
                Err(failure) => {
                    if failure.trial.claim_required {
                        let now = Utc::now();
                        let mut updated = seat.clone();
                        updated.claim_url = failure.trial.claim_url.or(updated.claim_url);
                        updated.expires_at = Some(
                            updated
                                .expires_at
                                .map_or(now, |expires_at| expires_at.min(now)),
                        );
                        let _ = store.save_seat(updated);
                    }
                    probe.detail = Some(failure.detail);
                }
            }
        }
        Some(CloudSeatKind::Ssh) => {
            let Some(alias) = seat.ssh_alias.clone() else {
                probe.detail = Some("no SSH alias configured".into());
                return probe;
            };
            match ssh_exec(
                &alias,
                "echo __cosmos_ok && docker version --format '{{.Server.Version}}' && docker ps --filter \"name=cosmos-cloud-\" --format '{{.Names}}'",
                PROBE_TIMEOUT,
            )
            .await
            {
                Ok(out) if out.code == 0 && out.stdout.contains("__cosmos_ok") => {
                    probe.online = true;
                    let mut lines = out.stdout.lines();
                    let _ = lines.next(); // the marker
                    if let Some(version) = lines.next() {
                        probe.detail = Some(format!("docker {version} @ {alias}"));
                    }
                    probe.containers = lines
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                Ok(out) => {
                    probe.detail = Some(
                        out.stderr
                            .lines()
                            .next()
                            .unwrap_or("host answered without the marker")
                            .to_string(),
                    );
                }
                Err(e) => probe.detail = Some(e),
            }
        }
        Some(CloudSeatKind::GitHub) => {
            let (Some(repo), Some(pat)) = (seat.repo.clone(), store.github_pat(&seat.id)) else {
                probe.detail = Some("needs a repo (owner/name) and a PAT".into());
                return probe;
            };
            match github(http, &pat).repo(&repo).await {
                Ok(info) => {
                    probe.online = true;
                    probe.detail = Some(format!(
                        "{} · default {}",
                        info.full_name, info.default_branch
                    ));
                    probe.containers = Vec::new();
                }
                Err(e) => probe.detail = Some(e),
            }
        }
        Some(CloudSeatKind::Box) => {
            let Some(token) = store.box_token(&seat.id) else {
                probe.detail = Some("no Boat token configured".into());
                return probe;
            };
            let client = crate::boat::boat(http, &token, seat.box_endpoint.as_deref());
            match seat.box_id.clone() {
                // Bound box: its state machine IS the health signal —
                // ready/idle/running = online, archived = sleeping.
                Some(box_id) => match client.sandbox(&box_id).await {
                    Ok(sandbox) => {
                        probe.sleeping = sandbox.sleeping();
                        probe.online = sandbox.usable();
                        probe.detail = Some(match sandbox.state.as_str() {
                            "archived" | "archiving" => "sleeping — billing off".into(),
                            "provisioning" | "init" | "cloning" => "provisioning…".into(),
                            "error" | "cancelled" => {
                                sandbox.error.clone().unwrap_or_else(|| "box failed".into())
                            }
                            state => format!("{state} — {}", sandbox.name),
                        });
                        // Stale fields the provider mints: ssh endpoint and
                        // preview URL refresh the seat config on probe.
                        if sandbox.ssh_endpoint.is_some()
                            && sandbox.ssh_endpoint != seat.ssh_endpoint
                        {
                            let mut seat = seat.clone();
                            seat.ssh_endpoint = sandbox.ssh_endpoint;
                            store.save_seat(seat);
                        }
                    }
                    Err(e) => probe.detail = Some(e),
                },
                // No box yet — "create-or-list": a bare list answers the
                // token check; only when nothing exists do we mint a probe
                // box (ttl 5m) and destroy it, so a ping never bills.
                None => match client.sandboxes().await {
                    Ok(list) => {
                        if let Some(live) = list
                            .iter()
                            .find(|s| s.usable() || s.sleeping())
                            .or_else(|| list.first())
                        {
                            probe.detail = Some(format!(
                                "token ok — {} sandbox{}; first run reuses `{}`",
                                list.len(),
                                if list.len() == 1 { "" } else { "es" },
                                live.id
                            ));
                            // Bind the newest box so the first run reuses it
                            // instead of minting a second billable sandbox.
                            let mut seat = seat.clone();
                            seat.box_id = Some(live.id.clone());
                            seat.ssh_endpoint = live.ssh_endpoint.clone();
                            store.save_seat(seat);
                            probe.online = live.usable();
                            probe.sleeping = live.sleeping();
                        } else {
                            // Spec's ping: create a probe sandbox, confirm
                            // the token can mint one, then destroy it — a
                            // bare ping never leaves a billable box behind.
                            let ping = client
                                .create("cosmos-probe", &format!("probe-{}", crate::new_id()))
                                .await;
                            match ping {
                                Ok(sandbox) => {
                                    probe.online = true;
                                    probe.detail = Some(format!(
                                        "token ok — created + destroyed probe {}",
                                        sandbox.id
                                    ));
                                    let _ = client.destroy(&sandbox.id).await;
                                }
                                Err(e) => probe.detail = Some(e),
                            }
                        }
                    }
                    Err(e) => probe.detail = Some(e),
                },
            }
        }
        None => probe.detail = Some("no backend selected".into()),
    }
    // A stamped CUA endpoint probed live upgrades `controllable`.
    if let Some(endpoint) = seat.cua_endpoint.clone() {
        let url = format!("{}/computer/ping", endpoint.trim_end_matches('/'));
        let mut req = http.post(&url).json(&serde_json::json!({}));
        if let Some(token) = store.cua_token(&seat.id) {
            req = req.bearer_auth(token);
        }
        if let Ok(r) = req.timeout(PROBE_TIMEOUT).send().await {
            probe.controllable = r.status().is_success();
        }
    }
    probe
}

// ---------------------------------------------------------------------------
// SSH / docker exec
// ---------------------------------------------------------------------------

pub struct ExecOutcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run `command` on the SSH host (via the user's ssh-agent / config alias —
/// we never touch a private key). `BatchMode` refuses interactive prompts so
/// a misconfigured alias fails fast instead of hanging.
pub async fn ssh_exec(
    alias: &str,
    command: &str,
    timeout: Duration,
) -> Result<ExecOutcome, String> {
    let output = tokio::time::timeout(
        timeout,
        tokio::process::Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=8",
                "-o",
                "StrictHostKeyChecking=accept-new",
                alias,
                command,
            ])
            .output(),
    )
    .await
    .map_err(|_| format!("ssh {alias} timed out"))?
    .map_err(|e| format!("could not run ssh: {e}"))?;
    Ok(ExecOutcome {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

pub async fn ssh_exec_stdin(
    alias: &str,
    command: &str,
    input: &[u8],
    timeout: Duration,
) -> Result<ExecOutcome, String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "StrictHostKeyChecking=accept-new",
            alias,
            command,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run ssh: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input)
            .await
            .map_err(|e| format!("could not write ssh stdin: {e}"))?;
    }
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| format!("ssh {alias} timed out"))?
        .map_err(|e| format!("could not run ssh: {e}"))?;
    Ok(ExecOutcome {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// `docker exec <container> sh -lc <command>` on the seat — or, for
/// Railway/Boat, the backend's own exec surface (plain `ssh railway.new`,
/// or Boat's `/commands` endpoint).
pub async fn seat_exec(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
    command: &str,
    timeout: Duration,
) -> Result<ExecOutcome, String> {
    match seat.kind {
        Some(CloudSeatKind::Railway) => {
            let alias = seat
                .ssh_alias
                .clone()
                .unwrap_or_else(|| crate::railway::RAILWAY_HOST.into());
            if seat.expired() {
                return Err("Railway trial seat expired — claim it or start a new trial".into());
            }
            ssh_exec(&alias, command, timeout).await
        }
        Some(CloudSeatKind::Ssh) => {
            let alias = seat
                .ssh_alias
                .clone()
                .ok_or_else(|| "seat has no SSH alias".to_string())?;
            let remote = format!(
                "docker exec {} sh -lc {}",
                shell_word(&seat.container_name()),
                shell_word(command)
            );
            ssh_exec(&alias, &remote, timeout).await
        }
        Some(CloudSeatKind::Box) => {
            let (Some(token), Some(box_id)) = (store.box_token(&seat.id), seat.box_id.clone())
            else {
                return Err("Boat seat has no box yet — it provisions on first run".into());
            };
            let out = crate::boat::boat(http, &token, seat.box_endpoint.as_deref())
                .exec(&box_id, command, None, timeout)
                .await?;
            Ok(ExecOutcome {
                code: out.exit_code.unwrap_or(if out.success { 0 } else { -1 }),
                stdout: out.stdout,
                stderr: out.stderr,
            })
        }
        Some(CloudSeatKind::GitHub) => {
            Err("GitHub seats have no shell — runs go through workflow dispatch".into())
        }
        None => Err("seat has no backend".into()),
    }
}

/// A remote command plus its stdout stream — the detached-run primitive.
/// `kill` stops the child (and so the ssh link); the remote command survives
/// only when it was launched detached on the seat.
pub struct ExecStream {
    pub child: tokio::process::Child,
    pub stdout: tokio::process::ChildStdout,
    pub stderr: tokio::process::ChildStderr,
}

/// Spawn a streaming command on the seat. SSH seats wrap in `docker exec`;
/// Railway execs directly on the trial VM (no container indirection — the
/// ssh session IS the box).
pub async fn seat_exec_stream(seat: &SeatConfig, command: &str) -> Result<ExecStream, String> {
    let (alias, remote) = match seat.kind {
        Some(CloudSeatKind::Ssh) => (
            seat.ssh_alias
                .clone()
                .ok_or_else(|| "seat has no SSH alias".to_string())?,
            format!(
                "docker exec {} sh -lc {}",
                shell_word(&seat.container_name()),
                shell_word(command)
            ),
        ),
        Some(CloudSeatKind::Railway) => {
            if seat.expired() {
                return Err("Railway trial seat expired".into());
            }
            (
                seat.ssh_alias
                    .clone()
                    .unwrap_or_else(|| crate::railway::RAILWAY_HOST.into()),
                command.to_string(),
            )
        }
        _ => return Err("streaming exec needs an SSH or Railway seat".to_string()),
    };
    let mut child = tokio::process::Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-tt",
            &alias,
            &remote,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .stdin(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("could not spawn ssh: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ssh stdout missing".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "ssh stderr missing".to_string())?;
    Ok(ExecStream {
        child,
        stdout,
        stderr,
    })
}

/// Minimal POSIX single-quote escaping for one shell word.
pub fn shell_word(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ---------------------------------------------------------------------------
// Workspace data over the seat
// ---------------------------------------------------------------------------

/// One row of a remote directory listing (matches `WorkspaceEntry`'s shape so
/// the UI renders seats and local folders identically).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

/// Parse `ls -la --time-style=+%s` output into remote entries (shared by the
/// SSH/docker exec and Boat `/commands` listings).
fn parse_ls(stdout: &str, base: &str) -> Vec<RemoteEntry> {
    let mut entries = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let Some(mode) = parts.next() else { continue };
        let mut skipped = 0usize;
        // mode links owner group size time name
        let name = loop {
            skipped += 1;
            if skipped > 6 {
                break None;
            }
            let _ = parts.next();
            if skipped == 4 {
                // size consumed; next token is time, then name
                let _ = parts.next();
                break parts.next();
            }
        };
        let Some(name) = name else { continue };
        if name == "." || name == ".." {
            continue;
        }
        entries.push(RemoteEntry {
            path: format!("{base}/{name}"),
            name: name.to_string(),
            is_dir: mode.starts_with('d'),
            size: None,
        });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    entries
}

/// `ls`-style listing of `dir` on the seat. GitHub seats list via the
/// contents API; SSH/Railway/Boat seats exec `ls -la`.
pub async fn list_dir(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
    dir: &str,
) -> Result<Vec<RemoteEntry>, String> {
    match seat.kind {
        Some(CloudSeatKind::Ssh) | Some(CloudSeatKind::Railway) | Some(CloudSeatKind::Box) => {
            let base = join_remote(&seat.workspace(), dir);
            let out = seat_exec(
                http,
                store,
                seat,
                &command_in_workspace(&base, "ls -la --time-style=+%s 2>/dev/null | tail -n +2"),
                PROBE_TIMEOUT,
            )
            .await?;
            if out.code != 0 {
                return Err(out.stderr.lines().next().unwrap_or("ls failed").to_string());
            }
            Ok(parse_ls(&out.stdout, &base))
        }
        Some(CloudSeatKind::GitHub) => {
            let repo = seat.repo.clone().ok_or("seat has no repo")?;
            let pat = store
                .github_pat(&seat.id)
                .ok_or("seat has no GitHub token")?;
            let reference = seat.branch.as_deref().unwrap_or("HEAD");
            github(http, &pat).contents(&repo, dir, reference).await
        }
        None => Err("seat has no backend".into()),
    }
}

/// Read a remote file (bounded at 256 KiB; truncation is reported honest).
pub async fn read_file(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
    path: &str,
) -> Result<(String, bool), String> {
    const CAP: usize = 256 * 1024;
    match seat.kind {
        Some(CloudSeatKind::Ssh) | Some(CloudSeatKind::Railway) | Some(CloudSeatKind::Box) => {
            let out = seat_exec(
                http,
                store,
                seat,
                &format!("head -c {CAP} -- {}", shell_word(path)),
                PROBE_TIMEOUT,
            )
            .await?;
            if out.code != 0 {
                return Err(out
                    .stderr
                    .lines()
                    .next()
                    .unwrap_or("read failed")
                    .to_string());
            }
            let truncated = out.stdout.len() >= CAP;
            Ok((out.stdout, truncated))
        }
        Some(CloudSeatKind::GitHub) => {
            let repo = seat.repo.clone().ok_or("seat has no repo")?;
            let pat = store
                .github_pat(&seat.id)
                .ok_or("seat has no GitHub token")?;
            let reference = seat.branch.as_deref().unwrap_or("HEAD");
            let (content, size) = github(http, &pat).file(&repo, path, reference).await?;
            let truncated = content.len() > CAP;
            Ok((
                content.chars().take(CAP).collect(),
                truncated || size > CAP as u64,
            ))
        }
        None => Err("seat has no backend".into()),
    }
}

/// `git status --porcelain` + a bounded `git diff` on the seat, or a GitHub
/// compare against the default branch.
pub async fn git_status(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
) -> Result<serde_json::Value, String> {
    match seat.kind {
        Some(CloudSeatKind::Ssh) | Some(CloudSeatKind::Railway) | Some(CloudSeatKind::Box) => {
            let ws = seat.workspace();
            let status = seat_exec(
                http,
                store,
                seat,
                &command_in_workspace(
                    &ws,
                    "git status --porcelain=v1 -b && git log -1 --format=%H 2>/dev/null",
                ),
                PROBE_TIMEOUT,
            )
            .await?;
            if status.code != 0 {
                return Err(status
                    .stderr
                    .lines()
                    .next()
                    .unwrap_or("not a git work tree")
                    .to_string());
            }
            let diff = seat_exec(
                http,
                store,
                seat,
                &command_in_workspace(&ws, "git diff --stat HEAD 2>/dev/null | tail -n 60"),
                PROBE_TIMEOUT,
            )
            .await?;
            Ok(serde_json::json!({
                "branch": status.stdout.lines().next().unwrap_or("").trim_start_matches("## ").trim(),
                "entries": status.stdout.lines().skip(1).filter_map(|l| {
                    let l = l.trim_end();
                    (l.len() > 3).then(|| serde_json::json!({"status": &l[..2], "path": &l[3..]}))
                }).collect::<Vec<_>>(),
                "stat": diff.stdout,
            }))
        }
        Some(CloudSeatKind::GitHub) => {
            let repo = seat.repo.clone().ok_or("seat has no repo")?;
            let pat = store
                .github_pat(&seat.id)
                .ok_or("seat has no GitHub token")?;
            github(http, &pat).repo(&repo).await.map(|info| {
                serde_json::json!({
                    "branch": info.default_branch,
                    "entries": [],
                    "stat": "",
                    "remote": info.full_name,
                })
            })
        }
        _ => Err("seat has no git surface".into()),
    }
}

/// The full unified diff for the seat workspace (bounded).
pub async fn git_diff(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
    path: Option<&str>,
) -> Result<String, String> {
    match seat.kind {
        Some(CloudSeatKind::Ssh) | Some(CloudSeatKind::Railway) | Some(CloudSeatKind::Box) => {
            let ws = seat.workspace();
            let path_arg = path
                .map(|p| format!("-- {}", shell_word(p)))
                .unwrap_or_default();
            let out = seat_exec(
                http,
                store,
                seat,
                &command_in_workspace(&ws, &format!("git diff HEAD -- . {path_arg}")),
                PROBE_TIMEOUT,
            )
            .await?;
            if out.code != 0 {
                return Err(out
                    .stderr
                    .lines()
                    .next()
                    .unwrap_or("diff failed")
                    .to_string());
            }
            Ok(out.stdout)
        }
        Some(CloudSeatKind::GitHub) => {
            let repo = seat.repo.clone().ok_or("seat has no repo")?;
            let branch = seat.branch.clone().unwrap_or_default();
            if branch.is_empty() {
                return Err("GitHub seats diff via a bound branch".into());
            }
            let pat = store
                .github_pat(&seat.id)
                .ok_or("seat has no GitHub token")?;
            github(http, &pat).compare(&repo, &branch).await
        }
        _ => Err("seat has no git surface".into()),
    }
}

/// Join a seat workspace root with a (possibly relative) directory.
fn join_remote(base: &str, dir: &str) -> String {
    if dir.is_empty() || dir == "." {
        return base.to_string();
    }
    if dir.starts_with('/') {
        return dir.to_string();
    }
    format!("{}/{}", base.trim_end_matches('/'), dir)
}

// ---------------------------------------------------------------------------
// GitHub REST — PAT auth, no SDK (binary budget).
// ---------------------------------------------------------------------------

pub struct GitHub<'a> {
    http: &'a reqwest::Client,
    pat: &'a str,
}

pub fn github<'a>(http: &'a reqwest::Client, pat: &'a str) -> GitHub<'a> {
    GitHub { http, pat }
}

pub struct RepoInfo {
    pub full_name: String,
    pub default_branch: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RepoPage {
    pub items: Vec<RepoHit>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OrgPage {
    pub items: Vec<String>,
    pub truncated: bool,
}

/// One PR row for the list pane.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrRow {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub head: String,
    pub base: String,
    pub draft: bool,
    pub state: String,
    pub url: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checks: Option<String>,
}

/// PR detail for the screenshot-shaped view.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrDetail {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub head: String,
    pub base: String,
    pub head_sha: String,
    pub draft: bool,
    pub state: String,
    pub mergeable: Option<bool>,
    pub mergeable_state: Option<String>,
    pub url: String,
    pub body: String,
    pub additions: u64,
    pub deletions: u64,
    pub changed_files: u64,
    pub created_at: String,
    pub comments: u64,
    pub review_comments: u64,
    pub commits: u64,
    pub files: Vec<PrFile>,
    pub checks: Vec<PrCheck>,
    /// Issue comments + review summaries — the Discussion tab.
    #[serde(default)]
    pub discussion: Vec<PrComment>,
    /// Head commits — the Commits tab.
    #[serde(default)]
    pub commit_list: Vec<PrCommit>,
}

/// One discussion row: an issue comment or a review verdict.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrComment {
    pub author: String,
    pub body: String,
    pub created_at: String,
    /// `"comment"` or `"review"`.
    pub kind: String,
    /// Review state (`APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`) for
    /// `kind == "review"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// One head commit on the PR.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrCommit {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub date: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrFile {
    pub path: String,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrCheck {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

impl<'a> GitHub<'a> {
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("https://api.github.com{path}"))
            .bearer_auth(self.pat)
            .header("accept", "application/vnd.github+json")
            .header("user-agent", "cosmos-desktop")
            .header("x-github-api-version", "2026-03-10")
    }

    async fn error(resp: reqwest::Response) -> String {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| body.chars().take(160).collect());
        format!("github {status}: {message}")
    }

    /// `/repos/{owner}/{name}` — the probe + repo header.
    pub async fn repo(&self, repo: &str) -> Result<RepoInfo, String> {
        let resp = self
            .get(&format!("/repos/{repo}"))
            .send()
            .await
            .map_err(|e| format!("github: {e}"))?;
        if !resp.status().is_success() {
            return Err(Self::error(resp).await);
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(RepoInfo {
            full_name: v["full_name"].as_str().unwrap_or(repo).to_string(),
            default_branch: v["default_branch"].as_str().unwrap_or("main").to_string(),
        })
    }

    /// `/repos/{r}/pulls?state=open` — the list pane.
    pub async fn pulls(&self, repo: &str) -> Result<Vec<PrRow>, String> {
        let resp = self
            .get(&format!("/repos/{repo}/pulls?state=open&per_page=50"))
            .send()
            .await
            .map_err(|e| format!("github: {e}"))?;
        if !resp.status().is_success() {
            return Err(Self::error(resp).await);
        }
        let rows: Vec<serde_json::Value> = resp.json().await.map_err(|e| e.to_string())?;
        Ok(rows
            .iter()
            .map(|v| PrRow {
                number: v["number"].as_u64().unwrap_or(0),
                title: v["title"].as_str().unwrap_or("").to_string(),
                author: v["user"]["login"].as_str().unwrap_or("").to_string(),
                head: v["head"]["ref"].as_str().unwrap_or("").to_string(),
                base: v["base"]["ref"].as_str().unwrap_or("").to_string(),
                draft: v["draft"].as_bool().unwrap_or(false),
                state: v["state"].as_str().unwrap_or("open").to_string(),
                url: v["html_url"].as_str().unwrap_or("").to_string(),
                updated_at: v["updated_at"].as_str().unwrap_or("").to_string(),
                checks: None,
            })
            .collect())
    }

    /// `/pulls/{n}` + `/pulls/{n}/files` + `/commits/{sha}/check-runs` —
    /// the screenshot-shaped detail.
    pub async fn pull(&self, repo: &str, number: u64) -> Result<PrDetail, String> {
        let resp = self
            .get(&format!("/repos/{repo}/pulls/{number}"))
            .send()
            .await
            .map_err(|e| format!("github: {e}"))?;
        if !resp.status().is_success() {
            return Err(Self::error(resp).await);
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let head_sha = v["head"]["sha"].as_str().unwrap_or("").to_string();

        let files_resp = self
            .get(&format!("/repos/{repo}/pulls/{number}/files?per_page=100"))
            .send()
            .await
            .map_err(|e| format!("github: {e}"))?;
        let files = if files_resp.status().is_success() {
            let rows: Vec<serde_json::Value> =
                files_resp.json().await.map_err(|e| e.to_string())?;
            rows.iter()
                .map(|f| PrFile {
                    path: f["filename"].as_str().unwrap_or("").to_string(),
                    status: f["status"].as_str().unwrap_or("").to_string(),
                    additions: f["additions"].as_u64().unwrap_or(0),
                    deletions: f["deletions"].as_u64().unwrap_or(0),
                    patch: f["patch"].as_str().map(str::to_string),
                })
                .collect()
        } else {
            Vec::new()
        };

        let checks = if head_sha.is_empty() {
            Vec::new()
        } else {
            match self
                .get(&format!(
                    "/repos/{repo}/commits/{head_sha}/check-runs?per_page=50"
                ))
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => {
                    let v: serde_json::Value = r.json().await.map_err(|e| e.to_string())?;
                    v["check_runs"]
                        .as_array()
                        .map(|runs| {
                            runs.iter()
                                .map(|c| PrCheck {
                                    name: c["name"].as_str().unwrap_or("").to_string(),
                                    status: c["status"].as_str().unwrap_or("").to_string(),
                                    conclusion: c["conclusion"].as_str().map(str::to_string),
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                }
                _ => Vec::new(),
            }
        };

        // Issue comments + review verdicts — merged and time-sorted for the
        // Discussion tab.
        let mut discussion: Vec<PrComment> = Vec::new();
        if let Ok(r) = self
            .get(&format!(
                "/repos/{repo}/issues/{number}/comments?per_page=100"
            ))
            .send()
            .await
            && r.status().is_success()
            && let Ok(rows) = r.json::<Vec<serde_json::Value>>().await
        {
            for c in rows {
                discussion.push(PrComment {
                    author: c["user"]["login"].as_str().unwrap_or("").to_string(),
                    body: c["body"].as_str().unwrap_or("").to_string(),
                    created_at: c["created_at"].as_str().unwrap_or("").to_string(),
                    kind: "comment".into(),
                    state: None,
                });
            }
        }
        if let Ok(r) = self
            .get(&format!(
                "/repos/{repo}/pulls/{number}/reviews?per_page=100"
            ))
            .send()
            .await
            && r.status().is_success()
            && let Ok(rows) = r.json::<Vec<serde_json::Value>>().await
        {
            for c in rows {
                // Empty review shells carry no content — skip them.
                let state = c["state"].as_str().unwrap_or("");
                let body = c["body"].as_str().unwrap_or("").to_string();
                if state.is_empty() || (body.is_empty() && state == "COMMENTED") {
                    continue;
                }
                discussion.push(PrComment {
                    author: c["user"]["login"].as_str().unwrap_or("").to_string(),
                    body,
                    created_at: c["submitted_at"]
                        .as_str()
                        .or_else(|| c["created_at"].as_str())
                        .unwrap_or("")
                        .to_string(),
                    kind: "review".into(),
                    state: Some(state.to_string()),
                });
            }
        }
        discussion.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        let commit_list: Vec<PrCommit> = match self
            .get(&format!(
                "/repos/{repo}/pulls/{number}/commits?per_page=100"
            ))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r
                .json::<Vec<serde_json::Value>>()
                .await
                .unwrap_or_default()
                .iter()
                .map(|c| PrCommit {
                    sha: c["sha"].as_str().unwrap_or("").to_string(),
                    message: c["commit"]["message"]
                        .as_str()
                        .unwrap_or("")
                        .lines()
                        .next()
                        .unwrap_or("")
                        .to_string(),
                    author: c["commit"]["author"]["name"]
                        .as_str()
                        .or_else(|| c["author"]["login"].as_str())
                        .unwrap_or("")
                        .to_string(),
                    date: c["commit"]["author"]["date"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                })
                .collect(),
            _ => Vec::new(),
        };

        Ok(PrDetail {
            number: v["number"].as_u64().unwrap_or(number),
            title: v["title"].as_str().unwrap_or("").to_string(),
            author: v["user"]["login"].as_str().unwrap_or("").to_string(),
            head: v["head"]["ref"].as_str().unwrap_or("").to_string(),
            base: v["base"]["ref"].as_str().unwrap_or("").to_string(),
            head_sha,
            draft: v["draft"].as_bool().unwrap_or(false),
            state: v["state"].as_str().unwrap_or("").to_string(),
            mergeable: v["mergeable"].as_bool(),
            mergeable_state: v["mergeable_state"].as_str().map(str::to_string),
            url: v["html_url"].as_str().unwrap_or("").to_string(),
            body: v["body"].as_str().unwrap_or("").to_string(),
            additions: v["additions"].as_u64().unwrap_or(0),
            deletions: v["deletions"].as_u64().unwrap_or(0),
            changed_files: v["changed_files"].as_u64().unwrap_or(0),
            created_at: v["created_at"].as_str().unwrap_or("").to_string(),
            comments: v["comments"].as_u64().unwrap_or(0)
                + v["review_comments"].as_u64().unwrap_or(0),
            review_comments: v["review_comments"].as_u64().unwrap_or(0),
            commits: v["commits"].as_u64().unwrap_or(0),
            files,
            checks,
            discussion,
            commit_list,
        })
    }

    /// `PUT /pulls/{n}/merge` — the Merge button.
    pub async fn merge(&self, repo: &str, number: u64) -> Result<(), String> {
        let resp = self
            .http
            .put(format!(
                "https://api.github.com/repos/{repo}/pulls/{number}/merge"
            ))
            .bearer_auth(self.pat)
            .header("accept", "application/vnd.github+json")
            .header("user-agent", "cosmos-desktop")
            .json(&serde_json::json!({"merge_method": "squash"}))
            .send()
            .await
            .map_err(|e| format!("github: {e}"))?;
        if !resp.status().is_success() {
            return Err(Self::error(resp).await);
        }
        Ok(())
    }

    /// `/contents/{dir}` → entries (used by the GitHub-seat Files pane).
    pub async fn contents(
        &self,
        repo: &str,
        dir: &str,
        reference: &str,
    ) -> Result<Vec<RemoteEntry>, String> {
        let path = if dir.is_empty() || dir == "." {
            format!("/repos/{repo}/contents?ref={reference}")
        } else {
            format!("/repos/{repo}/contents/{dir}?ref={reference}")
        };
        let resp = self
            .get(&path)
            .send()
            .await
            .map_err(|e| format!("github: {e}"))?;
        if !resp.status().is_success() {
            return Err(Self::error(resp).await);
        }
        let rows: Vec<serde_json::Value> = resp.json().await.map_err(|e| e.to_string())?;
        let mut entries: Vec<RemoteEntry> = rows
            .iter()
            .map(|v| RemoteEntry {
                path: v["path"].as_str().unwrap_or("").to_string(),
                name: v["name"].as_str().unwrap_or("").to_string(),
                is_dir: v["type"].as_str() == Some("dir"),
                size: v["size"].as_u64(),
            })
            .collect();
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
        Ok(entries)
    }

    /// `/contents/{path}` → decoded file bytes (the API returns base64).
    pub async fn file(
        &self,
        repo: &str,
        path: &str,
        reference: &str,
    ) -> Result<(String, u64), String> {
        let resp = self
            .get(&format!("/repos/{repo}/contents/{path}?ref={reference}"))
            .send()
            .await
            .map_err(|e| format!("github: {e}"))?;
        if !resp.status().is_success() {
            return Err(Self::error(resp).await);
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let size = v["size"].as_u64().unwrap_or(0);
        let raw = v["content"].as_str().unwrap_or_default();
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(raw.replace('\n', ""))
            .map_err(|e| format!("contents decode: {e}"))?;
        Ok((String::from_utf8_lossy(&decoded).into_owned(), size))
    }

    /// `/{base}...{branch}` compare → rendered file list + patches.
    pub async fn compare(&self, repo: &str, branch: &str) -> Result<String, String> {
        let info = self.repo(repo).await?;
        let resp = self
            .get(&format!(
                "/repos/{repo}/compare/{}...{branch}",
                info.default_branch
            ))
            .send()
            .await
            .map_err(|e| format!("github: {e}"))?;
        if !resp.status().is_success() {
            return Err(Self::error(resp).await);
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        let mut out = String::new();
        for f in v["files"].as_array().into_iter().flatten() {
            let path = f["filename"].as_str().unwrap_or("");
            out.push_str(&format!("diff --git a/{path} b/{path}\n"));
            if let Some(patch) = f["patch"].as_str() {
                out.push_str(patch);
                if !patch.ends_with('\n') {
                    out.push('\n');
                }
            }
        }
        Ok(out)
    }

    /// `workflow_dispatch` — the GitHub-agent run primitive. Honest: errors
    /// when the repo has no `cosmos-agent` workflow to dispatch.
    pub async fn dispatch_workflow(
        &self,
        repo: &str,
        workflow: &str,
        branch: &str,
        inputs: serde_json::Value,
    ) -> Result<(), String> {
        let resp = self
            .http
            .post(format!(
                "https://api.github.com/repos/{repo}/actions/workflows/{workflow}/dispatches"
            ))
            .bearer_auth(self.pat)
            .header("accept", "application/vnd.github+json")
            .header("user-agent", "cosmos-desktop")
            .json(&serde_json::json!({"ref": branch, "inputs": inputs}))
            .send()
            .await
            .map_err(|e| format!("github: {e}"))?;
        if resp.status().as_u16() == 204 {
            return Ok(());
        }
        Err(Self::error(resp).await)
    }

    /// The repo picker's row shape — `full_name`, freshness, privacy.
    /// `GET /user/repos?sort=pushed` serves recents; `GET /search/repositories`
    /// serves typed queries; org repos ride `/orgs/{org}/repos`.
    pub async fn repos(&self, query: Option<&str>) -> Result<RepoPage, String> {
        let mut out = Vec::new();
        let mut truncated = false;
        match query.map(str::trim).filter(|q| !q.is_empty()) {
            // Typed query → the search API: `in:name,description` keeps the
            // hits owner/name shaped; user/affiliation qualifier keeps the
            // result inside what the token can actually reach.
            Some(q) => {
                let search = format!("{q} in:name,description fork:true");
                for page in 1..=5 {
                    let resp = self
                        .get(&format!(
                            "/search/repositories?q={}&sort=updated&per_page=100&page={page}",
                            url_query(&search)
                        ))
                        .send()
                        .await
                        .map_err(|e| format!("github: {e}"))?;
                    if !resp.status().is_success() {
                        return Err(Self::error(resp).await);
                    }
                    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
                    let items = v["items"].as_array().cloned().unwrap_or_default();
                    let len = items.len();
                    out.extend(items.into_iter().map(RepoHit::from_value));
                    if len < 100 {
                        break;
                    }
                    if page == 5 {
                        truncated = true;
                    }
                }
            }
            // No query → the token's own recently-pushed repos (recents).
            None => {
                for page in 1..=5 {
                    let resp = self
                        .get(&format!(
                            "/user/repos?sort=pushed&per_page=100&page={page}&affiliation=owner,collaborator,organization_member"
                        ))
                        .send()
                        .await
                        .map_err(|e| format!("github: {e}"))?;
                    if !resp.status().is_success() {
                        return Err(Self::error(resp).await);
                    }
                    let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
                    let items = v.as_array().cloned().unwrap_or_default();
                    let len = items.len();
                    out.extend(items.into_iter().map(RepoHit::from_value));
                    if len < 100 {
                        break;
                    }
                    if page == 5 {
                        truncated = true;
                    }
                }
            }
        }
        Ok(RepoPage {
            items: out,
            truncated,
        })
    }

    /// `GET /user` — the PAT verify ping (save-only-on-success rule).
    /// `GET /user` → the account login.
    pub async fn login(&self) -> Result<String, String> {
        let resp = self
            .get("/user")
            .send()
            .await
            .map_err(|e| format!("github: {e}"))?;
        if !resp.status().is_success() {
            return Err(Self::error(resp).await);
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(v["login"].as_str().unwrap_or_default().to_string())
    }

    pub async fn viewer(&self) -> Result<String, String> {
        let resp = self
            .get("/user")
            .send()
            .await
            .map_err(|e| format!("github: {e}"))?;
        if !resp.status().is_success() {
            return Err(Self::error(resp).await);
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(v["login"]
            .as_str()
            .map(|l| format!("github token works — {l}"))
            .unwrap_or_else(|| "github token works".into()))
    }

    /// `GET /user/orgs` — orgs the token can see, for the picker's grouping.
    pub async fn orgs(&self) -> Result<OrgPage, String> {
        let mut out = Vec::new();
        let mut truncated = false;
        for page in 1..=5 {
            let resp = self
                .get(&format!("/user/orgs?per_page=100&page={page}"))
                .send()
                .await
                .map_err(|e| format!("github: {e}"))?;
            if !resp.status().is_success() {
                return Err(Self::error(resp).await);
            }
            let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let items = v.as_array().cloned().unwrap_or_default();
            let len = items.len();
            out.extend(
                items
                    .iter()
                    .filter_map(|o| o["login"].as_str().map(str::to_string)),
            );
            if len < 100 {
                break;
            }
            if page == 5 {
                truncated = true;
            }
        }
        Ok(OrgPage {
            items: out,
            truncated,
        })
    }
}

/// One repo-picker hit.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RepoHit {
    pub full_name: String,
    pub default_branch: String,
    pub private: bool,
    pub pushed_at: Option<String>,
    pub description: Option<String>,
}

impl RepoHit {
    fn from_value(v: serde_json::Value) -> Self {
        Self {
            full_name: v["full_name"].as_str().unwrap_or_default().to_string(),
            default_branch: v["default_branch"].as_str().unwrap_or("main").to_string(),
            private: v["private"].as_bool().unwrap_or(false),
            pushed_at: v["pushed_at"].as_str().map(str::to_string),
            description: v["description"].as_str().map(str::to_string),
        }
    }
}

/// Percent-encode a GitHub search query (`q=…` value only).
fn url_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Schedules — hourly/daily/weekly shorthands + a minimal 5-field cron.
// ---------------------------------------------------------------------------

/// Compute the next fire time after `from`. Returns `None` for an
/// unparseable schedule — the UI surfaces that as "never".
pub fn next_run(schedule: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let schedule = schedule.trim();
    match schedule {
        "hourly" => {
            let next = from + chrono::Duration::hours(1);
            Some(next)
        }
        "daily" => {
            let next = (from + chrono::Duration::days(1))
                .with_hour(9)?
                .with_minute(0)?
                .with_second(0)?
                .with_nanosecond(0)?;
            Some(next)
        }
        "weekly" => {
            let days = (7 - from.weekday().num_days_from_monday() as i64) % 7;
            let base = from + chrono::Duration::days(days.max(1));
            base.with_hour(9)?
                .with_minute(0)?
                .with_second(0)?
                .with_nanosecond(0)
        }
        cron => cron_next(cron, from),
    }
}

/// Minimal 5-field cron: `min hour dom month dow`, `*`, `*/n`, lists, ranges.
fn cron_next(expr: &str, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return None;
    }
    let minute = cron_field(fields[0], 0, 59)?;
    let hour = cron_field(fields[1], 0, 23)?;
    let dom = cron_field(fields[2], 1, 31)?;
    let month = cron_field(fields[3], 1, 12)?;
    let dow = cron_field(fields[4], 0, 7)?; // 0 or 7 = Sunday
    // Walk minute-by-minute up to a year out; the granularity bounds the cost.
    let mut t = (from + chrono::Duration::minutes(1))
        .with_second(0)?
        .with_nanosecond(0)?;
    for _ in 0..366 * 24 * 60 {
        let dom_match = dom.contains(&(t.day() as u8));
        let dow_num = t.weekday().num_days_from_sunday() as u8;
        let dow_match = dow.contains(&dow_num) || (dow_num == 0 && dow.contains(&7));
        // Vixie semantics: when BOTH dom and dow are restricted, either matches.
        let dom_restricted = fields[2] != "*";
        let dow_restricted = fields[4] != "*";
        let day_ok = match (dom_restricted, dow_restricted) {
            (true, true) => dom_match || dow_match,
            (true, false) => dom_match,
            (false, true) => dow_match,
            (false, false) => true,
        };
        if minute.contains(&(t.minute() as u8))
            && hour.contains(&(t.hour() as u8))
            && day_ok
            && month.contains(&(t.month() as u8))
        {
            return Some(t);
        }
        t += chrono::Duration::minutes(1);
    }
    None
}

fn cron_field(field: &str, lo: u8, hi: u8) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for part in field.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        if let Some(step) = part.strip_prefix("*/") {
            let step: u8 = step.parse().ok()?;
            if step == 0 {
                return None;
            }
            let mut v = lo;
            while v <= hi {
                out.push(v);
                v = v.saturating_add(step);
                if v == 0 {
                    break; // u8 wrapped
                }
            }
            continue;
        }
        if let Some((a, b)) = part.split_once('-') {
            let a: u8 = a.parse().ok()?;
            let b: u8 = b.parse().ok()?;
            if a > b || a < lo || b > hi {
                return None;
            }
            out.extend(a..=b);
            continue;
        }
        if part == "*" {
            out.extend(lo..=hi);
            continue;
        }
        let v: u8 = part.parse().ok()?;
        if v < lo || v > hi {
            return None;
        }
        out.push(v);
    }
    out.sort_unstable();
    out.dedup();
    Some(out)
}

/// The run ledger lives next to the config (`cloud-runs.json`) — the file
/// name it reports for diagnostics.
pub fn runs_file_name() -> &'static str {
    RUNS_FILE
}

// ---------------------------------------------------------------------------
// The automation scheduler — one tokio task per engine (the spec's "existing
// engine timer": the engine owns the tick, there is no separate daemon).
// A fire creates a fresh chat bound to the automation's workspace or seat and
// queues a durable `Run` command — the host's normal drain then executes it,
// so cloud-seat automations ride the same remote-dispatch path as the
// composer. A seat that isn't reachable marks the fire `missed` (surfaced on
// Kanban) instead of erroring a chat nobody asked for.
// ---------------------------------------------------------------------------

pub struct Scheduler {
    store: CloudStore,
    doc_host: crate::doc_host::DocHost,
    workspace: crate::workspace_host::WorkspaceHost,
    device_id: String,
    http: reqwest::Client,
}

impl Scheduler {
    pub fn new(
        store: CloudStore,
        doc_host: crate::doc_host::DocHost,
        workspace: crate::workspace_host::WorkspaceHost,
        device_id: String,
    ) -> Self {
        Self {
            store,
            doc_host,
            workspace,
            device_id,
            http: reqwest::Client::new(),
        }
    }

    /// Spawn the tick loop (30s granularity — schedules are minute-level).
    /// The task runs for the engine's lifetime (its fields keep the stores
    /// alive); there is deliberately no second daemon for schedules.
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            // A short boot delay lets the workspace doc hydrate before the
            // first claim pass.
            tokio::time::sleep(Duration::from_secs(10)).await;
            loop {
                if let Err(e) = self.tick().await {
                    tracing::warn!(error = %e, "cloud automation tick failed");
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        })
    }

    /// One scheduler pass: snapshot any Railway seat nearing its deadline,
    /// then claim due automations and fire each.
    async fn tick(&self) -> Result<(), EngineError> {
        self.backup_sweep().await;
        for automation in self.store.claim_due(Utc::now()) {
            if let Err(e) = self.fire(&automation).await {
                tracing::warn!(automation = %automation.id, error = %e, "automation fire failed");
                self.store.mark_automation(&automation.id, "missed", None);
            }
        }
        Ok(())
    }

    /// Railway safeguard: ~10 minutes before a trial expires, pull the
    /// workspace down to `seat-backups/` so a Boat seat can restore it.
    /// One snapshot per seat; failures retry on a ~2-minute cadence until
    /// the deadline kills the box (which ends retries naturally).
    async fn backup_sweep(&self) {
        let now = Utc::now();
        for seat in self.store.seats() {
            if seat.kind != Some(CloudSeatKind::Railway) || seat.backup_path.is_some() {
                continue;
            }
            let Some(deadline) = seat.expires_at else {
                continue;
            };
            let remaining = (deadline - now).num_seconds();
            if !(0..=600).contains(&remaining) {
                continue;
            }
            if seat
                .backup_attempted_at
                .is_some_and(|t| (now - t).num_seconds() < 120)
            {
                continue;
            }
            self.store.update_seat(&seat.id, |s| {
                s.backup_attempted_at = Some(now);
            });
            match snapshot_workspace(&self.store, &seat).await {
                Ok(path) => self.store.update_seat(&seat.id, |s| {
                    s.backup_path = Some(path.to_string_lossy().into_owned());
                    s.backup_at = Some(now);
                }),
                Err(e) => {
                    tracing::warn!(seat = %seat.id, error = %e, "railway workspace snapshot failed")
                }
            }
        }
    }

    /// Fire one automation: probe the seat (cloud runs only), create the
    /// run's chat, queue a durable `Run` — the drain dispatches it (remote
    /// for seat-bound chats). Unreachable seats → `missed`, no chat.
    async fn fire(&self, automation: &CloudAutomation) -> Result<(), EngineError> {
        let seat = automation
            .seat_id
            .as_deref()
            .and_then(|id| self.store.seat(id));
        let binding = match &seat {
            Some(seat) => {
                // Cheap reachability check BEFORE creating a chat.
                if !self.seat_reachable(seat).await {
                    self.store.mark_automation(&automation.id, "missed", None);
                    return Ok(());
                }
                Some(CloudSeatBinding {
                    seat_id: seat.id.clone(),
                    kind: seat.kind.unwrap_or(CloudSeatKind::Ssh),
                    label: seat.label.clone(),
                    repo: seat.repo.clone(),
                    workspace_path: Some(seat.workspace()),
                    model_label: None,
                    expires_at: seat.expires_at,
                })
            }
            None => None,
        };

        // The run's chat — one per fire, titled `name · timestamp`.
        let chat_id = crate::new_id();
        let title = format!(
            "{} · {}",
            automation.name,
            Utc::now().format("%b %-d %H:%M")
        );
        self.workspace.create_chat_with_parent(
            &chat_id,
            automation.space_id.as_deref(),
            Some(&self.device_id),
            None,
            seat.as_ref().map(|s| s.workspace()),
            None,
            binding,
        )?;
        self.workspace
            .rename_chat(&chat_id, &title)
            .map_err(|e| EngineError::Other(e.to_string()))?;

        let cwd = seat
            .as_ref()
            .map(|s| s.workspace())
            .or_else(|| {
                automation
                    .space_id
                    .as_deref()
                    .and_then(|id| self.workspace.space(id).ok().flatten())
                    .map(|space| space.path)
            })
            .unwrap_or_else(|| "~".to_string());
        self.doc_host.queue_command(
            &chat_id,
            zeron_doc::SessionCommandPayload::Run {
                request: zeron_proto::RunRequest {
                    prompt: automation.prompt.clone(),
                    harness: None,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    cwd,
                    sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
                    auto_approve: true,
                    resume: None,
                    attachments: Vec::new(),
                    worktree: None,
                },
                message_id: crate::new_id(),
            },
        )?;
        self.store
            .mark_automation(&automation.id, "ok", Some(&chat_id));
        Ok(())
    }

    /// A bounded liveness check for a fire decision — NOT the full probe
    /// (5s ssh ping, or a repo read for GitHub seats).
    async fn seat_reachable(&self, seat: &SeatConfig) -> bool {
        match seat.kind {
            // Railway trials die at the hour — automations never target them
            // (the save path rejects them too; belt and braces for rows
            // written by older builds).
            Some(CloudSeatKind::Railway) => false,
            Some(CloudSeatKind::Ssh) => match &seat.ssh_alias {
                Some(alias) => ssh_exec(alias, "true", Duration::from_secs(6))
                    .await
                    .map(|o| o.code == 0)
                    .unwrap_or(false),
                None => false,
            },
            Some(CloudSeatKind::GitHub) => {
                match (seat.repo.clone(), self.store.github_pat(&seat.id)) {
                    (Some(repo), Some(pat)) => github(&self.http, &pat).repo(&repo).await.is_ok(),
                    _ => false,
                }
            }
            Some(CloudSeatKind::Box) => {
                match (self.store.box_token(&seat.id), seat.box_id.clone()) {
                    (Some(token), Some(box_id)) => {
                        crate::boat::boat(&self.http, &token, seat.box_endpoint.as_deref())
                            .sandbox(&box_id)
                            .await
                            .map(|s| s.usable() || s.sleeping())
                            .unwrap_or(false)
                    }
                    _ => false,
                }
            }
            None => false,
        }
    }
}

/// An automation-enabled engine wires the scheduler at assembly; keeping it
/// `Arc` lets `shutdown` drop the task cleanly.
pub type SchedulerHandle = Arc<Scheduler>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_seat_uses_the_chat_checkout() {
        let seat = SeatConfig {
            id: "s".into(),
            workspace_path: Some("/app".into()),
            ..Default::default()
        };
        let scoped = seat.clone().scoped(Some("/app/cosmos"), Some("me/cosmos"));
        assert_eq!(scoped.workspace(), "/app/cosmos");
        assert_eq!(scoped.repo.as_deref(), Some("me/cosmos"));
        let unscoped = seat.scoped(Some(" "), None);
        assert_eq!(unscoped.workspace(), "/app");
        assert_eq!(unscoped.repo, None);
    }

    #[test]
    fn repo_prelude_keeps_the_token_out_of_the_remote() {
        let prelude = repo_checkout_prelude("me/cosmos");
        assert!(prelude.contains("'https://github.com/me/cosmos.git'"));
        assert!(prelude.contains("$GITHUB_TOKEN"));
        assert!(!prelude.contains("x-access-token:$GITHUB_TOKEN@"));
    }

    fn at(minute: u32) -> DateTime<Utc> {
        "2026-09-28T10:00:00Z".parse::<DateTime<Utc>>().unwrap()
            + chrono::Duration::minutes(minute as i64)
    }

    #[test]
    fn shorthands_compute_future_times() {
        let now = at(0);
        assert!(next_run("hourly", now).unwrap() > now);
        let daily = next_run("daily", now).unwrap();
        assert!(daily > now && daily.hour() == 9);
        assert!(next_run("weekly", now).unwrap() > now);
    }

    #[test]
    fn cron_matches_next_minute() {
        let now = at(0);
        // Every minute.
        assert_eq!(next_run("* * * * *", now), Some(at(1)));
        // On the hour.
        let n = next_run("0 * * * *", now).unwrap();
        assert_eq!(n.minute(), 0);
        assert!(n > now);
        // Lists and ranges.
        let n = next_run("*/15 9-17 * * *", now).unwrap();
        assert!(n.minute() % 15 == 0);
        // Sunday via dow=0.
        let n = next_run("0 9 * * 0", now).unwrap();
        assert_eq!(n.weekday().num_days_from_sunday(), 0);
        // Garbage never fires.
        assert!(next_run("not a cron", now).is_none());
        assert!(next_run("* * * *", now).is_none());
    }

    #[test]
    fn store_roundtrip_and_redaction() {
        let dir = tempfile::tempdir().unwrap();
        let store = CloudStore::load(dir.path());
        store.save_seat(SeatConfig {
            id: "vps-1".into(),
            kind: Some(CloudSeatKind::Ssh),
            label: "dev box".into(),
            ssh_alias: Some("devbox".into()),
            ..Default::default()
        });
        store.set_seat_token("vps-1", "githubPat", Some("pat-123".into()));
        store.save_provider(
            ProviderConfig {
                provider: Some(ByokProvider::Anthropic),
                model: Some("claude-opus-4-6".into()),
                use_for_cloud: true,
                ..Default::default()
            },
            Some("sk-ant-secret".into()),
        );
        let view = store.view();
        // Secrets never serialize into the view — only the has_* flags.
        let text = view.to_string();
        assert!(!text.contains("sk-ant-secret") && !text.contains("pat-123"));
        assert_eq!(view["providers"][0]["hasKey"], true);
        assert_eq!(view["seats"][0]["hasGithubPat"], true);
        assert_eq!(view["seats"][0]["capabilities"]["exec"], true);
        // Reloaded store sees the same state.
        let reloaded = CloudStore::load(dir.path());
        assert_eq!(reloaded.github_pat("vps-1").as_deref(), Some("pat-123"));
        assert_eq!(
            reloaded.provider_key(ByokProvider::Anthropic).as_deref(),
            Some("sk-ant-secret")
        );
        assert_eq!(
            reloaded.cloud_env(),
            vec![("ANTHROPIC_API_KEY".to_string(), "sk-ant-secret".to_string())]
        );
    }

    fn railway_seat(id: &str, expires_at: DateTime<Utc>, probed_at: DateTime<Utc>) -> SeatConfig {
        SeatConfig {
            id: id.into(),
            kind: Some(CloudSeatKind::Railway),
            expires_at: Some(expires_at),
            last_probe: Some(SeatProbe {
                seat_id: id.into(),
                online: true,
                detail: None,
                containers: Vec::new(),
                image_digest: None,
                controllable: false,
                keeps_running: false,
                sleeping: false,
                probed_at,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn load_keeps_provisioned_expired_railway_seat_over_empty_starting_seat() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let mut empty = railway_seat("railway-empty", now, now);
        empty.expires_at = None;
        let probe = empty.last_probe.as_mut().unwrap();
        probe.online = false;
        probe.detail = Some("railway.new refused the connection".into());

        let mut provisioned = railway_seat(
            "railway-provisioned",
            now - chrono::Duration::minutes(1),
            now - chrono::Duration::minutes(2),
        );
        provisioned.claim_url = Some("https://railway.com/claim".into());
        provisioned.harness_command = Some("opencode run -m opencode/big-pickle".into());
        provisioned.preview_url = Some("https://preview.railway.app".into());
        provisioned.backup_path = Some("seat-backups/railway-provisioned.tar.gz".into());
        provisioned.backup_at = Some(now - chrono::Duration::minutes(1));
        let config = CloudConfigFile {
            seats: vec![empty, provisioned],
            default_seat: Some("railway-empty".into()),
            ..Default::default()
        };
        CloudStore::write(dir.path(), CONFIG_FILE, &config, false).unwrap();

        let store = CloudStore::load(dir.path());
        let seats = store.seats();
        assert_eq!(seats.len(), 1);
        let kept = &seats[0];
        assert_eq!(kept.id, "railway-provisioned");
        assert_eq!(
            kept.harness_command.as_deref(),
            Some("opencode run -m opencode/big-pickle")
        );
        assert_eq!(kept.claim_url.as_deref(), Some("https://railway.com/claim"));
        assert_eq!(
            kept.preview_url.as_deref(),
            Some("https://preview.railway.app")
        );
        assert_eq!(
            kept.backup_path.as_deref(),
            Some("seat-backups/railway-provisioned.tar.gz")
        );
        assert_eq!(store.default_seat().unwrap().id, "railway-provisioned");
    }

    #[test]
    fn load_inherits_harness_from_dropped_expired_railway_seat() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let live = railway_seat(
            "railway-live",
            now + chrono::Duration::minutes(30),
            now - chrono::Duration::minutes(2),
        );
        let mut expired = railway_seat(
            "railway-expired-with-harness",
            now - chrono::Duration::minutes(1),
            now - chrono::Duration::minutes(1),
        );
        expired.harness_command = Some("claude -p".into());
        let config = CloudConfigFile {
            seats: vec![live, expired],
            ..Default::default()
        };
        CloudStore::write(dir.path(), CONFIG_FILE, &config, false).unwrap();

        let store = CloudStore::load(dir.path());
        let seats = store.seats();
        assert_eq!(seats.len(), 1);
        assert_eq!(seats[0].id, "railway-live");
        assert_eq!(seats[0].harness_command.as_deref(), Some("claude -p"));
    }

    #[test]
    fn load_deduplicates_railway_seats_and_preserves_backups() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let older_live = railway_seat(
            "railway-older-live",
            now + chrono::Duration::minutes(30),
            now - chrono::Duration::minutes(2),
        );
        let newest_live = railway_seat(
            "railway-newest-live",
            now + chrono::Duration::minutes(60),
            now - chrono::Duration::minutes(1),
        );
        let expired = railway_seat("railway-expired", now - chrono::Duration::minutes(1), now);
        let other = SeatConfig {
            id: "ssh-seat".into(),
            kind: Some(CloudSeatKind::Ssh),
            ..Default::default()
        };
        let config = CloudConfigFile {
            seats: vec![older_live, expired, newest_live, other.clone()],
            default_seat: Some("railway-expired".into()),
            ..Default::default()
        };
        CloudStore::write(dir.path(), CONFIG_FILE, &config, false).unwrap();
        let backup = dir.path().join("seat-backups").join("railway.tar.gz");
        std::fs::create_dir_all(backup.parent().unwrap()).unwrap();
        std::fs::write(&backup, b"workspace backup").unwrap();

        let store = CloudStore::load(dir.path());
        let seats = store.seats();
        assert_eq!(seats.len(), 2);
        assert!(seats.iter().any(|seat| seat.id == "railway-newest-live"));
        assert!(seats.iter().any(|seat| seat.id == "ssh-seat"));
        assert_eq!(store.default_seat().unwrap().id, "railway-newest-live");
        assert!(backup.exists());

        let persisted = CloudStore::load(dir.path());
        assert_eq!(persisted.seats().len(), 2);
        assert!(
            persisted
                .seats()
                .iter()
                .any(|seat| seat.id == "railway-newest-live")
        );
        assert!(persisted.seats().iter().any(|seat| seat.id == other.id));
    }

    #[test]
    fn load_keeps_most_recently_probed_railway_seat_when_all_expired() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let config = CloudConfigFile {
            seats: vec![
                railway_seat(
                    "railway-old-probe",
                    now - chrono::Duration::minutes(10),
                    now - chrono::Duration::minutes(5),
                ),
                railway_seat(
                    "railway-recent-probe",
                    now - chrono::Duration::minutes(2),
                    now - chrono::Duration::minutes(1),
                ),
            ],
            ..Default::default()
        };
        CloudStore::write(dir.path(), CONFIG_FILE, &config, false).unwrap();

        let store = CloudStore::load(dir.path());
        let seats = store.seats();
        assert_eq!(seats.len(), 1);
        assert_eq!(seats[0].id, "railway-recent-probe");
    }

    #[test]
    fn container_name_defaults_to_prefix() {
        let seat = SeatConfig {
            id: "myws".into(),
            ..Default::default()
        };
        assert_eq!(seat.container_name(), "cosmos-cloud-myws");
        let named = SeatConfig {
            id: "myws".into(),
            container: Some("custom-box".into()),
            ..Default::default()
        };
        assert_eq!(named.container_name(), "custom-box");
    }

    #[test]
    fn railway_without_alias_is_ready_until_expired() {
        let mut seat = SeatConfig {
            kind: Some(CloudSeatKind::Railway),
            detached: true,
            expires_at: Some(Utc::now() + chrono::Duration::minutes(5)),
            ..Default::default()
        };
        let capabilities = seat.capabilities();
        assert!(capabilities.exec && capabilities.shell && capabilities.files && capabilities.git);
        assert!(capabilities.keeps_running);

        seat.expires_at = Some(Utc::now() - chrono::Duration::minutes(1));
        assert!(!seat.capabilities().exec);
    }

    #[test]
    fn workspace_command_creates_the_target_before_entering_it() {
        assert_eq!(
            command_in_workspace("/app space", "printf ok"),
            "mkdir -p '/app space' && cd '/app space' && printf ok"
        );
    }

    #[test]
    fn claim_due_advances_next_run_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = CloudStore::load(dir.path());
        let now = at(0);
        store.save_automation(CloudAutomation {
            id: "a1".into(),
            name: "nightly".into(),
            prompt: "run the sweep".into(),
            schedule: "hourly".into(),
            seat_id: None,
            space_id: None,
            enabled: true,
            last_run_at: None,
            next_run_at: Some(now - chrono::Duration::minutes(5)),
            last_status: None,
            last_chat_id: None,
        });
        let due = store.claim_due(now);
        assert_eq!(due.len(), 1);
        // Claimed: a second call at the same instant fires nothing.
        assert!(store.claim_due(now).is_empty());
        let automation = store.automations().remove(0);
        assert!(automation.next_run_at.unwrap() > now);
    }
}

/// The Cosmos GitHub App's public OAuth client id — device flow needs no
/// client secret, so nothing sensitive ships in the app.
pub const COSMOS_GITHUB_CLIENT_ID: &str = "Iv23lijEVv5jdl2f7RTm";

/// Where a user installs the Cosmos GitHub App on their repos.
pub const COSMOS_GITHUB_INSTALL_URL: &str = "https://github.com/apps/cosmos-ade/installations/new";

#[derive(Debug)]
pub enum DevicePoll {
    /// `authorization_pending` or `slow_down`.
    Pending(String),
    Token(String),
}

/// Start GitHub's device flow for the Cosmos app.
pub async fn github_device_start(http: &reqwest::Client) -> Result<serde_json::Value, String> {
    let resp = http
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&[("client_id", COSMOS_GITHUB_CLIENT_ID)])
        .send()
        .await
        .map_err(|e| format!("github: {e}"))?;
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("github: {e}"))?;
    device_error(&v)?;
    let field = |k: &str| v.get(k).cloned().unwrap_or(serde_json::Value::Null);
    Ok(serde_json::json!({
        "deviceCode": field("device_code"),
        "userCode": field("user_code"),
        "verificationUri": field("verification_uri"),
        "interval": v.get("interval").and_then(|i| i.as_u64()).unwrap_or(5),
        "expiresIn": v.get("expires_in").and_then(|i| i.as_u64()).unwrap_or(900),
    }))
}

/// One device-flow token poll.
pub async fn github_device_poll(
    http: &reqwest::Client,
    device_code: &str,
) -> Result<DevicePoll, String> {
    let resp = http
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .form(&[
            ("client_id", COSMOS_GITHUB_CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .map_err(|e| format!("github: {e}"))?;
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("github: {e}"))?;
    parse_device_poll(&v)
}

fn parse_device_poll(v: &serde_json::Value) -> Result<DevicePoll, String> {
    if let Some(token) = v.get("access_token").and_then(|t| t.as_str()) {
        return Ok(DevicePoll::Token(token.to_string()));
    }
    match v.get("error").and_then(|e| e.as_str()) {
        Some(code @ ("authorization_pending" | "slow_down")) => {
            Ok(DevicePoll::Pending(code.to_string()))
        }
        _ => device_error(v).map(|_| DevicePoll::Pending("authorization_pending".into())),
    }
}

fn device_error(v: &serde_json::Value) -> Result<(), String> {
    let Some(code) = v.get("error").and_then(|e| e.as_str()) else {
        return Ok(());
    };
    Err(match code {
        "expired_token" => "The GitHub code expired — start again".into(),
        "access_denied" => "GitHub access was denied".into(),
        "device_flow_disabled" => {
            "Device flow is off for the Cosmos GitHub App — enable it in the app settings".into()
        }
        _ => v
            .get("error_description")
            .and_then(|d| d.as_str())
            .unwrap_or(code)
            .to_string(),
    })
}

#[cfg(test)]
mod device_flow_tests {
    use super::*;

    #[test]
    fn device_poll_states() {
        let pending = serde_json::json!({"error": "authorization_pending"});
        assert!(
            matches!(parse_device_poll(&pending), Ok(DevicePoll::Pending(s)) if s == "authorization_pending")
        );
        let done = serde_json::json!({"access_token": "ghu_x", "token_type": "bearer"});
        assert!(matches!(parse_device_poll(&done), Ok(DevicePoll::Token(t)) if t == "ghu_x"));
        let disabled = serde_json::json!({"error": "device_flow_disabled"});
        assert!(
            parse_device_poll(&disabled)
                .unwrap_err()
                .contains("Device flow is off")
        );
    }
}

/// Captures the seat's X display when one is running, otherwise a live
/// system snapshot — Railway, Boat and VPS seats are headless Linux boxes.
const LINUX_SCREEN_SCRIPT: &str = r#"
d=""
for s in /tmp/.X11-unix/X*; do [ -e "$s" ] && d=":${s##*/X}" && break; done
if [ -n "$d" ]; then
  f="/tmp/cosmos-screen-$$.png"
  if command -v import >/dev/null 2>&1; then DISPLAY=$d import -window root "$f" >/dev/null 2>&1
  elif command -v scrot >/dev/null 2>&1; then DISPLAY=$d scrot -o "$f" >/dev/null 2>&1
  fi
  if [ -s "$f" ]; then echo "__PNG__"; base64 "$f" | tr -d '\n'; echo; rm -f "$f"; exit 0; fi
  rm -f "$f"
fi
echo "__SYS__"
[ -r /etc/os-release ] && . /etc/os-release
echo "os=${PRETTY_NAME:-$(uname -s)}"
echo "kernel=$(uname -r)"
echo "arch=$(uname -m)"
echo "host=$(hostname 2>/dev/null || cat /etc/hostname)"
echo "user=$(id -un 2>/dev/null)"
echo "cpus=$(nproc 2>/dev/null || getconf _NPROCESSORS_ONLN)"
echo "uptime=$(cut -d' ' -f1 /proc/uptime 2>/dev/null)"
echo "load=$(cut -d' ' -f1 /proc/loadavg 2>/dev/null)"
awk '/^MemTotal:/{print "mem_total_kb="$2} /^MemAvailable:/{print "mem_avail_kb="$2}' /proc/meminfo 2>/dev/null
df -Pk / 2>/dev/null | awk 'NR==2{print "disk_total_kb="$2; print "disk_used_kb="$3}'
echo "display=$d"
echo "__PROCS__"
ps -eo pid=,pcpu=,pmem=,comm= --sort=-pcpu 2>/dev/null | head -8
"#;

pub async fn linux_screen(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
) -> Result<serde_json::Value, String> {
    let out = seat_exec(
        http,
        store,
        seat,
        LINUX_SCREEN_SCRIPT,
        Duration::from_secs(20),
    )
    .await?;
    parse_linux_screen(&crate::railway::redact_trial_banner(&out.stdout)).ok_or_else(|| {
        let err = crate::railway::redact_trial_banner(out.stderr.trim());
        if err.is_empty() {
            "couldn't reach the seat — it may be stopped or still starting".to_string()
        } else {
            err
        }
    })
}

fn parse_linux_screen(stdout: &str) -> Option<serde_json::Value> {
    let mut lines = stdout.lines().skip_while(|l| {
        let l = l.trim();
        l != "__PNG__" && l != "__SYS__"
    });
    match lines.next()?.trim() {
        "__PNG__" => {
            let png = lines.next()?.trim();
            (!png.is_empty()).then(|| serde_json::json!({ "pngBase64": png }))
        }
        _ => {
            let mut system = serde_json::Map::new();
            let mut procs = Vec::new();
            let mut in_procs = false;
            for line in lines {
                if line.trim() == "__PROCS__" {
                    in_procs = true;
                    continue;
                }
                if in_procs {
                    let cols: Vec<&str> = line.split_whitespace().collect();
                    if cols.len() >= 4 {
                        procs.push(serde_json::json!({
                            "pid": cols[0],
                            "cpu": cols[1],
                            "mem": cols[2],
                            "name": cols[3..].join(" "),
                        }));
                    }
                } else if let Some((k, v)) = line.split_once('=') {
                    system.insert(
                        k.trim().to_string(),
                        serde_json::Value::String(v.trim().to_string()),
                    );
                }
            }
            system.insert("procs".into(), serde_json::Value::Array(procs));
            Some(serde_json::json!({ "system": system }))
        }
    }
}

#[cfg(test)]
mod linux_screen_tests {
    use super::*;

    #[test]
    fn parses_system_snapshot() {
        let out = "noise\n__SYS__\nos=Ubuntu 24.04 LTS\ncpus=2\nmem_total_kb=2048000\n__PROCS__\n  1  0.5  1.0 node\n 22 12.0  3.1 opencode run\n";
        let v = parse_linux_screen(out).unwrap();
        assert_eq!(v["system"]["os"], "Ubuntu 24.04 LTS");
        assert_eq!(v["system"]["procs"][1]["name"], "opencode run");
    }

    #[test]
    fn parses_png_capture() {
        let v = parse_linux_screen("__PNG__\niVBORw0K\n").unwrap();
        assert_eq!(v["pngBase64"], "iVBORw0K");
    }
}
