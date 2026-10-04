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
    /// Railway requires the user to claim the expired box before it can be
    /// reached again.
    pub claim_required: bool,
    /// Build deadline (the 60-minute mark) — the seat's `expires_at`.
    pub expires_at: Option<DateTime<Utc>>,
    /// `https://railway.com/ssh-signup?code=…` — the human's claim link.
    pub claim_url: Option<String>,
    /// `https://railway.com/ssh-signup?code=…` — the human's signup link.
    pub signup_url: Option<String>,
    /// Railway's long-poll URL for signup completion.
    pub poll_url: Option<String>,
    /// Railway refused this trial until the visitor signs up.
    pub signup_required: bool,
    /// The box's preview URL (`https://preview-….up.railway.app`).
    pub preview_url: Option<String>,
    /// A human detail line for the probe record (e.g. "2 vCPU · 2 GB").
    pub detail: Option<String>,
}

pub fn trusted_railway_url(raw_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(raw_url) else {
        return false;
    };
    let has_userinfo = raw_url
        .split_once("://")
        .and_then(|(_, rest)| rest.split(['/', '?', '#']).next())
        .is_some_and(|authority| authority.contains('@'));
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && !has_userinfo
        && url
            .host_str()
            .is_some_and(|host| host == "railway.com" || host.ends_with(".railway.com"))
}

fn signup_poll_pending(body: &str) -> Option<bool> {
    let reply = serde_json::from_str::<serde_json::Value>(body).ok()?;
    let status = reply.get("status")?.as_str()?;
    Some(matches!(status, "signup_required" | "pending" | "waiting"))
}

pub async fn signup_status(http: &reqwest::Client, poll_url: &str) -> Result<bool, String> {
    if !trusted_railway_url(poll_url) {
        return Err("Railway sent an untrusted signup URL".into());
    }
    let response = match http
        .get(poll_url)
        .timeout(Duration::from_secs(70))
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) if error.is_timeout() => return Ok(true),
        Err(_) => return Err("Could not check Railway signup status".into()),
    };
    if !response.status().is_success() {
        return Err(format!(
            "Railway signup check failed (HTTP {})",
            response.status()
        ));
    }
    let body = match response.text().await {
        Ok(body) => body,
        Err(error) if error.is_timeout() => return Ok(true),
        Err(_) => return Err("Could not read Railway signup status".into()),
    };
    signup_poll_pending(&body).ok_or_else(|| "Railway sent an unexpected reply".into())
}

/// Extract the trial fields from whatever Railway printed on stderr — the
/// documented JSON manifest first, then a URL-pattern fallback for the
/// human-format welcome banner.
fn parse_manifest(stdout: &str, stderr: &str, assign_fallback_deadline: bool) -> RailwayTrial {
    let mut trial = RailwayTrial::default();
    let mut signup_refused = false;
    // The manifest is one JSON object; tolerate leading/trailing noise and
    // multi-line pretty prints by trying every brace-balanced slice.
    for line in stdout.lines().chain(stderr.lines()) {
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
        trial.claim_required |=
            value.get("status").and_then(|v| v.as_str()) == Some("claim_required");
        signup_refused |= value
            .get("status")
            .and_then(|v| v.as_str())
            .is_some_and(|status| matches!(status, "refused" | "signup_required"));
        let pick = |keys: &[&str]| -> Option<String> {
            keys.iter().find_map(|k| {
                value
                    .get(*k)
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
        };
        trial.claim_url = trial.claim_url.or(pick(&[
            "human_claim_url",
            "humanClaimUrl",
            "claimUrl",
            "claim_url",
            "claim",
        ]));
        trial.signup_url = trial.signup_url.or(pick(&[
            "human_signup_url",
            "humanSignupUrl",
            "signupUrl",
            "signup_url",
        ])
        .filter(|url| trusted_railway_url(url)));
        trial.poll_url = trial
            .poll_url
            .or(pick(&["poll_url", "pollUrl"]).filter(|url| trusted_railway_url(url)));
        trial.preview_url =
            trial
                .preview_url
                .or(pick(&["previewUrl", "preview_url", "preview", "url"]));
        if trial.expires_at.is_none() {
            for key in [
                "build_expires_at",
                "buildExpiresAt",
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
    trial.signup_required = signup_refused && trial.signup_url.is_some();
    // Banner fallback — the welcome message prints `Claim:` and `Try it:`.
    for line in stdout.lines().chain(stderr.lines()) {
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

/// Drop the lines of Railway's trial banner that carry the claim link or
/// code, so command output shown on screen never leaks them.
pub fn redact_trial_banner(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            !(line.contains("ssh-signup")
                || line.contains("claim_url")
                || line.contains("claimUrl")
                || line.contains("\"claim_code\"")
                || line.contains("\"claimCode\"")
                || line.contains("human_signup_url")
                || line.contains("humanSignupUrl")
                || line.contains("signup_url")
                || line.contains("signupUrl")
                || line.contains("poll_url")
                || line.contains("pollUrl"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A manifest line's human `message`/`error`/`reason`, or the raw line.
fn host_message(line: &str) -> String {
    let trimmed = line.trim();
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&trimmed[start..=end])
    {
        for key in ["message", "description", "error", "reason", "detail"] {
            if let Some(text) = value
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                return text.to_string();
            }
        }
    }
    trimmed.to_string()
}

/// The honest reason a connect failed, from ssh's own stderr.
fn ssh_failure(out: &crate::cloud::ExecOutcome) -> String {
    let stderr = out.stderr.trim();
    // Railway's own words first — "temporarily disabled", "sign up", rate
    // limits all arrive in the welcome/manifest text (stderr or stdout).
    let host_lines: Vec<String> = stderr
        .lines()
        .chain(out.stdout.lines())
        .map(host_message)
        .filter(|l| {
            let l = l.to_lowercase();
            l.contains("temporarily disabled")
                || l.contains("try again")
                || l.contains("limit")
                || l.contains("sign up")
                || l.contains("capacity")
        })
        .collect();
    if let Some(line) = host_lines
        .iter()
        .map(|line| redact_trial_banner(line.trim()))
        .find(|line| !line.is_empty())
    {
        return line;
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
    host: &str,
    command: &str,
    timeout: Duration,
) -> Result<crate::cloud::ExecOutcome, String> {
    let output = tokio::time::timeout(
        timeout,
        crate::cloud::ssh()
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=12",
                "-o",
                "StrictHostKeyChecking=accept-new",
                // The trial VM serves an interactive TUI on a bare connect;
                // a non-interactive command runs it directly on the box.
                host,
                command,
            ])
            .output(),
    )
    .await
    .map_err(|_| "ssh railway.new timed out — region may be saturated".to_string())?
    .map_err(crate::cloud::ssh_spawn_error)?;
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
        let without_fallback = parse_manifest("", r#"{"claimUrl":"https://example.test"}"#, false);
        assert!(without_fallback.expires_at.is_none());

        let with_fallback = parse_manifest("", r#"{"claimUrl":"https://example.test"}"#, true);
        assert!(with_fallback.expires_at.is_some());

        let explicit = parse_manifest("", r#"{"expiresAt":"2030-01-01T00:00:00Z"}"#, true);
        assert_eq!(
            explicit.expires_at,
            Some("2030-01-01T00:00:00Z".parse().unwrap())
        );
    }

    #[test]
    fn parses_current_trial_manifest_fields() {
        let trial = parse_manifest(
            "",
            r#"{"status":"trial_starting","human_claim_url":"https://example.test/claim","preview_url":"https://preview.example.test","build_expires_at":"2030-01-01T00:00:00Z"}"#,
            true,
        );
        assert_eq!(
            trial.claim_url.as_deref(),
            Some("https://example.test/claim")
        );
        assert_eq!(
            trial.preview_url.as_deref(),
            Some("https://preview.example.test")
        );
        assert_eq!(
            trial.expires_at,
            Some("2030-01-01T00:00:00Z".parse().unwrap())
        );
    }

    #[test]
    fn refused_manifest_surfaces_railway_message() {
        let out = crate::cloud::ExecOutcome {
            code: 0,
            stdout: r#"{"status":"refused","description":"limited. Sign up to keep building.","human_signup_url":"https://railway.com/ssh-signup?code=x"}"#.into(),
            stderr: String::new(),
        };
        assert_eq!(ssh_failure(&out), "limited. Sign up to keep building.");
    }

    #[test]
    fn redacts_claim_banner_lines() {
        let out = "hello\n{\"state\":\"expiring\",\"human_claim_url\":\"https://railway.com/ssh-signup?code=abc\"}\nworld";
        assert_eq!(redact_trial_banner(out), "hello\nworld");
    }

    #[test]
    fn parses_claim_required_manifest_from_stdout() {
        let trial = parse_manifest(
            r#"{"status":"claim_required","human_claim_url":"https://railway.com/ssh-signup?code=abc","poll_url":"https://backboard.railway.com/ssh-signup/poll?code=abc"}"#,
            "",
            true,
        );
        assert!(trial.claim_required);
        assert_eq!(
            trial.claim_url.as_deref(),
            Some("https://railway.com/ssh-signup?code=abc")
        );
        assert_eq!(
            trial.poll_url.as_deref(),
            Some("https://backboard.railway.com/ssh-signup/poll?code=abc")
        );

        let untrusted = parse_manifest(
            r#"{"status":"claim_required","poll_url":"https://example.test"}"#,
            "",
            false,
        );
        assert!(untrusted.poll_url.is_none());
    }

    #[test]
    fn parses_signup_refusal_and_trusted_poll_url() {
        let trial = parse_manifest(
            "",
            r#"{"status":"refused","human_signup_url":"https://railway.com/ssh-signup?code=abc","poll_url":"https://backboard.railway.com/ssh-signup/poll?code=abc"}"#,
            false,
        );
        assert!(trial.signup_required);
        assert_eq!(
            trial.signup_url.as_deref(),
            Some("https://railway.com/ssh-signup?code=abc")
        );
        assert_eq!(
            trial.poll_url.as_deref(),
            Some("https://backboard.railway.com/ssh-signup/poll?code=abc")
        );

        let aliases = parse_manifest(
            "",
            r#"{"status":"signup_required","signupUrl":"https://railway.com/signup?code=abc","pollUrl":"https://backboard.railway.com/poll?code=abc"}"#,
            false,
        );
        assert!(aliases.signup_required);
        assert_eq!(
            aliases.signup_url.as_deref(),
            Some("https://railway.com/signup?code=abc")
        );
        assert_eq!(
            aliases.poll_url.as_deref(),
            Some("https://backboard.railway.com/poll?code=abc")
        );

        let untrusted = parse_manifest(
            "",
            r#"{"status":"signup_required","signup_url":"https://railway.com.evil.test/sign-up","pollUrl":"https://evil.test/poll"}"#,
            false,
        );
        assert!(!untrusted.signup_required);
        assert!(untrusted.signup_url.is_none());
        assert!(untrusted.poll_url.is_none());
    }

    #[test]
    fn trusted_railway_url_requires_https_and_exact_domain_suffix() {
        assert!(trusted_railway_url("https://railway.com/ssh-signup"));
        assert!(trusted_railway_url("https://backboard.railway.com/poll"));
        assert!(!trusted_railway_url("http://railway.com/ssh-signup"));
        assert!(!trusted_railway_url(
            "https://railway.com.evil.test/ssh-signup"
        ));
        assert!(!trusted_railway_url(
            "https://railway.com.evil.com/ssh-signup"
        ));
        assert!(!trusted_railway_url(
            "https://user:pass@railway.com/ssh-signup"
        ));
        assert!(!trusted_railway_url("https://@railway.com/ssh-signup"));
    }

    #[test]
    fn signup_poll_status_recognizes_pending_states() {
        for status in ["signup_required", "pending", "waiting"] {
            assert_eq!(
                signup_poll_pending(&format!(r#"{{"status":"{status}"}}"#)),
                Some(true)
            );
        }
        assert_eq!(signup_poll_pending(r#"{"status":"approved"}"#), Some(false));
        assert_eq!(signup_poll_pending("not json"), None);
        assert_eq!(signup_poll_pending(r#"{"state":"waiting"}"#), None);
    }
}

#[derive(Debug, Clone)]
pub struct RailwayFailure {
    pub detail: String,
    pub trial: Box<RailwayTrial>,
}

fn claim_required_failure(trial: RailwayTrial) -> RailwayFailure {
    RailwayFailure {
        detail: "This machine's Railway trial box has expired — claim it on Railway to keep it, or use a Boat seat".into(),
        trial: Box::new(trial),
    }
}

fn ssh_failure_result(out: &crate::cloud::ExecOutcome, trial: RailwayTrial) -> RailwayFailure {
    if trial.claim_required {
        claim_required_failure(trial)
    } else {
        RailwayFailure {
            detail: ssh_failure(out),
            trial: Box::new(trial),
        }
    }
}

/// Provision-or-attach the trial: the SSH key identifies the box, so a
/// repeat connect reattaches to the SAME VM (until it dies or is claimed).
/// The manifest on stderr carries the deadline + claim link.
pub async fn provision() -> Result<RailwayTrial, RailwayFailure> {
    let out = railway_ssh(
        RAILWAY_HOST,
        "echo __cosmos_alive && uname -sm && pwd",
        PROVISION_TIMEOUT,
    )
    .await
    .map_err(|detail| RailwayFailure {
        detail,
        trial: Box::default(),
    })?;
    let mut trial = parse_manifest(&out.stdout, &out.stderr, true);
    if trial.claim_required {
        return Err(claim_required_failure(trial));
    }
    if out.code != 0 || !out.stdout.contains("__cosmos_alive") {
        return Err(ssh_failure_result(&out, trial));
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
pub async fn probe() -> Result<RailwayTrial, RailwayFailure> {
    probe_with_existing_deadline(RAILWAY_HOST, false).await
}

/// `host` is the seat's ssh alias (`railway.new` unless the seat names a
/// claimed box's own alias).
pub async fn probe_with_existing_deadline(
    host: &str,
    existing_deadline: bool,
) -> Result<RailwayTrial, RailwayFailure> {
    let out = railway_ssh(host, "echo __cosmos_alive", PROBE_TIMEOUT)
        .await
        .map_err(|detail| RailwayFailure {
            detail,
            trial: Box::default(),
        })?;
    let trial = parse_manifest(&out.stdout, &out.stderr, !existing_deadline);
    if trial.claim_required {
        return Err(claim_required_failure(trial));
    }
    if out.code != 0 {
        return Err(ssh_failure_result(&out, trial));
    }
    if !out.stdout.contains("__cosmos_alive") {
        let detail = ssh_failure(&out);
        return Err(RailwayFailure {
            detail: if detail.is_empty() {
                "railway.new answered without the echo marker".into()
            } else {
                detail
            },
            trial: Box::new(trial),
        });
    }
    Ok(trial)
}
