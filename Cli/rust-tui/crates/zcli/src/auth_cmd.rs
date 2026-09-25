//! `zcli auth …` and `zcli models …` — bring-your-own-key and ChatGPT sign-in
//! from the command line (the TUI has the same flows under /connect, /login).

use std::io::Write;
use std::sync::atomic::AtomicBool;

use zcore::{catalog, chatgpt, config::Config, creds};
use zproto::exit;

const USAGE: &str = "\
usage:
  zcli auth                          show connected providers
  zcli auth login chatgpt            sign in with a ChatGPT Plus/Pro plan (browser)
  zcli auth login <provider> [--key K]   store an API key (prompted, hidden)
  zcli auth logout <provider|chatgpt>
  zcli models [provider]             models you can use now (or one provider's list)";

/// Read a line without echoing it (shows • per character).
pub fn read_secret(prompt: &str) -> std::io::Result<String> {
    use crossterm::event::{read, Event, KeyCode, KeyEventKind, KeyModifiers};
    print!("{prompt}");
    std::io::stdout().flush()?;
    crossterm::terminal::enable_raw_mode()?;
    let mut s = String::new();
    let res = loop {
        match read() {
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Esc => break Err(std::io::Error::other("cancelled")),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(std::io::Error::other("cancelled"))
                }
                KeyCode::Backspace => {
                    if s.pop().is_some() {
                        print!("\u{8} \u{8}");
                    }
                }
                KeyCode::Char(c) => {
                    s.push(c);
                    print!("•");
                }
                _ => {}
            },
            Ok(Event::Paste(p)) => {
                let p = p.trim();
                s.push_str(p);
                print!("{}", "•".repeat(p.chars().count()));
            }
            Ok(_) => {}
            Err(e) => break Err(e),
        }
        let _ = std::io::stdout().flush();
    };
    let _ = crossterm::terminal::disable_raw_mode();
    println!();
    res.map(|_| s)
}

fn status() -> i32 {
    let cfg = Config::load(&zcore::cli_home());
    match chatgpt::load() {
        Some(t) => println!(
            "  ✓ chatgpt        signed in{}{}",
            t.email.map(|e| format!(" as {e}")).unwrap_or_default(),
            t.plan.map(|p| format!(" · {p} plan")).unwrap_or_default()
        ),
        None => println!("  · chatgpt        not signed in — zcli auth login chatgpt"),
    }
    for p in catalog::providers().iter().filter(|p| !p.is_subscription() && !p.env.is_empty()) {
        match creds::key_for(&p.id, &p.env) {
            Some((k, src)) => println!("  ✓ {:<14} {} ({}) · {} models", p.id, creds::mask(&k), src.label(), p.models.len()),
            None => println!("  · {:<14} {}", p.id, p.name),
        }
    }
    let n = cfg.available_models().len();
    println!("\n  {n} models available · zcli models · zcli auth login <provider>");
    exit::DONE
}

fn login(args: &[String]) -> i32 {
    let Some(id) = args.first() else {
        eprintln!("{USAGE}");
        return exit::USAGE;
    };
    if id == "chatgpt" || id == "openai-subscription" {
        let cancel = AtomicBool::new(false);
        let r = chatgpt::login(&cancel, |url| {
            println!("Opening your browser to sign in with ChatGPT…\nIf it doesn't open, visit:\n\n  {url}\n");
            chatgpt::open_browser(url);
        });
        return match r {
            Ok(t) => {
                println!(
                    "✓ Signed in{}{}. Models: {}",
                    t.email.map(|e| format!(" as {e}")).unwrap_or_default(),
                    t.plan.map(|p| format!(" ({p})")).unwrap_or_default(),
                    catalog::provider("chatgpt").map(|p| p.models.iter().map(|m| format!("chatgpt/{}", m.id)).collect::<Vec<_>>().join(", ")).unwrap_or_default()
                );
                exit::DONE
            }
            Err(e) => {
                eprintln!("✗ {e:#}");
                exit::FAILED
            }
        };
    }
    let known = catalog::provider(id).is_some() || Config::load(&zcore::cli_home()).providers.contains_key(id.as_str());
    if !known {
        eprintln!("unknown provider {id:?}. Known: {}", catalog::providers().iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", "));
        return exit::USAGE;
    }
    let key = match args.iter().position(|a| a == "--key").and_then(|i| args.get(i + 1)) {
        Some(k) => k.clone(),
        None => {
            let name = catalog::provider(id).map(|p| p.name.clone()).unwrap_or_else(|| id.clone());
            match read_secret(&format!("{name} API key: ")) {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("✗ {e}");
                    return exit::FAILED;
                }
            }
        }
    };
    match creds::store_key(id, &key) {
        Ok(src) => {
            println!("✓ Saved {} key ({}) to the {}", id, creds::mask(key.trim()), src.label());
            if let Some(p) = catalog::provider(id) {
                if let Some(m) = p.models.first() {
                    println!("  try: zcli --model {}/{}", p.id, m.id);
                }
            }
            exit::DONE
        }
        Err(e) => {
            eprintln!("✗ {e:#}");
            exit::FAILED
        }
    }
}

fn logout(args: &[String]) -> i32 {
    let Some(id) = args.first() else {
        eprintln!("{USAGE}");
        return exit::USAGE;
    };
    let r = if id == "chatgpt" { chatgpt::logout() } else { creds::delete_key(id) };
    match r {
        Ok(()) => {
            println!("✓ Removed {id} credentials");
            exit::DONE
        }
        Err(e) => {
            eprintln!("✗ {e:#}");
            exit::FAILED
        }
    }
}

pub fn auth_cmd(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("status") | Some("list") => status(),
        Some("login") => login(&args[1..]),
        Some("logout") => logout(&args[1..]),
        _ => {
            eprintln!("{USAGE}");
            exit::USAGE
        }
    }
}

fn price(p: &zproto::Prices) -> String {
    if *p == zproto::Prices::default() {
        "free / included".into()
    } else {
        format!("${:.2} in · ${:.2} out /1M", p.input, p.output)
    }
}

pub fn models_cmd(args: &[String]) -> i32 {
    if let Some(id) = args.first() {
        let Some(p) = catalog::provider(id) else {
            eprintln!("unknown provider {id:?}");
            return exit::USAGE;
        };
        println!("{} ({}) — {}\n", p.name, p.kind, p.base_url);
        for m in &p.models {
            println!("  {}/{:<40} {:>6}k ctx  {}", p.id, m.id, m.context / 1000, price(&m.prices()));
        }
        return exit::DONE;
    }
    let cfg = Config::load(&zcore::cli_home());
    for m in cfg.available_models() {
        println!("  {:<48} {:>6}k ctx  {}", m.alias, m.context / 1000, price(&m.price));
    }
    exit::DONE
}
