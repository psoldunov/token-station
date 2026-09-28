//! Incremental scanner for Codex rollout JSONL files.
//!
//! Layout: `<home>/sessions/YYYY/MM/DD/rollout-*.jsonl` and
//! `<home>/archived_sessions/*.jsonl`. Each line is
//! `{"timestamp","type","payload"}`; only `turn_context` (current model) and
//! `event_msg` with `payload.type == "token_count"` (usage) matter here.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use ts_core::tokens::{TokenCounts, TokenEvent};

use crate::dto::{RateLimitSnapshotDto, RateLimitWindowDto};

const FIRST_SCAN_MAX_AGE_DAYS: u64 = 8;
const DEFAULT_MODEL: &str = "gpt-5-codex";

/// Everything a fresh scan pass produced.
#[derive(Debug, Default)]
pub struct ScanOutput {
    pub events: Vec<TokenEvent>,
    /// The most recent `rate_limits` snapshot seen across every file, with its timestamp.
    pub latest_rate_limits: Option<(i64, RateLimitSnapshotDto)>,
}

/// Per-file read state, keyed by `(dev, ino)`: a session file moved from
/// `sessions/**` to `archived_sessions/` keeps the same inode, so keying on
/// the path (which changes) would otherwise make the move look like a brand
/// new, unscanned file and double-count its events.
#[derive(Debug, Clone, Default)]
struct FileState {
    offset: u64,
    /// The file's name at the time `offset` was recorded; used to build a
    /// dedup key that survives the sessions/archived_sessions move (the full
    /// path does not).
    file_name: String,
    last_seen_total: Option<i64>,
    model: Option<String>,
}

/// Stateful, incremental rollout reader. Owns no async runtime; run its
/// `scan` inside `spawn_blocking`.
#[derive(Debug)]
pub struct RolloutScanner {
    files: HashMap<(u64, u64), FileState>,
    first_scan: bool,
}

impl Default for RolloutScanner {
    fn default() -> RolloutScanner {
        RolloutScanner::new()
    }
}

impl RolloutScanner {
    pub fn new() -> RolloutScanner {
        RolloutScanner {
            files: HashMap::new(),
            first_scan: true,
        }
    }

    /// Scan every rollout file under `homes`, returning newly discovered
    /// events. State for inodes not seen in this pass (files removed since
    /// the last scan) is pruned so it never grows unbounded.
    pub fn scan(&mut self, homes: &[PathBuf], now: i64) -> ScanOutput {
        let mut out = ScanOutput::default();
        let min_mtime = if self.first_scan {
            Some(SystemTime::now() - Duration::from_secs(FIRST_SCAN_MAX_AGE_DAYS * 86_400))
        } else {
            None
        };
        let mut seen: HashSet<(u64, u64)> = HashSet::new();

        for home in homes {
            for path in discover_files(home) {
                let Ok(metadata) = std::fs::metadata(&path) else {
                    continue;
                };
                let key = (metadata.dev(), metadata.ino());
                seen.insert(key);
                if let Some(min) = min_mtime {
                    if !self.files.contains_key(&key) && !is_recent(&path, min) {
                        continue;
                    }
                }
                self.scan_file(&path, key, metadata.len(), now, &mut out);
            }
        }
        self.first_scan = false;
        self.files.retain(|key, _| seen.contains(key));
        out
    }

    fn scan_file(
        &mut self,
        path: &Path,
        key: (u64, u64),
        len: u64,
        now: i64,
        out: &mut ScanOutput,
    ) {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();

        let state = self.files.entry(key).or_default();
        let truncated = !state.file_name.is_empty() && len < state.offset;
        if truncated {
            *state = FileState::default();
        }
        state.file_name = file_name;
        if len <= state.offset {
            return;
        }

        let Ok(file) = std::fs::File::open(path) else {
            return;
        };
        let mut file = file;
        if file.seek(SeekFrom::Start(state.offset)).is_err() {
            return;
        }
        let mut reader = BufReader::new(file);
        let mut consumed = 0u64;
        loop {
            let mut raw_line = Vec::new();
            let n = match reader.read_until(b'\n', &mut raw_line) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            if raw_line.last() != Some(&b'\n') {
                break; // incomplete trailing line: wait for the next scan
            }
            let line_offset = state.offset + consumed;
            consumed += n as u64;
            let Some(without_newline) = raw_line.strip_suffix(b"\n") else {
                break;
            };
            let Ok(text) = std::str::from_utf8(without_newline) else {
                continue;
            };
            process_line(text, line_offset, state, now, out);
        }
        state.offset += consumed;
    }
}

#[derive(Debug, Deserialize)]
struct RolloutLine {
    timestamp: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<serde_json::Value>,
}

fn process_line(
    text: &str,
    byte_offset: u64,
    state: &mut FileState,
    now: i64,
    out: &mut ScanOutput,
) {
    let Ok(line) = serde_json::from_str::<RolloutLine>(text) else {
        return;
    };
    let Some(payload) = line.payload else {
        return;
    };
    match line.kind.as_deref() {
        Some("turn_context") => {
            if let Some(model) = payload.get("model").and_then(|v| v.as_str()) {
                state.model = Some(model.to_string());
            }
        }
        Some("event_msg")
            if payload.get("type").and_then(|v| v.as_str()) == Some("token_count") =>
        {
            let timestamp = line
                .timestamp
                .as_deref()
                .and_then(ts_core::time::parse_timestamp_str)
                .unwrap_or(now);
            handle_token_count(&payload, byte_offset, timestamp, state, out);
        }
        _ => {}
    }
}

fn handle_token_count(
    payload: &serde_json::Value,
    byte_offset: u64,
    timestamp: i64,
    state: &mut FileState,
    out: &mut ScanOutput,
) {
    if let Some(rate_limits) = payload
        .get("rate_limits")
        .and_then(|v| rollout_rate_limits(v, timestamp))
    {
        let newer = out
            .latest_rate_limits
            .as_ref()
            .map(|(ts, _)| timestamp >= *ts)
            .unwrap_or(true);
        if newer {
            out.latest_rate_limits = Some((timestamp, rate_limits));
        }
    }

    let Some(info) = payload.get("info").filter(|v| !v.is_null()) else {
        return;
    };
    let Some(total) = info
        .get("total_token_usage")
        .and_then(|v| v.get("total_tokens"))
        .and_then(|v| v.as_i64())
    else {
        return;
    };
    let increased = state
        .last_seen_total
        .map(|prev| total > prev)
        .unwrap_or(true);
    state.last_seen_total = Some(total);
    if !increased {
        return;
    }
    let Some(last) = info.get("last_token_usage") else {
        return;
    };
    let get_u64 = |field: &str| last.get(field).and_then(|v| v.as_u64()).unwrap_or(0);
    let input_tokens = get_u64("input_tokens");
    let cached_input_tokens = get_u64("cached_input_tokens");
    let counts = TokenCounts {
        input: input_tokens.saturating_sub(cached_input_tokens),
        output: get_u64("output_tokens"),
        cache_read: cached_input_tokens,
        cache_write: 0,
        reasoning: get_u64("reasoning_output_tokens"),
    };
    out.events.push(TokenEvent {
        timestamp,
        model: state
            .model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        counts,
        // Built from the file name (which survives the sessions ->
        // archived_sessions move) rather than the full path, so a moved
        // session's already-counted lines are not counted again.
        dedup_key: format!("{}:{byte_offset}", state.file_name),
    });
}

fn rollout_rate_limits(
    value: &serde_json::Value,
    line_timestamp: i64,
) -> Option<RateLimitSnapshotDto> {
    if value.is_null() {
        return None;
    }
    let get_window = |key: &str| -> Option<RateLimitWindowDto> {
        let w = value.get(key)?;
        if w.is_null() {
            return None;
        }
        // `resets_in_seconds` is relative to this line's own timestamp, not
        // an absolute epoch value: treating it as absolute would report a
        // reset time that is off by however old the rollout line is.
        let resets_at = w.get("resets_at").and_then(|v| v.as_i64()).or_else(|| {
            w.get("resets_in_seconds")
                .and_then(|v| v.as_i64())
                .map(|secs| line_timestamp.saturating_add(secs))
        });
        Some(RateLimitWindowDto {
            used_percent: w
                .get("used_percent")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            window_duration_mins: w.get("window_minutes").and_then(|v| v.as_i64()),
            resets_at,
        })
    };
    Some(RateLimitSnapshotDto {
        limit_id: value
            .get("limit_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        limit_name: value
            .get("limit_name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        normal_model_slug: value
            .get("normal_model_slug")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        primary: get_window("primary"),
        secondary: get_window("secondary"),
        credits: None,
        plan_type: value
            .get("plan_type")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

fn is_recent(path: &Path, min: SystemTime) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|mtime| mtime >= min)
        .unwrap_or(false)
}

/// `<home>/sessions/**/rollout-*.jsonl` and `<home>/archived_sessions/*.jsonl`.
fn discover_files(home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_for_rollouts(&home.join("sessions"), 0, &mut out);
    if let Ok(entries) = std::fs::read_dir(home.join("archived_sessions")) {
        for entry in entries.flatten() {
            if is_rollout_file(&entry.path()) {
                out.push(entry.path());
            }
        }
    }
    out
}

fn walk_for_rollouts(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_for_rollouts(&path, depth + 1, out);
        } else if is_rollout_file(&path) {
            out.push(path);
        }
    }
}

fn is_rollout_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;

    use super::*;

    fn write_lines(path: &Path, lines: &[&str]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn token_count_line(
        ts: &str,
        total: i64,
        input: i64,
        cached: i64,
        output: i64,
        reasoning: i64,
    ) -> String {
        format!(
            r#"{{"timestamp":"{ts}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":{total}}},"last_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output},"reasoning_output_tokens":{reasoning},"total_tokens":{total}}}}},"rate_limits":{{"primary":{{"used_percent":5.0,"window_minutes":10080,"resets_at":999}}}}}}}}"#
        )
    }

    fn turn_context_line(model: &str) -> String {
        format!(r#"{{"type":"turn_context","payload":{{"model":"{model}"}}}}"#)
    }

    #[test]
    fn counts_only_when_total_increases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions/2026/09/28/rollout-a.jsonl");
        write_lines(
            &path,
            &[
                &turn_context_line("gpt-5.3-codex"),
                &token_count_line("2026-09-28T12:00:00Z", 100, 50, 0, 50, 0),
                &token_count_line("2026-09-28T12:00:00Z", 100, 50, 0, 50, 0), // repeated, unchanged
                &token_count_line("2026-09-28T12:01:00Z", 250, 100, 20, 80, 5),
            ],
        );
        let mut scanner = RolloutScanner::new();
        let out = scanner.scan(&[dir.path().to_path_buf()], 0);
        assert_eq!(out.events.len(), 2);
        assert_eq!(out.events[0].model, "gpt-5.3-codex");
        assert_eq!(out.events[0].counts.input, 50);
        assert_eq!(out.events[1].counts.input, 80); // 100 - 20 cached
        assert_eq!(out.events[1].counts.cache_read, 20);
        assert_eq!(out.events[1].counts.reasoning, 5);
        assert!(out.latest_rate_limits.is_some());
    }

    #[test]
    fn null_info_is_skipped_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions/rollout-b.jsonl");
        write_lines(
            &path,
            &[
                r#"{"timestamp":"2026-09-28T12:00:00Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#,
            ],
        );
        let mut scanner = RolloutScanner::new();
        let out = scanner.scan(&[dir.path().to_path_buf()], 0);
        assert!(out.events.is_empty());
    }

    #[test]
    fn partial_trailing_line_is_not_consumed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions/rollout-c.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let complete = token_count_line("2026-09-28T12:00:00Z", 100, 10, 0, 10, 0);
        fs::write(&path, format!("{complete}\n{{\"partial")).unwrap();

        let mut scanner = RolloutScanner::new();
        let out = scanner.scan(&[dir.path().to_path_buf()], 0);
        assert_eq!(out.events.len(), 1);

        // Completing the line on a later scan picks it up.
        let next = token_count_line("2026-09-28T12:01:00Z", 200, 20, 0, 20, 0);
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "\"}}}}\n{next}").unwrap();
        let out2 = scanner.scan(&[dir.path().to_path_buf()], 0);
        assert_eq!(out2.events.len(), 1);
        assert_eq!(out2.events[0].counts.input, 20);
    }

    #[test]
    fn truncated_file_restarts_from_zero() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions/rollout-d.jsonl");
        write_lines(
            &path,
            &[&token_count_line("2026-09-28T12:00:00Z", 500, 50, 0, 50, 0)],
        );
        let mut scanner = RolloutScanner::new();
        let out = scanner.scan(&[dir.path().to_path_buf()], 0);
        assert_eq!(out.events.len(), 1);

        // Truncate then write a fresh, shorter line.
        write_lines(
            &path,
            &[&token_count_line("2026-09-28T13:00:00Z", 10, 5, 0, 5, 0)],
        );
        let out2 = scanner.scan(&[dir.path().to_path_buf()], 0);
        assert_eq!(out2.events.len(), 1);
        assert_eq!(out2.events[0].counts.input, 5);
    }

    #[test]
    fn old_files_are_skipped_on_first_scan_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions/rollout-old.jsonl");
        write_lines(
            &path,
            &[&token_count_line("2026-09-01T00:00:00Z", 10, 5, 0, 5, 0)],
        );
        let old = SystemTime::now() - Duration::from_secs(20 * 86_400);
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();

        let mut scanner = RolloutScanner::new();
        let out = scanner.scan(&[dir.path().to_path_buf()], 0);
        assert!(
            out.events.is_empty(),
            "old file must be skipped on first scan"
        );
    }

    #[test]
    fn model_falls_back_to_default_without_turn_context() {
        let out = scan_one_line_file("sessions/rollout-e.jsonl");
        assert_eq!(out.events[0].model, DEFAULT_MODEL);
    }

    #[test]
    fn archived_sessions_are_scanned_too() {
        let out = scan_one_line_file("archived_sessions/rollout-f.jsonl");
        assert_eq!(out.events.len(), 1);
    }

    /// Write one `token_count` line at `relative_path` under a fresh tempdir
    /// and scan it once.
    fn scan_one_line_file(relative_path: &str) -> ScanOutput {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(relative_path);
        write_lines(
            &path,
            &[&token_count_line("2026-09-28T12:00:00Z", 10, 5, 0, 5, 0)],
        );
        let mut scanner = RolloutScanner::new();
        scanner.scan(&[dir.path().to_path_buf()], 0)
    }

    #[test]
    fn moving_session_to_archived_does_not_double_count() {
        let dir = tempfile::tempdir().unwrap();
        let sessions_path = dir.path().join("sessions/2026/09/28/rollout-g.jsonl");
        write_lines(
            &sessions_path,
            &[&token_count_line("2026-09-28T12:00:00Z", 100, 50, 0, 50, 0)],
        );
        let mut scanner = RolloutScanner::new();
        let out = scanner.scan(&[dir.path().to_path_buf()], 0);
        assert_eq!(out.events.len(), 1);

        // Same content, same inode, new location: rename (not copy) so the
        // move preserves the inode, exactly like the real session rotation.
        let archived_path = dir.path().join("archived_sessions/rollout-g.jsonl");
        std::fs::create_dir_all(archived_path.parent().unwrap()).unwrap();
        std::fs::rename(&sessions_path, &archived_path).unwrap();
        let out2 = scanner.scan(&[dir.path().to_path_buf()], 0);
        assert!(
            out2.events.is_empty(),
            "moved file's already-scanned line must not be counted again"
        );

        // A genuinely new line appended after the move is still picked up.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&archived_path)
            .unwrap();
        std::io::Write::write_all(
            &mut file,
            format!(
                "{}\n",
                token_count_line("2026-09-28T12:05:00Z", 250, 100, 20, 80, 5)
            )
            .as_bytes(),
        )
        .unwrap();
        let out3 = scanner.scan(&[dir.path().to_path_buf()], 0);
        assert_eq!(out3.events.len(), 1);
    }

    /// `rollout_rate_limits`'s `primary.resets_at` for one raw `primary` window.
    fn primary_resets_at(primary: serde_json::Value) -> Option<i64> {
        let value = serde_json::json!({ "primary": primary });
        rollout_rate_limits(&value, 1_000)
            .and_then(|s| s.primary)
            .and_then(|w| w.resets_at)
    }

    #[test]
    fn resets_in_seconds_is_relative_to_the_line_timestamp() {
        let resets_at = primary_resets_at(
            serde_json::json!({"used_percent": 5.0, "window_minutes": 10080, "resets_in_seconds": 3600}),
        );
        assert_eq!(resets_at, Some(4_600));
    }

    #[test]
    fn resets_at_absolute_value_takes_priority_over_resets_in_seconds() {
        let resets_at = primary_resets_at(serde_json::json!({
            "used_percent": 5.0, "window_minutes": 10080, "resets_at": 999, "resets_in_seconds": 3600
        }));
        assert_eq!(resets_at, Some(999));
    }
}
