//! Session store — append-only JSONL at COSMOS_CLI_HOME/sessions/<id>.jsonl
//! (contract §6), parent-linked for branching, replayable across harnesses.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use zproto::{Block, Event, Message, Role};

use crate::compact::ViewOp;

pub struct Session {
    pub id: String,
    pub file: PathBuf,
    /// id of the last event appended — parent for the next one
    pub leaf: Option<String>,
    pub harness: String,
}

fn sessions_dir(home: &Path) -> PathBuf {
    home.join("sessions")
}

impl Session {
    pub fn create(home: &Path, harness: &str) -> Result<Self> {
        fs::create_dir_all(sessions_dir(home))?;
        let id = zproto::new_id();
        Ok(Session {
            file: sessions_dir(home).join(format!("{id}.jsonl")),
            id,
            leaf: None,
            harness: harness.into(),
        })
    }

    /// Open an existing session (any harness) — leaf = last event id.
    pub fn open(home: &Path, id: &str) -> Result<Self> {
        Self::open_at(home, id, None)
    }

    /// Open a session with its leaf at event `at` (any event, including a
    /// compaction node — a unique id prefix is enough); new events branch
    /// from there. `None` = the last event.
    pub fn open_at(home: &Path, id: &str, at: Option<&str>) -> Result<Self> {
        let file = sessions_dir(home).join(format!("{id}.jsonl"));
        if !file.is_file() {
            bail!("no session {id} (expected {file})", file = file.display());
        }
        let events = read_events(&file)?;
        let leaf = match at {
            // a full id, or a unique suffix as `tree` prints it (ULID prefixes
            // are timestamps and repeat within a millisecond)
            Some(a) => match events.iter().find(|e| e.id == a || (a.len() >= 6 && e.id.ends_with(a))) {
                Some(e) => Some(e.id.clone()),
                None => bail!("session {id} has no event {a}"),
            },
            None => events.last().map(|e| e.id.clone()),
        };
        let harness = events
            .first()
            .map(|e| e.harness.clone())
            .unwrap_or_default();
        Ok(Session {
            id: id.into(),
            file,
            leaf,
            harness,
        })
    }

    /// Append an event; returns it.
    pub fn append(&mut self, kind: &str, data: Value) -> Event {
        let ev = Event::new(&self.id, &self.harness, self.leaf.clone(), kind, data);
        let line = ev.to_line();
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
        {
            let _ = writeln!(f, "{line}");
        }
        self.leaf = Some(ev.id.clone());
        ev
    }

    /// Replay the parent chain from the last event → raw messages.
    pub fn replay(home: &Path, id: &str) -> Result<Vec<Message>> {
        Ok(Self::replay_view(home, id, None)?.0)
    }

    /// Replay the parent chain ending at `leaf` (default: the last event) →
    /// raw messages plus the sticky compaction ops logged along that chain.
    pub fn replay_view(home: &Path, id: &str, leaf: Option<&str>) -> Result<(Vec<Message>, Vec<ViewOp>)> {
        let events = read_events(&sessions_dir(home).join(format!("{id}.jsonl")))?;
        let c = match leaf {
            Some(l) => chain_from(&events, l),
            None => chain(&events),
        };
        Ok((events_to_messages(&c), events_to_ops(&c)))
    }

    /// §6 branch: append a `checkout` event whose parent is `at`, rewinding
    /// the replay chain to that point.
    pub fn branch(&mut self, at: &str) -> Event {
        self.leaf = Some(at.into());
        self.append("checkout", json!({"at": at}))
    }

    pub fn list(home: &Path) -> Vec<Value> {
        let mut out = Vec::new();
        let dir = sessions_dir(home);
        let Ok(rd) = fs::read_dir(&dir) else {
            return out;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "jsonl") != Some(true) {
                continue;
            }
            if let Ok(events) = read_events(&p) {
                if events.is_empty() {
                    continue;
                }
                let first_user = events
                    .iter()
                    .find(|e| e.kind == "user")
                    .and_then(|e| e.data["text"].as_str().map(|s| s.to_string()))
                    .unwrap_or_default();
                let cwd = events
                    .iter()
                    .find(|e| e.kind == "session_start")
                    .and_then(|e| e.data["cwd"].as_str())
                    .unwrap_or_default();
                out.push(json!({
                    "id": p.file_stem().unwrap().to_string_lossy(),
                    "harness": events[0].harness,
                    "started": events[0].ts,
                    "events": events.len(),
                    "preview": first_user.chars().take(80).collect::<String>(),
                    "cwd": cwd,
                }));
            }
        }
        out.sort_by(|a, b| a["started"].as_str().cmp(&b["started"].as_str()));
        out
    }

    pub fn latest_for_cwd(home: &Path, cwd: &Path) -> Option<String> {
        Self::list(home)
            .into_iter()
            .rev()
            .find(|v| v["cwd"].as_str() == cwd.to_str())
            .and_then(|v| v["id"].as_str().map(str::to_owned))
    }

    /// Export as markdown or raw jsonl.
    pub fn export(home: &Path, id: &str, format: &str) -> Result<String> {
        let file = sessions_dir(home).join(format!("{id}.jsonl"));
        let text = fs::read_to_string(&file).with_context(|| format!("session {id}"))?;
        if format == "jsonl" {
            return Ok(text);
        }
        let events = read_events(&file)?;
        let mut md = format!("# session {id}\n\n");
        for e in &events {
            match e.kind.as_str() {
                "user" => md.push_str(&format!(
                    "## user · {}\n\n{}\n\n",
                    e.ts,
                    e.data["text"].as_str().unwrap_or("")
                )),
                "assistant" => {
                    md.push_str(&format!("## assistant · {}\n\n", e.ts));
                    if let Some(t) = e.data["text"].as_str() {
                        md.push_str(t);
                        md.push('\n');
                    }
                    if let Some(calls) = e.data["tool_calls"].as_array() {
                        for c in calls {
                            md.push_str(&format!(
                                "\n- `{}` {}\n",
                                c["name"].as_str().unwrap_or(""),
                                c["input"]
                            ));
                        }
                    }
                    md.push('\n');
                }
                "tool_result" => md.push_str(&format!(
                    "_{}_ → `{}`\n\n",
                    e.data["name"].as_str().unwrap_or("tool"),
                    e.data["content"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(200)
                        .collect::<String>()
                )),
                _ => {}
            }
        }
        Ok(md)
    }
}

pub fn read_events(file: &Path) -> Result<Vec<Event>> {
    let text = fs::read_to_string(file)?;
    Ok(text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

/// Follow `parent` links back from the last-appended event (the leaf) —
/// a `checkout` event rewinds the chain (contract §6).
pub fn chain(events: &[Event]) -> Vec<Event> {
    match events.last() {
        Some(e) => chain_from(events, &e.id),
        None => Vec::new(),
    }
}

/// Parent chain ending at event `leaf`, oldest first.
pub fn chain_from(events: &[Event], leaf: &str) -> Vec<Event> {
    use std::collections::HashMap;
    let by_id: HashMap<&str, &Event> = events.iter().map(|e| (e.id.as_str(), e)).collect();
    let mut out = Vec::new();
    let mut cur = by_id.get(leaf).copied();
    while let Some(e) = cur {
        out.push(e.clone());
        cur = e
            .parent
            .as_ref()
            .and_then(|p| by_id.get(p.as_str()).copied());
    }
    out.reverse();
    out
}

/// Reconstruct provider messages from the event chain (parent-linked from
/// the leaf — a `checkout` rewinds the chain).
pub fn events_to_messages(events: &[Event]) -> Vec<Message> {
    let mut msgs = Vec::new();
    for e in events {
        match e.kind.as_str() {
            "user" => {
                if let Some(t) = e.data["text"].as_str() {
                    msgs.push(Message::text(Role::User, t));
                }
            }
            "assistant" => {
                let mut blocks = Vec::new();
                if let Some(t) = e.data["text"].as_str() {
                    if !t.is_empty() {
                        blocks.push(Block::Text { text: t.into() });
                    }
                }
                if let Some(calls) = e.data["tool_calls"].as_array() {
                    for c in calls {
                        blocks.push(Block::ToolUse {
                            id: c["id"].as_str().unwrap_or("").into(),
                            name: c["name"].as_str().unwrap_or("").into(),
                            input: c["input"].clone(),
                        });
                    }
                }
                if !blocks.is_empty() {
                    msgs.push(Message {
                        role: Role::Assistant,
                        content: blocks,
                    });
                }
            }
            "tool_result" => {
                msgs.push(Message {
                    role: Role::User,
                    content: vec![Block::ToolResult {
                        tool_use_id: e.data["tool_use_id"].as_str().unwrap_or("").into(),
                        content: e.data["content"].as_str().unwrap_or("").into(),
                        is_error: e.data["is_error"].as_bool().unwrap_or(false),
                    }],
                });
            }
            _ => {}
        }
    }
    msgs
}

/// Sticky compaction ops logged along a chain, in order.
pub fn events_to_ops(events: &[Event]) -> Vec<ViewOp> {
    events.iter().filter_map(|e| ViewOp::from_event(&e.kind, &e.data)).collect()
}

/// One line describing an event for `tree`.
fn tree_label(e: &Event) -> String {
    let short = |s: &str| -> String {
        let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if one.chars().count() > 60 {
            format!("{}…", one.chars().take(60).collect::<String>())
        } else {
            one
        }
    };
    let what = match e.kind.as_str() {
        "user" | "assistant" => short(e.data["text"].as_str().unwrap_or("")),
        "tool_call" => short(&format!("{} {}", e.data["name"].as_str().unwrap_or(""), e.data["input"]["path"].as_str().or(e.data["input"]["command"].as_str()).unwrap_or(""))),
        "tool_result" => short(e.data["content"].as_str().unwrap_or("").lines().next().unwrap_or("")),
        "compaction" => format!("{} → {} tokens", e.data["before_tokens"], e.data["after_tokens"]),
        "checkout" => format!("branch from {}", e.data["at"].as_str().map(|a| &a[a.len().saturating_sub(8)..]).unwrap_or("")),
        _ => String::new(),
    };
    let id = &e.id[e.id.len().saturating_sub(8)..];
    format!("{id} {}{}", e.kind, if what.is_empty() { String::new() } else { format!("  {what}") })
}

/// The session as a tree: a straight run of events stays at one depth and
/// only a branch point (a `checkout` that rewinds) indents. Ids are the last
/// eight characters — enough for `--resume <session>@<id>`.
pub fn tree(events: &[Event]) -> Vec<String> {
    use std::collections::HashMap;
    let mut kids: HashMap<Option<&str>, Vec<&Event>> = HashMap::new();
    let known: std::collections::HashSet<&str> = events.iter().map(|e| e.id.as_str()).collect();
    for e in events {
        // a parent outside this file is treated as a root
        let p = e.parent.as_deref().filter(|p| known.contains(p));
        kids.entry(p).or_default().push(e);
    }
    let mut out = Vec::new();
    // iterative walk: (node, indent)
    let mut stack: Vec<(&Event, usize)> = kids.get(&None).map(|r| r.iter().rev().map(|e| (*e, 0)).collect()).unwrap_or_default();
    while let Some((e, depth)) = stack.pop() {
        out.push(format!("{}{}", "  ".repeat(depth), tree_label(e)));
        if let Some(children) = kids.get(&Some(e.id.as_str())) {
            let d = if children.len() > 1 { depth + 1 } else { depth };
            for c in children.iter().rev() {
                stack.push((*c, d));
            }
        }
    }
    out
}
