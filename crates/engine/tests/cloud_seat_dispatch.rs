use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use zeron_engine::cloud::SeatConfig;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, CloudRunStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};
use zeron_rpc::methods;

const CHAT: &str = "chat-cloud-seat-dispatch";
const LATE_CHAT: &str = "chat-cloud-seat-late-create";
const SEAT: &str = "boat-test-seat";

struct CountingHarness(Arc<AtomicUsize>);

#[async_trait]
impl Harness for CountingHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }

    fn display_name(&self) -> &str {
        "Counting"
    }

    fn supports_steering(&self) -> bool {
        false
    }

    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }

    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }

    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(Vec::new())
    }

    async fn run(
        &self,
        _request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(futures::stream::empty().boxed())
    }
}

async fn stub_boat() -> (
    String,
    mpsc::UnboundedReceiver<String>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock Boat endpoint");
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = [0; 4096];
            let Ok(n) = stream.read(&mut request).await else {
                continue;
            };
            let request = String::from_utf8_lossy(&request[..n])
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();
            let _ = request_tx.send(request);
            let body = r#"{"error":{"message":"stubbed Boat transport"}}"#;
            let response = format!(
                "HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    (endpoint, request_rx, task)
}

fn run_request() -> RunRequest {
    RunRequest {
        prompt: "run remotely".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/workspace".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        resume: None,
        attachments: Vec::new(),
        worktree: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn late_create_chat_binding_persists_and_dispatches_to_boat() {
    let tmp = tempfile::tempdir().unwrap();
    let local_runs = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(Arc::new(CountingHarness(local_runs.clone())));
    let core = EngineCore::assemble(tmp.path(), registry, HarnessId::Mock, None)
        .expect("engine core assembles");
    let cloud = core
        .cloud
        .as_ref()
        .expect("Development profile loads a CloudStore");

    let (endpoint, mut requests, boat_task) = stub_boat().await;
    cloud
        .save_seat(SeatConfig {
            id: SEAT.into(),
            kind: Some(zeron_proto::CloudSeatKind::Box),
            label: "Boat test".into(),
            box_endpoint: Some(endpoint),
            box_id: Some("bx_test".into()),
            ..Default::default()
        })
        .unwrap();
    cloud
        .set_seat_token(SEAT, "boxToken", Some("test-token".into()))
        .unwrap();

    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            methods::MUTATE,
            serde_json::json!({
                "op": "createChat",
                "chatId": LATE_CHAT,
                "deviceId": core.device_id.as_str(),
                "cloudSeat": {
                    "seatId": SEAT,
                    "kind": "box",
                    "label": "Boat test",
                    "workspacePath": "/workspace"
                }
            }),
        )
        .await
        .expect("late CreateChat mutation");
    core.workspace
        .claim_chat(LATE_CHAT, Some("/workspace"))
        .expect("claiming an existing chat remains idempotent");
    let normal_binding = core
        .workspace
        .chat(LATE_CHAT)
        .expect("read normally created chat row")
        .expect("normally created chat row exists")
        .cloud_seat
        .expect("claim preserves the existing cloudSeat");
    assert_eq!(normal_binding.seat_id, SEAT);

    core.workspace
        .claim_chat(CHAT, Some("/workspace"))
        .expect("first Run claims the chat before CreateChat arrives");
    client
        .call(
            methods::MUTATE,
            serde_json::json!({
                "op": "createChat",
                "chatId": CHAT,
                "deviceId": core.device_id.as_str(),
                "cloudSeat": {
                    "seatId": SEAT,
                    "kind": "box",
                    "label": "Boat test",
                    "workspacePath": "/workspace"
                }
            }),
        )
        .await
        .expect("CreateChat mutation races the first Run claim");

    let binding = core
        .workspace
        .chat(CHAT)
        .expect("read chat row")
        .expect("chat row exists")
        .cloud_seat
        .expect("cloudSeat persisted");
    assert_eq!(binding.seat_id, SEAT);

    core.sessions
        .dispatch(
            CHAT,
            HarnessId::ClaudeCode,
            run_request(),
            Some("user-message".into()),
        )
        .await
        .expect("cloud run dispatches");

    let cloud_run = cloud
        .run_for_chat(CHAT)
        .expect("remote run ledger row exists");
    assert_eq!(cloud_run.seat_id, SEAT);
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), requests.recv())
            .await
            .expect("Boat transport receives request")
            .expect("mock request channel remains open"),
        "GET /sandboxes/bx_test HTTP/1.1"
    );
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), requests.recv())
            .await
            .expect("Boat provisioning request arrives")
            .expect("mock request channel remains open"),
        "POST /sandboxes HTTP/1.1"
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if cloud
                .run_for_chat(CHAT)
                .is_some_and(|run| run.status == CloudRunStatus::Error)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("stub transport error is surfaced on the cloud run");
    assert_eq!(local_runs.load(Ordering::SeqCst), 0);

    core.shutdown().await;
    boat_task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn unsupported_boat_harness_fails_before_prompt_or_provisioning() {
    let tmp = tempfile::tempdir().unwrap();
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(Arc::new(CountingHarness(Arc::new(AtomicUsize::new(0)))));
    let core = EngineCore::assemble(tmp.path(), registry, HarnessId::Mock, None)
        .expect("engine core assembles");
    let cloud = core.cloud.as_ref().unwrap();
    let (endpoint, mut requests, boat_task) = stub_boat().await;
    cloud
        .save_seat(SeatConfig {
            id: SEAT.into(),
            kind: Some(zeron_proto::CloudSeatKind::Box),
            label: "Boat test".into(),
            box_endpoint: Some(endpoint),
            ..Default::default()
        })
        .unwrap();
    cloud
        .set_seat_token(SEAT, "boxToken", Some("test-token".into()))
        .unwrap();
    let binding = zeron_proto::CloudSeatBinding {
        seat_id: SEAT.into(),
        kind: zeron_proto::CloudSeatKind::Box,
        label: "Boat test".into(),
        repo: None,
        workspace_path: Some("/workspace".into()),
        runner_device_id: None,
        branch: None,
        model_label: None,
        expires_at: None,
    };
    core.workspace
        .create_chat_with_parent(
            CHAT,
            None,
            Some(&core.device_id),
            None,
            Some("/workspace".into()),
            None,
            Some(binding),
        )
        .unwrap();

    core.sessions
        .dispatch(
            CHAT,
            HarnessId::Mock,
            run_request(),
            Some("unsupported-user-message".into()),
        )
        .await
        .expect("unsupported provider failure is surfaced in transcript");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if cloud
                .run_for_chat(CHAT)
                .is_some_and(|run| run.status == CloudRunStatus::Error)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("unsupported harness marks the cloud run failed");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(
        requests.try_recv().is_err(),
        "unsupported harness made no Boat request"
    );
    let entries = core
        .doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap();
    assert!(entries.iter().any(|entry| {
        entry.parts.iter().any(|part| {
            matches!(
                part,
                zeron_doc::MessagePart::Text { text, .. }
                    if text == "Cloud run failed: Boat's built-in agent runs Claude Code only — install a Cosmos runner on this box to use Mock"
            )
        })
    }));

    core.shutdown().await;
    boat_task.abort();
}
