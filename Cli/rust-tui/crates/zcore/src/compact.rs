//! Cache-aware five-layer compaction (contract §9).
//!
//! The conversation (`Agent::messages`) is the raw, append-only replay of the
//! session log. What the model sees is `project(raw, ops)`: the raw list with
//! a short list of **sticky** view ops applied. An op is decided once, at a
//! checkpoint, logged as an event, and from then on applied identically to
//! every request and on resume. So between checkpoints each request's
//! messages are a strict extension of the previous one and the provider's
//! prompt cache keeps hitting; the cache is re-paid once per checkpoint, not
//! once per turn.
//!
//! L1 tool-result budget — at write time: big results go to disk, the log and
//!    the model keep a middle-out preview plus the path.
//! L2 snip — keep the first 3 + newest 40 view messages when there are > 48.
//! L3 microcompact — tool results older than 6 turns become one-line stubs.
//! L4 collapse — a read superseded by a later read of the same range becomes
//!    a stub.
//! L5 world — one structured summary (`world/<session>.md`) + a verbatim
//!    tail of ~20k tokens.
//!
//! Checkpoints only happen when the projected input is over 0.80 × window, or
//! the cache is presumed cold (idle past the provider TTL) and the input is
//! over 0.50 × window — never on a warm cache under 80%.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zproto::{Block, Message, Role};

pub const SNIP_AFTER: usize = 48;
pub const SNIP_KEEP_HEAD: usize = 3;
pub const SNIP_KEEP_NEWEST: usize = 40;
/// L3: tool results older than this many assistant turns are stubbed.
pub const MICRO_KEEP_TURNS: usize = 6;
/// L1: results longer than this go to disk (non-`read` tools).
pub const L1_CHARS: usize = 8_000;
/// L1 for `read`: the model asked for that file — only offload past ~6k tokens.
pub const L1_READ_CHARS: usize = 24_000;
/// L1 preview size (middle-out).
pub const L1_PREVIEW: usize = 2_000;
/// L5 verbatim tail budget.
pub const RETAINED_TAIL_TOKENS: u64 = 20_000;
/// Results shorter than this are never stubbed — a stub would not save anything.
const STUB_MIN_CHARS: usize = 400;
pub const HEAD_LINES: usize = 24;
pub const TAIL_LINES: usize = 24;

#[derive(Debug, Clone)]
pub struct CompactCfg {
    pub large_read_lines: usize,
    pub reserved_output_tokens: u64,
    /// 0 = provider default (Anthropic 300 s, others 600 s)
    pub cache_warm_secs: u64,
    pub trigger_ratio: f64,
}

impl Default for CompactCfg {
    fn default() -> Self {
        Self {
            large_read_lines: 200,
            reserved_output_tokens: 8000,
            cache_warm_secs: 0,
            trigger_ratio: 0.80,
        }
    }
}

/// Prompt-cache TTL the harness assumes for a provider.
pub fn cache_ttl_secs(cfg: &CompactCfg, anthropic: bool) -> u64 {
    match cfg.cache_warm_secs {
        0 if anthropic => 300,
        0 => 600,
        s => s,
    }
}

/// A sticky model-view transform. Indices are positions in the raw message
/// list, which is append-only, so they never move.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ViewOp {
    /// Keep the first `head` view messages, a marker, then raw[from..].
    Snip { head: usize, from: usize, dropped: usize },
    /// Replace these tool results (by tool_use_id) with stub text.
    Stub { layer: String, stubs: Vec<(String, String)> },
    /// World summary, then raw[from..].
    World { world: String, from: usize },
}

impl ViewOp {
    /// Event kind this op is logged as (contract §6).
    pub fn kind(&self) -> &'static str {
        match self {
            ViewOp::Snip { .. } => "snip",
            ViewOp::Stub { .. } => "compact",
            ViewOp::World { .. } => "compaction",
        }
    }

    /// Rebuild an op from a logged event, if it carries one.
    pub fn from_event(kind: &str, data: &Value) -> Option<ViewOp> {
        match kind {
            "snip" | "compact" | "compaction" => serde_json::from_value(data.get("view")?.clone()).ok(),
            _ => None,
        }
    }
}

/// Apply sticky ops to the raw conversation → the messages the model sees.
pub fn project(raw: &[Message], ops: &[ViewOp]) -> Vec<Message> {
    /// A view position: a raw message or an op-generated marker.
    #[derive(Clone, Copy)]
    enum Slot {
        Raw(usize),
        Marker(usize),
    }
    let mut markers: Vec<Message> = Vec::new();
    let mut stubs: HashMap<&str, &str> = HashMap::new();
    let mut slots: Vec<Slot> = (0..raw.len()).map(Slot::Raw).collect();
    // every op keeps raw[from..], so messages appended after it survive
    let keep_from = |slots: &[Slot], from: usize| -> Vec<Slot> {
        slots.iter().copied().filter(|s| matches!(s, Slot::Raw(i) if *i >= from)).collect()
    };
    for op in ops {
        match op {
            ViewOp::Snip { head, from, dropped } => {
                let head = (*head).min(slots.len());
                markers.push(Message::text(
                    Role::User,
                    format!("[snipped {dropped} earlier messages; the full log is in the session file]"),
                ));
                let mut next = slots[..head].to_vec();
                next.push(Slot::Marker(markers.len() - 1));
                next.extend(keep_from(&slots[head..], *from));
                slots = next;
            }
            ViewOp::World { world, from } => {
                markers.push(Message::text(Role::User, format!("[compact world]\n{world}")));
                let mut next = vec![Slot::Marker(markers.len() - 1)];
                next.extend(keep_from(&slots, *from));
                slots = next;
            }
            ViewOp::Stub { stubs: list, .. } => {
                for (id, text) in list {
                    stubs.insert(id.as_str(), text.as_str());
                }
            }
        }
    }
    slots
        .into_iter()
        .map(|s| match s {
            Slot::Marker(k) => markers[k].clone(),
            Slot::Raw(i) => {
                let mut m = raw[i].clone();
                for b in m.content.iter_mut() {
                    if let Block::ToolResult { tool_use_id, content, .. } = b {
                        if let Some(s) = stubs.get(tool_use_id.as_str()) {
                            *content = (*s).to_string();
                        }
                    }
                }
                m
            }
        })
        .collect()
}

/// Byte-stable hash of prompt layers 1–3 (system + tools + boot).
pub fn prefix_hash(system: &[String], tools_json: &str) -> String {
    let mut h = Sha256::new();
    for s in system {
        h.update(s.as_bytes());
        h.update([0u8]);
    }
    h.update(tools_json.as_bytes());
    format!("{:x}", h.finalize())
}

fn block_tokens(b: &Block) -> u64 {
    match b {
        Block::Text { text } => (text.len() / 4) as u64,
        Block::ToolResult { content, .. } => (content.len() / 4) as u64 + 4,
        Block::ToolUse { input, name, .. } => ((name.len() + input.to_string().len()) / 4) as u64 + 8,
    }
}

pub fn message_tokens(m: &Message) -> u64 {
    m.content.iter().map(block_tokens).sum::<u64>() + 4
}

pub fn estimate_tokens(messages: &[Message], boot_tokens: u64) -> u64 {
    boot_tokens + messages.iter().map(message_tokens).sum::<u64>()
}

pub fn cache_is_warm(last_model_call: Option<Instant>, prefix_now: &str, prefix_then: &str, now: Instant, warm_secs: u64) -> bool {
    let Some(t) = last_model_call else {
        return false;
    };
    now.duration_since(t) < Duration::from_secs(warm_secs) && prefix_now == prefix_then
}

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

fn floor_char(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Middle-out to roughly `budget` chars: head and tail halves, cut on line
/// boundaries when one is near. Returns (preview, omitted_chars).
pub fn middle_out_chars(content: &str, budget: usize) -> (String, usize) {
    if content.len() <= budget {
        return (content.to_string(), 0);
    }
    let half = budget / 2;
    let mut head_end = floor_char(content, half);
    if let Some(nl) = content[..head_end].rfind('\n') {
        if nl > half / 2 {
            head_end = nl + 1;
        }
    }
    let mut tail_start = ceil_char(content, content.len() - half);
    if let Some(nl) = content[tail_start..].find('\n') {
        if nl < half / 2 {
            tail_start += nl + 1;
        }
    }
    let omitted = tail_start - head_end;
    (
        format!("{}\n… {omitted} chars omitted …\n{}", &content[..head_end], &content[tail_start..]),
        omitted,
    )
}

/// L1 — applied once, when a tool result is produced. Results over the budget
/// are written to `cache/tool/<sha256>.txt` and replaced (in the log and the
/// model view alike) by a preview that names the file and the omitted size.
pub fn budget_result(home: &Path, tool: &str, content: String) -> String {
    let limit = if tool == "read" { L1_READ_CHARS } else { L1_CHARS };
    if content.len() <= limit {
        return content;
    }
    let sha = sha256_hex(&content);
    let dir = home.join("cache").join("tool");
    let _ = std::fs::create_dir_all(&dir);
    let path: PathBuf = dir.join(format!("{sha}.txt"));
    if !path.exists() {
        let _ = std::fs::write(&path, &content);
    }
    let (preview, omitted) = middle_out_chars(&content, L1_PREVIEW);
    format!(
        "[tool_result persisted log_path={} sha256={} chars={} omitted_chars={omitted} — `read` the log_path with start/end to see more]\n{preview}",
        path.display(),
        sha,
        content.len(),
    )
}

/// Never let a kept range start on a tool result whose call was cut away.
fn starts_clean(raw: &[Message], i: usize) -> bool {
    raw.get(i).map_or(true, |m| !m.content.iter().any(|b| matches!(b, Block::ToolResult { .. })))
}

/// L2 — decided on the current view. `view_idx` maps view positions to raw
/// indices (None for markers).
fn plan_snip(view_idx: &[Option<usize>], view: &[Message], raw: &[Message]) -> Option<ViewOp> {
    if view.len() <= SNIP_AFTER {
        return None;
    }
    // head: never end on a tool call whose result is cut away
    let mut head = SNIP_KEEP_HEAD.min(view.len());
    while head < view.len() && view[head - 1].content.iter().any(|b| matches!(b, Block::ToolUse { .. })) {
        head += 1;
    }
    let mut start = view.len().saturating_sub(SNIP_KEEP_NEWEST).max(head);
    // tail must start on a raw message that is not an orphan tool result
    while start < view.len() && (view_idx[start].is_none() || !starts_clean(raw, view_idx[start].unwrap())) {
        start += 1;
    }
    if start >= view.len() {
        return None;
    }
    let dropped = start - head;
    if dropped == 0 {
        return None;
    }
    Some(ViewOp::Snip { head, from: view_idx[start].unwrap(), dropped })
}

fn tool_calls(view: &[Message]) -> HashMap<String, (String, Value)> {
    let mut out = HashMap::new();
    for m in view {
        for b in &m.content {
            if let Block::ToolUse { id, name, input } = b {
                out.insert(id.clone(), (name.clone(), input.clone()));
            }
        }
    }
    out
}

fn is_stub(content: &str) -> bool {
    content.starts_with("[stub ") || content.starts_with("[superseded ")
}

fn stub_text(name: &str, input: &Value, content: &str, is_error: bool) -> String {
    let path = input
        .get("path")
        .and_then(|v| v.as_str())
        .or_else(|| input.get("command").and_then(|v| v.as_str()))
        .or_else(|| input.get("pattern").and_then(|v| v.as_str()))
        .unwrap_or("");
    let path: String = path.chars().take(120).collect();
    let saved = content
        .strip_prefix("[tool_result persisted log_path=")
        .and_then(|r| r.split_whitespace().next())
        .map(|p| format!(" log_path={p}"))
        .unwrap_or_default();
    format!(
        "[stub tool={name} ok={} path={path} sha={} chars={}{saved} — re-run the tool if you need it]",
        !is_error,
        &sha256_hex(content)[..12],
        content.len()
    )
}

/// L3 — tool results older than `MICRO_KEEP_TURNS` assistant turns.
fn plan_microcompact(view: &[Message]) -> Vec<(String, String)> {
    let calls = tool_calls(view);
    let mut turns = 0;
    let mut cutoff = 0;
    for (i, m) in view.iter().enumerate().rev() {
        if m.role == Role::Assistant {
            turns += 1;
            if turns == MICRO_KEEP_TURNS {
                cutoff = i;
                break;
            }
        }
    }
    let mut out = Vec::new();
    for m in &view[..cutoff] {
        for b in &m.content {
            if let Block::ToolResult { tool_use_id, content, is_error } = b {
                if content.len() < STUB_MIN_CHARS || is_stub(content) {
                    continue;
                }
                let (name, input) = calls.get(tool_use_id).cloned().unwrap_or_else(|| ("tool".into(), json!({})));
                out.push((tool_use_id.clone(), stub_text(&name, &input, content, *is_error)));
            }
        }
    }
    out
}

/// L4 — reads superseded by a later read of the same path and range.
fn plan_collapse(view: &[Message], already: &[(String, String)]) -> Vec<(String, String)> {
    let calls = tool_calls(view);
    let done: std::collections::HashSet<&str> = already.iter().map(|(id, _)| id.as_str()).collect();
    let key = |input: &Value| {
        format!(
            "{}#{}#{}",
            input.get("path").and_then(|v| v.as_str()).unwrap_or(""),
            input.get("start").map(|v| v.to_string()).unwrap_or_default(),
            input.get("end").map(|v| v.to_string()).unwrap_or_default()
        )
    };
    let mut latest: HashMap<String, String> = HashMap::new();
    let mut order: Vec<(String, String, usize)> = Vec::new();
    for m in view {
        for b in &m.content {
            if let Block::ToolResult { tool_use_id, content, is_error } = b {
                if *is_error || is_stub(content) {
                    continue;
                }
                if let Some((name, input)) = calls.get(tool_use_id) {
                    if name == "read" {
                        let k = key(input);
                        latest.insert(k.clone(), tool_use_id.clone());
                        order.push((k, tool_use_id.clone(), content.len()));
                    }
                }
            }
        }
    }
    order
        .into_iter()
        .filter(|(k, id, len)| latest.get(k) != Some(id) && !done.contains(id.as_str()) && *len >= STUB_MIN_CHARS)
        .map(|(k, id, _)| {
            let path = k.split('#').next().unwrap_or("").to_string();
            (id, format!("[superseded by a later read of {path}]"))
        })
        .collect()
}

/// L5 tail: newest raw messages worth up to `budget` tokens, starting clean.
fn plan_tail(raw: &[Message], floor: usize, budget: u64) -> usize {
    let mut from = raw.len();
    let mut used = 0u64;
    while from > floor {
        let t = message_tokens(&raw[from - 1]);
        if used + t > budget && from < raw.len() {
            break;
        }
        used += t;
        from -= 1;
    }
    while from < raw.len() && !starts_clean(raw, from) {
        from += 1;
    }
    from
}

/// The structured world summary template (contract §9 L5).
pub const SUMMARY_PROMPT: &str = "Summarize this coding session for your future self. Reply with exactly these markdown sections and nothing else:\n# World\ngoal: <the user's goal, one line>\nstatus: <done | in progress | blocked>\ndecisions: <key decisions, ;-separated>\nfiles_touched: <path (short sha), …>\nfailing: <failing commands or tests, or none>\n# Next\n<the single next step>";

/// Deterministic structured summary (no model call). Used for the fixture
/// provider, when no summarizer is available, or when the model call fails.
pub fn summarize_deterministic(messages: &[Message], focus: Option<&str>) -> String {
    let mut goal = String::new();
    let mut files: Vec<String> = Vec::new();
    let mut decisions: Vec<String> = Vec::new();
    let mut failing: Vec<String> = Vec::new();
    let mut last_assistant = String::new();
    let calls = tool_calls(messages);
    for m in messages {
        for b in &m.content {
            match b {
                Block::Text { text } if m.role == Role::User && goal.is_empty() => {
                    if !text.starts_with('[') {
                        goal = text.chars().take(400).collect();
                    }
                }
                Block::Text { text } if m.role == Role::Assistant => {
                    last_assistant = text.chars().take(400).collect();
                }
                Block::ToolUse { name, input, .. } => {
                    if matches!(name.as_str(), "write" | "edit" | "read") {
                        if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
                            if !files.iter().any(|f| f == p) {
                                files.push(p.to_string());
                            }
                        }
                    }
                }
                Block::ToolResult { content, is_error, tool_use_id } => {
                    if *is_error {
                        let what = calls
                            .get(tool_use_id)
                            .map(|(n, i)| format!("{n} {}", i.get("command").or(i.get("path")).map(|v| v.to_string()).unwrap_or_default()))
                            .unwrap_or_default();
                        failing.push(format!("{what}: {}", content.chars().take(100).collect::<String>()));
                    } else if content.starts_with("plan submitted") {
                        decisions.push("plan submitted".into());
                    }
                }
                _ => {}
            }
        }
    }
    let focus_line = focus.map(|f| format!("focus: {f}\n")).unwrap_or_default();
    format!(
        "# World\n{focus_line}goal: {goal}\nstatus: {}\ndecisions: {}\nfiles_touched: {}\nfailing: {}\n# Next\n{last_assistant}\n",
        if last_assistant.is_empty() { "in progress" } else { "working" },
        decisions.join("; "),
        files.join(", "),
        if failing.is_empty() { "none".to_string() } else { failing.join("; ") },
    )
}

/// Why a checkpoint ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// projected input over the hot threshold (~0.80 × window)
    Hot,
    /// cache cold and input over 0.50 × window
    Cold,
    /// `/compact` or `new_context`
    Operator,
}

/// Contract §9 trigger.
pub fn trigger(tokens: u64, window: u64, reserved: u64, cache_warm: bool, ratio: f64) -> Option<Trigger> {
    if window == 0 {
        return None;
    }
    if tokens >= hot_threshold(window, reserved, ratio) {
        return Some(Trigger::Hot);
    }
    if !cache_warm && tokens >= window / 2 {
        return Some(Trigger::Cold);
    }
    None
}

pub fn hot_threshold(window: u64, reserved: u64, ratio: f64) -> u64 {
    let a = (window as f64 * ratio) as u64;
    let b = window.saturating_sub(reserved).saturating_sub(8000);
    a.min(b).max(1)
}

/// What a checkpoint decided. The caller logs each op, then (for L5) asks the
/// summarizer and builds the `World` op with `plan_world`.
pub struct Checkpoint {
    pub ops: Vec<ViewOp>,
    pub tokens_before: u64,
    pub tokens_after: u64,
    /// L5 should run (still over the threshold, or forced)
    pub needs_world: bool,
}

/// Run L2–L4 on the current view and decide whether L5 is needed.
pub fn checkpoint(raw: &[Message], ops: &[ViewOp], boot_tokens: u64, window: u64, cfg: &CompactCfg, why: Trigger) -> Checkpoint {
    let view = project(raw, ops);
    let before = estimate_tokens(&view, boot_tokens);
    let mut new_ops = Vec::new();
    let mut all = ops.to_vec();

    let idx = view_indices(raw, &all);
    if let Some(op) = plan_snip(&idx, &view, raw) {
        all.push(op.clone());
        new_ops.push(op);
    }
    let view = project(raw, &all);
    let l3 = plan_microcompact(&view);
    if !l3.is_empty() {
        let op = ViewOp::Stub { layer: "l3".into(), stubs: l3.clone() };
        all.push(op.clone());
        new_ops.push(op);
    }
    let view = project(raw, &all);
    let l4 = plan_collapse(&view, &l3);
    if !l4.is_empty() {
        let op = ViewOp::Stub { layer: "l4".into(), stubs: l4 };
        all.push(op.clone());
        new_ops.push(op);
    }
    let after = estimate_tokens(&project(raw, &all), boot_tokens);
    let limit = match why {
        Trigger::Hot | Trigger::Operator => hot_threshold(window, cfg.reserved_output_tokens, cfg.trigger_ratio),
        Trigger::Cold => window / 2,
    };
    Checkpoint {
        ops: new_ops,
        tokens_before: before,
        tokens_after: after,
        needs_world: why == Trigger::Operator || after >= limit,
    }
}

/// L5 op: `world` + the newest ~20k tokens verbatim.
pub fn plan_world(raw: &[Message], ops: &[ViewOp], world: String) -> ViewOp {
    let floor = ops
        .iter()
        .rev()
        .find_map(|o| match o {
            ViewOp::World { from, .. } => Some(*from),
            _ => None,
        })
        .unwrap_or(0);
    let from = plan_tail(raw, floor, RETAINED_TAIL_TOKENS);
    ViewOp::World { world, from }
}

/// Raw index for each view position (None for markers).
pub fn view_indices(raw: &[Message], ops: &[ViewOp]) -> Vec<Option<usize>> {
    // Project a list of tagged one-block messages that carry their raw index.
    let tagged: Vec<Message> = (0..raw.len()).map(|i| Message::text(Role::User, format!("\u{0}{i}"))).collect();
    project(&tagged, ops)
        .iter()
        .map(|m| match m.content.first() {
            Some(Block::Text { text }) => text.strip_prefix('\u{0}').and_then(|n| n.parse().ok()),
            _ => None,
        })
        .collect()
}

/// Stable hasher for doom-loop detection (name + canonical args).
pub fn call_hash(name: &str, input: &Value) -> u64 {
    let mut h = DefaultHasher::new();
    name.hash(&mut h);
    input.to_string().hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(id: &str, body: &str) -> Message {
        Message { role: Role::User, content: vec![Block::ToolResult { tool_use_id: id.into(), content: body.into(), is_error: false }] }
    }
    fn call(id: &str, name: &str, input: Value) -> Message {
        Message { role: Role::Assistant, content: vec![Block::ToolUse { id: id.into(), name: name.into(), input }] }
    }
    fn user(s: &str) -> Message {
        Message::text(Role::User, s)
    }
    fn asst(s: &str) -> Message {
        Message::text(Role::Assistant, s)
    }
    fn body(m: &Message) -> String {
        match &m.content[0] {
            Block::ToolResult { content, .. } => content.clone(),
            Block::Text { text } => text.clone(),
            _ => String::new(),
        }
    }

    #[test]
    fn prefix_hash_stable() {
        let a = prefix_hash(&["sys".into(), "boot".into()], "[{\"name\":\"read\"}]");
        assert_eq!(a, prefix_hash(&["sys".into(), "boot".into()], "[{\"name\":\"read\"}]"));
        assert_ne!(a, prefix_hash(&["sys".into(), "boot2".into()], "[{\"name\":\"read\"}]"));
    }

    #[test]
    fn l1_persists_large_results_with_exact_omitted() {
        let dir = tempfile::tempdir().unwrap();
        let big = (0..400).map(|i| format!("line{i} {}\n", "x".repeat(40))).collect::<String>();
        let out = budget_result(dir.path(), "grep", big.clone());
        assert!(out.starts_with("[tool_result persisted log_path="), "{out}");
        let path = out.split("log_path=").nth(1).unwrap().split_whitespace().next().unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), big);
        let omitted: usize = out.split("omitted_chars=").nth(1).unwrap().split_whitespace().next().unwrap().parse().unwrap();
        let (preview, o2) = middle_out_chars(&big, L1_PREVIEW);
        assert_eq!(omitted, o2);
        assert_eq!(preview.len() - preview.split('\n').filter(|l| l.starts_with('…')).map(|l| l.len() + 2).sum::<usize>() + omitted, big.len());
        // small results and normal-size reads pass through untouched
        assert_eq!(budget_result(dir.path(), "grep", "tiny".into()), "tiny");
        let mid = "y".repeat(L1_CHARS + 10);
        assert_eq!(budget_result(dir.path(), "read", mid.clone()), mid);
    }

    #[test]
    fn middle_out_never_splits_utf8() {
        let s = "é".repeat(5000);
        let (p, _) = middle_out_chars(&s, 2001);
        assert!(p.contains("chars omitted"));
    }

    #[test]
    fn snip_keeps_head_and_newest_and_is_sticky() {
        let mut raw = Vec::new();
        for i in 0..60 {
            raw.push(user(&format!("u{i}")));
            raw.push(asst(&format!("a{i}")));
        }
        let cfg = CompactCfg::default();
        let cp = checkpoint(&raw, &[], 0, 1000, &cfg, Trigger::Operator);
        let ViewOp::Snip { head, from, dropped } = cp.ops[0].clone() else { panic!("{:?}", cp.ops) };
        assert_eq!(head, SNIP_KEEP_HEAD);
        let view = project(&raw, &cp.ops);
        assert!(body(&view[head]).contains(&format!("snipped {dropped}")));
        assert_eq!(view.len(), head + 1 + (raw.len() - from));
        // append more raw messages — the old view is a strict prefix of the new
        let before = project(&raw, &cp.ops);
        raw.push(user("next"));
        let after = project(&raw, &cp.ops);
        assert_eq!(after.len(), before.len() + 1);
        assert_eq!(body(&after[after.len() - 1]), "next");
        for (a, b) in before.iter().zip(after.iter()) {
            assert_eq!(body(a), body(b));
        }
    }

    #[test]
    fn microcompact_stubs_results_older_than_six_turns() {
        let mut raw = Vec::new();
        for i in 0..10 {
            raw.push(call(&format!("c{i}"), "read", json!({"path": format!("f{i}.rs")})));
            raw.push(tr(&format!("c{i}"), &"z".repeat(1000)));
        }
        let cp = checkpoint(&raw, &[], 0, 100, &CompactCfg::default(), Trigger::Operator);
        let view = project(&raw, &cp.ops);
        let stubbed: Vec<String> = view.iter().map(body).filter(|b| b.starts_with("[stub ")).collect();
        // turns 0..=4 are older than the 6 newest assistant turns (4..=9 keep)
        assert_eq!(stubbed.len(), 4, "{stubbed:?}");
        assert!(stubbed[0].contains("tool=read") && stubbed[0].contains("path=f0.rs"), "{}", stubbed[0]);
        assert_eq!(body(&view[view.len() - 1]), "z".repeat(1000));
    }

    #[test]
    fn collapse_only_touches_same_path_reads() {
        // two different files read back to back, then a.rs read again
        let raw = vec![
            user("go"),
            call("r1", "read", json!({"path": "a.rs"})),
            tr("r1", &"A".repeat(500)),
            call("r2", "read", json!({"path": "b.rs"})),
            tr("r2", &"B".repeat(500)),
            call("r3", "read", json!({"path": "a.rs"})),
            tr("r3", &"a".repeat(500)),
        ];
        let stubs = plan_collapse(&raw, &[]);
        assert_eq!(stubs.len(), 1);
        assert_eq!(stubs[0].0, "r1");
        let view = project(&raw, &[ViewOp::Stub { layer: "l4".into(), stubs }]);
        assert_eq!(body(&view[4]), "B".repeat(500), "b.rs content still visible");
        assert_eq!(body(&view[6]), "a".repeat(500), "latest read still visible");
        assert!(body(&view[2]).starts_with("[superseded by a later read of a.rs"));
    }

    #[test]
    fn world_keeps_clean_tail() {
        let mut raw = vec![user("implement the login form")];
        for i in 0..30 {
            raw.push(call(&format!("c{i}"), "bash", json!({"command": "cargo test"})));
            raw.push(tr(&format!("c{i}"), &"t".repeat(8000)));
        }
        let op = plan_world(&raw, &[], summarize_deterministic(&raw, Some("API")));
        let ViewOp::World { from, ref world } = op else { panic!() };
        assert!(world.contains("goal: implement the login form"), "{world}");
        assert!(world.contains("focus: API"));
        assert!(starts_clean(&raw, from), "tail never starts on an orphan tool result");
        let view = project(&raw, std::slice::from_ref(&op));
        let tail_tokens = estimate_tokens(&view[1..], 0);
        assert!(tail_tokens <= RETAINED_TAIL_TOKENS + 2100, "{tail_tokens}");
        assert!(body(&view[0]).starts_with("[compact world]"));
    }

    #[test]
    fn trigger_respects_warm_cache() {
        // warm, 60% → nothing; cold, 60% → cold; 85% → hot either way
        assert_eq!(trigger(60_000, 100_000, 0, true, 0.8), None);
        assert_eq!(trigger(60_000, 100_000, 0, false, 0.8), Some(Trigger::Cold));
        assert_eq!(trigger(85_000, 100_000, 0, true, 0.8), Some(Trigger::Hot));
        assert_eq!(trigger(40_000, 100_000, 0, false, 0.8), None);
    }

    #[test]
    fn ops_round_trip_through_events() {
        let op = ViewOp::Stub { layer: "l3".into(), stubs: vec![("a".into(), "[stub x]".into())] };
        let data = json!({"view": op});
        assert_eq!(ViewOp::from_event("compact", &data), Some(op));
        assert_eq!(ViewOp::from_event("user", &data), None);
    }

    #[test]
    fn cache_warm_requires_prefix_and_clock() {
        let t0 = Instant::now();
        assert!(cache_is_warm(Some(t0), "abc", "abc", t0 + Duration::from_secs(10), 240));
        assert!(!cache_is_warm(Some(t0), "abc", "xyz", t0 + Duration::from_secs(10), 240));
        assert!(!cache_is_warm(Some(t0), "abc", "abc", t0 + Duration::from_secs(300), 240));
        assert_eq!(cache_ttl_secs(&CompactCfg::default(), true), 300);
        assert_eq!(cache_ttl_secs(&CompactCfg::default(), false), 600);
    }
}
