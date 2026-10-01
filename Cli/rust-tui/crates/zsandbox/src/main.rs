//! zsandbox — CLI front-end over the zsandbox library.
//!
//!   zsandbox exec --workspace <dir> --net deny|allow --timeout <s> -- <argv...>

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use zsandbox::{exec, ExecOpts, NetPolicy};

fn usage() -> ! {
    eprintln!(
        "zsandbox — sandboxed command execution (AppContainer + Job on Windows)\n\n\
         USAGE\n  zsandbox exec --workspace <dir> [--net deny|allow] [--timeout <secs>] -- <argv...>\n\n\
         Exits with the child's exit code; 124 on timeout; 2 on sandbox setup failure.\n\
         When the sandbox is degraded a `zsandbox: degraded: <reason>` line is printed to stderr."
    );
    std::process::exit(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        usage();
    }
    if args[0] != "exec" {
        usage();
    }
    let mut workspace = None;
    let mut net = NetPolicy::Deny;
    let mut timeout = Duration::from_secs(60);
    let mut argv = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--workspace" => {
                workspace = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--net" => {
                net = match args.get(i + 1).map(String::as_str) {
                    Some("allow") => NetPolicy::Allow,
                    _ => NetPolicy::Deny,
                };
                i += 2;
            }
            "--timeout" => {
                timeout =
                    Duration::from_secs(args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(60));
                i += 2;
            }
            "--" => {
                argv = args[i + 1..].to_vec();
                break;
            }
            _ => usage(),
        }
    }
    let Some(workspace) = workspace else { usage() };
    if argv.is_empty() {
        usage();
    }
    match exec(&ExecOpts::new(workspace, net, timeout, argv)) {
        Ok(r) => {
            let mut so = std::io::stdout().lock();
            let mut se = std::io::stderr().lock();
            let _ = so.write_all(&r.stdout);
            let _ = so.flush();
            if let Some(reason) = &r.degraded {
                let _ = writeln!(se, "zsandbox: degraded: {reason}");
            }
            let _ = se.write_all(&r.stderr);
            let _ = se.flush();
            ExitCode::from(if r.timed_out {
                124
            } else {
                (r.code & 0xff) as u8
            })
        }
        Err(e) => {
            eprintln!("zsandbox: {e:#}");
            ExitCode::from(2)
        }
    }
}
