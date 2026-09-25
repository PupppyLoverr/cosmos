//! bash → zsandbox.exe (contract §5). Never a raw spawn when the sandbox
//! binary is available.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::ToolContext;

/// Locate zsandbox.exe: ZSANDBOX_BIN env → next to current exe → PATH.
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
            for name in ["zsandbox.exe", "zsandbox"] {
                let cand = dir.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    // PATH search
    if let Ok(path) = std::env::var("PATH") {
        for d in std::env::split_paths(&path) {
            for name in ["zsandbox.exe", "zsandbox"] {
                let cand = d.join(name);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
    }
    None
}

fn shell_argv(command: &str) -> Vec<String> {
    // Git Bash when present, else powershell non-interactive (contract §1)
    if let Ok(path) = std::env::var("PATH") {
        for d in std::env::split_paths(&path) {
            let b = d.join("bash.exe");
            if b.is_file() {
                return vec![
                    b.to_string_lossy().into_owned(),
                    "-lc".into(),
                    command.into(),
                ];
            }
        }
    }
    vec![
        "powershell.exe".into(),
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-Command".into(),
        command.into(),
    ]
}

pub fn bash(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let command = input["command"].as_str().unwrap_or("");
    let timeout_s = input["timeout_s"].as_u64().unwrap_or(120).min(600);
    let argv = shell_argv(command);
    let net = if ctx.net_allow {
        zsandbox::NetPolicy::Allow
    } else {
        zsandbox::NetPolicy::Deny
    };

    if ctx.no_sandbox {
        // explicitly operator-disabled
        let mut it = argv.iter();
        let prog = it.next().unwrap();
        let out = std::process::Command::new(prog)
            .args(it)
            .current_dir(&ctx.workspace)
            .output()?;
        return Ok(format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    // Prefer the library in-process when linked (this crate), else the exe.
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
            args.extend(argv);
            let out = std::process::Command::new(bin)
                .args(&args)
                .current_dir(&ctx.workspace)
                .output()?;
            let mut s = String::new();
            s.push_str(&degraded_note);
            s.push_str(&String::from_utf8_lossy(&out.stdout));
            s.push_str(&String::from_utf8_lossy(&out.stderr));
            if out.status.code().unwrap_or(1) == 124 {
                s.push_str("\n[timeout]");
            }
            Ok(s)
        }
        None => {
            // in-process fallback — zsandbox is linked into this binary
            let r = zsandbox::exec(&zsandbox::ExecOpts {
                workspace: ctx.workspace.clone(),
                net,
                timeout: Duration::from_secs(timeout_s),
                argv,
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
            Ok(s)
        }
    }
}
