//! Host-local browser discovery. Viewports renew short leases; endpoints are
//! never persisted in chat configuration or synchronized to another device.
use std::time::{Duration, Instant};
use zeron_proto::{BROWSER_BRIDGE_OPTION, RunRequest};

const LEASE: Duration = Duration::from_secs(45);

#[derive(Default)]
pub(crate) struct BrowserRegistry {
    windows: Vec<(String, Instant)>,
}

impl BrowserRegistry {
    pub fn renew(&mut self, url: &str, now: Instant) -> Result<(), String> {
        let parsed = reqwest::Url::parse(url).map_err(|_| "Invalid browser endpoint")?;
        let token = parsed.path().strip_prefix("/mcp/").unwrap_or_default();
        if parsed.scheme() != "http"
            || parsed.host_str() != Some("127.0.0.1")
            || parsed.port().is_none_or(|port| port == 0)
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || token.len() != 32
            || !token.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("Browser endpoint must be a tokenized loopback URL".into());
        }
        self.windows.retain(|(_, deadline)| *deadline > now);
        if let Some((_, deadline)) = self.windows.iter_mut().find(|(key, _)| key == url) {
            *deadline = now + LEASE;
        } else {
            self.windows.push((url.to_owned(), now + LEASE));
        }
        Ok(())
    }

    pub fn prepare(&mut self, request: &mut RunRequest, now: Instant) {
        self.windows.retain(|(_, deadline)| *deadline > now);
        // Preserve a caller's window only if registered on THIS host. Queued
        // runs inherit a stable live window; stale/remote endpoints are removed.
        let preferred = zeron_proto::browser_bridge_url(request)
            .and_then(|url| self.windows.iter().find(|(key, _)| key == url))
            .or_else(|| self.windows.first())
            .map(|(url, _)| url.clone());
        request.model_options.remove(BROWSER_BRIDGE_OPTION);
        if let Some(url) = preferred {
            request.model_options.insert(
                BROWSER_BRIDGE_OPTION.into(),
                serde_json::json!({ "url": url }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const URL: &str = "http://127.0.0.1:43123/mcp/0123456789abcdef0123456789abcdef";
    fn request() -> RunRequest {
        serde_json::from_value(serde_json::json!({
            "prompt": "Browse the page", "cwd": ".", "sandbox": "workspace-write"
        }))
        .unwrap()
    }

    #[test]
    fn queued_runs_inherit_live_host_browser_without_changing_prompt() {
        let now = Instant::now();
        let mut registry = BrowserRegistry::default();
        registry.renew(URL, now).unwrap();
        let mut run = request();
        registry.prepare(&mut run, now);
        assert_eq!(zeron_proto::browser_bridge_url(&run), Some(URL));
        assert_eq!(run.prompt, "Browse the page");
        registry.prepare(&mut run, now + LEASE);
        assert_eq!(zeron_proto::browser_bridge_url(&run), None);
    }

    #[test]
    fn stale_endpoints_are_removed_and_windows_do_not_oscillate() {
        let now = Instant::now();
        let second = URL.replace("43123", "43124");
        let mut registry = BrowserRegistry::default();
        registry.renew(URL, now).unwrap();
        registry.renew(&second, now).unwrap();
        registry
            .renew(&second, now + Duration::from_secs(10))
            .unwrap();
        let mut run = request();
        registry.prepare(&mut run, now);
        assert_eq!(zeron_proto::browser_bridge_url(&run), Some(URL));
        run.model_options.insert(
            BROWSER_BRIDGE_OPTION.into(),
            serde_json::json!({"url": second}),
        );
        registry.prepare(&mut run, now);
        assert_eq!(zeron_proto::browser_bridge_url(&run), Some(second.as_str()));
        registry.prepare(&mut run, now + LEASE);
        assert_eq!(zeron_proto::browser_bridge_url(&run), Some(second.as_str()));
        BrowserRegistry::default().prepare(&mut run, now);
        assert_eq!(zeron_proto::browser_bridge_url(&run), None);
    }

    #[test]
    fn registration_rejects_nonlocal_and_malformed_endpoints() {
        let mut registry = BrowserRegistry::default();
        for url in [
            URL.replace("127.0.0.1", "example.com"),
            URL.replace("http:", "https:"),
            URL.replace(":43123", ""),
            format!("{URL}?token=x"),
            format!("{URL}/extra"),
            "http://127.0.0.1:43123/mcp/short".into(),
        ] {
            assert!(registry.renew(&url, Instant::now()).is_err(), "{url}");
        }
    }
}
