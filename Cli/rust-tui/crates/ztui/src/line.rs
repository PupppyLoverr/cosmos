//! Line-oriented REPL — the fallback when there's no terminal for ratatui
//! (piped stdin/stdout, CI). Streams assistant text, renders tool calls
//! inline, prompts for `ask` permissions, and exposes the slash commands.

use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::Result;
use zcore::agent::{Agent, Mode};
use zproto::{Event, RunOpts};

fn render(ev: &Event) {
    match ev.kind.as_str() {
        "assistant" => {
            if let Some(t) = ev.data["text"].as_str() {
                if !t.is_empty() {
                    println!("{t}");
                }
            }
            if let Some(calls) = ev.data["tool_calls"].as_array() {
                for c in calls {
                    eprintln!(
                        "  → {} {}",
                        c["name"].as_str().unwrap_or(""),
                        truncate(&c["input"].to_string(), 120)
                    );
                }
            }
        }
        "tool_result" => {
            let name = ev.data["name"].as_str().unwrap_or("tool");
            let err = ev.data["is_error"].as_bool().unwrap_or(false);
            let content = ev.data["content"].as_str().unwrap_or("");
            let mark = if err { "✗" } else { "✓" };
            eprintln!("  {mark} {name}: {}", truncate(content, 160));
        }
        "plan" => {
            println!(
                "\n── plan ──\n{}\n──────────\n(/approve to run, or keep refining)",
                ev.data["plan"].as_str().unwrap_or("")
            );
        }
        "permission_decision" => {}
        "compaction" => eprintln!(
            "  · compacted {}→{} tokens",
            ev.data["before_tokens"], ev.data["after_tokens"]
        ),
        "subagent_start" => eprintln!("  ⎇ subagent: {}", ev.data["prompt"].as_str().unwrap_or("")),
        _ => {}
    }
}

fn truncate(s: &str, n: usize) -> String {
    let s: String = s.chars().take(n).collect();
    if s.len() < n {
        s
    } else {
        format!("{s}…")
    }
}

fn prompt_line(prompt: &str) -> Option<String> {
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    let stdin = std::io::stdin();
    let mut line = String::new();
    match stdin.lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_string()),
    }
}

/// Interactive line-mode session loop. Returns the process exit code.
pub fn repl(opts: &RunOpts, cwd: &Path) -> Result<i32> {
    let mut agent = Agent::new(opts, zcore::NAME, cwd)?;
    eprintln!(
        "{} · {} · mode {} · session {}",
        zcore::NAME,
        agent.model_alias,
        agent.mode.name(),
        &agent.session.id[..8]
    );
    if let Some(d) = &agent.ctx.sandbox_degraded {
        eprintln!("sandbox degraded: {d}");
    }
    agent.asker = Some(Box::new(|tool: &str| {
        matches!(
            prompt_line(&format!("allow {tool}? [y/N] ")).as_deref(),
            Some("y") | Some("yes")
        )
    }));
    let mut emit = |ev: &Event| render(ev);
    let start = agent.session.append(
        "session_start",
        serde_json::json!({
            "model": agent.model_alias, "mode": agent.mode.name(),
            "sandbox": if agent.ctx.no_sandbox {"off"} else {"appcontainer"},
            "boot_tokens": agent.boot_tokens,
        }),
    );
    emit(&start);
    // If -p carried a prompt into the REPL (resume), run it first.
    if let Some(p) = &opts.prompt {
        if p != "-" {
            run_task(&mut agent, p, &mut emit);
        }
    }
    while let Some(line) = prompt_line("› ") {
        if line.is_empty() {
            continue;
        }
        match line.as_str() {
            "/quit" | "/exit" | "/q" => break,
            "/plan" => {
                agent.mode = Mode::Plan;
                eprintln!("mode: plan");
            }
            "/agent" => {
                agent.mode = Mode::Agent;
                eprintln!("mode: agent");
            }
            "/yolo" => {
                agent.mode = Mode::Yolo;
                eprintln!("mode: yolo (sandbox still on unless --no-sandbox)");
            }
            s if s.starts_with("/compact") => {
                let focus = s.strip_prefix("/compact").unwrap_or("").trim();
                agent.compact_now(if focus.is_empty() { None } else { Some(focus) }, &mut emit);
            }
            "/undo" => {
                eprintln!("{}", agent.undo_last(&mut emit));
            }
            "/approve" => {
                agent.approve_plan(&mut emit);
                run_turns(&mut agent, &mut emit);
            }
            "/session" => {
                eprintln!("session {}", agent.session.id);
            }
            "/help" => {
                eprintln!("commands: /plan /agent /yolo /approve /session /quit — anything else is a task");
            }
            _ => run_task(&mut agent, &line, &mut emit),
        }
    }
    agent
        .session
        .append("session_end", serde_json::json!({"reason": "quit"}));
    Ok(zproto::exit::DONE)
}

fn run_task(agent: &mut Agent, task: &str, emit: &mut dyn FnMut(&Event)) {
    agent.send_user(task, emit);
    run_turns(agent, emit);
}

fn run_turns(agent: &mut Agent, emit: &mut dyn FnMut(&Event)) {
    let max = agent.opts.max_turns.unwrap_or(32);
    for _ in 0..max {
        match agent.turn(emit) {
            Ok(true) => break,
            Ok(false) => {
                let calls: Vec<(String, String, serde_json::Value)> = agent
                    .messages
                    .last()
                    .map(|m| {
                        m.content
                            .iter()
                            .filter_map(|b| match b {
                                zproto::Block::ToolUse { id, name, input } => {
                                    Some((id.clone(), name.clone(), input.clone()))
                                }
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                for (id, name, input) in calls {
                    agent.exec_tool(&id, &name, &input, emit);
                    if name == "submit_plan" {
                        return; // wait for /approve
                    }
                }
            }
            Err(e) => {
                eprintln!("turn failed: {e}");
                break;
            }
        }
    }
}
