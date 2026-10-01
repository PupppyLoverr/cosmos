//! Deferred tools end to end: the model reaches `memory_write` through
//! `mcp_call {server:"cosmos"}`, the permission check applies to the inner
//! tool, the bullet lands in local memory, and the next session's boot
//! context carries it. No meridian process anywhere.

use zproto::RunOpts;

#[test]
fn memory_write_via_self_server_reaches_next_boot() {
    let home = tempfile::tempdir().unwrap();
    let ws = home.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    std::env::set_var("COSMOS_CLI_HOME", home.path());
    std::env::remove_var("COSMOS_MEMORY_DIR");
    std::env::remove_var("MERIDIAN_VAULT");
    let call = r#"tool:mcp_call {"server":"cosmos","tool":"memory_write","arguments":{"type":"preference","text":"always run cargo fmt before committing","scope":"global"}}"#;
    let run = |allow: &[&str]| {
        let opts = RunOpts {
            prompt: Some(call.into()),
            model: Some("fixture".into()),
            cwd: Some(ws.to_string_lossy().into()),
            allow: allow.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let mut events = Vec::new();
        let code = zcore::agent::run_headless(&opts, "cosmostui", &ws, &mut |e: &zproto::Event| {
            events.push((e.kind.clone(), e.data.clone()))
        })
        .unwrap();
        (code, events)
    };

    // allowing only the wrapper does not allow the wrapped tool
    let (code, events) = run(&["mcp_call"]);
    assert_eq!(code, zproto::exit::DENIED, "{events:?}");
    assert!(ztools::memory::all(home.path()).is_empty());

    let (code, events) = run(&["memory_write"]);
    assert_eq!(code, zproto::exit::DONE, "{events:?}");
    let saved = ztools::memory::all(home.path());
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].text, "always run cargo fmt before committing");

    let b = zcore::boot::build(&ws, home.path());
    assert!(b.text.contains("- [preference] always run cargo fmt before committing"), "{}", b.text);
}
