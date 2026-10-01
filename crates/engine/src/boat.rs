//! Boat (boat.dev) — the hosted cloud PC behind `CloudSeatKind::Box`.
//!
//! Thin reqwest client over the Boat Public API v1 — no SDK (binary budget).
//! Every call is bearer-authenticated with the user's `boat_…` API key, which
//! lives in `cloud-keys.json` (never the workspace doc, never the RPC view).
//!
//! Surface used:
//! - `GET /sandboxes`, `POST /sandboxes`, `GET /sandboxes/{id}` — lifecycle.
//! - `POST /sandboxes/{id}/stop|resume`, `DELETE /sandboxes/{id}` — sleeping
//!   (snapshot + billing off), wake, destroy.
//! - `POST /sandboxes/{id}/commands` (+ `/commands/{pid}`) — exec, the
//!   seat_exec backing for Boat seats; `stream: true` for the run tail.
//! - `GET|PUT /sandboxes/{id}/files` — the Files pane.
//! - `POST /sandboxes/{id}/prompt`, `GET …/events`, `POST …/steer`,
//!   `POST …/interrupt` — the integrated-agent run surface (the harness runs
//!   ON the box, satisfying the 24/7 rule).
//! - `POST /sandboxes/{id}/desktop` — the preview URL (browser, not embedded).
//! - `POST /sandboxes/{id}/sshkey` — registers the user's public key so
//!   `sshEndpoint` gives the Shell pane a real PTY command.

use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

pub const DEFAULT_BASE: &str = "https://boat.dev/api/v1";
const TIMEOUT: Duration = Duration::from_secs(30);
/// Command execution caps at 600s per Boat's API contract.
const EXEC_CAP: Duration = Duration::from_secs(600);

fn command_http_timeout(requested: Duration) -> Duration {
    requested.min(EXEC_CAP) + Duration::from_secs(5)
}

pub struct Boat<'a> {
    http: &'a reqwest::Client,
    token: &'a str,
    base: String,
}

pub fn boat<'a>(http: &'a reqwest::Client, token: &'a str, base: Option<&str>) -> Boat<'a> {
    Boat {
        http,
        token,
        base: base
            .filter(|b| !b.trim().is_empty())
            .unwrap_or(DEFAULT_BASE)
            .trim_end_matches('/')
            .to_string(),
    }
}

/// The sandbox fields Cosmos reads. Unknown fields are ignored so Boat can
/// add columns without breaking us.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BoatSandbox {
    pub id: String,
    pub name: String,
    /// init | provisioning | provisioned | cloning | ready | idle | running |
    /// archiving | archived | error | cancelled
    pub state: String,
    pub error: Option<String>,
    pub ssh_endpoint: Option<String>,
    pub desktop_url: Option<String>,
    pub desktop_available: bool,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub archive_after: Option<chrono::DateTime<chrono::Utc>>,
}

impl BoatSandbox {
    /// Ready to accept work (`ready`/`idle`/`running` — and `provisioned`,
    /// which answers commands while finishing first-boot tasks).
    pub fn usable(&self) -> bool {
        matches!(
            self.state.as_str(),
            "provisioned" | "ready" | "idle" | "running"
        )
    }

    /// Snapshot-archived: billing off, resumable — the "Sleeping" state.
    pub fn sleeping(&self) -> bool {
        matches!(self.state.as_str(), "archived" | "archiving")
    }

    /// Terminal or never-usable — the seat needs a fresh sandbox.
    pub fn dead(&self) -> bool {
        matches!(self.state.as_str(), "error" | "cancelled")
    }
}

/// `POST /commands` synchronous reply.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BoatCommandResult {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// `detached: true` returns this instead of a finished result.
    pub process_id: Option<i64>,
}

/// One frame of an `events` poll.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BoatEvent {
    pub cursor: Option<String>,
    /// Event payloads differ per type; the run driver fishes `text`/`tools`
    /// out of `data` rather than enumerating every kind.
    pub data: Option<serde_json::Value>,
    #[serde(alias = "type")]
    pub kind: Option<String>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl<'a> Boat<'a> {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// Unpack Boat's `{ok, type, error:{message}, …}` envelope: the error
    /// message is the human line (verbatim — never secret-bearing fields).
    async fn read<T: DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<serde_json::Value, String> {
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("boat read failed: {e}"))?;
        let value: serde_json::Value = serde_json::from_str(&body).map_err(|_| {
            format!(
                "boat HTTP {status}: {}",
                body.chars().take(200).collect::<String>()
            )
        })?;
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err("Boat token rejected — check it in Settings → Cloud computer".into());
        }
        if !status.is_success() && status.as_u16() != 202 {
            let message = value
                .get("error")
                .and_then(|e| e.get("message").cloned())
                .or_else(|| value.get("message").cloned())
                .and_then(|m| m.as_str().map(str::to_string))
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(format!("Boat: {message}"));
        }
        let _ = std::marker::PhantomData::<T>;
        Ok(value)
    }

    async fn get(&self, path: &str) -> Result<serde_json::Value, String> {
        let resp = self
            .http
            .get(self.url(path))
            .bearer_auth(self.token)
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("boat unreachable: {e}"))?;
        self.read::<serde_json::Value>(resp).await
    }

    async fn post<B: Serialize>(
        &self,
        path: &str,
        body: &B,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        let resp = self
            .http
            .post(self.url(path))
            .bearer_auth(self.token)
            .timeout(timeout)
            .json(body)
            .send()
            .await
            .map_err(|e| format!("boat unreachable: {e}"))?;
        self.read::<serde_json::Value>(resp).await
    }

    /// Token check — `GET /me` answers the account's GitHub identity.
    pub async fn verify(&self) -> Result<String, String> {
        let v = self.get("/me").await?;
        Ok(v.get("login")
            .or_else(|| v.get("user").and_then(|u| u.get("login")))
            .and_then(|l| l.as_str())
            .map(|l| format!("boat key works — {l}"))
            .unwrap_or_else(|| "boat key works".into()))
    }

    /// `GET /sandboxes` — the account's live boxes.
    pub async fn sandboxes(&self) -> Result<Vec<BoatSandbox>, String> {
        let v = self.get("/sandboxes?limit=50").await?;
        let items = v
            .get("sandboxes")
            .or_else(|| v.get("items"))
            .and_then(|s| s.as_array())
            .cloned()
            .ok_or_else(|| {
                "boat: unexpected response from /sandboxes: missing sandboxes".to_string()
            })?;
        Ok(items
            .into_iter()
            .enumerate()
            .map(|(ix, s)| {
                serde_json::from_value(s).map_err(|e| {
                    format!("boat: unexpected response from /sandboxes: invalid item {ix}: {e}")
                })
            })
            .collect::<Result<Vec<_>, _>>()?)
    }

    pub async fn sandbox(&self, id: &str) -> Result<BoatSandbox, String> {
        let v = self.get(&format!("/sandboxes/{id}")).await?;
        let sandbox = v.get("sandbox").cloned().unwrap_or(v);
        serde_json::from_value(sandbox).map_err(|e| format!("boat sandbox shape: {e}"))
    }

    /// `POST /sandboxes` — provision a persistent box (`ttlSeconds: null`
    /// disables auto-stop: the user owns the lifecycle via Sleep/Destroy).
    /// `idempotency` makes a retry after a lost reply safe.
    pub async fn create(&self, name: &str, idempotency: &str) -> Result<BoatSandbox, String> {
        let resp = self
            .http
            .post(self.url("/sandboxes"))
            .bearer_auth(self.token)
            .header("Idempotency-Key", idempotency)
            .timeout(TIMEOUT)
            .json(&serde_json::json!({ "name": name, "ttlSeconds": null }))
            .send()
            .await
            .map_err(|e| format!("boat unreachable: {e}"))?;
        let v = self.read::<serde_json::Value>(resp).await?;
        let sandbox = v.get("sandbox").cloned().unwrap_or(v);
        serde_json::from_value(sandbox).map_err(|e| {
            format!("boat: unexpected response from /sandboxes: missing or invalid sandbox: {e}")
        })
    }

    /// Poll until usable/dead — provisioning is seconds; cap at 3 minutes.
    pub async fn wait_ready(&self, id: &str) -> Result<BoatSandbox, String> {
        for _ in 0..36 {
            let sandbox = self.sandbox(id).await?;
            if sandbox.usable() {
                return Ok(sandbox);
            }
            if sandbox.dead() {
                return Err(sandbox
                    .error
                    .unwrap_or_else(|| "sandbox failed to provision".into()));
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        Err("sandbox still provisioning after 3 minutes".into())
    }

    /// `POST /sandboxes/{id}/stop` — snapshot & archive: billing off.
    pub async fn stop(&self, id: &str) -> Result<(), String> {
        self.post(
            &format!("/sandboxes/{id}/stop"),
            &serde_json::json!({}),
            TIMEOUT,
        )
        .await?;
        Ok(())
    }

    /// `POST /sandboxes/{id}/resume` — wake a sleeping box.
    pub async fn resume(&self, id: &str) -> Result<BoatSandbox, String> {
        self.post(
            &format!("/sandboxes/{id}/resume"),
            &serde_json::json!({}),
            TIMEOUT,
        )
        .await?;
        self.wait_ready(id).await
    }

    /// `DELETE /sandboxes/{id}` — the API requires the id echoed back in
    /// `X-Ascii-Confirm-Delete`, which doubles as our confirm check.
    pub async fn destroy(&self, id: &str) -> Result<(), String> {
        let resp = self
            .http
            .delete(self.url(&format!("/sandboxes/{id}")))
            .bearer_auth(self.token)
            .header("X-Ascii-Confirm-Delete", id)
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(|e| format!("boat unreachable: {e}"))?;
        self.read::<serde_json::Value>(resp).await?;
        Ok(())
    }

    /// `POST /sandboxes/{id}/commands` — synchronous exec (≤600s).
    pub async fn exec(
        &self,
        id: &str,
        command: &str,
        cwd: Option<&str>,
        timeout: Duration,
    ) -> Result<BoatCommandResult, String> {
        let secs = timeout.as_secs().clamp(1, 600);
        let v = self
            .post(
                &format!("/sandboxes/{id}/commands"),
                &serde_json::json!({
                    "command": command,
                    "cwd": cwd,
                    "timeoutSeconds": secs,
                }),
                command_http_timeout(timeout),
            )
            .await?;
        serde_json::from_value(v).map_err(|e| format!("boat command shape: {e}"))
    }

    /// Detached exec — returns the process id to poll.
    pub async fn exec_detached(
        &self,
        id: &str,
        command: &str,
        cwd: Option<&str>,
    ) -> Result<i64, String> {
        let v = self
            .post(
                &format!("/sandboxes/{id}/commands"),
                &serde_json::json!({
                    "command": command,
                    "cwd": cwd,
                    "detached": true,
                }),
                TIMEOUT,
            )
            .await?;
        let result: BoatCommandResult = serde_json::from_value(v).map_err(|e| {
            format!(
                "boat: unexpected response from /sandboxes/{id}/commands: missing processId: {e}"
            )
        })?;
        result.process_id.ok_or_else(|| {
            format!("boat: unexpected response from /sandboxes/{id}/commands: missing processId")
        })
    }

    /// `GET /sandboxes/{id}/commands/{pid}` — detached status + logs.
    pub async fn command_status(&self, id: &str, pid: i64) -> Result<serde_json::Value, String> {
        self.get(&format!("/sandboxes/{id}/commands/{pid}")).await
    }

    /// `PUT /sandboxes/{id}/files?path=` — write raw bytes (used to land the
    /// base64 text of a workspace snapshot before exec extracts it).
    pub async fn write_file(&self, id: &str, path: &str, bytes: &[u8]) -> Result<(), String> {
        let resp = self
            .http
            .put(self.url(&format!("/sandboxes/{id}/files?path={}", urlencoding(path))))
            .bearer_auth(self.token)
            .header("Content-Type", "application/octet-stream")
            .timeout(TIMEOUT)
            .body(bytes.to_vec())
            .send()
            .await
            .map_err(|e| format!("boat unreachable: {e}"))?;
        self.read::<serde_json::Value>(resp).await?;
        Ok(())
    }

    /// `GET /sandboxes/{id}/files?path=` — file contents (utf8).
    pub async fn read_file(&self, id: &str, path: &str) -> Result<String, String> {
        let v = self
            .get(&format!("/sandboxes/{id}/files?path={}", urlencoding(path)))
            .await?;
        v.get("content")
            .or_else(|| v.get("data"))
            .and_then(|c| c.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                format!("boat: unexpected response from /sandboxes/{id}/files: missing content")
            })
    }

    /// `POST /sandboxes/{id}/prompt` — the integrated-agent surface: runs a
    /// coding harness ON the box. Returns the conversation/prompt handle the
    /// run driver polls via `/events` + `/prompts/{promptId}`.
    pub async fn prompt(
        &self,
        id: &str,
        provider: &str,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        self.post(
            &format!("/sandboxes/{id}/prompt"),
            &serde_json::json!({
                "provider": provider,
                "prompt": prompt,
                "model": model,
                "new": true,
            }),
            TIMEOUT,
        )
        .await
    }

    /// `GET /sandboxes/{id}/events?cursor=` — poll-based event tail (the
    /// transcript mirror for prompt-driven runs).
    pub async fn events(
        &self,
        id: &str,
        cursor: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let path = match cursor {
            Some(c) => format!("/sandboxes/{id}/events?cursor={c}"),
            None => format!("/sandboxes/{id}/events"),
        };
        self.get(&path).await
    }

    /// `POST /sandboxes/{id}/steer` — mid-turn nudge (the Approve button's
    /// honest backing on Boat seats).
    pub async fn steer(&self, id: &str, prompt: &str) -> Result<(), String> {
        self.post(
            &format!("/sandboxes/{id}/steer"),
            &serde_json::json!({ "prompt": prompt }),
            TIMEOUT,
        )
        .await?;
        Ok(())
    }

    /// `POST /sandboxes/{id}/interrupt` — stop the running turn (other
    /// conversations on the box keep running).
    pub async fn interrupt(&self, id: &str) -> Result<(), String> {
        self.post(
            &format!("/sandboxes/{id}/interrupt"),
            &serde_json::json!({}),
            TIMEOUT,
        )
        .await?;
        Ok(())
    }

    /// `POST /sandboxes/{id}/desktop` — the live desktop stream URL. It is
    /// secret-bearing: returned to the UI for `open_url`, never logged or
    /// persisted.
    pub async fn desktop(&self, id: &str) -> Result<String, String> {
        let v = self
            .post(
                &format!("/sandboxes/{id}/desktop"),
                &serde_json::json!({}),
                TIMEOUT,
            )
            .await?;
        v.get("desktopUrl")
            .or_else(|| v.get("url"))
            .and_then(|u| u.as_str())
            .map(str::to_string)
            .ok_or_else(|| "boat desktop not ready — try again in a moment".into())
    }

    /// `POST /sandboxes/{id}/sshkey` — register the user's public key so the
    /// sandbox's `sshEndpoint` accepts it (the Shell pane's PTY path).
    pub async fn register_ssh_key(&self, id: &str, public_key: &str) -> Result<(), String> {
        self.post(
            &format!("/sandboxes/{id}/sshkey"),
            &serde_json::json!({ "publicKey": public_key }),
            TIMEOUT,
        )
        .await?;
        Ok(())
    }
}

/// Minimal percent-encoding for a single query value — enough for paths.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_timeout_is_capped_and_has_network_grace() {
        assert_eq!(
            command_http_timeout(Duration::from_secs(20)),
            Duration::from_secs(25)
        );
        assert_eq!(
            command_http_timeout(Duration::from_secs(900)),
            Duration::from_secs(605)
        );
    }
}
