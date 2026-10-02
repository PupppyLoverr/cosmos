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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;

pub use palette::Palette;

#[derive(Debug, Clone, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, Deserialize, PartialEq)]
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

#[derive(Debug, Clone, Deserialize, PartialEq)]
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

/// The whole `ui.toml`. Unknown keys are ignored on load (a file written
/// for a newer cosmostui still loads) but rejected by `/customise`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Skin {
    pub theme: ThemeCfg,
    pub spinner: SpinnerCfg,
    pub motion: MotionCfg,
}

impl Skin {
    /// The palette this skin resolves to, plus warnings for bad entries.
    pub fn palette(&self) -> (Palette, Vec<String>) {
        let mut warn = Vec::new();
        let mut p = palette::builtin(&self.theme.name).unwrap_or_else(|| {
            warn.push(format!("unknown theme {:?} — using cosmos (themes: {})", self.theme.name, palette::THEMES.join(", ")));
            palette::COSMOS
        });
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
}

/// The palette every style function reads.
pub fn palette() -> Palette {
    if let Some(p) = SCOPED.with(|c| c.get()) {
        return p;
    }
    with(|s| s.palette)
}

/// Run `f` with `p` as this thread's palette.
#[cfg(test)]
pub fn scoped<T>(p: Palette, f: impl FnOnce() -> T) -> T {
    let prev = SCOPED.with(|c| c.replace(Some(p)));
    let out = f();
    SCOPED.with(|c| c.set(prev));
    out
}

pub fn current() -> Arc<Skin> {
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
