//! Skins: everything the TUI looks like and how it behaves, from one file.
//!
//! `~/.cosmos-cli/ui.toml`, then the project's `.cosmos/ui.toml` on top, over
//! built-in defaults. The render loop polls the files once a second and
//! swaps the resolved skin in *between* frames, so edits — by hand, by
//! `/theme`, or by `/customise` — show up while you keep working. A file that
//! does not parse keeps the previous skin and reports the error.
//!
//! A skin holds looks and harmless behaviour only. Permissions, sandbox,
//! approval, hooks, MCP and credentials live in `config.toml` and have no
//! field here, so nothing that edits a skin can reach them.

pub mod edit;
pub mod palette;
pub mod schema;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

pub use palette::Palette;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ThemeCfg {
    /// a built-in theme: cosmos, classic, nebula, mono, solar, light, high-contrast
    pub name: String,
    /// token → colour overrides (`accent = "#9aa8ff"`, `"syntax.keyword" = "magenta"`)
    pub colors: BTreeMap<String, String>,
}

impl Default for ThemeCfg {
    fn default() -> Self {
        ThemeCfg { name: "cosmos".into(), colors: BTreeMap::new() }
    }
}

/// Default thinking verbs: `Present:Past` (the past tense is for the
/// turn-end footer, "✻ Cooked for 12s").
pub const VERBS: &[&str] = &[
    "Cosmosing:Cosmosed", "Plotting:Plotted", "Cooking:Cooked", "Orbiting:Orbited", "Stargazing:Stargazed",
    "Warping:Warped", "Terraforming:Terraformed", "Nebulising:Nebulised", "Charting:Charted", "Scheming:Schemed",
    "Brewing:Brewed", "Calibrating:Calibrated", "Conjuring:Conjured", "Pondering:Pondered", "Tinkering:Tinkered",
    "Moonwalking:Moonwalked", "Supernova-ing:Supernova'd", "Mapping constellations:Mapped constellations",
    "Bending spacetime:Bent spacetime", "Counting stars:Counted stars",
];

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SpinnerCfg {
    /// orb, twinkle, braille, moon, comet, pulse, galaxy, dots, line
    pub style: String,
    /// custom frames; when set they replace `style`
    pub frames: Vec<String>,
    pub fps: u32,
    /// `Present:Past` pairs (or just `Present`); one is picked per turn
    pub verbs: Vec<String>,
    /// verb while a tool runs, by tool name ("bash" = "Running")
    pub tool_verbs: BTreeMap<String, String>,
    /// a highlight sweeping across the verb
    pub shimmer: bool,
    pub show_elapsed: bool,
    pub show_tokens: bool,
    pub hint: String,
    /// "✻ Cooked for 12s · ↓ 1.2k tokens" after each turn
    pub footer: bool,
    pub footer_glyph: String,
}

impl Default for SpinnerCfg {
    fn default() -> Self {
        let tv = [
            ("read", "Reading"), ("grep", "Searching"), ("glob", "Scanning"), ("ls", "Listing"), ("bash", "Running"),
            ("edit", "Editing"), ("write", "Writing"), ("task", "Delegating"), ("memory_search", "Remembering"),
            ("memory_write", "Noting"), ("tool_search", "Looking up tools"), ("mcp_call", "Calling"),
        ];
        SpinnerCfg {
            style: "orb".into(),
            frames: Vec::new(),
            fps: 12,
            verbs: VERBS.iter().map(|s| s.to_string()).collect(),
            tool_verbs: tv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            shimmer: true,
            show_elapsed: true,
            show_tokens: true,
            hint: "esc to interrupt".into(),
            footer: true,
            footer_glyph: "✻".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct MotionCfg {
    /// no animation at all: static spinner, no shimmer, no fade-in
    pub reduce: bool,
    /// streamed text fades in instead of popping
    pub reveal: bool,
    pub reveal_ms: u64,
}

impl Default for MotionCfg {
    fn default() -> Self {
        MotionCfg { reduce: false, reveal: true, reveal_ms: 160 }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct ToolsCfg {
    /// "cosmos" (status glyph, meta, grouping) or "classic"
    pub style: String,
    /// result rows kept under a finished tool (searches keep `keep_rows_search`)
    pub keep_rows: usize,
    pub keep_rows_search: usize,
    /// collapse runs of read/search tools into one "Explored …" block
    pub group: bool,
    pub show_elapsed: bool,
}

impl Default for ToolsCfg {
    fn default() -> Self {
        ToolsCfg { style: "cosmos".into(), keep_rows: 4, keep_rows_search: 3, group: true, show_elapsed: true }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct DiffCfg {
    /// "cosmos" (word highlights, syntax colour, wrapping) or "classic"
    pub style: String,
    /// "unified", "split", or "auto" (split when the terminal is ≥ 140 wide)
    pub layout: String,
    /// rows shown before "N more lines" (ctrl+o shows the whole diff)
    pub max_rows: usize,
    /// unchanged lines around each change
    pub context: usize,
    pub word_diff: bool,
    pub syntax: bool,
    /// wrap long lines with ↳ instead of cutting them
    pub wrap: bool,
    /// diffs up to this many rows show inline; bigger ones fold to their
    /// +/− counts with ctrl+o to view (0 = always fold)
    pub inline_rows: usize,
}

impl Default for DiffCfg {
    fn default() -> Self {
        DiffCfg { style: "cosmos".into(), layout: "auto".into(), max_rows: 40, context: 2, word_diff: true, syntax: true, wrap: true, inline_rows: 12 }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct MarkdownCfg {
    /// "cosmos" (framed code, callouts, bordered tables, hyperlinks) or "classic"
    pub style: String,
    /// box code blocks with a language label
    pub code_frame: bool,
    /// tint code blocks with the theme's code_bg
    pub code_bg: bool,
    pub code_numbers: bool,
    /// "auto" (OSC 8 hyperlinks where the terminal supports them), "osc8", "inline"
    pub links: String,
    /// `> [!NOTE]` / TIP / IMPORTANT / WARNING / CAUTION
    pub callouts: bool,
    pub table_borders: bool,
    /// list bullets by depth
    pub bullets: Vec<String>,
    /// a rule under level-1 headings
    pub h1_rule: bool,
}

impl Default for MarkdownCfg {
    fn default() -> Self {
        MarkdownCfg {
            style: "cosmos".into(),
            code_frame: true,
            code_bg: true,
            code_numbers: false,
            links: "auto".into(),
            callouts: true,
            table_borders: true,
            bullets: ["•", "◦", "▸", "▹"].map(String::from).to_vec(),
            h1_rule: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct BuddyCfg {
    pub enabled: bool,
    /// "full" (a sprite beside the composer), "mini" (a face in the status line)
    pub size: String,
    /// empty: hatched from your name; or one of buddy::SPECIES
    pub species: String,
    /// empty: a generated name
    pub name: String,
    /// bump to hatch a different buddy (`/buddy reroll`)
    pub seed: u64,
    /// "builtin" one-liners, or "off"
    pub quips: String,
    /// how long a quip stays up
    pub quip_secs: u64,
    /// your own quips by moment: hello, done, long_done, error, tests_pass,
    /// tests_fail, waiting, compact, pet, sleep, degraded, break, level
    /// ({name} = the pet)
    pub lines: BTreeMap<String, Vec<String>>,
    /// recolour the pet: outline, body, shade, belly, accent, cheeks, details, eyes
    pub colors: BTreeMap<String, String>,
    /// real images of the pet: "auto", "sixel", "kitty" or "off" (pixel art)
    pub images: String,
    /// suggest a break after this many minutes of work (0 = never)
    pub break_mins: u64,
}

impl Default for BuddyCfg {
    fn default() -> Self {
        BuddyCfg {
            enabled: true,
            size: "full".into(),
            species: String::new(),
            name: String::new(),
            seed: 0,
            quips: "builtin".into(),
            quip_secs: 6,
            lines: BTreeMap::new(),
            colors: BTreeMap::new(),
            images: "auto".into(),
            break_mins: 50,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct NotifyCfg {
    /// show progress in the terminal title (OSC 0)
    pub title: bool,
    /// "unfocused", "always" or "never"
    pub bell: String,
    /// desktop notification: "unfocused", "always" or "never"
    pub desktop: String,
    /// a short chime (needs the voice feature; otherwise the bell)
    pub sound: bool,
    /// turns shorter than this never notify
    pub min_secs: u64,
    /// which moments notify: done, waiting (a permission prompt), error
    pub events: Vec<String>,
}

impl Default for NotifyCfg {
    fn default() -> Self {
        NotifyCfg {
            title: true,
            bell: "unfocused".into(),
            desktop: "unfocused".into(),
            sound: false,
            min_secs: 15,
            events: ["done", "waiting", "error"].map(String::from).to_vec(),
        }
    }
}

/// How `/customise` runs. The customiser itself can't change this section.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct CustomiseCfg {
    /// model alias for customising; empty = the session's model
    pub model: String,
    /// apply each change as soon as it validates (otherwise ctrl+y applies)
    pub auto_apply: bool,
    pub max_turns: u32,
    /// spend cap per request, USD
    pub max_usd: f64,
}

impl Default for CustomiseCfg {
    fn default() -> Self {
        CustomiseCfg { model: String::new(), auto_apply: false, max_turns: 12, max_usd: 0.25 }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct GlyphsCfg {
    /// the dot before a tool (coloured by its status)
    pub tool: String,
    /// a collapsed group of reads/searches
    pub group: String,
    /// the elbow before a tool's result rows
    pub elbow: String,
    /// continuation of a wrapped line
    pub wrap: String,
}

impl Default for GlyphsCfg {
    fn default() -> Self {
        GlyphsCfg { tool: "◆".into(), group: "◇".into(), elbow: "⎿".into(), wrap: "↳".into() }
    }
}

/// The whole `ui.toml`. Unknown keys are ignored on load (a file written
/// for a newer cosmostui still loads) but rejected by `/customise`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Skin {
    pub theme: ThemeCfg,
    pub spinner: SpinnerCfg,
    pub motion: MotionCfg,
    pub tools: ToolsCfg,
    pub diff: DiffCfg,
    pub markdown: MarkdownCfg,
    pub glyphs: GlyphsCfg,
    pub buddy: BuddyCfg,
    pub notify: NotifyCfg,
    pub customise: CustomiseCfg,
}

impl Skin {
    /// The look cosmostui had before skins: classic colours and layouts
    /// (`/customise reset classic` writes the same thing as TOML).
    #[cfg(test)]
    pub fn classic() -> Skin {
        let mut s = Skin::default();
        s.theme.name = "classic".into();
        s.tools.style = "classic".into();
        s.diff.style = "classic".into();
        s.markdown.style = "classic".into();
        s.buddy.enabled = false;
        s.glyphs.tool = "●".into();
        s
    }
}

impl Skin {
    /// The palette this skin resolves to, plus warnings for bad entries.
    pub fn palette(&self) -> (Palette, Vec<String>) {
        let mut warn = Vec::new();
        let mut p = match palette::builtin(&self.theme.name) {
            Some(p) => p,
            None => match saved_theme(&self.theme.name) {
                Some(t) => {
                    let mut p = palette::builtin(&t.base).unwrap_or(palette::COSMOS);
                    for (k, v) in &t.colors {
                        if let Some(c) = palette::parse_color(v) {
                            p.set(k, c);
                        }
                    }
                    p
                }
                None => {
                    warn.push(format!("unknown theme {:?} — using cosmos (themes: {})", self.theme.name, themes().join(", ")));
                    palette::COSMOS
                }
            },
        };
        for (k, v) in &self.theme.colors {
            match palette::parse_color(v) {
                Some(c) if p.set(k, c) => {}
                Some(_) => warn.push(format!("unknown colour token {k:?}")),
                None => warn.push(format!("{k} = {v:?} is not a colour (#rrggbb or a name)")),
            }
        }
        (p, warn)
    }
}

// ---- saved themes --------------------------------------------------------------------

/// A theme saved by `/customise` (`theme_save`): a built-in base plus colours,
/// in `~/.cosmos-cli/themes/<name>.toml`.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct SavedTheme {
    pub base: String,
    pub colors: BTreeMap<String, String>,
}

/// Lowercase letters, digits and dashes, 1–32 long.
pub fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len()) && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn themes_dir() -> PathBuf {
    zcore::cli_home().join("themes")
}

pub fn saved_theme(name: &str) -> Option<SavedTheme> {
    if !valid_name(name) {
        return None;
    }
    let text = std::fs::read_to_string(themes_dir().join(format!("{name}.toml"))).ok()?;
    let t: toml::Table = toml::from_str(&text).ok()?;
    toml::Value::Table(t).try_into().ok()
}

/// Built-in themes, then saved ones.
pub fn themes() -> Vec<String> {
    let mut v: Vec<String> = palette::THEMES.iter().map(|s| s.to_string()).collect();
    if let Ok(rd) = std::fs::read_dir(themes_dir()) {
        let mut saved: Vec<String> = rd
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".toml").map(str::to_string))
            .filter(|n| valid_name(n) && !v.contains(n))
            .collect();
        saved.sort();
        v.extend(saved);
    }
    v
}

// ---- current skin ----------------------------------------------------------------

struct State {
    skin: Arc<Skin>,
    palette: Palette,
}

static STATE: RwLock<Option<State>> = RwLock::new(None);

fn with<T>(f: impl FnOnce(&State) -> T) -> T {
    {
        if let Ok(g) = STATE.read() {
            if let Some(s) = g.as_ref() {
                return f(s);
            }
        }
    }
    install(Skin::default());
    let g = STATE.read().unwrap_or_else(|e| e.into_inner());
    f(g.as_ref().expect("installed"))
}

thread_local! {
    /// A palette for this thread only (tests render themes side by side).
    static SCOPED: std::cell::Cell<Option<Palette>> = const { std::cell::Cell::new(None) };
    /// A whole skin for this thread only (tests).
    static SCOPED_SKIN: std::cell::RefCell<Option<Arc<Skin>>> = const { std::cell::RefCell::new(None) };
}

/// The palette every style function reads.
pub fn palette() -> Palette {
    if let Some(p) = SCOPED.with(|c| c.get()) {
        return p;
    }
    with(|s| s.palette)
}

/// Run `f` with `p` as this thread's palette.
pub fn scoped<T>(p: Palette, f: impl FnOnce() -> T) -> T {
    let prev = SCOPED.with(|c| c.replace(Some(p)));
    let out = f();
    SCOPED.with(|c| c.set(prev));
    out
}

/// Run `f` with `skin` (and its palette) as this thread's skin — tests,
/// and `/customise` previews of a draft.
pub fn scoped_skin<T>(skin: Skin, f: impl FnOnce() -> T) -> T {
    let p = skin.palette().0;
    let prev = SCOPED_SKIN.with(|c| c.replace(Some(Arc::new(skin))));
    let out = scoped(p, f);
    SCOPED_SKIN.with(|c| *c.borrow_mut() = prev);
    out
}

pub fn current() -> Arc<Skin> {
    if let Some(s) = SCOPED_SKIN.with(|c| c.borrow().clone()) {
        return s;
    }
    with(|s| s.skin.clone())
}

/// Make `skin` current; returns its warnings.
pub fn install(skin: Skin) -> Vec<String> {
    let (palette, warn) = skin.palette();
    if let Ok(mut g) = STATE.write() {
        *g = Some(State { skin: Arc::new(skin), palette });
    }
    warn
}

// ---- files -------------------------------------------------------------------------

/// User file first, project file on top.
pub fn files(cwd: &Path) -> Vec<PathBuf> {
    vec![zcore::cli_home().join("ui.toml"), cwd.join(".cosmos").join("ui.toml")]
}

/// Deep-merge `top` into `base` (tables merge, anything else replaces).
fn merge(base: &mut toml::Table, top: toml::Table) {
    for (k, v) in top {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(t)) => merge(b, t),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

/// Parse one ui.toml text. Everything goes through a toml::Table first so
/// the crate carries one Skin deserializer, not one per entry point.
pub fn parse(text: &str) -> Result<Skin, String> {
    let t: toml::Table = toml::from_str(text).map_err(|e| format!("not valid TOML: {}", e.to_string().trim()))?;
    toml::Value::Table(t).try_into::<Skin>().map_err(|e| format!("not a valid ui.toml: {}", e.to_string().trim()))
}

/// Read and merge the files that exist. Err = the first file that does not
/// parse (path and message).
pub fn load(paths: &[PathBuf]) -> Result<Skin, String> {
    let mut table = toml::Table::new();
    for p in paths {
        let Ok(text) = std::fs::read_to_string(p) else { continue };
        let t: toml::Table = toml::from_str(&text).map_err(|e| format!("{}: {}", p.display(), e.to_string().trim()))?;
        merge(&mut table, t);
    }
    toml::Value::Table(table).try_into::<Skin>().map_err(|e| format!("ui.toml: {}", e.to_string().trim()))
}

/// Watches the skin files and reloads on change (checked at most once a
/// second, so the render loop can call it every frame).
pub struct Watcher {
    paths: Vec<PathBuf>,
    seen: Vec<Option<(SystemTime, u64)>>,
    last: Instant,
}

fn stamp(p: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

impl Watcher {
    /// Load the files now and remember their state. Returns load errors and
    /// palette warnings to show the operator.
    pub fn start(cwd: &Path) -> (Watcher, Vec<String>) {
        let paths = files(cwd);
        let seen = paths.iter().map(|p| stamp(p)).collect();
        let mut w = Watcher { paths, seen, last: Instant::now() };
        let notes = w.apply();
        (w, notes)
    }

    fn apply(&mut self) -> Vec<String> {
        match load(&self.paths) {
            Ok(skin) => install(skin),
            Err(e) => vec![format!("{e} — keeping the previous look")],
        }
    }

    /// Some(notes) when a file changed and the skin was reloaded.
    pub fn poll(&mut self) -> Option<Vec<String>> {
        if self.last.elapsed() < Duration::from_secs(1) {
            return None;
        }
        self.last = Instant::now();
        let now: Vec<_> = self.paths.iter().map(|p| stamp(p)).collect();
        if now == self.seen {
            return None;
        }
        self.seen = now;
        Some(self.apply())
    }

    /// Reload right away (after `/theme` or an applied draft).
    pub fn force(&mut self) -> Vec<String> {
        self.seen = self.paths.iter().map(|p| stamp(p)).collect();
        self.apply()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_file_overrides_user_file_and_tokens_apply() {
        let d = tempfile::tempdir().unwrap();
        let user = d.path().join("user.toml");
        let proj = d.path().join("proj.toml");
        std::fs::write(&user, "[theme]\nname = \"nebula\"\n[theme.colors]\naccent = \"#010203\"\nerror = \"red\"\n").unwrap();
        std::fs::write(&proj, "[theme.colors]\naccent = \"#0a0b0c\"\n").unwrap();
        let skin = load(&[user, proj]).unwrap();
        assert_eq!(skin.theme.name, "nebula");
        let (p, warn) = skin.palette();
        assert!(warn.is_empty(), "{warn:?}");
        assert_eq!(p.accent, ratatui_core::style::Color::Rgb(10, 11, 12));
        assert_eq!(p.error, ratatui_core::style::Color::Red);
        assert_eq!(p.code, palette::NEBULA.code);
    }

    #[test]
    fn bad_entries_warn_and_bad_files_report() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("ui.toml");
        std::fs::write(&f, "[theme]\nname = \"plaid\"\n[theme.colors]\nnope = \"#000000\"\naccent = \"sparkly\"\n").unwrap();
        let (_, warn) = load(std::slice::from_ref(&f)).unwrap().palette();
        assert_eq!(warn.len(), 3, "{warn:?}");
        std::fs::write(&f, "[theme\nname=").unwrap();
        assert!(load(&[f]).unwrap_err().contains("ui.toml"));
    }

    #[test]
    fn missing_files_mean_defaults() {
        let skin = load(&[PathBuf::from("/definitely/not/here.toml")]).unwrap();
        assert_eq!(skin, Skin::default());
        assert_eq!(skin.palette().0, palette::COSMOS);
    }
}
