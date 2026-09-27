//! shell → zsandbox + shell v2 job protocol.
//! Additive fields: wait=now|next_write|exit, from_offset, timeout_s.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::{jobs, ToolContext};

fn sandbox_bin(ctx: &ToolContext) -> Option<PathBuf> {
    if let Some(b) = &ctx.sandbox_bin {
        return Some(b.clone());
    }
    if let Ok(p) = std::env::var("ZSANDBOX_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in sandbox_names() {
                let cand = dir.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for d in std::env::split_paths(&path) {
            for name in sandbox_names() {
                let cand = d.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

pub fn shell_argv(command: &str) -> Vec<String> {
    if cfg!(windows) {
        return vec!["powershell.exe".into(), "-NoProfile".into(), "-NonInteractive".into(), "-Command".into(), command.into()];
    }
    if let Ok(path) = std::env::var("PATH") {
        for d in std::env::split_paths(&path) {
            let b = d.join("bash");
            if b.is_file() {
                return vec![
                    b.to_string_lossy().into_owned(),
                    "-lc".into(),
                    command.into(),
                ];
            }
        }
    }
    vec!["sh".into(), "-lc".into(), command.into()]
}

fn sandbox_names() -> &'static [&'static str] {
    if cfg!(windows) { &["zsandbox.exe", "zsandbox"] } else { &["zsandbox"] }
}

fn deny_env_command(command: &str) -> bool {
    let c = command.replace('\\', "/").to_ascii_lowercase();
    c.contains("/.ssh") || c.contains("/.gnupg") || c.contains(".env")
}

fn run_blocking(ctx: &ToolContext, argv: &[String], timeout_s: u64) -> Result<(i32, String)> {
    if ctx.no_sandbox {
        let mut it = argv.iter();
        let prog = it.next().unwrap();
        let out = std::process::Command::new(prog)
            .args(it)
            .current_dir(&ctx.workspace)
            .output()?;
        let code = out.status.code().unwrap_or(1);
        let mut s = String::new();
        s.push_str(&String::from_utf8_lossy(&out.stdout));
        s.push_str(&String::from_utf8_lossy(&out.stderr));
        return Ok((code, s));
    }
    let degraded_note = ctx
        .sandbox_degraded
        .as_ref()
        .map(|r| format!("[sandbox:degraded: {r}]\n"))
        .unwrap_or_default();
    match sandbox_bin(ctx) {
        Some(bin) => {
            let mut args = vec![
                "exec".to_string(),
                "--workspace".into(),
                ctx.workspace.to_string_lossy().into_owned(),
                "--net".into(),
                if ctx.net_allow { "allow" } else { "deny" }.into(),
                "--timeout".into(),
                timeout_s.to_string(),
                "--".into(),
            ];
            args.extend(argv.iter().cloned());
            let out = std::process::Command::new(bin)
                .args(&args)
                .current_dir(&ctx.workspace)
                .output()?;
            let mut s = degraded_note;
            s.push_str(&String::from_utf8_lossy(&out.stdout));
            s.push_str(&String::from_utf8_lossy(&out.stderr));
            let code = out.status.code().unwrap_or(1);
            if code == 124 {
                s.push_str("\n[timeout]");
            }
            Ok((code, s))
        }
        None => {
            let r = zsandbox::exec(&zsandbox::ExecOpts {
                workspace: ctx.workspace.clone(),
                net: if ctx.net_allow {
                    zsandbox::NetPolicy::Allow
                } else {
                    zsandbox::NetPolicy::Deny
                },
                timeout: Duration::from_secs(timeout_s),
                argv: argv.to_vec(),
            })?;
            let mut s = degraded_note;
            if let Some(d) = &r.degraded {
                s.push_str(&format!("[sandbox:degraded: {d}]\n"));
            }
            s.push_str(&String::from_utf8_lossy(&r.stdout));
            s.push_str(&String::from_utf8_lossy(&r.stderr));
            if r.timed_out {
                s.push_str("\n[timeout]");
            }
            Ok((r.code, s))
        }
    }
}

pub fn bash(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let command = input["command"].as_str().unwrap_or("");
    if deny_env_command(command) && ctx.sandbox_mode != "full" {
        anyhow::bail!("sandbox deny: command touches ~/.ssh, ~/.gnupg, or .env");
    }
    if ctx.sandbox_mode == "read-only" {
        anyhow::bail!("sandbox read-only: bash writes denied");
    }
    let timeout_s = input["timeout_s"].as_u64().unwrap_or(30).min(600);
    let wait = input["wait"].as_str().unwrap_or("exit");
    let from_offset = input["from_offset"].as_u64();
    let tool_id = if ctx.current_tool_id.is_empty() {
        format!("bash-{}", std::process::id())
    } else {
        ctx.current_tool_id.clone()
    };
    let session = if ctx.session_id.is_empty() {
        "adhoc"
    } else {
        ctx.session_id.as_str()
    };
    let log = jobs::log_path(&ctx.home, session, &tool_id);

    if let Some(off) = from_offset {
        let (bytes, _n) = if log.is_file() {
            jobs::read_from(&log, off)?
        } else {
            (Vec::new(), 0)
        };
        let raw = String::from_utf8_lossy(&bytes);
        return Ok(jobs::middle_out_fence(&raw, 0, &log, 0));
    }

    let argv = shell_argv(command);

    if wait == "now" {
        let start = jobs::spawn_now(&argv, &ctx.workspace, log.clone())?;
        return Ok(format!(
            "{{\"task_id\":\"{}\",\"pid\":{},\"log_path\":\"{}\",\"offset\":{}}}",
            start.task_id,
            start.pid,
            start.log_path.display().to_string().replace('\\', "/"),
            start.offset
        ));
    }

    if wait == "next_write" {
        if !log.is_file() {
            let _ = jobs::spawn_now(&argv, &ctx.workspace, log.clone())?;
        }
        let off = 0u64;
        let (bytes, _) = jobs::wait_next_write(&log, off, Duration::from_secs(timeout_s))?;
        let raw = String::from_utf8_lossy(&bytes);
        let omitted = 0usize;
        return Ok(jobs::middle_out_fence(&raw, 0, &log, omitted));
    }

    // wait=exit (default)
    let (code, raw) = run_blocking(ctx, &argv, timeout_s)?;
    let _ = jobs::append_log(&log, raw.as_bytes());
    let omitted = raw.len().saturating_sub(8_000);
    Ok(jobs::middle_out_fence(&raw, code, &log, omitted))
}
