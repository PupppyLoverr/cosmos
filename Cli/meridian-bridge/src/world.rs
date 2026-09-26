//! `world.upsert` / `world.get` — per-session world.md in the vault.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;

use crate::tokens;
use crate::vault::Vault;

pub fn path(vault: &Vault, session_id: &str) -> PathBuf {
    let sid = session_id.replace(['/', '\\'], "_");
    vault.root().join("world").join(format!("{sid}.md"))
}

pub fn upsert(vault: &Vault, session_id: &str, markdown: &str) -> Result<PathBuf> {
    let p = path(vault, session_id);
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension("md.tmp");
    fs::write(&tmp, markdown)?;
    fs::rename(&tmp, &p)?;
    Ok(p)
}

pub fn get(vault: &Vault, session_id: &str) -> Result<(String, usize)> {
    let p = path(vault, session_id);
    let md = if p.is_file() {
        fs::read_to_string(p)?
    } else {
        String::new()
    };
    let n = tokens::count(&md);
    Ok((md, n))
}
