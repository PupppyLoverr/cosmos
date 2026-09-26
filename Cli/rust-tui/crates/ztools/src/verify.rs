//! LSP-lite: after write/edit to a cheap source file, run a compiler
//! diagnostic with an 8s timeout. Never block the UI past that.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn ext(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
}

fn run_capped(cwd: &Path, argv: &[&str], timeout: Duration) -> Option<String> {
    let start = Instant::now();
    let mut child = Command::new(argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let out = child.wait_with_output().ok()?;
                let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
                s.push_str(&String::from_utf8_lossy(&out.stderr));
                let s = s.trim();
                if s.is_empty() {
                    return None;
                }
                return Some(s.chars().take(4000).collect());
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(_) => return None,
        }
    }
}

/// Best-effort diagnostics. Returns None when skipped or timed out.
pub fn after_write(workspace: &Path, path: &str) -> Option<String> {
    let timeout = Duration::from_secs(8);
    match ext(path) {
        "rs" if workspace.join("Cargo.toml").is_file() => {
            run_capped(workspace, &["cargo", "check", "-q"], timeout)
        }
        "ts" | "tsx" if workspace.join("tsconfig.json").is_file() => {
            run_capped(workspace, &["tsc", "--noEmit", "--pretty", "false"], timeout)
        }
        "js" if workspace.join("tsconfig.json").is_file() => {
            run_capped(workspace, &["tsc", "--noEmit", "--pretty", "false"], timeout)
        }
        "go" => run_capped(workspace, &["go", "build", "./..."], timeout),
        _ => None,
    }
}
