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
    if let Some(cell) = &opts.pid {
        cell.store(child.id(), std::sync::atomic::Ordering::Relaxed);
    }
    // drain both pipes from the start: a child that fills the pipe buffer
    // would otherwise block forever and only end at the timeout
    let drain = |mut r: Box<dyn Read + Send>, log: Option<std::path::PathBuf>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut buf = [0u8; 8192];
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..n]);
                crate::tee(log.as_deref(), &buf[..n]);
            }
            out
        })
    };
    let out_t = drain(Box::new(child.stdout.take().unwrap()), opts.log.clone());
    let err_t = drain(Box::new(child.stderr.take().unwrap()), opts.log.clone());
    let deadline = Instant::now() + opts.timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(ExecResult {
                code: status.code().unwrap_or(1),
                stdout: out_t.join().unwrap_or_default(),
                stderr: err_t.join().unwrap_or_default(),
                timed_out: false,
                degraded: None,
            });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let mut stderr = err_t.join().unwrap_or_default();
            stderr.extend_from_slice(b"zsandbox: timeout");
            return Ok(ExecResult {
                code: 124,
                stdout: out_t.join().unwrap_or_default(),
                stderr,
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
