//! Provider tests: §12 host allowlist, fixture provider, tool-fragment
//! accumulator, SSE parsing of OpenAI chunks.

use serde_json::json;
use zproto::{Chunk, Message, Role};
use zproviders::{complete, host_allowed, ProviderCfg, ProviderKind, Request, ToolAcc};

fn fixture() -> ProviderCfg {
    ProviderCfg {
        kind: ProviderKind::Fixture,
        base_url: String::new(),
        api_key_env: String::new(),
        model: "fixture".into(),
        context: 128_000,
        prices: zproto::Prices::default(),
        effort: None,
        api_key: None,
        account_id: None,
    }
}

#[test]
fn allowlist_rejects_foreign_hosts() {
    let allowed = vec!["api.openai.com".to_string()];
    assert!(host_allowed("https://api.openai.com/v1/chat", &allowed).is_ok());
    assert!(host_allowed("https://sub.api.openai.com/x", &allowed).is_ok());
    assert!(host_allowed("https://evil.com/x", &allowed).is_err());
    assert!(host_allowed("https://api.openai.com.evil.com/x", &allowed).is_err());
    assert!(host_allowed("https://notapi.openai.com/x", &allowed).is_err());
}

#[test]
fn fixture_ping_pong() {
    let cfg = fixture();
    let msgs = vec![Message::text(Role::User, "ping")];
    let req = Request {
        system: vec!["sys".into()],
        tools: &[],
        messages: &msgs,
        session_id: "s1",
        max_tokens: 128,
    };
    let mut text = String::new();
    let mut usage = zproto::Usage::default();
    complete(
        &cfg,
        &req,
        &mut |c| match c {
            Chunk::Text(t) => text.push_str(&t),
            Chunk::Done(u) => {
                usage.input += u.input;
                usage.output += u.output;
            }
            _ => {}
        },
        &[],
    )
    .unwrap();
    assert_eq!(text, "pong");
    assert!(usage.input > 0);
    assert!(usage.output > 0);
}

#[test]
fn fixture_tool_call() {
    let cfg = fixture();
    let msgs = vec![Message::text(Role::User, r#"tool:read {"path":"x.txt"}"#)];
    let req = Request {
        system: vec![],
        tools: &[],
        messages: &msgs,
        session_id: "s1",
        max_tokens: 128,
    };
    let mut got: Option<(String, String)> = None;
    complete(
        &cfg,
        &req,
        &mut |c| {
            if let Chunk::ToolUse { name, input, .. } = c {
                got = Some((name, input["path"].as_str().unwrap_or("").to_string()));
            }
        },
        &[],
    )
    .unwrap();
    assert_eq!(got, Some(("read".into(), "x.txt".into())));
}

#[test]
fn tool_fragment_accumulator() {
    let mut acc = ToolAcc::new();
    // streamed openai fragments: id+name on first, args split across two
    acc.feed(&json!({"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"pa"}}));
    acc.feed(&json!({"index":0,"function":{"arguments":"th\":\"x\"}"}}));
    let blocks = acc.finish();
    assert_eq!(blocks.len(), 1);
    match &blocks[0] {
        zproto::Block::ToolUse { id, name, input } => {
            assert_eq!(id, "call_1");
            assert_eq!(name, "read");
            assert_eq!(input["path"], json!("x"));
        }
        _ => panic!("expected ToolUse"),
    }
}

/// Serve one HTTP request with `body` as an SSE stream; returns (port, handle
/// yielding the raw request text).
fn mock_sse(extra_headers: &'static str, body: &'static str) -> (u16, std::thread::JoinHandle<String>) {
    mock("200 OK", "text/event-stream", extra_headers, body)
}

fn mock(status: &'static str, ctype: &'static str, extra_headers: &'static str, body: &'static str) -> (u16, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let h = std::thread::spawn(move || {
        let (mut s, _) = l.accept().unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_millis(500))).unwrap();
        let mut req = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            match s.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    req.extend_from_slice(&buf[..n]);
                    let t = String::from_utf8_lossy(&req);
                    if let Some(i) = t.find("\r\n\r\n") {
                        let len = t[..i].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if req.len() >= i + 4 + len {
                            break;
                        }
                    }
                }
            }
        }
        let _ = write!(s, "HTTP/1.1 {status}\r\ncontent-type: {ctype}\r\n{extra_headers}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
        String::from_utf8_lossy(&req).to_string()
    });
    (port, h)
}

#[test]
fn chatgpt_responses_stream_round_trip() {
    let sse = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"bash\",\"arguments\":\"{\\\"command\\\":\\\"ls\\\"}\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":100,\"output_tokens\":7,\"input_tokens_details\":{\"cached_tokens\":40}}}}\n\n",
    );
    let (port, h) = mock_sse("x-codex-primary-used-percent: 16\r\nx-codex-primary-reset-after-seconds: 3600\r\nx-codex-plan-type: plus\r\n", sse);
    let cfg = ProviderCfg {
        kind: ProviderKind::ChatGpt,
        base_url: format!("http://127.0.0.1:{port}/backend-api"),
        model: "gpt-5.5".into(),
        api_key: Some("tok".into()),
        account_id: Some("acc_9".into()),
        effort: Some("high".into()),
        ..Default::default()
    };
    let msgs = vec![Message::text(Role::User, "hi")];
    let tools = vec![zproto::ToolSpec { name: "bash".into(), description: "run".into(), input_schema: serde_json::json!({"type":"object"}) }];
    let req = Request { system: vec!["sys A".into(), "sys B".into()], tools: &tools, messages: &msgs, session_id: "s1", max_tokens: 1024 };
    let (mut text, mut calls, mut usage) = (String::new(), Vec::new(), zproto::Usage::default());
    complete(&cfg, &req, &mut |c| match c {
        Chunk::Text(t) => text.push_str(&t),
        Chunk::ToolUse { id, name, input } => calls.push((id, name, input)),
        Chunk::Done(u) => usage = u,
        Chunk::Err(e) => panic!("{e}"),
    }, &["127.0.0.1".into()]).unwrap();
    let raw = h.join().unwrap();
    assert_eq!(text, "Hello");
    assert_eq!(calls, vec![("call_1".to_string(), "bash".to_string(), serde_json::json!({"command": "ls"}))]);
    assert_eq!((usage.input, usage.cache_read, usage.output), (60, 40, 7));
    assert!(raw.starts_with("POST /backend-api/codex/responses "), "{raw}");
    let lower = raw.to_ascii_lowercase();
    assert!(lower.contains("chatgpt-account-id: acc_9"));
    assert!(lower.contains("openai-beta: responses=experimental"));
    assert!(lower.contains("authorization: bearer tok"));
    let body: serde_json::Value = serde_json::from_str(&raw[raw.find("\r\n\r\n").unwrap() + 4..]).unwrap();
    assert_eq!(body["instructions"], "sys A\n\nsys B");
    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(body["reasoning"]["effort"], "high");
    assert_eq!(body["store"], false);
    let rl = zproviders::last_rate_limits().unwrap();
    assert_eq!(rl.primary_used, Some(16.0));
    assert_eq!(rl.plan.as_deref(), Some("plus"));
}

#[test]
fn provider_errors_are_readable() {
    let (port, _h) = mock("401 Unauthorized", "application/json", "", r#"{"error":{"message":"Incorrect API key provided"}}"#);
    let cfg = ProviderCfg { kind: ProviderKind::OpenAI, base_url: format!("http://127.0.0.1:{port}/v1"), model: "m".into(), api_key: Some("bad".into()), ..Default::default() };
    let msgs = vec![Message::text(Role::User, "hi")];
    let req = Request { system: vec![], tools: &[], messages: &msgs, session_id: "s", max_tokens: 16 };
    let err = complete(&cfg, &req, &mut |_| {}, &["127.0.0.1".into()]).unwrap_err().to_string();
    assert!(err.contains("401") && err.contains("Incorrect API key") && err.contains("cosmostui auth login"), "{err}");
}

#[test]
fn chatgpt_dictation_round_trip() {
    let (port, h) = mock("200 OK", "application/json", "", r#"{"text":"fix the parser","asset_ttl":"30d"}"#);
    let base = format!("http://127.0.0.1:{port}/backend-api");
    let text = zproviders::transcribe_chatgpt(&base, "tok", "acc_1", b"RIFFfake", &["127.0.0.1".to_string()]).unwrap();
    assert_eq!(text, "fix the parser");
    let req = h.join().unwrap();
    assert!(req.starts_with("POST /backend-api/transcribe "), "{req}");
    let low = req.to_ascii_lowercase();
    assert!(low.contains("authorization: bearer tok") && low.contains("chatgpt-account-id: acc_1"));
    assert!(req.contains("name=\"file\"; filename=\"dictation.wav\"\r\nContent-Type: audio/wav\r\n\r\nRIFFfake\r\n--"), "{req}");
    assert!(req.trim_end().ends_with("--"), "closing boundary");
}

#[test]
fn allowlist_rejects_example_com() {
    let allowed = vec!["api.anthropic.com".to_string(), "127.0.0.1".to_string()];
    assert!(host_allowed("https://example.com/v1/messages", &allowed).is_err());
    let cfg = ProviderCfg { kind: ProviderKind::OpenAI, base_url: "https://example.com/v1".into(), model: "m".into(), ..Default::default() };
    let msgs = vec![Message::text(Role::User, "hi")];
    let req = Request { system: vec![], tools: &[], messages: &msgs, session_id: "s", max_tokens: 16 };
    let err = complete(&cfg, &req, &mut |_| {}, &allowed).unwrap_err().to_string();
    assert!(err.contains("not on the allowlist"), "{err}");
}

fn body_of(raw: &str) -> serde_json::Value {
    serde_json::from_str(&raw[raw.find("\r\n\r\n").unwrap() + 4..]).unwrap()
}

/// Collect every chunk kind; usage must arrive exactly once.
fn run(cfg: &ProviderCfg, req: &Request) -> (String, Vec<(String, String, serde_json::Value)>, zproto::Usage, usize) {
    let (mut text, mut calls, mut usage, mut dones) = (String::new(), Vec::new(), zproto::Usage::default(), 0);
    complete(cfg, req, &mut |c| match c {
        Chunk::Text(t) => text.push_str(&t),
        Chunk::ToolUse { id, name, input } => calls.push((id, name, input)),
        Chunk::Done(u) => {
            usage = u;
            dones += 1;
        }
        Chunk::Err(e) => panic!("{e}"),
    }, &["127.0.0.1".into()]).unwrap();
    (text, calls, usage, dones)
}

#[test]
fn openai_stream_round_trip() {
    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"po\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"ng\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"a.rs\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":9,\"prompt_tokens_details\":{\"cached_tokens\":800}}}\n\n",
        "data: [DONE]\n\n",
    );
    let (port, h) = mock_sse("", sse);
    let cfg = ProviderCfg { kind: ProviderKind::OpenAI, base_url: format!("http://127.0.0.1:{port}/v1"), model: "m".into(), api_key: Some("k".into()), ..Default::default() };
    let msgs = vec![Message::text(Role::User, "ping")];
    let req = Request { system: vec!["sys".into()], tools: &[], messages: &msgs, session_id: "sess-1", max_tokens: 64 };
    let (text, calls, usage, dones) = run(&cfg, &req);
    let body = body_of(&h.join().unwrap());
    assert_eq!(text, "pong");
    assert_eq!(calls, vec![("c1".to_string(), "read".to_string(), json!({"path": "a.rs"}))]);
    assert_eq!((usage.input, usage.cache_read, usage.output, dones), (200, 800, 9, 1));
    assert_eq!(body["prompt_cache_key"], "sess-1");
    assert_eq!(body["stream"], true);
    assert!(body.get("tools").is_none(), "empty tools omitted");
}

#[test]
fn anthropic_stream_merges_usage_and_marks_cache() {
    let sse = concat!(
        "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"cache_read_input_tokens\":3000,\"cache_creation_input_tokens\":50,\"output_tokens\":1}}}\n\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"pong\"}}\n\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu1\",\"name\":\"grep\"}}\n\n",
        "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"pattern\\\":\\\"x\\\"}\"}}\n\n",
        "data: {\"type\":\"message_delta\",\"usage\":{\"input_tokens\":12,\"cache_read_input_tokens\":3000,\"cache_creation_input_tokens\":50,\"output_tokens\":42}}\n\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let (port, h) = mock_sse("", sse);
    let cfg = ProviderCfg { kind: ProviderKind::Anthropic, base_url: format!("http://127.0.0.1:{port}"), model: "m".into(), api_key: Some("k".into()), ..Default::default() };
    let msgs = vec![
        Message::text(Role::User, "first"),
        Message::text(Role::Assistant, "ok"),
        Message::text(Role::User, "ping"),
    ];
    let tools = vec![zproto::ToolSpec { name: "grep".into(), description: "g".into(), input_schema: json!({"type":"object"}) }];
    let req = Request { system: vec!["snippet".into(), "".into(), "boot".into()], tools: &tools, messages: &msgs, session_id: "s", max_tokens: 64 };
    let (text, calls, usage, dones) = run(&cfg, &req);
    let body = body_of(&h.join().unwrap());
    assert_eq!(text, "pong");
    assert_eq!(calls, vec![("tu1".to_string(), "grep".to_string(), json!({"pattern": "x"}))]);
    // max per field, never summed across message_start + message_delta
    assert_eq!((usage.input, usage.cache_read, usage.cache_write, usage.output, dones), (12, 3000, 50, 42, 1));
    let sys = body["system"].as_array().unwrap();
    assert_eq!(sys.len(), 2, "empty system layer dropped");
    assert_eq!(sys[1]["cache_control"]["type"], "ephemeral");
    assert!(sys[0].get("cache_control").is_none());
    assert_eq!(body["tools"][0]["cache_control"]["type"], "ephemeral");
    let m = body["messages"].as_array().unwrap();
    assert_eq!(m[2]["content"][0]["cache_control"]["type"], "ephemeral", "last user turn breakpoint");
    assert!(m[0]["content"][0].get("cache_control").is_none());
}

#[test]
fn google_stream_keeps_latest_usage() {
    let sse = concat!(
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"po\"}]}}],\"usageMetadata\":{\"promptTokenCount\":500,\"cachedContentTokenCount\":400,\"candidatesTokenCount\":1}}\n\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ng\"}]}}],\"usageMetadata\":{\"promptTokenCount\":500,\"cachedContentTokenCount\":400,\"candidatesTokenCount\":2}}\n\n",
    );
    let (port, h) = mock_sse("", sse);
    let cfg = ProviderCfg { kind: ProviderKind::Google, base_url: format!("http://127.0.0.1:{port}"), model: "gem".into(), api_key: Some("k".into()), ..Default::default() };
    let msgs = vec![Message::text(Role::User, "ping")];
    let req = Request { system: vec!["sys".into()], tools: &[], messages: &msgs, session_id: "s", max_tokens: 64 };
    let (text, _calls, usage, dones) = run(&cfg, &req);
    let raw = h.join().unwrap();
    assert!(raw.starts_with("POST /v1beta/models/gem:streamGenerateContent?alt=sse "), "{raw}");
    let body = body_of(&raw);
    assert_eq!(text, "pong");
    assert_eq!((usage.input, usage.cache_read, usage.output, dones), (100, 400, 2, 1));
    assert!(body.get("tools").is_none(), "no empty functionDeclarations");
    assert_eq!(body["generationConfig"]["maxOutputTokens"], 64);
}

#[test]
fn fixture_reports_usage_once() {
    let msgs = vec![Message::text(Role::User, "ping")];
    let req = Request { system: vec!["sys".into()], tools: &[], messages: &msgs, session_id: "s", max_tokens: 16 };
    let (_, _, usage, dones) = run(&fixture(), &req);
    assert_eq!(dones, 1);
    assert!(usage.input > 0 && usage.output > 0);
}
