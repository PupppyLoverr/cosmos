#![cfg(unix)]

use async_trait::async_trait;
use futures::stream::BoxStream;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Notify,
};
use zeron_doc::SessionCommandPayload;
use zeron_engine::{EdgeConfig, EngineCore, EngineProfile, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};
use zeron_sync::chat_frames::{encode, frame_type};

const CHAT: &str = "runner-chat-continuity";

#[derive(Default)]
struct ChatRoom {
    rows: Vec<RelayRow>,
    ids: HashMap<String, u64>,
}

struct RelayRow {
    batch_id: String,
    device: String,
    bytes: Vec<u8>,
}

async fn chat2_relay() -> (String, Arc<Mutex<ChatRoom>>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind relay");
    let url = format!("http://{}", listener.local_addr().expect("relay address"));
    let room = Arc::new(Mutex::new(ChatRoom::default()));
    let shared = room.clone();
    let task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let room = shared.clone();
            tokio::spawn(async move {
                serve_chat2_request(stream, room).await;
            });
        }
    });
    (url, room, task)
}

async fn serve_chat2_request(mut stream: TcpStream, room: Arc<Mutex<ChatRoom>>) {
    let Some((request_line, headers, body)) = read_request(&mut stream).await else {
        return;
    };
    let mut request = request_line.split_whitespace();
    let Some(method) = request.next() else {
        return;
    };
    let Some(target) = request.next() else {
        return;
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    if headers.contains_key("upgrade") {
        respond(&mut stream, "426 Upgrade Required", "text/plain", b"").await;
        return;
    }

    if path.ends_with("/checkpoint") {
        respond(&mut stream, "404 Not Found", "text/plain", b"").await;
        return;
    }

    if !path.ends_with("/rows") {
        respond(&mut stream, "404 Not Found", "text/plain", b"").await;
        return;
    }

    if method == "GET" {
        let after = query_value(query, "after")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let frames = {
            let room = room
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let head = room.rows.len() as u64;
            let row_bytes: u64 = room.rows.iter().map(|row| row.bytes.len() as u64).sum();
            let mut frames = length_prefixed(
                frame_type::STATE,
                &serde_json::json!({
                    "headSeq": head,
                    "seqFloor": 0,
                    "checkpointSeq": 0,
                    "checkpointSize": 0,
                    "rowCount": head,
                    "rowBytes": row_bytes,
                }),
                &[],
            );
            for (index, row) in room.rows.iter().enumerate() {
                let seq = index as u64 + 1;
                if seq > after {
                    frames.extend(length_prefixed(
                        frame_type::ROW,
                        &serde_json::json!({
                            "seq": seq,
                            "device": row.device,
                            "batchId": row.batch_id,
                        }),
                        &row.bytes,
                    ));
                }
            }
            frames.extend(length_prefixed(
                frame_type::ROWS_DONE,
                &serde_json::json!({"headSeq": head}),
                &[],
            ));
            frames
        };
        respond(&mut stream, "200 OK", "application/octet-stream", &frames).await;
        return;
    }

    if method == "POST" {
        let Some(batch_id) = query_value(query, "batchId") else {
            respond(&mut stream, "400 Bad Request", "text/plain", b"").await;
            return;
        };
        let device = query_value(query, "device").unwrap_or("unknown");
        let response = {
            let mut room = room
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let seq = if let Some(seq) = room.ids.get(batch_id) {
                *seq
            } else {
                let seq = room.rows.len() as u64 + 1;
                room.ids.insert(batch_id.to_string(), seq);
                room.rows.push(RelayRow {
                    batch_id: batch_id.to_string(),
                    device: device.to_string(),
                    bytes: body,
                });
                seq
            };
            serde_json::to_vec(&serde_json::json!({
                "batchId": batch_id,
                "seq": seq,
            }))
            .expect("serialize push ack")
        };
        respond(&mut stream, "200 OK", "application/json", &response).await;
        return;
    }

    respond(&mut stream, "405 Method Not Allowed", "text/plain", b"").await;
}

async fn read_request(
    stream: &mut TcpStream,
) -> Option<(String, HashMap<String, String>, Vec<u8>)> {
    let mut bytes = Vec::new();
    let (header_end, content_length) = loop {
        let mut buffer = [0; 8192];
        let count = stream.read(&mut buffer).await.ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = index + 4;
            let header = String::from_utf8_lossy(&bytes[..index]);
            let content_length = header
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= header_end + content_length {
                break (header_end, content_length);
            }
            loop {
                let mut body = [0; 8192];
                let count = stream.read(&mut body).await.ok()?;
                if count == 0 {
                    return None;
                }
                bytes.extend_from_slice(&body[..count]);
                if bytes.len() >= header_end + content_length {
                    break;
                }
            }
            break (header_end, content_length);
        }
    };
    let header_text = String::from_utf8_lossy(&bytes[..header_end - 4]);
    let mut lines = header_text.lines();
    let request_line = lines.next()?.to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    Some((
        request_line,
        headers,
        bytes[header_end..header_end + content_length].to_vec(),
    ))
}

fn query_value<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

fn length_prefixed(kind: u8, header: &impl serde::Serialize, payload: &[u8]) -> Vec<u8> {
    let frame = encode(kind, header, payload);
    let mut bytes = Vec::with_capacity(frame.len() + 4);
    bytes.extend_from_slice(&(frame.len() as u32).to_le_bytes());
    bytes.extend(frame);
    bytes
}

async fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(headers.as_bytes()).await;
    let _ = stream.write_all(body).await;
}

struct GateHarness {
    runs: Arc<AtomicUsize>,
    release: Arc<Notify>,
}

#[async_trait]
impl Harness for GateHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }

    fn display_name(&self) -> &str {
        "Continuity gate"
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
        request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let run_id = self.runs.fetch_add(1, Ordering::AcqRel) + 1;
        let release = self.release.clone();
        let cwd = request.cwd;
        let stream = futures::stream::unfold(
            (0u8, run_id, release, cwd),
            |(step, run_id, release, cwd)| async move {
                let event = match step {
                    0 => AgentEvent::SessionStarted {
                        harness: HarnessId::Mock,
                        model: "continuity-test".into(),
                        tools: vec![],
                        cwd: cwd.clone(),
                        session_id: format!("continuity-session-{run_id}"),
                        assistant_message_id: format!("continuity-assistant-{run_id}"),
                    },
                    1 => AgentEvent::TextDelta {
                        text: "runner started".into(),
                    },
                    2 => {
                        release.notified().await;
                        AgentEvent::TextDelta {
                            text: "runner finished".into(),
                        }
                    }
                    3 => AgentEvent::Done {
                        status: DoneStatus::Completed,
                        result: None,
                        error: None,
                        session_id: Some(format!("continuity-session-{run_id}")),
                    },
                    _ => return None,
                };
                Some((Ok(event), (step + 1, run_id, release, cwd)))
            },
        );
        Ok(Box::pin(stream))
    }
}

fn assemble(
    data_dir: &std::path::Path,
    device_id: &str,
    edge_url: &str,
    runs: Arc<AtomicUsize>,
    release: Arc<Notify>,
) -> EngineCore {
    std::fs::create_dir_all(data_dir).expect("create device data dir");
    std::fs::write(data_dir.join("device-id"), device_id).expect("persist device id");
    let profile = EngineProfile::synced(data_dir, "continuity-org", "continuity-user");
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(Arc::new(GateHarness { runs, release }));
    EngineCore::assemble_with_profile(
        profile,
        registry,
        HarnessId::Mock,
        Some(EdgeConfig::with_static_token(edge_url, "continuity-test").with_device(device_id)),
    )
    .expect("assemble continuity engine")
}

fn binding(workspace: &std::path::Path) -> zeron_proto::CloudSeatBinding {
    zeron_proto::CloudSeatBinding {
        seat_id: "continuity-runner".into(),
        kind: zeron_proto::CloudSeatKind::Ssh,
        label: "Continuity runner".into(),
        repo: None,
        workspace_path: Some(workspace.display().to_string()),
        runner_device_id: Some("runner-b".into()),
        branch: None,
        model_label: None,
        expires_at: None,
    }
}

fn contains_text(entries: &[zeron_doc::SessionMessageEntry], text: &str) -> bool {
    entries.iter().any(|entry| {
        entry.parts.iter().any(|part| {
            matches!(part, zeron_doc::MessagePart::Text { text: value, .. } if value.contains(text))
        })
    })
}

async fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
    tokio::time::timeout(Duration::from_secs(45), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok()
}

async fn continuity_scenario(delete_docs_cache: bool) {
    let dirs = tempfile::tempdir().expect("temporary profile roots");
    let (edge_url, relay, relay_task) = chat2_relay().await;
    let runner_runs = Arc::new(AtomicUsize::new(0));
    let viewport_runs = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let runner_root = dirs.path().join("runner");
    let viewport_root = dirs.path().join("viewport");
    let workspace = dirs.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("runner workspace");

    let runner = assemble(
        &runner_root,
        "runner-b",
        &edge_url,
        runner_runs.clone(),
        release.clone(),
    );
    runner
        .cloud
        .as_ref()
        .expect("runner CloudStore")
        .save_seat(zeron_engine::cloud::SeatConfig {
            id: "continuity-runner".into(),
            kind: Some(zeron_proto::CloudSeatKind::Ssh),
            label: "Continuity runner".into(),
            runner_device_id: Some("runner-b".into()),
            ..Default::default()
        })
        .expect("save runner seat");
    runner
        .workspace
        .create_chat_with_parent(
            CHAT,
            None,
            Some("runner-b"),
            None,
            Some(workspace.display().to_string()),
            None,
            Some(binding(&workspace)),
        )
        .expect("create runner chat row");
    runner
        .workspace
        .rename_chat(CHAT, "Runner continuity")
        .expect("avoid title generation run");
    drop(runner.doc_host.open(CHAT).expect("open runner chat"));

    let viewport = assemble(
        &viewport_root,
        "viewport-a",
        &edge_url,
        viewport_runs.clone(),
        release.clone(),
    );
    viewport
        .workspace
        .create_chat_with_parent(
            CHAT,
            None,
            Some("runner-b"),
            None,
            Some(workspace.display().to_string()),
            None,
            Some(binding(&workspace)),
        )
        .expect("create viewport chat row");
    viewport
        .workspace
        .rename_chat(CHAT, "Runner continuity")
        .expect("name viewport chat");
    drop(viewport.doc_host.open(CHAT).expect("open viewport chat"));
    viewport
        .doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: RunRequest {
                    prompt: "run on the user-owned runner".into(),
                    harness: None,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    cwd: workspace.display().to_string(),
                    sandbox: SandboxLevel::ReadOnly,
                    auto_approve: true,
                    attachments: vec![],
                    worktree: None,
                    resume: None,
                },
                message_id: "viewport-runner-turn".into(),
            },
        )
        .expect("queue viewport command");

    assert!(
        wait_until(|| {
            runner_runs.load(Ordering::Acquire) == 1
                && runner
                    .doc_host
                    .open(CHAT)
                    .ok()
                    .and_then(|handle| handle.doc().read_entries().ok())
                    .is_some_and(|entries| contains_text(&entries, "runner started"))
        })
        .await,
        "runner harness starts"
    );

    viewport.shutdown().await;
    drop(viewport);
    release.notify_one();

    assert!(
        wait_until(|| {
            runner
                .sessions
                .session_status(CHAT)
                .is_some_and(|session| session.status == SessionStatus::Idle)
                && runner
                    .doc_host
                    .open(CHAT)
                    .ok()
                    .and_then(|handle| handle.doc().read_entries().ok())
                    .is_some_and(|entries| contains_text(&entries, "runner finished"))
        })
        .await,
        "runner turn finishes"
    );
    assert_eq!(
        runner_runs.load(Ordering::Acquire),
        1,
        "runner executes once"
    );
    assert_eq!(
        viewport_runs.load(Ordering::Acquire),
        0,
        "viewport does not execute the harness"
    );
    assert!(
        wait_until(|| {
            runner.doc_host.sync_statuses().iter().any(|(chat, stats)| {
                chat == CHAT
                    && stats.is_some_and(|stats| {
                        stats.server_known
                            && stats.cursor == stats.head_seq
                            && stats.pending_pushes == 0
                    })
            })
        })
        .await,
        "runner chat2 room catches up before transcript snapshot"
    );
    let expected = runner
        .doc_host
        .open(CHAT)
        .expect("read runner transcript")
        .doc()
        .read_entries()
        .expect("runner entries");

    let runner_profile = EngineProfile::synced(&runner_root, "continuity-org", "continuity-user");
    assert!(
        runner
            .cloud
            .as_ref()
            .expect("runner CloudStore")
            .runs()
            .is_empty(),
        "runner-hosted turns do not create CloudRun rows"
    );
    assert!(
        !runner_profile.store_root().join("cloud-runs.json").exists(),
        "runner-hosted turns do not write cloud-runs.json"
    );

    let viewport_profile =
        EngineProfile::synced(&viewport_root, "continuity-org", "continuity-user");
    if delete_docs_cache {
        let docs_db = viewport_profile.store_root().join("docs.sqlite3");
        for path in [
            docs_db.clone(),
            viewport_profile.store_root().join("docs.sqlite3-wal"),
            viewport_profile.store_root().join("docs.sqlite3-shm"),
        ] {
            if path.exists() {
                std::fs::remove_file(path).expect("delete viewport docs cache");
            }
        }
        assert!(!docs_db.exists(), "viewport docs cache was removed");
    }

    let reassembled = assemble(
        &viewport_root,
        "viewport-a",
        &edge_url,
        viewport_runs.clone(),
        release.clone(),
    );
    let viewport_handle = reassembled
        .doc_host
        .open(CHAT)
        .expect("reopen viewport chat");
    let caught_up = wait_until(|| {
        viewport_handle
            .doc()
            .read_entries()
            .is_ok_and(|entries| entries == expected)
    })
    .await;
    if !caught_up {
        let rows = relay
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .rows
            .iter()
            .map(|row| (row.device.clone(), row.bytes.len()))
            .collect::<Vec<_>>();
        let statuses = reassembled.doc_host.sync_statuses();
        let entries = viewport_handle
            .doc()
            .read_entries()
            .expect("read viewport transcript after cache recovery timeout");
        panic!(
            "viewport transcript catch-up timed out: sync={statuses:?}, relay_rows={rows:?}, actual={entries:?}, expected={expected:?}"
        );
    }
    assert_eq!(
        viewport_handle
            .doc()
            .read_entries()
            .expect("viewport entries"),
        expected,
        "chat2 reassembly reproduces the runner transcript"
    );
    assert_eq!(
        runner_runs.load(Ordering::Acquire),
        1,
        "runner executed exactly once"
    );
    assert_eq!(
        viewport_runs.load(Ordering::Acquire),
        0,
        "viewports never run harness"
    );
    assert!(
        reassembled
            .cloud
            .as_ref()
            .expect("viewport CloudStore")
            .runs()
            .is_empty(),
        "viewport does not create CloudRun rows"
    );
    assert!(
        !viewport_profile
            .store_root()
            .join("cloud-runs.json")
            .exists(),
        "viewport does not write cloud-runs.json"
    );

    drop(viewport_handle);
    reassembled.shutdown().await;
    runner.shutdown().await;
    relay_task.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runner_turn_survives_viewport_shutdown_and_reassembly() {
    continuity_scenario(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn chat2_rebuilds_transcript_after_viewport_docs_cache_deletion() {
    continuity_scenario(true).await;
}
