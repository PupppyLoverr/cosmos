//! `cache.read` — per-file summaries keyed by content hash.
//!
//! On a miss the deterministic extractor reports line count, language,
//! imports, top-level declaration signatures with line numbers, and the first
//! doc comment. Results are stored under `cache/<sha[0:2]>/<sha256>.md` with
//! frontmatter `{source, sha256, model, created, ttl_days:30}`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::vault::Vault;

pub fn file_sha256(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }
}

fn language_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "go" => "go",
        "ts" | "tsx" | "mts" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "md" | "markdown" => "markdown",
        "ps1" | "psm1" => "powershell",
        "toml" => "toml",
        "json" => "json",
        _ => "text",
    }
}

struct LangSpec {
    import: &'static str,
    decl: &'static str,
    doc: &'static str,
}

fn spec(lang: &str) -> LangSpec {
    match lang {
        "rust" => LangSpec {
            import: r"^\s*(use\s+\S|extern\s+crate)",
            decl: r"^\s*(pub\s+)?(async\s+)?(unsafe\s+)?(fn|struct|enum|trait|impl|mod|const|static|type)\s+\w+",
            doc: r"^\s*(///|//!)",
        },
        "python" => LangSpec {
            import: r"^\s*(import\s+\S|from\s+\S+\s+import)",
            decl: r"^(def|class|async\s+def)\s+\w+|^\w+\s*=\s*",
            doc: r##"^\s*("""|''')"##,
        },
        "go" => LangSpec {
            import: r#"^\s*(import\s|\t"[\w./]+"|\t\w+\s+"[\w./]+")"#,
            decl: r"^(func|type|var|const)\s+(\(\s*\w+\s+\*?\w+\s*\)\s+)?\w+",
            doc: r"^\s*//",
        },
        "typescript" | "javascript" => LangSpec {
            import: r#"^\s*(import\s|const\s+\w+\s*=\s*require)"#,
            decl: r"^\s*(export\s+)?(default\s+)?(async\s+)?(function\*?|class|interface|type|const|let|var)\s+\w*",
            doc: r"^\s*(//|/\*)",
        },
        "markdown" => LangSpec {
            import: r"a^", // never matches
            decl: r"^#{1,6}\s+\S",
            doc: r"a^",
        },
        _ => LangSpec {
            import: r"a^",
            decl: r"a^",
            doc: r"^\s*(#|//)",
        },
    }
}

/// Deterministic summary of a source file.
pub fn extract(path: &Path, content: &str, max_lines: usize) -> String {
    let lang = language_of(path);
    let spec = spec(lang);
    let import_re = Regex::new(spec.import).unwrap();
    let decl_re = Regex::new(spec.decl).unwrap();
    let doc_re = Regex::new(spec.doc).unwrap();

    let lines: Vec<&str> = content.lines().collect();
    let mut out = String::new();
    out.push_str(&format!(
        "# Summary: {}\n\nlanguage: {}\nlines: {}\n",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
        lang,
        lines.len()
    ));

    let mut imports = Vec::new();
    let mut decls = Vec::new();
    let mut doc: Option<String> = None;
    for (i, line) in lines.iter().enumerate() {
        let n = i + 1;
        if import_re.is_match(line) {
            imports.push(format!("{n}: {}", line.trim()));
        }
        if decl_re.is_match(line) {
            let sig = line.trim_end();
            let sig = if sig.len() > 160 { &sig[..160] } else { sig };
            decls.push(format!("{n}: {sig}"));
        }
        if doc.is_none() && doc_re.is_match(line) {
            doc = Some(line.trim().to_string());
        }
    }
    if let Some(d) = doc {
        out.push_str(&format!("\ndoc: {d}\n"));
    }
    out.push_str("\n## imports\n");
    for i in imports.iter().take(40) {
        out.push_str(i);
        out.push('\n');
    }
    out.push_str("\n## declarations\n");
    for d in decls.iter().take(80) {
        out.push_str(d);
        out.push('\n');
    }
    if lines.len() > max_lines {
        out.push_str(&format!(
            "\n(truncated: {} lines exceeds max_lines={max_lines})\n",
            lines.len()
        ));
    }
    out
}

fn cache_path(vault: &Vault, sha: &str) -> PathBuf {
    vault.cache_dir().join(&sha[..2]).join(format!("{sha}.md"))
}

/// `cache.read {path, max_lines}` — hit returns the stored summary; a miss
/// summarizes deterministically, stores, and returns. A changed file is a
/// different hash, hence a miss by construction.
pub fn cache_read(vault: &Vault, path: &Path, max_lines: usize) -> Result<String> {
    let sha = file_sha256(path)?;
    let cp = cache_path(vault, &sha);
    if cp.exists() {
        let text = fs::read_to_string(&cp)?;
        // strip frontmatter, return the summary body
        if let Some(idx) = text.find("\n---\n") {
            return Ok(text[idx + 5..].trim_start().to_string());
        }
        return Ok(text);
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading {} for summary", path.display()))?;
    let summary = extract(path, &content, max_lines);
    let fm = format!(
        "---\nsource: {}\nsha256: {sha}\nmodel: deterministic\ncreated: {}\nttl_days: 30\n---\n\n",
        path.display(),
        Local::now().format("%Y-%m-%d")
    );
    fs::create_dir_all(cp.parent().unwrap())?;
    let tmp = cp.with_extension("md.tmp");
    fs::write(&tmp, format!("{fm}{summary}"))?;
    fs::rename(&tmp, &cp)?;
    Ok(summary)
}
