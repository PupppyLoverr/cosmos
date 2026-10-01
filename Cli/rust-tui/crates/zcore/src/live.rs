//! Live model lists, straight from each provider's own API.
//!
//! The built-in catalog is a snapshot; this asks the provider what the key in
//! hand can actually use right now (`GET /models` in each wire dialect), keeps
//! the tool-capable chat models, orders them best-first (catalog order for the
//! models it knows, newest first for the rest) and fills in context size and
//! prices from the catalog or the provider's own metadata.
//!
//! Results are cached in `COSMOS_CLI_HOME/live_models.json`, and
//! `Config::available_models` prefers the cache, so the `/model` picker shows
//! the same real list the onboarding picker did.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::catalog::{host_of, ModelDef, ProviderDef};

/// A model as the provider reported it.
#[derive(Debug, Default, Clone)]
struct Raw {
    id: String,
    name: Option<String>,
    context: Option<u64>,
    output: Option<u64>,
    /// USD per 1M tokens (input, output)
    price: Option<(f64, f64)>,
    created: i64,
    reasoning: bool,
}

/// Ask `def`'s API for the models this machine's credentials can use.
pub fn fetch(def: &ProviderDef) -> Result<Vec<ModelDef>> {
    let raw = match def.kind.as_str() {
        "chatgpt" => chatgpt()?,
        #[cfg(feature = "devin-direct")]
        "devin" => devin()?,
        "acp" => bail!("{} picks its own models — use {}/default", def.name, def.id),
        "anthropic" => anthropic(def)?,
        "google" => google(def)?,
        _ => openai(def)?,
    };
    let models = merge(def, raw);
    if models.is_empty() {
        bail!("{} returned no chat models", def.host());
    }
    Ok(models)
}

fn key(def: &ProviderDef) -> Result<String> {
    crate::creds::key_for(&def.id, &def.env)
        .map(|(k, _)| k)
        .with_context(|| format!("no {} key — connect it first", def.name))
}

fn get_json(url: &str, headers: &[(&str, String)]) -> Result<Value> {
    // §12: same allowlist as every provider call
    let allowed = crate::config::Config::load(&crate::cli_home()).allowed_hosts();
    zproviders::host_allowed(url, &allowed)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.get(url);
    for (k, v) in headers {
        req = req.header(*k, v.as_str());
    }
    let host = host_of(url);
    let mut resp = req.call().with_context(|| format!("couldn't reach {host}"))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().with_config().limit(64 << 20).read_to_string().unwrap_or_default();
    match status {
        200..=299 => serde_json::from_str(&text).with_context(|| format!("{host} sent something that isn't JSON")),
        401 | 403 => bail!("{host} rejected the key ({status})"),
        404 => bail!("{host} has no model list endpoint (404)"),
        429 => bail!("{host} is rate limiting (429) — try again in a moment"),
        _ => {
            let msg = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().or(v["error"].as_str()).or(v["message"].as_str()).map(str::to_string))
                .unwrap_or_else(|| text.chars().take(160).collect());
            bail!("{host} answered {status}: {}", msg.trim())
        }
    }
}

fn u(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)).filter(|n| *n > 0)
}

/// Per-token price strings (OpenRouter) → USD per 1M tokens.
fn per_m(v: &Value) -> Option<f64> {
    let f = v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))?;
    (f >= 0.0).then_some((f * 1e6 * 10_000.0).round() / 10_000.0)
}

/// OpenAI-compatible `GET {base}/models` — OpenAI, OpenRouter, Groq, xAI, …
fn openai(def: &ProviderDef) -> Result<Vec<Raw>> {
    let url = format!("{}/models", def.base_url.trim_end_matches('/'));
    // OpenRouter's list is public; everyone else needs the key
    let headers = match key(def) {
        Ok(k) => vec![("authorization", format!("Bearer {k}"))],
        Err(e) if def.id != "openrouter" => return Err(e),
        Err(_) => Vec::new(),
    };
    if def.id == "openrouter" && !headers.is_empty() {
        // the list is public, so check the key on its own endpoint
        get_json(&format!("{}/key", def.base_url.trim_end_matches('/')), &headers)?;
    }
    let v = get_json(&url, &headers)?;
    let items = v["data"].as_array().or(v["models"].as_array()).or(v.as_array()).cloned().unwrap_or_default();
    Ok(items
        .iter()
        .filter(|m| {
            // when the provider says which parameters a model takes, it must take tools
            match m["supported_parameters"].as_array() {
                Some(p) => p.iter().any(|x| x == "tools"),
                None => true,
            }
        })
        .filter_map(|m| {
            let id = m["id"].as_str()?.to_string();
            Some(Raw {
                name: m["name"].as_str().or(m["display_name"].as_str()).map(str::to_string),
                context: u(&m["context_length"]).or(u(&m["context_window"])).or(u(&m["top_provider"]["context_length"])),
                output: u(&m["top_provider"]["max_completion_tokens"]).or(u(&m["max_output_tokens"])),
                price: match (per_m(&m["pricing"]["prompt"]), per_m(&m["pricing"]["completion"])) {
                    (Some(i), Some(o)) => Some((i, o)),
                    _ => None,
                },
                created: m["created"].as_i64().unwrap_or(0),
                reasoning: m["supported_parameters"].as_array().is_some_and(|p| p.iter().any(|x| x == "reasoning")),
                id,
            })
        })
        .collect())
}

/// Anthropic `GET /v1/models`.
fn anthropic(def: &ProviderDef) -> Result<Vec<Raw>> {
    let k = key(def)?;
    let url = format!("{}/v1/models?limit=1000", def.base_url.trim_end_matches('/'));
    let v = get_json(&url, &[("x-api-key", k), ("anthropic-version", "2023-06-01".into())])?;
    Ok(v["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            Some(Raw {
                id: m["id"].as_str()?.to_string(),
                name: m["display_name"].as_str().map(str::to_string),
                created: m["created_at"].as_str().map(date_key).unwrap_or(0),
                ..Raw::default()
            })
        })
        .collect())
}

/// Gemini `GET /v1beta/models` — only models that can `generateContent`.
fn google(def: &ProviderDef) -> Result<Vec<Raw>> {
    let k = key(def)?;
    let url = format!("{}/v1beta/models?pageSize=1000", def.base_url.trim_end_matches('/'));
    let v = get_json(&url, &[("x-goog-api-key", k)])?;
    Ok(v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["supportedGenerationMethods"].as_array().is_some_and(|a| a.iter().any(|x| x == "generateContent")))
        .filter_map(|m| {
            let id = m["name"].as_str()?.trim_start_matches("models/").to_string();
            Some(Raw {
                name: m["displayName"].as_str().map(str::to_string),
                context: u(&m["inputTokenLimit"]),
                output: u(&m["outputTokenLimit"]),
                reasoning: m["thinking"].as_bool().unwrap_or(false),
                id,
                ..Raw::default()
            })
        })
        .collect())
}

/// Devin subscription — `GetCliModelConfigs` over the signed-in plan.
#[cfg(feature = "devin-direct")]
fn devin() -> Result<Vec<Raw>> {
    let models = crate::devin::models()?;
    Ok(models
        .into_iter()
        .map(|m| Raw {
            id: m.id,
            name: Some(m.name),
            context: Some(m.context),
            output: Some(m.output),
            reasoning: m.reasoning,
            ..Raw::default()
        })
        .collect())
}

/// The models a ChatGPT plan serves, from the Codex backend's own list.
fn chatgpt() -> Result<Vec<Raw>> {
    let t = crate::chatgpt::fresh().context("not signed in to ChatGPT")?;
    let v = get_json(
        "https://chatgpt.com/backend-api/codex/models?client_version=1.0.0",
        &[
            ("authorization", format!("Bearer {}", t.access)),
            ("chatgpt-account-id", t.account_id.clone()),
            ("originator", crate::NAME.into()),
        ],
    )?;
    let items = v["models"].as_array().or(v["data"].as_array()).cloned().unwrap_or_default();
    Ok(items
        .iter()
        .filter(|m| m["visibility"].as_str() != Some("hide"))
        .filter_map(|m| {
            let id = m["slug"].as_str().or(m["id"].as_str())?.to_string();
            Some(Raw {
                name: m["display_name"].as_str().map(str::to_string),
                context: u(&m["context_window"]),
                reasoning: true,
                id,
                ..Raw::default()
            })
        })
        .collect())
}

/// "2025-05-22T00:00:00Z" → 20250522 (sortable).
fn date_key(s: &str) -> i64 {
    s.chars().filter(|c| c.is_ascii_digit()).take(8).collect::<String>().parse().unwrap_or(0)
}

/// Not a chat model an agent can drive.
fn noise(id: &str) -> bool {
    const SKIP: &[&str] = &[
        "embed", "tts", "whisper", "dall-e", "davinci", "babbage", "moderation", "rerank", "image",
        "audio", "speech", "transcribe", "realtime", "sora", "guard", "-search", "computer-use", "veo",
        "imagen", "lyria", "aqa", "native-audio", "clip", "stable-diffusion", "flux",
    ];
    let l = id.to_ascii_lowercase();
    // OpenRouter's offline batch variants are no use to an interactive agent
    l.ends_with(":batch") || SKIP.iter().any(|s| l.contains(s))
}

/// Filter, enrich from the catalog and order best-first.
fn merge(def: &ProviderDef, raw: Vec<Raw>) -> Vec<ModelDef> {
    let mut seen = std::collections::HashSet::new();
    let keep_all = def.is_subscription();
    let mut raw: Vec<Raw> = raw
        .into_iter()
        .filter(|r| (keep_all || !noise(&r.id)) && seen.insert(r.id.clone()))
        .collect();
    let rank = |id: &str| def.models.iter().position(|m| m.id == id);
    raw.sort_by(|a, b| match (rank(&a.id), rank(&b.id)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => b.created.cmp(&a.created).then_with(|| a.id.cmp(&b.id)),
    });
    raw.into_iter()
        .map(|r| {
            let known = def.model(&r.id);
            ModelDef {
                name: r.name.or_else(|| known.map(|k| k.name.clone())).unwrap_or_else(|| r.id.clone()),
                context: r.context.or(known.map(|k| k.context)).unwrap_or(128_000),
                output: r.output.or(known.map(|k| k.output)).unwrap_or(8_192),
                reasoning: r.reasoning || known.is_some_and(|k| k.reasoning),
                price: match (r.price, known) {
                    (Some((i, o)), _) => [i, o, 0.0, 0.0],
                    (None, Some(k)) => k.price,
                    _ => [0.0; 4],
                },
                id: r.id,
            }
        })
        .collect()
}

// ── cache ───────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
struct Cache {
    #[serde(flatten)]
    providers: BTreeMap<String, Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    at: u64,
    models: Vec<ModelDef>,
}

fn cache_path() -> std::path::PathBuf {
    crate::cli_home().join("live_models.json")
}

fn load() -> Cache {
    let Ok(t) = std::fs::read_to_string(cache_path()) else {
        return Cache::default();
    };
    if let Ok(c) = serde_json::from_str::<Cache>(&t) {
        if !c.providers.is_empty() {
            return c;
        }
    }
    serde_json::from_str::<BTreeMap<String, Entry>>(&t)
        .map(|providers| Cache { providers })
        .unwrap_or_default()
}

/// The last live list fetched for `provider`, if any.
pub fn cached(provider: &str) -> Option<Vec<ModelDef>> {
    load().providers.remove(provider).map(|e| e.models).filter(|m| !m.is_empty())
}

/// Remember a live list so `/model` shows it too.
pub fn store(provider: &str, models: &[ModelDef]) -> Result<()> {
    let mut c = load();
    let at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    c.providers.insert(provider.to_string(), Entry { at, models: models.to_vec() });
    std::fs::create_dir_all(crate::cli_home())?;
    std::fs::write(cache_path(), serde_json::to_vec(&c)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> ProviderDef {
        crate::catalog::provider("openai").unwrap().clone()
    }

    #[test]
    fn merge_filters_noise_and_orders_catalog_first() {
        let d = def();
        let known = d.models[1].id.clone();
        let raw = vec![
            Raw { id: "text-embedding-3-large".into(), created: 9, ..Raw::default() },
            Raw { id: "gpt-old".into(), created: 1, ..Raw::default() },
            Raw { id: "gpt-brand-new".into(), created: 5, ..Raw::default() },
            Raw { id: known.clone(), ..Raw::default() },
            Raw { id: "whisper-1".into(), ..Raw::default() },
            Raw { id: "gpt-old".into(), created: 1, ..Raw::default() },
        ];
        let m = merge(&d, raw);
        let ids: Vec<&str> = m.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec![known.as_str(), "gpt-brand-new", "gpt-old"]);
        assert_eq!(m[0].context, d.models[1].context, "catalog metadata fills in");
        assert_eq!(m[1].context, 128_000);
    }

    #[test]
    fn openrouter_prices_become_per_million() {
        assert_eq!(per_m(&Value::from("0.000003")), Some(3.0));
        assert_eq!(per_m(&Value::from("0")), Some(0.0));
        assert_eq!(per_m(&Value::Null), None);
    }

    #[test]
    fn cache_round_trip() {
        let _env = crate::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("COSMOS_CLI_HOME", dir.path());
        assert!(cached("openai").is_none());
        let d = def();
        store("openai", &d.models[..2]).unwrap();
        assert_eq!(cached("openai").unwrap().len(), 2);
    }

    #[test]
    fn cache_reads_map_shaped_file() {
        let _env = crate::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("COSMOS_CLI_HOME", dir.path());
        std::fs::write(
            dir.path().join("live_models.json"),
            r#"{"devin":{"at":1,"models":[{"id":"swe-2-max","name":"SWE-2 Max","context":200000,"output":32000,"reasoning":true,"price":[0,0,0,0]}]}}"#,
        )
        .unwrap();
        let m = cached("devin").expect("map-shaped live_models.json");
        assert_eq!(m[0].id, "swe-2-max");
    }

    #[test]
    fn merge_keeps_subscription_ids() {
        let d = crate::catalog::provider("chatgpt").unwrap().clone();
        let m = merge(
            &d,
            vec![Raw { id: "foo-clip-bar".into(), name: Some("Clip".into()), ..Raw::default() }],
        );
        assert_eq!(m[0].id, "foo-clip-bar");
    }

    /// Hits the network: `cargo test -p zcore live -- --ignored`.
    #[test]
    #[ignore]
    fn openrouter_live_list() {
        let d = crate::catalog::provider("openrouter").unwrap();
        let m = fetch(d).unwrap();
        assert!(m.len() > 20, "{}", m.len());
        assert!(m.iter().any(|x| x.price[0] > 0.0));
        println!("{} models, first: {:?}", m.len(), m.iter().take(5).map(|x| &x.id).collect::<Vec<_>>());
    }
}
