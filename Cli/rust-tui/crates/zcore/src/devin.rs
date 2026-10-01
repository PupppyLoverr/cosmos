//! Devin subscription — models from the signed-in plan; Cosmos runs the tools.
//!
//! Auth is the Devin CLI's `credentials.toml` (`windsurf_api_key`). Completions
//! go to Codeium `GetChatMessage` with *our* tool schemas. The agent loop
//! executes those tools — we never spawn `devin acp`.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use serde::Deserialize;
use serde_json::{json, Value};
use zproto::{Block, Chunk, Role, Usage};
use zproviders::{ProviderCfg, Request};

use crate::catalog::{host_of, ModelDef};

const CACHE_TTL: Duration = Duration::from_secs(45);
const DEFAULT_SERVER: &str = "https://server.codeium.com";
const SESSION_PREFIX: &str = "devin-session-token$";
const CLI_VERSION: &str = "3000.11.3";
const CHAT_PATH: &str = "/exa.api_server_pb.ApiServerService/GetChatMessage";
const MODELS_PATH: &str = "/exa.api_server_pb.ApiServerService/GetCliModelConfigs";
const MAX_FRAME: usize = 16 * 1024 * 1024;
const CONNECT_GZIP: u8 = 0x01;
const CONNECT_END: u8 = 0x02;

struct Cache {
    at: Instant,
    ready: bool,
    detail: String,
    models: Vec<ModelDef>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

struct Creds {
    key: String,
    server: String,
}

/// Windows extra install locations used by the official installer / desktop bundle.
fn extra_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = PathBuf::from(home);
        out.push(home.join(".local").join("bin").join("devin"));
        #[cfg(windows)]
        {
            out.push(home.join(".local").join("bin").join("devin.exe"));
            out.push(home.join(".local").join("bin").join("devin.cmd"));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        out.push(local.join("devin").join("cli").join("bin").join("devin.exe"));
        out.push(local.join("Programs").join("Devin").join("bin").join("devin.exe"));
        out.push(local.join("Programs").join("Devin").join("bin").join("devin.cmd"));
        out.push(
            local
                .join("Programs")
                .join("Devin")
                .join("resources")
                .join("app")
                .join("extensions")
                .join("windsurf")
                .join("devin")
                .join("bin")
                .join("devin.exe"),
        );
    }
    out.push(PathBuf::from("/opt/homebrew/bin/devin"));
    out.push(PathBuf::from("/usr/local/bin/devin"));
    out
}

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd;
}

/// The `devin` binary, if this machine has one.
pub fn executable() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("DEVIN_EXECUTABLE") {
        let p = PathBuf::from(v);
        if p.is_file() {
            return Some(p);
        }
    }
    let names: &[&str] = if cfg!(windows) {
        &["devin.exe", "devin.cmd", "devin"]
    } else {
        &["devin"]
    };
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for n in names {
                let p = dir.join(n);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    extra_paths().into_iter().find(|p| p.is_file())
}

fn cmd(exe: &Path) -> Command {
    let mut c = Command::new(exe);
    hide_window(&mut c);
    c
}

pub fn credentials_file() -> PathBuf {
    if let Some(p) = std::env::var_os("DEVIN_CREDENTIALS") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    if let Some(app) = std::env::var_os("APPDATA") {
        let p = PathBuf::from(app).join("devin").join("credentials.toml");
        if p.is_file() {
            return p;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            let p = PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("devin")
                .join("credentials.toml");
            if p.is_file() {
                return p;
            }
        }
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|h| PathBuf::from(h).join(".local").join("share"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    let primary = data.join("devin").join("credentials.toml");
    if primary.is_file() {
        return primary;
    }
    #[cfg(windows)]
    if let Some(app) = std::env::var_os("APPDATA") {
        return PathBuf::from(app).join("devin").join("credentials.toml");
    }
    primary
}

fn toml_basic_string(text: &str, key: &str) -> Option<String> {
    for raw in text.lines() {
        let line = raw.trim();
        let Some(rest) = line.strip_prefix(key) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        if !rest.starts_with('"') {
            continue;
        }
        let body = &rest[1..];
        let mut out = String::new();
        let mut chars = body.chars();
        while let Some(c) = chars.next() {
            if c == '"' {
                return Some(out);
            }
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => out.push(other),
                    None => break,
                }
            } else {
                out.push(c);
            }
        }
    }
    None
}

fn normalize_key(k: &str) -> String {
    if k.starts_with(SESSION_PREFIX) {
        k.to_string()
    } else {
        format!("{SESSION_PREFIX}{k}")
    }
}

fn trusted_server(url: &str) -> bool {
    let host = host_of(url).to_lowercase();
    host == "server.codeium.com"
        || host.ends_with(".codeium.com")
        || host == "api.devin.ai"
        || host.ends_with(".devin.ai")
        || host.ends_with(".windsurf.com")
}

fn load_creds() -> Option<Creds> {
    let text = std::fs::read_to_string(credentials_file()).ok()?;
    let key = toml_basic_string(&text, "windsurf_api_key")
        .or_else(|| toml_basic_string(&text, "devin_api_key"))
        .filter(|k| !k.is_empty())?;
    let server = toml_basic_string(&text, "api_server_url")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVER.into());
    if !trusted_server(&server) {
        return None;
    }
    Some(Creds {
        key: normalize_key(&key),
        server: server.trim_end_matches('/').to_string(),
    })
}

/// True when Devin credentials exist on this machine (no network).
pub fn signed_in() -> bool {
    load_creds().is_some()
}

fn metadata(key: &str) -> Value {
    json!({
        "apiKey": key,
        "ideName": "devin-cli",
        "ideVersion": CLI_VERSION,
        "extensionName": "chisel",
        "extensionVersion": CLI_VERSION,
        "locale": "en",
        "os": std::env::consts::OS,
        "ideType": "chisel",
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(180)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn post_json(url: &str, body: &Value) -> Result<(u16, Value)> {
    let mut resp = agent()
        .post(url)
        .header("content-type", "application/json")
        .header("connect-protocol-version", "1")
        .send(serde_json::to_vec(body)?.as_slice())
        .with_context(|| format!("couldn't reach {}", host_of(url)))?;
    let status = resp.status().as_u16();
    let text = resp
        .body_mut()
        .with_config()
        .limit(8 << 20)
        .read_to_string()
        .unwrap_or_default();
    let v = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok((status, v))
}

fn num_u64(v: &Value) -> u64 {
    v.as_u64()
        .or_else(|| v.as_i64().map(|n| n.max(0) as u64))
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

pub fn parse_cli_model_configs(v: &Value) -> Vec<ModelDef> {
    let mut models = Vec::new();
    let items = v["clientModelConfigs"]
        .as_array()
        .or(v["client_model_configs"].as_array());
    let Some(items) = items else {
        return models;
    };
    let mut seen = HashSet::new();
    for m in items {
        let id = m["modelUid"]
            .as_str()
            .or(m["model_uid"].as_str())
            .unwrap_or("")
            .trim();
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        let name = m["label"].as_str().unwrap_or("").trim();
        let name = if name.is_empty() { id } else { name };
        let max = num_u64(&m["maxTokens"]).max(num_u64(&m["max_tokens"]));
        models.push(ModelDef {
            id: id.into(),
            name: name.into(),
            context: 200_000,
            output: if max > 0 { max } else { 32_000 },
            reasoning: m
                .pointer("/modelInfo/modelFeatures/supportsThinking")
                .or(m.pointer("/model_info/model_features/supports_thinking"))
                .and_then(Value::as_bool)
                .unwrap_or(true),
            price: [0.0; 4],
        });
    }
    models
}

fn http_models(creds: &Creds) -> Result<Vec<ModelDef>> {
    let url = format!("{}{MODELS_PATH}", creds.server);
    let (status, v) = post_json(&url, &json!({ "metadata": metadata(&creds.key) }))?;
    if status == 401 || status == 403 {
        bail!("Devin rejected the subscription ({status}) — run `devin auth login`");
    }
    if !(200..300).contains(&status) {
        bail!("Devin model list failed ({status})");
    }
    Ok(parse_cli_model_configs(&v))
}

#[derive(Deserialize)]
struct ModelList {
    #[serde(default)]
    families: Vec<Family>,
}

#[derive(Deserialize)]
struct Family {
    #[serde(default)]
    variants: Vec<Variant>,
}

#[derive(Deserialize)]
struct Variant {
    #[serde(default)]
    model_uid: String,
    #[serde(default)]
    label: String,
}

/// Parse `devin models list --format json` into catalog rows.
pub fn parse_list(bytes: &[u8]) -> Result<Vec<ModelDef>> {
    let catalog: ModelList = serde_json::from_slice(bytes).context("invalid Devin model catalog")?;
    let mut models = Vec::new();
    for variant in catalog.families.into_iter().flat_map(|f| f.variants) {
        let id = variant.model_uid.trim();
        if id.is_empty() || models.iter().any(|m: &ModelDef| m.id == id) {
            continue;
        }
        let name = variant.label.trim();
        let name = if name.is_empty() { id } else { name };
        models.push(ModelDef {
            id: id.into(),
            name: name.into(),
            context: 200_000,
            output: 32_000,
            reasoning: true,
            price: [0.0; 4],
        });
    }
    if models.is_empty() {
        bail!("Devin returned an empty model list — is the CLI signed in?");
    }
    Ok(models)
}

fn output_timeout(c: &mut Command, secs: u64) -> Result<Output> {
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().context("couldn't start Devin")?;
    let mut stdout_pipe = child.stdout.take().context("Devin stdout")?;
    let mut stderr_pipe = child.stderr.take().context("Devin stderr")?;
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut b);
        b
    });
    let deadline = Instant::now() + Duration::from_secs(secs);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Devin model list timed out");
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => return Err(e.into()),
        }
    };
    let stdout = t_out.join().unwrap_or_default();
    let stderr = t_err.join().unwrap_or_default();
    Ok(Output { status, stdout, stderr })
}

fn cli_models() -> Result<Vec<ModelDef>> {
    let exe = executable().context("Devin CLI is not installed")?;
    let mut c = cmd(&exe);
    c.args(["models", "list", "--format", "json"]);
    let o = output_timeout(&mut c, 15)?;
    if !o.status.success() {
        let err = String::from_utf8_lossy(&o.stderr);
        let err = err.lines().next().unwrap_or("not signed in").trim();
        bail!("{}", if err.is_empty() { "run `devin auth login`" } else { err });
    }
    parse_list(&o.stdout)
}

fn union_models(primary: Vec<ModelDef>, extra: Vec<ModelDef>) -> Vec<ModelDef> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for m in primary.into_iter().chain(extra) {
        if m.id.is_empty() || !seen.insert(m.id.clone()) {
            continue;
        }
        out.push(m);
    }
    out
}

fn probe() -> (bool, String, Vec<ModelDef>) {
    let mut err = "run `devin auth login`".to_string();
    let mut models = Vec::new();
    if let Some(creds) = load_creds() {
        match http_models(&creds) {
            Ok(m) => models = m,
            Err(e) => err = e.to_string(),
        }
    }
    match cli_models() {
        Ok(m) => models = union_models(models, m),
        Err(e) if models.is_empty() => err = e.to_string(),
        Err(_) => {}
    }
    if models.is_empty() {
        return (false, err.chars().take(80).collect(), Vec::new());
    }
    let n = models.len();
    (true, format!("signed in · {n} models"), models)
}

fn cached() -> Cache {
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = g.as_ref() {
        if c.at.elapsed() < CACHE_TTL {
            return Cache {
                at: c.at,
                ready: c.ready,
                detail: c.detail.clone(),
                models: c.models.clone(),
            };
        }
    }
    let (ready, detail, models) = probe();
    let c = Cache {
        at: Instant::now(),
        ready,
        detail,
        models,
    };
    *g = Some(Cache {
        at: c.at,
        ready: c.ready,
        detail: c.detail.clone(),
        models: c.models.clone(),
    });
    c
}

/// Last probe result, if any — never starts the CLI or hits the network.
pub fn peek() -> Option<(bool, String)> {
    CACHE.lock().ok()?.as_ref().map(|c| (c.ready, c.detail.clone()))
}

/// Drop the status cache so the next `/providers` hit re-probes.
pub fn invalidate() {
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Last known sign-in, without spawning the CLI or hitting the network.
pub fn known_ready() -> bool {
    if let Ok(g) = CACHE.lock() {
        if let Some(c) = g.as_ref() {
            return c.ready;
        }
    }
    signed_in() || crate::live::cached("devin-direct").is_some_and(|m| !m.is_empty())
}

pub fn ready() -> bool {
    if signed_in() {
        return true;
    }
    cached().ready
}

pub fn detail() -> String {
    if let Some((_, d)) = peek() {
        return d;
    }
    if signed_in() {
        return "signed in".into();
    }
    "run `devin auth login`".into()
}

/// Live models for the signed-in Devin account.
pub fn models() -> Result<Vec<ModelDef>> {
    invalidate();
    let c = cached();
    if !c.ready {
        bail!("{}", c.detail);
    }
    Ok(c.models)
}

/// One-line install / sign-in hint.
pub fn hint() -> String {
    if signed_in() {
        return "Devin is signed in. /providers → Devin to load the live model list.".into();
    }
    if executable().is_none() {
        return "Devin isn't signed in. Windows: irm https://static.devin.ai/cli/setup.ps1 | iex — then `devin auth login`.".into();
    }
    format!(
        "Devin CLI found but not signed in ({}) — run `devin auth login` in a terminal, then /providers again.",
        detail()
    )
}

fn uuid() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

fn encode_frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.push(0);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn gunzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut d = GzDecoder::new(data);
    let mut out = Vec::new();
    d.read_to_end(&mut out).context("Devin gzip frame")?;
    Ok(out)
}

pub fn decode_connect_json(buf: &[u8]) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 5 <= buf.len() {
        let flags = buf[i];
        let len = u32::from_be_bytes([buf[i + 1], buf[i + 2], buf[i + 3], buf[i + 4]]) as usize;
        if len > MAX_FRAME {
            bail!("Devin Connect frame too large");
        }
        if i + 5 + len > buf.len() {
            bail!("Devin Connect stream ended with a partial frame");
        }
        let mut payload = buf[i + 5..i + 5 + len].to_vec();
        i += 5 + len;
        if flags & CONNECT_GZIP != 0 {
            payload = gunzip(&payload)?;
        }
        if flags & CONNECT_END != 0 {
            if let Ok(v) = serde_json::from_slice::<Value>(&payload) {
                if let Some(err) = v.get("error") {
                    let code = err["code"].as_str().unwrap_or("");
                    let message = err["message"].as_str().unwrap_or("");
                    if !code.is_empty() || !message.is_empty() {
                        bail!("Devin stream error{code}: {message}", code = if code.is_empty() { String::new() } else { format!(" {code}") });
                    }
                }
            }
            continue;
        }
        if payload.is_empty() {
            continue;
        }
        match serde_json::from_slice::<Value>(&payload) {
            Ok(v) => out.push(v),
            Err(_) => {}
        }
    }
    Ok(out)
}

fn text_of(blocks: &[Block]) -> String {
    let mut t = String::new();
    for b in blocks {
        if let Block::Text { text } = b {
            if !t.is_empty() {
                t.push('\n');
            }
            t.push_str(text);
        }
    }
    t
}

pub fn chat_prompts(req: &Request) -> Vec<Value> {
    let mut prompts = Vec::new();
    let cascade = if req.session_id.is_empty() {
        "cosmos"
    } else {
        req.session_id
    };
    let mut idx = 0u32;
    for m in req.messages {
        match m.role {
            Role::User | Role::System => {
                for b in &m.content {
                    match b {
                        Block::Text { text } if !text.is_empty() => {
                            idx += 1;
                            prompts.push(json!({
                                "messageId": format!("{cascade}-{idx}"),
                                "source": 1,
                                "prompt": text,
                            }));
                        }
                        Block::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => {
                            idx += 1;
                            prompts.push(json!({
                                "messageId": format!("{cascade}-{idx}"),
                                "source": 4,
                                "prompt": content,
                                "toolCallId": tool_use_id,
                                "toolResultIsError": is_error,
                            }));
                        }
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
                            text.push_str(t);
                        }
                        Block::ToolUse { id, name, input } => {
                            calls.push(json!({
                                "id": id,
                                "name": name,
                                "argumentsJson": input.to_string(),
                            }));
                        }
                        _ => {}
                    }
                }
                idx += 1;
                let mut p = json!({
                    "messageId": format!("{cascade}-{idx}"),
                    "source": 2,
                    "prompt": text,
                });
                if !calls.is_empty() {
                    p["toolCalls"] = json!(calls);
                }
                prompts.push(p);
            }
            Role::Tool => {
                for b in &m.content {
                    if let Block::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = b
                    {
                        idx += 1;
                        prompts.push(json!({
                            "messageId": format!("{cascade}-{idx}"),
                            "source": 4,
                            "prompt": content,
                            "toolCallId": tool_use_id,
                            "toolResultIsError": is_error,
                        }));
                    }
                }
            }
        }
    }
    if prompts.is_empty() {
        let fallback = text_of(
            req.messages
                .last()
                .map(|m| m.content.as_slice())
                .unwrap_or(&[]),
        );
        if !fallback.trim().is_empty() {
            prompts.push(json!({
                "messageId": format!("{cascade}-1"),
                "source": 1,
                "prompt": fallback,
            }));
        }
    }
    prompts
}

fn build_body(creds: &Creds, req: &Request) -> Value {
    let cascade = if req.session_id.is_empty() {
        uuid()
    } else {
        req.session_id.to_string()
    };
    let tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "jsonSchemaString": t.input_schema.to_string(),
                "strict": false,
            })
        })
        .collect();
    let mut body = json!({
        "metadata": metadata(&creds.key),
        "prompt": req.system.join("\n\n"),
        "chatMessagePrompts": chat_prompts(req),
        "chatModelUid": "",
        "requestType": 5,
        "plannerMode": 1,
        "disableParallelToolCalls": true,
        "cascadeId": cascade,
        "executionId": uuid(),
        "systemPromptCacheOptions": { "type": 1 },
        "configuration": {
            "numCompletions": "1",
            "maxTokens": req.max_tokens.to_string(),
            "maxNewlines": "200",
            "temperature": 0.4,
            "firstTemperature": 0.4,
            "topK": "50",
            "topP": 1,
            "stopPatterns": ["<|user|>", "<|bot|>", "<|context_request|>", "<|endoftext|>", "<|end_of_turn|>"],
            "fimEotProbThreshold": 1,
        },
    });
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["toolChoice"] = json!({ "optionName": "auto" });
    }
    body
}

struct ToolAcc {
    id: String,
    name: String,
    args: String,
}

fn feed_tool(acc: &mut Vec<ToolAcc>, v: &Value) {
    let id = v["id"].as_str().unwrap_or("");
    let name = v["name"].as_str().unwrap_or("");
    let args = v["argumentsJson"]
        .as_str()
        .or(v["arguments_json"].as_str())
        .unwrap_or("");
    if !id.is_empty() {
        if let Some(t) = acc.iter_mut().find(|t| t.id == id) {
            if !name.is_empty() {
                t.name = name.into();
            }
            if args.starts_with(&t.args) {
                t.args = args.into();
            } else {
                t.args.push_str(args);
            }
        } else {
            acc.push(ToolAcc {
                id: id.into(),
                name: name.into(),
                args: args.into(),
            });
        }
    } else if let Some(t) = acc.last_mut() {
        if !name.is_empty() {
            t.name = name.into();
        }
        t.args.push_str(args);
    }
}

/// Drive one Cosmos turn through Devin's chat API with *our* tools.
pub fn complete(cfg: &ProviderCfg, req: &Request, sink: &mut dyn FnMut(Chunk)) -> Result<()> {
    let creds = load_creds().context("Devin isn't signed in — run `devin auth login`")?;
    let mut body = build_body(&creds, req);
    let model = cfg.model.trim();
    if !model.is_empty() {
        body["chatModelUid"] = json!(model);
    }
    if body["chatMessagePrompts"]
        .as_array()
        .is_none_or(|a| a.is_empty())
    {
        bail!("nothing to send to Devin");
    }
    let payload = serde_json::to_vec(&body)?;
    let url = format!("{}{CHAT_PATH}", creds.server);
    let mut resp = agent()
        .post(&url)
        .header("content-type", "application/connect+json")
        .header("connect-protocol-version", "1")
        .send(encode_frame(&payload).as_slice())
        .context("couldn't reach Devin")?;
    let status = resp.status().as_u16();
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(MAX_FRAME as u64)
        .read_to_vec()
        .unwrap_or_default();
    if status == 401 || status == 403 {
        bail!("Devin rejected the subscription ({status}) — run `devin auth login`");
    }
    if !(200..300).contains(&status) {
        let preview: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        bail!("Devin chat failed ({status}): {preview}");
    }
    let frames = decode_connect_json(&bytes)?;
    let mut tools = Vec::new();
    let mut usage = Usage::default();
    for v in frames {
        if let Some(t) = v["deltaText"].as_str().or(v["delta_text"].as_str()) {
            if !t.is_empty() {
                sink(Chunk::Text(t.to_string()));
            }
        }
        let calls = v["deltaToolCalls"]
            .as_array()
            .or(v["delta_tool_calls"].as_array());
        if let Some(calls) = calls {
            for c in calls {
                feed_tool(&mut tools, c);
            }
        }
        if let Some(u) = v.get("usage") {
            let input = num_u64(&u["inputTokens"]).max(num_u64(&u["input_tokens"]));
            let output = num_u64(&u["outputTokens"]).max(num_u64(&u["output_tokens"]));
            let cr = num_u64(&u["cacheReadTokens"]).max(num_u64(&u["cache_read_tokens"]));
            let cw = num_u64(&u["cacheWriteTokens"]).max(num_u64(&u["cache_write_tokens"]));
            if input > 0 {
                usage.input = input;
            }
            if output > 0 {
                usage.output = output;
            }
            if cr > 0 {
                usage.cache_read = cr;
            }
            if cw > 0 {
                usage.cache_write = cw;
            }
        }
    }
    for t in tools {
        if t.name.is_empty() {
            continue;
        }
        let input = serde_json::from_str(&t.args).unwrap_or_else(|_| json!({}));
        let id = if t.id.is_empty() { uuid() } else { t.id };
        sink(Chunk::ToolUse {
            id,
            name: t.name,
            input,
        });
    }
    if usage.output == 0 && usage.input == 0 {
        usage.output = 1;
    }
    sink(Chunk::Done(usage));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zproto::{Message, ToolSpec};

    #[test]
    fn parse_devin_catalog() {
        let raw = br#"{"families":[{"variants":[
            {"model_uid":"swe-1-7-medium","label":"SWE-1.7 Medium","cost_summary":"default"},
            {"model_uid":"claude-fable-5-1-high","label":"Claude Fable 5.1 High"},
            {"model_uid":"swe-1-7-medium","label":"dup"},
            {"model_uid":"bare","label":""}
        ]}]}"#;
        let m = parse_list(raw).unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].id, "swe-1-7-medium");
        assert_eq!(m[1].name, "Claude Fable 5.1 High");
        assert_eq!(m[2].name, "bare");
    }

    #[test]
    fn parse_http_model_configs() {
        let v = json!({"clientModelConfigs":[
            {"label":"SWE-2 Max","modelUid":"swe-2-max","disabled":false},
            {"label":"skip","modelUid":"x","disabled":true},
            {"label":"SWE-2 High","modelUid":"swe-2-high"}
        ]});
        let m = parse_cli_model_configs(&v);
        assert_eq!(
            m.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
            ["swe-2-max", "x", "swe-2-high"]
        );
        let unnamed = parse_cli_model_configs(&json!({"clientModelConfigs":[{"modelUid":"bare"}]}));
        assert_eq!(unnamed[0].id, "bare");
        assert_eq!(unnamed[0].name, "bare");
    }

    #[test]
    fn connect_json_frames_round_trip() {
        let payload = br#"{"deltaText":"PONG"}"#;
        let buf = encode_frame(payload);
        let v = decode_connect_json(&buf).unwrap();
        assert_eq!(v[0]["deltaText"], "PONG");
    }

    #[test]
    fn prompts_map_tools_to_our_schema() {
        let tools = [ToolSpec {
            name: "read".into(),
            description: "Read a file".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}}}),
        }];
        let messages = [
            Message::text(Role::User, "hi"),
            Message {
                role: Role::Assistant,
                content: vec![
                    Block::Text { text: "ok".into() },
                    Block::ToolUse {
                        id: "c1".into(),
                        name: "read".into(),
                        input: json!({"path":"a.txt"}),
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    tool_use_id: "c1".into(),
                    content: "hello".into(),
                    is_error: false,
                }],
            },
        ];
        let req = Request {
            system: vec!["you are cosmos".into()],
            tools: &tools,
            messages: &messages,
            session_id: "s1",
            max_tokens: 64,
        };
        let p = chat_prompts(&req);
        assert_eq!(p[0]["source"], 1);
        assert_eq!(p[1]["source"], 2);
        assert_eq!(p[1]["toolCalls"][0]["name"], "read");
        assert_eq!(p[2]["source"], 4);
        assert_eq!(p[2]["toolCallId"], "c1");
    }

    #[test]
    fn toml_key_parser() {
        let t = "windsurf_api_key = \"abc\"\napi_server_url = \"https://server.codeium.com\"\n";
        assert_eq!(toml_basic_string(t, "windsurf_api_key").as_deref(), Some("abc"));
        assert!(trusted_server("https://server.codeium.com"));
        assert!(!trusted_server("https://evil.example"));
    }
}
