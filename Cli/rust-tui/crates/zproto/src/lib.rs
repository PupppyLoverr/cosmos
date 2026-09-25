//! zproto — wire types shared by every Cosmos CLI harness (CONTRACT.md §6).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const CONTRACT_VERSION: u32 = 1;

/// One append-only session event (§6). Serialized as a single JSON line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub v: u32,
    pub id: String,
    pub parent: Option<String>,
    pub ts: String,
    pub session: String,
    pub harness: String,
    pub kind: String,
    #[serde(default)]
    pub data: Value,
}

pub fn new_id() -> String {
    ulid::Ulid::new().to_string().to_lowercase()
}

pub fn now_ts() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl Event {
    pub fn new(
        session: &str,
        harness: &str,
        parent: Option<String>,
        kind: &str,
        data: Value,
    ) -> Self {
        Event {
            v: CONTRACT_VERSION,
            id: new_id(),
            parent,
            ts: now_ts(),
            session: session.into(),
            harness: harness.into(),
            kind: kind.into(),
            data,
        }
    }

    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("event serialize")
    }
}

/// Provider-agnostic conversation content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Block>,
}

impl Message {
    pub fn text(role: Role, s: impl Into<String>) -> Self {
        Message {
            role,
            content: vec![Block::Text { text: s.into() }],
        }
    }
}

/// Tool schema advertised to the model (contract §2 — byte-identical across
/// harnesses, verified by `tools dump`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Normalized token accounting (contract §8).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

/// Per-1M-token USD prices for a model alias.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Prices {
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
}

impl Prices {
    pub fn cost(&self, u: &Usage) -> Option<f64> {
        if *self == Prices::default() {
            return None; // unknown price → never guess
        }
        Some(
            (u.input as f64 * self.input
                + u.cache_read as f64 * self.cache_read
                + u.cache_write as f64 * self.cache_write
                + u.output as f64 * self.output)
                / 1_000_000.0,
        )
    }
}

/// Streamed provider chunk.
#[derive(Debug, Clone)]
pub enum Chunk {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    Done(Usage),
    Err(String),
}

/// Exit codes (contract §7).
pub mod exit {
    pub const DONE: i32 = 0;
    pub const FAILED: i32 = 1;
    pub const USAGE: i32 = 2;
    pub const DENIED: i32 = 3;
    pub const PROVIDER: i32 = 4;
    pub const BUDGET: i32 = 5;
}

/// Headless invocation options (contract §7).
#[derive(Debug, Clone, Default)]
pub struct RunOpts {
    pub prompt: Option<String>,
    pub output: OutputMode,
    pub mode: Option<String>,
    pub allow: Vec<String>,
    pub yolo: bool,
    pub no_sandbox: bool,
    pub model: Option<String>,
    pub max_turns: Option<u32>,
    pub max_usd: Option<f64>,
    pub cwd: Option<String>,
    pub resume: Option<String>,
    pub approve_plan: bool,
    pub allow_escalation: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputMode {
    #[default]
    Text,
    Json,
    JsonStream,
}

impl OutputMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            "json-stream" => Some(Self::JsonStream),
            _ => None,
        }
    }
}

/// Emit a §6 event line on stdout for json-stream mode.
pub fn stream_event(
    session: &str,
    harness: &str,
    parent: Option<String>,
    kind: &str,
    data: Value,
) -> Event {
    let ev = Event::new(session, harness, parent, kind, data);
    println!("{}", ev.to_line());
    ev
}

pub fn empty() -> Value {
    json!({})
}
