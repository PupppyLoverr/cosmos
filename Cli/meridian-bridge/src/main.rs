//! `meridian` — CLI front-end for the meridian bridge.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use meridian_bridge::rpc::{self, Bridge};
use meridian_bridge::vault::Vault;
use serde_json::json;

fn arg_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn usage() -> &'static str {
    "meridian — Cosmos CLI memory bridge\n\n\
     usage:\n\
       meridian serve --stdio\n\
       meridian write --type <decision|preference|map|correction|playbook> --scope <s> --text <t> [--src <h/s>]\n\
       meridian search --query <q> [--k <n>] [--scope <s>]\n\
       meridian boot --repo <dir> [--harness <h>]\n\
       meridian cache-read --path <file> [--max-lines <n>]\n\
       meridian playbook <name>\n\
       meridian session-register --session <id> --harness <h> --log-path <p> --title <t>\n"
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("meridian: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn run(args: &[String]) -> Result<u8> {
    let Some(cmd) = args.first().map(String::as_str) else {
        eprint!("{}", usage());
        return Ok(2);
    };
    if matches!(cmd, "--help" | "-h" | "help") {
        print!("{}", usage());
        return Ok(0);
    }
    let vault = Vault::open_default()?;
    let bridge = Bridge::open(vault);
    match cmd {
        "serve" => {
            rpc::serve_stdio(&bridge)?;
            Ok(0)
        }
        "write" => {
            let mtype = arg_value(args, "--type").unwrap_or("");
            let scope = arg_value(args, "--scope").unwrap_or("misc");
            let text = arg_value(args, "--text").unwrap_or("");
            let src = arg_value(args, "--src").unwrap_or("cli/manual");
            let id = bridge.memory_write(mtype, text, scope, src)?;
            println!("{}", serde_json::to_string(&json!({"id": id}))?);
            Ok(0)
        }
        "search" => {
            let query = arg_value(args, "--query").unwrap_or("");
            let k = arg_value(args, "--k")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(5);
            let scope = arg_value(args, "--scope");
            let hits = bridge.memory_search(query, k, scope, &[]);
            for h in &hits {
                println!(
                    "{:.3}  {}  [{}] [{}] {}",
                    h.score, h.id, h.kind, h.date, h.text
                );
            }
            Ok(0)
        }
        "boot" => {
            let repo = arg_value(args, "--repo")
                .map(PathBuf::from)
                .unwrap_or(std::env::current_dir()?);
            let harness = arg_value(args, "--harness").unwrap_or("cli");
            let ctx = bridge.boot_context(&repo, harness)?;
            println!("{}", serde_json::to_string_pretty(&ctx)?);
            Ok(0)
        }
        "cache-read" => {
            let path = arg_value(args, "--path").map(PathBuf::from);
            let Some(path) = path else {
                eprintln!("cache-read needs --path");
                return Ok(2);
            };
            let max_lines = arg_value(args, "--max-lines")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(200);
            println!("{}", bridge.cache_read(&path, max_lines)?);
            Ok(0)
        }
        "playbook" => {
            let Some(name) = args.get(1) else {
                eprintln!("playbook needs a name");
                return Ok(2);
            };
            println!("{}", bridge.playbook_get(name)?);
            Ok(0)
        }
        "session-register" => {
            let p = bridge.session_register(
                arg_value(args, "--session").unwrap_or(""),
                arg_value(args, "--harness").unwrap_or(""),
                arg_value(args, "--log-path").unwrap_or(""),
                arg_value(args, "--title").unwrap_or(""),
            )?;
            println!("{}", p.display());
            Ok(0)
        }
        other => {
            eprintln!("unknown command '{other}'\n\n{}", usage());
            Ok(2)
        }
    }
}
