//! On-disk session-store scanning for the `/resume` picker.
//!
//! Each harness CLI persists resumable sessions differently: claude and codex
//! keep one JSONL transcript per session, grok keeps a `summary.json` +
//! `chat_history.jsonl` pair per id, cursor's acp sessions carry a `meta.json`
//! stub, and antigravity keeps a sqlite summaries db. These helpers walk the
//! newest slice of each store and normalize everything into
//! [`zeron_proto::HarnessSession`] rows (+ flat transcript rows where the
//! format is cheaply readable).
//!
//! Everything here is best-effort by construction: a malformed line or entry
//! is skipped, never fatal — a half-written session file must not blank the
//! picker.

use std::path::{Path, PathBuf};

use zeron_proto::{HarnessSession, SessionTranscriptRow};

/// How many newest sessions one scan returns — enough for the picker, cheap
/// enough to re-run per `/resume` open.
pub(crate) const SESSION_SCAN_LIMIT: usize = 100;

/// Directory entries sorted newest-first by mtime (files or dirs — whatever
/// the caller globbed), capped at `limit`.
pub(crate) fn newest_paths(mut paths: Vec<PathBuf>, limit: usize) -> Vec<PathBuf> {
    let mtime = |p: &PathBuf| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    };
    paths.sort_by_key(mtime);
    paths.reverse();
    paths.truncate(limit);
    paths
}

/// Recursively collect `*.jsonl` files under `root`, `depth` levels deep.
/// Session stores nest by date (codex `YYYY/MM/DD`) — shallow-bounded so a
/// huge tree stays cheap.
pub(crate) fn collect_jsonl(root: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_jsonl_into(root, depth, &mut out);
    out
}

fn collect_jsonl_into(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_jsonl_into(&path, depth - 1, out);
            }
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
}

/// Read the first `max_lines` of a jsonl file as parsed values, skipping
/// malformed lines.
pub(crate) fn read_jsonl_head(path: &Path, max_lines: usize) -> Vec<serde_json::Value> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .take(max_lines)
        .collect()
}

/// Read a whole jsonl file as parsed values (transcript import — bounded by
/// [`TRANSCRIPT_ROW_LIMIT`] rows out, not by bytes in).
pub(crate) fn read_jsonl(path: &Path) -> Vec<serde_json::Value> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Transcript imports cap at this many rows — the tail of a long session is
/// what matters for context; an unbounded import would bloat the doc.
pub(crate) const TRANSCRIPT_ROW_LIMIT: usize = 400;

/// One-line preview text: collapse whitespace, cap the length.
pub(crate) fn preview_line(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = flat.chars().take(max).collect::<String>();
    if flat.chars().count() > max {
        out.push('…');
    }
    out
}

/// RFC3339 / `YYYY-MM-DDTHH:MM:SS(.f)Z` timestamp → epoch ms. Session stores
/// all stamp ISO-8601-ish strings; a parse failure yields `None`, never a
/// guessed clock value.
pub(crate) fn iso_to_epoch_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp_millis())
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|dt| dt.and_utc().timestamp_millis())
        })
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|dt| dt.and_utc().timestamp_millis())
        })
}

/// File mtime → epoch ms (for stores without an internal timestamp).
pub(crate) fn file_mtime_ms(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_millis() as i64)
        })
}

/// Pull the text out of a claude-style `message.content` field — either a
/// plain string or the `[{type:"text",text}]` block array.
pub(crate) fn content_text(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    content
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Assemble the picker row.
pub(crate) fn session(
    id: impl Into<String>,
    title: impl Into<String>,
    preview: impl Into<String>,
    cwd: impl Into<String>,
    updated_at: Option<i64>,
) -> HarnessSession {
    HarnessSession {
        id: id.into(),
        title: title.into(),
        preview: preview.into(),
        cwd: cwd.into(),
        updated_at,
    }
}

/// Claude: `~/.claude/projects/<cwd-slug>/<sessionId>.jsonl` — one file per
/// session; `cwd`/`sessionId` ride every line; the first real user text makes
/// the title.
pub(crate) fn scan_claude(root: &Path) -> Vec<HarnessSession> {
    let mut files = collect_jsonl(root, 1);
    files = newest_paths(files, SESSION_SCAN_LIMIT);
    let mut out = Vec::new();
    for path in files {
        let head = read_jsonl_head(&path, 200);
        let mut session_id = String::new();
        let mut cwd = String::new();
        let mut title = String::new();
        let mut preview = String::new();
        let mut first_ts: Option<i64> = None;
        for line in &head {
            if session_id.is_empty() {
                if let Some(id) = line.get("sessionId").and_then(|v| v.as_str()) {
                    session_id = id.to_string();
                }
            }
            if cwd.is_empty() {
                if let Some(dir) = line.get("cwd").and_then(|v| v.as_str()) {
                    cwd = dir.to_string();
                }
            }
            if first_ts.is_none() {
                first_ts = line
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .and_then(iso_to_epoch_ms);
            }
            if title.is_empty() && line.get("type").and_then(|v| v.as_str()) == Some("user") {
                let text = line
                    .get("message")
                    .map(|m| content_text(&m["content"]))
                    .unwrap_or_default();
                // Local-command caveats and command echoes aren't titles.
                let clean = text.trim();
                if !clean.is_empty() && !clean.starts_with('<') {
                    title = preview_line(clean, 60);
                    preview = preview_line(clean, 140);
                }
            }
        }
        if session_id.is_empty() {
            // The filename is the id even when head lines carry no field.
            session_id = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        if session_id.is_empty() {
            continue;
        }
        out.push(session(
            session_id,
            if title.is_empty() {
                "Claude session".to_string()
            } else {
                title
            },
            preview,
            cwd,
            file_mtime_ms(&path).or(first_ts),
        ));
    }
    out
}

/// Claude transcript: user/assistant text rows, skipping command echoes and
/// tool blocks (tools render as parts; a flat text import keeps them out).
pub(crate) fn claude_transcript(path: &Path) -> Vec<SessionTranscriptRow> {
    let mut rows = Vec::new();
    for line in read_jsonl(path) {
        let role = match line.get("type").and_then(|v| v.as_str()) {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => continue,
        };
        // Sidechain (subagent) frames and meta rows aren't the conversation.
        if line.get("isSidechain").and_then(|v| v.as_bool()) == Some(true)
            || line.get("isMeta").and_then(|v| v.as_bool()) == Some(true)
        {
            continue;
        }
        let text = line
            .get("message")
            .map(|m| content_text(&m["content"]))
            .unwrap_or_default();
        let text = text.trim();
        if text.is_empty() || text.starts_with("<local-command") || text.starts_with("<command-") {
            continue;
        }
        rows.push(SessionTranscriptRow {
            role: role.to_string(),
            text: text.to_string(),
            created_at: line
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(iso_to_epoch_ms)
                .unwrap_or_default(),
        });
        if rows.len() >= TRANSCRIPT_ROW_LIMIT {
            break;
        }
    }
    rows
}

/// Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` — first line is
/// `session_meta` (id, cwd, timestamp); the first user `response_item`
/// supplies the title.
pub(crate) fn scan_codex(root: &Path) -> Vec<HarnessSession> {
    let mut files = collect_jsonl(root, 4);
    files = newest_paths(files, SESSION_SCAN_LIMIT);
    let mut out = Vec::new();
    for path in files {
        let head = read_jsonl_head(&path, 300);
        let Some(meta) = head
            .iter()
            .find(|l| l.get("type").and_then(|v| v.as_str()) == Some("session_meta"))
            .and_then(|l| l.get("payload"))
        else {
            continue;
        };
        let session_id = meta
            .get("id")
            .or_else(|| meta.get("session_id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        if session_id.is_empty() {
            continue;
        }
        let cwd = meta
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let ts = meta
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(iso_to_epoch_ms)
            .or_else(|| file_mtime_ms(&path));
        // First user message = title. Codex rows: `response_item` payloads
        // shaped {type:"message", role, content:[{type, text}]}.
        let mut title = String::new();
        let mut preview = String::new();
        for line in &head {
            let payload = line.get("payload").cloned().unwrap_or_default();
            if payload.get("type").and_then(|v| v.as_str()) != Some("message")
                || payload.get("role").and_then(|v| v.as_str()) != Some("user")
            {
                continue;
            }
            let text = payload
                .get("content")
                .map(|c| {
                    c.as_array()
                        .map(|blocks| {
                            blocks
                                .iter()
                                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            let clean = text.trim();
            if !clean.is_empty() && !clean.starts_with('<') {
                title = preview_line(clean, 60);
                preview = preview_line(clean, 140);
                break;
            }
        }
        out.push(session(
            session_id,
            if title.is_empty() {
                "Codex session".to_string()
            } else {
                title
            },
            preview,
            cwd,
            ts,
        ));
    }
    out
}

/// Codex transcript: user/assistant `response_item` message text rows.
pub(crate) fn codex_transcript(path: &Path) -> Vec<SessionTranscriptRow> {
    let mut rows = Vec::new();
    for line in read_jsonl(path) {
        if line.get("type").and_then(|v| v.as_str()) != Some("response_item") {
            continue;
        }
        let payload = &line["payload"];
        if payload.get("type").and_then(|v| v.as_str()) != Some("message") {
            continue;
        }
        let role = match payload.get("role").and_then(|v| v.as_str()) {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => continue,
        };
        let text = payload
            .get("content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let text = text.trim();
        // Environment/developer wrappers aren't conversation.
        if text.is_empty() || text.starts_with('<') {
            continue;
        }
        rows.push(SessionTranscriptRow {
            role: role.to_string(),
            text: text.to_string(),
            created_at: line
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(iso_to_epoch_ms)
                .unwrap_or_default(),
        });
        if rows.len() >= TRANSCRIPT_ROW_LIMIT {
            break;
        }
    }
    rows
}

/// Find the session file a transcript id names, under `root` (a `*.jsonl`
/// basename match over the scanned set — the same bounded walk the listing
/// used, so an attacker-crafted id can't escape the store root).
pub(crate) fn find_session_file(root: &Path, session_id: &str, depth: usize) -> Option<PathBuf> {
    let wanted = format!("{session_id}.jsonl");
    collect_jsonl(root, depth)
        .into_iter()
        .find(|p| p.file_name().is_some_and(|n| n == wanted.as_str()))
        .or_else(|| {
            // Codex names files `rollout-<ts>-<id>.jsonl` — suffix match.
            collect_jsonl(root, depth).into_iter().find(|p| {
                p.file_stem()
                    .is_some_and(|s| s.to_string_lossy().ends_with(session_id))
            })
        })
}

/// Grok: `~/.grok/sessions/<urlenc-cwd>/<id>/summary.json` — id, cwd, title
/// and timestamps all in one small file.
pub(crate) fn scan_grok(root: &Path) -> Vec<HarnessSession> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for cwd_dir in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let cwd_path = cwd_dir.path();
        if !cwd_path.is_dir() {
            continue;
        }
        for session_dir in std::fs::read_dir(&cwd_path).into_iter().flatten().flatten() {
            let path = session_dir.path();
            if path.join("summary.json").is_file() {
                dirs.push(path);
            }
        }
    }
    dirs = newest_paths(dirs, SESSION_SCAN_LIMIT);
    let mut out = Vec::new();
    for dir in dirs {
        let summary_path = dir.join("summary.json");
        let Ok(raw) = std::fs::read_to_string(&summary_path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let id = value["info"]["id"].as_str().unwrap_or_default().to_string();
        if id.is_empty() {
            continue;
        }
        let cwd = value["info"]["cwd"]
            .as_str()
            .unwrap_or_default()
            .trim_start_matches("\\\\?\\")
            .to_string();
        let title = value["session_summary"].as_str().unwrap_or_default();
        let updated = value["updated_at"]
            .as_str()
            .and_then(iso_to_epoch_ms)
            .or_else(|| file_mtime_ms(&summary_path));
        out.push(session(
            id,
            if title.is_empty() {
                "Grok session".to_string()
            } else {
                preview_line(title, 60)
            },
            preview_line(title, 140),
            cwd,
            updated,
        ));
    }
    out
}

/// Grok transcript: `~/.grok/sessions/<urlenc-cwd>/<id>/chat_history.jsonl` —
/// `{"type": "user"|"assistant"|"system", "content": [{type:"text",text}]}`
/// rows. System rows and `<system-reminder>`/`<user_info>` wrappers are
/// scaffolding, not conversation.
pub(crate) fn grok_transcript(path: &Path) -> Vec<SessionTranscriptRow> {
    let mut rows = Vec::new();
    for line in read_jsonl(path) {
        let role = match line.get("type").and_then(|v| v.as_str()) {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => continue,
        };
        let text = content_text(&line["content"]);
        let text = text.trim();
        if text.is_empty()
            || text.starts_with("<system-reminder")
            || text.starts_with("<user_info")
            || text.starts_with("<action_safety")
            || text.starts_with("<work_policy")
        {
            continue;
        }
        rows.push(SessionTranscriptRow {
            role: role.to_string(),
            text: text.to_string(),
            created_at: 0,
        });
        if rows.len() >= TRANSCRIPT_ROW_LIMIT {
            break;
        }
    }
    rows
}

/// The grok session dir holding `session_id`'s history file.
pub(crate) fn find_grok_session(root: &Path, session_id: &str) -> Option<PathBuf> {
    for cwd_dir in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let dir = cwd_dir.path().join(session_id);
        if dir.join("chat_history.jsonl").is_file() {
            return Some(dir);
        }
    }
    None
}

/// Cursor: `~/.cursor/acp-sessions/<uuid>/meta.json` — just `{cwd}`; the
/// directory name is the session id and the dir mtime the activity stamp.
pub(crate) fn scan_cursor(root: &Path) -> Vec<HarnessSession> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("meta.json").is_file())
        .collect();
    dirs = newest_paths(dirs, SESSION_SCAN_LIMIT);
    let mut out = Vec::new();
    for dir in dirs {
        let meta_path = dir.join("meta.json");
        let Ok(raw) = std::fs::read_to_string(&meta_path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let id = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let cwd = value["cwd"].as_str().unwrap_or_default().to_string();
        out.push(session(
            id,
            "Cursor session",
            "",
            cwd,
            file_mtime_ms(&dir),
        ));
    }
    out
}

/// Antigravity: `~/.gemini/antigravity-cli/conversation_summaries.db` — one
/// row per conversation with title, preview, workspace URIs and timestamps.
pub(crate) fn scan_antigravity(db_path: &Path) -> Vec<HarnessSession> {
    if !db_path.is_file() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return Vec::new();
    };
    let mut stmt = match conn.prepare(
        "SELECT conversation_id, title, preview, workspace_uris, last_modified_time \
         FROM conversation_summaries ORDER BY last_modified_time DESC LIMIT ?1",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    let rows = stmt.query_map([SESSION_SCAN_LIMIT as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    });
    let Ok(rows) = rows else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for row in rows.flatten() {
        let (id, title, preview, uris, modified) = row;
        // workspace_uris is a JSON array of file:// URIs; the first is cwd.
        let cwd = serde_json::from_str::<Vec<String>>(&uris)
            .ok()
            .and_then(|v| v.into_iter().next())
            .map(|uri| {
                uri.trim_start_matches("file:///")
                    .trim_start_matches("file://")
                    .replace('/', "\\")
            })
            .unwrap_or_default();
        out.push(session(
            id,
            if title.is_empty() {
                "Antigravity conversation".to_string()
            } else {
                title
            },
            preview_line(&preview, 140),
            cwd,
            iso_to_epoch_ms(&modified),
        ));
    }
    out
}
