//! Job supervisor: spawn, persist log, offset reads, kill, wake.
//! `job_end` is queued for the agent loop — do not poll.

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

#[derive(Debug, Clone)]
pub struct JobStart {
    pub task_id: String,
    pub pid: u32,
    pub log_path: PathBuf,
    pub offset: u64,
}

#[derive(Debug, Clone)]
pub struct JobEnd {
    pub task_id: String,
    pub exit_code: i32,
    pub log_path: PathBuf,
}

fn wakes() -> &'static Mutex<Vec<JobEnd>> {
    static W: std::sync::OnceLock<Mutex<Vec<JobEnd>>> = std::sync::OnceLock::new();
    W.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn log_path(home: &Path, session: &str, tool_id: &str) -> PathBuf {
    home.join("logs").join(session).join(format!("{tool_id}.log"))
}

pub fn append_log(path: &Path, bytes: &[u8]) -> Result<u64> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(bytes)?;
    Ok(f.metadata()?.len())
}

pub fn read_from(path: &Path, offset: u64) -> Result<(Vec<u8>, u64)> {
    let mut f = fs::File::open(path)?;
    let len = f.metadata()?.len();
    if offset >= len {
        return Ok((Vec::new(), len));
    }
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok((buf, len))
}

/// Spawn `argv` in `cwd`, persist stdout+stderr to `log`, return immediately.
pub fn spawn_now(argv: &[String], cwd: &Path, log: PathBuf) -> Result<JobStart> {
    if let Some(p) = log.parent() {
        fs::create_dir_all(p)?;
    }
    let mut cmd = Command::new(&argv[0]);
    if argv.len() > 1 {
        cmd.args(&argv[1..]);
    }
    let mut child = cmd
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let task_id = format!("job-{pid}");
    if let Some(mut out) = child.stdout.take() {
        let log_c = log.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = append_log(&log_c, &buf[..n]);
            }
        });
    }
    if let Some(mut err) = child.stderr.take() {
        let log_c = log.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = append_log(&log_c, &buf[..n]);
            }
        });
    }
    let log_w = log.clone();
    let id = task_id.clone();
    thread::spawn(move || {
        let code = match child.wait() {
            Ok(s) => s.code().unwrap_or(1),
            Err(_) => 1,
        };
        if let Ok(mut w) = wakes().lock() {
            w.push(JobEnd {
                task_id: id,
                exit_code: code,
                log_path: log_w,
            });
        }
    });
    Ok(JobStart {
        task_id,
        pid,
        log_path: log,
        offset: 0,
    })
}

/// Block until the log grows or timeout.
pub fn wait_next_write(path: &Path, offset: u64, timeout: Duration) -> Result<(Vec<u8>, u64)> {
    let start = Instant::now();
    loop {
        if path.is_file() {
            let (b, n) = read_from(path, offset)?;
            if !b.is_empty() {
                return Ok((b, n));
            }
        }
        if start.elapsed() >= timeout {
            return Ok((Vec::new(), offset));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub fn drain_ended() -> Vec<JobEnd> {
    wakes()
        .lock()
        .map(|mut w| w.drain(..).collect())
        .unwrap_or_default()
}

pub fn middle_out_fence(raw: &str, exit_code: i32, log_path: &Path, omitted: usize) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let body = if lines.len() <= 48 {
        raw.to_string()
    } else {
        let head = lines[..24].join("\n");
        let tail = lines[lines.len() - 24..].join("\n");
        format!("{head}\n… {omitted} chars omitted …\n{tail}")
    };
    let note = if exit_code == 1 && (raw.trim().is_empty() || raw.to_ascii_lowercase().contains("no matches"))
    {
        "no matches"
    } else {
        ""
    };
    format!(
        "exit_code={exit_code} log_path={} omitted_chars={omitted} {note}\n<<<UNTRUSTED_SHELL\n{body}\n>>>",
        log_path.display()
    )
}

pub fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/pid", &pid.to_string(), "/t", "/f"])
            .output();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).output();
        thread::sleep(Duration::from_millis(200));
        let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).output();
    }
}
