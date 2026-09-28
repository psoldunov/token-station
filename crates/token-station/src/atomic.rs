//! Crash-safe file replacement: write a sibling temp file, fsync, rename.

use std::io::Write;
use std::path::Path;

/// Replace `path` with `bytes`, creating parent directories as needed.
///
/// The rename is atomic, so readers see either the old or the new file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let temp = parent.join(format!(".{name}.{}.tmp", std::process::id()));

    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Modification time of `path` in whole Unix seconds, if it exists.
pub fn modified_secs(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    i64::try_from(since).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_directories_and_replaces_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deep/file.json");
        write_atomic(&path, b"first").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");
        write_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
    }

    #[test]
    fn leaves_no_temp_files_behind() {
        let dir = tempfile::tempdir().unwrap();
        write_atomic(&dir.path().join("a.json"), b"{}").unwrap();
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["a.json".to_string()]);
    }

    #[test]
    fn modified_secs_reports_missing_file_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x");
        assert!(modified_secs(&path).is_none());
        write_atomic(&path, b"1").unwrap();
        assert!(modified_secs(&path).unwrap() > 1_700_000_000);
    }
}
