//! Crash-safe file replacement: write a sibling temp file, fsync, rename.

use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Makes two writers inside one process pick different temp names.
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Why a path must not be replaced where it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unwritable {
    /// A symlink: whoever owns the link owns the file (Nix, home-manager, dotfiles).
    Symlink,
    /// Nobody has the write bit, so the file is meant to stay as it is.
    ReadOnly,
}

/// Check that replacing `path` is allowed; a path that does not exist yet is fine.
///
/// Renaming over a symlink silently replaces the link, which is how a
/// declaratively managed file (a `/nix/store` symlink, say) gets clobbered.
///
/// # Errors
///
/// Returns [`Unwritable::Symlink`] when `path` is a symlink and
/// [`Unwritable::ReadOnly`] when it is a file with no write bit set.
pub fn check_replaceable(path: &Path) -> Result<(), Unwritable> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        return Err(Unwritable::Symlink);
    }
    if meta.is_file() && meta.permissions().mode() & 0o222 == 0 {
        return Err(Unwritable::ReadOnly);
    }
    Ok(())
}

/// Replace `path` with `bytes`, creating parent directories as needed.
///
/// The rename is atomic, so readers see either the old or the new file. An
/// existing file keeps its permission bits, so rewriting a `0600` file does not
/// widen it to the default `0644`.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] when the parent directories cannot
/// be created, the temporary file cannot be created, written or fsynced, or the
/// rename over `path` fails. The temporary file is removed on every failure.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(temp_name(path));

    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        if let Some(mode) = existing_mode(path) {
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// A temp name no other writer of `path` can collide with.
fn temp_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(".{name}.{}.{sequence}.tmp", std::process::id())
}

/// Permission bits of the file being replaced, if there is one.
fn existing_mode(path: &Path) -> Option<u32> {
    let meta = std::fs::metadata(path).ok()?;
    meta.is_file().then(|| meta.permissions().mode() & 0o7777)
}

/// Modification time of `path` in whole Unix seconds, if it exists.
#[must_use]
pub fn modified_secs(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    i64::try_from(since).ok()
}

/// Device, inode, size and nanosecond mtime of one directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileId {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_ns: i64,
}

impl FileId {
    fn of(meta: &std::fs::Metadata) -> FileId {
        FileId {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.size(),
            mtime: meta.mtime(),
            mtime_ns: meta.mtime_nsec(),
        }
    }
}

/// Identity of a path, taking the link and its target together.
///
/// A whole-second mtime is useless on NixOS, where every `/nix/store` file is
/// stamped with second 1: a `home-manager switch` only moves the symlink, and
/// only the inode of the target tells the two generations apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    link: FileId,
    target: Option<FileId>,
}

/// Fingerprint `path`, or `None` when nothing is there.
#[must_use]
pub fn fingerprint(path: &Path) -> Option<Fingerprint> {
    let link = std::fs::symlink_metadata(path).ok()?;
    let target = if link.file_type().is_symlink() {
        std::fs::metadata(path).ok().map(|m| FileId::of(&m))
    } else {
        None
    };
    Some(Fingerprint {
        link: FileId::of(&link),
        target,
    })
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
    fn temp_names_are_unique_per_call() {
        let path = Path::new("/tmp/settings.json");
        let first = temp_name(path);
        let second = temp_name(path);
        assert_ne!(first, second);
        assert!(first.starts_with(".settings.json."), "{first}");
        assert_eq!(
            Path::new(&first)
                .extension()
                .and_then(std::ffi::OsStr::to_str),
            Some("tmp"),
            "{first}"
        );
    }

    #[test]
    fn an_existing_files_mode_survives_the_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_atomic(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&path, b"{\"a\":1}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "0600 must not widen to 0644");
    }

    #[test]
    fn symlinks_and_read_only_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent.toml");
        assert_eq!(check_replaceable(&missing), Ok(()));

        let real = dir.path().join("store.toml");
        std::fs::write(&real, "x").unwrap();
        assert_eq!(check_replaceable(&real), Ok(()));

        let link = dir.path().join("linked.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(check_replaceable(&link), Err(Unwritable::Symlink));

        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert_eq!(check_replaceable(&real), Err(Unwritable::ReadOnly));
    }

    #[test]
    fn modified_secs_reports_missing_file_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x");
        assert!(modified_secs(&path).is_none());
        write_atomic(&path, b"1").unwrap();
        assert!(modified_secs(&path).unwrap() > 1_700_000_000);
    }

    #[test]
    fn a_fingerprint_follows_the_symlink_target() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("generation-1.toml");
        let second = dir.path().join("generation-2.toml");
        std::fs::write(&first, "a = 1\n").unwrap();
        std::fs::write(&second, "a = 1\n").unwrap();
        // Both targets carry the same whole-second mtime, as /nix/store does.
        let stamp = std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1));
        for path in [&first, &second] {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(stamp)
                .unwrap();
        }

        let link = dir.path().join("config.toml");
        std::os::unix::fs::symlink(&first, &link).unwrap();
        let before = fingerprint(&link).unwrap();

        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&second, &link).unwrap();
        assert_ne!(
            fingerprint(&link).unwrap(),
            before,
            "a switched symlink is a change"
        );
    }

    #[test]
    fn a_fingerprint_notices_a_rewritten_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(fingerprint(&path).is_none());
        write_atomic(&path, b"a = 1\n").unwrap();
        let before = fingerprint(&path).unwrap();
        write_atomic(&path, b"a = 22\n").unwrap();
        assert_ne!(fingerprint(&path).unwrap(), before);
    }
}
