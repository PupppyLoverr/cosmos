//! Streaming markdown → styled terminal lines.
//!
//! Line-oriented on purpose: a completed source line always renders the same
//! way whatever arrives after it, so the TUI can commit lines to scrollback
//! while the reply is still streaming (the `mdstream` idea from Devin's
//! stack). Inline spans go through pulldown-cmark; tables are held until the
//! block ends so columns can be aligned; fenced code is highlighted by
//! tree-sitter against everything seen so far in the fence.

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::highlight;
use crate::theme::{self, CODE_INLINE, DIM, RULE};
use crate::wrap::{self, width_of};

/// Left margin for assistant text (Devin indents replies one column).
const M: &str = " ";

struct Fence {
    lang: String,
    tick: String,
    code: String,
}

#[derive(Default)]
pub struct Md {
    fence: Option<Fence>,
    table: Vec<String>,
}

fn sp(s: &str) -> Span<'static> {
    Span::raw(s.to_string())
}

/// Inline markdown (bold, italic, `code`, ~~strike~~, links) → spans.
pub fn inline(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut stack: Vec<Style> = vec![base];
    let top = |s: &Vec<Style>| *s.last().unwrap();
    let mut link: Option<String> = None;
    let parser = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH);
    for ev in parser {
        match ev {
            Event::Start(Tag::Strong) => stack.push(top(&stack).add_modifier(Modifier::BOLD)),
            Event::Start(Tag::Emphasis) => stack.push(top(&stack).add_modifier(Modifier::ITALIC)),
            Event::Start(Tag::Strikethrough) => stack.push(top(&stack).add_modifier(Modifier::CROSSED_OUT)),
            Event::Start(Tag::Link { dest_url, .. }) => {
                link = Some(dest_url.to_string());
                stack.push(top(&stack).fg(theme::ACCENT).add_modifier(Modifier::UNDERLINED));
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
                // show the target when the label doesn't already say it
                if let Some(url) = link.take() {
                    let shown: String = out.iter().rev().take(1).map(|s| s.content.to_string()).collect();
                    if !url.is_empty() && shown != url {
                        out.push(Span::styled(format!(" ({url})"), DIM));
                    }
                }
            }
            Event::Text(t) => out.push(Span::styled(t.to_string(), top(&stack))),
            Event::Code(t) => out.push(Span::styled(t.to_string(), CODE_INLINE)),
            Event::Html(t) | Event::InlineHtml(t) => out.push(Span::styled(t.to_string(), top(&stack))),
            Event::SoftBreak | Event::HardBreak => out.push(Span::styled(" ", top(&stack))),
            Event::TaskListMarker(done) => out.push(Span::raw(if done { "☑ " } else { "☐ " })),
            _ => {}
        }
    }
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

impl Md {
    pub fn new() -> Md {
        Md::default()
    }

    /// Feed one complete source line; returns rendered rows ready to commit.
    pub fn push(&mut self, raw: &str, w: usize) -> Vec<Line<'static>> {
        let line = raw.trim_end_matches('\r');
        let t = line.trim_start();

        if let Some(f) = &mut self.fence {
            if t.starts_with(f.tick.as_str()) && t[f.tick.len()..].trim().is_empty() {
                self.fence = None;
                return Vec::new();
            }
            if !f.code.is_empty() {
                f.code.push('\n');
            }
            f.code.push_str(line);
            let spans = highlight::highlight(&f.lang, &f.code)
                .and_then(|mut l| l.pop())
                .unwrap_or_else(|| vec![Span::styled(line.to_string(), CODE_INLINE)]);
            return code_rows(&spans, w);
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
                self.fence = Some(Fence {
                    lang: rest.trim().trim_start_matches('`').to_string(),
                    tick: tick.to_string(),
                    code: String::new(),
                });
                return out;
            }
        }
        if is_table_row(t) {
            self.table.push(line.to_string());
            return out;
        }
        out.extend(render_line(line, w));
        out
    }

    /// End of message: flush anything held back (open table, open fence).
    pub fn finish(&mut self, w: usize) -> Vec<Line<'static>> {
        self.fence = None;
        self.flush_table(w)
    }

    /// Render an incomplete trailing line for the live region (no state change).
    pub fn preview(&self, partial: &str, w: usize) -> Vec<Line<'static>> {
        if partial.is_empty() {
            return Vec::new();
        }
        if self.fence.is_some() {
            return code_rows(&[Span::styled(partial.to_string(), CODE_INLINE)], w);
        }
        if !self.table.is_empty() || is_table_row(partial) {
            let mut rows = Vec::new();
            for l in self.table.iter().map(String::as_str).chain(std::iter::once(partial)) {
                rows.extend(wrap::wrap(&[Span::styled(l.to_string(), DIM)], w, &[sp(M)], &[sp(M)]));
            }
            return rows;
        }
        render_line(partial, w)
    }

    fn flush_table(&mut self, w: usize) -> Vec<Line<'static>> {
        if self.table.is_empty() {
            return Vec::new();
        }
        let rows: Vec<Vec<String>> = std::mem::take(&mut self.table).iter().map(|r| cells(r)).collect();
        render_table(rows, w)
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

/// One non-fence, non-table line.
fn render_line(line: &str, w: usize) -> Vec<Line<'static>> {
    let t = line.trim_start();
    if t.is_empty() {
        return vec![Line::default()];
    }
    if let Some((level, text)) = heading(t) {
        let style = if level <= 2 {
            Style::new().add_modifier(Modifier::BOLD)
        } else {
            Style::new().add_modifier(Modifier::BOLD).fg(ratatui::style::Color::Rgb(0xc8, 0xc8, 0xc8))
        };
        return wrap::wrap(&inline(text, style), w, &[sp(M)], &[sp(M)]);
    }
    if is_rule(t) {
        let n = w.saturating_sub(2).min(56);
        return vec![Line::from(vec![sp(M), Span::styled("─".repeat(n), RULE)])];
    }
    if let Some(q) = t.strip_prefix('>') {
        let q = q.strip_prefix(' ').unwrap_or(q);
        let bar = [sp(M), Span::styled("│ ", DIM)];
        return wrap::wrap(&inline(q, DIM.add_modifier(Modifier::ITALIC)), w, &bar, &bar);
    }
    if let Some((indent, marker, rest)) = list_item(line) {
        let level = (indent / 2).min(4);
        let pad = format!("{M}  {}", "  ".repeat(level));
        let first = [Span::raw(pad.clone()), Span::styled(format!("{marker} "), DIM)];
        let cont = [Span::raw(format!("{pad}{}", " ".repeat(width_of(&marker) + 1)))];
        return wrap::wrap(&inline(rest, Style::new()), w, &first, &cont);
    }
    wrap::wrap(&inline(t, Style::new()), w, &[sp(M)], &[sp(M)])
}

fn render_table(rows: Vec<Vec<String>>, w: usize) -> Vec<Line<'static>> {
    let mut body: Vec<Vec<Vec<Span<'static>>>> = Vec::new();
    let mut header_rows = 0usize;
    for (i, r) in rows.iter().enumerate() {
        if i == 1 && is_sep_row(r) {
            header_rows = 1;
            continue;
        }
        body.push(r.iter().map(|c| inline(c, Style::new())).collect());
    }
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
                    spans.push(Span::styled(" · ", DIM));
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
                spans.push(Span::styled(" │ ", RULE));
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
            out.push(Line::from(vec![sp(M), Span::styled(sep.join("─┼─"), RULE)]));
        }
    }
    out
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

    fn text(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn inline_styles() {
        let s = inline("a **b** *c* `d` ~~e~~", Style::new());
        let b = s.iter().find(|x| x.content == "b").unwrap();
        assert!(b.style.add_modifier.contains(Modifier::BOLD));
        let c = s.iter().find(|x| x.content == "c").unwrap();
        assert!(c.style.add_modifier.contains(Modifier::ITALIC));
        assert_eq!(s.iter().find(|x| x.content == "d").unwrap().style, CODE_INLINE);
        let e = s.iter().find(|x| x.content == "e").unwrap();
        assert!(e.style.add_modifier.contains(Modifier::CROSSED_OUT));
    }

    #[test]
    fn bullets_and_numbers() {
        let l = render("- one\n2. two\n  - nested", 40);
        assert_eq!(text(&l[0]), "   • one");
        assert_eq!(text(&l[1]), "   2. two");
        assert_eq!(text(&l[2]), "     • nested");
    }

    #[test]
    fn streaming_matches_whole() {
        let src = "# Title\nSome **bold** text\n```rust\nfn x() {}\n```\n| a | b |\n|---|---|\n| 1 | 22 |\nafter";
        let whole: Vec<String> = render(src, 60).iter().map(text).collect();
        let mut md = Md::new();
        let mut streamed = Vec::new();
        for l in src.split('\n') {
            streamed.extend(md.push(l, 60));
        }
        streamed.extend(md.finish(60));
        let streamed: Vec<String> = streamed.iter().map(text).collect();
        assert_eq!(whole, streamed);
    }

    #[test]
    fn table_aligns() {
        let l = render("| name | n |\n|---|---|\n| alpha | 1 |\n| b | 22 |", 60);
        let rows: Vec<String> = l.iter().map(text).collect();
        assert_eq!(rows[0], " name  │ n ");
        assert!(rows[1].contains("┼"));
        assert_eq!(rows[2], " alpha │ 1 ");
    }

    #[test]
    fn code_is_highlighted() {
        let l = render("```rust\nfn main() {}\n```", 60);
        assert_eq!(l.len(), 1);
        assert!(l[0].spans.iter().any(|s| s.content == "fn" && s.style != CODE_INLINE));
    }

    #[test]
    fn links_show_target() {
        let s = inline("see [docs](https://x.dev)", Style::new());
        let t: String = s.iter().map(|x| x.content.to_string()).collect();
        assert_eq!(t, "see docs (https://x.dev)");
    }
}
