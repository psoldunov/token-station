//! Spawns `<binary> app-server` and wires an [`AppServerClient`] to its stdio.

use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

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
pub fn spawn(binary: &Path) -> Result<AppServerProcess, SpawnError> {
    let mut child = Command::new(binary)
        .arg("app-server")
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

    #[tokio::test]
    async fn spawn_fails_for_missing_binary() {
        let result = spawn(Path::new("/definitely/does/not/exist/codex"));
        assert!(matches!(result, Err(SpawnError::Spawn { .. })));
    }
}
