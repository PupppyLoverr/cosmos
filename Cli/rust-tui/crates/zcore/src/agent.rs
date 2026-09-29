//! The agent loop (contract §4-§9). Headless `-p` runs through `run_agent`;
//! the TUI drives the same loop through `Agent` one turn at a time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use zproto::{Block, Chunk, Event, Message, OutputMode, Role, RunOpts, Usage};
use zproviders::{complete, ProviderCfg, ProviderKind, Request};
use ztools::{is_readonly, tool_specs, ToolContext, PLAN_TOOLS};

use crate::compact::{self, CompactCfg};
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
    if cfg.kind == ProviderKind::Devin {
        crate::devin::complete(cfg, req, sink)
    } else {
        complete(cfg, req, sink, allowed)
    }
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
    pub last_l5_before: Option<u64>,
    pub last_l5_after: Option<u64>,
    pub improve: SelfImproveCfg,
    pub compact_cfg: CompactCfg,
    pub sandbox_mode: String,
    pub ask_mode: String,
    pub force_compact: bool,
    pub compact_focus: Option<String>,
    pub doom: Vec<u64>,
    pub stale_hits: u32,
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

fn load_prompt(value: &str) -> Result<String> {
    if let Some(path) = value.strip_prefix('@') {
        return Ok(std::fs::read_to_string(path)
            .with_context(|| format!("read system prompt {path}"))?);
    }
    Ok(value.to_owned())
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
        let (session, messages) = if let Some(id) = resume_id {
            let s = Session::open(&home, &id)?;
            let msgs = Session::replay(&home, &id)?;
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
        let mut system = if let Some(prompt) = &opts.system_prompt {
            vec![load_prompt(prompt)?]
        } else {
            vec![boot]
        };
        if opts.system_prompt.is_none()
            && !crate::system_snippet().is_empty()
            && !system[0].contains("You are Cosmos")
        {
            system.insert(0, crate::system_snippet());
        }
        if let Some(p) = crate::project_instructions(&workspace) {
            system.push(p);
        }
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
        let prefix = compact::prefix_hash(&system, ztools::TOOLS_JSON);
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
            last_l5_before: None,
            last_l5_after: None,
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

    pub fn context_report(&self) -> String {
        let tokens = compact::estimate_tokens(&self.messages, self.boot_tokens);
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
            self.compact_cfg.cache_warm_secs,
        );
        format!(
            "{{\"tokens_in\":{tokens},\"tokens_budget\":{},\"percent\":{pct},\"cache_warm\":{warm}}}",
            self.context_window
        )
    }

    /// §9 compaction check, then one provider turn. Returns true when the
    /// model produced no tool calls (turn ends).
    pub fn turn(&mut self, emit: &mut dyn FnMut(&Event)) -> Result<bool> {
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
        let projected = compact::layer4_collapse(&self.messages);
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
        self.messages.push(Message {
            role: Role::Assistant,
            content: blocks,
        });
        let tokens = compact::estimate_tokens(&self.messages, self.boot_tokens);
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
        c.meridian_bin = self.ctx.meridian_bin.clone();
        c.meridian_vault = self.ctx.meridian_vault.clone();
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

    /// Five-layer compact before every model call.
    fn maybe_compact(&mut self, emit: &mut dyn FnMut(&Event)) {
        let now = Instant::now();
        let prefix_now = compact::prefix_hash(&self.system, ztools::TOOLS_JSON);
        let warm = compact::cache_is_warm(
            self.last_model_at,
            &prefix_now,
            &self.prefix_hash,
            now,
            self.compact_cfg.cache_warm_secs,
        );
        crate::hooks::run(
            &self.cfg,
            "pre_compact",
            "",
            &json!({}),
            &self.session.id.clone(),
        );
        let force = self.force_compact;
        let focus = self.compact_focus.clone();
        let outcome = compact::compact(
            &mut self.messages,
            &self.home,
            &self.session.id,
            &self.compact_cfg,
            self.context_window,
            self.boot_tokens,
            warm,
            force,
            focus.as_deref(),
            self.last_l5_before,
            self.last_l5_after,
        );
        if outcome.world_md.is_some() {
            self.last_l5_before = Some(outcome.tokens_before);
            self.last_l5_after = Some(outcome.tokens_after);
            if let Some(md) = &outcome.world_md {
                self.world.apply_compact_md(md);
                self.world.save(&self.home, &self.session.id);
            }
        }
        for ev in outcome.events {
            let e = self.session.append(&ev.kind, ev.data);
            emit(&e);
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
        let done = agent.turn(emit)?;
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
                            agent.messages.push(Message::text(
                                Role::User,
                                format!("[autonomous-gate failed]\n{body}"),
                            ));
                            continue;
                        }
                        Err(e) => {
                            agent.messages.push(Message::text(
                                Role::User,
                                format!("[autonomous-gate error] {e}"),
                            ));
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
