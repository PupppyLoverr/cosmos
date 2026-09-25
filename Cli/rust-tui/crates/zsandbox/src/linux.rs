//! Linux sandbox via bubblewrap — the approach Codex uses: bind the workspace
//! rw, the rest of the filesystem read-only, `--unshare-net` for `--net deny`.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{bail, Context, Result};

use crate::{ExecOpts, ExecResult, NetPolicy};

pub fn exec(opts: &ExecOpts) -> Result<ExecResult> {
    if opts.argv.is_empty() {
        bail!("zsandbox: empty argv");
    }
    if which::missing_bwrap() {
        bail!("zsandbox: bubblewrap (bwrap) not installed — degraded sandbox unavailable");
    }
    let ws = opts
        .workspace
        .canonicalize()
        .unwrap_or_else(|_| opts.workspace.clone());
    let mut cmd = Command::new("bwrap");
    cmd.arg("--die-with-parent")
        .arg("--new-session")
        .arg("--unshare-pid")
        .arg("--unshare-ipc")
        .arg("--unshare-uts")
        .arg("--unshare-cgroup");
    if opts.net == NetPolicy::Deny {
        cmd.arg("--unshare-net");
    }
    cmd.arg("--ro-bind")
        .arg("/")
        .arg("/")
        .arg("--bind")
        .arg(&ws)
        .arg(&ws)
        .arg("--chdir")
        .arg(&ws)
        .arg("--")
        .args(&opts.argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().context("spawn bwrap")?;
    let deadline = Instant::now() + opts.timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            child.stdout.take().unwrap().read_to_end(&mut stdout).ok();
            child.stderr.take().unwrap().read_to_end(&mut stderr).ok();
            return Ok(ExecResult {
                code: status.code().unwrap_or(1),
                stdout,
                stderr,
                timed_out: false,
                degraded: None,
            });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(ExecResult {
                code: 124,
                stdout: Vec::new(),
                stderr: b"zsandbox: timeout".to_vec(),
                timed_out: true,
                degraded: None,
            });
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

mod which {
    pub fn missing_bwrap() -> bool {
        std::process::Command::new("bwrap")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| !s.success())
            .unwrap_or(true)
    }
}
