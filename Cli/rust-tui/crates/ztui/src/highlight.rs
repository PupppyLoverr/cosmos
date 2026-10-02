//! Syntax colour for fenced code.
//!
//! The default build uses a small built-in lexer (keywords, strings,
//! comments, numbers, calls, types) — no grammar crates, so the binary stays
//! small. Build with `--features highlight` to use tree-sitter grammars
//! instead (`highlight_ts.rs`).

use ratatui_core::style::{Color, Modifier, Style};
use ratatui_core::text::Span;

/// Plain code text in the current theme.
#[cfg_attr(not(feature = "highlight"), allow(dead_code))]
pub(crate) fn base() -> Color {
    crate::skin::palette().syn_base
}

pub(crate) fn style_for(name: &str) -> Style {
    let p = crate::skin::palette();
    let c = |col: Color| Style::new().fg(col);
    match name {
        "comment" => c(p.syn_comment).add_modifier(Modifier::ITALIC),
        "keyword" | "label" => c(p.syn_keyword),
        "string" | "string.special" => c(p.syn_string),
        "string.escape" | "escape" => c(p.syn_escape),
        "number" | "boolean" | "constant" | "constant.builtin" => c(p.syn_number),
        "function" | "function.method" | "function.builtin" | "function.macro" => c(p.syn_function),
        "type" | "type.builtin" | "constructor" | "module" => c(p.syn_type),
        "tag" => c(p.syn_tag),
        "attribute" | "property" => c(p.syn_attribute),
        "operator" => c(p.syn_operator),
        "variable.builtin" | "variable.parameter" => c(p.syn_variable),
        n if n.starts_with("punctuation") => c(p.syn_punct),
        _ => Style::new().fg(p.syn_base),
    }
}

/// Canonical language name for a fence info string.
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
        "java" | "kotlin" | "kt" | "cs" | "csharp" | "swift" => "java",
        "sql" => "sql",
        _ => return None,
    })
}

/// Highlight `code`, one span list per source line. `None` when the language
/// is unknown (callers render plain).
#[cfg(feature = "highlight")]
pub fn highlight(lang: &str, code: &str) -> Option<Vec<Vec<Span<'static>>>> {
    crate::highlight_ts::highlight(lang, code)
}

/// Highlight `code`, one span list per source line. `None` when the language
/// is unknown (callers render plain).
#[cfg(not(feature = "highlight"))]
pub fn highlight(lang: &str, code: &str) -> Option<Vec<Vec<Span<'static>>>> {
    lite(canonical(lang)?, code)
}

fn keywords(lang: &str) -> &'static [&'static str] {
    match lang {
        "rust" => &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if",
            "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
            "super", "trait", "true", "type", "unsafe", "use", "where", "while",
        ],
        "python" => &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "False",
            "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "None", "nonlocal", "not", "or", "pass",
            "raise", "return", "True", "try", "while", "with", "yield", "self",
        ],
        "javascript" | "typescript" | "tsx" => &[
            "as", "async", "await", "break", "case", "catch", "class", "const", "continue", "default", "delete", "do", "else",
            "enum", "export", "extends", "false", "finally", "for", "from", "function", "if", "implements", "import", "in",
            "instanceof", "interface", "let", "new", "null", "of", "private", "protected", "public", "readonly", "return",
            "static", "super", "switch", "this", "throw", "true", "try", "type", "typeof", "undefined", "var", "void", "while",
            "yield",
        ],
        "go" => &[
            "break", "case", "chan", "const", "continue", "default", "defer", "else", "fallthrough", "false", "for", "func", "go",
            "goto", "if", "import", "interface", "map", "nil", "package", "range", "return", "select", "struct", "switch", "true",
            "type", "var",
        ],
        "bash" => &[
            "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if", "in", "local", "return", "then",
            "until", "while", "echo", "cd", "set", "param", "foreach",
        ],
        "c" | "cpp" | "java" => &[
            "auto", "bool", "break", "case", "catch", "char", "class", "const", "continue", "default", "delete", "do", "double",
            "else", "enum", "extends", "extern", "false", "final", "float", "for", "fun", "if", "import", "include", "int",
            "interface", "long", "namespace", "new", "null", "nullptr", "override", "package", "private", "protected", "public",
            "return", "short", "signed", "sizeof", "static", "struct", "switch", "template", "this", "throw", "true", "try",
            "typedef", "union", "unsigned", "using", "val", "var", "virtual", "void", "while",
        ],
        "sql" => &[
            "select", "from", "where", "and", "or", "not", "insert", "into", "values", "update", "set", "delete", "create", "table",
            "join", "left", "right", "inner", "on", "group", "by", "order", "limit", "as", "null", "is", "primary", "key",
        ],
        "json" | "toml" | "yaml" => &["true", "false", "null", "yes", "no"],
        _ => &[],
    }
}

fn line_comment(lang: &str) -> &'static [&'static str] {
    match lang {
        "python" | "bash" | "toml" | "yaml" => &["#"],
        "sql" => &["--"],
        "json" | "html" | "css" => &[],
        _ => &["//"],
    }
}

/// Built-in lexer: good enough to read code at a glance, a few KB of binary.
fn lite(lang: &'static str, code: &str) -> Option<Vec<Vec<Span<'static>>>> {
    let kw = keywords(lang);
    let lc = line_comment(lang);
    let block = !matches!(lang, "python" | "bash" | "toml" | "yaml" | "json");
    let ci = lang == "sql";
    let mut in_block = false;
    let mut out = Vec::new();
    for raw in code.split('\n') {
        let line = raw.trim_end_matches('\r');
        let mut spans: Vec<Span<'static>> = Vec::new();
        let push = |spans: &mut Vec<Span<'static>>, s: &str, kind: &str| {
            if !s.is_empty() {
                spans.push(Span::styled(s.to_string(), style_for(kind)));
            }
        };
        let b = line.as_bytes();
        let mut i = 0;
        let mut plain = 0; // start of pending plain text
        macro_rules! flush {
            () => {
                push(&mut spans, &line[plain..i], "");
            };
        }
        while i < b.len() {
            if in_block {
                let end = line[i..].find("*/").map(|p| i + p + 2).unwrap_or(b.len());
                push(&mut spans, &line[i..end], "comment");
                i = end;
                plain = i;
                in_block = end == b.len() && !line.ends_with("*/");
                continue;
            }
            let rest = &line[i..];
            if lc.iter().any(|c| rest.starts_with(c)) && !(lang == "bash" && i > 0 && b[i - 1] == b'$') {
                flush!();
                push(&mut spans, rest, "comment");
                i = b.len();
                plain = i;
                break;
            }
            if block && rest.starts_with("/*") {
                flush!();
                in_block = true;
                continue;
            }
            let c = b[i];
            if c == b'"' || c == b'\'' || c == b'`' {
                // a Rust lifetime / char is short; treat a lone ' with no
                // closing quote on the line as plain text
                let close = line[i + 1..].bytes().enumerate().scan(false, |esc, (k, ch)| {
                    let hit = !*esc && ch == c;
                    *esc = !*esc && ch == b'\\';
                    Some((k, hit))
                });
                let end = close.filter(|(_, hit)| *hit).map(|(k, _)| i + 1 + k + 1).next();
                if let Some(end) = end {
                    flush!();
                    push(&mut spans, &line[i..end], "string");
                    i = end;
                    plain = i;
                    continue;
                }
            }
            if c.is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) {
                let end = i + line[i..].bytes().take_while(|x| x.is_ascii_alphanumeric() || *x == b'.' || *x == b'_').count();
                flush!();
                push(&mut spans, &line[i..end], "number");
                i = end;
                plain = i;
                continue;
            }
            if c.is_ascii_alphabetic() || c == b'_' {
                let end = i + line[i..].bytes().take_while(|x| x.is_ascii_alphanumeric() || *x == b'_').count();
                let word = &line[i..end];
                let is_kw = if ci { kw.iter().any(|k| k.eq_ignore_ascii_case(word)) } else { kw.contains(&word) };
                let next = line[end..].trim_start().bytes().next();
                let kind = if is_kw {
                    "keyword"
                } else if next == Some(b'(') || (lang == "rust" && next == Some(b'!')) {
                    "function"
                } else if word.as_bytes()[0].is_ascii_uppercase() && lang != "sql" {
                    "type"
                } else if matches!(lang, "toml" | "yaml" | "json") && matches!(next, Some(b'=') | Some(b':')) {
                    "property"
                } else {
                    ""
                };
                if !kind.is_empty() {
                    flush!();
                    push(&mut spans, word, kind);
                    plain = end;
                }
                i = end;
                continue;
            }
            // advance one UTF-8 char
            i += line[i..].chars().next().map(char::len_utf8).unwrap_or(1);
        }
        push(&mut spans, &line[plain.min(b.len())..], "");
        out.push(spans);
    }
    Some(out)
}

#[cfg(all(test, not(feature = "highlight")))]
mod tests {
    use super::*;

    fn kinds(l: &[Span]) -> Vec<(String, Style)> {
        l.iter().map(|s| (s.content.to_string(), s.style)).collect()
    }

    #[test]
    fn rust_keywords_strings_comments() {
        let l = highlight("rust", "fn main() {\n    let s = \"hi\"; // note\n}").unwrap();
        assert_eq!(l.len(), 3);
        let k = kinds(&l[0]);
        assert!(k.contains(&("fn".into(), style_for("keyword"))));
        assert!(k.contains(&("main".into(), style_for("function"))));
        let k = kinds(&l[1]);
        assert!(k.contains(&("\"hi\"".into(), style_for("string"))));
        assert!(k.contains(&("// note".into(), style_for("comment"))));
        // text round-trips exactly
        let text: String = l[1].iter().map(|s| s.content.to_string()).collect();
        assert_eq!(text, "    let s = \"hi\"; // note");
    }

    #[test]
    fn block_comments_span_lines() {
        let l = highlight("c", "/* a\nb */ int x = 1;").unwrap();
        assert_eq!(kinds(&l[1])[0], ("b */".into(), style_for("comment")));
        assert!(kinds(&l[1]).contains(&("int".into(), style_for("keyword"))));
    }

    #[test]
    fn unicode_and_lifetimes_do_not_panic() {
        let l = highlight("rust", "fn f<'a>(x: &'a str) -> é { 42 }").unwrap();
        let text: String = l[0].iter().map(|s| s.content.to_string()).collect();
        assert_eq!(text, "fn f<'a>(x: &'a str) -> é { 42 }");
    }

    #[test]
    fn unknown_language_is_none() {
        assert!(highlight("brainfuck", "+++").is_none());
    }
}
