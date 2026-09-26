//! File tools: read/write/edit with hashline anchors, ls, glob, grep.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{ckpt, resolve, ToolContext};

fn deny_write_path(ctx: &ToolContext, path: &std::path::Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    if ctx.sandbox_mode == "full" {
        return false;
    }
    s.contains("/.ssh/")
        || s.ends_with("/.ssh")
        || s.contains("/.gnupg/")
        || s.contains("/.env/")
        || s.ends_with("/.env")
        || s.contains(".env.")
}

fn checkpoint(ctx: &ToolContext, path: &std::path::Path) {
    if ctx.session_id.is_empty() {
        return;
    }
    let _ = ckpt::save(&ctx.home, &ctx.session_id, path);
}

fn sha6(line: &str) -> String {
    let mut h = Sha256::new();
    h.update(line.trim_end_matches(['\r', '\n']).as_bytes());
    let d = h.finalize();
    d[..3].iter().map(|b| format!("{b:02x}")).collect()
}

fn file_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Split preserving per-line endings info for later reconstruction.
fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

fn read_file(path: &PathBuf) -> Result<(String, String)> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let hash = file_sha256(&bytes);
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok((text, hash))
}

pub fn read(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let path = resolve(ctx, input["path"].as_str().unwrap_or(""))?;
    let (text, hash) = read_file(&path)?;
    let lines: Vec<&str> = split_lines(&text);
    // drop the phantom final element when file ends with newline
    let n = if lines.last() == Some(&"") && text.ends_with('\n') {
        lines.len() - 1
    } else {
        lines.len()
    };
    let start = input["start"].as_u64().unwrap_or(1).max(1) as usize;
    let end = input["end"]
        .as_u64()
        .map(|e| e as usize)
        .unwrap_or(n)
        .min(n);
    // large-read guard (contract §11 hook, enforced here too)
    if input["start"].is_null() && input["end"].is_null() && n > ctx.large_read_lines {
        bail!(
            "large_read_guard: {n} lines > {} limit — read with start/end ranges \
             (<=200 lines) or fetch the meridian cache.read summary",
            ctx.large_read_lines
        );
    }
    let mut out = format!("file_sha256={hash} lines={n}\n");
    for i in start..=end {
        if i < 1 || i > n {
            break;
        }
        let line = lines[i - 1];
        out.push_str(&format!("{i}:{}|{line}\n", sha6(line)));
    }
    ctx.read_state
        .insert(path.to_string_lossy().to_string(), hash);
    Ok(out.trim_end_matches('\n').to_string())
}

/// Parse an anchor `n:hhhhhh`.
fn parse_anchor(s: &str) -> Option<(usize, String)> {
    let (n, h) = s.split_once(':')?;
    Some((n.parse().ok()?, h.to_string()))
}

fn fresh_anchors(lines: &[&str], around: usize, n: usize) -> String {
    let lo = around.saturating_sub(5).max(1);
    let hi = (around + 5).min(n);
    let mut out = String::new();
    for i in lo..=hi {
        let l = lines.get(i - 1).copied().unwrap_or("");
        out.push_str(&format!("{i}:{}|{l}\n", sha6(l)));
    }
    out
}

pub fn edit(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let path = resolve(ctx, input["path"].as_str().unwrap_or(""))?;
    if deny_write_path(ctx, &path) {
        bail!("sandbox deny: write to {} blocked", path.display());
    }
    if ctx.sandbox_mode == "read-only" {
        bail!("sandbox read-only: edit denied");
    }
    let (text, hash) = read_file(&path)?;
    if let Some(want) = input["file_sha256"].as_str() {
        if want != hash {
            bail!("STALE: file_sha256 mismatch (want {want}, have {hash})");
        }
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let lines: Vec<&str> = split_lines(&text);
    let n = lines.len();
    let edits = input["edits"].as_array().cloned().unwrap_or_default();

    // validate ALL anchors before mutating (contract: whole call rejected)
    struct Pending {
        start: usize,
        end: usize,
        new_text: String,
    }
    let mut pending = Vec::new();
    let mut stale = String::new();
    for e in &edits {
        let (s_n, s_h) = parse_anchor(e["start"].as_str().unwrap_or(""))
            .with_context(|| format!("bad start anchor: {}", e["start"]))?;
        let (e_n, e_h) = parse_anchor(e["end"].as_str().unwrap_or(""))
            .with_context(|| format!("bad end anchor: {}", e["end"]))?;
        let ok = s_n >= 1
            && e_n >= s_n
            && e_n <= n
            && sha6(lines[s_n - 1]) == s_h
            && sha6(lines[e_n - 1]) == e_h;
        if !ok {
            stale.push_str(&format!(
                "anchor mismatch near line {s_n}..{e_n}:\n{}\n",
                fresh_anchors(&lines, s_n, n)
            ));
        } else {
            pending.push(Pending {
                start: s_n,
                end: e_n,
                new_text: e["new_text"].as_str().unwrap_or("").to_string(),
            });
        }
    }
    if !stale.is_empty() {
        bail!("STALE\n{stale}");
    }

    checkpoint(ctx, &path);

    // apply bottom-up so earlier line numbers stay valid
    let mut owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    pending.sort_by_key(|p| std::cmp::Reverse(p.start));
    for p in pending {
        let mut repl: Vec<String> = p.new_text.split('\n').map(|s| s.to_string()).collect();
        if repl.last() == Some(&String::new()) && !p.new_text.is_empty() {
            repl.pop();
        }
        owned.splice(p.start - 1..p.end, repl);
    }
    let mut body = owned.join(newline);
    if text.ends_with(newline) && !body.ends_with(newline) {
        body.push_str(newline);
    }
    atomic_write(&path, body.as_bytes())?;
    Ok(format!("ok {path}", path = path.display()))
}

pub fn write(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let path = resolve(ctx, input["path"].as_str().unwrap_or(""))?;
    if deny_write_path(ctx, &path) {
        bail!("sandbox deny: write to {} blocked", path.display());
    }
    if ctx.sandbox_mode == "read-only" {
        bail!("sandbox read-only: write denied");
    }
    let content = input["content"].as_str().unwrap_or("");
    if path.exists() {
        let (_, hash) = read_file(&path)?;
        let key = path.to_string_lossy().to_string();
        match ctx.read_state.get(&key) {
            Some(h) if *h == hash => {}
            _ => bail!(
                "STALE: {} exists and was not read this session (or changed)",
                path.display()
            ),
        }
    }
    if path.exists() {
        checkpoint(ctx, &path);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    atomic_write(&path, content.as_bytes())?;
    let (_, hash) = read_file(&path)?;
    ctx.read_state
        .insert(path.to_string_lossy().to_string(), hash);
    Ok(format!("ok {}", path.display()))
}

fn atomic_write(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("ztmp{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, path).or_else(|_| {
        fs::write(path, bytes)?;
        fs::remove_file(&tmp)
    })?;
    Ok(())
}

pub fn ls(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let path = resolve(ctx, input["path"].as_str().unwrap_or("."))?;
    let mut entries: Vec<String> = fs::read_dir(&path)
        .with_context(|| format!("ls {}", path.display()))?
        .filter_map(|e| e.ok())
        .map(|e| {
            let mut n = e.file_name().to_string_lossy().to_string();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                n.push('/');
            }
            n
        })
        .collect();
    entries.sort();
    Ok(entries.join("\n"))
}

pub fn glob(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let pattern = input["pattern"].as_str().unwrap_or("*");
    let gs = globset::GlobBuilder::new(pattern)
        .literal_separator(false)
        .build()?
        .compile_matcher();
    let mut out = Vec::new();
    for entry in ignore::WalkBuilder::new(&ctx.workspace)
        .hidden(false)
        .build()
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(&ctx.workspace) {
            if gs.is_match(rel) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        if out.len() >= 500 {
            break;
        }
    }
    out.sort();
    Ok(out.join("\n"))
}

pub fn grep(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let pattern = input["pattern"].as_str().unwrap_or("");
    let re = regex::Regex::new(pattern).with_context(|| format!("bad regex: {pattern}"))?;
    let root = input["path"]
        .as_str()
        .map(|p| resolve(ctx, p))
        .transpose()?
        .unwrap_or_else(|| ctx.workspace.clone());
    let gset = input["glob"].as_str().and_then(|g| {
        globset::GlobBuilder::new(g)
            .literal_separator(false)
            .build()
            .ok()
            .map(|b| b.compile_matcher())
    });
    let mut out = Vec::new();
    for entry in ignore::WalkBuilder::new(&root).hidden(false).build() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let p = entry.path();
        if let Some(gs) = &gset {
            if !gs.is_match(p) {
                continue;
            }
        }
        if let Ok(text) = fs::read_to_string(p) {
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    let rel = p
                        .strip_prefix(&ctx.workspace)
                        .unwrap_or(p)
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push(format!("{rel}:{}:{line}", i + 1));
                    if out.len() >= 200 {
                        return Ok(out.join("\n") + "\n[cap: 200 matches]");
                    }
                }
            }
        }
    }
    Ok(out.join("\n"))
}
