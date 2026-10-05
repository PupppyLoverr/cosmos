//! SessionsEngine — per-chat agent runs: dispatch, steering, interrupts, input bridging,
//! journal + broadcast fan-out, and 120ms coalesced doc streaming.
//!
//! Pragmatic port of zeron's `sessions.ts` (spec: feature-inventory §3.2):
//! - every `AgentEvent` is (a) appended to the on-disk run journal, (b) broadcast to
//!   in-process subscribers, (c) folded via `fold_event_into_parts` and diffed into the
//!   chat's `SessionDoc` through `SegmentWriter` on a coalesced `STREAM_COMMIT_MS` timer;
//! - the user message entry is pushed to the doc immediately on dispatch (id = the
//!   command's client-minted message id, so optimistic echoes never flicker);
//! - a `Steered` event splits the assistant entry at the exact boundary;
//! - recovery (interrupt or a stale journal at boot) stamps the streaming entry `aborted`.
//!
//! Scope notes: sessions are keyed by chat id (one live run per chat). Zeron's pulse
//! loop is ported as the 15s liveness heartbeat in `drive_run`; its stall watchdog is
//! deliberately NOT ported (rejected in review — agents may legitimately wait on
//! something for far longer than any timeout, and a live child IS the working signal).
//! Every dying path must instead carry its own visible error (child crash with stderr,
//! spawn failure, stream error, engine-restart recovery).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use chrono::Utc;
use futures::StreamExt;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use zeron_doc::{
    DocError, MessagePart, MessageRole, MessageStatus, STREAM_COMMIT_MS, SegmentWriter, SessionDoc,
    SessionMessageEntry, fold_event_into_parts, sanitize_tool_call,
};
use zeron_harness::{CancellationToken, Harness, RunControls, RunEnvironment, SteerMessage};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, RunRequest, Session, SessionStatus, UserInputAnswer,
    UserInputQuestion,
};

use crate::doc_host::{ChatDocHandle, DocHost};
use crate::registry::{HarnessDescriptor, HarnessRegistry};
use crate::run_journal::RunJournal;
use crate::{EngineError, new_id, now_ms};

/// One journaled event: the durable seq plus the event, as broadcast to subscribers.
#[derive(Debug, Clone)]
pub struct JournaledEvent {
    pub seq: u64,
    pub event: AgentEvent,
}

/// Outcome of a steer attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteerOutcome {
    /// Delivered into the live run's steering mailbox.
    Accepted,
    /// No live steerable run — the caller should dispatch the prompt as a new turn.
    NotSteerable,
}

type PendingInputs = Arc<Mutex<HashMap<String, oneshot::Sender<Vec<UserInputAnswer>>>>>;

/// A harness-native session id plus the cwd it was created under. Harness
/// session stores are cwd-scoped (claude keys conversations by project
/// directory — zeron sessions.ts:563 "harness session stores are keyed by
/// cwd"), so resume is only injected for runs launched from the same cwd.
#[derive(Debug, Clone)]
struct HarnessSessionRef {
    session_id: String,
    cwd: String,
}

/// Configuration baked into a live harness runtime. The steering mailbox only
/// carries prompt text, so routing a request whose model/effort/sandbox changed
/// would silently run it with the old process configuration. Such requests
/// must replace the runtime and resume its harness-native session instead.
#[derive(Debug, Clone, PartialEq)]
struct RuntimeConfig {
    harness_id: HarnessId,
    model: Option<String>,
    reasoning: Option<zeron_proto::ReasoningLevel>,
    model_options: serde_json::Map<String, serde_json::Value>,
    cwd: String,
    sandbox: zeron_proto::SandboxLevel,
    auto_approve: bool,
    worktree: Option<zeron_proto::WorktreeSpec>,
}

impl RuntimeConfig {
    fn from_request(harness_id: HarnessId, request: &RunRequest) -> Self {
        // Internal keys mark ONE run's behavior (an attach's replay flag, the
        // host-local browser endpoint), never the session's runtime shape — a
        // stamped attach followed by an unstamped send must still route into
        // the parked session, so they are filtered out of the comparison.
        let model_options: serde_json::Map<String, serde_json::Value> = request
            .model_options
            .iter()
            .filter(|(k, _)| !zeron_proto::is_internal_option(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Self {
            harness_id,
            model: request.model.clone(),
            reasoning: request.reasoning,
            model_options,
            cwd: request.cwd.clone(),
            sandbox: request.sandbox,
            auto_approve: request.auto_approve,
            worktree: request.worktree.clone(),
        }
    }

    fn can_route(&self, harness_id: HarnessId, request: &RunRequest) -> bool {
        request.attachments.is_empty() && self == &Self::from_request(harness_id, request)
    }
}

struct RunHandle {
    run_id: String,
    steerable: bool,
    runtime_config: RuntimeConfig,
    steer_tx: mpsc::Sender<SteerMessage>,
    /// Harness-level cancellation (protocol interrupt + child teardown).
    interrupt_token: CancellationToken,
    /// Engine-level cancel: arms the run task's grace deadline so a harness that
    /// ignores its token can never strand the run.
    cancel: watch::Sender<bool>,
    engine_tx: mpsc::UnboundedSender<AgentEvent>,
    pending_inputs: PendingInputs,
    /// Steers accepted into the mailbox but not yet confirmed by a `Steered`
    /// event — the at-least-once ledger. A run can die with accepted steers
    /// still in its mailbox (idle reaper vs. a routed send; a mid-turn error
    /// discarding queued boundary steers): the run task drains this at exit
    /// and re-dispatches each entry as a fresh turn, so an accepted message
    /// can never silently evaporate from a transcript that shows it as sent.
    routed_steers: Arc<Mutex<std::collections::VecDeque<RoutedSteer>>>,
}

/// One accepted-but-unconfirmed steer: enough to re-dispatch it verbatim.
#[derive(Debug, Clone)]
struct RoutedSteer {
    prompt: String,
    message_id: String,
}

struct Inner {
    browsers: Mutex<crate::browser::BrowserRegistry>,
    /// Cosmos Computer — the device-local desktop service (wired at engine
    /// assembly). Its loopback MCP endpoint is stamped onto local runs the
    /// same way the browser bridge is; absent in bare tests.
    computer: OnceLock<zeron_computer::ComputerService>,
    /// Cosmos Tools — the device-local merged MCP gateway (Composio + user
    /// MCP servers). Same stamping seam as computer; absent in bare tests.
    tools: OnceLock<zeron_tools::ToolService>,
    device_id: String,
    journal: Arc<RunJournal>,
    registry: Arc<HarnessRegistry>,
    /// Set-once (first wins), cleared on runtime retirement: sessions and
    /// doc-host reference each other through Arcs, so this back-edge must be
    /// severable for a replaced engine graph to drop.
    doc_host: Mutex<Option<DocHost>>,
    /// chat_id → live run.
    runs: Mutex<HashMap<String, RunHandle>>,
    /// chat_id → broadcast hub (retained across runs so subscribers survive turns).
    hubs: Mutex<HashMap<String, broadcast::Sender<JournaledEvent>>>,
    statuses: Mutex<HashMap<String, Session>>,
    sessions_tx: watch::Sender<Vec<Session>>,
    /// Last dispatched request per chat — the steer→new-turn fallback re-derives its
    /// run config from this (chat config rows land with the workspace doc in M4).
    last_requests: Mutex<HashMap<String, RunRequest>>,
    /// Cloud-seat store (Cloud/GitHub agent mode): seat config, BYOK keys,
    /// the remote-run ledger. A chat whose row carries `cloudSeat` dispatches
    /// to the seat instead of a local harness. Absent in bare tests.
    cloud: OnceLock<crate::cloud::CloudStore>,
    runner_credentials: OnceLock<Arc<crate::runner_credentials::RunnerCredentialVault>>,
    /// Harness-native session ids per chat (resume continuity across turns) —
    /// the live-process cache over the durable copy on the workspace chat row
    /// (zeron kept the same pair on `chats.harness_session_id`). An empty
    /// session id is the "do not resume" tombstone after a rejected resume.
    harness_sessions: Mutex<HashMap<String, HarnessSessionRef>>,
    /// Auto-titler for untitled chats (wired at engine assembly; absent in bare tests).
    titles: OnceLock<crate::titles::TitleGenerator>,
    generated_images: OnceLock<(crate::uploads::Uploads, std::path::PathBuf)>,
    /// Fired with `(chat_id, cwd)` when a user prompt starts a turn (fresh
    /// dispatch or accepted steer) — the diff sync snapshots the checkout tree
    /// for the Changes pane's "Latest turn" scope. Absent in bare tests.
    turn_listener: OnceLock<TurnListener>,
}

/// Turn-start hook: called with `(chat_id, cwd)`.
pub type TurnListener = Arc<dyn Fn(&str, &str) + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
pub struct SessionsEngine {
    inner: Arc<Inner>,
}

fn cloud_run_is_live(run: &zeron_proto::CloudRun) -> bool {
    matches!(
        run.status,
        zeron_proto::CloudRunStatus::Working | zeron_proto::CloudRunStatus::Queued
    )
}

fn insert_cloud_run_candidate(
    candidates: &mut HashMap<String, zeron_proto::CloudRun>,
    run: zeron_proto::CloudRun,
) {
    match candidates.entry(run.chat_id.clone()) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(run);
        }
        std::collections::hash_map::Entry::Occupied(mut entry) => {
            if cloud_run_progress_cmp(&run, entry.get()).is_gt() {
                entry.insert(run);
            }
        }
    }
}

fn cloud_run_progress_cmp(
    left: &zeron_proto::CloudRun,
    right: &zeron_proto::CloudRun,
) -> std::cmp::Ordering {
    left.mirrored_bytes
        .cmp(&right.mirrored_bytes)
        .then_with(|| match (&left.remote_cursor, &right.remote_cursor) {
            (Some(left), Some(right)) => match (left.parse::<u64>(), right.parse::<u64>()) {
                (Ok(left), Ok(right)) => left.cmp(&right),
                _ => left.cmp(right),
            },
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        })
        .then_with(|| left.updated_at.cmp(&right.updated_at))
}

impl SessionsEngine {
    /// A live desktop viewport renews its bridge lease over local IPC.
    pub fn register_browser(&self, url: &str) -> Result<(), String> {
        lock(&self.inner.browsers).renew(url, std::time::Instant::now())
    }

    pub fn new(
        device_id: String,
        journal: Arc<RunJournal>,
        registry: Arc<HarnessRegistry>,
    ) -> Self {
        let (sessions_tx, _) = watch::channel(Vec::new());
        Self {
            inner: Arc::new(Inner {
                browsers: Mutex::new(Default::default()),
                computer: OnceLock::new(),
                tools: OnceLock::new(),
                device_id,
                journal,
                registry,
                doc_host: Mutex::new(None),
                runs: Mutex::new(HashMap::new()),
                hubs: Mutex::new(HashMap::new()),
                statuses: Mutex::new(HashMap::new()),
                sessions_tx,
                last_requests: Mutex::new(HashMap::new()),
                cloud: OnceLock::new(),
                runner_credentials: OnceLock::new(),
                harness_sessions: Mutex::new(HashMap::new()),
                titles: OnceLock::new(),
                generated_images: OnceLock::new(),
                turn_listener: OnceLock::new(),
            }),
        }
    }

    /// Wire the device-local Cosmos Computer service (called once at engine
    /// assembly). `dispatch_inner` stamps its loopback endpoint onto runs
    /// only while the service reports ready on THIS host.
    pub fn set_computer(&self, computer: zeron_computer::ComputerService) {
        let _ = self.inner.computer.set(computer);
    }

    /// Wire the device-local Cosmos Tools service (called once at engine
    /// assembly). Same stamping contract as [`set_computer`].
    pub fn set_tools(&self, tools: zeron_tools::ToolService) {
        let _ = self.inner.tools.set(tools);
    }

    /// Wire the doc host (called once at engine assembly; the two services are mutually
    /// referential by design — sessions stream into docs, docs execute commands here).
    pub fn set_doc_host(&self, host: DocHost) {
        // First set wins (the OnceLock contract this slot replaced).
        let mut slot = lock(&self.inner.doc_host);
        if slot.is_none() {
            *slot = Some(host);
        }
    }

    /// Sever the doc-host back-edge (runtime retirement; the doc host's
    /// `shutdown_workers` clears its own sessions edge). Every access site
    /// already treats a missing doc host as "not wired".
    pub fn clear_doc_host(&self) {
        lock(&self.inner.doc_host).take();
    }

    /// Bind generated-image intake to the same profile store used by attachment RPCs.
    pub fn set_generated_images(
        &self,
        uploads: crate::uploads::Uploads,
        allowed_root: std::path::PathBuf,
    ) {
        let _ = self.inner.generated_images.set((uploads, allowed_root));
    }

    /// Wire the chat auto-titler (called once at engine assembly). After each
    /// completed exchange the run task fires it for still-untitled chats.
    pub fn set_titles(&self, titles: crate::titles::TitleGenerator) {
        let _ = self.inner.titles.set(titles);
    }

    /// Wire the turn-start listener (called once at engine assembly).
    pub fn set_turn_listener(&self, listener: TurnListener) {
        let _ = self.inner.turn_listener.set(listener);
    }

    /// Wire the cloud-seat store (called once at engine assembly). Chats
    /// whose workspace row carries a `cloudSeat` binding dispatch remotely.
    pub fn set_cloud(&self, store: crate::cloud::CloudStore) {
        let _ = self.inner.cloud.set(store);
    }

    pub fn set_runner_credentials(
        &self,
        vault: Arc<crate::runner_credentials::RunnerCredentialVault>,
    ) {
        let _ = self.inner.runner_credentials.set(vault);
    }

    /// After a restart, pick up the cloud runs the ledger still calls live.
    /// Detached seat runs and Boat runs kept working while Cosmos was closed
    /// — their output resumes mirroring into the chat from where it stopped.
    /// A run that streamed over this app's own ssh link died with the app;
    /// it is marked stopped, with one honest line in the transcript.
    pub fn resume_cloud_runs(&self) {
        let Some(store) = self.inner.cloud.get().cloned() else {
            return;
        };
        let sessions = self.clone();
        tokio::spawn(async move {
            // Chat docs live in the workspace host, which attaches after the
            // engine core is assembled.
            for _ in 0..120 {
                if sessions.inner.workspace().is_some() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            let Some(workspace) = sessions.inner.workspace() else {
                return;
            };
            let mut live = HashMap::new();
            for run in store.runs().into_iter().filter(cloud_run_is_live) {
                insert_cloud_run_candidate(&mut live, run);
            }
            let chats = match workspace.read_chats() {
                Ok(chats) => chats,
                Err(error) => {
                    tracing::warn!(%error, "could not read cloud runs from registry");
                    Vec::new()
                }
            };
            let runner_chats = chats
                .iter()
                .filter(|chat| {
                    chat.cloud_seat
                        .as_ref()
                        .is_some_and(|binding| binding.runner_device_id.is_some())
                })
                .map(|chat| chat.id.clone())
                .collect::<std::collections::HashSet<_>>();
            for chat in chats {
                if runner_chats.contains(&chat.id) {
                    continue;
                }
                if let Some(run) = chat.cloud_run.filter(cloud_run_is_live) {
                    insert_cloud_run_candidate(&mut live, run);
                }
            }
            for (chat_id, run) in live {
                if runner_chats.contains(&chat_id) {
                    continue;
                }
                store.upsert_run(run.clone());
                if lock(&sessions.inner.runs).contains_key(&run.chat_id) {
                    continue;
                }
                sessions.reattach_cloud_run(&store, run);
            }
        });
    }

    fn reattach_cloud_run(&self, store: &crate::cloud::CloudStore, run: zeron_proto::CloudRun) {
        let chat_id = run.chat_id.clone();
        let mark = |status: zeron_proto::CloudRunStatus, error: &str| {
            store.update_run(&run.id, |r| {
                r.status = status;
                r.error = Some(error.to_string());
                r.finished_at = Some(Utc::now());
            });
        };
        let binding = self
            .inner
            .workspace()
            .and_then(|ws| ws.chat(&chat_id).ok().flatten())
            .and_then(|chat| chat.cloud_seat);
        if binding
            .as_ref()
            .is_some_and(|binding| binding.runner_device_id.is_some())
        {
            return;
        }
        let seat = binding
            .as_ref()
            .and_then(|binding| store.seat_for(binding))
            .or_else(|| {
                store
                    .seat(&run.seat_id)
                    .map(|seat| seat.scoped(run.workspace.as_deref(), None))
            });
        let Some(seat) = seat else {
            mark(
                zeron_proto::CloudRunStatus::Error,
                "this run's cloud seat is no longer configured",
            );
            return;
        };
        let Ok(handle) = self.doc_handle(&chat_id) else {
            mark(
                zeron_proto::CloudRunStatus::Error,
                "this run's chat is no longer available",
            );
            return;
        };
        let doc = handle.writer();
        let box_id = run
            .remote_ref
            .as_deref()
            .and_then(|r| r.strip_prefix("boat:"))
            .map(str::to_string);
        if box_id.is_none() && run.log_path.is_none() {
            let message = "Cosmos closed while this run was streaming from the seat, and the run ended with it. Send the message again to continue.";
            write_transcript_note(&doc, &self.inner.device_id, message);
            mark(zeron_proto::CloudRunStatus::Stopped, message);
            return;
        }
        if seat.expired() {
            let message = "This trial seat's hour ended while Cosmos was closed.";
            write_transcript_note(&doc, &self.inner.device_id, message);
            mark(zeron_proto::CloudRunStatus::Error, message);
            return;
        }

        // A live handle, so Stop and the status row work as for a fresh run.
        let (steer_tx, _steer_rx) = mpsc::channel::<SteerMessage>(1);
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let (engine_tx, _engine_rx) = mpsc::unbounded_channel::<AgentEvent>();
        let interrupt_token = CancellationToken::new();
        let workspace = seat.workspace();
        lock(&self.inner.runs).insert(
            chat_id.clone(),
            RunHandle {
                run_id: run.id.clone(),
                steerable: false,
                runtime_config: RuntimeConfig {
                    harness_id: HarnessId::ClaudeCode,
                    model: None,
                    reasoning: None,
                    model_options: serde_json::Map::new(),
                    cwd: workspace,
                    sandbox: zeron_proto::SandboxLevel::DangerFullAccess,
                    auto_approve: true,
                    worktree: None,
                },
                steer_tx,
                interrupt_token: interrupt_token.clone(),
                cancel: cancel_tx,
                engine_tx,
                pending_inputs: Arc::new(Mutex::new(HashMap::new())),
                routed_steers: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            },
        );
        self.set_status(&chat_id, SessionStatus::Working, true);
        write_transcript_note(
            &doc,
            &self.inner.device_id,
            "Reconnected — this run kept going on the cloud seat while Cosmos was closed.",
        );

        let inner = self.inner.clone();
        let store = store.clone();
        tokio::spawn(async move {
            let http = reqwest::Client::new();
            let command = run.command.clone().unwrap_or_default();
            let session_id = crate::cloud::harness_session_id(&chat_id);
            let turn = RemoteTurn {
                prompt: run.prompt.clone().unwrap_or_default(),
                recap: String::new(),
                session_tag: (box_id.is_none()
                    && crate::cloud::continuation_flags(&command, &session_id, true).is_some())
                .then(|| format!("{}:{session_id}", crate::cloud::command_bin(&command))),
                harness: run.harness.unwrap_or(HarnessId::ClaudeCode),
                model: run.model.clone(),
                install: String::new(),
            };
            let persist_seat = match box_id {
                Some(box_id) => {
                    let Some(token) = store.box_token(&seat.id) else {
                        write_remote_error(
                            &doc,
                            &inner.device_id,
                            "this Boat seat has no token — set one in Settings → Cloud computer",
                        );
                        inner.set_status(&chat_id, SessionStatus::Errored, false);
                        store.update_run(&run.id, |r| {
                            r.status = zeron_proto::CloudRunStatus::Error;
                            r.finished_at = Some(Utc::now());
                        });
                        lock(&inner.runs).remove(&chat_id);
                        return;
                    };
                    let api = crate::boat::boat(&http, &token, seat.box_endpoint.as_deref());
                    let outcome = mirror_boat_events(
                        &inner,
                        &store,
                        &api,
                        &box_id,
                        &run.id,
                        &doc,
                        &cancel_rx,
                        &interrupt_token,
                        run.remote_cursor.clone(),
                        run.entry_id.as_deref(),
                    )
                    .await;
                    let (status, run_status, error) = match outcome {
                        BoatOutcome::Done => {
                            (SessionStatus::Idle, zeron_proto::CloudRunStatus::Done, None)
                        }
                        BoatOutcome::Stopped => (
                            SessionStatus::Idle,
                            zeron_proto::CloudRunStatus::Stopped,
                            None,
                        ),
                        BoatOutcome::Failed(message) => {
                            write_remote_error(&doc, &inner.device_id, &message);
                            (
                                SessionStatus::Errored,
                                zeron_proto::CloudRunStatus::Error,
                                Some(message),
                            )
                        }
                    };
                    inner.set_status(&chat_id, status, false);
                    store.update_run(&run.id, |r| {
                        r.status = run_status;
                        r.error = error;
                        r.finished_at = Some(Utc::now());
                    });
                    lock(&inner.runs).remove(&chat_id);
                    let mut live = seat.clone();
                    live.box_id = Some(box_id);
                    live
                }
                None => {
                    let log_path = run.log_path.clone().unwrap_or_default();
                    tail_remote_log(
                        &inner,
                        &http,
                        &store,
                        &chat_id,
                        &run.id,
                        &seat,
                        &log_path,
                        &doc,
                        &mut cancel_rx,
                        run.entry_id.as_deref(),
                        &command,
                    )
                    .await;
                    seat.clone()
                }
            };
            persist_remote_turn(
                &inner,
                &store,
                &http,
                &persist_seat,
                &chat_id,
                &run.id,
                &turn,
                &doc,
            )
            .await;
        });
    }

    fn note_turn_start(&self, chat_id: &str, cwd: &str) {
        if let Some(listener) = self.inner.turn_listener.get() {
            listener(chat_id, cwd);
        }
    }

    fn doc_handle(&self, chat_id: &str) -> Result<Arc<ChatDocHandle>, EngineError> {
        let host = self
            .inner
            .doc_host()
            .ok_or_else(|| EngineError::Other("doc host not wired into sessions engine".into()))?;
        host.open(chat_id)
    }

    /// Status watch: the full session list, re-sent on every transition.
    pub fn watch_sessions(&self) -> watch::Receiver<Vec<Session>> {
        self.inner.sessions_tx.subscribe()
    }

    pub fn session_status(&self, chat_id: &str) -> Option<Session> {
        lock(&self.inner.statuses).get(chat_id).cloned()
    }

    /// Whether this harness takes a prompt *during* a turn, rather than only at
    /// the boundary between turns. The queue drain's central question: a
    /// mid-turn agent gets the message now, everything else holds it until the
    /// turn ends. Catalog lookup only — never spawns.
    pub fn steers_mid_turn(&self, harness: HarnessId) -> bool {
        self.inner
            .registry
            .descriptors()
            .iter()
            .find(|d| d.id == harness)
            .is_some_and(HarnessDescriptor::steers_mid_turn)
    }

    /// Whether this chat has a turn in flight — streaming, or parked on a
    /// question it is still owed an answer to (see [`is_active`]). A persistent
    /// session parked BETWEEN turns is not: it holds a warm child with nothing
    /// outstanding, and takes the next prompt straight through its mailbox.
    pub fn turn_in_flight(&self, chat_id: &str) -> bool {
        lock(&self.inner.statuses)
            .get(chat_id)
            .is_some_and(is_active)
    }

    /// Any run currently working or blocked on input — the auto-updater's
    /// "don't restart from under a session" gate.
    pub fn any_active(&self) -> bool {
        lock(&self.inner.statuses).values().any(is_active)
    }

    /// The last request dispatched for a chat (steer→new-turn fallback).
    pub fn last_request(&self, chat_id: &str) -> Option<RunRequest> {
        lock(&self.inner.last_requests).get(chat_id).cloned()
    }

    /// Subscribe to a chat's live event stream: returns the journal replay after
    /// `after_seq` plus a live receiver. Subscribe-then-replay ordering means overlap
    /// (dedupe by seq) rather than gaps.
    pub fn subscribe(
        &self,
        chat_id: &str,
        after_seq: u64,
    ) -> Result<(Vec<JournaledEvent>, broadcast::Receiver<JournaledEvent>), EngineError> {
        let rx = {
            let mut hubs = lock(&self.inner.hubs);
            hubs.entry(chat_id.to_string())
                .or_insert_with(|| broadcast::channel(1024).0)
                .subscribe()
        };
        let replay = self
            .inner
            .journal
            .replay(chat_id, after_seq)?
            .into_iter()
            .map(|(seq, event)| JournaledEvent { seq, event })
            .collect();
        Ok((replay, rx))
    }

    /// Start (or route) a run for `chat_id`.
    ///
    /// - The user message entry is written to the doc immediately (id = `message_id`).
    /// - A live steerable run receives the prompt as its next turn via the mailbox
    ///   (zeron's persistent-session routing); otherwise any live run is interrupted
    ///   first — never two runtimes driving one chat.
    pub async fn dispatch(
        &self,
        chat_id: &str,
        harness_id: HarnessId,
        request: RunRequest,
        message_id: Option<String>,
    ) -> Result<String, EngineError> {
        self.dispatch_with(chat_id, harness_id, request, message_id, false)
            .await
    }

    /// [`Self::dispatch`] with the startup-crash retry marker: the retry
    /// re-dispatches with `startup_retry = true`, which makes that attempt
    /// final (its own startup death surfaces instead of retrying again).
    /// Boxed future: `drive_run` re-enters this for that retry, and the
    /// erasure breaks the opaque-type cycle the recursion would otherwise form.
    fn dispatch_with<'a>(
        &'a self,
        chat_id: &'a str,
        harness_id: HarnessId,
        request: RunRequest,
        message_id: Option<String>,
        startup_retry: bool,
    ) -> futures::future::BoxFuture<'a, Result<String, EngineError>> {
        Box::pin(self.dispatch_inner(chat_id, harness_id, request, message_id, startup_retry))
    }

    async fn dispatch_inner(
        &self,
        chat_id: &str,
        harness_id: HarnessId,
        mut request: RunRequest,
        mut message_id: Option<String>,
        startup_retry: bool,
    ) -> Result<String, EngineError> {
        // Project-less chats store cwd `~` (the creating device can't know the
        // host's home); expand it here, on the host, where the run spawns.
        request.cwd = crate::repos::expand_home(&request.cwd)
            .map_err(|error| EngineError::Other(error.to_string()))?;
        // Native-only catalog entries have no portable file fallback. Reject
        // cross-harness delivery before recording or routing the user turn.
        zeron_proto::invocation::validate_harness_invocations(&request.prompt, harness_id)
            .map_err(EngineError::Other)?;
        let mut runner_environment = RunEnvironment::default();
        if let Some(binding) = self
            .inner
            .workspace()
            .and_then(|ws| ws.chat(chat_id).ok().flatten())
            .and_then(|chat| chat.cloud_seat)
        {
            if let Some(runner_id) = binding.runner_device_id.as_deref() {
                if runner_id != self.inner.device_id {
                    return Err(EngineError::Other(
                        "this Cloud chat runs on its runner".into(),
                    ));
                }
                let vault = self.inner.runner_credentials.get().map(Arc::as_ref);
                runner_environment = match crate::runner_runtime::harness_environment(
                    vault,
                    harness_id,
                    request.model.as_deref(),
                ) {
                    Ok(environment) => environment,
                    Err(message) => {
                        return self.runner_dispatch_failure(
                            chat_id,
                            &request.prompt,
                            message_id,
                            &message,
                        );
                    }
                };
                let workspace =
                    match crate::runner_runtime::prepare_workspace(&binding, vault).await {
                        Ok(path) => path,
                        Err(message) => {
                            return self.runner_dispatch_failure(
                                chat_id,
                                &request.prompt,
                                message_id,
                                &message,
                            );
                        }
                    };
                request.cwd = workspace.to_string_lossy().into_owned();
                request.worktree = None;
            } else if let Some(store) = self.inner.cloud.get() {
                return self
                    .dispatch_remote(store, chat_id, &binding, harness_id, request, message_id)
                    .await;
            } else {
                return Err(EngineError::Other(
                    "Cloud seat is unavailable on this profile".into(),
                ));
            }
        }
        lock(&self.inner.browsers).prepare(&mut request, std::time::Instant::now());
        // Cosmos Computer rides the same seam: engine-stamped only, only while
        // the service is enabled + ready on THIS host, and always re-stamped so
        // a stale/remote endpoint can never survive into a local run.
        request.model_options.remove(zeron_proto::COMPUTER_OPTION);
        if let Some(computer) = self.inner.computer.get()
            && let Some(url) = computer.endpoint_url()
        {
            request.model_options.insert(
                zeron_proto::COMPUTER_OPTION.into(),
                serde_json::json!({ "url": url }),
            );
        }
        // Cosmos Tools identical: the merged MCP gateway stamps only while at
        // least one provider is enabled on THIS host; stale options are always
        // removed first.
        request.model_options.remove(zeron_proto::TOOLS_OPTION);
        if let Some(tools) = self.inner.tools.get()
            && let Some(url) = tools.endpoint_url()
        {
            request.model_options.insert(
                zeron_proto::TOOLS_OPTION.into(),
                serde_json::json!({ "url": url }),
            );
        }
        // OpenCode discovers MCP servers only from its own config files —
        // without a seed, project-less chats (any cwd, e.g. the phone's
        // "Desktop" sessions landing in ~) get no computer/tools surface at
        // all. Seed once here, when a service is actually live.
        if request
            .model_options
            .contains_key(zeron_proto::COMPUTER_OPTION)
            || request
                .model_options
                .contains_key(zeron_proto::TOOLS_OPTION)
        {
            zeron_harness::opencode::seed_global_config();
        }
        // Every dispatched prompt is a turn — routed steer or fresh run alike.
        self.note_turn_start(chat_id, &request.cwd);
        let routed = lock(&self.inner.runs).get(chat_id).map(|h| {
            (
                h.run_id.clone(),
                h.steerable,
                h.runtime_config.can_route(harness_id, &request),
                h.steer_tx.clone(),
                h.routed_steers.clone(),
            )
        });
        if let Some((run_id, steerable, same_runtime, steer_tx, ledger)) = routed {
            let user_id = message_id.clone().unwrap_or_else(new_id);
            let accepted = if steerable && same_runtime {
                // Warm dispatch uses the same mailbox as explicit steering.
                // Register acceptance before a fast boundary can retire it.
                let mut pending = lock(&ledger);
                let message = SteerMessage {
                    // OpenCode must see the canonical selection before it
                    // decodes the provider command: a project-scoped command
                    // can disappear between composer discovery and delivery.
                    prompt: if harness_id == HarnessId::Opencode {
                        request.prompt.clone()
                    } else {
                        zeron_proto::invocation::harness_prompt(&request.prompt, harness_id)
                    },
                    message_id: Some(user_id.clone()),
                };
                if steer_tx.try_send(message).is_ok() {
                    pending.push_back(RoutedSteer {
                        prompt: request.prompt.clone(),
                        message_id: user_id.clone(),
                    });
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if accepted {
                let handle = self.doc_handle(chat_id)?;
                handle.write_user_message(&user_id, &request.prompt, now_ms())?;
                if self.is_live(chat_id, &run_id) {
                    // Working BEFORE the lastMessageAt bump: both ride the
                    // workspace doc from this one peer, so causal order makes it
                    // impossible for an observer to hold [new message, old status]
                    // — that gap read as unseen-with-no-live-run = a phantom
                    // "completed" flash on every remote send (2026-07-31).
                    self.set_status(chat_id, SessionStatus::Working, false);
                    self.inner.note_message(chat_id, &request.prompt);
                    return Ok(run_id);
                }
                // The run died around the send. If its exit drain already
                // claimed the entry, that re-dispatch owns the message —
                // otherwise reclaim it and fall through to a fresh run.
                let reclaimed = {
                    let mut ledger = lock(&ledger);
                    let before = ledger.len();
                    ledger.retain(|s| s.message_id != user_id);
                    ledger.len() != before
                };
                if !reclaimed {
                    self.inner.note_message(chat_id, &request.prompt);
                    return Ok(run_id);
                }
                // Keep the already-written doc entry's id for the fresh run
                // below (write_user_message dedupes by id).
                message_id = Some(user_id);
            }
            if !same_runtime {
                tracing::debug!(
                    chat = %chat_id,
                    "restarting live harness to apply changed run configuration"
                );
            }
            // Mailbox closed (runtime mid-teardown / non-steering harness) or
            // the routed run died with the message reclaimed, or configuration
            // changed beyond what the text-only mailbox can carry: replace it.
            self.interrupt(chat_id).await?;
        }

        let harness = self.inner.registry.resolve(harness_id)?;
        let handle = self.doc_handle(chat_id)?;
        let user_id = message_id.unwrap_or_else(new_id);
        handle.write_user_message(&user_id, &request.prompt, now_ms())?;

        // Engine-owned resume (zeron sessions.ts:736 — every dispatch read the
        // chat's stored harness session): callers always send `resume: None`;
        // the engine threads the chat's prior harness session back in so a new
        // process (app restart) continues the same harness conversation. The
        // startup-crash retry injects too — a stale id is the harness's
        // problem now (`session/load` falls back to `session/new` internally),
        // and starting the retry fresh silently dropped a good conversation.
        let mut resume_injected = false;
        if request.resume.is_none() {
            request.resume = self.inner.resume_for(chat_id, &request.cwd);
            resume_injected = request.resume.is_some();
        }
        lock(&self.inner.last_requests).insert(chat_id.to_string(), request.clone());

        let run_id = new_id();
        let (steer_tx, steer_rx) = mpsc::channel::<SteerMessage>(32);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (engine_tx, engine_rx) = mpsc::unbounded_channel::<AgentEvent>();
        let pending_inputs: PendingInputs = Arc::new(Mutex::new(HashMap::new()));

        // Input bridge: the harness asks questions; we mint the request id, park the
        // resolver for `respond_input`, and surface the event through the run pipeline.
        let request_input = {
            let pending = pending_inputs.clone();
            let engine_tx = engine_tx.clone();
            Box::new(move |questions: Vec<UserInputQuestion>| {
                let (tx, rx) = oneshot::channel();
                let request_id = new_id();
                lock(&pending).insert(request_id.clone(), tx);
                let _ = engine_tx.send(AgentEvent::InputRequested {
                    request_id,
                    questions,
                });
                rx
            })
        };
        let interrupt_token = CancellationToken::new();
        let controls = RunControls {
            request_input,
            steering: steer_rx,
            interrupt: interrupt_token.clone(),
            environment: runner_environment,
        };

        lock(&self.inner.runs).insert(
            chat_id.to_string(),
            RunHandle {
                run_id: run_id.clone(),
                steerable: harness.supports_steering(),
                runtime_config: RuntimeConfig::from_request(harness_id, &request),
                steer_tx,
                interrupt_token,
                cancel: cancel_tx,
                engine_tx,
                pending_inputs,
                routed_steers: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            },
        );
        self.set_status(chat_id, SessionStatus::Working, true);
        // AFTER Working (same causal-order guarantee as the steer path): the
        // lastMessageAt bump must never be observable ahead of the live run.
        self.inner.note_message(chat_id, &request.prompt);

        // Name the chat NOW, off the first prompt — not after the first
        // exchange completes ("called New session for a long time for no
        // reason"; the titler only needs the prompt and skips titled chats;
        // the Done-time call below stays as the retry for a failed
        // generation).
        if let Some(titles) = self.inner.titles.get() {
            titles.maybe_generate(chat_id, harness_id, &request.prompt, &request.cwd);
        }

        tokio::spawn(drive_run(
            self.inner.clone(),
            chat_id.to_string(),
            run_id.clone(),
            harness,
            request,
            handle.writer(),
            controls,
            engine_rx,
            cancel_rx,
            RunResumeState {
                user_message_id: user_id,
                resume_injected,
                startup_retry,
            },
        ));
        Ok(run_id)
    }

    fn runner_dispatch_failure(
        &self,
        chat_id: &str,
        prompt: &str,
        message_id: Option<String>,
        message: &str,
    ) -> Result<String, EngineError> {
        let handle = self.doc_handle(chat_id)?;
        let message_id = message_id.unwrap_or_else(new_id);
        handle.write_user_message(&message_id, prompt, now_ms())?;
        write_transcript_note(&handle.writer(), &self.inner.device_id, message);
        self.set_status(chat_id, SessionStatus::Errored, false);
        self.inner.note_message(chat_id, prompt);
        Ok(new_id())
    }

    /// Cloud-seat dispatch (Cloud/GitHub agent mode): the chat's `cloudSeat`
    /// binding reroutes the run to the seat's exec surface — today SSH seats
    /// (`docker exec` the seat's `harness_command`), with the output streamed
    /// back into the same doc so the transcript, status row, and Kanban all
    /// read it identically to a local run. Interrupt rides the normal
    /// `RunHandle` (the token kills the ssh link; the run task also pkills
    /// the remote marker best-effort). Honest failure modes: unconfigured
    /// seat, missing harness command, or a backend without exec → the send
    /// surfaces the error instead of silently running on this PC.
    async fn dispatch_remote(
        &self,
        store: &crate::cloud::CloudStore,
        chat_id: &str,
        binding: &zeron_proto::CloudSeatBinding,
        harness_id: HarnessId,
        request: RunRequest,
        message_id: Option<String>,
    ) -> Result<String, EngineError> {
        let seat = store.seat_for(binding).ok_or_else(|| {
            EngineError::Other(format!(
                "cloud seat `{}` is not configured on this device",
                binding.seat_id
            ))
        })?;
        if seat.expired() {
            return Err(EngineError::Other(
                "this Railway trial seat's hour ended — claim it from the rail, or start a new trial seat".into(),
            ));
        }
        let capabilities = seat.capabilities();
        // Boat seats provision their sandbox lazily on first run — a saved
        // token alone is enough to start (exec capability flips on once the
        // box exists).
        let boat_lazy = matches!(seat.kind, Some(zeron_proto::CloudSeatKind::Box))
            && seat.box_id.is_none()
            && store.box_token(&seat.id).is_some();
        if !capabilities.exec && !boat_lazy {
            return Err(EngineError::Other(format!(
                "seat `{}` ({}) has no exec surface — Files/Changes/PRs still work, but runs need an SSH seat, a Railway trial, or a Boat token",
                seat.label,
                seat.kind.map(|k| k.label()).unwrap_or("unconfigured")
            )));
        }
        // Boat runs ride the provider's integrated-agent `/prompt` surface —
        // no seat-local harness command needed. SSH/Railway exec the seat's
        // configured harness; Railway defaults to the preinstalled `claude`.
        // The composer's harness + model pick runs on the seat — the same
        // agents as on this computer, headless, with the user's logins
        // staged. A run with no explicit pick (automations, older clients)
        // keeps the seat's own configured command.
        let is_boat_seat = matches!(seat.kind, Some(zeron_proto::CloudSeatKind::Box));
        let configured = seat
            .harness_command
            .clone()
            .filter(|c| !c.trim().is_empty());
        let (command, install) = if is_boat_seat {
            (String::new(), String::new())
        } else if request.harness.is_some() || configured.is_none() {
            let picked =
                crate::cloud::seat_harness(harness_id, request.model.as_deref(), request.reasoning)
                    .map_err(EngineError::Other)?;
            (picked.command, picked.install)
        } else {
            (configured.unwrap_or_default(), String::new())
        };

        let handle = self.doc_handle(chat_id)?;
        // Conversation continuity: harnesses with a resumable conversation
        // (Claude Code's session id, OpenCode/Codex "continue last") pick
        // the thread back up natively once a turn on this seat completed;
        // otherwise — and on a chat's first turn on a new seat — the prior
        // turns ride the prompt as a compact recap.
        let session_id = crate::cloud::harness_session_id(chat_id);
        // A turn resumes only the SAME agent's conversation — switching from
        // Claude Code to Codex mid-thread starts Codex fresh (with a recap).
        let bin = crate::cloud::command_bin(&command);
        let resuming = store
            .harness_session_for_chat(chat_id, &seat.id)
            .is_some_and(|tag| tag.starts_with(&format!("{bin}:")));
        let is_boat = matches!(seat.kind, Some(zeron_proto::CloudSeatKind::Box));
        let flags = (!is_boat)
            .then(|| crate::cloud::continuation_flags(&command, &session_id, resuming))
            .flatten();
        let session_tag = (!is_boat
            && crate::cloud::continuation_flags(&command, &session_id, true).is_some())
        .then(|| format!("{bin}:{session_id}"));
        let recap = if resuming && flags.is_some() {
            String::new()
        } else {
            prior_turns_recap(&handle.doc().read_entries().unwrap_or_default())
        };
        let command = if is_boat {
            command
        } else {
            crate::cloud::with_continuation(&command, flags.as_deref())
        };
        let user_id = message_id.unwrap_or_else(new_id);
        handle.write_user_message(&user_id, &request.prompt, now_ms())?;

        let run_id = new_id();
        let (steer_tx, _steer_rx) = mpsc::channel::<SteerMessage>(1);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (engine_tx, _engine_rx) = mpsc::unbounded_channel::<AgentEvent>();
        let interrupt_token = CancellationToken::new();
        lock(&self.inner.runs).insert(
            chat_id.to_string(),
            RunHandle {
                run_id: run_id.clone(),
                steerable: false,
                runtime_config: RuntimeConfig::from_request(harness_id, &request),
                steer_tx,
                interrupt_token: interrupt_token.clone(),
                cancel: cancel_tx,
                engine_tx,
                pending_inputs: Arc::new(Mutex::new(HashMap::new())),
                routed_steers: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            },
        );
        self.set_status(chat_id, SessionStatus::Working, true);
        // Same causal ordering as the local path: the lastMessageAt bump never
        // leads the Working transition.
        self.inner.note_message(chat_id, &request.prompt);
        lock(&self.inner.last_requests).insert(chat_id.to_string(), request.clone());

        // The ledger row drives the Kanban card and the bot bar.
        store.upsert_run(zeron_proto::CloudRun {
            id: run_id.clone(),
            chat_id: chat_id.to_string(),
            seat_id: seat.id.clone(),
            command: Some(command.clone()),
            status: zeron_proto::CloudRunStatus::Working,
            log_path: None,
            remote_ref: None,
            mirrored_bytes: 0,
            remote_cursor: None,
            entry_id: None,
            error: None,
            workspace: Some(seat.workspace()),
            harness: Some(harness_id),
            model: request.model.clone(),
            harness_session: None,
            prompt: request
                .prompt
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(|l| l.chars().take(140).collect()),
            started_at: Utc::now(),
            finished_at: None,
            updated_at: Utc::now(),
        });
        if let Some(titles) = self.inner.titles.get() {
            titles.maybe_generate(chat_id, harness_id, &request.prompt, &seat.workspace());
        }

        tokio::spawn(drive_remote_run(
            self.inner.clone(),
            store.clone(),
            chat_id.to_string(),
            run_id.clone(),
            seat,
            command,
            RemoteTurn {
                prompt: request.prompt,
                recap,
                session_tag,
                harness: harness_id,
                model: request.model.clone(),
                install,
            },
            handle.writer(),
            cancel_rx,
            interrupt_token,
        ));
        Ok(run_id)
    }

    /// Push a steer prompt into the live run's mailbox. `NotSteerable` when no live
    /// steerable run exists — the caller (command executor) dispatches a new turn.
    pub async fn steer(
        &self,
        chat_id: &str,
        prompt: &str,
        message_id: Option<String>,
    ) -> Result<SteerOutcome, EngineError> {
        let target = lock(&self.inner.runs)
            .get(chat_id)
            .filter(|h| h.steerable)
            .map(|h| {
                (
                    h.run_id.clone(),
                    h.runtime_config.harness_id,
                    h.steer_tx.clone(),
                    h.routed_steers.clone(),
                )
            });
        let Some((run_id, harness_id, steer_tx, ledger)) = target else {
            return Ok(SteerOutcome::NotSteerable);
        };
        zeron_proto::invocation::validate_harness_invocations(prompt, harness_id)
            .map_err(EngineError::Other)?;
        let user_id = message_id.unwrap_or_else(new_id);
        let message = SteerMessage {
            prompt: if harness_id == HarnessId::Opencode {
                prompt.to_owned()
            } else {
                zeron_proto::invocation::harness_prompt(prompt, harness_id)
            },
            message_id: Some(user_id.clone()),
        };
        {
            // Serialize mailbox acceptance with confirmation and Done-time
            // inspection: a fast consumer must never outrun its ledger entry.
            let mut pending = lock(&ledger);
            if steer_tx.try_send(message).is_err() {
                return Ok(SteerOutcome::NotSteerable);
            }
            pending.push_back(RoutedSteer {
                prompt: prompt.to_string(),
                message_id: user_id.clone(),
            });
        }
        let handle = self.doc_handle(chat_id)?;
        handle.write_user_message(&user_id, prompt, now_ms())?;
        // A routed steer is a turn too. Fired here (not only on the confirmed
        // path) — a reclaim falls back to dispatch, which just re-snapshots.
        if let Some(request) = self.last_request(chat_id) {
            self.note_turn_start(chat_id, &request.cwd);
        }
        if self.is_live(chat_id, &run_id) {
            self.set_status(chat_id, SessionStatus::Working, false);
            self.inner.note_message(chat_id, prompt);
            return Ok(SteerOutcome::Accepted);
        }
        // The run died around the send. Exit drain claimed the entry → its
        // re-dispatch owns the message; still ours → reclaim and report
        // NotSteerable so the executor falls back to a fresh dispatch
        // (same message id — the doc entry dedupes).
        let reclaimed = {
            let mut ledger = lock(&ledger);
            let before = ledger.len();
            ledger.retain(|s| s.message_id != user_id);
            ledger.len() != before
        };
        if reclaimed {
            return Ok(SteerOutcome::NotSteerable);
        }
        self.inner.note_message(chat_id, prompt);
        Ok(SteerOutcome::Accepted)
    }

    /// Whether a run (local or seat) is live for this chat.
    pub fn is_running(&self, chat_id: &str) -> bool {
        lock(&self.inner.runs).contains_key(chat_id)
    }

    /// Interrupt the live run, if any. The run settles with a synthetic
    /// `Done{interrupted}` and its streaming entry stamped `aborted`; this waits
    /// (bounded) for that settlement so callers observe a consistent doc.
    pub async fn interrupt(&self, chat_id: &str) -> Result<bool, EngineError> {
        let target = lock(&self.inner.runs).get(chat_id).map(|h| {
            (
                h.run_id.clone(),
                h.interrupt_token.clone(),
                h.cancel.clone(),
                h.pending_inputs.clone(),
            )
        });
        let Some((run_id, token, cancel, pending)) = target else {
            return Ok(false);
        };
        // Publish cancellation BEFORE waking the harness. Otherwise a fast
        // EOF after token.cancel() can beat this watch notification and be
        // classified as an error instead of an interrupted turn.
        let _ = cancel.send(true);
        // Unpark questions before harness teardown, which can await them.
        let parked: Vec<_> = lock(&pending).drain().map(|(_, tx)| tx).collect();
        for tx in parked {
            let _ = tx.send(Vec::new());
        }
        // Harness-level interrupt (protocol + child teardown) …
        token.cancel();
        // Bounded settle wait (the run task appends Done + stamps `aborted`).
        for _ in 0..500 {
            if !self.is_live(chat_id, &run_id) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        Ok(true)
    }

    /// Resolve a pending `request_input` question set. Returns `false` when no such
    /// request is pending (unknown id, or the run already settled).
    pub fn respond_input(
        &self,
        chat_id: &str,
        request_id: &str,
        answers: Vec<UserInputAnswer>,
    ) -> Result<bool, EngineError> {
        let target = lock(&self.inner.runs)
            .get(chat_id)
            .map(|h| (h.pending_inputs.clone(), h.engine_tx.clone()));
        let Some((pending, engine_tx)) = target else {
            return Ok(false);
        };
        let Some(resolver) = lock(&pending).remove(request_id) else {
            return Ok(false);
        };
        let _ = resolver.send(answers);
        let _ = engine_tx.send(AgentEvent::InputResolved {
            request_id: request_id.to_string(),
        });
        Ok(true)
    }

    /// Boot recovery: for every journal whose last event is not `Done` (a run died
    /// mid-stream), stamp this device's abandoned `streaming` doc entries `aborted`
    /// with a VISIBLE "Run interrupted by engine restart" error part, close the
    /// journal with a synthetic `Done{interrupted}` — and then PICK THE RUN BACK
    /// UP: a fresh crashed turn with revival budget left is re-dispatched against
    /// the remembered harness session (zeron: "not just eulogized";
    /// `MAX_AUTO_RESUME` = 3 consecutive revivals, fresh = crashed < 12h ago).
    pub fn recover_stale(&self) -> Result<usize, EngineError> {
        const MAX_AUTO_RESUME: u32 = 3;
        const RESUME_FRESH_MS: i64 = 12 * 60 * 60 * 1000;

        let stale = self.inner.journal.stale_sessions()?;
        let mut recovered = 0usize;
        for chat_id in stale {
            if lock(&self.inner.runs).contains_key(&chat_id) {
                continue; // a live run owns this journal
            }
            let handle = self.doc_handle(&chat_id)?;
            let runner_bound = self
                .inner
                .workspace()
                .and_then(|workspace| workspace.chat(&chat_id).ok().flatten())
                .and_then(|chat| chat.cloud_seat)
                .is_some_and(|binding| {
                    binding.runner_device_id.as_deref() == Some(&self.inner.device_id)
                });
            // Harness continuity first: the crashed run's session id may only
            // exist in the journal (the debounced workspace-row write may
            // never have landed) — remember it so the revived run resumes the
            // same harness conversation (zeron recoverDraft, sessions.ts:538).
            if let Some((session_id, cwd)) = self.inner.journal_harness_session(&chat_id) {
                self.inner
                    .remember_harness_session(&chat_id, &session_id, &cwd);
            }
            // The revival prompt: the last user message (idempotent re-dispatch
            // under the SAME id — `write_user_message` dedupes by id, so the
            // transcript never shows a duplicate).
            let prompt = handle.doc().read_entries().ok().and_then(|entries| {
                entries
                    .iter()
                    .rev()
                    .find(|e| e.role == MessageRole::User)
                    .and_then(|e| {
                        e.parts.iter().find_map(|p| match p {
                            MessagePart::Text { text, .. } => Some((e.id.clone(), text.clone())),
                            _ => None,
                        })
                    })
            });
            let attempts = self.inner.journal.resume_attempts(&chat_id);
            let fresh = handle
                .doc()
                .read_entries()
                .ok()
                .and_then(|entries| {
                    entries
                        .iter()
                        .rev()
                        .find(|e| e.status == Some(MessageStatus::Streaming))
                        .map(|e| now_ms() - e.created_at < RESUME_FRESH_MS)
                })
                .unwrap_or(false);
            let will_resume =
                !runner_bound && fresh && prompt.is_some() && attempts < MAX_AUTO_RESUME;

            let note = if runner_bound {
                "Runner restarted mid-turn — send your message again to continue"
            } else if will_resume {
                "Run interrupted by engine restart — resuming"
            } else {
                "Run interrupted by engine restart"
            };
            let done = AgentEvent::Done {
                status: DoneStatus::Interrupted,
                result: None,
                error: Some(note.into()),
                session_id: None,
            };
            self.inner.publish(&chat_id, &done);
            let stamped = handle.mark_abandoned_streams(note)?.len();
            self.set_status(&chat_id, SessionStatus::Idle, false);
            tracing::info!(chat = %chat_id, stamped, will_resume, attempts, "recovered stale session journal");
            recovered += 1;

            if !will_resume {
                continue;
            }
            let attempt = self.inner.journal.note_resume_attempt(&chat_id);
            let (user_id, prompt_text) = prompt.expect("gated by will_resume");
            let sessions = self.clone();
            tokio::spawn(async move {
                let Some(host) = sessions.inner.doc_host() else {
                    return;
                };
                let request = sessions
                    .last_request(&chat_id)
                    .or_else(|| host.request_from_chat_row(&chat_id, &prompt_text))
                    // Last resort: the journal's own cwd (zeron's draft config)
                    // — a crash can predate the debounced workspace-row write.
                    .or_else(|| {
                        let (_, cwd) = sessions.inner.journal_harness_session(&chat_id)?;
                        Some(RunRequest {
                            prompt: String::new(),
                            harness: None,
                            model: None,
                            reasoning: None,
                            model_options: Default::default(),
                            cwd,
                            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
                            auto_approve: false,
                            attachments: Vec::new(),
                            resume: None,
                            worktree: None,
                        })
                    });
                let Some(mut request) = request else {
                    tracing::warn!(chat = %chat_id, "auto-resume skipped: no run config");
                    return;
                };
                request.prompt = prompt_text;
                request.resume = None; // dispatch re-injects the remembered session
                request.attachments = Vec::new();
                let harness_id = host.harness_for_request(&chat_id, &request);
                match host
                    .dispatch_with_source_context(
                        &sessions,
                        &chat_id,
                        harness_id,
                        request,
                        Some(user_id),
                    )
                    .await
                {
                    Ok(_) => {
                        tracing::info!(chat = %chat_id, attempt, "auto-resumed crashed run")
                    }
                    Err(err) => {
                        tracing::warn!(chat = %chat_id, error = %err, "auto-resume dispatch failed")
                    }
                }
            });
        }
        Ok(recovered)
    }

    /// Graceful shutdown: interrupt every live run so streaming entries settle.
    pub async fn shutdown(&self) {
        let chats: Vec<String> = lock(&self.inner.runs).keys().cloned().collect();
        for chat_id in chats {
            if let Err(err) = self.interrupt(&chat_id).await {
                tracing::warn!(chat = %chat_id, error = %err, "shutdown interrupt failed");
            }
        }
    }

    fn is_live(&self, chat_id: &str, run_id: &str) -> bool {
        lock(&self.inner.runs)
            .get(chat_id)
            .is_some_and(|h| h.run_id == run_id)
    }

    fn set_status(&self, chat_id: &str, status: SessionStatus, fresh_start: bool) {
        self.inner.set_status(chat_id, status, fresh_start);
    }
}

impl Inner {
    /// Journal + broadcast one event (the two unconditional legs of the pipeline).
    /// Sanitize before ANY journal/publish path, including nested subagent events.
    async fn prepare_generated_image(
        &self,
        chat_id: &str,
        event: AgentEvent,
        seen: &mut std::collections::HashSet<String>,
    ) -> Vec<AgentEvent> {
        let mut parents = Vec::new();
        let mut leaf = event;
        while let AgentEvent::Subagent {
            parent_tool_use_id,
            event,
        } = leaf
        {
            parents.push(parent_tool_use_id);
            leaf = *event;
        }
        let events = if let AgentEvent::GeneratedImage { id, path, .. } = leaf {
            let key = std::iter::once(chat_id)
                .chain(parents.iter().map(String::as_str))
                .chain(std::iter::once(id.as_str()))
                .collect::<Vec<_>>()
                .join("\0");
            if seen.contains(&key) {
                return vec![];
            }
            let result = if let Some((uploads, allowed_root)) = self.generated_images.get() {
                let uploads = uploads.clone();
                let root = allowed_root.clone();
                let stable_key = key.clone();
                tokio::task::spawn_blocking(move || {
                    uploads.import_generated_image(std::path::Path::new(&path), &root, &stable_key)
                })
                .await
                .unwrap_or_else(|err| Err(EngineError::Other(err.to_string())))
            } else {
                Err(EngineError::Other(
                    "Generated image intake is not configured".into(),
                ))
            };
            match result {
                Ok(image) => {
                    seen.insert(key);
                    vec![AgentEvent::GeneratedImage {
                        id,
                        path: image.path,
                        name: image.name,
                        mime_type: image.mime_type,
                    }]
                }
                Err(err) => {
                    tracing::warn!(chat = %chat_id, error = %err, "generated image import failed");
                    vec![
                        AgentEvent::ToolResult {
                            id: id.strip_suffix(":image").unwrap_or(&id).into(),
                            is_error: true,
                            output: None,
                            diff: None,
                        },
                        AgentEvent::Error {
                            message: "Generated image unavailable".into(),
                        },
                    ]
                }
            }
        } else {
            vec![leaf]
        };
        events
            .into_iter()
            .map(|mut event| {
                for parent in parents.iter().rev() {
                    event = AgentEvent::Subagent {
                        parent_tool_use_id: parent.clone(),
                        event: Box::new(event),
                    };
                }
                event
            })
            .collect()
    }

    fn publish(&self, chat_id: &str, event: &AgentEvent) -> u64 {
        let seq = match self.journal.append(chat_id, event) {
            Ok(seq) => seq,
            Err(err) => {
                tracing::error!(chat = %chat_id, error = %err, "journal append failed");
                0
            }
        };
        if let Some(hub) = lock(&self.hubs).get(chat_id) {
            let _ = hub.send(JournaledEvent {
                seq,
                event: event.clone(),
            });
        }
        seq
    }

    /// Bump the session's freshness on stream activity WITHOUT a status
    /// transition. Long silent-LOOKING stretches (thinking heartbeats, a big
    /// tool input being generated) still carry events — the UI's 45s
    /// staleness gate must not flip "Working" off mid-run. Throttled: a
    /// workspace-doc mirror per delta would be far too chatty.
    fn touch_session(&self, chat_id: &str) {
        const TOUCH_THROTTLE_MS: i64 = 10_000;
        let now = Utc::now();
        let session = {
            let mut statuses = lock(&self.statuses);
            let Some(entry) = statuses.get_mut(chat_id) else {
                return;
            };
            let age = now
                .signed_duration_since(entry.updated_at)
                .num_milliseconds();
            if age < TOUCH_THROTTLE_MS {
                return;
            }
            entry.updated_at = now;
            let session = entry.clone();
            let mut list: Vec<Session> = statuses.values().cloned().collect();
            list.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
            self.sessions_tx.send_replace(list);
            session
        };
        if let Some(ws) = self.workspace() {
            ws.record_session(&session);
        }
    }

    fn set_status(&self, chat_id: &str, status: SessionStatus, fresh_start: bool) {
        self.set_status_with_completion(chat_id, status, fresh_start, None);
    }

    fn has_pending_steers(&self, chat_id: &str, run_id: &str) -> bool {
        lock(&self.runs)
            .get(chat_id)
            .filter(|h| h.run_id == run_id)
            .is_some_and(|h| !lock(&h.routed_steers).is_empty())
    }

    /// Publish the completion marker atomically with the settled status. Keep
    /// it on subsequent Working/heartbeat rows for coalesced and remote watches.
    fn set_status_with_completion(
        &self,
        chat_id: &str,
        status: SessionStatus,
        fresh_start: bool,
        completed_turn: Option<String>,
    ) {
        let now = Utc::now();
        let session = {
            let mut statuses = lock(&self.statuses);
            let entry = statuses
                .entry(chat_id.to_string())
                .or_insert_with(|| Session {
                    last_completed_turn: None,
                    chat_id: chat_id.to_string(),
                    device_id: self.device_id.clone(),
                    status,
                    started_at: None,
                    updated_at: now,
                });
            // `started_at` is the elapsed-timer base and must only ever mean
            // "this turn". Entering Working from a settled state always
            // restamps (a steer into a parked session is a NEW turn — reusing
            // the old base showed the previous turn's 30min on send), and a
            // settled session drops its base entirely so no later reader can
            // resurrect a stale elapsed.
            let was_active = matches!(
                entry.status,
                SessionStatus::Working | SessionStatus::AwaitingInput
            );
            if let Some(turn) = completed_turn {
                entry.last_completed_turn = Some(turn);
            }
            entry.status = status;
            entry.updated_at = now;
            match status {
                SessionStatus::Working if fresh_start || !was_active => {
                    entry.started_at = Some(now);
                }
                SessionStatus::Working | SessionStatus::AwaitingInput => {}
                SessionStatus::Idle | SessionStatus::Errored => {
                    entry.started_at = None;
                }
            }
            let session = entry.clone();
            let mut list: Vec<Session> = statuses.values().cloned().collect();
            list.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
            // send_replace: keep the current value fresh even with no receivers,
            // so late WatchSessions subscribers see the last transition.
            self.sessions_tx.send_replace(list);
            session
        };
        // Mirror the transition into the workspace doc's session-status row so
        // remote devices' sidebars show this run (staleness-checked client-side).
        if let Some(ws) = self.workspace() {
            ws.record_session(&session);
        }
    }

    /// The doc host, once wired. `None` before assembly or after retirement.
    fn doc_host(&self) -> Option<DocHost> {
        lock(&self.doc_host).clone()
    }

    fn workspace(&self) -> Option<crate::workspace_host::WorkspaceHost> {
        self.doc_host().and_then(|host| host.workspace().cloned())
    }

    /// Sidebar freshness: push a message-persist preview into the chat's workspace row.
    fn note_message(&self, chat_id: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(ws) = self.workspace() {
            ws.note_message(chat_id, text);
        }
    }

    /// Record the chat's harness-native session id (and its cwd): live-process
    /// cache plus the durable workspace chat row — the row is what survives an
    /// engine restart (zeron sessions.ts:1039).
    fn remember_harness_session(&self, chat_id: &str, session_id: &str, cwd: &str) {
        if session_id.is_empty() {
            return;
        }
        lock(&self.harness_sessions).insert(
            chat_id.to_string(),
            HarnessSessionRef {
                session_id: session_id.to_string(),
                cwd: cwd.to_string(),
            },
        );
        if let Some(ws) = self.workspace() {
            ws.set_chat_harness_session(chat_id, session_id, cwd);
        }
    }

    // NB: there is deliberately no `forget_harness_session` anymore. The old
    // tombstone fired on "run died before SessionStarted", which — since the
    // ACP conversion made stale ids a harness-internal fallback — only ever
    // meant a child STARTUP failure, and permanently severed good
    // conversations (user incident 2026-08-13). A truly stale id simply
    // yields a fresh session whose SessionStarted overwrites the row.

    /// The session id to resume for a run in `chat_id` launching from `cwd`
    /// (zeron sessions.ts:736, looked up on every dispatch):
    /// live-process cache → workspace chat row → journal scan (the crash path
    /// where the debounced row write never landed — SessionStarted/Done events
    /// are journaled per event, flushed immediately). Cwd-gated throughout:
    /// harness session stores are keyed by cwd, so a session created elsewhere
    /// never rides `--resume`. An empty stored id is the explicit tombstone —
    /// no resume, no falling through to staler sources.
    fn resume_for(&self, chat_id: &str, cwd: &str) -> Option<String> {
        let cwd_ok = |session_cwd: &str| session_cwd.is_empty() || session_cwd == cwd;
        if let Some(known) = lock(&self.harness_sessions).get(chat_id).cloned() {
            return (!known.session_id.is_empty() && cwd_ok(&known.cwd))
                .then_some(known.session_id);
        }
        if let Some(ws) = self.workspace()
            && let Some((session_id, session_cwd)) = ws.chat_harness_session(chat_id)
        {
            return (!session_id.is_empty() && cwd_ok(session_cwd.as_deref().unwrap_or("")))
                .then_some(session_id);
        }
        let (session_id, session_cwd) = self.journal_harness_session(chat_id)?;
        // Cache the journal hit (memory + row) so later dispatches skip the scan.
        self.remember_harness_session(chat_id, &session_id, &session_cwd);
        cwd_ok(&session_cwd).then_some(session_id)
    }

    /// The last harness session id named anywhere in the chat's journal, with
    /// the cwd of the `SessionStarted` that governs it. `Done.session_id`
    /// inherits the cwd of the most recent `SessionStarted` (same run).
    fn journal_harness_session(&self, chat_id: &str) -> Option<(String, String)> {
        let events = match self.journal.replay(chat_id, 0) {
            Ok(events) => events,
            Err(err) => {
                tracing::warn!(chat = %chat_id, error = %err, "journal scan for harness session failed");
                return None;
            }
        };
        let mut current_cwd = String::new();
        let mut found: Option<(String, String)> = None;
        for (_, event) in events {
            match event {
                AgentEvent::SessionStarted {
                    session_id, cwd, ..
                } => {
                    current_cwd = cwd;
                    if !session_id.is_empty() {
                        found = Some((session_id, current_cwd.clone()));
                    }
                }
                AgentEvent::Done {
                    session_id: Some(session_id),
                    ..
                } if !session_id.is_empty() => {
                    found = Some((session_id, current_cwd.clone()));
                }
                _ => {}
            }
        }
        found
    }

    fn remove_run(&self, chat_id: &str, run_id: &str) {
        let mut runs = lock(&self.runs);
        if runs.get(chat_id).is_some_and(|h| h.run_id == run_id) {
            runs.remove(chat_id);
        }
    }
}

// ── run task ────────────────────────────────────────────────────────────────

/// A turn is in flight: streaming, or parked on a question it is still owed an
/// answer to. Notably NOT a persistent session that has parked between turns —
/// that holds a warm child with nothing outstanding.
fn is_active(session: &Session) -> bool {
    matches!(
        session.status,
        SessionStatus::Working | SessionStatus::AwaitingInput
    )
}

// ── subagent docs ───────────────────────────────────────────────────────────

/// The per-subagent doc id: `{chatId}--sub--{suffix}`. Constrained by the
/// edge's `ID_RE` (`^[A-Za-z0-9_-]{1,128}$` — the same id names the ChatRoom
/// `chat2/{id}/ws` and the frozen blob `blob/{chatId}/{id}`): a clean, short
/// tool-use id rides verbatim; anything unclean or over budget hashes.
pub(crate) fn subagent_doc_id(chat_id: &str, tool_use_id: &str) -> String {
    let clean = tool_use_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let budget = 128usize.saturating_sub(chat_id.len() + "--sub--".len());
    if clean && !tool_use_id.is_empty() && tool_use_id.len() <= budget {
        return format!("{chat_id}--sub--{tool_use_id}");
    }
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(tool_use_id.as_bytes());
    let mut hex = String::with_capacity(16);
    for b in &digest[..8] {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }
    format!("{chat_id}--sub--{hex}")
}

/// Test-only instrumentation: how often `drive_run`'s coalesced commit branch
/// fires. The sink-dirty regression test proves the branch ticks once per
/// commit window instead of spinning on a deadline left in the past.
/// Thread-local so parallel tests' runs can't bleed into the count: the
/// test's current-thread runtime runs its spawned `drive_run` on its own
/// thread.
#[cfg(test)]
thread_local! {
    static FLUSH_TICKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A live subagent transcript sink: its own doc (opened by id — the room
/// `chat2/{docId}/ws` dials automatically, so viewers sync it like a chat),
/// one streaming assistant entry folded from the tagged events. The held
/// doc Arc pins the doc warm for the LRU while the subagent runs.
struct SubagentSink {
    doc_id: String,
    doc: crate::doc_host::DocWriter,
    entry_id: String,
    started_at: i64,
    entry_index: Option<usize>,
    written: Vec<MessagePart>,
    folded: Vec<MessagePart>,
    dirty: bool,
}

impl SubagentSink {
    fn flush(&mut self, device_id: &str) {
        if !self.dirty {
            return;
        }
        // An empty fold still counts as flushed: leaving dirty set would hold
        // the run loop's coalesced-commit guard open on a deadline already in
        // the past, spinning it until the subagent emits real content.
        self.dirty = false;
        if self.folded.is_empty() {
            return;
        }
        let rendered = render_parts(&self.folded);
        let result = match self.entry_index {
            Some(ix) => {
                let mut w = SegmentWriter::resume(&self.doc, ix, std::mem::take(&mut self.written));
                let r = w.sync(&rendered);
                let (ix, written) = w.into_state();
                self.entry_index = Some(ix);
                self.written = written;
                r
            }
            None => {
                match SegmentWriter::begin(&self.doc, &self.entry_id, device_id, self.started_at) {
                    Ok(mut w) => {
                        let r = w.sync(&rendered);
                        let (ix, written) = w.into_state();
                        self.entry_index = Some(ix);
                        self.written = written;
                        r
                    }
                    Err(e) => Err(e),
                }
            }
        };
        if let Err(err) = result {
            // Fail soft: a broken subagent doc degrades to chip-only, never
            // errors the chat.
            tracing::warn!(doc = %self.doc_id, error = %err, "subagent sink flush failed");
        }
    }

    /// A parent→subagent steer ([`AgentEvent::UserMessage`], tagged): close
    /// the open assistant segment (it reads Complete above the steer), write
    /// the steer as its own USER entry, and reset so the next tagged delta
    /// opens a fresh assistant entry below it — the subagent transcript then
    /// reads like any steered chat.
    fn push_user(&mut self, device_id: &str, text: &str) {
        let rendered = render_parts(&self.folded);
        let closed = match self.entry_index.take() {
            Some(ix) => SegmentWriter::resume(&self.doc, ix, std::mem::take(&mut self.written))
                .finish(&rendered, MessageStatus::Complete),
            None if !self.folded.is_empty() => {
                match SegmentWriter::begin(&self.doc, &self.entry_id, device_id, self.started_at) {
                    Ok(w) => w.finish(&rendered, MessageStatus::Complete),
                    Err(e) => Err(e),
                }
            }
            None => Ok(()),
        };
        if let Err(err) = closed {
            tracing::warn!(doc = %self.doc_id, error = %err, "subagent segment close failed");
        }
        let entry = SessionMessageEntry {
            id: new_id(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.to_owned(),
            }],
            created_at: now_ms(),
            device_id: device_id.to_owned(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        };
        if let Err(err) = self.doc.push_message(&entry) {
            tracing::warn!(doc = %self.doc_id, error = %err, "subagent steer write failed");
        }
        self.entry_id = new_id();
        self.started_at = now_ms();
        self.written = Vec::new();
        self.folded = Vec::new();
        self.dirty = false;
    }

    /// Final flush + entry finalize; returns the transcript JSON for the
    /// frozen blob (entries as the client renders them).
    fn finish(mut self, device_id: &str, status: MessageStatus) -> Option<String> {
        let rendered = render_parts(&self.folded);
        let finished = match self.entry_index {
            Some(ix) => SegmentWriter::resume(&self.doc, ix, std::mem::take(&mut self.written))
                .finish(&rendered, status),
            None if !self.folded.is_empty() => {
                match SegmentWriter::begin(&self.doc, &self.entry_id, device_id, self.started_at) {
                    Ok(w) => w.finish(&rendered, status),
                    Err(e) => Err(e),
                }
            }
            None => Ok(()),
        };
        if let Err(err) = finished {
            tracing::warn!(doc = %self.doc_id, error = %err, "subagent sink finish failed");
        }
        let entries = zeron_doc::join_continuation_entries(self.doc.read_entries().ok()?);
        serde_json::to_string(&entries).ok()
    }
}

/// The parent-chip refresh a tagged event implies, for the in-place (parked)
/// path — LIFECYCLE ONLY, mirroring the fold (live tails were rejected:
/// per-delta chip rewrites grew the parent doc for the whole run).
fn subagent_chip_update(event: &AgentEvent) -> Option<&'static str> {
    match event {
        AgentEvent::Done { status, .. } => Some(match status {
            DoneStatus::Errored => "failed",
            _ => "done",
        }),
        _ => Some("running"),
    }
}

/// Apply the render-parts privacy policy: strip heavy/sensitive tool inputs before doc
/// entry. Full inputs live only in the local run journal.
fn render_parts(parts: &[MessagePart]) -> Vec<MessagePart> {
    parts
        .iter()
        .map(|part| match part {
            MessagePart::Tool {
                id,
                call,
                is_error,
                resolved,
                output,
                diff,
                output_ref,
                output_bytes,
                diff_ref,
                diff_stats,
                subagent_ref,
                subagent_status,
                subagent_tail,
            } => MessagePart::Tool {
                id: id.clone(),
                call: sanitize_tool_call(call),
                is_error: *is_error,
                resolved: *resolved,
                // Output summaries, diff stats, and sidecar refs are
                // deliberately kept: unlike raw tool inputs they are the
                // transcript's record of what happened, and the strip already
                // bounded them (docs/chat2-sync.md A1).
                output: output.clone(),
                diff: diff.clone(),
                output_ref: output_ref.clone(),
                output_bytes: *output_bytes,
                diff_ref: diff_ref.clone(),
                diff_stats: diff_stats.clone(),
                subagent_ref: subagent_ref.clone(),
                subagent_status: *subagent_status,
                subagent_tail: subagent_tail.clone(),
            },
            other => other.clone(),
        })
        .collect()
}

/// The persisted assistant text of a folded segment (workspace preview source).
fn folded_text(parts: &[MessagePart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sync_segment<'a>(
    doc: &'a SessionDoc,
    writer: &mut Option<SegmentWriter<'a>>,
    entry_id: &str,
    device_id: &str,
    started_at: i64,
    folded: &[MessagePart],
) -> Result<(), DocError> {
    if folded.is_empty() {
        return Ok(());
    }
    let rendered = render_parts(folded);
    if writer.is_none() {
        *writer = Some(SegmentWriter::begin(doc, entry_id, device_id, started_at)?);
    }
    if let Some(w) = writer.as_mut() {
        w.sync(&rendered)?;
    }
    Ok(())
}

fn finish_segment<'a>(
    doc: &'a SessionDoc,
    writer: Option<SegmentWriter<'a>>,
    entry_id: &str,
    device_id: &str,
    started_at: i64,
    folded: &[MessagePart],
    status: MessageStatus,
) -> Result<(), DocError> {
    let rendered = render_parts(folded);
    match writer {
        Some(w) => w.finish(&rendered, status),
        None if !folded.is_empty() => {
            SegmentWriter::begin(doc, entry_id, device_id, started_at)?.finish(&rendered, status)
        }
        None => Ok(()),
    }
}

/// Resume bookkeeping for one run task: which user entry the run answers (so
/// the startup-crash retry re-dispatches idempotently against the same doc
/// entry), whether `dispatch` injected the resume id itself (only
/// engine-injected resumes retry — a caller-specified resume fails loudly),
/// and whether this run already IS the retry (one attempt only).
struct RunResumeState {
    user_message_id: String,
    resume_injected: bool,
    startup_retry: bool,
}

fn cursor_unstarted_history(
    doc: &SessionDoc,
    current_id: &str,
    prompt: &str,
    has_session: bool,
) -> Result<String, DocError> {
    // Convert each message before JSON encoding. Rewriting canonical chips in
    // the encoded envelope can introduce unescaped quotes or newlines and can
    // cause Cursor's current message to be converted twice.
    let prompt = zeron_proto::invocation::harness_prompt(prompt, HarnessId::Cursor);
    let entries = doc.read_entries()?;
    let preceding: Vec<_> = entries
        .iter()
        .take_while(|entry| entry.id != current_id)
        .collect();
    // With an existing session, only bridge the tail whose assistant never
    // produced content. A process can die before writing its SDK-side receipt,
    // even though an older session ID still exists.
    let after = if has_session {
        preceding
            .iter()
            .rposition(|entry| {
                entry.role == MessageRole::Assistant
                    && entry.parts.iter().any(|part| match part {
                        MessagePart::Text { text, .. } | MessagePart::Reasoning { text, .. } => {
                            !text.is_empty()
                        }
                        MessagePart::Tool { .. } | MessagePart::Input { .. } => true,
                        _ => false,
                    })
            })
            .map_or(0, |i| i + 1)
    } else {
        0
    };
    let previous: Vec<String> = preceding
        .iter()
        .skip(after)
        .filter(|entry| entry.role == MessageRole::User)
        .map(|entry| {
            entry
                .parts
                .iter()
                .filter_map(|part| match part {
                    MessagePart::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.is_empty())
        .map(|text| zeron_proto::invocation::harness_prompt(&text, HarnessId::Cursor))
        .collect();
    if previous.is_empty() {
        return Ok(prompt);
    }
    Ok(format!(
        "The preceding user messages may not have reached a Cursor checkpoint before startup stopped. Retain this JSON as conversation history; do not rerun prior tools or side effects. Respond to the current message.\n{}",
        serde_json::json!({"previousUserMessages": previous, "currentUserMessage": prompt})
    ))
}

/// The remote-run driver for cloud-seat chats. Streams the seat's harness
/// output into one assistant text segment; `interrupt` (the engine's normal
/// Stop path) kills the ssh link and best-effort pkills the remote command.
/// Detached seats instead write a run log on the seat (`~/.cosmos/runs/`) —
/// the run survives this app closing and the driver tails the log while
/// connected, backfilling on reopen.
#[allow(clippy::too_many_arguments)]
/// What one seat turn carries beyond the command: the user's words, a recap
/// of earlier turns when the harness can't resume natively, and the session
/// tag to stamp on the ledger once the turn produced output.
struct RemoteTurn {
    prompt: String,
    recap: String,
    session_tag: Option<String>,
    harness: HarnessId,
    model: Option<String>,
    /// Best-effort install of the harness CLI when the seat lacks it.
    install: String,
}

fn boat_provider(harness: HarnessId) -> Option<&'static str> {
    match harness {
        HarnessId::ClaudeCode => Some("claude"),
        _ => None,
    }
}

fn harness_display_name(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "Claude Code",
        HarnessId::Codex => "Codex",
        HarnessId::Cursor => "Cursor",
        HarnessId::Devin => "Devin",
        HarnessId::Grok => "Grok",
        HarnessId::Hermes => "Hermes",
        HarnessId::Pi => "Pi",
        HarnessId::Opencode => "OpenCode",
        HarnessId::Antigravity => "Antigravity",
        HarnessId::Amp => "Amp",
        HarnessId::KimiCode => "Kimi Code",
        HarnessId::Mock => "Mock",
    }
}

/// A compact recap of the thread so far — the continuity fallback for
/// harnesses without a resumable session, and for a chat's first turn on a
/// seat it moved to. Bounded: the last few text turns, clipped.
fn prior_turns_recap(entries: &[SessionMessageEntry]) -> String {
    const MAX_TURNS: usize = 8;
    const MAX_PER_TURN: usize = 700;
    const MAX_TOTAL: usize = 4_000;
    let mut turns: Vec<String> = Vec::new();
    for entry in entries.iter().rev() {
        let who = match entry.role {
            zeron_doc::MessageRole::User => "User",
            zeron_doc::MessageRole::Assistant => "Assistant",
            _ => continue,
        };
        let text: String = entry
            .parts
            .iter()
            .filter_map(|part| match part {
                MessagePart::Text { text, .. } => Some(text.trim()),
                _ => None,
            })
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if text.is_empty() {
            continue;
        }
        let clipped: String = if text.chars().count() > MAX_PER_TURN {
            let head: String = text.chars().take(MAX_PER_TURN).collect();
            format!("{head}…")
        } else {
            text
        };
        turns.push(format!("{who}: {clipped}"));
        if turns.len() >= MAX_TURNS || turns.iter().map(String::len).sum::<usize>() > MAX_TOTAL {
            break;
        }
    }
    if turns.is_empty() {
        return String::new();
    }
    turns.reverse();
    format!(
        "<previous-turns>\nEarlier in this thread (most recent last):\n{}\n</previous-turns>\n\n",
        turns.join("\n\n")
    )
}

/// After a seat turn ends: checkpoint a repo-less workspace with a local git
/// commit, then pull a snapshot of the chat's workspace down to this
/// computer — so the work survives the seat (a trial's hour ending, a box
/// destroyed) and restores onto whichever seat the chat runs on next.
#[allow(clippy::too_many_arguments)]
async fn persist_remote_turn(
    inner: &Arc<Inner>,
    store: &crate::cloud::CloudStore,
    http: &reqwest::Client,
    seat: &crate::cloud::SeatConfig,
    chat_id: &str,
    run_id: &str,
    turn: &RemoteTurn,
    doc: &crate::doc_host::DocWriter,
) {
    let run = store.runs().into_iter().find(|r| r.id == run_id);
    let produced = run.as_ref().is_some_and(|r| {
        r.mirrored_bytes > 0 || matches!(r.status, zeron_proto::CloudRunStatus::Done)
    });
    if produced && let Some(tag) = turn.session_tag.clone() {
        store.update_run(run_id, |run| run.harness_session = Some(tag));
    }
    // Repo chats keep their record in git (commits, pushed branches, PRs —
    // plus the trial-expiry sweep and "Back up now"); per-turn snapshots are
    // for repo-less workspaces, whose only other copy is the seat itself.
    if seat.expired() || seat.repo.is_some() {
        return;
    }
    let workspace = seat.workspace();
    {
        match crate::cloud::seat_exec(
            http,
            store,
            seat,
            &crate::cloud::checkpoint_command(&workspace, &turn.prompt),
            std::time::Duration::from_secs(60),
        )
        .await
        {
            Ok(out) if out.code == 0 && !out.stdout.trim().is_empty() => {
                tracing::debug!(chat = %chat_id, commit = %out.stdout.trim(), "seat checkpoint");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(chat = %chat_id, error = %e, "seat checkpoint failed"),
        }
    }
    match crate::cloud::backup_chat_workspace(http, store, seat, chat_id, &workspace).await {
        Ok(backup) => {
            tracing::info!(chat = %chat_id, bytes = backup.bytes, "chat workspace backed up");
        }
        Err(e) => {
            tracing::warn!(chat = %chat_id, error = %e, "chat workspace backup failed");
            // Say so in the transcript: a repo-less workspace has no other
            // home, so a failed save is worth one honest line.
            if produced {
                write_transcript_note(
                    doc,
                    &inner.device_id,
                    &format!("Couldn't back up this workspace to your computer this turn: {e}"),
                );
            }
        }
    }
}

/// Before a seat turn: the chat's workspace exists (restored from this
/// computer's backup when the seat came up empty) and the seat's desktop +
/// `cosmos-computer` are staged. Returns the prompt the harness receives.
#[allow(clippy::too_many_arguments)]
async fn prepare_remote_turn(
    inner: &Arc<Inner>,
    store: &crate::cloud::CloudStore,
    http: &reqwest::Client,
    seat: &crate::cloud::SeatConfig,
    chat_id: &str,
    turn: &RemoteTurn,
    doc: &crate::doc_host::DocWriter,
) -> String {
    let workspace = seat.workspace();
    // One round-trip: stage the desktop tooling (detached bring-up), then
    // create/inspect the chat's workspace.
    let prep = format!(
        "{{ {} ; }} ; {}",
        crate::seat_desktop::bootstrap_command(),
        crate::cloud::chat_workspace_prepare(&workspace, seat.repo.is_none())
    );
    let (desktop_ok, empty) =
        match crate::cloud::seat_exec(http, store, seat, &prep, std::time::Duration::from_secs(45))
            .await
        {
            Ok(out) => (
                out.stdout.contains("__cosmos_desktop_ok"),
                out.stdout.contains("__cosmos_ws_empty"),
            ),
            Err(e) => {
                write_transcript_note(
                    doc,
                    &inner.device_id,
                    &format!("Couldn't prepare the workspace on the seat: {e}"),
                );
                (false, false)
            }
        };
    if let Some(note) =
        crate::cloud::restore_chat_workspace_if_empty(http, store, seat, chat_id, &workspace, empty)
            .await
    {
        write_transcript_note(doc, &inner.device_id, &note);
    }
    // SSH/Railway runs clone inside the harness command; a Boat box's
    // integrated agent has no such prelude, so its repo checkout (on the
    // picked branch) lands here, before the prompt.
    if matches!(seat.kind, Some(zeron_proto::CloudSeatKind::Box))
        && let Some(repo) = seat.repo.as_deref().filter(|r| !r.trim().is_empty())
    {
        let token = store
            .github_pat(&seat.id)
            .map(|pat| format!("GITHUB_TOKEN={} ", crate::cloud::shell_word(&pat)))
            .unwrap_or_default();
        let clone = crate::cloud::command_in_workspace(
            &workspace,
            &format!(
                "{token}sh -c {}",
                crate::cloud::shell_word(&crate::cloud::repo_checkout_prelude(
                    repo,
                    seat.branch.as_deref()
                ))
            ),
        );
        match crate::cloud::seat_exec(
            http,
            store,
            seat,
            &clone,
            std::time::Duration::from_secs(300),
        )
        .await
        {
            Ok(out) if out.code == 0 => {}
            Ok(out) => write_transcript_note(
                doc,
                &inner.device_id,
                &format!(
                    "Couldn't check out {repo} on the box: {}",
                    out.stderr.lines().last().unwrap_or("git clone failed")
                ),
            ),
            Err(e) => write_transcript_note(
                doc,
                &inner.device_id,
                &format!("Couldn't check out {repo} on the box: {e}"),
            ),
        }
    }
    if !desktop_ok {
        tracing::warn!(seat = %seat.id, "seat desktop bootstrap failed");
    }
    let brief = if desktop_ok {
        crate::seat_desktop::agent_brief(&workspace, seat.repo.as_deref())
    } else {
        String::new()
    };
    format!("{brief}{}{}", turn.recap, turn.prompt)
}

#[allow(clippy::too_many_arguments)]
async fn drive_remote_run(
    inner: Arc<Inner>,
    store: crate::cloud::CloudStore,
    chat_id: String,
    run_id: String,
    seat: crate::cloud::SeatConfig,
    command: String,
    turn: RemoteTurn,
    doc: crate::doc_host::DocWriter,
    mut cancel_rx: watch::Receiver<bool>,
    interrupt_token: CancellationToken,
) {
    use tokio::io::AsyncReadExt;

    let finish = |inner: &Inner,
                  store: &crate::cloud::CloudStore,
                  status: SessionStatus,
                  run_status: zeron_proto::CloudRunStatus,
                  error: Option<String>| {
        inner.set_status(&chat_id, status, false);
        store.update_run(&run_id, |run| {
            run.status = run_status;
            run.error = error;
            run.finished_at = Some(Utc::now());
        });
        lock(&inner.runs).remove(&chat_id);
    };

    let http = reqwest::Client::new();

    // Boat seats ride the provider's integrated-agent surface (`/prompt` +
    // `/events` + `/interrupt`): the coding agent runs ON the box even with
    // the desktop closed — the 24/7 rule satisfied honestly.
    if matches!(seat.kind, Some(zeron_proto::CloudSeatKind::Box)) {
        drive_boat_run(
            inner,
            store,
            http,
            chat_id,
            run_id,
            seat,
            turn,
            doc,
            cancel_rx,
            interrupt_token,
        )
        .await;
        return;
    }

    let prompt = prepare_remote_turn(&inner, &store, &http, &seat, &chat_id, &turn, &doc).await;

    let env = store
        .cloud_env()
        .into_iter()
        .map(|(k, v)| format!("{k}={}", crate::cloud::shell_word(&v)))
        .collect::<Vec<_>>()
        .join(" ");
    // `{prompt}` in the configured command names where the prompt goes;
    // otherwise it appends as the trailing argument. Both shell-quoted.
    let prompt_word = crate::cloud::shell_word(&prompt);
    let user_cmd = if command.contains("{prompt}") {
        command.replace("{prompt}", &prompt_word)
    } else {
        format!("{command} {prompt_word}")
    };
    let user_cmd = match seat.repo.as_deref().filter(|r| !r.trim().is_empty()) {
        Some(repo) => format!(
            "sh -c {}",
            crate::cloud::shell_word(&format!(
                "{} && exec env {user_cmd}",
                crate::cloud::repo_checkout_prelude(repo, seat.branch.as_deref())
            ))
        ),
        None => user_cmd,
    };
    let remote_command = if env.is_empty() {
        user_cmd
    } else {
        format!("env {env} {user_cmd}")
    };
    // The seat's display + `cosmos-computer` ride the harness environment;
    // a missing harness CLI installs first (best-effort, never fatal).
    let remote_command = if turn.install.is_empty() {
        format!("{} {remote_command}", crate::seat_desktop::env_prefix())
    } else {
        // When the install couldn't land the CLI, say exactly that instead
        // of the shell's bare "command not found".
        let bin = crate::cloud::command_bin(&command);
        format!(
            "{} export PATH=\"$HOME/.local/bin:$HOME/.cursor/bin:$HOME/.npm-global/bin:$PATH\" && {{ {} ; }} ; command -v {} >/dev/null 2>&1 || {{ echo {} >&2; exit 127; }} ; {remote_command}",
            crate::seat_desktop::env_prefix(),
            turn.install,
            crate::cloud::shell_word(&bin),
            crate::cloud::shell_word(&format!(
                "`{bin}` isn't installed on this seat and the automatic install didn't work — install it on the seat, or pick another agent in the model picker."
            )),
        )
    };
    let full = crate::cloud::command_in_workspace(&seat.workspace(), &remote_command);

    // The user's existing CLI logins ride to the seat — `~/.codex/auth.json`,
    // opencode/grok/pi/hermes stores — staged per run so the harness auths
    // on the box and keeps working after the laptop closes. A staging miss
    // only warns; BYOK env keys may still cover the harness.
    if seat.detached {
        // Detached: nohup into a per-run log on the seat, then tail it. The
        // process belongs to the seat's init tree, so closing this app (or
        // dropping the ssh link) never kills the run — the ledger row lets a
        // later session reattach.
        let log_path = format!("$HOME/.cosmos/runs/{run_id}.log");
        let detached_command = format!(
            "{full}; code=$?; printf '%s\\n' \"$code\" > \"$HOME/.cosmos/runs/{run_id}.exit\"; exit \"$code\""
        );
        let launch = format!(
            "mkdir -p \"$HOME/.cosmos/runs\" && {{ nohup sh -c {} > \"$HOME/.cosmos/runs/{run_id}.log\" 2>&1 & pid=$!; printf '%s\\n' \"$pid\" > \"$HOME/.cosmos/runs/{run_id}.pid\"; printf '%s\\n' \"$pid\"; }}",
            crate::cloud::shell_word(&detached_command),
        );
        match crate::cloud::seat_exec(
            &http,
            &store,
            &seat,
            &launch,
            std::time::Duration::from_secs(30),
        )
        .await
        {
            Ok(out) if out.code == 0 => {
                let pid = out.stdout.trim().to_string();
                store.update_run(&run_id, |run| {
                    run.log_path = Some(log_path.clone());
                    run.remote_ref = Some(pid);
                });
                tail_remote_log(
                    &inner,
                    &http,
                    &store,
                    &chat_id,
                    &run_id,
                    &seat,
                    &log_path,
                    &doc,
                    &mut cancel_rx,
                    None,
                    &command,
                )
                .await;
                persist_remote_turn(&inner, &store, &http, &seat, &chat_id, &run_id, &turn, &doc)
                    .await;
            }
            Ok(out) => {
                let message = out
                    .stderr
                    .lines()
                    .next()
                    .unwrap_or("detached launch failed")
                    .to_string();
                write_remote_error(&doc, &inner.device_id, &message);
                finish(
                    &inner,
                    &store,
                    SessionStatus::Errored,
                    zeron_proto::CloudRunStatus::Error,
                    Some(message),
                );
            }
            Err(e) => {
                write_remote_error(&doc, &inner.device_id, &e);
                finish(
                    &inner,
                    &store,
                    SessionStatus::Errored,
                    zeron_proto::CloudRunStatus::Error,
                    Some(e),
                );
            }
        }
        return;
    }

    let stream = crate::cloud::seat_exec_stream(&seat, &full).await;
    let stream = match stream {
        Ok(s) => s,
        Err(e) => {
            write_remote_error(&doc, &inner.device_id, &e);
            finish(
                &inner,
                &store,
                SessionStatus::Errored,
                zeron_proto::CloudRunStatus::Error,
                Some(e),
            );
            return;
        }
    };

    let entry_id = new_id();
    let mut segment: Option<zeron_doc::SegmentWriter> = None;
    let mut text = String::new();
    let mut render = RemoteRenderer::for_command(&command);
    let mut ansi = AnsiStripper::default();
    let mut buf = [0u8; 8192];
    let mut err_buf = [0u8; 4096];
    let mut stderr_tail = Vec::new();
    let mut stdout_open = true;
    let mut stderr_open = true;
    let mut interrupted = false;
    let mut expired = false;
    let mut child = stream.child;
    let mut stdout = stream.stdout;
    let mut stderr = stream.stderr;
    let deadline = seat.expires_at.map(|deadline| {
        let seconds = (deadline - Utc::now()).num_seconds().max(0) as u64;
        tokio::time::Instant::now() + std::time::Duration::from_secs(seconds)
    });
    loop {
        tokio::select! {
            _ = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                expired = true;
                let _ = child.kill().await;
                break;
            }
            _ = cancel_rx.changed() => {
                interrupted = true;
                let _ = child.kill().await;
                // Best-effort remote kill so a hung harness can't linger —
                // seat_exec routes to docker-exec (SSH) or direct ssh
                // (Railway); the run-id marker matches the detached cmdline.
                let marker = format!("pkill -f {}", crate::cloud::shell_word(&run_id));
                let _ = crate::cloud::seat_exec(
                    &http,
                    &store,
                    &seat,
                    &marker,
                    std::time::Duration::from_secs(10),
                )
                .await;
                break;
            }
            _ = interrupt_token.cancelled() => {
                interrupted = true;
                let _ = child.kill().await;
                break;
            }
            read = stdout.read(&mut buf), if stdout_open => {
                match read {
                    Ok(0) => stdout_open = false,
                    Ok(n) => {
                        let clean = ansi.push(&buf[..n]);
                        match render.as_mut() {
                            Some(render) => text.push_str(&render.push(&clean)),
                            None => text.push_str(&String::from_utf8_lossy(&clean)),
                        }
                        if segment.is_none() && !text.trim().is_empty() {
                            segment = zeron_doc::SegmentWriter::begin(
                                &doc,
                                &entry_id,
                                &inner.device_id,
                                now_ms(),
                            )
                            .ok();
                        }
                        if let Some(segment) = segment.as_mut() {
                            let _ = segment.sync(&[MessagePart::Text {
                                id: "t0".into(),
                                text: preserve_line_breaks(&strip_orphaned_ansi(&text)),
                            }]);
                        }
                    }
                    Err(_) => stdout_open = false,
                }
            }
            read = stderr.read(&mut err_buf), if stderr_open => {
                match read {
                    Ok(0) => stderr_open = false,
                    Ok(n) => {
                        stderr_tail.extend_from_slice(&err_buf[..n]);
                        if stderr_tail.len() > 65_536 {
                            let excess = stderr_tail.len() - 65_536;
                            stderr_tail.drain(..excess);
                        }
                    }
                    Err(_) => stderr_open = false,
                }
            }
        }
        if !stdout_open && !stderr_open {
            break;
        }
    }
    if let Some(render) = render.as_mut() {
        text.push_str(&render.finish());
    }
    let status = child
        .wait()
        .await
        .map(|s| s.code().unwrap_or(-1))
        .unwrap_or(-1);
    if expired {
        terminal_cloud_run(&inner, &store, &chat_id, &run_id, &mut segment, &text);
    } else if interrupted {
        if let Some(segment) = segment {
            let _ = segment.finish(
                &[MessagePart::Text {
                    id: "t0".into(),
                    text: preserve_line_breaks(&strip_orphaned_ansi(&text)),
                }],
                zeron_doc::MessageStatus::Aborted,
            );
        }
        finish(
            &inner,
            &store,
            SessionStatus::Idle,
            zeron_proto::CloudRunStatus::Stopped,
            None,
        );
    } else if !remote_agent_failed(status, &text) {
        store.update_run(&run_id, |run| run.mirrored_bytes = text.len() as u64);
        if let Some(segment) = segment {
            let _ = segment.finish(
                &[MessagePart::Text {
                    id: "t0".into(),
                    text: preserve_line_breaks(&strip_orphaned_ansi(&text)),
                }],
                zeron_doc::MessageStatus::Complete,
            );
        }
        finish(
            &inner,
            &store,
            SessionStatus::Idle,
            zeron_proto::CloudRunStatus::Done,
            None,
        );
    } else {
        let stderr_text = String::from_utf8_lossy(&stderr_tail);
        let message = remote_agent_failure_message(status, &text, &stderr_text);
        if let Some(segment) = segment {
            let _ = segment.finish(
                &[MessagePart::Text {
                    id: "t0".into(),
                    text: preserve_line_breaks(&text),
                }],
                zeron_doc::MessageStatus::Aborted,
            );
        }
        write_remote_error(&doc, &inner.device_id, &message);
        finish(
            &inner,
            &store,
            SessionStatus::Errored,
            zeron_proto::CloudRunStatus::Error,
            Some(message),
        );
    }
    if !expired {
        persist_remote_turn(&inner, &store, &http, &seat, &chat_id, &run_id, &turn, &doc).await;
    }
}

/// Boat seats run the provider's integrated agent: provision (or wake) the
/// sandbox, POST `/prompt`, then mirror `/events` into the transcript. The
/// agent process lives on the box — closing this app doesn't kill the run,
/// so `keeps_running` is honest, not the desktop-lifetime warning.
#[allow(clippy::too_many_arguments)]
async fn drive_boat_run(
    inner: Arc<Inner>,
    store: crate::cloud::CloudStore,
    http: reqwest::Client,
    chat_id: String,
    run_id: String,
    seat: crate::cloud::SeatConfig,
    turn: RemoteTurn,
    doc: crate::doc_host::DocWriter,
    cancel_rx: watch::Receiver<bool>,
    interrupt_token: CancellationToken,
) {
    drive_boat_turn(
        inner.clone(),
        store.clone(),
        http.clone(),
        chat_id.clone(),
        run_id.clone(),
        seat.clone(),
        &turn,
        &doc,
        cancel_rx,
        interrupt_token,
    )
    .await;
    // The box id may have been minted this turn — re-read it, keeping the
    // chat's workspace/repo scope.
    let Some(live) = store
        .seat(&seat.id)
        .map(|s| s.scoped(seat.workspace_path.as_deref(), seat.repo.as_deref()))
        .filter(|s| s.box_id.is_some())
    else {
        return;
    };
    persist_remote_turn(&inner, &store, &http, &live, &chat_id, &run_id, &turn, &doc).await;
}

#[allow(clippy::too_many_arguments)]
async fn drive_boat_turn(
    inner: Arc<Inner>,
    store: crate::cloud::CloudStore,
    http: reqwest::Client,
    chat_id: String,
    run_id: String,
    seat: crate::cloud::SeatConfig,
    turn: &RemoteTurn,
    doc: &crate::doc_host::DocWriter,
    cancel_rx: watch::Receiver<bool>,
    interrupt_token: CancellationToken,
) {
    let finish =
        |status: SessionStatus, run_status: zeron_proto::CloudRunStatus, error: Option<String>| {
            inner.set_status(&chat_id, status, false);
            store.update_run(&run_id, |run| {
                run.status = run_status;
                run.error = error;
                run.finished_at = Some(Utc::now());
            });
            lock(&inner.runs).remove(&chat_id);
        };
    let fail = |e: String| {
        write_remote_error(doc, &inner.device_id, &e);
        finish(
            SessionStatus::Errored,
            zeron_proto::CloudRunStatus::Error,
            Some(e),
        );
    };

    let Some(provider) = boat_provider(turn.harness) else {
        fail(format!(
            "Boat's built-in agent runs Claude Code only — install a Cosmos runner on this box to use {}",
            harness_display_name(turn.harness)
        ));
        return;
    };

    let Some(token) = store.box_token(&seat.id) else {
        fail("this Boat seat has no token — set one in Settings → Cloud computer".into());
        return;
    };
    let api = crate::boat::boat(&http, &token, seat.box_endpoint.as_deref());

    // Ensure a usable sandbox: reuse the bound box (waking it if Sleeping),
    // else provision one and bind its `bx_…` id onto the seat. A dead box
    // falls through to a fresh create — the seat keeps working.
    let box_id = match seat.box_id.clone() {
        Some(id) => match api.sandbox(&id).await {
            Ok(s) if s.usable() => Some(id),
            Ok(s) if s.sleeping() => match api.resume(&id).await {
                Ok(_) => Some(id),
                Err(e) => {
                    fail(format!("couldn't wake the Boat box: {e}"));
                    return;
                }
            },
            Ok(_) | Err(_) => None,
        },
        None => None,
    };
    let box_id = match box_id {
        Some(id) => id,
        None => {
            let name = format!("cosmos-{}", &chat_id[..chat_id.len().min(8)]);
            match api.create(&name, &run_id).await {
                Ok(s) => match api.wait_ready(&s.id).await {
                    Ok(s) => {
                        let id = s.id.clone();
                        store.update_seat(&seat.id, |seat| {
                            seat.box_id = Some(id.clone());
                            seat.ssh_endpoint = s.ssh_endpoint.clone();
                            seat.preview_url = s.desktop_url.clone().or(seat.preview_url.clone());
                        });
                        id
                    }
                    Err(e) => {
                        let mut destroyed = false;
                        let mut destroy_error = String::new();
                        for attempt in 0..3 {
                            match api.destroy(&s.id).await {
                                Ok(()) => {
                                    destroyed = true;
                                    break;
                                }
                                Err(error) => {
                                    destroy_error = error;
                                    if attempt < 2 {
                                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                    }
                                }
                            }
                        }
                        if destroyed {
                            fail(format!("created but not ready; destroyed ({e})"));
                        } else {
                            fail(format!(
                                "created but not ready; destroy failed: {destroy_error} (readiness: {e})"
                            ));
                        }
                        return;
                    }
                },
                Err(e) => {
                    fail(format!("couldn't provision a Boat box: {e}"));
                    return;
                }
            }
        }
    };
    store.update_run(&run_id, |run| {
        run.remote_ref = Some(format!("boat:{box_id}"));
    });

    // Migration path: a never-restored Boat box ingests the newest Railway
    // trial snapshot, so work survives the trial's 60-minute death. The
    // upload rides `/files` as base64 (the only write surface), then exec
    // extracts it into the seat workspace.
    if seat.restored_at.is_none()
        && let Some(backup) = store.latest_railway_backup()
    {
        let note = |text: String| {
            let _ = doc.push_message(&SessionMessageEntry {
                id: new_id(),
                role: zeron_doc::MessageRole::Assistant,
                parts: vec![MessagePart::Text {
                    id: "t0".into(),
                    text,
                }],
                created_at: now_ms(),
                device_id: inner.device_id.clone(),
                status: Some(zeron_doc::MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            });
        };
        match std::fs::read(&backup) {
            Ok(bytes) => {
                use base64::Engine as _;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                let ws = crate::cloud::shell_word(&seat.workspace());
                let staged = format!("/tmp/cosmos-restore-{run_id}.tgz.b64");
                let _ = api
                    .exec(
                        &box_id,
                        "rm -f /tmp/cosmos-restore-*.tgz.b64",
                        None,
                        std::time::Duration::from_secs(30),
                    )
                    .await;
                if let Err(e) = api.write_file(&box_id, &staged, b64.as_bytes()).await {
                    let _ = api
                        .exec(
                            &box_id,
                            &format!("rm -f {staged}"),
                            None,
                            std::time::Duration::from_secs(30),
                        )
                        .await;
                    note(format!(
                        "Couldn't upload the Railway backup to this box ({e}) — starting fresh."
                    ));
                } else {
                    match api
                        .exec(
                            &box_id,
                            &format!(
                                "trap 'rm -f {staged}' EXIT; mkdir -p {ws} && base64 -d {staged} | tar xzf - -C {ws}"
                            ),
                            None,
                            std::time::Duration::from_secs(300),
                        )
                        .await
                    {
                        Ok(r) if r.success => {
                            store.update_seat(&seat.id, |s| {
                                s.restored_at = Some(Utc::now());
                            });
                            note("Restored this workspace from the Railway trial backup — your earlier work is here.".to_string());
                        }
                        Ok(r) => note(format!(
                            "Railway backup upload worked but extract failed ({}) — starting fresh.",
                            r.stderr.lines().next().unwrap_or("tar error")
                        )),
                        Err(e) => note(format!(
                            "Couldn't extract the Railway backup ({e}) — starting fresh."
                        )),
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "railway backup unreadable — skipping restore")
            }
        }
    }

    // Kick the integrated agent. The prompt response carries the
    // conversation handle; events stream the transcript we mirror.
    let live_seat = {
        let mut live = seat.clone();
        live.box_id = Some(box_id.clone());
        live
    };
    let prompt = prepare_remote_turn(&inner, &store, &http, &live_seat, &chat_id, turn, doc).await;
    if let Err(e) = api
        .prompt(&box_id, provider, &prompt, turn.model.as_deref())
        .await
    {
        fail(format!("Boat couldn't start the agent: {e}"));
        return;
    }

    let outcome = mirror_boat_events(
        &inner,
        &store,
        &api,
        &box_id,
        &run_id,
        doc,
        &cancel_rx,
        &interrupt_token,
        None,
        None,
    )
    .await;
    match outcome {
        BoatOutcome::Done => finish(SessionStatus::Idle, zeron_proto::CloudRunStatus::Done, None),
        BoatOutcome::Stopped => finish(
            SessionStatus::Idle,
            zeron_proto::CloudRunStatus::Stopped,
            None,
        ),
        BoatOutcome::Failed(message) => fail(message),
    }
}

/// How a mirrored Boat turn ended.
enum BoatOutcome {
    Done,
    Stopped,
    Failed(String),
}

/// Mirror a Boat box's integrated-agent `/events` into the chat until the
/// turn finishes, errors, stalls, or is cancelled. The cursor is recorded on
/// the run after every page, so a reopened app resumes from it.
#[allow(clippy::too_many_arguments)]
async fn mirror_boat_events(
    inner: &Arc<Inner>,
    store: &crate::cloud::CloudStore,
    api: &crate::boat::Boat<'_>,
    box_id: &str,
    run_id: &str,
    doc: &crate::doc_host::DocWriter,
    cancel_rx: &watch::Receiver<bool>,
    interrupt_token: &CancellationToken,
    mut cursor: Option<String>,
    // A reattach continues the run's own entry from the recorded cursor.
    resume_entry: Option<&str>,
) -> BoatOutcome {
    let resumed = resume_entry.and_then(|id| {
        doc.find_entry(id).map(|(index, written)| {
            let text = written
                .iter()
                .find_map(|part| match part {
                    MessagePart::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            (
                id.to_string(),
                zeron_doc::SegmentWriter::resume(doc, index, written),
                text,
            )
        })
    });
    let (entry_id, mut segment, mut text) = match resumed {
        Some((id, segment, text)) => (id, Some(segment), text),
        None => {
            let id = new_id();
            let segment =
                zeron_doc::SegmentWriter::begin(doc, &id, &inner.device_id, now_ms()).ok();
            (id, segment, String::new())
        }
    };
    store.update_run(run_id, |run| run.entry_id = Some(entry_id.clone()));
    let mut idle_ticks = 0u32;
    let close = |segment: &mut Option<zeron_doc::SegmentWriter>,
                 text: &str,
                 status: zeron_doc::MessageStatus| {
        if let Some(segment) = segment.take() {
            let _ = segment.finish(
                &[MessagePart::Text {
                    id: "t0".into(),
                    text: text.to_string(),
                }],
                status,
            );
        }
    };
    loop {
        if *cancel_rx.borrow() || interrupt_token.is_cancelled() {
            let _ = api.interrupt(box_id).await;
            close(&mut segment, &text, zeron_doc::MessageStatus::Aborted);
            return BoatOutcome::Stopped;
        }
        match api.events(box_id, cursor.as_deref()).await {
            Ok(v) => {
                let Some(items) = v
                    .get("events")
                    .or_else(|| v.get("items"))
                    .and_then(|e| e.as_array())
                    .cloned()
                else {
                    close(&mut segment, &text, zeron_doc::MessageStatus::Aborted);
                    return BoatOutcome::Failed(
                        "boat: unexpected response from /sandboxes/{id}/events: missing events"
                            .into(),
                    );
                };
                let mut grew = false;
                let mut done = false;
                for event in items {
                    cursor = event
                        .get("cursor")
                        .and_then(|c| c.as_str())
                        .map(str::to_string)
                        .or(cursor);
                    let kind = event
                        .get("type")
                        .or_else(|| event.get("kind"))
                        .and_then(|k| k.as_str())
                        .unwrap_or_default();
                    let data = event.get("data").cloned().unwrap_or_default();
                    match kind {
                        // Assistant text deltas — mirror verbatim.
                        "assistant" | "message" | "text" => {
                            if let Some(t) = data
                                .get("text")
                                .or_else(|| data.get("delta"))
                                .and_then(|t| t.as_str())
                            {
                                text.push_str(t);
                                grew = true;
                            }
                        }
                        // Tool lines keep the transcript honest without a
                        // second surface.
                        "tool" | "tool_use" | "tool_result" => {
                            if let Some(t) = data
                                .get("summary")
                                .or_else(|| data.get("name"))
                                .and_then(|t| t.as_str())
                            {
                                text.push_str(&format!("\n\n› {t}\n"));
                                grew = true;
                            }
                        }
                        "done" | "complete" | "finished" => done = true,
                        "error" => {
                            let message = data
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("the Boat agent errored")
                                .to_string();
                            close(&mut segment, &text, zeron_doc::MessageStatus::Aborted);
                            return BoatOutcome::Failed(message);
                        }
                        _ => {}
                    }
                }
                if let Some(next) = v
                    .get("nextCursor")
                    .or_else(|| v.get("cursor"))
                    .and_then(|c| c.as_str())
                {
                    cursor = Some(next.to_string());
                }
                if cursor.is_some() {
                    let at = cursor.clone();
                    store.update_run(run_id, |run| run.remote_cursor = at);
                }
                if grew {
                    if let Some(segment) = segment.as_mut() {
                        let _ = segment.sync(&[MessagePart::Text {
                            id: "t0".into(),
                            text: text.clone(),
                        }]);
                    }
                    store.update_run(run_id, |run| run.mirrored_bytes = text.len() as u64);
                    idle_ticks = 0;
                } else {
                    idle_ticks += 1;
                }
                if done {
                    close(&mut segment, &text, zeron_doc::MessageStatus::Complete);
                    return BoatOutcome::Done;
                }
            }
            Err(e) => {
                // A dead box mid-run is terminal; a transient network error
                // is not — keep polling before giving up.
                idle_ticks += 1;
                if api.sandbox(box_id).await.map(|s| s.dead()).unwrap_or(false) {
                    close(&mut segment, &text, zeron_doc::MessageStatus::Aborted);
                    return BoatOutcome::Failed(format!("the Boat box died mid-run: {e}"));
                }
            }
        }
        // No events for ~10 min → the agent likely wedged; end honestly.
        if idle_ticks > 300 {
            close(&mut segment, &text, zeron_doc::MessageStatus::Aborted);
            return BoatOutcome::Failed(
                "no agent events for 10 minutes — the run likely stalled on the box".into(),
            );
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

/// Renders Claude Code's `--output-format stream-json` events as transcript
/// text while the run is still going: assistant prose as paragraphs, each
/// tool call as one `›` line, the final result only when it adds something.
/// Lines that aren't JSON (an install notice, a shell error) pass through.
#[derive(Default)]
struct StreamJsonRenderer {
    partial: Vec<u8>,
    last_text: String,
    after_tool: bool,
}

impl StreamJsonRenderer {
    #[cfg(test)]
    fn for_command(command: &str) -> Option<Self> {
        command
            .contains("--output-format stream-json")
            .then(Self::default)
    }

    /// Feed raw output; returns the text for every line it completes.
    fn push(&mut self, chunk: &[u8]) -> String {
        self.partial.extend_from_slice(chunk);
        let mut out = String::new();
        while let Some(at) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=at).collect();
            out.push_str(&self.render_line(&String::from_utf8_lossy(&line)));
        }
        out
    }

    /// The stream ended: render a last unterminated line, if any.
    fn finish(&mut self) -> String {
        let line = std::mem::take(&mut self.partial);
        self.render_line(&String::from_utf8_lossy(&line))
    }

    fn render_line(&mut self, line: &str) -> String {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            return String::new();
        }
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            self.after_tool = false;
            return format!("{line}\n");
        };
        let mut out = String::new();
        match event.get("type").and_then(|t| t.as_str()) {
            Some("assistant") => {
                let blocks = event
                    .pointer("/message/content")
                    .and_then(|c| c.as_array())
                    .cloned()
                    .unwrap_or_default();
                for block in blocks {
                    match block.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            let text = block
                                .get("text")
                                .and_then(|t| t.as_str())
                                .unwrap_or_default()
                                .trim();
                            if text.is_empty() {
                                continue;
                            }
                            if self.after_tool {
                                out.push('\n');
                            }
                            out.push_str(text);
                            out.push_str("\n\n");
                            self.last_text = text.to_string();
                            self.after_tool = false;
                        }
                        Some("tool_use") => {
                            let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                            let input = block.get("input").cloned().unwrap_or_default();
                            out.push_str(&format!("› {}\n", tool_line(name, &input)));
                            self.after_tool = true;
                        }
                        _ => {}
                    }
                }
            }
            Some("result") => {
                let result = event
                    .get("result")
                    .and_then(|r| r.as_str())
                    .unwrap_or_default()
                    .trim();
                let is_error = event
                    .get("is_error")
                    .and_then(|e| e.as_bool())
                    .unwrap_or(false);
                if self.after_tool {
                    out.push('\n');
                }
                if is_error {
                    out.push_str(&format!(
                        "⚠ {}\n",
                        if result.is_empty() {
                            "the agent stopped with an error"
                        } else {
                            result
                        }
                    ));
                } else if !result.is_empty() && result != self.last_text {
                    out.push_str(result);
                    out.push('\n');
                }
                self.after_tool = false;
            }
            // system/init, tool results, rate-limit notices: not transcript.
            _ => {}
        }
        out
    }
}

/// One readable line for a tool call: what it touched, not its full input.
fn tool_line(name: &str, input: &serde_json::Value) -> String {
    let field = |key: &str| {
        input
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let clip = |text: &str| {
        let first = text.lines().next().unwrap_or_default();
        let mut clipped: String = first.chars().take(120).collect();
        if clipped.len() < first.len() || text.lines().nth(1).is_some() {
            clipped.push('…');
        }
        clipped
    };
    let detail = match name {
        "Bash" => field("command").map(|c| format!("`{}`", clip(c))),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => field("file_path")
            .or_else(|| field("notebook_path"))
            .map(str::to_string),
        "Glob" | "Grep" => field("pattern").map(|p| format!("`{}`", clip(p))),
        "WebFetch" => field("url").map(str::to_string),
        "WebSearch" => field("query").map(str::to_string),
        "Task" | "Agent" => field("description").map(str::to_string),
        "TodoWrite" => Some("task list".to_string()),
        _ => None,
    };
    match detail {
        Some(detail) => format!("{name} {detail}"),
        None => name.to_string(),
    }
}

/// Renders `opencode run --format json` events the same way the Claude Code
/// stream does: text parts as paragraphs, each tool call as one `›` line.
/// Lines that aren't JSON (a login banner, a shell notice) pass through.
#[derive(Default)]
struct OpencodeJsonRenderer {
    partial: Vec<u8>,
    after_tool: bool,
}

impl OpencodeJsonRenderer {
    fn push(&mut self, chunk: &[u8]) -> String {
        self.partial.extend_from_slice(chunk);
        let mut out = String::new();
        while let Some(at) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=at).collect();
            out.push_str(&self.render_line(&String::from_utf8_lossy(&line)));
        }
        out
    }

    fn finish(&mut self) -> String {
        let line = std::mem::take(&mut self.partial);
        self.render_line(&String::from_utf8_lossy(&line))
    }

    fn render_line(&mut self, line: &str) -> String {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            return String::new();
        }
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            self.after_tool = false;
            return format!("{line}\n");
        };
        let mut out = String::new();
        match event.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                let text = event
                    .pointer("/part/text")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .trim();
                if !text.is_empty() {
                    if self.after_tool {
                        out.push('\n');
                    }
                    out.push_str(text);
                    out.push_str("\n\n");
                    self.after_tool = false;
                }
            }
            Some("tool_use") => {
                let part = event.get("part").cloned().unwrap_or_default();
                let tool = part
                    .get("tool")
                    .and_then(|t| t.as_str())
                    .unwrap_or("tool");
                let input = part
                    .pointer("/state/input")
                    .cloned()
                    .unwrap_or_default();
                out.push_str(&format!("› {}\n", opencode_tool_line(tool, &input)));
                self.after_tool = true;
            }
            Some("error") => {
                let message = event
                    .get("error")
                    .or_else(|| event.pointer("/part/error"))
                    .map(|e| {
                        e.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| e.to_string())
                    })
                    .unwrap_or_else(|| "the agent stopped with an error".into());
                out.push_str(&format!("⚠ {message}\n"));
                self.after_tool = false;
            }
            // step_start/step_finish, session bookkeeping: not transcript.
            _ => {}
        }
        out
    }
}

/// One readable line for an opencode tool call: what it touched, not its
/// full input. Input keys vary by tool (`path`, `filePath`, `command`, …).
fn opencode_tool_line(tool: &str, input: &serde_json::Value) -> String {
    let field = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|key| input.get(*key).and_then(|v| v.as_str()))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let clip = |text: &str| {
        let first = text.lines().next().unwrap_or_default();
        let mut clipped: String = first.chars().take(120).collect();
        if clipped.len() < first.len() || text.lines().nth(1).is_some() {
            clipped.push('…');
        }
        clipped
    };
    let detail = field(&["filePath", "file_path", "path", "notebookPath"])
        .or_else(|| field(&["command", "pattern", "query", "url"]).map(|v| format!("`{}`", clip(&v))))
        .or_else(|| field(&["description", "prompt", "task_id"]));
    match detail {
        Some(detail) => format!("{tool} {detail}"),
        None => tool.to_string(),
    }
}

/// The remote-output renderer a seat command needs, if any.
enum RemoteRenderer {
    StreamJson(StreamJsonRenderer),
    OpencodeJson(OpencodeJsonRenderer),
}

impl RemoteRenderer {
    fn for_command(command: &str) -> Option<Self> {
        if command.contains("--output-format stream-json") {
            return Some(Self::StreamJson(StreamJsonRenderer::default()));
        }
        if command.contains("--format json")
            && crate::cloud::command_bin(command).contains("opencode")
        {
            return Some(Self::OpencodeJson(OpencodeJsonRenderer::default()));
        }
        None
    }

    fn push(&mut self, chunk: &[u8]) -> String {
        match self {
            Self::StreamJson(render) => render.push(chunk),
            Self::OpencodeJson(render) => render.push(chunk),
        }
    }

    fn finish(&mut self) -> String {
        match self {
            Self::StreamJson(render) => render.finish(),
            Self::OpencodeJson(render) => render.finish(),
        }
    }
}

/// CSI fragments left when a control byte was eaten before the stream
/// reached us (`[33m`, `[?25h`, `[38;2;155;109;255;4m`). Matches a `[`
/// followed by CSI parameters and a final byte — narrow enough that real
/// text (`[0m` inside a string literal, `[K` in a log message) is rare
/// enough to sacrifice for never drawing escape garbage in the transcript.
fn strip_orphaned_ansi(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut copy_from = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            while j < bytes.len() && matches!(bytes[j], b'0'..=b'9' | b';' | b'?' | b':') {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && (0x40..=0x7e).contains(&bytes[j]) {
                out.push_str(&text[copy_from..i]);
                i = j + 1;
                copy_from = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&text[copy_from..]);
    out
}

#[derive(Default)]
struct AnsiStripper {
    state: AnsiState,
}

#[derive(Default, Clone, Copy)]
enum AnsiState {
    #[default]
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

impl AnsiStripper {
    fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut text = Vec::with_capacity(bytes.len());
        for &byte in bytes {
            match self.state {
                AnsiState::Text if byte == 0x1b => self.state = AnsiState::Escape,
                AnsiState::Text => text.push(byte),
                AnsiState::Escape => {
                    self.state = match byte {
                        b'[' => AnsiState::Csi,
                        b']' => AnsiState::Osc,
                        _ => {
                            text.extend_from_slice(&[0x1b, byte]);
                            AnsiState::Text
                        }
                    };
                }
                AnsiState::Csi if (0x40..=0x7e).contains(&byte) => {
                    self.state = AnsiState::Text;
                }
                AnsiState::Csi => {}
                AnsiState::Osc if byte == 0x07 => self.state = AnsiState::Text,
                AnsiState::Osc if byte == 0x1b => self.state = AnsiState::OscEscape,
                AnsiState::Osc => {}
                AnsiState::OscEscape if byte == b'\\' => self.state = AnsiState::Text,
                AnsiState::OscEscape if byte == 0x07 => self.state = AnsiState::Text,
                AnsiState::OscEscape if byte == 0x1b => {}
                AnsiState::OscEscape => self.state = AnsiState::Osc,
            }
        }
        text
    }
}

fn parse_remote_tail_output(
    run_id: &str,
    stdout: &str,
) -> Result<(Vec<u8>, bool, Option<i32>), String> {
    use base64::Engine as _;

    let begin = format!("__cosmos_tail_begin_{run_id}__\n");
    let end = format!("\n__cosmos_tail_end_{run_id}__");
    let (_, framed) = stdout
        .split_once(&begin)
        .ok_or_else(|| "remote log response has no start marker".to_string())?;
    let (encoded, metadata) = framed
        .split_once(&end)
        .ok_or_else(|| "remote log response has no end marker".to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|err| format!("invalid remote log frame: {err}"))?;

    let mut alive = false;
    let mut remote_exit = None;
    for line in metadata.lines() {
        if let Some(flag) = line.strip_prefix("__alive_") {
            alive = flag.trim() == "0";
        } else if let Some(status) = line.strip_prefix("__exit_") {
            remote_exit = status.trim().parse::<i32>().ok();
        }
    }
    Ok((bytes, alive, remote_exit))
}

/// Tail a detached run's log on the seat, mirroring new bytes into the doc
/// until the remote process exits (or the caller cancels).
#[allow(clippy::too_many_arguments)]
async fn tail_remote_log(
    inner: &Arc<Inner>,
    http: &reqwest::Client,
    store: &crate::cloud::CloudStore,
    chat_id: &str,
    run_id: &str,
    seat: &crate::cloud::SeatConfig,
    log_path: &str,
    doc: &crate::doc_host::DocWriter,
    cancel_rx: &mut watch::Receiver<bool>,
    // A reattach reopens the run's own transcript entry and rebuilds it from
    // the start of the log — whatever reached the doc before the app closed
    // is rewritten in place, so nothing is lost or repeated.
    resume_entry: Option<&str>,
    // The run's harness command — picks how its output renders.
    command: &str,
) {
    let strip_headers = is_opencode_command(command);
    let mut render = RemoteRenderer::for_command(command);
    let (entry_id, mut segment) = match resume_entry.and_then(|id| {
        doc.find_entry(id).map(|(index, written)| {
            (
                id.to_string(),
                zeron_doc::SegmentWriter::resume(doc, index, written),
            )
        })
    }) {
        Some((id, segment)) => (id, Some(segment)),
        None => (new_id(), None),
    };
    store.update_run(run_id, |run| run.entry_id = Some(entry_id.clone()));
    let mut mirrored: u64 = 0;
    let mut text = String::new();
    let mut ansi = AnsiStripper::default();
    let mut ssh_255_streak = 0u8;
    loop {
        if *cancel_rx.borrow() {
            let _ = segment.take().and_then(|s| {
                s.finish(
                    &[MessagePart::Text {
                        id: "t0".into(),
                        text: cloud_run_doc_text(&text, strip_headers),
                    }],
                    zeron_doc::MessageStatus::Aborted,
                )
                .ok()
            });
            inner.set_status(chat_id, SessionStatus::Idle, false);
            store.update_run(run_id, |run| {
                run.status = zeron_proto::CloudRunStatus::Stopped;
                run.finished_at = Some(Utc::now());
            });
            lock(&inner.runs).remove(chat_id);
            return;
        }
        // Base64 frames the log bytes separately from metadata so neither
        // framing newlines nor line-ending normalization can move the cursor.
        let cmd = format!(
            "printf '__cosmos_tail_begin_{run_id}__\\n'; tail -c +{} -- \"{log_path}\" 2>/dev/null | head -c 262144 | base64 | tr -d '\\n'; printf '\\n__cosmos_tail_end_{run_id}__\\n'; if [ -f \"$HOME/.cosmos/runs/{run_id}.exit\" ]; then printf '__exit_%s\\n' \"$(cat \"$HOME/.cosmos/runs/{run_id}.exit\")\"; else kill -0 \"$(cat \"$HOME/.cosmos/runs/{run_id}.pid\" 2>/dev/null)\" 2>/dev/null; echo __alive_$?; fi",
            mirrored + 1,
        );
        match crate::cloud::seat_exec(http, store, seat, &cmd, std::time::Duration::from_secs(20))
            .await
        {
            Ok(out) => {
                if out.code == 255 {
                    ssh_255_streak = ssh_255_streak.saturating_add(1);
                } else {
                    ssh_255_streak = 0;
                }
                if ssh_255_streak >= 3
                    || out.stderr.contains("Permission denied")
                    || out.stdout.contains("Permission denied")
                {
                    let text = cloud_run_output(&text, strip_headers);
                    terminal_cloud_run(inner, store, chat_id, run_id, &mut segment, text.as_ref());
                    return;
                }
                let (chunk, alive, remote_exit) =
                    match parse_remote_tail_output(run_id, &out.stdout) {
                        Ok(frame) => frame,
                        Err(error) => {
                            tracing::warn!(run_id, %error, "failed to parse remote log tail");
                            (Vec::new(), false, None)
                        }
                    };
                if !chunk.is_empty() {
                    mirrored += chunk.len() as u64;
                    let clean = ansi.push(&chunk);
                    match render.as_mut() {
                        Some(render) => text.push_str(&render.push(&clean)),
                        None => text.push_str(&String::from_utf8_lossy(&clean)),
                    }
                    if segment.is_none() {
                        segment = zeron_doc::SegmentWriter::begin(
                            doc,
                            &entry_id,
                            &inner.device_id,
                            now_ms(),
                        )
                        .ok();
                    }
                    if let Some(segment) = segment.as_mut() {
                        let _ = segment.sync(&[MessagePart::Text {
                            id: "t0".into(),
                            text: cloud_run_doc_text(&text, strip_headers),
                        }]);
                    }
                    store.update_run(run_id, |run| run.mirrored_bytes = mirrored);
                }
                if out.code != 0 || remote_exit.is_some() || !alive {
                    if let Some(render) = render.as_mut() {
                        text.push_str(&render.finish());
                    }
                    let done =
                        out.code == 0 && remote_exit == Some(0) && !remote_agent_failed(0, &text);
                    let error = (!done).then(|| {
                        let status = remote_exit.unwrap_or_else(|| out.code.max(1));
                        let stderr = if remote_exit.is_none() && out.stderr.is_empty() {
                            "remote agent ended without an exit status".to_string()
                        } else {
                            out.stderr.clone()
                        };
                        remote_agent_failure_message(status, &text, &stderr)
                    });
                    if let Some(message) = error.as_deref() {
                        write_remote_error(doc, &inner.device_id, message);
                    }
                    let _ = segment.take().and_then(|s| {
                        s.finish(
                            &[MessagePart::Text {
                                id: "t0".into(),
                                text: cloud_run_doc_text(&text, strip_headers),
                            }],
                            if done {
                                zeron_doc::MessageStatus::Complete
                            } else {
                                zeron_doc::MessageStatus::Aborted
                            },
                        )
                        .ok()
                    });
                    inner.set_status(
                        chat_id,
                        if done {
                            SessionStatus::Idle
                        } else {
                            SessionStatus::Errored
                        },
                        false,
                    );
                    store.update_run(run_id, |run| {
                        run.status = if done {
                            zeron_proto::CloudRunStatus::Done
                        } else {
                            zeron_proto::CloudRunStatus::Error
                        };
                        run.error = error;
                        run.finished_at = Some(Utc::now());
                    });
                    lock(&inner.runs).remove(chat_id);
                    return;
                }
            }
            Err(e) => {
                if e.contains("expired")
                    || e.contains("Permission denied")
                    || (matches!(seat.kind, Some(zeron_proto::CloudSeatKind::Railway))
                        && e.contains("Railway"))
                {
                    let text = cloud_run_output(&text, strip_headers);
                    terminal_cloud_run(inner, store, chat_id, run_id, &mut segment, text.as_ref());
                    return;
                }
                tracing::warn!(seat = %seat.id, error = %e, "cloud run log tail failed");
            }
        }
        tokio::select! {
            _ = cancel_rx.changed() => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
        }
    }
}

const CLOUD_EXPIRY_MESSAGE: &str =
    "Cloud seat expired — the run was cut off. Reprovision the seat to continue.";

fn terminal_cloud_run(
    inner: &Arc<Inner>,
    store: &crate::cloud::CloudStore,
    chat_id: &str,
    run_id: &str,
    segment: &mut Option<zeron_doc::SegmentWriter>,
    text: &str,
) {
    let _ = segment.take().and_then(|s| {
        s.finish(
            &[MessagePart::Text {
                id: "t0".into(),
                text: text.to_string(),
            }],
            zeron_doc::MessageStatus::Aborted,
        )
        .ok()
    });
    inner.set_status(chat_id, SessionStatus::Errored, false);
    store.update_run(run_id, |run| {
        run.status = zeron_proto::CloudRunStatus::Error;
        run.error = Some(CLOUD_EXPIRY_MESSAGE.to_string());
        run.finished_at = Some(Utc::now());
    });
    lock(&inner.runs).remove(chat_id);
}

/// Seat harnesses print plain terminal text; in the transcript's markdown a
/// single newline would fold lines into one paragraph. Hard-break them
/// (outside code fences) so the output reads the way it printed.
fn preserve_line_breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 16);
    let mut in_fence = false;
    let mut lines = text.split('\n').peekable();
    while let Some(line) = lines.next() {
        let fence = line.trim_start().starts_with("```");
        if fence {
            in_fence = !in_fence;
        }
        out.push_str(line);
        if let Some(next) = lines.peek() {
            let hard = !in_fence
                && !fence
                && !line.trim().is_empty()
                && !next.trim().is_empty()
                && !line.ends_with("  ")
                && !next.trim_start().starts_with("```");
            if hard {
                out.push_str("  ");
            }
            out.push('\n');
        }
    }
    out
}

fn is_opencode_command(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .and_then(|executable| std::path::Path::new(executable).file_name())
        .and_then(|basename| basename.to_str())
        == Some("opencode")
}

fn is_opencode_header_line(line: &str) -> bool {
    line.strip_prefix("> ").is_some_and(|header| {
        let Some((a, b)) = header.split_once(" · ") else {
            return false;
        };
        let token = |part: &str| {
            !part.is_empty() && !part.chars().any(|ch| ch.is_whitespace() || ch == '·')
        };
        token(a) && token(b)
    })
}

fn strip_opencode_headers(text: &str) -> std::borrow::Cow<'_, str> {
    let has_header = text.split_inclusive('\n').any(|raw_line| {
        if !raw_line.ends_with('\n') {
            return false;
        }
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        is_opencode_header_line(line)
    });
    if !has_header {
        return std::borrow::Cow::Borrowed(text);
    }

    let mut skip_empty_line = false;
    let mut out = String::with_capacity(text.len());

    for raw_line in text.split_inclusive('\n') {
        let terminated = raw_line.ends_with('\n');
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        let line = line.strip_suffix('\r').unwrap_or(line);

        if skip_empty_line {
            skip_empty_line = false;
            if terminated && line.is_empty() {
                continue;
            }
        }

        if terminated && is_opencode_header_line(line) {
            skip_empty_line = true;
            continue;
        }

        out.push_str(raw_line);
    }

    std::borrow::Cow::Owned(out)
}

fn cloud_run_output(text: &str, strip_headers: bool) -> std::borrow::Cow<'_, str> {
    let cleaned: std::borrow::Cow<'_, str> = if text.contains('[') {
        std::borrow::Cow::Owned(strip_orphaned_ansi(text))
    } else {
        std::borrow::Cow::Borrowed(text)
    };
    if strip_headers {
        match strip_opencode_headers(&cleaned) {
            std::borrow::Cow::Borrowed(_) => cleaned,
            stripped @ std::borrow::Cow::Owned(_) => {
                std::borrow::Cow::Owned(stripped.into_owned())
            }
        }
    } else {
        cleaned
    }
}

fn cloud_run_doc_text(text: &str, strip_headers: bool) -> String {
    preserve_line_breaks(cloud_run_output(text, strip_headers).as_ref())
}

/// An honest error line in the transcript when a remote run can't even start.
fn write_remote_error(doc: &crate::doc_host::DocWriter, device_id: &str, message: &str) {
    let _ = doc.push_message(&SessionMessageEntry {
        id: new_id(),
        role: zeron_doc::MessageRole::Assistant,
        parts: vec![MessagePart::Text {
            id: "t0".into(),
            text: format!("Cloud run failed: {message}"),
        }],
        created_at: now_ms(),
        device_id: device_id.to_string(),
        status: Some(zeron_doc::MessageStatus::Aborted),
        continuation_of: None,
        duration_ms: None,
    });
}

fn remote_agent_failed(status: i32, stdout: &str) -> bool {
    status != 0 || stdout.trim().is_empty()
}

fn remote_agent_failure_message(status: i32, stdout: &str, stderr: &str) -> String {
    let output = [stdout.trim_end(), stderr.trim_end()]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    // ssh's own "Connection to … closed." (from `-tt`) is transport noise,
    // never the reason a run failed.
    let lines = output
        .lines()
        .map(str::trim_end)
        .filter(|line| {
            let line = line.trim();
            !line.is_empty() && !(line.starts_with("Connection to ") && line.ends_with(" closed."))
        })
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        return lines[lines.len().saturating_sub(20)..].join("\n");
    }
    if status == 0 {
        "remote agent exited successfully without output".into()
    } else {
        format!("seat run exited {status}")
    }
}

fn write_transcript_note(doc: &crate::doc_host::DocWriter, device_id: &str, message: &str) {
    let _ = doc.push_message(&SessionMessageEntry {
        id: new_id(),
        role: zeron_doc::MessageRole::Assistant,
        parts: vec![MessagePart::Text {
            id: "t0".into(),
            text: message.to_string(),
        }],
        created_at: now_ms(),
        device_id: device_id.to_string(),
        status: Some(zeron_doc::MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    });
}

#[cfg(test)]
mod line_break_tests {
    use super::preserve_line_breaks;

    #[test]
    fn plain_lines_hard_break_but_fences_stay_verbatim() {
        assert_eq!(preserve_line_breaks("a\nb\n\nc"), "a  \nb\n\nc");
        assert_eq!(
            preserve_line_breaks("x\n```\nl1\nl2\n```\ny"),
            "x\n```\nl1\nl2\n```\ny"
        );
        assert_eq!(preserve_line_breaks("done"), "done");
    }
}

#[cfg(test)]
mod opencode_header_tests {
    use super::{
        cloud_run_doc_text, is_opencode_command, preserve_line_breaks, strip_opencode_headers,
    };

    #[test]
    fn removes_complete_opencode_headers_and_the_following_blank_line() {
        assert_eq!(
            strip_opencode_headers("\n> build · big-pickle\n\nHey!\n").as_ref(),
            "\nHey!\n"
        );
    }

    #[test]
    fn keeps_quotes_and_incomplete_trailing_headers() {
        let quote = "> note: this is quoted\n";
        let incomplete = "Hello\n> build · big-pickle";
        assert_eq!(strip_opencode_headers(quote).as_ref(), quote);
        assert_eq!(strip_opencode_headers(incomplete).as_ref(), incomplete);
    }

    #[test]
    fn command_detection_uses_the_executable_not_the_model_id() {
        assert!(is_opencode_command("opencode run -m opencode/big-pickle"));
        assert!(is_opencode_command(
            "/usr/local/bin/opencode run -m opencode/big-pickle"
        ));
        assert!(!is_opencode_command("claude --model opencode/big-pickle"));

        let text = "> build · big-pickle\n\nHey!\n";
        assert_eq!(cloud_run_doc_text(text, false), preserve_line_breaks(text));
    }
}

#[cfg(test)]
mod remote_agent_failure_tests {
    use super::{StreamJsonRenderer, remote_agent_failed, remote_agent_failure_message};

    #[test]
    fn failed_stub_remote_surfaces_recent_stderr() {
        let output = std::process::Command::new("sh")
            .args([
                "-c",
                "printf '%s\\n' 'Not logged in · Please run /login' >&2; exit 1",
            ])
            .output()
            .expect("run stub remote command");
        let status = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(remote_agent_failed(status, &stdout));
        assert_eq!(
            remote_agent_failure_message(status, &stdout, &stderr),
            "Not logged in · Please run /login"
        );
    }

    #[test]
    fn claude_stream_json_renders_progress_as_it_arrives() {
        let mut render = StreamJsonRenderer::for_command(
            "IS_SANDBOX=1 claude -p --output-format stream-json --verbose",
        )
        .expect("stream-json command renders");
        assert!(StreamJsonRenderer::for_command("codex exec").is_none());
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"s"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"I'll build the page."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls -la\necho hi"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"total 0"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Write","input":{"file_path":"/app/index.html","content":"<html>"}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done — the page is live."}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Done — the page is live."}"#,
        ];
        let stream = lines.join("\n") + "\n";
        // Split mid-line (and mid-character): output only lands per line.
        let bytes = stream.as_bytes();
        let cut = stream.find("page is").unwrap() + 2;
        let mut out = render.push(&bytes[..cut]);
        assert!(!out.contains("Done"));
        out.push_str(&render.push(&bytes[cut..]));
        out.push_str(&render.finish());
        assert_eq!(
            out,
            "I'll build the page.\n\n› Bash `ls -la…`\n› Write /app/index.html\n\nDone — the page is live.\n\n"
        );
    }

    #[test]
    fn claude_stream_json_surfaces_errors_and_plain_lines() {
        let mut render = StreamJsonRenderer::default();
        let out = render.push(
            b"`claude` isn't installed on this seat\n{\"type\":\"result\",\"is_error\":true,\"result\":\"Invalid API key \xc2\xb7 Please run /login\"}",
        );
        assert_eq!(out, "`claude` isn't installed on this seat\n");
        assert_eq!(render.finish(), "⚠ Invalid API key · Please run /login\n");
        assert_eq!(render.finish(), "");
    }

    #[test]
    fn ssh_transport_noise_never_reads_as_the_failure() {
        assert_eq!(
            remote_agent_failure_message(
                127,
                "`claude` isn't installed on this seat\r\n",
                "Connection to 127.0.0.1 closed.\r\n"
            ),
            "`claude` isn't installed on this seat"
        );
        assert_eq!(
            remote_agent_failure_message(255, "", "Connection to railway.new closed.\n"),
            "seat run exited 255"
        );
    }

    #[test]
    fn successful_remote_without_agent_output_is_a_failure() {
        assert!(remote_agent_failed(0, " \n"));
        assert_eq!(
            remote_agent_failure_message(0, "", ""),
            "remote agent exited successfully without output"
        );
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive_run(
    inner: Arc<Inner>,
    chat_id: String,
    run_id: String,
    harness: Arc<dyn Harness>,
    request: RunRequest,
    doc: crate::doc_host::DocWriter,
    controls: RunControls,
    mut engine_rx: mpsc::UnboundedReceiver<AgentEvent>,
    mut cancel_rx: watch::Receiver<bool>,
    resume_state: RunResumeState,
) {
    let device_id = inner.device_id.clone();
    // Captured for post-run auto-titling (the request moves into the harness).
    let harness_id = harness.id();
    let user_prompt = request.prompt.clone();
    let run_cwd = request.cwd.clone();
    if request.resume.is_none() {
        let _ = doc.clear_context_usage();
    }
    // Kept whole for the startup-crash retry (same user entry; dispatch
    // re-injects the stored resume id). Option so the retry branch (inside
    // the event loop) can take ownership.
    let mut retry_request = Some(RunRequest {
        resume: None,
        ..request.clone()
    });
    let mut harness_request = request;
    zeron_harness::prepare_browser_prompt(&mut harness_request);
    zeron_harness::prepare_computer_prompt(&mut harness_request);
    zeron_harness::prepare_tools_prompt(&mut harness_request);
    // Startup can stop before the SDK saves user text, with no new session
    // ID or receipt. Bridge that unacknowledged tail from our transcript;
    // a fresh session needs all prior user text, not just the latest tail.
    let prepared = if harness_id == HarnessId::Cursor {
        cursor_unstarted_history(
            &doc,
            &resume_state.user_message_id,
            &harness_request.prompt,
            harness_request.resume.is_some(),
        )
        .map(|prompt| harness_request.prompt = prompt)
        .map_err(|e| zeron_harness::HarnessError::Protocol(e.to_string()))
    } else {
        Ok(())
    };
    let started = match prepared {
        Ok(()) => {
            let mut wire_request = harness_request;
            if !matches!(harness_id, HarnessId::Cursor | HarnessId::Opencode) {
                wire_request.prompt =
                    zeron_proto::invocation::harness_prompt(&wire_request.prompt, harness_id);
            }
            harness.run(wire_request, controls).await
        }
        Err(error) => Err(error),
    };
    let mut stream = match started {
        Ok(stream) => stream,
        Err(err) => {
            let message = err.to_string();
            inner.publish(
                &chat_id,
                &AgentEvent::Error {
                    message: message.clone(),
                },
            );
            inner.publish(
                &chat_id,
                &AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: Some(message),
                    session_id: None,
                },
            );
            inner.remove_run(&chat_id, &run_id);
            inner.set_status(&chat_id, SessionStatus::Errored, false);
            return;
        }
    };

    let doc_ref: &SessionDoc = &doc;
    let mut folded: Vec<MessagePart> = Vec::new();
    // Every tool id this run has folded, across segment resets. Adapters
    // re-emit shape-bearing `tool_call_update`s (title/rawInput refreshes,
    // long-running completions) as full ToolCall events; once the fold has
    // reset at a steer/park boundary those ids are gone from `folded`, and
    // folding the echo would mint an orphan chip mid-text in the NEXT
    // segment — the mid-word transcript splits.
    let mut seen_tools: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_images = std::collections::HashSet::new();
    for entry in doc_ref.read_entries().unwrap_or_default() {
        for part in entry.parts {
            if let MessagePart::Image { id, .. } = part {
                seen_images.insert(format!("{chat_id}\0{id}"));
                if let Some(tool_id) = id.strip_suffix(":image") {
                    seen_tools.insert(tool_id.to_owned());
                }
            }
        }
    }
    let mut prepared_events = std::collections::VecDeque::new();
    let mut entry_id = new_id();
    let mut segment_started = now_ms();
    let mut writer: Option<SegmentWriter<'_>> = None;
    let mut dirty = false;
    let mut flush_at = tokio::time::Instant::now();
    // Set when the engine interrupts the run: the harness gets this long to end its own
    // stream (its token was cancelled); past it, a terminal Done is synthesized.
    let mut interrupt_deadline: Option<tokio::time::Instant> = None;
    let mut interrupted = false;
    let mut saw_session_started = false;
    // Liveness heartbeat: this loop RUNNING is proof the harness stream is
    // open, so freshness must not depend on events arriving. Silent stretches
    // are normal and UNBOUNDED — a long tool call, redacted thinking, an
    // agent waiting on an external process, a question parked for an hour —
    // and each starved the UI's 45s staleness gate in turn (working strip /
    // AwaitingInput dot vanishing mid-run, both user-reported). No stall
    // timeout here by design (a first port was rejected — agents may
    // legitimately be quiet for >10min): a live child means Working, dying
    // paths each carry their own error, and engine death stops these ticks
    // so the gate still catches real crashes. touch_session throttles at 10s.
    let mut live_heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));
    live_heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // PERSISTENT SESSION (zeron runsBySession): a completed turn on a
    // steerable harness parks here instead of ending the run — the child and
    // its steering mailbox stay warm, and the next user message (dispatch
    // routes into a live run) starts the next turn with zero respawn/resume
    // latency. `Some(when)` = idle since then; the 30-min reaper below ends
    // a session nobody comes back to (zeron SESSION_IDLE_MS).
    // `ZERON_SESSION_IDLE_MS` overrides the window (tests).
    let session_idle = std::env::var("ZERON_SESSION_IDLE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(std::time::Duration::from_millis)
        .unwrap_or(std::time::Duration::from_secs(30 * 60));
    // A live subagent stretches the window, but never unbounded: every
    // driver's subagent terminal is a best-effort vendor signal (claude's
    // untagged task_notification, grok's subagent_finished, codex thread
    // closure), and one lost signal would otherwise pin the child — and this
    // loop's registry heartbeat — until the chat is archived. A subagent
    // silent this long (4h by default) is presumed lost; the reap stamps its
    // chip failed.
    let subagent_silence = session_idle * 8;
    let mut idle_since: Option<tokio::time::Instant> = None;
    let steerable = harness.supports_steering();
    // TURN-QUIESCE WATCHDOG (2026-08-12 stuck-Working incident): a harness
    // that loses a turn's Done — the adapter never settles `session/prompt`
    // even though the agent finished — strands Working forever: the live
    // heartbeat above keeps the row fresh, and there is no per-turn timeout
    // by design. This is NOT that stall timeout: it never ends the run or
    // errors anything. When the stream has been silent past the window AND
    // the fold shows completed output with nothing in flight (no unresolved
    // tool, no open question), the turn parks exactly like a Done would —
    // segment finalized Complete, status Idle, child and mailbox warm. A
    // false trip (the agent was quietly waiting on something invisible)
    // costs a status dip: the parked-resume path below re-arms Working the
    // moment output flows again, and nothing is lost. `ZERON_TURN_QUIESCE_MS`
    // overrides the window; 0 disables.
    // RETIRED for native drivers: a harness whose every turn shape ends with
    // a deterministic wire Done (claude/codex/cursor native) needs no
    // quiesce backstop — arming one only risks false parks on long silent
    // work. The env knob can configure a diagnostic window, but a prompt
    // with an authoritative completion signal must still await that signal.
    // ACP retains the watchdog only for unowned self-continued activity.
    let deterministic_turn_end = harness.deterministic_turn_end();
    let authoritative_prompt_end = harness.authoritative_prompt_end();
    let quiesce_after: Option<std::time::Duration> = match std::env::var("ZERON_TURN_QUIESCE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(0) => None,
        Some(ms) => Some(std::time::Duration::from_millis(ms)),
        None if deterministic_turn_end => None,
        None => Some(std::time::Duration::from_secs(120)),
    };
    let mut last_stream_activity = tokio::time::Instant::now();
    // SELF-CONTINUED turns get a much SHORTER quiesce window. A turn the
    // agent starts on its own (background-task wake) can never receive a
    // harness Done: the adapter has no `session/prompt` outstanding to
    // settle — verified in claude-agent-acp's autonomous-result lane, which
    // consumes the SDK's turn-end without emitting anything; codex shows
    // the same shape. The watchdog is that turn shape's ONLY settle path,
    // so the default 120s window read as 2min of stuck-Working after every
    // background notification (user report 2026-08-13). The in-flight
    // fold gate below still protects running tools; reasoning heartbeats
    // push the window during real thinking. `ZERON_SELF_TURN_QUIESCE_MS`
    // overrides; 0 falls back to the normal window. An explicit
    // `ZERON_TURN_QUIESCE_MS=0` still disables the watchdog entirely.
    let self_quiesce_after: Option<std::time::Duration> =
        match std::env::var("ZERON_SELF_TURN_QUIESCE_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            Some(0) => None,
            Some(ms) => Some(std::time::Duration::from_millis(ms)),
            None => Some(std::time::Duration::from_secs(20)),
        };
    let mut self_continued_turn = false;
    // Spawn chips that have SETTLED (tagged Done seen). Content events for a
    // settled chip with no live sink are dropped — a straggler frame after
    // the freeze must not mint a new doc entry or wedge the transcript back
    // into Streaming. Only a steer (UserMessage) legitimately REOPENS a
    // settled subagent: it announces more work is coming.
    let mut settled_subagents: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Live subagent sinks, parent tool-use id → transcript doc state.
    let mut subagents: std::collections::HashMap<String, SubagentSink> =
        std::collections::HashMap::new();
    // Last tagged subagent event. A background subagent outlives the turn that
    // spawned it, and its traffic never un-parks the chat, so the idle reaper
    // measures from here as well as from the park.
    let mut last_subagent_activity: Option<tokio::time::Instant> = None;

    let mut final_completed_turn = None;
    let final_status = loop {
        let event: AgentEvent = if let Some(event) = prepared_events.pop_front() {
            event
        } else {
            let raw_event = tokio::select! {
                biased;
                changed = cancel_rx.changed(), if !interrupted => {
                    let _ = changed;
                    interrupted = true;
                    interrupt_deadline = Some(
                        tokio::time::Instant::now() + std::time::Duration::from_secs(3),
                    );
                    continue;
                }
                _ = tokio::time::sleep_until(
                    interrupt_deadline.unwrap_or_else(tokio::time::Instant::now)
                ), if interrupt_deadline.is_some() => AgentEvent::Done {
                    status: DoneStatus::Interrupted,
                    result: None,
                    error: None,
                    session_id: None,
                },
                _ = live_heartbeat.tick() => {
                    inner.touch_session(&chat_id);
                    continue;
                }
                // Idle reaper (zeron SESSION_IDLE_MS): a parked persistent session
                // nobody returned to in 30 minutes releases its child. The turn
                // was finalized at Done, so this end is clean — no aborted stamp.
                // A live background subagent is somebody still using the child:
                // the window counts from its last activity, and stretches to
                // `subagent_silence` while its sink is open.
                _ = tokio::time::sleep_until(
                    idle_since
                        .map(|at| {
                            at.max(last_subagent_activity.unwrap_or(at))
                                + if subagents.is_empty() { session_idle } else { subagent_silence }
                        })
                        .unwrap_or_else(tokio::time::Instant::now)
                ), if idle_since.is_some() => {
                    tracing::info!(
                        chat = %chat_id,
                        live_subagents = subagents.len(),
                        "reaping idle persistent session"
                    );
                    if let Some(token) = lock(&inner.runs)
                        .get(&chat_id)
                        .filter(|h| h.run_id == run_id)
                        .map(|h| h.interrupt_token.clone())
                    {
                        token.cancel();
                    }
                    break SessionStatus::Idle;
                }
                Some(event) = engine_rx.recv() => event,
                next = stream.next() => match next {
                    Some(Ok(event)) => event,
                    // A stream error while PARKED is a post-turn child death —
                    // the turn was already finalized, so the run ends clean
                    // instead of stamping a completed session Errored.
                    Some(Err(err)) if idle_since.is_some() => {
                        tracing::warn!(chat = %chat_id, error = %err, "parked session child died; ending clean");
                        break SessionStatus::Idle;
                    }
                    Some(Err(err)) => AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: Some(err.to_string()),
                        session_id: None,
                    },
                    None if interrupted => AgentEvent::Done {
                        status: DoneStatus::Interrupted,
                        result: None,
                        error: None,
                        session_id: None,
                    },
                    // Stream end while PARKED idle: a per-turn adapter closing
                    // after its final Done — a clean end, not a crash (the turn
                    // was already finalized). Persistent adapters keep the
                    // stream open and never hit this.
                    None if idle_since.is_some() => break SessionStatus::Idle,
                    None => AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: Some("harness stream ended without Done".into()),
                        session_id: None,
                    },
                },
                _ = tokio::time::sleep_until(flush_at), if dirty || subagents.values().any(|s| s.dirty) => {
                    #[cfg(test)]
                    FLUSH_TICKS.with(|t| t.set(t.get() + 1));
                    // Coalesced STREAM_COMMIT_MS tick: one doc commit per window
                    // (parent + any dirty subagent docs).
                    if dirty {
                        if let Err(err) = sync_segment(
                            doc_ref, &mut writer, &entry_id, &device_id, segment_started, &folded,
                        ) {
                            tracing::warn!(chat = %chat_id, error = %err, "segment sync failed");
                        }
                        dirty = false;
                    }
                    for sink in subagents.values_mut() {
                        sink.flush(&device_id);
                    }
                    continue;
                }
                // Turn-quiesce watchdog (see the knob above). Armed only when the
                // fold says nothing is in flight: an unresolved tool part is a
                // command still running (legitimately silent for minutes — the
                // rejected stall-timeout case), an unresolved input part is a
                // question awaiting the user. The live-plan chip is exempt from
                // the tool check: it is a singleton that never resolves. An EMPTY
                // fold still arms — a Steered boundary that no output ever
                // follows is one of the wedge shapes — it just parks without
                // writing a segment (an empty finalize would leave a stub entry).
                _ = tokio::time::sleep_until({
                    let mut window = quiesce_after.unwrap_or_default();
                    if self_continued_turn && let Some(short) = self_quiesce_after {
                        window = window.min(short);
                    }
                    last_stream_activity + window
                }), if quiesce_after.is_some()
                    && (self_continued_turn || !authoritative_prompt_end)
                    && idle_since.is_none()
                    && !interrupted
                    && steerable
                    && !folded.iter().any(|p| match p {
                        MessagePart::Tool { id, resolved: false, .. } => {
                            id != zeron_proto::LIVE_PLAN_TOOL_ID
                        }
                        MessagePart::Input { resolved: false, .. } => true,
                        _ => false,
                    }) =>
                {
                    tracing::warn!(
                        chat = %chat_id,
                        quiet_ms = quiesce_after.unwrap_or_default().as_millis() as u64,
                        "turn quiesced: stream silent after completed output with no \
                         turn-end; parking (suspected missing harness Done)"
                    );
                    // Some adapters close a completed response through this
                    // engine watchdog instead of a native Done. Preserve that
                    // completion notice, but never notify for an empty boundary
                    // or while an accepted steer still awaits delivery.
                    let completed_turn = ((!folded.is_empty() || writer.is_some())
                        && !inner.has_pending_steers(&chat_id, &run_id))
                        .then(|| entry_id.clone());
                    if !folded.is_empty() || writer.is_some() {
                        if let Err(err) = finish_segment(
                            doc_ref,
                            writer.take(),
                            &entry_id,
                            &device_id,
                            segment_started,
                            &folded,
                            MessageStatus::Complete,
                        ) {
                            tracing::warn!(chat = %chat_id, error = %err, "quiesce segment finish failed");
                        }
                        inner.note_message(&chat_id, &folded_text(&folded));
                    }
                    folded.clear();
                    dirty = false;
                    entry_id = new_id();
                    segment_started = now_ms();
                    idle_since = Some(tokio::time::Instant::now());
                    self_continued_turn = false;
                    inner.set_status_with_completion(
                        &chat_id, SessionStatus::Idle, false, completed_turn,
                    );
                    continue;
                }
            };

            prepared_events.extend(
                inner
                    .prepare_generated_image(&chat_id, raw_event, &mut seen_images)
                    .await,
            );
            let Some(event) = prepared_events.pop_front() else {
                continue;
            };
            event
        };

        // ── subagent routing ───────────────────────────────────────────
        // Tagged events NEVER fold into the parent transcript: they stream
        // into the subagent's own doc, and the parent keeps only the spawn
        // chip (ref + status + one-line tail) — refreshed through the live
        // fold while its segment still streams, else stamped in place (the
        // eager-done norm: the chip's entry finished before the background
        // subagent spoke). Handled BEFORE the parked gate so a parked
        // session's background traffic still reaches the subagent doc
        // without un-parking the chat.
        if let AgentEvent::Subagent {
            parent_tool_use_id,
            event: sub_event,
        } = &event
        {
            inner.publish(&chat_id, &event);
            last_subagent_activity = Some(tokio::time::Instant::now());
            let is_steer = matches!(
                sub_event.as_ref(),
                AgentEvent::UserMessage { .. } | AgentEvent::Steered { .. }
            );
            if is_steer {
                settled_subagents.remove(parent_tool_use_id);
            } else if settled_subagents.contains(parent_tool_use_id)
                && !subagents.contains_key(parent_tool_use_id)
            {
                // Straggler after the freeze: chip-only silence (the old
                // pre-viz behavior), never a reopened doc.
                continue;
            }
            let sub_id = subagent_doc_id(&chat_id, parent_tool_use_id);
            let chip_streaming = folded
                .iter()
                .any(|p| matches!(p, MessagePart::Tool { id, .. } if id == parent_tool_use_id));
            let sink_known = subagents.contains_key(parent_tool_use_id);
            // A Done with NO sink (a subagent that never streamed — codex
            // turn ends can beat registration) is chip-only: minting a doc
            // just to freeze it empty helps no one, and stamping the ref
            // would link the chip to that never-created doc (an empty tab
            // on click).
            let done_only = !sink_known && matches!(sub_event.as_ref(), AgentEvent::Done { .. });
            if chip_streaming {
                if !sink_known && !done_only {
                    for p in folded.iter_mut() {
                        if let MessagePart::Tool {
                            id,
                            call,
                            subagent_ref,
                            ..
                        } = p
                            && id == parent_tool_use_id
                            // Genus gate: a subagent ref may only ever bind
                            // to a SPAWN call — mis-keyed tagged traffic
                            // must not turn an ordinary chip into a spawn
                            // link (empty tab on click).
                            && call.is_subagent_spawn()
                        {
                            *subagent_ref = Some(sub_id.clone());
                        }
                    }
                }
                zeron_doc::fold_event_into_parts(&mut folded, &event);
                if !dirty {
                    dirty = true;
                    flush_at = tokio::time::Instant::now()
                        + std::time::Duration::from_millis(STREAM_COMMIT_MS);
                }
            }
            // Open the sink lazily; an open failure degrades to chip-only.
            if !sink_known && !done_only {
                let opened = inner.doc_host().and_then(|host| match host.open(&sub_id) {
                    Ok(handle) => Some(handle.writer()),
                    Err(err) => {
                        tracing::warn!(doc = %sub_id, error = %err, "subagent doc open failed (chip-only)");
                        None
                    }
                });
                if let Some(sub_doc) = opened {
                    subagents.insert(
                        parent_tool_use_id.clone(),
                        SubagentSink {
                            doc_id: sub_id.clone(),
                            doc: sub_doc,
                            entry_id: new_id(),
                            started_at: now_ms(),
                            entry_index: None,
                            written: Vec::new(),
                            folded: Vec::new(),
                            dirty: false,
                        },
                    );
                    if !chip_streaming {
                        let _ = doc_ref.update_subagent_chip(
                            parent_tool_use_id,
                            Some(&sub_id),
                            Some("running"),
                            None,
                        );
                    }
                }
            }
            let done = matches!(sub_event.as_ref(), AgentEvent::Done { .. });
            if done {
                settled_subagents.insert(parent_tool_use_id.clone());
            }
            if let Some(sink) = subagents.get_mut(parent_tool_use_id) {
                if let AgentEvent::UserMessage { text } = sub_event.as_ref() {
                    // A steer splits ENTRIES, not parts — handled at the
                    // sink level (the fold ignores UserMessage).
                    sink.push_user(&device_id, text);
                    continue;
                }
                let was_clean = !sink.dirty;
                zeron_doc::fold_event_into_parts(&mut sink.folded, sub_event);
                sink.dirty = true;
                // A sink waking on its own must arm the same commit window
                // the parent's dirty flag does — `flush_at` is otherwise only
                // rescheduled on the parent's flip, so a past deadline would
                // fire the commit branch on every event instead of once per
                // STREAM_COMMIT_MS window.
                if was_clean && !dirty && flush_at <= tokio::time::Instant::now() {
                    flush_at = tokio::time::Instant::now()
                        + std::time::Duration::from_millis(STREAM_COMMIT_MS);
                }
                if !chip_streaming && done {
                    // In-place chip refresh on lifecycle transitions only —
                    // content never rewrites the parent doc.
                    let _ = doc_ref.update_subagent_chip(
                        parent_tool_use_id,
                        None,
                        subagent_chip_update(sub_event),
                        None,
                    );
                }
                if done {
                    let status = match sub_event.as_ref() {
                        AgentEvent::Done {
                            status: DoneStatus::Errored,
                            ..
                        } => MessageStatus::Complete,
                        AgentEvent::Done {
                            status: DoneStatus::Interrupted,
                            ..
                        } => MessageStatus::Aborted,
                        _ => MessageStatus::Complete,
                    };
                    let sink = subagents.remove(parent_tool_use_id).expect("checked");
                    let doc_id = sink.doc_id.clone();
                    // FREEZE: the finished transcript uploads as a static R2
                    // blob (`blob/{chatId}/{subDocId}`) so viewers of
                    // finished subagents never wake the doc's room; dropping
                    // the sink unpins the doc for the LRU and the room
                    // idles. The live doc remains the fallback.
                    if let Some(json) = sink.finish(&device_id, status)
                        && let Some(host) = inner.doc_host()
                    {
                        host.upload_tool_sidecar(
                            &chat_id,
                            zeron_doc::SidecarPayload {
                                part_id: doc_id,
                                output: Some(json),
                                diff: None,
                            },
                        );
                    }
                }
            }
            continue;
        }

        // Any stream activity proves the run is alive — keep the session's
        // freshness inside the UI's 45s staleness window (throttled), and
        // push the quiesce watchdog's window out.
        inner.touch_session(&chat_id);
        last_stream_activity = tokio::time::Instant::now();
        // The engine's input bridge is the sole authority on input requests:
        // it mints the id and parks the resolver BEFORE emitting the event,
        // so a legitimate id is always pending here. A harness emitting its
        // own copy (a different id no resolver knows) would fold an
        // unanswerable twin chip into the doc — and answering the twin would
        // never resume the run. Dropped BEFORE the parked gate below: letting
        // it un-park the session and then dropping it would disarm the idle
        // reaper with no turn running (a leaked child no reap ever ends).
        if let AgentEvent::InputRequested { request_id, .. } = &event {
            let pending = lock(&inner.runs)
                .get(&chat_id)
                .map(|h| h.pending_inputs.clone());
            let known = pending.is_some_and(|p| lock(&p).contains_key(request_id));
            if !known {
                tracing::warn!(
                    chat = %chat_id,
                    request = %request_id,
                    "dropping harness-emitted InputRequested (unknown id; \
                     the engine input bridge owns this lifecycle)"
                );
                continue;
            }
        }
        // Capacity/occupancy can settle after Done; updating it must not reopen a turn.
        if let AgentEvent::ContextUsage { tokens, window } = &event {
            if let Err(err) = doc_ref.update_context_usage(*tokens, *window) {
                tracing::warn!(%chat_id, error = %err, "context usage write failed");
            }
            continue;
        }
        // PARKED: a steer boundary, a terminal Done, or SELF-CONTINUED OUTPUT
        // re-opens the session; everything else stays gated. The ACP child
        // keeps forwarding `session/update` frames after a turn completes,
        // and they split two ways:
        //
        // - Post-turn NOISE — late tool_call_updates for commands folded in a
        //   prior segment, command refreshes, reasoning heartbeats. Treating
        //   those as "the next turn" re-armed Working with no Done ever
        //   coming (the eternally-running session bug) and folded orphan
        //   parts into a phantom segment. Still dropped.
        // - SELF-CONTINUED WORK — Claude Code re-invokes itself when a
        //   background task finishes (turns no prompt started) and streams
        //   real output for them. Dropping those LOST transcript content
        //   (2026-08-12: "Build finished successfully…" streamed by the
        //   agent, absent from the doc). Fresh text or a genuinely new tool
        //   call resumes the session: new segment, Working, and the turn
        //   settles again via Done — or via the quiesce watchdog, which is
        //   what makes this resume safe where the naive version was not.
        //
        // The RESUME_GATE separates the two by arrival time: a finished
        // turn's tail flush lands within milliseconds of its Done, while a
        // self-continued turn starts a whole new agent round trip (seconds
        // at minimum, minutes in the incident). Inside the gate everything
        // non-boundary stays inert, exactly as before.
        let turn_was_active = idle_since.is_none();
        const RESUME_GATE: std::time::Duration = std::time::Duration::from_secs(1);
        if idle_since.is_some() {
            let self_continued = idle_since
                .is_some_and(|parked_at| parked_at.elapsed() >= RESUME_GATE)
                && (matches!(
                    &event,
                    AgentEvent::TextDelta { text } if !text.is_empty()
                ) || matches!(
                    &event,
                    AgentEvent::ToolCall { id, .. }
                        if id == zeron_proto::LIVE_PLAN_TOOL_ID || !seen_tools.contains(id)
                ));
            if self_continued {
                tracing::info!(
                    chat = %chat_id,
                    "parked session resumed by self-continued agent output"
                );
                idle_since = None;
                self_continued_turn = true;
                // The park cleared the fold; rotate to a fresh entry and
                // fall through — this event is the new segment's first part.
                entry_id = new_id();
                segment_started = now_ms();
                inner.set_status(&chat_id, SessionStatus::Working, true);
            } else {
                match &event {
                    AgentEvent::Steered { .. } => {
                        idle_since = None;
                        inner.set_status(&chat_id, SessionStatus::Working, true);
                    }
                    AgentEvent::Done { .. } => {
                        idle_since = None;
                    }
                    // A question with NO turn behind it (post-turn permission
                    // noise): answer it empty right here. Un-parking into
                    // AwaitingInput would disarm the idle reaper with no Done
                    // ever coming — stranded status, leaked warm child.
                    AgentEvent::InputRequested { request_id, .. } => {
                        let resolver = lock(&inner.runs)
                            .get(&chat_id)
                            .and_then(|h| lock(&h.pending_inputs).remove(request_id));
                        if let Some(tx) = resolver {
                            let _ = tx.send(Vec::new());
                        }
                        tracing::debug!(chat = %chat_id, "parked session: post-turn input request auto-declined");
                        continue;
                    }
                    // A stale answer settling after its turn already closed:
                    // nothing is running — stay parked.
                    AgentEvent::InputResolved { .. } => continue,
                    _ => continue,
                }
            }
        }
        // Empty reasoning deltas are PURE heartbeats: redacted thinking and
        // tool-input-generation windows stream them with no text. They fold
        // to nothing, so journaling/publishing them is only noise (hundreds
        // per long turn observed) — the touch above already did their job.
        if matches!(&event, AgentEvent::ReasoningDelta { text } if text.is_empty()) {
            continue;
        }

        // Stale tool echoes: a ToolCall/ToolResult naming an id folded in a
        // PRIOR segment (the fold reset at a steer or park since) belongs to
        // a chip that already rendered in its own entry. Folding the echo
        // would splice a phantom chip into the middle of the current
        // segment's streaming text (the mid-word transcript splits). Dropped;
        // same-segment refreshes still land in place.
        let in_segment = |folded: &[MessagePart], id: &str| {
            folded
                .iter()
                .any(|p| matches!(p, MessagePart::Tool { id: pid, .. } if pid == id))
        };
        match &event {
            // The live plan chip is a deliberate singleton (every update
            // reuses `LIVE_PLAN_TOOL_ID` so the fold refreshes in place):
            // treating its reappearance after a park/steer reset as a stale
            // echo dropped the todo list for the rest of the run — from the
            // first boundary on, plans never rendered again.
            AgentEvent::ToolCall { id, .. } if id == zeron_proto::LIVE_PLAN_TOOL_ID => {}
            AgentEvent::ToolResult { id, .. } if id == zeron_proto::LIVE_PLAN_TOOL_ID => {}
            AgentEvent::ToolCall { id, .. } => {
                if !in_segment(&folded, id) && seen_tools.contains(id) {
                    continue;
                }
                seen_tools.insert(id.clone());
            }
            AgentEvent::ToolResult { id, .. }
                if !in_segment(&folded, id) && seen_tools.contains(id) =>
            {
                continue;
            }
            _ => {}
        }

        // Startup-crash retry: a run that dies before ever starting (errored
        // Done, no SessionStarted, nothing streamed) means the AGENT CHILD
        // failed to come up — not that the injected resume id was bad. Since
        // the ACP conversion (2026-08-08) a stale id is handled inside the
        // harness (`session/load` falls back to `session/new`), so the old
        // guess here — tombstone the id, retry fresh — fired only on child
        // startup failures and permanently severed GOOD conversations (user
        // incident 2026-08-13). The id stays; retry ONCE against the same
        // user entry, resume and all, in case the crash was transient. A
        // helper that is down hard fails the retry too and surfaces its
        // crash text (the harness now appends exit status + stderr).
        if resume_state.resume_injected
            && !resume_state.startup_retry
            && !saw_session_started
            && folded.is_empty()
            && !interrupted
            && matches!(
                &event,
                AgentEvent::Done {
                    status: DoneStatus::Errored,
                    ..
                }
            )
            && let Some(retry) = retry_request.take()
        {
            tracing::warn!(
                chat = %chat_id,
                "run died before session start; retrying once (resume kept)"
            );
            inner.remove_run(&chat_id, &run_id);
            let engine = SessionsEngine {
                inner: inner.clone(),
            };
            let chat = chat_id.clone();
            let message_id = resume_state.user_message_id.clone();
            tokio::spawn(async move {
                // The user entry write inside dispatch is idempotent by
                // message id; `startup_retry` makes this attempt final.
                if let Err(err) = engine
                    .dispatch_with(&chat, harness_id, retry, Some(message_id), true)
                    .await
                {
                    tracing::error!(chat = %chat, error = %err, "startup-crash retry dispatch failed");
                    // No run is coming: leaving the row Working would spin
                    // the session forever with nothing behind it.
                    engine
                        .inner
                        .set_status(&chat, SessionStatus::Errored, false);
                }
            });
            return;
        }

        // A steer boundary splits the assistant entry exactly where the fold resets.
        if let AgentEvent::Steered {
            next_assistant_message_id,
            ..
        } = &event
        {
            inner.publish(&chat_id, &event);
            // A steer boundary means a real prompt owns the turn again — its
            // Done will come; the short self-continued window stands down.
            self_continued_turn = false;
            if let Err(err) = finish_segment(
                doc_ref,
                writer.take(),
                &entry_id,
                &device_id,
                segment_started,
                &folded,
                MessageStatus::Complete,
            ) {
                tracing::warn!(chat = %chat_id, error = %err, "segment finish failed");
            }
            inner.note_message(&chat_id, &folded_text(&folded));
            folded.clear();
            dirty = false;
            entry_id = next_assistant_message_id.clone().unwrap_or_else(new_id);
            segment_started = now_ms();
            // The elapsed timer is per user message, not per child process: a
            // steer boundary restarts it (matches the parked-resume path and
            // the composer's optimistic overlay, which already reads 0:00).
            inner.set_status(&chat_id, SessionStatus::Working, true);
            // The boundary confirms delivery of the oldest accepted steer —
            // retire its at-least-once ledger entry.
            if let Some(h) = lock(&inner.runs)
                .get(&chat_id)
                .filter(|h| h.run_id == run_id)
            {
                lock(&h.routed_steers).pop_front();
            }
            continue;
        }

        match &event {
            AgentEvent::SessionStarted {
                session_id, cwd, ..
            } => {
                saw_session_started = true;
                // The event's own cwd (where the harness actually created the
                // session) scopes the stored id, not the request's.
                inner.remember_harness_session(&chat_id, session_id, cwd);
            }
            AgentEvent::Done {
                session_id: Some(session_id),
                ..
            } => {
                inner.remember_harness_session(&chat_id, session_id, &run_cwd);
            }
            AgentEvent::InputRequested { .. } => {
                // Known-id guaranteed: the unknown-id twin was dropped above,
                // before the parked gate.
                inner.set_status(&chat_id, SessionStatus::AwaitingInput, false);
            }
            AgentEvent::InputResolved { .. } => {
                inner.set_status(&chat_id, SessionStatus::Working, false);
            }
            _ => {}
        }

        inner.publish(&chat_id, &event);

        // Defensive rule from zeron: a mid-run SessionStarted re-emission (Claude SDK
        // background re-invocations) must not wipe the segment being written.
        let skip_fold = matches!(&event, AgentEvent::SessionStarted { .. }) && !folded.is_empty();
        if !skip_fold {
            fold_event_into_parts(&mut folded, &event);
            // R2 sidecar PARKED (2026-08-10, product call): the fold's
            // summary/stats ARE the doc's whole record — no refs stamped, no
            // uploads. Full outputs survive only in the host's local run
            // journal. To reintroduce: `zeron_doc::sidecar_payload(&event)`
            // → `apply_sidecar_refs` → `doc_host.upload_tool_sidecar`, all
            // still in place and tested.
        }

        if let AgentEvent::Done { status, .. } = &event {
            // A question still pending at turn end can never be legitimately
            // answered (its turn is over): drain the resolvers NOW, or a late
            // `respond_input` finds one, emits InputResolved, and un-parks
            // the session into Working with no turn behind it — stranded
            // Working, timer forever, reaper disarmed. Empty answers unblock
            // the harness-side bridge like an interrupt does.
            let pending = lock(&inner.runs)
                .get(&chat_id)
                .filter(|h| h.run_id == run_id)
                .map(|h| h.pending_inputs.clone());
            if let Some(pending) = pending {
                for (_, tx) in lock(&pending).drain() {
                    let _ = tx.send(Vec::new());
                }
            }
            let message_status = match status {
                DoneStatus::Interrupted => MessageStatus::Aborted,
                DoneStatus::Completed | DoneStatus::Errored => MessageStatus::Complete,
            };
            // No dangling chips: a run that ends for ANY reason (completed,
            // errored, interrupted) terminally resolves its input parts — an
            // unresolved question must not outlive the run that asked it
            // (its resolver died with the run; an answer could never land).
            for part in folded.iter_mut() {
                if let MessagePart::Input { resolved, .. } = part {
                    *resolved = true;
                }
            }
            // A Done landing on a PARKED session with nothing streamed (the
            // idle reaper's or an interrupt's own teardown) has no entry to
            // finalize — writing one would leave an empty aborted stub.
            let nothing_streamed = writer.is_none() && folded.is_empty();
            if !nothing_streamed {
                if let Err(err) = finish_segment(
                    doc_ref,
                    writer.take(),
                    &entry_id,
                    &device_id,
                    segment_started,
                    &folded,
                    message_status,
                ) {
                    tracing::warn!(chat = %chat_id, error = %err, "final segment finish failed");
                }
                inner.note_message(&chat_id, &folded_text(&folded));
            }
            if *status == DoneStatus::Completed {
                // A cleanly completed turn resets the auto-resume revival
                // budget: only consecutive crash-revive-crash cycles spend it.
                inner.journal.clear_resume_attempts(&chat_id);
            }
            // Exchange completed on an untitled chat → name it (fire-and-forget;
            // interrupted/errored turns never trigger naming).
            if *status == DoneStatus::Completed
                && let Some(titles) = inner.titles.get()
            {
                titles.maybe_generate(&chat_id, harness_id, &user_prompt, &run_cwd);
            }
            // An accepted steer awaiting its boundary owns the continuation:
            // the previous Done is an internal handoff, not a completion ping.
            // Ordinary queued rows are not in this ledger and still notify.
            let pending_steer = inner.has_pending_steers(&chat_id, &run_id);
            let completed_turn = (*status == DoneStatus::Completed
                && !interrupted
                && turn_was_active
                && !pending_steer)
                .then(|| entry_id.clone());
            // PERSISTENT SESSION: a cleanly completed turn on a steerable
            // harness PARKS instead of ending — child + mailbox stay warm for
            // the next routed dispatch; per-turn state resets for it.
            if *status == DoneStatus::Completed && steerable && !interrupted {
                folded.clear();
                dirty = false;
                entry_id = new_id();
                segment_started = now_ms();
                // Resume-retry is strictly a first-turn concern.
                saw_session_started = true;
                idle_since = Some(tokio::time::Instant::now());
                self_continued_turn = false;
                inner.set_status_with_completion(
                    &chat_id,
                    SessionStatus::Idle,
                    false,
                    completed_turn,
                );
                continue;
            }
            final_completed_turn = completed_turn;
            break match status {
                DoneStatus::Errored => SessionStatus::Errored,
                _ => SessionStatus::Idle,
            };
        }

        if !folded.is_empty() && !dirty {
            dirty = true;
            flush_at =
                tokio::time::Instant::now() + std::time::Duration::from_millis(STREAM_COMMIT_MS);
        }
    };

    // Any subagent still streaming when the run ends freezes as-is: the
    // parent process is gone, so nothing more can arrive on this stream.
    for (parent_id, sink) in subagents.drain() {
        let doc_id = sink.doc_id.clone();
        let _ = doc_ref.update_subagent_chip(&parent_id, None, Some("failed"), None);
        if let Some(json) = sink.finish(&device_id, MessageStatus::Aborted)
            && let Some(host) = inner.doc_host()
        {
            host.upload_tool_sidecar(
                &chat_id,
                zeron_doc::SidecarPayload {
                    part_id: doc_id,
                    output: Some(json),
                    diff: None,
                },
            );
        }
    }

    // Claim any accepted-but-unconfirmed steers BEFORE the handle goes away:
    // a routed send that raced this exit either finds its entry gone (we own
    // it — re-dispatched below) or reclaims it and starts a fresh run itself.
    let orphans: Vec<RoutedSteer> = lock(&inner.runs)
        .get(&chat_id)
        .filter(|h| h.run_id == run_id)
        .map(|h| std::mem::take(&mut *lock(&h.routed_steers)).into())
        .unwrap_or_default();
    inner.remove_run(&chat_id, &run_id);
    inner.set_status_with_completion(&chat_id, final_status, false, final_completed_turn);
    if !interrupted && !orphans.is_empty() {
        // The dying run accepted these into its mailbox but never confirmed a
        // Steered boundary (idle-reaper race, a mid-turn error discarding
        // queued boundary steers, a parked child death). Their user entries
        // are already in the transcript — a message that shows as sent must
        // never silently not run. Re-dispatch each as a fresh turn
        // (write_user_message dedupes by id; resume is engine-injected).
        let engine = SessionsEngine {
            inner: inner.clone(),
        };
        let chat = chat_id.clone();
        tokio::spawn(async move {
            for steer in orphans {
                let Some(mut request) = engine.last_request(&chat) else {
                    tracing::warn!(chat = %chat, "orphaned steer lost: no run config to re-dispatch");
                    break;
                };
                request.prompt = steer.prompt.clone();
                request.resume = None;
                request.attachments = Vec::new();
                tracing::info!(chat = %chat, "re-dispatching steer orphaned by a dying run");
                let Some(host) = engine.inner.doc_host() else {
                    tracing::warn!(chat = %chat, "orphaned steer lost: doc host unavailable");
                    break;
                };
                if let Err(err) = host
                    .dispatch_with_source_context(
                        &engine,
                        &chat,
                        harness_id,
                        request,
                        Some(steer.message_id.clone()),
                    )
                    .await
                {
                    tracing::warn!(chat = %chat, error = %err, "orphaned steer re-dispatch failed");
                    engine
                        .inner
                        .set_status(&chat, SessionStatus::Errored, false);
                    break;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use futures::stream::BoxStream;
    use zeron_doc::{SessionCommandEntry, SessionCommandPayload, SessionCommandStatus};
    use zeron_harness::HarnessError;
    use zeron_proto::{Model, ReasoningLevel, SteeringMode};

    struct SpawnCounter(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl Harness for SpawnCounter {
        fn id(&self) -> HarnessId {
            HarnessId::ClaudeCode
        }

        fn display_name(&self) -> &str {
            "Spawn counter"
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
            Ok(Vec::new())
        }

        async fn run(
            &self,
            _request: RunRequest,
            _controls: RunControls,
        ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
            self.0.fetch_add(1, Ordering::AcqRel);
            Ok(futures::stream::iter([Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            })])
            .boxed())
        }
    }

    fn cloud_run_candidate(bytes: u64, cursor: Option<&str>) -> zeron_proto::CloudRun {
        let now = Utc::now();
        zeron_proto::CloudRun {
            id: "run-1".into(),
            chat_id: "chat-1".into(),
            seat_id: "seat-1".into(),
            command: None,
            status: zeron_proto::CloudRunStatus::Working,
            log_path: None,
            remote_ref: None,
            mirrored_bytes: bytes,
            remote_cursor: cursor.map(str::to_owned),
            entry_id: None,
            error: None,
            workspace: None,
            harness: None,
            model: None,
            harness_session: None,
            prompt: None,
            started_at: now,
            finished_at: None,
            updated_at: now,
        }
    }

    #[test]
    fn cloud_run_recovery_prefers_greater_byte_and_cursor_progress() {
        let mut candidates = HashMap::new();
        insert_cloud_run_candidate(&mut candidates, cloud_run_candidate(99, Some("9")));
        insert_cloud_run_candidate(&mut candidates, cloud_run_candidate(100, Some("2")));
        assert_eq!(
            candidates["chat-1"].mirrored_bytes, 100,
            "greater mirrored byte progress wins"
        );

        insert_cloud_run_candidate(&mut candidates, cloud_run_candidate(100, Some("10")));
        assert_eq!(
            candidates["chat-1"].remote_cursor.as_deref(),
            Some("10"),
            "numeric cursors compare by progress rather than lexically"
        );
    }

    #[tokio::test]
    async fn runner_bound_turn_without_sealed_key_fails_in_transcript_without_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let profile = crate::profile::EngineProfile::development(dir.path(), "org", "user");
        let store_root = profile.store_root().to_path_buf();
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(SpawnCounter(spawn_count.clone())));
        let core = crate::EngineCore::assemble_with_profile(
            profile,
            registry,
            HarnessId::ClaudeCode,
            None,
        )
        .unwrap();
        let binding = zeron_proto::CloudSeatBinding {
            seat_id: "runner-seat".into(),
            kind: zeron_proto::CloudSeatKind::Ssh,
            label: "Runner".into(),
            repo: None,
            workspace_path: Some(dir.path().join("runner-workspace").display().to_string()),
            runner_device_id: Some(core.device_id.clone()),
            branch: None,
            model_label: None,
            expires_at: None,
        };
        core.workspace
            .create_chat_with_parent(
                "runner-chat",
                None,
                Some(&core.device_id),
                None,
                Some("/viewport/cwd".into()),
                None,
                Some(binding),
            )
            .unwrap();
        let mut run = request();
        run.harness = Some(HarnessId::ClaudeCode);
        run.model = None;
        run.cwd = "/viewport/cwd".into();

        core.sessions
            .dispatch(
                "runner-chat",
                HarnessId::ClaudeCode,
                run,
                Some("user-1".into()),
            )
            .await
            .unwrap();

        assert_eq!(spawn_count.load(Ordering::Acquire), 0);
        assert!(!core.sessions.is_running("runner-chat"));
        assert_eq!(
            core.sessions.session_status("runner-chat").unwrap().status,
            SessionStatus::Errored
        );
        let handle = core.doc_host.open("runner-chat").unwrap();
        let entries = handle.doc().read_entries().unwrap();
        assert!(entries.iter().any(|entry| {
            entry
                .parts
                .iter()
                .any(|part| matches!(part, MessagePart::Text { text, .. } if text.contains("No key sealed for this runner")))
        }));
        assert!(core.cloud.as_ref().unwrap().runs().is_empty());
        assert!(!store_root.join("cloud-runs.json").exists());
        core.shutdown().await;
    }

    #[tokio::test]
    async fn runner_bound_stale_turn_is_not_resumed_and_has_runner_note() {
        let dir = tempfile::tempdir().unwrap();
        let profile = crate::profile::EngineProfile::development(dir.path(), "org", "user");
        let spawn_count = Arc::new(AtomicUsize::new(0));
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(SpawnCounter(spawn_count.clone())));
        let core = crate::EngineCore::assemble_with_profile(
            profile,
            registry,
            HarnessId::ClaudeCode,
            None,
        )
        .unwrap();
        let binding = zeron_proto::CloudSeatBinding {
            seat_id: "runner-seat".into(),
            kind: zeron_proto::CloudSeatKind::Ssh,
            label: "Runner".into(),
            repo: None,
            workspace_path: Some("/runner/workspace".into()),
            runner_device_id: Some(core.device_id.clone()),
            branch: None,
            model_label: None,
            expires_at: None,
        };
        core.workspace
            .create_chat_with_parent(
                "runner-stale",
                None,
                Some(&core.device_id),
                None,
                Some("/runner/workspace".into()),
                None,
                Some(binding),
            )
            .unwrap();
        let handle = core.doc_host.open("runner-stale").unwrap();
        handle
            .doc()
            .queue_command(&SessionCommandEntry {
                id: "runner-stale-command".into(),
                payload: SessionCommandPayload::Run {
                    request: request(),
                    message_id: "runner-stale-user".into(),
                },
                issued_by: "viewport".into(),
                issued_at: now_ms(),
                based_on: None,
                expires_at: None,
                status: SessionCommandStatus::Applied,
                resolution: None,
            })
            .unwrap();
        handle
            .doc()
            .push_message(&SessionMessageEntry {
                id: "runner-stale-assistant".into(),
                role: MessageRole::Assistant,
                parts: vec![MessagePart::Text {
                    id: "t0".into(),
                    text: "partial response".into(),
                }],
                created_at: now_ms(),
                device_id: core.device_id.clone(),
                status: Some(MessageStatus::Streaming),
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
        core.sessions
            .inner
            .journal
            .append(
                "runner-stale",
                &AgentEvent::TextDelta {
                    text: "partial response".into(),
                },
            )
            .unwrap();

        assert_eq!(core.sessions.recover_stale().unwrap(), 1);

        assert_eq!(spawn_count.load(Ordering::Acquire), 0);
        let entries = handle.doc().read_entries().unwrap();
        let assistant = entries
            .iter()
            .find(|entry| entry.id == "runner-stale-assistant")
            .unwrap();
        assert_eq!(assistant.status, Some(MessageStatus::Aborted));
        assert!(assistant.parts.iter().any(|part| {
            matches!(part, MessagePart::Error { message, .. } if message == "Runner restarted mid-turn — send your message again to continue")
        }));
        assert_eq!(
            core.sessions.inner.journal.resume_attempts("runner-stale"),
            0
        );
        assert_eq!(
            handle
                .doc()
                .read_commands()
                .unwrap()
                .into_iter()
                .find(|command| command.id == "runner-stale-command")
                .unwrap()
                .status,
            SessionCommandStatus::Applied
        );
        core.shutdown().await;
    }

    #[tokio::test]
    async fn runner_bound_cloud_runs_are_not_reattached_on_a_viewport() {
        let dir = tempfile::tempdir().unwrap();
        let profile = crate::profile::EngineProfile::development(dir.path(), "org", "user");
        let core = crate::EngineCore::assemble_with_profile(
            profile,
            Arc::new(crate::default_registry()),
            HarnessId::Mock,
            None,
        )
        .unwrap();
        let binding = zeron_proto::CloudSeatBinding {
            seat_id: "runner-seat".into(),
            kind: zeron_proto::CloudSeatKind::Ssh,
            label: "Runner".into(),
            repo: None,
            workspace_path: Some("/runner/workspace".into()),
            runner_device_id: Some("other-device".into()),
            branch: None,
            model_label: None,
            expires_at: None,
        };
        core.workspace
            .create_chat_with_parent(
                "runner-chat",
                None,
                Some("other-device"),
                None,
                binding.workspace_path.clone(),
                None,
                Some(binding),
            )
            .unwrap();
        let cloud = core.cloud.as_ref().unwrap();
        let mut run = cloud_run_candidate(24, Some("4"));
        run.chat_id = "runner-chat".into();
        run.seat_id = "runner-seat".into();
        cloud.upsert_run(run);

        core.sessions.resume_cloud_runs();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(!core.sessions.is_running("runner-chat"));
        assert_eq!(
            cloud.run_for_chat("runner-chat").unwrap().status,
            zeron_proto::CloudRunStatus::Working
        );
        core.shutdown().await;
    }
    fn remote_tail_frame(run_id: &str, bytes: &[u8], metadata: &str) -> String {
        use base64::Engine as _;

        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        format!(
            "__cosmos_tail_begin_{run_id}__\n{encoded}\n__cosmos_tail_end_{run_id}__\n{metadata}\n"
        )
    }

    #[test]
    fn remote_log_ansi_stripper_removes_csi_and_osc_sequences() {
        let mut stripper = AnsiStripper::default();
        assert_eq!(
            stripper.push(b"\x1b[0mWrite hello.txt\x1b]0;remote title\x07!"),
            b"Write hello.txt!"
        );
    }

    #[test]
    fn remote_log_ansi_stripper_handles_sequences_split_across_chunks() {
        let mut stripper = AnsiStripper::default();
        assert_eq!(stripper.push(b"\x1b[0"), b"");
        assert_eq!(stripper.push(b"mHello\x1b]0;title\x1b"), b"Hello");
        assert_eq!(stripper.push(b"\\!"), b"!");
    }

    #[test]
    fn remote_tail_offset_preserves_first_character_after_partial_poll() {
        let run_id = "offset-test";
        let first_chunk = b"\x1b[0mworking";
        let first_stdout = remote_tail_frame(run_id, first_chunk, "__alive_0");
        let (decoded, alive, exit) = parse_remote_tail_output(run_id, &first_stdout).unwrap();
        assert_eq!(decoded, first_chunk);
        assert!(alive);
        assert_eq!(exit, None);

        let old_reconstructed_offset = String::from_utf8_lossy(first_chunk)
            .lines()
            .map(|line| line.len() + 1)
            .sum::<usize>();
        let appended = b"Hello!";
        let mut full_log = first_chunk.to_vec();
        full_log.extend_from_slice(appended);
        assert_eq!(&full_log[old_reconstructed_offset..], b"ello!");

        let next_stdout = remote_tail_frame(run_id, &full_log[decoded.len()..], "__exit_0");
        let (next, _, exit) = parse_remote_tail_output(run_id, &next_stdout).unwrap();
        assert_eq!(next, appended);
        assert_eq!(exit, Some(0));
    }

    #[test]
    fn cursor_recovery_converts_rich_messages_before_json_encoding() {
        let doc = zeron_doc::SessionDoc::init("cursor-rich-recovery").unwrap();
        let skill = zeron_proto::invocation::Invocation::Skill {
            name: "review \"quoted\"".into(),
            path: "/repo/quoted \"path\"/SKILL.md".into(),
            command: None,
        }
        .link();
        let previous = format!("Previous {skill}\nSecond line with \\ and \"quotes\"");
        doc.push_message(&zeron_doc::SessionMessageEntry {
            id: "u1".into(),
            role: zeron_doc::MessageRole::User,
            parts: vec![zeron_doc::MessagePart::Text {
                id: "u1-text".into(),
                text: previous.clone(),
            }],
            created_at: 0,
            device_id: "test".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        })
        .unwrap();
        let current = format!("Current {skill}\nKeep **Markdown**");
        let delivered = super::cursor_unstarted_history(&doc, "u2", &current, false).unwrap();
        let (_, json) = delivered.split_once('\n').unwrap();
        let parsed: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed["currentUserMessage"],
            zeron_proto::invocation::harness_prompt(&current, zeron_proto::HarnessId::Cursor)
        );
        assert_eq!(
            parsed["previousUserMessages"][0],
            zeron_proto::invocation::harness_prompt(&previous, zeron_proto::HarnessId::Cursor)
        );
    }

    #[test]
    fn cursor_without_a_session_id_retains_only_preceding_user_messages() {
        let doc = zeron_doc::SessionDoc::init("cursor-unstarted").unwrap();
        for (id, role, text) in [
            (
                "u1",
                zeron_doc::MessageRole::User,
                "first interrupted request",
            ),
            ("a1", zeron_doc::MessageRole::Assistant, "partial output"),
            ("u2", zeron_doc::MessageRole::User, "current request"),
            ("u3", zeron_doc::MessageRole::User, "future pending request"),
        ] {
            doc.push_message(&zeron_doc::SessionMessageEntry {
                id: id.into(),
                role,
                parts: vec![zeron_doc::MessagePart::Text {
                    id: format!("{id}-text"),
                    text: text.into(),
                }],
                created_at: 0,
                device_id: "test".into(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
        }
        assert_eq!(
            super::cursor_unstarted_history(&doc, "u1", "first interrupted request", false)
                .unwrap(),
            "first interrupted request"
        );
        let prompt = super::cursor_unstarted_history(&doc, "u2", "current request", false).unwrap();
        assert!(prompt.contains("first interrupted request"));
        assert!(prompt.contains("current request"));
        assert!(!prompt.contains("partial output"));
        assert!(!prompt.contains("future pending request"));
        assert!(prompt.contains("do not rerun prior tools or side effects"));
        assert_eq!(
            super::cursor_unstarted_history(&doc, "u2", "current request", true).unwrap(),
            "current request",
            "content from the preceding turn is already in the native session"
        );
        assert!(
            super::cursor_unstarted_history(&doc, "u3", "future pending request", true)
                .unwrap()
                .contains("current request"),
            "an unacknowledged prompt after an older checkpoint must survive"
        );
        assert_eq!(doc.read_entries().unwrap().len(), 4);
    }

    use zeron_proto::{HarnessId, RunRequest, SandboxLevel};

    #[tokio::test]
    async fn generated_image_failure_is_sanitized_even_inside_subagents() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let sessions = SessionsEngine::new(
            "host".into(),
            Arc::new(RunJournal::open(dir.path().join("journals")).unwrap()),
            Arc::new(HarnessRegistry::new()),
        );
        let source = "/outside/private/source.png";
        let event = AgentEvent::Subagent {
            parent_tool_use_id: "child".into(),
            event: Box::new(AgentEvent::GeneratedImage {
                id: "i:image".into(),
                path: source.into(),
                name: "secret".into(),
                mime_type: "bad".into(),
            }),
        };
        let events = sessions
            .inner
            .prepare_generated_image("chat", event, &mut std::collections::HashSet::new())
            .await;
        assert_eq!(events.len(), 2);
        let mut parts = vec![];
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ToolCall {
                id: "i".into(),
                call: zeron_proto::ToolCall::Unknown {
                    name: "Generate image".into(),
                    input: None,
                },
            },
        );
        for event in &events {
            sessions.inner.publish("chat", event);
            let AgentEvent::Subagent {
                parent_tool_use_id,
                event,
            } = event
            else {
                panic!("lost owner")
            };
            assert_eq!(parent_tool_use_id, "child");
            fold_event_into_parts(&mut parts, event);
        }
        assert!(matches!(
            parts[0],
            MessagePart::Tool {
                is_error: true,
                resolved: true,
                ..
            }
        ));
        assert!(matches!(parts[1], MessagePart::Error { .. }));
        let journal =
            serde_json::to_string(&sessions.inner.journal.replay("chat", 0).unwrap()).unwrap();
        assert!(!journal.contains(source));
        assert!(!journal.contains("secret"));
    }

    fn request() -> RunRequest {
        RunRequest {
            prompt: "first".into(),
            harness: None,
            model: Some("grok-4.6".into()),
            reasoning: Some(zeron_proto::ReasoningLevel::High),
            model_options: serde_json::Map::new(),
            cwd: "/tmp".into(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            resume: None,
            attachments: Vec::new(),
            worktree: None,
        }
    }

    #[test]
    fn live_routing_requires_the_same_runtime_configuration() {
        let initial = request();
        let config = RuntimeConfig::from_request(HarnessId::Grok, &initial);

        let mut follow_up = initial.clone();
        follow_up.prompt = "second".into();
        follow_up.resume = Some("session-1".into());
        assert!(config.can_route(HarnessId::Grok, &follow_up));

        follow_up.model = Some("grok-4.5".into());
        assert!(!config.can_route(HarnessId::Grok, &follow_up));
        follow_up.model = initial.model.clone();

        follow_up.reasoning = Some(zeron_proto::ReasoningLevel::Medium);
        assert!(!config.can_route(HarnessId::Grok, &follow_up));
        follow_up.reasoning = initial.reasoning;

        follow_up.attachments.push("/tmp/image.png".into());
        assert!(!config.can_route(HarnessId::Grok, &follow_up));
    }

    #[test]
    fn clean_tool_ids_ride_verbatim_and_fit_id_re() {
        let id = subagent_doc_id("chat-abc123", "toolu_01EjFLnNhCiMR2PBNKcVvkT");
        assert_eq!(id, "chat-abc123--sub--toolu_01EjFLnNhCiMR2PBNKcVvkT");
        assert!(id.len() <= 128);
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
    }

    #[test]
    fn unclean_or_oversized_ids_hash_deterministically() {
        let dirty = subagent_doc_id("chat", "call/with:odd chars");
        assert!(
            dirty
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "{dirty}"
        );
        assert_eq!(dirty, subagent_doc_id("chat", "call/with:odd chars"));
        let long_chat = "c".repeat(100);
        let long = subagent_doc_id(&long_chat, &"t".repeat(64));
        assert!(long.len() <= 128, "{}", long.len());
        // Distinct inputs stay distinct.
        assert_ne!(
            subagent_doc_id("chat", "a:b"),
            subagent_doc_id("chat", "a:c")
        );
    }

    #[tokio::test]
    async fn subagent_sink_flush_clears_dirty_when_nothing_folded() {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(zeron_sync::DocsStore::open(dir.path()).unwrap());
        let host = DocHost::new(
            store,
            crate::doc_host::DocHostConfig {
                device_id: "dev-test".into(),
                default_harness: HarnessId::Mock,
                edge: None,
            },
        );
        let handle = host.open("chat-sub-flush--sub--t1").unwrap();
        let mut sink = SubagentSink {
            doc_id: "chat-sub-flush--sub--t1".into(),
            doc: handle.writer(),
            entry_id: "entry-1".into(),
            started_at: 0,
            entry_index: None,
            written: Vec::new(),
            folded: Vec::new(),
            dirty: true,
        };
        // Nothing to write is still a completed flush — left set, the flag
        // holds the run loop's commit guard open on a past deadline.
        sink.flush("dev-test");
        assert!(!sink.dirty);
    }

    /// Feed-by-hand harness: the test pushes wire events through a channel
    /// and decides when the stream ends.
    struct FeedHarness {
        feed: Mutex<Option<mpsc::UnboundedReceiver<AgentEvent>>>,
    }

    #[async_trait::async_trait]
    impl Harness for FeedHarness {
        fn id(&self) -> HarnessId {
            HarnessId::Mock
        }
        fn display_name(&self) -> &str {
            "Feed"
        }
        fn supports_steering(&self) -> bool {
            true
        }
        fn steering_mode(&self) -> zeron_proto::SteeringMode {
            zeron_proto::SteeringMode::StepBoundary
        }
        fn reasoning_levels(&self) -> &[zeron_proto::ReasoningLevel] {
            &[zeron_proto::ReasoningLevel::Medium]
        }
        async fn models(&self) -> Result<Vec<zeron_proto::Model>, zeron_harness::HarnessError> {
            Ok(vec![])
        }
        async fn run(
            &self,
            _request: RunRequest,
            _controls: RunControls,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<AgentEvent, zeron_harness::HarnessError>>,
            zeron_harness::HarnessError,
        > {
            let mut feed = self
                .feed
                .lock()
                .unwrap()
                .take()
                .expect("FeedHarness serves one run per test");
            Ok(futures::stream::poll_fn(move |cx| feed.poll_recv(cx).map(|e| e.map(Ok))).boxed())
        }
    }

    // A tagged subagent event that folds to NO parts used to leave its sink
    // dirty forever: the commit branch's guard stayed true on a deadline in
    // the past and the run loop burned a core until real content arrived.
    #[tokio::test]
    async fn empty_subagent_events_commit_once_per_window_not_per_tick() {
        use super::*;
        FLUSH_TICKS.with(|t| t.set(0));
        let (feed, rx) = mpsc::unbounded_channel();
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(FeedHarness {
            feed: Mutex::new(Some(rx)),
        }));
        let dir = tempfile::tempdir().unwrap();
        let core =
            crate::EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
                .unwrap();
        let chat = "chat-sink-spin";
        core.sessions
            .dispatch(chat, HarnessId::Mock, request(), None)
            .await
            .unwrap();
        feed.send(AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: "mock-1".into(),
            tools: vec![],
            cwd: "/tmp".into(),
            session_id: "hs-1".into(),
            assistant_message_id: "a1".into(),
        })
        .unwrap();
        feed.send(AgentEvent::Subagent {
            parent_tool_use_id: "tool-1".into(),
            event: Box::new(AgentEvent::ReasoningDelta {
                text: String::new(),
            }),
        })
        .unwrap();
        // Well past several commit windows with the stream still open: the
        // loop must idle between commits, not spin on a past deadline.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let ticks = FLUSH_TICKS.with(|t| t.get());
        assert!(
            (1..=8).contains(&ticks),
            "commit branch fired {ticks} times in 1s for one empty subagent event"
        );
        assert_eq!(
            core.sessions.session_status(chat).map(|s| s.status),
            Some(SessionStatus::Working),
            "stream still open, turn still running"
        );
        feed.send(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        })
        .unwrap();
        core.sessions.shutdown().await;
    }
}
