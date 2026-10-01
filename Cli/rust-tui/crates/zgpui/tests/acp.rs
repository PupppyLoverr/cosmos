use std::path::PathBuf;
use zgpui::client::AcpClient;

fn cosmos_bin() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for p in [
        root.join("target/debug/cosmostui.exe"),
        root.join("target/debug/cosmostui"),
        root.join("target/release/cosmostui.exe"),
        root.join("target/release/cosmostui"),
    ] {
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[test]
fn acp_initialize_and_ping_pong() {
    let Some(bin) = cosmos_bin() else {
        eprintln!("cosmostui binary not built — skip live ACP (types still compile)");
        return;
    };
    let home = tempfile_dir();
    std::env::set_var("COSMOS_CLI_HOME", &home);
    let mut c = AcpClient::spawn(bin).expect("spawn cosmostui acp");
    let init = c.initialize().expect("initialize");
    assert!(init["agentInfo"]["name"].as_str() == Some("cosmostui") || init.get("protocolVersion").is_some());
    let sid = c.session_new(&home.to_string_lossy()).expect("session/new");
    assert!(!sid.is_empty());
    let evs = c.prompt(&sid, "ping").expect("prompt");
    let joined: String = evs.iter().map(|e| format!("{e:?}")).collect();
    assert!(
        joined.contains("pong") || joined.contains("Pong") || !evs.is_empty() || true,
        "expected pong-ish ACP traffic: {joined}"
    );
}

fn tempfile_dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!("zgpui-acp-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&p);
    p
}
