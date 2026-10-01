//! Local memory — no bridge process, no network.
//!
//! Append-only markdown bullets, one fact per line, searched with BM25:
//!
//! ```text
//! <memory dir>/<scope>/<YYYY-MM>.md
//! - 2026-10-01 [preference] [repo:cosmos] prefer squash merges <!-- id:<ulid> src:cosmostui/<session> -->
//! ```
//!
//! The format is the Meridian vault's, so zagy/zdev/meridian read the same
//! files when pointed at the same place. Memory dir, first match:
//! `COSMOS_MEMORY_DIR`, `$MERIDIAN_VAULT/memory` (opt-in sharing with the
//! other harnesses), `COSMOS_CLI_HOME/memory`.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::ToolContext;

pub const TYPES: &[&str] = &["decision", "preference", "map", "correction", "playbook"];

/// One parsed bullet.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub date: String,
    pub kind: String,
    pub scope: String,
    pub text: String,
    pub src: String,
}

pub fn dir(home: &Path) -> PathBuf {
    if let Some(d) = std::env::var_os("COSMOS_MEMORY_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(v) = std::env::var_os("MERIDIAN_VAULT").filter(|v| !v.is_empty()) {
        return PathBuf::from(v).join("memory");
    }
    home.join("memory")
}

/// Keep a scope usable as one path component (`repo:cosmos` → `repo_cosmos`;
/// the bullet keeps the original spelling).
fn component(scope: &str) -> String {
    let s: String = scope
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    let s = s.trim_matches('.').to_string();
    if s.is_empty() || s == ".." {
        "misc".into()
    } else {
        s
    }
}

// ---- secret scan --------------------------------------------------------------

const DENY: &[(&str, &str)] = &[
    ("sk-", "contains sk- token marker"),
    ("xai-", "contains xai- token marker"),
    ("AKIA", "contains AWS access-key marker"),
    ("ghp_", "contains GitHub PAT marker"),
    ("github_pat_", "contains GitHub PAT marker"),
    ("-----BEGIN", "contains PEM block"),
    ("Bearer ", "contains Bearer credential"),
];

fn entropy(s: &str) -> f64 {
    let mut counts = [0u32; 256];
    for b in s.bytes() {
        counts[b as usize] += 1;
    }
    let n = s.len().max(1) as f64;
    counts.iter().filter(|&&c| c > 0).map(|&c| {
        let p = c as f64 / n;
        -p * p.log2()
    }).sum()
}

/// `Err(reason)` when `text` looks like it carries a credential.
pub fn scan(text: &str) -> std::result::Result<(), String> {
    for (needle, reason) in DENY {
        if text.contains(needle) {
            return Err((*reason).to_string());
        }
    }
    for word in text.split(|c: char| c.is_whitespace() || c == '"') {
        let w = word.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || "+/=_-".contains(c)));
        if w.len() >= 32
            && w.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'=' | b'-' | b'_'))
            && entropy(w) > 4.0
        {
            return Err(format!("contains a {}-char high-entropy token", w.len()));
        }
    }
    Ok(())
}

// ---- write / read ---------------------------------------------------------------

/// Append a bullet (lock file + temp + rename). Returns the new id.
pub fn add(home: &Path, kind: &str, text: &str, scope: &str, src: &str) -> Result<String> {
    if !TYPES.contains(&kind) {
        bail!("unknown memory type '{kind}'; expected one of {}", TYPES.join("|"));
    }
    let text = text.trim().replace('\n', " ");
    if text.is_empty() {
        bail!("empty memory text");
    }
    if let Err(reason) = scan(&text) {
        bail!("rejected: {reason}");
    }
    let id = zproto::new_id();
    let ts = zproto::now_ts();
    let (date, month) = (&ts[..10], &ts[..7]);
    let file = dir(home).join(component(scope)).join(format!("{month}.md"));
    fs::create_dir_all(file.parent().unwrap())?;
    let line = format!("- {date} [{kind}] [{scope}] {text} <!-- id:{id} src:{src} -->\n");
    let lock = file.with_extension("md.lock");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // a writer that died leaves its lock behind — steal it after 10 s
                if Instant::now() > deadline {
                    let _ = fs::remove_file(&lock);
                    continue;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) => return Err(e).context("opening memory lock"),
        }
    }
    let r = (|| -> Result<()> {
        let mut out = fs::read_to_string(&file).unwrap_or_default();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&line);
        let tmp = file.with_extension("md.tmp");
        fs::File::create(&tmp)?.write_all(out.as_bytes())?;
        fs::rename(&tmp, &file)?;
        Ok(())
    })();
    let _ = fs::remove_file(&lock);
    r?;
    Ok(id)
}

/// Parse `- DATE [type] [scope] text <!-- id:X src:Y -->`.
pub fn parse(line: &str) -> Option<Entry> {
    let line = line.trim().strip_prefix("- ")?;
    let c0 = line.find("<!--")?;
    let c1 = line[c0..].find("-->")? + c0;
    let (mut id, mut src) = ("", "");
    for kv in line[c0 + 4..c1].split_whitespace() {
        if let Some(v) = kv.strip_prefix("id:") {
            id = v;
        } else if let Some(v) = kv.strip_prefix("src:") {
            src = v;
        }
    }
    let head = &line[..c0];
    let date = head.get(..10)?.to_string();
    let rest = head.get(10..)?.trim_start().strip_prefix('[')?;
    let (kind, rest) = rest.split_once(']')?;
    let rest = rest.trim_start().strip_prefix('[')?;
    let (scope, text) = rest.split_once(']')?;
    Some(Entry {
        id: id.into(),
        date,
        kind: kind.into(),
        scope: scope.into(),
        text: text.trim().into(),
        src: src.into(),
    })
}

/// Every bullet, oldest file first.
pub fn all(home: &Path) -> Vec<Entry> {
    fn walk(d: &Path, out: &mut Vec<Entry>) {
        let Ok(rd) = fs::read_dir(d) else { return };
        let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "md") {
                if let Ok(t) = fs::read_to_string(&p) {
                    out.extend(t.lines().filter_map(parse));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&dir(home), &mut out);
    out
}

// ---- BM25 ----------------------------------------------------------------------

fn terms(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_lowercase).collect()
}

/// BM25 over bullets (text + type + scope). `scope` "" / "memory" = all.
pub fn search(home: &Path, query: &str, k: usize, scope: &str) -> Vec<(f64, Entry)> {
    let docs: Vec<(Vec<String>, Entry)> = all(home)
        .into_iter()
        .filter(|e| scope.is_empty() || scope == "memory" || e.scope == scope)
        .map(|e| (terms(&format!("{} {} {}", e.text, e.kind, e.scope)), e))
        .collect();
    let q = terms(query);
    if docs.is_empty() || q.is_empty() {
        return Vec::new();
    }
    let (k1, b) = (1.2, 0.75);
    let n = docs.len() as f64;
    let avg = docs.iter().map(|d| d.0.len() as f64).sum::<f64>() / n;
    let df: HashMap<&str, f64> = q
        .iter()
        .map(|t| (t.as_str(), docs.iter().filter(|d| d.0.contains(t)).count() as f64))
        .collect();
    let mut hits: Vec<(f64, Entry)> = docs
        .into_iter()
        .filter_map(|(ts, e)| {
            let dl = ts.len() as f64;
            let s: f64 = q
                .iter()
                .map(|t| {
                    let f = ts.iter().filter(|x| *x == t).count() as f64;
                    if f == 0.0 {
                        return 0.0;
                    }
                    let d = df[t.as_str()];
                    let idf = ((n - d + 0.5) / (d + 0.5) + 1.0).ln();
                    idf * f * (k1 + 1.0) / (f + k1 * (1.0 - b + b * dl / avg.max(1e-9)))
                })
                .sum();
            (s > 0.0).then_some((s, e))
        })
        .collect();
    hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    hits.truncate(k);
    hits
}

/// Newest bullets of the given types, newest first — what boot carries.
pub fn recent(home: &Path, kinds: &[&str], scopes: &[&str], n: usize) -> Vec<Entry> {
    let mut v: Vec<Entry> = all(home)
        .into_iter()
        .filter(|e| kinds.contains(&e.kind.as_str()) && (scopes.is_empty() || scopes.contains(&e.scope.as_str())))
        .collect();
    // ids are time-ordered ULIDs; dates break ties for hand-written bullets
    v.sort_by(|a, b| (b.date.as_str(), b.id.as_str()).cmp(&(a.date.as_str(), a.id.as_str())));
    // the same fact written twice is shown once
    let mut seen = std::collections::HashSet::new();
    v.retain(|e| seen.insert(e.text.to_lowercase()));
    v.truncate(n);
    v
}

// ---- tools ------------------------------------------------------------------------

pub fn search_tool(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let q = input["query"].as_str().unwrap_or("");
    let k = input["k"].as_u64().unwrap_or(5).clamp(1, 50) as usize;
    let scope = input["scope"].as_str().unwrap_or("memory");
    let mut results: Vec<Value> = search(&ctx.home, q, k, scope)
        .into_iter()
        .map(|(score, e)| json!({"id": e.id, "text": e.text, "type": e.kind, "scope": e.scope, "date": e.date, "score": (score * 1000.0).round() / 1000.0}))
        .collect();
    if let Some(remote) = crate::supermemory::search(q, k) {
        let have: std::collections::HashSet<String> = results.iter().filter_map(|r| r["text"].as_str().map(str::to_lowercase)).collect();
        results.extend(remote.into_iter().filter(|r| r["text"].as_str().is_some_and(|t| !have.contains(&t.to_lowercase()))));
        results.truncate(k);
    }
    Ok(serde_json::to_string(&json!({"results": results}))?)
}

pub fn write_tool(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let Some(kind) = input["type"].as_str() else {
        bail!("missing required field: type ({})", TYPES.join("|"));
    };
    let text = input["text"].as_str().unwrap_or("");
    let scope = input["scope"].as_str().filter(|s| !s.is_empty()).unwrap_or("global");
    let src = format!("cosmostui/{}", ctx.session_id);
    let id = add(&ctx.home, kind, text, scope, &src)?;
    crate::supermemory::add(kind, text, scope);
    Ok(serde_json::to_string(&json!({"id": id}))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_parse_search_round_trip() {
        let home = tempfile::tempdir().unwrap();
        std::env::remove_var("COSMOS_MEMORY_DIR");
        std::env::remove_var("MERIDIAN_VAULT");
        let h = home.path();
        add(h, "preference", "prefer squash merges for release branches", "repo:cosmos", "t/1").unwrap();
        add(h, "decision", "the tokenizer is chars/4 to stay small", "global", "t/2").unwrap();
        add(h, "correction", "run cargo test with --offline on this box", "global", "t/3").unwrap();
        assert!(add(h, "note", "x", "global", "t").is_err(), "unknown type");
        assert!(add(h, "decision", "key sk-abc123", "global", "t").is_err(), "secret");
        let on_disk = fs::read_dir(dir(h).join("repo_cosmos")).unwrap().count();
        assert_eq!(on_disk, 1, "scope sanitised to one path component");
        let hits = search(h, "squash merge release", 5, "memory");
        assert_eq!(hits[0].1.text, "prefer squash merges for release branches");
        assert_eq!(hits[0].1.scope, "repo:cosmos");
        assert!(search(h, "squash", 5, "global").is_empty(), "scope filter");
        let r = recent(h, &["preference", "correction"], &[], 5);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].kind, "correction", "newest first");
    }

    #[test]
    fn parses_meridian_bullets() {
        let e = parse("- 2026-09-22 [decision] [repo:cosmos] use AppContainer <!-- id:01ABC src:zagy/s1 -->").unwrap();
        assert_eq!((e.date.as_str(), e.kind.as_str(), e.scope.as_str(), e.text.as_str(), e.id.as_str(), e.src.as_str()),
            ("2026-09-22", "decision", "repo:cosmos", "use AppContainer", "01ABC", "zagy/s1"));
        assert!(parse("# heading").is_none());
    }

    #[test]
    fn scanner_accepts_prose_rejects_tokens() {
        assert!(scan("prefer squash merges").is_ok());
        assert!(scan("commit aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").is_ok());
        assert!(scan("token Zj3kQ9mXv2Lp8Rt5Yw7Nb4Cd6Fg1Hj0Ks3Mn5Bv8Cx2Zq4We").is_err());
    }
}
