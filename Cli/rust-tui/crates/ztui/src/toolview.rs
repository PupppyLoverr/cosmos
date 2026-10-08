//! Tool calls and file diffs, drawn in the skin's style.
//!
//! `classic` keeps the original layout (`view::tool_block`). `cosmos` puts
//! what matters in the header (`◆ Read src/a.rs · 412 lines · 0.2s`), folds
//! runs of reads and searches into one group, and draws diffs with hunk
//! markers, syntax colour, word-level highlights, wrapped long lines and a
//! side-by-side layout on wide terminals. Full output is kept for the ctrl+o
//! pager, so nothing is lost by showing less.

use std::time::Duration;

use ratatui_core::style::{Modifier, Style};
use ratatui_core::text::{Line, Span};
use serde_json::Value;

use crate::skin::Skin;
use crate::theme::{self, DIM, GREEN, RED};
use crate::view::{arg_summary, tool_label};
use crate::worker::{Diff, DiffLine};
use crate::wrap::{ellipsize, pad_line, width_of};

/// One finished (or interrupted) tool call.
pub struct ToolView<'a> {
    pub name: &'a str,
    pub input: &'a Value,
    pub content: Option<&'a str>,
    pub err: bool,
    pub diff: Option<&'a Diff>,
    pub interrupted: bool,
    pub elapsed: Option<Duration>,
}

/// A read/search folded into an explore group.
#[derive(Clone)]
pub struct Explored {
    pub name: String,
    pub input: Value,
    pub content: String,
    pub elapsed: Option<Duration>,
}

/// Tools that only look around — safe to fold together.
pub fn is_explore(name: &str) -> bool {
    matches!(name, "read" | "grep" | "glob" | "ls")
}

/// Whether a finished call should wait to be folded into a group.
pub fn groups(sk: &Skin, name: &str, err: bool) -> bool {
    sk.tools.style != "classic" && sk.tools.group && !err && is_explore(name)
}

fn sp(s: impl Into<String>) -> Span<'static> {
    Span::raw(s.into())
}
fn dim(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), DIM())
}

/// `0.2s`, `4.1s`, `12s`, `1m 05s` — `None` under 50 ms.
pub fn took(d: Option<Duration>) -> Option<String> {
    let d = d?;
    let ms = d.as_millis();
    if ms < 50 {
        None
    } else if ms < 10_000 {
        Some(format!("{:.1}s", ms as f64 / 1000.0))
    } else {
        Some(crate::motion::elapsed(d.as_secs()))
    }
}

/// The bash result split into (exit code, note, body). Anything that isn't
/// the shell fence comes back as the body unchanged.
pub fn shell_parts(content: &str) -> (Option<String>, Option<String>, String) {
    let Some(rest) = content.strip_prefix("exit_code=") else {
        return (None, None, content.to_string());
    };
    let (head, tail) = rest.split_once('\n').unwrap_or((rest, ""));
    let code = head.split([' ', '(']).next().unwrap_or("").to_string();
    let note = head.split_once('(').and_then(|(_, n)| n.split_once(')')).map(|(n, _)| n.to_string());
    let body = tail.strip_prefix("<<<UNTRUSTED_SHELL\n").unwrap_or(tail);
    let body = body.strip_suffix(">>>").unwrap_or(body);
    let body = body.strip_suffix('\n').unwrap_or(body);
    (Some(code), note, body.to_string())
}

/// The text a call's result rows are drawn from (bash loses its fence).
pub fn body_of(name: &str, content: &str) -> String {
    if name == "bash" {
        shell_parts(content).2
    } else {
        content.to_string()
    }
}

fn nonblank(s: &str) -> Vec<&str> {
    s.lines().filter(|l| !l.trim().is_empty()).collect()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Short result facts for the header: (text, style).
fn meta(v: &ToolView, body: &str) -> Vec<(String, Style)> {
    let mut m: Vec<(String, Style)> = Vec::new();
    let content = v.content.unwrap_or("");
    if v.interrupted {
        m.push(("interrupted".into(), DIM()));
        return m;
    }
    if let Some(d) = v.diff {
        if d.created {
            m.push(("new file".into(), theme::fg(GREEN())));
            m.push((plural(d.added, "line", "lines"), DIM()));
        } else {
            m.push((format!("+{}", d.added), theme::fg(GREEN())));
            m.push((format!("−{}", d.removed), theme::fg(RED())));
        }
    } else if !v.err {
        let n = nonblank(body).len();
        match v.name {
            "read" => m.push((plural(content.lines().count(), "line", "lines"), DIM())),
            "grep" if n == 0 => m.push(("no matches".into(), DIM())),
            "grep" => m.push((plural(n, "match", "matches"), DIM())),
            "glob" | "ls" if n == 0 => m.push(("nothing found".into(), DIM())),
            "glob" => m.push((plural(n, "file", "files"), DIM())),
            "ls" => m.push((plural(n, "entry", "entries"), DIM())),
            "memory_write" => m.push(("saved".into(), DIM())),
            "bash" => {
                let (code, note, _) = shell_parts(content);
                match code.as_deref() {
                    Some("running") => m.push(("in background".into(), theme::fg(theme::YELLOW()))),
                    Some("0") | None => {}
                    Some(c) => {
                        let mut s = format!("exit {c}");
                        if let Some(n) = note.as_deref().and_then(|n| n.split(" (").next()) {
                            s.push_str(&format!(" · {n}"));
                        }
                        m.push((s, theme::fg(theme::YELLOW())));
                    }
                }
            }
            _ => {}
        }
    }
    if crate::skin::current().tools.show_elapsed {
        if let Some(t) = took(v.elapsed) {
            m.push((t, DIM()));
        }
    }
    m
}

/// ` ◆ Label arg · meta · meta`, fitted to `w`.
/// `devin:execute` → Some(("devin", "execute")) for a subscription agent's tool.
pub fn vendor(name: &str) -> Option<(&str, &str)> {
    name.split_once(':').filter(|(v, _)| matches!(*v, "cursor" | "devin" | "codex"))
}

fn header(glyph: Span<'static>, name: &str, input: &Value, meta: &[(String, Style)], w: usize) -> Line<'static> {
    let mut meta: Vec<(String, Style)> = meta.to_vec();
    let (label, arg_from) = match vendor(name) {
        // the vendor's title already says what it did ("Ran python.exe")
        Some((v, kind)) => {
            meta.insert(0, (v.to_string(), DIM()));
            let t = input["title"].as_str().map(str::trim).filter(|t| !t.is_empty());
            (t.map(str::to_string).unwrap_or_else(|| kind.to_string()), false)
        }
        None => (tool_label(name), true),
    };
    let meta_w: usize = meta.iter().map(|(s, _)| width_of(s) + 3).sum();
    let room = w.saturating_sub(5 + meta_w);
    let label = ellipsize(&label.replace('\n', " "), room.max(12));
    let room = room.saturating_sub(width_of(&label));
    let arg = if arg_from { arg_summary(name, input, room.max(8)) } else { String::new() };
    let mut spans = vec![sp(" "), glyph, Span::styled(label, theme::bold(Style::new()))];
    if !arg.is_empty() {
        spans.push(Span::styled(format!(" {arg}"), Style::new().fg(crate::skin::palette().text)));
    }
    for (s, st) in &meta {
        spans.push(dim(" · "));
        spans.push(Span::styled(s.clone(), *st));
    }
    Line::from(spans)
}

fn elbow(first: bool) -> String {
    if first {
        format!("   {}  ", crate::skin::current().glyphs.elbow)
    } else {
        "      ".to_string()
    }
}

/// Up to `keep` rows of output, then `… +N lines · ctrl+o`.
fn rows(lines: &[&str], style: Style, w: usize, keep: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (i, l) in lines.iter().take(keep).enumerate() {
        let pre = elbow(i == 0);
        let room = w.saturating_sub(width_of(&pre));
        out.push(Line::from(vec![dim(pre), Span::styled(ellipsize(&l.replace('\t', "    "), room), style)]));
    }
    if lines.len() > keep {
        out.push(Line::from(dim(format!("      … +{} lines · ctrl+o to expand", lines.len() - keep))));
    }
    out
}

/// The glyph before a tool, coloured by how it went.
fn status_glyph(v: &ToolView) -> Span<'static> {
    let style = if v.interrupted {
        DIM()
    } else if v.err {
        theme::fg(RED())
    } else {
        theme::fg(GREEN())
    };
    Span::styled(format!("{} ", crate::skin::current().glyphs.tool), style)
}

/// A finished tool call in the skin's style.
pub fn render(v: &ToolView, w: usize) -> Vec<Line<'static>> {
    let sk = crate::skin::current();
    if sk.tools.style == "classic" {
        return crate::view::tool_block(v.name, v.input, v.content, v.err, v.diff, w, v.interrupted);
    }
    let content = v.content.unwrap_or("");
    let body = body_of(v.name, content);
    let meta = meta(v, &body);
    let mut out = vec![header(status_glyph(v), v.name, v.input, &meta, w)];
    if v.interrupted {
        return out;
    }
    let lines = nonblank(&body);
    let keep = sk.tools.keep_rows;
    if v.err {
        out.extend(rows(&lines, theme::fg(RED()), w, keep.max(1)));
        return out;
    }
    if let Some(d) = v.diff {
        let rows = d.lines.len();
        if sk.diff.inline_rows > 0 && rows <= sk.diff.inline_rows {
            out.extend(diff(d, w));
        } else {
            out.push(Line::from(vec![
                dim(elbow(true)),
                dim(format!("{rows} diff lines · ")),
                Span::styled("ctrl+o", theme::fg(theme::ACCENT())),
                dim(" to view"),
            ]));
        }
        return out;
    }
    match v.name {
        // the header already says it all
        "read" | "memory_write" => {}
        "grep" | "glob" | "ls" => out.extend(rows(&lines, DIM(), w, sk.tools.keep_rows_search)),
        "edit" | "write" => out.extend(rows(&lines[..lines.len().min(1)], DIM(), w, 1)),
        "bash" if lines.is_empty() => {}
        _ if lines.is_empty() => out.push(Line::from(vec![dim(elbow(true)), dim("(no output)")])),
        _ => out.extend(rows(&lines, DIM(), w, keep)),
    }
    out
}

/// A tool still running: ` ✦ Label arg · 3s`, the glyph from the spinner.
pub fn running(name: &str, input: &Value, spinner: Span<'static>, elapsed: Duration, w: usize) -> Line<'static> {
    let meta: Vec<(String, Style)> = match elapsed.as_secs() {
        0 => Vec::new(),
        s => vec![(crate::motion::elapsed(s), DIM())],
    };
    header(spinner, name, input, &meta, w)
}

/// Reads and searches folded together:
/// ` ◇ Explored 3 files · 2 searches` plus one compact row per call.
pub fn group(items: &[Explored], w: usize) -> Vec<Line<'static>> {
    if items.len() == 1 {
        let it = &items[0];
        let v = ToolView { name: &it.name, input: &it.input, content: Some(&it.content), err: false, diff: None, interrupted: false, elapsed: it.elapsed };
        return render(&v, w);
    }
    let sk = crate::skin::current();
    let count = |f: &dyn Fn(&str) -> bool| items.iter().filter(|i| f(&i.name)).count();
    let mut facts = Vec::new();
    let reads = count(&|n| n == "read");
    let searches = count(&|n| n == "grep" || n == "glob");
    let lists = count(&|n| n == "ls");
    if reads > 0 {
        facts.push(plural(reads, "file", "files"));
    }
    if searches > 0 {
        facts.push(plural(searches, "search", "searches"));
    }
    if lists > 0 {
        facts.push(plural(lists, "listing", "listings"));
    }
    let total: Duration = items.iter().filter_map(|i| i.elapsed).sum();
    if sk.tools.show_elapsed {
        if let Some(t) = took(Some(total)) {
            facts.push(t);
        }
    }
    let mut out = vec![Line::from(vec![
        sp(" "),
        Span::styled(format!("{} ", sk.glyphs.group), theme::fg(theme::ACCENT())),
        Span::styled("Explored", theme::bold(Style::new())),
        dim(format!(" {}", facts.join(" · "))),
    ])];
    const SHOW: usize = 6;
    for (i, it) in items.iter().take(SHOW).enumerate() {
        let v = ToolView { name: &it.name, input: &it.input, content: Some(&it.content), err: false, diff: None, interrupted: false, elapsed: None };
        let body = body_of(&it.name, &it.content);
        let facts: Vec<String> = meta(&v, &body).into_iter().map(|(s, _)| s).collect();
        let pre = elbow(i == 0);
        let label = tool_label(&it.name);
        let tail = if facts.is_empty() { String::new() } else { format!(" · {}", facts.join(" · ")) };
        let room = w.saturating_sub(width_of(&pre) + width_of(&label) + width_of(&tail) + 1);
        out.push(Line::from(vec![
            dim(pre),
            Span::styled(label, DIM().add_modifier(Modifier::BOLD)),
            dim(format!(" {}{tail}", arg_summary(&it.name, &it.input, room.max(8)))),
        ]));
    }
    if items.len() > SHOW {
        out.push(Line::from(dim(format!("      … +{} more · ctrl+o to expand", items.len() - SHOW))));
    }
    out
}

// ---- diffs -------------------------------------------------------------------

/// The rows the skin shows, and how many it leaves for the pager.
pub fn capped(lines: &[DiffLine], max: usize) -> (&[DiffLine], usize) {
    let max = max.max(1);
    if lines.len() > max {
        (&lines[..max], lines.len() - max)
    } else {
        (lines, 0)
    }
}

/// A file diff in the skin's diff style.
pub fn diff(d: &Diff, w: usize) -> Vec<Line<'static>> {
    let sk = crate::skin::current();
    if sk.diff.style == "classic" {
        return crate::view::diff_rows(d, w);
    }
    diff_with(d, w, sk.diff.max_rows)
}

/// The whole diff, every row (the ctrl+o pager).
pub fn diff_full(d: &Diff, w: usize) -> Vec<Line<'static>> {
    diff_with(d, w, usize::MAX)
}

fn diff_with(d: &Diff, w: usize, max: usize) -> Vec<Line<'static>> {
    let sk = crate::skin::current();
    let (shown, more) = capped(&d.lines, max);
    let split = match sk.diff.layout.as_str() {
        "split" => w >= 80,
        "unified" => false,
        _ => w >= 140,
    } && !d.created
        && d.removed > 0;
    let mut out = if split { split_rows(d, shown, w, &sk) } else { unified_rows(d, shown, w, &sk) };
    if more > 0 {
        out.push(Line::from(dim(format!("      … {} more lines · ctrl+o to expand", more))));
    }
    out
}

/// Syntax spans for each row's text (one lexer pass over the visible rows).
fn syntax(path: &str, rows: &[DiffLine], on: bool) -> Vec<Option<Vec<Span<'static>>>> {
    let none = || rows.iter().map(|_| None).collect();
    if !on {
        return none();
    }
    let ext = path.rsplit('.').next().unwrap_or("");
    let Some(lang) = crate::highlight::canonical(ext) else {
        return none();
    };
    let text: Vec<String> = rows.iter().map(|r| r.text.replace('\t', "    ")).collect();
    match crate::highlight::highlight(lang, &text.join("\n")) {
        Some(lines) if lines.len() == rows.len() => lines.into_iter().map(Some).collect(),
        _ => none(),
    }
}

/// Words, runs of spaces, and single punctuation marks.
fn tokens(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
    let mut prev: Option<u8> = None;
    for (i, c) in s.char_indices() {
        let k = class(c);
        if prev.is_some_and(|p| p != k || k == 2) {
            out.push(&s[start..i]);
            start = i;
        }
        prev = Some(k);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Changed character ranges in (the old line, the new line).
type Marks = (Vec<(usize, usize)>, Vec<(usize, usize)>);

/// Character ranges that changed between a removed and an added line.
pub fn word_changes(old: &str, new: &str) -> Option<Marks> {
    use similar::{ChangeTag, TextDiff};
    let (ot, nt) = (tokens(old), tokens(new));
    let d = TextDiff::configure().timeout(Duration::from_millis(5)).diff_slices(&ot, &nt);
    // a rewrite, not an edit — word marks would just be noise
    if d.ratio() < 0.35 {
        return None;
    }
    let (mut o, mut n) = (Vec::new(), Vec::new());
    let (mut oi, mut ni) = (0usize, 0usize);
    for ch in d.iter_all_changes() {
        let len = ch.value().chars().count();
        match ch.tag() {
            ChangeTag::Equal => {
                oi += len;
                ni += len;
            }
            ChangeTag::Delete => {
                if !ch.value().trim().is_empty() {
                    o.push((oi, oi + len));
                }
                oi += len;
            }
            ChangeTag::Insert => {
                if !ch.value().trim().is_empty() {
                    n.push((ni, ni + len));
                }
                ni += len;
            }
        }
    }
    Some((o, n))
}

/// Restyle the characters inside `ranges` (split spans at the edges).
fn overlay(spans: Vec<Span<'static>>, ranges: &[(usize, usize)], f: impl Fn(Style) -> Style) -> Vec<Span<'static>> {
    if ranges.is_empty() {
        return spans;
    }
    let inside = |i: usize| ranges.iter().any(|&(a, b)| i >= a && i < b);
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut idx = 0usize;
    for s in spans {
        let mut buf = String::new();
        let mut cur: Option<bool> = None;
        for c in s.content.chars() {
            let hit = inside(idx);
            if cur.is_some_and(|h| h != hit) {
                let st = if cur == Some(true) { f(s.style) } else { s.style };
                out.push(Span::styled(std::mem::take(&mut buf), st));
            }
            cur = Some(hit);
            buf.push(c);
            idx += 1;
        }
        if !buf.is_empty() {
            let st = if cur == Some(true) { f(s.style) } else { s.style };
            out.push(Span::styled(buf, st));
        }
    }
    out
}

/// For each row, the paired row's index when a `-` run meets a `+` run.
fn pairs(rows: &[DiffLine]) -> Vec<Option<usize>> {
    let mut out = vec![None; rows.len()];
    let mut i = 0;
    while i < rows.len() {
        if rows[i].kind != '-' {
            i += 1;
            continue;
        }
        let del_start = i;
        while i < rows.len() && rows[i].kind == '-' {
            i += 1;
        }
        let add_start = i;
        while i < rows.len() && rows[i].kind == '+' {
            i += 1;
        }
        let n = (add_start - del_start).min(i - add_start);
        for k in 0..n {
            out[del_start + k] = Some(add_start + k);
            out[add_start + k] = Some(del_start + k);
        }
    }
    out
}

/// The styled text of every row: syntax colour, then word-level marks.
fn row_spans(d: &Diff, rows: &[DiffLine], sk: &Skin) -> Vec<Vec<Span<'static>>> {
    let pal = crate::skin::palette();
    let syn = syntax(&d.path, rows, sk.diff.syntax);
    let pair = pairs(rows);
    rows.iter()
        .enumerate()
        .map(|(i, r)| {
            let text = r.text.replace('\t', "    ");
            let base = match r.kind {
                ' ' => DIM(),
                _ => Style::new().fg(pal.text),
            };
            let mut spans = match &syn[i] {
                Some(s) if r.kind != '…' => s.clone(),
                _ => vec![Span::styled(text.clone(), base)],
            };
            if r.kind == ' ' && syn[i].is_some() {
                spans = spans.into_iter().map(|s| Span::styled(s.content, s.style.add_modifier(Modifier::DIM))).collect();
            }
            if sk.diff.word_diff {
                if let Some(j) = pair[i] {
                    let (old, new) = if r.kind == '-' { (&r.text, &rows[j].text) } else { (&rows[j].text, &r.text) };
                    if let Some((o, n)) = word_changes(&old.replace('\t', "    "), &new.replace('\t', "    ")) {
                        let (ranges, bg) = if r.kind == '-' { (o, pal.diff_del_word) } else { (n, pal.diff_add_word) };
                        spans = overlay(spans, &ranges, |s| s.bg(bg));
                    }
                }
            }
            spans
        })
        .collect()
}

fn line_bg(kind: char) -> Option<Style> {
    match kind {
        '+' => Some(Style::new().bg(theme::DIFF_ADD_BG())),
        '-' => Some(Style::new().bg(theme::DIFF_DEL_BG())),
        _ => None,
    }
}

fn sign(kind: char) -> (&'static str, Style) {
    match kind {
        '+' => ("+", theme::fg(GREEN())),
        '-' => ("-", theme::fg(RED())),
        _ => (" ", DIM()),
    }
}

/// Give every span the row background (word marks keep their own).
fn with_bg(spans: Vec<Span<'static>>, bg: Option<Style>) -> Vec<Span<'static>> {
    match bg {
        None => spans,
        Some(b) => spans.into_iter().map(|s| Span::styled(s.content, if s.style.bg.is_some() { s.style } else { s.style.patch(b) })).collect(),
    }
}

/// `⋯ L42` between hunks — where the next one starts.
fn hunk_mark(rows: &[DiffLine], i: usize, indent: &str) -> Line<'static> {
    let next = rows[i + 1..].iter().find_map(|r| r.no);
    let text = if !rows[i].text.is_empty() {
        rows[i].text.clone()
    } else {
        next.map(|n| format!("⋯ L{n}")).unwrap_or_else(|| "⋯".into())
    };
    Line::from(vec![sp(indent.to_string()), Span::styled(text, DIM())])
}

fn unified_rows(d: &Diff, rows: &[DiffLine], w: usize, sk: &Skin) -> Vec<Line<'static>> {
    let nw = rows.iter().filter_map(|l| l.no).max().unwrap_or(1).to_string().len().max(3);
    let texts = row_spans(d, rows, sk);
    let mut out = Vec::new();
    for (i, (r, spans)) in rows.iter().zip(texts).enumerate() {
        if r.kind == '…' {
            out.push(hunk_mark(rows, i, "      "));
            continue;
        }
        let bg = line_bg(r.kind);
        let base = bg.unwrap_or_default();
        let (s, sst) = sign(r.kind);
        let no = r.no.map(|n| format!("{n:>nw$}")).unwrap_or_else(|| " ".repeat(nw));
        let first = vec![sp("      "), Span::styled(format!("{no} "), DIM().patch(base)), Span::styled(format!("{s} "), sst.patch(base))];
        let rest = vec![sp("      "), Span::styled(" ".repeat(nw + 1), base), Span::styled(format!("{} ", sk.glyphs.wrap), DIM().patch(base))];
        let spans = with_bg(spans, bg);
        let mut lines = if sk.diff.wrap {
            crate::wrap::wrap(&spans, w, &first, &rest)
        } else {
            let room = w.saturating_sub(nw + 10);
            let mut l = first;
            l.extend(clip(spans, room));
            vec![Line::from(l)]
        };
        for l in &mut lines {
            if bg.is_some() {
                pad_line(l, w, base);
            }
        }
        out.extend(lines);
    }
    out
}

/// Cut spans to `room` columns, ending in `…` when anything was dropped.
fn clip(spans: Vec<Span<'static>>, room: usize) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(|s| width_of(&s.content)).sum();
    if total <= room {
        return spans;
    }
    let mut out = Vec::new();
    let mut col = 0;
    for s in spans {
        let sw = width_of(&s.content);
        if col + sw < room {
            col += sw;
            out.push(s);
            continue;
        }
        let keep = ellipsize(&s.content, room.saturating_sub(col));
        out.push(Span::styled(keep, s.style));
        break;
    }
    out
}

/// Old on the left, new on the right; changed lines side by side.
fn split_rows(d: &Diff, rows: &[DiffLine], w: usize, sk: &Skin) -> Vec<Line<'static>> {
    let nw = rows.iter().filter_map(|l| l.no.max(l.old)).max().unwrap_or(1).to_string().len().max(3);
    let half = w.saturating_sub(9) / 2;
    let texts = row_spans(d, rows, sk);
    let side = |r: Option<(&DiffLine, &Vec<Span<'static>>)>, no: Option<usize>| -> Vec<Span<'static>> {
        let Some((r, spans)) = r else {
            return vec![sp(" ".repeat(half))];
        };
        let bg = line_bg(r.kind);
        let base = bg.unwrap_or_default();
        let (s, sst) = sign(r.kind);
        let no = no.map(|n| format!("{n:>nw$}")).unwrap_or_else(|| " ".repeat(nw));
        let mut l = vec![Span::styled(format!("{no} "), DIM().patch(base)), Span::styled(format!("{s} "), sst.patch(base))];
        l.extend(clip(with_bg(spans.clone(), bg), half.saturating_sub(nw + 3)));
        let mut line = Line::from(l);
        pad_line(&mut line, half, base);
        line.spans
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let r = &rows[i];
        match r.kind {
            '…' => {
                out.push(hunk_mark(rows, i, "      "));
                i += 1;
            }
            ' ' => {
                let mut l = vec![sp("      ")];
                l.extend(side(Some((r, &texts[i])), r.old));
                l.push(dim(" │ "));
                l.extend(side(Some((r, &texts[i])), r.no));
                out.push(Line::from(l));
                i += 1;
            }
            _ => {
                let ds = i;
                while i < rows.len() && rows[i].kind == '-' {
                    i += 1;
                }
                let as_ = i;
                while i < rows.len() && rows[i].kind == '+' {
                    i += 1;
                }
                let (dels, adds) = (as_ - ds, i - as_);
                for k in 0..dels.max(adds) {
                    let left = (k < dels).then(|| (&rows[ds + k], &texts[ds + k]));
                    let right = (k < adds).then(|| (&rows[as_ + k], &texts[as_ + k]));
                    let mut l = vec![sp("      ")];
                    l.extend(side(left, left.and_then(|(r, _)| r.no)));
                    l.push(dim(" │ "));
                    l.extend(side(right, right.and_then(|(r, _)| r.no)));
                    out.push(Line::from(l));
                }
            }
        }
    }
    out
}

/// Plain text of a diff for the pager (every row, uncapped).
pub fn diff_text(d: &Diff) -> Vec<String> {
    d.lines
        .iter()
        .map(|l| match l.kind {
            '…' if l.text.is_empty() => "⋯".to_string(),
            '…' => l.text.clone(),
            k => format!("{} {k} {}", l.no.map(|n| format!("{n:>4}")).unwrap_or_else(|| "    ".into()), l.text),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skin::{scoped_skin, Skin};
    use serde_json::json;

    fn text(lines: &[Line]) -> String {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect::<Vec<_>>().join("\n")
    }

    fn view<'a>(name: &'a str, input: &'a Value, content: &'a str) -> ToolView<'a> {
        ToolView { name, input, content: Some(content), err: false, diff: None, interrupted: false, elapsed: Some(Duration::from_millis(230)) }
    }

    #[test]
    fn cosmos_headers_carry_the_facts() {
        scoped_skin(Skin::default(), || {
            let i = json!({"path": "src/a.rs"});
            let t = text(&render(&view("read", &i, "a\nb\nc\n"), 80));
            assert_eq!(t, " ◆ Read src/a.rs · 3 lines · 0.2s");
            let i = json!({"pattern": "foo"});
            let t = text(&render(&view("grep", &i, "a.rs:1:foo\nb.rs:2:foo\n"), 80));
            assert!(t.starts_with(" ◆ Search foo · 2 matches · 0.2s\n   ⎿  a.rs:1:foo"), "{t}");
            let i = json!({"command": "false"});
            let out = "exit_code=1 (no matches) log_path=x omitted_chars=0\n<<<UNTRUSTED_SHELL\n\n>>>";
            let t = text(&render(&view("bash", &i, out), 80));
            assert_eq!(t, " ◆ Bash false · exit 1 · no matches · 0.2s");
            let out = "exit_code=0 log_path=x omitted_chars=0\n<<<UNTRUSTED_SHELL\nok 1\nok 2\n>>>";
            let t = text(&render(&view("bash", &i, out), 80));
            assert_eq!(t, " ◆ Bash false · 0.2s\n   ⎿  ok 1\n      ok 2");
        });
    }

    #[test]
    fn long_output_points_at_the_pager() {
        scoped_skin(Skin::default(), || {
            let i = json!({"command": "seq 9"});
            let body: String = (1..=9).map(|n| format!("{n}\n")).collect();
            let t = text(&render(&view("bash", &i, &body), 80));
            assert!(t.ends_with("… +5 lines · ctrl+o to expand"), "{t}");
        });
    }

    #[test]
    fn classic_style_is_the_old_block() {
        scoped_skin(Skin::classic(), || {
            let i = json!({"path": "src/a.rs"});
            let v = view("read", &i, "a\nb\n");
            assert_eq!(text(&render(&v, 80)), text(&crate::view::tool_block("read", &i, Some("a\nb\n"), false, None, 80, false)));
        });
    }

    #[test]
    fn explores_fold_into_a_group() {
        scoped_skin(Skin::default(), || {
            let it = |n: &str, i: Value, c: &str| Explored { name: n.into(), input: i, content: c.into(), elapsed: Some(Duration::from_millis(100)) };
            let items = vec![it("read", json!({"path": "a.rs"}), "x\ny\n"), it("read", json!({"path": "b.rs"}), "x\n"), it("grep", json!({"pattern": "fn"}), "a.rs:1:fn\n")];
            let t = text(&group(&items, 80));
            assert!(t.starts_with(" ◇ Explored 2 files · 1 search · 0.3s"), "{t}");
            assert!(t.contains("⎿  Read a.rs · 2 lines"), "{t}");
            assert!(t.contains("Search fn · 1 match"), "{t}");
            // one call alone keeps its normal look
            assert!(text(&group(&items[..1], 80)).starts_with(" ◆ Read a.rs"));
            assert!(groups(&Skin::default(), "read", false));
            assert!(!groups(&Skin::default(), "read", true));
            assert!(!groups(&Skin::classic(), "read", false));
        });
    }

    fn dl(kind: char, no: usize, old: Option<usize>, text: &str) -> DiffLine {
        DiffLine { kind, no: Some(no), old, text: text.into() }
    }

    #[test]
    fn word_marks_land_on_the_changed_words() {
        let (o, n) = word_changes("let x = 1;", "let x = 2;").unwrap();
        assert_eq!(o, vec![(8, 9)]);
        assert_eq!(n, vec![(8, 9)]);
        assert!(word_changes("abc", "completely different text here").is_none());
        let spans = overlay(vec![Span::raw("let x = 1;")], &[(8, 9)], |s| s.bg(ratatui_core::style::Color::Red));
        assert_eq!(spans.iter().map(|s| s.content.as_ref()).collect::<Vec<_>>(), vec!["let x = ", "1", ";"]);
        assert_eq!(spans[1].style.bg, Some(ratatui_core::style::Color::Red));
    }

    #[test]
    fn cosmos_diff_wraps_marks_hunks_and_caps() {
        scoped_skin(Skin::default(), || {
            let long = "x".repeat(120);
            let d = Diff {
                path: "a.rs".into(),
                added: 2,
                removed: 1,
                created: false,
                lines: vec![
                    dl(' ', 1, Some(1), "fn a() {"),
                    dl('-', 2, Some(2), "    let v = 1;"),
                    dl('+', 2, None, "    let v = 2;"),
                    DiffLine { kind: '…', no: None, old: None, text: String::new() },
                    dl('+', 40, None, &long),
                ],
            };
            let lines = diff(&d, 80);
            let t = text(&lines);
            assert!(t.contains("⋯ L40"), "{t}");
            assert!(t.contains("↳ "), "long lines wrap: {t}");
            assert!(lines.iter().all(|l| crate::wrap::line_width(l) <= 80), "{t}");
            // the changed digit carries the word background
            let p = crate::skin::palette();
            assert!(lines.iter().flat_map(|l| &l.spans).any(|s| s.content == "2" && s.style.bg == Some(p.diff_add_word)), "{t}");
            let mut sk = Skin::default();
            sk.diff.max_rows = 2;
            scoped_skin(sk, || assert!(text(&diff(&d, 80)).ends_with("… 3 more lines · ctrl+o to expand")));
        });
    }

    #[test]
    fn wide_terminals_get_side_by_side() {
        scoped_skin(Skin::default(), || {
            let d = Diff {
                path: "notes.txt".into(),
                added: 1,
                removed: 1,
                created: false,
                lines: vec![dl(' ', 1, Some(1), "same"), dl('-', 2, Some(2), "old line"), dl('+', 2, None, "new line")],
            };
            let lines = diff(&d, 150);
            assert_eq!(lines.len(), 2);
            let t = text(&lines);
            assert!(t.contains("old line") && t.contains("│") && t.contains("new line"), "{t}");
            assert!(lines.iter().all(|l| crate::wrap::line_width(l) <= 150));
            assert_eq!(diff(&d, 100).len(), 3, "narrow stays unified");
        });
    }

    /// Eyeball test — `cargo test -p ztui dump_tools -- --nocapture`.
    #[test]
    fn dump_tools() {
        scoped_skin(Skin::default(), || {
            let mut shown = Vec::new();
            let i = json!({"path": "crates/ztui/src/view.rs"});
            shown.extend(render(&view("read", &i, &"x
".repeat(412)), 96));
            let i = json!({"command": "cargo test -p ztui"});
            let out = "exit_code=101 log_path=x omitted_chars=0
<<<UNTRUSTED_SHELL
running 81 tests
test a ... ok
test b ... FAILED
failures:
  b panicked
test result: FAILED
>>>";
            shown.extend(render(&view("bash", &i, out), 96));
            let it = |n: &str, i: Value, c: &str| Explored { name: n.into(), input: i, content: c.into(), elapsed: Some(Duration::from_millis(120)) };
            shown.extend(group(&[it("read", json!({"path": "src/a.rs"}), "a
b
"), it("grep", json!({"pattern": "fn main"}), "a.rs:1:fn main
"), it("glob", json!({"pattern": "**/*.toml"}), "Cargo.toml
")], 96));
            let d = Diff {
                path: "src/a.rs".into(),
                added: 3,
                removed: 2,
                created: false,
                lines: vec![
                    dl(' ', 10, Some(10), "fn render(v: &View, w: usize) -> Vec<Line> {"),
                    dl('-', 11, Some(11), "    let keep = 4;"),
                    dl('-', 12, Some(12), "    let out = rows(&lines, DIM, w, keep);"),
                    dl('+', 11, None, "    let keep = sk.tools.keep_rows;"),
                    dl('+', 12, None, "    let out = rows(&lines, DIM(), w, keep);"),
                    dl('+', 13, None, "    // a comment long enough that it has to wrap onto a second row when the terminal is narrow"),
                    DiffLine { kind: '…', no: None, old: None, text: String::new() },
                    dl(' ', 40, Some(39), "}"),
                ],
            };
            let iv = json!({"path": "src/a.rs"});
            let v = ToolView { name: "edit", input: &iv, content: Some("ok"), err: false, diff: Some(&d), interrupted: false, elapsed: Some(Duration::from_millis(40)) };
            shown.extend(render(&v, 96));
            println!("{}", text(&shown));
            println!("{}", text(&diff(&d, 150)));
        });
    }

    #[test]
    fn shell_parts_unwrap_the_fence() {
        let (c, n, b) = shell_parts("exit_code=137 (killed (SIGKILL — often out of memory)) log_path=x omitted_chars=0\n<<<UNTRUSTED_SHELL\nboom\n>>>");
        assert_eq!(c.as_deref(), Some("137"));
        assert_eq!(n.as_deref(), Some("killed (SIGKILL — often out of memory"));
        assert_eq!(b, "boom");
        assert_eq!(shell_parts("plain").2, "plain");
    }
}
