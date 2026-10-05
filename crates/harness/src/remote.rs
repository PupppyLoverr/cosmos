//! Remote-run feeds: translate a seat's streamed harness stdout into the same
//! [`AgentEvent`] stream a local harness emits.
//!
//! The engine folds those events through the exact local pipeline
//! (`fold_event_into_parts` → `render_parts` → `SegmentWriter`), so a cloud
//! run's transcript — text, tool chips, reasoning, errors — renders
//! identically to a local run instead of flattening into a text blob.
//!
//! Line-buffered: [`RemoteFeed::push`] takes raw bytes (ANSI already stripped
//! upstream), completes JSONL frames, and hands each to the command's mapper.
//! [`RemoteFeed::finish`] flushes the trailing unterminated line.

use std::collections::{HashMap, HashSet};

use serde_json::Value;
use zeron_proto::{AgentEvent, HarnessId, ToolCall};

use crate::claude::normalize::Normalizer;
use crate::claude::wire::parse_frame;
use crate::opencode::oc_tool_call;

/// The stdout event feed for one seat command.
pub struct RemoteFeed {
    partial: Vec<u8>,
    kind: FeedKind,
}

enum FeedKind {
    /// `claude -p --output-format stream-json` — the local normalizer itself.
    Claude(Normalizer),
    /// `opencode run --format json` — part snapshots deduped into deltas.
    Opencode(OpencodeFeed),
    /// Unknown harness: raw lines become text deltas.
    Passthrough,
}

impl RemoteFeed {
    /// Pick the feed a seat command needs by its argv.
    pub fn for_command(command: &str) -> Self {
        let kind = if command.contains("--output-format stream-json") {
            FeedKind::Claude(Normalizer::new(HarnessId::ClaudeCode))
        } else if command.contains("--format json") && command_bin_is(command, "opencode") {
            FeedKind::Opencode(OpencodeFeed::default())
        } else {
            FeedKind::Passthrough
        };
        Self {
            partial: Vec::new(),
            kind,
        }
    }

    /// Feed raw output; returns the events every completed line produced.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<AgentEvent> {
        self.partial.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(at) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=at).collect();
            events.extend(self.render_line(&String::from_utf8_lossy(&line)));
        }
        events
    }

    /// The stream ended: render a last unterminated line, if any.
    pub fn finish(&mut self) -> Vec<AgentEvent> {
        let line = std::mem::take(&mut self.partial);
        self.render_line(&String::from_utf8_lossy(&line))
    }

    fn render_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return Vec::new();
        }
        if line.trim().is_empty() {
            // Blank passthrough lines keep paragraph breaks.
            return vec![AgentEvent::TextDelta { text: "\n".into() }];
        }
        match &mut self.kind {
            FeedKind::Claude(normalizer) => match parse_frame(line.trim()) {
                Ok(frame) => normalizer.normalize(frame, false),
                Err(_) => vec![passthrough_delta(line)],
            },
            FeedKind::Opencode(feed) => feed.render_line(line),
            FeedKind::Passthrough => vec![passthrough_delta(line)],
        }
    }
}

/// A plain stdout line as a hard-broken text line (seat harnesses print
/// terminal text; markdown would fold a bare `\n` into one paragraph).
fn passthrough_delta(line: &str) -> AgentEvent {
    AgentEvent::TextDelta {
        text: format!("{}  \n", strip_orphaned_ansi(line)),
    }
}

/// A remote transcript tool call from a raw `{name, input}` pair. The
/// opencode table covers the common genus after a case fold, so claude-style
/// names (`Bash`, `Read`, `Task`) map onto the same typed calls; anything
/// else stays `Unknown` with its input attached — the same shape the local
/// fold renders.
pub fn remote_tool_call(name: &str, input: &Value) -> ToolCall {
    oc_tool_call(&name.to_lowercase(), input)
}

/// opencode's `> mode · model` banner line (`> build · big-pickle`).
fn is_opencode_header_line(line: &str) -> bool {
    line.strip_prefix("> ").is_some_and(|header| {
        let Some((a, b)) = header.split_once(" · ") else {
            return false;
        };
        let token = |part: &str| {
            !part.is_empty() && !part.chars().any(|ch| ch.is_whitespace() || ch == '·')
        };
        token(a) && token(b)
    })
}

fn command_bin_is(command: &str, name: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .and_then(|exe| std::path::Path::new(exe).file_name())
        .and_then(|base| base.to_str())
        == Some(name)
}

/// CSI fragments left when a control byte was eaten before the stream
/// reached us (`[33m`, `[?25h`). Matches `[` + CSI params + final byte —
/// narrow enough that real text rarely sacrifices a `[K`.
fn strip_orphaned_ansi(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut copy_from = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let mut j = i + 1;
            while j < bytes.len() && matches!(bytes[j], b'0'..=b'9' | b';' | b'?' | b':') {
                j += 1;
            }
            if j > i + 1 && j < bytes.len() && (0x40..=0x7e).contains(&bytes[j]) {
                out.push_str(&text[copy_from..i]);
                i = j + 1;
                copy_from = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&text[copy_from..]);
    out
}

/// Maps `opencode run --format json` part events onto the same stream the
/// local opencode driver emits. CLI parts arrive as full snapshots (text
/// grows in place, tool state transitions pending→completed), so each is
/// deduped into deltas before folding.
#[derive(Default)]
struct OpencodeFeed {
    /// part id → bytes of text already emitted (snapshot dedup).
    text_emitted: HashMap<String, usize>,
    reasoning_emitted: HashMap<String, usize>,
    /// Tool calls already announced (first snapshot emits the chip).
    seen_calls: HashSet<String>,
    /// Tool calls already resolved (terminal state emits one result).
    resolved_calls: HashSet<String>,
}

impl OpencodeFeed {
    fn render_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            // The `> build · model` banner opencode prints ahead of its
            // JSONL is chrome, not transcript — drop it like the seat tail
            // used to.
            if is_opencode_header_line(line) {
                return Vec::new();
            }
            return vec![passthrough_delta(line)];
        };
        let part = event.get("part").cloned().unwrap_or_default();
        match event.get("type").and_then(Value::as_str) {
            Some("text") => self.emit_text(&part, "text").into_iter().collect(),
            Some("reasoning") => self.emit_text(&part, "reasoning").into_iter().collect(),
            Some("tool_use") => self.emit_tool(&part),
            Some("error") => vec![AgentEvent::Error {
                message: event
                    .get("error")
                    .or_else(|| part.get("error"))
                    .map(|e| {
                        e.as_str()
                            .map(str::to_string)
                            .or_else(|| {
                                e.get("message")
                                    .or_else(|| e.get("data"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                            .unwrap_or_else(|| e.to_string())
                    })
                    .unwrap_or_else(|| "the agent stopped with an error".into()),
            }],
            // step_start / step_finish / session bookkeeping: not transcript.
            _ => Vec::new(),
        }
    }

    /// A text/reasoning part snapshot → a delta of what hasn't been emitted.
    fn emit_text(&mut self, part: &Value, kind: &str) -> Option<AgentEvent> {
        let text = part.get("text").and_then(Value::as_str)?;
        let id = part
            .get("id")
            .or_else(|| part.get("messageID"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let seen = match kind {
            "reasoning" => &mut self.reasoning_emitted,
            _ => &mut self.text_emitted,
        };
        let emitted = seen.entry(id).or_insert(0);
        if text.len() <= *emitted {
            return None;
        }
        // Snapshot boundary is always a char boundary (the tail grew).
        let delta = text[*emitted..].to_owned();
        *emitted = text.len();
        match kind {
            "reasoning" => Some(AgentEvent::ReasoningDelta { text: delta }),
            _ => Some(AgentEvent::TextDelta { text: delta }),
        }
    }

    /// A tool part: first snapshot announces the call, a terminal state
    /// resolves it — the same pair the local fold renders as one chip.
    fn emit_tool(&mut self, part: &Value) -> Vec<AgentEvent> {
        let id = part
            .get("callID")
            .or_else(|| part.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let state = part.get("state").cloned().unwrap_or_default();
        let status = state.get("status").and_then(Value::as_str).unwrap_or("");
        let mut events = Vec::new();
        if self.seen_calls.insert(id.clone()) {
            let tool = part.get("tool").and_then(Value::as_str).unwrap_or("tool");
            let input = state.get("input").cloned().unwrap_or_default();
            events.push(AgentEvent::ToolCall {
                id: id.clone(),
                call: oc_tool_call(tool, &input),
            });
        }
        let terminal = matches!(status, "completed" | "error");
        if terminal && self.resolved_calls.insert(id.clone()) {
            let output = state
                .get("output")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    state.get("error").map(|e| {
                        e.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| e.to_string())
                    })
                })
                .or_else(|| {
                    state
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            events.push(AgentEvent::ToolResult {
                id,
                is_error: status == "error",
                output,
                diff: None,
            });
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_lines(feed: &mut RemoteFeed, lines: &[&str]) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        for line in lines {
            let mut chunk = line.as_bytes().to_vec();
            chunk.push(b'\n');
            out.extend(feed.push(&chunk));
        }
        out.extend(feed.finish());
        out
    }

    #[test]
    fn opencode_text_snapshot_dedupes_into_deltas() {
        let mut feed = RemoteFeed {
            partial: Vec::new(),
            kind: FeedKind::Opencode(OpencodeFeed::default()),
        };
        let events = feed_lines(
            &mut feed,
            &[
                r#"{"type":"step_start","part":{"type":"step-start"}}"#,
                r#"{"type":"text","part":{"id":"p1","text":"hel"}}"#,
                r#"{"type":"text","part":{"id":"p1","text":"hello"}}"#,
                r#"{"type":"text","part":{"id":"p1","text":"hello"}}"#,
            ],
        );
        let deltas: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, ["hel", "lo"]);
    }

    #[test]
    fn opencode_tool_emits_call_then_result() {
        let mut feed = RemoteFeed {
            partial: Vec::new(),
            kind: FeedKind::Opencode(OpencodeFeed::default()),
        };
        let events = feed_lines(
            &mut feed,
            &[
                r#"{"type":"tool_use","part":{"id":"p1","callID":"c1","tool":"bash","state":{"status":"running","input":{"command":"ls"}}}}"#,
                r#"{"type":"tool_use","part":{"id":"p1","callID":"c1","tool":"bash","state":{"status":"completed","input":{"command":"ls"},"output":"hi\n"}}}"#,
            ],
        );
        assert!(matches!(
            events[0],
            AgentEvent::ToolCall {
                call: zeron_proto::ToolCall::Exec { .. },
                ..
            }
        ));
        assert!(matches!(
            events[1],
            AgentEvent::ToolResult {
                is_error: false,
                ..
            }
        ));
    }

    #[test]
    fn opencode_error_event_maps_to_error() {
        let mut feed = RemoteFeed {
            partial: Vec::new(),
            kind: FeedKind::Opencode(OpencodeFeed::default()),
        };
        let events = feed_lines(
            &mut feed,
            &[r#"{"type":"error","error":{"message":"boom"}}"#],
        );
        assert!(matches!(
            events[0],
            AgentEvent::Error { ref message } if message == "boom"
        ));
    }

    #[test]
    fn passthrough_lines_become_hard_broken_text() {
        let mut feed = RemoteFeed::for_command("some-cli --flag");
        let events = feed_lines(&mut feed, &["Opening your box", "Railway VM"]);
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Opening your box  \nRailway VM  \n");
    }

    #[test]
    fn claude_stream_json_uses_local_normalizer() {
        let mut feed = RemoteFeed::for_command("claude -p --output-format stream-json");
        let events = feed_lines(
            &mut feed,
            &[
                r#"{"type":"system","subtype":"init","model":"opus","tools":[],"cwd":"/x","session_id":"s1"}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
            ],
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::SessionStarted { .. }))
        );
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolCall {
                call: zeron_proto::ToolCall::Exec { .. },
                ..
            }
        )));
    }

    #[test]
    fn opencode_banner_lines_drop_but_quoted_text_survives() {
        let mut feed = RemoteFeed {
            partial: Vec::new(),
            kind: FeedKind::Opencode(OpencodeFeed::default()),
        };
        let events = feed_lines(&mut feed, &["> build · big-pickle", "> note: quoted"]);
        let text: String = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "> note: quoted  \n");
    }

    #[test]
    fn partial_jsonl_buffers_until_newline() {
        let mut feed = RemoteFeed::for_command("some-cli");
        assert!(feed.push(b"half-li").is_empty());
        let events = feed.push(b"ne\nnext");
        assert_eq!(events.len(), 1);
        assert_eq!(feed.finish().len(), 1);
    }
}
