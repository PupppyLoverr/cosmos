//! The agent loop (contract §4-§9). Headless `-p` runs through `run_agent`;
//! the TUI drives the same loop through `Agent` one turn at a time.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde_json::{json, Value};
use zproto::{Block, Chunk, Event, Message, OutputMode, Role, RunOpts, Usage};
use zproviders::{complete, ProviderCfg, Request};
use ztools::{tool_specs, ToolContext, PLAN_TOOLS};

use crate::config::Config;
use crate::session::Session;

pub struct Agent {
    pub session: Session,
    pub provider: ProviderCfg,
    pub model_alias: String,
    pub system: Vec<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<zproto::ToolSpec>,
    pub ctx: ToolContext,
    pub mode: Mode,
    pub opts: RunOpts,
    pub allowed: Vec<String>,
    pub prices: zproto::Prices,
    pub context_window: u64,
    pub cum_usd: f64,
    pub last_usage: Usage,
    pub boot_tokens: u64,
    /// Interactive approval prompt — headless leaves this None (ask = deny).
    pub asker: Option<Asker>,
    /// Streaming text sink — the TUI paints tokens as they arrive. Not part
    /// of the event log (the full `assistant` event still follows the turn).
    pub on_delta: Option<DeltaSink>,
    /// Set by submit_plan — the TUI watches this to offer /approve.
    pub pending_plan: Option<String>,
    /// git worktrees created by `task {worktree:true}` — (path, branch).
    /// Removed on drop only if the worktree is clean (contract §10).
    pub worktrees: Vec<(std::path::PathBuf, String)>,
    cfg: Config,
}

/// Interactive approval callback used by the TUI; headless runs leave it unset.
/// `Send` so the agent can live on a worker thread while the TUI owns the
/// terminal on the main thread.
pub type Asker = Box<dyn FnMut(&str) -> bool + Send>;

/// Receives each streamed text fragment of a top-level turn.
pub type DeltaSink = Box<dyn FnMut(&str) + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Plan,
    Agent,
    Yolo,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Plan => "plan",
            Mode::Agent => "agent",
            Mode::Yolo => "yolo",
        }
    }
}

fn meridian_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("MERIDIAN_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for n in ["meridian.exe", "meridian"] {
                let c = dir.join(n);
                if c.is_file() {
                    return Some(c);
                }
            }
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for d in std::env::split_paths(&path) {
            for n in ["meridian.exe", "meridian"] {
                let c = d.join(n);
                if c.is_file() {
                    return Some(c);
                }
            }
        }
    }
    None
}

/// Layer-3 boot context via the bridge; falls back to the snippet alone.
fn boot_context(workspace: &Path) -> (String, u64, Vec<String>) {
    if let Some(bin) = meridian_bin() {
        let mut cmd = std::process::Command::new(bin);
        cmd.args(["boot", "--repo"]).arg(workspace);
        if let Ok(v) = std::env::var("MERIDIAN_VAULT") {
            cmd.env("MERIDIAN_VAULT", v);
        }
        if let Ok(out) = cmd.output() {
            if out.status.success() {
                if let Ok(v) = serde_json::from_slice::<Value>(&out.stdout) {
                    let ctx = v["context"].as_str().unwrap_or("").to_string();
                    let tokens = v["tokens"].as_u64().unwrap_or(0);
                    let warnings = v["warnings"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|w| w.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    return (ctx, tokens, warnings);
                }
            }
        }
    }
    (
        crate::system_snippet(),
        0,
        vec!["meridian bridge not found; boot context is the contract snippet only".into()],
    )
}

fn default_provider() -> ProviderCfg {
    ProviderCfg {
        kind: zproviders::ProviderKind::Fixture,
        base_url: String::new(),
        api_key_env: String::new(),
        model: "fixture".into(),
        context: 128_000,
        prices: zproto::Prices::default(),
        effort: None,
        api_key: None,
        account_id: None,
    }
}

impl Agent {
    /// Create or resume an agent. `harness` is `zcli`/`zdev`/`zagy`.
    pub fn new(opts: &RunOpts, harness: &str, cwd: &Path) -> Result<Agent> {
        let home = crate::cli_home();
        let cfg = Config::load(&home);
        let workspace = opts
            .cwd
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd.to_path_buf());
        let model_alias = opts
            .model
            .clone()
            .or_else(|| cfg.default_model.clone())
            .unwrap_or_else(|| "fixture".into());
        let (provider, model_id) = cfg
            .provider_for(&model_alias)
            .unwrap_or_else(|| (default_provider(), "fixture".into()));

        let mode = if opts.yolo {
            Mode::Yolo
        } else {
            match opts.mode.as_deref() {
                Some("plan") => Mode::Plan,
                _ => Mode::Agent,
            }
        };

        let mut ctx = ToolContext::new(&workspace);
        ctx.mcp_servers = cfg.mcp_servers();
        ctx.large_read_lines = cfg.bridge.large_read_lines;
        ctx.no_sandbox = opts.no_sandbox;
        ctx.escalations = opts.allow_escalation.clone();
        ctx.net_allow = opts.allow_escalation.iter().any(|e| e == "network");

        let (session, messages) = if let Some(id) = &opts.resume {
            let s = Session::open(&home, id)?;
            let msgs = Session::replay(&home, id)?;
            (s, msgs)
        } else {
            (Session::create(&home, harness)?, Vec::new())
        };
        // resumed sessions keep their own harness name in history; new events
        // carry ours — that IS the handoff mechanism (shared home).
        let mut session = session;
        session.harness = harness.into();

        let (boot, boot_tokens, _warnings) = boot_context(&workspace);
        let tools = tool_specs();
        let mut system = vec![boot];
        if !crate::system_snippet().is_empty() && !system[0].contains("You are Cosmos") {
            system.insert(0, crate::system_snippet());
        }
        if let Some(p) = crate::project_instructions(&workspace) {
            system.push(p);
        }
        let prices = provider.prices;
        let context_window = provider.context;
        let allowed = cfg.allowed_hosts();
        Ok(Agent {
            session,
            provider,
            model_alias: format!("{model_alias} ({model_id})"),
            system,
            messages,
            tools,
            ctx,
            mode,
            opts: opts.clone(),
            allowed,
            prices,
            context_window,
            cum_usd: 0.0,
            last_usage: Usage::default(),
            boot_tokens,
            asker: None,
            on_delta: None,
            pending_plan: None,
            worktrees: Vec::new(),
            cfg,
        })
    }

    /// Decide whether `name` may run (contract §3).
    fn permitted(&self, name: &str) -> Permission {
        if self.mode == Mode::Yolo {
            return Permission::Allow;
        }
        if self.mode == Mode::Plan && !PLAN_TOOLS.contains(&name) {
            return Permission::Deny;
        }
        if self.opts.allow.iter().any(|t| t == name) {
            return Permission::Allow;
        }
        if self.cfg.permissions.allow.iter().any(|t| t == name) {
            return Permission::Allow;
        }
        if self.cfg.permissions.ask.iter().any(|t| t == name) {
            return Permission::Ask;
        }
        Permission::Deny
    }

    /// §9 compaction check, then one provider turn. Returns true when the
    /// model produced no tool calls (turn ends).
    pub fn turn(&mut self, emit: &mut dyn FnMut(&Event)) -> Result<bool> {
        self.maybe_compact(emit);
        // subscription tokens are short-lived — renew before they lapse
        if self.provider.kind == zproviders::ProviderKind::ChatGpt {
            if let Some(t) = crate::chatgpt::fresh() {
                self.provider.api_key = Some(t.access);
                self.provider.account_id = Some(t.account_id);
            }
        }
        let mut text = String::new();
        let mut calls: Vec<Block> = Vec::new();
        let mut usage = Usage::default();
        let req = Request {
            system: self.system.clone(),
            tools: &self.tools,
            messages: &self.messages,
            session_id: &self.session.id,
            max_tokens: 4096,
        };
        let mut err: Option<String> = None;
        let delta = &mut self.on_delta;
        complete(
            &self.provider,
            &req,
            &mut |c| match c {
                Chunk::Text(t) => {
                    if let Some(f) = delta.as_mut() {
                        f(&t);
                    }
                    text.push_str(&t)
                }
                Chunk::ToolUse { id, name, input } => {
                    calls.push(Block::ToolUse { id, name, input })
                }
                Chunk::Done(u) => {
                    // providers may emit several Done chunks (input est, then output)
                    usage.input += u.input;
                    usage.cache_read += u.cache_read;
                    usage.cache_write += u.cache_write;
                    usage.output += u.output;
                }
                Chunk::Err(e) => err = Some(e),
            },
            &self.allowed,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
        if let Some(e) = err {
            bail!("provider stream error: {e}");
        }
        self.last_usage = usage;
        let cost = self.prices.cost(&usage);
        self.cum_usd += cost.unwrap_or(0.0);
        let ev = self.session.append(
            "assistant",
            json!({
                "text": text,
                "tool_calls": calls.iter().map(|b| match b {
                    Block::ToolUse{id,name,input} => json!({"id":id,"name":name,"input":input}),
                    _ => json!(null),
                }).collect::<Vec<_>>(),
            }),
        );
        emit(&ev);
        let uev = self.session.append(
            "usage",
            json!({
                "input": usage.input, "cache_read": usage.cache_read,
                "cache_write": usage.cache_write, "output": usage.output,
                "usd": cost, "cum_usd": self.cum_usd,
            }),
        );
        emit(&uev);
        eprintln!(
            "turn · in {} · cache_r {} · cache_w {} · out {} · {} · Σ ${:.4} · ctx n/a",
            usage.input,
            usage.cache_read,
            usage.cache_write,
            usage.output,
            cost.map(|c| format!("${c:.4}"))
                .unwrap_or_else(|| "$?".into()),
            self.cum_usd,
        );
        // append assistant message
        let mut blocks = Vec::new();
        if !text.is_empty() {
            blocks.push(Block::Text { text });
        }
        blocks.extend(calls.iter().cloned());
        self.messages.push(Message {
            role: Role::Assistant,
            content: blocks,
        });
        Ok(calls.is_empty())
    }

    /// Execute one tool call; returns the tool-result text.
    pub fn exec_tool(
        &mut self,
        id: &str,
        name: &str,
        input: &Value,
        emit: &mut dyn FnMut(&Event),
    ) -> (String, bool) {
        self.session
            .append("tool_call", json!({"id":id,"name":name,"input":input}));
        // §11 pre_tool hooks — exit 2 blocks with stderr as the result
        if let Some(blocked) =
            crate::hooks::run(&self.cfg, "pre_tool", name, input, &self.session.id.clone())
        {
            self.session.append(
                "tool_result",
                json!({"tool_use_id":id,"name":name,"content":blocked,"is_error":true,"blocked_by":"hook"}),
            );
            self.messages.push(Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    tool_use_id: id.into(),
                    content: blocked.clone(),
                    is_error: true,
                }],
            });
            return (blocked, true);
        }
        if let Permission::Ask = self.permitted(name) {
            self.session
                .append("permission_request", json!({"tool": name, "input": input}));
        }
        let (text, is_error) = match self.permitted(name) {
            Permission::Deny => (
                format!(
                    "permission denied: {name} is not allowed in {} mode",
                    self.mode.name()
                ),
                true,
            ),
            Permission::Ask => {
                let granted = self.asker.as_mut().map(|f| f(name)).unwrap_or(false); // headless: ask == deny (contract §3)
                let ev = self.session.append(
                    "permission_decision",
                    json!({"tool": name, "input": input, "granted": granted}),
                );
                emit(&ev);
                if granted {
                    self.dispatch(name, input, emit)
                } else {
                    (
                        format!("permission denied: {name} requires operator approval"),
                        true,
                    )
                }
            }
            Permission::Allow => self.dispatch(name, input, emit),
        };
        let ev = self.session.append(
            "tool_result",
            json!({"tool_use_id":id,"name":name,"content":text,"is_error":is_error}),
        );
        emit(&ev);
        crate::hooks::run(
            &self.cfg,
            "post_tool",
            name,
            &json!({"input": input, "is_error": is_error}),
            &self.session.id.clone(),
        );
        self.messages.push(Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: id.into(),
                content: text.clone(),
                is_error,
            }],
        });
        (text, is_error)
    }

    fn dispatch(
        &mut self,
        name: &str,
        input: &Value,
        emit: &mut dyn FnMut(&Event),
    ) -> (String, bool) {
        match name {
            "submit_plan" => {
                let plan = input["plan"].as_str().unwrap_or("").to_string();
                self.pending_plan = Some(plan.clone());
                let ev = self.session.append("plan", json!({"plan": plan}));
                emit(&ev);
                (format!("plan submitted ({} chars)", plan.len()), false)
            }
            "request_escalation" => {
                let cap = input["capability"].as_str().unwrap_or("").to_string();
                let reason = input["reason"].as_str().unwrap_or("");
                let allowed = self.ctx.escalations.iter().any(|e| e == &cap);
                let ev = self.session.append(
                    "permission_decision",
                    json!({"tool":"request_escalation","capability":cap,"granted":allowed}),
                );
                emit(&ev);
                if allowed {
                    if cap == "network" {
                        self.ctx.net_allow = true;
                    }
                    (format!("escalation granted: {cap}"), false)
                } else {
                    (
                        format!("escalation denied: {cap} ({reason}) — rerun with --allow-escalation {cap}"),
                        true,
                    )
                }
            }
            "task" => {
                let prompt = input["prompt"].as_str().unwrap_or("").to_string();
                let worktree = input["worktree"].as_bool().unwrap_or(false);
                let result = self.run_subagent(&prompt, worktree, emit);
                (result, false)
            }
            _ => {
                let out = ztools::run(&mut self.ctx, name, input);
                (out.clone(), out.starts_with("ERROR"))
            }
        }
    }

    /// `task` subagent: fresh conversation over the same stable prefix,
    /// child session file linked via events (contract §10).
    /// `worktree:true` runs it in `git worktree add COSMOS_CLI_HOME\worktrees\
    /// <session>\<child>` on a `cosmos-task-<child>` branch.
    fn run_subagent(
        &mut self,
        prompt: &str,
        worktree: bool,
        emit: &mut dyn FnMut(&Event),
    ) -> String {
        let home = crate::cli_home();
        let mut child = match Session::create(&home, &self.session.harness.clone()) {
            Ok(s) => s,
            Err(e) => return format!("subagent spawn failed: {e}"),
        };
        let mut wt_note = String::new();
        if worktree {
            match self.add_worktree(&home, &child.id) {
                Ok(branch) => wt_note = format!("\n[worktree branch: {branch}]"),
                Err(e) => wt_note = format!("\n[worktree unavailable: {e}]"),
            }
        }
        let ev = self.session.append(
            "subagent_start",
            json!({"child_session": child.id,
                   "worktree": self.worktrees.last().map(|(p, _)| p.to_string_lossy().to_string()),
                   "prompt": prompt.chars().take(200).collect::<String>()}),
        );
        emit(&ev);
        child.append("session_start", json!({"model": self.model_alias, "mode": "agent",
                                           "sandbox": "appcontainer", "boot_tokens": self.boot_tokens,
                                           "parent_session": self.session.id}));
        // Worktree subagents run their tools against the worktree root.
        let mut child_ctx = self.worktrees.last().map(|(p, _)| self.child_ctx(p));
        let mut msgs = vec![Message::text(Role::User, prompt.to_string())];
        let tools = self.tools.clone();
        for _ in 0..8 {
            let mut text = String::new();
            let mut calls: Vec<Block> = Vec::new();
            let req = Request {
                system: self.system.clone(),
                tools: &tools,
                messages: &msgs,
                session_id: &child.id,
                max_tokens: 2048,
            };
            if complete(
                &self.provider,
                &req,
                &mut |c| match c {
                    Chunk::Text(t) => text.push_str(&t),
                    Chunk::ToolUse { id, name, input } => {
                        calls.push(Block::ToolUse { id, name, input })
                    }
                    _ => {}
                },
                &self.allowed,
            )
            .is_err()
            {
                self.subagent_done(&child.id, emit);
                return format!("subagent provider error{wt_note}");
            }
            let mut blocks = Vec::new();
            if !text.is_empty() {
                blocks.push(Block::Text { text: text.clone() });
            }
            let call_count = calls.len();
            blocks.extend(calls.iter().cloned());
            msgs.push(Message {
                role: Role::Assistant,
                content: blocks,
            });
            if call_count == 0 {
                self.subagent_done(&child.id, emit);
                return format!("{}{wt_note}", text.chars().take(4000).collect::<String>());
            }
            for b in calls {
                if let Block::ToolUse { id, name, input } = b {
                    // subagents never get write tools
                    let tctx = match child_ctx.as_mut() {
                        Some(c) => c,
                        None => &mut self.ctx,
                    };
                    let out = if PLAN_TOOLS.contains(&name.as_str()) {
                        ztools::run(tctx, &name, &input)
                    } else {
                        format!("subagent tool denied: {name}")
                    };
                    msgs.push(Message {
                        role: Role::User,
                        content: vec![Block::ToolResult {
                            tool_use_id: id,
                            content: out,
                            is_error: false,
                        }],
                    });
                }
            }
        }
        self.subagent_done(&child.id, emit);
        format!("subagent turn cap reached{wt_note}")
    }

    /// `git worktree add COSMOS_CLI_HOME\worktrees\<session>\<child>` on a new
    /// `cosmos-task-<child>` branch. Returns the branch name.
    fn add_worktree(&mut self, home: &Path, child_id: &str) -> Result<String> {
        let wt = home.join("worktrees").join(&self.session.id).join(child_id);
        let branch = format!("cosmos-task-{child_id}");
        let out = std::process::Command::new("git")
            .args([
                "-C",
                &self.ctx.workspace.to_string_lossy(),
                "worktree",
                "add",
                &wt.to_string_lossy(),
                "-b",
                &branch,
            ])
            .output()?;
        if !out.status.success() {
            anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
        }
        self.worktrees.push((wt, branch.clone()));
        Ok(branch)
    }

    /// Tool context rooted at a worktree path, carrying over policy settings.
    fn child_ctx(&self, workspace: &Path) -> ToolContext {
        let mut c = ToolContext::new(workspace);
        c.sandbox_bin = self.ctx.sandbox_bin.clone();
        c.meridian_bin = self.ctx.meridian_bin.clone();
        c.meridian_vault = self.ctx.meridian_vault.clone();
        c.escalations = self.ctx.escalations.clone();
        c.net_allow = self.ctx.net_allow;
        c.sandbox_degraded = self.ctx.sandbox_degraded.clone();
        c.no_sandbox = self.ctx.no_sandbox;
        c.large_read_lines = self.ctx.large_read_lines;
        c.mcp_servers = self.ctx.mcp_servers.clone();
        c
    }

    fn subagent_done(&mut self, child_id: &str, emit: &mut dyn FnMut(&Event)) {
        let ev = self
            .session
            .append("subagent_end", json!({"child_session": child_id}));
        emit(&ev);
    }

    /// Append a user message (TUI path — headless does this inline).
    pub fn send_user(&mut self, text: &str, emit: &mut dyn FnMut(&Event)) {
        let ev = self.session.append("user", json!({"text": text}));
        emit(&ev);
        self.messages.push(Message::text(Role::User, text));
    }

    /// `/approve` — pin the pending plan as the next user message and switch
    /// to agent mode (contract §3).
    pub fn approve_plan(&mut self, emit: &mut dyn FnMut(&Event)) {
        if let Some(plan) = self.pending_plan.take() {
            self.mode = Mode::Agent;
            let ev = self
                .session
                .append("mode", json!({"from": "plan", "to": "agent"}));
            emit(&ev);
            self.messages.push(Message::text(
                Role::User,
                format!("[approved plan]\n{plan}"),
            ));
        }
    }

    /// §9: compact when projected input > 80% of the window (chars/4 est).
    fn maybe_compact(&mut self, emit: &mut dyn FnMut(&Event)) {
        let est = |m: &Message| -> u64 {
            m.content
                .iter()
                .map(|b| match b {
                    Block::Text { text } => (text.len() / 4) as u64,
                    Block::ToolResult { content, .. } => (content.len() / 4) as u64,
                    _ => 8,
                })
                .sum()
        };
        let total: u64 = self.messages.iter().map(est).sum::<u64>() + self.boot_tokens;
        if total <= (self.context_window as f64 * 0.8) as u64 {
            return;
        }
        let before = total;
        let keep_head = 2usize;
        let keep_tail = 6usize;
        if self.messages.len() <= keep_head + keep_tail {
            return;
        }
        let dropped = self.messages.len() - keep_head - keep_tail;
        let mut compacted: Vec<Message> = self.messages[..keep_head].to_vec();
        compacted.push(Message::text(
            Role::User,
            format!("[compaction: {dropped} earlier messages elided — see session log for detail]"),
        ));
        compacted.extend_from_slice(&self.messages[self.messages.len() - keep_tail..]);
        self.messages = compacted;
        let after: u64 = self.messages.iter().map(est).sum::<u64>() + self.boot_tokens;
        crate::hooks::run(
            &self.cfg,
            "pre_compact",
            "",
            &json!({"before_tokens": before}),
            &self.session.id.clone(),
        );
        let ev = self.session.append(
            "compaction",
            json!({"before_tokens": before, "after_tokens": after, "reason": "context>80%"}),
        );
        emit(&ev);
    }
}

/// Contract §10: worktrees are removed on session end only if clean.
impl Drop for Agent {
    fn drop(&mut self) {
        for (path, branch) in self.worktrees.drain(..) {
            let clean = std::process::Command::new("git")
                .args(["-C", &path.to_string_lossy(), "status", "--porcelain"])
                .output()
                .map(|o| o.status.success() && o.stdout.is_empty())
                .unwrap_or(false);
            if !clean {
                continue;
            }
            let ws = self.ctx.workspace.to_string_lossy().to_string();
            let _ = std::process::Command::new("git")
                .args(["-C", &ws, "worktree", "remove", &path.to_string_lossy()])
                .output();
            let _ = std::process::Command::new("git")
                .args(["-C", &ws, "branch", "-D", &branch])
                .output();
            // drop the now-empty worktrees/<session> dir if nothing else uses it
            if let Some(parent) = path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
}

enum Permission {
    Allow,
    Ask,
    Deny,
}

/// Headless `-p` run (contract §7). Returns the process exit code.
pub fn run_headless(
    opts: &RunOpts,
    harness: &str,
    cwd: &Path,
    emit: &mut dyn FnMut(&Event),
) -> Result<i32> {
    let prompt = match &opts.prompt {
        Some(p) if p == "-" => {
            let mut s = String::new();
            use std::io::Read;
            std::io::stdin().read_to_string(&mut s)?;
            s
        }
        Some(p) => p.clone(),
        None => bail!("run_headless needs a prompt"),
    };
    let mut agent = Agent::new(opts, harness, cwd)?;
    let sandbox_state = if agent.ctx.no_sandbox {
        "off"
    } else {
        "appcontainer"
    };
    let ev = agent.session.append(
        "session_start",
        json!({
            "model": agent.model_alias, "mode": agent.mode.name(),
            "sandbox": sandbox_state, "boot_tokens": agent.boot_tokens,
            "resume": opts.resume,
        }),
    );
    emit(&ev);
    crate::hooks::run(
        &agent.cfg,
        "session_start",
        "",
        &json!({}),
        &agent.session.id.clone(),
    );
    let user = agent.session.append("user", json!({"text": prompt}));
    emit(&user);
    crate::hooks::run(
        &agent.cfg,
        "user_prompt",
        "",
        &json!({"text": prompt}),
        &agent.session.id.clone(),
    );
    agent.messages.push(Message::text(Role::User, prompt));

    let max_turns = opts.max_turns.unwrap_or(16);
    for _ in 0..max_turns {
        if let Some(max) = opts.max_usd {
            if agent.cum_usd >= max {
                return Ok(zproto::exit::BUDGET);
            }
        }
        let done = agent.turn(emit)?;
        if done {
            let ev = agent
                .session
                .append("session_end", json!({"reason":"done"}));
            emit(&ev);
            return Ok(zproto::exit::DONE);
        }
        // collect the tool calls we just emitted
        let calls: Vec<(String, String, Value)> = agent
            .messages
            .last()
            .map(|m| {
                m.content
                    .iter()
                    .filter_map(|b| match b {
                        Block::ToolUse { id, name, input } => {
                            Some((id.clone(), name.clone(), input.clone()))
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut denied_exit = false;
        for (id, name, input) in calls {
            let (text, is_error) = agent.exec_tool(&id, &name, &input, emit);
            if name == "submit_plan" && !is_error {
                if agent.opts.approve_plan {
                    agent.mode = Mode::Agent;
                    agent.messages.push(Message::text(
                        Role::User,
                        format!("[approved plan]\n{}", input["plan"].as_str().unwrap_or("")),
                    ));
                } else {
                    let ev = agent
                        .session
                        .append("session_end", json!({"reason":"plan_submitted"}));
                    emit(&ev);
                    return Ok(zproto::exit::DONE);
                }
            }
            if is_error && text.starts_with("permission denied") {
                denied_exit = true;
            }
        }
        if denied_exit {
            let ev = agent
                .session
                .append("session_end", json!({"reason":"permission_denied"}));
            emit(&ev);
            return Ok(zproto::exit::DENIED);
        }
    }
    let ev = agent
        .session
        .append("session_end", json!({"reason":"max_turns"}));
    emit(&ev);
    Ok(zproto::exit::FAILED)
}

/// Emit a json-stream event on stdout when mode demands it.
pub fn emitter(mode: OutputMode) -> impl FnMut(&Event) {
    move |ev: &Event| {
        if mode == OutputMode::JsonStream {
            println!("{}", ev.to_line());
        } else if mode == OutputMode::Json {
            println!("{}", serde_json::to_string_pretty(ev).unwrap());
        } else {
            match ev.kind.as_str() {
                "assistant" => {
                    if let Some(t) = ev.data["text"].as_str() {
                        println!("{t}");
                    }
                }
                "tool_result" => {
                    if let Some(c) = ev.data["content"].as_str() {
                        eprintln!(
                            "  [{}] {}",
                            ev.data["name"].as_str().unwrap_or(""),
                            c.chars().take(200).collect::<String>()
                        );
                    }
                }
                "session_start" | "session_end" => {}
                _ => {}
            }
        }
    }
}
