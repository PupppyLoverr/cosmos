//! BM25 over memory bullets and playbook headings. No embeddings, no network.

use std::collections::HashMap;
use std::fs;

use serde::Serialize;

use crate::memory;
use crate::vault::Vault;

const K1: f64 = 1.2;
const B: f64 = 0.75;

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub id: String,
    pub text: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub date: String,
    pub score: f64,
}

/// A searchable document: a memory bullet or a playbook heading.
struct Doc {
    id: String,
    text: String,
    kind: String,
    date: String,
    terms: Vec<String>,
}

fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

fn collect_docs(vault: &Vault, scope: Option<&str>) -> Vec<Doc> {
    let mut docs = Vec::new();
    for e in memory::all_entries(vault) {
        if let Some(s) = scope {
            if !s.is_empty() && e.scope != s {
                continue;
            }
        }
        docs.push(Doc {
            id: e.id.clone(),
            text: e.text.clone(),
            kind: e.mtype.clone(),
            date: e.date.clone(),
            terms: tokenize(&format!("{} {} {}", e.text, e.mtype, e.scope)),
        });
    }
    // Playbook headings count as searchable docs.
    if let Ok(rd) = fs::read_dir(vault.playbooks_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&p) else { continue };
            let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            for line in text.lines() {
                let h = line.trim_start_matches('#').trim();
                if line.trim_start().starts_with('#') && !h.is_empty() {
                    docs.push(Doc {
                        id: format!("playbook:{name}"),
                        text: h.to_string(),
                        kind: "playbook".to_string(),
                        date: String::new(),
                        terms: tokenize(&format!("{h} {name}")),
                    });
                    break; // first heading only
                }
            }
        }
    }
    docs
}

/// BM25 search. `seen_ids` are dropped before ranking (map-reduce dedup).
pub fn search(
    vault: &Vault,
    query: &str,
    k: usize,
    scope: Option<&str>,
    seen_ids: &[String],
) -> Vec<SearchHit> {
    let docs = collect_docs(vault, scope);
    let docs: Vec<&Doc> = docs.iter().filter(|d| !seen_ids.contains(&d.id)).collect();
    if docs.is_empty() {
        return Vec::new();
    }
    let qterms = tokenize(query);
    if qterms.is_empty() {
        return Vec::new();
    }
    let n = docs.len() as f64;
    let avgdl = docs.iter().map(|d| d.terms.len() as f64).sum::<f64>() / n.max(1.0);

    // document frequency per query term
    let mut df: HashMap<&str, usize> = HashMap::new();
    for qt in &qterms {
        let mut c = 0;
        for d in &docs {
            if d.terms.iter().any(|t| t == qt) {
                c += 1;
            }
        }
        df.insert(qt.as_str(), c);
    }

    let mut scored: Vec<(f64, &Doc)> = Vec::new();
    for d in &docs {
        let mut tf: HashMap<&str, usize> = HashMap::new();
        for t in &d.terms {
            *tf.entry(t.as_str()).or_insert(0) += 1;
        }
        let mut score = 0.0;
        for qt in &qterms {
            let freq = *tf.get(qt.as_str()).unwrap_or(&0) as f64;
            if freq == 0.0 {
                continue;
            }
            let dfv = *df.get(qt.as_str()).unwrap_or(&0) as f64;
            let idf = ((n - dfv + 0.5) / (dfv + 0.5) + 1.0).ln();
            let dl = d.terms.len() as f64;
            score += idf * (freq * (K1 + 1.0)) / (freq + K1 * (1.0 - B + B * dl / avgdl.max(1e-9)));
        }
        if score > 0.0 {
            scored.push((score, d));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(k)
        .map(|(score, d)| SearchHit {
            id: d.id.clone(),
            text: d.text.clone(),
            kind: d.kind.clone(),
            date: d.date.clone(),
            score,
        })
        .collect()
}
