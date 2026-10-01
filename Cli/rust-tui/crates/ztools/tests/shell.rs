//! bash background jobs through the real sandbox: wait=now returns at once
//! with a pid, next_write / from_offset read only new bytes from the same
//! command's log, a second start of a running command is reported, and the
//! exit code lands when the job ends.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use ztools::{run, ToolContext};

fn ctx() -> (tempfile::TempDir, ToolContext) {
    let d = tempfile::tempdir().unwrap();
    let ws = d.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let mut c = ToolContext::new(&ws);
    c.home = d.path().join("home");
    c.session_id = "s1".into();
    (d, c)
}

fn cmd(steps: &[&str]) -> String {
    if cfg!(windows) {
        steps.join("; ")
    } else {
        steps.iter().map(|s| s.replace("Start-Sleep -Milliseconds ", "sleep 0.").replace("Write-Output", "echo")).collect::<Vec<_>>().join("; ")
    }
}

#[test]
fn background_job_streams_through_the_sandbox() {
    let (_d, mut c) = ctx();
    let command = cmd(&["Write-Output first", "Start-Sleep -Milliseconds 1500", "Write-Output second"]);
    let t0 = Instant::now();
    let out = run(&mut c, "bash", &json!({"command": command, "wait": "now"}));
    let v: Value = serde_json::from_str(&out).unwrap_or_else(|_| panic!("{out}"));
    assert!(t0.elapsed() < Duration::from_secs(10), "wait=now must not block on the command");
    assert!(v["pid"].as_u64().unwrap_or(0) > 0, "{v}");
    assert_eq!(v["offset"], 0);
    let task = v["task_id"].as_str().unwrap().to_string();

    // the same command while it runs is not started twice
    let again: Value = serde_json::from_str(&run(&mut c, "bash", &json!({"command": command, "wait": "now"}))).unwrap();
    assert_eq!(again["task_id"], task.as_str());
    assert!(again["note"].as_str().unwrap().contains("already running"), "{again}");

    // next_write returns once there is output, with the next offset
    let first = run(&mut c, "bash", &json!({"command": command, "wait": "next_write", "timeout_s": 60}));
    assert!(first.contains("first"), "{first}");
    let off: u64 = first.split("offset=").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
    assert!(off > 0);

    // wait for the job to end, then read only what came after `off`
    let deadline = Instant::now() + Duration::from_secs(60);
    while ztools::jobs::state(&task).map(|(r, _)| r).unwrap_or(true) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let rest = run(&mut c, "bash", &json!({"command": command, "from_offset": off}));
    assert!(rest.starts_with("exit_code=0"), "{rest}");
    assert!(rest.contains("second") && !rest.contains("first"), "{rest}");
    let ended: Vec<_> = ztools::jobs::drain_ended().into_iter().filter(|j| j.task_id == task).collect();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].exit_code, 0);
}

#[test]
fn reading_a_job_that_never_started_is_an_error() {
    let (_d, mut c) = ctx();
    let out = run(&mut c, "bash", &json!({"command": "never-started-xyz", "wait": "next_write"}));
    assert!(out.starts_with("ERROR") && out.contains("wait=now"), "{out}");
    let out = run(&mut c, "bash", &json!({"command": "never-started-xyz", "from_offset": 0}));
    assert!(out.starts_with("ERROR"), "{out}");
}

#[test]
fn foreground_result_has_exit_code_log_and_fence() {
    let (_d, mut c) = ctx();
    let out = run(&mut c, "bash", &json!({"command": cmd(&["Write-Output hello"])}));
    assert!(out.starts_with("exit_code=0 log_path="), "{out}");
    assert!(out.contains("omitted_chars=0") && out.contains("<<<UNTRUSTED_SHELL") && out.contains("hello"), "{out}");
    let log = out.split("log_path=").nth(1).unwrap().split(" omitted_chars").next().unwrap();
    assert!(std::fs::read_to_string(log).unwrap().contains("hello"), "full output kept on disk");
    let state = ztools::shell::last_sandbox(&c.home);
    assert!(state.is_some(), "the sandbox state is recorded after a run");
}
