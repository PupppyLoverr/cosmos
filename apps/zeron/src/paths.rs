//! Application storage paths. Provider credentials keep their own locations.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub fn data_dir() -> PathBuf {
    resolve_data_dir(|name| std::env::var_os(name))
}

fn resolve_data_dir(mut env: impl FnMut(&str) -> Option<OsString>) -> PathBuf {
    if let Some(dir) = env("ZERON_DATA_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(windows)]
    {
        // Explorer does not set HOME. Do not let a shell-specific HOME select
        // a different workspace from a desktop launch, or migrate credentials
        // between Unix-style and native Windows directories implicitly.
        let local = env("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                env("USERPROFILE")
                    .filter(|value| !value.is_empty())
                    .map(|home| PathBuf::from(home).join("AppData").join("Local"))
            })
            .expect("LOCALAPPDATA and USERPROFILE not set; set ZERON_DATA_DIR");
        let dir = local.join("Zeron");
        // One-shot migration: adopt the pre-native-Windows data dir
        // (sign-in, device identity, sessions, prefs).
        for var in ["HOME", "USERPROFILE"] {
            let Some(home) = env(var).filter(|v| !v.is_empty()) else {
                continue;
            };
            let old = PathBuf::from(home).join(".zeron");
            if old.exists() {
                adopt_legacy_data_dir(&old, &dir);
                break;
            }
        }
        dir
    }
    #[cfg(not(windows))]
    {
        let home = PathBuf::from(env("HOME").expect("HOME not set"));
        let dir = home.join(".zeron");
        // One-shot 0.2.0 migration: adopt the pre-rename data dir.
        let old = home.join(".comet-native");
        if old.exists() {
            adopt_legacy_data_dir(&old, &dir);
        }
        dir
    }
}

/// One-shot data-dir adoption shared by the pre-rename migrations. Plain
/// case: `old` renames into place. When `dir` already exists — a post-rename
/// binary created it before this migration shipped, permanently orphaning
/// the established home under the old `!dir.exists()` guard — a strictly
/// larger legacy store means `old` holds the install's real sessions: `dir`
/// is parked as `*.pre-migration-*` (never deleted — its contents stay
/// recoverable) and `old` renames over. Otherwise both are left as-is.
/// A failed rename (a still-running instance holding files open) falls back
/// to the current layout so a later launch can retry.
fn adopt_legacy_data_dir(old: &Path, dir: &Path) {
    if !dir.exists() {
        if std::fs::rename(old, dir).is_ok() {
            eprintln!("migrated data dir {} -> {}", old.display(), dir.display());
        }
        return;
    }
    if profile_store_bytes(old) <= profile_store_bytes(dir) {
        return;
    }
    let aside = dir.with_file_name(format!(
        "{}.pre-migration-{}",
        dir.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "data".into()),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|at| at.as_millis())
            .unwrap_or(0)
    ));
    if std::fs::rename(dir, &aside).is_err() {
        return;
    }
    if std::fs::rename(old, dir).is_ok() {
        eprintln!(
            "migrated data dir {} -> {} (previous dir moved to {})",
            old.display(),
            dir.display(),
            aside.display()
        );
    } else {
        let _ = std::fs::rename(&aside, dir);
    }
}

/// Total bytes of every profile store (`docs.sqlite3*`) under `root` — the
/// signal for which side of a split data dir actually holds the sessions.
fn profile_store_bytes(root: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                pending.push(entry.path());
            } else if entry
                .file_name()
                .to_string_lossy()
                .starts_with("docs.sqlite3")
            {
                total += meta.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(vars: &[(&str, &str)]) -> PathBuf {
        resolve_data_dir(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.into())
        })
    }

    #[test]
    fn explicit_data_dir_needs_no_home() {
        assert_eq!(
            resolve(&[("ZERON_DATA_DIR", "custom data")]),
            PathBuf::from("custom data")
        );
    }

    #[cfg(windows)]
    #[test]
    fn explorer_launch_without_home_uses_local_app_data() {
        assert_eq!(
            resolve(&[("LOCALAPPDATA", r"C:\Users\Test User\AppData\Local")]),
            PathBuf::from(r"C:\Users\Test User\AppData\Local\Zeron"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_profile_fallback_handles_unicode_and_apostrophes() {
        assert_eq!(
            resolve(&[("USERPROFILE", r"C:\Users\O'Brien 日本語")]),
            PathBuf::from(r"C:\Users\O'Brien 日本語\AppData\Local\Zeron"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_default_does_not_depend_on_shell_home() {
        assert_eq!(
            resolve(&[("HOME", r"D:\msys-home"), ("LOCALAPPDATA", r"C:\Local")]),
            PathBuf::from(r"C:\Local\Zeron"),
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_migrates_home_dot_zeron_once() {
        let tmp = std::env::temp_dir().join(format!("zeron-paths-migrate-{}", std::process::id()));
        let home = tmp.join("home");
        let local = tmp.join("local");
        std::fs::create_dir_all(home.join(".zeron")).unwrap();
        std::fs::create_dir_all(&local).unwrap();
        let vars = [
            ("HOME", home.to_str().unwrap()),
            ("USERPROFILE", home.to_str().unwrap()),
            ("LOCALAPPDATA", local.to_str().unwrap()),
        ];
        let dir = resolve(&vars);
        assert_eq!(dir, local.join("Zeron"));
        assert!(dir.exists());
        assert!(!home.join(".zeron").exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_native_dir_wins_over_dot_zeron() {
        let tmp = std::env::temp_dir().join(format!("zeron-paths-nowin-{}", std::process::id()));
        let home = tmp.join("home");
        let local = tmp.join("local");
        std::fs::create_dir_all(home.join(".zeron")).unwrap();
        std::fs::create_dir_all(local.join("Zeron")).unwrap();
        let vars = [
            ("USERPROFILE", home.to_str().unwrap()),
            ("LOCALAPPDATA", local.to_str().unwrap()),
        ];
        let dir = resolve(&vars);
        assert_eq!(dir, local.join("Zeron"));
        assert!(home.join(".zeron").exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[cfg(windows)]
    fn write_store(root: &Path, bytes: usize) {
        let dir = root.join("profiles").join("local");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("docs.sqlite3"), vec![0u8; bytes]).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_larger_legacy_store_is_adopted_over_fresh_target() {
        // The split-brain this migration exists for: a pre-migration native
        // build had already created %LOCALAPPDATA%\Zeron, so the old
        // `!dir.exists()` guard never adopted the established ~/.zeron.
        let tmp = std::env::temp_dir().join(format!("zeron-paths-split-{}", std::process::id()));
        let home = tmp.join("home");
        let local = tmp.join("local");
        write_store(&home.join(".zeron"), 4096);
        write_store(&local.join("Zeron"), 64);
        let vars = [
            ("USERPROFILE", home.to_str().unwrap()),
            ("LOCALAPPDATA", local.to_str().unwrap()),
        ];
        let dir = resolve(&vars);
        assert_eq!(dir, local.join("Zeron"));
        assert_eq!(
            std::fs::metadata(dir.join("profiles/local/docs.sqlite3"))
                .unwrap()
                .len(),
            4096,
            "adopted dir must be the legacy one"
        );
        assert!(!home.join(".zeron").exists());
        // The newer dir was parked, not deleted.
        let parked: Vec<_> = std::fs::read_dir(&local)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("Zeron.pre-migration-")
            })
            .collect();
        assert_eq!(parked.len(), 1);
        assert_eq!(
            std::fs::metadata(parked[0].path().join("profiles/local/docs.sqlite3"))
                .unwrap()
                .len(),
            64
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[cfg(windows)]
    #[test]
    fn windows_richer_target_store_keeps_legacy_in_place() {
        let tmp = std::env::temp_dir().join(format!("zeron-paths-rich-{}", std::process::id()));
        let home = tmp.join("home");
        let local = tmp.join("local");
        write_store(&home.join(".zeron"), 64);
        write_store(&local.join("Zeron"), 4096);
        let vars = [
            ("USERPROFILE", home.to_str().unwrap()),
            ("LOCALAPPDATA", local.to_str().unwrap()),
        ];
        let dir = resolve(&vars);
        assert_eq!(dir, local.join("Zeron"));
        assert!(home.join(".zeron").exists(), "legacy left untouched");
        assert!(
            std::fs::read_dir(&local).unwrap().flatten().all(|e| !e
                .file_name()
                .to_string_lossy()
                .starts_with("Zeron.pre-migration-")),
            "no park dir created"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }
}
