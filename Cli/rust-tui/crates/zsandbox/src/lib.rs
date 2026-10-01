//! zsandbox — the one sandbox used by all three Cosmos CLI harnesses.
//!
//! Windows: the child runs inside an AppContainer with no capabilities (no
//! network unless `--net allow` adds the internetClient capability), file
//! access granted only where we add ACLs, inside a Job object with
//! KILL_ON_JOB_CLOSE + memory and process caps. If AppContainer setup fails
//! the child runs under a restricted token at low integrity inside the Job
//! and the result reports `degraded` with the reason.
//!
//! Linux: bubblewrap (`bwrap`) with `--unshare-net` for `--net deny`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

#[cfg(unix)]
mod linux;
#[cfg(windows)]
mod win;
#[cfg(target_os = "macos")]
pub mod seatbelt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetPolicy {
    Deny,
    Allow,
}

/// Windows isolation tiers, strongest first. `exec` starts at
/// `ExecOpts::tier` and falls back one tier at a time; any run below
/// AppContainer reports `degraded` with the reason. (Linux uses bwrap only.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tier {
    /// AppContainer: no network unless allowed, file access only where granted
    #[default]
    AppContainer,
    /// restricted token at low integrity: writes only where labelled low;
    /// network is not blocked
    Restricted,
    /// job object limits only (time, memory, process count) — no isolation
    JobOnly,
}

impl Tier {
    pub fn parse(s: &str) -> Option<Tier> {
        match s {
            "appcontainer" => Some(Tier::AppContainer),
            "restricted" => Some(Tier::Restricted),
            "job" | "jobonly" => Some(Tier::JobOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecOpts {
    pub workspace: PathBuf,
    pub net: NetPolicy,
    pub timeout: Duration,
    pub argv: Vec<String>,
    /// Append stdout and stderr here as they arrive (background jobs read
    /// the file while the command runs). The result still carries both.
    pub log: Option<PathBuf>,
    /// Set to the child's process id once it is running.
    pub pid: Option<Arc<AtomicU32>>,
    /// Strongest tier to try first (Windows). `ZSANDBOX_TIER` overrides.
    pub tier: Tier,
}

impl ExecOpts {
    pub fn new(workspace: PathBuf, net: NetPolicy, timeout: Duration, argv: Vec<String>) -> Self {
        ExecOpts { workspace, net, timeout, argv, log: None, pid: None, tier: Tier::default() }
    }
}

/// Append a chunk to the live log (best effort — the result keeps a copy).
pub(crate) fn tee(log: Option<&Path>, bytes: &[u8]) {
    if let Some(p) = log {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = f.write_all(bytes);
        }
    }
}

#[derive(Debug)]
pub struct ExecResult {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    /// Some(reason) when the sandbox fell back to the degraded path.
    pub degraded: Option<String>,
}

/// Run `argv` sandboxed inside `workspace`.
pub fn exec(opts: &ExecOpts) -> Result<ExecResult> {
    #[cfg(windows)]
    {
        win::exec(opts)
    }
    #[cfg(unix)]
    {
        linux::exec(opts)
    }
    #[cfg(not(any(windows, unix)))]
    {
        anyhow::bail!("zsandbox: unsupported platform")
    }
}
