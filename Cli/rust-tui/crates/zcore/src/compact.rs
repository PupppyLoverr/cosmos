//! Five-layer pre-model compaction. Order is mandatory. L1–L4 make zero
//! model calls. Port comments in zdev `compact.ts` and zagy `compact.go`
//! must stay in lockstep with this file.
//!
//! L1 tool-result budget (disk offload)
//! L2 history snip (keep first 3 + newest 40 when count > 48)
//! L3 microcompact (clear consumed tool results except latest 3)
//! L4 context collapse (read-time projection; JSONL untouched)
//! L5 auto-summarize (last resort; structured world.md)
//!
//! Cache warm: last model call < 4 minutes AND prefix hash unchanged.
//! Warm sessions may only edit the tail (never rewrite layers 1–3 prefix).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zproto::{Block, Message, Role};

pub const SNIP_AFTER: usize = 48;
pub const SNIP_KEEP_HEAD: usize = 3;
pub const SNIP_KEEP_NEWEST: usize = 40;
pub const MICRO_KEEP_LATEST: usize = 3;
pub const DEFAULT_KEEP_TAIL: usize = 6;
pub const CACHE_WARM_SECS: u64 = 240;
pub const HEAD_LINES: usize = 24;
pub const TAIL_LINES: usize = 24;
pub const THRASH_MIN_REDUCTION: f64 = 0.20;

#[derive(Debug, Clone)]
pub struct CompactCfg {
    pub large_read_lines: usize,
    pub reserved_output_tokens: u64,
    pub keep_tail: usize,
    pub cache_warm_secs: u64,
    pub trigger_ratio: f64,
}

impl Default for CompactCfg {
    fn default() -> Self {
        Self {
            large_read_lines: 200,
            reserved_output_tokens: 8000,
            keep_tail: DEFAULT_KEEP_TAIL,
            cache_warm_secs: CACHE_WARM_SECS,
            trigger_ratio: 0.80,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompactEvent {
    pub kind: String,
    pub data: Value,
}

#[derive(Debug, Clone)]
pub struct CompactOutcome {
    pub messages: Vec<Message>,
    pub events: Vec<CompactEvent>,
    pub world_md: Option<String>,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub cache_warm: bool,
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

pub fn estimate_tokens(messages: &[Message], boot_tokens: u64) -> u64 {
    boot_tokens
        + messages
            .iter()
            .map(|m| {
                m.content
                    .iter()
                    .map(|b| match b {
                        Block::Text { text } => (text.len() / 4) as u64,
                        Block::ToolResult { content, .. } => (content.len() / 4) as u64,
                        Block::ToolUse { input, name, .. } => {
                            ((name.len() + input.to_string().len()) / 4) as u64 + 8
                        }
                    })
                    .sum::<u64>()
            })
            .sum::<u64>()
}

pub fn cache_is_warm(
    last_model_call: Option<Instant>,
    prefix_now: &str,
    prefix_then: &str,
    now: Instant,
    warm_secs: u64,
) -> bool {
    let Some(t) = last_model_call else {
        return false;
    };
    now.duration_since(t) < Duration::from_secs(warm_secs) && prefix_now == prefix_then
}

fn persist_tool_result(home: &Path, content: &str) -> (PathBuf, String, usize, usize) {
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    let sha = format!("{:x}", h.finalize());
    let dir = home.join("cache").join("tool");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{sha}.txt"));
    if !path.exists() {
        let _ = std::fs::write(&path, content);
    }
    let lines = content.lines().count();
    (path, sha, content.len(), lines)
}

fn middle_out(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= HEAD_LINES + TAIL_LINES {
        return content.to_string();
    }
    let omitted = content.len().saturating_sub(
        lines[..HEAD_LINES].join("\n").len() + lines[lines.len() - TAIL_LINES..].join("\n").len(),
    );
    format!(
        "{}\n… {} chars omitted …\n{}",
        lines[..HEAD_LINES].join("\n"),
        omitted,
        lines[lines.len() - TAIL_LINES..].join("\n")
    )
}

fn is_tool_pair_split(messages: &[Message], idx: usize) -> bool {
    // Never split an assistant tool-call from its following tool-result.
    let Some(m) = messages.get(idx) else {
        return false;
    };
    let has_call = m.content.iter().any(|b| matches!(b, Block::ToolUse { .. }));
    if !has_call {
        return false;
    }
    messages.get(idx + 1).is_some_and(|n| {
        n.content
            .iter()
            .any(|b| matches!(b, Block::ToolResult { .. }))
    })
}

/// L1: persist oversized tool results; replace in-prompt body with a stub.
pub fn layer1_budget(messages: &mut [Message], home: &Path, large_read_lines: usize) {
    let cap_chars = large_read_lines.saturating_mul(40).max(8_000);
    for m in messages.iter_mut() {
        for b in m.content.iter_mut() {
            if let Block::ToolResult { content, .. } = b {
                let lines = content.lines().count();
                if content.len() > cap_chars || lines > large_read_lines {
                    let (path, sha, bytes, nlines) = persist_tool_result(home, content);
                    *content = format!(
                        "[tool_result persisted path={} sha256={} bytes={} lines={}]\n{}\n",
                        path.display(),
                        sha,
                        bytes,
                        nlines,
                        middle_out(content)
                    );
                }
            }
        }
    }
}

/// L2: keep first 3 + newest 40 when message count > 48. Do not split pairs.
pub fn layer2_snip(messages: &mut Vec<Message>) -> Option<CompactEvent> {
    if messages.len() <= SNIP_AFTER {
        return None;
    }
    let mut head_end = SNIP_KEEP_HEAD.min(messages.len());
    while head_end > 0 && is_tool_pair_split(messages, head_end.saturating_sub(1)) {
        head_end += 1;
        if head_end >= messages.len() {
            break;
        }
    }
    let mut tail_start = messages.len().saturating_sub(SNIP_KEEP_NEWEST);
    if tail_start < head_end {
        return None;
    }
    while tail_start > 0 && is_tool_pair_split(messages, tail_start.saturating_sub(1)) {
        tail_start -= 1;
    }
    let dropped = tail_start.saturating_sub(head_end);
    if dropped == 0 {
        return None;
    }
    let mut next = messages[..head_end].to_vec();
    next.push(Message::text(
        Role::User,
        format!("[snipped {dropped} messages; full log in session]"),
    ));
    next.extend_from_slice(&messages[tail_start..]);
    *messages = next;
    Some(CompactEvent {
        kind: "snip".into(),
        data: json!({"dropped": dropped}),
    })
}

/// L3: replace consumed old tool results (not the latest 3) with a cleared stub.
/// Cache-warm: only edit the tail (last 3 messages).
pub fn layer3_microcompact(messages: &mut [Message], cache_warm: bool) {
    if messages.is_empty() {
        return;
    }
    let start = if cache_warm {
        messages.len().saturating_sub(MICRO_KEEP_LATEST)
    } else {
        0
    };
    let protect_from = messages.len().saturating_sub(MICRO_KEEP_LATEST);
    for (i, m) in messages.iter_mut().enumerate() {
        if i < start || i >= protect_from {
            continue;
        }
        for b in m.content.iter_mut() {
            if let Block::ToolResult {
                tool_use_id,
                content,
                ..
            } = b
            {
                if content.starts_with("[cleared tool_result")
                    || content.starts_with("[tool_result persisted")
                {
                    continue;
                }
                let path = content
                    .lines()
                    .find_map(|l| l.split("path=").nth(1).map(|s| s.split_whitespace().next().unwrap_or("").to_string()))
                    .unwrap_or_default();
                *content = format!("[cleared tool_result id={tool_use_id} path={path}]");
            }
        }
    }
}

fn read_paths(m: &Message) -> Vec<String> {
    let mut out = Vec::new();
    for b in &m.content {
        if let Block::ToolUse { name, input, .. } = b {
            if name == "read" {
                if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
                    out.push(p.to_string());
                }
            }
        }
    }
    out
}

/// L4: collapse consecutive read groups. Does not mutate the store.
pub fn layer4_collapse(messages: &[Message]) -> Vec<Message> {
    let mut out = Vec::with_capacity(messages.len());
    let mut i = 0;
    while i < messages.len() {
        let paths = read_paths(&messages[i]);
        if paths.is_empty() {
            out.push(messages[i].clone());
            i += 1;
            continue;
        }
        let mut all = paths;
        let mut j = i + 1;
        while j < messages.len() {
            let more = read_paths(&messages[j]);
            if more.is_empty() {
                // include the matching tool_result then stop grouping
                if messages[j]
                    .content
                    .iter()
                    .any(|b| matches!(b, Block::ToolResult { .. }))
                {
                    j += 1;
                    continue;
                }
                break;
            }
            all.extend(more);
            j += 1;
        }
        if all.len() >= 2 {
            out.push(Message::text(
                Role::User,
                format!("[{} reads: {}]", all.len(), all.join(", ")),
            ));
            i = j;
        } else {
            out.push(messages[i].clone());
            i += 1;
        }
    }
    out
}

pub fn should_l5(tokens: u64, context: u64, reserved: u64, cache_warm: bool, trigger_ratio: f64) -> bool {
    if context == 0 {
        return false;
    }
    let a = (context as f64 * trigger_ratio) as u64;
    let b = context.saturating_sub(reserved).saturating_sub(8000);
    let thresh = a.min(b);
    if tokens >= thresh {
        return true;
    }
    !cache_warm && tokens >= context / 2
}

/// Deterministic structured compact (no model call). Used when
/// `summarizer_model` is empty or the fixture provider is active.
pub fn summarize_deterministic(messages: &[Message], focus: Option<&str>) -> String {
    let mut goal = String::new();
    let mut files: Vec<String> = Vec::new();
    let mut decisions: Vec<String> = Vec::new();
    let mut tests: Vec<String> = Vec::new();
    let mut last_assistant = String::new();
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
                    if let Some(p) = input.get("path").and_then(|v| v.as_str()) {
                        if !files.iter().any(|f| f == p) {
                            files.push(p.to_string());
                        }
                    }
                    if name == "bash" {
                        if let Some(c) = input.get("command").and_then(|v| v.as_str()) {
                            if c.contains("test") {
                                tests.push(c.chars().take(80).collect());
                            }
                        }
                    }
                }
                Block::ToolResult { content, is_error, .. } => {
                    if *is_error {
                        decisions.push(content.chars().take(120).collect());
                    }
                }
                _ => {}
            }
        }
    }
    let focus_line = focus
        .map(|f| format!("focus: {f}\n"))
        .unwrap_or_default();
    format!(
        "# World\n{focus_line}goal: {goal}\nstatus: {}\ndecisions: {}\nfiles_touched: {}\ntests: {}\nopen_questions: \n# Plan\n\n# Next\n{last_assistant}\n",
        if last_assistant.is_empty() { "in progress" } else { "working" },
        decisions.join("; "),
        files.iter().map(|f| format!("{f} (????)")).collect::<Vec<_>>().join(", "),
        tests.join("; "),
    )
}

pub fn layer5_summarize(
    messages: &mut Vec<Message>,
    keep_tail: usize,
    focus: Option<&str>,
) -> (String, CompactEvent) {
    let world = summarize_deterministic(messages, focus);
    let keep = keep_tail.min(messages.len());
    let tail = messages[messages.len() - keep..].to_vec();
    let mut next = Vec::new();
    next.push(Message::text(
        Role::User,
        format!("[compact world]\n{world}"),
    ));
    next.extend(tail);
    *messages = next;
    (
        world,
        CompactEvent {
            kind: "compact".into(),
            data: json!({"reason": "l5", "focus": focus}),
        },
    )
}

/// Run L1–L5. `force_l5` is `/compact` or `new_context`.
pub fn compact(
    messages: &mut Vec<Message>,
    home: &Path,
    session_id: &str,
    cfg: &CompactCfg,
    context_window: u64,
    boot_tokens: u64,
    cache_warm: bool,
    force_l5: bool,
    focus: Option<&str>,
    last_l5_before: Option<u64>,
    last_l5_after: Option<u64>,
) -> CompactOutcome {
    let before = estimate_tokens(messages, boot_tokens);
    let mut events = Vec::new();

    layer1_budget(messages, home, cfg.large_read_lines);
    if let Some(ev) = layer2_snip(messages) {
        events.push(ev);
    }
    layer3_microcompact(messages, cache_warm);

    let after_l3 = estimate_tokens(messages, boot_tokens);
    let mut world_md = None;
    let mut thrash = false;
    if force_l5 || should_l5(after_l3, context_window, cfg.reserved_output_tokens, cache_warm, cfg.trigger_ratio)
    {
        if let (Some(b), Some(a)) = (last_l5_before, last_l5_after) {
            if b > 0 && (a as f64) > (b as f64) * (1.0 - THRASH_MIN_REDUCTION) {
                thrash = true;
                events.push(CompactEvent {
                    kind: "compact_thrash".into(),
                    data: json!({"before": b, "after": a}),
                });
            }
        }
        if !thrash {
            let (md, ev) = layer5_summarize(messages, cfg.keep_tail, focus);
            let dest = home.join("world").join(format!("{session_id}.md"));
            if let Some(p) = dest.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::fs::write(&dest, &md);
            world_md = Some(md);
            events.push(ev);
            events.push(CompactEvent {
                kind: "compaction".into(),
                data: json!({
                    "before_tokens": before,
                    "after_tokens": estimate_tokens(messages, boot_tokens),
                    "reason": if force_l5 { "operator" } else { "auto" },
                }),
            });
        }
    }

    let after = estimate_tokens(messages, boot_tokens);
    CompactOutcome {
        messages: messages.clone(),
        events,
        world_md,
        tokens_before: before,
        tokens_after: after,
        cache_warm,
    }
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
    use zproto::Block;

    fn tr(id: &str, body: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: id.into(),
                content: body.into(),
                is_error: false,
            }],
        }
    }

    fn user(s: &str) -> Message {
        Message::text(Role::User, s)
    }

    fn asst(s: &str) -> Message {
        Message::text(Role::Assistant, s)
    }

    #[test]
    fn prefix_hash_stable() {
        let a = prefix_hash(&["sys".into(), "boot".into()], "[{\"name\":\"read\"}]");
        let b = prefix_hash(&["sys".into(), "boot".into()], "[{\"name\":\"read\"}]");
        assert_eq!(a, b);
        let c = prefix_hash(&["sys".into(), "boot2".into()], "[{\"name\":\"read\"}]");
        assert_ne!(a, c);
    }

    #[test]
    fn l1_persists_large_and_keeps_path() {
        let dir = tempfile::tempdir().unwrap();
        let big = (0..300).map(|i| format!("line{i} {}\n", "x".repeat(40))).collect::<String>();
        let mut msgs = vec![tr("t1", &big)];
        layer1_budget(&mut msgs, dir.path(), 200);
        let body = match &msgs[0].content[0] {
            Block::ToolResult { content, .. } => content.clone(),
            _ => panic!(),
        };
        assert!(body.contains("tool_result persisted path="), "{body}");
        assert!(body.contains("sha256="), "{body}");
        assert!(dir.path().join("cache").join("tool").exists());
    }

    #[test]
    fn l2_snips_60_messages_keeps_pairs() {
        let mut msgs = Vec::new();
        for i in 0..60 {
            msgs.push(user(&format!("u{i}")));
            msgs.push(asst(&format!("a{i}")));
        }
        assert!(msgs.len() > SNIP_AFTER);
        let ev = layer2_snip(&mut msgs).expect("snip");
        assert_eq!(ev.kind, "snip");
        assert!(msgs.len() < 60 * 2);
        assert!(matches!(&msgs[SNIP_KEEP_HEAD].content[0], Block::Text { text } if text.contains("snipped")));
    }

    #[test]
    fn l3_clears_old_results() {
        let mut msgs = vec![
            tr("old", "huge old result body that was consumed"),
            user("mid"),
            tr("keep1", "recent"),
            tr("keep2", "recent2"),
            tr("keep3", "recent3"),
        ];
        layer3_microcompact(&mut msgs, false);
        match &msgs[0].content[0] {
            Block::ToolResult { content, .. } => {
                assert!(content.contains("[cleared tool_result id=old"), "{content}");
            }
            _ => panic!(),
        }
        match &msgs[4].content[0] {
            Block::ToolResult { content, .. } => {
                assert_eq!(content, "recent3");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn l5_keeps_goal() {
        let mut msgs = vec![user("implement the login form"), asst("ok"), user("and tests")];
        for i in 0..10 {
            msgs.push(user(&format!("turn {i}")));
            msgs.push(asst("working"));
        }
        let (md, _) = layer5_summarize(&mut msgs, 6, Some("API changes"));
        assert!(md.contains("goal: implement the login form"), "{md}");
        assert!(md.contains("focus: API changes"), "{md}");
        assert!(msgs.len() <= 7);
    }

    #[test]
    fn cache_warm_requires_prefix_and_clock() {
        let t0 = Instant::now();
        assert!(cache_is_warm(Some(t0), "abc", "abc", t0 + Duration::from_secs(10), 240));
        assert!(!cache_is_warm(Some(t0), "abc", "xyz", t0 + Duration::from_secs(10), 240));
        assert!(!cache_is_warm(
            Some(t0),
            "abc",
            "abc",
            t0 + Duration::from_secs(300),
            240
        ));
    }
}
