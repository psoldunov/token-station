//! `token-station statusline`: Claude Code's statusline hook.
//!
//! Claude Code runs this on every turn, so it must stay cheap: one short D-Bus call
//! with a hard timeout, and a file drop when the daemon is not there to answer. It
//! also must never fail — a non-zero exit turns the user's status line into an
//! error message — so every step here degrades instead of returning.

use std::io::{Read, Write};
use std::path::Path;
use std::process::Child;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::atomic::write_atomic;
use crate::dbus::client;
use crate::notify::compact_duration;
use crate::output;
use crate::paths::Paths;

/// Largest payload handed to the daemon, over the bus or through the drop box.
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
/// Ceiling on what is read from stdin, so a runaway writer cannot exhaust memory.
/// The wrapped command sees all of it; only the daemon's copy is capped.
pub const MAX_STDIN_BYTES: usize = 8 * 1024 * 1024;
/// The daemon gets this long to answer before we fall back to the drop box.
pub const CALL_TIMEOUT: Duration = Duration::from_millis(150);
/// A wrapped status line gets this long before it is killed.
pub const WRAP_TIMEOUT: Duration = Duration::from_secs(5);
/// How often a running wrapped command is checked on.
const WRAP_POLL: Duration = Duration::from_millis(5);

/// Read at most `cap` bytes; anything beyond is dropped.
pub fn read_capped(source: &mut impl Read, cap: usize) -> std::io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    source.take(cap as u64 + 1).read_to_end(&mut buffer)?;
    buffer.truncate(cap);
    Ok(buffer)
}

/// The longest prefix of `text` that is at most `cap` bytes and still valid UTF-8.
pub fn truncate_to(text: &str, cap: usize) -> &str {
    if text.len() <= cap {
        return text;
    }
    let mut end = cap;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default()
}

/// The one-line summary printed when no wrapped command produces output.
///
/// Prefers the plan windows Claude Code reports; falls back to the model name.
pub fn compact_line(payload: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return String::new();
    };
    let parts: Vec<String> = [("five_hour", "5h"), ("seven_day", "7d")]
        .into_iter()
        .filter_map(|(key, label)| {
            let percent = value
                .get("rate_limits")?
                .get(key)?
                .get("used_percentage")?
                .as_f64()?;
            Some(format!("{label} {}%", percent.round() as i64))
        })
        .collect();
    if !parts.is_empty() {
        return parts.join(" · ");
    }
    value
        .get("model")
        .and_then(|model| model.get("display_name"))
        .and_then(serde_json::Value::as_str)
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

/// Give the payload to the daemon, or to the drop box when it is not listening.
fn deliver_or_drop(payload: &str, paths: &Paths) {
    if payload.trim().is_empty() {
        return;
    }
    let capped = truncate_to(payload, MAX_PAYLOAD_BYTES);
    if let Err(error) = deliver_blocking(capped) {
        tracing::debug!(%error, "daemon unreachable, leaving the statusline on disk");
        if let Err(error) = drop_to_disk(&paths.statusline_drop(), capped) {
            tracing::debug!(%error, "cannot leave the statusline on disk");
        }
    }
}

/// A wrapped status line, running while the D-Bus call happens.
pub struct Wrapped {
    child: Child,
    stdout: Option<JoinHandle<Vec<u8>>>,
}

/// Start `sh -c command` with `stdin` on its input, reading its output in the
/// background so neither pipe can deadlock the caller.
pub fn spawn_wrapped(command: &str, stdin: &[u8]) -> Option<Wrapped> {
    use std::process::{Command, Stdio};
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!(%error, "cannot start the wrapped status line");
            return None;
        }
    };
    if let Some(mut pipe) = child.stdin.take() {
        let payload = stdin.to_vec();
        // A command that ignores its input closes the pipe early; EPIPE is normal.
        std::thread::spawn(move || {
            let _ = pipe.write_all(&payload);
        });
    }
    let stdout = child.stdout.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = pipe.read_to_end(&mut buffer);
            buffer
        })
    });
    Some(Wrapped { child, stdout })
}

/// Wait for the wrapped command, killing it after `timeout`, and take its output.
pub fn finish_wrapped(mut wrapped: Wrapped, timeout: Duration) -> Vec<u8> {
    let deadline = Instant::now() + timeout;
    while matches!(wrapped.child.try_wait(), Ok(None)) {
        if Instant::now() >= deadline {
            tracing::warn!("the wrapped status line took too long; killing it");
            let _ = wrapped.child.kill();
            let _ = wrapped.child.wait();
            break;
        }
        std::thread::sleep(WRAP_POLL);
    }
    wrapped
        .stdout
        .take()
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
}

/// Run `sh -c command` with `stdin` and return its stdout, whatever it exits with.
pub fn run_wrapped(command: &str, stdin: &[u8], timeout: Duration) -> Vec<u8> {
    match spawn_wrapped(command, stdin) {
        Some(wrapped) => finish_wrapped(wrapped, timeout),
        None => Vec::new(),
    }
}

/// The command itself. Always succeeds, so Claude Code's statusline never breaks.
pub fn run(wrap: Option<&str>, paths: &Paths) {
    let bytes = read_capped(&mut std::io::stdin().lock(), MAX_STDIN_BYTES).unwrap_or_default();
    let payload = String::from_utf8_lossy(&bytes).into_owned();

    // Started first, so the bus round trip overlaps with the wrapped command.
    let wrapped = wrap.and_then(|command| spawn_wrapped(command, &bytes));
    deliver_or_drop(&payload, paths);

    let out = match wrapped {
        Some(wrapped) => finish_wrapped(wrapped, WRAP_TIMEOUT),
        None => format!("{}\n", compact_line(&payload)).into_bytes(),
    };
    output::bytes(&out);
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
    fn truncation_keeps_whole_characters() {
        assert_eq!(truncate_to("abc", 10), "abc");
        assert_eq!(truncate_to("abcdef", 3), "abc");
        // "·" is two bytes: cutting at 2 must drop it whole.
        assert_eq!(truncate_to("a·b", 2), "a");
        assert_eq!(truncate_to("a·b", 3), "a·");
        assert_eq!(truncate_to("·", 1), "");
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
        // Wrong shapes must not panic on an index that is not there.
        assert_eq!(compact_line(r#"{"rate_limits":[1,2]}"#), "");
        assert_eq!(compact_line(r#"{"model":"opus"}"#), "");
        assert_eq!(compact_line("[1,2,3]"), "");
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
        let out = run_wrapped("cat", SAMPLE.as_bytes(), WRAP_TIMEOUT);
        assert_eq!(out, SAMPLE.as_bytes());
        assert_eq!(run_wrapped("printf hi", b"", WRAP_TIMEOUT), b"hi");
    }

    #[test]
    fn a_wrapped_command_that_ignores_its_input_still_prints() {
        // `printf` never reads stdin, so the writer thread sees EPIPE.
        let big = vec![b'x'; 512 * 1024];
        assert_eq!(run_wrapped("printf ready", &big, WRAP_TIMEOUT), b"ready");
    }

    #[test]
    fn a_payload_over_the_bus_limit_still_reaches_the_wrapped_command() {
        let big = vec![b'y'; MAX_PAYLOAD_BYTES * 2];
        let out = run_wrapped("wc -c", &big, WRAP_TIMEOUT);
        let counted: usize = String::from_utf8_lossy(&out)
            .trim()
            .parse()
            .expect("a count");
        assert_eq!(counted, big.len());
    }

    #[test]
    fn a_hanging_wrapped_command_is_killed_and_its_output_kept() {
        let started = Instant::now();
        let out = run_wrapped("printf slow; sleep 30", b"", Duration::from_millis(200));
        assert!(started.elapsed() < Duration::from_secs(5), "it was killed");
        assert_eq!(out, b"slow");
    }

    #[test]
    fn a_wrapped_command_that_cannot_start_is_not_fatal() {
        assert!(run_wrapped("exit 7", b"", WRAP_TIMEOUT).is_empty());
    }

    #[test]
    fn delivering_without_a_bus_fails_quickly() {
        let started = Instant::now();
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
