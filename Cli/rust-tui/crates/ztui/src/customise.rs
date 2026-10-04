//! `/customise` — ask any model to restyle the TUI while you keep working.
//!
//! A *runner* on its own thread holds its own conversation with the model
//! (its own session id and prompt cache — your main conversation never sees
//! it) and gets a deliberately narrow tool set: read and write a **draft**
//! of `ui.toml`, render a preview, save a theme, add a provider / model /
//! custom command, check a subscription. Every write goes through
//! `skin::schema::check`, so permissions, sandbox, approvals, hooks, MCP,
//! credentials and the customiser's own limits are out of reach in code,
//! not just in the prompt.
//!
//! Drafts land in `ui.draft.toml` next to `ui.toml`, with the `ui.toml` they
//! started from kept as `ui.draft.base.toml`. Applying is a per-key
//! three-way merge — if you changed a key by hand (or with `/theme`) while
//! the customiser worked, your value wins — then a normal validated write
//! with the backup ring, and the render loop swaps the new skin in between
//! frames.
//!
//! Subscription agents (Cursor, Devin, Codex over ACP) run their own tools,
//! so they are asked for the finished file instead, from a scratch folder
//! with nothing else in it; their answer goes through the same checks.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use ratatui_core::text::Line;
use serde_json::{json, Value};
use zproto::{Block, Chunk, Message, Role, ToolSpec, Usage};

use crate::skin::edit::{self, Val};
use crate::skin::{schema, Skin};

// ---- files ---------------------------------------------------------------------

/// Where the customiser reads and writes.
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub ui: PathBuf,
    pub draft: PathBuf,
    pub base: PathBuf,
}

impl Paths {
    pub fn new(home: &Path) -> Paths {
        Paths {
            home: home.to_path_buf(),
            ui: home.join("ui.toml"),
            draft: home.join("ui.draft.toml"),
            base: home.join("ui.draft.base.toml"),
        }
    }

    pub fn has_draft(&self) -> bool {
        self.draft.exists()
    }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

/// Start a draft from the current `ui.toml` (kept when one is already open).
pub fn begin(p: &Paths) -> Result<(), String> {
    if p.draft.exists() {
        return Ok(());
    }
    let cur = read(&p.ui);
    std::fs::create_dir_all(&p.home).map_err(|e| e.to_string())?;
    std::fs::write(&p.base, &cur).map_err(|e| e.to_string())?;
    std::fs::write(&p.draft, &cur).map_err(|e| e.to_string())
}

/// Carry the operator's `[customise]` table from `from` onto `onto` — the
/// customiser never sees, writes or drops it.
fn keep_leash(from: &str, onto: &str) -> String {
    let mut out = edit::remove_table(onto, "customise");
    for ((t, k), v) in edit::flatten(from) {
        if t == "customise" {
            if let Some(v) = Val::from_toml(&v) {
                out = edit::set(&out, &t, &k, &v);
            }
        }
    }
    out
}

/// Drop the draft when it doesn't change anything. True when one remains.
pub fn tidy(p: &Paths) -> bool {
    if p.draft.exists() && changes(p).is_empty() {
        discard(p);
    }
    p.draft.exists()
}

pub fn discard(p: &Paths) {
    let _ = std::fs::remove_file(&p.draft);
    let _ = std::fs::remove_file(&p.base);
}

fn show(v: &toml::Value) -> String {
    Val::from_toml(v).map(|x| x.toml()).unwrap_or_else(|| "(table)".into())
}

fn path_of(table: &str, key: &str) -> String {
    if table.is_empty() {
        key.to_string()
    } else {
        format!("{table}.{key}")
    }
}

/// What the draft changes, one `path: old → new` per line.
pub fn changes(p: &Paths) -> Vec<String> {
    if !p.draft.exists() {
        return Vec::new();
    }
    let base = edit::flatten(&read(&p.base));
    let draft = edit::flatten(&read(&p.draft));
    let mut out = Vec::new();
    for (k, v) in &draft {
        match base.get(k) {
            Some(b) if b == v => {}
            Some(b) => out.push(format!("{}: {} → {}", path_of(&k.0, &k.1), show(b), show(v))),
            None => out.push(format!("{}: {}", path_of(&k.0, &k.1), show(v))),
        }
    }
    for k in base.keys().filter(|k| !draft.contains_key(*k)) {
        out.push(format!("{}: (back to default)", path_of(&k.0, &k.1)));
    }
    out
}

/// The outcome of applying a draft.
#[derive(Debug, Default, PartialEq)]
pub struct Applied {
    pub changed: Vec<String>,
    /// keys you changed meanwhile — yours kept
    pub kept: Vec<String>,
}

/// Three-way merge of the draft into `ui.toml`, then a validated write.
pub fn apply(p: &Paths) -> Result<Applied, String> {
    if !p.draft.exists() {
        return Err("no draft to apply".into());
    }
    let base_text = read(&p.base);
    let ours_text = read(&p.ui);
    let theirs_text = read(&p.draft);
    let (base, ours, theirs) = (edit::flatten(&base_text), edit::flatten(&ours_text), edit::flatten(&theirs_text));
    let mut text = ours_text.clone();
    let mut out = Applied::default();
    let keys: std::collections::BTreeSet<_> = base.keys().chain(ours.keys()).chain(theirs.keys()).cloned().collect();
    for k in keys {
        let (b, o, t) = (base.get(&k), ours.get(&k), theirs.get(&k));
        if t == b || o == t {
            continue; // the draft didn't touch it, or you both agree
        }
        let path = path_of(&k.0, &k.1);
        if o != b {
            out.kept.push(path); // you changed it too: yours wins
            continue;
        }
        text = match t.and_then(Val::from_toml) {
            Some(v) => edit::set(&text, &k.0, &k.1, &v),
            None => edit::remove(&text, &k.0, &k.1),
        };
        out.changed.push(path);
    }
    if !out.changed.is_empty() {
        edit::write(&p.ui, &text)?;
    }
    discard(p);
    Ok(out)
}

// ---- tools ---------------------------------------------------------------------

fn spec(name: &str, description: &str, schema: Value) -> ToolSpec {
    ToolSpec { name: name.into(), description: description.into(), input_schema: schema }
}

/// The only tools a customiser gets.
pub fn tools() -> Vec<ToolSpec> {
    let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
    vec![
        spec("ui_get", "Read the draft ui.toml (whole file, or one path like \"spinner.style\").", obj(json!({"path": {"type": "string"}}), &[])),
        spec(
            "ui_set",
            "Change the draft. Either {path, value} or {changes: [{path, value}, …]}. Paths and values must match the schema; bad ones are refused with the reason.",
            obj(
                json!({
                    "path": {"type": "string"},
                    "value": {},
                    "changes": {"type": "array", "items": {"type": "object", "properties": {"path": {"type": "string"}, "value": {}}, "required": ["path", "value"]}}
                }),
                &[],
            ),
        ),
        spec("ui_preview", "Render a sample screen (busy line, tool calls, diff, markdown, buddy) in the draft's look, as text, plus its colours.", obj(json!({}), &[])),
        spec("ui_apply", "Apply the draft now. Only works when the operator turned on auto_apply; otherwise they review it with ctrl+y.", obj(json!({}), &[])),
        spec("ui_reset", "Drop a section from the draft (back to defaults), or the whole draft when no section is given.", obj(json!({"section": {"type": "string"}}), &[])),
        spec("theme_save", "Save the draft's theme + colour overrides as a named theme (lowercase-dashes) and switch the draft to it.", obj(json!({"name": {"type": "string"}}), &["name"])),
        spec(
            "provider_add",
            "Add an OpenAI/Anthropic/Google-compatible provider to config.toml. api_key_env is the NAME of an environment variable; keys themselves are added by the operator with /connect.",
            obj(
                json!({
                    "id": {"type": "string"},
                    "kind": {"type": "string", "enum": ["openai", "anthropic", "google", "responses"]},
                    "base_url": {"type": "string"},
                    "api_key_env": {"type": "string"}
                }),
                &["id", "kind", "base_url"],
            ),
        ),
        spec(
            "model_add",
            "Add a model alias to config.toml (it shows up in /model). price is USD per 1M tokens.",
            obj(
                json!({
                    "alias": {"type": "string"},
                    "provider": {"type": "string"},
                    "id": {"type": "string"},
                    "context": {"type": "integer"},
                    "price": {"type": "object", "properties": {"input": {"type": "number"}, "output": {"type": "number"}, "cache_read": {"type": "number"}, "cache_write": {"type": "number"}}}
                }),
                &["alias", "provider", "id"],
            ),
        ),
        spec("subscription_enable", "Check a subscription agent (cursor, devin, codex): installed or not, and how the operator signs in.", obj(json!({"name": {"type": "string", "enum": ["cursor", "devin", "codex"]}}), &["name"])),
        spec(
            "command_add",
            "Add a custom slash command (~/.cosmos-cli/commands/<name>.md). body is the prompt; $ARGUMENTS is replaced by what follows the command.",
            obj(json!({"name": {"type": "string"}, "description": {"type": "string"}, "body": {"type": "string"}, "replace": {"type": "boolean"}}), &["name", "body"]),
        ),
    ]
}

/// Side effects the UI needs to know about.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Draft,
    Apply,
    Config,
    Command,
}

/// Run one customiser tool. Returns (result text, is_error, effect).
pub fn exec(p: &Paths, auto_apply: bool, name: &str, input: &Value) -> (String, bool, Option<Effect>) {
    match run_tool(p, auto_apply, name, input) {
        Ok((s, e)) => (s, false, e),
        Err(e) => (e, true, None),
    }
}

fn str_arg<'a>(input: &'a Value, k: &str) -> Result<&'a str, String> {
    input[k].as_str().map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| format!("missing {k}"))
}

fn run_tool(p: &Paths, auto_apply: bool, name: &str, input: &Value) -> Result<(String, Option<Effect>), String> {
    match name {
        "ui_get" => {
            begin(p)?;
            let text = read(&p.draft);
            match input["path"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
                None => Ok((if text.trim().is_empty() { "(the draft is empty — everything is at its default)".into() } else { text }, None)),
                Some(path) => {
                    let skin = crate::skin::parse(&text)?;
                    let mut v = serde_json::to_value(skin).map_err(|e| e.to_string())?;
                    for seg in path.split('.') {
                        v = v.get(seg).cloned().unwrap_or(Value::Null);
                    }
                    Ok((format!("{path} = {v}"), None))
                }
            }
        }
        "ui_set" => {
            begin(p)?;
            let mut list: Vec<(String, Value)> = Vec::new();
            if let Some(arr) = input["changes"].as_array() {
                for c in arr {
                    list.push((c["path"].as_str().unwrap_or("").to_string(), c["value"].clone()));
                }
            }
            if let Some(path) = input["path"].as_str() {
                list.push((path.to_string(), input["value"].clone()));
            }
            if list.is_empty() {
                return Err("give {path, value} or {changes: [...]}".into());
            }
            let mut text = read(&p.draft);
            let mut done = Vec::new();
            let mut refused = Vec::new();
            for (path, value) in list {
                match schema::check(&path, &value) {
                    Ok((table, key, val)) => {
                        let next = edit::set(&text, &table, &key, &val);
                        // the whole draft must still load, colours must resolve
                        match crate::skin::parse(&next) {
                            Ok(sk) if sk.palette().1.is_empty() => {
                                text = next;
                                done.push(format!("{path} = {}", val.toml()));
                            }
                            Ok(sk) => refused.push(format!("{path}: {}", sk.palette().1.join("; "))),
                            Err(e) => refused.push(format!("{path}: {e}")),
                        }
                    }
                    Err(e) => refused.push(e),
                }
            }
            if !done.is_empty() {
                std::fs::write(&p.draft, &text).map_err(|e| e.to_string())?;
            }
            let mut msg = String::new();
            if !done.is_empty() {
                msg.push_str(&format!("set in the draft: {}", done.join("; ")));
            }
            if !refused.is_empty() {
                if !msg.is_empty() {
                    msg.push('\n');
                }
                msg.push_str(&format!("refused: {}", refused.join("\n  ")));
            }
            if done.is_empty() {
                return Err(msg);
            }
            Ok((msg, Some(Effect::Draft)))
        }
        "ui_preview" => {
            begin(p)?;
            let skin = crate::skin::parse(&read(&p.draft))?;
            let lines = preview(&skin, 72);
            // plain text for the model: no invisible hyperlink markers
            let plain = |s: &str| s.chars().filter(|c| !('\u{E0000}'..='\u{E007F}').contains(c)).collect::<String>();
            let text: Vec<String> = lines.iter().map(|l| l.spans.iter().map(|s| plain(&s.content)).collect()).collect();
            let pal = skin.palette().0;
            let colours: Vec<String> = ["text", "accent", "success", "warn", "error", "dim", "code", "link", "heading"]
                .iter()
                .filter_map(|t| pal.get(t).map(|c| format!("{t} {}", crate::skin::palette::show_color(c))))
                .collect();
            Ok((format!("{}\n\ncolours: {}", text.join("\n"), colours.join(", ")), None))
        }
        "ui_apply" => {
            if !auto_apply {
                return Err("not applied — the operator reviews drafts (ctrl+y applies, ctrl+n discards). Tell them what you changed.".into());
            }
            Ok(("applying".into(), Some(Effect::Apply)))
        }
        "ui_reset" => {
            begin(p)?;
            let text = match input["section"].as_str().map(str::trim).filter(|s| !s.is_empty()) {
                // everything back to defaults — except the operator's own limits
                None => keep_leash(&read(&p.draft), ""),
                Some(s) if schema::SECTIONS.contains(&s) => edit::remove_table(&read(&p.draft), s),
                Some(s) => return Err(format!("unknown section {s:?} (sections: {})", schema::SECTIONS.join(", "))),
            };
            std::fs::write(&p.draft, &text).map_err(|e| e.to_string())?;
            Ok(("reset in the draft".into(), Some(Effect::Draft)))
        }
        "theme_save" => {
            begin(p)?;
            let name = str_arg(input, "name")?;
            if !crate::skin::valid_name(name) || crate::skin::palette::THEMES.contains(&name) {
                return Err(format!("{name:?}: use lowercase letters, digits and dashes, and not a built-in theme name"));
            }
            let text = read(&p.draft);
            let skin = crate::skin::parse(&text)?;
            let mut saved = match crate::skin::saved_theme(&skin.theme.name) {
                Some(t) => t,
                None => crate::skin::SavedTheme { base: skin.theme.name.clone(), colors: Default::default() },
            };
            saved.colors.extend(skin.theme.colors.clone());
            let dir = crate::skin::themes_dir();
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let mut body = edit::set("", "", "base", &Val::Str(saved.base.clone()));
            for (k, v) in &saved.colors {
                body = edit::set(&body, "colors", k, &Val::Str(v.clone()));
            }
            std::fs::write(dir.join(format!("{name}.toml")), body).map_err(|e| e.to_string())?;
            let text = edit::remove_table(&text, "theme.colors");
            let text = edit::set(&text, "theme", "name", &Val::Str(name.into()));
            std::fs::write(&p.draft, text).map_err(|e| e.to_string())?;
            Ok((format!("saved theme {name:?} (base {}) and switched the draft to it", saved.base), Some(Effect::Draft)))
        }
        "provider_add" => provider_add(p, input).map(|s| (s, Some(Effect::Config))),
        "model_add" => model_add(p, input).map(|s| (s, Some(Effect::Config))),
        "subscription_enable" => {
            let id = str_arg(input, "name")?;
            let plugin = zcore::acp_client::plugin(id).ok_or_else(|| format!("{id:?}: one of cursor, devin, codex"))?;
            Ok((
                if zcore::acp_client::installed(id) {
                    format!("{} is installed: {}. The operator picks it in /model.", plugin.name, zcore::acp_client::status(id))
                } else {
                    format!("{} isn't installed yet. Tell the operator: {} — then it appears in /model.", plugin.name, plugin.install)
                },
                None,
            ))
        }
        "command_add" => command_add(p, input).map(|s| (s, Some(Effect::Command))),
        other => Err(format!("{other}: not a customise tool")),
    }
}

fn config_path(p: &Paths) -> PathBuf {
    p.home.join("config.toml")
}

/// Append a table to config.toml after checking the result still parses
/// and nothing but that one table changed.
fn append_config(p: &Paths, table: &str, name: &str, body: &str) -> Result<(), String> {
    let path = config_path(p);
    let cur = read(&path);
    let parsed: toml::Table = toml::from_str(&cur).map_err(|e| format!("config.toml doesn't parse: {}", e.to_string().trim()))?;
    if parsed.get(table).and_then(|t| t.get(name)).is_some() {
        return Err(format!("{table}.{name} already exists in config.toml"));
    }
    let snippet = format!("\n[{table}.{}]\n{body}", toml_key(name));
    let check: toml::Table = toml::from_str(&snippet).map_err(|e| e.to_string())?;
    if check.len() != 1 || check.get(table).and_then(|t| t.as_table()).map(|t| t.len()) != Some(1) {
        return Err("refused: the entry would touch more than its own table".into());
    }
    let next = format!("{}{snippet}", cur.trim_end_matches('\n').to_string() + if cur.is_empty() { "" } else { "\n" });
    toml::from_str::<toml::Table>(&next).map_err(|e| e.to_string())?;
    if path.exists() {
        let _ = std::fs::copy(&path, path.with_extension("toml.bak"));
    }
    std::fs::write(&path, next).map_err(|e| e.to_string())
}

fn toml_key(k: &str) -> String {
    if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        k.to_string()
    } else {
        format!("{k:?}")
    }
}

fn provider_add(p: &Paths, input: &Value) -> Result<String, String> {
    let id = str_arg(input, "id")?;
    let kind = str_arg(input, "kind")?;
    let url = str_arg(input, "base_url")?;
    let env = input["api_key_env"].as_str().unwrap_or("").trim();
    if !(1..=32).contains(&id.len()) || !id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
        return Err(format!("{id:?}: ids are lowercase letters, digits, - and _"));
    }
    if !["openai", "anthropic", "google", "responses"].contains(&kind) {
        return Err(format!("{kind:?}: kind is openai, anthropic, google or responses (subscriptions are built in — see subscription_enable)"));
    }
    let local = ["http://localhost", "http://127.0.0.1", "http://[::1]"].iter().any(|h| url.starts_with(h));
    if !url.starts_with("https://") && !local {
        return Err("base_url must be https:// (or http://localhost for a local server)".into());
    }
    if url.chars().any(|c| c.is_whitespace() || c == '"') {
        return Err("base_url has spaces or quotes".into());
    }
    let looks_like_name = env.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && env.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && env.len() <= 64;
    if !env.is_empty() && !looks_like_name {
        return Err("api_key_env is the NAME of an environment variable (like GROQ_API_KEY) — never a key. The operator adds keys with /connect.".into());
    }
    let mut body = format!("kind = {kind:?}\nbase_url = {url:?}\n");
    if !env.is_empty() {
        body.push_str(&format!("api_key_env = {env:?}\n"));
    }
    append_config(p, "providers", id, &body)?;
    Ok(format!("added provider {id} ({kind}, {url}){}", if env.is_empty() { String::new() } else { format!(" — the operator sets {env} or runs /connect") }))
}

fn model_add(p: &Paths, input: &Value) -> Result<String, String> {
    let alias = str_arg(input, "alias")?;
    let provider = str_arg(input, "provider")?;
    let id = str_arg(input, "id")?;
    if !(1..=48).contains(&alias.len()) || !alias.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.')) {
        return Err(format!("{alias:?}: aliases are lowercase letters, digits, - _ ."));
    }
    let cfg: toml::Table = toml::from_str(&read(&config_path(p))).unwrap_or_default();
    let known = cfg.get("providers").and_then(|t| t.get(provider)).is_some() || zcore::catalog::provider(provider).is_some();
    if !known {
        return Err(format!("no provider {provider:?} — add it with provider_add first"));
    }
    if id.chars().any(|c| c.is_control() || c == '"') {
        return Err("bad model id".into());
    }
    let mut body = format!("provider = {provider:?}\nid = {id:?}\n");
    if let Some(c) = input["context"].as_u64() {
        body.push_str(&format!("context = {}\n", c.clamp(1024, 10_000_000)));
    }
    if let Some(pr) = input["price"].as_object() {
        let num = |k: &str| pr.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0 && *v < 10_000.0);
        let parts: Vec<String> = ["input", "cache_read", "cache_write", "output"].iter().filter_map(|k| num(k).map(|v| format!("{k} = {v}"))).collect();
        if !parts.is_empty() {
            body.push_str(&format!("price = {{ {} }}\n", parts.join(", ")));
        }
    }
    append_config(p, "models", alias, &body)?;
    Ok(format!("added model {alias} → {provider}/{id}; it's in /model now"))
}

fn command_add(p: &Paths, input: &Value) -> Result<String, String> {
    let name = str_arg(input, "name")?.trim_start_matches('/');
    if !crate::skin::valid_name(name) {
        return Err(format!("{name:?}: command names are lowercase letters, digits and dashes"));
    }
    if crate::slash::builtins().iter().any(|s| s.name == name) {
        return Err(format!("/{name} is a built-in command"));
    }
    let body = input["body"].as_str().unwrap_or("").trim();
    if body.is_empty() || body.len() > 8000 {
        return Err("body must be 1–8000 characters".into());
    }
    let dir = p.home.join("commands");
    let file = dir.join(format!("{name}.md"));
    if file.exists() && !input["replace"].as_bool().unwrap_or(false) {
        return Err(format!("/{name} already exists — pass replace: true to overwrite it"));
    }
    let desc = input["description"].as_str().unwrap_or("").replace(['\n', '\r'], " ");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = if desc.trim().is_empty() { format!("{body}\n") } else { format!("---\ndescription: {}\n---\n{body}\n", desc.trim()) };
    std::fs::write(&file, text).map_err(|e| e.to_string())?;
    Ok(format!("added /{name}"))
}

// ---- preview -------------------------------------------------------------------

/// A sample screen in `skin`'s look: busy line, tool calls, a diff,
/// markdown and the buddy. Rendered on the calling thread only.
pub fn preview(skin: &Skin, w: usize) -> Vec<Line<'static>> {
    use crate::toolview::{render, ToolView};
    use ratatui_core::text::Span;
    let skin = skin.clone();
    crate::skin::scoped_skin(skin.clone(), || {
        let pal = crate::skin::palette();
        let mut out: Vec<Line<'static>> = Vec::new();
        let verb = crate::motion::pick_verb(&skin.spinner, 0).0;
        let mut busy = vec![Span::raw(" "), crate::motion::spinner_span(&skin.spinner, pal.accent, 700, true)];
        busy.extend(crate::motion::shimmer(&format!("{verb}…"), pal.accent, 700, skin.spinner.shimmer));
        busy.push(Span::styled(format!(" (12s · {})", skin.spinner.hint), crate::theme::DIM()));
        out.push(Line::from(busy));
        out.push(Line::default());
        let read_in = json!({"path": "src/main.rs"});
        out.extend(render(&ToolView { name: "read", input: &read_in, content: Some("a\nb\nc\n"), err: false, diff: None, interrupted: false, elapsed: Some(std::time::Duration::from_millis(200)) }, w));
        let bash_in = json!({"command": "cargo test"});
        let bash = "exit_code=0 log_path=x omitted_chars=0\n<<<UNTRUSTED_SHELL\ntest result: ok. 42 passed\n>>>";
        out.extend(render(&ToolView { name: "bash", input: &bash_in, content: Some(bash), err: false, diff: None, interrupted: false, elapsed: Some(std::time::Duration::from_millis(3400)) }, w));
        let d = crate::worker::Diff {
            path: "src/main.rs".into(),
            added: 1,
            removed: 1,
            created: false,
            lines: vec![
                crate::worker::DiffLine { kind: ' ', no: Some(1), old: Some(1), text: "fn main() {".into() },
                crate::worker::DiffLine { kind: '-', no: Some(2), old: Some(2), text: "    println!(\"hi\");".into() },
                crate::worker::DiffLine { kind: '+', no: Some(2), old: None, text: "    println!(\"hello, cosmos\");".into() },
            ],
        };
        let edit_in = json!({"path": "src/main.rs"});
        out.extend(render(&ToolView { name: "edit", input: &edit_in, content: Some("ok"), err: false, diff: Some(&d), interrupted: false, elapsed: None }, w));
        out.push(Line::default());
        out.extend(crate::markdown::render("## Preview\nSome **bold**, `code` and a [link](https://example.com).\n- a list\n> [!TIP]\n> Like it? ctrl+y applies.", w));
        if skin.buddy.enabled {
            let id = crate::buddy::hatch("preview", &skin.buddy);
            out.push(Line::default());
            let pose = crate::pixel::Pose { eyes: crate::pixel::Eyes::Happy, dy: 1, extras: Default::default() };
            for row in crate::buddy::sprite(&id, &skin.buddy, pose) {
                let mut l = vec![Span::raw(" ")];
                l.extend(row);
                out.push(Line::from(l));
            }
        }
        out
    })
}

// ---- the picker (no model needed) ---------------------------------------------

/// `/customise` with nothing after it: the settings people change most,
/// each cycled with ←/→ and written straight to ui.toml.
pub const PICKER: &[(&str, &str)] = &[
    ("Theme", "theme.name"),
    ("Spinner", "spinner.style"),
    ("Tool calls", "tools.style"),
    ("Diffs", "diff.layout"),
    ("Markdown", "markdown.style"),
    ("Links", "markdown.links"),
    ("Buddy", "buddy.size"),
    ("Species", "buddy.species"),
    ("Motion", "motion.reduce"),
    ("Notifications", "notify.desktop"),
    ("Chime", "notify.sound"),
    ("Turn footer", "spinner.footer"),
];

fn picker_options(path: &str) -> Vec<Value> {
    let s = |v: &[&str]| v.iter().map(|x| json!(x)).collect();
    match path {
        "theme.name" => crate::skin::themes().into_iter().map(Value::String).collect(),
        "spinner.style" => s(crate::motion::STYLES),
        "tools.style" | "markdown.style" => s(&["cosmos", "classic"]),
        "diff.layout" => s(&["auto", "unified", "split"]),
        "markdown.links" => s(&["auto", "osc8", "inline"]),
        "buddy.size" => s(&["full", "mini", "off"]),
        "buddy.species" => std::iter::once(json!("")).chain(crate::pixel::SPECIES.iter().map(|s| json!(s.name))).collect(),
        "notify.desktop" => s(&["unfocused", "always", "never"]),
        _ => vec![json!(false), json!(true)],
    }
}

/// How a picker value reads.
pub fn picker_shown(path: &str, v: &Value) -> String {
    match (path, v) {
        ("buddy.species", Value::String(s)) if s.is_empty() => "hatched".into(),
        ("motion.reduce", Value::Bool(b)) => if *b { "reduced" } else { "on" }.into(),
        (_, Value::Bool(b)) => if *b { "on" } else { "off" }.into(),
        (_, Value::String(s)) => s.clone(),
        (_, other) => other.to_string(),
    }
}

/// The current value of `path` in `skin`.
pub fn picker_value(skin: &Skin, path: &str) -> Value {
    let mut v = serde_json::to_value(skin).unwrap_or(Value::Null);
    for seg in path.split('.') {
        v = v.get(seg).cloned().unwrap_or(Value::Null);
    }
    v
}

/// Move row `i` one step (`dir` = ±1) and save it to `ui`.
pub fn picker_step(ui: &Path, i: usize, dir: i32) -> Result<String, String> {
    let (label, path) = PICKER.get(i).ok_or("no such row")?;
    let opts = picker_options(path);
    let cur = picker_value(&crate::skin::current(), path);
    let at = opts.iter().position(|o| *o == cur).unwrap_or(0) as i32;
    let next = &opts[(at + dir).rem_euclid(opts.len() as i32) as usize];
    let (table, key, val) = schema::check(path, next)?;
    edit::set_in_file(ui, &table, &key, &val)?;
    Ok(format!("{label} → {}", picker_shown(path, next)))
}

// ---- the runner -----------------------------------------------------------------

const PROMPT: &str = "You are the customiser inside cosmostui, a terminal coding assistant. The operator asked you to change how cosmostui looks or behaves. You work on a DRAFT of ~/.cosmos-cli/ui.toml through tools only; the operator keeps working while you do and applies your draft with ctrl+y.

How to work:
- Plan briefly, then make the changes with ui_set (batch them with `changes`). Paths and values must match the schema below — refused writes tell you why; fix and retry.
- Use ui_preview to check the result, then finish with a short summary of what changed (2–4 lines). Don't ask questions back; make tasteful choices.
- Colours: keep text readable on a dark terminal (or light for the light theme); prefer theme.name plus a few theme.colors overrides.
- Verbs are \"Present:Past\" pairs, e.g. \"Warping:Warped\".
- To add a model provider use provider_add then model_add; API keys are NEVER yours to handle — the operator adds them with /connect.

Hard boundaries (enforced in code — don't try): permissions, sandbox, approval/ask mode, hooks, MCP servers, credentials, self-improvement and your own [customise] limits.";

/// What the runner tells the UI.
#[derive(Debug)]
pub enum Up {
    /// one line for the status strip
    Status(String),
    /// one line for `/customise log`
    Log(String),
    Effect(Effect),
    /// cost of the last model call, when the price is known
    Spent(f64),
    /// finished: the model's closing words
    Done(String),
    Failed(String),
}

pub struct Runner {
    pub rx: Receiver<Up>,
    cancel: Arc<AtomicBool>,
    pub started: Instant,
    pub request: String,
    pub model: String,
}

impl Runner {
    pub fn start(request: String, model: String, p: Paths, auto_apply: bool, max_turns: u32, max_usd: f64) -> Runner {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (c, req, m) = (cancel.clone(), request.clone(), model.clone());
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&req, &m, &p, auto_apply, max_turns, max_usd, &c, &tx)));
            let msg = match r {
                Ok(Ok(s)) => Up::Done(s),
                Ok(Err(e)) => Up::Failed(e),
                Err(_) => Up::Failed("the customiser crashed".into()),
            };
            let _ = tx.send(msg);
        });
        Runner { rx, cancel, started: Instant::now(), request, model }
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// `ui_set {theme.name: "nebula"}` — what a call is doing, for the strip.
pub fn describe(name: &str, input: &Value) -> String {
    match name {
        "ui_set" => {
            let mut parts: Vec<String> = input["changes"].as_array().map(|a| a.iter().map(|c| format!("{} = {}", c["path"].as_str().unwrap_or("?"), c["value"])).collect()).unwrap_or_default();
            if let Some(p) = input["path"].as_str() {
                parts.push(format!("{p} = {}", input["value"]));
            }
            format!("setting {}", crate::wrap::ellipsize(&parts.join(", "), 80))
        }
        "ui_get" => "reading the draft".into(),
        "ui_preview" => "previewing".into(),
        "ui_apply" => "applying".into(),
        "ui_reset" => format!("resetting {}", input["section"].as_str().unwrap_or("the draft")),
        "theme_save" => format!("saving theme {}", input["name"].as_str().unwrap_or("?")),
        "provider_add" => format!("adding provider {}", input["id"].as_str().unwrap_or("?")),
        "model_add" => format!("adding model {}", input["alias"].as_str().unwrap_or("?")),
        "subscription_enable" => format!("checking {}", input["name"].as_str().unwrap_or("?")),
        "command_add" => format!("adding /{}", input["name"].as_str().unwrap_or("?")),
        other => other.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
fn run(request: &str, alias: &str, p: &Paths, auto_apply: bool, max_turns: u32, max_usd: f64, cancel: &AtomicBool, tx: &Sender<Up>) -> Result<String, String> {
    let cfg = zcore::config::Config::load(&p.home);
    let (mut provider, model_id) = cfg.provider_for(alias).ok_or_else(|| format!("no model {alias:?} — pick one with /customise model <alias>"))?;
    begin(p)?;
    if provider.kind == zproviders::ProviderKind::Acp {
        return run_acp(request, alias, p, cancel, tx);
    }
    let _ = tx.send(Up::Status(format!("thinking with {alias}")));
    let session = format!("customise-{}", zproto::new_id());
    let system = vec![
        PROMPT.to_string(),
        format!("## ui.toml schema\n{}", schema::text()),
        format!("## The draft right now\n{}", { let t = read(&p.draft); if t.trim().is_empty() { "(empty — all defaults)".to_string() } else { t } }),
    ];
    let specs = tools();
    let allowed = cfg.allowed_hosts();
    let mut messages = vec![Message::text(Role::User, request)];
    let mut spent = 0.0;
    for turn in 0..max_turns.max(1) {
        if cancel.load(Ordering::Relaxed) {
            return Ok("stopped".into());
        }
        if provider.kind == zproviders::ProviderKind::ChatGpt {
            if let Some(t) = zcore::chatgpt::fresh() {
                provider.api_key = Some(t.access);
                provider.account_id = Some(t.account_id);
            }
        }
        let req = zproviders::Request { system: system.clone(), tools: &specs, messages: &messages, session_id: &session, max_tokens: 2048 };
        let mut text = String::new();
        let mut calls: Vec<Block> = Vec::new();
        let mut usage = Usage::default();
        let mut err = None;
        zproviders::complete(
            &provider,
            &req,
            &mut |c| match c {
                Chunk::Text(t) => text.push_str(&t),
                Chunk::ToolUse { id, name, input } => calls.push(Block::ToolUse { id, name, input }),
                Chunk::Done(u) => {
                    usage.input += u.input;
                    usage.cache_read += u.cache_read;
                    usage.cache_write += u.cache_write;
                    usage.output += u.output;
                }
                Chunk::Err(e) => err = Some(e),
            },
            &allowed,
        )
        .map_err(|e| format!("{model_id}: {e}"))?;
        if let Some(e) = err {
            return Err(format!("{model_id}: {e}"));
        }
        if let Some(c) = provider.prices.cost(&usage) {
            spent += c;
            let _ = tx.send(Up::Spent(c));
        }
        let mut blocks = Vec::new();
        if !text.trim().is_empty() {
            let _ = tx.send(Up::Log(format!("{alias}: {}", text.trim())));
            blocks.push(Block::Text { text: text.clone() });
        }
        blocks.extend(calls.iter().cloned());
        if !blocks.is_empty() {
            messages.push(Message { role: Role::Assistant, content: blocks });
        }
        if calls.is_empty() {
            save_log(p, &session, &messages);
            return Ok(text.trim().to_string());
        }
        let mut results = Vec::new();
        for c in calls {
            let Block::ToolUse { id, name, input } = c else { continue };
            if cancel.load(Ordering::Relaxed) {
                return Ok("stopped".into());
            }
            let what = describe(&name, &input);
            let _ = tx.send(Up::Status(what.clone()));
            let (out, is_error, effect) = exec(p, auto_apply, &name, &input);
            let summary = crate::wrap::ellipsize(&out.lines().map(str::trim).collect::<Vec<_>>().join(" · "), 300);
            let _ = tx.send(Up::Log(format!("{} {what} — {summary}", if is_error { "✗" } else { "✓" })));
            if let Some(e) = effect {
                let _ = tx.send(Up::Effect(e));
            }
            results.push(Block::ToolResult { tool_use_id: id, content: out, is_error });
        }
        messages.push(Message { role: Role::User, content: results });
        if spent > max_usd {
            save_log(p, &session, &messages);
            return Ok(format!("stopped at the ${max_usd:.2} budget after {} turns", turn + 1));
        }
    }
    save_log(p, &session, &messages);
    Ok(format!("stopped after {max_turns} turns (the limit)"))
}

/// Keep the customiser's conversation next to the sessions (`/customise log`
/// shows the live one; this is the record).
fn save_log(p: &Paths, session: &str, messages: &[Message]) {
    let dir = p.home.join("customise");
    if std::fs::create_dir_all(&dir).is_ok() {
        if let Ok(s) = serde_json::to_string_pretty(messages) {
            let _ = std::fs::write(dir.join(format!("{session}.json")), s);
        }
    }
}

/// The first ```toml block in `text` (or None).
pub fn toml_block(text: &str) -> Option<String> {
    let start = text.find("```toml")?;
    let body = &text[start + 7..];
    let body = body.strip_prefix('\n').or_else(|| body.strip_prefix("\r\n")).unwrap_or(body);
    let end = body.find("```")?;
    Some(body[..end].to_string())
}

/// Subscription agents: ask for the finished file from an empty scratch
/// folder, then check it like any other draft.
fn run_acp(request: &str, alias: &str, p: &Paths, cancel: &AtomicBool, tx: &Sender<Up>) -> Result<String, String> {
    let id = zproto::new_id();
    let scratch = p.home.join("customise").join(format!("scratch-{id}"));
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    // removed however this returns (errors and stops included)
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(scratch.clone());
    // the agent sees the look only; the operator's [customise] limits stay here
    std::fs::write(scratch.join("ui.toml"), edit::remove_table(&read(&p.draft), "customise")).map_err(|e| e.to_string())?;
    std::fs::write(scratch.join("SCHEMA.md"), schema::text()).map_err(|e| e.to_string())?;
    let _ = tx.send(Up::Status(format!("asking {alias}")));
    let opts = zproto::RunOpts { model: Some(alias.to_string()), cwd: Some(scratch.display().to_string()), ..Default::default() };
    let mut agent = zcore::agent::Agent::new(&opts, "cosmostui-customise", &scratch).map_err(|e| e.to_string())?;
    let flag = Arc::new(AtomicBool::new(false));
    agent.interrupt = Some(flag.clone());
    let prompt = format!(
        "{PROMPT}\n\nThis folder holds the current draft (ui.toml) and its schema (SCHEMA.md). Don't edit files or run commands. Reply with a short summary and then the COMPLETE new ui.toml in one ```toml block.\n\nRequest: {request}"
    );
    agent.send_user(&prompt, &mut |_| {});
    for _ in 0..4 {
        if cancel.load(Ordering::Relaxed) {
            flag.store(true, Ordering::Relaxed);
            return Ok("stopped".into());
        }
        match agent.turn(&mut |_| {}) {
            Ok(true) => break,
            Ok(false) => continue,
            Err(e) => return Err(format!("{alias}: {e}")),
        }
    }
    let reply = agent
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant)
        .map(|m| m.content.iter().filter_map(|b| if let Block::Text { text } = b { Some(text.as_str()) } else { None }).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    let file = toml_block(&reply).ok_or_else(|| format!("{alias} didn't reply with a ```toml block"))?;
    schema::check_text(&file).map_err(|e| format!("{alias}'s file was refused: {e}"))?;
    let skin = crate::skin::parse(&file)?;
    let warn = skin.palette().1;
    if !warn.is_empty() {
        return Err(format!("{alias}'s file was refused: {}", warn.join("; ")));
    }
    std::fs::write(&p.draft, keep_leash(&read(&p.draft), &file)).map_err(|e| e.to_string())?;
    let _ = tx.send(Up::Effect(Effect::Draft));
    Ok(reply.split("```toml").next().unwrap_or("").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> (tempfile::TempDir, Paths) {
        let d = tempfile::tempdir().unwrap();
        let p = Paths::new(d.path());
        (d, p)
    }

    #[test]
    fn set_writes_the_draft_and_refuses_boundaries() {
        let (_d, p) = home();
        let (out, err, eff) = exec(&p, false, "ui_set", &json!({"changes": [
            {"path": "theme.name", "value": "nebula"},
            {"path": "spinner.style", "value": "comet"},
            {"path": "sandbox.mode", "value": "full"},
            {"path": "theme.colors.accent", "value": "#ff00aa"}
        ]}));
        assert!(!err, "{out}");
        assert_eq!(eff, Some(Effect::Draft));
        assert!(out.contains("refused: sandbox.mode: that's a hard boundary"), "{out}");
        let draft = std::fs::read_to_string(&p.draft).unwrap();
        assert!(draft.contains("name = \"nebula\"") && draft.contains("style = \"comet\"") && draft.contains("accent = \"#ff00aa\""), "{draft}");
        assert!(!draft.contains("sandbox"));
        assert!(!p.ui.exists(), "ui.toml is untouched until applied");
        let (out, err, _) = exec(&p, false, "ui_set", &json!({"path": "permissions.allow", "value": ["bash"]}));
        assert!(err && out.contains("hard boundary"), "{out}");
        let (out, err, _) = exec(&p, false, "ui_apply", &json!({}));
        assert!(err && out.contains("ctrl+y"));
        assert_eq!(exec(&p, true, "ui_apply", &json!({})).2, Some(Effect::Apply));
        let ch = changes(&p);
        assert!(ch.iter().any(|c| c.starts_with("theme.name: \"nebula\"")), "{ch:?}");
    }

    #[test]
    fn previews_for_the_model_are_plain_text() {
        let (_d, p) = home();
        exec(&p, false, "ui_set", &json!({"path": "markdown.links", "value": "osc8"}));
        let (out, err, _) = exec(&p, false, "ui_preview", &json!({}));
        assert!(!err, "{out}");
        assert!(out.contains("a link.") && !out.chars().any(|c| ('\u{E0000}'..='\u{E007F}').contains(&c)), "{out:?}");
    }

    #[test]
    fn the_operators_limits_survive_resets_and_whole_files() {
        let (_d, p) = home();
        std::fs::write(&p.ui, "[customise]\nmax_turns = 30\nmax_usd = 1.0\n\n[spinner]\nstyle = \"moon\"\n").unwrap();
        exec(&p, false, "ui_reset", &json!({}));
        let draft = std::fs::read_to_string(&p.draft).unwrap();
        assert!(draft.contains("max_turns = 30") && !draft.contains("moon"), "{draft}");
        // an ACP agent's whole file comes back without [customise]
        let file = "[theme]\nname = \"mono\"\n";
        assert!(schema::check_text(file).is_ok());
        let merged = keep_leash(&std::fs::read_to_string(&p.ui).unwrap(), file);
        assert!(merged.contains("name = \"mono\"") && merged.contains("max_usd = 1.0"), "{merged}");
        std::fs::write(&p.draft, merged).unwrap();
        apply(&p).unwrap();
        let ui = std::fs::read_to_string(&p.ui).unwrap();
        assert!(ui.contains("max_turns = 30") && ui.contains("name = \"mono\"") && !ui.contains("moon"), "{ui}");
    }

    #[test]
    fn empty_drafts_are_tidied_away() {
        let (_d, p) = home();
        std::fs::write(&p.ui, "[theme]\nname = \"mono\"\n").unwrap();
        begin(&p).unwrap();
        assert!(!tidy(&p) && !p.draft.exists() && !p.base.exists(), "an untouched draft goes");
        exec(&p, false, "ui_set", &json!({"path": "spinner.fps", "value": 9}));
        assert!(tidy(&p), "a real one stays");
    }

    #[test]
    fn apply_merges_and_your_edits_win() {
        let (_d, p) = home();
        std::fs::write(&p.ui, "[theme]\nname = \"mono\"\n").unwrap();
        exec(&p, false, "ui_set", &json!({"changes": [{"path": "theme.name", "value": "solar"}, {"path": "spinner.style", "value": "moon"}, {"path": "spinner.fps", "value": 8}]}));
        // meanwhile the operator runs /spinner pulse by hand
        edit::set_in_file(&p.ui, "spinner", "style", &Val::Str("pulse".into())).unwrap();
        let a = apply(&p).unwrap();
        assert_eq!(a.changed, vec!["spinner.fps".to_string(), "theme.name".to_string()]);
        assert_eq!(a.kept, vec!["spinner.style".to_string()]);
        let ui = std::fs::read_to_string(&p.ui).unwrap();
        assert!(ui.contains("name = \"solar\"") && ui.contains("style = \"pulse\"") && ui.contains("fps = 8"), "{ui}");
        assert!(!p.draft.exists() && !p.base.exists());
        assert!(apply(&p).is_err(), "nothing left to apply");
    }

    #[test]
    fn reset_and_saved_themes() {
        let _env = crate::env_lock();
        let (d, p) = home();
        std::env::set_var("COSMOS_CLI_HOME", d.path());
        exec(&p, false, "ui_set", &json!({"changes": [{"path": "theme.name", "value": "nebula"}, {"path": "theme.colors.accent", "value": "#123456"}, {"path": "spinner.fps", "value": 5}]}));
        let (out, err, _) = exec(&p, false, "theme_save", &json!({"name": "deep-space"}));
        assert!(!err, "{out}");
        let saved = crate::skin::saved_theme("deep-space").unwrap();
        assert_eq!((saved.base.as_str(), saved.colors.get("accent").map(String::as_str)), ("nebula", Some("#123456")));
        let draft = std::fs::read_to_string(&p.draft).unwrap();
        assert!(draft.contains("name = \"deep-space\"") && !draft.contains("#123456"), "{draft}");
        let sk: Skin = toml::from_str(&draft).unwrap();
        assert!(sk.palette().1.is_empty(), "the saved theme resolves");
        assert!(exec(&p, false, "theme_save", &json!({"name": "mono"})).1, "built-in names are taken");
        exec(&p, false, "ui_reset", &json!({"section": "spinner"}));
        assert!(!std::fs::read_to_string(&p.draft).unwrap().contains("fps"));
        exec(&p, false, "ui_reset", &json!({}));
        assert!(std::fs::read_to_string(&p.draft).unwrap().trim().is_empty());
    }

    #[test]
    fn providers_models_and_commands_stay_in_their_lane() {
        let (_d, p) = home();
        std::fs::write(p.home.join("config.toml"), "[permissions]\nallow = [\"read\"]\n").unwrap();
        let (out, err, _) = exec(&p, false, "provider_add", &json!({"id": "groq", "kind": "openai", "base_url": "https://api.groq.com/openai/v1", "api_key_env": "GROQ_API_KEY"}));
        assert!(!err, "{out}");
        let (out, err, _) = exec(&p, false, "provider_add", &json!({"id": "x", "kind": "openai", "base_url": "https://x.dev", "api_key_env": "gsk_live_abcdef123"}));
        assert!(err && out.contains("NAME of an environment variable"), "{out}");
        assert!(exec(&p, false, "provider_add", &json!({"id": "y", "kind": "openai", "base_url": "http://evil.example"})).1);
        assert!(exec(&p, false, "provider_add", &json!({"id": "groq", "kind": "openai", "base_url": "https://a.dev"})).0.contains("already exists"));
        let (out, err, _) = exec(&p, false, "model_add", &json!({"alias": "llama-fast", "provider": "groq", "id": "llama-3.3-70b", "context": 128000, "price": {"input": 0.59, "output": 0.79}}));
        assert!(!err, "{out}");
        assert!(exec(&p, false, "model_add", &json!({"alias": "z", "provider": "nowhere", "id": "m"})).1);
        let cfg = std::fs::read_to_string(p.home.join("config.toml")).unwrap();
        let t: toml::Table = toml::from_str(&cfg).unwrap();
        assert_eq!(t["permissions"]["allow"].as_array().unwrap().len(), 1, "permissions untouched");
        assert_eq!(t["models"]["llama-fast"]["price"]["output"].as_float(), Some(0.79));
        let (out, err, _) = exec(&p, false, "command_add", &json!({"name": "standup", "description": "Daily notes", "body": "Summarise $ARGUMENTS"}));
        assert!(!err, "{out}");
        assert!(std::fs::read_to_string(p.home.join("commands/standup.md")).unwrap().contains("description: Daily notes"));
        assert!(exec(&p, false, "command_add", &json!({"name": "model", "body": "x"})).1, "built-ins are protected");
        assert!(exec(&p, false, "command_add", &json!({"name": "standup", "body": "y"})).1, "no silent overwrite");
    }

    #[test]
    fn preview_renders_in_the_draft_look() {
        let mut sk = Skin::default();
        sk.theme.name = "nebula".into();
        let lines = preview(&sk, 72);
        let text: String = lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>() + "\n").collect();
        assert!(text.contains("Read src/main.rs") && text.contains("Preview"), "{text}");
        let accent = crate::skin::palette::builtin("nebula").unwrap().accent;
        assert!(lines.iter().flat_map(|l| &l.spans).any(|s| s.style.fg == Some(accent)), "draws in the draft's colours");
    }

    #[test]
    fn the_runner_drives_tools_with_any_model() {
        // the fixture model turns "tool:<name> <json>" into a tool call
        let _env = crate::env_lock();
        let (d, p) = home();
        std::env::set_var("COSMOS_CLI_HOME", d.path());
        let r = Runner::start(r#"tool:ui_set {"path":"spinner.style","value":"galaxy"}"#.into(), "fixture".into(), p.clone(), false, 4, 1.0);
        let mut seen = Vec::new();
        loop {
            match r.rx.recv_timeout(std::time::Duration::from_secs(20)).expect("runner answers") {
                Up::Done(s) => {
                    seen.push(format!("done {s}"));
                    break;
                }
                Up::Failed(e) => panic!("{e}"),
                other => seen.push(format!("{other:?}")),
            }
        }
        assert!(seen.iter().any(|s| s.contains("Effect(Draft)")), "{seen:?}");
        assert!(std::fs::read_to_string(&p.draft).unwrap().contains("style = \"galaxy\""));
        assert!(p.home.join("customise").read_dir().unwrap().count() >= 1, "the conversation is kept");
    }

    #[test]
    fn the_picker_cycles_and_saves() {
        let _env = crate::env_lock();
        let (_d, p) = home();
        crate::skin::scoped_skin(Skin::default(), || {
            assert_eq!(picker_step(&p.ui, 1, 1).unwrap(), "Spinner → twinkle");
            assert_eq!(picker_step(&p.ui, 8, 1).unwrap(), "Motion → reduced");
            assert_eq!(picker_step(&p.ui, 7, -1).unwrap(), "Species → robot");
        });
        let ui = std::fs::read_to_string(&p.ui).unwrap();
        assert!(ui.contains("style = \"twinkle\"") && ui.contains("reduce = true") && ui.contains("species = \"robot\""), "{ui}");
        for (_, path) in PICKER {
            assert!(picker_options(path).iter().all(|v| schema::check(path, v).is_ok()), "{path}");
        }
    }

    #[test]
    fn acp_replies_are_parsed_and_checked() {
        assert_eq!(toml_block("Done!\n```toml\n[theme]\nname = \"mono\"\n```\n").as_deref(), Some("[theme]\nname = \"mono\"\n"));
        assert!(toml_block("no file").is_none());
    }
}
