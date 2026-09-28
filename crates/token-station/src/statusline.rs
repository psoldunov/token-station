//! `token-station statusline`: Claude Code's statusline hook.
//!
//! Claude Code runs this on every turn, so it must stay cheap: one short D-Bus call
//! with a hard timeout, and a file drop when the daemon is not there to answer.

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use crate::atomic::write_atomic;
use crate::dbus::client;
use crate::notify::compact_duration;
use crate::paths::Paths;

/// Largest statusline payload accepted on stdin.
pub const MAX_STDIN_BYTES: usize = 64 * 1024;
/// The daemon gets this long to answer before we fall back to the drop box.
pub const CALL_TIMEOUT: Duration = Duration::from_millis(150);

/// Read at most `cap` bytes; anything beyond is dropped.
pub fn read_capped(source: &mut impl Read, cap: usize) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    source.take(cap as u64 + 1).read_to_end(&mut buffer)?;
    buffer.truncate(cap);
    Ok(buffer)
}

/// The one-line summary printed when no wrapped command produces output.
///
/// Prefers the plan windows Claude Code reports; falls back to the model name.
pub fn compact_line(payload: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return String::new();
    };
    let limits = &value["rate_limits"];
    let parts: Vec<String> = [("five_hour", "5h"), ("seven_day", "7d")]
        .into_iter()
        .filter_map(|(key, label)| {
            let percent = limits[key]["used_percentage"].as_f64()?;
            Some(format!("{label} {}%", percent.round() as i64))
        })
        .collect();
    if !parts.is_empty() {
        return parts.join(" · ");
    }
    value["model"]["display_name"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `5h 34% · 7d 15% · resets in 2h 14m` — used when a reset time is known.
pub fn compact_line_with_reset(payload: &str, resets_in: Option<i64>) -> String {
    let base = compact_line(payload);
    match resets_in.filter(|s| *s > 0) {
        Some(seconds) if !base.is_empty() => {
            format!("{base} · resets in {}", compact_duration(seconds))
        }
        _ => base,
    }
}

/// Hand the payload to the daemon, with a short timeout.
pub async fn deliver(payload: &str) -> Result<(), String> {
    let connection = zbus::Connection::session()
        .await
        .map_err(|e| e.to_string())?;
    let proxy = client::connect(&connection)
        .await
        .map_err(|e| e.to_string())?;
    tokio::time::timeout(CALL_TIMEOUT, proxy.ingest_claude_statusline(payload))
        .await
        .map_err(|_| "timed out".to_string())?
        .map_err(|e| e.to_string())
}

/// Blocking wrapper: a one-thread runtime just for this call.
pub fn deliver_blocking(payload: &str) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(deliver(payload))
}

/// Leave the payload where the daemon will find it on its next tokens tick.
pub fn drop_to_disk(path: &Path, payload: &str) -> Result<(), String> {
    write_atomic(path, payload.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Run `sh -c command` with the same stdin and forward its stdout untouched.
pub fn run_wrapped(command: &str, stdin: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    if let Some(mut pipe) = child.stdin.take() {
        pipe.write_all(stdin)?;
    }
    let output = child.wait_with_output()?;
    Ok(output.stdout)
}

/// The command itself. Always succeeds, so Claude Code's statusline never breaks.
pub fn run(wrap: Option<&str>, paths: &Paths) -> std::io::Result<()> {
    let bytes = read_capped(&mut std::io::stdin().lock(), MAX_STDIN_BYTES)?;
    let payload = String::from_utf8_lossy(&bytes).into_owned();

    if !payload.trim().is_empty() {
        if let Err(error) = deliver_blocking(&payload) {
            tracing::debug!(%error, "daemon unreachable, leaving the statusline on disk");
            if let Err(error) = drop_to_disk(&paths.statusline_drop(), &payload) {
                tracing::debug!(%error, "cannot leave the statusline on disk");
            }
        }
    }

    let out = match wrap {
        Some(command) => run_wrapped(command, &bytes).unwrap_or_default(),
        None => format!("{}\n", compact_line(&payload)).into_bytes(),
    };
    std::io::stdout().lock().write_all(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "model": {"id": "claude-opus-5-5", "display_name": "Opus 5.5"},
        "rate_limits": {
            "five_hour": {"used_percentage": 34, "resets_at": 1790638199},
            "seven_day": {"used_percentage": 15.4, "resets_at": 1791003599}
        }
    }"#;

    #[test]
    fn reads_at_most_the_cap() {
        let big = vec![b'x'; 200];
        let read = read_capped(&mut big.as_slice(), 64).unwrap();
        assert_eq!(read.len(), 64);
        let small = read_capped(&mut b"hi".as_slice(), 64).unwrap();
        assert_eq!(small, b"hi");
    }

    #[test]
    fn compact_line_prefers_rate_limits() {
        assert_eq!(compact_line(SAMPLE), "5h 34% · 7d 15%");
    }

    #[test]
    fn compact_line_falls_back_to_the_model_name() {
        assert_eq!(
            compact_line(r#"{"model":{"display_name":"Opus 5.5"}}"#),
            "Opus 5.5"
        );
        assert_eq!(compact_line("{}"), "");
        assert_eq!(compact_line("not json"), "");
    }

    #[test]
    fn partial_rate_limits_still_render() {
        assert_eq!(
            compact_line(r#"{"rate_limits":{"seven_day":{"used_percentage":7}}}"#),
            "7d 7%"
        );
    }

    #[test]
    fn reset_suffix_is_optional() {
        assert_eq!(
            compact_line_with_reset(SAMPLE, Some(8_040)),
            "5h 34% · 7d 15% · resets in 2h 14m"
        );
        assert_eq!(compact_line_with_reset(SAMPLE, None), "5h 34% · 7d 15%");
        assert_eq!(compact_line_with_reset("{}", Some(60)), "");
    }

    #[test]
    fn dropping_to_disk_creates_the_runtime_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("token-station/claude-statusline.json");
        drop_to_disk(&path, SAMPLE).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SAMPLE);
    }

    #[test]
    fn wrapping_passes_stdin_through_and_returns_stdout() {
        let out = run_wrapped("cat", SAMPLE.as_bytes()).unwrap();
        assert_eq!(out, SAMPLE.as_bytes());
        assert_eq!(run_wrapped("printf hi", b"").unwrap(), b"hi");
    }

    #[test]
    fn delivering_without_a_bus_fails_quickly() {
        // SAFETY: single-threaded assertion on this process' own environment.
        let started = std::time::Instant::now();
        let error = deliver_blocking_with_address("unix:path=/nonexistent/token-station-test");
        assert!(error.is_err());
        assert!(started.elapsed() < Duration::from_secs(2), "must stay fast");
    }

    fn deliver_blocking_with_address(address: &str) -> Result<(), String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        runtime.block_on(async {
            let connection = zbus::connection::Builder::address(address)
                .map_err(|e| e.to_string())?
                .build()
                .await
                .map_err(|e| e.to_string())?;
            let proxy = client::connect(&connection)
                .await
                .map_err(|e| e.to_string())?;
            proxy
                .ingest_claude_statusline("{}")
                .await
                .map_err(|e| e.to_string())
        })
    }
}
