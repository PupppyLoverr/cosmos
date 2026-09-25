//! Memory bullets: append-only markdown log under `memory/<scope>/<YYYY-MM>.md`.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Local;
use ulid::Ulid;

use crate::secrets;
use crate::vault::Vault;

pub const MEMORY_TYPES: &[&str] = &["decision", "preference", "map", "correction", "playbook"];

/// One parsed memory bullet.
#[derive(Debug, Clone)]
pub struct MemoryEntry {
    pub id: String,
    pub date: String,
    pub mtype: String,
    pub scope: String,
    pub text: String,
    pub src: String,
}

/// Append a memory bullet. Takes a lock file, writes a temp file, renames.
/// Returns the new entry id.
pub fn write(vault: &Vault, mtype: &str, text: &str, scope: &str, src: &str) -> Result<String> {
    if !MEMORY_TYPES.contains(&mtype) {
        bail!("unknown memory type '{mtype}'; expected one of {MEMORY_TYPES:?}");
    }
    if let Err(reason) = secrets::scan(text) {
        bail!("rejected: {reason}");
    }
    let id = Ulid::new().to_string();
    let date = Local::now().format("%Y-%m-%d").to_string();
    let month = Local::now().format("%Y-%m").to_string();
    let file = vault.memory_file(scope, &month);
    fs::create_dir_all(file.parent().unwrap())?;

    let line = format!("- {date} [{mtype}] [{scope}] {} <!-- id:{id} src:{src} -->\n", text.trim());

    // Lock file: spin until acquired (bounded).
    let lock = file.with_extension("md.lock");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if Instant::now() > deadline {
                    bail!("memory lock timeout on {}", lock.display());
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) => return Err(e).context("opening memory lock"),
        }
    }
    let result = append_atomic(&file, &line);
    let _ = fs::remove_file(&lock);
    result?;
    Ok(id)
}

/// Read current content, append the line, write temp + rename.
fn append_atomic(file: &PathBuf, line: &str) -> Result<()> {
    let mut existing = String::new();
    if file.exists() {
        let mut f = fs::File::open(file)?;
        f.read_to_string(&mut existing)?;
    }
    let mut out = existing;
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(line);
    let tmp = file.with_extension("md.tmp");
    {
        let mut w = fs::File::create(&tmp)?;
        w.write_all(out.as_bytes())?;
        w.sync_all()?;
    }
    fs::rename(&tmp, file)?;
    Ok(())
}

/// Parse all memory bullets in the vault.
pub fn all_entries(vault: &Vault) -> Vec<MemoryEntry> {
    let mut out = Vec::new();
    collect_dir(&vault.memory_dir(), &mut out);
    out
}

fn collect_dir(dir: &PathBuf, out: &mut Vec<MemoryEntry>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_dir(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("md") {
            if let Ok(text) = fs::read_to_string(&p) {
                for line in text.lines() {
                    if let Some(entry) = parse_line(line) {
                        out.push(entry);
                    }
                }
            }
        }
    }
}

/// Parse `- 2026-09-22 [decision] [repo:cosmos] text <!-- id:X src:y/z -->`.
fn parse_line(line: &str) -> Option<MemoryEntry> {
    let line = line.trim();
    if !line.starts_with("- ") {
        return None;
    }
    let comment_start = line.find("<!--")?;
    let comment_end = line[comment_start..].find("-->")? + comment_start;
    let comment = &line[comment_start + 4..comment_end];
    let mut id = "";
    let mut src = "";
    for kv in comment.split_whitespace() {
        if let Some(v) = kv.strip_prefix("id:") {
            id = v;
        } else if let Some(v) = kv.strip_prefix("src:") {
            src = v;
        }
    }
    let head = &line[2..comment_start.min(line.len())];
    // head: "DATE [type] [scope] text"
    let date = head.get(..10)?.trim().to_string();
    let rest = head.get(10..)?.trim_start();
    let t0 = rest.strip_prefix('[')?;
    let t_end = t0.find(']')?;
    let mtype = t0[..t_end].to_string();
    let rest2 = t0[t_end + 1..].trim_start();
    let s0 = rest2.strip_prefix('[')?;
    let s_end = s0.find(']')?;
    let scope = s0[..s_end].to_string();
    let text = s0[s_end + 1..].trim().to_string();
    Some(MemoryEntry {
        id: id.to_string(),
        date,
        mtype,
        scope,
        text,
        src: src.to_string(),
    })
}

/// Playbook bodies for `playbook.get` and search indexing.
pub fn playbook(vault: &Vault, name: &str) -> Result<String> {
    let name = name.trim_matches(|c: char| c == '/' || c == '\\');
    let p = vault.playbooks_dir().join(format!("{name}.md"));
    let p = if p.exists() {
        p
    } else {
        // allow nested names like "blueprint-init"
        vault.playbooks_dir().join(name).with_extension("md")
    };
    if !p.exists() {
        bail!("no playbook named '{name}'");
    }
    Ok(fs::read_to_string(&p)?)
}

/// Register a session pointer card. Never stores the transcript.
pub fn register_session(
    vault: &Vault,
    session_id: &str,
    harness: &str,
    log_path: &str,
    title: &str,
) -> Result<PathBuf> {
    let sid = session_id.replace(['/', '\\'], "_");
    let p = vault.sessions_dir().join(format!("{sid}.md"));
    let today = Local::now().format("%Y-%m-%d").to_string();
    let body = format!(
        "---\nsession: {session_id}\nharness: {harness}\nlog: {log_path}\ncreated: {today}\n---\n\n# {title}\n\nSession pointer only — the transcript lives in the harness JSONL log.\n"
    );
    let tmp = p.with_extension("md.tmp");
    fs::write(&tmp, body)?;
    fs::rename(&tmp, &p)?;
    Ok(p)
}
