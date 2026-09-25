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

#[derive(Debug, Deserialize, Default)]
pub struct Permissions {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub ask: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Subagents {
    #[serde(default = "default_parallel")]
    pub max_parallel: usize,
}
impl Default for Subagents {
    fn default() -> Self {
        Self {
            max_parallel: default_parallel(),
        }
    }
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
    if p.is_subscription() {
        return crate::chatgpt::load().is_some();
    }
    if p.env.is_empty() {
        return false; // local servers list their models via config.toml
    }
    crate::creds::key_for(&p.id, &p.env).is_some()
}

#[derive(Debug, Deserialize)]
pub struct McpEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct HookCfg {
    pub event: String,
    #[serde(default)]
    pub matcher: String,
    pub command: String,
}

impl Config {
    /// Load `home/config.toml`; missing file → defaults with a fixture model
    /// so the harness works offline out of the box.
    pub fn load(home: &Path) -> Config {
        let file = home.join("config.toml");
        let mut cfg: Config = match std::fs::read_to_string(&file) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                eprintln!("config.toml parse error: {e} — using defaults");
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
        let model = def.model(mid);
        let (api_key, account_id) = if def.is_subscription() {
            match crate::chatgpt::fresh() {
                Some(t) => (Some(t.access), Some(t.account_id)),
                None => (None, None),
            }
        } else {
            (crate::creds::key_for(pid, &def.env).map(|(k, _)| k), None)
        };
        Some((
            ProviderCfg {
                kind,
                base_url: def.base_url.clone(),
                api_key_env: def.env.first().cloned().unwrap_or_default(),
                model: mid.to_string(),
                context: model.map(|m| m.context).unwrap_or(128_000),
                prices: model.map(|m| m.prices()).unwrap_or_default(),
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
            .chain(["auth.openai.com".to_string()])
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
        for p in crate::catalog::providers() {
            if self.providers.contains_key(&p.id) || !provider_ready(p) {
                continue;
            }
            // the live list from the provider (onboarding / refresh) beats the snapshot
            let live = crate::live::cached(&p.id);
            for m in live.as_deref().unwrap_or(&p.models) {
                out.push(ModelChoice {
                    alias: format!("{}/{}", p.id, m.id),
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

    pub fn mcp_servers(&self) -> HashMap<String, McpServerCfg> {
        self.mcp
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    McpServerCfg {
                        command: v.command.clone(),
                        args: v.args.clone(),
                    },
                )
            })
            .collect()
    }
}
