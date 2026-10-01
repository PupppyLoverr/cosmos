//! Optional Supermemory mirror (https://supermemory.ai). Off unless
//! `SUPERMEMORY_API_KEY` is set — without it no socket is ever opened and
//! memory is purely local. With it, `memory_write` also adds the bullet
//! (in the background) and `memory_search` merges remote hits after the
//! local BM25 ones; any error or non-200 just means "local only".
//!
//! The free plan ($0/month) covers 1M tokens and 10K searches a month.

use std::time::Duration;

use serde_json::{json, Value};

const BASE: &str = "https://api.supermemory.ai";
/// Every cosmos memory lives under one container tag.
const TAG: &str = "cosmos";

fn key() -> Option<String> {
    std::env::var("SUPERMEMORY_API_KEY").ok().filter(|k| !k.trim().is_empty())
}

pub fn enabled() -> bool {
    key().is_some()
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(4)))
        .http_status_as_error(false)
        .build()
        .into()
}

pub fn add_body(kind: &str, text: &str, scope: &str) -> Value {
    json!({
        "content": text,
        "containerTag": TAG,
        "metadata": {"type": kind, "scope": scope, "source": "cosmos"},
    })
}

/// Mirror one bullet. Returns at once; the request runs on its own thread.
pub fn add(kind: &str, text: &str, scope: &str) {
    let Some(k) = key() else { return };
    let body = add_body(kind, text, scope);
    std::thread::spawn(move || {
        let _ = agent()
            .post(&format!("{BASE}/v3/documents"))
            .header("authorization", &format!("Bearer {k}"))
            .send_json(&body);
    });
}

/// Remote hits as `{id, text, type, scope, score, source:"supermemory"}`.
/// `None` when disabled or the call fails.
pub fn search(query: &str, k: usize) -> Option<Vec<Value>> {
    let key = key()?;
    let mut resp = agent()
        .post(&format!("{BASE}/v4/search"))
        .header("authorization", &format!("Bearer {key}"))
        .send_json(&json!({"q": query, "limit": k, "containerTag": TAG}))
        .ok()?;
    if resp.status().as_u16() != 200 {
        return None;
    }
    let v: Value = resp.body_mut().read_json().ok()?;
    Some(parse_results(&v))
}

pub fn parse_results(v: &Value) -> Vec<Value> {
    v["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    let text = r["memory"].as_str().or(r["chunk"].as_str())?;
                    Some(json!({
                        "id": r["id"],
                        "text": text,
                        "type": r["metadata"]["type"].as_str().unwrap_or("map"),
                        "scope": r["metadata"]["scope"].as_str().unwrap_or("global"),
                        "score": r["similarity"],
                        "source": "supermemory",
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_without_key_and_shapes_requests() {
        std::env::remove_var("SUPERMEMORY_API_KEY");
        assert!(!enabled());
        assert!(search("anything", 3).is_none(), "no key → no request");
        let b = add_body("preference", "tabs over spaces", "repo:x");
        assert_eq!(b["containerTag"], "cosmos");
        assert_eq!(b["metadata"]["scope"], "repo:x");
        let hits = parse_results(&json!({"results": [
            {"id": "m1", "memory": "tabs over spaces", "similarity": 0.91, "metadata": {"type": "preference", "scope": "repo:x"}},
            {"id": "c1", "chunk": "a doc chunk", "similarity": 0.5},
            {"id": "x"}
        ]}));
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0]["text"], "tabs over spaces");
        assert_eq!(hits[1]["type"], "map");
    }
}
