//! Google Antigravity (`agy`) harness: the CLI's own stream-json protocol —
//! `agy --input-format stream-json --output-format stream-json`, NDJSON both
//! ways (live-validated against agy 1.2.2).
//!
//! Wire shape (NOT Claude's schema — do not share the normalizer):
//! - out: `{"event":"init","conversation_id":…,"init":{cwd,tools,…}}`
//! - out: `{"event":"step_update","step_update":{step_index,state,step_type,
//!   text_delta?,tool_name?,tool_info?{name,parameters,output},usage?}}`
//! - out: `{"event":"result","result":{conversation_id,status,response,
//!   usage,denied_actions?}}`
//! - in:  `{"event":"user","message":{"content":"…"}}` per turn — the wire is
//!   turn-boundary: each user line runs one turn and yields its own `result`.
//!
//! Permissions: headless `ask_permission` requests auto-DENY unless
//! `--dangerously-skip-permissions` is passed (denials land in
//! `result.denied_actions`, surfaced as an `Error` event). Zeron sessions run
//! unattended, so `auto_approve` maps onto the skip flag; without it the run
//! still completes but refuses tool calls visibly.
//!
//! Cancel: no interrupt input event exists — drop/kill the child
//! (`kill_on_drop` + [`shutdown_child`] escalation).
//!
//! Model catalog: `agy models` emits `<id>\t<label>` TSV (plus a
//! `Fetching available models…` preamble line that is filtered out). Effort
//! is a model-id suffix (`…-low`/`-medium`/`-high`), NOT a combinable flag:
//! agy rejects `--effort` whenever `--model` rides along (a suffixed model
//! conflicts on mismatch; a suffix-free model isn't effort-capable at all),
//! so a picked reasoning level rewrites the model's own suffix instead —
//! `--effort` is only ever sent with no `--model`, where it sets the CLI
//! default's effort.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use crate::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode, ToolCall,
};

use crate::{Harness, HarnessError, RunControls, Signal, send_signal, shutdown_child};

/// `ZERON_AGY_EXECUTABLE` first, then PATH/login-shell/node-manager dirs via
/// the shared resolver, then the documented install locations
/// (`%LOCALAPPDATA%\agy\bin` on Windows, `~/.local/bin` + `~/.agy/bin`
/// elsewhere — the CLI's own installer drops it in both shapes).
fn resolve_agy_executable() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZERON_AGY_EXECUTABLE")
        && !p.is_empty()
    {
        return Some(PathBuf::from(p));
    }
    let mut extra: Vec<PathBuf> = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        extra.push(local.join("agy").join("bin").join("agy"));
    }
    if let Some(home) = crate::home_dir() {
        extra.push(home.join(".local").join("bin").join("agy"));
        extra.push(home.join(".agy").join("bin").join("agy"));
        extra.push(
            home.join("AppData")
                .join("Local")
                .join("agy")
                .join("bin")
                .join("agy"),
        );
    }
    extra.push(PathBuf::from("/opt/homebrew/bin/agy"));
    extra.push(PathBuf::from("/usr/local/bin/agy"));
    crate::find_on_paths("agy", extra)
}

/// One parsed stdout line.
#[derive(Debug)]
enum Frame {
    Init(InitFrame),
    Step(StepFrame),
    Result(ResultFrame),
    /// Unknown `event` values are skipped — forward-compatible.
    Other,
}

#[derive(Debug, Default, Deserialize)]
struct InitFrame {
    #[serde(default)]
    conversation_id: String,
    #[serde(default)]
    init: InitBody,
}

#[derive(Debug, Default, Deserialize)]
struct InitBody {
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    model: String,
}

#[derive(Debug, Default, Deserialize)]
struct StepFrame {
    #[serde(default)]
    step_update: StepBody,
}

#[derive(Debug, Default, Deserialize)]
struct StepBody {
    /// Correlates a tool step's ACTIVE open with its DONE settle — the wire
    /// has no per-call id, so the step index stands in.
    #[serde(default)]
    step_index: u64,
    /// ACTIVE for in-flight work, DONE once the step settles. Tool calls
    /// arrive as a DONE `step_type:"tool"` carrying `tool_info`; text arrives
    /// as `text_delta` on `agent_response` steps (a DONE step repeats the
    /// step's full text, so only ACTIVE deltas are folded).
    #[serde(default)]
    state: String,
    #[serde(default)]
    step_type: String,
    #[serde(default)]
    text_delta: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    tool_info: Option<ToolInfo>,
    #[serde(default)]
    usage: Option<UsageBody>,
}

#[derive(Debug, Default, Deserialize)]
struct ToolInfo {
    #[serde(default)]
    name: String,
    #[serde(default)]
    parameters: Value,
    #[serde(default)]
    output: Option<Value>,
    /// Present instead of `output` when the tool call was refused.
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ResultFrame {
    #[serde(default)]
    result: ResultBody,
}

#[derive(Debug, Default, Deserialize)]
struct ResultBody {
    #[serde(default)]
    conversation_id: String,
    /// `SUCCESS`, `FAILURE`, `CANCELLED` (observed SUCCESS + denials).
    #[serde(default)]
    status: String,
    #[serde(default)]
    response: Option<String>,
    #[serde(default)]
    usage: Option<UsageBody>,
    /// Tool calls auto-denied while running permission-gated — surfaced so a
    /// denied run never reads as a clean success.
    #[serde(default)]
    denied_actions: Vec<Value>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct UsageBody {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

fn parse_frame(line: &str) -> Result<Frame, serde_json::Error> {
    let value: Value = serde_json::from_str(line)?;
    let frame = match value.get("event").and_then(Value::as_str).unwrap_or("") {
        "init" => Frame::Init(serde_json::from_value(value)?),
        "step_update" => Frame::Step(serde_json::from_value(value)?),
        "result" => Frame::Result(serde_json::from_value(value)?),
        _ => Frame::Other,
    };
    Ok(frame)
}

/// The stdin turn line — `{"event":"user","message":{"content":…}}`.
fn user_line(text: &str) -> String {
    json!({
        "event": "user",
        "message": { "content": text },
    })
    .to_string()
}

/// Decode an agy `tool_info` into a typed [`ToolCall`]. Antigravity spells
/// tools in snake_case (`run_command`, `view_file`, `write_to_file`,
/// `replace_file_content`, `grep_search`, `list_dir`, `read_url_content`,
/// `search_web`, `manage_task`, `invoke_subagent`, `ask_question`, …); the
/// parameters it reports are TitleCase (`CommandLine`, `AbsolutePath`, …).
fn decode_tool(name: &str, params: &Value) -> ToolCall {
    let str_any = |keys: &[&str]| -> String {
        keys.iter()
            .find_map(|k| params.get(*k).and_then(Value::as_str))
            .unwrap_or("")
            .into()
    };
    let opt_any = |keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| params.get(*k).and_then(Value::as_str))
            .map(str::to_owned)
    };
    match name {
        "run_command" | "send_command_input" | "command_status" | "notebook_execution" => {
            ToolCall::Exec {
                command: str_any(&["CommandLine", "command", "cmd", "input"]),
            }
        }
        "view_file" | "read_resource" | "read_browser_page" => ToolCall::ReadFile {
            path: str_any(&["AbsolutePath", "path", "file_path", "uri", "url"]),
        },
        "write_to_file" | "create_file" => ToolCall::WriteFile {
            path: str_any(&["AbsolutePath", "path", "file_path"]),
            content: opt_any(&["CodeContent", "content", "contents"]),
        },
        "replace_file_content" | "multi_replace_file_content" | "sed_file" | "notebook_edit" => {
            ToolCall::EditFile {
                path: str_any(&["AbsolutePath", "path", "file_path"]),
                old_string: opt_any(&["TargetContent", "old_string", "old"]),
                new_string: opt_any(&["ReplacementContent", "new_string", "new"]),
            }
        }
        "grep_search" => ToolCall::Search {
            pattern: str_any(&["Query", "pattern", "query", "regex"]),
            path: opt_any(&["SearchPath", "path", "dir"]),
        },
        "find_by_name" | "list_dir" | "list_resources" => ToolCall::Glob {
            pattern: str_any(&[
                "Pattern",
                "DirectoryPath",
                "pattern",
                "glob",
                "path",
                "query",
            ]),
        },
        "read_url_content" | "open_browser_url" | "read_browser_page_url" => ToolCall::WebFetch {
            url: str_any(&["Url", "url"]),
            prompt: None,
        },
        "search_web" => ToolCall::WebSearch {
            query: str_any(&["Query", "query"]),
        },
        "manage_task" => ToolCall::Todo { items: Vec::new() },
        // A subagent spawn: same `Agent: <desc>` convention the engine keys
        // subagent binding on.
        "invoke_subagent" | "browser_subagent" | "define_subagent" => {
            let description = str_any(&["Task", "task", "description", "name"]);
            ToolCall::Unknown {
                name: if description.is_empty() {
                    "Agent".into()
                } else {
                    format!("Agent: {description}")
                },
                input: (!params.is_null()).then(|| params.clone()),
            }
        }
        "call_mcp_tool" => ToolCall::Mcp {
            server: str_any(&["ServerName", "server"]),
            tool: str_any(&["ToolName", "tool"]),
            input: opt_any(&["Arguments", "arguments"])
                .and_then(|a| serde_json::from_str(&a).ok())
                .or_else(|| params.get("Arguments").cloned()),
        },
        _ => ToolCall::Unknown {
            name: name.into(),
            input: (!params.is_null()).then(|| params.clone()),
        },
    }
}

/// The Antigravity harness. `AntigravityHarness::new()` resolves `agy` per
/// call; tests pin a fake CLI with [`AntigravityHarness::with_executable`].
pub struct AntigravityHarness {
    executable: Option<PathBuf>,
    kill_grace: Duration,
    /// `agy models` cache: only a successful non-empty catalog is cached, so a
    /// transient failure retries on the next picker open.
    models: tokio::sync::OnceCell<Vec<Model>>,
}

impl Default for AntigravityHarness {
    fn default() -> Self {
        Self {
            executable: None,
            kill_grace: Duration::from_secs(3),
            models: tokio::sync::OnceCell::new(),
        }
    }
}

impl AntigravityHarness {
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
        resolve_agy_executable().ok_or_else(|| {
            HarnessError::NotInstalled(
                "agy (searched PATH, the login shell's PATH, %LOCALAPPDATA%\\agy\\bin, \
                 ~/.local/bin, ~/.agy/bin, /opt/homebrew/bin, /usr/local/bin; set \
                 ZERON_AGY_EXECUTABLE to override)"
                    .into(),
            )
        })
    }

    fn build_command(
        &self,
        exe: &PathBuf,
        request: &RunRequest,
        catalog: Option<&[Model]>,
    ) -> Command {
        let mut cmd = Command::new(exe);
        crate::compose_child_path(&mut cmd, exe);
        cmd.args([
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
        ]);
        let effort = to_effort(request.reasoning);
        match &request.model {
            // `--model` and `--effort` never combine — effort rides the
            // model id's suffix instead.
            Some(model) => {
                let model = model_for_effort(model, effort, catalog);
                cmd.args(["--model", &model]);
            }
            // `--effort` alone IS legal: it applies to the CLI's default model.
            None => {
                if let Some(effort) = effort {
                    cmd.args(["--effort", effort]);
                }
            }
        }
        // Zeron sessions run unattended; auto_approve maps onto the CLI's own
        // skip flag (without it every tool call auto-denies — surfaced via
        // `denied_actions` rather than silently succeeding).
        if request.auto_approve {
            cmd.arg("--dangerously-skip-permissions");
        }
        if let Some(resume) = &request.resume {
            cmd.args(["--conversation", resume]);
        }
        // agy has no per-invocation MCP flag (config files only, and writing
        // the user's is out of scope) — the env var is the documented seam:
        // the bridge speaks plain JSON-RPC POSTs, so the agent can drive it
        // through any shell. The computer and tools endpoints travel the
        // same seam.
        crate::browser_bridge_env(&mut cmd, request);
        crate::computer_env(&mut cmd, request);
        crate::tools_env(&mut cmd, request);
        if !request.cwd.is_empty() {
            cmd.current_dir(&request.cwd);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd
    }

    async fn discover_models(&self) -> Result<Vec<Model>, HarnessError> {
        let exe = self.resolve_executable()?;
        let mut cmd = Command::new(&exe);
        crate::compose_child_path(&mut cmd, &exe);
        cmd.arg("models")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HarnessError::NotInstalled(exe.display().to_string())
            } else {
                HarnessError::Io(e)
            }
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::Protocol("agy models: no stdout".into()))?;
        let read = async move {
            let mut text = String::new();
            let mut lines = BufReader::new(stdout).lines();
            while let Some(line) = lines.next_line().await? {
                text.push_str(&line);
                text.push('\n');
            }
            Ok::<String, std::io::Error>(text)
        };
        let text = tokio::time::timeout(Duration::from_secs(20), read)
            .await
            .map_err(|_| HarnessError::Protocol("agy models timed out".into()))?
            .map_err(HarnessError::Io)?;
        shutdown_child(&mut child, self.kill_grace).await;
        let models = parse_models(&text);
        if models.is_empty() {
            return Err(HarnessError::Protocol(
                "agy models returned no usable entries".into(),
            ));
        }
        Ok(models)
    }
}

/// `agy models` TSV → catalog: `<id>\t<label>` per line, preamble/progress
/// lines (no tab) skipped. Reasoning suffixes on the id
/// (`-low`/`-medium`/`-high`) map onto the model's effort ladder so the
/// picker's reasoning control has something real to send.
fn parse_models(text: &str) -> Vec<Model> {
    text.lines()
        .filter_map(|line| {
            let (id, label) = line.split_once('\t')?;
            let id = id.trim();
            let label = label.trim();
            if id.is_empty() {
                return None;
            }
            let label = if label.is_empty() { id } else { label };
            Some(Model {
                id: id.to_owned(),
                label: label.to_owned(),
                description: None,
                reasoning_levels: Vec::new(),
                options: Vec::new(),
            })
        })
        .collect()
}

/// `RunRequest::reasoning` → agy's effort spelling; `None` when the picker
/// sent nothing (agy then applies the model's own default).
fn to_effort(reasoning: Option<ReasoningLevel>) -> Option<&'static str> {
    match reasoning? {
        ReasoningLevel::Minimal | ReasoningLevel::Low => Some("low"),
        ReasoningLevel::Medium => Some("medium"),
        ReasoningLevel::High | ReasoningLevel::XHigh | ReasoningLevel::Max => Some("high"),
        // No agy spelling for the Claude-shaped tiers; omit rather than guess.
        ReasoningLevel::Ultra | ReasoningLevel::Ultracode | ReasoningLevel::Ultrathink => None,
    }
}

/// Split an agy model id into stem + effort suffix (`-low`/`-medium`/`-high`).
fn split_effort_suffix(model: &str) -> Option<(&str, &str)> {
    ["low", "medium", "high"].iter().find_map(|suffix| {
        model
            .strip_suffix(format!("-{suffix}").as_str())
            .map(|stem| (stem, *suffix))
    })
}

/// The model id to run for a picked effort. agy has no combinable effort flag
/// alongside `--model`, so the pick rewrites the id's own suffix — but only to
/// a variant the discovered catalog actually advertises (`gpt-oss-120b` ships
/// only `-medium`, `gemini-3.1-pro` skips `-medium`). Anything unverifiable —
/// no suffix on the id, no catalog, target variant absent — keeps the picked
/// id as-is: its baked-in tier still runs where a fabricated id would hard-fail.
fn model_for_effort(model: &str, effort: Option<&str>, catalog: Option<&[Model]>) -> String {
    let Some(effort) = effort else {
        return model.to_owned();
    };
    let Some((stem, _)) = split_effort_suffix(model) else {
        return model.to_owned();
    };
    let candidate = format!("{stem}-{effort}");
    if catalog.is_some_and(|models| models.iter().any(|m| m.id == candidate)) {
        candidate
    } else {
        model.to_owned()
    }
}

#[async_trait]
impl Harness for AntigravityHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Antigravity
    }
    fn display_name(&self) -> &str {
        "Antigravity"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    /// Each `user` stdin line runs one full turn — a steer is delivered at
    /// the turn boundary (the CLI has no mid-turn input event).
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
        ]
    }
    fn installed(&self) -> bool {
        self.executable.is_some() || resolve_agy_executable().is_some()
    }
    /// `result` is the CLI's own terminal frame.
    fn deterministic_turn_end(&self) -> bool {
        true
    }

    /// Dynamic catalog straight from `agy models`; cached on first success.
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.models
            .get_or_try_init(|| self.discover_models())
            .await
            .cloned()
    }

    /// `~/.gemini/antigravity-cli/conversation_summaries.db` — one sqlite row
    /// per conversation; resumable through `agy --conversation <id>`.
    async fn sessions(&self) -> Result<Vec<zeron_proto::HarnessSession>, HarnessError> {
        self.resolve_executable()?;
        let Some(db) = crate::home_dir().map(|h| {
            h.join(".gemini")
                .join("antigravity-cli")
                .join("conversation_summaries.db")
        }) else {
            return Ok(Vec::new());
        };
        Ok(
            tokio::task::spawn_blocking(move || crate::sessions::scan_antigravity(&db))
                .await
                .unwrap_or_default(),
        )
    }

    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let exe = self.resolve_executable()?;
        // The catalog validates effort→model-suffix rewrites — already warm
        // whenever the picker supplied this model (same harness instance),
        // and only worth a cold `agy models` spawn when the pick actually
        // implies a suffix change. A failed discovery degrades to keeping
        // the picked id, never to a failed run.
        let needs_catalog = matches!(
            (&request.model, to_effort(request.reasoning)),
            (Some(model), Some(effort))
                if split_effort_suffix(model).is_some_and(|(_, current)| current != effort)
        );
        let catalog = if needs_catalog {
            self.models().await.ok()
        } else {
            None
        };
        let mut child = self
            .build_command(&exe, &request, catalog.as_deref())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    HarnessError::NotInstalled(exe.display().to_string())
                } else {
                    HarnessError::Io(e)
                }
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| HarnessError::Protocol("agy child has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::Protocol("agy child has no stdout".into()))?;
        let stderr_tail = crate::StderrTail::default();
        if let Some(stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "zeron_harness::antigravity", "stderr: {line}");
                    tail.push(&line);
                }
            });
        }

        let (stdin_tx, stdin_rx) = mpsc::unbounded_channel::<StdinMsg>();
        tokio::spawn(stdin_writer(stdin, stdin_rx));
        let _ = stdin_tx.send(StdinMsg::Line(user_line(&request.prompt)));

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
                    tracing::debug!(target: "zeron_harness::antigravity", "stdin write failed (tolerated): {e}");
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

/// Per-run normalization state — much smaller than Claude's: the wire has no
/// streaming partials to stitch, no subagent tagging, no control channel.
struct Normalizer {
    saw_init: bool,
    session_id: Option<String>,
    assistant_message_id: String,
    /// step_index of the tool call currently open → its AgentEvent id, so a
    /// tool's DONE update lands a ToolResult on the SAME chip.
    open_tools: HashMap<u64, String>,
}

impl Normalizer {
    fn new() -> Self {
        Self {
            saw_init: false,
            session_id: None,
            assistant_message_id: uuid::Uuid::new_v4().to_string(),
            open_tools: HashMap::new(),
        }
    }

    fn rotate_for_steer(&mut self) -> (String, String) {
        let prev = std::mem::replace(
            &mut self.assistant_message_id,
            uuid::Uuid::new_v4().to_string(),
        );
        (prev, self.assistant_message_id.clone())
    }

    fn normalize(&mut self, frame: Frame, interrupted: bool) -> Vec<AgentEvent> {
        match frame {
            Frame::Init(f) => {
                self.session_id = (!f.conversation_id.is_empty())
                    .then(|| f.conversation_id.clone())
                    .or(self.session_id.take());
                if self.saw_init {
                    return Vec::new();
                }
                self.saw_init = true;
                vec![AgentEvent::SessionStarted {
                    harness: HarnessId::Antigravity,
                    model: f.init.model,
                    tools: f.init.tools,
                    cwd: f.init.cwd,
                    session_id: f.conversation_id,
                    assistant_message_id: self.assistant_message_id.clone(),
                }]
            }
            Frame::Step(f) => {
                let step = f.step_update;
                let step_index = step.step_index;
                let mut out = Vec::new();
                match step.step_type.as_str() {
                    // ACTIVE deltas are increments; the step's DONE update
                    // repeats its full text — fold only ACTIVE deltas or the
                    // transcript doubles.
                    "agent_response" => {
                        if step.state == "ACTIVE"
                            && let Some(delta) = step.text_delta.filter(|t| !t.is_empty())
                        {
                            out.push(AgentEvent::TextDelta { text: delta });
                        }
                    }
                    "tool" => {
                        let id = format!("agy-tool-{step_index}");
                        if step.state == "ACTIVE" {
                            if let Some(info) = &step.tool_info {
                                self.open_tools.insert(step_index, id.clone());
                                out.push(AgentEvent::ToolCall {
                                    id,
                                    call: decode_tool(
                                        step.tool_name.as_deref().unwrap_or(info.name.as_str()),
                                        &info.parameters,
                                    ),
                                });
                            }
                        } else if step.state == "DONE" {
                            let id = self.open_tools.remove(&step_index).unwrap_or(id);
                            if let Some(info) = step.tool_info {
                                let (is_error, output) = match (info.error, info.output) {
                                    (Some(err), _) => (true, Some(err)),
                                    (None, out) => (
                                        false,
                                        out.map(|v| match v {
                                            Value::String(s) => s,
                                            other => other.to_string(),
                                        }),
                                    ),
                                };
                                out.push(AgentEvent::ToolResult {
                                    id,
                                    is_error,
                                    output,
                                    diff: None,
                                });
                            }
                        }
                    }
                    _ => {}
                }
                if let Some(usage) = step.usage {
                    out.push(AgentEvent::Usage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                    });
                }
                out
            }
            Frame::Result(f) => {
                let r = f.result;
                if !r.conversation_id.is_empty() {
                    self.session_id = Some(r.conversation_id);
                }
                let mut out = Vec::new();
                if let Some(usage) = r.usage {
                    out.push(AgentEvent::Usage {
                        input_tokens: usage.input_tokens,
                        output_tokens: usage.output_tokens,
                    });
                }
                if !r.denied_actions.is_empty() {
                    let denied: Vec<String> = r
                        .denied_actions
                        .iter()
                        .map(|d| match d {
                            Value::String(s) => s.clone(),
                            other => other
                                .get("tool_name")
                                .or_else(|| other.get("name"))
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .unwrap_or_else(|| other.to_string()),
                        })
                        .collect();
                    out.push(AgentEvent::Error {
                        message: format!("Permission denied: {}", denied.join(", ")),
                    });
                }
                let failed = !r.status.eq_ignore_ascii_case("success")
                    && !r.status.eq_ignore_ascii_case("ok");
                let status = if interrupted {
                    DoneStatus::Interrupted
                } else if failed {
                    DoneStatus::Errored
                } else {
                    DoneStatus::Completed
                };
                out.push(AgentEvent::Done {
                    status,
                    result: if failed { None } else { r.response.clone() },
                    error: if failed {
                        Some(
                            r.error
                                .or(r.response)
                                .unwrap_or_else(|| format!("agy reported status {}", r.status)),
                        )
                    } else {
                        r.error
                    },
                    session_id: self.session_id.clone(),
                });
                out
            }
            Frame::Other => Vec::new(),
        }
    }
}

/// The per-run event loop: stdout frames + steering mailbox + interrupt.
/// agy has no interrupt input event — cancelling escalates straight to
/// terminate/kill on the child.
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

    let mut norm = Normalizer::new();
    let mut steering_open = true;
    let mut interrupted = false;
    let mut any_done = false;
    let mut done_after_interrupt = false;

    'main: loop {
        tokio::select! {
            line = stdout_lines.next_line() => match line {
                Ok(Some(line)) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let frame = match parse_frame(line) {
                        Ok(frame) => frame,
                        Err(e) => {
                            tracing::debug!(target: "zeron_harness::antigravity", "unparseable frame (skipped): {e}");
                            continue;
                        }
                    };
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
                            // Turn-boundary input: after each `result` the CLI
                            // waits for the next `user` line — a queued steer
                            // flows into the next turn; nothing queued ⇒ the
                            // run is complete and stdin closes so agy exits.
                            if !steering_open {
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
                    let _ = stdin_tx.send(StdinMsg::Line(user_line(&msg.prompt)));
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

            _ = interrupt.cancelled(), if !interrupted => {
                interrupted = true;
                // No protocol-level interrupt on this wire — terminate the
                // child; the engine still gets a Done{Interrupted} below if
                // the CLI dies before emitting `result`.
                if let Some(target) = crate::process::signal_target(&child) {
                    send_signal(&target, Signal::Term);
                }
                let _ = child.start_kill();
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
                    error: Some(crate::crash_message("agy", status, &stderr_tail)),
                    session_id: norm.session_id.clone(),
                }))
                .await;
        }
    }

    shutdown_child(&mut child, kill_grace).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_init_step_and_result_frames() {
        let init = r#"{"event":"init","conversation_id":"c1","init":{"cwd":"/x","tools":["run_command"],"permission_mode":"always-proceed"}}"#;
        match parse_frame(init).unwrap() {
            Frame::Init(f) => {
                assert_eq!(f.conversation_id, "c1");
                assert_eq!(f.init.tools, vec!["run_command".to_string()]);
            }
            other => panic!("unexpected frame: {other:?}"),
        }
        assert!(matches!(
            parse_frame(r#"{"event":"mystery"}"#).unwrap(),
            Frame::Other
        ));
        assert!(parse_frame("not json").is_err());
    }

    #[test]
    fn user_line_matches_wire_shape() {
        let v: Value = serde_json::from_str(&user_line("hi")).unwrap();
        assert_eq!(v["event"], "user");
        assert_eq!(v["message"]["content"], "hi");
    }

    #[test]
    fn step_active_text_emits_delta_done_repeat_does_not() {
        let mut norm = Normalizer::new();
        let active = r#"{"event":"step_update","step_update":{"step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"hel"}}"#;
        let done = r#"{"event":"step_update","step_update":{"step_index":1,"state":"DONE","step_type":"agent_response","text_delta":"hello"}}"#;
        let a = norm.normalize(parse_frame(active).unwrap(), false);
        assert_eq!(a, vec![AgentEvent::TextDelta { text: "hel".into() }]);
        assert!(norm.normalize(parse_frame(done).unwrap(), false).is_empty());
    }

    #[test]
    fn tool_step_pairs_call_and_result_on_one_id() {
        let mut norm = Normalizer::new();
        let call = r#"{"event":"step_update","step_update":{"step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hi"}}}}"#;
        let done = r#"{"event":"step_update","step_update":{"step_index":2,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"echo hi"},"output":"hi\n"}}}"#;
        let a = norm.normalize(parse_frame(call).unwrap(), false);
        let [AgentEvent::ToolCall { id, call }] = a.as_slice() else {
            panic!("expected one ToolCall, got {a:?}")
        };
        assert_eq!(
            *call,
            ToolCall::Exec {
                command: "echo hi".into()
            }
        );
        let b = norm.normalize(parse_frame(done).unwrap(), false);
        assert_eq!(
            b,
            vec![AgentEvent::ToolResult {
                id: id.clone(),
                is_error: false,
                output: Some("hi\n".into()),
                diff: None,
            }]
        );
    }

    #[test]
    fn denied_actions_and_failure_surface_on_result() {
        let mut norm = Normalizer::new();
        let denied = r#"{"event":"result","result":{"conversation_id":"c1","status":"SUCCESS","response":"done","denied_actions":[{"tool_name":"run_command"}]}}"#;
        let out = norm.normalize(parse_frame(denied).unwrap(), false);
        assert!(out.iter().any(|e| matches!(
            e,
            AgentEvent::Error { message } if message.contains("run_command")
        )));
        assert!(matches!(
            out.last(),
            Some(AgentEvent::Done {
                status: DoneStatus::Completed,
                ..
            })
        ));

        let mut norm = Normalizer::new();
        let failed = r#"{"event":"result","result":{"conversation_id":"c1","status":"FAILURE","error":"boom"}}"#;
        let out = norm.normalize(parse_frame(failed).unwrap(), false);
        assert!(matches!(
            out.last(),
            Some(AgentEvent::Done {
                status: DoneStatus::Errored,
                error: Some(e),
                ..
            }) if e == "boom"
        ));
    }

    #[test]
    fn parse_models_reads_tsv_and_skips_preamble() {
        let text = "Fetching available models...\n\
                    gemini-3.8-flash-high\tGemini 3.8 Flash (High)\n\
                    gemini-3.8-flash-low\tGemini 3.8 Flash (Low)\n\
                    claude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)\n";
        let models = parse_models(text);
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "gemini-3.8-flash-high");
        assert_eq!(models[0].label, "Gemini 3.8 Flash (High)");
        assert_eq!(models[2].id, "claude-sonnet-4-6");
        // Garbage and blank lines never yield phantom models.
        assert!(parse_models("Fetching available models...\n").is_empty());
        assert!(parse_models("").is_empty());
    }

    #[test]
    fn effort_maps_only_ags_spellings() {
        assert_eq!(to_effort(Some(ReasoningLevel::Low)), Some("low"));
        assert_eq!(to_effort(Some(ReasoningLevel::Medium)), Some("medium"));
        assert_eq!(to_effort(Some(ReasoningLevel::High)), Some("high"));
        assert_eq!(to_effort(Some(ReasoningLevel::Ultra)), None);
        assert_eq!(to_effort(None), None);
    }

    #[test]
    fn effort_rewrites_the_model_suffix_only_when_the_variant_is_real() {
        let catalog = parse_models(
            "gemini-3.8-flash-high\tGemini 3.8 Flash (High)\n\
             gemini-3.8-flash-medium\tGemini 3.8 Flash (Medium)\n\
             gemini-3.8-flash-low\tGemini 3.8 Flash (Low)\n\
             gemini-3.1-pro-high\tGemini 3.1 Pro (High)\n\
             gemini-3.1-pro-low\tGemini 3.1 Pro (Low)\n\
             gpt-oss-120b-medium\tGPT-OSS 120B (Medium)\n\
             claude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)\n",
        );
        let catalog = Some(catalog.as_slice());
        // A real sibling variant: the effort pick wins over the id's suffix.
        assert_eq!(
            model_for_effort("gemini-3.8-flash-low", Some("medium"), catalog),
            "gemini-3.8-flash-medium"
        );
        // Same tier: the id stands unchanged.
        assert_eq!(
            model_for_effort("gemini-3.8-flash-low", Some("low"), catalog),
            "gemini-3.8-flash-low"
        );
        // The family ships no such variant — keep the picked id rather than
        // fabricate `gpt-oss-120b-high` / `gemini-3.1-pro-medium` (agy
        // hard-fails unknown ids).
        assert_eq!(
            model_for_effort("gpt-oss-120b-medium", Some("high"), catalog),
            "gpt-oss-120b-medium"
        );
        assert_eq!(
            model_for_effort("gemini-3.1-pro-high", Some("medium"), catalog),
            "gemini-3.1-pro-high"
        );
        // Suffix-free ids aren't effort-capable at all (agy rejects --effort
        // for them): the id passes through untouched.
        assert_eq!(
            model_for_effort("claude-sonnet-4-6", Some("medium"), catalog),
            "claude-sonnet-4-6"
        );
        // No effort pick / no catalog: never guess.
        assert_eq!(
            model_for_effort("gemini-3.8-flash-low", None, catalog),
            "gemini-3.8-flash-low"
        );
        assert_eq!(
            model_for_effort("gemini-3.8-flash-low", Some("medium"), None),
            "gemini-3.8-flash-low"
        );
    }

    #[test]
    fn command_carries_model_effort_resume_and_permissions() {
        let exe = PathBuf::from(if cfg!(windows) { "agy.exe" } else { "agy" });
        let h = AntigravityHarness::new().with_executable(&exe);
        let request = RunRequest {
            prompt: "p".into(),
            harness: Some(HarnessId::Antigravity),
            model: Some("gemini-3.8-flash-high".into()),
            reasoning: Some(ReasoningLevel::High),
            model_options: Default::default(),
            cwd: ".".into(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            resume: Some("conv-1".into()),
            attachments: Vec::new(),
            worktree: None,
        };
        let mut cmd = h.build_command(&exe, &request, None);
        let args: Vec<String> = cmd
            .as_std_mut()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let joined = args.join(" ");
        assert!(joined.contains("--model gemini-3.8-flash-high"), "{joined}");
        // `--effort` NEVER rides along with `--model` — agy rejects the pair.
        assert!(!joined.contains("--effort"), "{joined}");
        assert!(joined.contains("--conversation conv-1"), "{joined}");
        assert!(
            joined.contains("--dangerously-skip-permissions"),
            "{joined}"
        );
        assert!(joined.contains("--output-format stream-json"), "{joined}");

        // With NO model the effort flag is legal again (CLI default model).
        let request = RunRequest {
            model: None,
            ..request
        };
        let joined = h
            .build_command(&exe, &request, None)
            .as_std_mut()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(!joined.contains("--model"), "{joined}");
        assert!(joined.contains("--effort high"), "{joined}");

        // Without auto_approve the skip flag is absent (denials then ride
        // `denied_actions`, not silence).
        let request = RunRequest {
            auto_approve: false,
            resume: None,
            ..request
        };
        let mut cmd = h.build_command(&exe, &request, None);
        let joined = cmd
            .as_std_mut()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            !joined.contains("--dangerously-skip-permissions"),
            "{joined}"
        );
        assert!(!joined.contains("--conversation"), "{joined}");
    }
}
