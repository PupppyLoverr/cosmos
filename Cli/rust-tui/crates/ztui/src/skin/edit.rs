//! Small, comment-preserving edits to `ui.toml`: set or remove one key in
//! one table, leaving every other line (and the operator's comments) as it
//! was. Every write is re-parsed before it lands and keeps a backup ring.
// Val variants, from_json and remove serve /customise.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// A TOML value for one key. Strings are escaped; lists hold strings.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    List(Vec<String>),
}

fn quote(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

impl Val {
    pub fn toml(&self) -> String {
        match self {
            Val::Str(s) => quote(s),
            Val::Int(i) => i.to_string(),
            Val::Float(f) if f.fract() == 0.0 => format!("{f:.1}"),
            Val::Float(f) => f.to_string(),
            Val::Bool(b) => b.to_string(),
            Val::List(v) => format!("[{}]", v.iter().map(|s| quote(s)).collect::<Vec<_>>().join(", ")),
        }
    }

    /// From JSON (what `/customise` tools pass).
    pub fn from_json(v: &serde_json::Value) -> Option<Val> {
        Some(match v {
            serde_json::Value::String(s) => Val::Str(s.clone()),
            serde_json::Value::Bool(b) => Val::Bool(*b),
            serde_json::Value::Number(n) if n.is_i64() => Val::Int(n.as_i64()?),
            serde_json::Value::Number(n) => Val::Float(n.as_f64()?),
            serde_json::Value::Array(a) => Val::List(a.iter().map(|x| x.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()?),
            _ => return None,
        })
    }
}

/// Bare key as written, quoted when it needs to be (`"syntax.keyword"`).
fn key_text(k: &str) -> String {
    if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        k.to_string()
    } else {
        quote(k)
    }
}

fn header_of(line: &str) -> Option<String> {
    let t = line.trim();
    if t.starts_with("[[") || !t.starts_with('[') {
        return None;
    }
    let end = t.find(']')?;
    Some(t[1..end].split('.').map(|p| p.trim().trim_matches('"')).collect::<Vec<_>>().join("."))
}

fn key_of(line: &str) -> Option<String> {
    let t = line.trim_start();
    if t.starts_with('#') || t.starts_with('[') {
        return None;
    }
    let eq = t.find('=')?;
    Some(t[..eq].trim().trim_matches('"').to_string())
}

/// Set `table.key = value` (table like "theme.colors"; "" = top level).
pub fn set(text: &str, table: &str, key: &str, value: &Val) -> String {
    let line = format!("{} = {}", key_text(key), value.toml());
    let lines: Vec<&str> = text.lines().collect();
    let mut cur = String::new();
    let mut table_end: Option<usize> = None;
    for (i, l) in lines.iter().enumerate() {
        if let Some(h) = header_of(l) {
            cur = h;
            continue;
        }
        if cur == table {
            table_end = Some(i + 1);
            if key_of(l).as_deref() == Some(key) {
                // keep a trailing comment on the line
                let comment = l.find(" #").map(|c| &l[c..]).filter(|c| !c.contains('"')).unwrap_or("");
                let mut out: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
                out[i] = format!("{line}{comment}");
                return out.join("\n") + "\n";
            }
        }
    }
    let mut out: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    // the table exists: add the key after its last line (or after its header)
    let header_at = lines.iter().position(|l| header_of(l).as_deref() == Some(table));
    match (header_at, table_end) {
        (_, Some(end)) if !table.is_empty() || header_at.is_some() => out.insert(end, line),
        (Some(h), None) => out.insert(h + 1, line),
        _ if table.is_empty() => {
            // top level: before the first header
            let first = lines.iter().position(|l| header_of(l).is_some()).unwrap_or(lines.len());
            out.insert(first, line);
        }
        _ => {
            if out.last().is_some_and(|l| !l.trim().is_empty()) {
                out.push(String::new());
            }
            out.push(format!("[{}]", table.split('.').map(key_text).collect::<Vec<_>>().join(".")));
            out.push(line);
        }
    }
    out.join("\n") + "\n"
}

/// Remove `table.key`; unchanged when absent.
pub fn remove(text: &str, table: &str, key: &str) -> String {
    let mut cur = String::new();
    let mut out = Vec::new();
    for l in text.lines() {
        if let Some(h) = header_of(l) {
            cur = h;
        } else if cur == table && key_of(l).as_deref() == Some(key) {
            continue;
        }
        out.push(l);
    }
    out.join("\n") + "\n"
}

/// How many previous versions `ui.toml` keeps (`ui.toml.bak.1` newest).
pub const BACKUPS: usize = 10;

/// Write `text` to `path` after checking it parses as a skin; the old file
/// rotates into the backup ring first.
pub fn write(path: &Path, text: &str) -> Result<(), String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| format!("not valid TOML: {}", e.to_string().trim()))?;
    toml::Value::Table(table).try_into::<super::Skin>().map_err(|e| format!("not a valid ui.toml: {}", e.to_string().trim()))?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    if path.exists() {
        let bak = |n: usize| PathBuf::from(format!("{}.bak.{n}", path.display()));
        let _ = std::fs::remove_file(bak(BACKUPS));
        for n in (1..BACKUPS).rev() {
            let _ = std::fs::rename(bak(n), bak(n + 1));
        }
        let _ = std::fs::copy(path, bak(1));
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Put `ui.toml.bak.1` back (and shift the ring down). Err when none.
pub fn undo(path: &Path) -> Result<(), String> {
    let bak = |n: usize| PathBuf::from(format!("{}.bak.{n}", path.display()));
    if !bak(1).exists() {
        return Err("nothing to undo".into());
    }
    std::fs::rename(bak(1), path).map_err(|e| e.to_string())?;
    for n in 2..=BACKUPS {
        let _ = std::fs::rename(bak(n), bak(n - 1));
    }
    Ok(())
}

/// Read, set one key, write back.
pub fn set_in_file(path: &Path, table: &str, key: &str, value: &Val) -> Result<(), String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    write(path, &set(&text, table, key, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_keeps_comments_and_other_keys() {
        let src = "# my look\n[theme]\nname = \"mono\" # calm\n\n[theme.colors]\naccent = \"#111111\"\n";
        let out = set(src, "theme", "name", &Val::Str("nebula".into()));
        assert_eq!(out, "# my look\n[theme]\nname = \"nebula\" # calm\n\n[theme.colors]\naccent = \"#111111\"\n");
        let out = set(&out, "theme.colors", "syntax.keyword", &Val::Str("magenta".into()));
        assert!(out.contains("accent = \"#111111\"\n\"syntax.keyword\" = \"magenta\"\n"), "{out}");
        let out = set(&out, "spinner", "verbs", &Val::List(vec!["Warping".into(), "Say \"hi\"".into()]));
        assert!(out.ends_with("[spinner]\nverbs = [\"Warping\", \"Say \\\"hi\\\"\"]\n"), "{out}");
        let t: toml::Table = toml::from_str(&out).unwrap();
        assert_eq!(t["theme"]["colors"]["syntax.keyword"].as_str(), Some("magenta"));
        let out = remove(&out, "theme.colors", "accent");
        assert!(!out.contains("#111111"));
    }

    #[test]
    fn set_into_empty_file_and_backup_ring() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("ui.toml");
        set_in_file(&f, "theme", "name", &Val::Str("solar".into())).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "[theme]\nname = \"solar\"\n");
        set_in_file(&f, "theme", "name", &Val::Str("mono".into())).unwrap();
        assert!(std::fs::read_to_string(&f).unwrap().contains("mono"));
        undo(&f).unwrap();
        assert!(std::fs::read_to_string(&f).unwrap().contains("solar"));
        assert!(undo(&f).is_err());
        assert!(write(&f, "[theme\n").is_err(), "invalid TOML never lands");
        assert!(write(&f, "[theme]\nname = 3\n").is_err(), "wrong types never land");
    }
}
