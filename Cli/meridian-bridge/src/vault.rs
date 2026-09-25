use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// On-disk vault layout:
///
/// ```text
/// index\ (active.md, this-week.md, by-area\)
/// memory\<scope>\<YYYY-MM>.md
/// cache\<sha[0:2]>\<sha256>.md
/// playbooks\*.md
/// sessions\<session_id>.md
/// MERIDIAN.md
/// ```
pub struct Vault {
    root: PathBuf,
}

const MERIDIAN_MD_DEFAULT: &str = r#"# MERIDIAN.md — Cosmos CLI shared memory vault

This vault is the shared memory of the Cosmos CLI family (zcli, zdev, zagy).
All three harnesses read and write it through the meridian bridge. The bridge
creates this file when absent and never rewrites it mid-session.

## Layout

- `index/` — hand-maintained entry points (`active.md`, `this-week.md`, `by-area/`).
- `memory/<scope>/<YYYY-MM>.md` — the memory log. One bullet per fact.
- `cache/<sha[0:2]>/<sha256>.md` — per-file summaries keyed by content hash.
- `playbooks/*.md` — named procedures fetched with `playbook.get`.
- `sessions/<session_id>.md` — session pointer cards (harness, log path, title,
  dates — never the transcript).

## Memory format

One bullet per fact:

```
- 2026-09-22 [decision] [repo:cosmos] <text> <!-- id:<ulid> src:<harness>/<session> -->
```

Types: `decision | preference | map | correction | playbook`.

## Retrieval policy

- `memory.search` runs BM25 over memory bullets and playbook headings.
  No embeddings, no network.
- Writes take a lock file, write a temp file, then rename.
- The secret scanner rejects anything that looks like a credential; rejected
  text is never stored.
"#;

impl Vault {
    /// Resolve the vault root: `MERIDIAN_VAULT` env, else
    /// `%USERPROFILE%\.meridian\vault`.
    pub fn default_root() -> Result<PathBuf> {
        if let Ok(v) = std::env::var("MERIDIAN_VAULT") {
            if !v.trim().is_empty() {
                return Ok(PathBuf::from(v));
            }
        }
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .context("no USERPROFILE/HOME for default vault path")?;
        Ok(PathBuf::from(home).join(".meridian").join("vault"))
    }

    /// Open (and initialize if needed) the vault at `root`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        for d in ["index", "index\\by-area", "memory", "cache", "playbooks", "sessions"] {
            fs::create_dir_all(root.join(d))
                .with_context(|| format!("creating vault dir {d}"))?;
        }
        let schema = root.join("MERIDIAN.md");
        if !schema.exists() {
            fs::write(&schema, MERIDIAN_MD_DEFAULT).context("writing MERIDIAN.md")?;
        }
        Ok(Self { root })
    }

    /// Open the default vault.
    pub fn open_default() -> Result<Self> {
        Self::open(Self::default_root()?)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn memory_dir(&self) -> PathBuf {
        self.root.join("memory")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn playbooks_dir(&self) -> PathBuf {
        self.root.join("playbooks")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn meridian_md(&self) -> PathBuf {
        self.root.join("MERIDIAN.md")
    }

    /// Path of the memory file for a scope + month, e.g.
    /// `memory\repo:cosmos\2026-09.md`. Scope segments are sanitized to a
    /// single path component (no traversal).
    pub fn memory_file(&self, scope: &str, month: &str) -> PathBuf {
        let scope = sanitize_component(scope);
        self.memory_dir().join(scope).join(format!("{month}.md"))
    }
}

/// Keep a scope usable as one path component. Windows forbids `:` in names,
/// so `repo:cosmos` lives on disk as `repo_cosmos` — the bullet text keeps
/// the original `:` spelling.
fn sanitize_component(scope: &str) -> String {
    let cleaned: String = scope
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() || cleaned == ".." {
        "misc".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_blocks_traversal() {
        assert_eq!(sanitize_component("repo:cosmos"), "repo_cosmos");
        assert_eq!(sanitize_component("..\\evil"), "_evil");
        assert_eq!(sanitize_component(".."), "misc");
        assert_eq!(sanitize_component(""), "misc");
        assert_eq!(sanitize_component("a/b\\c"), "a_b_c");
    }
}
