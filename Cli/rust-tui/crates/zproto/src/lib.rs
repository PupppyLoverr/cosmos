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

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A lowercase ULID: 48-bit millisecond time + 80 random bits, Crockford
/// base32 (26 chars). Sorts by creation time like `ulid::Ulid`.
pub fn new_id() -> String {
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut rnd = [0u8; 10];
    if getrandom::fill(&mut rnd).is_err() {
        // no OS RNG: fall back to time + address entropy (ids stay unique per process)
        let seed = unix_ms() ^ (&rnd as *const _ as u64).rotate_left(17) ^ std::process::id() as u64;
        for (i, b) in rnd.iter_mut().enumerate() {
            *b = (seed >> ((i % 8) * 8)) as u8 ^ i as u8;
        }
    }
    let mut v: u128 = (unix_ms() as u128 & 0xffff_ffff_ffff) << 80;
    for (i, b) in rnd.iter().enumerate() {
        v |= (*b as u128) << (72 - i * 8);
    }
    (0..26).rev().map(|i| ALPHABET[((v >> (i * 5)) & 31) as usize] as char).collect()
}

/// Current UTC time as RFC 3339 with milliseconds (`2026-10-01T08:17:50.755Z`).
pub fn now_ts() -> String {
    ts_from_ms(unix_ms())
}

fn ts_from_ms(ms: u64) -> String {
    let (secs, milli) = (ms / 1000, ms % 1000);
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_match_rfc3339() {
        assert_eq!(ts_from_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(ts_from_ms(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(ts_from_ms(1_790_843_870_755), "2026-10-01T08:37:50.755Z");
        assert_eq!(now_ts().len(), 24);
    }

    #[test]
    fn ids_are_ulid_shaped_and_sortable() {
        let a = new_id();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = new_id();
        assert_eq!(a.len(), 26);
        assert!(a.chars().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase()));
        assert!(a < b, "{a} < {b}");
        assert_ne!(new_id(), new_id());
    }
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
    pub add_dirs: Vec<String>,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub approve_plan: bool,
    pub allow_escalation: Vec<String>,
    pub sandbox: Option<String>,
    pub ask: Option<String>,
    pub autonomous: bool,
    pub autonomous_gate: Option<String>,
    pub allow_self_mutate: bool,
    pub arena: bool,
    pub self_improve: Option<bool>,
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
            "json-stream" | "stream-json" => Some(Self::JsonStream),
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
