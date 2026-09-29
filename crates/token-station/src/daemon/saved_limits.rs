//! The plan-limit state each provider asks to keep across a restart, on disk.
//!
//! The daemon only moves bytes: what goes into the file, and how much of what
//! comes back out is still believable, is each provider's own business (see
//! [`ts_core::Provider::saved_state`]). Every read and write runs off the
//! runtime's threads.

use std::path::PathBuf;

use crate::atomic::write_atomic;

/// Read `path` back as JSON: `None` when there is no file, or none worth
/// handing to the provider.
pub async fn load(path: PathBuf) -> Option<serde_json::Value> {
    let read = tokio::task::spawn_blocking(move || std::fs::read(&path)).await;
    let bytes = match read {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Ok(Err(error)) => {
            tracing::warn!(%error, "cannot read the saved plan limits");
            return None;
        }
        Err(error) => {
            tracing::warn!(%error, "the saved plan limits read did not run");
            return None;
        }
    };
    serde_json::from_slice(&bytes)
        .inspect_err(|error| tracing::warn!(%error, "ignoring unreadable saved plan limits"))
        .ok()
}

/// `saved` as the bytes to write, or `None` when it cannot be serialised.
pub fn encode(saved: &serde_json::Value) -> Option<Vec<u8>> {
    serde_json::to_vec_pretty(saved)
        .inspect_err(|error| tracing::warn!(%error, "cannot serialise the saved plan limits"))
        .ok()
}

/// Replace `path` with `bytes`; whether the write landed.
pub async fn store(path: PathBuf, bytes: Vec<u8>) -> bool {
    match tokio::task::spawn_blocking(move || write_atomic(&path, &bytes)).await {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            tracing::warn!(%error, "cannot save the plan limits");
            false
        }
        Err(error) => {
            tracing::warn!(%error, "the saved plan limits write did not run");
            false
        }
    }
}
