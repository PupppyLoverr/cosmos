//! Streaming markdown → styled terminal lines.
//!
//! Line-oriented on purpose: a completed source line always renders the same
//! way whatever arrives after it, so the TUI can commit lines to scrollback
//! while the reply is still streaming (the `mdstream` idea from Devin's
//! stack). Inline spans go through pulldown-cmark; tables are held until the
//! block ends so columns can be aligned; fenced code is highlighted line by
//! line against a short window of the lines before it.
//!
//! The `cosmos` style (`[markdown]` in ui.toml) keeps a little block state
//! — the open callout, the list item being continued, whether the last line
//! was paragraph text — so code gets a frame, callouts get an icon and
//! colour, tables get borders and wrapped cells, and links become real
//! hyperlinks (OSC 8) where the terminal supports them. `classic` renders
//! exactly what it always did.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui_core::style::{Color, Modifier, Style};
use ratatui_core::text::{Line, Span};

use crate::highlight;
use crate::skin::Skin;
use crate::theme::{self, CODE_INLINE, DIM, RULE};
use crate::wrap::{self, pad_line, width_of};

/// Left margin for assistant text (Devin indents replies one column).
const M: &str = " ";

/// Lines of context the highlighter sees before the line it colours.
const HL_WINDOW: usize = 40;

#[derive(Clone)]
struct Fence {
    lang: String,
    tick: String,
    code: String,
    /// columns the fence was indented (code inside a list item)
    indent: usize,
    /// code lines so far
    n: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Callout {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl Callout {
    fn parse(s: &str) -> Option<Callout> {
        let tag = s.trim().strip_prefix("[!")?.strip_suffix(']')?;
        Some(match tag.to_ascii_uppercase().as_str() {
            "NOTE" | "INFO" => Callout::Note,
            "TIP" | "HINT" => Callout::Tip,
            "IMPORTANT" => Callout::Important,
            "WARNING" => Callout::Warning,
            "CAUTION" | "DANGER" => Callout::Caution,
            _ => return None,
        })
    }

    fn label(self) -> (&'static str, &'static str) {
        match self {
            Callout::Note => ("ⓘ", "Note"),
            Callout::Tip => ("✦", "Tip"),
            Callout::Important => ("❖", "Important"),
            Callout::Warning => ("▲", "Warning"),
            Callout::Caution => ("✖", "Caution"),
        }
    }

    fn color(self) -> Color {
        let p = crate::skin::palette();
        match self {
            Callout::Note => p.link,
            Callout::Tip => p.success,
            Callout::Important => p.accent,
            Callout::Warning => p.warn,
            Callout::Caution => p.error,
        }
    }
}

#[derive(Default, Clone)]
pub struct Md {
    fence: Option<Fence>,
    table: Vec<String>,
    /// the callout the current quote opened with
    callout: Option<Callout>,
    /// hanging indent of the list item that indented lines continue
    item: Option<String>,
    /// the previous line was paragraph text (a `===` under it underlines it)
    para: bool,
}

fn sp(s: &str) -> Span<'static> {
    Span::raw(s.to_string())
}

fn cosmos(sk: &Skin) -> bool {
    sk.markdown.style != "classic"
}

// ---- hyperlinks ---------------------------------------------------------------
//
// A link travels through wrapping as zero-width Unicode tag characters
// (U+E0001, the URL as tags, U+E007F) and `term::write_line` turns it into
// an OSC 8 escape. Tags are default-ignorable, so widths stay right.

const LINK_START: char = '\u{E0001}';
const LINK_END: char = '\u{E007F}';

/// The invisible marker that opens a hyperlink to `url` (empty closes one).
pub fn link_mark(url: &str) -> String {
    let mut s = String::from(LINK_START);
    for b in url.bytes() {
        if (0x21..0x7f).contains(&b) {
            s.push(char::from_u32(0xE0000 + b as u32).unwrap_or(LINK_END));
        } else {
            for c in format!("%{b:02X}").chars() {
                s.push(char::from_u32(0xE0000 + c as u32).unwrap_or(LINK_END));
            }
        }
    }
    s.push(LINK_END);
    s
}

/// If `chars` starts a link marker, consume it and return the URL.
pub fn take_link(c: char, rest: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    if c != LINK_START {
        return None;
    }
    let mut url = String::new();
    while let Some(&n) = rest.peek() {
        rest.next();
        if n == LINK_END {
            break;
        }
        if let Some(ch) = char::from_u32((n as u32).wrapping_sub(0xE0000)) {
            url.push(ch);
        }
    }
    Some(url)
}

/// Whether links render as OSC 8 hyperlinks.
pub fn hyperlinks(sk: &Skin) -> bool {
    match sk.markdown.links.as_str() {
        "osc8" => true,
        "inline" => false,
        _ => {
            static AUTO: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *AUTO.get_or_init(|| {
                let env = |k: &str| std::env::var(k).unwrap_or_default();
                let prog = env("TERM_PROGRAM");
                std::env::var_os("WT_SESSION").is_some()
                    || std::env::var_os("KITTY_WINDOW_ID").is_some()
                    || std::env::var_os("WEZTERM_PANE").is_some()
                    || matches!(prog.as_str(), "iTerm.app" | "WezTerm" | "vscode" | "ghostty" | "Hyper" | "Tabby" | "rio")
                    || env("VTE_VERSION").parse::<u32>().is_ok_and(|v| v >= 5000)
                    || env("TERM").contains("kitty")
                    || env("TERM").contains("foot")
            })
        }
    }
}

/// Bare `https://…` in plain text becomes a link.
fn autolink(text: &str, style: Style, osc: bool, out: &mut Vec<Span<'static>>) {
    let link = style.fg(crate::skin::palette().link).add_modifier(Modifier::UNDERLINED);
    let mut rest = text;
    while let Some(at) = rest.find("https://").or_else(|| rest.find("http://")) {
        let tail = &rest[at..];
        let mut end = tail.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`')).unwrap_or(tail.len());
        while end > 0 {
            let last = tail[..end].chars().last().unwrap_or(' ');
            let unbalanced = last == ')' && !tail[..end].contains('(');
            if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '\'') || unbalanced {
                end -= last.len_utf8();
            } else {
                break;
            }
        }
        let url = &tail[..end];
        if url.len() <= "https://".len() {
            out.push(Span::styled(rest[..at + end.max(1)].to_string(), style));
            rest = &rest[at + end.max(1)..];
            continue;
        }
        if at > 0 {
            out.push(Span::styled(rest[..at].to_string(), style));
        }
        if osc {
            out.push(Span::raw(link_mark(url)));
        }
        out.push(Span::styled(url.to_string(), link));
        if osc {
            out.push(Span::raw(link_mark("")));
        }
        rest = &tail[end..];
    }
    if !rest.is_empty() {
        out.push(Span::styled(rest.to_string(), style));
    }
}

/// Inline markdown (bold, italic, `code`, ~~strike~~, links) → spans.
pub fn inline(text: &str, base: Style) -> Vec<Span<'static>> {
    let sk = crate::skin::current();
    let cm = cosmos(&sk);
    let osc = cm && hyperlinks(&sk);
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut stack: Vec<Style> = vec![base];
    let top = |s: &Vec<Style>| *s.last().unwrap();
    let mut link: Option<String> = None;
    // the parser splits text at `_`, `(` and friends; cosmos joins the runs
    // back up so a bare URL is seen whole
    let mut pend: Option<(String, Style)> = None;
    let flush = |pend: &mut Option<(String, Style)>, out: &mut Vec<Span<'static>>| {
        if let Some((t, st)) = pend.take() {
            autolink(&t, st, osc, out);
        }
    };
    let parser = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH);
    for ev in parser {
        if cm && link.is_none() {
            if let Event::Text(t) = &ev {
                let st = top(&stack);
                match &mut pend {
                    Some((p, s)) if *s == st => p.push_str(t),
                    _ => {
                        flush(&mut pend, &mut out);
                        pend = Some((t.to_string(), st));
                    }
                }
                continue;
            }
        }
        flush(&mut pend, &mut out);
        match ev {
            Event::Start(Tag::Strong) => stack.push(top(&stack).add_modifier(Modifier::BOLD)),
            Event::Start(Tag::Emphasis) => stack.push(top(&stack).add_modifier(Modifier::ITALIC)),
            Event::Start(Tag::Strikethrough) => stack.push(top(&stack).add_modifier(Modifier::CROSSED_OUT)),
            Event::Start(Tag::Link { dest_url, .. }) => {
                link = Some(dest_url.to_string());
                if osc {
                    out.push(Span::raw(link_mark(&dest_url)));
                }
                stack.push(top(&stack).fg(crate::skin::palette().link).add_modifier(Modifier::UNDERLINED));
            }
            Event::End(TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough) => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            Event::End(TagEnd::Link) => {
                if stack.len() > 1 {
                    stack.pop();
                }
                if let Some(url) = link.take() {
                    if osc {
                        out.push(Span::raw(link_mark("")));
                        continue;
                    }
                    // show the target when the label doesn't already say it
                    let shown: String = out.iter().rev().take(1).map(|s| s.content.to_string()).collect();
                    if !url.is_empty() && shown != url {
                        out.push(Span::styled(format!(" ({url})"), DIM()));
                    }
                }
            }
            Event::Text(t) => out.push(Span::styled(t.to_string(), top(&stack))),
            Event::Code(t) => out.push(Span::styled(t.to_string(), CODE_INLINE())),
            Event::Html(t) | Event::InlineHtml(t) => out.push(Span::styled(t.to_string(), top(&stack))),
            Event::SoftBreak | Event::HardBreak => out.push(Span::styled(" ", top(&stack))),
            Event::TaskListMarker(done) => out.push(Span::raw(if done { "☑ " } else { "☐ " })),
            _ => {}
        }
    }
    flush(&mut pend, &mut out);
    if out.is_empty() && !text.is_empty() {
        out.push(Span::styled(text.to_string(), base));
    }
    out
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        Some((hashes, line[hashes..].trim()))
    } else {
        None
    }
}

fn is_rule(t: &str) -> bool {
    let c: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    c.len() >= 3 && (c.chars().all(|x| x == '-') || c.chars().all(|x| x == '*') || c.chars().all(|x| x == '_'))
}

/// `(indent, marker, rest)` for a list item line.
fn list_item(line: &str) -> Option<(usize, String, &str)> {
    let indent = line.len() - line.trim_start().len();
    let t = line.trim_start();
    for b in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(b) {
            if let Some(r) = rest.strip_prefix("[ ] ") {
                return Some((indent, "☐".into(), r));
            }
            if let Some(r) = rest.strip_prefix("[x] ").or_else(|| rest.strip_prefix("[X] ")) {
                return Some((indent, "☑".into(), r));
            }
            return Some((indent, "•".into(), rest));
        }
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=3).contains(&digits) {
        let after = &t[digits..];
        if let Some(rest) = after.strip_prefix(". ").or_else(|| after.strip_prefix(") ")) {
            return Some((indent, format!("{}.", &t[..digits]), rest));
        }
    }
    None
}

fn is_table_row(t: &str) -> bool {
    let t = t.trim();
    t.starts_with('|') && t[1..].contains('|')
}

fn cells(row: &str) -> Vec<String> {
    let t = row.trim().trim_start_matches('|').trim_end_matches('|');
    t.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_sep_row(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.trim_matches(':');
            !c.is_empty() && c.chars().all(|x| x == '-')
        })
}

/// The last `HL_WINDOW` lines of `code` — all the highlighter ever sees.
fn window(code: &str) -> &str {
    let mut seen = 0;
    for (i, b) in code.bytes().enumerate().rev() {
        if b == b'\n' {
            seen += 1;
            if seen == HL_WINDOW {
                return &code[i + 1..];
            }
        }
    }
    code
}

/// The last line of `code`, coloured with a short window of context (the
/// whole fence each line made long blocks quadratic).
fn last_line(lang: &str, code: &str) -> Vec<Span<'static>> {
    let line = code.rsplit('\n').next().unwrap_or("");
    highlight::highlight(lang, window(code))
        .and_then(|mut l| l.pop())
        .unwrap_or_else(|| vec![Span::styled(line.to_string(), CODE_INLINE())])
}

impl Md {
    pub fn new() -> Md {
        Md::default()
    }

    /// Feed one complete source line; returns rendered rows ready to commit.
    pub fn push(&mut self, raw: &str, w: usize) -> Vec<Line<'static>> {
        let sk = crate::skin::current();
        let cm = cosmos(&sk);
        let line = raw.trim_end_matches('\r');
        let t = line.trim_start();

        if let Some(f) = &mut self.fence {
            if t.starts_with(f.tick.as_str()) && t[f.tick.len()..].trim().is_empty() {
                let f = self.fence.take().expect("open fence");
                return if cm && sk.markdown.code_frame { vec![frame_edge(false, "", f.indent, w)] } else { Vec::new() };
            }
            if !f.code.is_empty() {
                f.code.push('\n');
            }
            f.code.push_str(line);
            f.n += 1;
            let spans = last_line(&f.lang, &f.code);
            return if cm && sk.markdown.code_frame { framed_rows(&spans, f.n, f.indent, w, &sk) } else { code_rows(&spans, w) };
        }

        let mut out = Vec::new();
        if !self.table.is_empty() {
            if is_table_row(t) {
                self.table.push(line.to_string());
                return out;
            }
            out.extend(self.flush_table(w));
        }

        for tick in ["```", "~~~"] {
            if let Some(rest) = t.strip_prefix(tick) {
                let indent = (line.len() - t.len()).min(8);
                let lang = rest.trim().trim_start_matches('`').to_string();
                if cm && sk.markdown.code_frame {
                    out.push(frame_edge(true, &lang, indent, w));
                }
                self.fence = Some(Fence { lang, tick: tick.to_string(), code: String::new(), indent, n: 0 });
                self.callout = None;
                self.para = false;
                return out;
            }
        }
        if is_table_row(t) {
            self.table.push(line.to_string());
            self.callout = None;
            self.para = false;
            return out;
        }
        if cm {
            out.extend(self.line(line, w, &sk));
        } else {
            out.extend(render_line(line, w));
        }
        out
    }

    /// End of message: flush anything held back (open table, open fence).
    pub fn finish(&mut self, w: usize) -> Vec<Line<'static>> {
        let sk = crate::skin::current();
        let mut out = Vec::new();
        if let Some(f) = self.fence.take() {
            // an unclosed fence still gets its bottom edge
            if cosmos(&sk) && sk.markdown.code_frame {
                out.push(frame_edge(false, "", f.indent, w));
            }
        }
        out.extend(self.flush_table(w));
        out
    }

    /// Render an incomplete trailing line for the live region (no state change).
    pub fn preview(&self, partial: &str, w: usize) -> Vec<Line<'static>> {
        if partial.is_empty() {
            return Vec::new();
        }
        let sk = crate::skin::current();
        let cm = cosmos(&sk);
        if let Some(f) = &self.fence {
            let spans = [Span::styled(partial.to_string(), CODE_INLINE())];
            return if cm && sk.markdown.code_frame { framed_rows(&spans, f.n + 1, f.indent, w, &sk) } else { code_rows(&spans, w) };
        }
        if !self.table.is_empty() || is_table_row(partial) {
            let mut rows = Vec::new();
            for l in self.table.iter().map(String::as_str).chain(std::iter::once(partial)) {
                rows.extend(wrap::wrap(&[Span::styled(l.to_string(), DIM())], w, &[sp(M)], &[sp(M)]));
            }
            return rows;
        }
        if cm {
            return self.clone().line(partial, w, &sk);
        }
        render_line(partial, w)
    }

    fn flush_table(&mut self, w: usize) -> Vec<Line<'static>> {
        if self.table.is_empty() {
            return Vec::new();
        }
        let rows: Vec<Vec<String>> = std::mem::take(&mut self.table).iter().map(|r| cells(r)).collect();
        let sk = crate::skin::current();
        if cosmos(&sk) && sk.markdown.table_borders {
            if let Some(out) = boxed_table(&rows, w) {
                return out;
            }
        }
        render_table(rows, w)
    }

    /// One non-fence, non-table line in the cosmos style.
    fn line(&mut self, line: &str, w: usize, sk: &Skin) -> Vec<Line<'static>> {
        let t = line.trim_start();
        let indent = line.len() - t.len();
        let p = crate::skin::palette();
        if t.starts_with('>') {
            self.para = false;
            return self.quote(t, w, sk);
        }
        self.callout = None;
        if t.is_empty() {
            self.para = false;
            return vec![Line::default()];
        }
        if let Some((level, text)) = heading(t) {
            self.item = None;
            self.para = false;
            let style = match level {
                1 | 2 => theme::fg_or_default(p.heading).add_modifier(Modifier::BOLD),
                3 => Style::new().add_modifier(Modifier::BOLD).fg(p.heading_sub),
                _ => Style::new().add_modifier(Modifier::ITALIC).fg(p.heading_sub),
            };
            let spans = inline(text, style);
            let mut out = wrap::wrap(&spans, w, &[sp(M)], &[sp(M)]);
            if level == 1 && sk.markdown.h1_rule {
                let n = spans.iter().map(|s| width_of(&s.content)).sum::<usize>().clamp(3, w.saturating_sub(2).max(3));
                out.push(Line::from(vec![sp(M), Span::styled("━".repeat(n), theme::fg_or_default(p.heading))]));
            }
            return out;
        }
        // setext: `===` under paragraph text underlines it
        if self.para && t.len() >= 3 && t.chars().all(|c| c == '=') {
            self.para = false;
            let n = w.saturating_sub(2).min(56);
            return vec![Line::from(vec![sp(M), Span::styled("━".repeat(n), theme::fg_or_default(p.heading))])];
        }
        if is_rule(t) {
            self.para = false;
            self.item = None;
            let n = w.saturating_sub(2).min(56);
            return vec![Line::from(vec![sp(M), Span::styled("─".repeat(n), RULE())])];
        }
        if let Some((ind, marker, rest)) = list_item(line) {
            self.para = false;
            let level = (ind / 2).min(4);
            let shown = match marker.as_str() {
                "•" if !sk.markdown.bullets.is_empty() => sk.markdown.bullets[level % sk.markdown.bullets.len()].clone(),
                _ => marker,
            };
            let pad = format!("{M}  {}", "  ".repeat(level));
            let hang = format!("{pad}{}", " ".repeat(width_of(&shown) + 1));
            let first = [Span::raw(pad), Span::styled(format!("{shown} "), theme::fg(theme::ACCENT()))];
            self.item = Some(hang.clone());
            return wrap::wrap(&inline(rest, Style::new()), w, &first, &[Span::raw(hang)]);
        }
        // an indented line under a list item continues it
        if let Some(hang) = &self.item {
            if indent >= 2 {
                let pre = [Span::raw(hang.clone())];
                return wrap::wrap(&inline(t, Style::new()), w, &pre, &pre);
            }
            self.item = None;
        }
        self.para = true;
        wrap::wrap(&inline(t, Style::new()), w, &[sp(M)], &[sp(M)])
    }

    /// `>` lines: nested bars, and `> [!NOTE]`-style callouts.
    fn quote(&mut self, t: &str, w: usize, sk: &Skin) -> Vec<Line<'static>> {
        let mut rest = t;
        let mut depth = 0;
        while let Some(r) = rest.trim_start().strip_prefix('>') {
            depth += 1;
            rest = r.strip_prefix(' ').unwrap_or(r);
        }
        let p = crate::skin::palette();
        if depth == 1 && sk.markdown.callouts {
            if let Some(c) = Callout::parse(rest) {
                self.callout = Some(c);
                let (icon, label) = c.label();
                let col = Style::new().fg(c.color());
                return vec![Line::from(vec![
                    sp(M),
                    Span::styled("▌ ", col),
                    Span::styled(format!("{icon} {label}"), col.add_modifier(Modifier::BOLD)),
                ])];
            }
        }
        let mut bar = vec![sp(M)];
        for d in 0..depth {
            match self.callout {
                Some(c) if d == 0 => bar.push(Span::styled("▌ ", Style::new().fg(c.color()))),
                _ => bar.push(Span::styled("│ ", DIM())),
            }
        }
        let style = if self.callout.is_some() { Style::new() } else { Style::new().fg(p.quote).add_modifier(Modifier::ITALIC) };
        if rest.trim().is_empty() {
            return vec![Line::from(bar)];
        }
        wrap::wrap(&inline(rest, style), w, &bar, &bar)
    }
}

fn code_rows(spans: &[Span<'static>], w: usize) -> Vec<Line<'static>> {
    let pre = [sp(M), Span::styled("  ", Style::new())];
    let mut v = wrap::wrap(spans, w, &pre, &pre);
    if v.is_empty() {
        v.push(Line::from(pre.to_vec()));
    }
    v
}

/// Inner width of a code frame.
fn frame_width(indent: usize, w: usize) -> usize {
    w.saturating_sub(M.len() + indent + 1).clamp(10, 100)
}

/// `╭─ rust ──────` (top) or `╰──────────` (bottom).
fn frame_edge(top: bool, lang: &str, indent: usize, w: usize) -> Line<'static> {
    let fw = frame_width(indent, w);
    let pad = format!("{M}{}", " ".repeat(indent));
    if !top {
        return Line::from(vec![Span::raw(pad), Span::styled(format!("╰{}", "─".repeat(fw.saturating_sub(1))), RULE())]);
    }
    let label = if lang.is_empty() { String::new() } else { format!(" {lang} ") };
    let fill = fw.saturating_sub(2 + width_of(&label));
    Line::from(vec![
        Span::raw(pad),
        Span::styled("╭─", RULE()),
        Span::styled(label, DIM().add_modifier(Modifier::BOLD)),
        Span::styled("─".repeat(fill), RULE()),
    ])
}

/// One code line inside a frame: `│ code`, tinted, optionally numbered.
fn framed_rows(spans: &[Span<'static>], n: usize, indent: usize, w: usize, sk: &Skin) -> Vec<Line<'static>> {
    let fw = frame_width(indent, w);
    let p = crate::skin::palette();
    let tint = sk.markdown.code_bg && p.code_bg != Color::Reset;
    let bg = if tint { Style::new().bg(p.code_bg) } else { Style::new() };
    let pad = format!("{M}{}", " ".repeat(indent));
    let mut first = vec![Span::raw(pad.clone()), Span::styled("│", RULE()), Span::styled(" ", bg)];
    let mut rest = first.clone();
    if sk.markdown.code_numbers {
        first.push(Span::styled(format!("{n:>3} "), DIM().patch(bg)));
        rest.push(Span::styled("    ", bg));
    }
    let body: Vec<Span<'static>> = spans.iter().map(|s| Span::styled(s.content.clone(), s.style.patch(bg))).collect();
    let mut v = wrap::wrap(&body, M.len() + indent + fw, &first, &rest);
    if v.is_empty() {
        v.push(Line::from(first));
    }
    if tint {
        for l in &mut v {
            pad_line(l, M.len() + indent + fw, bg);
        }
    }
    v
}

/// One non-fence, non-table line.
fn render_line(line: &str, w: usize) -> Vec<Line<'static>> {
    let t = line.trim_start();
    if t.is_empty() {
        return vec![Line::default()];
    }
    if let Some((level, text)) = heading(t) {
        let p = crate::skin::palette();
        let style = if level <= 2 {
            theme::fg_or_default(p.heading).add_modifier(Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::BOLD).fg(p.heading_sub)
        };
        return wrap::wrap(&inline(text, style), w, &[sp(M)], &[sp(M)]);
    }
    if is_rule(t) {
        let n = w.saturating_sub(2).min(56);
        return vec![Line::from(vec![sp(M), Span::styled("─".repeat(n), RULE())])];
    }
    if let Some(q) = t.strip_prefix('>') {
        let q = q.strip_prefix(' ').unwrap_or(q);
        let bar = [sp(M), Span::styled("│ ", DIM())];
        return wrap::wrap(&inline(q, DIM().add_modifier(Modifier::ITALIC)), w, &bar, &bar);
    }
    if let Some((indent, marker, rest)) = list_item(line) {
        let level = (indent / 2).min(4);
        let pad = format!("{M}  {}", "  ".repeat(level));
        let first = [Span::raw(pad.clone()), Span::styled(format!("{marker} "), DIM())];
        let cont = [Span::raw(format!("{pad}{}", " ".repeat(width_of(&marker) + 1)))];
        return wrap::wrap(&inline(rest, Style::new()), w, &first, &cont);
    }
    wrap::wrap(&inline(t, Style::new()), w, &[sp(M)], &[sp(M)])
}

/// Rendered cells, row by row.
type Cells = Vec<Vec<Vec<Span<'static>>>>;

/// Header row count, the rendered body and its source rows (separator dropped).
fn table_body(rows: &[Vec<String>]) -> (usize, Cells, Vec<&Vec<String>>) {
    let mut body = Vec::new();
    let mut raw = Vec::new();
    let mut header_rows = 0usize;
    for (i, r) in rows.iter().enumerate() {
        if i == 1 && is_sep_row(r) {
            header_rows = 1;
            continue;
        }
        body.push(r.iter().map(|c| inline(c, Style::new())).collect());
        raw.push(r);
    }
    (header_rows, body, raw)
}

fn render_table(rows: Vec<Vec<String>>, w: usize) -> Vec<Line<'static>> {
    let (header_rows, body, _) = table_body(&rows);
    let ncols = body.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; ncols];
    for r in &body {
        for (i, c) in r.iter().enumerate() {
            let cw: usize = c.iter().map(|s| width_of(&s.content)).sum();
            widths[i] = widths[i].max(cw);
        }
    }
    let total = widths.iter().sum::<usize>() + 3 * ncols.saturating_sub(1) + M.len() + 1;
    if ncols == 0 || total > w {
        // too wide to align — fall back to one wrapped row per table row
        let mut out = Vec::new();
        for r in &body {
            let mut spans = Vec::new();
            for (i, c) in r.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(" · ", DIM()));
                }
                spans.extend(c.iter().cloned());
            }
            out.extend(wrap::wrap(&spans, w, &[sp(M)], &[sp("   ")]));
        }
        return out;
    }
    let mut out = Vec::new();
    for (ri, r) in body.iter().enumerate() {
        let mut spans = vec![sp(M)];
        for (i, wdt) in widths.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", RULE()));
            }
            let cell = r.get(i).cloned().unwrap_or_default();
            let cw: usize = cell.iter().map(|s| width_of(&s.content)).sum();
            for s in cell {
                let st = if ri < header_rows { s.style.add_modifier(Modifier::BOLD) } else { s.style };
                spans.push(Span::styled(s.content, st));
            }
            spans.push(Span::raw(" ".repeat(wdt - cw)));
        }
        out.push(Line::from(spans));
        if ri + 1 == header_rows {
            let sep: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
            out.push(Line::from(vec![sp(M), Span::styled(sep.join("─┼─"), RULE())]));
        }
    }
    out
}

fn numeric(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '%' | '$' | '€' | '£' | '-' | '+' | '×' | 'x' | 'k' | 'M' | 'G' | 'B' | ' '))
}

/// Share `avail` columns: narrow columns keep their width, wide ones split
/// what is left. Every column gets at least `min`.
fn fit(natural: &[usize], avail: usize, min: usize) -> Vec<usize> {
    if natural.iter().sum::<usize>() <= avail {
        return natural.to_vec();
    }
    let mut order: Vec<usize> = (0..natural.len()).collect();
    order.sort_by_key(|&i| natural[i]);
    let mut out = vec![0; natural.len()];
    let mut left = avail;
    for (k, &i) in order.iter().enumerate() {
        let share = left / (natural.len() - k);
        out[i] = natural[i].min(share).max(min);
        left = left.saturating_sub(out[i]);
    }
    out
}

/// A bordered table whose cells wrap to fit; `None` when even that won't fit.
fn boxed_table(rows: &[Vec<String>], w: usize) -> Option<Vec<Line<'static>>> {
    const MIN: usize = 4;
    let (header_rows, body, raw) = table_body(rows);
    let ncols = body.iter().map(Vec::len).max().unwrap_or(0);
    let avail = w.checked_sub(M.len() + 3 * ncols + 1)?;
    if ncols == 0 || avail < ncols * MIN {
        return None;
    }
    let mut natural = vec![1usize; ncols];
    for r in &body {
        for (i, c) in r.iter().enumerate() {
            natural[i] = natural[i].max(c.iter().map(|s| width_of(&s.content)).sum());
        }
    }
    let widths = fit(&natural, avail, MIN);
    let right: Vec<bool> = (0..ncols)
        .map(|c| {
            let mut vals = raw.iter().skip(header_rows).filter_map(|r| r.get(c)).filter(|v| !v.trim().is_empty()).peekable();
            vals.peek().is_some() && vals.all(|v| numeric(v))
        })
        .collect();
    let edge = |l: &str, m: &str, r: &str| {
        let segs: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        Line::from(vec![sp(M), Span::styled(format!("{l}{}{r}", segs.join(m)), RULE())])
    };
    let mut out = vec![edge("┌", "┬", "┐")];
    for (ri, r) in body.iter().enumerate() {
        let head = ri < header_rows;
        let cols: Vec<Vec<Line<'static>>> = (0..ncols)
            .map(|i| {
                let cell: Vec<Span<'static>> = r
                    .get(i)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| if head { Span::styled(s.content, s.style.add_modifier(Modifier::BOLD)) } else { s })
                    .collect();
                if cell.is_empty() {
                    return vec![Line::default()];
                }
                wrap::wrap(&cell, widths[i], &[], &[])
            })
            .collect();
        let height = cols.iter().map(Vec::len).max().unwrap_or(1);
        for k in 0..height {
            let mut spans = vec![sp(M), Span::styled("│", RULE())];
            for (i, c) in cols.iter().enumerate() {
                let line = c.get(k).cloned().unwrap_or_default();
                let lw = wrap::line_width(&line);
                let gap = " ".repeat(widths[i].saturating_sub(lw));
                spans.push(sp(" "));
                if right[i] && !head {
                    spans.push(Span::raw(gap.clone()));
                    spans.extend(line.spans);
                } else {
                    spans.extend(line.spans);
                    spans.push(Span::raw(gap));
                }
                spans.push(sp(" "));
                spans.push(Span::styled("│", RULE()));
            }
            out.push(Line::from(spans));
        }
        if ri + 1 == header_rows {
            out.push(edge("├", "┼", "┤"));
        }
    }
    out.push(edge("└", "┴", "┘"));
    Some(out)
}

/// One paragraph of inline markdown wrapped to `w` with no margin.
pub fn inline_wrapped(text: &str, w: usize) -> Vec<Line<'static>> {
    wrap::wrap(&inline(text, Style::new()), w, &[], &[])
}

/// Render a whole (non-streamed) message.
pub fn render(text: &str, w: usize) -> Vec<Line<'static>> {
    let mut md = Md::new();
    let mut out = Vec::new();
    for l in text.split('\n') {
        out.extend(md.push(l, w));
    }
    out.extend(md.finish(w));
    while out.last().is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty())) {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skin::scoped_skin;

    fn text(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.to_string()).collect()
    }

    fn classic<T>(f: impl FnOnce() -> T) -> T {
        scoped_skin(Skin::classic(), f)
    }

    /// Cosmos with plain inline links, so text is the same in every terminal.
    fn cosmos<T>(f: impl FnOnce() -> T) -> T {
        let mut sk = Skin::default();
        sk.markdown.links = "inline".into();
        scoped_skin(sk, f)
    }

    fn rows(src: &str, w: usize) -> Vec<String> {
        render(src, w).iter().map(text).collect()
    }

    #[test]
    fn inline_styles() {
        let s = inline("a **b** *c* `d` ~~e~~", Style::new());
        let b = s.iter().find(|x| x.content == "b").unwrap();
        assert!(b.style.add_modifier.contains(Modifier::BOLD));
        let c = s.iter().find(|x| x.content == "c").unwrap();
        assert!(c.style.add_modifier.contains(Modifier::ITALIC));
        assert_eq!(s.iter().find(|x| x.content == "d").unwrap().style, CODE_INLINE());
        let e = s.iter().find(|x| x.content == "e").unwrap();
        assert!(e.style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn bullets_and_numbers() {
        classic(|| {
            let l = rows("- one\n2. two\n  - nested", 40);
            assert_eq!(l, vec!["   • one", "   2. two", "     • nested"]);
        });
        cosmos(|| {
            let l = rows("- one\n2. two\n  - nested\n    - deeper", 40);
            assert_eq!(l, vec!["   • one", "   2. two", "     ◦ nested", "       ▸ deeper"]);
        });
    }

    #[test]
    fn streaming_matches_whole() {
        let src = "# Title\nSome **bold** text\n```rust\nfn x() {}\n```\n| a | b |\n|---|---|\n| 1 | 22 |\nafter\n> [!TIP]\n> try it\n- item\n  more\nUnder\n===\n```\nunclosed";
        let check = || {
            let whole: Vec<String> = render(src, 60).iter().map(text).collect();
            let mut md = Md::new();
            let mut streamed = Vec::new();
            for l in src.split('\n') {
                streamed.extend(md.push(l, 60));
            }
            streamed.extend(md.finish(60));
            let mut streamed: Vec<String> = streamed.iter().map(text).collect();
            while streamed.last().is_some_and(|l| l.trim().is_empty()) {
                streamed.pop();
            }
            assert_eq!(whole, streamed);
        };
        classic(check);
        cosmos(check);
    }

    #[test]
    fn table_aligns() {
        classic(|| {
            let l = rows("| name | n |\n|---|---|\n| alpha | 1 |\n| b | 22 |", 60);
            assert_eq!(l[0], " name  │ n ");
            assert!(l[1].contains("┼"));
            assert_eq!(l[2], " alpha │ 1 ");
        });
    }

    #[test]
    fn cosmos_tables_have_borders_numbers_right_and_wrap() {
        cosmos(|| {
            let l = rows("| name | n |\n|---|---|\n| alpha | 1 |\n| b | 22 |", 60);
            assert_eq!(l, vec![
                " ┌───────┬────┐",
                " │ name  │ n  │",
                " ├───────┼────┤",
                " │ alpha │  1 │",
                " │ b     │ 22 │",
                " └───────┴────┘",
            ]);
            let long = "| what | notes |\n|---|---|\n| a | this cell is far too long to fit on one row of a narrow terminal |";
            let l = rows(long, 40);
            assert!(l.iter().all(|r| width_of(r) <= 40), "{l:#?}");
            assert!(l.len() > 5, "the long cell wraps onto more rows: {l:#?}");
            assert!(l.iter().filter(|r| r.starts_with(" │")).all(|r| r.ends_with('│')), "{l:#?}");
        });
    }

    #[test]
    fn code_is_highlighted() {
        classic(|| {
            let l = render("```rust\nfn main() {}\n```", 60);
            assert_eq!(l.len(), 1);
            assert!(l[0].spans.iter().any(|s| s.content == "fn" && s.style != CODE_INLINE()));
        });
    }

    #[test]
    fn cosmos_code_is_framed_and_tinted() {
        cosmos(|| {
            let l = render("```rust\nfn main() {}\n```", 40);
            let t: Vec<String> = l.iter().map(text).collect();
            assert!(t[0].starts_with(" ╭─ rust ─"), "{t:?}");
            assert!(t[1].starts_with(" │ fn main() {}"), "{t:?}");
            assert!(t[2].starts_with(" ╰──"), "{t:?}");
            assert!(l[1].spans.iter().any(|s| s.content == "fn" && s.style.bg == Some(crate::skin::palette().code_bg)));
            assert!(t.iter().all(|r| width_of(r) <= 40), "{t:?}");
            // an unclosed fence still closes its frame
            let t = rows("```\nlet x = 1;", 40);
            assert!(t.last().unwrap().starts_with(" ╰"), "{t:?}");
        });
    }

    #[test]
    fn long_fences_highlight_a_bounded_window() {
        classic(|| {
            // the highlighter only ever sees a bounded tail, however long the fence
            let code = "let x = \"s\"; // c\n".repeat(3000);
            assert_eq!(window(&code).lines().count(), HL_WINDOW - 1);
            assert_eq!(window("a\nb"), "a\nb");
            let src = format!("```rust\n{code}```");
            let l = render(&src, 80);
            assert_eq!(l.len(), 3000);
            assert!(l[2999].spans.iter().any(|s| s.content.contains("// c") && s.style != CODE_INLINE()));
        });
    }

    #[test]
    fn callouts_and_nested_quotes() {
        cosmos(|| {
            let l = render("> [!WARNING]\n> mind the gap\n\n> > deep", 40);
            let t: Vec<String> = l.iter().map(text).collect();
            assert_eq!(t[0], " ▌ ▲ Warning");
            assert_eq!(t[1], " ▌ mind the gap");
            assert_eq!(t[3], " │ │ deep");
            assert_eq!(l[0].spans[1].style.fg, Some(crate::skin::palette().warn));
        });
    }

    #[test]
    fn list_items_continue_and_setext_underlines() {
        cosmos(|| {
            let t = rows("- first\n  still first\nplain\nTitle\n===", 40);
            assert_eq!(t[0], "   • first");
            assert_eq!(t[1], "     still first");
            assert_eq!(t[2], " plain");
            assert_eq!(t[3], " Title");
            assert!(t[4].starts_with(" ━━━"), "{t:?}");
            let t = rows("# Big", 40);
            assert_eq!(t, vec![" Big", " ━━━"]);
        });
    }

    #[test]
    fn links_show_target_or_become_hyperlinks() {
        classic(|| {
            let s = inline("see [docs](https://x.dev)", Style::new());
            let t: String = s.iter().map(|x| x.content.to_string()).collect();
            assert_eq!(t, "see docs (https://x.dev)");
        });
        cosmos(|| {
            let s = inline("go to https://x.dev/a_(b). Now", Style::new());
            let link = s.iter().find(|x| x.content.starts_with("https")).unwrap();
            assert_eq!(link.content, "https://x.dev/a_(b)");
            assert_eq!(s.last().unwrap().content, ". Now");
        });
        let mut sk = Skin::default();
        sk.markdown.links = "osc8".into();
        scoped_skin(sk, || {
            let s = inline("see [docs](https://x.dev/ü)", Style::new());
            let t: String = s.iter().map(|x| x.content.to_string()).collect();
            assert_eq!(width_of(&t), "see docs".len(), "markers take no columns");
            let mut buf = Vec::new();
            crate::term::write_line(&mut buf, &Line::from(s), 80);
            let out = String::from_utf8_lossy(&buf);
            assert!(out.contains("\x1b]8;;https://x.dev/%C3%BC\x1b\\"), "{out:?}");
            assert!(out.contains("\x1b]8;;\x1b\\"), "{out:?}");
        });
    }
}

#[cfg(test)]
mod dump {
    /// Eyeball test — `cargo test -p ztui dump_markdown -- --nocapture`.
    #[test]
    fn dump_markdown() {
        let mut sk = crate::skin::Skin::default();
        sk.markdown.links = "inline".into();
        crate::skin::scoped_skin(sk, || {
            let src = "# Cosmos markdown\nPlain **bold** _it_ `code` and https://example.com/docs.\n\n## Lists\n- one\n  continued under one\n  - nested\n    - deeper\n1. first\n- [x] done\n\n> [!NOTE]\n> Notes get an icon.\n\n> [!CAUTION]\n> Careful now.\n\n> quoted\n> > nested quote\n\n```rust\nfn main() {\n    let s = \"hi\"; // greet\n}\n```\n\n| tool | calls | notes |\n|------|------:|-------|\n| read | 12 | reads files from the workspace with line ranges |\n| bash | 3 | runs commands |\n\nSetext\n===\n### Third\n---";
            for l in super::render(src, 64) {
                println!("{}", l.spans.iter().map(|s| s.content.to_string()).collect::<String>());
            }
        });
    }
}
