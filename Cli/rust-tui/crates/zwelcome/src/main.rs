//! cosmostui-welcome — the first-run flow, as its own full-screen executable.
//!
//! It takes over the whole terminal (alternate screen). A soft bloom of sound
//! plays while you fly into a spiral galaxy; the field dissolves and its dust
//! rearranges into "welcome to cosmos"; then your name, your keys (BYOK or
//! ChatGPT sign-in) with a live model list fetched from each provider you
//! connect, and a one-screen how-to. When it's done the terminal is cleared
//! so only cosmostui is left on screen.
//!
//! cosmostui launches this on the first interactive start and deletes the
//! executable once it exits successfully (see `zcore::welcome`), so none of
//! the intro, synth or setup code stays behind in the binary you run daily.
//! Exit codes: 0 done, 130 (`zcore::welcome::QUIT`) Ctrl+C.

use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event as CEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;
use zcore::catalog::{ModelDef, ProviderDef};
use ztui::kit::{self, width_of, ACCENT, ACCENT_BOLD, DIM, GHOST, GREEN, RED, RULE};

const WHITE: Color = Color::Rgb(0xf4, 0xf4, 0xf4);
const SUBTITLE: &str = concat!("cosmostui v", env!("CARGO_PKG_VERSION"), " · the cosmos coding agent");
const QUESTION: &str = "what should i call you?";
const STEPS: [&str; 4] = ["name", "keys", "model", "start"];
const SPIN: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Providers floated to the top of the BYOK list, with a one-line pitch and
/// where to get a key. The rest keep catalog order and show their env var.
const PINNED: &[(&str, &str, &str)] = &[
    ("chatgpt", "use your Plus / Pro plan — no API key", ""),
    ("devin", "your Devin subscription — Cosmos runs the tools", ""),
    ("openrouter", "one key, 300+ models from every lab", "https://openrouter.ai/keys"),
    ("router9", "your local 9Router — routes the subscriptions you have", "http://localhost:20128 → API keys"),
    ("anthropic", "Claude", "https://console.anthropic.com/settings/keys"),
    ("openai", "GPT and o-series", "https://platform.openai.com/api-keys"),
    ("google", "Gemini", "https://aistudio.google.com/apikey"),
    ("xai", "Grok", "https://console.x.ai"),
    ("meta", "Muse", ""),
    ("groq", "very fast open models", "https://console.groq.com/keys"),
    ("deepseek", "DeepSeek V and R", "https://platform.deepseek.com/api_keys"),
    ("mistral", "Mistral and Codestral", "https://console.mistral.ai/api-keys"),
    ("xiaomi", "MiMo", ""),
    ("moonshotai", "Kimi", "https://platform.moonshot.ai"),
    ("zai", "GLM", "https://z.ai/manage-apikey"),
    ("cerebras", "fastest inference", "https://cloud.cerebras.ai"),
    ("minimax", "MiniMax", ""),
];

fn main() {
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        eprintln!("cosmostui-welcome needs a terminal — run `cosmostui` in one");
        std::process::exit(2);
    }
    let code = match flow() {
        Ok(true) => 0,
        Ok(false) => zcore::welcome::QUIT,
        Err(e) => {
            eprintln!("welcome: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

/// Ok(true) = completed, Ok(false) = Ctrl+C.
fn flow() -> Result<bool> {
    let mut tty = Tty::enter()?;
    let r = run(&mut tty);
    // done → wipe the terminal so cosmostui starts on a clean screen;
    // quit or error → put the shell back exactly as it was
    tty.leave(matches!(r, Ok(true)));
    r
}

// ── terminal ────────────────────────────────────────────────────────────────

/// Full-screen output on the alternate screen, redrawing only changed rows.
struct Tty {
    w: usize,
    h: usize,
    prev: Vec<Vec<u8>>,
}

impl Tty {
    fn enter() -> Result<Tty> {
        use crossterm::{cursor, event::EnableBracketedPaste, execute, terminal};
        terminal::enable_raw_mode()?;
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore(false);
            prev_hook(info);
        }));
        execute!(
            std::io::stdout(),
            terminal::EnterAlternateScreen,
            cursor::Hide,
            EnableBracketedPaste,
            terminal::Clear(terminal::ClearType::All)
        )?;
        let (w, h) = terminal::size()?;
        Ok(Tty { w: w as usize, h: h as usize, prev: Vec::new() })
    }

    fn resized(&mut self, w: u16, h: u16) {
        self.w = w as usize;
        self.h = h as usize;
        self.prev.clear();
    }

    fn draw(&mut self, c: &Canvas, cursor: Option<(usize, usize)>) -> Result<()> {
        use crossterm::cursor::{Hide, MoveTo, Show};
        use crossterm::terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate};
        use crossterm::QueueableCommand;
        let mut buf = Vec::with_capacity(16 * 1024);
        buf.queue(BeginSynchronizedUpdate)?;
        buf.queue(Hide)?;
        if self.prev.len() != c.h {
            buf.queue(Clear(ClearType::All))?;
            self.prev = vec![Vec::new(); c.h];
        }
        for y in 0..c.h {
            let mut row = Vec::with_capacity(c.w * 2);
            // never touch the bottom-right cell: some terminals scroll on it
            let max = if y + 1 == c.h { c.w.saturating_sub(1) } else { c.w };
            kit::write_line(&mut row, &c.row(y), max);
            if row != self.prev[y] {
                buf.queue(MoveTo(0, y as u16))?;
                buf.extend_from_slice(&row);
                self.prev[y] = row;
            }
        }
        if let Some((x, y)) = cursor {
            buf.queue(MoveTo(x as u16, y as u16))?;
            buf.queue(Show)?;
        }
        buf.queue(EndSynchronizedUpdate)?;
        let mut out = std::io::stdout();
        out.write_all(&buf)?;
        out.flush()?;
        Ok(())
    }

    fn leave(self, clear: bool) {
        restore(clear);
    }
}

fn restore(clear: bool) {
    use crossterm::{cursor, event::DisableBracketedPaste, execute, terminal};
    let mut out = std::io::stdout();
    let _ = execute!(out, DisableBracketedPaste, cursor::Show, terminal::LeaveAlternateScreen);
    if clear {
        let _ = execute!(
            out,
            terminal::Clear(terminal::ClearType::Purge),
            terminal::Clear(terminal::ClearType::All),
            cursor::MoveTo(0, 0)
        );
    }
    let _ = terminal::disable_raw_mode();
}

/// A grid of styled cells the size of the terminal. `'\0'` marks the second
/// column of a wide character.
struct Canvas {
    w: usize,
    h: usize,
    cells: Vec<(char, Style)>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Canvas {
        Canvas { w, h, cells: vec![(' ', Style::new()); w * h] }
    }

    fn set(&mut self, x: usize, y: usize, ch: char, st: Style) {
        if x < self.w && y < self.h {
            self.cells[y * self.w + x] = (ch, st);
        }
    }

    #[cfg(test)]
    fn get(&self, x: usize, y: usize) -> char {
        if x < self.w && y < self.h {
            self.cells[y * self.w + x].0
        } else {
            ' '
        }
    }

    /// Write `s` at (x, y), clipped at the right edge. Returns the next column.
    fn text(&mut self, x: usize, y: usize, s: &str, st: Style) -> usize {
        let mut cx = x;
        for ch in s.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if cw == 0 || ch.is_control() {
                continue;
            }
            if cx + cw > self.w {
                break;
            }
            self.set(cx, y, ch, st);
            if cw == 2 {
                self.set(cx + 1, y, '\0', st);
            }
            cx += cw;
        }
        cx
    }

    fn spans(&mut self, x: usize, y: usize, spans: &[Span<'_>]) -> usize {
        spans.iter().fold(x, |cx, s| self.text(cx, y, &s.content, s.style))
    }

    /// Spans centred on row `y`; returns their start column.
    fn center(&mut self, y: usize, spans: &[Span<'_>]) -> usize {
        let w: usize = spans.iter().map(|s| width_of(&s.content)).sum();
        let x = self.w.saturating_sub(w) / 2;
        self.spans(x, y, spans);
        x
    }

    /// Blank a rectangle (so background stars don't show through panels).
    fn clear(&mut self, x: usize, y: usize, w: usize, h: usize) {
        for yy in y..(y + h).min(self.h) {
            for xx in x..(x + w).min(self.w) {
                self.set(xx, yy, ' ', Style::new());
            }
        }
    }

    fn row(&self, y: usize) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for &(ch, st) in &self.cells[y * self.w..(y + 1) * self.w] {
            if ch == '\0' {
                continue;
            }
            match spans.last_mut() {
                Some(s) if s.style == st => s.content.to_mut().push(ch),
                _ => spans.push(Span::styled(ch.to_string(), st)),
            }
        }
        Line::from(spans)
    }

    #[cfg(test)]
    fn plain(&self) -> String {
        (0..self.h)
            .map(|y| self.row(y).spans.iter().map(|s| s.content.to_string()).collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ── config writes ───────────────────────────────────────────────────────────

/// Write `default_model = "<alias>"` at the top level of config.toml —
/// replacing the existing key if present, else inserting before any table.
fn set_default_model(alias: &str) -> Result<()> {
    let home = zcore::cli_home();
    std::fs::create_dir_all(&home)?;
    let file = home.join("config.toml");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let alias = alias.replace(['"', '\\'], "");
    let newline = format!("default_model = \"{alias}\"");
    let mut out: Vec<String> = Vec::new();
    let (mut done, mut in_table) = (false, false);
    for l in text.lines() {
        let t = l.trim_start();
        if t.starts_with('[') {
            in_table = true;
        }
        if !in_table && !done && t.starts_with("default_model") {
            out.push(newline.clone());
            done = true;
        } else {
            out.push(l.to_string());
        }
    }
    if !done {
        out.insert(0, newline);
    }
    std::fs::write(&file, format!("{}\n", out.join("\n")))?;
    Ok(())
}

/// Make `alias` the model cosmostui opens with: config.toml's `default_model`,
/// and the TUI's "last used" state, which would otherwise take precedence.
fn remember_model(alias: &str) -> Result<()> {
    set_default_model(alias)?;
    let state = zcore::home_file("state.json", "zcli_state.json");
    let mut v: serde_json::Value = std::fs::read_to_string(&state)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    v["model"] = serde_json::Value::from(alias);
    std::fs::write(state, v.to_string())?;
    Ok(())
}

// ── sound: a soft galaxy bloom ──────────────────────────────────────────────

fn smooth01(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

const SOUND_SECS: f32 = 4.4;

/// The launch sound, timed to the picture: a low swell as the galaxy comes
/// into view, air that opens up as you fly through it, a soft bloom when the
/// field bursts into dust (~2.9 s), pentatonic pings as the letters land and
/// a quiet chord under "welcome to cosmos".
fn synth(rate: u32) -> Vec<f32> {
    use std::f32::consts::{PI, TAU};
    const PINGS: &[(f32, f32)] = &[(3.05, 784.0), (3.22, 987.77), (3.40, 1174.7), (3.62, 1568.0), (3.88, 2093.0)];
    const CHORD: &[f32] = &[261.63, 392.0, 523.25, 659.25];
    let r = rate as f32;
    let n = (r * SOUND_SECS) as usize;
    let mut out = vec![0f32; n];
    let mut rng: u64 = 0x243f_6a88_85a3_08d3;
    let (mut lp, mut lp2, mut sub, mut boom) = (0f32, 0f32, 0f32, 0f32);
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / r;
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let white = (rng >> 40) as f32 / 16_777_216.0 * 2.0 - 1.0;
        let mut s = 0f32;
        // arrival: a low swell sliding 70 → 30 Hz
        if t < 1.6 {
            sub += (30.0 + 40.0 * (-1.8 * t).exp()) / r;
            s += (TAU * sub).sin() * 0.28 * smooth01(t / 0.35) * (-1.6 * t).exp();
        }
        // fly-through: noise through a lowpass that opens as you dive in
        if t < 3.1 {
            let u = t / 3.1;
            let cut = 180.0 + 5200.0 * u.powi(3);
            lp += (1.0 - (-TAU * cut / r).exp()) * (white - lp);
            s += lp * 0.55 * u.powf(2.2) * (1.0 - u).powf(0.5);
        }
        // the soft burst into dust
        if t >= 2.9 {
            let d = t - 2.9;
            boom += 48.0 / r;
            s += (TAU * boom).sin() * 0.3 * smooth01(d / 0.03) * (-3.0 * d).exp();
            let cut = 200.0 + 1400.0 * (-3.5 * d).exp();
            lp2 += (1.0 - (-TAU * cut / r).exp()) * (white - lp2);
            s += lp2 * 0.45 * smooth01(d / 0.02) * (-4.0 * d).exp();
        }
        // letters landing
        for &(pt, pf) in PINGS {
            if t >= pt {
                let d = t - pt;
                s += (TAU * pf * d).sin() * 0.07 * (-6.0 * d).exp();
            }
        }
        // a quiet chord under the title
        if t >= 3.0 {
            let d = t - 3.0;
            let env = smooth01(d / 0.5) * smooth01((SOUND_SECS - t) / 0.9);
            for (k, f) in CHORD.iter().enumerate() {
                s += (TAU * f * d + k as f32 * PI / 3.0).sin() * 0.022 * env;
            }
        }
        let fade = smooth01(t / 0.02) * smooth01((SOUND_SECS - t) / 0.3);
        *o = (s * fade * 1.4).tanh() * 0.85;
    }
    out
}

/// Set when the intro is skipped — playback fades out fast.
static HUSH: AtomicBool = AtomicBool::new(false);

/// Fire-and-forget playback on a thread; no output device → silence.
/// `COSMOS_QUIET=1` mutes it.
fn play_galaxy() {
    if zcore::env_var("QUIET").is_some() {
        return;
    }
    std::thread::spawn(|| {
        let _ = play();
    });
}

fn play() -> Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let Some(dev) = host.default_output_device() else {
        return Ok(());
    };
    let supported = dev.default_output_config()?;
    let config = supported.config();
    let ch = config.channels.max(1) as usize;
    let samples = Arc::new(synth(config.sample_rate));
    let pos = Arc::new(AtomicUsize::new(0));

    macro_rules! build {
        ($t:ty, $conv:expr) => {{
            let samples = samples.clone();
            let pos = pos.clone();
            let conv: fn(f32) -> $t = $conv;
            let mut gain = 1f32;
            dev.build_output_stream(
                config,
                move |data: &mut [$t], _: &cpal::OutputCallbackInfo| {
                    for frame in data.chunks_mut(ch) {
                        if HUSH.load(Ordering::Relaxed) {
                            gain *= 0.9993;
                        }
                        let i = pos.fetch_add(1, Ordering::Relaxed);
                        let v = conv(samples.get(i).copied().unwrap_or(0.0) * gain);
                        for c in frame.iter_mut() {
                            *c = v;
                        }
                    }
                },
                |_| {},
                None,
            )
        }};
    }
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build!(f32, |s| s),
        cpal::SampleFormat::F64 => build!(f64, |s| s as f64),
        cpal::SampleFormat::I16 => build!(i16, |s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16),
        cpal::SampleFormat::I32 => build!(i32, |s| (s.clamp(-1.0, 1.0) * i32::MAX as f32) as i32),
        cpal::SampleFormat::U16 => build!(u16, |s| ((s.clamp(-1.0, 1.0) * 0.5 + 0.5) * u16::MAX as f32) as u16),
        _ => return Ok(()),
    }?;
    stream.play()?;
    let total = samples.len();
    let t0 = Instant::now();
    while pos.load(Ordering::Relaxed) < total && t0.elapsed() < Duration::from_secs(8) {
        if HUSH.load(Ordering::Relaxed) && t0.elapsed() > Duration::from_millis(50) {
            // let the fade run ~150 ms, then stop
            std::thread::sleep(Duration::from_millis(150));
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Ok(())
}

// ── "welcome to cosmos" in star dust ───────────────────────────────────────

/// 5×8 bitmaps for the few letters the title needs: a 2-row ascender and a
/// 6-row x-height, so at 2× every pair of pixel rows fills exactly one
/// braille cell and the letters land crisp on the cell grid.
fn glyph(c: char) -> [&'static str; 8] {
    match c {
        'c' => [".....", ".....", ".###.", "#...#", "#....", "#....", "#...#", ".###."],
        'e' => [".....", ".....", ".###.", "#...#", "#####", "#....", "#...#", ".###."],
        'l' => [".##..", "..#..", "..#..", "..#..", "..#..", "..#..", "..#..", ".###."],
        'm' => [".....", ".....", "##.#.", "#.#.#", "#.#.#", "#.#.#", "#.#.#", "#.#.#"],
        'o' => [".....", ".....", ".###.", "#...#", "#...#", "#...#", "#...#", ".###."],
        's' => [".....", ".....", ".###.", "#....", ".###.", "....#", "#...#", ".###."],
        't' => [".#...", ".#...", "####.", ".#...", ".#...", ".#...", ".#..#", "..##."],
        'w' => [".....", ".....", "#...#", "#...#", "#...#", "#.#.#", "#.#.#", ".#.#."],
        _ => ["....."; 8],
    }
}

/// The title rendered to braille cells: `(rows, cols, cells)` where cells
/// are the lit `(col, row, char)`. Picks the biggest layout that fits.
fn title_art(w: usize, h: usize) -> (usize, usize, Vec<(usize, usize, char)>) {
    let layouts: [(&[&str], usize); 4] = [
        (&["welcome to cosmos"], 3),
        (&["welcome to cosmos"], 2),
        (&["welcome to", "cosmos"], 2),
        (&["welcome to cosmos"], 1),
    ];
    let line_px = |s: &str, k: usize| -> usize {
        let n: usize = s.chars().map(|c| if c == ' ' { 3 * k } else { 6 * k }).sum();
        n.saturating_sub(k)
    };
    let mut pick = layouts[3];
    for l in layouts {
        let wpx = l.0.iter().map(|s| line_px(s, l.1)).max().unwrap_or(0);
        let hpx = l.0.len() * 10 * l.1 - 2 * l.1;
        if wpx.div_ceil(2) + 2 <= w && hpx.div_ceil(4) <= h {
            pick = l;
            break;
        }
    }
    let (lines, k) = pick;
    let wpx = lines.iter().map(|s| line_px(s, k)).max().unwrap_or(0);
    let hpx = lines.len() * 10 * k - 2 * k;
    let mut dots = vec![vec![false; wpx]; hpx];
    for (li, s) in lines.iter().enumerate() {
        // centre on an even dot so every glyph stays on the braille cell grid
        let mut x = ((wpx - line_px(s, k)) / 2) & !1;
        let y0 = li * 10 * k;
        for c in s.chars() {
            if c == ' ' {
                x += 3 * k;
                continue;
            }
            for (gy, row) in glyph(c).iter().enumerate() {
                for (gx, b) in row.bytes().enumerate() {
                    if b == b'#' {
                        for dy in 0..k {
                            for dx in 0..k {
                                dots[y0 + gy * k + dy][x + gx * k + dx] = true;
                            }
                        }
                    }
                }
            }
            x += 6 * k;
        }
    }
    let (rows, cols) = (hpx.div_ceil(4), wpx.div_ceil(2));
    const BITS: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
    let mut cells = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let mut bits = 0;
            for (dx, col) in BITS.iter().enumerate() {
                for (dy, bit) in col.iter().enumerate() {
                    if dots.get(r * 4 + dy).and_then(|row| row.get(c * 2 + dx)).copied().unwrap_or(false) {
                        bits |= bit;
                    }
                }
            }
            if bits != 0 {
                cells.push((c, r, char::from_u32(0x2800 + bits).unwrap_or(' ')));
            }
        }
    }
    (rows, cols, cells)
}

// ── the star field ──────────────────────────────────────────────────────────

fn rand01(s: &mut u64) -> f32 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    (*s >> 40) as f32 / 16_777_216.0
}

struct Star {
    x: f32,
    y: f32,
    z: f32,
    v: f32,
    /// intrinsic brightness — the galaxy core glows
    l: f32,
    /// last projected position (screen cells)
    px: f32,
    py: f32,
    /// coalesce origin / target
    ox: f32,
    oy: f32,
    tx: f32,
    ty: f32,
    delay: f32,
    /// true = drifts off and fades instead of joining the title
    dissolve: bool,
}

fn gray(v: u8) -> Color {
    Color::Rgb(v, v, v)
}

fn star_style(b: f32) -> (char, Style) {
    if b > 0.85 {
        ('✶', Style::new().fg(WHITE).add_modifier(Modifier::BOLD))
    } else if b > 0.6 {
        ('✦', Style::new().fg(gray(0xd8)))
    } else if b > 0.35 {
        ('•', Style::new().fg(gray(0xa8)))
    } else {
        ('·', Style::new().fg(gray(0x66)))
    }
}

type Grid = Vec<Option<(char, Style, f32)>>;

/// Brightest-wins plot into a scratch grid `w` cells wide.
fn plot(grid: &mut Grid, w: usize, (x, y): (f32, f32), ch: char, st: Style, b: f32) {
    let h = grid.len() / w.max(1);
    if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
        return;
    }
    let cell = &mut grid[y as usize * w + x as usize];
    if cell.map(|c| c.2 < b).unwrap_or(true) {
        *cell = Some((ch, st, b));
    }
}

/// Copy a scratch star grid onto the canvas.
fn blit(c: &mut Canvas, grid: &Grid) {
    for (i, cell) in grid.iter().enumerate() {
        if let Some((ch, st, _)) = cell {
            c.set(i % c.w, i / c.w, *ch, *st);
        }
    }
}

fn star_count(w: usize, h: usize) -> usize {
    (w * h / 9).clamp(240, 640)
}

/// A two-armed spiral seen at an angle, far off in the middle of the field.
fn galaxy(rng: &mut u64, n: usize) -> Vec<Star> {
    use std::f32::consts::{PI, TAU};
    (0..n)
        .map(|i| {
            let core = i % 8 == 0;
            let r = if core { 0.07 * rand01(rng).powf(1.5) } else { 0.04 + 0.34 * rand01(rng).powf(0.9) };
            let th = if core {
                rand01(rng) * TAU
            } else {
                (i % 2) as f32 * PI + r * 9.0 + (rand01(rng) - 0.5) * 0.7
            };
            Star {
                x: r * th.cos(),
                y: r * th.sin() * 1.6,
                z: 0.92 + rand01(rng) * 0.08,
                v: 0.9 + rand01(rng) * 0.2,
                l: if core { 1.2 + 1.6 * (1.0 - r / 0.07).powi(2) } else { 0.7 + 0.9 * rand01(rng) * (1.0 - r) },
                px: 0.0,
                py: 0.0,
                ox: 0.0,
                oy: 0.0,
                tx: 0.0,
                ty: 0.0,
                delay: 0.0,
                dissolve: false,
            }
        })
        .collect()
}

/// Faint stars that twinkle behind the setup screens.
struct Twinkle {
    x: usize,
    y: usize,
    phase: f32,
    speed: f32,
    base: f32,
}

fn sky(w: usize, h: usize) -> Vec<Twinkle> {
    let mut rng = 0x5851_f42d_4c95_7f2d_u64 ^ (w as u64 * 131 + h as u64);
    (0..(w * h / 55).max(12))
        .map(|_| Twinkle {
            x: (rand01(&mut rng) * w as f32) as usize,
            y: (rand01(&mut rng) * h as f32) as usize,
            phase: rand01(&mut rng) * std::f32::consts::TAU,
            speed: 0.6 + rand01(&mut rng) * 1.8,
            base: 0.3 + rand01(&mut rng) * 0.7,
        })
        .collect()
}

// ── providers and models ────────────────────────────────────────────────────

struct Prov {
    def: &'static ProviderDef,
    /// short name ("ChatGPT", not "ChatGPT (Plus / Pro subscription)")
    name: String,
    pitch: String,
    key_url: &'static str,
}

impl Prov {
    fn sub(&self) -> bool {
        self.def.is_subscription()
    }
}

fn provider_list() -> Vec<Prov> {
    let mut v: Vec<Prov> = zcore::catalog::providers()
        .iter()
        .filter(|p| p.is_subscription() || !p.env.is_empty())
        .map(|p| {
            let pin = PINNED.iter().find(|x| x.0 == p.id);
            Prov {
                def: p,
                name: if p.is_subscription() { p.name.split(" (").next().unwrap_or(&p.name).to_string() } else { p.name.clone() },
                pitch: pin.map(|x| x.1.to_string()).unwrap_or_else(|| p.env.first().cloned().unwrap_or_default()),
                key_url: pin.map(|x| x.2).unwrap_or(""),
            }
        })
        .collect();
    let rank = |id: &str| PINNED.iter().position(|x| x.0 == id).unwrap_or(PINNED.len());
    v.sort_by_key(|p| rank(&p.def.id));
    v
}

type Fetcher = fn(&ProviderDef) -> Result<Vec<ModelDef>>;

enum OauthMsg {
    Url(String),
    Done(Result<zcore::chatgpt::Tokens>),
}

struct Oauth {
    rx: Receiver<OauthMsg>,
    cancel: Arc<AtomicBool>,
    url: Option<String>,
}

/// The model picker for one provider.
struct Pick {
    prov: usize,
    /// the live fetch, while it's still running
    rx: Option<Receiver<Result<Vec<ModelDef>>>>,
    models: Vec<ModelDef>,
    /// "N models live from host", or why we fell back to the built-in list
    source: String,
    live: bool,
    filter: Vec<char>,
    sel: usize,
}

fn kfmt(n: u64) -> String {
    if n >= 1_000_000 {
        let m = n as f64 / 1_048_576.0;
        if (m - m.round()).abs() < 0.05 {
            format!("{}M", m.round())
        } else {
            format!("{m:.1}M")
        }
    } else if n >= 1000 {
        format!("{}k", (n as f64 / 1000.0).round())
    } else {
        n.to_string()
    }
}

fn money(x: f64) -> String {
    let s = format!("{x:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("${s}")
}

fn price_label(p: &Prov, m: &ModelDef) -> String {
    if p.sub() {
        "included".into()
    } else if m.price[0] == 0.0 && m.price[1] == 0.0 {
        if p.def.id == "openrouter" || m.id.ends_with(":free") {
            "free".into()
        } else {
            String::new()
        }
    } else {
        format!("{} / {}", money(m.price[0]), money(m.price[1]))
    }
}

// ── the flow ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Warp,
    Coalesce,
    Name,
    Keys,
    KeyEntry(usize),
    Models,
    Ready,
}

struct Ob {
    phase: Phase,
    t0: Instant,
    /// wall clock for twinkles and spinners
    clock: Instant,
    w: usize,
    h: usize,
    stars: Vec<Star>,
    sky: Vec<Twinkle>,
    rot: f32,
    rng: u64,
    /// the title in braille dust and where it sits
    art: Vec<(usize, usize, char)>,
    art_rows: usize,
    gx: usize,
    gy: usize,

    name: Vec<char>,
    nc: usize,

    provs: Vec<Prov>,
    filter: Vec<char>,
    sel: usize,
    key_buf: Vec<char>,
    /// provider whose key was typed in this run and not yet verified
    unverified: Option<usize>,
    oauth: Option<Oauth>,
    keyed: HashSet<String>,
    oauth_email: Option<String>,
    pick: Option<Pick>,
    fetcher: Fetcher,
    /// model chosen per provider in this run
    chosen: HashMap<String, String>,
    /// providers connected in this run, in order — preferred for the default
    connected: Vec<String>,
    /// the default cosmostui will open with: (alias, set in this run)
    model: Option<(String, bool)>,

    status: Option<(String, Style, Instant)>,
    done: bool,
    quit: bool,
}

impl Ob {
    fn sized(w: usize, h: usize) -> Ob {
        let mut rng = 0x9e37_79b9_7f4a_7c15_u64;
        let stars = galaxy(&mut rng, star_count(w, h));
        let mut ob = Ob {
            phase: Phase::Warp,
            t0: Instant::now(),
            clock: Instant::now(),
            w,
            h,
            stars,
            sky: sky(w, h),
            rot: 0.0,
            rng,
            art: Vec::new(),
            art_rows: 0,
            gx: 0,
            gy: 0,
            name: Vec::new(),
            nc: 0,
            provs: provider_list(),
            filter: Vec::new(),
            sel: 0,
            key_buf: Vec::new(),
            unverified: None,
            oauth: None,
            keyed: HashSet::new(),
            oauth_email: None,
            pick: None,
            fetcher: zcore::live::fetch,
            chosen: HashMap::new(),
            connected: Vec::new(),
            model: None,
            status: None,
            done: false,
            quit: false,
        };
        ob.layout_title();
        ob.refresh_keyed();
        ob
    }

    /// Windows reports a (same-size) resize as soon as the alternate screen
    /// opens, so this must never cut the intro short: equal sizes are
    /// ignored, the warp simply reprojects, and a coalesce in flight is
    /// re-aimed at the title's new position.
    fn resized(&mut self, w: usize, h: usize) {
        if (w, h) == (self.w, self.h) {
            return;
        }
        self.w = w;
        self.h = h;
        self.sky = sky(w, h);
        self.layout_title();
        if self.phase == Phase::Coalesce {
            self.begin_coalesce();
        }
    }

    /// Place the title so title + subtitle + name prompt sit centred.
    fn layout_title(&mut self) {
        let (rows, cols, art) = title_art(self.w.saturating_sub(4), self.h.saturating_sub(14));
        self.art = art;
        self.art_rows = rows;
        self.gx = self.w.saturating_sub(cols) / 2;
        self.gy = self.h.saturating_sub(rows + 9) / 2;
    }

    fn refresh_keyed(&mut self) {
        self.keyed = self
            .provs
            .iter()
            .filter(|p| {
                match p.def.kind.as_str() {
                    "chatgpt" => zcore::chatgpt::load().is_some(),
                    "devin" => zcore::devin::known_ready(),
                    _ => zcore::creds::key_for(&p.def.id, &p.def.env).is_some(),
                }
            })
            .map(|p| p.def.id.clone())
            .collect();
        self.oauth_email = zcore::chatgpt::load().and_then(|t| t.email);
    }

    fn flash(&mut self, s: impl Into<String>, ok: bool) {
        self.status = Some((s.into(), if ok { kit::fg(GREEN) } else { kit::fg(RED) }, Instant::now()));
    }

    fn secs(&self) -> f32 {
        self.clock.elapsed().as_secs_f32()
    }

    fn spinner(&self) -> char {
        SPIN[(self.clock.elapsed().as_millis() / 80) as usize % SPIN.len()]
    }

    // ---- intro animation ---------------------------------------------------

    fn begin_coalesce(&mut self) {
        let (cx, cy) = (self.w as f32 / 2.0, self.h as f32 / 2.0);
        let n = self.art.len();
        let targets = n.max(self.stars.len() * 3 / 4);
        for (i, s) in self.stars.iter_mut().enumerate() {
            // stars that flew off-screen re-enter from a random edge
            let on = s.px >= 0.0 && s.px < self.w as f32 && s.py >= 0.0 && s.py < self.h as f32;
            if on {
                s.ox = s.px;
                s.oy = s.py;
            } else {
                let edge = (rand01(&mut self.rng) * 4.0) as usize;
                s.ox = match edge {
                    0 => 0.0,
                    1 => self.w as f32 - 1.0,
                    _ => rand01(&mut self.rng) * self.w as f32,
                };
                s.oy = match edge {
                    2 => 0.0,
                    3 => self.h as f32 - 1.0,
                    _ => rand01(&mut self.rng) * self.h as f32,
                };
            }
            s.delay = rand01(&mut self.rng) * 0.5;
            if n > 0 && i < targets {
                // spread the particles evenly over the letters
                let (c, r, _) = self.art[(i * 7919) % n];
                s.tx = (self.gx + c) as f32 + 0.5;
                s.ty = (self.gy + r) as f32 + 0.5;
                s.dissolve = false;
            } else {
                // the rest of the field dissolves — drifts out and fades
                s.dissolve = true;
                s.tx = s.ox + (s.ox - cx) * 0.6;
                s.ty = s.oy + (s.oy - cy) * 0.6;
            }
        }
        self.phase = Phase::Coalesce;
        self.t0 = Instant::now();
    }

    fn draw_warp(&mut self, c: &mut Canvas, dt: f32) {
        let t = self.t0.elapsed().as_secs_f32();
        if t > 2.9 {
            self.begin_coalesce();
            return self.draw_coalesce(c);
        }
        // drift toward the galaxy, then dive through it
        let u = smooth01(t / 2.6);
        let speed = 0.04 + 1.7 * u * u;
        self.rot += dt * (0.35 + 0.8 * u);
        let (sn, cs) = self.rot.sin_cos();
        let (w, h) = (self.w, self.h);
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        // cells are ~2:1, so keep the disc's aspect on tall screens
        let sy = cy.min(cx * 0.45);
        let mut grid: Grid = vec![None; w * h];
        for s in &mut self.stars {
            s.z -= speed * dt * s.v;
            if s.z < 0.07 {
                // passed the camera — respawn deep in the field
                s.x = rand01(&mut self.rng) * 2.0 - 1.0;
                s.y = rand01(&mut self.rng) * 1.1 - 0.55;
                s.z = 0.55 + rand01(&mut self.rng) * 0.45;
                s.l = 1.0;
            }
            let proj = |z: f32| {
                let rx = s.x * cs - s.y * sn;
                let ry = s.x * sn + s.y * cs;
                (cx + (rx / z) * cx * 0.92, cy + (ry / z) * sy * 0.92)
            };
            let (hx, hy) = proj(s.z);
            s.px = hx;
            s.py = hy;
            let (tx, ty) = proj(s.z + speed * dt * s.v * 3.5);
            let b = (0.32 * s.l / s.z).min(1.0);
            let (ch, st) = star_style(b);
            // streak: head plus a short fading tail
            let dist = ((hx - tx).hypot(hy - ty)).round() as usize;
            let n = dist.clamp(1, 5);
            for k in 0..=n {
                let f = k as f32 / n as f32;
                let bk = b * (1.0 - f * 0.7);
                let (ck, sk) = if k == 0 { (ch, st) } else { star_style(bk) };
                plot(&mut grid, w, (hx + (tx - hx) * f, hy + (ty - hy) * f), ck, sk, bk);
            }
        }
        blit(c, &grid);
        if t > 0.6 {
            let hint = "any key to skip";
            c.text(w.saturating_sub(width_of(hint) + 2), h.saturating_sub(2), hint, GHOST);
        }
    }

    fn draw_coalesce(&mut self, c: &mut Canvas) {
        let t = self.t0.elapsed().as_secs_f32();
        if t > 2.0 {
            self.begin_name();
            self.draw_name(c);
            return;
        }
        let (w, h) = (self.w, self.h);
        let mut grid: Grid = vec![None; w * h];
        for s in &mut self.stars {
            let e = smooth01((t - s.delay) / 0.9);
            let x = s.ox + (s.tx - s.ox) * e;
            let y = s.oy + (s.ty - s.oy) * e;
            s.px = x;
            s.py = y;
            let (b, ch) = if s.dissolve {
                (0.5 * (1.0 - e), '·')
            } else {
                (0.35 + 0.65 * e, if e > 0.75 { '✦' } else { '·' })
            };
            if b > 0.03 {
                plot(&mut grid, w, (x, y), ch, star_style(b).1, b);
            }
        }
        blit(c, &grid);
        // the dust settles into crisp letters, left to right
        if t > 1.0 {
            let reveal = ((t - 1.0) / 0.5).min(1.0);
            let span = self.art.iter().map(|a| a.0).max().unwrap_or(0) as f32 + 1.0;
            let st = kit::bold(Style::new().fg(WHITE));
            for &(x, y, ch) in &self.art {
                if (x as f32) < span * reveal {
                    c.set(self.gx + x, self.gy + y, ch, st);
                }
            }
        }
        if t > 1.3 {
            let n = ((t - 1.3) / 0.012) as usize;
            let y = self.gy + self.art_rows + 1;
            let x = w.saturating_sub(width_of(SUBTITLE)) / 2;
            c.text(x, y, &SUBTITLE.chars().take(n).collect::<String>(), DIM);
        }
    }

    fn begin_name(&mut self) {
        HUSH.store(true, Ordering::Relaxed);
        self.phase = Phase::Name;
        self.t0 = Instant::now();
    }

    // ---- shared chrome -----------------------------------------------------

    fn draw_sky(&self, c: &mut Canvas) {
        let t = self.secs();
        for s in &self.sky {
            let b = s.base * (0.5 + 0.5 * (t * s.speed + s.phase).sin());
            let (ch, v) = if b > 0.85 {
                ('✦', 0x70)
            } else if b > 0.45 {
                ('·', 0x5a)
            } else {
                ('·', 0x36)
            };
            c.set(s.x, s.y, ch, Style::new().fg(gray(v)));
        }
    }

    /// "✶ cosmos" on the left, the step you're on at the right.
    fn draw_header(&self, c: &mut Canvas, step: usize) {
        c.clear(0, 0, c.w, 3);
        c.text(3, 1, "✶ ", kit::fg(ACCENT));
        c.text(5, 1, "cosmos", kit::bold(Style::new()));
        let mut spans: Vec<Span> = Vec::new();
        for (i, s) in STEPS.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" ─ ", RULE));
            }
            spans.push(match i.cmp(&step) {
                std::cmp::Ordering::Less => Span::styled(format!("✓ {s}"), kit::fg(GREEN)),
                std::cmp::Ordering::Equal => Span::styled(s.to_string(), ACCENT_BOLD),
                std::cmp::Ordering::Greater => Span::styled(s.to_string(), DIM),
            });
        }
        let sw: usize = spans.iter().map(|s| width_of(&s.content)).sum();
        if c.w > sw + 20 {
            let x = c.w - sw - 3;
            c.spans(x, 1, &spans);
        }
    }

    fn draw_footer(&self, c: &mut Canvas, hint: &str) {
        let y = c.h.saturating_sub(2);
        c.clear(0, y, c.w, 1);
        c.center(y, &[Span::styled(hint.to_string(), DIM)]);
    }

    fn draw_status(&self, c: &mut Canvas, x: usize, y: usize, w: usize) {
        if let Some((s, st, at)) = &self.status {
            if at.elapsed() < Duration::from_secs(8) {
                c.text(x, y, &kit::ellipsize(s, w), *st);
            }
        }
    }

    /// The centred content column: (x, width).
    fn column(&self) -> (usize, usize) {
        let cw = self.w.saturating_sub(8).min(100);
        ((self.w - cw) / 2, cw)
    }

    // ---- name --------------------------------------------------------------

    fn draw_title(&self, c: &mut Canvas) {
        // a slow shimmer sweeps across the letters
        let span = self.art.iter().map(|a| a.0).max().unwrap_or(1) as f32;
        let pos = (self.secs() * 0.35).fract() * (span + 40.0) - 20.0;
        c.clear(self.gx.saturating_sub(2), self.gy.saturating_sub(1), span as usize + 5, self.art_rows + 3);
        for &(x, y, ch) in &self.art {
            let d = (x as f32 - pos) / 6.0;
            let v = 200.0 + 55.0 * (-d * d).exp();
            c.set(self.gx + x, self.gy + y, ch, kit::bold(Style::new().fg(gray(v as u8))));
        }
        c.center(self.gy + self.art_rows + 1, &[Span::styled(SUBTITLE, DIM)]);
    }

    fn draw_name(&mut self, c: &mut Canvas) -> Option<(usize, usize)> {
        self.draw_sky(c);
        self.draw_title(c);
        let y = self.gy + self.art_rows + 4;
        let bw = 34.min(self.w.saturating_sub(4));
        let x = self.w.saturating_sub(bw) / 2;
        c.clear(x, y, bw, 4);
        c.center(y, &[Span::styled(QUESTION, kit::bold(Style::new()))]);
        let text: String = self.name.iter().collect();
        c.text(x, y + 2, "❯ ", ACCENT_BOLD);
        c.text(x + 2, y + 2, &text, Style::new());
        c.text(x, y + 3, &"─".repeat(bw), RULE);
        self.draw_footer(c, "enter to continue · esc to skip");
        let col = x + 2 + width_of(&self.name[..self.nc].iter().collect::<String>());
        Some((col.min(self.w.saturating_sub(1)), y + 2))
    }

    // ---- keys --------------------------------------------------------------

    fn prov_order(&self) -> Vec<usize> {
        let q: String = self.filter.iter().collect();
        kit::rank(&self.provs, &q, |p| format!("{} {} {}", p.def.id, p.name, p.pitch))
    }

    fn draw_keys(&mut self, c: &mut Canvas) -> Option<(usize, usize)> {
        self.harvest_oauth();
        if self.phase == Phase::Models {
            return self.draw_models(c);
        }
        self.draw_sky(c);
        self.draw_header(c, 1);
        let (x, cw) = self.column();
        let top = 4;
        c.clear(x.saturating_sub(2), top, cw + 4, c.h.saturating_sub(top + 3));
        let n = self.keyed.len();
        c.text(x, top, "Bring your own keys", kit::bold(Style::new()));
        if n > 0 {
            let s = format!("{n} connected");
            c.text(x + cw.saturating_sub(width_of(&s)), top, &s, kit::fg(GREEN));
        }
        c.text(
            x,
            top + 1,
            &kit::ellipsize("Keys stay on this machine, in your OS keychain. Connect as many as you like.", cw),
            DIM,
        );

        if let Some(o) = &self.oauth {
            let y = top + 4;
            c.spans(
                x,
                y,
                &[Span::styled(format!("{} ", self.spinner()), kit::fg(ACCENT)), Span::raw("Signing in with ChatGPT — finish in your browser")],
            );
            if let Some(u) = &o.url {
                c.text(x + 2, y + 2, "If it didn't open, visit:", DIM);
                c.text(x + 2, y + 3, u, kit::fg(ACCENT));
            }
            self.draw_footer(c, "esc cancel");
            return None;
        }

        if let Phase::KeyEntry(i) = self.phase {
            let p = &self.provs[i];
            let y = top + 4;
            c.text(x, y, &format!("{} API key", p.name), kit::bold(Style::new()));
            c.text(x, y + 1, "Paste or type it — it's hidden as you type.", DIM);
            c.text(x, y + 3, "❯ ", ACCENT_BOLD);
            let dots = "•".repeat(self.key_buf.len().min(cw.saturating_sub(4)));
            c.text(x + 2, y + 3, &dots, Style::new());
            c.text(x, y + 4, &"─".repeat(cw.min(64)), RULE);
            let mut yy = y + 6;
            if !p.key_url.is_empty() {
                c.spans(x, yy, &[Span::styled("get one at ", DIM), Span::styled(p.key_url, kit::fg(ACCENT))]);
                yy += 1;
            }
            if let Some(e) = p.def.env.first() {
                c.text(x, yy, &format!("or set {e} in your environment"), DIM);
            }
            self.draw_status(c, x, yy + 2, cw);
            self.draw_footer(c, "enter save · esc back");
            return Some((x + 2 + dots.chars().count(), y + 3));
        }

        let q: String = self.filter.iter().collect();
        let fy = top + 3;
        c.text(x, fy, "/ ", DIM);
        if q.is_empty() {
            c.text(x + 2, fy, &format!("Type to search {} providers", self.provs.len()), GHOST);
        } else {
            c.text(x + 2, fy, &q, Style::new());
        }
        c.text(x, fy + 1, &"─".repeat(cw), RULE);

        let order = self.prov_order();
        let items = order.len() + 1; // + continue row
        self.sel = self.sel.min(items - 1);
        let list_y = fy + 2;
        let rows = c.h.saturating_sub(list_y + 6).max(3);
        let first = self.sel.saturating_sub(rows - 1);
        let name_w = self.provs.iter().map(|p| width_of(&p.name)).max().unwrap_or(20).min(30) + 2;
        for (r, vi) in (first..items.min(first + rows)).enumerate() {
            let y = list_y + r;
            let on = vi == self.sel;
            c.text(x, y, if on { "❯ " } else { "  " }, ACCENT_BOLD);
            if vi == order.len() {
                let label = if self.model.is_some() || !self.keyed.is_empty() { "Continue →" } else { "Skip for now →" };
                c.text(x + 2, y, label, if on { ACCENT_BOLD } else { DIM });
                continue;
            }
            let p = &self.provs[order[vi]];
            c.text(x + 2, y, &kit::ellipsize(&p.name, name_w - 2), if on { ACCENT_BOLD } else { Style::new() });
            // right side: connection and the model chosen for it
            let right = if let Some(m) = self.chosen.get(&p.def.id) {
                let alias = format!("{}/{m}", p.def.id);
                let star = if self.model.as_ref().map(|d| &d.0) == Some(&alias) { " ★" } else { "" };
                format!("✓ {m}{star}")
            } else if self.keyed.contains(&p.def.id) {
                match (p.def.kind.as_str(), &self.oauth_email) {
                    ("chatgpt", Some(e)) => format!("✓ {e}"),
                    ("devin", _) => match zcore::devin::peek() {
                        Some((true, d)) => format!("✓ {d}"),
                        _ => "✓ connected".into(),
                    },
                    _ => "✓ connected".into(),
                }
            } else {
                String::new()
            };
            let rw = width_of(&right).min(cw / 2);
            let pitch_w = cw.saturating_sub(2 + name_w + rw + 2);
            c.text(x + 2 + name_w, y, &kit::ellipsize(&p.pitch, pitch_w), DIM);
            if rw > 0 {
                c.text(x + cw - rw, y, &kit::ellipsize(&right, rw), kit::fg(GREEN));
            }
        }
        let below = items.saturating_sub(first + rows);
        let by = list_y + rows;
        c.text(x, by, &"─".repeat(cw), RULE);
        if below > 0 {
            c.text(x + 2, by, &format!(" ↓ {below} more "), DIM);
        }
        self.draw_status(c, x, by + 1, cw);
        self.draw_footer(c, "↑↓ choose · enter connect or pick a model · type to search · esc continue");
        Some((x + 2 + width_of(&q), fy))
    }

    fn harvest_oauth(&mut self) {
        let Some(o) = &mut self.oauth else { return };
        let mut done = None;
        while let Ok(m) = o.rx.try_recv() {
            match m {
                OauthMsg::Url(u) => o.url = Some(u),
                OauthMsg::Done(r) => {
                    done = Some(r);
                    break;
                }
            }
        }
        if let Some(r) = done {
            self.oauth = None;
            match r {
                Ok(t) => {
                    self.refresh_keyed();
                    let who = t.email.map(|e| format!(" as {e}")).unwrap_or_default();
                    self.flash(format!("✓ signed in{who}"), true);
                    self.connected.push("chatgpt".into());
                    if let Some(i) = self.provs.iter().position(|p| p.def.kind == "chatgpt") {
                        self.open_models(i);
                    }
                }
                Err(e) => self.flash(format!("✗ sign-in failed: {e:#}"), false),
            }
        }
    }

    // ---- models ------------------------------------------------------------

    /// Fetch `provs[i]`'s live model list in the background and open the picker.
    fn open_models(&mut self, i: usize) {
        let (tx, rx) = channel();
        let def = self.provs[i].def;
        let f = self.fetcher;
        std::thread::spawn(move || {
            let _ = tx.send(f(def));
        });
        self.pick = Some(Pick { prov: i, rx: Some(rx), models: Vec::new(), source: String::new(), live: false, filter: Vec::new(), sel: 0 });
        self.phase = Phase::Models;
    }

    /// Land a finished fetch: the live list, or the built-in one with the
    /// reason — or, when the provider rejects a key typed just now, drop it
    /// and go back to key entry.
    fn harvest_models(&mut self) {
        let Some(pk) = &mut self.pick else { return };
        let Some(rx) = &pk.rx else { return };
        let Ok(r) = rx.try_recv() else { return };
        pk.rx = None;
        let i = pk.prov;
        let def = self.provs[i].def;
        match r {
            Ok(models) => {
                let _ = zcore::live::store(&def.id, &models);
                pk.source = format!("{} models live from {}", models.len(), def.host());
                pk.models = models;
                pk.live = true;
                pk.sel = 0;
                if self.unverified == Some(i) {
                    self.unverified = None;
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains("rejected the key") && self.unverified == Some(i) {
                    let _ = zcore::creds::delete_key(&def.id);
                    self.unverified = None;
                    self.refresh_keyed();
                    self.pick = None;
                    self.key_buf.clear();
                    self.phase = Phase::KeyEntry(i);
                    self.flash(format!("✗ {msg} — check it and try again"), false);
                    return;
                }
                pk.source = format!("couldn't fetch the live list ({msg}) — showing known models");
                pk.models = def.models.clone();
                pk.live = false;
                pk.sel = 0;
                self.unverified = None;
            }
        }
    }

    fn model_order(&self) -> Vec<usize> {
        let Some(pk) = &self.pick else { return Vec::new() };
        let q: String = pk.filter.iter().collect();
        kit::rank(&pk.models, &q, |m| format!("{} {}", m.id, m.name))
    }

    fn draw_models(&mut self, c: &mut Canvas) -> Option<(usize, usize)> {
        self.harvest_models();
        if self.phase != Phase::Models {
            return self.draw_keys(c);
        }
        self.draw_sky(c);
        self.draw_header(c, 2);
        let (x, cw) = self.column();
        let top = 4;
        c.clear(x.saturating_sub(2), top, cw + 4, c.h.saturating_sub(top + 3));
        let spin = self.spinner();
        let order = self.model_order();
        let pk = self.pick.as_ref()?;
        let p = &self.provs[pk.prov];
        c.text(x, top, &format!("Choose a model — {}", p.name), kit::bold(Style::new()));

        if pk.rx.is_some() {
            c.spans(
                x,
                top + 1,
                &[Span::styled(format!("{spin} "), kit::fg(ACCENT)), Span::styled(format!("fetching live models from {}…", p.def.host()), DIM)],
            );
            self.draw_status(c, x, top + 3, cw);
            self.draw_footer(c, "esc back");
            return None;
        }
        c.text(
            x,
            top + 1,
            &kit::ellipsize(&format!("{} · the one you pick becomes your default", pk.source), cw),
            if pk.live { DIM } else { kit::fg(RED) },
        );

        let q: String = pk.filter.iter().collect();
        let fy = top + 3;
        c.text(x, fy, "/ ", DIM);
        if q.is_empty() {
            c.text(x + 2, fy, &format!("Type to search {} models", pk.models.len()), GHOST);
        } else {
            c.text(x + 2, fy, &q, Style::new());
        }
        // columns: model id · context · price
        let (ctx_w, price_w) = (7, 17);
        let id_w = cw.saturating_sub(2 + ctx_w + price_w + 4);
        let (cx_ctx, cx_price) = (x + 2 + id_w + 2, x + 2 + id_w + 4 + ctx_w);
        c.text(x + 2, fy + 1, "model", DIM);
        c.text(cx_ctx, fy + 1, &format!("{:>ctx_w$}", "context"), DIM);
        c.text(cx_price, fy + 1, &format!("{:>price_w$}", "$/M in / out"), DIM);
        c.text(x, fy + 2, &"─".repeat(cw), RULE);

        let list_y = fy + 3;
        let rows = c.h.saturating_sub(list_y + 6).max(3);
        let sel = pk.sel.min(order.len().saturating_sub(1));
        let first = sel.saturating_sub(rows - 1);
        let current = self.model.as_ref().map(|m| m.0.clone());
        for (r, vi) in (first..order.len().min(first + rows)).enumerate() {
            let y = list_y + r;
            let on = vi == sel;
            let m = &pk.models[order[vi]];
            c.text(x, y, if on { "❯ " } else { "  " }, ACCENT_BOLD);
            let alias = format!("{}/{}", p.def.id, m.id);
            let mark = if current.as_deref() == Some(alias.as_str()) { " ★" } else { "" };
            c.text(x + 2, y, &kit::ellipsize(&format!("{}{mark}", m.id), id_w), if on { ACCENT_BOLD } else { Style::new() });
            c.text(cx_ctx, y, &format!("{:>ctx_w$}", kfmt(m.context)), DIM);
            let price = kit::ellipsize(&price_label(p, m), price_w);
            c.text(cx_price, y, &format!("{price:>price_w$}"), DIM);
        }
        if order.is_empty() {
            c.text(x + 2, list_y, "no model matches", DIM);
        }
        let by = list_y + rows;
        c.text(x, by, &"─".repeat(cw), RULE);
        let below = order.len().saturating_sub(first + rows);
        if below > 0 {
            c.text(x + 2, by, &format!(" ↓ {below} more "), DIM);
        }
        if let Some(m) = order.get(sel).map(|&i| &pk.models[i]) {
            if m.name != m.id {
                c.text(x, by + 1, &kit::ellipsize(&m.name, cw), DIM);
            }
        }
        self.draw_footer(c, "↑↓ choose · enter use this model · type to search · esc back");
        Some((x + 2 + width_of(&q), fy))
    }

    fn choose_model(&mut self) {
        let order = self.model_order();
        let Some(pk) = &self.pick else { return };
        let Some(&mi) = order.get(pk.sel.min(order.len().saturating_sub(1))) else { return };
        let (prov, id) = (pk.prov, pk.models[mi].id.clone());
        let def = self.provs[prov].def;
        let alias = format!("{}/{id}", def.id);
        match remember_model(&alias) {
            Ok(()) => {
                self.chosen.insert(def.id.clone(), id);
                self.model = Some((alias.clone(), true));
                self.flash(format!("✓ default model → {alias}"), true);
            }
            Err(e) => self.flash(format!("✗ couldn't write config.toml: {e:#}"), false),
        }
        self.pick = None;
        self.phase = Phase::Keys;
        // back on the full list, cursor on the provider just set up
        self.filter.clear();
        self.sel = self.prov_order().iter().position(|&i| i == prov).unwrap_or(0);
    }

    // ---- ready -------------------------------------------------------------

    /// Leaving the keys screen. If nothing was chosen, keep a real configured
    /// default, else take the best model of a connected provider.
    fn after_keys(&mut self) {
        if self.model.is_none() {
            let cfg = zcore::config::Config::load(&zcore::cli_home());
            // `fixture` is the offline placeholder Config falls back to — not a choice
            if let Some(d) = cfg.default_model.clone().filter(|d| d != "fixture") {
                self.model = Some((d, false));
            } else if let Some(a) = {
                // best model of the first provider connected now, else of any keyed one
                let models = cfg.available_models();
                self.connected
                    .iter()
                    .find_map(|pid| models.iter().find(|m| !m.from_config && &m.provider == pid))
                    .or_else(|| models.iter().find(|m| !m.from_config && self.keyed.contains(&m.provider)))
                    .map(|m| m.alias.clone())
            } {
                match remember_model(&a) {
                    Ok(()) => self.model = Some((a, true)),
                    Err(e) => self.flash(format!("✗ couldn't write config.toml: {e:#}"), false),
                }
            }
        }
        self.phase = Phase::Ready;
    }

    fn draw_ready(&self, c: &mut Canvas) {
        const ROWS: &[(&str, &str)] = &[
            ("type a task, enter", "Cosmos reads, edits and runs code with you"),
            ("/", "commands — /model /connect /usage /context /help"),
            ("@path", "attach a file · !cmd runs a shell command"),
            ("ctrl+space", "dictate instead of typing"),
            ("shift+tab", "cycle plan → agent → yolo"),
            ("esc", "interrupt the agent · ctrl+c twice quits"),
            ("cosmostui -p \"…\"", "headless runs for scripts and CI"),
        ];
        self.draw_sky(c);
        self.draw_header(c, 3);
        let name: String = self.name.iter().collect::<String>().trim().to_string();
        let kw = ROWS.iter().map(|(k, _)| width_of(k)).max().unwrap_or(0);
        let dw = ROWS.iter().map(|(_, d)| width_of(d)).max().unwrap_or(0);
        let bw = (kw + 3 + dw).max(60).min(self.w.saturating_sub(4));
        let bh = ROWS.len() + 7;
        let x = self.w.saturating_sub(bw) / 2;
        let y = self.h.saturating_sub(bh) / 2;
        c.clear(x.saturating_sub(2), y.saturating_sub(1), bw + 4, bh + 2);
        let hello = if name.is_empty() { "You're all set.".to_string() } else { format!("You're all set, {name}.") };
        c.spans(x, y, &[Span::styled("✶ ", kit::fg(ACCENT)), Span::styled(hello, kit::bold(Style::new()))]);
        match &self.model {
            Some((m, _)) => {
                c.spans(x, y + 2, &[Span::styled("model  ", DIM), Span::styled(m.clone(), kit::bold(Style::new())), Span::styled("  · /model to switch", DIM)]);
            }
            None => {
                c.text(x, y + 2, "no provider yet — /connect or /login inside Cosmos when you're ready", DIM);
            }
        }
        let keys: Vec<&str> = self.provs.iter().filter(|p| self.keyed.contains(&p.def.id)).map(|p| p.name.as_str()).collect();
        if !keys.is_empty() {
            c.spans(x, y + 3, &[Span::styled("keys   ", DIM), Span::styled(kit::ellipsize(&keys.join(" · "), bw.saturating_sub(7)), kit::fg(GREEN))]);
        }
        for (i, (k, d)) in ROWS.iter().enumerate() {
            let yy = y + 5 + i;
            c.text(x, yy, &format!("{k:<kw$}"), kit::fg(ACCENT));
            c.text(x + kw + 3, yy, d, DIM);
        }
        c.spans(x, y + 6 + ROWS.len(), &[Span::styled("enter ", ACCENT_BOLD), Span::raw("launch Cosmos")]);
        self.draw_footer(c, "enter launch · esc back to keys · this setup removes itself once you launch");
    }

    // ---- frame -------------------------------------------------------------

    fn frame(&mut self, dt: f32) -> (Canvas, Option<(usize, usize)>) {
        let mut c = Canvas::new(self.w, self.h);
        let cursor = match self.phase {
            Phase::Warp => {
                self.draw_warp(&mut c, dt);
                None
            }
            Phase::Coalesce => {
                self.draw_coalesce(&mut c);
                None
            }
            Phase::Name => self.draw_name(&mut c),
            Phase::Keys | Phase::KeyEntry(_) => self.draw_keys(&mut c),
            Phase::Models => self.draw_models(&mut c),
            Phase::Ready => {
                self.draw_ready(&mut c);
                None
            }
        };
        (c, cursor)
    }

    // ---- input -------------------------------------------------------------

    fn submit_key(&mut self, i: usize) {
        let key: String = self.key_buf.iter().collect::<String>().trim().to_string();
        self.key_buf.clear();
        if key.is_empty() {
            self.phase = Phase::Keys;
            return;
        }
        let (pid, pname) = (self.provs[i].def.id.clone(), self.provs[i].name.clone());
        match zcore::creds::store_key(&pid, &key) {
            Ok(src) => {
                self.flash(format!("✓ {pname} key saved to the {} — checking it…", src.label()), true);
                self.unverified = Some(i);
                self.connected.push(pid.clone());
                self.refresh_keyed();
                self.open_models(i);
            }
            Err(e) => {
                self.flash(format!("✗ {pname}: {e:#}"), false);
                self.phase = Phase::Keys;
            }
        }
    }

    fn connect_devin(&mut self, i: usize) {
        zcore::devin::invalidate();
        if zcore::devin::ready() {
            self.connected.push("devin".into());
            self.refresh_keyed();
            self.flash(format!("✓ Devin · {}", zcore::devin::detail()), true);
            self.open_models(i);
        } else {
            self.flash(zcore::devin::hint(), false);
        }
    }

    fn start_oauth(&mut self) {
        let (tx, rx) = channel::<OauthMsg>();
        let cancel = Arc::new(AtomicBool::new(false));
        let (c2, tx2) = (cancel.clone(), tx.clone());
        std::thread::spawn(move || {
            let r = zcore::chatgpt::login(&c2, move |url| {
                let _ = tx2.send(OauthMsg::Url(url.to_string()));
                zcore::chatgpt::open_browser(url);
            });
            let _ = tx.send(OauthMsg::Done(r));
        });
        self.oauth = Some(Oauth { rx, cancel, url: None });
    }

    fn text_edit(buf: &mut Vec<char>, cur: &mut usize, k: KeyEvent, max: usize) {
        match k.code {
            KeyCode::Backspace => {
                if *cur > 0 {
                    *cur -= 1;
                    buf.remove(*cur);
                }
            }
            KeyCode::Delete => {
                if *cur < buf.len() {
                    buf.remove(*cur);
                }
            }
            KeyCode::Left => *cur = cur.saturating_sub(1),
            KeyCode::Right => *cur = (*cur + 1).min(buf.len()),
            KeyCode::Home => *cur = 0,
            KeyCode::End => *cur = buf.len(),
            KeyCode::Char(c) if !c.is_control() && buf.len() < max => {
                buf.insert(*cur, c);
                *cur += 1;
            }
            _ => {}
        }
    }

    fn finish(&mut self) {
        let name: String = self.name.iter().collect::<String>().trim().to_string();
        let model = self.model.as_ref().filter(|m| m.1).map(|m| m.0.clone());
        zcore::welcome::mark_done(&name, model.as_deref());
        self.done = true;
    }

    fn on_paste(&mut self, s: &str) {
        let clean: Vec<char> = s.chars().filter(|c| !c.is_control()).collect();
        match self.phase {
            Phase::Name => {
                for c in clean.into_iter().take(40usize.saturating_sub(self.name.len())) {
                    self.name.insert(self.nc, c);
                    self.nc += 1;
                }
            }
            Phase::KeyEntry(_) => self.key_buf.extend(clean.into_iter().take(400usize.saturating_sub(self.key_buf.len()))),
            Phase::Keys if self.oauth.is_none() => {
                self.filter.extend(clean.into_iter().take(60usize.saturating_sub(self.filter.len())));
                self.sel = 0;
            }
            Phase::Models => {
                if let Some(pk) = &mut self.pick {
                    pk.filter.extend(clean.into_iter().take(60usize.saturating_sub(pk.filter.len())));
                    pk.sel = 0;
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(k.code, KeyCode::Char('c') | KeyCode::Char('d')) {
            if let Some(o) = self.oauth.take() {
                o.cancel.store(true, Ordering::Relaxed);
            }
            HUSH.store(true, Ordering::Relaxed);
            self.quit = true;
            self.done = true;
            return;
        }
        match self.phase {
            Phase::Warp => {
                HUSH.store(true, Ordering::Relaxed);
                self.begin_coalesce();
            }
            Phase::Coalesce => self.begin_name(),
            Phase::Name => match k.code {
                KeyCode::Enter => self.phase = Phase::Keys,
                KeyCode::Esc => {
                    self.name.clear();
                    self.nc = 0;
                    self.phase = Phase::Keys;
                }
                _ => Self::text_edit(&mut self.name, &mut self.nc, k, 40),
            },
            Phase::Keys => {
                if self.oauth.is_some() {
                    if k.code == KeyCode::Esc {
                        if let Some(o) = self.oauth.take() {
                            o.cancel.store(true, Ordering::Relaxed);
                        }
                        self.flash("sign-in cancelled", false);
                    }
                    return;
                }
                let order = self.prov_order();
                let items = order.len() + 1;
                match k.code {
                    KeyCode::Esc => self.after_keys(),
                    KeyCode::Up | KeyCode::BackTab => self.sel = if self.sel == 0 { items - 1 } else { self.sel.min(items) - 1 },
                    KeyCode::Down | KeyCode::Tab => self.sel = if self.sel + 1 >= items { 0 } else { self.sel + 1 },
                    KeyCode::PageDown => self.sel = (self.sel + 10).min(items - 1),
                    KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
                    KeyCode::Home => self.sel = 0,
                    KeyCode::End => self.sel = items - 1,
                    KeyCode::Backspace => {
                        self.filter.pop();
                        self.sel = 0;
                    }
                    KeyCode::Enter => {
                        let sel = self.sel.min(items - 1);
                        if sel == order.len() {
                            self.after_keys();
                        } else {
                            let i = order[sel];
                            self.status = None;
                            if self.keyed.contains(&self.provs[i].def.id) {
                                // already connected — straight to its live models
                                self.open_models(i);
                            } else if self.provs[i].def.kind == "chatgpt" {
                                self.start_oauth();
                            } else if self.provs[i].def.kind == "devin" {
                                self.connect_devin(i);
                            } else {
                                self.key_buf.clear();
                                self.phase = Phase::KeyEntry(i);
                            }
                        }
                    }
                    KeyCode::Char(c) if !ctrl && !c.is_control() => {
                        self.filter.push(c);
                        self.sel = 0;
                    }
                    _ => {}
                }
            }
            Phase::KeyEntry(i) => match k.code {
                KeyCode::Esc => {
                    self.key_buf.clear();
                    self.phase = Phase::Keys;
                }
                KeyCode::Enter => self.submit_key(i),
                // masked input always edits at the end
                _ => {
                    let mut cur = self.key_buf.len();
                    Self::text_edit(&mut self.key_buf, &mut cur, k, 400);
                }
            },
            Phase::Models => {
                let n = self.model_order().len();
                let loading = self.pick.as_ref().is_none_or(|p| p.rx.is_some());
                match k.code {
                    KeyCode::Esc => {
                        // a key typed just now stays saved if you back out
                        self.unverified = None;
                        self.pick = None;
                        self.phase = Phase::Keys;
                    }
                    _ if loading => {}
                    KeyCode::Enter => self.choose_model(),
                    code => {
                        let Some(pk) = &mut self.pick else { return };
                        match code {
                            KeyCode::Up | KeyCode::BackTab => pk.sel = if pk.sel == 0 { n.saturating_sub(1) } else { pk.sel - 1 },
                            KeyCode::Down | KeyCode::Tab => pk.sel = if pk.sel + 1 >= n { 0 } else { pk.sel + 1 },
                            KeyCode::PageDown => pk.sel = (pk.sel + 10).min(n.saturating_sub(1)),
                            KeyCode::PageUp => pk.sel = pk.sel.saturating_sub(10),
                            KeyCode::Home => pk.sel = 0,
                            KeyCode::End => pk.sel = n.saturating_sub(1),
                            KeyCode::Backspace => {
                                pk.filter.pop();
                                pk.sel = 0;
                            }
                            KeyCode::Char(c) if !ctrl && !c.is_control() => {
                                pk.filter.push(c);
                                pk.sel = 0;
                            }
                            _ => {}
                        }
                    }
                }
            }
            Phase::Ready => match k.code {
                KeyCode::Enter => self.finish(),
                KeyCode::Esc => self.phase = Phase::Keys,
                _ => {}
            },
        }
    }
}

/// Run the whole flow. Ok(true) = completed, Ok(false) = Ctrl+C.
fn run(tty: &mut Tty) -> Result<bool> {
    let mut ob = Ob::sized(tty.w, tty.h);
    play_galaxy();
    let mut last = Instant::now();
    while !ob.done {
        let dt = last.elapsed().as_secs_f32().min(0.1);
        last = Instant::now();
        let (canvas, cursor) = ob.frame(dt);
        tty.draw(&canvas, cursor)?;
        if event::poll(Duration::from_millis(16))? {
            loop {
                match event::read()? {
                    CEvent::Key(k) if k.kind != KeyEventKind::Release => ob.on_key(k),
                    CEvent::Paste(s) => ob.on_paste(&s),
                    CEvent::Resize(w, h) if (w as usize, h as usize) != (tty.w, tty.h) => {
                        tty.resized(w, h);
                        ob.resized(w as usize, h as usize);
                    }
                    _ => {}
                }
                if ob.done || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
    }
    Ok(!ob.quit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
        L.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn typed(ob: &mut Ob, s: &str) {
        for c in s.chars() {
            ob.on_key(key(KeyCode::Char(c)));
        }
    }

    /// Temp COSMOS_CLI_HOME with the file credential store.
    fn home() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let g = env_lock();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("COSMOS_CLI_HOME", dir.path());
        std::env::set_var("COSMOS_CLI_KEYRING", "0");
        (g, dir)
    }

    fn fake_ok(def: &ProviderDef) -> Result<Vec<ModelDef>> {
        let m = |id: &str| ModelDef { id: id.into(), name: id.into(), context: 200_000, output: 8192, reasoning: false, price: [3.0, 15.0, 0.0, 0.0] };
        Ok(vec![m(&format!("{}-live-best", def.id)), m("second"), m("third")])
    }

    fn fake_rejected(def: &ProviderDef) -> Result<Vec<ModelDef>> {
        anyhow::bail!("{} rejected the key (401)", def.host())
    }

    fn fake_offline(_: &ProviderDef) -> Result<Vec<ModelDef>> {
        anyhow::bail!("couldn't reach the host")
    }

    /// Pump frames until the background fetch lands; returns the screen text.
    fn settle(ob: &mut Ob) -> String {
        for _ in 0..400 {
            ob.frame(0.016);
            if !ob.pick.as_ref().is_some_and(|p| p.rx.is_some()) {
                return ob.frame(0.016).0.plain();
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("fetch never finished");
    }

    fn to_keys(ob: &mut Ob) {
        ob.on_key(key(KeyCode::Char('x'))); // skip warp
        ob.on_key(key(KeyCode::Char('x'))); // skip coalesce
        assert_eq!(ob.phase, Phase::Name);
        ob.on_key(key(KeyCode::Enter));
        assert_eq!(ob.phase, Phase::Keys);
    }

    fn select(ob: &mut Ob, id: &str) -> usize {
        typed(ob, id);
        let i = ob.prov_order()[0];
        assert_eq!(ob.provs[i].def.id, id);
        ob.on_key(key(KeyCode::Enter));
        i
    }

    #[test]
    fn remember_model_sets_config_and_state() {
        let (_g, dir) = home();
        std::fs::write(dir.path().join("config.toml"), "[providers.r9]\nkind = \"openai\"\n").unwrap();
        std::fs::write(dir.path().join("state.json"), r#"{"model":"r9","other":1}"#).unwrap();
        remember_model("anthropic/claude-opus-5-5").unwrap();
        remember_model("openai/gpt-5.5").unwrap();
        let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        assert_eq!(text.matches("default_model").count(), 1);
        let cfg = zcore::config::Config::load(dir.path());
        assert_eq!(cfg.default_model.as_deref(), Some("openai/gpt-5.5"));
        assert!(cfg.providers.contains_key("r9"), "table kept intact");
        let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("state.json")).unwrap()).unwrap();
        assert_eq!(state["model"], "openai/gpt-5.5");
        assert_eq!(state["other"], 1, "other state kept");
    }

    #[test]
    fn synth_is_bounded_and_full_length() {
        let s = synth(24_000);
        assert_eq!(s.len(), (24_000.0 * SOUND_SECS) as usize);
        assert!(s.iter().all(|x| x.abs() <= 1.0));
        assert!(s.iter().any(|x| x.abs() > 0.05), "sound should be audible");
    }

    #[test]
    fn title_fits_every_size() {
        for (w, h) in [(200, 40), (160, 16), (120, 16), (80, 14), (60, 8), (40, 6)] {
            let (rows, cols, cells) = title_art(w, h);
            assert!(!cells.is_empty());
            assert!(rows <= h.max(2), "{w}x{h}: {rows} rows");
            if w >= 52 {
                assert!(cols + 2 <= w, "{w}x{h}: {cols} cols");
            }
            assert!(cells.iter().all(|&(c, r, ch)| c < cols && r < rows && ch != '\u{2800}'));
        }
        assert_eq!(title_art(200, 40).0, 6, "huge terminals get 3× letters");
        assert_eq!(title_art(160, 16).0, 6);
        assert_eq!(title_art(120, 16).0, 4);
        assert_eq!(title_art(80, 14).0, 9);
        assert_eq!(title_art(60, 8).0, 2);
    }

    #[test]
    fn canvas_clips_and_handles_wide_chars() {
        let mut c = Canvas::new(6, 2);
        assert_eq!(c.text(0, 0, "你好世界", Style::new()), 6);
        assert_eq!(c.get(1, 0), '\0');
        assert_eq!(c.text(4, 1, "abcdef", Style::new()), 6);
        assert_eq!(c.plain(), "你好世\n    ab");
        c.text(99, 99, "off-canvas", Style::new());
    }

    #[test]
    fn every_phase_fills_the_whole_screen() {
        let (_g, _dir) = home();
        let mut ob = Ob::sized(120, 40);
        ob.fetcher = fake_ok;
        // a few warp frames: the field spans the full height, not a strip
        for _ in 0..30 {
            ob.frame(0.05);
        }
        let (c, _) = ob.frame(0.05);
        assert_eq!((c.w, c.h), (120, 40));
        let rows: Vec<usize> = (0..40).filter(|&y| (0..120).any(|x| c.get(x, y) != ' ')).collect();
        assert!(rows.len() > 5, "{rows:?}");
        ob.begin_coalesce();
        ob.begin_name();
        let (c, cursor) = ob.frame(0.016);
        let text = c.plain();
        assert!(text.contains(QUESTION) && text.contains(SUBTITLE));
        assert!(text.chars().any(|c| ('\u{2801}'..='\u{28ff}').contains(&c)), "braille title");
        assert!(cursor.is_some());
        ob.on_key(key(KeyCode::Enter));
        let text = ob.frame(0.016).0.plain();
        assert!(text.contains("Bring your own keys") && text.contains("OpenRouter") && text.contains("ChatGPT"));
        assert!(text.contains("✓ name") && text.contains("start"), "step header");
        // 40 rows tall → the list shows far more than a handful of providers
        assert!(text.contains("Mistral"), "{text}");
    }

    #[test]
    fn a_resize_never_skips_the_intro() {
        let mut ob = Ob::sized(120, 30);
        ob.frame(0.016);
        ob.resized(120, 30); // what Windows sends when the alternate screen opens
        assert_eq!(ob.phase, Phase::Warp);
        ob.resized(100, 40);
        assert_eq!(ob.phase, Phase::Warp);
        assert_eq!(ob.frame(0.016).0.h, 40);
        ob.begin_coalesce();
        ob.resized(140, 36);
        assert_eq!(ob.phase, Phase::Coalesce, "re-aimed, not skipped");
        let (gx, gy) = (ob.gx, ob.gy);
        assert!(ob.stars.iter().filter(|s| !s.dissolve).all(|s| s.tx >= gx as f32 && s.ty >= gy as f32));
    }

    #[test]
    fn openrouter_is_near_the_top() {
        let p = provider_list();
        assert_eq!(p[0].def.id, "chatgpt");
        assert_eq!(p[0].name, "ChatGPT");
        assert_eq!(p[1].def.id, "devin");
        assert_eq!(p[2].def.id, "openrouter");
        assert_eq!(p[2].key_url, "https://openrouter.ai/keys");
        assert_eq!(p[3].def.id, "router9");
        assert_eq!(p[3].name, "9Router");
    }

    #[test]
    fn flow_name_skip_keys_finish() {
        let (_g, _dir) = home();
        let mut ob = Ob::sized(100, 30);
        ob.on_key(key(KeyCode::Char('x')));
        ob.on_key(key(KeyCode::Char('x')));
        typed(&mut ob, "Rav");
        ob.on_key(key(KeyCode::Enter));
        ob.on_key(key(KeyCode::Esc));
        assert_eq!(ob.phase, Phase::Ready);
        assert!(ob.frame(0.016).0.plain().contains("You're all set, Rav."));
        ob.on_key(key(KeyCode::Enter));
        assert!(ob.done && !ob.quit);
        assert!(!zcore::welcome::needed());
        assert_eq!(zcore::welcome::user_name().as_deref(), Some("Rav"));
    }

    #[test]
    fn a_key_fetches_live_models_and_the_pick_becomes_default() {
        let (_g, dir) = home();
        let mut ob = Ob::sized(100, 30);
        ob.fetcher = fake_ok;
        to_keys(&mut ob);
        let i = select(&mut ob, "anthropic");
        assert_eq!(ob.phase, Phase::KeyEntry(i));
        ob.on_paste("sk-ant-test-123");
        let text = ob.frame(0.016).0.plain();
        assert!(!text.contains("sk-ant") && text.contains("console.anthropic.com"), "key never echoed");
        ob.on_key(key(KeyCode::Enter));
        assert_eq!(ob.phase, Phase::Models);
        let text = settle(&mut ob);
        assert!(text.contains("anthropic-live-best") && text.contains("3 models live from api.anthropic.com"), "{text}");
        assert!(text.contains("$3 / $15") && text.contains("200k"));
        typed(&mut ob, "second");
        ob.on_key(key(KeyCode::Enter));
        assert_eq!(ob.phase, Phase::Keys);
        assert_eq!(ob.model, Some(("anthropic/second".into(), true)));
        assert!(ob.frame(0.016).0.plain().contains("✓ second ★"));
        let cfg = zcore::config::Config::load(dir.path());
        assert_eq!(cfg.default_model.as_deref(), Some("anthropic/second"));
        // the live list is cached, so cosmostui's /model shows it too
        assert_eq!(zcore::live::cached("anthropic").unwrap().len(), 3);
        assert!(cfg.available_models().iter().any(|m| m.alias == "anthropic/anthropic-live-best"));
        ob.on_key(key(KeyCode::Esc));
        assert_eq!(ob.phase, Phase::Ready);
        assert!(ob.frame(0.016).0.plain().contains("anthropic/second"));
        ob.on_key(key(KeyCode::Enter));
        assert!(ob.done);
    }

    #[test]
    fn a_connected_provider_opens_its_models_directly() {
        let (_g, _dir) = home();
        zcore::creds::store_key("openrouter", "sk-or-x").unwrap();
        let mut ob = Ob::sized(100, 30);
        ob.fetcher = fake_ok;
        to_keys(&mut ob);
        select(&mut ob, "openrouter");
        assert_eq!(ob.phase, Phase::Models);
        assert!(settle(&mut ob).contains("openrouter-live-best"));
    }

    #[test]
    fn a_rejected_key_is_dropped_and_asked_again() {
        let (_g, _dir) = home();
        let mut ob = Ob::sized(100, 30);
        ob.fetcher = fake_rejected;
        to_keys(&mut ob);
        let i = select(&mut ob, "openrouter");
        ob.on_paste("sk-or-bad");
        ob.on_key(key(KeyCode::Enter));
        for _ in 0..400 {
            ob.frame(0.016);
            if ob.phase != Phase::Models {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(ob.phase, Phase::KeyEntry(i));
        assert!(!ob.keyed.contains("openrouter"));
        assert!(zcore::creds::stored_key("openrouter").is_none());
        assert!(ob.frame(0.016).0.plain().contains("rejected the key"));
    }

    #[test]
    fn offline_falls_back_to_known_models() {
        let (_g, _dir) = home();
        let mut ob = Ob::sized(100, 30);
        ob.fetcher = fake_offline;
        to_keys(&mut ob);
        let i = select(&mut ob, "openai");
        ob.on_paste("sk-test");
        ob.on_key(key(KeyCode::Enter));
        let text = settle(&mut ob);
        assert!(text.contains("showing known models"), "{text}");
        assert!(ob.keyed.contains("openai"), "key kept when only the network failed");
        let first = ob.provs[i].def.models[0].id.clone();
        ob.on_key(key(KeyCode::Enter));
        assert_eq!(ob.model, Some((format!("openai/{first}"), true)));
    }

    #[test]
    fn continue_without_choosing_picks_a_real_model() {
        let (_g, _dir) = home();
        let mut ob = Ob::sized(100, 30);
        ob.fetcher = fake_ok;
        to_keys(&mut ob);
        // connect Anthropic, then back out of its model list without choosing
        select(&mut ob, "anthropic");
        ob.on_paste("sk-ant-x");
        ob.on_key(key(KeyCode::Enter));
        settle(&mut ob);
        ob.on_key(key(KeyCode::Esc));
        assert_eq!(ob.phase, Phase::Keys);
        ob.on_key(key(KeyCode::Esc));
        // the provider connected in this run wins over keys already in the environment
        let (m, fresh) = ob.model.clone().expect("default picked");
        assert_eq!((m.as_str(), fresh), ("anthropic/anthropic-live-best", true));
    }
}
