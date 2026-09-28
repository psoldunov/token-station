//! Incremental scanner for `<config_dir>/projects/**/*.jsonl` transcript logs.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::Value;
use ts_core::tokens::{TokenCounts, TokenEvent};

/// Only files younger than this are picked up the first time they are noticed.
const MAX_LOOKBACK_SECS: i64 = 8 * 86_400;

/// Per-file byte offsets, carried across scans.
#[derive(Debug, Clone, Default)]
pub struct LogScanState {
    offsets: HashMap<(u64, u64), u64>,
}

/// Scan every `*.jsonl` file under `roots` for new [`TokenEvent`]s, advancing
/// `state`'s stored offsets. Never panics: unreadable files or malformed lines
/// are skipped.
pub fn scan(roots: &[PathBuf], state: &mut LogScanState, now: i64) -> Vec<TokenEvent> {
    let cutoff = now - MAX_LOOKBACK_SECS;
    let mut events = Vec::new();
    for root in roots {
        for path in collect_jsonl_files(root) {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let key = (meta.dev(), meta.ino());
            let is_new = !state.offsets.contains_key(&key);
            if is_new && file_mtime(&meta) < cutoff {
                continue;
            }
            let start_offset = match state.offsets.get(&key) {
                Some(&off) if off <= meta.len() => off,
                _ => 0,
            };
            let Some((mut file_events, new_offset)) = scan_file(&path, start_offset) else {
                continue;
            };
            state.offsets.insert(key, new_offset);
            events.append(&mut file_events);
        }
    }
    events
}

fn file_mtime(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn collect_jsonl_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(path);
            }
        }
    }
    out
}

/// Read new complete lines starting at `offset`; returns events and the offset
/// just past the last complete line (a trailing partial line is left for next
/// time).
fn scan_file(path: &Path, offset: u64) -> Option<(Vec<TokenEvent>, u64)> {
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;

    let mut events = Vec::new();
    let mut consumed: u64 = 0;
    for line in buf.split_inclusive(|&b| b == b'\n') {
        if line.last() != Some(&b'\n') {
            break;
        }
        consumed += line.len() as u64;
        if let Ok(text) = std::str::from_utf8(line) {
            if let Some(event) = parse_line(text) {
                events.push(event);
            }
        }
    }
    Some((events, offset + consumed))
}

fn parse_line(text: &str) -> Option<TokenEvent> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let message = value.get("message")?;
    let model = message.get("model").and_then(Value::as_str)?;
    if model == "<synthetic>" {
        return None;
    }
    let message_id = message.get("id").and_then(Value::as_str)?;
    let usage = message.get("usage")?;
    let timestamp = value
        .get("timestamp")
        .and_then(ts_core::time::parse_timestamp)?;

    let request_id = value.get("requestId").and_then(Value::as_str);
    let uuid = value.get("uuid").and_then(value_as_id_str);
    let dedup_key = match request_id.or(uuid.as_deref()) {
        Some(tail) => format!("{message_id}:{tail}"),
        None => message_id.to_string(),
    };

    let get_u64 = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let reasoning = usage
        .get("output_tokens_details")
        .and_then(|d| d.get("thinking_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0);

    Some(TokenEvent {
        timestamp,
        model: model.to_string(),
        counts: TokenCounts {
            input: get_u64("input_tokens"),
            output: get_u64("output_tokens"),
            cache_read: get_u64("cache_read_input_tokens"),
            cache_write: get_u64("cache_creation_input_tokens"),
            reasoning,
        },
        dedup_key,
    })
}

fn value_as_id_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant_line(id: &str, request_id: Option<&str>, model: &str, ts: &str) -> String {
        let req = request_id.map_or("null".to_string(), |r| format!("\"{r}\""));
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","requestId":{req},"uuid":"u-{id}","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":1,"cache_creation_input_tokens":2,"output_tokens_details":{{"thinking_tokens":3}}}}}}}}"#
        )
    }

    fn write_jsonl(dir: &Path, name: &str, lines: &[String]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    const NOW: i64 = 1_790_596_800; // 2026-09-28T12:00:00Z

    #[test]
    fn scans_new_lines_and_dedups_by_request_id() {
        let dir = tempfile::tempdir().unwrap();
        let l1 = assistant_line(
            "msg1",
            Some("req1"),
            "claude-opus-5-5",
            "2026-09-28T11:00:00Z",
        );
        let l2 = assistant_line(
            "msg1",
            Some("req1"),
            "claude-opus-5-5",
            "2026-09-28T11:00:00Z",
        );
        write_jsonl(dir.path(), "a.jsonl", &[l1, l2]);

        let mut state = LogScanState::default();
        let events = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert_eq!(events.len(), 2); // scanner itself does not dedup
        assert_eq!(events[0].dedup_key, events[1].dedup_key);
        assert_eq!(events[0].dedup_key, "msg1:req1");
        assert_eq!(events[0].counts.reasoning, 3);
    }

    #[test]
    fn leaves_trailing_partial_line_for_next_scan() {
        let dir = tempfile::tempdir().unwrap();
        let complete = assistant_line("msg1", Some("req1"), "m", "2026-09-28T11:00:00Z");
        let path = dir.path().join("a.jsonl");
        std::fs::write(&path, format!("{complete}\n{{\"type\":\"assistant\"")).unwrap();

        let mut state = LogScanState::default();
        let events = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert_eq!(events.len(), 1);

        // Nothing new yet: partial line still incomplete.
        let events2 = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert!(events2.is_empty());

        // Complete the line, next scan picks it up.
        let l2 = assistant_line("msg2", Some("req2"), "m", "2026-09-28T11:05:00Z");
        std::fs::write(
            &path,
            format!("{complete}\n{{\"type\":\"assistant\"}}\n{l2}\n"),
        )
        .unwrap();
        let events3 = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert_eq!(events3.len(), 1);
        assert_eq!(events3[0].dedup_key, "msg2:req2");
    }

    #[test]
    fn restarts_from_zero_when_file_shrinks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        let l1 = assistant_line(
            "msg1",
            Some("req1"),
            "claude-opus-5-5-with-a-long-name",
            "2026-09-28T11:00:00Z",
        );
        std::fs::write(&path, format!("{l1}\n")).unwrap();
        let mut state = LogScanState::default();
        scan(&[dir.path().to_path_buf()], &mut state, NOW);

        // Truncate and write a much shorter line.
        let l2 = assistant_line("msg2", Some("req2"), "m", "2026-09-28T11:05:00Z");
        std::fs::write(&path, format!("{l2}\n")).unwrap();
        let events = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].dedup_key, "msg2:req2");
    }

    #[test]
    fn skips_old_unseen_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.jsonl");
        let l1 = assistant_line("msg1", Some("req1"), "m", "2026-09-01T00:00:00Z");
        std::fs::write(&path, format!("{l1}\n")).unwrap();
        let old_time =
            std::time::UNIX_EPOCH + std::time::Duration::from_secs((NOW - 9 * 86_400) as u64);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old_time)
            .unwrap();

        let mut state = LogScanState::default();
        let events = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert!(events.is_empty());
    }

    #[test]
    fn scans_nested_directories() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("proj1/sub");
        std::fs::create_dir_all(&nested).unwrap();
        let l1 = assistant_line("msg1", Some("req1"), "m", "2026-09-28T11:00:00Z");
        write_jsonl(&nested, "sess.jsonl", &[l1]);

        let mut state = LogScanState::default();
        let events = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn skips_synthetic_model_and_non_assistant_lines() {
        let dir = tempfile::tempdir().unwrap();
        let synthetic = assistant_line("msg1", Some("req1"), "<synthetic>", "2026-09-28T11:00:00Z");
        let user_line = r#"{"type":"user","timestamp":"2026-09-28T11:00:00Z"}"#.to_string();
        write_jsonl(dir.path(), "a.jsonl", &[synthetic, user_line]);

        let mut state = LogScanState::default();
        let events = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert!(events.is_empty());
    }

    #[test]
    fn falls_back_to_uuid_when_request_id_missing() {
        let dir = tempfile::tempdir().unwrap();
        let l1 = assistant_line("msg1", None, "m", "2026-09-28T11:00:00Z");
        write_jsonl(dir.path(), "a.jsonl", &[l1]);

        let mut state = LogScanState::default();
        let events = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert_eq!(events[0].dedup_key, "msg1:u-msg1");
    }
}
