//! The agent loop (contract §4-§9). Headless `-p` runs through `run_agent`;
//! the TUI drives the same loop through `Agent` one turn at a time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use zproto::{Block, Chunk, Event, Message, OutputMode, Role, RunOpts, Usage};
use zproviders::{complete, ProviderCfg, ProviderKind, Request};
use ztools::{is_readonly, ToolContext, PLAN_TOOLS};

use crate::compact::{self, CompactCfg, Trigger, ViewOp};
use crate::config::Config;
use crate::improve::{self, SelfImproveCfg};
use crate::session::Session;
use crate::world::World;

fn dispatch_complete(
    cfg: &ProviderCfg,
    req: &Request,
    sink: &mut dyn FnMut(Chunk),
    allowed: &[String],
) -> Result<()> {
    #[cfg(feature = "devin-direct")]
    if cfg.kind == ProviderKind::Devin {
        return crate::devin::complete(cfg, req, sink);
    }
    complete(cfg, req, sink, allowed)
}

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
    pub world: World,
    pub last_model_at: Option<Instant>,
    pub prefix_hash: String,
    /// Sticky compaction ops — the model sees `project(messages, view_ops)`.
    pub view_ops: Vec<ViewOp>,
    /// L5 ran at the previous check — a refill right after is thrash.
    pub l5_last: bool,
    /// set by thrash; only an operator `/compact` runs L5 again
    pub auto_l5_off: bool,
    pub improve: SelfImproveCfg,
    pub compact_cfg: CompactCfg,
    pub sandbox_mode: String,
    pub ask_mode: String,
    pub force_compact: bool,
    pub compact_focus: Option<String>,
    pub doom: Vec<u64>,
    pub stale_hits: u32,
    /// Set by the UI to interrupt a running turn (ACP turns poll it).
    pub interrupt: Option<Arc<AtomicBool>>,
    /// The vendor agent process for an ACP provider, started on first use.
    acp: Option<crate::acp_client::AcpSession>,
    /// `messages[..acp_sent]` have already been given to the vendor agent.
    acp_sent: usize,
    pub home: PathBuf,
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
    AcceptEdits,
    Yolo,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Plan => "plan",
            Mode::Agent => "agent",
            Mode::AcceptEdits => "accept-edits",
            Mode::Yolo => "yolo",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Mode::Plan => Mode::Agent,
            Mode::Agent => Mode::AcceptEdits,
            Mode::AcceptEdits => Mode::Yolo,
            Mode::Yolo => Mode::Plan,
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "plan" => Mode::Plan,
            "yolo" => Mode::Yolo,
            "accept-edits" | "accept_edits" => Mode::AcceptEdits,
            _ => Mode::Agent,
        }
    }
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

/// The tools block exactly as serialized for the wire (prefix hashing).
fn tools_json(tools: &[zproto::ToolSpec]) -> String {
    serde_json::to_string(tools).unwrap_or_default()
}

fn load_prompt(value: &str) -> Result<String> {
    if let Some(path) = value.strip_prefix('@') {
        return Ok(std::fs::read_to_string(path)
            .with_context(|| format!("read system prompt {path}"))?);
    }
    Ok(value.to_owned())
}

impl Agent {
    /// Create or resume an agent. `harness` is `cosmostui`/`zdev`/`zagy`.
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
            Mode::parse(opts.mode.as_deref().unwrap_or("agent"))
        };

        let mut ctx = ToolContext::new(&workspace);
        ctx.allowed_roots = opts
            .add_dirs
            .iter()
            .map(PathBuf::from)
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
            .collect();
        ctx.mcp_servers = cfg.mcp_servers();
        ctx.allowed_hosts = cfg.allowed_hosts();
        ctx.large_read_lines = cfg.bridge.large_read_lines;
        ctx.no_sandbox = opts.no_sandbox;
        ctx.escalations = opts.allow_escalation.clone();
        ctx.net_allow = opts.allow_escalation.iter().any(|e| e == "network");
        ctx.home = home.clone();
        ctx.sandbox_mode = opts
            .sandbox
            .clone()
            .unwrap_or_else(|| cfg.sandbox.mode.clone());
        if ctx.sandbox_mode == "full" && !opts.no_sandbox {
            // full still sandboxed unless --no-sandbox
        }

        let resume_id = match opts.resume.as_deref() {
            Some("__latest__") => Some(
                Session::latest_for_cwd(&home, &workspace)
                    .ok_or_else(|| anyhow::anyhow!(
                        "no previous session in {} to continue",
                        workspace.display()
                    ))?,
            ),
            Some(id) => Some(id.to_owned()),
            None => None,
        };
        // `--resume <id>@<event>` resumes from any event, compaction nodes included
        let (session, messages, view_ops) = if let Some(id) = resume_id {
            let (id, at) = match id.split_once('@') {
                Some((i, a)) => (i.to_string(), Some(a.to_string())),
                None => (id, None),
            };
            let s = Session::open_at(&home, &id, at.as_deref())?;
            let (msgs, ops) = Session::replay_view(&home, &id, s.leaf.as_deref())?;
            (s, msgs, ops)
        } else {
            (Session::create(&home, harness)?, Vec::new(), Vec::new())
        };
        // resumed sessions keep their own harness name in history; new events
        // carry ours — that IS the handoff mechanism (shared home).
        let mut session = session;
        session.harness = harness.into();

        // §4 layers: 1 snippet, 2 tools, 3 boot — fixed for the session
        let boot = crate::boot::build(&workspace, &home);
        let boot_tokens = boot.tokens;
        let tools = ztools::wire_tools(mode == Mode::Plan);
        let mut system = match &opts.system_prompt {
            Some(prompt) => vec![load_prompt(prompt)?],
            None => vec![crate::system_snippet()],
        };
        system.push(boot.text);
        if !opts.add_dirs.is_empty() {
            system.push(format!(
                "Additional tool roots granted for this run: {}",
                opts.add_dirs.join(", ")
            ));
        }
        if let Some(prompt) = &opts.append_system_prompt {
            system.push(load_prompt(prompt)?);
        }
        let prices = provider.prices;
        let context_window = provider.context;
        let allowed = cfg.allowed_hosts();
        let prefix = compact::prefix_hash(&system, &tools_json(&tools));
        ctx.session_id = session.id.clone();
        restore_read_state(&mut ctx, &messages);
        let world = World::load(&home, &session.id);
        if !world.goal.is_empty() {
            let frag = crate::world::boot_fragment(&world);
            if !frag.is_empty() && !system.iter().any(|s| s.contains("## World")) {
                system.push(frag);
            }
        }
        let sandbox_mode = ctx.sandbox_mode.clone();
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
            world,
            last_model_at: None,
            prefix_hash: prefix,
            view_ops,
            l5_last: false,
            auto_l5_off: false,
            improve: {
                let mut i = SelfImproveCfg {
                    enabled: cfg.self_improve.enabled,
                    write_skills: cfg.self_improve.write_skills,
                    write_preferences: cfg.self_improve.write_preferences,
                    write_corrections: cfg.self_improve.write_corrections,
                    require_gate: cfg.self_improve.require_gate,
                    allow_self_mutate: opts.allow_self_mutate,
                    auto_writes: 0,
                };
                if let Some(v) = opts.self_improve {
                    i.enabled = v;
                } else if opts.output != OutputMode::Text {
                    i.enabled = false;
                }
                i
            },
            compact_cfg: cfg.compact_cfg(),
            sandbox_mode,
            ask_mode: opts.ask.clone().unwrap_or_else(|| cfg.sandbox.ask.clone()),
            force_compact: false,
            compact_focus: None,
            doom: Vec::new(),
            stale_hits: 0,
            interrupt: None,
            acp: None,
            acp_sent: 0,
            home,
            cfg,
        })
    }

    /// Decide whether `name` may run (contract §3 + permission × sandbox dials).
    fn permitted(&self, name: &str) -> Permission {
        if self.mode == Mode::Yolo {
            return Permission::Allow;
        }
        if self.mode == Mode::Plan && !PLAN_TOOLS.contains(&name) {
            return Permission::Deny;
        }
        if self.mode == Mode::AcceptEdits && matches!(name, "write" | "edit") {
            return Permission::Allow;
        }
        if self.opts.allow.iter().any(|t| t == name) {
            return Permission::Allow;
        }
        if self.cfg.permissions.allow.iter().any(|t| t == name) {
            return Permission::Allow;
        }
        if self.cfg.permissions.ask.iter().any(|t| t == name) {
            if self.ask_mode == "never" {
                return Permission::Deny;
            }
            return Permission::Ask;
        }
        Permission::Deny
    }

    pub fn cycle_mode(&mut self, emit: &mut dyn FnMut(&Event)) {
        let from = self.mode.name();
        self.mode = self.mode.cycle();
        let ev = self
            .session
            .append("mode", json!({"from": from, "to": self.mode.name()}));
        emit(&ev);
        // the tools block never changes mid-session (cache); tell the model
        // about the mode in the conversation instead
        if self.mode == Mode::Plan && !self.tools.iter().any(|t| t.name == "submit_plan") {
            self.push_note(format!(
                "[plan mode: read-only tools only. When the plan is ready, call mcp_call {{\"server\": \"{}\", \"tool\": \"submit_plan\", \"arguments\": {{\"plan\": \"…\"}}}}]",
                ztools::SELF_SERVER
            ));
        }
    }

    pub fn compact_now(&mut self, focus: Option<&str>, emit: &mut dyn FnMut(&Event)) {
        self.force_compact = true;
        self.compact_focus = focus.map(|s| s.to_string());
        self.maybe_compact(emit);
        self.force_compact = false;
        self.compact_focus = None;
    }

    pub fn undo_last(&mut self, emit: &mut dyn FnMut(&Event)) -> String {
        match ztools::ckpt::restore_last(&self.home, &self.session.id) {
            Ok(p) => {
                let ev = self.session.append(
                    "checkpoint",
                    json!({"op": "restore", "path": p.display().to_string()}),
                );
                emit(&ev);
                format!("restored {}", p.display())
            }
            Err(e) => format!("undo failed: {e}"),
        }
    }

    pub fn skillify(&mut self, name: &str, emit: &mut dyn FnMut(&Event)) -> String {
        if !self.improve.enabled && !self.opts.yolo {
            return "self_improve disabled".into();
        }
        match improve::skillify(
            &self.home,
            name,
            &self.world,
            false,
            self.improve.allow_self_mutate,
        ) {
            Ok((p, detail)) => {
                let ev = self
                    .session
                    .append("self_improve", improve::event_data("skillify", detail));
                emit(&ev);
                format!("wrote {}", p.display())
            }
            Err(e) => e,
        }
    }

    /// Hash of layers 1–3 as they would be sent now.
    pub fn prefix_now(&self) -> String {
        compact::prefix_hash(&self.system, &tools_json(&self.tools))
    }

    /// Contract §4: the next request pays the whole prompt again (new model,
    /// or a resumed session's own prefix). Log it with the estimated cost.
    pub fn log_cache_break(&mut self, reason: &str, emit: &mut dyn FnMut(&Event)) {
        let prefix_chars: usize =
            self.system.iter().map(String::len).sum::<usize>() + tools_json(&self.tools).len();
        let tokens = prefix_chars as u64 / 4 + compact::estimate_tokens(&self.model_view(), 0);
        let price = if self.prices.cache_write > 0.0 {
            self.prices.cache_write
        } else {
            self.prices.input
        };
        let usd = (price > 0.0).then(|| tokens as f64 * price / 1_000_000.0);
        let ev = self.session.append(
            "cache_break",
            json!({"reason": reason, "tokens": tokens, "est_usd": usd}),
        );
        emit(&ev);
    }

    /// One turn on a subscription agent over ACP: the user text not yet sent
    /// goes to the vendor's agent, which works until it stops. Its replies
    /// stream to the UI; its tool activity is logged as `tool_call` events
    /// (`source: "acp"` — never replayed as our own tool turns); permission
    /// requests go to the operator. Always ends the turn.
    fn acp_turn(&mut self, emit: &mut dyn FnMut(&Event)) -> Result<bool> {
        let plugin = crate::acp_client::id_of(&self.provider.base_url)
            .context("ACP provider without an acp:// address")?
            .to_string();
        let from = self.acp_sent.min(self.messages.len());
        let fresh: Vec<String> = self.messages[from..]
            .iter()
            .filter(|m| m.role == Role::User)
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        if fresh.is_empty() {
            return Ok(true);
        }
        let mut prompt = fresh.join("\n\n");
        let mut session = match self.acp.take() {
            Some(s) => s,
            None => {
                let s = crate::acp_client::AcpSession::start(&plugin, &self.ctx.workspace, &self.provider.model)?;
                if let Some(note) = &s.model_note {
                    if let Some(f) = self.on_delta.as_mut() {
                        f(&format!("[{note}]\n"));
                    }
                }
                // a resumed session or a model switch: the vendor never saw
                // what came before, so it gets a short recap
                let recap = acp_recap(&self.messages[..from]);
                if !recap.is_empty() {
                    prompt = format!("[Earlier in this session]\n{recap}\n\n[Now]\n{prompt}");
                }
                s
            }
        };
        self.acp_sent = self.messages.len();

        let mut text = String::new();
        let mut tools: Vec<(String, String, String, Value, String, String)> = Vec::new();
        let mode = self.mode;
        let allow: Vec<String> = self.opts.allow.iter().chain(self.cfg.permissions.allow.iter()).cloned().collect();
        let interrupt = self.interrupt.clone();
        let result = {
            let delta = &mut self.on_delta;
            let asker = &mut self.asker;
            let log = &mut self.session;
            let mut on = |u: crate::acp_client::Update| match u {
                crate::acp_client::Update::Text(t) => {
                    if let Some(f) = delta.as_mut() {
                        f(&t);
                    }
                    text.push_str(&t);
                }
                crate::acp_client::Update::Tool { id, kind, title, status, input, output } => {
                    match tools.iter_mut().find(|t| t.0 == id) {
                        Some(t) => {
                            if !kind.is_empty() {
                                t.1 = kind;
                            }
                            if !title.is_empty() {
                                t.2 = title;
                            }
                            if !input.is_null() {
                                t.3 = input;
                            }
                            if !status.is_empty() {
                                t.4 = status;
                            }
                            if !output.is_empty() {
                                t.5 = output;
                            }
                        }
                        None => tools.push((id, kind, title, input, status, output)),
                    }
                }
            };
            let mut decide = |title: &str, params: &Value| -> bool {
                let kind = params["toolCall"]["kind"].as_str().unwrap_or("");
                let label = format!("{plugin}: {title}");
                log.append("permission_request", json!({"tool": label, "kind": kind, "source": "acp"}));
                let granted = mode == Mode::Yolo
                    || allow.iter().any(|a| a == "acp" || a == &plugin || (!kind.is_empty() && a == kind))
                    || asker.as_mut().map(|f| f(&label)).unwrap_or(false);
                log.append("permission_decision", json!({"tool": label, "granted": granted, "source": "acp"}));
                granted
            };
            session.prompt(&prompt, &mut on, &mut decide, interrupt.as_deref())
        };
        for (id, kind, title, input, status, output) in tools {
            let out: String = output.chars().take(2000).collect();
            let ev = self.session.append(
                "tool_call",
                json!({"id": id, "name": format!("{plugin}:{}", if kind.is_empty() { "tool" } else { &kind }),
                       "title": title, "input": input, "status": status, "output": out, "source": "acp"}),
            );
            emit(&ev);
        }
        let stop = match result {
            Ok(stop) => {
                self.acp = Some(session);
                stop
            }
            // the agent process is gone or broken — start fresh next turn
            Err(e) => return Err(e),
        };
        self.last_model_at = Some(Instant::now());
        let ev = self.session.append("assistant", json!({"text": text, "tool_calls": [], "stop_reason": stop}));
        emit(&ev);
        let uev = self.session.append(
            "usage",
            json!({"input": 0, "cache_read": 0, "cache_write": 0, "output": 0,
                   "usd": null, "cum_usd": self.cum_usd, "model": self.model_alias,
                   "billing": "subscription", "plugin": plugin}),
        );
        emit(&uev);
        if !text.is_empty() {
            self.messages.push(Message::text(Role::Assistant, text));
        }
        eprintln!("turn · {plugin} subscription · stop {stop}");
        Ok(true)
    }

    /// Exactly what the next request sends as the conversation (§9).
    pub fn model_view(&self) -> Vec<Message> {
        compact::project(&self.messages, &self.view_ops)
    }

    fn cache_ttl(&self) -> u64 {
        compact::cache_ttl_secs(&self.compact_cfg, self.provider.kind == ProviderKind::Anthropic)
    }

    pub fn context_report(&self) -> String {
        let tokens = compact::estimate_tokens(&self.model_view(), self.boot_tokens);
        let pct = if self.context_window == 0 {
            0
        } else {
            tokens * 100 / self.context_window
        };
        let warm = compact::cache_is_warm(
            self.last_model_at,
            &self.prefix_hash,
            &self.prefix_hash,
            Instant::now(),
            self.cache_ttl(),
        );
        format!(
            "{{\"tokens_in\":{tokens},\"tokens_budget\":{},\"percent\":{pct},\"cache_warm\":{warm}}}",
            self.context_window
        )
    }

    /// §9 compaction check, then one provider turn. Returns true when the
    /// model produced no tool calls (turn ends).
    pub fn turn(&mut self, emit: &mut dyn FnMut(&Event)) -> Result<bool> {
        if self.provider.kind == ProviderKind::Acp {
            return self.acp_turn(emit);
        }
        self.maybe_compact(emit);
        self.drain_jobs(emit);
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
        let projected = self.model_view();
        let req = Request {
            system: self.system.clone(),
            tools: &self.tools,
            messages: &projected,
            session_id: &self.session.id,
            max_tokens: 4096,
        };
        let mut err: Option<String> = None;
        let delta = &mut self.on_delta;
        dispatch_complete(
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
                    // providers emit one merged Done per call; summing keeps
                    // any multi-call provider (Devin) correct too
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
        self.last_model_at = Some(Instant::now());
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
                "model": self.model_alias, "turn_usd": cost,
            }),
        );
        emit(&uev);
        // append assistant message
        let mut blocks = Vec::new();
        if !text.is_empty() {
            blocks.push(Block::Text { text });
        }
        blocks.extend(calls.iter().cloned());
        // an empty reply is logged but not replayed — keep memory == replay
        if !blocks.is_empty() {
            self.messages.push(Message {
                role: Role::Assistant,
                content: blocks,
            });
        }
        let tokens = compact::estimate_tokens(&self.model_view(), self.boot_tokens);
        let pct = if self.context_window == 0 {
            0
        } else {
            tokens * 100 / self.context_window
        };
        eprintln!(
            "turn · in {} · cache_r {} · cache_w {} · out {} · {} · Σ ${:.4} · ctx {}%",
            usage.input,
            usage.cache_read,
            usage.cache_write,
            usage.output,
            cost.map(|c| format!("${c:.4}"))
                .unwrap_or_else(|| "$?".into()),
            self.cum_usd,
            pct,
        );
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
        // a deferred tool called through the built-in server is that tool
        let unwrapped = ztools::unwrap_self_call(name, input);
        let (name, input) = match &unwrapped {
            Some((n, i)) => (n.as_str(), i),
            None => (name, input),
        };
        let repaired = ztools::repair::repair(name, input);
        for r in &repaired.repairs {
            let ev = self.session.append(
                "tool_repair",
                json!({"from": r.from, "to": r.to, "reason": r.reason}),
            );
            emit(&ev);
        }
        if let Some(msg) = repaired.reject {
            self.session
                .append("tool_call", json!({"id":id,"name":name,"input":input}));
            let ev = self.session.append(
                "tool_result",
                json!({"tool_use_id":id,"name":name,"content":msg,"is_error":true}),
            );
            emit(&ev);
            self.messages.push(Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    tool_use_id: id.into(),
                    content: msg.clone(),
                    is_error: true,
                }],
            });
            return (msg, true);
        }
        let name = repaired.name.as_str();
        let input = &repaired.input;
        let h = compact::call_hash(name, input);
        self.doom.push(h);
        if self.doom.len() > 3 {
            self.doom.remove(0);
        }
        if self.doom.len() == 3 && self.doom[0] == self.doom[1] && self.doom[1] == self.doom[2] {
            let ev = self.session.append(
                "doom_loop",
                json!({"tool": name, "input": input}),
            );
            emit(&ev);
            let msg = "doom_loop: same tool+args fired 3 times with no file change — stop and ask";
            // log it so a resumed session replays the same conversation
            self.session.append(
                "tool_result",
                json!({"tool_use_id":id,"name":name,"content":msg,"is_error":true}),
            );
            self.messages.push(Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    tool_use_id: id.into(),
                    content: msg.into(),
                    is_error: true,
                }],
            });
            return (msg.into(), true);
        }
        self.ctx.current_tool_id = id.to_string();
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
        // §9 L1: big results go to disk once; log and model keep the preview
        let text = compact::budget_result(&self.home, name, text);
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
                let write = input["write"].as_bool().unwrap_or(false);
                let worktree = input["worktree"].as_bool().unwrap_or(write);
                let result = self.run_subagent(&prompt, worktree, emit);
                (result, false)
            }
            "context_remaining" => (self.context_report(), false),
            "new_context" => {
                self.compact_now(None, emit);
                (format!("reseeding from world.md\n{}", self.world.render()), false)
            }
            _ => {
                let out = ztools::run(&mut self.ctx, name, input);
                let err = out.starts_with("ERROR") || out.contains("STALE");
                if err && out.contains("STALE") {
                    self.stale_hits += 1;
                    if self.stale_hits >= 2
                        && self.improve.enabled
                        && self.improve.write_corrections
                        && self.improve.auto_writes < improve::AUTO_WRITE_CAP
                    {
                        self.improve.auto_writes += 1;
                        let ev = self.session.append(
                            "self_improve",
                            improve::event_data(
                                "correction",
                                json!({"text": improve::correction_row(name, "re-read then edit")}),
                            ),
                        );
                        emit(&ev);
                    }
                }
                if !err {
                    if matches!(name, "write" | "edit") {
                        if let Some(p) = input["path"].as_str() {
                            if let Ok(bytes) = std::fs::read(self.ctx.workspace.join(p)) {
                                self.world.touch_file(p, &bytes);
                                self.world.save(&self.home, &self.session.id);
                            }
                            if let Some(diag) = ztools::verify::after_write(&self.ctx.workspace, p) {
                                let ev = self.session.append(
                                    "verify",
                                    json!({"path": p, "diagnostics": diag}),
                                );
                                emit(&ev);
                            }
                        }
                    }
                    if name == "bash" {
                        if let Some(c) = input["command"].as_str() {
                            if c.contains("test") {
                                self.world.touch_test(c, !err);
                                self.world.save(&self.home, &self.session.id);
                            }
                        }
                    }
                }
                (out.clone(), err)
            }
        }
    }

    fn drain_jobs(&mut self, emit: &mut dyn FnMut(&Event)) {
        for j in ztools::jobs::drain_ended() {
            let ev = self.session.append(
                "job_end",
                json!({
                    "task_id": j.task_id,
                    "exit_code": j.exit_code,
                    "log_path": j.log_path.display().to_string(),
                }),
            );
            emit(&ev);
        }
    }

    /// Run read-only tools concurrently (cap 8); serialize writes/bash/mcp/task.
    pub fn exec_tools_batch(
        &mut self,
        calls: &[(String, String, Value)],
        emit: &mut dyn FnMut(&Event),
    ) -> Vec<(String, String, bool)> {
        let mut out = Vec::new();
        let mut ro: Vec<(String, String, Value)> = Vec::new();
        let mut wr: Vec<(String, String, Value)> = Vec::new();
        for c in calls {
            if is_readonly(&c.1) {
                ro.push(c.clone());
            } else {
                wr.push(c.clone());
            }
        }
        if ro.len() > 1 {
            let ws = self.ctx.workspace.clone();
            let home = self.ctx.home.clone();
            let sid = self.ctx.session_id.clone();
            let large = self.ctx.large_read_lines;
            let mode = self.ctx.sandbox_mode.clone();
            let slice: Vec<(String, String, Value)> = ro.iter().take(8).cloned().collect();
            let rest = if ro.len() > 8 { ro[8..].to_vec() } else { Vec::new() };
            let results = std::thread::scope(|s| {
                let mut handles = Vec::new();
                for (id, name, input) in &slice {
                    let ws = &ws;
                    let home = &home;
                    let sid = &sid;
                    let mode = &mode;
                    handles.push(s.spawn(move || {
                        let mut c = ToolContext::new(ws);
                        c.home = home.clone();
                        c.session_id = sid.clone();
                        c.large_read_lines = large;
                        c.sandbox_mode = mode.clone();
                        c.current_tool_id = id.clone();
                        let text = ztools::run(&mut c, name, input);
                        (id.clone(), name.clone(), input.clone(), text, c.read_state)
                    }));
                }
                handles.into_iter().map(|h| h.join().unwrap()).collect::<Vec<_>>()
            });
            for (id, name, input, text, state) in results {
                for (k, v) in state {
                    self.ctx.read_state.insert(k, v);
                }
                let text = compact::budget_result(&self.home, &name, text);
                self.session
                    .append("tool_call", json!({"id":id,"name":name,"input":input}));
                let err = text.starts_with("ERROR");
                let ev = self.session.append(
                    "tool_result",
                    json!({"tool_use_id":id,"name":name,"content":text,"is_error":err}),
                );
                emit(&ev);
                self.messages.push(Message {
                    role: Role::User,
                    content: vec![Block::ToolResult {
                        tool_use_id: id,
                        content: text.clone(),
                        is_error: err,
                    }],
                });
                out.push((name, text, err));
            }
            for (id, name, input) in rest {
                let (text, err) = self.exec_tool(&id, &name, &input, emit);
                out.push((name, text, err));
            }
        } else {
            for (id, name, input) in &ro {
                let (text, err) = self.exec_tool(id, name, input, emit);
                out.push((name.clone(), text, err));
            }
        }
        for (id, name, input) in &wr {
            let (text, err) = self.exec_tool(id, name, input, emit);
            out.push((name.clone(), text, err));
        }
        out
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
        let brief = format!(
            "goal: {}\nconstraints: worktree={worktree}\nfiles: {}\nsuccess: finish the task and return a ≤4000 char summary\n\n{prompt}",
            self.world.goal,
            self.world
                .files
                .iter()
                .map(|(p, _)| p.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        let mut msgs = vec![Message::text(Role::User, brief)];
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
            if dispatch_complete(
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
        c.escalations = self.ctx.escalations.clone();
        c.net_allow = self.ctx.net_allow;
        c.sandbox_degraded = self.ctx.sandbox_degraded.clone();
        c.no_sandbox = self.ctx.no_sandbox;
        c.large_read_lines = self.ctx.large_read_lines;
        c.mcp_servers = self.ctx.mcp_servers.clone();
        c.home = self.ctx.home.clone();
        c.session_id = self.ctx.session_id.clone();
        c.sandbox_mode = self.ctx.sandbox_mode.clone();
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
        self.world.set_goal_if_empty(text);
        self.world.save(&self.home, &self.session.id);
        self.messages.push(Message::text(Role::User, text));
    }

    /// A harness-authored user message. Logged (`synthetic`) so a resumed
    /// session replays exactly the conversation the model saw.
    pub fn push_note(&mut self, text: String) {
        self.session
            .append("user", json!({"text": text, "synthetic": true}));
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
            self.push_note(format!("[approved plan]\n{plan}"));
        }
    }

    /// §9 checkpoint before every model call. Does nothing on a warm cache
    /// under the hot threshold; otherwise decides sticky L2–L4 ops and, if
    /// still over, an L5 world summary. Every op is logged so resume replays
    /// the same view.
    fn maybe_compact(&mut self, emit: &mut dyn FnMut(&Event)) {
        let prefix_now = self.prefix_now();
        let warm = compact::cache_is_warm(
            self.last_model_at,
            &prefix_now,
            &self.prefix_hash,
            Instant::now(),
            self.cache_ttl(),
        );
        let view = self.model_view();
        let tokens = compact::estimate_tokens(&view, self.boot_tokens);
        let why = if self.force_compact {
            Some(Trigger::Operator)
        } else {
            compact::trigger(
                tokens,
                self.context_window,
                self.compact_cfg.reserved_output_tokens,
                warm,
                self.compact_cfg.trigger_ratio,
            )
        };
        let l5_last = std::mem::replace(&mut self.l5_last, false);
        let Some(why) = why else {
            return;
        };
        crate::hooks::run(&self.cfg, "pre_compact", "", &json!({}), &self.session.id.clone());
        // thrash: L5 just ran and the context is already over the line again
        if why == Trigger::Hot && l5_last && !self.auto_l5_off {
            self.auto_l5_off = true;
            let ev = self.session.append(
                "compact_thrash",
                json!({"tokens": tokens, "window": self.context_window,
                       "note": "auto-compact off until /compact"}),
            );
            emit(&ev);
        }
        if why == Trigger::Operator {
            self.auto_l5_off = false;
        }
        let cp = compact::checkpoint(
            &self.messages,
            &self.view_ops,
            self.boot_tokens,
            self.context_window,
            &self.compact_cfg,
            why,
        );
        for op in cp.ops {
            let data = match &op {
                ViewOp::Snip { dropped, .. } => json!({"view": op, "dropped": dropped}),
                ViewOp::Stub { layer, stubs } => json!({"view": op, "layer": layer, "count": stubs.len()}),
                ViewOp::World { .. } => json!({"view": op}),
            };
            let ev = self.session.append(op.kind(), data);
            emit(&ev);
            self.view_ops.push(op);
        }
        if !cp.needs_world || (self.auto_l5_off && why != Trigger::Operator) {
            return;
        }
        let focus = self.compact_focus.clone();
        let md = self.summarize(focus.as_deref());
        let dest = self.home.join("world").join(format!("{}.md", self.session.id));
        if let Some(p) = dest.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        let _ = std::fs::write(&dest, &md);
        self.world.apply_compact_md(&md);
        self.world.save(&self.home, &self.session.id);
        let op = compact::plan_world(&self.messages, &self.view_ops, md);
        let mut ops = self.view_ops.clone();
        ops.push(op.clone());
        let after = compact::estimate_tokens(&compact::project(&self.messages, &ops), self.boot_tokens);
        let ev = self.session.append(
            "compaction",
            json!({
                "view": op,
                "before_tokens": cp.tokens_before,
                "after_tokens": after,
                "reason": match why { Trigger::Operator => "operator", Trigger::Hot => "auto", Trigger::Cold => "cold_cache" },
                "focus": focus,
            }),
        );
        emit(&ev);
        self.view_ops.push(op);
        self.l5_last = why != Trigger::Operator;
    }

    /// L5 summary. One model call over the *current* prefix and view (so it
    /// reads from the prompt cache) plus the template as the final user turn.
    /// Fixture provider, a tool call instead of text, or any error → the
    /// deterministic extractor.
    fn summarize(&mut self, focus: Option<&str>) -> String {
        let view = self.model_view();
        let fallback = || compact::summarize_deterministic(&view, focus);
        if matches!(self.provider.kind, ProviderKind::Fixture | ProviderKind::Devin | ProviderKind::Acp) {
            return fallback();
        }
        let mut msgs = view.clone();
        let ask = match focus {
            Some(f) => format!("{}\nFocus on: {f}", compact::SUMMARY_PROMPT),
            None => compact::SUMMARY_PROMPT.to_string(),
        };
        msgs.push(Message::text(Role::User, ask));
        let req = Request {
            system: self.system.clone(),
            tools: &self.tools,
            messages: &msgs,
            session_id: &self.session.id,
            max_tokens: 1500,
        };
        let mut text = String::new();
        let mut usage = Usage::default();
        let ok = dispatch_complete(
            &self.provider,
            &req,
            &mut |c| match c {
                Chunk::Text(t) => text.push_str(&t),
                Chunk::Done(u) => usage = u,
                _ => {}
            },
            &self.allowed,
        )
        .is_ok();
        if let Some(c) = self.prices.cost(&usage) {
            self.cum_usd += c;
        }
        if !ok || !text.contains("goal:") {
            return fallback();
        }
        let text = text.trim().to_string();
        match focus {
            Some(f) if !text.contains("focus:") => text.replacen("# World\n", &format!("# World\nfocus: {f}\n"), 1),
            _ => text,
        }
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

fn restore_read_state(ctx: &mut ToolContext, messages: &[Message]) {
    let mut paths: HashMap<String, String> = HashMap::new();
    for m in messages {
        for b in &m.content {
            match b {
                Block::ToolUse { id, name, input } if name == "read" => {
                    if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
                        if let Ok(abs) = ztools::resolve(ctx, p) {
                            paths.insert(id.clone(), abs.to_string_lossy().to_string());
                        }
                    }
                }
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } if !*is_error => {
                    if let Some(p) = paths.get(tool_use_id) {
                        if let Some(rest) = content.split("file_sha256=").nth(1) {
                            let sha: String = rest
                                .chars()
                                .take_while(|c| c.is_ascii_hexdigit())
                                .collect();
                            if sha.len() == 64 {
                                ctx.read_state.insert(p.clone(), sha);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Plain-text recap of earlier turns for an agent that wasn't there,
/// newest last, capped at ~12k chars.
fn acp_recap(messages: &[Message]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for m in messages {
        for b in &m.content {
            if let Block::Text { text } = b {
                let who = if m.role == Role::Assistant { "assistant" } else { "user" };
                lines.push(format!("{who}: {}", text.trim()));
            }
        }
    }
    let mut out = String::new();
    for l in lines.iter().rev() {
        if out.len() + l.len() > 12_000 {
            break;
        }
        out = if out.is_empty() { l.clone() } else { format!("{l}\n{out}") };
    }
    out
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
            "cwd": agent.ctx.workspace,
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
    agent.world.set_goal_if_empty(&prompt);
    agent.world.save(&agent.home, &agent.session.id);
    if prompt.starts_with("/compact") {
        let focus = prompt.strip_prefix("/compact").unwrap_or("").trim();
        let focus = focus.strip_prefix("focus:").or_else(|| focus.strip_prefix("focus")).unwrap_or(focus).trim();
        agent.compact_now(if focus.is_empty() { None } else { Some(focus) }, emit);
        let ev = agent.session.append("session_end", json!({"reason":"compact"}));
        emit(&ev);
        return Ok(zproto::exit::DONE);
    }
    if prompt.trim() == "/undo" {
        let _ = agent.undo_last(emit);
        let ev = agent.session.append("session_end", json!({"reason":"undo"}));
        emit(&ev);
        return Ok(zproto::exit::DONE);
    }
    if let Some(rest) = prompt.strip_prefix("/skillify") {
        let name = rest.trim();
        let name = if name.is_empty() { "session" } else { name };
        let msg = agent.skillify(name, emit);
        eprintln!("{msg}");
        let ev = agent.session.append("session_end", json!({"reason":"skillify"}));
        emit(&ev);
        return Ok(zproto::exit::DONE);
    }
    agent.messages.push(Message::text(Role::User, prompt));

    let max_turns = opts.max_turns.unwrap_or(16);
    for _ in 0..max_turns {
        if let Some(max) = opts.max_usd {
            if agent.cum_usd >= max {
                return Ok(zproto::exit::BUDGET);
            }
        }
        // turn() only fails on the provider/network path — contract §7 exit 4
        let done = match agent.turn(emit) {
            Ok(d) => d,
            Err(e) => {
                let msg = format!("{e:#}");
                eprintln!("{msg}");
                let ev = agent
                    .session
                    .append("error", json!({"kind": "provider", "message": msg}));
                emit(&ev);
                let ev = agent
                    .session
                    .append("session_end", json!({"reason":"provider_error"}));
                emit(&ev);
                return Ok(zproto::exit::PROVIDER);
            }
        };
        if done {
            if let Some(gate) = &opts.autonomous_gate {
                if opts.autonomous || opts.autonomous_gate.is_some() {
                    let out = if cfg!(windows) {
                        std::process::Command::new("cmd")
                            .args(["/C", gate])
                            .current_dir(&agent.ctx.workspace)
                            .output()
                    } else {
                        std::process::Command::new("sh")
                            .args(["-lc", gate])
                            .current_dir(&agent.ctx.workspace)
                            .output()
                    };
                    match out {
                        Ok(o) if o.status.success() => {}
                        Ok(o) => {
                            let body = format!(
                                "{}{}",
                                String::from_utf8_lossy(&o.stdout),
                                String::from_utf8_lossy(&o.stderr)
                            );
                            agent.push_note(format!("[autonomous-gate failed]\n{body}"));
                            continue;
                        }
                        Err(e) => {
                            agent.push_note(format!("[autonomous-gate error] {e}"));
                            continue;
                        }
                    }
                }
            }
            if agent.improve.enabled && agent.improve.write_skills {
                let draft = format!("draft-{}", &agent.session.id[..8.min(agent.session.id.len())]);
                if let Ok((_, detail)) = improve::skillify(
                    &agent.home,
                    &draft,
                    &agent.world,
                    true,
                    agent.improve.allow_self_mutate,
                ) {
                    let ev = agent
                        .session
                        .append("self_improve", improve::event_data("draft_skill", detail));
                    emit(&ev);
                }
            }
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
        let results = agent.exec_tools_batch(&calls, emit);
        for ((_id, name, input), (_n, text, is_error)) in calls.into_iter().zip(results) {
            let (name, input) = ztools::unwrap_self_call(&name, &input).unwrap_or((name, input));
            if name == "submit_plan" && !is_error {
                if agent.opts.approve_plan {
                    agent.mode = Mode::Agent;
                    agent.push_note(format!(
                        "[approved plan]\n{}",
                        input["plan"].as_str().unwrap_or("")
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
