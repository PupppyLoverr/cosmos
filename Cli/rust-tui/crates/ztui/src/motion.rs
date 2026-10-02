//! Motion: spinners, the thinking orb, the shimmer that sweeps across the
//! verb, and the fade-in of freshly streamed text. Everything here is a pure
//! function of wall-clock milliseconds, so the live region can be redrawn at
//! any rate and committed scrollback is never touched.

use ratatui_core::style::{Color, Modifier, Style};
use ratatui_core::text::Span;

use crate::skin::SpinnerCfg;

/// Built-in spinner styles.
pub const STYLES: &[&str] = &["orb", "twinkle", "braille", "moon", "comet", "pulse", "galaxy", "dots", "line"];

pub fn frames(style: &str) -> &'static [&'static str] {
    match style {
        "twinkle" => &["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"],
        "braille" => &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
        "moon" => &["◯", "◔", "◑", "◕", "●", "◕", "◑", "◔"],
        "comet" => &["✦··", "·✦·", "··✦", "·✦·"],
        "pulse" => &["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█", "▇", "▆", "▅", "▄", "▃", "▂"],
        "galaxy" => &["◜", "◠", "◝", "◞", "◡", "◟"],
        "dots" => &["⠁", "⠂", "⠄", "⡀", "⢀", "⠠", "⠐", "⠈"],
        "line" => &["-", "\\", "|", "/"],
        // orb: a glow that swells and settles
        _ => &["·", "∘", "○", "◎", "◉", "●", "◉", "◎", "○", "∘"],
    }
}

/// The frame for `ms`, from custom frames or the named style.
pub fn frame(cfg: &SpinnerCfg, ms: u128, reduce: bool) -> String {
    let fps = cfg.fps.clamp(1, 60) as u128;
    let i = if reduce { 0 } else { (ms * fps / 1000) as usize };
    if !cfg.frames.is_empty() {
        return cfg.frames[i % cfg.frames.len()].clone();
    }
    let f = frames(&cfg.style);
    f[i % f.len()].to_string()
}

/// Mix two colours; non-RGB colours do not blend (returns `a` until t ≥ .5).
pub fn lerp(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => {
            if t < 0.5 {
                a
            } else {
                b
            }
        }
    }
}

/// Toward white (or black for a dark target) by `t`.
pub fn brighten(c: Color, t: f32) -> Color {
    lerp(c, Color::Rgb(255, 255, 255), t)
}

pub fn darken(c: Color, t: f32) -> Color {
    lerp(c, Color::Rgb(0, 0, 0), t)
}

/// The spinner glyph; the orb also breathes in colour.
pub fn spinner_span(cfg: &SpinnerCfg, accent: Color, ms: u128, reduce: bool) -> Span<'static> {
    let f = frame(cfg, ms, reduce);
    let color = if reduce || cfg.style != "orb" || !cfg.frames.is_empty() {
        accent
    } else {
        // ~1.6 s breath
        let phase = (ms % 1600) as f32 / 1600.0 * std::f32::consts::TAU;
        lerp(darken(accent, 0.35), brighten(accent, 0.25), (phase.sin() + 1.0) / 2.0)
    };
    Span::styled(format!("{f} "), Style::new().fg(color))
}

/// `text` with a soft highlight sweeping across it (one span per char).
pub fn shimmer(text: &str, base: Color, ms: u128, on: bool) -> Vec<Span<'static>> {
    if !on || !matches!(base, Color::Rgb(..)) {
        return vec![Span::styled(text.to_string(), Style::new().fg(base))];
    }
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len() as f32;
    // the highlight crosses the text, then rests off-screen for a beat
    let period = 1400.0 + n * 40.0;
    let pos = (ms as f32 % period) / period * (n + 8.0) - 4.0;
    chars
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let d = (i as f32 - pos).abs();
            let glow = (1.0 - d / 3.0).max(0.0);
            Span::styled(c.to_string(), Style::new().fg(brighten(base, glow * 0.6)))
        })
        .collect()
}

/// Fade in the last `fresh` characters of a line: the newest are faintest.
pub fn fade_tail(spans: Vec<Span<'static>>, fresh: usize, dim: Color) -> Vec<Span<'static>> {
    if fresh == 0 {
        return spans;
    }
    let total: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let start = total.saturating_sub(fresh);
    let mut out = Vec::new();
    let mut idx = 0usize;
    for s in spans {
        let len = s.content.chars().count();
        if idx + len <= start {
            idx += len;
            out.push(s);
            continue;
        }
        for (k, c) in s.content.chars().enumerate() {
            let at = idx + k;
            if at < start {
                out.push(Span::styled(c.to_string(), s.style));
                continue;
            }
            // 0 = oldest fresh char (nearly settled), 1 = newest
            let age = (at - start + 1) as f32 / fresh.max(1) as f32;
            let target = s.style.fg.unwrap_or(Color::Rgb(0xd8, 0xd8, 0xd8));
            let mut st = s.style.fg(lerp(target, dim, age * 0.85));
            if age > 0.7 {
                st = st.add_modifier(Modifier::DIM);
            }
            out.push(Span::styled(c.to_string(), st));
        }
        idx += len;
    }
    out
}

/// (present, past) for a `Present:Past` verb entry.
pub fn verb_forms(entry: &str) -> (String, String) {
    match entry.split_once(':') {
        Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
        None => (entry.trim().to_string(), "Worked".to_string()),
    }
}

/// The turn's verb, stable for the whole turn (`seed` is fixed at its start).
pub fn pick_verb(cfg: &SpinnerCfg, seed: u64) -> (String, String) {
    if cfg.verbs.is_empty() {
        return ("Thinking".into(), "Thought".into());
    }
    verb_forms(&cfg.verbs[(seed as usize) % cfg.verbs.len()])
}

/// The verb while a tool runs.
pub fn tool_verb(cfg: &SpinnerCfg, tool: &str, label: &str) -> String {
    cfg.tool_verbs.get(tool).cloned().unwrap_or_else(|| format!("Running {}", label.to_lowercase()))
}

/// `9s`, `1m 05s`, `1h 02m`.
pub fn elapsed(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

pub fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn every_style_has_frames_and_custom_frames_win() {
        for s in STYLES {
            assert!(!frames(s).is_empty(), "{s}");
        }
        let mut cfg = SpinnerCfg::default();
        assert_eq!(frame(&cfg, 0, false), "·");
        cfg.frames = vec!["A".into(), "B".into()];
        cfg.fps = 10;
        assert_eq!(frame(&cfg, 0, false), "A");
        assert_eq!(frame(&cfg, 100, false), "B");
        assert_eq!(frame(&cfg, 100, true), "A", "reduced motion holds still");
    }

    #[test]
    fn shimmer_and_fade_keep_the_text() {
        let acc = Color::Rgb(100, 120, 255);
        let s = shimmer("Cosmosing…", acc, 700, true);
        assert_eq!(text(&s), "Cosmosing…");
        assert!(s.iter().any(|x| x.style.fg != Some(acc)), "something glows");
        assert_eq!(shimmer("x", acc, 0, false).len(), 1);
        let line = vec![Span::raw("hello "), Span::styled("world", Style::new().fg(acc))];
        let f = fade_tail(line, 4, Color::Rgb(60, 60, 60));
        assert_eq!(text(&f), "hello world");
        assert_eq!(f.last().unwrap().style.fg, Some(lerp(acc, Color::Rgb(60, 60, 60), 0.85)));
    }

    #[test]
    fn verbs_and_elapsed() {
        let cfg = SpinnerCfg::default();
        let (now, past) = pick_verb(&cfg, 2);
        assert_eq!((now.as_str(), past.as_str()), ("Cooking", "Cooked"));
        assert_eq!(pick_verb(&cfg, 2), pick_verb(&cfg, 2 + cfg.verbs.len() as u64));
        assert_eq!(verb_forms("Zooming"), ("Zooming".into(), "Worked".into()));
        assert_eq!(tool_verb(&cfg, "grep", "Search"), "Searching");
        assert_eq!(tool_verb(&cfg, "weird", "Weird"), "Running weird");
        assert_eq!(elapsed(9), "9s");
        assert_eq!(elapsed(65), "1m 05s");
        assert_eq!(elapsed(3720), "1h 02m");
    }

    #[test]
    fn lerp_blends_rgb() {
        assert_eq!(lerp(Color::Rgb(0, 0, 0), Color::Rgb(200, 100, 50), 0.5), Color::Rgb(100, 50, 25));
        assert_eq!(lerp(Color::Red, Color::Blue, 0.2), Color::Red);
    }
}
