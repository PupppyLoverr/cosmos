//! `refine.propose` / `apply` / `rollback` — bounded skill/playbook edits.

use std::fs;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use ulid::Ulid;

use crate::vault::Vault;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefineItem {
    pub id: String,
    pub target: String,
    pub diff: String,
    pub evidence: String,
    pub applied: bool,
    pub previous: Option<String>,
}

fn file(vault: &Vault, id: &str) -> std::path::PathBuf {
    vault.root().join("refine").join(format!("{id}.json"))
}

pub fn propose(vault: &Vault, target: &str, diff: &str, evidence: &str) -> Result<String> {
    fs::create_dir_all(vault.root().join("refine"))?;
    let id = Ulid::new().to_string().to_lowercase();
    let item = RefineItem {
        id: id.clone(),
        target: target.into(),
        diff: diff.into(),
        evidence: evidence.into(),
        applied: false,
        previous: None,
    };
    fs::write(file(vault, &id), serde_json::to_string_pretty(&item)?)?;
    Ok(id)
}

pub fn apply(vault: &Vault, id: &str) -> Result<serde_json::Value> {
    let p = file(vault, id);
    if !p.is_file() {
        bail!("no refine id {id}");
    }
    let mut item: RefineItem = serde_json::from_str(&fs::read_to_string(&p)?)?;
    let target = std::path::PathBuf::from(&item.target);
    let previous = if target.is_file() {
        Some(fs::read_to_string(&target)?)
    } else {
        None
    };
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    // diff is treated as the full replacement body when it doesn't start with ---
    let body = if item.diff.starts_with("***") || item.diff.contains("@@") {
        previous.clone().unwrap_or_default() + "\n" + &item.diff
    } else {
        item.diff.clone()
    };
    fs::write(&target, body)?;
    item.applied = true;
    item.previous = previous;
    fs::write(&p, serde_json::to_string_pretty(&item)?)?;
    Ok(json!({"id": id, "applied": true, "target": item.target}))
}

pub fn rollback(vault: &Vault, id: &str) -> Result<serde_json::Value> {
    let p = file(vault, id);
    if !p.is_file() {
        bail!("no refine id {id}");
    }
    let mut item: RefineItem = serde_json::from_str(&fs::read_to_string(&p)?)?;
    let target = std::path::PathBuf::from(&item.target);
    match &item.previous {
        Some(prev) => fs::write(&target, prev)?,
        None => {
            let _ = fs::remove_file(&target);
        }
    }
    item.applied = false;
    fs::write(&p, serde_json::to_string_pretty(&item)?)?;
    Ok(json!({"id": id, "rolled_back": true}))
}
