//! zproviders — model providers per CONTRACT.md.
//!
//! Kinds: `openai` (any OpenAI-compatible /chat/completions endpoint — covers
//! Ollama, vLLM, LM Studio, OpenRouter, xAI), `anthropic`, `google`,
//! `fixture` (offline, deterministic — used when no live endpoint exists and
//! in tests). Every HTTP call goes through the §12 host allowlist.

use std::io::{BufRead, BufReader, Read};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use zproto::{Block, Chunk, Message, Role, ToolSpec, Usage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// OpenAI-compatible `/chat/completions` (xAI, Groq, Mistral, Qwen, …)
    OpenAI,
    /// OpenAI Responses API `/responses` (OpenAI, Meta)
    Responses,
    /// ChatGPT subscription — Responses API on the ChatGPT backend with an
    /// OAuth access token (the Codex / pi "Sign in with ChatGPT" path)
    ChatGpt,
    Anthropic,
    Google,
    Fixture,
}

impl ProviderKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "openai" => Some(Self::OpenAI),
            "responses" | "openai-responses" => Some(Self::Responses),
            "chatgpt" => Some(Self::ChatGpt),
            "anthropic" => Some(Self::Anthropic),
            "google" => Some(Self::Google),
            "fixture" => Some(Self::Fixture),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderCfg {
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key_env: String,
    pub model: String,
    /// context window in tokens
    pub context: u64,
    pub prices: zproto::Prices,
    /// Reasoning effort (`low`/`medium`/`high`) chosen in the TUI model
    /// picker. Sent as `reasoning_effort` (chat completions) or
    /// `reasoning.effort` (Responses); `None` leaves the provider default.
    pub effort: Option<String>,
    /// Resolved credential (stored key / OAuth access token). Takes
    /// precedence over `api_key_env`.
    pub api_key: Option<String>,
    /// ChatGPT account id (`chatgpt-account-id` header) for `ChatGpt`.
    pub account_id: Option<String>,
}

impl Default for ProviderCfg {
    fn default() -> Self {
        ProviderCfg {
            kind: ProviderKind::Fixture,
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
}

/// ChatGPT plan usage reported on the last subscription response
/// (`x-codex-*` headers) — what Devin shows as "84% remaining".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RateLimits {
    pub plan: Option<String>,
    /// percent of the primary (short) window used
    pub primary_used: Option<f64>,
    pub primary_reset_secs: Option<u64>,
    /// percent of the secondary (weekly) window used
    pub secondary_used: Option<f64>,
    pub secondary_reset_secs: Option<u64>,
}

static RATE_LIMITS: std::sync::Mutex<Option<RateLimits>> = std::sync::Mutex::new(None);

/// Latest ChatGPT plan usage seen, if any.
pub fn last_rate_limits() -> Option<RateLimits> {
    RATE_LIMITS.lock().ok().and_then(|g| g.clone())
}

/// A completion request: stable prefix layers 1-3 live in `system`, the
/// conversation in `messages`.
pub struct Request<'a> {
    /// layer 1: contract system snippet
    /// layer 2: (tools ride in `tools`)
    /// layer 3: bridge boot context — appended to system, index [1]
    pub system: Vec<String>,
    pub tools: &'a [ToolSpec],
    pub messages: &'a [Message],
    pub session_id: &'a str,
    pub max_tokens: u32,
}

/// Reject any outbound host not on the §12 allowlist.
pub fn host_allowed(url: &str, allowed: &[String]) -> Result<()> {
    let host = url
        .split("://")
        .nth(1)
        .and_then(|s| s.split(['/', ':', '?']).next())
        .unwrap_or("")
        .to_lowercase();
    let ok = allowed.iter().any(|a| {
        let a = a.trim_end_matches('/').to_lowercase();
        host == a || host.ends_with(&format!(".{a}"))
    });
    if ok {
        Ok(())
    } else {
        bail!("privacy: outbound host {host} not on the allowlist")
    }
}

fn api_key(cfg: &ProviderCfg) -> Option<String> {
    if let Some(k) = cfg.api_key.as_ref().filter(|k| !k.is_empty()) {
        return Some(k.clone());
    }
    if cfg.api_key_env.is_empty() {
        return None;
    }
    std::env::var(&cfg.api_key_env)
        .ok()
        .filter(|k| !k.is_empty())
}

/// Readable provider error: pull `error.message` out of JSON bodies.
fn http_error(status: u16, body: &str) -> anyhow::Error {
    let msg = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .or_else(|| v["detail"].as_str())
                .or_else(|| v["message"].as_str())
                .or_else(|| v["error"].as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(400).collect());
    let hint = match status {
        401 | 403 => " — check the API key (zcli auth login <provider>)",
        404 => " — unknown model or endpoint for this provider",
        429 => " — rate limited or out of quota",
        _ => "",
    };
    anyhow::anyhow!("provider {status}: {msg}{hint}")
}

/// Read a `data:` SSE stream, invoking `f` per event payload.
fn sse<R: Read>(reader: R, mut f: impl FnMut(&str) -> Result<bool>) -> Result<()> {
    for line in BufReader::new(reader).lines() {
        let line = line?;
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if data == "[DONE]" {
                break;
            }
            if f(data)? {
                break;
            }
        }
    }
    Ok(())
}

fn openai_messages(req: &Request) -> Vec<Value> {
    let mut out = Vec::new();
    for s in &req.system {
        out.push(json!({"role": "system", "content": s}));
    }
    for m in req.messages {
        match m.role {
            Role::User | Role::System => {
                for b in &m.content {
                    match b {
                        Block::Text { text } => out.push(json!({"role":"user","content":text})),
                        Block::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => out.push(json!({
                            "role":"tool","tool_call_id":tool_use_id,"content":content})),
                        _ => {}
                    }
                }
            }
            Role::Assistant => {
                let mut text = String::new();
                let mut calls = Vec::new();
                for b in &m.content {
                    match b {
                        Block::Text { text: t } => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t)
                        }
                        Block::ToolUse { id, name, input } => calls.push(json!({
                            "id": id, "type": "function",
                            "function": {"name": name, "arguments": input.to_string()}})),
                        _ => {}
                    }
                }
                let mut msg = json!({"role":"assistant"});
                if !text.is_empty() {
                    msg["content"] = json!(text);
                }
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                }
                out.push(msg);
            }
            Role::Tool => {}
        }
    }
    out
}

fn openai_tools(tools: &[ToolSpec]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| {
                json!({"type":"function","function":{
                    "name": t.name, "description": t.description,
                    "parameters": t.input_schema }})
            })
            .collect(),
    )
}

fn openai_parse_chunk(v: &Value, sink: &mut dyn FnMut(Chunk)) -> Result<()> {
    if let Some(u) = v.get("usage") {
        let usage = Usage {
            input: u["prompt_tokens"].as_u64().unwrap_or(0).saturating_sub(
                u["prompt_tokens_details"]["cached_tokens"]
                    .as_u64()
                    .unwrap_or(0),
            ),
            cache_read: u["prompt_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or(0),
            cache_write: 0,
            output: u["completion_tokens"].as_u64().unwrap_or(0),
        };
        sink(Chunk::Done(usage));
    }
    for ch in v["choices"].as_array().cloned().unwrap_or_default() {
        let d = &ch["delta"];
        if let Some(t) = d["content"].as_str() {
            if !t.is_empty() {
                sink(Chunk::Text(t.to_string()));
            }
        }
        for tc in d["tool_calls"].as_array().cloned().unwrap_or_default() {
            // streamed tool_calls arrive fragmented; we only need the
            // assembled call — accumulate via index across chunks.
            let idx = tc["index"].as_u64().unwrap_or(0);
            sink(Chunk::ToolUse {
                id: format!("partial:{idx}"),
                name: "__partial".into(),
                input: tc.clone(),
            });
        }
    }
    Ok(())
}

/// Accumulate streamed OpenAI tool_call fragments into complete calls.
#[derive(Default)]
pub struct ToolAcc {
    cur: Vec<(String, String, String)>, // id, name, args-json
}

impl ToolAcc {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn feed(&mut self, frag: &Value) {
        let idx = frag["index"].as_u64().unwrap_or(0) as usize;
        while self.cur.len() <= idx {
            self.cur.push((String::new(), String::new(), String::new()));
        }
        let slot = &mut self.cur[idx];
        if let Some(id) = frag["id"].as_str() {
            slot.0 = id.into();
        }
        if let Some(n) = frag["function"]["name"].as_str() {
            slot.1.push_str(n);
        }
        if let Some(a) = frag["function"]["arguments"].as_str() {
            slot.2.push_str(a);
        }
    }
    pub fn finish(self) -> Vec<Block> {
        self.cur
            .into_iter()
            .filter(|(id, _, _)| !id.is_empty())
            .map(|(id, name, args)| Block::ToolUse {
                id,
                name,
                input: serde_json::from_str(&args).unwrap_or(json!({})),
            })
            .collect()
    }
}

/// Streaming chat-completions call (OpenAI-compatible endpoints).
fn openai_complete(
    cfg: &ProviderCfg,
    req: &Request,
    sink: &mut dyn FnMut(Chunk),
    allowed: &[String],
) -> Result<()> {
    let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
    host_allowed(&url, allowed)?;
    let mut body = json!({
        "model": cfg.model,
        "messages": openai_messages(req),
        "tools": openai_tools(req.tools),
        "max_tokens": req.max_tokens,
        "stream": true,
        "stream_options": {"include_usage": true},
        "prompt_cache_key": req.session_id,
    });
    if req.tools.is_empty() {
        body.as_object_mut().unwrap().remove("tools");
    }
    if let Some(e) = &cfg.effort {
        body["reasoning_effort"] = json!(e);
    }
    let key = api_key(cfg);
    let mut call = ureq::post(&url)
        .header("content-type", "application/json")
        .config()
        .http_status_as_error(false)
        .build();
    if let Some(k) = &key {
        call = call.header("authorization", &format!("Bearer {k}"));
    }
    let mut resp = call.send_json(&body).context("openai request failed")?;
    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(http_error(status, &text));
    }
    let mut acc = ToolAcc::new();
    let mut usage_seen = false;
    let reader = resp.body_mut().as_reader();
    sse(reader, |data| {
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Ok(false),
        };
        // intercept partial tool_calls, never forward raw fragments
        let mut tool_frags = Vec::new();
        openai_parse_chunk(&v, &mut |c| {
            if let Chunk::ToolUse { input, .. } = &c {
                tool_frags.push(input.clone());
            } else {
                if matches!(c, Chunk::Done(_)) {
                    usage_seen = true;
                }
                sink(c);
            }
        })?;
        for f in tool_frags {
            acc.feed(&f);
        }
        Ok(false)
    })?;
    for b in acc.finish() {
        if let Block::ToolUse { id, name, input } = b {
            sink(Chunk::ToolUse { id, name, input });
        }
    }
    if !usage_seen {
        sink(Chunk::Done(Usage::default()));
    }
    Ok(())
}

fn anthropic_messages(req: &Request) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in req.messages {
        match m.role {
            Role::User | Role::Tool => {
                let mut parts = Vec::new();
                for b in &m.content {
                    match b {
                        Block::Text { text } => parts.push(json!({"type":"text","text":text})),
                        Block::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => parts.push(json!({
                            "type":"tool_result","tool_use_id":tool_use_id,
                            "content":[{"type":"text","text":content}],
                            "is_error":is_error})),
                        _ => {}
                    }
                }
                if !parts.is_empty() {
                    out.push(json!({"role":"user","content":parts}));
                }
            }
            Role::Assistant => {
                let mut parts = Vec::new();
                for b in &m.content {
                    match b {
                        Block::Text { text } => parts.push(json!({"type":"text","text":text})),
                        Block::ToolUse { id, name, input } => parts.push(json!({
                            "type":"tool_use","id":id,"name":name,"input":input})),
                        _ => {}
                    }
                }
                if !parts.is_empty() {
                    out.push(json!({"role":"assistant","content":parts}));
                }
            }
            Role::System => {}
        }
    }
    out
}

fn anthropic_complete(
    cfg: &ProviderCfg,
    req: &Request,
    sink: &mut dyn FnMut(Chunk),
    allowed: &[String],
) -> Result<()> {
    let url = format!("{}/v1/messages", cfg.base_url.trim_end_matches('/'));
    host_allowed(&url, allowed)?;
    // system layers as block array with cache_control breakpoints (§4)
    let sys: Vec<Value> = req
        .system
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mut b = json!({"type":"text","text":s});
            if i == req.system.len() - 1 {
                b["cache_control"] = json!({"type":"ephemeral"});
            }
            b
        })
        .collect();
    let tools: Value = Value::Array(
        req.tools
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut o = json!({"name":t.name,"description":t.description,"input_schema":t.input_schema});
                if i + 1 == req.tools.len() {
                    o["cache_control"] = json!({"type":"ephemeral"});
                }
                o
            })
            .collect(),
    );
    let body = json!({
        "model": cfg.model,
        "system": sys,
        "messages": anthropic_messages(req),
        "tools": tools,
        "max_tokens": req.max_tokens,
        "stream": true,
    });
    let key = api_key(cfg);
    let mut call = ureq::post(&url)
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .config()
        .http_status_as_error(false)
        .build();
    if let Some(k) = &key {
        call = call.header("x-api-key", k);
    }
    let mut resp = call.send_json(&body).context("anthropic request failed")?;
    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(http_error(status, &text));
    }
    // accumulate tool_use input_json fragments per content block index
    let mut tool_idx: Vec<(String, String, String)> = Vec::new();
    let reader = resp.body_mut().as_reader();
    sse(reader, |data| {
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Ok(false),
        };
        match v["type"].as_str().unwrap_or("") {
            "content_block_start" => {
                let i = v["index"].as_u64().unwrap_or(0) as usize;
                if v["content_block"]["type"] == "tool_use" {
                    while tool_idx.len() <= i {
                        tool_idx.push((String::new(), String::new(), String::new()));
                    }
                    tool_idx[i].0 = v["content_block"]["id"].as_str().unwrap_or("").into();
                    tool_idx[i].1 = v["content_block"]["name"].as_str().unwrap_or("").into();
                }
            }
            "content_block_delta" => {
                let i = v["index"].as_u64().unwrap_or(0) as usize;
                if v["delta"]["type"] == "text_delta" {
                    if let Some(t) = v["delta"]["text"].as_str() {
                        sink(Chunk::Text(t.into()));
                    }
                } else if v["delta"]["type"] == "input_json_delta" {
                    while tool_idx.len() <= i {
                        tool_idx.push((String::new(), String::new(), String::new()));
                    }
                    if let Some(p) = v["delta"]["partial_json"].as_str() {
                        tool_idx[i].2.push_str(p);
                    }
                }
            }
            "message_delta" => {
                let u = &v["usage"];
                sink(Chunk::Done(Usage {
                    input: v["message"]["usage"]["input_tokens"]
                        .as_u64()
                        .unwrap_or(0)
                        .max(u["input_tokens"].as_u64().unwrap_or(0)),
                    cache_read: u["cache_read_input_tokens"].as_u64().unwrap_or(0).max(
                        v["message"]["usage"]["cache_read_input_tokens"]
                            .as_u64()
                            .unwrap_or(0),
                    ),
                    cache_write: u["cache_creation_input_tokens"].as_u64().unwrap_or(0).max(
                        v["message"]["usage"]["cache_creation_input_tokens"]
                            .as_u64()
                            .unwrap_or(0),
                    ),
                    output: u["output_tokens"].as_u64().unwrap_or(0),
                }));
            }
            "message_start" => {
                let u = &v["message"]["usage"];
                sink(Chunk::Done(Usage {
                    input: u["input_tokens"].as_u64().unwrap_or(0),
                    cache_read: u["cache_read_input_tokens"].as_u64().unwrap_or(0),
                    cache_write: u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
                    output: u["output_tokens"].as_u64().unwrap_or(0),
                }));
            }
            _ => {}
        }
        Ok(false)
    })?;
    for (id, name, args) in tool_idx {
        if !name.is_empty() {
            sink(Chunk::ToolUse {
                id,
                name,
                input: serde_json::from_str(&args).unwrap_or(json!({})),
            });
        }
    }
    Ok(())
}

fn google_complete(
    cfg: &ProviderCfg,
    req: &Request,
    sink: &mut dyn FnMut(Chunk),
    allowed: &[String],
) -> Result<()> {
    let url = format!(
        "{}/v1beta/models/{}:streamGenerateContent?alt=sse",
        cfg.base_url.trim_end_matches('/'),
        cfg.model
    );
    host_allowed(&url, allowed)?;
    let sys: Vec<Value> = req.system.iter().map(|s| json!({"text": s})).collect();
    // Gemini matches a functionResponse to its call by *name*, so map ids back.
    let mut call_names: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for m in req.messages {
        for b in &m.content {
            if let Block::ToolUse { id, name, .. } = b {
                call_names.insert(id.as_str(), name.as_str());
            }
        }
    }
    let mut contents = Vec::new();
    for m in req.messages {
        let role = match m.role {
            Role::Assistant => "model",
            _ => "user",
        };
        let mut parts = Vec::new();
        for b in &m.content {
            match b {
                Block::Text { text } => parts.push(json!({"text": text})),
                Block::ToolUse { name, input, .. } => {
                    parts.push(json!({"functionCall": {"name": name, "args": input}}))
                }
                Block::ToolResult { tool_use_id, content, .. } => {
                    let name = call_names.get(tool_use_id.as_str()).copied().unwrap_or("tool");
                    parts.push(json!({"functionResponse": {"name": name, "response": {"output": content}}}))
                }
            }
        }
        if !parts.is_empty() {
            contents.push(json!({"role": role, "parts": parts}));
        }
    }
    let decls: Vec<Value> = req
        .tools
        .iter()
        .map(|t| json!({"name":t.name,"description":t.description,"parameters":t.input_schema}))
        .collect();
    let body = json!({
        "systemInstruction": {"parts": sys},
        "contents": contents,
        "tools": [{"functionDeclarations": decls}],
    });
    let key = api_key(cfg);
    let mut call = ureq::post(&url)
        .header("content-type", "application/json")
        .config()
        .http_status_as_error(false)
        .build();
    if let Some(k) = &key {
        call = call.header("x-goog-api-key", k);
    }
    let mut resp = call.send_json(&body).context("google request failed")?;
    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(http_error(status, &text));
    }
    let reader = resp.body_mut().as_reader();
    sse(reader, |data| {
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Ok(false),
        };
        for cand in v["candidates"].as_array().cloned().unwrap_or_default() {
            for p in cand["content"]["parts"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                if let Some(t) = p["text"].as_str() {
                    sink(Chunk::Text(t.into()));
                }
                if let Some(fc) = p.get("functionCall") {
                    sink(Chunk::ToolUse {
                        id: zproto::new_id(),
                        name: fc["name"].as_str().unwrap_or("").into(),
                        input: fc["args"].clone(),
                    });
                }
            }
        }
        if let Some(u) = v.get("usageMetadata") {
            sink(Chunk::Done(Usage {
                input: u["promptTokenCount"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_sub(u["cachedContentTokenCount"].as_u64().unwrap_or(0)),
                cache_read: u["cachedContentTokenCount"].as_u64().unwrap_or(0),
                cache_write: 0,
                output: u["candidatesTokenCount"].as_u64().unwrap_or(0),
            }));
        }
        Ok(false)
    })?;
    Ok(())
}

/// Deterministic offline provider. Last message carrying a `tool_result` →
/// `done.`; `tool:NAME {json}` → emits that tool call; `ping` → `pong`;
/// anything else echoes a short acknowledgement. Usage is a 4-chars-per-token
/// estimate so the cost ledger path is exercised.
fn fixture_complete(cfg: &ProviderCfg, req: &Request, sink: &mut dyn FnMut(Chunk)) -> Result<()> {
    let last_msg = req.messages.last();
    let has_tool_result = last_msg
        .map(|m| {
            m.content
                .iter()
                .any(|b| matches!(b, Block::ToolResult { .. }))
        })
        .unwrap_or(false);
    let last = last_msg
        .map(|m| {
            m.content
                .iter()
                .filter_map(|b| match b {
                    Block::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let est = |s: &str| (s.len() / 4) as u64;
    let sys_tokens: u64 = req.system.iter().map(|s| est(s)).sum();
    let msg_tokens: u64 = req
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .map(|b| match b {
            Block::Text { text } => est(text),
            _ => 4,
        })
        .sum();
    sink(Chunk::Done(Usage {
        input: sys_tokens + msg_tokens,
        cache_read: 0,
        cache_write: 0,
        output: 0,
    }));
    let lower = last.to_lowercase();
    if has_tool_result {
        sink(Chunk::Text("done.".into()));
    } else if let Some(rest) = last.strip_prefix("tool:") {
        let mut it = rest.splitn(2, ' ');
        let name = it.next().unwrap_or("").to_string();
        let input: Value = it
            .next()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(json!({}));
        sink(Chunk::ToolUse {
            id: zproto::new_id(),
            name,
            input,
        });
    } else if lower.trim() == "ping" || lower.contains("ping") {
        sink(Chunk::Text("pong".into()));
    } else {
        sink(Chunk::Text(format!(
            "fixture: no live provider configured — received {:?}. Configure [providers] + [models] in config.toml for a real model.",
            &last[..last.len().min(80)]
        )));
    }
    sink(Chunk::Done(Usage {
        input: 0,
        cache_read: 0,
        cache_write: 0,
        output: est(&format!("{cfg:?}")) / 8 + 8,
    }));
    Ok(())
}

// ---- OpenAI Responses API (OpenAI, Meta, ChatGPT subscription) -------------

/// Conversation → Responses `input` items.
fn responses_input(req: &Request) -> Vec<Value> {
    let mut out = Vec::new();
    for m in req.messages {
        for b in &m.content {
            match (m.role, b) {
                (Role::Assistant, Block::Text { text }) => out.push(json!({
                    "type": "message", "role": "assistant",
                    "content": [{"type": "output_text", "text": text}]})),
                (_, Block::Text { text }) => out.push(json!({
                    "type": "message", "role": "user",
                    "content": [{"type": "input_text", "text": text}]})),
                (_, Block::ToolUse { id, name, input }) => out.push(json!({
                    "type": "function_call", "call_id": id, "name": name,
                    "arguments": input.to_string()})),
                (_, Block::ToolResult { tool_use_id, content, .. }) => out.push(json!({
                    "type": "function_call_output", "call_id": tool_use_id, "output": content})),
            }
        }
    }
    out
}

fn header_f64(resp: &ureq::http::Response<ureq::Body>, k: &str) -> Option<f64> {
    resp.headers().get(k)?.to_str().ok()?.trim().parse().ok()
}

fn capture_rate_limits(resp: &ureq::http::Response<ureq::Body>) {
    let rl = RateLimits {
        plan: resp
            .headers()
            .get("x-codex-plan-type")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
        primary_used: header_f64(resp, "x-codex-primary-used-percent"),
        primary_reset_secs: header_f64(resp, "x-codex-primary-reset-after-seconds").map(|v| v as u64),
        secondary_used: header_f64(resp, "x-codex-secondary-used-percent"),
        secondary_reset_secs: header_f64(resp, "x-codex-secondary-reset-after-seconds").map(|v| v as u64),
    };
    if rl.primary_used.is_some() || rl.secondary_used.is_some() || rl.plan.is_some() {
        if let Ok(mut g) = RATE_LIMITS.lock() {
            *g = Some(rl);
        }
    }
}

fn responses_complete(
    cfg: &ProviderCfg,
    req: &Request,
    sink: &mut dyn FnMut(Chunk),
    allowed: &[String],
) -> Result<()> {
    let chatgpt = cfg.kind == ProviderKind::ChatGpt;
    let base = cfg.base_url.trim_end_matches('/');
    let url = if chatgpt {
        format!("{base}/codex/responses")
    } else {
        format!("{base}/responses")
    };
    host_allowed(&url, allowed)?;
    let tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| json!({"type": "function", "name": t.name, "description": t.description, "parameters": t.input_schema}))
        .collect();
    let mut body = json!({
        "model": cfg.model,
        "instructions": req.system.join("\n\n"),
        "input": responses_input(req),
        "tools": tools,
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "stream": true,
        "store": false,
        "prompt_cache_key": req.session_id,
    });
    if !chatgpt {
        body["max_output_tokens"] = json!(req.max_tokens.max(4096));
    }
    if let Some(e) = &cfg.effort {
        body["reasoning"] = json!({"effort": e, "summary": "auto"});
    }
    let key = api_key(cfg);
    let mut call = ureq::post(&url)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .config()
        .http_status_as_error(false)
        .build();
    if let Some(k) = &key {
        call = call.header("authorization", &format!("Bearer {k}"));
    }
    if chatgpt {
        call = call
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", "zcli")
            .header("session_id", req.session_id);
        if let Some(a) = &cfg.account_id {
            call = call.header("chatgpt-account-id", a);
        }
    }
    let mut resp = call.send_json(&body).context("responses request failed")?;
    if chatgpt {
        capture_rate_limits(&resp);
    }
    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        return Err(http_error(status, &text));
    }
    let mut failure: Option<String> = None;
    let mut usage_seen = false;
    let reader = resp.body_mut().as_reader();
    sse(reader, |data| {
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Ok(false),
        };
        match v["type"].as_str().unwrap_or("") {
            "response.output_text.delta" => {
                if let Some(t) = v["delta"].as_str() {
                    if !t.is_empty() {
                        sink(Chunk::Text(t.to_string()));
                    }
                }
            }
            "response.output_item.done" => {
                let item = &v["item"];
                if item["type"] == "function_call" {
                    let args = item["arguments"].as_str().unwrap_or("{}");
                    sink(Chunk::ToolUse {
                        id: item["call_id"].as_str().unwrap_or("").to_string(),
                        name: item["name"].as_str().unwrap_or("").to_string(),
                        input: serde_json::from_str(args).unwrap_or(json!({})),
                    });
                }
            }
            "response.completed" | "response.incomplete" => {
                let u = &v["response"]["usage"];
                let cached = u["input_tokens_details"]["cached_tokens"].as_u64().unwrap_or(0);
                sink(Chunk::Done(Usage {
                    input: u["input_tokens"].as_u64().unwrap_or(0).saturating_sub(cached),
                    cache_read: cached,
                    cache_write: 0,
                    output: u["output_tokens"].as_u64().unwrap_or(0),
                }));
                usage_seen = true;
                return Ok(true);
            }
            "response.failed" => {
                failure = v["response"]["error"]["message"].as_str().map(str::to_string).or(Some("response failed".into()));
                return Ok(true);
            }
            "error" => {
                failure = v["message"].as_str().or(v["error"]["message"].as_str()).map(str::to_string).or(Some("stream error".into()));
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    })?;
    if let Some(f) = failure {
        bail!("provider error: {f}");
    }
    if !usage_seen {
        sink(Chunk::Done(Usage::default()));
    }
    Ok(())
}

// ---- speech to text ----------------------------------------------------------

/// Transcribe a WAV clip with an OpenAI-compatible `/audio/transcriptions`
/// endpoint (OpenAI, Groq). Returns the recognised text.
pub fn transcribe(base_url: &str, key: &str, model: &str, wav: &[u8], allowed: &[String]) -> Result<String> {
    let url = format!("{}/audio/transcriptions", base_url.trim_end_matches('/'));
    host_allowed(&url, allowed)?;
    let boundary = format!("zcli{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let mut body: Vec<u8> = Vec::with_capacity(wav.len() + 512);
    let field = |b: &mut Vec<u8>, name: &str, value: &str| {
        b.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    };
    field(&mut body, "model", model);
    field(&mut body, "response_format", "json");
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"dictation.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut resp = ureq::post(&url)
        .header("authorization", &format!("Bearer {key}"))
        .header("content-type", &format!("multipart/form-data; boundary={boundary}"))
        .config()
        .http_status_as_error(false)
        .build()
        .send(&body[..])
        .context("transcription request failed")?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    if status >= 400 {
        return Err(http_error(status, &text));
    }
    let v: Value = serde_json::from_str(&text).unwrap_or(json!({"text": text}));
    Ok(v["text"].as_str().unwrap_or("").trim().to_string())
}

/// Transcribe with a ChatGPT Plus/Pro sign-in — the backend the ChatGPT apps
/// use for dictation (`POST {base}/transcribe`, multipart `file`). No API
/// key needed; the audio is kept by ChatGPT like any app dictation.
pub fn transcribe_chatgpt(base_url: &str, access: &str, account_id: &str, wav: &[u8], allowed: &[String]) -> Result<String> {
    let url = format!("{}/transcribe", base_url.trim_end_matches('/'));
    host_allowed(&url, allowed)?;
    let boundary = format!("zcli{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let mut body: Vec<u8> = Vec::with_capacity(wav.len() + 256);
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"dictation.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut req = ureq::post(&url)
        .header("authorization", &format!("Bearer {access}"))
        .header("content-type", &format!("multipart/form-data; boundary={boundary}"))
        .header("originator", "zcli");
    if !account_id.is_empty() {
        req = req.header("chatgpt-account-id", account_id);
    }
    let mut resp = req
        .config()
        .http_status_as_error(false)
        .build()
        .send(&body[..])
        .context("transcription request failed")?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    if status >= 400 {
        return Err(http_error(status, &text));
    }
    let v: Value = serde_json::from_str(&text).context("transcription reply wasn't JSON")?;
    Ok(v["text"].as_str().unwrap_or("").trim().to_string())
}

/// Run one completion. `allowed` = §12 host allowlist.
pub fn complete(
    cfg: &ProviderCfg,
    req: &Request,
    sink: &mut dyn FnMut(Chunk),
    allowed: &[String],
) -> Result<()> {
    match cfg.kind {
        ProviderKind::Fixture => fixture_complete(cfg, req, sink),
        ProviderKind::OpenAI => openai_complete(cfg, req, sink, allowed),
        ProviderKind::Responses | ProviderKind::ChatGpt => responses_complete(cfg, req, sink, allowed),
        ProviderKind::Anthropic => anthropic_complete(cfg, req, sink, allowed),
        ProviderKind::Google => google_complete(cfg, req, sink, allowed),
    }
}
