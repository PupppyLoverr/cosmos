//! Crate tests — routing, merging, failure isolation and secret hygiene run
//! against injected `MockUpstream`s; the endpoint test exercises the real
//! loopback HTTP path.

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::mock::MockUpstream;
use crate::{
    POPULAR_APPS, ProviderConfig, ToolError, ToolProvider, ToolService, mask_secret, masked_config,
    provider_slug, save_store, slugify,
};

fn provider(id: &str, name: &str, config: ProviderConfig) -> ToolProvider {
    ToolProvider {
        id: id.to_string(),
        name: name.to_string(),
        enabled: true,
        config,
        toolkits: Vec::new(),
    }
}

fn http_config() -> ProviderConfig {
    ProviderConfig::Http {
        url: "http://127.0.0.1:9/mcp".into(),
        headers: Default::default(),
    }
}

/// A service with the given providers stored and (optionally) mock
/// upstreams injected — exercises the real store/routing path.
fn service(
    providers: Vec<(ToolProvider, Option<Arc<MockUpstream>>)>,
) -> (ToolService, tempfile::TempDir) {
    let data = tempfile::tempdir().unwrap();
    let service = ToolService::new(data.path().into());
    for (provider, upstream) in providers {
        service.upsert_provider(provider.clone()).unwrap();
        if let Some(upstream) = upstream {
            let count = upstream.tool_count();
            service.inject_upstream(&provider.id, upstream, count);
        }
    }
    (service, data)
}

#[test]
fn slugify_normalizes_provider_names() {
    assert_eq!(slugify("GitHub"), "github");
    assert_eq!(slugify("My MCP Server!"), "my_mcp_server");
    assert_eq!(slugify("  "), "server");
    let composio = provider(
        "composio",
        "Composio",
        ProviderConfig::Composio {
            api_key: "ck_x".into(),
        },
    );
    assert_eq!(provider_slug(&composio), "composio");
    let github = provider("p1", "GitHub", http_config());
    assert_eq!(provider_slug(&github), "github");
}

#[test]
fn store_round_trip_and_corrupt_fallback() {
    let data = tempfile::tempdir().unwrap();
    let store = crate::ToolStore {
        providers: vec![
            provider(
                "composio",
                "Composio",
                ProviderConfig::Composio {
                    api_key: "ck_secret".into(),
                },
            ),
            provider("gh", "GitHub", http_config()),
        ],
    };
    save_store(data.path(), &store).unwrap();
    assert_eq!(crate::load_store(data.path()), store);
    std::fs::write(crate::store_path(data.path()), b"{not json").unwrap();
    assert_eq!(crate::load_store(data.path()), crate::ToolStore::default());
}

#[test]
fn masked_config_never_carries_secrets() {
    let config = ProviderConfig::Composio {
        api_key: "ck_ab1cd2ef3456".into(),
    };
    let masked = serde_json::to_string(&masked_config(&config)).unwrap();
    assert!(!masked.contains("ab1cd2ef"), "key body leaked: {masked}");
    assert!(masked.contains("ck_"));

    let http = ProviderConfig::Http {
        url: "https://mcp.example.com/".into(),
        headers: [("Authorization".into(), "Bearer tok_live_secret".into())]
            .into_iter()
            .collect(),
    };
    let masked = serde_json::to_string(&masked_config(&http)).unwrap();
    assert!(masked.contains("Authorization"), "header name missing");
    assert!(
        !masked.contains("tok_live"),
        "header value leaked: {masked}"
    );

    let stdio = ProviderConfig::Stdio {
        command: "server".into(),
        args: vec!["--key".into(), "s3cret".into()],
        env: [("API_KEY".into(), "s3cret".into())].into_iter().collect(),
    };
    let masked = serde_json::to_string(&masked_config(&stdio)).unwrap();
    assert!(masked.contains("API_KEY"), "env name missing");
    // Args are echoed for recognizability; env VALUES never are.
    assert!(!masked.contains("\"s3cret\"") || masked.contains("args"));
    let parsed: Value = serde_json::from_str(&masked).unwrap();
    let env_names = parsed["envNames"].as_array().unwrap();
    assert_eq!(env_names.len(), 1);
    assert_ne!(env_names[0].as_str().unwrap(), "s3cret");

    assert_eq!(mask_secret("ck_abcdef123456"), "ck_…3456");
}

#[test]
fn error_codes_are_stable() {
    assert_eq!(ToolError::Disabled.code(), "disabled");
    assert_eq!(
        ToolError::Unavailable("x".into()).code(),
        "upstream_unavailable"
    );
    assert_eq!(ToolError::refused("timeout", "m").code(), "timeout");
    assert_eq!(ToolError::InvalidKey("x".into()).code(), "invalid_key");
    let refused = ToolError::from_tool_error("connection_required: link the account");
    assert_eq!(refused.code(), "connection_required");
    assert!(ToolError::Unavailable("x".into()).retryable());
    assert!(!ToolError::Disabled.retryable());
}

#[tokio::test]
async fn dispatch_routes_by_prefix() {
    let github = Arc::new(
        MockUpstream::new(&["create_issue"])
            .with_result("create_issue", MockUpstream::text_result("issue created")),
    );
    let notion = Arc::new(
        MockUpstream::new(&["search"])
            .with_result("search", MockUpstream::text_result("notion results")),
    );
    let (service, _data) = service(vec![
        (
            provider("gh", "GitHub", http_config()),
            Some(github.clone()),
        ),
        (
            provider("nt", "Notion", http_config()),
            Some(notion.clone()),
        ),
    ]);

    let result = service
        .dispatch_tool("github__create_issue", json!({"title": "bug"}))
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "issue created");

    // The upstream saw the STRIPPED name and the args verbatim.
    let calls = github.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "create_issue");
    assert_eq!(calls[0].1["title"], "bug");
    drop(calls);
    assert!(notion.calls.lock().unwrap().is_empty());

    // Cross-provider and malformed names are refused, not misrouted.
    let err = service
        .dispatch_tool("notion__create_issue", json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::Refused { .. }));
    let err = service
        .dispatch_tool("no_prefix", json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArgs(_)));
    let err = service
        .dispatch_tool("unknown__tool", json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, ToolError::InvalidArgs(_)));
}

#[tokio::test]
async fn merged_tools_prefixes_and_isolates() {
    let github = Arc::new(MockUpstream::new(&["create_issue", "list_prs"]));
    let (service, _data) = service(vec![
        (provider("gh", "GitHub", http_config()), Some(github)),
        // No injected upstream AND an unconnectable config — a stdio
        // command that can't exist. Failure must not break the merge.
        (
            provider(
                "dead",
                "Dead Server",
                ProviderConfig::Stdio {
                    command: "cosmos-tools-definitely-not-a-binary".into(),
                    args: vec![],
                    env: Default::default(),
                },
            ),
            None,
        ),
        // A disabled provider contributes nothing.
        {
            let mut p = provider("off", "Off", http_config());
            p.enabled = false;
            (p, None)
        },
    ]);

    let tools = service.merged_tools().await;
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"github__create_issue"));
    assert!(names.contains(&"github__list_prs"));
    assert_eq!(
        tools.len(),
        2,
        "the dead and disabled providers leaked tools"
    );

    // The merged list annotates provenance.
    assert_eq!(tools[0]["x-provider"], "GitHub");

    // The failed provider's status surfaces without affecting the rest.
    let snapshot = service.snapshot();
    let dead = snapshot.providers.iter().find(|p| p.id == "dead").unwrap();
    assert_eq!(dead.status, "error");
    assert!(dead.last_error.is_some());
    let gh = snapshot.providers.iter().find(|p| p.id == "gh").unwrap();
    assert_eq!(gh.status, "connected");
    assert_eq!(snapshot.tool_count, 2);
}

#[tokio::test]
async fn tool_list_is_cached_per_upstream() {
    let upstream = Arc::new(MockUpstream::new(&["only_tool"]));
    let (service, _data) = service(vec![(
        provider("m", "Mock", http_config()),
        Some(upstream.clone()),
    )]);
    let first = service.merged_tools().await;
    let second = service.merged_tools().await;
    assert_eq!(first, second);
    // The upstream answered zero CALLS — list traffic hits the handshake
    // cache, never the wire.
    assert!(upstream.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn disabled_provider_refuses_calls() {
    let upstream = Arc::new(MockUpstream::new(&["t"]));
    let (service, _data) = service(vec![(
        provider("m", "Mock", http_config()),
        Some(upstream.clone()),
    )]);
    service.set_provider_enabled("m", false).unwrap();
    let err = service
        .dispatch_tool("mock__t", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err, ToolError::Disabled);
    // Disabling drops the cached upstream.
    assert_eq!(service.merged_tools().await.len(), 0);
    assert!(upstream.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn snapshot_masks_every_provider() {
    let (service, _data) = service(vec![
        (
            provider(
                "composio",
                "Composio",
                ProviderConfig::Composio {
                    api_key: "ck_livekey_body".into(),
                },
            ),
            Some(Arc::new(MockUpstream::new(&["COMPOSIO_SEARCH_TOOLS"]))),
        ),
        (
            provider(
                "gh",
                "GitHub",
                ProviderConfig::Http {
                    url: "https://api.githubcopilot.com/mcp/".into(),
                    headers: [("Authorization".into(), "Bearer ghp_secret".into())]
                        .into_iter()
                        .collect(),
                },
            ),
            None,
        ),
    ]);
    let json = serde_json::to_string(&service.snapshot()).unwrap();
    assert!(!json.contains("livekey"), "api key leaked: {json}");
    assert!(!json.contains("ghp_secret"), "header leaked: {json}");
    assert!(json.contains("apiKeyMasked"));
    assert!(json.contains("headerNames"));
}

/// The settings page edits against the masked config echo — which never
/// carries header/env VALUES — so an empty secret map on upsert must keep
/// the stored values rather than wipe them.
#[test]
fn edit_preserves_stored_secrets_when_fields_left_blank() {
    let data = tempfile::tempdir().unwrap();
    let service = ToolService::new(data.path().into());
    service
        .upsert_provider(provider(
            "gh",
            "GitHub",
            ProviderConfig::Http {
                url: "https://api.githubcopilot.com/mcp/".into(),
                headers: [("Authorization".into(), "Bearer ghp_secret".into())]
                    .into_iter()
                    .collect(),
            },
        ))
        .unwrap();
    service
        .upsert_provider(provider(
            "gh",
            "GitHub (renamed)",
            ProviderConfig::Http {
                url: "https://mcp.example.com/".into(),
                headers: Default::default(),
            },
        ))
        .unwrap();
    let stored = crate::load_store(data.path());
    let gh = stored.providers.iter().find(|p| p.id == "gh").unwrap();
    let ProviderConfig::Http { url, headers } = &gh.config else {
        panic!("expected http config");
    };
    assert_eq!(url, "https://mcp.example.com/");
    assert_eq!(
        headers.get("Authorization").map(String::as_str),
        Some("Bearer ghp_secret")
    );
    // A non-empty map replaces normally — the rule only guards the
    // "can't echo secrets" edit path.
    service
        .upsert_provider(provider(
            "gh",
            "GitHub (renamed)",
            ProviderConfig::Http {
                url: "https://mcp.example.com/".into(),
                headers: [("X-Token".into(), "new_value".into())]
                    .into_iter()
                    .collect(),
            },
        ))
        .unwrap();
    let stored = crate::load_store(data.path());
    let gh = stored.providers.iter().find(|p| p.id == "gh").unwrap();
    let ProviderConfig::Http { headers, .. } = &gh.config else {
        panic!("expected http config");
    };
    assert_eq!(headers.len(), 1);
    assert!(headers.contains_key("X-Token"));
}

#[tokio::test]
async fn endpoint_url_requires_a_provider() {
    let data = tempfile::tempdir().unwrap();
    let service = ToolService::new(data.path().into());
    assert!(
        service.endpoint_url().is_none(),
        "no providers → no endpoint"
    );
    service
        .upsert_provider(provider("m", "Mock", http_config()))
        .unwrap();
    assert!(service.endpoint_url().is_some());
}

/// Raw JSON-RPC POST to the endpoint; returns the response body.
async fn post(url: &str, body: &str) -> String {
    let parsed = url::Url::parse(url).unwrap();
    let addr = format!("{}:{}", parsed.host_str().unwrap(), parsed.port().unwrap());
    let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        parsed.path(),
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let raw = String::from_utf8_lossy(&buf).into_owned();
    raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string()
}

#[tokio::test]
async fn endpoint_serves_the_merged_surface() {
    let upstream = Arc::new(
        MockUpstream::new(&["search"]).with_result("search", MockUpstream::text_result("found it")),
    );
    let (service, _data) = service(vec![(provider("m", "Mock", http_config()), Some(upstream))]);
    let url = service.endpoint_url().expect("endpoint should bind");
    assert!(url.starts_with("http://127.0.0.1:"));

    let init = post(
        &url,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
    )
    .await;
    let init: Value = serde_json::from_str(&init).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "cosmos-tools");

    let list = post(
        &url,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    )
    .await;
    let list: Value = serde_json::from_str(&list).unwrap();
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert_eq!(names, vec!["mock__search"]);

    let call = post(
        &url,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"mock__search","arguments":{"q":"x"}}}"#,
    )
    .await;
    let call: Value = serde_json::from_str(&call).unwrap();
    assert_eq!(call["result"]["content"][0]["text"], "found it");

    // An unknown wire name is a protocol error, not a misroute.
    let call = post(
        &url,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
    )
    .await;
    let call: Value = serde_json::from_str(&call).unwrap();
    assert!(call["error"].is_object());

    // Wrong token path → 404.
    let parsed = url::Url::parse(&url).unwrap();
    let mut stream = tokio::net::TcpStream::connect(format!(
        "{}:{}",
        parsed.host_str().unwrap(),
        parsed.port().unwrap()
    ))
    .await
    .unwrap();
    stream
        .write_all(
            b"POST /mcp/deadbeef HTTP/1.1\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        )
        .await
        .unwrap();
    stream.shutdown().await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    assert!(String::from_utf8_lossy(&buf).starts_with("HTTP/1.1 404"));
}

#[test]
fn popular_apps_cover_the_everyday_ten() {
    let toolkits: Vec<&str> = POPULAR_APPS.iter().map(|a| a.toolkit).collect();
    for expected in [
        "gmail",
        "googlecalendar",
        "googledrive",
        "slack",
        "notion",
        "github",
        "linear",
        "outlook",
        "googlesheets",
        "todoist",
    ] {
        assert!(toolkits.contains(&expected), "{expected} missing");
    }
    // Google's canonical slugs are single words — the underscored legacy
    // spellings resolve to phantom toolkits that never carry connections.
    for legacy in ["google_calendar", "google_drive", "google_sheets"] {
        assert!(!toolkits.contains(&legacy), "{legacy} is not canonical");
    }
}
