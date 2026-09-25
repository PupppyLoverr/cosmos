//! Built-in provider catalog for bring-your-own-key use.
//!
//! `catalog/catalog.json` is generated from models.dev (`catalog/gen.mjs`):
//! 30 providers, their wire protocol, base URL, key env vars and current
//! tool-calling models with context sizes and prices. A model is addressed as
//! `provider/model`, e.g. `anthropic/claude-opus-5-5` or `xiaomi/mimo-v2.6-pro`.
//! The ChatGPT subscription is added here by hand — it isn't an API product.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use zproto::Prices;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelDef {
    pub id: String,
    pub name: String,
    pub context: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub reasoning: bool,
    /// USD per 1M tokens: [input, output, cache read, cache write]
    #[serde(default)]
    pub price: [f64; 4],
}

impl ModelDef {
    pub fn prices(&self) -> Prices {
        Prices { input: self.price[0], output: self.price[1], cache_read: self.price[2], cache_write: self.price[3] }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderDef {
    pub id: String,
    pub name: String,
    /// `openai` | `responses` | `anthropic` | `google` | `chatgpt`
    pub kind: String,
    pub base_url: String,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub models: Vec<ModelDef>,
}

impl ProviderDef {
    /// Signs in with an account rather than an API key.
    pub fn is_subscription(&self) -> bool {
        self.kind == "chatgpt"
    }

    pub fn model(&self, id: &str) -> Option<&ModelDef> {
        self.models.iter().find(|m| m.id == id)
    }

    pub fn host(&self) -> String {
        host_of(&self.base_url)
    }
}

pub fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|s| s.split(['/', ':', '?']).next())
        .unwrap_or("")
        .to_string()
}

#[derive(Deserialize)]
struct CatalogFile {
    providers: Vec<ProviderDef>,
}

/// Models served on a ChatGPT Plus/Pro plan (the Codex CLI list).
fn chatgpt() -> ProviderDef {
    let m = |id: &str, name: &str, context: u64| ModelDef {
        id: id.into(),
        name: name.into(),
        context,
        output: 128_000,
        reasoning: true,
        price: [0.0; 4],
    };
    ProviderDef {
        id: "chatgpt".into(),
        name: "ChatGPT (Plus / Pro subscription)".into(),
        kind: "chatgpt".into(),
        base_url: "https://chatgpt.com/backend-api".into(),
        env: Vec::new(),
        models: vec![
            m("gpt-5.5", "GPT-5.5", 272_000),
            m("gpt-6-astra", "GPT-6 Astra", 272_000),
            m("gpt-5.6-sol", "GPT-5.6 Sol", 272_000),
            m("gpt-5.6-terra", "GPT-5.6 Terra", 272_000),
            m("gpt-5.6-luna", "GPT-5.6 Luna", 272_000),
            m("gpt-5.3-codex-spark", "GPT-5.3 Codex Spark", 128_000),
        ],
    }
}

/// 9Router — a local OpenAI-compatible gateway that routes the operator's
/// existing subscriptions (Kiro, Kilo, Antigravity, …). Its key comes from
/// the 9Router dashboard. The routes below are the verified defaults; the
/// live list from its `/models` replaces them once it's running.
fn router9() -> ProviderDef {
    let m = |id: &str, name: &str, context: u64| ModelDef {
        id: id.into(),
        name: name.into(),
        context,
        output: 32_000,
        reasoning: false,
        price: [0.0; 4],
    };
    ProviderDef {
        id: "router9".into(),
        name: "9Router".into(),
        kind: "openai".into(),
        base_url: "http://localhost:20128/v1".into(),
        env: vec!["ROUTER9_API_KEY".into()],
        models: vec![
            m("kr/claude-sonnet-4.5", "Claude Sonnet 4.5 (Kiro)", 200_000),
            m("ag/gemini-3.8-flash", "Gemini 3.8 Flash (Antigravity)", 1_048_576),
            m("kc/kilo-auto/free", "Kilo Auto (free)", 200_000),
        ],
    }
}

/// Local servers that need no key.
fn local() -> Vec<ProviderDef> {
    vec![
        ProviderDef {
            id: "ollama".into(),
            name: "Ollama (local)".into(),
            kind: "openai".into(),
            base_url: "http://127.0.0.1:11434/v1".into(),
            env: Vec::new(),
            models: Vec::new(),
        },
        ProviderDef {
            id: "lmstudio".into(),
            name: "LM Studio (local)".into(),
            kind: "openai".into(),
            base_url: "http://127.0.0.1:1234/v1".into(),
            env: Vec::new(),
            models: Vec::new(),
        },
    ]
}

pub fn providers() -> &'static [ProviderDef] {
    static P: OnceLock<Vec<ProviderDef>> = OnceLock::new();
    P.get_or_init(|| {
        let file: CatalogFile = serde_json::from_str(include_str!("../catalog/catalog.json"))
            .expect("catalog.json is generated and checked in");
        let mut v = vec![chatgpt(), router9()];
        v.extend(file.providers);
        v.extend(local());
        v
    })
}

pub fn provider(id: &str) -> Option<&'static ProviderDef> {
    providers().iter().find(|p| p.id == id)
}

/// `provider/model` → (provider, model id). The model part may itself
/// contain slashes (`openrouter/anthropic/claude-…`).
pub fn split_ref(alias: &str) -> Option<(&str, &str)> {
    let (p, m) = alias.split_once('/')?;
    (!p.is_empty() && !m.is_empty()).then_some((p, m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads_with_requested_providers() {
        for id in ["chatgpt", "router9", "openai", "anthropic", "google", "meta", "llama", "xiaomi", "alibaba", "bailing", "xai", "deepseek", "moonshotai", "zai", "minimax", "mistral", "groq", "openrouter", "ollama"] {
            assert!(provider(id).is_some(), "{id} missing from catalog");
        }
        let xiaomi = provider("xiaomi").unwrap();
        assert!(!xiaomi.models.is_empty());
        assert_eq!(xiaomi.kind, "openai");
        assert!(provider("chatgpt").unwrap().is_subscription());
    }

    #[test]
    fn refs_split_on_first_slash() {
        assert_eq!(split_ref("openrouter/anthropic/claude-x"), Some(("openrouter", "anthropic/claude-x")));
        assert_eq!(split_ref("r9"), None);
    }
}
