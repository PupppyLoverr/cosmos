//! Checkpoints — copy previous bytes before `edit`/`write` overwrite.
//! `/undo` or `cosmostui checkpoint restore --last` walks back one file.
//! Not git. Do not touch the user's index.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

pub fn dir(home: &Path, session: &str) -> PathBuf {
    home.join("ckpt").join(session)
}

fn next_seq(dir: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(dir) else {
        return 1;
    };
    rd.flatten()
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .split('-')
                .next()
                .and_then(|s| s.parse::<u64>().ok())
        })
        .max()
        .unwrap_or(0)
        + 1
}

/// Snapshot `path` if it exists. Returns the checkpoint path.
pub fn save(home: &Path, session: &str, path: &Path) -> Result<Option<PathBuf>> {
    if !path.is_file() {
        return Ok(None);
    }
    let dir = dir(home, session);
    fs::create_dir_all(&dir)?;
    let seq = next_seq(&dir);
    let base = path.file_name().unwrap_or_default().to_string_lossy();
    let dest = dir.join(format!("{seq}-{base}"));
    fs::copy(path, &dest)?;
    let meta = dir.join(format!("{seq}.meta"));
    fs::write(meta, path.to_string_lossy().as_bytes())?;
    Ok(Some(dest))
}

/// Restore the most recent checkpoint. Returns the restored workspace path.
pub fn restore_last(home: &Path, session: &str) -> Result<PathBuf> {
    let dir = dir(home, session);
    let Ok(rd) = fs::read_dir(&dir) else {
        bail!("no checkpoints");
    };
    let mut metas: Vec<(u64, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("meta") {
                return None;
            }
            let seq = p.file_stem()?.to_str()?.parse::<u64>().ok()?;
            Some((seq, p))
        })
        .collect();
    metas.sort_by_key(|(s, _)| *s);
    let Some((seq, meta)) = metas.pop() else {
        bail!("no checkpoints");
    };
    let target = PathBuf::from(fs::read_to_string(&meta)?);
    let base = target.file_name().unwrap_or_default().to_string_lossy();
    let src = dir.join(format!("{seq}-{base}"));
    if !src.is_file() {
        bail!("checkpoint blob missing: {}", src.display());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&src, &target)?;
    let _ = fs::remove_file(&src);
    let _ = fs::remove_file(&meta);
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_restore() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let f = ws.path().join("a.txt");
        fs::write(&f, "v1").unwrap();
        save(home.path(), "s1", &f).unwrap();
        fs::write(&f, "v2").unwrap();
        let restored = restore_last(home.path(), "s1").unwrap();
        assert_eq!(restored, f);
        assert_eq!(fs::read_to_string(&f).unwrap(), "v1");
    }
}
