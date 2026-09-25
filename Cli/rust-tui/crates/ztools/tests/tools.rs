//! Contract §2 tool tests: hashlines, stale-edit rejection, atomic write
//! guard, workspace escape, result caps, grep/gitignore.

use serde_json::json;
use ztools::{cap_result, run, ToolContext, RESULT_CAP};

fn ctx() -> (tempfile::TempDir, ToolContext) {
    let dir = tempfile::tempdir().unwrap();
    let c = ToolContext::new(dir.path());
    (dir, c)
}

#[test]
fn read_emits_hashline_anchors() {
    let (dir, mut c) = ctx();
    std::fs::write(dir.path().join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let out = run(&mut c, "read", &json!({"path": "a.txt"}));
    assert!(out.contains("file_sha256="), "missing sha header: {out}");
    assert!(out.contains("lines=3"), "missing line count: {out}");
    // anchor format N:hhhhhh|text
    assert!(out.contains("1:"), "missing anchor: {out}");
    assert!(out.contains("|alpha"), "missing anchored line: {out}");
}

#[test]
fn write_then_read_then_edit_roundtrip() {
    let (dir, mut c) = ctx();
    run(
        &mut c,
        "write",
        &json!({"path": "b.txt", "content": "one\ntwo\nthree\n"}),
    );
    let r = run(&mut c, "read", &json!({"path": "b.txt"}));
    // extract the anchor for line 2
    let line2 = r.lines().find(|l| l.contains("|two")).unwrap();
    let anchor = line2.split('|').next().unwrap();
    let e = run(
        &mut c,
        "edit",
        &json!({"path": "b.txt",
                "edits": [{"start": anchor, "end": anchor, "new_text": "TWO"}]}),
    );
    assert!(e.starts_with("ok"), "edit failed: {e}");
    let body = std::fs::read_to_string(dir.path().join("b.txt")).unwrap();
    assert_eq!(body, "one\nTWO\nthree\n");
}

#[test]
fn edit_rejects_stale_hash_and_bad_anchor() {
    let (dir, mut c) = ctx();
    std::fs::write(dir.path().join("c.txt"), "x\ny\n").unwrap();
    let e = run(
        &mut c,
        "edit",
        &json!({"path": "c.txt", "file_sha256": "deadbeef",
                "edits": [{"start": "1:aaaaaa", "end": "1:aaaaaa", "new_text": "z"}]}),
    );
    assert!(e.contains("STALE"), "expected STALE: {e}");
    // now read to get real anchors, then send a wrong anchor hash
    run(&mut c, "read", &json!({"path": "c.txt"}));
    let e2 = run(
        &mut c,
        "edit",
        &json!({"path": "c.txt",
                "edits": [{"start": "1:000000", "end": "1:000000", "new_text": "z"}]}),
    );
    assert!(e2.contains("STALE"), "expected anchor STALE: {e2}");
    assert!(
        e2.contains("anchor mismatch"),
        "expected fresh anchors: {e2}"
    );
    // file must be untouched (atomic rejection)
    assert_eq!(
        std::fs::read_to_string(dir.path().join("c.txt")).unwrap(),
        "x\ny\n"
    );
}

#[test]
fn write_requires_prior_read_state() {
    let (dir, mut c) = ctx();
    std::fs::write(dir.path().join("d.txt"), "existing").unwrap();
    let e = run(
        &mut c,
        "write",
        &json!({"path": "d.txt", "content": "clobber"}),
    );
    assert!(e.contains("STALE"), "expected STALE: {e}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("d.txt")).unwrap(),
        "existing"
    );
    // after read, overwrite works
    run(&mut c, "read", &json!({"path": "d.txt"}));
    let e2 = run(
        &mut c,
        "write",
        &json!({"path": "d.txt", "content": "clobber"}),
    );
    assert!(e2.starts_with("ok"), "{e2}");
}

#[test]
fn workspace_escape_denied() {
    let (_dir, mut c) = ctx();
    for tool_input in [
        ("read", json!({"path": "../outside.txt"})),
        ("write", json!({"path": "../outside.txt", "content": "x"})),
        ("edit", json!({"path": "..\\evil.txt", "edits": []})),
        ("ls", json!({"path": ".."})),
    ] {
        let out = run(&mut c, tool_input.0, &tool_input.1);
        assert!(
            out.contains("escapes workspace"),
            "{:?} did not deny escape: {out}",
            tool_input.0
        );
    }
}

#[test]
fn large_read_guard_and_range_read() {
    let (dir, mut c) = ctx();
    let big: String = (0..300).map(|i| format!("line{i}\n")).collect();
    std::fs::write(dir.path().join("big.txt"), big).unwrap();
    let e = run(&mut c, "read", &json!({"path": "big.txt"}));
    assert!(e.contains("large_read_guard"), "expected guard: {e}");
    let ok = run(
        &mut c,
        "read",
        &json!({"path": "big.txt", "start": 1, "end": 10}),
    );
    assert!(ok.contains("lines=300") && ok.contains("|line9"), "{ok}");
}

#[test]
fn result_cap() {
    let big = "x".repeat(RESULT_CAP + 100);
    let capped = cap_result(big);
    assert!(capped.len() < RESULT_CAP + 200);
    assert!(capped.contains("[truncated:"));
}

#[test]
fn grep_caps_200_matches() {
    let (dir, mut c) = ctx();
    let big: String = (0..250).map(|i| format!("hit{i}\n")).collect();
    std::fs::write(dir.path().join("hits.txt"), big).unwrap();
    let out = run(&mut c, "grep", &json!({"pattern": "hit"}));
    assert!(out.contains("[cap: 200 matches]"), "{out}");
}

#[test]
fn unknown_tool_is_error() {
    let (_d, mut c) = ctx();
    let out = run(&mut c, "nonsense", &json!({}));
    assert!(out.starts_with("ERROR"), "{out}");
}
