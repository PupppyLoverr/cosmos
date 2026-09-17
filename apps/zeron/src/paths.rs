//! Application storage paths. Provider credentials keep their own locations.

use std::ffi::OsString;
use std::path::PathBuf;

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
        // (sign-in, device identity, sessions, prefs) on first launch.
        if !dir.exists() {
            for var in ["HOME", "USERPROFILE"] {
                let Some(home) = env(var).filter(|v| !v.is_empty()) else {
                    continue;
                };
                let old = PathBuf::from(home).join(".zeron");
                if old.exists() && std::fs::rename(&old, &dir).is_ok() {
                    eprintln!("migrated data dir {} -> {}", old.display(), dir.display());
                    break;
                }
            }
        }
        dir
    }
    #[cfg(not(windows))]
    {
        let home = PathBuf::from(env("HOME").expect("HOME not set"));
        let dir = home.join(".zeron");
        // One-shot 0.2.0 migration: adopt the pre-rename data dir.
        if !dir.exists() {
            let old = home.join(".comet-native");
            if old.exists() && std::fs::rename(&old, &dir).is_ok() {
                eprintln!("migrated data dir {} -> {}", old.display(), dir.display());
            }
        }
        dir
    }
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
}
