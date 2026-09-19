//! Cosmos Computer — desktop perception and control for every harness.
//!
//! The user's own harness/model stays the reasoning layer; a locally installed
//! open-source Cua Driver (`cua-driver mcp`, MIT-licensed) supplies semantic
//! (accessibility-tree) and visual (screenshot) observation plus input actions
//! on Windows, macOS and Linux. The driver is an OPTIONAL component: it is
//! downloaded, checksum-verified and stored under the Cosmos data dir — never
//! linked into the Cosmos binary and never required to start the app.
//!
//! Layout mirrors the embedded-browser bridge: an engine-owned loopback MCP
//! endpoint (token in the path) is stamped onto local run requests, so any
//! harness can reach the computer the same way it reaches Cosmos tabs — over
//! plain JSON-RPC POSTs or through the `cosmos computer-stdio` MCP shim.
//!
//! Policy funnels through [`service::ComputerService`]: observe-only vs
//! background vs full-control, and the explicit foreground-escalation gate —
//! the same rules the Settings page shows. The Cua driver additionally runs
//! under `CUA_DRIVER_PERMISSION_MODE` matching the Cosmos control mode
//! (observe → `bounded` + a generated observe-only manifest; control modes →
//! `standard`), so a harness cannot reach around Cosmos policy by crafting
//! raw driver calls — it only ever sees the Cosmos tool surface anyway.

pub mod backend;
pub mod cua;
pub mod digest;
pub mod endpoint;
pub mod install;
pub mod mcp_client;
pub mod mock;
pub mod service;
pub mod stdio;
pub mod surface;

pub use service::{ComputerService, Probe};

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failure classes the UI and the MCP surface can act on. `retryable` marks
/// transient states (driver still warming up, install still running) where a
/// harness may re-issue the same call shortly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComputerError {
    /// Screen Context is switched off in Settings → Computer.
    Disabled,
    /// The optional Cua Driver component is not installed on this device.
    NotInstalled,
    /// A partial/interrupted install was found (rerun install to repair).
    Incomplete,
    /// This OS/arch has no published driver build.
    UnsupportedPlatform,
    /// The driver failed to start or answer.
    Unavailable(String),
    /// An OS permission is missing (Accessibility / Screen Recording / a
    /// desktop session on Linux). `hint` names the user-visible fix.
    Permission(String),
    /// An install step failed (download/verify/extract) — `stage` says which.
    Install { stage: String, message: String },
    /// The driver refused the action (foreground escalation without Full
    /// Control, background delivery genuinely unavailable, stale element…).
    Refused { code: String, message: String },
    /// The tool's arguments were invalid.
    InvalidArgs(String),
    /// Internal/unexpected.
    Internal(String),
}

impl ComputerError {
    pub fn refused(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Refused {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Stable machine-readable code for MCP/RPC payloads. The driver's own
    /// refusal vocabulary (`background_unavailable`, `stale_element`…) is
    /// preserved verbatim.
    pub fn code(&self) -> &str {
        match self {
            Self::Disabled => "disabled",
            Self::NotInstalled => "component_not_installed",
            Self::Incomplete => "component_incomplete",
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::Unavailable(_) => "driver_unavailable",
            Self::Permission(_) => "permission_required",
            Self::Install { .. } => "install_failed",
            Self::Refused { code, .. } => {
                if code.is_empty() {
                    "refused"
                } else {
                    code.as_str()
                }
            }
            Self::InvalidArgs(_) => "invalid_args",
            Self::Internal(_) => "internal",
        }
    }

    /// One-line, user-safe explanation (no paths to secrets, no raw stderr
    /// dumps beyond the first line).
    pub fn message(&self) -> String {
        match self {
            Self::Disabled => "Cosmos Computer is disabled. Enable Screen Context in Settings → Computer.".into(),
            Self::NotInstalled => {
                "The Computer component is not installed on this device. Install it from Settings → Computer.".into()
            }
            Self::Incomplete => {
                "The Computer component install is incomplete. Reinstall it from Settings → Computer.".into()
            }
            Self::UnsupportedPlatform => {
                "Cosmos Computer has no driver build for this OS/architecture.".into()
            }
            Self::Unavailable(m) => format!("The Computer driver is unavailable: {m}"),
            Self::Permission(hint) => format!("An OS permission is missing: {hint}"),
            Self::Install { stage, message } => {
                format!("Computer component install failed during {stage}: {message}")
            }
            Self::Refused { message, .. } => message.clone(),
            Self::InvalidArgs(m) => format!("Invalid tool arguments: {m}"),
            Self::Internal(m) => format!("Cosmos Computer error: {m}"),
        }
    }

    /// Whether a caller may reasonably retry the same call shortly.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::Internal(_))
    }

    /// Map an MCP tool failure payload (`isError: true` text) into a
    /// classified error, preserving the driver's structured refusal codes
    /// (`background_unavailable`, `foreground_required`, `stale_element`…).
    pub fn from_tool_error(text: &str) -> Self {
        let lower = text.to_ascii_lowercase();
        let code = [
            "background_unavailable",
            "foreground_required",
            "stale_element",
            "stale_snapshot",
            "element_not_found",
            "window_not_found",
            "app_not_found",
            "permission_denied",
            "screen_recording_denied",
            "accessibility_denied",
            "timeout",
            "unsupported",
            "refused",
        ]
        .into_iter()
        .find(|needle| lower.contains(needle))
        .unwrap_or("refused");
        if code == "permission_denied"
            || code == "screen_recording_denied"
            || code == "accessibility_denied"
        {
            return Self::Permission(text.to_string());
        }
        Self::refused(code, first_line(text))
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or(text).trim();
    line.chars().take(300).collect()
}

impl std::fmt::Display for ComputerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for ComputerError {}

// ---------------------------------------------------------------------------
// Settings (per-device, JSON under the engine data dir)
// ---------------------------------------------------------------------------

pub const COMPUTER_SETTINGS_FILENAME: &str = "computer-settings.json";

/// Who may act, and how far. Order matters: capabilities strictly grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ControlMode {
    /// Inspect only — no input actions at all.
    #[default]
    Observe,
    /// Actions allowed through background delivery only. The physical cursor
    /// and OS focus are never touched; a driver `background_unavailable`
    /// refusal surfaces to the harness as a structured error.
    Background,
    /// Actions may escalate to `delivery_mode:"foreground"` after a
    /// background refusal — a brief focus swap the driver restores
    /// afterwards. Requires the user's explicit opt-in.
    FullControl,
}

impl ControlMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Observe => "Observe only",
            Self::Background => "Background control",
            Self::FullControl => "Full control",
        }
    }

    pub fn acts(self) -> bool {
        self != Self::Observe
    }
}

/// Everything the user controls from Settings → Computer. Persisted
/// per-device so remote devices keep their own posture; never synced into
/// chat docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ComputerSettings {
    /// Master switch — when off, runs are not stamped and every RPC/MCP call
    /// fails `disabled`.
    pub enabled: bool,
    /// Observe returns semantic accessibility/native-UI context.
    pub semantic: bool,
    /// Observe may capture PNG screenshots (macOS needs Screen Recording;
    /// Wayland needs the desktop portal).
    pub screenshots: bool,
    /// Action ceiling.
    pub control: ControlMode,
    /// Cua Driver product telemetry (PostHog EU, content-free). Cosmos keeps
    /// it OPT-IN: the driver's own default is enabled, so Cosmos explicitly
    /// passes the env override either way.
    pub telemetry: bool,
    /// Cap on accessibility elements walked per window snapshot. Bounds both
    /// driver work and model context (Electron trees exceed 10k nodes).
    pub inspect_max_elements: u32,
    /// Cap on the returned screenshot's long edge in pixels.
    pub screenshot_max_dimension: u32,
}

impl Default for ComputerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            semantic: true,
            screenshots: false,
            control: ControlMode::Observe,
            telemetry: false,
            inspect_max_elements: 3_000,
            screenshot_max_dimension: 1440,
        }
    }
}

impl ComputerSettings {
    /// The Cua permission profile for this posture (fixed at driver launch;
    /// a settings change respawns the driver).
    pub fn permission_mode(&self) -> PermissionMode {
        match self.control {
            ControlMode::Observe => PermissionMode::Bounded,
            _ => PermissionMode::Standard,
        }
    }

    /// Environment handed to `cua-driver mcp`. Telemetry + update-check are
    /// always set explicitly so the user's saved driver prefs can't override
    /// the Cosmos posture; the update check is always off — Cosmos manages
    /// the component itself.
    pub fn driver_env(&self, manifest_path: Option<&Path>) -> Vec<(String, String)> {
        let mut env = vec![
            (
                "CUA_DRIVER_PERMISSION_MODE".into(),
                self.permission_mode().as_str().into(),
            ),
            (
                "CUA_DRIVER_RS_TELEMETRY_ENABLED".into(),
                if self.telemetry { "true" } else { "false" }.into(),
            ),
            ("CUA_DRIVER_RS_UPDATE_CHECK".into(), "false".into()),
        ];
        if let Some(manifest) = manifest_path {
            env.push((
                "CUA_DRIVER_CAPABILITY_MANIFEST_FILE".into(),
                manifest.to_string_lossy().into_owned(),
            ));
            env.push(("CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED".into(), "1".into()));
        }
        env
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    Standard,
    Bounded,
    Unrestricted,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Bounded => "bounded",
            Self::Unrestricted => "unrestricted",
        }
    }
}

/// The observe-only capability manifest written next to the driver when the
/// Cosmos control mode is `observe`: the driver itself refuses every action
/// tool, so even a direct `tools/call` bypass attempt (impossible — harnesses
/// only see the Cosmos surface) or a Cosmos bug degrades to deny-by-default.
pub const OBSERVE_MANIFEST_YAML: &str = r#"schema_version: 1
kind: cosmos-observe-only
capabilities:
  tools:
    allow:
      - list_apps
      - list_windows
      - get_window_state
      - get_accessibility_tree
      - get_desktop_state
      - get_screen_size
      - get_cursor_position
      - get_config
      - get_agent_cursor_state
      - get_recording_state
      - get_history_status
      - history_status
"#;

/// Settings file location under the engine data dir.
pub fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join(COMPUTER_SETTINGS_FILENAME)
}

pub fn load_settings(data_dir: &Path) -> ComputerSettings {
    let path = settings_path(data_dir);
    let Ok(raw) = std::fs::read(&path) else {
        return ComputerSettings::default();
    };
    serde_json::from_slice(&raw).unwrap_or_else(|err| {
        tracing::warn!(error = %err, path = %path.display(), "corrupt computer settings; using defaults");
        ComputerSettings::default()
    })
}

pub fn save_settings(data_dir: &Path, settings: &ComputerSettings) -> Result<(), ComputerError> {
    let path = settings_path(data_dir);
    let body =
        serde_json::to_vec_pretty(settings).map_err(|e| ComputerError::Internal(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &body).map_err(|e| ComputerError::Internal(e.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|e| ComputerError::Internal(e.to_string()))
}

// ---------------------------------------------------------------------------
// Release manifest — the pinned optional component
// ---------------------------------------------------------------------------

/// The Cua Driver release Cosmos installs. Pinning (not "latest") keeps the
/// checksum meaningful: a silent upstream re-publish fails verification
/// instead of sliding in new bytes.
pub const CUA_REPO: &str = "trycua/cua";
pub const CUA_VERSION: &str = "0.28.2";
pub const CUA_TAG: &str = "cua-driver-rs-v0.28.2";

#[derive(Debug, Clone, Copy)]
pub struct ReleaseAsset {
    /// Release-asset file name under the pinned tag.
    pub name: &'static str,
    /// Expected SHA-256 of the downloaded archive (from the release's
    /// `checksums.txt`, captured at pin time).
    pub sha256: &'static str,
    /// Compressed download size — disclosed to the user before install.
    pub size_bytes: u64,
}

impl ReleaseAsset {
    pub fn url(&self) -> String {
        format!(
            "https://github.com/{CUA_REPO}/releases/download/{CUA_TAG}/{}",
            self.name
        )
    }
}

/// The asset for the CURRENT OS/arch, or None where upstream ships no build.
/// macOS uses the full `darwin-universal` archive: it alone contains
/// `CuaDriver.app`, whose stable bundle identity TCC requires for
/// Accessibility / Screen Recording attribution (a bare binary has no
/// production-grade identity there — see the driver's EMBEDDING doc).
pub fn release_asset() -> Option<ReleaseAsset> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some(ReleaseAsset {
            name: "cua-driver-rs-0.28.2-windows-x86_64-binary.zip",
            sha256: "1f4bfceeab64cb7f56be7aad774c3dc2d2910d1427e4be1d79939c706e8029ba",
            size_bytes: 29_085_823,
        })
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        Some(ReleaseAsset {
            name: "cua-driver-rs-0.28.2-windows-arm64-binary.zip",
            sha256: "578b88ff2dd56f06eb7e984d73aaf5e76f59c6fde9542c967d6a30d00213c680",
            size_bytes: 27_394_922,
        })
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(ReleaseAsset {
            name: "cua-driver-rs-0.28.2-linux-x86_64-binary.tar.gz",
            sha256: "a1d99fd04bb4927ef5ffdbe60eb91ed8b51a2bab60e10fc604a75bd59ce69c3e",
            size_bytes: 30_603_028,
        })
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(ReleaseAsset {
            name: "cua-driver-rs-0.28.2-linux-arm64-binary.tar.gz",
            sha256: "55e8a32839a4ac369a773df4dac87b345bd4567779221ade4a5e39223a45a2e8",
            size_bytes: 30_639_613,
        })
    } else if cfg!(target_os = "macos") {
        // Universal archive: one download covers Apple Silicon and Intel.
        Some(ReleaseAsset {
            name: "cua-driver-rs-0.28.2-darwin-universal.tar.gz",
            sha256: "e273181b26709c88b1d809474deb3c592b4efae3530b11d76318f1887fc3fbb1",
            size_bytes: 70_078_129,
        })
    } else {
        None
    }
}

/// Platform label shown in diagnostics.
pub fn platform_label() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        std::env::consts::OS
    }
}

/// Linux display-server detail for diagnostics/limitations.
pub fn linux_session_label() -> Option<&'static str> {
    #[cfg(target_os = "linux")]
    {
        match std::env::var("XDG_SESSION_TYPE").as_deref() {
            Ok("wayland") => Some("wayland"),
            Ok("x11") => Some("x11"),
            _ => Some(if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                "wayland"
            } else {
                "x11"
            }),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

// ---------------------------------------------------------------------------
// Install layout under the Cosmos data dir
// ---------------------------------------------------------------------------

/// `install-state.json` — what a completed install claims to be. Verification
/// re-hashes the archive hash recorded here against the live binary? No — the
/// archive is not retained; verify checks the marker matches the on-disk
/// layout (binary present, version matches, sidecar files on Windows/macOS).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallState {
    pub version: String,
    pub tag: String,
    pub asset: String,
    pub sha256: String,
    /// The directory the driver was laid down in.
    pub install_dir: PathBuf,
    pub installed_at: chrono::DateTime<chrono::Utc>,
}

/// Versioned component root: `{data_dir}/computer/cua-driver/`.
pub struct ComputerPaths {
    pub root: PathBuf,
}

impl ComputerPaths {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("computer").join("cua-driver"),
        }
    }

    /// `{root}/versions` — one dir per installed release+target.
    pub fn versions_dir(&self) -> PathBuf {
        self.root.join("versions")
    }

    /// The dir a given version installs into, e.g.
    /// `versions/0.28.2-x86_64-pc-windows-msvc`.
    pub fn install_dir(&self, version: &str) -> PathBuf {
        self.versions_dir()
            .join(format!("{version}-{}", target_triple()))
    }

    pub fn state_file(&self) -> PathBuf {
        self.root.join("install-state.json")
    }

    /// `{root}/manifest-observe.yaml` — the generated bounded-mode manifest.
    pub fn observe_manifest(&self) -> PathBuf {
        self.root.join("manifest-observe.yaml")
    }

    /// The driver executable inside an install dir. On macOS the bundle's
    /// embedded binary wins (stable TCC identity `CuaDriver.app`); the bare
    /// sibling binary is the fallback.
    pub fn binary_in(install_dir: &Path) -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            let bundled = install_dir.join("CuaDriver.app/Contents/MacOS/cua-driver");
            if bundled.is_file() {
                return Some(bundled);
            }
        }
        let name = if cfg!(windows) {
            "cua-driver.exe"
        } else {
            "cua-driver"
        };
        let direct = install_dir.join(name);
        direct.is_file().then_some(direct)
    }

    /// Current install, if the marker and on-disk layout agree. A dir whose
    /// marker exists but whose binary is missing is reported as Incomplete so
    /// the UI offers reinstall rather than a confusing Unavailable later.
    pub fn installed(&self) -> Result<Option<InstallState>, ComputerError> {
        let raw = match std::fs::read(self.state_file()) {
            Ok(raw) => raw,
            Err(_) => return Ok(None),
        };
        let state: InstallState = serde_json::from_slice(&raw)
            .map_err(|e| ComputerError::Internal(format!("corrupt install-state.json: {e}")))?;
        Ok(Some(state))
    }

    pub fn load_state_lenient(&self) -> Option<InstallState> {
        self.installed().ok().flatten()
    }
}

/// Rust target triple of this build, used to name per-version install dirs.
pub fn target_triple() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "aarch64-pc-windows-msvc"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
    )))]
    {
        "unknown"
    }
}

/// Where install drops the `install-state.json` — written only after the
/// extracted layout checks out.
pub fn write_install_state(
    paths: &ComputerPaths,
    state: &InstallState,
) -> Result<(), ComputerError> {
    std::fs::create_dir_all(&paths.root).map_err(|e| ComputerError::Internal(e.to_string()))?;
    let body =
        serde_json::to_vec_pretty(state).map_err(|e| ComputerError::Internal(e.to_string()))?;
    let tmp = paths.root.join(".install-state.tmp");
    std::fs::write(&tmp, &body).map_err(|e| ComputerError::Internal(e.to_string()))?;
    std::fs::rename(&tmp, paths.state_file()).map_err(|e| ComputerError::Internal(e.to_string()))
}

// ---------------------------------------------------------------------------
// Status — the shape `ComputerStatus`/`computer_status` returns
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusCapabilities {
    /// Driver can serve semantic (accessibility) context.
    pub semantic: bool,
    /// Driver can capture screenshots (OS permission dependent on macOS).
    pub screenshots: bool,
    /// Background input delivery (never steals focus).
    pub background: bool,
    /// Foreground escalation exists AND the user enabled Full Control.
    pub foreground: bool,
    /// Wayland compositors often refuse background input; callers must treat
    /// `background_unavailable` as expected, not a driver crash, and only
    /// escalate to foreground under Full Control.
    #[serde(default)]
    pub wayland_background_limited: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusPermissions {
    /// Screen capture / Screen Recording.
    pub screen: String,
    /// Accessibility control (macOS TCC; elsewhere implicit).
    pub accessibility: String,
    /// Free-text platform caveat (Wayland portal limits etc).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The full status surface — Settings page + diagnostics + `computer_status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerStatus {
    /// `disabled | missing | incomplete | unsupported | starting | ready | error`
    pub state: String,
    pub platform: String,
    pub arch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_dir: Option<String>,
    /// Download size of this platform's component — always disclosed.
    pub component_download_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_installed_bytes: Option<u64>,
    pub settings: ComputerSettingsEcho,
    pub capabilities: StatusCapabilities,
    pub permissions: StatusPermissions,
    /// Last driver error (first line only — never raw payloads).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub checked_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerSettingsEcho {
    pub enabled: bool,
    pub semantic: bool,
    pub screenshots: bool,
    pub control: ControlMode,
    pub telemetry: bool,
    pub inspect_max_elements: u32,
    pub screenshot_max_dimension: u32,
    pub permission_mode: String,
}

// ---------------------------------------------------------------------------
// Observation — what `computer_observe` returns
// ---------------------------------------------------------------------------

/// The window an observation describes (frontmost for `scope:"frontmost"`,
/// explicit for `scope:"window"`, absent for a pure desktop read).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedWindow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_id: Option<i64>,
}

/// One semantic observation. `text` is the accessibility/markdown rendering —
/// explicitly untrusted observed content; the harness treats it as data, not
/// instructions. `screenshot` is present only when the caller asked for it
/// and settings allow.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    /// Which granularity produced this observation.
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<ObservedWindow>,
    /// Semantic desktop/window content (apps+windows lists, UIA tree text).
    pub text: String,
    /// True when the driver truncated the tree at `inspect_max_elements`.
    pub truncated: bool,
    /// Staged PNG (engine-side absolute path under the uploads jail) — only
    /// when `includeScreenshot` was requested AND settings.screenshots is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<StagedScreenshot>,
    /// Content modality for the UI chip: "semantic" | "visual" | "both".
    pub modality: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagedScreenshot {
    /// Absolute path on the HOST device (under the uploads dir so the UI can
    /// read it through the attachment-chunk path jail).
    pub path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub bytes: u64,
}
