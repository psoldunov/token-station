//! Spawns `<binary> app-server` and wires an [`AppServerClient`] to its stdio.

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use ts_core::discovery::{SearchEnv, child_path_in};

use super::rpc::AppServerClient;

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("cannot start {path}: {source}")]
    Spawn {
        path: String,
        source: std::io::Error,
    },
    #[error("child process did not expose its stdio")]
    NoStdio,
}

/// A running `app-server` child plus the client talking to it over stdio.
///
/// Dropping this kills the child (`kill_on_drop`).
pub struct AppServerProcess {
    pub child: Child,
    pub client: AppServerClient,
}

/// Spawn `binary app-server` with piped stdio; forward stderr to `tracing::debug!`.
///
/// The child is given the `PATH` from [`child_path_in`] rather than the one
/// this process inherited: a `codex` installed by a JavaScript package manager
/// is a script that needs its interpreter on `PATH`, and the daemon's own is
/// whatever Finder or systemd handed it.
///
/// # Errors
///
/// Returns [`SpawnError::Spawn`] when the binary cannot be started and
/// [`SpawnError::NoStdio`] when the child's pipes are missing.
pub fn spawn(binary: &Path, env: &SearchEnv) -> Result<AppServerProcess, SpawnError> {
    let mut child = Command::new(binary)
        .arg("app-server")
        .env("PATH", child_path_in(binary, env))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| SpawnError::Spawn {
            path: binary.display().to_string(),
            source,
        })?;

    let stdin = child.stdin.take().ok_or(SpawnError::NoStdio)?;
    let stdout = child.stdout.take().ok_or(SpawnError::NoStdio)?;
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(forward_stderr(stderr));
    }

    let client = AppServerClient::connect(stdout, stdin);
    Ok(AppServerProcess { child, client })
}

async fn forward_stderr(stderr: tokio::process::ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "ts_codex::app_server::stderr", "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> SearchEnv {
        SearchEnv {
            path: Some("/usr/bin:/bin".into()),
            home: std::path::PathBuf::from("/home/u"),
            user: None,
        }
    }

    #[tokio::test]
    async fn spawn_fails_for_missing_binary() {
        let result = spawn(Path::new("/definitely/does/not/exist/codex"), &env());
        assert!(matches!(result, Err(SpawnError::Spawn { .. })));
    }

    #[tokio::test]
    async fn the_child_gets_a_path_that_can_find_an_interpreter() {
        // What the child would be given: the inherited entries first, then the
        // binary's own directory, which is where a package-manager install puts
        // the interpreter its script needs.
        let binary = Path::new("/home/u/.local/bin/codex");
        let path = child_path_in(binary, &env());
        let built = path.to_string_lossy().into_owned();
        assert!(built.starts_with("/usr/bin:/bin:"), "{built}");
        assert!(built.contains("/home/u/.local/bin"), "{built}");
    }
}
