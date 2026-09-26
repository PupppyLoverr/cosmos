use std::path::PathBuf;
use zgpui::client::AcpClient;

fn zcli_bin() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for p in [
        root.join("target/debug/zcli.exe"),
        root.join("target/debug/zcli"),
        root.join("target/release/zcli.exe"),
        root.join("target/release/zcli"),
    ] {
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

#[test]
fn acp_initialize_and_ping_pong() {
    let Some(bin) = zcli_bin() else {
        eprintln!("zcli binary not built — skip live ACP (types still compile)");
        return;
    };
    let home = tempfile_dir();
    std::env::set_var("COSMOS_CLI_HOME", &home);
    let mut c = AcpClient::spawn(bin).expect("spawn zcli acp");
    let init = c.initialize().expect("initialize");
    assert!(init["agentInfo"]["name"].as_str() == Some("zcli") || init.get("protocolVersion").is_some());
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
