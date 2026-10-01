//! §9 compaction gate: a 40-turn session of fat `read`s through the real
//! agent loop (fixture provider). Measures what every request actually sends
//! against naive full replay, and checks the prompt-cache behaviour:
//! the stable prefix never changes, and the conversation prefix only changes
//! at a logged checkpoint (never turn by turn).

use zcore::agent::Agent;
use zcore::compact::{estimate_tokens, prefix_hash};
use zproto::{Block, Message, RunOpts};

struct Run {
    naive_final: u64,
    view_final: u64,
    naive_cum: u64,
    view_cum: u64,
    busts: usize,
    checkpoints: usize,
    max_view: u64,
    prefix_stable: bool,
    resume_matches: bool,
}

fn same(a: &Message, b: &Message) -> bool {
    serde_json::to_string(a).unwrap() == serde_json::to_string(b).unwrap()
}

/// Drive 40 `tool:read` turns. `line_chars` sets how fat each file is.
fn run(home: &std::path::Path, line_chars: usize, window: Option<u64>) -> Run {
    std::env::set_var("COSMOS_CLI_HOME", home);
    std::env::set_var("MERIDIAN_BIN", home.join("no-meridian"));
    let ws = home.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    for i in 0..40 {
        let body: String = (0..80).map(|l| format!("{:0>w$}\n", format!("f{i}l{l}"), w = line_chars)).collect();
        std::fs::write(ws.join(format!("f{i}.txt")), body).unwrap();
    }
    let opts = RunOpts { model: Some("fixture".into()), cwd: Some(ws.to_string_lossy().into()), yolo: true, ..Default::default() };
    let mut a = Agent::new(&opts, "cosmos", &ws).unwrap();
    if let Some(w) = window {
        a.context_window = w;
    }
    let prefix0 = prefix_hash(&a.system, ztools::TOOLS_JSON);
    let mut sink = |_: &zproto::Event| {};
    let (mut naive_cum, mut view_cum, mut busts, mut max_view) = (0u64, 0u64, 0usize, 0u64);
    let mut inflation = 0u64; // tokens L1 moved to disk (naive replay keeps them)
    let mut prev: Option<Vec<Message>> = None;
    let mut ops_before = 0usize;
    let mut checkpoints = 0usize;
    let mut naive_final = 0;
    let mut view_final = 0;
    for i in 0..40 {
        a.send_user(&format!(r#"tool:read {{"path":"f{i}.txt"}}"#), &mut sink);
        for step in 0..2 {
            let done = a.turn(&mut sink).unwrap();
            // the request just sent = current view minus the reply it produced
            let v = a.model_view();
            let req = v[..v.len() - 1].to_vec();
            let t = estimate_tokens(&req, a.boot_tokens);
            let naive = estimate_tokens(&a.messages[..a.messages.len() - 1], a.boot_tokens) + inflation;
            view_cum += t;
            naive_cum += naive;
            max_view = max_view.max(t);
            naive_final = naive;
            view_final = t;
            let cp = a.view_ops.len() > ops_before;
            if cp {
                checkpoints += 1;
                ops_before = a.view_ops.len();
            }
            if let Some(p) = &prev {
                let extends = p.len() <= req.len() && p.iter().zip(&req).all(|(x, y)| same(x, y));
                if !extends {
                    busts += 1;
                    assert!(cp, "turn {i}.{step}: conversation prefix changed without a checkpoint");
                }
            }
            prev = Some(req);
            if done {
                break;
            }
            let calls: Vec<(String, String, serde_json::Value)> = a
                .messages
                .last()
                .unwrap()
                .content
                .iter()
                .filter_map(|b| match b {
                    Block::ToolUse { id, name, input } => Some((id.clone(), name.clone(), input.clone())),
                    _ => None,
                })
                .collect();
            let full = std::fs::read_to_string(ws.join(format!("f{i}.txt"))).unwrap().len() as u64;
            a.exec_tools_batch(&calls, &mut sink);
            if let Some(Block::ToolResult { content, .. }) = a.messages.last().unwrap().content.first() {
                // a hashline read is ~1.15× the file; naive keeps all of it
                let full_read = (full * 115 / 100).min(ztools::RESULT_CAP as u64);
                inflation += full_read.saturating_sub(content.len() as u64) / 4;
            }
        }
    }
    let prefix_stable = prefix_hash(&a.system, ztools::TOOLS_JSON) == prefix0 && a.prefix_hash == prefix0;
    // resume from the log must rebuild exactly the view the model last saw
    let id = a.session.id.clone();
    let live = a.model_view();
    drop(a);
    let opts = RunOpts { model: Some("fixture".into()), cwd: Some(ws.to_string_lossy().into()), resume: Some(id), ..Default::default() };
    let b = Agent::new(&opts, "cosmos", &ws).unwrap();
    let resumed = b.model_view();
    let resume_matches = live.len() == resumed.len() && live.iter().zip(&resumed).all(|(x, y)| same(x, y));
    Run { naive_final, view_final, naive_cum, view_cum, busts, checkpoints, max_view, prefix_stable, resume_matches }
}

#[test]
fn forty_turns_of_fat_reads() {
    // A: the EVIDENCE.md fixture — 40 reads of ~32 KB (80 fat lines) each.
    let home = tempfile::tempdir().unwrap();
    let r = run(home.path(), 400, None);
    let pct = |a: u64, b: u64| a as f64 * 100.0 / b.max(1) as f64;
    println!(
        "A fat reads: final {} / naive {} = {:.1}% · cumulative {} / {} = {:.1}% · checkpoints {} · busts {} · prefix stable {} · resume == live {}",
        r.view_final, r.naive_final, pct(r.view_final, r.naive_final),
        r.view_cum, r.naive_cum, pct(r.view_cum, r.naive_cum),
        r.checkpoints, r.busts, r.prefix_stable, r.resume_matches
    );
    assert!(pct(r.view_final, r.naive_final) <= 35.0);
    assert!(pct(r.view_cum, r.naive_cum) <= 35.0);
    assert!(r.prefix_stable);
    assert!(r.busts <= r.checkpoints);
    assert!(r.resume_matches);

    // B: normal-size reads (~6 KB, under L1) in a 30k window — the
    // checkpoint layers (L2–L5) have to do the work.
    let home = tempfile::tempdir().unwrap();
    let r = run(home.path(), 70, Some(30_000));
    println!(
        "B 30k window: final {} / naive {} = {:.1}% · cumulative {} / {} = {:.1}% · checkpoints {} · busts {} · max request {} · prefix stable {} · resume == live {}",
        r.view_final, r.naive_final, pct(r.view_final, r.naive_final),
        r.view_cum, r.naive_cum, pct(r.view_cum, r.naive_cum),
        r.checkpoints, r.busts, r.max_view, r.prefix_stable, r.resume_matches
    );
    assert!(r.max_view < 30_000, "never over the window");
    assert!(pct(r.view_final, r.naive_final) <= 35.0);
    assert!(r.prefix_stable);
    assert!(r.busts <= r.checkpoints);
    assert!(r.busts < 40, "cache re-paid at checkpoints, not every turn");
    assert!(r.resume_matches);
}
