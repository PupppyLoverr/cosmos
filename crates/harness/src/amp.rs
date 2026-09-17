//! Amp harness (`amp`): execute mode over the Claude-Code-compatible
//! stream-json wire — `amp --execute --stream-json --stream-json-input`
//! (docs: ampcode.com/docs/cli/streaming-json). The schema is close enough to
//! reuse [`crate::claude::wire`] / [`normalize::Normalizer`] wholesale; the
//! differences live here:
//!
//! - INPUT frames carry a block array (Amp rejects Claude's string shorthand)
//!   and mid-turn steering rides a top-level `"steer": true` flag.
//! - SELECTION is the Dial — `low`/`medium`/`high`/`ultra` modes — not a model
//!   id. Each mode is its own model+effort+prompt+tools bundle; the catalog
//!   exposes the four modes as "models" (`--mode <id>` selects them, matching
//!   the SDK's `mode` option which maps to the CLI flag), and the effort
//!   ladder rides `--effort` (none/minimal/low/medium/high/xhigh/max —
//!   mapped onto [`ReasoningLevel`]). Amp publishes no stable model list to
//!   pin against; the modes page (ampcode.com/modes) is a routing table that
//!   changes under the stable mode names.
//! - RESUME is `amp threads continue <id> --execute …` (thread ids are
//!   `T-…`, carried on `session_id`).
//! - INTERRUPT has no wire-level request; cancel escalates straight to the
//!   child kill.
//! - Permissions ride `--dangerously-allow-all` for `auto_approve`; denied
//!   calls surface in `result.permission_denials` (handled by the shared
//!   normalizer).

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use crate::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use tokio::sync::mpsc;

use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
};

use crate::claude::normalize::Normalizer;
use crate::claude::wire::{self, Frame};
use crate::{Harness, HarnessError, RunControls, Signal, send_signal, shutdown_child};

/// `ZERON_AMP_EXECUTABLE` first, then PATH/login-shell/node-manager dirs via
/// the shared resolver (Amp installs through npm — `.cmd` shims on Windows —
/// and via its own installer into `~/.local/bin` / `~/.amp`).
fn resolve_amp_executable() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZERON_AMP_EXECUTABLE")
        && !p.is_empty()
    {
        return Some(PathBuf::from(p));
    }
    let mut extra: Vec<PathBuf> = Vec::new();
    if let Some(home) = crate::home_dir() {
        extra.push(home.join(".local").join("bin").join("amp"));
        extra.push(home.join(".amp").join("bin").join("amp"));
    }
    extra.push(PathBuf::from("/opt/homebrew/bin/amp"));
    extra.push(PathBuf::from("/usr/local/bin/amp"));
    crate::find_on_paths("amp", extra)
}

/// The four Dial modes, exposed as the catalog's "models". Mode ids are
/// stable API surface (`amp.getBuiltinAgent`, the SDK's `mode` option); the
/// underlying model routing changes behind them by design.
fn dial_models() -> Vec<Model> {
    // Every mode accepts the full effort ladder (the SDK's `effort` option
    // maps onto the CLI's `--effort`); unsupported levels fall to the closest
    // supported one upstream.
    const EFFORT: &[ReasoningLevel] = &[
        ReasoningLevel::Minimal,
        ReasoningLevel::Low,
        ReasoningLevel::Medium,
        ReasoningLevel::High,
        ReasoningLevel::XHigh,
        ReasoningLevel::Max,
    ];
    [
        (
            "low",
            "Low",
            "Fast, low-cost mode for small, well-defined tasks",
        ),
        (
            "medium",
            "Medium",
            "Balanced intelligence, speed, and cost — Amp's default",
        ),
        ("high", "High", "Deep reasoning for hard tasks"),
        (
            "ultra",
            "Ultra",
            "The most capable mode for hard, open-ended tasks",
        ),
    ]
    .into_iter()
    .map(|(id, label, description)| Model {
        id: id.into(),
        label: label.into(),
        description: Some(description.into()),
        reasoning_levels: EFFORT.to_vec(),
        options: Vec::new(),
    })
    .collect()
}

/// `RunRequest::reasoning` → `--effort` spelling. Amp's ladder is
/// none/minimal/low/medium/high/xhigh/max; `Ultra`-and-above clamp to `max`.
fn to_effort(reasoning: Option<ReasoningLevel>) -> Option<&'static str> {
    match reasoning? {
        ReasoningLevel::Minimal => Some("minimal"),
        ReasoningLevel::Low => Some("low"),
        ReasoningLevel::Medium => Some("medium"),
        ReasoningLevel::High => Some("high"),
        ReasoningLevel::XHigh => Some("xhigh"),
        ReasoningLevel::Max | ReasoningLevel::Ultra | ReasoningLevel::Ultracode => Some("max"),
        ReasoningLevel::Ultrathink => None,
    }
}

/// The Amp harness. `AmpHarness::new()` resolves `amp` per call; tests pin a
/// fake CLI with [`AmpHarness::with_executable`].
pub struct AmpHarness {
    executable: Option<PathBuf>,
    kill_grace: Duration,
}

impl Default for AmpHarness {
    fn default() -> Self {
        Self {
            executable: None,
            kill_grace: Duration::from_secs(3),
        }
    }
}

impl AmpHarness {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    pub fn with_kill_grace(mut self, grace: Duration) -> Self {
        self.kill_grace = grace;
        self
    }

    fn resolve_executable(&self) -> Result<PathBuf, HarnessError> {
        if let Some(p) = &self.executable {
            return Ok(p.clone());
        }
        resolve_amp_executable().ok_or_else(|| {
            HarnessError::NotInstalled(
                "amp (searched PATH, the login shell's PATH, ~/.local/bin, ~/.amp/bin, \
                 /opt/homebrew/bin, /usr/local/bin, and fnm/nvm/volta/pnpm/bun install \
                 dirs; set ZERON_AMP_EXECUTABLE to override)"
                    .into(),
            )
        })
    }

    fn build_command(&self, exe: &PathBuf, request: &RunRequest) -> Command {
        let mut cmd = Command::new(exe);
        crate::compose_child_path(&mut cmd, exe);
        if let Some(resume) = &request.resume {
            // `amp threads continue <T-id> --execute …` resumes the thread.
            cmd.args(["threads", "continue", resume]);
        }
        cmd.args([
            "--execute",
            "--stream-json",
            "--stream-json-input",
            // Thinking blocks ride the assistant frames as `thinking` content;
            // the normalizer folds them into ReasoningDeltas.
            "--stream-json-thinking",
        ]);
        if let Some(mode) = &request.model {
            cmd.args(["--mode", mode]);
        }
        if let Some(effort) = to_effort(request.reasoning) {
            cmd.args(["--effort", effort]);
        }
        if request.auto_approve {
            cmd.arg("--dangerously-allow-all");
        }
        // The app stamps local runs with the loopback embedded-browser
        // bridge; amp's `--mcp-config` takes a server-name → config map
        // (`url` for remote servers) and outranks settings-file servers.
        if let Some(url) = crate::browser_bridge_url(request) {
            cmd.arg("--mcp-config")
                .arg(serde_json::json!({"zeron-browser": {"url": url}}).to_string());
            cmd.env(crate::BROWSER_BRIDGE_ENV, url);
        }
        if !request.cwd.is_empty() {
            cmd.current_dir(&request.cwd);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd
    }
}

#[async_trait]
impl Harness for AmpHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Amp
    }
    fn display_name(&self) -> &str {
        "Amp"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    /// `steer: true` input lines are consumed at the next interruption point
    /// mid-turn — a real step boundary, same as Claude.
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[
            ReasoningLevel::Minimal,
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::XHigh,
            ReasoningLevel::Max,
        ]
    }
    fn installed(&self) -> bool {
        self.executable.is_some() || resolve_amp_executable().is_some()
    }
    /// `result` is the CLI's own terminal frame.
    fn deterministic_turn_end(&self) -> bool {
        true
    }

    /// The Dial modes are the catalog — Amp's public selection abstraction.
    /// Requires an installed CLI so a missing binary surfaces as
    /// [`HarnessError::NotInstalled`], matching the other harnesses.
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.resolve_executable()?;
        Ok(dial_models())
    }

    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let exe = self.resolve_executable()?;
        let mut child = self.build_command(&exe, &request).spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HarnessError::NotInstalled(exe.display().to_string())
            } else {
                HarnessError::Io(e)
            }
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| HarnessError::Protocol("amp child has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::Protocol("amp child has no stdout".into()))?;
        let stderr_tail = crate::StderrTail::default();
        if let Some(stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "zeron_harness::amp", "stderr: {line}");
                    tail.push(&line);
                }
            });
        }

        let (stdin_tx, stdin_rx) = mpsc::unbounded_channel::<StdinMsg>();
        tokio::spawn(stdin_writer(stdin, stdin_rx));

        // First turn: the prompt as a user frame (Amp reads stdin frames when
        // `--stream-json-input` is set — `-x` without a message waits on it).
        // Staged image attachments inline as base64 blocks (documented input
        // schema), degrading to the prompt's path refs when unreadable.
        let images = crate::claude::load_image_blocks(&request.attachments).await;
        let _ = stdin_tx.send(StdinMsg::Line(wire::amp_user_line(
            &request.prompt,
            false,
            &images,
        )));

        let (event_tx, event_rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(256);
        tokio::spawn(run_session(Session {
            child,
            stdout_lines: BufReader::new(stdout).lines(),
            stdin_tx,
            event_tx,
            controls,
            kill_grace: self.kill_grace,
            stderr_tail,
        }));

        Ok(futures::stream::unfold(event_rx, |mut rx| async move {
            rx.recv().await.map(|ev| (ev, rx))
        })
        .boxed())
    }
}

enum StdinMsg {
    Line(String),
    Close,
}

async fn stdin_writer(mut stdin: ChildStdin, mut rx: mpsc::UnboundedReceiver<StdinMsg>) {
    while let Some(msg) = rx.recv().await {
        match msg {
            StdinMsg::Line(line) => {
                let write = async {
                    stdin.write_all(line.as_bytes()).await?;
                    stdin.write_all(b"\n").await?;
                    stdin.flush().await
                };
                if let Err(e) = write.await {
                    tracing::debug!(target: "zeron_harness::amp", "stdin write failed (tolerated): {e}");
                    return;
                }
            }
            StdinMsg::Close => {
                let _ = stdin.shutdown().await;
                return;
            }
        }
    }
}

struct Session {
    child: Child,
    stdout_lines: tokio::io::Lines<BufReader<ChildStdout>>,
    stdin_tx: mpsc::UnboundedSender<StdinMsg>,
    event_tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
    controls: RunControls,
    kill_grace: Duration,
    stderr_tail: crate::StderrTail,
}

/// The per-run event loop, mirroring the Claude driver's: stdout frames +
/// steering mailbox + interrupt. Amp's wire has no interrupt request, so
/// cancel escalates straight to the child kill.
async fn run_session(session: Session) {
    let Session {
        mut child,
        mut stdout_lines,
        stdin_tx,
        event_tx,
        controls,
        kill_grace,
        stderr_tail,
    } = session;
    let RunControls {
        mut steering,
        interrupt,
        ..
    } = controls;

    let mut norm = Normalizer::for_amp();
    let mut steering_open = true;
    let mut interrupted = false;
    let mut interrupt_sent = false;
    let mut any_done = false;
    let mut done_after_interrupt = false;
    let mut escalation: Option<tokio::task::JoinHandle<()>> = None;

    'main: loop {
        tokio::select! {
            line = stdout_lines.next_line() => match line {
                Ok(Some(line)) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let frame = match wire::parse_frame(line) {
                        Ok(frame) => frame,
                        Err(e) => {
                            tracing::debug!(target: "zeron_harness::amp", "unparseable frame (skipped): {e}");
                            continue;
                        }
                    };
                    // Amp speaks no control channel; ControlRequest frames
                    // normalize to nothing and are ignored here.
                    if matches!(frame, Frame::ControlRequest(_)) {
                        continue;
                    }
                    for ev in norm.normalize(frame, interrupted) {
                        let is_done = matches!(ev, AgentEvent::Done { .. });
                        if event_tx.send(Ok(ev)).await.is_err() {
                            break 'main;
                        }
                        if is_done {
                            any_done = true;
                            if interrupted {
                                done_after_interrupt = true;
                                break 'main;
                            }
                        }
                    }
                }
                Ok(None) => break 'main,
                Err(e) => {
                    let _ = event_tx.send(Err(HarnessError::Io(e))).await;
                    break 'main;
                }
            },

            steer = steering.recv(), if steering_open && !interrupted => match steer {
                Some(msg) => {
                    // `steer: true` — consumed at the next interruption point.
                    let _ = stdin_tx.send(StdinMsg::Line(wire::amp_user_line(
                        &msg.prompt,
                        true,
                        &[],
                    )));
                    let (prev, next) = norm.rotate_for_steer();
                    let ev = AgentEvent::Steered {
                        assistant_message_id: Some(prev),
                        next_assistant_message_id: Some(next),
                    };
                    if event_tx.send(Ok(ev)).await.is_err() {
                        break 'main;
                    }
                }
                None => {
                    steering_open = false;
                    let _ = stdin_tx.send(StdinMsg::Close);
                }
            },

            _ = interrupt.cancelled(), if !interrupt_sent => {
                interrupt_sent = true;
                interrupted = true;
                // No protocol-level interrupt on this wire — escalate
                // straight to Term then Kill on the child.
                if let Some(target) = crate::process::signal_target(&child) {
                    send_signal(&target, Signal::Term);
                    escalation = Some(tokio::spawn(async move {
                        tokio::time::sleep(kill_grace).await;
                        send_signal(&target, Signal::Kill);
                    }));
                }
            },

            _ = event_tx.closed() => break 'main,
        }
    }

    if !event_tx.is_closed() {
        if interrupted && !done_after_interrupt {
            let _ = event_tx
                .send(Ok(AgentEvent::Done {
                    status: DoneStatus::Interrupted,
                    result: None,
                    error: None,
                    session_id: norm.session_id.clone(),
                }))
                .await;
        } else if !interrupted && !any_done {
            let status = child.try_wait().ok().flatten();
            let _ = event_tx
                .send(Ok(AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: Some(crate::crash_message("amp", status, &stderr_tail)),
                    session_id: norm.session_id.clone(),
                }))
                .await;
        }
    }

    shutdown_child(&mut child, kill_grace).await;
    if let Some(handle) = escalation {
        handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn dial_modes_are_the_catalog() {
        let models = dial_models();
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["low", "medium", "high", "ultra"]
        );
        // Every mode offers the effort ladder the CLI's `--effort` takes.
        assert!(models.iter().all(|m| m.reasoning_levels.len() == 6));
        assert!(models.iter().all(|m| m.description.is_some()));
    }

    #[test]
    fn effort_maps_to_amps_spellings() {
        assert_eq!(to_effort(Some(ReasoningLevel::Minimal)), Some("minimal"));
        assert_eq!(to_effort(Some(ReasoningLevel::XHigh)), Some("xhigh"));
        // Claude-shaped tiers clamp onto Amp's ceiling rather than guessing.
        assert_eq!(to_effort(Some(ReasoningLevel::Ultra)), Some("max"));
        assert_eq!(to_effort(Some(ReasoningLevel::Ultracode)), Some("max"));
        assert_eq!(to_effort(None), None);
    }

    #[test]
    fn command_carries_execute_mode_effort_and_permissions() {
        let exe = PathBuf::from(if cfg!(windows) { "amp.exe" } else { "amp" });
        let h = AmpHarness::new().with_executable(&exe);
        let request = RunRequest {
            prompt: "p".into(),
            harness: Some(HarnessId::Amp),
            model: Some("high".into()),
            reasoning: Some(ReasoningLevel::High),
            model_options: Default::default(),
            cwd: ".".into(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            resume: None,
            attachments: Vec::new(),
            worktree: None,
        };
        let mut cmd = h.build_command(&exe, &request);
        let joined = cmd
            .as_std_mut()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("--execute"), "{joined}");
        assert!(joined.contains("--stream-json-input"), "{joined}");
        assert!(joined.contains("--mode high"), "{joined}");
        assert!(joined.contains("--effort high"), "{joined}");
        assert!(joined.contains("--dangerously-allow-all"), "{joined}");
    }

    #[test]
    fn resume_routes_through_threads_continue() {
        let exe = PathBuf::from(if cfg!(windows) { "amp.exe" } else { "amp" });
        let h = AmpHarness::new().with_executable(&exe);
        let request = RunRequest {
            prompt: "p".into(),
            harness: Some(HarnessId::Amp),
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: ".".into(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            resume: Some("T-abc-123".into()),
            attachments: Vec::new(),
            worktree: None,
        };
        let args: Vec<String> = h
            .build_command(&exe, &request)
            .as_std_mut()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // `threads continue <id>` precedes the execute flags.
        let pos = |needle: &str| args.iter().position(|a| a == needle).unwrap();
        assert_eq!(args[0], "threads");
        assert_eq!(args[1], "continue");
        assert_eq!(args[2], "T-abc-123");
        assert!(pos("--execute") > 2);
    }

    #[test]
    fn amp_user_line_uses_block_array_and_steer_flag() {
        let v: Value = serde_json::from_str(&wire::amp_user_line("hi", false, &[])).unwrap();
        assert_eq!(v["type"], "user");
        assert!(v["message"]["content"].is_array());
        assert_eq!(v["message"]["content"][0]["type"], "text");
        assert_eq!(v["message"]["content"][0]["text"], "hi");
        assert!(v.get("steer").is_none());
        let v: Value = serde_json::from_str(&wire::amp_user_line("steer", true, &[])).unwrap();
        assert_eq!(v["steer"], json!(true));
    }

    #[test]
    fn amp_frames_normalize_through_the_shared_path() {
        // Real recorded Amp output (docs): init → user echo → assistant with a
        // settled text block → success result. The block-array input echo is
        // filtered as a non-tool_result user frame.
        let mut norm = Normalizer::for_amp();
        let init = wire::parse_frame(
            r#"{"type":"system","subtype":"init","cwd":"/p","session_id":"T-1","tools":["Bash"],"mcp_servers":[],"agent_mode":"medium"}"#,
        )
        .unwrap();
        let out = norm.normalize(init, false);
        assert!(matches!(
            &out[0],
            AgentEvent::SessionStarted { harness: HarnessId::Amp, session_id, model, .. }
                if session_id == "T-1" && model == "medium"
        ));

        let echo = wire::parse_frame(
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"what is 3+5?"}]},"session_id":"T-1"}"#,
        )
        .unwrap();
        assert!(norm.normalize(echo, false).is_empty());

        let assistant = wire::parse_frame(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"8"}],"usage":{"input_tokens":10,"output_tokens":5}},"session_id":"T-1"}"#,
        )
        .unwrap();
        let out = norm.normalize(assistant, false);
        assert!(out.iter().any(|e| matches!(
            e,
            AgentEvent::TextDelta { text } if text.contains('8')
        )));

        let result = wire::parse_frame(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"8","session_id":"T-1","usage":{"input_tokens":10,"output_tokens":5}}"#,
        )
        .unwrap();
        let out = norm.normalize(result, false);
        assert!(matches!(
            out.last(),
            Some(AgentEvent::Done {
                status: DoneStatus::Completed,
                session_id: Some(id),
                ..
            }) if id == "T-1"
        ));
    }

    #[test]
    fn amp_error_result_and_permission_denials_surface() {
        let mut norm = Normalizer::for_amp();
        let err = wire::parse_frame(
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"error":"boom","session_id":"T-1"}"#,
        )
        .unwrap();
        let out = norm.normalize(err, false);
        assert!(matches!(
            out.last(),
            Some(AgentEvent::Done {
                status: DoneStatus::Errored,
                error: Some(e),
                ..
            }) if e.contains("boom")
        ));

        let mut norm = Normalizer::for_amp();
        let denied = wire::parse_frame(
            r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"T-1","permission_denials":["Bash"]}"#,
        )
        .unwrap();
        let out = norm.normalize(denied, false);
        assert!(out.iter().any(|e| matches!(
            e,
            AgentEvent::Error { message } if message.contains("Bash")
        )));
    }
}
