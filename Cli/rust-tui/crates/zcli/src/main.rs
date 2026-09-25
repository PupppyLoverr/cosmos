//! zcli — Cosmos Rust harness. `zcli -p "task"` headless, bare `zcli` REPL.

use std::path::PathBuf;

use zcore::agent::{emitter, run_headless};
use zcore::session::Session;
use zproto::{exit, OutputMode, RunOpts};

const HELP: &str = "\
zcli — Cosmos coding-agent harness (Rust)

usage:
  zcli                        interactive session (REPL)
  zcli -p \"<task>\" [flags]    headless run; -p - reads the task from stdin
  zcli tools dump             canonical tools.json (byte-identical to zdev/zagy)
  zcli session list           sessions in COSMOS_CLI_HOME
  zcli session export <id> [--format md|jsonl]
  zcli session branch <id> --at <event_id>
  zcli memory search <q>      search the shared Meridian vault
  zcli memory add <text>      append to the vault (scope: zcli)
  zcli auth [login|logout] …  API keys (30 providers) and ChatGPT sign-in
  zcli models [provider]      models you can use now
  zcli onboarding             run the first-run setup again (until it retires itself)

flags:
  --output text|json|json-stream   (default text)
  --mode plan|agent
  --allow <tool[,tool…]>           permit tools for this run
  --yolo                           allow all tools (sandbox stays on)
  --no-sandbox                     run bash without the sandbox
  --model <alias|provider/model>   e.g. anthropic/claude-opus-5-5, chatgpt/gpt-5.5
  --max-turns <n>                  (default 16)
  --max-usd <f>                    budget cap → exit 5
  --cwd <dir>                      workspace root (default: cwd)
  --resume <session-id>
  --approve-plan                   headless: auto-approve a submitted plan
  --allow-escalation <cap[,…]>     e.g. network
  --version | --help
";

fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn parse(args: &[String]) -> Result<RunOpts, String> {
    let mut o = RunOpts::default();
    let mut i = 0;
    let mut positional: Vec<String> = Vec::new();
    while i < args.len() {
        let a = args[i].as_str();
        let mut val = |flag: &str| -> Result<String, String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match a {
            "-p" | "--prompt" => o.prompt = Some(val("-p")?),
            "--output" => {
                let v = val("--output")?;
                o.output = OutputMode::parse(&v).ok_or_else(|| format!("bad --output {v}"))?;
            }
            "--mode" => o.mode = Some(val("--mode")?),
            "--allow" => o.allow = split_list(&val("--allow")?),
            "--yolo" => o.yolo = true,
            "--no-sandbox" => o.no_sandbox = true,
            "--model" => o.model = Some(val("--model")?),
            "--max-turns" => {
                o.max_turns = Some(val("--max-turns")?.parse().map_err(|_| "bad --max-turns")?)
            }
            "--max-usd" => {
                o.max_usd = Some(val("--max-usd")?.parse().map_err(|_| "bad --max-usd")?)
            }
            "--cwd" => o.cwd = Some(val("--cwd")?),
            "--resume" => o.resume = Some(val("--resume")?),
            "--approve-plan" => o.approve_plan = true,
            "--allow-escalation" => o.allow_escalation = split_list(&val("--allow-escalation")?),
            "--version" | "--help" | "-h" | "-V" => {
                println!(
                    "zcli {} (contract v{})\n{HELP}",
                    env!("CARGO_PKG_VERSION"),
                    zproto::CONTRACT_VERSION
                );
                std::process::exit(exit::DONE);
            }
            _ if a.starts_with('-') => return Err(format!("unknown flag {a}")),
            _ => positional.push(a.to_string()),
        }
        i += 1;
    }
    if o.prompt.is_none() && !positional.is_empty() {
        o.prompt = Some(positional.join(" "));
    }
    Ok(o)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let code = match args.first().map(String::as_str) {
        Some("tools") => {
            if args.get(1).map(String::as_str) == Some("dump") {
                print!("{}", ztools::tools_dump());
                exit::DONE
            } else {
                eprintln!("usage: zcli tools dump");
                exit::USAGE
            }
        }
        Some("session") => session_cmd(&args[1..]),
        Some("memory") => memory_cmd(&args[1..]),
        Some("auth") | Some("login") => auth_cmd::auth_cmd(if args[0] == "login" { &args[..] } else { &args[1..] }),
        Some("logout") => auth_cmd::auth_cmd(&args[..]),
        Some("models") => auth_cmd::models_cmd(&args[1..]),
        Some("onboarding") | Some("onboard") => match zcore::welcome::launch() {
            zcore::welcome::Outcome::Done | zcore::welcome::Outcome::Quit => exit::DONE,
            zcore::welcome::Outcome::Missing => {
                eprintln!("onboarding already ran and removed itself — use /connect, /login and /model inside zcli, or  zcli auth login <provider>");
                exit::DONE
            }
            zcore::welcome::Outcome::Failed(e) => {
                eprintln!("{e}");
                exit::FAILED
            }
        },
        Some("acp") => acp::run(),
        _ => {
            let opts = match parse(&args) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("{e}\n{HELP}");
                    std::process::exit(exit::USAGE);
                }
            };
            if opts.prompt.is_some() {
                match run_headless(&opts, "zcli", &cwd, &mut emitter(opts.output)) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("{e:#}");
                        exit::FAILED
                    }
                }
            } else {
                match ztui::repl(&opts, &cwd) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("{e:#}");
                        exit::FAILED
                    }
                }
            }
        }
    };
    std::process::exit(code);
}

fn session_cmd(args: &[String]) -> i32 {
    let home = zcore::cli_home();
    match args.first().map(String::as_str) {
        Some("list") => {
            for s in Session::list(&home) {
                println!("{}", serde_json::to_string(&s).unwrap());
            }
            exit::DONE
        }
        Some("export") => {
            let id = args.get(1).cloned().unwrap_or_default();
            let fmt = args
                .iter()
                .position(|a| a == "--format")
                .and_then(|i| args.get(i + 1))
                .cloned()
                .unwrap_or_else(|| "md".into());
            match Session::export(&home, &id, &fmt) {
                Ok(t) => {
                    print!("{t}");
                    exit::DONE
                }
                Err(e) => {
                    eprintln!("{e:#}");
                    exit::USAGE
                }
            }
        }
        Some("branch") => {
            let id = args.get(1).cloned().unwrap_or_default();
            let at = args
                .iter()
                .position(|a| a == "--at")
                .and_then(|i| args.get(i + 1))
                .cloned()
                .unwrap_or_default();
            match Session::open(&home, &id) {
                Ok(mut s) => {
                    s.branch(&at);
                    println!("branched {id} at {at}");
                    exit::DONE
                }
                Err(e) => {
                    eprintln!("{e:#}");
                    exit::USAGE
                }
            }
        }
        Some("tree") => {
            let id = args.get(1).cloned().unwrap_or_default();
            let file = home.join("sessions").join(format!("{id}.jsonl"));
            match zcore::session::read_events(&file) {
                Ok(events) => {
                    use std::collections::HashMap;
                    let by_parent: HashMap<Option<String>, Vec<&zproto::Event>> = {
                        let mut m: HashMap<Option<String>, Vec<&zproto::Event>> = HashMap::new();
                        for e in &events {
                            m.entry(e.parent.clone()).or_default().push(e);
                        }
                        m
                    };
                    fn walk(
                        parent: &Option<String>,
                        depth: usize,
                        by: &HashMap<Option<String>, Vec<&zproto::Event>>,
                    ) {
                        if let Some(kids) = by.get(parent) {
                            for e in kids {
                                println!("{}{} [{}] {}", "  ".repeat(depth), e.id, e.kind, e.ts);
                                walk(&Some(e.id.clone()), depth + 1, by);
                            }
                        }
                    }
                    walk(&None, 0, &by_parent);
                    // orphans (parent points at a checkout target mid-file)
                    for e in &events {
                        if e.parent.is_some()
                            && !events.iter().any(|x| Some(&x.id) == e.parent.as_ref())
                        {
                            println!("  {} [{}] {} (orphan)", e.id, e.kind, e.ts);
                        }
                    }
                    exit::DONE
                }
                Err(e) => {
                    eprintln!("{e:#}");
                    exit::USAGE
                }
            }
        }
        _ => {
            eprintln!("usage: zcli session list|export|branch|tree");
            exit::USAGE
        }
    }
}

mod acp;
mod auth_cmd;

fn memory_cmd(args: &[String]) -> i32 {
    let home = zcore::cli_home();
    let ws = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut ctx = ztools::ToolContext::new(&ws);
    let _ = home;
    match args.first().map(String::as_str) {
        Some("search") => {
            let q = args.get(1..).map(|a| a.join(" ")).unwrap_or_default();
            println!(
                "{}",
                ztools::run(&mut ctx, "memory_search", &serde_json::json!({"query": q}))
            );
            exit::DONE
        }
        Some("add") => {
            let mut ty = "map".to_string();
            let mut words: Vec<String> = Vec::new();
            let mut it = args[1..].iter().peekable();
            while let Some(a) = it.next() {
                if a == "--type" {
                    ty = it.next().cloned().unwrap_or_else(|| "map".into());
                } else {
                    words.push(a.clone());
                }
            }
            let text = words.join(" ");
            println!(
                "{}",
                ztools::run(
                    &mut ctx,
                    "memory_write",
                    &serde_json::json!({"type": ty, "text": text, "scope": "zcli"})
                )
            );
            exit::DONE
        }
        _ => {
            eprintln!("usage: zcli memory search|add");
            exit::USAGE
        }
    }
}
