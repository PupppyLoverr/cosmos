//! `bash` → zsandbox, plus background jobs (contract §2, §5).
//!
//! `wait=exit` (default) runs to completion; `wait=now` starts a background
//! job and returns at once; `wait=next_write` and `from_offset` read only new
//! output of that job — jobs are keyed by their command (see jobs.rs).
//! Results carry `exit_code`, `log_path`, an exact `omitted_chars`, the next
//! `offset` for job reads, and the output fenced as `<<<UNTRUSTED_SHELL`.

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
                return vec![b.to_string_lossy().into_owned(), "-lc".into(), command.into()];
            }
        }
    }
    vec!["sh".into(), "-lc".into(), command.into()]
}

/// One line for the boot context: which shell `bash` really runs.
pub fn shell_name() -> &'static str {
    if cfg!(windows) {
        "Windows PowerShell 5.1"
    } else {
        "bash (or sh)"
    }
}

fn sandbox_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["zsandbox.exe", "zsandbox"]
    } else {
        &["zsandbox"]
    }
}

fn deny_env_command(command: &str) -> bool {
    let c = command.replace('\\', "/").to_ascii_lowercase();
    c.contains("/.ssh") || c.contains("/.gnupg") || c.contains(".env")
}

// ---- sandbox tier (Windows) --------------------------------------------------------

/// Bump when the canary or the tier logic changes — forces a re-probe.
const CANARY_VERSION: u32 = 1;

fn tier_file(home: &std::path::Path) -> PathBuf {
    home.join("sandbox_tier.json")
}

pub fn tier_name(t: zsandbox::Tier) -> &'static str {
    match t {
        zsandbox::Tier::AppContainer => "appcontainer",
        zsandbox::Tier::Restricted => "restricted",
        zsandbox::Tier::JobOnly => "job",
    }
}

/// What the last canary decided, without running anything: `None` = never
/// probed; `Some(None)` = full sandbox; `Some(Some(r))` = degraded, why.
pub fn last_sandbox(home: &std::path::Path) -> Option<Option<String>> {
    if let Ok(t) = std::env::var("COSMOS_SANDBOX_TIER") {
        if let Some(tier) = zsandbox::Tier::parse(&t.to_ascii_lowercase()) {
            return Some((tier != zsandbox::Tier::AppContainer).then(|| format!("tier pinned by COSMOS_SANDBOX_TIER={t}")));
        }
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(tier_file(home)).ok()?).ok()?;
    Some(v["degraded"].as_str().map(str::to_string))
}

/// Windows: the strongest zsandbox tier in which this machine's shell
/// actually works, decided once by a canary and cached in
/// `COSMOS_CLI_HOME/sandbox_tier.json`. Returns the tier and, below
/// AppContainer, the honest reason it is degraded (contract §5).
///
/// The canary must print, write and read back a file in a scratch folder, and
/// list every PATH folder under the user profile (where cargo, node and
/// friends live). Machines differ: PowerShell 5.1 cannot start its file
/// system provider inside an AppContainer, and some profiles carry ACLs or
/// labels that keep low-integrity processes out.
pub fn shell_tier(ctx: &ToolContext) -> (zsandbox::Tier, Option<String>) {
    if !cfg!(windows) {
        return (zsandbox::Tier::AppContainer, None);
    }
    if let Ok(t) = std::env::var("COSMOS_SANDBOX_TIER") {
        if let Some(tier) = zsandbox::Tier::parse(&t.to_ascii_lowercase()) {
            let why = (tier != zsandbox::Tier::AppContainer).then(|| format!("tier pinned by COSMOS_SANDBOX_TIER={t}"));
            return (tier, why);
        }
    }
    let key = format!("v{CANARY_VERSION}|{}|{}", env!("CARGO_PKG_VERSION"), shell_argv("")[0]);
    let file = tier_file(&ctx.home);
    if let Some(v) = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        if v["key"] == key.as_str() {
            if let Some(t) = v["tier"].as_str().and_then(zsandbox::Tier::parse) {
                return (t, v["degraded"].as_str().map(str::to_string));
            }
        }
    }
    let mut reasons: Vec<String> = Vec::new();
    let mut chosen = zsandbox::Tier::JobOnly;
    for tier in [zsandbox::Tier::AppContainer, zsandbox::Tier::Restricted] {
        match canary(ctx, tier) {
            Ok(()) => {
                chosen = tier;
                break;
            }
            Err(why) => reasons.push(format!("{}: {why}", tier_name(tier))),
        }
    }
    let degraded = match chosen {
        zsandbox::Tier::AppContainer => None,
        zsandbox::Tier::Restricted => Some(format!(
            "low-integrity restricted token — writes outside the workspace are blocked, the network is NOT ({})",
            reasons.join("; ")
        )),
        zsandbox::Tier::JobOnly => Some(format!(
            "no isolation on this machine — time, memory and process limits only; files and network are NOT protected ({})",
            reasons.join("; ")
        )),
    };
    let _ = std::fs::create_dir_all(&ctx.home);
    let _ = std::fs::write(
        &file,
        serde_json::json!({"key": key, "tier": tier_name(chosen), "degraded": degraded, "at": zproto::now_ts()}).to_string(),
    );
    (chosen, degraded)
}

/// A fallback happened below the tier the canary chose: forget the cached
/// decision so the next session probes again.
fn invalidate_tier(ctx: &ToolContext) {
    let _ = std::fs::remove_file(tier_file(&ctx.home));
}

/// Did zsandbox fall below `tier` on this run?
fn fell_below(tier: zsandbox::Tier, degraded: &str) -> bool {
    match tier {
        zsandbox::Tier::AppContainer => degraded.contains("appcontainer_unavailable"),
        zsandbox::Tier::Restricted => degraded.contains("restricted_token_unavailable"),
        zsandbox::Tier::JobOnly => false,
    }
}

/// Run the canary at `tier`; Err(what failed).
fn canary(ctx: &ToolContext, tier: zsandbox::Tier) -> std::result::Result<(), String> {
    let script = "Write-Output cosmos-canary; \
        Set-Content -Path .cosmos-canary.txt -Value cosmos-wrote; Get-Content .cosmos-canary.txt; \
        Remove-Item .cosmos-canary.txt -ErrorAction SilentlyContinue; \
        foreach ($d in ($env:Path -split ';')) { \
          if ($d -and $env:USERPROFILE -and $d.StartsWith($env:USERPROFILE, 'OrdinalIgnoreCase') -and (Test-Path -LiteralPath $d)) { \
            try { Get-ChildItem -LiteralPath $d -ErrorAction Stop | Out-Null } catch { Write-Output \"cosmos-denied:$d\" } } }";
    // A scratch folder, not the operator's workspace: each tier changes the
    // folder's ACL/label, and a failed AppContainer probe must not leave
    // grants behind that break the next tier (or the operator's repo).
    let scratch = ctx.home.join("canary").join(tier_name(tier));
    std::fs::create_dir_all(&scratch).map_err(|e| format!("canary folder: {e}"))?;
    let mut opts = zsandbox::ExecOpts::new(scratch, zsandbox::NetPolicy::Deny, Duration::from_secs(90), shell_argv(script));
    opts.tier = tier;
    let r = zsandbox::exec(&opts).map_err(|e| format!("{e:#}"))?;
    if let Some(d) = &r.degraded {
        if fell_below(tier, d) {
            return Err(format!("could not start the shell ({})", d.lines().next().unwrap_or("")));
        }
    }
    let out = String::from_utf8_lossy(&r.stdout).to_string();
    let err = String::from_utf8_lossy(&r.stderr).to_string();
    if !out.contains("cosmos-canary") {
        return Err(format!("the shell produced no output (exit {})", r.code));
    }
    if !out.contains("cosmos-wrote") {
        let first = err.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
        return Err(format!("could not write a test file{}", if first.is_empty() { String::new() } else { format!(" ({first})") }));
    }
    if let Some(d) = out.lines().find_map(|l| l.trim().strip_prefix("cosmos-denied:")) {
        return Err(format!("cannot read {d}"));
    }
    Ok(())
}

// ---- running commands -------------------------------------------------------------

fn run_blocking(ctx: &ToolContext, argv: &[String], timeout_s: u64, tier: zsandbox::Tier) -> Result<(i32, String)> {
    if ctx.no_sandbox {
        let mut it = argv.iter();
        let prog = it.next().unwrap();
        let out = std::process::Command::new(prog).args(it).current_dir(&ctx.workspace).output()?;
        let code = out.status.code().unwrap_or(1);
        let mut s = String::new();
        s.push_str(&String::from_utf8_lossy(&out.stdout));
        s.push_str(&String::from_utf8_lossy(&out.stderr));
        return Ok((code, s));
    }
    let mut s = ctx
        .sandbox_degraded
        .as_ref()
        .map(|r| format!("[sandbox:degraded: {r}]\n"))
        .unwrap_or_default();
    let (code, stdout, stderr, degraded, timed_out) = match sandbox_bin(ctx) {
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
                .env("ZSANDBOX_TIER", tier_name(tier))
                .current_dir(&ctx.workspace)
                .output()?;
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let degraded = stderr.lines().find_map(|l| l.strip_prefix("zsandbox: degraded: ")).map(str::to_string);
            // the marker line is ours to report, not the command's output
            let stderr: String = stderr.lines().filter(|l| !l.starts_with("zsandbox: degraded: ")).map(|l| format!("{l}\n")).collect();
            let code = out.status.code().unwrap_or(1);
            (code, String::from_utf8_lossy(&out.stdout).to_string(), stderr, degraded, code == 124)
        }
        None => {
            let mut opts = zsandbox::ExecOpts::new(
                ctx.workspace.clone(),
                if ctx.net_allow { zsandbox::NetPolicy::Allow } else { zsandbox::NetPolicy::Deny },
                Duration::from_secs(timeout_s),
                argv.to_vec(),
            );
            opts.tier = tier;
            let r = zsandbox::exec(&opts)?;
            (r.code, String::from_utf8_lossy(&r.stdout).to_string(), String::from_utf8_lossy(&r.stderr).to_string(), r.degraded, r.timed_out)
        }
    };
    if let Some(d) = degraded.filter(|d| fell_below(tier, d)) {
        // worse than the canary promised — say so and re-probe next session
        s.push_str(&format!("[sandbox:degraded: {d}]\n"));
        invalidate_tier(ctx);
    }
    s.push_str(&stdout);
    s.push_str(&stderr);
    if timed_out {
        s.push_str("\n[timeout]");
    }
    Ok((code, s))
}

/// Default lifetime of a background job — long enough for a dev server.
const JOB_TIMEOUT_S: u64 = 1800;

pub fn bash(ctx: &mut ToolContext, input: &Value) -> Result<String> {
    let command = input["command"].as_str().unwrap_or("");
    if deny_env_command(command) && ctx.sandbox_mode != "full" {
        anyhow::bail!("sandbox deny: command touches ~/.ssh, ~/.gnupg, or .env");
    }
    if ctx.sandbox_mode == "read-only" {
        anyhow::bail!("sandbox read-only: bash writes denied");
    }
    let tier = if ctx.no_sandbox {
        zsandbox::Tier::JobOnly
    } else {
        let (tier, why) = shell_tier(ctx);
        ctx.sandbox_degraded = why;
        tier
    };
    let wait = input["wait"].as_str().unwrap_or("exit");
    let from_offset = input["from_offset"].as_u64();
    let session = if ctx.session_id.is_empty() { "adhoc".to_string() } else { ctx.session_id.clone() };
    let job = jobs::job_id(command);
    let job_log = jobs::log_path(&ctx.home, &session, &job);
    let net = if ctx.net_allow { zsandbox::NetPolicy::Allow } else { zsandbox::NetPolicy::Deny };
    let degraded_note = ctx.sandbox_degraded.clone();

    match wait {
        "now" => {
            let timeout = Duration::from_secs(input["timeout_s"].as_u64().unwrap_or(JOB_TIMEOUT_S).min(24 * 3600));
            let start = jobs::start(jobs::Spawn {
                task_id: job.clone(),
                argv: &shell_argv(command),
                cwd: &ctx.workspace,
                log: job_log,
                timeout,
                sandbox: (!ctx.no_sandbox).then_some((net, tier)),
            })?;
            Ok(serde_json::json!({
                "task_id": start.task_id,
                "pid": start.pid,
                "log_path": start.log_path.display().to_string().replace('\\', "/"),
                "offset": start.offset,
                "running": true,
                "sandbox": if ctx.no_sandbox { "off".to_string() } else { degraded_note.map(|d| format!("degraded: {d}")).unwrap_or_else(|| tier_name(tier).to_string()) },
                "note": if start.already_running {
                    "already running — read new output with the same command and from_offset"
                } else {
                    "started in the background — read output with the same command and wait=next_write or from_offset"
                },
            })
            .to_string())
        }
        "next_write" => {
            if jobs::state(&job).is_none() && !job_log.is_file() {
                anyhow::bail!("no background job for this command — start it with wait=now");
            }
            let off = from_offset.unwrap_or(0);
            let timeout = Duration::from_secs(input["timeout_s"].as_u64().unwrap_or(30).min(600));
            let (bytes, end) = jobs::wait_next_write(&job_log, off, &job, timeout)?;
            let code = jobs::state(&job).and_then(|(running, c)| if running { None } else { c });
            Ok(jobs::fence(&String::from_utf8_lossy(&bytes), code, &job_log, Some(end)))
        }
        _ if from_offset.is_some() => {
            if !job_log.is_file() {
                anyhow::bail!("no background job log for this command — start it with wait=now");
            }
            let (bytes, end) = jobs::read_from(&job_log, from_offset.unwrap_or(0))?;
            let code = jobs::state(&job).and_then(|(running, c)| if running { None } else { c });
            Ok(jobs::fence(&String::from_utf8_lossy(&bytes), code, &job_log, Some(end)))
        }
        _ => {
            // wait=exit: run to completion; the full output is kept on disk
            let timeout_s = input["timeout_s"].as_u64().unwrap_or(30).min(600);
            let tool_id = if ctx.current_tool_id.is_empty() {
                format!("bash-{}", zproto::new_id())
            } else {
                ctx.current_tool_id.clone()
            };
            let log = jobs::log_path(&ctx.home, &session, &tool_id);
            let (code, raw) = run_blocking(ctx, &shell_argv(command), timeout_s, tier)?;
            let _ = jobs::append_log(&log, raw.as_bytes());
            Ok(jobs::fence(&raw, Some(code), &log, None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_detection_matches_the_requested_tier() {
        let d = "appcontainer_unavailable: x; restricted_token_unavailable: y; running with job limits only";
        assert!(fell_below(zsandbox::Tier::AppContainer, d));
        assert!(fell_below(zsandbox::Tier::Restricted, d));
        assert!(!fell_below(zsandbox::Tier::Restricted, "appcontainer_unavailable: not attempted (tier=restricted)"));
        assert!(!fell_below(zsandbox::Tier::JobOnly, d));
    }
}
