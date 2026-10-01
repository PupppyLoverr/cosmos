//! Config loading — `COSMOS_CLI_HOME/config.toml` (contract §1).

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use zproto::Prices;
use zproviders::{ProviderCfg, ProviderKind};
use ztools::McpServerCfg;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    pub default_model: Option<String>,
    #[serde(default)]
    pub providers: HashMap<String, ProviderEntry>,
    #[serde(default)]
    pub models: HashMap<String, ModelEntry>,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub subagents: Subagents,
    #[serde(default)]
    pub bridge: Bridge,
    #[serde(default)]
    pub mcp: HashMap<String, McpEntry>,
    #[serde(default)]
    pub hooks: Vec<HookCfg>,
    #[serde(default)]
    pub compact: CompactCfgFile,
    #[serde(default)]
    pub sandbox: SandboxCfg,
    #[serde(default)]
    pub self_improve: SelfImproveFile,
}

#[derive(Debug, Deserialize)]
pub struct ProviderEntry {
    pub kind: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: String,
}

#[derive(Debug, Deserialize)]
pub struct ModelEntry {
    pub provider: String,
    pub id: String,
    #[serde(default = "default_context")]
    pub context: u64,
    #[serde(default)]
    pub price: Prices,
}

fn default_context() -> u64 {
    128_000
}

#[derive(Debug, Deserialize)]
pub struct Permissions {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub ask: Vec<String>,
}

/// Without a `[permissions]` table (a fresh install), read-only tools run and
/// anything that changes files or runs commands asks first — the same table
/// as config.example.toml. An explicit table replaces this entirely.
impl Default for Permissions {
    fn default() -> Self {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect();
        Permissions {
            allow: v(&["read", "grep", "glob", "ls", "memory_search", "tool_search"]),
            ask: v(&["write", "edit", "bash", "memory_write", "task", "mcp_call"]),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Subagents {
    #[serde(default = "default_parallel")]
    pub max_parallel: usize,
    #[serde(default)]
    pub model: String,
}
impl Default for Subagents {
    fn default() -> Self {
        Self {
            max_parallel: default_parallel(),
            model: String::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CompactCfgFile {
    #[serde(default = "default_reserved")]
    pub reserved_output_tokens: u64,
    #[serde(default = "default_keep_tail")]
    pub keep_tail: usize,
    #[serde(default = "default_ratio")]
    pub trigger_ratio: f64,
    #[serde(default = "default_warm")]
    pub cache_warm_secs: u64,
}
impl Default for CompactCfgFile {
    fn default() -> Self {
        Self {
            reserved_output_tokens: default_reserved(),
            keep_tail: default_keep_tail(),
            trigger_ratio: default_ratio(),
            cache_warm_secs: default_warm(),
        }
    }
}
fn default_reserved() -> u64 {
    8000
}
fn default_keep_tail() -> usize {
    6
}
fn default_ratio() -> f64 {
    0.80
}
/// 0 = provider default TTL (Anthropic 300 s, others 600 s).
fn default_warm() -> u64 {
    0
}

#[derive(Debug, Deserialize)]
pub struct SandboxCfg {
    #[serde(default = "default_sandbox_mode")]
    pub mode: String,
    #[serde(default = "default_ask")]
    pub ask: String,
}
impl Default for SandboxCfg {
    fn default() -> Self {
        Self {
            mode: default_sandbox_mode(),
            ask: default_ask(),
        }
    }
}
fn default_sandbox_mode() -> String {
    "workspace-write".into()
}
fn default_ask() -> String {
    "on-request".into()
}

#[derive(Debug, Deserialize)]
pub struct SelfImproveFile {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub write_skills: bool,
    #[serde(default = "default_true")]
    pub write_preferences: bool,
    #[serde(default = "default_true")]
    pub write_corrections: bool,
    #[serde(default)]
    pub require_gate: bool,
}
impl Default for SelfImproveFile {
    fn default() -> Self {
        Self {
            enabled: true,
            write_skills: true,
            write_preferences: true,
            write_corrections: true,
            require_gate: false,
        }
    }
}
fn default_true() -> bool {
    true
}
fn default_parallel() -> usize {
    3
}

#[derive(Debug, Deserialize)]
pub struct Bridge {
    #[serde(default = "default_large_read")]
    pub large_read_lines: usize,
    #[serde(default)]
    pub summarizer_model: String,
}
impl Default for Bridge {
    fn default() -> Self {
        Self {
            large_read_lines: default_large_read(),
            summarizer_model: String::new(),
        }
    }
}
fn default_large_read() -> usize {
    200
}

/// One row of the model picker.
#[derive(Debug, Clone)]
pub struct ModelChoice {
    /// what `--model` / `SetModel` takes
    pub alias: String,
    pub provider: String,
    pub provider_name: String,
    pub model: String,
    pub name: String,
    pub context: u64,
    pub price: Prices,
    /// wire protocol (`openai`, `responses`, `anthropic`, …)
    pub kind: String,
    pub reasoning: bool,
    pub from_config: bool,
}

/// A catalog provider is usable when signed in, keyed, or local.
pub fn provider_ready(p: &crate::catalog::ProviderDef) -> bool {
    match p.kind.as_str() {
        "chatgpt" => crate::chatgpt::load().is_some(),
        "devin" => crate::devin::known_ready(),
        _ if p.env.is_empty() => false, // local servers list their models via config.toml
        _ => crate::creds::key_for(&p.id, &p.env).is_some(),
    }
}

#[derive(Debug, Deserialize)]
pub struct McpEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct HookCfg {
    pub event: String,
    #[serde(default)]
    pub matcher: String,
    pub command: String,
}

/// Strip NULs and Go `%q` `\xHH` sequences that aren't valid TOML escapes.
pub fn sanitize_toml_text(s: &str) -> String {
    let s = s.replace('\0', "");
    let ch: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < ch.len() {
        if ch[i] == '\\'
            && i + 3 < ch.len()
            && ch[i + 1] == 'x'
            && ch[i + 2].is_ascii_hexdigit()
            && ch[i + 3].is_ascii_hexdigit()
        {
            i += 4;
            continue;
        }
        out.push(ch[i]);
        i += 1;
    }
    out
}

fn parse_config_text(text: &str) -> Result<Config, String> {
    match toml::from_str(text) {
        Ok(c) => Ok(c),
        Err(e) => {
            let cleaned = sanitize_toml_text(text);
            if cleaned != text {
                toml::from_str(&cleaned).map_err(|e2| e2.to_string())
            } else {
                Err(e.to_string())
            }
        }
    }
}

fn short_err(e: &str) -> String {
    e.chars()
        .map(|c| if c < ' ' && c != '\n' { ' ' } else { c })
        .take(180)
        .collect()
}

impl Config {
    /// Load `home/config.toml`; missing file → defaults with a fixture model
    /// so the harness works offline out of the box.
    pub fn load(home: &Path) -> Config {
        let file = home.join("config.toml");
        let mut cfg: Config = match std::fs::read_to_string(&file) {
            Ok(text) => parse_config_text(&text).unwrap_or_else(|e| {
                eprintln!("config.toml parse error: {} — using defaults", short_err(&e));
                Config::default()
            }),
            Err(_) => Config::default(),
        };
        if cfg.providers.is_empty() {
            cfg.providers.insert(
                "fixture".into(),
                ProviderEntry {
                    kind: "fixture".into(),
                    base_url: String::new(),
                    api_key_env: String::new(),
                },
            );
        }
        if cfg.models.is_empty() {
            cfg.models.insert(
                "fixture".into(),
                ModelEntry {
                    provider: "fixture".into(),
                    id: "fixture".into(),
                    context: 128_000,
                    price: Prices::default(),
                },
            );
            // keep a configured default (e.g. a catalog ref like
            // anthropic/claude-opus-5-5 written without a [models] table)
            if cfg.default_model.is_none() {
                cfg.default_model = Some("fixture".into());
            }
        }
        if let Ok(file) = std::env::current_dir().map(|p| p.join(".mcp.json")) {
            if let Ok(text) = std::fs::read_to_string(file) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                    if let Some(servers) = value.get("mcpServers").and_then(|v| v.as_object()) {
                        for (name, server) in servers {
                            if let Ok(entry) = serde_json::from_value::<McpEntry>(server.clone()) {
                                cfg.mcp.entry(name.clone()).or_insert(entry);
                            }
                        }
                    }
                }
            }
        }
        cfg
    }

    /// Resolve a model alias → provider config.
    ///
    /// Two forms: a `[models]` alias from config.toml (`r9`), or a catalog
    /// reference `provider/model` (`anthropic/claude-opus-5-5`,
    /// `chatgpt/gpt-5.5`, `openrouter/qwen/qwen3.8-max`). Keys come from the
    /// provider's env vars or the credential store (`zcli auth login`).
    pub fn provider_for(&self, alias: &str) -> Option<(ProviderCfg, String)> {
        if let Some(m) = self.models.get(alias) {
            let p = self.providers.get(&m.provider)?;
            let kind = ProviderKind::parse(&p.kind)?;
            // a config provider with no usable env var can still use a stored key
            let env_ok = !p.api_key_env.is_empty() && std::env::var(&p.api_key_env).is_ok_and(|v| !v.is_empty());
            let api_key = if env_ok || p.kind == "fixture" { None } else { crate::creds::stored_key(&m.provider).map(|(k, _)| k) };
            return Some((
                ProviderCfg {
                    kind,
                    base_url: p.base_url.clone(),
                    api_key_env: p.api_key_env.clone(),
                    model: m.id.clone(),
                    context: m.context,
                    prices: m.price,
                    effort: None,
                    api_key,
                    account_id: None,
                },
                m.id.clone(),
            ));
        }
        let (pid, mid) = crate::catalog::split_ref(alias)?;
        // a config.toml provider of that name wins over the catalog entry
        if let Some(p) = self.providers.get(pid) {
            let kind = ProviderKind::parse(&p.kind)?;
            let def = crate::catalog::provider(pid).and_then(|d| d.model(mid));
            return Some((
                ProviderCfg {
                    kind,
                    base_url: p.base_url.clone(),
                    api_key_env: p.api_key_env.clone(),
                    model: mid.to_string(),
                    context: def.map(|d| d.context).unwrap_or(128_000),
                    prices: def.map(|d| d.prices()).unwrap_or_default(),
                    effort: None,
                    api_key: crate::creds::stored_key(pid).map(|(k, _)| k),
                    account_id: None,
                },
                mid.to_string(),
            ));
        }
        let def = crate::catalog::provider(pid)?;
        let kind = ProviderKind::parse(&def.kind)?;
        let model = def.model(mid).cloned().or_else(|| {
            crate::live::cached(pid).and_then(|ms| ms.into_iter().find(|m| m.id == mid))
        });
        let (api_key, account_id) = match def.kind.as_str() {
            "chatgpt" => match crate::chatgpt::fresh() {
                Some(t) => (Some(t.access), Some(t.account_id)),
                None => (None, None),
            },
            "devin" => (None, None),
            _ => (crate::creds::key_for(pid, &def.env).map(|(k, _)| k), None),
        };
        Some((
            ProviderCfg {
                kind,
                base_url: def.base_url.clone(),
                api_key_env: def.env.first().cloned().unwrap_or_default(),
                model: mid.to_string(),
                context: model.as_ref().map(|m| m.context).unwrap_or(128_000),
                prices: model.as_ref().map(|m| m.prices()).unwrap_or_default(),
                effort: None,
                api_key,
                account_id,
            },
            mid.to_string(),
        ))
    }

    /// §12 outbound allowlist = configured provider hosts + the built-in
    /// catalog's provider hosts + the ChatGPT sign-in host.
    pub fn allowed_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self
            .providers
            .values()
            .filter(|p| !p.base_url.is_empty())
            .map(|p| crate::catalog::host_of(&p.base_url))
            .chain(crate::catalog::providers().iter().map(|p| p.host()))
            .chain([
                "auth.openai.com".to_string(),
                "server.codeium.com".to_string(),
                "inference.codeium.com".to_string(),
                "api.devin.ai".to_string(),
            ])
            .filter(|h| !h.is_empty())
            .collect();
        hosts.sort();
        hosts.dedup();
        hosts
    }

    /// Every model the picker can offer: config.toml aliases, then catalog
    /// models of providers usable right now (key present or signed in).
    pub fn available_models(&self) -> Vec<ModelChoice> {
        let mut out: Vec<ModelChoice> = self
            .models
            .iter()
            .map(|(alias, m)| ModelChoice {
                alias: alias.clone(),
                provider: m.provider.clone(),
                provider_name: m.provider.clone(),
                model: m.id.clone(),
                name: alias.clone(),
                context: m.context,
                price: m.price,
                kind: self.providers.get(&m.provider).map(|p| p.kind.clone()).unwrap_or_default(),
                reasoning: false,
                from_config: true,
            })
            .collect();
        out.sort_by(|a, b| a.alias.cmp(&b.alias));
        let mut seen: std::collections::HashSet<String> = out.iter().map(|m| m.alias.clone()).collect();
        for p in crate::catalog::providers() {
            if !provider_ready(p) {
                continue;
            }
            // the live list from the provider (onboarding / /providers) beats the snapshot
            let live = crate::live::cached(&p.id);
            for m in live.as_deref().unwrap_or(&p.models) {
                let alias = format!("{}/{}", p.id, m.id);
                if !seen.insert(alias.clone()) {
                    continue;
                }
                out.push(ModelChoice {
                    alias,
                    provider: p.id.clone(),
                    provider_name: p.name.clone(),
                    model: m.id.clone(),
                    name: m.name.clone(),
                    context: m.context,
                    price: m.prices(),
                    kind: p.kind.clone(),
                    reasoning: m.reasoning,
                    from_config: false,
                });
            }
        }
        out
    }

    pub fn compact_cfg(&self) -> crate::compact::CompactCfg {
        crate::compact::CompactCfg {
            large_read_lines: self.bridge.large_read_lines,
            reserved_output_tokens: self.compact.reserved_output_tokens,
            cache_warm_secs: self.compact.cache_warm_secs,
            trigger_ratio: self.compact.trigger_ratio,
        }
    }

    pub fn mcp_servers(&self) -> HashMap<String, McpServerCfg> {
        self.mcp
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    McpServerCfg {
                        command: v.command.clone(),
                        args: v.args.clone(),
                        env: v.env.clone(),
                    },
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_models_show_even_when_provider_is_in_config() {
        let _env = crate::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("COSMOS_CLI_HOME", dir.path());
        std::env::set_var("COSMOS_CLI_KEYRING", "0");
        std::fs::write(
            dir.path().join("config.toml"),
            "[providers.openai]\nkind = \"responses\"\nbase_url = \"https://api.openai.com/v1\"\napi_key_env = \"OPENAI_API_KEY\"\n",
        )
        .unwrap();
        crate::creds::store_key("openai", "sk-test").unwrap();
        let cfg = Config::load(dir.path());
        let models = cfg.available_models();
        assert!(
            models.iter().any(|m| m.alias.starts_with("openai/")),
            "catalog openai models missing when [providers.openai] exists: {:?}",
            models.iter().map(|m| &m.alias).collect::<Vec<_>>()
        );
    }

    #[test]
    fn recovers_go_percent_q_nul_escapes() {
        let _env = crate::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("COSMOS_CLI_HOME", dir.path());
        std::fs::write(
            dir.path().join("config.toml"),
            "user_name = \"Rav\\x00\\x00\"\ndefault_model = \"devin/swe-2-max\"\n",
        )
        .unwrap();
        let cfg = Config::load(dir.path());
        assert_eq!(cfg.default_model.as_deref(), Some("devin/swe-2-max"));
        assert!(cfg.provider_for("devin/swe-2-max").is_some());
    }

    #[test]
    fn sanitize_drops_x_escapes() {
        assert_eq!(sanitize_toml_text("user_name = \"Rav\\x00\\x00\""), "user_name = \"Rav\"");
    }
}
