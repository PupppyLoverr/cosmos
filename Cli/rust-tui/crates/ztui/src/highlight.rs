//! Syntax highlighting for fenced code — tree-sitter-highlight, the same
//! engine Devin CLI ships. Grammars compile in; each language's query is
//! built lazily on first use and cached for the session.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Capture names we colour; index = `Highlight.0` in events.
const NAMES: &[&str] = &[
    "attribute",
    "boolean",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
    "keyword",
    "label",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.escape",
    "string.special",
    "tag",
    "type",
    "type.builtin",
    "variable.builtin",
    "variable.parameter",
];

const BASE: Color = Color::Rgb(0xd4, 0xd4, 0xd4);

fn style_for(name: &str) -> Style {
    let c = |r, g, b| Style::new().fg(Color::Rgb(r, g, b));
    match name {
        "comment" => c(0x6a, 0x73, 0x7d).add_modifier(Modifier::ITALIC),
        "keyword" | "label" => c(0xc6, 0x8f, 0xf2),
        "string" | "string.special" => c(0x9e, 0xce, 0x6a),
        "string.escape" | "escape" => c(0x89, 0xdd, 0xff),
        "number" | "boolean" | "constant" | "constant.builtin" => c(0xff, 0x9e, 0x64),
        "function" | "function.method" | "function.builtin" | "function.macro" => c(0x7a, 0xa2, 0xf7),
        "type" | "type.builtin" | "constructor" | "module" => c(0x2a, 0xc3, 0xde),
        "tag" => c(0xf7, 0x76, 0x8e),
        "attribute" | "property" => c(0x73, 0xda, 0xca),
        "operator" => c(0x89, 0xdd, 0xff),
        "variable.builtin" | "variable.parameter" => c(0xe0, 0xaf, 0x68),
        n if n.starts_with("punctuation") => c(0x9a, 0xa5, 0xb8),
        _ => Style::new().fg(BASE),
    }
}

/// Canonical grammar name for a fence info string.
pub fn canonical(lang: &str) -> Option<&'static str> {
    let l = lang.trim().to_ascii_lowercase();
    let l = l.split([' ', ',', '{']).next().unwrap_or("");
    Some(match l {
        "rs" | "rust" => "rust",
        "py" | "python" | "python3" => "python",
        "js" | "javascript" | "jsx" | "mjs" | "cjs" | "node" => "javascript",
        "ts" | "typescript" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "go" | "golang" => "go",
        "sh" | "bash" | "shell" | "zsh" | "console" | "shellsession" | "ps1" | "powershell" => "bash",
        "json" | "jsonc" | "json5" => "json",
        "toml" => "toml",
        "yaml" | "yml" => "yaml",
        "c" | "h" => "c",
        "cpp" | "c++" | "cc" | "cxx" | "hpp" => "cpp",
        "css" | "scss" => "css",
        "html" | "htm" | "xml" | "svg" | "vue" => "html",
        _ => return None,
    })
}

fn build(lang: &str) -> Option<HighlightConfiguration> {
    let js = tree_sitter_javascript::HIGHLIGHT_QUERY;
    let (language, name, query): (tree_sitter::Language, &str, String) = match lang {
        "rust" => (tree_sitter_rust::LANGUAGE.into(), "rust", tree_sitter_rust::HIGHLIGHTS_QUERY.into()),
        "python" => (tree_sitter_python::LANGUAGE.into(), "python", tree_sitter_python::HIGHLIGHTS_QUERY.into()),
        "javascript" => (
            tree_sitter_javascript::LANGUAGE.into(),
            "javascript",
            format!("{js}\n{}", tree_sitter_javascript::JSX_HIGHLIGHT_QUERY),
        ),
        "typescript" => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            "typescript",
            format!("{}\n{js}", tree_sitter_typescript::HIGHLIGHTS_QUERY),
        ),
        "tsx" => (
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            "tsx",
            format!(
                "{}\n{js}\n{}",
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
            ),
        ),
        "go" => (tree_sitter_go::LANGUAGE.into(), "go", tree_sitter_go::HIGHLIGHTS_QUERY.into()),
        "bash" => (tree_sitter_bash::LANGUAGE.into(), "bash", tree_sitter_bash::HIGHLIGHT_QUERY.into()),
        "json" => (tree_sitter_json::LANGUAGE.into(), "json", tree_sitter_json::HIGHLIGHTS_QUERY.into()),
        "toml" => (tree_sitter_toml_ng::LANGUAGE.into(), "toml", tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into()),
        "yaml" => (tree_sitter_yaml::LANGUAGE.into(), "yaml", tree_sitter_yaml::HIGHLIGHTS_QUERY.into()),
        "c" => (tree_sitter_c::LANGUAGE.into(), "c", tree_sitter_c::HIGHLIGHT_QUERY.into()),
        "cpp" => (
            tree_sitter_cpp::LANGUAGE.into(),
            "cpp",
            format!("{}\n{}", tree_sitter_cpp::HIGHLIGHT_QUERY, tree_sitter_c::HIGHLIGHT_QUERY),
        ),
        "css" => (tree_sitter_css::LANGUAGE.into(), "css", tree_sitter_css::HIGHLIGHTS_QUERY.into()),
        "html" => (tree_sitter_html::LANGUAGE.into(), "html", tree_sitter_html::HIGHLIGHTS_QUERY.into()),
        _ => return None,
    };
    let mut cfg = HighlightConfiguration::new(language, name, &query, "", "").ok()?;
    cfg.configure(NAMES);
    Some(cfg)
}

thread_local! {
    static CONFIGS: RefCell<HashMap<&'static str, Option<Rc<HighlightConfiguration>>>> = RefCell::new(HashMap::new());
    static HL: RefCell<Highlighter> = RefCell::new(Highlighter::new());
}

fn config(lang: &'static str) -> Option<Rc<HighlightConfiguration>> {
    CONFIGS.with(|c| {
        c.borrow_mut()
            .entry(lang)
            .or_insert_with(|| build(lang).map(Rc::new))
            .clone()
    })
}

/// Highlight `code`, returning one span list per source line. `None` when
/// the language is unknown (callers render plain).
pub fn highlight(lang: &str, code: &str) -> Option<Vec<Vec<Span<'static>>>> {
    let cfg = config(canonical(lang)?)?;
    let events: Vec<HighlightEvent> = HL.with(|h| {
        let mut h = h.borrow_mut();
        h.highlight(&cfg, code.as_bytes(), None, |_| None)
            .ok()
            .map(|it| it.filter_map(Result::ok).collect())
    })?;

    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut stack: Vec<Style> = Vec::new();
    for ev in events {
        match ev {
            HighlightEvent::HighlightStart(h) => stack.push(style_for(NAMES[h.0])),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let style = stack.last().copied().unwrap_or(Style::new().fg(BASE));
                let text = &code[start..end];
                for (i, part) in text.split('\n').enumerate() {
                    if i > 0 {
                        lines.push(Vec::new());
                    }
                    let part = part.trim_end_matches('\r');
                    if !part.is_empty() {
                        lines.last_mut().unwrap().push(Span::styled(part.to_string(), style));
                    }
                }
            }
        }
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_keywords_get_colour() {
        let l = highlight("rust", "fn main() {\n    let x = 1;\n}").unwrap();
        assert_eq!(l.len(), 3);
        let kw = l[0].iter().find(|s| s.content == "fn").unwrap();
        assert_eq!(kw.style, style_for("keyword"));
    }

    #[test]
    fn every_grammar_loads() {
        for lang in [
            "rust", "python", "javascript", "typescript", "tsx", "go", "bash", "json", "toml",
            "yaml", "c", "cpp", "css", "html",
        ] {
            assert!(config(lang).is_some(), "{lang} failed to build");
        }
    }

    #[test]
    fn unknown_language_is_none() {
        assert!(highlight("brainfuck", "+++").is_none());
    }
}
