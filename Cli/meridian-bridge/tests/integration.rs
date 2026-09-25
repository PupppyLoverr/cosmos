use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use meridian_bridge::rpc::Bridge;
use meridian_bridge::vault::Vault;
use meridian_bridge::{boot, tokens};

fn temp_vault(name: &str) -> (Vault, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "cosmos-mv-test-{}-{}",
        name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let vault = Vault::open(&dir).unwrap();
    (vault, dir)
}

#[test]
fn write_then_search_ranks() {
    let (vault, dir) = temp_vault("rank");
    let b = Bridge::open(vault);
    b.memory_write("decision", "the release pipeline uses squash merges", "repo:cosmos", "t/1")
        .unwrap();
    b.memory_write("preference", "dark theme for all dashboards", "repo:cosmos", "t/1")
        .unwrap();
    b.memory_write("map", "the sandbox crate lives in rust-tui", "repo:cosmos", "t/1")
        .unwrap();

    let hits = b.memory_search("squash merges release pipeline", 3, None, &[]);
    assert!(!hits.is_empty());
    assert!(hits[0].text.contains("squash merges"));
    let hits = b.memory_search("dashboard theme", 3, None, &[]);
    assert!(hits[0].text.contains("dark theme"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn seen_ids_dedup() {
    let (vault, dir) = temp_vault("dedup");
    let b = Bridge::open(vault);
    let id = b
        .memory_write("decision", "pick the same fact twice", "repo:x", "t/1")
        .unwrap();
    let first = b.memory_search("same fact twice", 5, None, &[]);
    assert_eq!(first.len(), 1);
    let second = b.memory_search("same fact twice", 5, None, &[id]);
    assert!(second.is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn secret_rejection() {
    let (vault, dir) = temp_vault("secret");
    let b = Bridge::open(vault);
    let err = b
        .memory_write("decision", "api key sk-live-secret-here", "repo:x", "t/1")
        .unwrap_err();
    assert!(err.to_string().contains("rejected"));
    // nothing stored
    let hits = b.memory_search("api key", 5, None, &[]);
    assert!(hits.is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn cache_miss_hit_edit_miss() {
    let (vault, dir) = temp_vault("cache");
    let b = Bridge::open(vault);
    let src = dir.join("sample.rs");
    fs::write(&src, "use std::fs;\n\nfn main() {}\nfn helper() {}\n").unwrap();

    let s1 = b.cache_read(&src, 200).unwrap();
    assert!(s1.contains("language: rust"));
    assert!(s1.contains("fn main"));
    let s2 = b.cache_read(&src, 200).unwrap();
    assert_eq!(s1, s2); // hit

    fs::write(&src, "use std::fs;\nuse std::io;\n\nfn main() {}\n").unwrap();
    let s3 = b.cache_read(&src, 200).unwrap(); // changed hash => miss
    assert!(s3.contains("use std::io;"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn concurrent_process_writes_no_corruption() {
    let (vault, dir) = temp_vault("conc");
    let bin = env!("CARGO_BIN_EXE_meridian");
    let mut children = Vec::new();
    for i in 0..4 {
        let v = vault.root().to_path_buf();
        let child = Command::new(bin)
            .env("MERIDIAN_VAULT", &v)
            .args([
                "write",
                "--type",
                "decision",
                "--scope",
                "repo:conc",
                "--text",
                &format!("concurrent fact number {i}"),
            ])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        children.push(child);
    }
    for mut c in children {
        assert!(c.wait().unwrap().success());
    }
    let b = Bridge::open(vault);
    let hits = b.memory_search("concurrent fact", 10, None, &[]);
    assert_eq!(hits.len(), 4, "all four writes landed intact");
    // Every bullet parses — file is well formed.
    let entries = meridian_bridge::memory::all_entries(b.vault());
    assert_eq!(entries.len(), 4);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn boot_budget_holds() {
    let (vault, dir) = temp_vault("boot");
    let b = Bridge::open(vault);
    let repo = dir.join("repo");
    fs::create_dir_all(&repo).unwrap();
    fs::write(repo.join("AGENTS.md"), "# test repo\n\nSome rules here.\n").unwrap();
    let ctx = b.boot_context(&repo, "zcli").unwrap();
    assert!(ctx.tokens <= boot::BOOT_BUDGET, "tokens={}", ctx.tokens);
    assert!(ctx.text.contains("Cosmos"));
    assert!(ctx.text.contains("Meridian vault"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn boot_trims_huge_agents_md() {
    let (vault, dir) = temp_vault("bootbig");
    let b = Bridge::open(vault);
    let repo = dir.join("repo");
    fs::create_dir_all(&repo).unwrap();
    let huge = "word ".repeat(60_000);
    fs::write(repo.join("AGENTS.md"), format!("# huge\n\n{huge}")).unwrap();
    let ctx = b.boot_context(&repo, "zcli").unwrap();
    assert!(ctx.tokens <= boot::BOOT_BUDGET);
    assert!(ctx.text.contains("truncated"));
    assert!(!ctx.warnings.is_empty());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn serve_stdio_roundtrip() {
    let (vault, dir) = temp_vault("stdio");
    let bin = env!("CARGO_BIN_EXE_meridian");
    let mut child = Command::new(bin)
        .env("MERIDIAN_VAULT", vault.root())
        .args(["serve", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = std::io::BufReader::new(stdout);

    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"memory.write","params":{{"type":"decision","scope":"repo:x","text":"stdio roundtrip works","src":"t/1"}}}}"#
    )
    .unwrap();
    stdin.flush().unwrap();
    use std::io::BufRead;
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["id"], 1);
    assert!(resp["result"]["id"].is_string(), "resp={resp}");

    line.clear();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":2,"method":"memory.search","params":{{"query":"stdio roundtrip","k":3}}}}"#
    )
    .unwrap();
    stdin.flush().unwrap();
    reader.read_line(&mut line).unwrap();
    let resp: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(resp["id"], 2);
    assert!(!resp["result"]["results"].as_array().unwrap().is_empty());
    drop(stdin);
    let _ = child.kill();
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn tokenizer_budget_unit() {
    // sanity on the counting unit used for the budget
    assert_eq!(tokens::count(""), 0);
    assert!(tokens::count("Meridian") >= 1);
}
