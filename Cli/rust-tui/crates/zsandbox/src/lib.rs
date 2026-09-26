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

use std::path::PathBuf;
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

#[derive(Debug, Clone)]
pub struct ExecOpts {
    pub workspace: PathBuf,
    pub net: NetPolicy,
    pub timeout: Duration,
    pub argv: Vec<String>,
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
