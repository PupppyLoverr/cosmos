//! The cloud-seat backend: a `ComputerBackend` that forwards observe/act to a
//! remote `cua-driver` HTTP endpoint running ON the seat (a `cosmos-cloud-*`
//! container's published port, or any user-stamped endpoint URL). Same request
//! and response shapes as the local driver's MCP tools — the seat speaks the
//! service's own wire, so nothing upstream has to know the difference.
//!
//! Honest-by-default: with no endpoint configured the backend reports
//! `driver_unavailable` for every call — the Computer pane renders that as
//! "seat not controllable yet" rather than pretending a desktop exists.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::backend::{
    ActionOutcome, ActionRequest, BackendDescriptor, ComputerBackend, InspectKind, ObserveRequest,
    VerifyOutcome, WindowState,
};
use crate::{ComputerError, Observation, StagedScreenshot};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A `ComputerBackend` over an HTTP endpoint. Constructed per seat when the
/// seat config carries `cua_endpoint`; the token (if any) is a Bearer.
pub struct RemoteBackend {
    client: reqwest::Client,
    endpoint: String,
    token: Option<String>,
    /// Seat label — surfaces in `computer_status` so a harness can tell which
    /// machine it is looking at.
    seat: String,
    /// Last `ping` outcome; the service drops dead backends, so track it here
    /// the same way `CuaBackend` tracks its child process.
    alive: AtomicBool,
}

impl RemoteBackend {
    pub fn new(
        endpoint: impl Into<String>,
        token: Option<String>,
        seat: impl Into<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            endpoint: endpoint.into().trim_end_matches('/').to_string(),
            token,
            seat: seat.into(),
            alive: AtomicBool::new(true),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.endpoint, path)
    }

    fn authed(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    async fn post<Req: serde::Serialize, Res: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &Req,
    ) -> Result<Res, ComputerError> {
        let response = self
            .authed(self.client.post(self.url(path)).json(body))
            .send()
            .await
            .map_err(|e| {
                self.alive.store(false, Ordering::Relaxed);
                ComputerError::Unavailable(format!("seat {} unreachable: {e}", self.seat))
            })?;
        self.alive.store(true, Ordering::Relaxed);
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            let text = text
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>();
            return Err(ComputerError::Unavailable(format!(
                "seat {} answered {status}: {text}",
                self.seat
            )));
        }
        response.json::<Res>().await.map_err(|e| {
            ComputerError::Internal(format!(
                "seat {} returned unreadable {path}: {e}",
                self.seat
            ))
        })
    }
}

/// Wire shape of a remote observation — mirrors the local driver's
/// `Observation` one-for-one so `computer_observe` digests read identically.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteObservation {
    #[serde(flatten)]
    observation: Observation,
    /// Screenshot PNG as base64 when the endpoint stages server-side. The
    /// backend materializes it into the caller's `screenshot_dir` so the
    /// uploads-jail path stays true.
    #[serde(default)]
    screenshot_png_b64: Option<String>,
}

#[async_trait::async_trait]
impl ComputerBackend for RemoteBackend {
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    async fn ping(&self) -> Result<(), ComputerError> {
        let value: serde_json::Value = self.post("/computer/ping", &serde_json::json!({})).await?;
        let _ = value;
        Ok(())
    }

    async fn observe(&self, request: &ObserveRequest) -> Result<Observation, ComputerError> {
        let scope = match &request.scope {
            crate::backend::ObserveScope::Frontmost => serde_json::json!("frontmost"),
            crate::backend::ObserveScope::Window { pid, window_id } => {
                serde_json::json!({"window": {"pid": pid, "windowId": window_id}})
            }
            crate::backend::ObserveScope::Desktop => serde_json::json!("desktop"),
        };
        let mut remote: RemoteObservation = self
            .post(
                "/computer/observe",
                &serde_json::json!({
                    "scope": scope,
                    "semantic": request.semantic,
                    "screenshot": request.screenshot,
                    "maxElements": request.max_elements,
                    "fullTree": request.full_tree,
                    "maxDimension": request.max_dimension,
                }),
            )
            .await?;
        if request.screenshot
            && remote.observation.screenshot.is_none()
            && let Some(b64) = remote.screenshot_png_b64.take()
        {
            remote.observation.screenshot = stage_png(&request.screenshot_dir, &b64, 0, 0)
                .ok()
                .flatten();
        }
        Ok(remote.observation)
    }

    async fn window_state(
        &self,
        pid: i64,
        window_id: Option<i64>,
        semantic: bool,
        screenshot: bool,
        max_elements: u32,
        max_dimension: u32,
        screenshot_dir: &Path,
        query: Option<&str>,
    ) -> Result<WindowState, ComputerError> {
        let state: WindowState = self
            .post(
                "/computer/window_state",
                &serde_json::json!({
                    "pid": pid,
                    "windowId": window_id,
                    "semantic": semantic,
                    "screenshot": screenshot,
                    "maxElements": max_elements,
                    "maxDimension": max_dimension,
                    "query": query,
                }),
            )
            .await?;
        // Screenshots staged seat-side arrive via the `observe` path's
        // `screenshotPngB64`; window_state's dir stays unused until the
        // remote driver stages per-window shots the same way.
        let _ = screenshot_dir;
        let _ = screenshot;
        Ok(state)
    }

    async fn verify(
        &self,
        pid: i64,
        window_id: Option<i64>,
        expect: serde_json::Value,
    ) -> Result<VerifyOutcome, ComputerError> {
        self.post(
            "/computer/verify",
            &serde_json::json!({"pid": pid, "windowId": window_id, "expect": expect}),
        )
        .await
    }

    async fn inspect(&self, kind: &InspectKind) -> Result<serde_json::Value, ComputerError> {
        let params = match kind {
            InspectKind::Apps => serde_json::json!({"kind": "apps"}),
            InspectKind::Windows {
                pid,
                on_screen_only,
            } => serde_json::json!({"kind": "windows", "pid": pid, "onScreenOnly": on_screen_only}),
        };
        self.post("/computer/inspect", &params).await
    }

    async fn act(&self, request: &ActionRequest) -> Result<ActionOutcome, ComputerError> {
        let outcome: ActionOutcome = self
            .post(
                "/computer/act",
                &serde_json::json!({
                    "tool": request.tool,
                    "args": request.args,
                    "delivery": request.delivery.as_str(),
                    "session": request.session,
                    "observe": {
                        "screenshot": request.observe_screenshot,
                        "semantic": request.observe_semantic,
                        "maxElements": request.max_elements,
                        "maxDimension": request.max_dimension,
                    },
                }),
            )
            .await?;
        Ok(outcome)
    }

    async fn health_report(&self) -> Option<serde_json::Value> {
        self.post("/computer/health_report", &serde_json::json!({}))
            .await
            .ok()
    }

    fn describe(&self) -> BackendDescriptor {
        BackendDescriptor {
            kind: "cloud-seat",
            version: None,
            platform: "linux",
            semantic: true,
            screenshots: true,
            background_control: true,
            foreground_control: false,
        }
    }
}

/// Decode a base64 PNG into `screenshot-<uuid>.png` under `dir`; the returned
/// `StagedScreenshot` records the absolute host path, exactly like the local
/// driver's staged files.
fn stage_png(
    dir: &Path,
    b64: &str,
    width: u32,
    height: u32,
) -> Result<Option<StagedScreenshot>, ComputerError> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| ComputerError::Internal(format!("seat screenshot is not base64: {e}")))?;
    if dir.as_os_str().is_empty() {
        return Ok(None);
    }
    std::fs::create_dir_all(dir).map_err(|e| ComputerError::Internal(e.to_string()))?;
    let path = dir.join(format!("screenshot-{}.png", uuid::Uuid::new_v4()));
    std::fs::write(&path, &bytes).map_err(|e| ComputerError::Internal(e.to_string()))?;
    Ok(Some(StagedScreenshot {
        path,
        width: (width > 0).then_some(width),
        height: (height > 0).then_some(height),
        bytes: bytes.len() as u64,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_reports_a_cloud_seat() {
        let backend = RemoteBackend::new("http://127.0.0.1:1", None, "vps-1");
        let d = backend.describe();
        assert_eq!(d.kind, "cloud-seat");
        assert!(d.background_control);
        assert!(!d.foreground_control);
    }

    #[tokio::test]
    async fn dead_endpoint_is_unavailable_not_panicked() {
        let backend = RemoteBackend::new("http://127.0.0.1:1", None, "vps-1");
        let err = backend.ping().await.unwrap_err();
        assert!(matches!(err, ComputerError::Unavailable(_)));
        assert!(!backend.is_alive());
    }
}
