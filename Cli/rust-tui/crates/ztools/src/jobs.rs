//! Background jobs for `bash {wait: "now" | "next_write"}` and the log
//! reads behind `from_offset`.
//!
//! A job is keyed by its command within a session, so a later call with the
//! same command and a `from_offset` reads the same log — the bash schema has
//! no job id field. Jobs run through the sandbox like any other command
//! (unless the operator turned it off); output streams into
//! `logs/<session>/<job id>.log` as it arrives. `job_end` is queued for the
//! agent loop — never polled by the model.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct JobStart {
    pub task_id: String,
    pub pid: u32,
    pub log_path: PathBuf,
    /// bytes already in the log when this call returned
    pub offset: u64,
    /// the same command was already running — nothing new was started
    pub already_running: bool,
}

#[derive(Debug, Clone)]
pub struct JobEnd {
    pub task_id: String,
    pub exit_code: i32,
    pub log_path: PathBuf,
    pub degraded: Option<String>,
}

#[derive(Debug, Clone)]
struct Job {
    pid: u32,
    log: PathBuf,
    /// exit code once finished
    done: Option<i32>,
}

fn wakes() -> &'static Mutex<Vec<JobEnd>> {
    static W: std::sync::OnceLock<Mutex<Vec<JobEnd>>> = std::sync::OnceLock::new();
    W.get_or_init(|| Mutex::new(Vec::new()))
}

fn table() -> &'static Mutex<HashMap<String, Job>> {
    static T: std::sync::OnceLock<Mutex<HashMap<String, Job>>> = std::sync::OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Stable id for a background command within a session.
pub fn job_id(command: &str) -> String {
    let mut h = Sha256::new();
    h.update(command.trim().as_bytes());
    let d = h.finalize();
    format!("job-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}", d[0], d[1], d[2], d[3], d[4], d[5])
}

pub fn log_path(home: &Path, session: &str, id: &str) -> PathBuf {
    home.join("logs").join(session).join(format!("{id}.log"))
}

pub fn append_log(path: &Path, bytes: &[u8]) -> Result<u64> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(bytes)?;
    Ok(f.metadata()?.len())
}

pub fn log_len(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
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
    let end = offset + buf.len() as u64;
    Ok((buf, end))
}

/// (running, exit code) for a job this process started.
pub fn state(task_id: &str) -> Option<(bool, Option<i32>)> {
    table().lock().ok()?.get(task_id).map(|j| (j.done.is_none(), j.done))
}

pub fn pid_of(task_id: &str) -> Option<u32> {
    table().lock().ok()?.get(task_id).map(|j| j.pid)
}

/// How to run a background command.
pub struct Spawn<'a> {
    pub task_id: String,
    pub argv: &'a [String],
    pub cwd: &'a Path,
    pub log: PathBuf,
    pub timeout: Duration,
    /// None = run without the sandbox (operator's --no-sandbox)
    pub sandbox: Option<(zsandbox::NetPolicy, zsandbox::Tier)>,
}

/// Start (or find) a background job and return at once.
pub fn start(s: Spawn) -> Result<JobStart> {
    if let Ok(t) = table().lock() {
        if let Some(j) = t.get(&s.task_id).filter(|j| j.done.is_none()) {
            return Ok(JobStart {
                task_id: s.task_id.clone(),
                pid: j.pid,
                log_path: j.log.clone(),
                offset: log_len(&j.log),
                already_running: true,
            });
        }
    }
    if let Some(p) = s.log.parent() {
        fs::create_dir_all(p)?;
    }
    fs::write(&s.log, b"")?; // a fresh run starts a fresh log
    if let Ok(mut t) = table().lock() {
        t.insert(s.task_id.clone(), Job { pid: 0, log: s.log.clone(), done: None });
    }
    let pid = match s.sandbox {
        Some((net, tier)) => spawn_sandboxed(&s, net, tier)?,
        None => spawn_plain(&s)?,
    };
    if let Ok(mut t) = table().lock() {
        if let Some(j) = t.get_mut(&s.task_id) {
            j.pid = pid;
        }
    }
    Ok(JobStart { task_id: s.task_id, pid, log_path: s.log, offset: 0, already_running: false })
}

fn finish(task_id: &str, log: &Path, code: i32, degraded: Option<String>) {
    if let Ok(mut t) = table().lock() {
        if let Some(j) = t.get_mut(task_id) {
            j.done = Some(code);
        }
    }
    if let Ok(mut w) = wakes().lock() {
        w.push(JobEnd { task_id: task_id.to_string(), exit_code: code, log_path: log.to_path_buf(), degraded });
    }
}

/// Run through zsandbox on a thread; output streams to the log.
fn spawn_sandboxed(s: &Spawn, net: zsandbox::NetPolicy, tier: zsandbox::Tier) -> Result<u32> {
    let cell = Arc::new(AtomicU32::new(0));
    let mut opts = zsandbox::ExecOpts::new(s.cwd.to_path_buf(), net, s.timeout, s.argv.to_vec());
    opts.log = Some(s.log.clone());
    opts.pid = Some(cell.clone());
    opts.tier = tier;
    let (id, log) = (s.task_id.clone(), s.log.clone());
    let failed = Arc::new(Mutex::new(None::<String>));
    let failed_w = failed.clone();
    let handle = thread::spawn(move || match zsandbox::exec(&opts) {
        Ok(r) => {
            if r.timed_out {
                let _ = append_log(&log, b"\n[timeout]\n");
            }
            finish(&id, &log, r.code, r.degraded);
        }
        Err(e) => {
            let msg = format!("{e:#}");
            let _ = append_log(&log, format!("[sandbox error] {msg}\n").as_bytes());
            if let Ok(mut f) = failed_w.lock() {
                *f = Some(msg);
            }
            finish(&id, &log, 1, None);
        }
    });
    // the pid appears once the sandbox has created the process
    let t0 = Instant::now();
    while cell.load(Ordering::Relaxed) == 0 && !handle.is_finished() && t0.elapsed() < Duration::from_secs(10) {
        thread::sleep(Duration::from_millis(5));
    }
    if let Some(msg) = failed.lock().ok().and_then(|f| f.clone()) {
        anyhow::bail!("sandbox: {msg}");
    }
    Ok(cell.load(Ordering::Relaxed))
}

/// No sandbox: a plain child whose output is piped into the log.
fn spawn_plain(s: &Spawn) -> Result<u32> {
    let mut cmd = Command::new(&s.argv[0]);
    cmd.args(&s.argv[1..]).current_dir(s.cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let pump = |mut r: Box<dyn Read + Send>, log: PathBuf| {
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = r.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = append_log(&log, &buf[..n]);
            }
        })
    };
    let a = pump(Box::new(child.stdout.take().unwrap()), s.log.clone());
    let b = pump(Box::new(child.stderr.take().unwrap()), s.log.clone());
    let (id, log, timeout) = (s.task_id.clone(), s.log.clone(), s.timeout);
    thread::spawn(move || {
        let t0 = Instant::now();
        let code = loop {
            match child.try_wait() {
                Ok(Some(st)) => break st.code().unwrap_or(1),
                Ok(None) if t0.elapsed() >= timeout => {
                    kill_pid(pid);
                    let _ = child.wait();
                    let _ = append_log(&log, b"\n[timeout]\n");
                    break 124;
                }
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => break 1,
            }
        };
        let _ = a.join();
        let _ = b.join();
        finish(&id, &log, code, None);
    });
    Ok(pid)
}

/// Block until the log grows past `offset`, the job ends, or timeout.
pub fn wait_next_write(path: &Path, offset: u64, task_id: &str, timeout: Duration) -> Result<(Vec<u8>, u64)> {
    let start = Instant::now();
    loop {
        if path.is_file() {
            let (b, n) = read_from(path, offset)?;
            if !b.is_empty() {
                return Ok((b, n));
            }
        }
        let running = state(task_id).map(|(r, _)| r).unwrap_or(false);
        if !running || start.elapsed() >= timeout {
            return read_from(path, offset).or(Ok((Vec::new(), offset)));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub fn drain_ended() -> Vec<JobEnd> {
    wakes().lock().map(|mut w| w.drain(..).collect()).unwrap_or_default()
}

/// Keep the first and last `HEAD`/`TAIL` lines; returns (body, exact count
/// of bytes left out between them).
pub fn middle_out(raw: &str) -> (String, usize) {
    const HEAD: usize = 24;
    const TAIL: usize = 24;
    let lines: Vec<&str> = raw.lines().collect();
    if lines.len() <= HEAD + TAIL {
        return (raw.trim_end_matches('\n').to_string(), 0);
    }
    let head = lines[..HEAD].join("\n");
    let tail = lines[lines.len() - TAIL..].join("\n");
    // bytes between the end of line HEAD and the start of the tail
    let head_end = raw.match_indices('\n').nth(HEAD - 1).map(|(i, _)| i + 1).unwrap_or(raw.len());
    let tail_start = {
        let trimmed = raw.trim_end_matches('\n');
        let mut idx = trimmed.len();
        for _ in 0..TAIL {
            idx = trimmed[..idx].rfind('\n').unwrap_or(0);
        }
        if idx == 0 { 0 } else { idx + 1 }
    };
    let omitted = tail_start.saturating_sub(head_end);
    (format!("{head}\n… {omitted} chars omitted …\n{tail}"), omitted)
}

/// What an exit code means beyond pass/fail.
pub fn exit_note(code: Option<i32>, raw: &str) -> &'static str {
    match code {
        None => "running",
        Some(124) => "timed out",
        Some(137) => "killed (SIGKILL — often out of memory)",
        Some(143) => "terminated (SIGTERM)",
        // grep-style: exit 1 with nothing printed means "no matches"
        Some(1) if raw.trim().is_empty() || raw.to_ascii_lowercase().contains("no matches") => "no matches",
        _ => "",
    }
}

/// The bash result: a header the harness trusts, then the untrusted output.
pub fn fence(raw: &str, code: Option<i32>, log_path: &Path, offset: Option<u64>) -> String {
    let (body, omitted) = middle_out(raw);
    let exit = code.map(|c| c.to_string()).unwrap_or_else(|| "running".into());
    let off = offset.map(|o| format!(" offset={o}")).unwrap_or_default();
    let note = exit_note(code, raw);
    let note = if note.is_empty() || code.is_none() { String::new() } else { format!(" ({note})") };
    format!(
        "exit_code={exit}{note} log_path={} omitted_chars={omitted}{off}\n<<<UNTRUSTED_SHELL\n{body}\n>>>",
        log_path.display()
    )
}

pub fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill").args(["/pid", &pid.to_string(), "/t", "/f"]).output();
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).output();
        thread::sleep(Duration::from_millis(200));
        let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_out_counts_exactly_what_it_drops() {
        let raw: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let (body, omitted) = middle_out(&raw);
        let kept_head: String = (0..24).map(|i| format!("line {i}\n")).collect();
        let kept_tail: String = (76..100).map(|i| format!("line {i}\n")).collect();
        assert_eq!(omitted, raw.len() - kept_head.len() - kept_tail.len());
        assert!(body.starts_with("line 0\n") && body.ends_with("line 99"));
        assert!(body.contains(&format!("… {omitted} chars omitted …")));
        assert_eq!(middle_out("a\nb\n"), ("a\nb".to_string(), 0));
    }

    #[test]
    fn exit_codes_are_named() {
        assert_eq!(exit_note(Some(137), "x"), "killed (SIGKILL — often out of memory)");
        assert_eq!(exit_note(Some(143), "x"), "terminated (SIGTERM)");
        assert_eq!(exit_note(Some(124), ""), "timed out");
        assert_eq!(exit_note(Some(1), "  \n"), "no matches");
        assert_eq!(exit_note(Some(1), "error: boom"), "");
        assert_eq!(exit_note(Some(0), ""), "");
        let f = fence("", Some(137), Path::new("x.log"), Some(5));
        assert!(f.starts_with("exit_code=137 (killed"), "{f}");
        assert!(f.contains("offset=5") && f.contains("<<<UNTRUSTED_SHELL\n") && f.ends_with(">>>"), "{f}");
        let f = fence("partial", None, Path::new("x.log"), Some(7));
        assert!(f.starts_with("exit_code=running log_path="), "{f}");
    }

    #[test]
    fn job_ids_follow_the_command() {
        assert_eq!(job_id("npm run dev"), job_id("  npm run dev "));
        assert_ne!(job_id("npm run dev"), job_id("npm test"));
    }
}
