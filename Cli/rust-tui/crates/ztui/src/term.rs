//! Inline terminal renderer.
//!
//! No alternate screen: finished transcript lines are printed straight into
//! the terminal's own scrollback (native scroll, selection and copy), and a
//! small live region — streaming tail, composer, status, menus — is redrawn
//! in place underneath. The live region grows and shrinks with its content.
//!
//! Every frame is one buffered write wrapped in synchronized-output markers,
//! so it lands atomically (no flicker) and costs a single syscall.

use std::io::{stdout, Write};

use anyhow::Result;
use crossterm::cursor::{Hide, MoveToColumn, MoveUp, Show};
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::style::{
    Attribute, Color as CColor, Print, ResetColor, SetAttribute, SetBackgroundColor,
    SetForegroundColor,
};
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate,
};
use crossterm::{queue, QueueableCommand};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

pub struct Screen {
    buf: Vec<u8>,
    /// rows the live region occupied after the last frame
    rows: u16,
    /// row (within the live region) the terminal cursor was left on
    cursor_row: u16,
    pub width: u16,
    pub height: u16,
    enhanced_keys: bool,
}

/// What the live region wants drawn this frame.
pub struct Frame<'a> {
    pub lines: &'a [Line<'static>],
    /// (row, col) of the text cursor inside `lines`, if one should show
    pub cursor: Option<(u16, u16)>,
}

impl Screen {
    pub fn enter() -> Result<Screen> {
        terminal::enable_raw_mode()?;
        // a panic must never leave the terminal in raw mode
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::cursor::Show,
                crossterm::event::DisableBracketedPaste
            );
            eprintln!();
            prev(info);
        }));
        let mut out = stdout();
        queue!(out, EnableBracketedPaste)?;
        // Disambiguated keys (shift+enter etc.) where the terminal speaks the
        // kitty protocol; Windows consoles report modifiers natively anyway.
        let enhanced_keys = matches!(terminal::supports_keyboard_enhancement(), Ok(true));
        if enhanced_keys {
            queue!(
                out,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
        }
        out.flush()?;
        let (width, height) = terminal::size()?;
        Ok(Screen {
            buf: Vec::with_capacity(16 * 1024),
            rows: 0,
            cursor_row: 0,
            width,
            height,
            enhanced_keys,
        })
    }

    /// Usable text width — one column short of the edge so a full row never
    /// triggers the terminal's auto-wrap.
    pub fn text_width(&self) -> usize {
        (self.width as usize).saturating_sub(1).max(10)
    }

    pub fn resized(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
    }

    /// Return to the first row of the live region and wipe everything below.
    fn rewind(&mut self) {
        if self.cursor_row > 0 {
            let _ = self.buf.queue(MoveUp(self.cursor_row));
        }
        let _ = self.buf.queue(MoveToColumn(0));
        let _ = self.buf.queue(Clear(ClearType::FromCursorDown));
        self.rows = 0;
        self.cursor_row = 0;
    }

    /// Commit `history` to scrollback, then redraw the live region.
    pub fn draw(&mut self, history: &[Line<'static>], frame: Frame<'_>) -> Result<()> {
        self.compose(history, frame);
        let mut out = stdout().lock();
        out.write_all(&self.buf)?;
        out.flush()?;
        Ok(())
    }

    /// Build the bytes for one frame into `self.buf` (no I/O — testable).
    pub fn compose(&mut self, history: &[Line<'static>], frame: Frame<'_>) -> &[u8] {
        self.buf.clear();
        let _ = self.buf.queue(BeginSynchronizedUpdate);
        let _ = self.buf.queue(Hide);
        self.rewind();

        let w = self.text_width();
        for line in history {
            write_line(&mut self.buf, line, w);
            let _ = self.buf.queue(Print("\r\n"));
        }

        // never let the live region outgrow the screen — keep its bottom
        let max = (self.height as usize).saturating_sub(1).max(1);
        let skip = frame.lines.len().saturating_sub(max);
        let live = &frame.lines[skip..];
        for (i, line) in live.iter().enumerate() {
            write_line(&mut self.buf, line, w);
            if i + 1 < live.len() {
                let _ = self.buf.queue(Print("\r\n"));
            }
        }
        let n = live.len() as u16;
        self.rows = n;
        self.cursor_row = n.saturating_sub(1);

        if let Some((r, c)) = frame.cursor {
            let r = (r as usize).checked_sub(skip).map(|r| r as u16);
            if let Some(r) = r.filter(|r| *r < n) {
                let up = self.cursor_row - r;
                if up > 0 {
                    let _ = self.buf.queue(MoveUp(up));
                }
                let _ = self.buf.queue(MoveToColumn(c.min(self.width.saturating_sub(1))));
                self.cursor_row = r;
                let _ = self.buf.queue(Show);
            }
        }
        let _ = self.buf.queue(EndSynchronizedUpdate);
        &self.buf
    }

    /// A screen that renders into memory only (tests).
    #[cfg(test)]
    pub fn headless(width: u16, height: u16) -> Screen {
        Screen { buf: Vec::new(), rows: 0, cursor_row: 0, width, height, enhanced_keys: false }
    }

    /// Ctrl+L: clear the visible screen (scrollback above is kept).
    pub fn clear_screen(&mut self) -> Result<()> {
        let mut out = stdout();
        queue!(out, Clear(ClearType::All), crossterm::cursor::MoveTo(0, 0))?;
        out.flush()?;
        self.rows = 0;
        self.cursor_row = 0;
        Ok(())
    }

    /// Wipe the live region and hand the terminal back.
    pub fn leave(&mut self, farewell: &[Line<'static>]) {
        self.buf.clear();
        self.rewind();
        let w = self.text_width();
        for line in farewell {
            write_line(&mut self.buf, line, w);
            let _ = self.buf.queue(Print("\r\n"));
        }
        let _ = self.buf.queue(Show);
        if self.enhanced_keys {
            let _ = self.buf.queue(PopKeyboardEnhancementFlags);
        }
        let _ = self.buf.queue(DisableBracketedPaste);
        let mut out = stdout();
        let _ = out.write_all(&self.buf);
        let _ = out.flush();
        let _ = terminal::disable_raw_mode();
    }
}

fn to_ct(c: Color) -> CColor {
    match c {
        Color::Reset => CColor::Reset,
        Color::Black => CColor::Black,
        Color::Red => CColor::DarkRed,
        Color::Green => CColor::DarkGreen,
        Color::Yellow => CColor::DarkYellow,
        Color::Blue => CColor::DarkBlue,
        Color::Magenta => CColor::DarkMagenta,
        Color::Cyan => CColor::DarkCyan,
        Color::Gray => CColor::Grey,
        Color::DarkGray => CColor::DarkGrey,
        Color::LightRed => CColor::Red,
        Color::LightGreen => CColor::Green,
        Color::LightYellow => CColor::Yellow,
        Color::LightBlue => CColor::Blue,
        Color::LightMagenta => CColor::Magenta,
        Color::LightCyan => CColor::Cyan,
        Color::White => CColor::White,
        Color::Rgb(r, g, b) => CColor::Rgb { r, g, b },
        Color::Indexed(i) => CColor::AnsiValue(i),
    }
}

fn apply(buf: &mut Vec<u8>, s: Style) {
    let _ = buf.queue(SetAttribute(Attribute::Reset));
    if let Some(fg) = s.fg {
        let _ = buf.queue(SetForegroundColor(to_ct(fg)));
    }
    if let Some(bg) = s.bg {
        let _ = buf.queue(SetBackgroundColor(to_ct(bg)));
    }
    let m = s.add_modifier;
    for (flag, attr) in [
        (Modifier::BOLD, Attribute::Bold),
        (Modifier::DIM, Attribute::Dim),
        (Modifier::ITALIC, Attribute::Italic),
        (Modifier::UNDERLINED, Attribute::Underlined),
        (Modifier::REVERSED, Attribute::Reverse),
        (Modifier::CROSSED_OUT, Attribute::CrossedOut),
    ] {
        if m.contains(flag) {
            let _ = buf.queue(SetAttribute(attr));
        }
    }
}

/// Write one styled line, hard-clipped to `max` display columns.
pub fn write_line(buf: &mut Vec<u8>, line: &Line<'_>, max: usize) {
    let mut col = 0usize;
    'spans: for span in &line.spans {
        let style = line.style.patch(span.style);
        apply(buf, style);
        let mut chunk = String::new();
        for ch in span.content.chars() {
            if ch == '\n' || ch == '\r' {
                continue;
            }
            let cw = if ch == '\t' { 1 } else { UnicodeWidthChar::width(ch).unwrap_or(0) };
            if col + cw > max {
                let _ = buf.queue(Print(&chunk));
                break 'spans;
            }
            chunk.push(if ch == '\t' { ' ' } else { ch });
            col += cw;
        }
        let _ = buf.queue(Print(&chunk));
    }
    let _ = buf.queue(SetAttribute(Attribute::Reset));
    let _ = buf.queue(ResetColor);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    #[test]
    fn clips_to_width() {
        let mut b = Vec::new();
        write_line(&mut b, &Line::from(Span::raw("abcdefgh")), 4);
        let s = String::from_utf8_lossy(&b);
        assert!(s.contains("abcd") && !s.contains("abcde"));
    }

    #[test]
    fn wide_chars_respect_columns() {
        let mut b = Vec::new();
        write_line(&mut b, &Line::from("你好世界"), 5);
        let s = String::from_utf8_lossy(&b);
        assert!(s.contains("你好") && !s.contains("世"));
    }
}

/// Feed real frames through a real terminal emulator (alacritty's) and check
/// what ends up on screen and in scrollback — the part unit tests of the
/// line builders can't see.
#[cfg(test)]
mod emulator {
    use super::*;
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line as GLine};
    use alacritty_terminal::term::{Config, Term};
    use alacritty_terminal::vte::ansi::Processor;

    struct Size(usize, usize);
    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            self.1
        }
        fn screen_lines(&self) -> usize {
            self.1
        }
        fn columns(&self) -> usize {
            self.0
        }
    }

    struct Emu {
        term: Term<VoidListener>,
        vt: Processor,
    }

    impl Emu {
        fn new(w: usize, h: usize) -> Emu {
            Emu { term: Term::new(Config::default(), &Size(w, h), VoidListener), vt: Processor::new() }
        }
        fn feed(&mut self, b: &[u8]) {
            self.vt.advance(&mut self.term, b);
        }
        fn row(&self, i: i32) -> String {
            let g = self.term.grid();
            let mut s = String::new();
            for c in 0..g.columns() {
                s.push(g[GLine(i)][Column(c)].c);
            }
            s.trim_end().to_string()
        }
        /// scrollback (oldest first) followed by the visible screen
        fn all(&self) -> Vec<String> {
            let g = self.term.grid();
            let hist = g.history_size() as i32;
            (-hist..g.screen_lines() as i32).map(|i| self.row(i)).collect()
        }
        fn screen(&self) -> Vec<String> {
            (0..self.term.grid().screen_lines() as i32).map(|i| self.row(i)).collect()
        }
    }

    fn lines(v: &[&str]) -> Vec<Line<'static>> {
        v.iter().map(|s| Line::from(s.to_string())).collect()
    }

    #[test]
    fn history_lands_once_and_live_region_redraws_in_place() {
        let (w, h) = (40, 10);
        let mut emu = Emu::new(w, h);
        let mut scr = Screen::headless(w as u16, h as u16);

        let live_a = lines(&["-- rule", "> hello", "-- rule", "status"]);
        emu.feed(scr.compose(&lines(&["banner 1", "banner 2"]), Frame { lines: &live_a, cursor: Some((1, 7)) }));
        assert_eq!(&emu.screen()[..6], &["banner 1", "banner 2", "-- rule", "> hello", "-- rule", "status"]);
        assert_eq!(emu.term.grid().cursor.point.line.0, 3, "cursor on the input row");

        // grow the live region and commit more history — nothing duplicated
        let live_b = lines(&["* working", "", "-- rule", "> hello world", "-- rule", "status", "menu 1", "menu 2"]);
        let hist: Vec<String> = (1..=12).map(|i| format!("hist {i}")).collect();
        let hist_refs: Vec<&str> = hist.iter().map(String::as_str).collect();
        emu.feed(scr.compose(&lines(&hist_refs), Frame { lines: &live_b, cursor: Some((3, 13)) }));
        let all = emu.all();
        for i in 1..=12 {
            let n = all.iter().filter(|r| **r == format!("hist {i}")).count();
            assert_eq!(n, 1, "hist {i} appears {n} times: {all:?}");
        }
        assert_eq!(all.iter().filter(|r| r.starts_with("> hello")).count(), 1, "old composer left behind: {all:?}");
        let screen = emu.screen();
        assert_eq!(screen.last().unwrap(), "menu 2");

        // shrink: the menu rows must be wiped, not left as residue
        let live_c = lines(&["-- rule", "> ", "-- rule", "status"]);
        emu.feed(scr.compose(&[], Frame { lines: &live_c, cursor: Some((1, 2)) }));
        let screen = emu.screen();
        assert!(!screen.iter().any(|r| r.starts_with("menu")), "stale menu rows: {screen:?}");
        assert!(!screen.iter().any(|r| r.starts_with("* working")), "stale working row: {screen:?}");
        assert_eq!(emu.all().iter().filter(|r| *r == "hist 12").count(), 1);
    }

    #[test]
    fn oversized_live_region_keeps_its_bottom() {
        let (w, h) = (30, 6);
        let mut emu = Emu::new(w, h);
        let mut scr = Screen::headless(w as u16, h as u16);
        let tall: Vec<String> = (0..20).map(|i| format!("row {i}")).collect();
        let refs: Vec<&str> = tall.iter().map(String::as_str).collect();
        emu.feed(scr.compose(&[], Frame { lines: &lines(&refs), cursor: None }));
        let screen = emu.screen();
        let last = screen.iter().rev().find(|r| !r.is_empty()).unwrap();
        assert_eq!(last, "row 19", "{screen:?}");
        // and the next frame still lines up
        emu.feed(scr.compose(&lines(&["done"]), Frame { lines: &lines(&["> "]), cursor: Some((0, 2)) }));
        assert!(emu.all().iter().any(|r| r == "done"));
        assert_eq!(emu.screen().iter().filter(|r| r.starts_with("row")).count(), 0);
    }
}
