//! Phone pairing against a REAL edge worker — the in-memory sibling
//! (`pairing_relay.rs`) covers gate/redeem/revoke semantics; this test covers
//! the pieces only the deployed artifact has: the owner gate (wrong-account
//! conn rejected at upgrade), `/status` presence, and the `k:" relay"`
//! `host_closed` frame clients must survive.
//!
//! Run: `wrangler dev -c edge/wrangler.cosmos.jsonc --port 27640 --var
//! AUTH_MODE:dev`, then `COSMOS_LIVE_EDGE_URL=http://127.0.0.1:27640 cargo test
//! -p zeron-engine --test pairing_edge_live`. Skipped when the env var is unset.
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::StreamExt;

use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
};
use zeron_rpc::{decode_device_frame, device_room_ws_url, methods, DeviceLink};

/// Edge dev auth: the bearer IS the user id — engine dev mode answers
/// `dev-user`, so every client must present the same bearer to pass the
/// room's owner gate.
const DEV_TOKEN: &str = "dev-user";

fn live_edge() -> Option<String> {
    std::env::var("COSMOS_LIVE_EDGE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .map(|u| u.trim_end_matches('/').to_string())
}

struct InstantHarness;

#[async_trait]
impl Harness for InstantHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Instant"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        _request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        Ok(futures::stream::iter([Ok(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: Some("hs".into()),
        })])
        .boxed())
    }
}

fn assemble(dir: &std::path::Path, device_id: &str) -> EngineCore {
    std::fs::create_dir_all(dir).expect("create data dir");
    std::fs::write(dir.join("device-id"), device_id).expect("write device id");
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(InstantHarness));
    EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None).expect("engine assembles")
}

async fn status(edge_url: &str, device_id: &str, token: &str) -> reqwest::StatusCode {
    reqwest::Client::new()
        .get(format!("{edge_url}/device/{device_id}/status"))
        .bearer_auth(token)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map(|r| r.status())
        .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR)
}

async fn call(
    c: &Arc<zeron_rpc::RpcClient>,
    m: &'static str,
    p: serde_json::Value,
) -> Result<serde_json::Value, zeron_rpc::RpcError> {
    tokio::time::timeout(std::time::Duration::from_secs(15), c.call(m, p))
        .await
        .unwrap_or_else(|_| panic!("{m} did not answer within 15s"))
}

async fn watch(
    c: &Arc<zeron_rpc::RpcClient>,
    m: &'static str,
) -> Result<zeron_rpc::RpcSubscription, zeron_rpc::RpcError> {
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        c.subscribe_checked(m, serde_json::json!({})),
    )
    .await
    .unwrap_or_else(|_| panic!("{m} did not answer within 15s"))
}

async fn wait_for_host(edge_url: &str, device_id: &str) {
    for _ in 0..40 {
        if let Ok(resp) = reqwest::Client::new()
            .get(format!("{edge_url}/device/{device_id}/status"))
            .bearer_auth(DEV_TOKEN)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                if body["hostConnected"].as_bool() == Some(true) {
                    return;
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    panic!("host never checked into the device room");
}

#[tokio::test]
async fn live_edge_pairing_roundtrip() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_test_writer()
        .try_init();
    let Some(edge_url) = live_edge() else {
        eprintln!("COSMOS_LIVE_EDGE_URL unset — skipping live edge test");
        return;
    };
    // Fresh room every run — local DO storage persists across invocations.
    let device_id = format!("e2e-mac-{}", std::process::id() % 100_000);
    let device_id = device_id.as_str();
    let dir = tempfile::tempdir().expect("data dir");
    let core = assemble(dir.path(), device_id);

    // Nobody has claimed this device room yet — /status is 404 until the
    // first host join sets the owner.
    assert_eq!(
        status(&edge_url, device_id, "other-user").await,
        reqwest::StatusCode::NOT_FOUND,
        "unclaimed room must 404"
    );

    let relay = core.start_host_relay(&edge_url);
    wait_for_host(&edge_url, device_id).await;

    // Once the host owns the room, a different account is refused even the
    // presence read.
    assert_eq!(
        status(&edge_url, device_id, "other-user").await,
        reqwest::StatusCode::FORBIDDEN,
        "non-owner status probe must 403"
    );

    // A client from a DIFFERENT account must be refused at the WS upgrade —
    // the edge's owner gate is the only thing standing between a stranger and
    // the desktop's relay-forwarded surface.
    let stranger_url = device_room_ws_url(&edge_url, device_id, "client", Some("stranger"), "other-user");
    assert!(
        DeviceLink::connect(&stranger_url).await.is_err(),
        "a different account must be rejected at upgrade"
    );

    let phone = DeviceLink::connect(&device_room_ws_url(
        &edge_url,
        device_id,
        "client",
        Some("phone-1"),
        DEV_TOKEN,
    ))
    .await
    .expect("same-account client dials");
    let client = phone.client();

    // Bootstrap surface open, phone surface gated.
    eprintln!("step: EngineInfo");
    call(&client, methods::ENGINE_INFO, serde_json::json!({}))
        .await
        .expect("EngineInfo is bootstrap");
    eprintln!("step: WatchSessions (deny)");
    assert!(
        watch(&client, methods::WATCH_SESSIONS).await.is_err(),
        "WatchSessions denied before pairing"
    );

    // Mint + redeem the on-screen code.
    let (code, _exp) = core.pairing.issue();
    eprintln!("step: PairPhone redeem code={code}");
    let reply = call(&client, methods::PAIR_PHONE, serde_json::json!({ "code": code }))
        .await
        .expect("PairPhone redeems the code");
    let token = reply["deviceToken"].as_str().expect("deviceToken").to_string();

    eprintln!("step: WatchSessions (open)");
    watch(&client, methods::WATCH_SESSIONS)
        .await
        .expect("WatchSessions opens after pairing");
    eprintln!("step: phone-2 dial");

    // A fresh conn re-authorizes with the stored grant token.
    let phone2 = DeviceLink::connect(&device_room_ws_url(
        &edge_url,
        device_id,
        "client",
        Some("phone-2"),
        DEV_TOKEN,
    ))
    .await
    .expect("second client dials");
    eprintln!("step: AuthorizePhone");
    call(&phone2.client(), methods::AUTHORIZE_PHONE, serde_json::json!({ "deviceToken": token }))
        .await
        .expect("AuthorizePhone accepts the grant");
    eprintln!("step: ComputerStatus");
    call(&phone2.client(), methods::COMPUTER_STATUS, serde_json::json!({}))
        .await
        .expect("authorized conn reaches the full surface");
    eprintln!("step: host drop / host_closed watch");

    // Host drop → the edge must broadcast `k:" relay"` / host_closed so
    // clients fail fast instead of hanging pending calls.
    let (raw_ws, _) = tokio_tungstenite::connect_async(&device_room_ws_url(
        &edge_url,
        device_id,
        "client",
        Some("watcher"),
        DEV_TOKEN,
    ))
    .await
    .expect("raw watcher dials");
    let (_, mut raw_read) = raw_ws.split();
    drop(relay);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut saw_host_closed = false;
    while std::time::Instant::now() < deadline {
        let Ok(Some(Ok(msg))) =
            tokio::time::timeout(deadline - std::time::Instant::now(), raw_read.next()).await
        else {
            break;
        };
        let tokio_tungstenite::tungstenite::Message::Binary(bytes) = msg else {
            continue;
        };
        let Ok((header, payload)) = decode_device_frame(&bytes) else {
            continue;
        };
        if header.k == " relay" {
            let body: serde_json::Value =
                serde_json::from_slice(&payload).unwrap_or(serde_json::json!({}));
            if body["error"].as_str() == Some("host_closed") {
                saw_host_closed = true;
                break;
            }
        }
    }
    assert!(saw_host_closed, "client must see host_closed when the host drops");
}
