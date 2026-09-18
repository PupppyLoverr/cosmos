//! Cosmos Tools — one gateway that gives every harness real-world tools.
//!
//! The user's own harness/model stays the reasoning layer; configured
//! upstream MCP servers supply the tool surface. Two upstream shapes:
//! **Composio Connect** (`https://connect.composio.dev/mcp` +
//! `x-consumer-api-key`, whose COMPOSIO_* meta-tools reach Gmail/Notion/Slack and the
//! rest of the catalog) and **arbitrary MCP servers** — stdio children or
//! remote streamable-HTTP endpoints with static headers.
//!
//! Layout mirrors the embedded-browser bridge and Cosmos Computer: an
//! engine-owned loopback MCP endpoint (128-bit token in the path) is stamped
//! onto local run requests, so any harness reaches the merged tool surface
//! the same way it reaches Cosmos tabs — over plain JSON-RPC POSTs or
//! through the `cosmos tools-stdio` MCP shim.
//!
//! Secrets (the Composio key, custom-server headers and env) live ONLY in
//! the device-local `tools.json` and inside this process — harnesses see the
//! loopback URL and nothing else. Wire names are `<provider>__<tool>` so
//! upstream namespaces can never collide.

pub mod composio;
pub mod endpoint;
pub mod mock;
pub mod service;
pub mod stdio;
pub mod upstream;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use service::ToolService;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failure classes the UI and the MCP surface can act on. `retryable` marks
/// transient states (upstream still warming up, a dropped child not yet
/// respawned) where a harness may re-issue the same call shortly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolError {
    /// The provider (or the whole gateway) is switched off.
    Disabled,
    /// An upstream failed to start, connect, or answer.
    Unavailable(String),
    /// The upstream refused the call (`isError: true` payloads); the
    /// upstream's own error text is preserved verbatim.
    Refused { code: String, message: String },
    /// The provider id, tool name, or arguments were invalid.
    InvalidArgs(String),
    /// The Composio key failed validation (no meta-tools came back).
    InvalidKey(String),
    /// Internal/unexpected.
    Internal(String),
}

impl ToolError {
    pub fn refused(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Refused {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Stable machine-readable code for MCP/RPC payloads.
    pub fn code(&self) -> &str {
        match self {
            Self::Disabled => "disabled",
            Self::Unavailable(_) => "upstream_unavailable",
            Self::Refused { code, .. } => {
                if code.is_empty() {
                    "refused"
                } else {
                    code.as_str()
                }
            }
            Self::InvalidArgs(_) => "invalid_args",
            Self::InvalidKey(_) => "invalid_key",
            Self::Internal(_) => "internal",
        }
    }

    /// One-line, user-safe explanation (no raw stderr dumps beyond the first
    /// line, no secret echo).
    pub fn message(&self) -> String {
        match self {
            Self::Disabled => {
                "This tool provider is disabled. Enable it in Settings → Tools.".into()
            }
            Self::Unavailable(m) => format!("The tool server is unavailable: {m}"),
            Self::Refused { message, .. } => message.clone(),
            Self::InvalidArgs(m) => format!("Invalid tool arguments: {m}"),
            Self::InvalidKey(m) => format!("The Composio API key was rejected: {m}"),
            Self::Internal(m) => format!("Cosmos Tools error: {m}"),
        }
    }

    /// Whether a caller may reasonably retry the same call shortly.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Unavailable(_) | Self::Internal(_))
    }

    /// Map an MCP tool failure payload (`isError: true` text) into a
    /// classified error. Upstream refusal vocabulary (`timeout`,
    /// `unauthorized`, `connection_required`…) is preserved where visible.
    pub fn from_tool_error(text: &str) -> Self {
        let lower = text.to_ascii_lowercase();
        let code = [
            "connection_required",
            "unauthorized",
            "forbidden",
            "not_found",
            "timeout",
            "rate_limit",
            "unsupported",
            "refused",
        ]
        .into_iter()
        .find(|needle| lower.contains(needle))
        .unwrap_or("refused");
        Self::refused(code, first_line(text))
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or(text).trim();
    line.chars().take(300).collect()
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for ToolError {}

// ---------------------------------------------------------------------------
// Providers — the device-local tool store
// ---------------------------------------------------------------------------

pub const TOOLS_FILENAME: &str = "tools.json";

/// One configured upstream. `name` is the display label AND (slugified) the
/// wire prefix — `composio__COMPOSIO_SEARCH_TOOLS` rides the Composio
/// singleton, `github__create_issue` a server the user named "GitHub".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolProvider {
    pub id: String,
    pub name: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub config: ProviderConfig,
    /// Composio only: toolkit slugs the user connected or searched for —
    /// remembered so `ListComposioApps` can re-query their live status past
    /// the curated [`POPULAR_APPS`] list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toolkits: Vec<String>,
}

fn default_enabled() -> bool {
    true
}

/// Transport + secrets for one provider. Serialized plainly into
/// `tools.json` (device-local, same boundary as CLI account files); the RPC
/// surface only ever returns the [`ProviderStatus::config`] masked echo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "transport",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ProviderConfig {
    /// Composio Connect — `connect.composio.dev/mcp` + consumer key header.
    Composio { api_key: String },
    /// Remote streamable-HTTP MCP server with static headers.
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
    },
    /// A local MCP server spawned as a stdio child.
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
    },
}

impl ProviderConfig {
    /// Wire label for the transport (badges on the settings page).
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Composio { .. } => "composio",
            Self::Http { .. } => "http",
            Self::Stdio { .. } => "stdio",
        }
    }
}

pub const COMPOSIO_PROVIDER_ID: &str = "composio";

/// A provider's wire prefix: the Composio singleton is always `composio`;
/// custom providers slugify their display name.
pub fn provider_slug(provider: &ToolProvider) -> String {
    if matches!(provider.config, ProviderConfig::Composio { .. }) {
        return "composio".into();
    }
    slugify(&provider.name)
}

/// `My GitHub Server!` → `my_github_server`; empty degenerates to `server`.
pub fn slugify(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut last_underscore = true; // leading trim
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore {
            slug.push('_');
            last_underscore = true;
        }
    }
    let slug = slug.trim_end_matches('_').to_string();
    if slug.is_empty() {
        "server".into()
    } else {
        slug
    }
}

/// The whole `tools.json` document.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolStore {
    pub providers: Vec<ToolProvider>,
}

pub fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join(TOOLS_FILENAME)
}

pub fn load_store(data_dir: &Path) -> ToolStore {
    let path = store_path(data_dir);
    let Ok(raw) = std::fs::read(&path) else {
        return ToolStore::default();
    };
    serde_json::from_slice(&raw).unwrap_or_else(|err| {
        tracing::warn!(error = %err, path = %path.display(), "corrupt tools store; using defaults");
        ToolStore::default()
    })
}

pub fn save_store(data_dir: &Path, store: &ToolStore) -> Result<(), ToolError> {
    let path = store_path(data_dir);
    let body = serde_json::to_vec_pretty(store).map_err(|e| ToolError::Internal(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &body).map_err(|e| ToolError::Internal(e.to_string()))?;
    std::fs::rename(&tmp, &path).map_err(|e| ToolError::Internal(e.to_string()))
}

// ---------------------------------------------------------------------------
// Status — the shape `ListTools`/`WatchTools` return
// ---------------------------------------------------------------------------

/// Live state of one provider in the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub id: String,
    pub name: String,
    /// `composio` | `http` | `stdio` — the transport badge.
    pub kind: String,
    /// The wire prefix this provider's tools carry.
    pub slug: String,
    pub enabled: bool,
    /// `disabled` | `idle` | `connected` | `error`
    pub status: String,
    /// Advertised tool count from the last successful handshake (0 while
    /// idle or after a failed connect).
    pub tool_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Masked config echo — enough to recognize the entry, never the
    /// secrets: composio shows `apiKeyMasked`, http shows `url` + header
    /// NAMES, stdio shows `command` + `args` + env NAMES.
    pub config: serde_json::Value,
}

/// The `ListTools`/`WatchTools` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsSnapshot {
    pub providers: Vec<ProviderStatus>,
    /// The merged, prefixed tool count harnesses currently see.
    pub tool_count: usize,
}

/// Masked config echo for the settings page — deliberately field-by-field so
/// a future secret-bearing key can't leak by `serde(skip)` accident.
pub fn masked_config(config: &ProviderConfig) -> serde_json::Value {
    match config {
        ProviderConfig::Composio { api_key } => serde_json::json!({
            "apiKeyMasked": mask_secret(api_key),
        }),
        ProviderConfig::Http { url, headers } => serde_json::json!({
            "url": url,
            "headerNames": headers.keys().collect::<Vec<_>>(),
        }),
        ProviderConfig::Stdio { command, args, env } => serde_json::json!({
            "command": command,
            "args": args,
            "envNames": env.keys().collect::<Vec<_>>(),
        }),
    }
}

/// `ck_abcdef123456` → `ck_…3456` — prefix kind hint plus the tail only.
pub fn mask_secret(secret: &str) -> String {
    let prefix: String = secret.chars().take(3).collect();
    let tail: String = secret
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}…{tail}")
}

// ---------------------------------------------------------------------------
// Composio app rows — `ListComposioApps` / `StartAppConnect` / `PollAppConnect`
// ---------------------------------------------------------------------------

/// One toolkit's connection posture on this device's Composio key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposioApp {
    pub toolkit: String,
    /// The upstream status verbatim (`ACTIVE`, `INITIATED`, `EXPIRED`, …)
    /// or `not-connected` when no connection exists.
    pub status: String,
    /// `status == ACTIVE` — the connect button reads "Connected" off this.
    pub connected: bool,
    /// The active/default account's display identity (email or name) when
    /// the upstream reports one — "Connected as …" on the settings page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// `StartAppConnect`'s reply — the URL the settings page opens for the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConnectStart {
    pub toolkit: String,
    /// The Composio Connect Link (short-lived — open promptly; retry
    /// regenerates). Absent when the toolkit is already ACTIVE.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    pub status: String,
    pub connected: bool,
}

/// One search hit from `COMPOSIO_SEARCH_TOOLS`, flattened for the settings
/// page's toolkit rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComposioSearchHit {
    pub toolkit: String,
    /// Sample tool slugs the search matched inside this toolkit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    /// Composio's own description of the toolkit, when the payload carries
    /// `toolkit_connection_statuses`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `has_active_connection` from the payload — the row's connect state
    /// without a second round trip.
    #[serde(default)]
    pub connected: bool,
}

/// `ProbeMcpServer`'s reply — a throwaway config test (never persisted).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub ok: bool,
    pub tool_count: usize,
    /// First few advertised tool names (preview chips on the form).
    pub tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Curated catalog entries
// ---------------------------------------------------------------------------

/// Everyday picks shown as the settings page's connect grid. `toolkit` is
/// Composio's slug (what `COMPOSIO_MANAGE_CONNECTIONS` takes — Google's
/// apps are single-word slugs: `googledrive`, not `google_drive`).
pub struct PopularApp {
    pub toolkit: &'static str,
    pub label: &'static str,
    /// One-line "what it does" under the label on the connect grid.
    pub blurb: &'static str,
}

/// How many curated picks the grid shows by default — the rest of the
/// catalog is one search or the Composio marketplace away.
pub const FEATURED_APP_COUNT: usize = 5;

pub const POPULAR_APPS: &[PopularApp] = &[
    PopularApp {
        toolkit: "gmail",
        label: "Gmail",
        blurb: "Email, drafts & threads",
    },
    PopularApp {
        toolkit: "slack",
        label: "Slack",
        blurb: "Channels & messages",
    },
    PopularApp {
        toolkit: "notion",
        label: "Notion",
        blurb: "Pages & databases",
    },
    PopularApp {
        toolkit: "github",
        label: "GitHub",
        blurb: "Repos, issues & PRs",
    },
    PopularApp {
        toolkit: "googlecalendar",
        label: "Google Calendar",
        blurb: "Events & scheduling",
    },
    PopularApp {
        toolkit: "googledrive",
        label: "Google Drive",
        blurb: "Files & sharing",
    },
    PopularApp {
        toolkit: "linear",
        label: "Linear",
        blurb: "Issues & projects",
    },
    PopularApp {
        toolkit: "outlook",
        label: "Outlook",
        blurb: "Mail & calendar",
    },
    PopularApp {
        toolkit: "googlesheets",
        label: "Google Sheets",
        blurb: "Spreadsheets & values",
    },
    PopularApp {
        toolkit: "todoist",
        label: "Todoist",
        blurb: "Tasks & projects",
    },
];

/// The public toolkit marketplace — the settings page's "browse the full
/// catalog" target. Per-app OAuth runs through `MANAGE_CONNECTIONS`
/// Connect Links; the marketplace is the discovery surface for everything
/// beyond the curated grid and search.
pub const COMPOSIO_BROWSE_URL: &str = "https://composio.dev/toolkits";

/// A toolkit's brand tile — the logo feed Composio's own hosted Connect
/// pages render (`logos.composio.dev/api/<slug>`).
pub fn toolkit_logo_url(toolkit: &str) -> String {
    format!(
        "https://logos.composio.dev/api/{}",
        toolkit.trim().to_ascii_lowercase()
    )
}

/// One-click MCP server suggestions for the settings page. `needs_auth`
/// marks OAuth-only remote servers — they prefill the form but can't
/// complete until remote-MCP OAuth lands; header-auth and local servers
/// work today.
pub struct PresetServer {
    pub id: &'static str,
    pub name: &'static str,
    /// `stdio` | `http`
    pub transport: &'static str,
    pub command: &'static str,
    pub args: &'static [&'static str],
    pub url: &'static str,
    /// Form hint (e.g. required env) rendered next to the chip.
    pub note: &'static str,
    pub needs_auth: bool,
}

pub const PRESET_SERVERS: &[PresetServer] = &[
    PresetServer {
        id: "figma-local",
        name: "Figma (Dev Mode)",
        transport: "http",
        command: "",
        args: &[],
        url: "http://127.0.0.1:3845/mcp",
        note: "Figma desktop app with Dev Mode MCP enabled",
        needs_auth: false,
    },
    PresetServer {
        id: "figma-remote",
        name: "Figma",
        transport: "http",
        command: "",
        args: &[],
        url: "https://mcp.figma.com/mcp",
        note: "sign-in required",
        needs_auth: true,
    },
    PresetServer {
        id: "pencil",
        name: "Pencil",
        transport: "stdio",
        command: "pencil",
        args: &["mcp-server"],
        url: "",
        note: "pen.dev app must be running",
        needs_auth: false,
    },
    PresetServer {
        id: "playwright",
        name: "Playwright",
        transport: "stdio",
        command: "npx",
        args: &["-y", "@playwright/mcp"],
        url: "",
        note: "browser automation via npx",
        needs_auth: false,
    },
    PresetServer {
        id: "github",
        name: "GitHub",
        transport: "http",
        command: "",
        args: &[],
        url: "https://api.githubcopilot.com/mcp/",
        note: "add an Authorization header with a PAT",
        needs_auth: true,
    },
    PresetServer {
        id: "notion",
        name: "Notion",
        transport: "http",
        command: "",
        args: &[],
        url: "https://mcp.notion.com/mcp",
        note: "sign-in required",
        needs_auth: true,
    },
    PresetServer {
        id: "linear",
        name: "Linear",
        transport: "http",
        command: "",
        args: &[],
        url: "https://mcp.linear.app/mcp",
        note: "sign-in required",
        needs_auth: true,
    },
    PresetServer {
        id: "filesystem",
        name: "Filesystem",
        transport: "stdio",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-filesystem"],
        url: "",
        note: "append allowed directories as args",
        needs_auth: false,
    },
];
