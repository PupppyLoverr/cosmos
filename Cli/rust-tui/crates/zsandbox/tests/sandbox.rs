//! zsandbox probes — AppContainer write/net deny, timeout. Windows-only.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use zsandbox::{exec, ExecOpts, NetPolicy};

fn workspace(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zsb_test_{}_{}", std::process::id(), tag));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(ws: &Path, net: NetPolicy, timeout_s: u64, argv: &[&str]) -> zsandbox::ExecResult {
    exec(&ExecOpts {
        workspace: ws.to_path_buf(),
        net,
        timeout: Duration::from_secs(timeout_s),
        argv: argv.iter().map(|s| s.to_string()).collect(),
    })
    .unwrap()
}

fn cmd() -> &'static str {
    r"C:\Windows\System32\cmd.exe"
}

#[test]
fn writes_inside_workspace() {
    let ws = workspace("in");
    let r = run(
        &ws,
        NetPolicy::Deny,
        30,
        &[cmd(), "/c", "echo hi > probe.txt && type probe.txt"],
    );
    assert_eq!(r.code, 0, "stderr={}", String::from_utf8_lossy(&r.stderr));
    assert!(ws.join("probe.txt").is_file());
}

#[test]
fn write_outside_workspace_denied() {
    let ws = workspace("out");
    let outside = std::env::temp_dir().join("zsb_outside_probe.txt");
    let _ = std::fs::remove_file(&outside);
    let script = format!(
        "echo x > \"{}\" 2> nul && echo WROTE || echo DENIED",
        outside.display()
    );
    let r = run(&ws, NetPolicy::Deny, 30, &[cmd(), "/c", &script]);
    let out = String::from_utf8_lossy(&r.stdout);
    // AppContainer: file must not exist and write must fail.
    assert!(
        !outside.exists(),
        "sandbox escaped: wrote {}",
        outside.display()
    );
    assert!(out.contains("DENIED") || r.code != 0, "stdout={out}");
}

#[test]
fn network_denied() {
    let ws = workspace("net");
    let r = run(
        &ws,
        NetPolicy::Deny,
        30,
        &[
            r"C:\Windows\System32\curl.exe",
            "-sS",
            "--max-time",
            "10",
            "https://example.com",
        ],
    );
    assert_ne!(r.code, 0, "curl unexpectedly succeeded under --net deny");
}

#[test]
fn timeout_kills_child() {
    let ws = workspace("to");
    // Infinite no-op loop — powershell can exit instantly under AppContainer.
    let r = run(
        &ws,
        NetPolicy::Deny,
        3,
        &[cmd(), "/c", "for /l %i in () do @set x=%i"],
    );
    assert!(r.timed_out || r.code == 124, "code={}", r.code);
}
