//! Composio Connect — the HTTP MCP provider whose meta-tools reach Gmail,
//! Notion, Slack and the rest of the catalog.
//!
//! `https://connect.composio.dev/mcp` + the `x-consumer-api-key` header is
//! the whole integration: key validation is one `tools/list`, per-app OAuth
//! links ride `COMPOSIO_MANAGE_CONNECTIONS`, and agents call the meta-tools
//! (plus whatever toolkit slugs are connected) through the same gateway.
//! The API key never leaves this module — `headers` below is the only place
//! it is stamped.
//!
//! Wire shapes, verified against the live endpoint (2026-09):
//! - `COMPOSIO_SEARCH_TOOLS` takes `queries: [{use_case}]` (NOT `query`)
//!   and answers `{data: {results[], toolkit_connection_statuses[],
//!   tool_schemas{}}}` — structured rows, so hits are read from those
//!   fields rather than scraped out of arbitrary strings.
//! - `COMPOSIO_MANAGE_CONNECTIONS` takes `toolkits: [{name, action}]` —
//!   operation OBJECTS. `action: "list"` reads connection posture,
//!   `action: "add"` mints a hosted Connect Link (`redirect_url`), and
//!   `toolkits` is required and non-empty.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::upstream::{self, HttpUpstream, Upstream};
use crate::{ComposioApp, ComposioSearchHit, ToolError};

pub const CONNECT_URL: &str = "https://connect.composio.dev/mcp";

/// Headers carrying the consumer key — the one place it touches a request.
fn headers(api_key: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("x-consumer-api-key".to_string(), api_key.to_string())])
}

/// Validate `api_key` against the real endpoint and return a connected
/// upstream. A key that yields no `COMPOSIO_*` tools is rejected — invalid
/// keys answer with an error surface or an empty catalog.
pub async fn connect(api_key: &str) -> Result<Arc<HttpUpstream>, ToolError> {
    let upstream = HttpUpstream::connect(CONNECT_URL, &headers(api_key), "Composio").await?;
    let tools = upstream.list_tools().await?;
    let has_meta = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .any(|n| n.starts_with("COMPOSIO_"));
    if !has_meta {
        return Err(ToolError::InvalidKey(
            "the key answered but no COMPOSIO_* tools were advertised".into(),
        ));
    }
    Ok(Arc::new(upstream))
}

/// Call one Composio tool and unwrap its payload — the meta-tools answer in
/// `structuredContent`; a JSON-bearing text block is the fallback.
async fn call(upstream: &HttpUpstream, name: &str, arguments: Value) -> Result<Value, ToolError> {
    let result = upstream.call_tool(name, arguments).await?;
    Ok(upstream::result_payload(&result))
}

/// `COMPOSIO_SEARCH_TOOLS` — fuzzy search across the catalog. Replies carry
/// per-toolkit rows (slug, description, live connection posture) plus the
/// matched tool schemas; both become settings-page hits.
pub async fn search_tools(
    upstream: &HttpUpstream,
    query: &str,
) -> Result<Vec<ComposioSearchHit>, ToolError> {
    let payload = call(
        upstream,
        "COMPOSIO_SEARCH_TOOLS",
        json!({
            "queries": [{ "use_case": query }],
            "session": { "generate_id": true },
        }),
    )
    .await?;
    Ok(parse_search_hits(&payload))
}

/// `COMPOSIO_MANAGE_CONNECTIONS` `action: "list"` — the non-blocking read
/// side of the connect surface for the given toolkit slugs.
pub async fn connection_status(
    upstream: &HttpUpstream,
    toolkits: &[String],
) -> Result<Vec<ComposioApp>, ToolError> {
    if toolkits.is_empty() {
        return Ok(Vec::new());
    }
    let ops: Vec<Value> = toolkits
        .iter()
        .map(|name| json!({ "name": name, "action": "list" }))
        .collect();
    let payload = call(
        upstream,
        "COMPOSIO_MANAGE_CONNECTIONS",
        json!({ "toolkits": ops }),
    )
    .await?;
    Ok(parse_apps(&payload))
}

/// `COMPOSIO_MANAGE_CONNECTIONS` `action: "add"` for one toolkit — mints a
/// fresh hosted Connect Link (Composio always generates a new one and caps
/// accounts per toolkit), so callers `connection_status` first and skip the
/// add when the toolkit is already ACTIVE. Returns the app row plus the
/// auth URL the settings page opens.
pub async fn add_connection(
    upstream: &HttpUpstream,
    toolkit: &str,
) -> Result<(ComposioApp, Option<String>), ToolError> {
    let payload = call(
        upstream,
        "COMPOSIO_MANAGE_CONNECTIONS",
        json!({ "toolkits": [{ "name": toolkit, "action": "add" }] }),
    )
    .await?;
    let app = parse_apps(&payload)
        .into_iter()
        .find(|row| row.toolkit.eq_ignore_ascii_case(toolkit))
        .unwrap_or(ComposioApp {
            toolkit: toolkit.to_string(),
            status: "unknown".into(),
            connected: false,
            account: None,
        });
    Ok((app, find_auth_url(&payload)))
}

// ---------------------------------------------------------------------------
// Payload parsing — the upstream shape has drifted between releases, so the
// structured fields below are read first and a tolerant walk stays as the
// fallback. Everything keys off `data.*`, accepting the bare object too.
// ---------------------------------------------------------------------------

/// `payload["data"]` when present, else the payload itself — Connect wraps
/// replies in `{data, error, log_id, successful}`.
fn data(payload: &Value) -> &Value {
    payload.get("data").unwrap_or(payload)
}

fn parse_search_hits(payload: &Value) -> Vec<ComposioSearchHit> {
    fn hit<'a>(hits: &'a mut BTreeMap<String, ComposioSearchHit>, toolkit: String) -> &'a mut ComposioSearchHit {
        hits.entry(toolkit.clone()).or_insert(ComposioSearchHit {
            toolkit,
            tools: Vec::new(),
            description: None,
            connected: false,
        })
    }
    let mut hits: BTreeMap<String, ComposioSearchHit> = BTreeMap::new();
    let data = data(payload);

    // toolkit_connection_statuses — slug, description, live posture.
    if let Some(statuses) = data
        .get("toolkit_connection_statuses")
        .and_then(Value::as_array)
    {
        for row in statuses {
            let Some(toolkit) = row.get("toolkit").and_then(Value::as_str) else {
                continue;
            };
            let toolkit = canonical_toolkit(toolkit);
            if toolkit == "composio" {
                continue;
            }
            let hit = hit(&mut hits, toolkit);
            hit.connected |= row
                .get("has_active_connection")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if hit.description.is_none() {
                hit.description = row
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
        }
    }

    // tool_schemas — group each matched tool under its toolkit so rows can
    // show a sample ("e.g. GMAIL_SEND_EMAIL").
    if let Some(schemas) = data.get("tool_schemas").and_then(Value::as_object) {
        for (slug, schema) in schemas {
            let toolkit = schema
                .get("toolkit")
                .and_then(Value::as_str)
                .map(canonical_toolkit)
                .or_else(|| slug_toolkit(slug));
            let Some(toolkit) = toolkit.filter(|t| t != "composio") else {
                continue;
            };
            let name = schema
                .get("tool_slug")
                .and_then(Value::as_str)
                .unwrap_or(slug);
            let hit = hit(&mut hits, toolkit);
            if !hit.tools.iter().any(|t| t == name) {
                hit.tools.push(name.to_string());
            }
        }
    }

    // results[] — the ranked match list: toolkit membership plus
    // primary/related slugs attributed by prefix.
    if let Some(results) = data.get("results").and_then(Value::as_array) {
        for result in results {
            if let Some(toolkits) = result.get("toolkits").and_then(Value::as_array) {
                for slug in toolkits.iter().filter_map(Value::as_str) {
                    let toolkit = canonical_toolkit(slug);
                    if toolkit != "composio" {
                        hit(&mut hits, toolkit);
                    }
                }
            }
            for key in [
                "primary_tool_slugs",
                "related_tool_slugs",
                "tool_slugs",
                "tools",
            ] {
                if let Some(slugs) = result.get(key).and_then(Value::as_array) {
                    for slug in slugs.iter().filter_map(Value::as_str) {
                        let Some(toolkit) = slug_toolkit(slug).filter(|t| t != "composio") else {
                            continue;
                        };
                        let hit = hit(&mut hits, toolkit);
                        if !hit.tools.iter().any(|t| t == slug) {
                            hit.tools.push(slug.to_string());
                        }
                    }
                }
            }
        }
    }

    // Last resort for older payloads: walk every string and bucket strict
    // `TOOLKIT_ACTION` slugs. The strict shape is what keeps payload noise
    // (`log_*` ids, `thread_id` arg names, prose) out of the app list.
    if hits.is_empty() {
        let mut slugs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        collect_tool_slugs(payload, &mut slugs);
        for (toolkit, tools) in slugs {
            if toolkit == "composio" {
                continue;
            }
            hit(&mut hits, toolkit).tools = tools;
        }
    }

    hits.into_values().collect()
}

/// Bucket every `<TOOLKIT>_<SLUG>`-shaped string under its toolkit —
/// `GMAIL_SEND_EMAIL` lands under `gmail` as a sample row.
fn collect_tool_slugs(value: &Value, hits: &mut BTreeMap<String, Vec<String>>) {
    match value {
        Value::String(s) => {
            if let Some(toolkit) = slug_toolkit(s) {
                let entry = hits.entry(toolkit).or_default();
                if !entry.contains(s) {
                    entry.push(s.clone());
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| collect_tool_slugs(v, hits)),
        Value::Object(map) => map.values().for_each(|v| collect_tool_slugs(v, hits)),
        _ => {}
    }
}

/// `GMAIL_SEND_EMAIL` → `gmail`. Only strict `SHOUTY_TOOLKIT_SHOUTY_ACTION`
/// strings qualify: lowercase-headed strings (`log_hmH-x`, `thread_id`),
/// mixed-case keys, free text and URLs are payload noise, not tool names.
fn slug_toolkit(s: &str) -> Option<String> {
    let (head, tail) = s.split_once('_')?;
    if head.is_empty()
        || tail.is_empty()
        || !head.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        || !tail.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    Some(head.to_ascii_lowercase())
}

/// Lowercase the slug and fold legacy `google_*` spellings onto Composio's
/// canonical toolkit slugs (`googledrive`, `googlecalendar`, `googlesheets`)
/// — the underscored spellings resolve to phantom toolkits that never
/// carry connections.
pub fn canonical_toolkit(slug: &str) -> String {
    let slug = slug.trim().to_ascii_lowercase();
    match slug.as_str() {
        "google_drive" => "googledrive".into(),
        "google_calendar" => "googlecalendar".into(),
        "google_sheets" => "googlesheets".into(),
        _ => slug,
    }
}

/// Pull `status`-bearing toolkit rows out of a manage payload. The current
/// shape is `{data: {results: {<slug>: {toolkit, status, accounts[]}}}}`;
/// the generic walk below stays as the fallback for drifted releases.
fn parse_apps(payload: &Value) -> Vec<ComposioApp> {
    let mut rows = Vec::new();
    if let Some(results) = data(payload).get("results").and_then(Value::as_object) {
        for (key, value) in results {
            if let Some(app) = parse_app_row(key, value) {
                rows.push(app);
            }
        }
    }
    if rows.is_empty() {
        collect_app_rows(payload, &mut rows);
    }
    let mut seen = BTreeSet::new();
    rows.retain(|r| seen.insert(r.toolkit.to_ascii_lowercase()));
    rows
}

/// One `results.<slug>` entry → a [`ComposioApp`]. A toolkit nobody ever
/// started connecting comes back as `initiated` with NO accounts — that's
/// Composio's "nothing here" and the row reports `not-connected` so the
/// settings page doesn't claim a half-finished OAuth exists.
fn parse_app_row(key: &str, value: &Value) -> Option<ComposioApp> {
    let obj = value.as_object()?;
    let toolkit = obj
        .get("toolkit")
        .and_then(Value::as_str)
        .unwrap_or(key)
        .to_ascii_lowercase();
    let status = obj
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let accounts = obj.get("accounts").and_then(Value::as_array);
    let active_account = accounts.and_then(|accounts| {
        accounts
            .iter()
            .find(|a| is_active_status(a.get("status")))
            .or_else(|| {
                accounts.iter().find(|a| {
                    a.get("is_default").and_then(Value::as_bool) == Some(true)
                })
            })
    });
    let connected = is_active_status(value.get("status")) || active_account.is_some();
    Some(ComposioApp {
        toolkit,
        status: if !connected && accounts.is_some_and(|a| a.is_empty()) {
            "not-connected".to_string()
        } else {
            status.to_string()
        },
        connected,
        account: active_account.and_then(account_label),
    })
}

fn is_active_status(status: Option<&Value>) -> bool {
    status
        .and_then(Value::as_str)
        .map(|s| s.eq_ignore_ascii_case("active") || s.eq_ignore_ascii_case("connected"))
        .unwrap_or(false)
}

/// The account's display identity for "Connected as …" — email first, then
/// name; the opaque account id is deliberately not a fallback.
fn account_label(account: &Value) -> Option<String> {
    let info = account.get("user_info").unwrap_or(account);
    ["email", "name", "username", "displayName", "display_name"]
        .iter()
        .find_map(|k| info.get(*k).and_then(Value::as_str))
        .map(str::to_string)
}

fn collect_app_rows(value: &Value, rows: &mut Vec<ComposioApp>) {
    match value {
        Value::Object(map) => {
            let toolkit = ["toolkit", "toolKit", "toolkitSlug", "app", "appName"]
                .iter()
                .find_map(|k| map.get(*k).and_then(Value::as_str));
            let status = ["status", "state", "connectionStatus"]
                .iter()
                .find_map(|k| map.get(*k).and_then(Value::as_str));
            let connected = ["connected", "isConnected", "active", "hasActiveConnection"]
                .iter()
                .find_map(|k| map.get(*k).and_then(Value::as_bool));
            if let (Some(toolkit), true) = (toolkit, status.is_some() || connected.is_some()) {
                let connected = connected.unwrap_or_else(|| {
                    status
                        .map(|s| {
                            s.eq_ignore_ascii_case("active") || s.eq_ignore_ascii_case("connected")
                        })
                        .unwrap_or(false)
                });
                rows.push(ComposioApp {
                    toolkit: toolkit.to_ascii_lowercase(),
                    status: status.unwrap_or("unknown").to_string(),
                    connected,
                    account: None,
                });
            }
            map.values().for_each(|v| collect_app_rows(v, rows));
        }
        Value::Array(items) => items.iter().for_each(|v| collect_app_rows(v, rows)),
        _ => {}
    }
}

/// The first composio.dev URL in a payload — the hosted connect link.
/// Explicit reply fields (`redirect_url`, `auth_url`, `link`) are checked
/// before falling back to any URL-shaped string.
pub fn find_auth_url(payload: &Value) -> Option<String> {
    match payload {
        Value::Object(map) => {
            for key in ["redirect_url", "auth_url", "authUrl", "link", "url"] {
                if let Some(url) = map.get(key).and_then(Value::as_str) {
                    if url.starts_with("https://") && url.contains("composio.dev") {
                        return Some(url.to_string());
                    }
                }
            }
            map.values().find_map(find_auth_url)
        }
        Value::Array(items) => items.iter().find_map(find_auth_url),
        Value::String(s) => {
            if s.starts_with("https://") && s.contains("composio.dev") {
                Some(s.clone())
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live `COMPOSIO_SEARCH_TOOLS` reply shape (2026-09), trimmed to
    /// the fields the parser reads.
    fn search_payload() -> Value {
        json!({
            "data": {
                "results": [
                    {
                        "toolkits": ["gmail", "google_drive"],
                        "primary_tool_slugs": ["GMAIL_SEND_EMAIL"],
                        "related_tool_slugs": ["GOOGLEDRIVE_UPLOAD_FILE"]
                    }
                ],
                "toolkit_connection_statuses": [
                    {
                        "toolkit": "gmail",
                        "description": "Send and read email",
                        "has_active_connection": true
                    },
                    {
                        "toolkit": "googledrive",
                        "description": "Files and sharing",
                        "has_active_connection": false
                    },
                    {
                        "toolkit": "composio",
                        "has_active_connection": false
                    }
                ],
                "tool_schemas": {
                    "GMAIL_SEND_EMAIL": {
                        "tool_slug": "GMAIL_SEND_EMAIL",
                        "toolkit": "gmail"
                    }
                }
            }
        })
    }

    #[test]
    fn search_hits_read_structured_rows() {
        let hits = parse_search_hits(&search_payload());
        let gmail = hits.iter().find(|h| h.toolkit == "gmail").unwrap();
        assert!(gmail.connected);
        assert_eq!(gmail.description.as_deref(), Some("Send and read email"));
        assert!(gmail.tools.contains(&"GMAIL_SEND_EMAIL".to_string()));

        // Legacy `google_drive` folds onto the canonical slug.
        let drive = hits.iter().find(|h| h.toolkit == "googledrive").unwrap();
        assert!(!drive.connected);
        assert!(drive.tools.contains(&"GOOGLEDRIVE_UPLOAD_FILE".to_string()));
        assert!(hits.iter().all(|h| h.toolkit != "google_drive"));

        // The meta-toolkit itself is never a connectable app row.
        assert!(hits.iter().all(|h| h.toolkit != "composio"));
    }

    #[test]
    fn fallback_slug_walk_rejects_payload_noise() {
        // No structured fields → the strict TOOLKIT_ACTION walk runs. It
        // must keep arg names and log ids out: only SHOUTY_HEAD_SHOUTY_TAIL
        // strings qualify.
        let payload = json!({
            "data": {
                "log_id": "log_hmH-x1",
                "arguments": {"thread_id": "thread_9f", "query": "send a message"},
                "tools": ["SLACK_SEND_MESSAGE", "slack", "Send_Message", "NOTION"]
            }
        });
        let hits = parse_search_hits(&payload);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].toolkit, "slack");
        assert_eq!(hits[0].tools, vec!["SLACK_SEND_MESSAGE".to_string()]);
    }

    #[test]
    fn slug_toolkit_only_takes_strict_tool_names() {
        assert_eq!(slug_toolkit("GMAIL_SEND_EMAIL"), Some("gmail".into()));
        assert_eq!(slug_toolkit("HUBSPOT_V2_LIST"), Some("hubspot".into()));
        for noise in [
            "log_hmH-x",
            "thread_id",
            "slack",
            "Send_Message",
            "NOTION",
            "gmail_send_email",
            "",
            "GMAIL_",
            "_SEND",
        ] {
            assert_eq!(slug_toolkit(noise), None, "{noise} must not parse");
        }
    }

    #[test]
    fn apps_parse_status_and_account() {
        let payload = json!({
            "data": {
                "results": {
                    "gmail": {
                        "toolkit": "gmail",
                        "status": "ACTIVE",
                        "accounts": [
                            {
                                "status": "ACTIVE",
                                "is_default": true,
                                "user_info": {"email": "me@example.com"}
                            },
                            {"status": "EXPIRED", "user_info": {"email": "old@example.com"}}
                        ]
                    },
                    "slack": {
                        "toolkit": "slack",
                        "status": "INITIATED",
                        "accounts": []
                    }
                }
            }
        });
        let apps = parse_apps(&payload);
        let gmail = apps.iter().find(|a| a.toolkit == "gmail").unwrap();
        assert!(gmail.connected);
        assert_eq!(gmail.status, "ACTIVE");
        assert_eq!(gmail.account.as_deref(), Some("me@example.com"));

        // Initiated with no accounts is "nothing here", not a live OAuth.
        let slack = apps.iter().find(|a| a.toolkit == "slack").unwrap();
        assert!(!slack.connected);
        assert_eq!(slack.status, "not-connected");
        assert_eq!(slack.account, None);
    }

    #[test]
    fn account_label_prefers_email_then_name() {
        let account = json!({"user_info": {"name": "Rav T", "email": "r@x.io"}});
        assert_eq!(account_label(&account).as_deref(), Some("r@x.io"));
        let account = json!({"user_info": {"name": "Rav T"}});
        assert_eq!(account_label(&account).as_deref(), Some("Rav T"));
        // Bare account objects (no user_info wrapper) read the same keys.
        let account = json!({"username": "ravt"});
        assert_eq!(account_label(&account).as_deref(), Some("ravt"));
        // An opaque id alone is not a display identity.
        let account = json!({"id": "acc_123"});
        assert_eq!(account_label(&account), None);
    }

    #[test]
    fn auth_url_comes_from_redirect_fields() {
        let payload = json!({
            "data": {
                "results": {"linear": {"redirect_url": "https://connect.composio.dev/link/abc"}},
                "note": "visit https://example.com/docs"
            }
        });
        assert_eq!(
            find_auth_url(&payload).as_deref(),
            Some("https://connect.composio.dev/link/abc")
        );
        // Non-Composio URLs are never treated as auth links.
        let payload = json!({"data": {"url": "https://example.com/x", "link": "not-a-url"}});
        assert_eq!(find_auth_url(&payload), None);
    }

    #[test]
    fn canonical_toolkit_folds_google_legacy_slugs() {
        assert_eq!(canonical_toolkit("google_drive"), "googledrive");
        assert_eq!(canonical_toolkit("Google_Calendar"), "googlecalendar");
        assert_eq!(canonical_toolkit(" google_sheets "), "googlesheets");
        assert_eq!(canonical_toolkit("Gmail"), "gmail");
        assert_eq!(canonical_toolkit("slack"), "slack");
    }
}
