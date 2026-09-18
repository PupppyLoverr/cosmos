//! The model-facing digest — turn a raw `elements[]` dump into compact,
//! human-level UI state. A 336-node Electron tree is mostly `Group`/`Text`
//! noise; what an agent needs is the labeled, actionable surface plus enough
//! geometry to aim pixels when semantics run out.
//!
//! Output contract (documented in the tool descriptions):
//! - `E{n}` is the driver's `element_index` for THIS snapshot; pass it back as
//!   `elementIndex` together with `snapshotId`/`windowId`, or use
//!   `elementToken` = `"{snapshotId}:{index}"`. Any newer snapshot of the same
//!   window invalidates every ref.
//! - Lines are one element each, depth-indented two spaces per level (capped),
//!   with state flags and a geometry tail. Offscreen elements have no frame.

use crate::backend::{ElementFrame, WindowElement, WindowState};

/// Elements shown per digest before the overflow note kicks in. Sized so a
/// busy window stays well under a page of model input.
pub const DIGEST_ELEMENT_CAP: usize = 140;
const MAX_DEPTH_INDENT: u32 = 8;
const LABEL_MAX: usize = 72;
const VALUE_MAX: usize = 96;

/// Roles that always survive pruning — the actionable/structural surface a
/// human would point at. Everything else needs a label, value, or actions to
/// earn a line.
const KEEP_ROLES: &[&str] = &[
    "button",
    "checkbox",
    "radiobutton",
    "edit",
    "textbox",
    "searchbox",
    "combobox",
    "spinner",
    "slider",
    "list",
    "listitem",
    "tree",
    "treeitem",
    "menu",
    "menubar",
    "menuitem",
    "tab",
    "tabitem",
    "table",
    "datagrid",
    "grid",
    "cell",
    "row",
    "scrollbar",
    "thumb",
    "toolbar",
    "statusbar",
    "titlebar",
    "dialog",
    "alert",
    "window",
    "hyperlink",
    "link",
    "progressbar",
    "switch",
    "togglebutton",
    "splitbutton",
    "separator",
    "tooltip",
    "calendar",
    "document",
];

/// Roles pruned unless they carry signal (label/value/actions/flags) —
/// generic containers and free text dominate real trees.
const PRUNE_ROLES: &[&str] = &[
    "group", "pane", "panel", "text", "static", "image", "graphic",
];

fn role_keeps(role: &str) -> bool {
    KEEP_ROLES.contains(&role.to_ascii_lowercase().as_str())
}

fn role_prunes(role: &str) -> bool {
    PRUNE_ROLES.contains(&role.to_ascii_lowercase().as_str())
}

/// Does this element carry enough signal to show a user-facing line?
fn element_significant(el: &WindowElement) -> bool {
    if el.focused == Some(true) || el.scrollable == Some(true) {
        return true;
    }
    if !el.actions.is_empty()
        || el.value.is_some()
        || el.selected == Some(true)
        || el.checked.is_some()
        || el.expanded.is_some()
    {
        return true;
    }
    if role_keeps(&el.role) {
        // Structural roles still need something to say — an unnamed empty
        // row of a grid is noise.
        return el.label.is_some()
            || !matches!(
                el.role.to_ascii_lowercase().as_str(),
                "row" | "cell" | "separator"
            );
    }
    !role_prunes(&el.role) && el.label.is_some()
}

fn clip(text: &str, max: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let flat = flat.trim();
    if flat.chars().count() <= max {
        return flat.to_string();
    }
    let mut out: String = flat.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn frame_tail(frame: Option<&ElementFrame>) -> String {
    match frame {
        Some(f) => format!(
            " @{},{} {}x{}",
            f.x as i64, f.y as i64, f.w as i64, f.h as i64
        ),
        None => String::new(),
    }
}

/// `sel chk exp focus scroll web !en` — only set flags, space-separated.
fn flags_tail(el: &WindowElement) -> String {
    let mut flags = Vec::new();
    if el.selected == Some(true) {
        flags.push("sel");
    }
    if let Some(checked) = el.checked {
        flags.push(if checked { "chk" } else { "unchk" });
    }
    if let Some(expanded) = el.expanded {
        flags.push(if expanded { "exp" } else { "coll" });
    }
    if el.focused == Some(true) {
        flags.push("focus");
    }
    if el.scrollable == Some(true) {
        flags.push("scroll");
    }
    if el.in_web_content == Some(true) {
        flags.push("web");
    }
    if !el.enabled {
        flags.push("disabled");
    }
    if flags.is_empty() {
        String::new()
    } else {
        format!(" [{}]", flags.join(" "))
    }
}

fn actions_tail(el: &WindowElement) -> String {
    if el.actions.is_empty() {
        return String::new();
    }
    format!(" act:{}", el.actions.join(","))
}

/// One element → one digest line. `E{index}` is the action ref.
fn element_line(el: &WindowElement) -> String {
    let indent = "  ".repeat(el.depth.min(MAX_DEPTH_INDENT) as usize);
    let mut line = format!("{indent}E{} {}", el.index, el.role);
    if let Some(label) = &el.label {
        line.push_str(&format!(" \"{}\"", clip(label, LABEL_MAX)));
    }
    if let Some(value) = &el.value {
        line.push_str(&format!(" = \"{}\"", clip(value, VALUE_MAX)));
    }
    line.push_str(&flags_tail(el));
    line.push_str(&actions_tail(el));
    line.push_str(&frame_tail(el.frame.as_ref()));
    line
}

/// Elements for the digest — significant ones in tree order, capped.
pub fn visible_elements(state: &WindowState) -> (Vec<&WindowElement>, usize) {
    let significant: Vec<&WindowElement> = state
        .elements
        .iter()
        .filter(|e| element_significant(e))
        .collect();
    let total = significant.len();
    (
        significant.into_iter().take(DIGEST_ELEMENT_CAP).collect(),
        total,
    )
}

/// The compact observation — what `computer_observe` returns by default.
pub fn render(state: &WindowState) -> String {
    let (shown, significant_total) = visible_elements(state);
    let mut out = String::with_capacity(4 * 1024);
    let app = state.app_name.as_deref().unwrap_or("app");
    let title = state.window_title.as_deref().unwrap_or("");
    out.push_str(&format!(
        "window \"{}\" — {} (pid {}, hwnd {}, snapshot {})\n",
        clip(title, 100),
        app,
        state.pid,
        state.window_id,
        state.snapshot_id
    ));
    if let Some(b) = &state.window_bounds {
        out.push_str(&format!(
            "bounds {},{} {}x{} — ",
            b.x as i64, b.y as i64, b.w as i64, b.h as i64
        ));
    }
    out.push_str(&format!(
        "{} elements ({} shown{})\n",
        state.total_element_count.max(state.returned_element_count),
        shown.len(),
        if significant_total > shown.len() {
            format!(
                ", {} more via computer_find",
                significant_total - shown.len()
            )
        } else {
            String::new()
        }
    ));
    // Driver-reported caveats (degradation, Chromium compositing gaps,
    // truncation) change how much the element set can be trusted.
    if let Some(note) = &state.note {
        out.push_str(&format!("note: {}\n", clip(note, 140)));
    }
    // Focused element + scroll containers are the two most useful landmarks.
    if let Some(focus) = state.elements.iter().find(|e| e.focused == Some(true)) {
        out.push_str(&format!(
            "focused: E{} {} {}\n",
            focus.index,
            focus.role,
            quoted_label(focus)
        ));
    }
    let scrollables: Vec<&WindowElement> = state
        .elements
        .iter()
        .filter(|e| e.scrollable == Some(true))
        .take(4)
        .collect();
    if !scrollables.is_empty() {
        out.push_str("scrollable:");
        for el in &scrollables {
            out.push_str(&format!(" E{}", el.index));
        }
        out.push('\n');
    }
    // Modal/dialog landmarks — a dialog changes what a click means.
    let modals: Vec<&WindowElement> = state
        .elements
        .iter()
        .filter(|e| {
            matches!(e.role.to_ascii_lowercase().as_str(), "dialog" | "alert") && e.depth <= 2
        })
        .take(3)
        .collect();
    if !modals.is_empty() {
        out.push_str("modal:");
        for el in &modals {
            out.push_str(&format!(" E{} {}", el.index, quoted_label(el)));
        }
        out.push('\n');
    }
    out.push_str("refs: E{n} = elementIndex for this snapshot — pass snapshotId+windowId, or elementToken \"");
    out.push_str(&state.snapshot_id);
    out.push_str(":{n}\"; stale after any UI change\n");
    for el in shown {
        out.push_str(&element_line(el));
        out.push('\n');
    }
    if significant_total > DIGEST_ELEMENT_CAP {
        out.push_str(&format!(
            "… {} more elements not shown — use computer_find to search the full set\n",
            significant_total - DIGEST_ELEMENT_CAP
        ));
    }
    out
}

fn quoted_label(el: &WindowElement) -> String {
    el.label
        .as_deref()
        .map(|l| format!("\"{}\"", clip(l, 40)))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// find — ranked semantic search over the FULL element set (not just shown)
// ---------------------------------------------------------------------------

/// One `computer_find` match — the ref plus enough context to act directly.
#[derive(Debug, Clone)]
pub struct FindHit {
    pub index: u32,
    pub token: Option<String>,
    pub role: String,
    pub label: Option<String>,
    pub value: Option<String>,
    pub frame: Option<ElementFrame>,
    pub depth: u32,
    pub actions: Vec<String>,
    pub score: u32,
}

/// Score a candidate against a free-text query: exact > prefix > contains >
/// word overlap. Role filtering happens before scoring.
fn score(el: &WindowElement, query_words: &[String]) -> u32 {
    let hay_label = el.label.as_deref().unwrap_or("").to_ascii_lowercase();
    let hay_value = el.value.as_deref().unwrap_or("").to_ascii_lowercase();
    let query = query_words.join(" ");
    if hay_label == query {
        return 100;
    }
    let mut score = 0;
    if hay_label.starts_with(&query) {
        score = score.max(80);
    }
    if hay_label.contains(&query) {
        score = score.max(60);
    }
    if hay_value.contains(&query) && !query.is_empty() {
        score = score.max(40);
    }
    let hits = query_words
        .iter()
        .filter(|w| {
            !w.is_empty() && (hay_label.contains(w.as_str()) || hay_value.contains(w.as_str()))
        })
        .count();
    score = score.max((hits * 20) as u32);
    // Actionable elements win ties — a found element is meant to be used.
    if score > 0 && !el.actions.is_empty() {
        score += 5;
    }
    score
}

/// `computer_find` — search the snapshot's full element set for `query`
/// (label/value text) optionally narrowed by `role`. Returns up to `limit`
/// ranked hits, best first.
pub fn find(state: &WindowState, query: &str, role: Option<&str>, limit: usize) -> Vec<FindHit> {
    // Words are lowercased once — the haystacks are too, so matching is
    // case-insensitive end to end.
    let words: Vec<String> = query
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_ascii_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    let role_lc = role.map(|r| r.to_ascii_lowercase());
    let mut hits: Vec<FindHit> = state
        .elements
        .iter()
        .filter(|e| {
            role_lc
                .as_ref()
                .is_none_or(|r| e.role.eq_ignore_ascii_case(r))
        })
        .filter_map(|e| {
            let s = score(e, &words);
            (s > 0).then(|| FindHit {
                index: e.index,
                token: e.token.clone(),
                role: e.role.clone(),
                label: e.label.clone(),
                value: e.value.clone(),
                frame: e.frame,
                depth: e.depth,
                actions: e.actions.clone(),
                score: s,
            })
        })
        .collect();
    hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.index.cmp(&b.index)));
    hits.truncate(limit);
    hits
}

/// Render find hits as compact lines (same grammar as the digest).
pub fn render_hits(hits: &[FindHit], snapshot_id: &str) -> String {
    if hits.is_empty() {
        return "no matching elements — try computer_observe or a different query".into();
    }
    let mut out = format!("snapshot {snapshot_id}\n");
    for hit in hits {
        let el = WindowElement {
            index: hit.index,
            role: hit.role.clone(),
            label: hit.label.clone(),
            value: hit.value.clone(),
            enabled: true,
            frame: hit.frame,
            depth: hit.depth,
            actions: hit.actions.clone(),
            ..Default::default()
        };
        out.push_str(&element_line(&el));
        out.push('\n');
    }
    out
}
