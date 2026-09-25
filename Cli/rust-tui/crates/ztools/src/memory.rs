//! memory_* tools → meridian bridge binary (`meridian` / meridian.exe).
//! The bridge is the single writer; all harnesses share MERIDIAN_VAULT.

use std::path::PathBuf;

use anyhow::{bail, Result};
use serde_json::Value;

use crate::ToolContext;

fn meridian_bin(ctx: &ToolContext) -> Option<PathBuf> {
    if let Some(b) = &ctx.meridian_bin {
        return Some(b.clone());
    }
    if let Ok(p) = std::env::var("MERIDIAN_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["meridian.exe", "meridian"] {
                let cand = dir.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for d in std::env::split_paths(&path) {
            for name in ["meridian.exe", "meridian"] {
                let cand = d.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

fn run_bridge(ctx: &ToolContext, args: &[String]) -> Result<String> {
    let Some(bin) = meridian_bin(ctx) else {
        bail!("meridian bridge binary not found (set MERIDIAN_BIN)")
    };
    let mut cmd = std::process::Command::new(bin);
    cmd.args(args);
    if let Some(v) = &ctx.meridian_vault {
        cmd.env("MERIDIAN_VAULT", v);
    } else if let Ok(v) = std::env::var("MERIDIAN_VAULT") {
        cmd.env("MERIDIAN_VAULT", v);
    }
    let out = cmd.output()?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        bail!(
            "meridian {}: {stderr}",
            args.first().map(|s| s.as_str()).unwrap_or("")
        );
    }
    Ok(stdout)
}

pub fn search(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let q = input["query"].as_str().unwrap_or("");
    let k = input["k"].as_u64().unwrap_or(5).to_string();
    let out = run_bridge(
        ctx,
        &["search".into(), "--query".into(), q.into(), "--k".into(), k],
    )?;
    Ok(out.trim().to_string())
}

pub fn write(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let Some(ty) = input["type"].as_str() else {
        bail!("missing required field: type (decision|preference|map|correction|playbook)");
    };
    let text = input["text"].as_str().unwrap_or("");
    let scope = input["scope"].as_str().unwrap_or("global");
    let out = run_bridge(
        ctx,
        &[
            "write".into(),
            "--type".into(),
            ty.into(),
            "--scope".into(),
            scope.into(),
            "--text".into(),
            text.into(),
        ],
    )?;
    Ok(out.trim().to_string())
}
