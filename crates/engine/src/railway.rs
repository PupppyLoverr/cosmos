//! Railway trial — the free 60-minute VM every user gets for
//! `ssh railway.new`. No Cosmos account exists upstream: the user's own SSH
//! key IS the credential (the same key always lands on the same box).
//!
//! Provisioning is a single non-interactive connect: Railway writes a JSON
//! manifest to stderr on the first connects — preview URL, build deadline,
//! and the claim link a human can open to keep the box. We capture that
//! manifest; errors (3/IP/day, region caps, missing key) surface verbatim —
//! never a retry loop.
//!
//! Exec rides the same channel: `ssh railway.new '<cmd>'` runs the command
//! inside the trial VM (coding agents are preinstalled on it).

use std::time::Duration;

use chrono::{DateTime, Utc};

/// The SSH target — the whole trial protocol lives behind this host alias.
pub const RAILWAY_HOST: &str = "railway.new";
/// Provisioning can take a few seconds on a cold connect; bound it.
const PROVISION_TIMEOUT: Duration = Duration::from_secs(45);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// What a successful provision hands back.
#[derive(Debug, Clone, Default)]
pub struct RailwayTrial {
    /// Build deadline (the 60-minute mark) — the seat's `expires_at`.
    pub expires_at: Option<DateTime<Utc>>,
    /// `https://railway.com/ssh-signup?code=…` — the human's claim link.
    pub claim_url: Option<String>,
    /// The box's preview URL (`https://preview-….up.railway.app`).
    pub preview_url: Option<String>,
    /// A human detail line for the probe record (e.g. "2 vCPU · 2 GB").
    pub detail: Option<String>,
}

/// Extract the trial fields from whatever Railway printed on stderr — the
/// documented JSON manifest first, then a URL-pattern fallback for the
/// human-format welcome banner.
fn parse_manifest(stderr: &str, assign_fallback_deadline: bool) -> RailwayTrial {
    let mut trial = RailwayTrial::default();
    // The manifest is one JSON object; tolerate leading/trailing noise and
    // multi-line pretty prints by trying every brace-balanced slice.
    for line in stderr.lines() {
        let trimmed = line.trim();
        let Some(start) = trimmed.find('{') else {
            continue;
        };
        let Some(end) = trimmed.rfind('}') else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&trimmed[start..=end]) else {
            continue;
        };
        let pick = |keys: &[&str]| -> Option<String> {
            keys.iter().find_map(|k| {
                value
                    .get(*k)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
        };
        trial.claim_url =
            trial
                .claim_url
                .or(pick(&["claimUrl", "claim_url", "claim", "signupUrl"]));
        trial.preview_url =
            trial
                .preview_url
                .or(pick(&["previewUrl", "preview_url", "preview", "url"]));
        if trial.expires_at.is_none() {
            for key in [
                "buildDeadline",
                "build_deadline",
                "deadline",
                "expiresAt",
                "expires_at",
            ] {
                if let Some(raw) = value.get(key) {
                    trial.expires_at = raw
                        .as_str()
                        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                        .or_else(|| {
                            raw.as_i64().and_then(|secs| {
                                // Epoch seconds or ms.
                                DateTime::from_timestamp_secs(if secs > 10_000_000_000 {
                                    secs / 1000
                                } else {
                                    secs
                                })
                            })
                        });
                    if trial.expires_at.is_some() {
                        break;
                    }
                }
            }
        }
        trial.detail = trial
            .detail
            .or_else(|| pick(&["region"]).map(|r| format!("region {r}")));
    }
    // Banner fallback — the welcome message prints `Claim:` and `Try it:`.
    for line in stderr.lines() {
        for word in line.split_whitespace() {
            let word = word.trim_matches(|c: char| !c.is_ascii() || c == ' ');
            if trial.claim_url.is_none()
                && (word.starts_with("https://railway.com/ssh-signup")
                    || word.starts_with("https://railway.app/ssh-signup"))
            {
                trial.claim_url = Some(word.to_string());
            }
            if trial.preview_url.is_none()
                && (word.contains(".up.railway.app") || word.contains(".railway.app"))
                && word.starts_with("https://")
            {
                trial.preview_url = Some(word.to_string());
            }
        }
    }
    // No deadline in the manifest → the documented 60-minute window.
    if assign_fallback_deadline && trial.expires_at.is_none() {
        trial.expires_at = Some(Utc::now() + chrono::Duration::minutes(60));
    }
    trial
}

/// The honest reason a connect failed, from ssh's own stderr.
fn ssh_failure(out: &crate::cloud::ExecOutcome) -> String {
    let stderr = out.stderr.trim();
    // Railway's own words first — "temporarily disabled", "sign up", rate
    // limits all arrive in the welcome/manifest text on stderr.
    let host_lines: Vec<&str> = stderr
        .lines()
        .filter(|l| {
            let l = l.to_lowercase();
            l.contains("temporarily disabled")
                || l.contains("try again")
                || l.contains("limit")
                || l.contains("sign up")
                || l.contains("capacity")
        })
        .collect();
    if let Some(line) = host_lines.first() {
        return line.trim().to_string();
    }
    if stderr.contains("identity") || stderr.contains("Permission denied") {
        return "no SSH key on this machine — run `ssh-keygen -t ed25519`, then retry".into();
    }
    if stderr.contains("Operation timed out") || stderr.contains("timed out") {
        return "railway.new unreachable (check IPv4 — the trial needs it)".into();
    }
    stderr
        .lines()
        .next()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .unwrap_or("railway.new refused the connection")
        .to_string()
}

/// One bounded `ssh railway.new` invocation: BatchMode refuses interactive
/// prompts (the trial is headless-friendly by design), `accept-new` trusts
/// the host key on first connect. `command` runs on the trial VM itself —
/// the box IS the ssh session's target.
async fn railway_ssh(
    command: &str,
    timeout: Duration,
) -> Result<crate::cloud::ExecOutcome, String> {
    let output = tokio::time::timeout(
        timeout,
        tokio::process::Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=12",
                "-o",
                "StrictHostKeyChecking=accept-new",
                // The trial VM serves an interactive TUI on a bare connect;
                // a non-interactive command runs it directly on the box.
                RAILWAY_HOST,
                command,
            ])
            .output(),
    )
    .await
    .map_err(|_| "ssh railway.new timed out — region may be saturated".to_string())?
    .map_err(|e| format!("could not run ssh: {e}"))?;
    Ok(crate::cloud::ExecOutcome {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_deadline_is_only_assigned_when_requested() {
        let without_fallback = parse_manifest(r#"{"claimUrl":"https://example.test"}"#, false);
        assert!(without_fallback.expires_at.is_none());

        let with_fallback = parse_manifest(r#"{"claimUrl":"https://example.test"}"#, true);
        assert!(with_fallback.expires_at.is_some());

        let explicit = parse_manifest(r#"{"expiresAt":"2030-01-01T00:00:00Z"}"#, true);
        assert_eq!(
            explicit.expires_at,
            Some("2030-01-01T00:00:00Z".parse().unwrap())
        );
    }
}

/// Provision-or-attach the trial: the SSH key identifies the box, so a
/// repeat connect reattaches to the SAME VM (until it dies or is claimed).
/// The manifest on stderr carries the deadline + claim link.
pub async fn provision() -> Result<RailwayTrial, String> {
    let out = railway_ssh("echo __cosmos_alive && uname -sm && pwd", PROVISION_TIMEOUT).await?;
    let mut trial = parse_manifest(&out.stderr, true);
    if out.code != 0 || !out.stdout.contains("__cosmos_alive") {
        return Err(ssh_failure(&out));
    }
    if trial.detail.is_none() {
        trial.detail = out
            .stdout
            .lines()
            .nth(1)
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|s| s.to_string());
    }
    Ok(trial)
}

/// Cheap liveness + manifest refresh — the deadline can move when a box is
/// claimed or reattached.
pub async fn probe() -> Result<RailwayTrial, String> {
    probe_with_existing_deadline(false).await
}

pub async fn probe_with_existing_deadline(existing_deadline: bool) -> Result<RailwayTrial, String> {
    let out = railway_ssh("echo __cosmos_alive", PROBE_TIMEOUT).await?;
    let trial = parse_manifest(&out.stderr, !existing_deadline);
    if out.code != 0 {
        return Err(ssh_failure(&out));
    }
    if !out.stdout.contains("__cosmos_alive") {
        return Err("railway.new answered without the echo marker".into());
    }
    Ok(trial)
}
