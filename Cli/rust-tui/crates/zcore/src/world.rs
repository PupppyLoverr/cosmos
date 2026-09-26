//! Per-session world.md snapshot. Survives compaction so the mission
//! (goal / files / tests) is never dropped. Written to
//! `$COSMOS_CLI_HOME/world/<session>.md` and mirrored via Meridian
//! `world.upsert` when the bridge is available.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Default)]
pub struct World {
    pub goal: String,
    pub status: String,
    pub decisions: Vec<String>,
    pub files: Vec<(String, String)>, // path, sha8
    pub tests: Vec<(String, String)>, // command, pass/fail
    pub open_questions: Vec<String>,
    pub plan: String,
    pub next: String,
    pub focus: String,
}

impl World {
    pub fn path(home: &Path, session: &str) -> PathBuf {
        home.join("world").join(format!("{session}.md"))
    }

    pub fn load(home: &Path, session: &str) -> Self {
        let p = Self::path(home, session);
        let Ok(text) = std::fs::read_to_string(p) else {
            return Self::default();
        };
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Self {
        let mut w = Self::default();
        let mut section = "";
        for line in text.lines() {
            if line == "# World" {
                section = "world";
                continue;
            }
            if line == "# Plan" {
                section = "plan";
                continue;
            }
            if line == "# Next" {
                section = "next";
                continue;
            }
            if let Some(v) = line.strip_prefix("goal: ") {
                w.goal = v.to_string();
            } else if let Some(v) = line.strip_prefix("status: ") {
                w.status = v.to_string();
            } else if let Some(v) = line.strip_prefix("focus: ") {
                w.focus = v.to_string();
            } else if let Some(v) = line.strip_prefix("decisions: ") {
                w.decisions = split_list(v);
            } else if let Some(v) = line.strip_prefix("files_touched: ") {
                w.files = v
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        if let Some((p, h)) = s.rsplit_once(' ') {
                            (p.to_string(), h.trim_matches(['(', ')']).to_string())
                        } else {
                            (s.to_string(), String::new())
                        }
                    })
                    .collect();
            } else if let Some(v) = line.strip_prefix("tests: ") {
                w.tests = v
                    .split(';')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        if let Some((c, r)) = s.split_once(" → ") {
                            (c.to_string(), r.to_string())
                        } else {
                            (s.to_string(), String::new())
                        }
                    })
                    .collect();
            } else if let Some(v) = line.strip_prefix("open_questions: ") {
                w.open_questions = split_list(v);
            } else if section == "plan" && !line.is_empty() {
                if !w.plan.is_empty() {
                    w.plan.push('\n');
                }
                w.plan.push_str(line);
            } else if section == "next" && !line.is_empty() {
                if !w.next.is_empty() {
                    w.next.push('\n');
                }
                w.next.push_str(line);
            }
        }
        w
    }

    pub fn render(&self) -> String {
        let files = self
            .files
            .iter()
            .map(|(p, h)| {
                if h.is_empty() {
                    p.clone()
                } else {
                    format!("{p} ({h})")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let tests = self
            .tests
            .iter()
            .map(|(c, r)| {
                if r.is_empty() {
                    c.clone()
                } else {
                    format!("{c} → {r}")
                }
            })
            .collect::<Vec<_>>()
            .join("; ");
        let mut out = String::from("# World\n");
        if !self.focus.is_empty() {
            out.push_str(&format!("focus: {}\n", self.focus));
        }
        out.push_str(&format!(
            "goal: {}\nstatus: {}\ndecisions: {}\nfiles_touched: {}\ntests: {}\nopen_questions: {}\n# Plan\n{}\n# Next\n{}\n",
            self.goal,
            self.status,
            self.decisions.join("; "),
            files,
            tests,
            self.open_questions.join("; "),
            self.plan,
            self.next,
        ));
        out
    }

    pub fn save(&self, home: &Path, session: &str) {
        let p = Self::path(home, session);
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, self.render());
    }

    pub fn touch_file(&mut self, path: &str, bytes: &[u8]) {
        let sha8 = sha8(bytes);
        if let Some(slot) = self.files.iter_mut().find(|(p, _)| p == path) {
            slot.1 = sha8;
        } else {
            self.files.push((path.to_string(), sha8));
        }
        self.status = format!("touched {path}");
    }

    pub fn touch_test(&mut self, command: &str, pass: bool) {
        let r = if pass { "pass" } else { "fail" }.to_string();
        if let Some(slot) = self.tests.iter_mut().find(|(c, _)| c == command) {
            slot.1 = r;
        } else {
            self.tests.push((command.to_string(), r));
        }
    }

    pub fn set_goal_if_empty(&mut self, text: &str) {
        if self.goal.is_empty() && !text.starts_with('[') && !text.starts_with('/') {
            self.goal = text.chars().take(400).collect();
        }
    }

    pub fn apply_compact_md(&mut self, md: &str) {
        let parsed = Self::parse(md);
        if !parsed.goal.is_empty() {
            self.goal = parsed.goal;
        }
        if !parsed.status.is_empty() {
            self.status = parsed.status;
        }
        if !parsed.files.is_empty() {
            self.files = parsed.files;
        }
        if !parsed.plan.is_empty() {
            self.plan = parsed.plan;
        }
        if !parsed.next.is_empty() {
            self.next = parsed.next;
        }
        if !parsed.focus.is_empty() {
            self.focus = parsed.focus;
        }
    }
}

pub fn sha8(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())[..8].to_string()
}

fn split_list(v: &str) -> Vec<String> {
    v.split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Boot-context fragment. Never drop goal/status when trimming.
pub fn boot_fragment(w: &World) -> String {
    if w.goal.is_empty() && w.files.is_empty() {
        return String::new();
    }
    format!("## World\n{}", w.render())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_file_touch() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = World::default();
        w.set_goal_if_empty("ship the compact engine");
        w.touch_file("src/main.rs", b"fn main() {}");
        w.save(dir.path(), "sess1");
        let loaded = World::load(dir.path(), "sess1");
        assert_eq!(loaded.goal, "ship the compact engine");
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files[0].0, "src/main.rs");
        assert_eq!(loaded.files[0].1.len(), 8);
    }
}
