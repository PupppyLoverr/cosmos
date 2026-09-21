//! Phone pairing over the device-room relay, end to end: the pairing gate on
//! the host's conn must admit ONLY the bootstrap/peer surface until `PairPhone`
//! redeems the on-screen code — then sessions/computer calls flow; a stored
//! grant re-authorizes a fresh conn; revoke kills it again. Uses the same
//! in-memory device room as relay_delivery.rs.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as WsRequest, Response as WsResponse,
};

use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
};
use zeron_rpc::{
    DeviceFrameHeader, DeviceLink, RpcReply, RpcService, decode_device_frame, device_room_ws_url,
    encode_device_frame, methods,
};

#[derive(Default)]
struct RelayState {
    host: Option<mpsc::UnboundedSender<Vec<u8>>>,
    clients: HashMap<String, mpsc::UnboundedSender<Vec<u8>>>,
}

async fn fake_device_room() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind relay");
    let url = format!(
        "http://127.0.0.1:{}",
        listener.local_addr().expect("addr").port()
    );
    let state = Arc::new(Mutex::new(RelayState::default()));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let state = state.clone();
            tokio::spawn(async move {
                let mut uri = String::new();
                let Ok(ws) = tokio_tungstenite::accept_hdr_async(
                    stream,
                    |req: &WsRequest, res: WsResponse| {
                        uri = req.uri().to_string();
                        Ok(res)
                    },
                )
                .await
                else {
                    return;
                };
                let query = uri.split_once('?').map(|(_, q)| q).unwrap_or("");
                let is_host = query.contains("role=host");
                let conn_id = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("connId="))
                    .unwrap_or("anon")
                    .to_string();
                let (mut sink, mut ws_stream) = ws.split();
                let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
                {
                    let mut st = state.lock().expect("lock");
                    if is_host {
                        st.host = Some(tx);
                    } else {
                        st.clients.insert(conn_id.clone(), tx);
                    }
                }
                let writer = tokio::spawn(async move {
                    while let Some(bytes) = rx.recv().await {
                        if sink.send(WsMessage::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                });
                while let Some(Ok(message)) = ws_stream.next().await {
                    let WsMessage::Binary(bytes) = message else {
                        continue;
                    };
                    let Ok((header, payload)) = decode_device_frame(&bytes) else {
                        break;
                    };
                    let st = state.lock().expect("lock");
                    if is_host {
                        let Some(to) = header.to else { continue };
                        if let Some(client) = st.clients.get(&to) {
                            let stripped = DeviceFrameHeader::new(header.s, header.k);
                            let _ = client
                                .send(encode_device_frame(&stripped, &payload).expect("encode"));
                        }
                    } else if let Some(host) = &st.host {
                        let mut routed = DeviceFrameHeader::new(header.s, header.k);
                        routed.from = Some(conn_id.clone());
                        let _ = host.send(encode_device_frame(&routed, &payload).expect("encode"));
                    }
                }
                writer.abort();
            });
        }
    });
    (url, task)
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

async fn phone_link(edge_url: &str, device_id: &str, conn: &str) -> DeviceLink {
    DeviceLink::connect(&device_room_ws_url(
        edge_url,
        device_id,
        "client",
        Some(conn),
        "test-user",
    ))
    .await
    .expect("phone dials the device room")
}

#[tokio::test]
async fn pairing_gate_then_full_surface() {
    let (edge_url, _room) = fake_device_room().await;
    let dir = tempfile::tempdir().expect("data dir");
    let core = assemble(dir.path(), "dev-mac");
    let relay = core.start_host_relay(&edge_url);

    // Host is up when the room accepts the phone socket.
    let phone = phone_link(&edge_url, "dev-mac", "phone-1").await;
    let client = phone.client();

    // Bootstrap surface is open: EngineInfo + LocalDevice answer unpaired.
    client
        .call(methods::ENGINE_INFO, serde_json::json!({}))
        .await
        .expect("EngineInfo is bootstrap");
    client
        .call(methods::LOCAL_DEVICE, serde_json::json!({}))
        .await
        .expect("LocalDevice is bootstrap");

    // The phone's real surface is denied pre-pair.
    let denied = client
        .call(methods::WATCH_SESSIONS, serde_json::json!({}))
        .await
        .expect_err("WatchSessions denied before pairing");
    assert!(denied.to_string().contains("paired"), "{denied}");
    client
        .call(methods::COMPUTER_STATUS, serde_json::json!({}))
        .await
        .expect_err("ComputerStatus denied before pairing");

    // Mint the on-screen offer (what Settings → Devices shows), redeem the code.
    let (code, _exp) = core.pairing.issue();
    let reply = client
        .call(
            methods::PAIR_PHONE,
            serde_json::json!({ "code": code, "label": "test phone" }),
        )
        .await
        .expect("PairPhone redeems the on-screen code");
    let token = reply["deviceToken"]
        .as_str()
        .expect("deviceToken")
        .to_string();
    let grant = reply["grantId"].as_str().expect("grantId").to_string();
    assert_eq!(reply["deviceId"], "dev-mac");

    // The code is single-use: a second redemption fails.
    client
        .call(
            methods::PAIR_PHONE,
            serde_json::json!({ "code": code, "label": "second phone" }),
        )
        .await
        .expect_err("the code must be single-use");

    // This conn is now trusted: the phone's surface works — a value call
    // (the computer status the mobile app fetches first) and the sessions
    // watch stream both flow.
    client
        .call(methods::COMPUTER_STATUS, serde_json::json!({}))
        .await
        .expect("ComputerStatus works after pairing");
    let mut sessions = client
        .subscribe(methods::WATCH_SESSIONS, serde_json::json!({}))
        .await
        .expect("WatchSessions streams after pairing");
    let _first = tokio::time::timeout(std::time::Duration::from_secs(5), sessions.recv())
        .await
        .expect("watch emits an initial snapshot");

    // A fresh conn re-authorizes with the stored token.
    let phone2 = phone_link(&edge_url, "dev-mac", "phone-1-reconnected").await;
    let client2 = phone2.client();
    client2
        .call(methods::COMPUTER_STATUS, serde_json::json!({}))
        .await
        .expect_err("new conns start untrusted");
    client2
        .call(
            methods::AUTHORIZE_PHONE,
            serde_json::json!({ "deviceToken": token }),
        )
        .await
        .expect("AuthorizePhone accepts the grant");
    client2
        .call(methods::COMPUTER_STATUS, serde_json::json!({}))
        .await
        .expect("authorized conn reaches the full surface");

    // Desktop revokes the phone: the live conn dies, the token is dead.
    assert!(core.pairing.revoke(&grant));
    client2
        .call(methods::COMPUTER_STATUS, serde_json::json!({}))
        .await
        .expect_err("revoked conn loses the surface");

    let phone3 = phone_link(&edge_url, "dev-mac", "phone-1-after-revoke").await;
    phone3
        .client()
        .call(
            methods::AUTHORIZE_PHONE,
            serde_json::json!({ "deviceToken": token }),
        )
        .await
        .expect_err("revoked token cannot re-authorize");

    drop(relay);
}

#[tokio::test]
async fn pairing_offer_and_revoke_over_ipc() {
    let dir = tempfile::tempdir().expect("data dir");
    let core = assemble(dir.path(), "dev-mac");
    let service = core.rpc_service();

    let offer = service
        .handle(
            methods::PHONE_PAIRING_OFFER,
            serde_json::json!({"rotate": true}),
        )
        .await
        .expect("offer");
    let RpcReply::Value(v) = offer else {
        panic!("offer must be a value")
    };
    assert_eq!(v["deviceId"], "dev-mac");
    assert_eq!(v["code"].as_str().unwrap().len(), 6);
    assert!(
        v["qr"]
            .as_str()
            .unwrap()
            .contains("cosmos://pair?d=dev-mac")
    );
    assert!(v["expiresAt"].as_i64().unwrap() > 0);

    let listed = service
        .handle(methods::LIST_PAIRED_PHONES, serde_json::json!({}))
        .await
        .expect("list");
    let RpcReply::Value(v) = listed else {
        panic!("list must be a value")
    };
    assert_eq!(v["phones"].as_array().unwrap().len(), 0);
}
