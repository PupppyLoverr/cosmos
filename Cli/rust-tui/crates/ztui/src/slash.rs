//! Slash commands and fuzzy matching (nucleo — the matcher Devin's `/` menu
//! uses) shared by the command menu, the model picker and `@` mentions.
//!
//! Besides the built-ins, markdown files become commands, Claude-Code style:
//! `.cosmos/commands/<name>.md` (project; `.zcli/` still read), `~/.cosmos-cli/commands/<name>.md`
//! (personal) and `.claude/commands/<name>.md` (so existing Claude Code
//! commands work as-is). The first line (or a `description:` front-matter
//! field) is the menu description; `$ARGUMENTS` is replaced by what follows
//! the command.

use std::path::{Path, PathBuf};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

#[derive(Clone, Debug)]
pub struct Slash {
    pub name: String,
    /// inline argument hint, e.g. `[plan|agent|yolo]`
    pub hint: String,
    pub desc: String,
    /// custom command body (markdown prompt); `None` for built-ins
    pub body: Option<String>,
}

const BUILTIN: &[(&str, &str, &str)] = &[
    ("model", "", "Interactively choose a model, or specify one directly"),
    ("providers", "", "Configure providers — API keys, ChatGPT, Devin subscription"),
    ("connect", "[provider]", "Add an API key for OpenAI, Anthropic, Google, Meta, MiMo, Qwen, …"),
    ("login", "", "Sign in with your ChatGPT Plus/Pro plan"),
    ("logout", "[provider]", "Remove a stored key or the ChatGPT sign-in"),
    ("voice", "", "Dictate your task (ctrl+space)"),
    ("usage", "", "Show session usage of tokens, cost and plan limits"),
    ("context", "", "Show context window usage"),
    ("mode", "[plan|agent|accept-edits|yolo]", "Show or switch mode"),
    ("plan", "", "Switch to plan mode (read-only)"),
    ("agent", "", "Switch to agent mode"),
    ("accept-edits", "", "Writes and edits run without asking"),
    ("resume", "", "Continue an earlier session"),
    ("init", "", "Write an AGENTS.md that explains this repo to agents"),
    ("review", "", "Review the uncommitted changes"),
    ("diff", "", "Show what changed in the working tree"),
    ("copy", "", "Copy the last reply to the clipboard"),
    ("approve", "", "Approve the pending plan and run it"),
    ("compact", "[focus: …]", "Five-layer compact; optional focus steers the summary"),
    ("undo", "", "Restore the last file checkpoint"),
    ("skillify", "[name]", "Distill this session into a skill"),
    ("yolo", "", "Switch to yolo mode"),
    ("session", "", "Show the session id and how to resume it"),
    ("tree", "", "Show the session's event tree and branch points"),
    ("clear", "", "Clear the screen"),
    ("theme", "<name>", "Colour theme: cosmos, classic, nebula, mono, solar, light, high-contrast"),
    ("buddy", "[pet|trick|species|…]", "Your pet — its card, level and tricks"),
    ("pettalk", "<message>", "Talk to your pet — it knows the session, sets reminders, does tricks"),
    ("notify", "[test]", "Title, bell and desktop notifications — settings, or send a test"),
    ("customise", "[request|…]", "Restyle the TUI: a quick picker, or ask any model (stop, undo, reset, log, diff, model)"),
    ("verbs", "[add|remove|reset]", "The thinking verbs — Cosmosing, Plotting, Cooking…"),
    ("spinner", "<style>", "Thinking spinner: orb, twinkle, braille, moon, comet, pulse, galaxy, dots, line"),
    ("onboarding", "", "Replay the first-run setup (until it retires itself)"),
    ("help", "", "Keyboard shortcuts"),
    ("quit", "", "Exit Cosmos"),
];

pub fn builtins() -> Vec<Slash> {
    BUILTIN
        .iter()
        .map(|(n, h, d)| Slash { name: n.to_string(), hint: h.to_string(), desc: d.to_string(), body: None })
        .collect()
}

fn describe(text: &str) -> (String, String) {
    // optional front matter: ---\ndescription: …\n---
    if let Some(rest) = text.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            let fm = &rest[..end];
            let desc = fm
                .lines()
                .find_map(|l| l.strip_prefix("description:"))
                .map(|d| d.trim().trim_matches('"').to_string())
                .unwrap_or_default();
            let body = rest[end + 4..].trim_start_matches(['-', '\r', '\n']).to_string();
            return (desc, body);
        }
    }
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim_start_matches('#').trim();
    (first.chars().take(80).collect(), text.to_string())
}

fn scan(dir: &Path, out: &mut Vec<Slash>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")).collect();
    files.sort();
    for p in files {
        let Some(name) = p.file_stem().map(|s| s.to_string_lossy().to_string()) else { continue };
        if out.iter().any(|s| s.name == name) {
            continue; // project commands shadow personal ones; built-ins win
        }
        if let Ok(text) = std::fs::read_to_string(&p) {
            let (desc, body) = describe(&text);
            let hint = if body.contains("$ARGUMENTS") { "[args]".into() } else { String::new() };
            out.push(Slash { name, hint, desc: if desc.is_empty() { "Custom command".into() } else { desc }, body: Some(body) });
        }
    }
}

/// Built-ins plus custom commands for `cwd`.
pub fn load(cwd: &Path) -> Vec<Slash> {
    let mut all = builtins();
    scan(&zcore::project_path(cwd, "commands"), &mut all);
    scan(&zcore::cli_home().join("commands"), &mut all);
    scan(&cwd.join(".claude").join("commands"), &mut all);
    all
}

/// Fuzzy-rank `items` against `query` with nucleo; keeps the original order
/// for an empty query. Returns indices into `items`, best first.
pub fn rank<T>(items: &[T], query: &str, key: impl Fn(&T) -> String) -> Vec<usize> {
    let q = query.trim();
    if q.is_empty() {
        return (0..items.len()).collect();
    }
    let mut m = Matcher::new(Config::DEFAULT.match_paths());
    let pat = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            let k = key(it);
            pat.score(Utf32Str::new(&k, &mut buf), &mut m).map(|s| (s, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

pub fn filter<'a>(all: &'a [Slash], q: &str) -> Vec<&'a Slash> {
    rank(all, q, |s| s.name.clone()).into_iter().map(|i| &all[i]).collect()
}

/// If `input` starts with `/`, return (command token, args).
pub fn parse(input: &[char]) -> Option<(String, String)> {
    if input.first() != Some(&'/') {
        return None;
    }
    let s: String = input.iter().collect();
    let body = &s[1..];
    match body.find(char::is_whitespace) {
        Some(i) => Some((body[..i].to_string(), body[i + 1..].trim().to_string())),
        None => Some((body.to_string(), String::new())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_lists_all_in_order() {
        let all = builtins();
        let f = filter(&all, "");
        assert_eq!(f.len(), all.len());
        assert_eq!(f[0].name, "model");
    }

    #[test]
    fn fuzzy_finds_commands() {
        let all = builtins();
        assert_eq!(filter(&all, "mdl")[0].name, "model");
        assert_eq!(filter(&all, "prov")[0].name, "providers");
        assert_eq!(filter(&all, "ctx")[0].name, "context");
        assert_eq!(filter(&all, "res")[0].name, "resume");
        assert!(filter(&all, "zzzz").is_empty());
    }

    #[test]
    fn custom_commands_load_with_front_matter() {
        let _env = crate::env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("COSMOS_CLI_HOME", dir.path().join("home"));
        let cmds = dir.path().join(".claude").join("commands");
        std::fs::create_dir_all(&cmds).unwrap();
        std::fs::write(cmds.join("ship.md"), "---\ndescription: Tag and release\n---\nRelease version $ARGUMENTS now").unwrap();
        std::fs::write(cmds.join("model.md"), "shadowed by the built-in").unwrap();
        let all = load(dir.path());
        let ship = all.iter().find(|s| s.name == "ship").unwrap();
        assert_eq!(ship.desc, "Tag and release");
        assert_eq!(ship.hint, "[args]");
        assert_eq!(ship.body.as_deref(), Some("Release version $ARGUMENTS now"));
        assert!(all.iter().filter(|s| s.name == "model").all(|s| s.body.is_none()));
    }

    #[test]
    fn parse_cmd_and_args() {
        let chars: Vec<char> = "/mode plan".chars().collect();
        assert_eq!(parse(&chars), Some(("mode".into(), "plan".into())));
        assert!(parse(&"hi".chars().collect::<Vec<_>>()).is_none());
    }
}
