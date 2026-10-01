//! Width-aware, word-aware wrapping for styled lines, with hanging indents
//! (the first row gets one prefix, continuation rows another) so bullets,
//! quotes and tool output wrap under their text instead of the margin.

use ratatui_core::style::Style;
use ratatui_core::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

pub fn cw(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

pub fn width_of(s: &str) -> usize {
    s.chars().map(cw).sum()
}

pub fn line_width(l: &Line<'_>) -> usize {
    l.spans.iter().map(|s| width_of(&s.content)).sum()
}

/// A run of characters sharing one style, either all-space or no-space.
struct Tok {
    text: String,
    style: Style,
    space: bool,
    w: usize,
}

fn tokens(spans: &[Span<'_>]) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    for s in spans {
        for ch in s.content.chars() {
            let ch = if ch == '\t' { ' ' } else { ch };
            let space = ch == ' ';
            match out.last_mut() {
                Some(t) if t.space == space && t.style == s.style => {
                    t.text.push(ch);
                    t.w += cw(ch);
                }
                _ => out.push(Tok { text: ch.to_string(), style: s.style, space, w: cw(ch) }),
            }
        }
    }
    out
}

/// Wrap `spans` into lines of at most `width` columns. `first` prefixes the
/// first row, `rest` every continuation row. Words move to the next row
/// whole; a word longer than a row is split.
pub fn wrap(spans: &[Span<'_>], width: usize, first: &[Span<'static>], rest: &[Span<'static>]) -> Vec<Line<'static>> {
    let width = width.max(4);
    let pw = |p: &[Span<'static>]| p.iter().map(|s| width_of(&s.content)).sum::<usize>();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = first.to_vec();
    let mut col = pw(first);
    let mut fresh = true; // nothing but prefix on this row yet
    // prefix spans on the current row — never merged into or trimmed
    let mut base = first.len();

    let push = |cur: &mut Vec<Span<'static>>, base: usize, text: &str, style: Style| {
        if cur.len() > base {
            let last = cur.last_mut().unwrap();
            if last.style == style {
                let mut s = last.content.to_string();
                s.push_str(text);
                last.content = s.into();
                return;
            }
        }
        cur.push(Span::styled(text.to_string(), style));
    };

    for t in tokens(spans) {
        if t.space {
            if fresh && !out.is_empty() {
                continue; // drop leading spaces on continuation rows
            }
            if col + t.w <= width {
                push(&mut cur, base, &t.text, t.style);
                col += t.w;
            }
            continue;
        }
        if col + t.w > width && !fresh {
            // trim trailing spaces before breaking (never into the prefix)
            while cur.len() > base {
                let last = cur.last_mut().unwrap();
                let trimmed = last.content.trim_end().to_string();
                if trimmed.is_empty() {
                    cur.pop();
                } else {
                    last.content = trimmed.into();
                    break;
                }
            }
            out.push(Line::from(std::mem::take(&mut cur)));
            cur = rest.to_vec();
            base = rest.len();
            col = pw(rest);
        }
        if col + t.w <= width {
            push(&mut cur, base, &t.text, t.style);
            col += t.w;
            fresh = false;
            continue;
        }
        // word longer than a whole row: split by characters
        let mut piece = String::new();
        for ch in t.text.chars() {
            let w = cw(ch);
            if col + w > width && col > pw(rest) {
                push(&mut cur, base, &piece, t.style);
                piece.clear();
                out.push(Line::from(std::mem::take(&mut cur)));
                cur = rest.to_vec();
                base = rest.len();
                col = pw(rest);
            }
            piece.push(ch);
            col += w;
        }
        push(&mut cur, base, &piece, t.style);
        fresh = false;
    }
    out.push(Line::from(cur));
    out
}

/// Pad a line to exactly `width` columns with spaces styled `fill`
/// (full-width bars: user turns, selected rows, diff lines).
pub fn pad_line(line: &mut Line<'static>, width: usize, fill: Style) {
    let w = line_width(line);
    if w < width {
        line.spans.push(Span::styled(" ".repeat(width - w), fill));
    }
}

/// Truncate a plain string to `max` columns, adding `…` when cut.
pub fn ellipsize(s: &str, max: usize) -> String {
    if width_of(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let c = cw(ch);
        if w + c + 1 > max {
            break;
        }
        out.push(ch);
        w += c;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn wraps_at_words() {
        let l = wrap(&[Span::raw("the quick brown fox")], 10, &[], &[]);
        let rows: Vec<String> = l.iter().map(text).collect();
        assert_eq!(rows, vec!["the quick", "brown fox"]);
    }

    #[test]
    fn hanging_indent() {
        let l = wrap(&[Span::raw("alpha beta gamma")], 10, &[Span::raw("• ")], &[Span::raw("  ")]);
        let rows: Vec<String> = l.iter().map(text).collect();
        assert_eq!(rows, vec!["• alpha", "  beta", "  gamma"]);
    }

    #[test]
    fn splits_long_words() {
        let l = wrap(&[Span::raw("abcdefghij")], 4, &[], &[]);
        assert!(l.iter().all(|r| line_width(r) <= 4));
        assert_eq!(l.iter().map(text).collect::<String>(), "abcdefghij");
    }

    #[test]
    fn wide_chars() {
        let l = wrap(&[Span::raw("你好世界")], 4, &[], &[]);
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn empty_is_one_row() {
        assert_eq!(wrap(&[], 10, &[], &[]).len(), 1);
    }

    #[test]
    fn ellipsis() {
        assert_eq!(ellipsize("abcdef", 4), "abc…");
        assert_eq!(ellipsize("ab", 4), "ab");
    }
}
