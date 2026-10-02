//! The shape of `ui.toml`, for `/customise` and the docs: every editable
//! path, its type and default (taken from `Skin::default()`), the values it
//! accepts, and one line on what it does.
//!
//! `check` is the gate every customiser write goes through — an unknown
//! path, a wrong type, an out-of-range number, or anything that smells like
//! a hard boundary is refused here, in code, whatever the model was told.

use serde_json::Value;

use super::edit::Val;
use super::{palette, Skin};

/// Sections the customiser may write.
pub const SECTIONS: &[&str] = &["theme", "spinner", "motion", "tools", "diff", "markdown", "glyphs", "buddy", "notify"];

/// Never writable by `/customise` (most live in config.toml; `customise`
/// is the customiser's own leash).
pub const BOUNDARIES: &[&str] = &[
    "permissions", "permission", "sandbox", "ask", "approval", "approvals", "self_improve", "self_mutate", "allow",
    "deny", "hooks", "mcp", "mcp_servers", "credentials", "creds", "keys", "api_key", "api_keys", "auth", "customise",
    "customize", "yolo", "escalation", "network",
];

/// `{name}` placeholders a buddy line can be keyed by.
pub const LINE_KINDS: &[&str] =
    &["hello", "done", "long_done", "error", "tests_pass", "tests_fail", "waiting", "compact", "pet", "sleep", "degraded"];

fn enums(path: &str) -> Option<Vec<String>> {
    let v = |s: &[&str]| Some(s.iter().map(|x| x.to_string()).collect());
    match path {
        "theme.name" => Some(super::themes()),
        "spinner.style" => v(crate::motion::STYLES),
        "tools.style" | "diff.style" | "markdown.style" => v(&["cosmos", "classic"]),
        "diff.layout" => v(&["auto", "unified", "split"]),
        "markdown.links" => v(&["auto", "osc8", "inline"]),
        "buddy.size" => v(&["full", "mini", "off"]),
        "buddy.species" => Some(std::iter::once(String::new()).chain(crate::buddy::SPECIES.iter().map(|(n, _)| n.to_string())).collect()),
        "buddy.quips" => v(&["builtin", "off"]),
        "notify.bell" | "notify.desktop" => v(&["unfocused", "always", "never"]),
        "notify.events" => v(&["done", "waiting", "error"]),
        _ => None,
    }
}

fn range(path: &str) -> Option<(i64, i64)> {
    Some(match path {
        "spinner.fps" => (1, 60),
        "motion.reveal_ms" => (0, 2000),
        "tools.keep_rows" | "tools.keep_rows_search" => (0, 50),
        "diff.max_rows" => (1, 2000),
        "diff.context" => (0, 20),
        "buddy.seed" => (0, 1_000_000),
        "buddy.quip_secs" => (1, 60),
        "notify.min_secs" => (0, 3600),
        _ => return None,
    })
}

/// Most display columns a text value may take.
fn max_cols(path: &str) -> usize {
    match path {
        p if p.starts_with("glyphs.") => 2,
        "spinner.footer_glyph" => 2,
        "spinner.frames" => 6,
        "spinner.hint" | "buddy.name" => 40,
        _ => 200,
    }
}

/// One line per path — what it does.
pub const HELP: &[(&str, &str)] = &[
    ("theme.name", "colour theme (built-in or saved with theme_save)"),
    ("theme.colors.<token>", "override one colour token: \"#rrggbb\", \"#rgb\" or an ANSI name"),
    ("spinner.style", "the thinking spinner"),
    ("spinner.frames", "custom spinner frames; non-empty replaces style"),
    ("spinner.fps", "spinner frames per second"),
    ("spinner.verbs", "\"Present:Past\" thinking verbs, one picked per turn (\"Cooking:Cooked\")"),
    ("spinner.tool_verbs.<tool>", "verb shown while a tool runs (read, grep, glob, ls, bash, edit, write, task, …)"),
    ("spinner.shimmer", "a highlight sweeping across the verb"),
    ("spinner.show_elapsed", "seconds in the busy line"),
    ("spinner.show_tokens", "streamed tokens in the busy line"),
    ("spinner.hint", "hint at the end of the busy line"),
    ("spinner.footer", "\"✻ Cooked for 12s\" after each turn"),
    ("spinner.footer_glyph", "glyph before that footer"),
    ("motion.reduce", "no animation at all"),
    ("motion.reveal", "streamed text fades in"),
    ("motion.reveal_ms", "how long the fade-in takes"),
    ("tools.style", "tool-call layout"),
    ("tools.keep_rows", "output rows shown under a finished tool (ctrl+o shows all)"),
    ("tools.keep_rows_search", "rows shown under search tools"),
    ("tools.group", "fold runs of reads/searches into one \"Explored\" block"),
    ("tools.show_elapsed", "how long each tool took"),
    ("diff.style", "diff layout"),
    ("diff.layout", "split = side by side; auto splits on wide terminals"),
    ("diff.max_rows", "diff rows shown before \"N more lines\""),
    ("diff.context", "unchanged lines around each change"),
    ("diff.word_diff", "highlight the changed words"),
    ("diff.syntax", "syntax colour in diffs"),
    ("diff.wrap", "wrap long diff lines instead of cutting them"),
    ("markdown.style", "markdown rendering"),
    ("markdown.code_frame", "box code blocks with a language label"),
    ("markdown.code_bg", "tint code blocks"),
    ("markdown.code_numbers", "line numbers in code blocks"),
    ("markdown.links", "osc8 = clickable hyperlinks; inline = show the URL"),
    ("markdown.callouts", "render > [!NOTE] / TIP / WARNING blocks"),
    ("markdown.table_borders", "box-drawn tables with wrapped cells"),
    ("markdown.bullets", "list bullets by nesting depth"),
    ("markdown.h1_rule", "a rule under top-level headings"),
    ("glyphs.tool", "glyph before each tool call"),
    ("glyphs.group", "glyph before a folded group"),
    ("glyphs.elbow", "glyph before tool output"),
    ("glyphs.wrap", "glyph on wrapped diff lines"),
    ("buddy.enabled", "show the companion"),
    ("buddy.size", "full = sprite beside the input; mini = face in the status line"),
    ("buddy.species", "\"\" = hatched from the user's name"),
    ("buddy.name", "\"\" = a generated name"),
    ("buddy.seed", "change to hatch a different buddy"),
    ("buddy.quips", "one-liners when things happen"),
    ("buddy.quip_secs", "how long a quip stays up"),
    ("buddy.lines.<kind>", "custom quips for a moment ({name} = the buddy)"),
    ("notify.title", "turn state in the terminal title"),
    ("notify.bell", "terminal bell"),
    ("notify.desktop", "desktop notification"),
    ("notify.sound", "a short chime instead of the bell"),
    ("notify.min_secs", "turns shorter than this never notify"),
    ("notify.events", "which moments notify"),
];

fn help(path: &str) -> &'static str {
    HELP.iter().find(|(p, _)| *p == path).map(|(_, h)| *h).unwrap_or("")
}

/// The open-ended maps: (prefix, what the key must be).
const MAPS: &[&str] = &["theme.colors", "spinner.tool_verbs", "buddy.lines"];

fn boundary(seg: &str) -> bool {
    let s = seg.to_ascii_lowercase();
    BOUNDARIES.contains(&s.as_str()) || s == "providers" || s == "models"
}

fn refuse(path: &str) -> String {
    let first = path.split('.').next().unwrap_or("");
    if first == "providers" || first == "models" {
        return format!("{path}: providers and models are added with provider_add / model_add, not ui_set");
    }
    format!(
        "{path}: that's a hard boundary — /customise never changes permissions, sandbox, approvals, hooks, MCP, credentials or its own limits"
    )
}

/// The default value at `section.key` (None for unknown paths).
fn default_at(section: &str, key: &str) -> Option<Value> {
    let v = serde_json::to_value(Skin::default()).ok()?;
    v.get(section)?.get(key).cloned()
}

fn text_ok(path: &str, s: &str) -> Result<(), String> {
    if s.chars().any(|c| c.is_control()) {
        return Err(format!("{path}: no control characters"));
    }
    let cols = crate::wrap::width_of(s);
    let max = max_cols(path);
    if cols > max {
        return Err(format!("{path}: {s:?} is {cols} columns wide (at most {max})"));
    }
    Ok(())
}

fn strings(path: &str, v: &Value) -> Result<Vec<String>, String> {
    let arr = v.as_array().ok_or_else(|| format!("{path}: expected a list of strings"))?;
    if arr.len() > 100 {
        return Err(format!("{path}: at most 100 entries"));
    }
    arr.iter()
        .map(|x| {
            let s = x.as_str().ok_or_else(|| format!("{path}: expected a list of strings"))?;
            text_ok(path, s)?;
            Ok(s.to_string())
        })
        .collect()
}

/// Validate one write. Ok = (table, key, value) ready for `edit::set`.
pub fn check(path: &str, value: &Value) -> Result<(String, String, Val), String> {
    let path = path.trim();
    if path.split('.').any(boundary) {
        return Err(refuse(path));
    }
    let (section, rest) = path.split_once('.').ok_or_else(|| format!("{path}: use section.key (sections: {})", SECTIONS.join(", ")))?;
    if !SECTIONS.contains(&section) {
        return Err(format!("{path}: unknown section {section:?} (sections: {})", SECTIONS.join(", ")));
    }
    // open maps: theme.colors.<token>, spinner.tool_verbs.<tool>, buddy.lines.<kind>
    for m in MAPS {
        if let Some(k) = path.strip_prefix(m).and_then(|r| r.strip_prefix('.')) {
            let val = match *m {
                "theme.colors" => {
                    if !palette::TOKENS.contains(&k) {
                        return Err(format!("{path}: unknown colour token (tokens: {})", palette::TOKENS.join(", ")));
                    }
                    let s = value.as_str().ok_or_else(|| format!("{path}: expected a colour string"))?;
                    if palette::parse_color(s).is_none() {
                        return Err(format!("{path}: {s:?} is not a colour (\"#rrggbb\", \"#rgb\" or an ANSI name)"));
                    }
                    Val::Str(s.to_string())
                }
                "spinner.tool_verbs" => {
                    if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                        return Err(format!("{path}: tool names are letters, digits and _"));
                    }
                    let s = value.as_str().ok_or_else(|| format!("{path}: expected a string"))?;
                    text_ok(path, s)?;
                    Val::Str(s.to_string())
                }
                _ => {
                    if !LINE_KINDS.contains(&k) {
                        return Err(format!("{path}: unknown moment (moments: {})", LINE_KINDS.join(", ")));
                    }
                    Val::List(strings(path, value)?)
                }
            };
            return Ok((m.to_string(), k.to_string(), val));
        }
    }
    if rest.contains('.') {
        return Err(format!("{path}: unknown path"));
    }
    let def = default_at(section, rest).ok_or_else(|| format!("{path}: unknown key (see the schema)"))?;
    let val = match def {
        Value::Bool(_) => Val::Bool(value.as_bool().ok_or_else(|| format!("{path}: expected true or false"))?),
        Value::String(_) => {
            let s = value.as_str().ok_or_else(|| format!("{path}: expected a string"))?;
            text_ok(path, s)?;
            if let Some(ok) = enums(path) {
                if !ok.iter().any(|o| o == s) {
                    return Err(format!("{path}: {s:?} isn't one of {}", ok.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>().join(", ")));
                }
            }
            Val::Str(s.to_string())
        }
        Value::Number(n) if n.is_f64() => Val::Float(value.as_f64().ok_or_else(|| format!("{path}: expected a number"))?),
        Value::Number(_) => {
            let i = value.as_i64().or_else(|| value.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)).ok_or_else(|| format!("{path}: expected a whole number"))?;
            if let Some((lo, hi)) = range(path) {
                if !(lo..=hi).contains(&i) {
                    return Err(format!("{path}: {i} is outside {lo}..={hi}"));
                }
            }
            Val::Int(i)
        }
        Value::Array(_) => {
            let list = strings(path, value)?;
            if let Some(ok) = enums(path) {
                if let Some(bad) = list.iter().find(|x| !ok.contains(x)) {
                    return Err(format!("{path}: {bad:?} isn't one of {}", ok.join(", ")));
                }
            }
            if path == "spinner.verbs" && list.is_empty() {
                return Err("spinner.verbs: keep at least one verb".into());
            }
            Val::List(list)
        }
        _ => return Err(format!("{path}: unknown path")),
    };
    Ok((section.to_string(), rest.to_string(), val))
}

/// A TOML leaf as JSON (by hand: no second serializer in the binary).
fn json_of(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) => Value::from(*f),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Array(a) => Value::Array(a.iter().map(json_of).collect()),
        _ => Value::Null,
    }
}

/// Check a whole candidate `ui.toml` (an ACP agent's reply): only known
/// sections, every leaf valid.
pub fn check_text(text: &str) -> Result<(), String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| format!("not valid TOML: {}", e.to_string().trim()))?;
    fn walk(prefix: &str, t: &toml::Table, out: &mut Vec<(String, toml::Value)>) {
        for (k, v) in t {
            let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
            match v {
                toml::Value::Table(sub) if !MAPS.contains(&p.as_str()) || prefix.is_empty() => walk(&p, sub, out),
                toml::Value::Table(sub) => {
                    for (kk, vv) in sub {
                        out.push((format!("{p}.{kk}"), vv.clone()));
                    }
                }
                _ => out.push((p, v.clone())),
            }
        }
    }
    let mut leaves = Vec::new();
    walk("", &table, &mut leaves);
    for (p, v) in leaves {
        check(&p, &json_of(&v))?;
    }
    Ok(())
}

/// The schema as text for the customiser's system prompt (and the docs).
pub fn text() -> String {
    let mut out = String::new();
    let v = serde_json::to_value(Skin::default()).unwrap_or(Value::Null);
    for s in SECTIONS {
        out.push_str(&format!("[{s}]\n"));
        let Some(obj) = v.get(*s).and_then(Value::as_object) else { continue };
        for (k, d) in obj {
            let path = format!("{s}.{k}");
            if MAPS.contains(&path.as_str()) {
                let (key, what) = match path.as_str() {
                    "theme.colors" => ("<token>", format!("colour; tokens: {}", palette::TOKENS.join(", "))),
                    "spinner.tool_verbs" => ("<tool>", "string".to_string()),
                    _ => ("<kind>", format!("list of strings; kinds: {}", LINE_KINDS.join(", "))),
                };
                out.push_str(&format!("  {k}.{key}: {what} — {}\n", help(&format!("{path}.{key}"))));
                continue;
            }
            let ty = match d {
                Value::Bool(_) => "bool".to_string(),
                Value::String(_) => "string".to_string(),
                Value::Number(n) if n.is_f64() => "number".to_string(),
                Value::Number(_) => "integer".to_string(),
                Value::Array(_) => "list of strings".to_string(),
                _ => "value".to_string(),
            };
            let mut line = format!("  {k}: {ty} = {}", short(d));
            if let Some(e) = enums(&path) {
                line.push_str(&format!(" — one of {}", e.iter().map(|x| format!("{x:?}")).collect::<Vec<_>>().join(" | ")));
            }
            if let Some((lo, hi)) = range(&path) {
                line.push_str(&format!(" — {lo}..={hi}"));
            }
            let h = help(&path);
            if !h.is_empty() {
                line.push_str(&format!(" — {h}"));
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// A default value, cut short when long (the verb list).
fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() > 90 {
        let cut: String = s.chars().take(87).collect();
        format!("{cut}…")
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_are_checked_against_the_schema() {
        assert_eq!(check("theme.name", &json!("nebula")).unwrap(), ("theme".into(), "name".into(), Val::Str("nebula".into())));
        assert!(check("theme.name", &json!("purple-haze")).unwrap_err().contains("isn't one of"));
        assert_eq!(check("theme.colors.accent", &json!("#9b5cff")).unwrap().0, "theme.colors");
        assert_eq!(check("theme.colors.syntax.keyword", &json!("magenta")).unwrap().1, "syntax.keyword");
        assert!(check("theme.colors.accent", &json!("purple-ish")).is_err());
        assert!(check("theme.colors.nope", &json!("#fff")).is_err());
        assert_eq!(check("spinner.fps", &json!(20.0)).unwrap().2, Val::Int(20));
        assert!(check("spinner.fps", &json!(500)).unwrap_err().contains("outside"));
        assert!(check("spinner.style", &json!("comet")).is_ok());
        assert!(check("spinner.verbs", &json!(["Warping:Warped"])).is_ok());
        assert!(check("spinner.verbs", &json!([])).is_err());
        assert!(check("glyphs.tool", &json!("◆◆◆")).unwrap_err().contains("columns"));
        assert!(check("buddy.lines.done", &json!(["{name} did it"])).is_ok());
        assert!(check("buddy.species", &json!("dragon")).is_ok());
        assert!(check("notify.events", &json!(["done", "explode"])).is_err());
        assert!(check("motion.reduce", &json!("yes")).unwrap_err().contains("true or false"));
        assert!(check("spinner.nope", &json!(1)).is_err());
        assert!(check("nope", &json!(1)).is_err());
    }

    #[test]
    fn hard_boundaries_are_refused() {
        for p in ["permissions.allow", "sandbox.mode", "ask", "hooks.cmd", "mcp.servers", "customise.max_usd", "customise.auto_apply", "theme.permissions", "credentials.openai"] {
            let e = check(p, &json!("x")).unwrap_err();
            assert!(e.contains("hard boundary"), "{p}: {e}");
        }
        assert!(check("providers.groq", &json!("x")).unwrap_err().contains("provider_add"));
    }

    #[test]
    fn whole_files_are_checked() {
        assert!(check_text("[theme]\nname = \"mono\"\n[theme.colors]\naccent = \"#ff00ff\"\n\"syntax.keyword\" = \"red\"\n").is_ok());
        assert!(check_text("[permissions]\nallow = [\"bash\"]\n").unwrap_err().contains("hard boundary"));
        assert!(check_text("[spinner]\nfps = 0\n").is_err());
        assert!(check_text("not toml [").is_err());
    }

    #[test]
    fn schema_text_lists_every_section_and_choice() {
        let t = text();
        for s in SECTIONS {
            assert!(t.contains(&format!("[{s}]")), "{s}");
        }
        assert!(t.contains("style: string = \"orb\" — one of"));
        assert!(t.contains("colors.<token>"));
        assert!(!t.contains("customise"), "the leash isn't advertised as editable");
    }
}
