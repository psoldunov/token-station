//! The record of what `setup` did, so `uninstall` removes exactly that.
//!
//! Nothing outside this file is ever deleted: `uninstall` walks the manifest and
//! stops. A missing manifest means nothing was installed by us.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic::write_atomic;
use crate::paths::Env;

/// Format of the manifest; bump on a breaking change.
pub const VERSION: u32 = 1;
/// File name under `$XDG_STATE_HOME/token-station`.
pub const FILE_NAME: &str = "install-manifest.json";

/// What `--claude-statusline` changed, and how to put it back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeRecord {
    pub settings: PathBuf,
    pub backup: PathBuf,
    /// The `statusLine` value before Token Station first touched it; `None` when
    /// the key was absent, in which case `uninstall` removes it again.
    pub previous: Option<serde_json::Value>,
    /// The command we wrote, so `uninstall` can tell ours from a later edit.
    pub command: String,
}

/// Everything one `setup` run created or changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: u32,
    /// The binary the installed files point at.
    pub exec: PathBuf,
    /// `kde`, `gnome` or `other`.
    pub desktop: String,
    pub installed_at: i64,
    /// Directories we own outright and may remove whole.
    pub dirs: Vec<PathBuf>,
    /// Individual files we wrote or copied.
    pub files: Vec<PathBuf>,
    pub systemd_unit: Option<PathBuf>,
    /// UUID of the extension `setup` enabled, if it did.
    pub gnome_extension: Option<String>,
    pub claude: Option<ClaudeRecord>,
}

impl Manifest {
    /// An empty manifest for `exec` on `desktop`.
    pub fn new(exec: &Path, desktop: &str, installed_at: i64) -> Manifest {
        Manifest {
            version: VERSION,
            exec: exec.to_path_buf(),
            desktop: desktop.to_string(),
            installed_at,
            dirs: Vec::new(),
            files: Vec::new(),
            systemd_unit: None,
            gnome_extension: None,
            claude: None,
        }
    }

    /// `$XDG_STATE_HOME/token-station/install-manifest.json`.
    pub fn path(env: &Env) -> PathBuf {
        env.state_home().join(crate::paths::APP_DIR).join(FILE_NAME)
    }

    /// Read a manifest, or `None` when there is none (or it is unreadable).
    pub fn load(path: &Path) -> Option<Manifest> {
        let json = std::fs::read_to_string(path).ok()?;
        match serde_json::from_str(&json) {
            Ok(manifest) => Some(manifest),
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "ignoring an unreadable manifest");
                None
            }
        }
    }

    /// Write the manifest atomically.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        write_atomic(path, &json)
    }

    /// Record a file this run created.
    pub fn add_file(&mut self, path: &Path) {
        if !self.files.iter().any(|known| known == path) {
            self.files.push(path.to_path_buf());
        }
    }

    /// Record a directory this run owns.
    pub fn add_dir(&mut self, path: &Path) {
        if !self.dirs.iter().any(|known| known == path) {
            self.dirs.push(path.to_path_buf());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(state: &str) -> Env {
        Env::from_map(&HashMap::from([
            ("HOME", "/home/u"),
            ("XDG_STATE_HOME", state),
        ]))
    }

    #[test]
    fn the_manifest_lives_under_the_state_home() {
        assert_eq!(
            Manifest::path(&env("/st")),
            Path::new("/st/token-station/install-manifest.json")
        );
        let default = Env::from_map(&HashMap::from([("HOME", "/home/u")]));
        assert_eq!(
            Manifest::path(&default),
            Path::new("/home/u/.local/state/token-station/install-manifest.json")
        );
    }

    #[test]
    fn a_manifest_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/install-manifest.json");
        let mut manifest = Manifest::new(
            Path::new("/opt/TokenStation.AppImage"),
            "kde",
            1_790_596_800,
        );
        manifest.add_dir(Path::new(
            "/data/plasma/plasmoids/dev.soldunov.tokenstation",
        ));
        manifest.add_file(Path::new(
            "/data/applications/dev.soldunov.TokenStation.desktop",
        ));
        manifest.systemd_unit = Some(PathBuf::from("/cfg/systemd/user/token-station.service"));
        manifest.gnome_extension = Some("token-station@soldunov.dev".into());
        manifest.claude = Some(ClaudeRecord {
            settings: PathBuf::from("/home/u/.claude/settings.json"),
            backup: PathBuf::from("/home/u/.claude/settings.json.token-station-backup"),
            previous: Some(serde_json::json!({"type": "command", "command": "starship"})),
            command: "'/opt/TokenStation.AppImage' statusline --wrap 'starship'".into(),
        });

        manifest.save(&path).unwrap();
        assert_eq!(Manifest::load(&path), Some(manifest));
    }

    #[test]
    fn camel_case_keys_keep_the_file_readable() {
        let manifest = Manifest::new(Path::new("/x"), "other", 7);
        let json: serde_json::Value = serde_json::to_value(&manifest).unwrap();
        assert_eq!(json["installedAt"], 7);
        assert_eq!(json["systemdUnit"], serde_json::Value::Null);
        assert_eq!(json["version"], VERSION);
    }

    #[test]
    fn recording_the_same_path_twice_keeps_one_entry() {
        let mut manifest = Manifest::new(Path::new("/x"), "kde", 0);
        manifest.add_file(Path::new("/a"));
        manifest.add_file(Path::new("/a"));
        manifest.add_dir(Path::new("/b"));
        manifest.add_dir(Path::new("/b"));
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.dirs.len(), 1);
    }

    #[test]
    fn a_missing_or_broken_manifest_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::load(&dir.path().join("absent.json")), None);
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{ not json").unwrap();
        assert_eq!(Manifest::load(&broken), None);
    }
}
