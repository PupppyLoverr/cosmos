//! Rendering — laid out after the Devin CLI reference:
//!
//! ```text
//!  ⠀⣀⣤⣤⣥⣥⣄⡀              ← galaxy mark, name + version beside it
//!  …                        ← model · workspace (dim)
//! ❯ Hello what can you do   ← user turn: full-width grey bar
//!  I can help with …        ← reply, one-column margin, • bullets
//! ────────────────────────  ← rule
//! ❯ Ask zcli to build …     ← composer
//! ────────────────────────  ← rule
//! r9 Medium          ? for shortcuts   ← status line
//!   /model …  Interactively choose …   ← menus open *below* the status line
//! ```

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use zcore::agent::Mode;

use crate::markdown;
use crate::slash;
use crate::theme::{self, ACCENT, ACCENT_BOLD, DIM, GHOST, GREEN, RED, RULE, USER_BG, YELLOW};
use crate::tui::{App, ModelRow, Perm, Popup, EFFORTS, PLACEHOLDER};
use crate::worker::Diff;
use crate::wrap::{self, ellipsize, line_width, pad_line, width_of};

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// twinkling star for the working line
const TWINKLE: &[&str] = &["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"];
pub const MENU_ROWS: usize = 6;
const MODEL_ROWS: usize = 9;

fn sp(s: impl Into<String>) -> Span<'static> {
    Span::raw(s.into())
}
fn dim(s: impl Into<String>) -> Span<'static> {
    Span::styled(s.into(), DIM)
}

pub fn kfmt(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 10_000 {
        format!("{}k", n / 1000)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

// ---- transcript blocks -------------------------------------------------------

pub fn note(s: &str, w: usize) -> Vec<Line<'static>> {
    wrap::wrap(&[dim(s.to_string())], w, &[dim(" · ")], &[sp("   ")])
}

pub fn error(s: &str, w: usize) -> Vec<Line<'static>> {
    wrap::wrap(&[Span::styled(s.to_string(), theme::fg(RED))], w, &[Span::styled(" ✗ ", theme::fg(RED))], &[sp("   ")])
}

pub fn warn(s: &str, w: usize) -> Vec<Line<'static>> {
    wrap::wrap(&[Span::styled(s.to_string(), theme::fg(YELLOW))], w, &[Span::styled(" ! ", theme::fg(YELLOW))], &[sp("   ")])
}

fn home_short(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let h = home.to_string_lossy().to_string();
        if !h.is_empty() && s.starts_with(&h) {
            return format!("~{}", &s[h.len()..]);
        }
    }
    s
}

/// The dim line under the banner (Devin shows plan/quota here).
pub fn info_line(app: &App) -> Line<'static> {
    let m = app.models.iter().find(|m| m.alias == app.alias);
    let mut parts = Vec::new();
    if let Some(u) = &app.user {
        parts.push(format!("hey {u}"));
    }
    parts.push(app.model.clone());
    if let Some(m) = m {
        parts.push(format!("{} context", kfmt(m.context)));
    }
    parts.push(home_short(&app.cwd));
    Line::from(Span::styled(parts.join(" · "), DIM))
}

/// A user turn: `❯ text` on a full-width grey bar.
pub fn user(text: &str, w: usize) -> Vec<Line<'static>> {
    let bg = Style::new().bg(USER_BG);
    let mut out = Vec::new();
    for (i, para) in text.split('\n').enumerate() {
        let first = if i == 0 { vec![Span::styled("❯ ", bg)] } else { vec![Span::styled("  ", bg)] };
        let rows = wrap::wrap(&[Span::styled(para.to_string(), bg)], w, &first, &[Span::styled("  ", bg)]);
        out.extend(rows);
    }
    for l in &mut out {
        pad_line(l, w, bg);
    }
    out
}

pub fn plan(text: &str, w: usize) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(vec![Span::styled(" ▍", theme::fg(YELLOW)), Span::styled("Plan", theme::bold(Style::new()))])];
    out.extend(markdown::render(text, w));
    out.push(Line::default());
    out.push(Line::from(dim(" /approve to run it · or keep typing to refine")));
    out
}

pub fn tool_label(name: &str) -> String {
    match name {
        "bash" => "Bash",
        "read" => "Read",
        "edit" => "Edit",
        "write" => "Write",
        "grep" => "Search",
        "glob" => "Glob",
        "ls" => "List",
        "memory_search" => "Recall",
        "memory_write" => "Remember",
        "task" => "Task",
        "submit_plan" => "Plan",
        "tool_search" => "Tools",
        "mcp_call" => "MCP",
        "request_escalation" => "Escalate",
        other => return other.to_string(),
    }
    .to_string()
}

/// A representative argument for the tool header line.
pub fn arg_summary(name: &str, input: &Value, max: usize) -> String {
    let pick = |k: &str| input[k].as_str().map(str::to_string);
    let s = match name {
        "bash" => pick("command"),
        "read" => pick("path").map(|p| match (input["start"].as_u64(), input["end"].as_u64()) {
            (Some(a), Some(b)) => format!("{p}:{a}-{b}"),
            _ => p,
        }),
        "write" | "edit" | "ls" => pick("path"),
        "grep" => pick("pattern").map(|p| match input["path"].as_str() {
            Some(d) => format!("{p}  in {d}"),
            None => p,
        }),
        "glob" => pick("pattern"),
        "memory_search" | "tool_search" => pick("query"),
        "memory_write" => pick("text"),
        "task" => pick("prompt"),
        "request_escalation" => pick("capability"),
        "mcp_call" => Some(format!("{}.{}", input["server"].as_str().unwrap_or("?"), input["tool"].as_str().unwrap_or("?"))),
        _ => input.as_object().and_then(|o| o.values().find_map(|v| v.as_str().map(str::to_string))),
    };
    ellipsize(&s.unwrap_or_default().replace(['\n', '\r'], " "), max)
}

fn head(name: &str, input: &Value, dot: Style, w: usize) -> Line<'static> {
    let label = tool_label(name);
    let room = w.saturating_sub(width_of(&label) + 5);
    let arg = arg_summary(name, input, room);
    let mut spans = vec![sp(" "), Span::styled("● ", dot), Span::styled(label, theme::bold(Style::new()))];
    if !arg.is_empty() {
        spans.push(dim(format!(" {arg}")));
    }
    Line::from(spans)
}

fn result_rows(lines: &[&str], style: Style, w: usize, keep: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (i, l) in lines.iter().take(keep).enumerate() {
        let pre = if i == 0 { "   ⎿  " } else { "      " };
        let room = w.saturating_sub(width_of(pre));
        out.push(Line::from(vec![Span::styled(pre, DIM), Span::styled(ellipsize(l, room), style)]));
    }
    if lines.len() > keep {
        out.push(Line::from(dim(format!("      … +{} lines", lines.len() - keep))));
    }
    out
}

fn diff_rows(d: &Diff, w: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let head = if d.created {
        format!("Created {} ({} lines)", d.path, d.added)
    } else {
        format!("Updated {} with {} addition{} and {} removal{}", d.path, d.added, if d.added == 1 { "" } else { "s" }, d.removed, if d.removed == 1 { "" } else { "s" })
    };
    out.push(Line::from(vec![dim("   ⎿  "), sp(head)]));
    let nw = d.lines.iter().filter_map(|l| l.no).max().unwrap_or(1).to_string().len().max(3);
    for l in &d.lines {
        let (bg, sign, fg) = match l.kind {
            '+' => (Some(theme::DIFF_ADD_BG), "+", theme::fg(GREEN)),
            '-' => (Some(theme::DIFF_DEL_BG), "-", theme::fg(RED)),
            '…' => (None, "…", DIM),
            _ => (None, " ", DIM),
        };
        let no = l.no.map(|n| format!("{n:>nw$}")).unwrap_or_else(|| " ".repeat(nw));
        let base = bg.map(|b| Style::new().bg(b)).unwrap_or_default();
        let room = w.saturating_sub(nw + 10);
        let text = if l.kind == '…' { l.text.clone() } else { ellipsize(&l.text.replace('\t', "    "), room) };
        let mut line = Line::from(vec![
            sp("      "),
            Span::styled(format!("{no} "), DIM.patch(base)),
            Span::styled(format!("{sign} "), fg.patch(base)),
            Span::styled(text, if l.kind == ' ' { DIM.patch(base) } else { base }),
        ]);
        if bg.is_some() {
            pad_line(&mut line, w, base);
        }
        out.push(line);
    }
    out
}

/// A finished (or interrupted) tool call: header + result summary / diff.
pub fn tool_block(name: &str, input: &Value, content: Option<&str>, err: bool, diff: Option<&Diff>, w: usize, interrupted: bool) -> Vec<Line<'static>> {
    let dot = if interrupted {
        DIM
    } else if err {
        theme::fg(RED)
    } else {
        theme::fg(GREEN)
    };
    let mut out = vec![head(name, input, dot, w)];
    if interrupted {
        out.push(Line::from(vec![dim("   ⎿  "), dim("Interrupted")]));
        return out;
    }
    let content = content.unwrap_or("");
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    if err {
        out.extend(result_rows(&lines, theme::fg(RED), w, 4));
        return out;
    }
    if let Some(d) = diff {
        out.extend(diff_rows(d, w));
        return out;
    }
    let summary = match name {
        "read" => Some(format!("Read {} lines", content.lines().count())),
        "grep" | "glob" | "ls" if lines.is_empty() => Some("No matches".into()),
        "memory_write" => Some("Saved to memory".into()),
        "edit" | "write" => lines.first().map(|l| l.to_string()),
        _ => None,
    };
    match summary {
        Some(s) => out.push(Line::from(vec![dim("   ⎿  "), dim(s)])),
        None if lines.is_empty() => out.push(Line::from(vec![dim("   ⎿  "), dim("(no output)")])),
        None => {
            let keep = if matches!(name, "grep" | "glob" | "ls") { 3 } else { 4 };
            out.extend(result_rows(&lines, DIM, w, keep));
        }
    }
    out
}

fn reset_in(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs % 86_400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

/// "Plus · 84% remaining (resets in 4d 3h)" — Devin's plan line, for ChatGPT.
pub fn plan_line(r: &zproviders::RateLimits) -> Option<String> {
    let (used, reset) = match (r.secondary_used, r.primary_used) {
        (Some(s), Some(p)) if s >= p => (s, r.secondary_reset_secs),
        (_, Some(p)) => (p, r.primary_reset_secs),
        (Some(s), None) => (s, r.secondary_reset_secs),
        _ => return None,
    };
    let plan = r.plan.as_deref().map(|p| {
        let mut c = p.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    });
    let mut s = format!("{:.0}% remaining", (100.0 - used).clamp(0.0, 100.0));
    if let Some(p) = plan {
        s = format!("{p} · {s}");
    }
    if let Some(r) = reset {
        s.push_str(&format!(" (resets in {})", reset_in(r)));
    }
    Some(s)
}

pub fn usage(app: &App) -> Vec<Line<'static>> {
    let u = &app.usage;
    let cost = u.cum_usd.map(|c| format!("${c:.4}")).unwrap_or_else(|| "no charge / unpriced".into());
    let mut out = vec![
        Line::from(vec![sp(" "), Span::styled("Usage", theme::bold(Style::new())), dim(format!("  {} turn{}", u.turns, if u.turns == 1 { "" } else { "s" }))]),
        Line::from(vec![dim("   Input     "), sp(kfmt(u.input)), dim(format!("   cache read {} · write {}", kfmt(u.cache_read), kfmt(u.cache_write)))]),
        Line::from(vec![dim("   Output    "), sp(kfmt(u.output))]),
        Line::from(vec![dim("   Cost      "), sp(cost)]),
    ];
    if let Some(p) = app.rate.as_ref().and_then(plan_line) {
        out.push(Line::from(vec![dim("   Plan      "), sp(p)]));
    }
    out
}

pub fn context(app: &App) -> Vec<Line<'static>> {
    let window = app.models.iter().find(|m| m.alias == app.alias).map(|m| m.context).unwrap_or(128_000).max(1);
    let used = app.usage.last_prompt;
    let pct = (used as f64 / window as f64 * 100.0).min(100.0);
    let cells = 24usize;
    let filled = ((pct / 100.0) * cells as f64).round() as usize;
    let color = if pct > 85.0 { RED } else if pct > 60.0 { YELLOW } else { ACCENT };
    vec![
        Line::from(vec![sp(" "), Span::styled("Context", theme::bold(Style::new())), dim(format!("  {}", app.model))]),
        Line::from(vec![
            sp("   "),
            Span::styled("■".repeat(filled), theme::fg(color)),
            Span::styled("■".repeat(cells - filled), RULE),
            sp(format!("  {} / {} tokens", kfmt(used), kfmt(window))),
            dim(format!("  ({pct:.0}%)")),
        ]),
    ]
}

pub fn farewell(app: &App) -> Vec<Line<'static>> {
    if app.session.is_empty() {
        return Vec::new();
    }
    vec![Line::default(), Line::from(vec![dim(" Resume this session with  "), sp(format!("zcli --resume {}", app.session))]), Line::default()]
}

/// The sign-in URL, shown in case the browser didn't open.
pub fn login_url(url: &str, w: usize) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(vec![sp(" "), Span::styled("Sign in with ChatGPT", theme::bold(Style::new())), dim("  — finish in your browser (esc cancels)")])];
    out.extend(wrap::wrap(&[Span::styled(url.to_string(), theme::fg(ACCENT))], w, &[dim("   ")], &[dim("   ")]));
    out
}

/// Output of a `!command` (or /diff): header + the first rows.
pub fn shell_block(cmd: &str, output: &str, code: Option<i32>, w: usize) -> Vec<Line<'static>> {
    let ok = code == Some(0);
    let mut out = vec![Line::from(vec![
        sp(" "),
        Span::styled("! ", theme::fg(theme::ORANGE)),
        Span::styled(ellipsize(cmd, w.saturating_sub(12)), theme::bold(Style::new())),
        if ok { sp("") } else { Span::styled(format!("  exit {}", code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())), theme::fg(RED)) },
    ])];
    let lines: Vec<&str> = output.lines().collect();
    if lines.iter().all(|l| l.trim().is_empty()) {
        out.push(Line::from(vec![dim("   ⎿  "), dim("(no output)")]));
        return out;
    }
    let keep = 14;
    for (i, l) in lines.iter().take(keep).enumerate() {
        let pre = if i == 0 { "   ⎿  " } else { "      " };
        out.push(Line::from(vec![dim(pre), Span::styled(ellipsize(&l.replace('\t', "    "), w.saturating_sub(7)), DIM)]));
    }
    if lines.len() > keep {
        out.push(Line::from(dim(format!("      … +{} lines (sent with your next message)", lines.len() - keep))));
    }
    out
}

// ---- composer -----------------------------------------------------------------

/// Wrap the input buffer at `width` columns. Returns (visual rows with their
/// buffer start index, cursor row, cursor column in chars).
pub fn wrapped_input(input: &[char], cursor: usize, width: usize) -> (Vec<(usize, String)>, usize, usize) {
    let width = width.max(1);
    let mut lines: Vec<(usize, String)> = Vec::new();
    let (mut start, mut cur, mut col) = (0usize, String::new(), 0usize);
    let mut i = 0usize;
    while i <= input.len() {
        let c = if i == input.len() { '\n' } else { input[i] };
        if c == '\n' {
            lines.push((start, std::mem::take(&mut cur)));
            col = 0;
            start = i + 1;
            i += 1;
            continue;
        }
        let w = wrap::cw(c).max(1);
        if col + w > width && col > 0 {
            lines.push((start, std::mem::take(&mut cur)));
            start = i;
            col = 0;
        }
        cur.push(c);
        col += w;
        i += 1;
    }
    let crow = lines.iter().rposition(|(s, _)| *s <= cursor).unwrap_or(0);
    let ccol = cursor - lines[crow].0;
    (lines, crow, ccol)
}

fn rule(w: usize) -> Line<'static> {
    Line::from(Span::styled("─".repeat(w), RULE))
}

/// Left and right halves on one row, padded apart.
fn split_row(left: Vec<Span<'static>>, right: Vec<Span<'static>>, w: usize) -> Line<'static> {
    let lw: usize = left.iter().map(|s| width_of(&s.content)).sum();
    let rw: usize = right.iter().map(|s| width_of(&s.content)).sum();
    let mut spans = left;
    if lw + rw + 2 <= w {
        spans.push(sp(" ".repeat(w - lw - rw)));
        spans.extend(right);
    }
    Line::from(spans)
}

fn effort_label(app: &App) -> Option<&'static str> {
    let ok = app.models.iter().find(|m| m.alias == app.alias).is_some_and(|m| m.effort_ok);
    (ok && app.effort > 0).then(|| EFFORTS[app.effort].0)
}

/// Short model label for the status line: the display name, not the path.
fn model_label(app: &App) -> String {
    if app.alias.is_empty() {
        return "…".into();
    }
    app.models
        .iter()
        .find(|m| m.alias == app.alias)
        .map(|m| m.name.clone())
        .unwrap_or_else(|| app.alias.rsplit('/').next().unwrap_or(&app.alias).to_string())
}

fn status_line(app: &App, w: usize) -> Line<'static> {
    let mut left = vec![sp(model_label(app))];
    if let Some(e) = effort_label(app) {
        left.push(sp(format!(" {e}")));
    }
    match app.mode {
        Mode::Plan => left.push(Span::styled("  plan mode", theme::fg(YELLOW))),
        Mode::Yolo => left.push(Span::styled("  yolo", theme::fg(RED))),
        Mode::Agent => {}
    }
    let first = app.input.first().copied();
    let hint = if matches!(app.popup, Some(Popup::Slash { .. } | Popup::Files { .. })) {
        "tab next · shift+tab prev · ↵ accept · esc close".to_string()
    } else if app.key_entry.is_some() {
        "↵ save key · esc cancel".to_string()
    } else if app.busy {
        "esc to interrupt".to_string()
    } else if app.plan_pending {
        "/approve to run the plan".to_string()
    } else if first == Some('!') {
        "shell mode · output goes with your next message".to_string()
    } else if first == Some('#') {
        "↵ to save to memory".to_string()
    } else if app.input.is_empty() {
        "ctrl+space to dictate".to_string()
    } else {
        "shift+↵ newline · @ to mention a file".to_string()
    };
    split_row(left, vec![dim(hint)], w)
}

fn menu_row(on: bool, label: String, col: usize, desc: String, w: usize) -> Line<'static> {
    let lab = ellipsize(&label, col.saturating_sub(3));
    let gap = col.saturating_sub(2 + width_of(&lab));
    let desc = ellipsize(&desc, w.saturating_sub(col + 1));
    Line::from(vec![
        if on { Span::styled("○ ", theme::fg(ACCENT)) } else { sp("  ") },
        Span::styled(lab, if on { theme::bold(Style::new()) } else { Style::new() }),
        sp(" ".repeat(gap)),
        Span::styled(desc, if on { Style::new() } else { DIM }),
    ])
}

fn slash_rows(app: &App, sel: usize, w: usize) -> Vec<Line<'static>> {
    let token = app.slash_state().map(|(t, _)| t).unwrap_or_default();
    let rows = slash::filter(&app.commands, &token);
    if rows.is_empty() {
        return vec![Line::from(dim("  No matching commands"))];
    }
    let sel = sel.min(rows.len() - 1);
    let top = sel.saturating_sub(MENU_ROWS - 1);
    let aliases: Vec<String> = app.models.iter().map(|m| m.alias.rsplit('/').next().unwrap_or(&m.alias).to_string()).collect();
    let label = |s: &slash::Slash| -> String {
        let hint = if s.name == "model" && !aliases.is_empty() {
            let mut h: Vec<String> = aliases.iter().take(3).map(|a| ellipsize(a, 14)).collect();
            if aliases.len() > 3 {
                h.push("...".into());
            }
            format!("[{}]", h.join("|"))
        } else {
            s.hint.clone()
        };
        if hint.is_empty() { format!("/{}", s.name) } else { format!("/{} {hint}", s.name) }
    };
    let col = rows.iter().map(|s| width_of(&label(s))).max().unwrap_or(0).min(w / 2) + 4;
    let mut out = Vec::new();
    for (i, s) in rows.iter().enumerate().skip(top).take(MENU_ROWS) {
        out.push(menu_row(i == sel, label(s), col, s.desc.clone(), w));
    }
    if rows.len() > top + MENU_ROWS {
        out.push(Line::from(dim("  ↓ more")));
    }
    out
}

fn file_rows(app: &App, sel: usize, w: usize) -> Vec<Line<'static>> {
    if !app.files.ready() {
        return vec![Line::from(dim("  Indexing files…"))];
    }
    let q = crate::files::mention_at(&app.input, app.cursor).map(|(_, q)| q).unwrap_or_default();
    let hits = app.files.search(&q, MENU_ROWS);
    if hits.is_empty() {
        return vec![Line::from(dim("  No matching files"))];
    }
    let sel = sel.min(hits.len() - 1);
    hits.iter()
        .enumerate()
        .map(|(i, p)| {
            let on = i == sel;
            let (dir, name) = match p.trim_end_matches('/').rsplit_once('/') {
                Some((d, n)) => (format!("{d}/"), format!("{n}{}", if p.ends_with('/') { "/" } else { "" })),
                None => (String::new(), p.clone()),
            };
            Line::from(vec![
                if on { Span::styled("○ ", theme::fg(ACCENT)) } else { sp("  ") },
                Span::styled(ellipsize(&name, w / 2), if on { theme::bold(Style::new()) } else { Style::new() }),
                dim(format!("  {}", ellipsize(&dir, w.saturating_sub(width_of(&name) + 6)))),
            ])
        })
        .collect()
}

fn help_rows(w: usize) -> Vec<Line<'static>> {
    const ROWS: &[(&str, &str)] = &[
        ("enter", "send"),
        ("shift+enter · ctrl+j", "newline"),
        ("ctrl+space", "dictate — speak your task"),
        ("@path", "attach a file or folder"),
        ("!command", "run a shell command; output joins your next message"),
        ("#note", "save a note to shared memory"),
        ("shift+tab", "cycle mode: plan → agent → yolo"),
        ("esc · ctrl+c", "interrupt the agent"),
        ("ctrl+c ctrl+c · ctrl+d", "quit"),
        ("↑ ↓", "prompt history"),
        ("ctrl+w · ctrl+u · ctrl+k", "delete word · to start · to end"),
        ("ctrl+l", "clear the screen"),
        ("/", "commands — /model /connect /login /resume /review …"),
    ];
    let kw = ROWS.iter().map(|(k, _)| width_of(k)).max().unwrap_or(0);
    let mut out: Vec<Line<'static>> = ROWS
        .iter()
        .map(|(k, d)| Line::from(vec![sp(format!("  {k:<kw$}   ")), dim(ellipsize(d, w.saturating_sub(kw + 6)))]))
        .collect();
    out.push(Line::from(dim("  esc close")));
    out
}

/// Fuzzy-ranked model order for the picker.
pub fn model_order(models: &[ModelRow], q: &str) -> Vec<usize> {
    slash::rank(models, q, |m| format!("{} {} {}", m.alias, m.name, m.provider))
}

/// Fuzzy-ranked provider order for /connect (indices into the catalog).
pub fn connect_order(q: &str) -> Vec<usize> {
    let ps = zcore::catalog::providers();
    let ids: Vec<usize> = (0..ps.len()).filter(|&i| ps[i].is_subscription() || !ps[i].env.is_empty()).collect();
    slash::rank(&ids, q, |&i| format!("{} {}", ps[i].id, ps[i].name)).into_iter().map(|k| ids[k]).collect()
}

fn bars(level: usize, on: bool) -> Vec<Span<'static>> {
    let n = EFFORTS.len() - 1;
    let lit = if on { ACCENT_BOLD } else { Style::new() };
    vec![Span::styled("■".repeat(level), lit), Span::styled("■".repeat(n - level), RULE)]
}

/// Search box shared by the pickers: `/ Type to search` between rules.
fn search_box(q: &str, placeholder: &str, w: usize) -> (Vec<Line<'static>>, (u16, u16)) {
    let search = if q.is_empty() { vec![dim("/ "), Span::styled(placeholder.to_string(), GHOST)] } else { vec![dim("/ "), sp(q.to_string())] };
    (vec![rule(w), Line::from(search), rule(w)], (1, 2 + width_of(q) as u16))
}

fn model_picker(app: &App, filter: &[char], sel: usize, effort: usize, w: usize) -> (Vec<Line<'static>>, (u16, u16)) {
    let q: String = filter.iter().collect();
    let order = model_order(&app.models, &q);
    let (mut out, cursor) = search_box(&q, "Type to search", w);

    let total = order.len() + 1; // + connect row
    let sel = sel.min(total - 1);
    let top = sel.saturating_sub(MODEL_ROWS - 1);
    let nw = app.models.iter().map(|m| width_of(&m.name)).max().unwrap_or(8).clamp(10, 26) + 2;
    for (vi, &mi) in order.iter().enumerate().skip(top).take(MODEL_ROWS) {
        let m = &app.models[mi];
        let on = vi == sel;
        let mut spans = vec![if on { Span::styled("❯ ", ACCENT_BOLD) } else { dim("· ") }];
        let name = ellipsize(&m.name, nw - 1);
        let pad = nw - width_of(&name);
        spans.push(Span::styled(name, if on { ACCENT_BOLD } else { Style::new() }));
        spans.push(sp(" ".repeat(pad)));
        spans.push(if m.alias == app.alias { Span::styled("✱ ", theme::fg(GREEN)) } else { sp("  ") });
        if m.effort_ok {
            spans.push(if on { Span::styled("← ", theme::fg(ACCENT)) } else { sp("  ") });
            spans.extend(bars(if on { effort } else { app.effort }, on));
            spans.push(if on { Span::styled(" → ", theme::fg(ACCENT)) } else { sp("   ") });
            let lvl = EFFORTS[if on { effort } else { app.effort }].0;
            spans.push(Span::styled(format!("{lvl:<8}"), if on { theme::fg(ACCENT) } else { DIM }));
        } else {
            spans.push(sp(" ".repeat(2 + EFFORTS.len() - 1 + 3 + 8)));
        }
        spans.push(dim(format!("  {} · {}", ellipsize(&m.provider, 24), kfmt(m.context))));
        let mut line = Line::from(spans);
        if line_width(&line) > w {
            line = Line::from(line.spans.into_iter().take(6).collect::<Vec<_>>());
        }
        out.push(line);
    }
    // last row: add a provider
    if order.len() >= top && order.len() < top + MODEL_ROWS {
        let on = sel == order.len();
        out.push(Line::from(vec![
            if on { Span::styled("❯ ", ACCENT_BOLD) } else { dim("+ ") },
            Span::styled("Connect a provider…", if on { ACCENT_BOLD } else { DIM }),
            dim("  API key or ChatGPT sign-in"),
        ]));
    }
    if total > top + MODEL_ROWS {
        out.push(Line::from(dim("  ↓ more below")));
    }
    out.push(Line::default());
    if let Some(m) = order.get(sel).map(|&i| &app.models[i]) {
        let mut detail = if m.free() {
            let what = if m.alias.starts_with("chatgpt/") { " · included in your ChatGPT plan" } else { " · no quota consumed" };
            vec![sp("  "), Span::styled(" FREE ", Style::new().fg(ratatui::style::Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)), dim(what)]
        } else {
            vec![sp("  "), sp(format!("${:.2} in · ${:.2} out", m.price.input, m.price.output)), dim(" per 1M tokens")]
        };
        detail.push(dim(format!("  ·  {} · {}", m.provider, ellipsize(&m.id, w.saturating_sub(56)))));
        out.push(Line::from(detail));
    } else {
        out.push(Line::from(dim("  30 providers: OpenAI, Anthropic, Google, xAI, Meta, MiMo, Qwen, Kimi, GLM, MiniMax, DeepSeek, Mistral…")));
    }
    out.push(Line::default());
    out.push(Line::from(vec![sp("  "), Span::styled("✱ Current", theme::fg(GREEN)), sp("   "), Span::styled("■ Effort", theme::fg(ACCENT)), sp("   "), Span::styled("+ Connect", DIM)]));
    out.push(Line::default());
    out.push(Line::from(dim("↑↓ select · ↔ reasoning effort · ↵ confirm · esc cancel")));
    (out, cursor)
}

fn connect_picker(filter: &[char], sel: usize, w: usize) -> (Vec<Line<'static>>, (u16, u16)) {
    let q: String = filter.iter().collect();
    let order = connect_order(&q);
    let (mut out, cursor) = search_box(&q, "Search providers", w);
    let ps = zcore::catalog::providers();
    if order.is_empty() {
        out.push(Line::from(dim("  No providers match")));
    }
    let sel = sel.min(order.len().saturating_sub(1));
    let top = sel.saturating_sub(MODEL_ROWS - 1);
    for (vi, &i) in order.iter().enumerate().skip(top).take(MODEL_ROWS) {
        let p = &ps[i];
        let on = vi == sel;
        let status = if p.is_subscription() {
            match zcore::chatgpt::load() {
                Some(t) => Span::styled(format!("✓ signed in{}", t.email.map(|e| format!(" as {e}")).unwrap_or_default()), theme::fg(GREEN)),
                None => dim("sign in"),
            }
        } else {
            match zcore::creds::key_for(&p.id, &p.env) {
                Some((k, src)) => Span::styled(format!("✓ {} · {}", zcore::creds::mask(&k), src.label()), theme::fg(GREEN)),
                None => dim(format!("{} models", p.models.len())),
            }
        };
        out.push(Line::from(vec![
            if on { Span::styled("❯ ", ACCENT_BOLD) } else { dim("· ") },
            Span::styled(format!("{:<30}", ellipsize(&p.name, 29)), if on { ACCENT_BOLD } else { Style::new() }),
            status,
        ]));
    }
    if order.len() > top + MODEL_ROWS {
        out.push(Line::from(dim("  ↓ more below")));
    }
    out.push(Line::default());
    if let Some(p) = order.get(sel).map(|&i| &ps[i]) {
        let how = if p.is_subscription() {
            "Opens your browser — use your ChatGPT Plus/Pro plan instead of an API key".to_string()
        } else {
            format!("Paste a key, or set {} · {}", p.env.join(" / "), p.base_url)
        };
        out.push(Line::from(dim(format!("  {}", ellipsize(&how, w.saturating_sub(3))))));
    }
    out.push(Line::default());
    out.push(Line::from(dim("↑↓ select · ↵ connect · esc cancel")));
    (out, cursor)
}

fn resume_picker(items: &[(String, String, String)], sel: usize, w: usize) -> Vec<Line<'static>> {
    let mut out = vec![rule(w), Line::from(vec![sp(" "), Span::styled("Resume a session", theme::bold(Style::new()))]), rule(w)];
    let sel = sel.min(items.len().saturating_sub(1));
    let top = sel.saturating_sub(MODEL_ROWS - 1);
    for (i, (_, preview, started)) in items.iter().enumerate().skip(top).take(MODEL_ROWS) {
        let on = i == sel;
        let when = started.get(..16).unwrap_or(started).replace('T', " ");
        out.push(Line::from(vec![
            if on { Span::styled("❯ ", ACCENT_BOLD) } else { dim("· ") },
            Span::styled(ellipsize(&preview.replace('\n', " "), w.saturating_sub(24)), if on { ACCENT_BOLD } else { Style::new() }),
            dim(format!("  {when}")),
        ]));
    }
    if items.len() > top + MODEL_ROWS {
        out.push(Line::from(dim("  ↓ more below")));
    }
    out.push(Line::default());
    out.push(Line::from(dim("↑↓ select · ↵ resume · esc cancel")));
    out
}

fn perm_prompt(p: &Perm, w: usize) -> Vec<Line<'static>> {
    let label = tool_label(&p.tool);
    let mut out = vec![rule(w)];
    let mut head = vec![Span::styled(" ! ", theme::fg(YELLOW)), Span::styled(format!("Allow {label}?"), theme::bold(Style::new()))];
    if !p.detail.is_empty() {
        head.push(dim(format!("  {}", ellipsize(&p.detail, w.saturating_sub(width_of(&label) + 12)))));
    }
    out.push(Line::from(head));
    out.push(rule(w));
    let opts = [
        "Yes".to_string(),
        format!("Yes, and don't ask again for {label} this session"),
        "No, tell zcli what to do instead".to_string(),
    ];
    for (i, o) in opts.iter().enumerate() {
        let on = i == p.sel;
        out.push(Line::from(vec![
            if on { Span::styled("❯ ", ACCENT_BOLD) } else { sp("  ") },
            dim(format!("{}. ", i + 1)),
            Span::styled(o.clone(), if on { ACCENT_BOLD } else { Style::new() }),
        ]));
    }
    out.push(Line::default());
    out.push(Line::from(dim("↑↓ select · ↵ confirm · y / a / n · esc deny")));
    out
}

/// Dictation row: `● Listening ▁▃▅▇▅▃ 0:04  ↵ done · esc cancel`.
fn recorder_row(app: &App, tick: usize) -> Line<'static> {
    let Some(rec) = &app.rec else { return Line::default() };
    if rec.transcribing {
        return Line::from(vec![
            Span::styled(format!("{} ", SPINNER[tick % SPINNER.len()]), theme::fg(ACCENT)),
            Span::styled(format!("Transcribing with {}…", rec.backend.label()), theme::fg(ACCENT)),
        ]);
    }
    const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let meter: String = rec.meter.iter().map(|v| LEVELS[((v * 7.0).round() as usize).min(7)]).collect();
    let secs = rec.started.elapsed().as_secs();
    let blink = (rec.started.elapsed().as_millis() / 500) % 2 == 0;
    Line::from(vec![
        Span::styled(if blink { "● " } else { "○ " }, theme::fg(RED)),
        Span::styled("Listening ", theme::bold(Style::new())),
        Span::styled(format!("{meter:<24}"), theme::fg(ACCENT)),
        dim(format!(" {}:{:02}   ↵ done · esc cancel", secs / 60, secs % 60)),
    ])
}

/// Everything still in motion, plus the composer and whatever menu is open.
pub fn live(app: &App) -> (Vec<Line<'static>>, Option<(u16, u16)>) {
    let w = app.width;
    let tick = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) / 80) as usize;
    let mut out: Vec<Line<'static>> = Vec::new();

    // streaming tail — the line still being written
    if let Some(st) = &app.stream {
        let mut tail = st.md.preview(&st.partial, w);
        if tail.len() > 6 {
            tail.drain(..tail.len() - 6);
        }
        if !tail.is_empty() && !app.last_out_blank() {
            out.push(Line::default());
        }
        out.extend(tail);
    }
    // tools in flight
    for t in &app.tools {
        let mut l = head(&t.name, &t.input, Style::new(), w);
        l.spans[1] = Span::styled(format!("{} ", SPINNER[tick % SPINNER.len()]), theme::fg(ACCENT));
        if out.is_empty() && !app.last_out_blank() {
            out.push(Line::default());
        }
        out.push(l);
    }
    if let Some(job) = &app.shell {
        if out.is_empty() && !app.last_out_blank() {
            out.push(Line::default());
        }
        out.push(Line::from(vec![
            sp(" "),
            Span::styled(format!("{} ", SPINNER[tick % SPINNER.len()]), theme::fg(theme::ORANGE)),
            Span::styled(format!("! {}", ellipsize(&job.cmd, w.saturating_sub(8))), theme::bold(Style::new())),
        ]));
    }
    if app.busy && app.perm.is_none() {
        let secs = app.busy_since.elapsed().as_secs();
        let verb = if let Some(t) = app.tools.first() {
            format!("Running {}", tool_label(&t.name).to_lowercase())
        } else if app.stream.is_some() {
            "Writing".into()
        } else {
            "Thinking".into()
        };
        let mut meta = format!("{secs}s");
        if let Some(st) = &app.stream {
            meta.push_str(&format!(" · ↓ {} tokens", kfmt((st.chars / 4) as u64)));
        }
        out.push(Line::default());
        out.push(Line::from(vec![
            sp(" "),
            Span::styled(format!("{} ", TWINKLE[tick % TWINKLE.len()]), theme::fg(ACCENT)),
            Span::styled(format!("{verb}…"), Style::new().fg(ACCENT)),
            dim(format!(" ({meta} · esc to interrupt)")),
        ]));
    }
    if !out.is_empty() {
        out.push(Line::default());
    }

    if let Some(p) = &app.perm {
        out.extend(perm_prompt(p, w));
        return (out, None);
    }
    match &app.popup {
        Some(Popup::Model { filter, sel, effort }) => {
            // keep the command that opened the picker visible above it (Devin)
            out.push(rule(w));
            out.push(Line::from(vec![sp("❯ "), dim("/model")]));
            let base = out.len() as u16;
            let (rows, (r, c)) = model_picker(app, filter, *sel, *effort, w);
            out.extend(rows);
            return (out, Some((base + r, c)));
        }
        Some(Popup::Connect { filter, sel }) => {
            out.push(rule(w));
            out.push(Line::from(vec![sp("❯ "), dim("/connect")]));
            let base = out.len() as u16;
            let (rows, (r, c)) = connect_picker(filter, *sel, w);
            out.extend(rows);
            return (out, Some((base + r, c)));
        }
        Some(Popup::Resume { items, sel }) => {
            out.extend(resume_picker(items, *sel, w));
            return (out, None);
        }
        _ => {}
    }

    // composer between two rules
    out.push(rule(w));
    let tw = w.saturating_sub(2);
    let masked: Vec<char>;
    let shown: &[char] = if app.key_entry.is_some() {
        masked = vec!['•'; app.input.len()];
        &masked
    } else {
        &app.input
    };
    let (vis, crow, ccol) = wrapped_input(shown, app.cursor, tw);
    let input_top = out.len();
    const MAX_INPUT: usize = 10;
    let skip = (crow + 1).saturating_sub(MAX_INPUT);
    let (glyph, glyph_style) = match (app.key_entry.is_some(), app.input.first()) {
        (true, _) => ("⚿ ", theme::fg(ACCENT)),
        (_, Some('!')) => ("! ", theme::fg(theme::ORANGE)),
        (_, Some('#')) => ("# ", theme::fg(ACCENT)),
        _ => ("❯ ", Style::new()),
    };
    let mut cursor = None;
    if app.rec.is_some() {
        out.push(recorder_row(app, tick));
    } else if app.input.is_empty() {
        let ph = match &app.key_entry {
            Some(k) => format!("Paste your {} API key", k.name),
            None => PLACEHOLDER.to_string(),
        };
        out.push(Line::from(vec![Span::styled(glyph, glyph_style), Span::styled(ph, GHOST)]));
        cursor = Some((input_top as u16, 2));
    } else {
        let ghost = match (&app.popup, app.slash_state()) {
            (Some(Popup::Slash { sel }), Some((tok, args))) if args.is_empty() && app.cursor == app.input.len() => slash::filter(&app.commands, &tok)
                .get(*sel)
                .and_then(|s| s.name.strip_prefix(tok.as_str()))
                .map(str::to_string),
            _ => None,
        };
        for (i, (_, text)) in vis.iter().enumerate().skip(skip).take(MAX_INPUT) {
            let mut spans = vec![if i == 0 { Span::styled(glyph, glyph_style) } else { sp("  ") }];
            // the mode character itself is shown as the glyph, not repeated
            let body = if i == 0 && !glyph.starts_with('❯') && app.key_entry.is_none() { text.chars().skip(1).collect() } else { text.clone() };
            spans.push(sp(body));
            if i + 1 == vis.len() {
                if let Some(g) = &ghost {
                    spans.push(Span::styled(g.clone(), GHOST));
                }
            }
            out.push(Line::from(spans));
        }
        let hide_first = crow == 0 && !glyph.starts_with('❯') && app.key_entry.is_none();
        let col: usize = vis
            .get(crow)
            .map(|(_, t)| t.chars().take(ccol).skip(usize::from(hide_first)).map(|c| wrap::cw(c).max(1)).sum())
            .unwrap_or(0);
        cursor = Some(((input_top + crow - skip) as u16, (2 + col) as u16));
    }
    out.push(rule(w));
    out.push(status_line(app, w));

    match &app.popup {
        Some(Popup::Slash { sel }) => out.extend(slash_rows(app, *sel, w)),
        Some(Popup::Files { sel }) => out.extend(file_rows(app, *sel, w)),
        Some(Popup::Help) => out.extend(help_rows(w)),
        _ => {}
    }
    if let Some((msg, color, _)) = &app.notice {
        out.push(Line::from(Span::styled(msg.clone(), theme::fg(*color))));
    } else if let Some(p) = app.rate.as_ref().and_then(plan_line) {
        out.push(Line::from(dim(p)));
    }
    (out, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::DiffLine;

    fn text(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn user_bar_is_full_width() {
        let l = user("hello what can you do", 40);
        assert_eq!(text(&l[0]).chars().count(), 40);
        assert!(text(&l[0]).starts_with("❯ hello"));
    }

    #[test]
    fn diff_block_shows_counts_and_signs() {
        let d = Diff {
            path: "src/a.rs".into(),
            added: 1,
            removed: 1,
            created: false,
            lines: vec![
                DiffLine { kind: '-', no: Some(3), text: "old".into() },
                DiffLine { kind: '+', no: Some(3), text: "new".into() },
            ],
        };
        let rows: Vec<String> = tool_block("edit", &serde_json::json!({"path": "src/a.rs"}), Some("ok"), false, Some(&d), 60, false)
            .iter()
            .map(text)
            .collect();
        assert!(rows[0].contains("Edit") && rows[0].contains("src/a.rs"));
        assert!(rows[1].contains("1 addition and 1 removal"));
        assert!(rows[2].contains("- old") && rows[3].contains("+ new"));
    }

    #[test]
    fn long_output_is_summarised() {
        let out = (0..20).map(|i| format!("row {i}")).collect::<Vec<_>>().join("\n");
        let rows: Vec<String> = tool_block("bash", &serde_json::json!({"command": "ls"}), Some(&out), false, None, 60, false).iter().map(text).collect();
        assert!(rows.iter().any(|r| r.contains("+16 lines")));
    }

    /// Eyeball test — `cargo test -p ztui dump_screens -- --nocapture`
    /// prints the three reference layouts (main, `/` menu, model picker).
    #[test]
    fn dump_screens() {
        let _env = crate::env_lock();
        use crate::tui::ModelRow;
        std::env::set_var("COSMOS_CLI_HOME", std::env::temp_dir().join("zcli-view-test"));
        let _ = std::fs::create_dir_all(std::env::temp_dir().join("zcli-view-test"));
        let w = 96;
        let free = zproto::Prices::default();
        let paid = zproto::Prices { input: 3.0, cache_read: 0.3, cache_write: 3.75, output: 15.0 };
        let m = |a: &str, p: &str, id: &str, c: u64, price: zproto::Prices, e: bool| ModelRow {
            alias: a.into(), provider: p.into(), id: id.into(), name: a.into(), context: c, price, effort_ok: e,
        };
        let mut app = App::new(
            std::path::Path::new(r"C:\Users\ravth\zeron"),
            vec![
                m("r9", "router9", "kr/claude-sonnet-4.5", 200_000, free, true),
                m("r9free", "router9", "kc/kilo-auto/free", 200_000, free, true),
                m("r9gem", "router9", "ag/gemini-3.8-flash", 1_000_000, free, true),
                m("sonnet", "anthropic", "claude-sonnet-5", 200_000, paid, false),
                m("fixture", "fixture", "fixture", 32_768, free, false),
            ],
            w,
        );
        app.alias = "r9".into();
        app.model = "r9 (kr/claude-sonnet-4.5)".into();
        app.session = "01k5zq8m3v".into();
        app.effort = 2;

        let mut scroll: Vec<Line<'static>> = crate::logo::banner("zcli", "v0.1.0");
        scroll.push(Line::default());
        scroll.push(info_line(&app));
        scroll.push(Line::default());
        scroll.extend(user("Hello what can you do", w));
        scroll.push(Line::default());
        scroll.extend(markdown::render(
            "I can help with software engineering tasks end-to-end, for example:\n\n- Explore a codebase and explain how it works\n- Fix bugs, refactor code, add features, or write tests\n- Run commands, builds, tests, and inspect **failures**\n\nIf you have a project, tell me the path and I'll start with `ls`.",
            w,
        ));
        scroll.push(Line::default());
        scroll.extend(tool_block("bash", &serde_json::json!({"command": "cargo test --workspace"}), Some("running 32 tests\ntest result: ok. 32 passed"), false, None, w, false));
        let print = |title: &str, lines: &[Line<'static>]| {
            println!("──── {title} ────");
            for l in lines {
                println!("{}", text(l));
            }
        };
        let (live_main, _) = live(&app);
        print("main", &[scroll.clone(), vec![Line::default()], live_main].concat());

        app.input = "/".chars().collect();
        app.cursor = 1;
        app.popup = Some(Popup::Slash { sel: 0 });
        let (live_slash, _) = live(&app);
        print("slash menu", &live_slash);

        app.input.clear();
        app.cursor = 0;
        app.popup = Some(Popup::Model { filter: vec![], sel: 0, effort: 3 });
        let (live_model, _) = live(&app);
        print("model picker", &live_model);
    }

    fn test_app() -> App {
        use crate::tui::ModelRow;
        let row = |a: &str| ModelRow { alias: a.into(), provider: "p".into(), id: a.into(), name: a.into(), context: 1000, price: zproto::Prices::default(), effort_ok: true };
        let mut app = App::new(std::path::Path::new("."), vec![row("alpha"), row("beta")], 80);
        app.alias = "alpha".into();
        app
    }

    #[test]
    fn pickers_put_the_cursor_in_the_search_box() {
        let _env = crate::env_lock();
        std::env::set_var("COSMOS_CLI_KEYRING", "0");
        let mut app = test_app();
        app.popup = Some(Popup::Model { filter: "be".chars().collect(), sel: 0, effort: 0 });
        let (lines, cur) = live(&app);
        let (r, c) = cur.unwrap();
        assert_eq!(text(&lines[r as usize]), "/ be");
        assert_eq!(c, 4);
        assert!(text(&lines[r as usize - 1]).starts_with('─') && text(&lines[r as usize - 2]) == "❯ /model");
        app.popup = Some(Popup::Connect { filter: vec![], sel: 0 });
        let (lines, cur) = live(&app);
        assert!(text(&lines[cur.unwrap().0 as usize]).starts_with("/ Search providers"));
    }

    #[test]
    fn shell_mode_shows_bang_glyph_once() {
        let mut app = test_app();
        app.input = "!ls -la".chars().collect();
        app.cursor = app.input.len();
        let (lines, cur) = live(&app);
        let (r, c) = cur.unwrap();
        assert_eq!(text(&lines[r as usize]), "! ls -la");
        assert_eq!(c, 2 + 6, "cursor after 'ls -la'");
    }

    #[test]
    fn plan_line_reads_like_devin() {
        let r = zproviders::RateLimits { plan: Some("plus".into()), primary_used: Some(16.0), primary_reset_secs: Some(4 * 86_400 + 3 * 3600 + 5), ..Default::default() };
        assert_eq!(plan_line(&r).unwrap(), "Plus · 84% remaining (resets in 4d 3h)");
    }

    #[test]
    fn input_wrap_cursor_math() {
        let input: Vec<char> = "abcd".chars().collect();
        let (lines, crow, ccol) = wrapped_input(&input, 2, 2);
        assert_eq!(lines.len(), 2);
        assert_eq!((crow, ccol), (1, 0));
        let (_, crow, ccol) = wrapped_input(&input, 4, 2);
        assert_eq!((crow, ccol), (1, 2));
    }
}
