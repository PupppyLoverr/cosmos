//! `@` mentions: a gitignore-aware file index (ripgrep's `ignore` walker,
//! the same crate Devin ships) built in the background on first use, and
//! prompt expansion that attaches the mentioned files' contents.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const MAX_FILES: usize = 25_000;
const MAX_ATTACH_BYTES: usize = 120_000;

#[derive(Clone, Default)]
pub struct Index {
    inner: Arc<Mutex<Option<Vec<String>>>>,
    started: Arc<Mutex<bool>>,
}

impl Index {
    /// Start indexing `root` (once). Paths are relative, `/`-separated;
    /// directories end with `/`.
    pub fn ensure(&self, root: &Path) {
        let mut s = self.started.lock().unwrap();
        if *s {
            return;
        }
        *s = true;
        let inner = self.inner.clone();
        let root = root.to_path_buf();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let walk = ignore::WalkBuilder::new(&root).hidden(true).git_ignore(true).parents(true).max_depth(Some(24)).build();
            for e in walk.flatten() {
                if out.len() >= MAX_FILES {
                    break;
                }
                let Ok(rel) = e.path().strip_prefix(&root) else { continue };
                if rel.as_os_str().is_empty() {
                    continue;
                }
                let mut s = rel.to_string_lossy().replace('\\', "/");
                if e.file_type().is_some_and(|t| t.is_dir()) {
                    s.push('/');
                }
                out.push(s);
            }
            out.sort_by(|a, b| a.len().cmp(&b.len()).then(a.cmp(b)));
            *inner.lock().unwrap() = Some(out);
        });
    }

    pub fn ready(&self) -> bool {
        self.inner.lock().unwrap().is_some()
    }

    /// Best matches for `q` (fuzzy, path-aware).
    pub fn search(&self, q: &str, limit: usize) -> Vec<String> {
        let g = self.inner.lock().unwrap();
        let Some(all) = g.as_ref() else { return Vec::new() };
        crate::slash::rank(all, q, |s| s.clone()).into_iter().take(limit).map(|i| all[i].clone()).collect()
    }
}

/// The `@token` the cursor is in, as (start index in chars, token without `@`).
pub fn mention_at(input: &[char], cursor: usize) -> Option<(usize, String)> {
    let mut i = cursor;
    while i > 0 && !input[i - 1].is_whitespace() {
        i -= 1;
    }
    if input.get(i) != Some(&'@') {
        return None;
    }
    // an email address or decorator isn't a mention
    if i > 0 && !input[i - 1].is_whitespace() {
        return None;
    }
    Some((i, input[i + 1..cursor].iter().collect()))
}

/// Append the contents of every `@path` in `text` that names a file under
/// `root` (directories get a listing). Returns the prompt to send.
pub fn expand_mentions(text: &str, root: &Path) -> String {
    let mut attached: Vec<String> = Vec::new();
    let mut budget = MAX_ATTACH_BYTES;
    for word in text.split_whitespace() {
        let Some(rel) = word.strip_prefix('@') else { continue };
        let rel = rel.trim_end_matches([',', '.', ';', ':', ')', '?', '!']);
        if rel.is_empty() || attached.iter().any(|a| a.contains(&format!("path=\"{rel}\""))) {
            continue;
        }
        let p: PathBuf = root.join(rel);
        if !p.starts_with(root) {
            continue;
        }
        if p.is_file() {
            let Ok(body) = std::fs::read(&p) else { continue };
            if body.iter().take(4096).any(|b| *b == 0) {
                attached.push(format!("<file path=\"{rel}\">(binary file, {} bytes)</file>", body.len()));
                continue;
            }
            let text = String::from_utf8_lossy(&body);
            let take = text.len().min(budget);
            let mut cut = take;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            budget -= cut;
            let note = if cut < text.len() { "\n… (truncated)" } else { "" };
            attached.push(format!("<file path=\"{rel}\">\n{}{note}\n</file>", &text[..cut]));
        } else if p.is_dir() {
            let mut names: Vec<String> = std::fs::read_dir(&p)
                .map(|rd| rd.flatten().map(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    if e.path().is_dir() { format!("{n}/") } else { n }
                }).collect())
                .unwrap_or_default();
            names.sort();
            attached.push(format!("<directory path=\"{rel}\">\n{}\n</directory>", names.join("\n")));
        }
        if budget == 0 {
            break;
        }
    }
    if attached.is_empty() {
        text.to_string()
    } else {
        format!("{text}\n\n{}", attached.join("\n\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_mention_under_cursor() {
        let s: Vec<char> = "fix @src/ma now".chars().collect();
        assert_eq!(mention_at(&s, 11), Some((4, "src/ma".into())));
        assert_eq!(mention_at(&s, 3), None);
        let e: Vec<char> = "mail a@b".chars().collect();
        assert_eq!(mention_at(&e, 8), None);
    }

    #[test]
    fn expands_files_and_dirs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "fn a() {}").unwrap();
        let out = expand_mentions("look at @src/a.rs and @src, not @missing.txt", dir.path());
        assert!(out.contains("<file path=\"src/a.rs\">\nfn a() {}"));
        assert!(out.contains("<directory path=\"src\">\na.rs"));
        assert!(!out.contains("missing.txt\">"));
    }

    #[test]
    fn index_walks_and_ranks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("crates/ztui/src")).unwrap();
        std::fs::write(dir.path().join("crates/ztui/src/view.rs"), "").unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();
        let idx = Index::default();
        idx.ensure(dir.path());
        for _ in 0..200 {
            if idx.ready() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(idx.search("view", 3).first().map(String::as_str), Some("crates/ztui/src/view.rs"));
    }
}
