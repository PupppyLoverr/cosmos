//! Session store conformance: append-only JSONL, parent chains, replay,
//! branch/checkout rewind, cross-harness visibility.

use serde_json::json;
use zcore::session::{chain, events_to_messages, read_events, tree, Session};
use zproto::{Block, Role};

#[test]
fn append_only_parent_chain() {
    let home = tempfile::tempdir().unwrap();
    let mut s = Session::create(home.path(), "cosmostui").unwrap();
    let e1 = s.append("session_start", json!({"model": "fixture"}));
    let e2 = s.append("user", json!({"text": "ping"}));
    let e3 = s.append("assistant", json!({"text": "pong"}));
    assert_eq!(e1.parent, None);
    assert_eq!(e2.parent.as_deref(), Some(e1.id.as_str()));
    assert_eq!(e3.parent.as_deref(), Some(e2.id.as_str()));
    let on_disk = read_events(&s.file).unwrap();
    assert_eq!(on_disk.len(), 3);
    assert_eq!(on_disk[0].v, 1);
    assert_eq!(on_disk[0].harness, "cosmostui");
}

#[test]
fn resume_replays_messages() {
    let home = tempfile::tempdir().unwrap();
    let mut s = Session::create(home.path(), "cosmostui").unwrap();
    s.append("session_start", json!({}));
    s.append("user", json!({"text": "ping"}));
    s.append(
        "assistant",
        json!({"text": "pong", "tool_calls": [
        {"id": "t1", "name": "read", "input": {"path": "a.txt"}}]}),
    );
    s.append(
        "tool_result",
        json!({"tool_use_id": "t1", "name": "read",
        "content": "contents", "is_error": false}),
    );
    let id = s.id.clone();

    let reopened = Session::open(home.path(), &id).unwrap();
    assert_eq!(reopened.id, id);
    let msgs = Session::replay(home.path(), &id).unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0].role, Role::User);
    assert_eq!(msgs[1].role, Role::Assistant);
    assert!(matches!(msgs[1].content[1], Block::ToolUse { .. }));
    assert!(matches!(msgs[2].content[0], Block::ToolResult { .. }));
}

#[test]
fn checkout_rewinds_chain() {
    let home = tempfile::tempdir().unwrap();
    let mut s = Session::create(home.path(), "cosmostui").unwrap();
    s.append("session_start", json!({}));
    let u1 = s.append("user", json!({"text": "first"}));
    s.append("assistant", json!({"text": "a1"}));
    s.append("user", json!({"text": "second"}));
    s.append("assistant", json!({"text": "a2"}));
    // branch back to just after u1
    let id = s.id.clone();
    let mut s2 = Session::open(home.path(), &id).unwrap();
    s2.branch(&u1.id);

    let events = read_events(&s2.file).unwrap();
    let c = chain(&events);
    let msgs = events_to_messages(&c);
    // chain should end at u1's era: session_start, u1 — the a2 turn is cut off
    assert_eq!(msgs.len(), 1);
    assert!(matches!(&msgs[0].content[0], Block::Text { text } if text == "first"));
    // then continuing appends after checkout keep the branch
    s2.append("assistant", json!({"text": "branched"}));
    let msgs2 = Session::replay(home.path(), &id).unwrap();
    assert_eq!(msgs2.len(), 2);
}

#[test]
fn tree_indents_only_at_branch_points() {
    let home = tempfile::tempdir().unwrap();
    let mut s = Session::create(home.path(), "cosmostui").unwrap();
    s.append("session_start", json!({}));
    let u1 = s.append("user", json!({"text": "first"}));
    s.append("assistant", json!({"text": "a1"}));
    s.append("user", json!({"text": "second"}));
    let a2 = s.append("assistant", json!({"text": "a2"}));
    // straight run, no branch: every line at depth 0
    let flat = tree(&read_events(&s.file).unwrap());
    assert_eq!(flat.len(), 5);
    assert!(flat.iter().all(|l| !l.starts_with(' ')), "{flat:?}");
    assert!(flat[4].starts_with(&a2.id[a2.id.len() - 8..]), "{flat:?}");

    // a checkout rewinds to u1 → u1 now has two children, so everything below
    // it indents one level on both timelines
    let mut s2 = Session::open(home.path(), &s.id).unwrap();
    s2.branch(&u1.id);
    s2.append("assistant", json!({"text": "branched"}));
    let lines = tree(&read_events(&s2.file).unwrap());
    assert_eq!(lines.len(), 7);
    assert!(!lines[0].starts_with(' ') && !lines[1].starts_with(' '), "{lines:?}");
    assert!(lines[2..].iter().all(|l| l.starts_with("  ") && !l.starts_with("   ")), "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("checkout") && l.contains(&u1.id[u1.id.len() - 8..])), "{lines:?}");
    // tree ids are the unique suffixes --resume <session>@<id> accepts
    let reopened = Session::open_at(home.path(), &s.id, Some(&a2.id[a2.id.len() - 8..])).unwrap();
    assert_eq!(reopened.leaf.as_deref(), Some(a2.id.as_str()));
}

#[test]
fn cross_harness_visibility() {
    let home = tempfile::tempdir().unwrap();
    let mut s = Session::create(home.path(), "cosmostui").unwrap();
    s.append("session_start", json!({"model": "fixture"}));
    s.append("user", json!({"text": "remember this"}));
    let id = s.id.clone();
    // zdev opens the same file — that's the handoff
    let s2 = Session::open(home.path(), &id).unwrap();
    assert_eq!(s2.harness, "cosmostui"); // original harness recorded
    let list = Session::list(home.path());
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], json!(id));
    let md = Session::export(home.path(), &id, "md").unwrap();
    assert!(md.contains("remember this"));
    let raw = Session::export(home.path(), &id, "jsonl").unwrap();
    assert!(raw.contains("\"kind\":\"user\""));
}
