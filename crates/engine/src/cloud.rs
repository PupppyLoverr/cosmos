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
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use zeron_proto::{
    ByokProvider, CloudAutomation, CloudRun, CloudSeatBinding, CloudSeatKind, SeatCapabilities,
    SeatProbe, TaskDraft, WorkspaceBackup,
};

use crate::EngineError;

const CONFIG_FILE: &str = "cloud.json";
const KEYS_FILE: &str = "cloud-keys.json";
const RUNS_FILE: &str = "cloud-runs.json";
const BACKUPS_DIR: &str = "seat-backups";
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const REGISTRY_PROGRESS_INTERVAL: Duration = Duration::from_secs(2);
/// `docker exec` container prefix (spec): `cosmos-cloud-<workspace>`.
pub const CONTAINER_PREFIX: &str = "cosmos-cloud";

pub(crate) fn migrate_legacy_cloud_files(
    data_dir: &Path,
    store_root: &Path,
) -> std::io::Result<bool> {
    let legacy_config = data_dir.join(CONFIG_FILE);
    if store_root.join(CONFIG_FILE).exists() || !legacy_config.is_file() {
        return Ok(false);
    }

    std::fs::create_dir_all(store_root)?;
    let mut moved = false;
    for name in [KEYS_FILE, RUNS_FILE, BACKUPS_DIR, CONFIG_FILE] {
        let source = data_dir.join(name);
        let destination = store_root.join(name);
        if source.exists() && !destination.exists() {
            move_legacy_path(&source, &destination)?;
            moved = true;
        }
    }
    if moved {
        tracing::info!(
            source = %data_dir.display(),
            destination = %store_root.display(),
            "migrated legacy cloud state into the synced profile"
        );
    }
    Ok(moved)
}

fn move_legacy_path(source: &Path, destination: &Path) -> std::io::Result<()> {
    match std::fs::rename(source, destination) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            let staging = destination.with_extension("cloud-migration");
            let _ = remove_path(&staging);
            let copied = if source.is_dir() {
                copy_dir_recursive(source, &staging)
            } else {
                std::fs::copy(source, &staging).map(|_| ())
            };
            if let Err(error) = copied {
                let _ = remove_path(&staging);
                return Err(error);
            }
            if let Err(error) = std::fs::rename(&staging, destination) {
                let _ = remove_path(&staging);
                return Err(error);
            }
            remove_path(source)?;
        }
        Err(error) => return Err(error),
    }
    if destination
        .file_name()
        .is_some_and(|name| name == KEYS_FILE)
    {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(from, to)?;
        }
    }
    Ok(())
}

fn remove_path(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

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
    /// Registry device id of the headless Cosmos runner assigned to this seat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_device_id: Option<String>,
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
    /// Railway signup link — opens in the user's browser, never embedded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signup_url: Option<String>,
    /// Railway's long-poll URL for signup completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signup_poll_url: Option<String>,
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
                    // The seat's own virtual desktop (`seat_desktop`) rides
                    // exec — no separate CUA endpoint needed.
                    computer: alive || self.cua_endpoint.is_some(),
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
                computer: self.ssh_alias.is_some() || self.cua_endpoint.is_some(),
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
                computer: self.box_id.is_some() || self.cua_endpoint.is_some(),
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
pub fn repo_checkout_prelude(repo: &str, branch: Option<&str>) -> String {
    let url = shell_word(&format!("https://github.com/{repo}.git"));
    let helper =
        shell_word("!f() { echo username=x-access-token; echo \"password=$GITHUB_TOKEN\"; }; f");
    // The picked branch only steers the FIRST clone: later turns keep
    // whatever branch the agent moved the checkout to.
    let branch = branch
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(|b| format!(" --branch {}", shell_word(b)))
        .unwrap_or_default();
    format!(
        "if [ ! -d .git ]; then git -c credential.helper= -c credential.helper={helper} clone --quiet{branch} {url} . || exit 1; fi; \
git config credential.helper {helper}"
    )
}

/// Where a repo chat's checkout lives on a seat: `<workspace>/<name>` for
/// the repo's default branch, `<workspace>/<name>@<branch>` for another
/// branch — so two chats on different branches never share a working tree.
pub fn repo_workspace(base: &str, full_name: &str, branch: Option<&str>) -> String {
    let name = full_name.rsplit('/').next().unwrap_or(full_name);
    let base = base.trim_end_matches('/');
    match branch.map(str::trim).filter(|b| !b.is_empty()) {
        None => format!("{base}/{name}"),
        Some(branch) => {
            let slug: String = branch
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                        c
                    } else {
                        '-'
                    }
                })
                .collect();
            format!("{base}/{name}@{}", slug.trim_matches('.'))
        }
    }
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
    /// Per-chat workspace snapshots on this computer (`seat-backups/`).
    backups: Vec<WorkspaceBackup>,
    /// Kanban drafts — tasks created but not started.
    drafts: Vec<TaskDraft>,
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

pub(crate) enum RegistryWrite {
    Run(Box<CloudRun>),
    Backup(WorkspaceBackup),
}

struct CloudStoreInner {
    dir: PathBuf,
    state: RwLock<CloudState>,
    registry_sync: std::sync::Mutex<RegistrySyncState>,
    /// Automations asked to fire now ("Run now"), drained by the scheduler.
    run_now: std::sync::Mutex<Vec<String>>,
    run_now_signal: tokio::sync::Notify,
}

#[derive(Default)]
struct RegistrySyncState {
    sink: Option<Arc<dyn Fn(RegistryWrite) + Send + Sync>>,
    last_progress_write: HashMap<String, Instant>,
}

fn registry_safe_run(run: CloudRun) -> CloudRun {
    let mut run = run;
    run.command = None;
    run.error = None;
    run.prompt = None;
    run
}

fn cloud_run_progress_only(before: &CloudRun, after: &CloudRun) -> bool {
    if before.mirrored_bytes == after.mirrored_bytes && before.remote_cursor == after.remote_cursor
    {
        return false;
    }
    let mut comparable_before = before.clone();
    comparable_before.mirrored_bytes = after.mirrored_bytes;
    comparable_before.remote_cursor = after.remote_cursor.clone();
    comparable_before.updated_at = after.updated_at;
    comparable_before == *after
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
                registry_sync: std::sync::Mutex::new(RegistrySyncState::default()),
                run_now: std::sync::Mutex::new(Vec::new()),
                run_now_signal: tokio::sync::Notify::new(),
            }),
        }
    }

    pub(crate) fn set_registry_sink(&self, sink: Arc<dyn Fn(RegistryWrite) + Send + Sync>) {
        self.inner
            .registry_sync
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sink = Some(sink);
    }

    fn emit_registry_write(&self, write: RegistryWrite) {
        let sync = self
            .inner
            .registry_sync
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sink) = &sync.sink {
            sink(write);
        }
    }

    fn emit_registry_run(&self, run: CloudRun, progress_only: bool) {
        let now = Instant::now();
        let mut sync = self
            .inner
            .registry_sync
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if progress_only
            && sync
                .last_progress_write
                .get(&run.chat_id)
                .is_some_and(|last| now.duration_since(*last) < REGISTRY_PROGRESS_INTERVAL)
        {
            return;
        }
        let Some(sink) = sync.sink.clone() else {
            return;
        };
        sync.last_progress_write.insert(run.chat_id.clone(), now);
        sink(RegistryWrite::Run(Box::new(registry_safe_run(run))));
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
        self.seat(&binding.seat_id).map(|seat| {
            let mut seat = seat.scoped(binding.workspace_path.as_deref(), binding.repo.as_deref());
            // The chat's branch pick wins; a GitHub seat's own configured
            // branch stays when the chat names none.
            if let Some(branch) = binding.branch.as_deref().map(str::trim)
                && !branch.is_empty()
            {
                seat.branch = Some(branch.to_string());
            }
            seat
        })
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

    /// Provider keys a legacy cloud run may export to the seat — only rows
    /// explicitly toggled "use this key for Cloud sessions" are eligible.
    pub fn cloud_env(&self) -> Vec<(String, String)> {
        let state = self.inner.state.read().unwrap();
        state
            .config
            .providers
            .iter()
            .filter(|p| p.use_for_cloud)
            .filter_map(|p| {
                let provider = p.provider?;
                let key = state.keys.provider_keys.get(provider.id())?;
                let env = provider_key_env(provider);
                Some((env.to_string(), key.clone()))
            })
            .collect()
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
        let _ = self.persist_config();
    }

    pub fn remove_automation(&self, id: &str) {
        let mut state = self.inner.state.write().unwrap();
        state.config.automations.retain(|a| a.id != id);
        drop(state);
        let _ = self.persist_config();
    }

    /// Due automations are claimed in one mutation: each gets `next_run_at`
    /// advanced past `now` before the caller dispatches, so a crash can't
    /// double-fire the same tick.
    /// Queue an off-schedule fire ("Run now") — the scheduler wakes at once.
    /// Runs even when the automation is paused; its schedule is untouched.
    pub fn request_run_now(&self, id: &str) -> Result<(), String> {
        if !self.automations().iter().any(|a| a.id == id) {
            return Err(format!("no such automation: {id}"));
        }
        self.inner.run_now.lock().unwrap().push(id.to_string());
        self.inner.run_now_signal.notify_one();
        Ok(())
    }

    fn take_run_now(&self) -> Vec<CloudAutomation> {
        let ids = std::mem::take(&mut *self.inner.run_now.lock().unwrap());
        let automations = self.automations();
        let mut out: Vec<CloudAutomation> = Vec::new();
        for id in ids {
            if out.iter().any(|a| a.id == id) {
                continue;
            }
            if let Some(a) = automations.iter().find(|a| a.id == id) {
                out.push(a.clone());
            }
        }
        if !out.is_empty() {
            let now = Utc::now();
            let mut state = self.inner.state.write().unwrap();
            for automation in state.config.automations.iter_mut() {
                if out.iter().any(|a| a.id == automation.id) {
                    automation.last_run_at = Some(now);
                }
            }
            drop(state);
            let _ = self.persist_config();
        }
        out
    }

    pub fn claim_due(&self, now: DateTime<Utc>) -> Vec<CloudAutomation> {
        let mut state = self.inner.state.write().unwrap();
        let mut due = Vec::new();
        let mut scheduled = false;
        for automation in state.config.automations.iter_mut() {
            if !automation.enabled {
                continue;
            }
            // A row with no next fire (hand-edited config, an older build)
            // gets scheduled from now — never fired on sight.
            let Some(due_at) = automation.next_run_at else {
                automation.next_run_at = next_run(&automation.schedule, now);
                scheduled = true;
                continue;
            };
            if due_at > now {
                continue;
            }
            automation.last_run_at = Some(now);
            automation.next_run_at = next_run(&automation.schedule, now);
            due.push(automation.clone());
        }
        if !due.is_empty() || scheduled {
            drop(state);
            let _ = self.persist_config();
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
        let _ = self.persist_config();
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
        let registry_run = run.clone();
        let mut state = self.inner.state.write().unwrap();
        match state.runs.runs.iter_mut().find(|r| r.id == run.id) {
            Some(existing) => *existing = run,
            None => state.runs.runs.push(run),
        }
        drop(state);
        let _ = self.persist_runs();
        self.emit_registry_run(registry_run, false);
    }

    pub fn update_run(&self, run_id: &str, f: impl FnOnce(&mut CloudRun)) {
        let mut registry_update = None;
        let mut state = self.inner.state.write().unwrap();
        if let Some(run) = state.runs.runs.iter_mut().find(|r| r.id == run_id) {
            let before = run.clone();
            f(run);
            run.updated_at = Utc::now();
            let mut before_with_updated_at = before.clone();
            before_with_updated_at.updated_at = run.updated_at;
            if before_with_updated_at != *run {
                registry_update = Some((before, run.clone()));
            }
        }
        drop(state);
        let _ = self.persist_runs();
        if let Some((before, after)) = registry_update {
            self.emit_registry_run(after.clone(), cloud_run_progress_only(&before, &after));
        }
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
            "backups": self.backups(),
            "drafts": self.drafts(),
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

    // ── Kanban drafts ────────────────────────────────────────────────────

    pub fn drafts(&self) -> Vec<TaskDraft> {
        self.inner.state.read().unwrap().config.drafts.clone()
    }

    pub fn save_draft(&self, draft: TaskDraft) {
        let mut state = self.inner.state.write().unwrap();
        state.config.drafts.retain(|d| d.chat_id != draft.chat_id);
        state.config.drafts.push(draft);
        drop(state);
        let _ = self.persist_config();
    }

    /// Remove and return a draft (starting or discarding it).
    pub fn take_draft(&self, chat_id: &str) -> Option<TaskDraft> {
        let mut state = self.inner.state.write().unwrap();
        let ix = state
            .config
            .drafts
            .iter()
            .position(|d| d.chat_id == chat_id)?;
        let draft = state.config.drafts.remove(ix);
        drop(state);
        let _ = self.persist_config();
        Some(draft)
    }

    // ── per-chat workspace backups ───────────────────────────────────────

    pub fn backups(&self) -> Vec<WorkspaceBackup> {
        self.inner.state.read().unwrap().config.backups.clone()
    }

    /// The chat's newest snapshot whose file still exists on disk.
    pub fn backup_for_chat(&self, chat_id: &str) -> Option<WorkspaceBackup> {
        self.backups()
            .into_iter()
            .filter(|b| b.chat_id == chat_id && Path::new(&b.path).exists())
            .max_by_key(|b| b.taken_at)
    }

    /// Record (replace) the chat's snapshot row.
    pub fn record_backup(&self, backup: WorkspaceBackup) {
        let registry_backup = backup.clone();
        let mut state = self.inner.state.write().unwrap();
        state.config.backups.retain(|b| b.chat_id != backup.chat_id);
        state.config.backups.push(backup);
        drop(state);
        let _ = self.persist_config();
        self.emit_registry_write(RegistryWrite::Backup(registry_backup));
    }

    pub fn mark_backup_restored(&self, chat_id: &str, seat_id: &str) {
        let mut state = self.inner.state.write().unwrap();
        for backup in state
            .config
            .backups
            .iter_mut()
            .filter(|b| b.chat_id == chat_id)
        {
            backup.restored_to = Some(seat_id.to_string());
        }
        drop(state);
        let _ = self.persist_config();
    }

    /// Where a chat's snapshot lives on this computer.
    pub fn chat_backup_path(&self, chat_id: &str) -> PathBuf {
        let safe: String = chat_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        self.backups_dir().join(format!("chat-{safe}.tar.gz"))
    }

    /// Whether a chat has completed a turn on a seat before — the next turn
    /// continues the harness conversation instead of starting cold.
    pub fn harness_session_for_chat(&self, chat_id: &str, seat_id: &str) -> Option<String> {
        self.runs()
            .into_iter()
            .filter(|r| r.chat_id == chat_id && r.seat_id == seat_id)
            .filter_map(|r| r.harness_session.clone().map(|s| (r.started_at, s)))
            .max_by_key(|(at, _)| *at)
            .map(|(_, s)| s)
    }
}

pub(crate) fn provider_key_env(provider: ByokProvider) -> &'static str {
    match provider {
        ByokProvider::Anthropic => "ANTHROPIC_API_KEY",
        ByokProvider::OpenAi | ByokProvider::ChatgptSub => "OPENAI_API_KEY",
        ByokProvider::Xai => "XAI_API_KEY",
        ByokProvider::OpenRouter => "OPENROUTER_API_KEY",
        ByokProvider::DeepSeek => "DEEPSEEK_API_KEY",
        ByokProvider::Xiaomi => "XIAOMI_API_KEY",
        ByokProvider::DevinSub => "DEVIN_API_KEY",
        ByokProvider::Custom => "CUSTOM_API_KEY",
    }
}

pub(crate) fn provider_base_env(provider: ByokProvider) -> &'static str {
    match provider {
        ByokProvider::Anthropic => "ANTHROPIC_BASE_URL",
        ByokProvider::OpenAi | ByokProvider::ChatgptSub => "OPENAI_BASE_URL",
        ByokProvider::Xai => "XAI_BASE_URL",
        ByokProvider::OpenRouter => "OPENROUTER_BASE_URL",
        ByokProvider::DeepSeek => "DEEPSEEK_BASE_URL",
        ByokProvider::Xiaomi => "XIAOMI_BASE_URL",
        ByokProvider::DevinSub => "DEVIN_BASE_URL",
        ByokProvider::Custom => "CUSTOM_API_BASE_URL",
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
    let bin = command_bin(command);
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
    if seat.kind != Some(CloudSeatKind::Railway) {
        return Err("seat-level snapshots only cover Railway trials".into());
    }
    let path = store.backups_dir().join(format!("{}.tar.gz", seat.id));
    let http = reqwest::Client::new();
    snapshot_dir(&http, store, seat, &seat.workspace(), &path).await?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Workspace persistence — per-chat workspaces, local git checkpoints, and
// snapshots pulled down to this computer so a cloud workspace's progress
// outlives the seat it ran on.
// ---------------------------------------------------------------------------

/// Snapshot excludes: build output and caches that rebuild on demand.
const SNAPSHOT_EXCLUDES: &str = "--exclude=./node_modules --exclude=*/node_modules \
--exclude=./target --exclude=./dist --exclude=./.next --exclude=./.cache \
--exclude=./.venv --exclude=*/__pycache__ --exclude=*.log";
/// Largest snapshot pulled down (compressed).
const MAX_SNAPSHOT: usize = 512 * 1024 * 1024;
/// Boat snapshots ride exec stdout as base64 — keep them smaller.
const MAX_BOAT_SNAPSHOT_B64: usize = 96 * 1024 * 1024;

/// The workspace a repo-less cloud chat owns on its seat:
/// `<seat workspace>/chats/<chat>` — isolated per thread, so a checkpoint,
/// snapshot, or restore never touches another chat's files.
pub fn chat_workspace(base: &str, chat_id: &str) -> String {
    let short: String = chat_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(12)
        .collect();
    let short = if short.is_empty() {
        "chat".to_string()
    } else {
        short
    };
    format!("{}/chats/{short}", base.trim_end_matches('/'))
}

/// The workspace a repo-less automation reuses across fires:
/// `<seat workspace>/automations/<id>`.
pub fn automation_workspace(base: &str, automation_id: &str) -> String {
    let short: String = automation_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(40)
        .collect();
    format!("{}/automations/{short}", base.trim_end_matches('/'))
}

// ---------------------------------------------------------------------------
// SSH identity — Railway's trial (and a fresh VPS) need *some* key to offer.
// ---------------------------------------------------------------------------

/// Where Cosmos keeps its own key when the user has none (`<data>/ssh`).
static SSH_KEY_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
/// The resolved identity, once: `None` = the user's own default key/agent.
static SSH_IDENTITY: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

pub(crate) fn set_ssh_key_dir(data_dir: &Path) {
    let _ = SSH_KEY_DIR.set(data_dir.join("ssh"));
}

/// The user's own default private keys (what plain `ssh` offers).
fn default_ssh_keys() -> Vec<PathBuf> {
    let ssh = dirs_like_home().join(".ssh");
    [
        "id_ed25519",
        "id_ecdsa",
        "id_rsa",
        "id_ed25519_sk",
        "id_ecdsa_sk",
    ]
    .into_iter()
    .map(|name| ssh.join(name))
    .filter(|path| path.exists())
    .collect()
}

/// An identity for seats to use. A user with their own default key keeps
/// using it (Railway knows a trial by the key that opened it). Without one,
/// Cosmos mints `<data>/ssh/cosmos_ed25519` once — Railway's "run ssh-keygen
/// then retry", done for them — and offers it on every seat connection.
pub fn ssh_identity() -> Option<PathBuf> {
    SSH_IDENTITY
        .get_or_init(|| {
            if default_ssh_identity_is_usable() {
                return None;
            }
            let dir = SSH_KEY_DIR.get()?.clone();
            let key = dir.join("cosmos_ed25519");
            if key.exists() {
                return Some(key);
            }
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::warn!(error = %e, "could not create the Cosmos ssh dir");
                return None;
            }
            match mint_ssh_key(&key) {
                Ok(()) => {
                    tracing::info!(path = %key.display(), "created the Cosmos ssh key");
                    Some(key)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "could not create an ssh key");
                    None
                }
            }
        })
        .clone()
}

fn default_ssh_identity_is_usable() -> bool {
    if std::env::var_os("SSH_AUTH_SOCK").is_some() {
        let mut ssh_add = std::process::Command::new("ssh-add");
        ssh_add
            .arg("-L")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            ssh_add.creation_flags(0x08000000);
        }
        if ssh_add.status().is_ok_and(|status| status.success()) {
            return true;
        }
    }
    default_ssh_keys().into_iter().any(|key| {
        let mut keygen = std::process::Command::new("ssh-keygen");
        keygen
            .args(["-y", "-P", "", "-f"])
            .arg(key)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            keygen.creation_flags(0x08000000);
        }
        keygen.status().is_ok_and(|status| status.success())
    })
}

/// `ssh-keygen` an unencrypted ed25519 key at `key` (+ `key.pub`).
fn mint_ssh_key(key: &Path) -> Result<(), String> {
    let mut keygen = std::process::Command::new("ssh-keygen");
    keygen
        .args([
            "-q",
            "-t",
            "ed25519",
            "-N",
            "",
            "-C",
            "cosmos-desktop",
            "-f",
        ])
        .arg(key)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        keygen.creation_flags(0x08000000);
    }
    let out = keygen.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "ssh-keygen isn't installed (it ships with the OpenSSH client)".to_string()
        } else {
            format!("could not run ssh-keygen: {e}")
        }
    })?;
    if out.status.success() && key.exists() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// `ssh` with the options every seat connection shares: no prompts
/// (BatchMode), first-connect host trust, Cosmos's key when the user has
/// none, and no console window flashing up on Windows.
pub fn ssh() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("ssh");
    if let Some(key) = ssh_identity() {
        command.arg("-i").arg(key);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(0x08000000);
    }
    command
}

/// Spawn failures read as instructions, not `os error 2`.
pub fn ssh_spawn_error(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        if cfg!(windows) {
            "the OpenSSH client isn't installed — add it in Settings → System → Optional features → OpenSSH Client, then retry".into()
        } else {
            "ssh isn't installed on this computer — install the OpenSSH client, then retry".into()
        }
    } else {
        format!("could not run ssh: {e}")
    }
}

/// The interactive-terminal form (`ssh -tt …`) for the Shell pane's PTY.
pub fn ssh_terminal_prefix() -> String {
    match ssh_identity() {
        // cmd and PowerShell both take a double-quoted path; POSIX shells
        // get the single-quoted word.
        Some(key) if cfg!(windows) => format!("ssh -i \"{}\" -tt", key.display()),
        Some(key) => format!("ssh -i {} -tt", shell_word(&key.to_string_lossy())),
        None => "ssh -tt".into(),
    }
}

/// `(ssh alias, remote command)` for a raw-bytes exec on an ssh-reached
/// seat: Railway execs on the VM itself, a VPS seat inside its container
/// (`-i` so stdin reaches `tar x`).
fn ssh_seat_target(seat: &SeatConfig, command: &str) -> Result<(String, String), String> {
    match seat.kind {
        Some(CloudSeatKind::Railway) => {
            if seat.expired() {
                return Err("Railway trial seat expired".into());
            }
            Ok((
                seat.ssh_alias
                    .clone()
                    .unwrap_or_else(|| crate::railway::RAILWAY_HOST.into()),
                command.to_string(),
            ))
        }
        Some(CloudSeatKind::Ssh) => Ok((
            seat.ssh_alias
                .clone()
                .ok_or_else(|| "seat has no SSH alias".to_string())?,
            format!(
                "docker exec -i {} sh -lc {}",
                shell_word(&seat.container_name()),
                shell_word(command)
            ),
        )),
        _ => Err("not an ssh seat".into()),
    }
}

async fn ssh_raw(
    alias: &str,
    command: &str,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    let mut child = ssh()
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            "-o",
            "StrictHostKeyChecking=accept-new",
            alias,
            command,
        ])
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(ssh_spawn_error)?;
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        let bytes = bytes.to_vec();
        tokio::spawn(async move {
            let _ = stdin.write_all(&bytes).await;
            let _ = stdin.shutdown().await;
        });
    }
    tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| format!("ssh {alias} timed out"))?
        .map_err(ssh_spawn_error)
}

fn first_line(bytes: &[u8], fallback: &str) -> String {
    let text = crate::railway::redact_trial_banner(&String::from_utf8_lossy(bytes));
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// Pull `workspace` off the seat as a `.tar.gz` at `dest`; returns its size.
pub async fn snapshot_dir(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
    workspace: &str,
    dest: &Path,
) -> Result<u64, String> {
    let tar = format!(
        "cd {} 2>/dev/null || exit 3; tar czf - {SNAPSHOT_EXCLUDES} .",
        shell_word(workspace)
    );
    let bytes = match seat.kind {
        Some(CloudSeatKind::Railway | CloudSeatKind::Ssh) => {
            let (alias, remote) = ssh_seat_target(seat, &tar)?;
            let output = ssh_raw(&alias, &remote, None, Duration::from_secs(300)).await?;
            if output.status.code() == Some(3) {
                return Err("workspace doesn't exist on the seat yet".into());
            }
            if !output.status.success() {
                return Err(format!(
                    "snapshot failed: {}",
                    first_line(&output.stderr, "ssh error")
                ));
            }
            output.stdout
        }
        Some(CloudSeatKind::Box) => {
            let (Some(token), Some(box_id)) = (store.box_token(&seat.id), seat.box_id.clone())
            else {
                return Err("Boat seat has no box yet".into());
            };
            let out = crate::boat::boat(http, &token, seat.box_endpoint.as_deref())
                .exec(
                    &box_id,
                    &format!("{tar} | base64 | tr -d '\\n'"),
                    None,
                    Duration::from_secs(300),
                )
                .await?;
            if !out.success && out.stdout.trim().is_empty() {
                return Err(format!(
                    "snapshot failed: {}",
                    first_line(out.stderr.as_bytes(), "exec error")
                ));
            }
            let b64 = out.stdout.trim();
            if b64.len() > MAX_BOAT_SNAPSHOT_B64 {
                return Err("workspace too large to snapshot over Boat exec".into());
            }
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("snapshot decode: {e}"))?
        }
        _ => return Err("this seat can't snapshot a workspace".into()),
    };
    if bytes.len() < 20 || bytes[..2] != [0x1f, 0x8b] {
        return Err("snapshot was empty".into());
    }
    if bytes.len() > MAX_SNAPSHOT {
        return Err("workspace too large to snapshot (>512 MB compressed)".into());
    }
    let staging = dest.with_extension("partial");
    std::fs::write(&staging, &bytes).map_err(|e| format!("write snapshot: {e}"))?;
    std::fs::rename(&staging, dest).map_err(|e| format!("write snapshot: {e}"))?;
    Ok(bytes.len() as u64)
}

/// Unpack a local `.tar.gz` into `workspace` on the seat.
pub async fn restore_dir(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
    workspace: &str,
    src: &Path,
) -> Result<(), String> {
    let bytes = std::fs::read(src).map_err(|e| format!("read snapshot: {e}"))?;
    let ws = shell_word(workspace);
    match seat.kind {
        Some(CloudSeatKind::Railway | CloudSeatKind::Ssh) => {
            let (alias, remote) =
                ssh_seat_target(seat, &format!("mkdir -p {ws} && tar xzf - -C {ws}"))?;
            let output = ssh_raw(&alias, &remote, Some(&bytes), Duration::from_secs(300)).await?;
            if output.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "restore failed: {}",
                    first_line(&output.stderr, "tar error")
                ))
            }
        }
        Some(CloudSeatKind::Box) => {
            let (Some(token), Some(box_id)) = (store.box_token(&seat.id), seat.box_id.clone())
            else {
                return Err("Boat seat has no box yet".into());
            };
            use base64::Engine as _;
            let api = crate::boat::boat(http, &token, seat.box_endpoint.as_deref());
            let staged = format!("/tmp/cosmos-restore-{}.tgz.b64", crate::new_id());
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            api.write_file(&box_id, &staged, b64.as_bytes()).await?;
            let out = api
                .exec(
                    &box_id,
                    &format!(
                        "trap 'rm -f {staged}' EXIT; mkdir -p {ws} && base64 -d {staged} | tar xzf - -C {ws}"
                    ),
                    None,
                    Duration::from_secs(300),
                )
                .await?;
            if out.success {
                Ok(())
            } else {
                Err(format!(
                    "restore failed: {}",
                    first_line(out.stderr.as_bytes(), "tar error")
                ))
            }
        }
        _ => Err("this seat can't restore a workspace".into()),
    }
}

/// Prepare a repo-less workspace: create it, and start a local-only git
/// repo (with build output excluded) so the Changes pane can diff the
/// agent's work and each turn can be checkpointed.
pub fn workspace_prepare_command(workspace: &str) -> String {
    let ws = shell_word(workspace);
    format!(
        "mkdir -p {ws} && cd {ws} && if command -v git >/dev/null 2>&1 && [ ! -d .git ]; then \
git init -q && printf '%s\\n' node_modules/ target/ dist/ .next/ .cache/ .venv/ __pycache__/ '*.log' >> .git/info/exclude; fi; \
[ -n \"$(ls -A 2>/dev/null | grep -v '^\\.git$')\" ] && echo __cosmos_ws_has || echo __cosmos_ws_empty"
    )
}

/// Commit everything in a repo-less workspace as a checkpoint. Prints the
/// short hash (or nothing when there was nothing new / no git).
pub fn checkpoint_command(workspace: &str, message: &str) -> String {
    let subject: String = message
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("agent turn")
        .chars()
        .take(72)
        .collect();
    format!(
        "cd {} 2>/dev/null && command -v git >/dev/null 2>&1 && [ -d .git ] && git add -A >/dev/null 2>&1 && \
git -c user.name=Cosmos -c user.email=cosmos@localhost commit -q --no-verify -m {} >/dev/null 2>&1 && git rev-parse --short HEAD",
        shell_word(workspace),
        shell_word(&format!("Cosmos checkpoint: {subject}"))
    )
}

/// Snapshot a chat's workspace to this computer and record it.
pub async fn backup_chat_workspace(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
    chat_id: &str,
    workspace: &str,
) -> Result<WorkspaceBackup, String> {
    let path = store.chat_backup_path(chat_id);
    let bytes = snapshot_dir(http, store, seat, workspace, &path).await?;
    let backup = WorkspaceBackup {
        chat_id: chat_id.to_string(),
        seat_id: seat.id.clone(),
        workspace: workspace.to_string(),
        path: path.to_string_lossy().into_owned(),
        bytes,
        taken_at: Utc::now(),
        restored_to: Some(seat.id.clone()),
    };
    store.record_backup(backup.clone());
    Ok(backup)
}

/// Before a turn: make sure the chat's workspace exists on this seat, and if
/// it came up empty while this computer holds a snapshot of it (the trial
/// died, the box was destroyed, the chat moved seats) — restore it. Returns
/// a transcript note when something was restored or couldn't be.
pub async fn ensure_chat_workspace(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
    chat_id: &str,
    workspace: &str,
    repo_less: bool,
) -> Option<String> {
    let prepare = chat_workspace_prepare(workspace, repo_less);
    let empty = match seat_exec(http, store, seat, &prepare, Duration::from_secs(40)).await {
        Ok(out) => out.stdout.contains("__cosmos_ws_empty"),
        Err(e) => return Some(format!("Couldn't prepare the workspace on the seat: {e}")),
    };
    restore_chat_workspace_if_empty(http, store, seat, chat_id, workspace, empty).await
}

/// The shell line that creates a chat's workspace (git-initialising a
/// repo-less one) and prints `__cosmos_ws_empty` / `__cosmos_ws_has`.
pub fn chat_workspace_prepare(workspace: &str, repo_less: bool) -> String {
    if repo_less {
        workspace_prepare_command(workspace)
    } else {
        let ws = shell_word(workspace);
        format!(
            "mkdir -p {ws} && cd {ws} && {{ [ -n \"$(ls -A 2>/dev/null)\" ] && echo __cosmos_ws_has || echo __cosmos_ws_empty; }}"
        )
    }
}

/// Restore the chat's snapshot when its workspace came up `empty`.
pub async fn restore_chat_workspace_if_empty(
    http: &reqwest::Client,
    store: &CloudStore,
    seat: &SeatConfig,
    chat_id: &str,
    workspace: &str,
    empty: bool,
) -> Option<String> {
    if !empty {
        return None;
    }
    let backup = store.backup_for_chat(chat_id)?;
    match restore_dir(http, store, seat, workspace, Path::new(&backup.path)).await {
        Ok(()) => {
            store.mark_backup_restored(chat_id, &seat.id);
            Some(format!(
                "Restored this chat's workspace from the backup on your computer ({}, {}) — earlier work is back on the seat.",
                human_bytes(backup.bytes),
                backup.taken_at.format("%b %-d %H:%M UTC")
            ))
        }
        Err(e) => Some(format!(
            "This chat has a workspace backup on your computer, but restoring it failed ({e}) — the seat starts empty."
        )),
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Harness-native conversation continuity for seat runs: the flags that
/// start (first turn) or resume (later turns) the same conversation, spliced
/// into the seat's harness command. `session` is a stable UUID per chat.
/// Harnesses without a resumable id get `None` — the caller falls back to a
/// transcript digest in the prompt.
pub fn continuation_flags(command: &str, session: &str, resume: bool) -> Option<String> {
    let bin = command_bin(command);
    if bin.contains("claude") {
        return Some(if resume {
            format!("--resume {session}")
        } else {
            format!("--session-id {session}")
        });
    }
    if bin.contains("opencode") {
        return resume.then(|| "--continue".to_string());
    }
    if bin.contains("codex") {
        // `codex exec` → `codex exec resume --last` on later turns.
        return resume.then(|| "resume --last".to_string());
    }
    None
}

/// The program a seat command line runs: the first word after any
/// `KEY=value` environment prefixes, path and `.exe` stripped, lowercased.
pub fn command_bin(command: &str) -> String {
    command
        .split_whitespace()
        .find(|word| !word.contains('='))
        .unwrap_or("")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim_end_matches(".exe")
        .to_lowercase()
}

/// How one harness runs headless on a seat: its CLI line (the prompt is
/// appended, or fills `{prompt}`) and a best-effort install for seats that
/// don't have it yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatHarness {
    pub command: String,
    pub install: String,
}

/// Quote a model id for the command line; ids are catalog strings
/// (`claude-opus-…`, `openai/gpt-…`), but never trust them raw.
fn model_arg(flag: &str, model: Option<&str>) -> String {
    model
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| format!(" {flag} {}", shell_word(m)))
        .unwrap_or_default()
}

/// The composer's harness + model pick, as the CLI line that runs it on a
/// seat — the same agents and logins as on this computer, headless. Agents
/// without a scriptable one-shot mode say so instead of guessing.
pub fn seat_harness(
    harness: zeron_proto::HarnessId,
    model: Option<&str>,
    reasoning: Option<zeron_proto::ReasoningLevel>,
) -> Result<SeatHarness, String> {
    use zeron_proto::HarnessId as H;
    use zeron_proto::ReasoningLevel as R;
    let install = |bin: &str, how: &str| {
        format!("command -v {bin} >/dev/null 2>&1 || {{ {how} ; }} >/dev/null 2>&1 || true")
    };
    Ok(match harness {
        // Root seats (Railway's VM) refuse --dangerously-skip-permissions
        // unless the CLI is told it's in a sandbox — which a seat is.
        H::ClaudeCode => SeatHarness {
            command: format!(
                "IS_SANDBOX=1 claude -p --dangerously-skip-permissions --output-format stream-json --verbose{}",
                model_arg("--model", model)
            ),
            install: install(
                "claude",
                "npm i -g @anthropic-ai/claude-code || curl -fsSL https://claude.ai/install.sh | bash",
            ),
        },
        H::Codex => {
            let effort = match reasoning {
                Some(R::Minimal) => Some("minimal"),
                Some(R::Low) => Some("low"),
                Some(R::Medium) => Some("medium"),
                Some(R::High | R::Max | R::Ultra | R::Ultrathink) => Some("high"),
                Some(R::XHigh | R::Ultracode) => Some("xhigh"),
                None => None,
            };
            SeatHarness {
                command: format!(
                    "codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox{}{}",
                    model_arg("-m", model),
                    effort
                        .map(|e| format!(" -c model_reasoning_effort={e}"))
                        .unwrap_or_default()
                ),
                install: install("codex", "npm i -g @openai/codex"),
            }
        }
        H::Opencode => SeatHarness {
            command: format!("opencode run --format json{}", model_arg("-m", model)),
            install: install(
                "opencode",
                "npm i -g opencode-ai || curl -fsSL https://opencode.ai/install | bash",
            ),
        },
        H::Cursor => SeatHarness {
            command: format!(
                "cursor-agent -p --force --output-format text{}",
                model_arg("--model", model)
            ),
            install: install(
                "cursor-agent",
                "curl -fsS https://cursor.com/install | bash",
            ),
        },
        H::Devin => SeatHarness {
            command: format!(
                "devin -p --permission-mode dangerous --respect-workspace-trust false{} -- {{prompt}}",
                model_arg("--model", model)
            ),
            install: install("devin", "curl -fsSL https://cli.devin.ai/install.sh | bash"),
        },
        H::Grok => SeatHarness {
            command: "grok -p".into(),
            install: install(
                "grok",
                "curl -fsSL https://x.ai/cli/install.sh | bash || npm i -g @xai-official/grok",
            ),
        },
        H::KimiCode => SeatHarness {
            command: format!("kimi --print{} -p", model_arg("--model", model)),
            install: install(
                "kimi",
                "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash || npm i -g @moonshot-ai/kimi-code",
            ),
        },
        H::Amp => SeatHarness {
            command: "amp --dangerously-allow-all -x".into(),
            install: install("amp", "npm i -g @sourcegraph/amp"),
        },
        H::Hermes | H::Pi | H::Antigravity | H::Mock => {
            return Err(format!(
                "{harness:?} can't run on a cloud seat yet (it has no headless one-shot mode) — pick Claude Code, Codex, OpenCode, Cursor, Devin, Grok, Kimi or Amp for cloud runs"
            ));
        }
    })
}

/// Splice continuation flags into a harness command, just before the
/// `{prompt}` slot when there is one, else at the end (the prompt is
/// appended after).
pub fn with_continuation(command: &str, flags: Option<&str>) -> String {
    let Some(flags) = flags.filter(|f| !f.is_empty()) else {
        return command.to_string();
    };
    if command.contains("{prompt}") {
        command.replacen("{prompt}", &format!("{flags} {{prompt}}"), 1)
    } else {
        format!("{command} {flags}")
    }
}

/// A UUID-shaped harness session id derived from the chat id, stable across
/// turns and devices (`claude --session-id` requires a UUID).
pub fn harness_session_id(chat_id: &str) -> String {
    if let Ok(uuid) = uuid::Uuid::parse_str(chat_id) {
        return uuid.to_string();
    }
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(chat_id.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_random_bytes(bytes)
        .into_uuid()
        .to_string()
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

fn fold_railway_trial_failure(seat: &mut SeatConfig, trial: &crate::railway::RailwayTrial) {
    if trial.signup_required {
        seat.signup_url = trial.signup_url.clone();
    }
    if trial.poll_url.is_some() {
        seat.signup_poll_url = trial.poll_url.clone();
    }
    if trial.claim_required {
        let now = Utc::now();
        seat.claim_url = trial.claim_url.clone().or(seat.claim_url.clone());
        seat.expires_at = Some(
            seat.expires_at
                .map_or(now, |expires_at| expires_at.min(now)),
        );
    }
}

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
            match crate::railway::probe_with_existing_deadline(
                seat.ssh_alias
                    .as_deref()
                    .unwrap_or(crate::railway::RAILWAY_HOST),
                seat.expires_at.is_some(),
            )
            .await
            {
                Ok(trial) => {
                    probe.online = true;
                    probe.detail = trial.detail.or_else(|| Some("railway.new trial VM".into()));
                    // A fresh manifest can extend or clarify the deadline and
                    // claim link — fold them into the stored seat.
                    let mut updated = seat.clone();
                    updated.expires_at = trial.expires_at.or(updated.expires_at);
                    updated.claim_url = trial.claim_url.or(updated.claim_url);
                    updated.preview_url = trial.preview_url.or(updated.preview_url);
                    updated.signup_url = None;
                    updated.signup_poll_url = None;
                    if &updated != seat {
                        let _ = store.save_seat(updated);
                    }
                }
                Err(failure) => {
                    let mut updated = seat.clone();
                    fold_railway_trial_failure(&mut updated, &failure.trial);
                    if &updated != seat {
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
                            let _ = store.save_seat(seat);
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
                            let _ = store.save_seat(seat);
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
        ssh()
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
    .map_err(ssh_spawn_error)?;
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
    let mut child = ssh()
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
        .map_err(ssh_spawn_error)?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input)
            .await
            .map_err(|e| format!("could not write ssh stdin: {e}"))?;
    }
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| format!("ssh {alias} timed out"))?
        .map_err(ssh_spawn_error)?;
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
    let mut child = ssh()
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
        .map_err(ssh_spawn_error)?;
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

fn format_github_error(status: reqwest::StatusCode, message: &str) -> String {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        "GitHub connection expired — reconnect GitHub in Settings → Cloud computer".into()
    } else {
        format!("github {status}: {message}")
    }
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

/// One repo's branches for the cloud branch picker.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchPage {
    pub items: Vec<String>,
    pub default_branch: String,
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
        format_github_error(status, &message)
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

    /// `GET /repos/{repo}/branches` — the branch picker's rows (up to 500,
    /// `truncated` past that), the default branch first.
    pub async fn branches(&self, repo: &str) -> Result<BranchPage, String> {
        let info = self.repo(repo).await?;
        let mut items = Vec::new();
        let mut truncated = false;
        for page in 1..=5 {
            let resp = self
                .get(&format!("/repos/{repo}/branches?per_page=100&page={page}"))
                .send()
                .await
                .map_err(|e| format!("github: {e}"))?;
            if !resp.status().is_success() {
                return Err(Self::error(resp).await);
            }
            let v: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let rows = v.as_array().cloned().unwrap_or_default();
            let len = rows.len();
            items.extend(
                rows.iter()
                    .filter_map(|b| b["name"].as_str().map(str::to_string)),
            );
            if len < 100 {
                break;
            }
            if page == 5 {
                truncated = true;
            }
        }
        let default_branch = info.default_branch;
        if let Some(ix) = items.iter().position(|b| *b == default_branch) {
            let name = items.remove(ix);
            items.insert(0, name);
        } else if !default_branch.is_empty() {
            items.insert(0, default_branch.clone());
        }
        Ok(BranchPage {
            items,
            default_branch,
            truncated,
        })
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

/// What a new task (Kanban "+ New task", an automation fire) runs.
pub struct TaskSpec {
    pub title: String,
    pub prompt: String,
    /// Cloud seat; `None` = this computer.
    pub seat_id: Option<String>,
    /// Local project (and the project context for seat runs).
    pub space_id: Option<String>,
    pub harness: Option<zeron_proto::HarnessId>,
    /// Seat workspace to use instead of the per-chat default (automations
    /// keep one workspace across fires).
    pub seat_workspace: Option<String>,
}

/// Create the chat a task runs in — bound to its seat (repo-less seats get
/// the per-chat workspace) — and return `(chat_id, cwd)`.
pub fn create_task_chat(
    store: &CloudStore,
    workspace: &crate::workspace_host::WorkspaceHost,
    device_id: &str,
    spec: &TaskSpec,
) -> Result<(String, String), EngineError> {
    let chat_id = crate::new_id();
    let seat = match spec.seat_id.as_deref() {
        Some(id) => Some(
            store
                .seat(id)
                .ok_or_else(|| EngineError::Other(format!("no such seat: {id}")))?,
        ),
        None => None,
    };
    if let Some(seat) = &seat
        && seat.expired()
    {
        return Err(EngineError::Other(
            "that Railway trial has ended — pick a live seat".into(),
        ));
    }
    let binding = seat.as_ref().map(|seat| CloudSeatBinding {
        seat_id: seat.id.clone(),
        kind: seat.kind.unwrap_or(CloudSeatKind::Ssh),
        label: seat.label.clone(),
        repo: seat.repo.clone(),
        workspace_path: Some(match (&spec.seat_workspace, &seat.repo) {
            (Some(ws), _) => ws.clone(),
            (None, Some(_)) => seat.workspace(),
            (None, None) => chat_workspace(&seat.workspace(), &chat_id),
        }),
        runner_device_id: seat.runner_device_id.clone(),
        branch: None,
        model_label: None,
        expires_at: seat.expires_at,
    });
    let seat_cwd = binding.as_ref().and_then(|b| b.workspace_path.clone());
    workspace.create_chat_with_parent(
        &chat_id,
        spec.space_id.as_deref(),
        Some(device_id),
        None,
        seat_cwd.clone(),
        None,
        binding,
    )?;
    workspace
        .rename_chat(&chat_id, &spec.title)
        .map_err(|e| EngineError::Other(e.to_string()))?;
    let cwd = seat_cwd
        .or_else(|| {
            spec.space_id
                .as_deref()
                .and_then(|id| workspace.space(id).ok().flatten())
                .map(|space| space.path)
        })
        .unwrap_or_else(|| "~".to_string());
    Ok((chat_id, cwd))
}

/// Queue a task's first run (the host's normal drain dispatches it —
/// remotely for seat-bound chats).
pub fn queue_task_run(
    doc_host: &crate::doc_host::DocHost,
    chat_id: &str,
    prompt: &str,
    cwd: &str,
    harness: Option<zeron_proto::HarnessId>,
) -> Result<(), EngineError> {
    doc_host.queue_command(
        chat_id,
        zeron_doc::SessionCommandPayload::Run {
            request: zeron_proto::RunRequest {
                prompt: prompt.to_string(),
                harness,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                cwd: cwd.to_string(),
                sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
                auto_approve: true,
                resume: None,
                attachments: Vec::new(),
                worktree: None,
            },
            message_id: crate::new_id(),
        },
    )?;
    Ok(())
}

/// A task title from its prompt's first line.
pub fn task_title(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("New task");
    let mut chars = line.chars();
    let head: String = chars.by_ref().take(60).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

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
                // "Run now" wakes the loop early; otherwise the 30s cadence.
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                    _ = self.store.inner.run_now_signal.notified() => {}
                }
            }
        })
    }

    /// One scheduler pass: snapshot any Railway seat nearing its deadline,
    /// then claim due automations (and any "Run now" requests) and fire each.
    async fn tick(&self) -> Result<(), EngineError> {
        for automation in self.store.take_run_now() {
            if let Err(e) = self.fire(&automation).await {
                tracing::warn!(automation = %automation.id, error = %e, "automation run-now failed");
                self.store
                    .mark_automation(&automation.id, &format!("error:{e}"), None);
            }
        }
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
            if seat.runner_device_id.is_some()
                || seat.kind != Some(CloudSeatKind::Railway)
                || seat.backup_path.is_some()
            {
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
                    // Repo-less automations keep one workspace across fires
                    // (each fire is a fresh chat, the files carry over).
                    workspace_path: Some(if seat.repo.is_some() {
                        seat.workspace()
                    } else {
                        automation_workspace(&seat.workspace(), &automation.id)
                    }),
                    runner_device_id: seat.runner_device_id.clone(),
                    branch: None,
                    model_label: None,
                    expires_at: seat.expires_at,
                })
            }
            None => None,
        };

        // The run's chat — one per fire, titled `name · timestamp`.
        let title = format!(
            "{} · {}",
            automation.name,
            Utc::now().format("%b %-d %H:%M")
        );
        let (chat_id, cwd) = create_task_chat(
            &self.store,
            &self.workspace,
            &self.device_id,
            &TaskSpec {
                title,
                prompt: automation.prompt.clone(),
                seat_id: automation.seat_id.clone(),
                space_id: automation.space_id.clone(),
                harness: automation.harness,
                seat_workspace: binding.and_then(|b| b.workspace_path),
            },
        )?;
        queue_task_run(
            &self.doc_host,
            &chat_id,
            &automation.prompt,
            &cwd,
            automation.harness,
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
    fn github_unauthorized_error_prompts_reconnection() {
        assert_eq!(
            format_github_error(reqwest::StatusCode::UNAUTHORIZED, "Bad credentials"),
            "GitHub connection expired — reconnect GitHub in Settings → Cloud computer"
        );
        assert_eq!(
            format_github_error(reqwest::StatusCode::FORBIDDEN, "Forbidden"),
            "github 403 Forbidden: Forbidden"
        );
    }

    #[test]
    fn folds_railway_signup_and_claim_failures_into_the_seat() {
        let mut seat = SeatConfig::default();
        let signup = crate::railway::RailwayTrial {
            signup_required: true,
            signup_url: Some("https://railway.com/ssh-signup?code=signup".into()),
            poll_url: Some("https://backboard.railway.com/poll?code=signup".into()),
            ..Default::default()
        };
        fold_railway_trial_failure(&mut seat, &signup);
        assert_eq!(seat.signup_url, signup.signup_url);
        assert_eq!(seat.signup_poll_url, signup.poll_url);

        let later_deadline = Utc::now() + chrono::Duration::minutes(5);
        seat.expires_at = Some(later_deadline);
        let claim = crate::railway::RailwayTrial {
            claim_required: true,
            claim_url: Some("https://railway.com/ssh-signup?code=claim".into()),
            poll_url: Some("https://backboard.railway.com/poll?code=claim".into()),
            ..Default::default()
        };
        fold_railway_trial_failure(&mut seat, &claim);
        assert_eq!(seat.claim_url, claim.claim_url);
        assert_eq!(seat.signup_poll_url, claim.poll_url);
        assert!(seat.expires_at.unwrap() <= later_deadline);
    }

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
    fn chat_and_automation_workspaces_are_isolated_paths() {
        assert_eq!(
            chat_workspace("/app/", "3f2a9c1e-77aa-4b1b-9e0e-1234"),
            "/app/chats/3f2a9c1e77aa"
        );
        assert_eq!(chat_workspace("/app", "../.."), "/app/chats/chat");
        assert_eq!(
            automation_workspace("/w", "auto-9f; rm"),
            "/w/automations/auto-9frm"
        );
    }

    #[test]
    fn migrates_legacy_cloud_state_into_a_profile_store_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let store_root = dir.path().join("orgs/org/user");
        let backup = dir.path().join("seat-backups/seat/chat.tar");
        std::fs::create_dir_all(backup.parent().unwrap()).unwrap();
        std::fs::write(dir.path().join(CONFIG_FILE), b"{\"seats\":[]}").unwrap();
        std::fs::write(dir.path().join(KEYS_FILE), b"{\"key\":\"local\"}").unwrap();
        std::fs::write(dir.path().join(RUNS_FILE), b"{\"runs\":[]}").unwrap();
        std::fs::write(&backup, b"backup").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                dir.path().join(KEYS_FILE),
                std::fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }

        assert!(migrate_legacy_cloud_files(dir.path(), &store_root).unwrap());
        assert_eq!(
            std::fs::read(store_root.join(CONFIG_FILE)).unwrap(),
            b"{\"seats\":[]}"
        );
        assert_eq!(
            std::fs::read(store_root.join(KEYS_FILE)).unwrap(),
            b"{\"key\":\"local\"}"
        );
        assert_eq!(
            std::fs::read(store_root.join(RUNS_FILE)).unwrap(),
            b"{\"runs\":[]}"
        );
        assert_eq!(
            std::fs::read(store_root.join(BACKUPS_DIR).join("seat/chat.tar")).unwrap(),
            b"backup"
        );
        assert!(!dir.path().join(CONFIG_FILE).exists());
        assert!(!dir.path().join(KEYS_FILE).exists());
        assert!(!dir.path().join(RUNS_FILE).exists());
        assert!(!dir.path().join(BACKUPS_DIR).exists());
        assert!(!migrate_legacy_cloud_files(dir.path(), &store_root).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(store_root.join(KEYS_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn mints_an_ssh_key_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("cosmos_ed25519");
        if std::process::Command::new("ssh-keygen")
            .arg("-?")
            .output()
            .is_err()
        {
            return; // no OpenSSH on this machine
        }
        mint_ssh_key(&key).unwrap();
        assert!(key.exists());
        let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
        assert!(public.starts_with("ssh-ed25519 ") && public.contains("cosmos-desktop"));
    }

    #[test]
    fn picked_harness_runs_headless_on_the_seat() {
        use zeron_proto::{HarnessId, ReasoningLevel};
        let claude = seat_harness(HarnessId::ClaudeCode, Some("claude-opus-4-5"), None).unwrap();
        assert_eq!(
            claude.command,
            "IS_SANDBOX=1 claude -p --dangerously-skip-permissions --output-format stream-json --verbose --model 'claude-opus-4-5'"
        );
        assert_eq!(command_bin(&claude.command), "claude");
        assert!(claude.install.starts_with("command -v claude"));
        let codex =
            seat_harness(HarnessId::Codex, Some("gpt-5"), Some(ReasoningLevel::High)).unwrap();
        assert!(
            codex
                .command
                .ends_with("-m 'gpt-5' -c model_reasoning_effort=high")
        );
        let opencode =
            seat_harness(HarnessId::Opencode, Some("opencode/big-pickle"), None).unwrap();
        assert_eq!(
            opencode.command,
            "opencode run --format json -m 'opencode/big-pickle'"
        );
        let devin = seat_harness(HarnessId::Devin, Some("swe-2-max"), None).unwrap();
        assert!(devin.command.ends_with("--model 'swe-2-max' -- {prompt}"));
        // A hostile model id can't break out of its word.
        let evil = seat_harness(HarnessId::Opencode, Some("x'; rm -rf / #"), None).unwrap();
        assert_eq!(
            evil.command,
            "opencode run --format json -m 'x'\\''; rm -rf / #'"
        );
        assert!(seat_harness(HarnessId::Mock, None, None).is_err());
        assert_eq!(command_bin("FOO=1 /usr/bin/Codex.exe exec"), "codex");
        // The install prelude is valid sh.
        let ok = std::process::Command::new("sh")
            .arg("-n")
            .arg("-c")
            .arg(format!("{} ; true", claude.install))
            .status()
            .map(|s| s.success())
            .unwrap_or(true);
        assert!(ok);
    }

    #[test]
    fn continuation_flags_per_harness() {
        let sid = harness_session_id("not-a-uuid");
        assert!(uuid::Uuid::parse_str(&sid).is_ok());
        assert_eq!(sid, harness_session_id("not-a-uuid"));
        assert_eq!(
            continuation_flags("claude -p", &sid, false).unwrap(),
            format!("--session-id {sid}")
        );
        assert_eq!(
            continuation_flags("/usr/bin/claude -p", &sid, true).unwrap(),
            format!("--resume {sid}")
        );
        assert_eq!(continuation_flags("opencode run", &sid, false), None);
        assert_eq!(
            continuation_flags("opencode run -m x", &sid, true).as_deref(),
            Some("--continue")
        );
        assert_eq!(continuation_flags("aider --yes", &sid, true), None);
        assert_eq!(
            with_continuation("claude -p {prompt} --verbose", Some("--resume x")),
            "claude -p --resume x {prompt} --verbose"
        );
        assert_eq!(
            with_continuation("opencode run", Some("--continue")),
            "opencode run --continue"
        );
        assert_eq!(with_continuation("opencode run", None), "opencode run");
    }

    #[test]
    fn backups_round_trip_and_resume_tag() {
        let dir = tempfile::tempdir().unwrap();
        let store = CloudStore::load(dir.path());
        let path = store.chat_backup_path("chat/../1");
        assert!(path.ends_with("chat-chat1.tar.gz"));
        std::fs::write(&path, b"x").unwrap();
        store.record_backup(WorkspaceBackup {
            chat_id: "chat/../1".into(),
            seat_id: "s".into(),
            workspace: "/app/chats/c1".into(),
            path: path.to_string_lossy().into_owned(),
            bytes: 1,
            taken_at: Utc::now(),
            restored_to: None,
        });
        assert!(store.backup_for_chat("chat/../1").is_some());
        store.mark_backup_restored("chat/../1", "s2");
        let reloaded = CloudStore::load(dir.path());
        assert_eq!(
            reloaded
                .backup_for_chat("chat/../1")
                .unwrap()
                .restored_to
                .as_deref(),
            Some("s2")
        );
        assert_eq!(reloaded.harness_session_for_chat("c", "s"), None);
        let now = Utc::now();
        reloaded.upsert_run(CloudRun {
            id: "r1".into(),
            chat_id: "c".into(),
            seat_id: "s".into(),
            command: None,
            status: zeron_proto::CloudRunStatus::Done,
            log_path: None,
            remote_ref: None,
            mirrored_bytes: 3,
            remote_cursor: None,
            entry_id: None,
            error: None,
            workspace: None,
            harness: None,
            model: None,
            harness_session: Some("sid".into()),
            prompt: None,
            started_at: now,
            finished_at: Some(now),
            updated_at: now,
        });
        assert_eq!(
            reloaded.harness_session_for_chat("c", "s").as_deref(),
            Some("sid")
        );
        assert_eq!(reloaded.harness_session_for_chat("c", "other"), None);
    }

    #[test]
    fn registry_sink_strips_private_run_fields_and_coalesces_progress() {
        let dir = tempfile::tempdir().unwrap();
        let store = CloudStore::load(dir.path());
        let writes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = writes.clone();
        store.set_registry_sink(Arc::new(move |write| {
            if let RegistryWrite::Run(run) = write {
                observed.lock().unwrap().push(run);
            }
        }));

        let now = Utc::now();
        store.upsert_run(CloudRun {
            id: "run-1".into(),
            chat_id: "chat-1".into(),
            seat_id: "seat-1".into(),
            command: Some("claude --api-key private".into()),
            status: zeron_proto::CloudRunStatus::Working,
            log_path: Some("/home/seat/run.log".into()),
            remote_ref: Some("123".into()),
            mirrored_bytes: 0,
            remote_cursor: None,
            entry_id: Some("entry-1".into()),
            error: Some("private error".into()),
            workspace: Some("/workspace/chat-1".into()),
            harness: None,
            model: None,
            harness_session: None,
            prompt: Some("private prompt".into()),
            started_at: now,
            finished_at: None,
            updated_at: now,
        });
        let initial = writes.lock().unwrap()[0].clone();
        assert_eq!(initial.command, None);
        assert_eq!(initial.error, None);
        assert_eq!(initial.prompt, None);

        store.update_run("run-1", |run| run.mirrored_bytes = 24);
        store.update_run("run-1", |run| run.remote_cursor = Some("7".into()));
        assert_eq!(writes.lock().unwrap().len(), 1);

        store.update_run("run-1", |run| {
            run.status = zeron_proto::CloudRunStatus::Done;
            run.finished_at = Some(Utc::now());
        });
        let writes = writes.lock().unwrap();
        assert_eq!(writes.len(), 2);
        assert_eq!(writes[1].mirrored_bytes, 24);
        assert_eq!(writes[1].remote_cursor.as_deref(), Some("7"));
        assert_eq!(writes[1].status, zeron_proto::CloudRunStatus::Done);
    }

    #[test]
    fn unscheduled_automation_is_scheduled_not_fired() {
        let dir = tempfile::tempdir().unwrap();
        let store = CloudStore::load(dir.path());
        store.save_automation(CloudAutomation {
            id: "a".into(),
            name: "n".into(),
            prompt: "p".into(),
            schedule: "hourly".into(),
            seat_id: None,
            space_id: None,
            harness: None,
            enabled: true,
            last_run_at: None,
            next_run_at: None,
            last_status: None,
            last_chat_id: None,
        });
        let now = at(0);
        assert!(store.claim_due(now).is_empty());
        assert_eq!(
            store.automations()[0].next_run_at,
            Some(now + chrono::Duration::hours(1))
        );
    }

    #[test]
    fn run_now_queues_known_automations_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = CloudStore::load(dir.path());
        assert!(store.request_run_now("nope").is_err());
        store.save_automation(CloudAutomation {
            id: "a".into(),
            name: "n".into(),
            prompt: "p".into(),
            schedule: "daily".into(),
            seat_id: None,
            space_id: None,
            harness: None,
            enabled: false,
            last_run_at: None,
            next_run_at: None,
            last_status: None,
            last_chat_id: None,
        });
        store.request_run_now("a").unwrap();
        store.request_run_now("a").unwrap();
        let fired = store.take_run_now();
        assert_eq!(fired.len(), 1, "duplicate requests collapse");
        assert!(store.take_run_now().is_empty());
        assert!(store.automations()[0].last_run_at.is_some());
    }

    /// Workspace persistence over a real ssh link: prepare (git init) →
    /// write work → checkpoint → snapshot to this computer → wipe the seat
    /// → ensure_chat_workspace restores it. Opt-in: point
    /// `COSMOS_SSH_E2E_ALIAS` at an ssh alias whose host has git + tar.
    #[tokio::test]
    async fn repo_less_workspace_checkpoint_backup_restore_over_ssh() {
        let Some(alias) = std::env::var_os("COSMOS_SSH_E2E_ALIAS") else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let store = CloudStore::load(dir.path());
        let base = format!("/tmp/cosmos-e2e-{}", crate::new_id());
        let seat = SeatConfig {
            id: "seat-e2e".into(),
            kind: Some(CloudSeatKind::Railway),
            label: "e2e".into(),
            ssh_alias: Some(alias.to_string_lossy().into_owned()),
            workspace_path: Some(base.clone()),
            ..Default::default()
        };
        store.save_seat(seat.clone()).unwrap();
        let http = reqwest::Client::new();
        let ws = chat_workspace(&base, "chat-e2e-1");
        // Fresh: prepared + git-initialised, nothing to restore.
        assert!(
            ensure_chat_workspace(&http, &store, &seat, "chat-e2e-1", &ws, true)
                .await
                .is_none()
        );
        let run = |cmd: String| {
            let http = http.clone();
            let store = store.clone();
            let seat = seat.clone();
            async move {
                seat_exec(&http, &store, &seat, &cmd, Duration::from_secs(30))
                    .await
                    .unwrap()
            }
        };
        run(format!(
            "cd {} && echo 'print(1)' > app.py && mkdir -p node_modules && echo junk > node_modules/x",
            shell_word(&ws)
        ))
        .await;
        let commit = run(checkpoint_command(&ws, "build the app\nmore")).await;
        assert!(!commit.stdout.trim().is_empty(), "{}", commit.stderr);
        let log = run(format!("cd {} && git log --format=%s", shell_word(&ws))).await;
        assert_eq!(log.stdout.trim(), "Cosmos checkpoint: build the app");
        let tracked = run(format!("cd {} && git ls-files", shell_word(&ws))).await;
        assert_eq!(tracked.stdout.trim(), "app.py", "build dirs stay out");
        let backup = backup_chat_workspace(&http, &store, &seat, "chat-e2e-1", &ws)
            .await
            .unwrap();
        assert!(backup.bytes > 0);
        // The seat dies: its workspace is gone.
        run(format!("rm -rf {}", shell_word(&base))).await;
        let note = ensure_chat_workspace(&http, &store, &seat, "chat-e2e-1", &ws, true)
            .await
            .expect("restore note");
        assert!(note.starts_with("Restored"), "{note}");
        let back = run(format!(
            "cd {} && cat app.py && git log --format=%s && ls node_modules 2>&1 | head -1",
            shell_word(&ws)
        ))
        .await;
        assert!(back.stdout.contains("print(1)"), "{}", back.stdout);
        assert!(back.stdout.contains("Cosmos checkpoint: build the app"));
        assert!(!back.stdout.contains("\nx\n"), "node_modules excluded");

        // The desktop over the same link: the turn-start prep returns fast
        // (bring-up is detached — the ssh session must not hang on it), then
        // the pane's capture and an act round-trip.
        let started = std::time::Instant::now();
        let prep = format!(
            "{{ {} ; }} ; {}",
            crate::seat_desktop::bootstrap_command(),
            chat_workspace_prepare(&ws, true)
        );
        let out = run(prep).await;
        assert!(out.stdout.contains("__cosmos_desktop_ok"), "{}", out.stderr);
        assert!(out.stdout.contains("__cosmos_ws_has"));
        assert!(started.elapsed() < Duration::from_secs(20), "prep hung");
        let ready = run(crate::seat_desktop::start_command()).await;
        assert!(ready.stdout.contains("__status=ready"), "{}", ready.stderr);
        let screen = linux_screen(&http, &store, &seat).await.unwrap();
        assert_eq!(screen["width"], 1280, "{screen}");
        let act = crate::seat_desktop::SeatAction::Type { text: "hi".into() }
            .command()
            .unwrap();
        assert_eq!(run(act).await.code, 0);
        run(format!("rm -rf {}", shell_word(&base))).await;
    }

    #[test]
    fn repo_prelude_keeps_the_token_out_of_the_remote() {
        let prelude = repo_checkout_prelude("me/cosmos", None);
        assert!(prelude.contains("'https://github.com/me/cosmos.git'"));
        assert!(prelude.contains("$GITHUB_TOKEN"));
        assert!(!prelude.contains("x-access-token:$GITHUB_TOKEN@"));
        assert!(!prelude.contains("--branch"));
        let on_branch = repo_checkout_prelude("me/cosmos", Some("feat/x"));
        assert!(on_branch.contains("clone --quiet --branch 'feat/x' "));
    }

    #[test]
    fn repo_workspaces_split_by_branch() {
        assert_eq!(repo_workspace("/app/", "me/cosmos", None), "/app/cosmos");
        assert_eq!(
            repo_workspace("/app", "me/cosmos", Some(" ")),
            "/app/cosmos"
        );
        assert_eq!(
            repo_workspace("/app", "me/cosmos", Some("feat/new ui")),
            "/app/cosmos@feat-new-ui"
        );
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
        store
            .save_seat(SeatConfig {
                id: "vps-1".into(),
                kind: Some(CloudSeatKind::Ssh),
                label: "dev box".into(),
                ssh_alias: Some("devbox".into()),
                ..Default::default()
            })
            .unwrap();
        store
            .set_seat_token("vps-1", "githubPat", Some("pat-123".into()))
            .unwrap();
        store
            .save_provider(
                ProviderConfig {
                    provider: Some(ByokProvider::Anthropic),
                    model: Some("claude-opus-4-6".into()),
                    use_for_cloud: true,
                    ..Default::default()
                },
                Some("sk-ant-secret".into()),
            )
            .unwrap();
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
            harness: None,
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
pub(crate) const LINUX_SCREEN_SCRIPT: &str = r#"
d=""
if [ -e /tmp/.X11-unix/X99 ]; then d=":99"; else
for s in /tmp/.X11-unix/X*; do [ -e "$s" ] && d=":${s##*/X}" && break; done
fi
ds=$(cat "$HOME/.cosmos/desktop.status" 2>/dev/null)
if [ -n "$d" ]; then
  f="/tmp/cosmos-screen-$$.png"
  if [ -x "$HOME/.cosmos/bin/cosmos-computer" ]; then DISPLAY=$d "$HOME/.cosmos/bin/cosmos-computer" screenshot "$f" >/dev/null 2>&1
  elif command -v import >/dev/null 2>&1; then DISPLAY=$d import -window root "$f" >/dev/null 2>&1
  elif command -v scrot >/dev/null 2>&1; then DISPLAY=$d scrot -o "$f" >/dev/null 2>&1
  fi
  if [ -s "$f" ]; then echo "__PNG__"; base64 "$f" | tr -d '\n'; echo; echo "__DISPLAY__ $d"; rm -f "$f"; exit 0; fi
  rm -f "$f"
fi
echo "__SYS__"
echo "desktop=${ds}"
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

pub(crate) fn parse_linux_screen(stdout: &str) -> Option<serde_json::Value> {
    let mut lines = stdout.lines().skip_while(|l| {
        let l = l.trim();
        l != "__PNG__" && l != "__SYS__"
    });
    match lines.next()?.trim() {
        "__PNG__" => {
            let png = lines.next()?.trim();
            if png.is_empty() {
                return None;
            }
            // The first 32 base64 chars decode the PNG signature + IHDR —
            // enough for the pixel size "Take control" maps clicks onto.
            let size = {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(&png[..png.len().min(32)])
                    .ok()
                    .and_then(|head| crate::seat_desktop::png_size(&head))
            };
            let display =
                lines.find_map(|l| l.trim().strip_prefix("__DISPLAY__ ").map(str::to_string));
            Some(serde_json::json!({
                "pngBase64": png,
                "width": size.map(|s| s.0),
                "height": size.map(|s| s.1),
                "display": display,
            }))
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
