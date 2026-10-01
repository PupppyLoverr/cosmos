//! First-run bookkeeping. The onboarding flow itself lives in a separate
//! `cosmostui-welcome` executable next to `cosmostui`, which launches it on the first
//! interactive start and deletes it once the flow completes, so the
//! intro/sound/setup code never ships in the binary you run every day.
//!
//! What stays here is tiny: the `onboarded.json` marker (one `exists()`
//! check per launch) and the operator's name for the greeting line.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Exit code `cosmostui-welcome` uses for Ctrl+C — the operator wants out.
pub const QUIT: i32 = 130;

fn marker_path() -> PathBuf {
    crate::cli_home().join("onboarded.json")
}

/// The pre-rename marker. Left in place (zagy reads it) and still honoured.
fn legacy_marker() -> PathBuf {
    crate::cli_home().join("zcli-onboarded.json")
}

fn marker_read() -> Option<String> {
    std::fs::read_to_string(marker_path())
        .or_else(|_| std::fs::read_to_string(legacy_marker()))
        .ok()
}

/// True while onboarding hasn't completed (and isn't disabled).
pub fn needed() -> bool {
    crate::env_var("NO_ONBOARDING").is_none() && !marker_path().exists() && !legacy_marker().exists()
}

/// The name entered during onboarding.
pub fn user_name() -> Option<String> {
    let text = marker_read()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let n = v["name"].as_str()?.trim().to_string();
    (!n.is_empty()).then_some(n)
}

/// Record a finished onboarding.
pub fn mark_done(name: &str, model: Option<&str>) {
    let p = marker_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let _ = std::fs::write(
        &p,
        serde_json::to_vec_pretty(&serde_json::json!({"v": 1, "name": name, "model": model, "at": at}))
            .unwrap_or_default(),
    );
}

/// `cosmostui-welcome` beside the running executable, if it's still there.
pub fn exe() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.parent()?.join(format!("cosmostui-welcome{}", std::env::consts::EXE_SUFFIX));
    p.is_file().then_some(p)
}

pub enum Outcome {
    /// flow finished; the executable has been removed
    Done,
    /// Ctrl+C inside the flow
    Quit,
    /// no `cosmostui-welcome` next to cosmostui (already retired)
    Missing,
    /// it failed to start or crashed — carry on without it
    Failed(String),
}

/// Run `cosmostui-welcome` in this terminal and wait for it. On success the
/// executable is deleted — onboarding retires itself.
pub fn launch() -> Outcome {
    let Some(exe) = exe() else { return Outcome::Missing };
    match Command::new(&exe).status() {
        Ok(s) if s.success() => {
            retire();
            Outcome::Done
        }
        Ok(s) if s.code() == Some(QUIT) => Outcome::Quit,
        Ok(s) => Outcome::Failed(format!("cosmostui-welcome exited with {s}")),
        Err(e) => Outcome::Failed(format!("couldn't start cosmostui-welcome: {e}")),
    }
}

/// Delete `cosmostui-welcome` once onboarding is done. `COSMOS_KEEP_WELCOME=1`
/// keeps it (development). Windows can hold the image for a moment after
/// the process exits, so retry briefly.
pub fn retire() {
    if crate::env_var("KEEP_WELCOME").is_some() {
        return;
    }
    let Some(exe) = exe() else { return };
    if std::fs::remove_file(&exe).is_ok() {
        return;
    }
    // still locked — retry off the startup path
    std::thread::spawn(move || {
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(50));
            if std::fs::remove_file(&exe).is_ok() || !exe.exists() {
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_round_trip() {
        let _env = crate::env_lock();
        let dir = std::env::temp_dir().join(format!("cosmostui-welcome-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("COSMOS_CLI_HOME", &dir);
        std::env::remove_var("ZCLI_NO_ONBOARDING");
        std::env::remove_var("COSMOS_NO_ONBOARDING");
        assert!(needed());
        assert_eq!(user_name(), None);
        mark_done("rav", Some("anthropic/claude-opus-5-5"));
        assert!(!needed());
        assert_eq!(user_name().as_deref(), Some("rav"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
