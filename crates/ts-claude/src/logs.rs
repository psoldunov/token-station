//! Incremental scanner for `<config_dir>/projects/**/*.jsonl` transcript logs.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::Value;
use ts_core::tokens::{TokenCounts, TokenEvent};

/// Only files younger than this are picked up the first time they are noticed.
const MAX_LOOKBACK_SECS: i64 = 8 * 86_400;

/// How many leading bytes of a file are hashed into its fingerprint.
const FINGERPRINT_BYTES: usize = 256;

/// Per-file read state, keyed by `(dev, ino)`.
#[derive(Debug, Clone, Default)]
struct FileState {
    offset: u64,
    /// Hash of the file's first bytes plus its mtime at the time `offset` was
    /// recorded. A mismatch means the inode was reused for a different file
    /// (e.g. after transcript cleanup), so the offset must not be trusted.
    fingerprint: Option<u64>,
}

/// Per-file byte offsets, carried across scans.
#[derive(Debug, Clone, Default)]
pub struct LogScanState {
    files: HashMap<(u64, u64), FileState>,
}

/// Scan every `*.jsonl` file under `roots` for new [`TokenEvent`]s, advancing
/// `state`'s stored offsets. Never panics: unreadable files or malformed lines
/// are skipped. Keys not seen in this scan (files removed since the last
/// scan) are pruned so the state never grows unbounded.
pub fn scan(roots: &[PathBuf], state: &mut LogScanState, now: i64) -> Vec<TokenEvent> {
    let cutoff = now - MAX_LOOKBACK_SECS;
    let mut events = Vec::new();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    for root in roots {
        for path in collect_jsonl_files(root) {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let key = (meta.dev(), meta.ino());
            seen.insert(key);
            let prior = state.files.get(&key).cloned();
            let is_new = prior.is_none();
            if is_new && file_mtime(&meta) < cutoff {
                continue;
            }
            if let Some(p) = &prior {
                if meta.len() == p.offset {
                    continue; // unchanged: skip opening the file entirely
                }
            }
            let Some((mut file_events, new_state)) = scan_file(&path, prior.as_ref()) else {
                continue;
            };
            state.files.insert(key, new_state);
            events.append(&mut file_events);
        }
    }
    state.files.retain(|key, _| seen.contains(key));
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

/// Hash of the file's leading bytes; used to detect inode reuse. Deliberately
/// excludes mtime: a normal append changes mtime on every scan, which would
/// otherwise make every legitimate incremental scan look like a reused inode.
/// The header bytes of an append-only log stay stable, so a real content
/// change there is a strong signal that this is a different file.
fn fingerprint_of(file: &mut std::fs::File) -> Option<u64> {
    let mut header = [0u8; FINGERPRINT_BYTES];
    let n = file.read(&mut header).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    header.get(..n).unwrap_or(&[]).hash(&mut hasher);
    Some(hasher.finish())
}

/// Read new complete lines starting at `prior`'s offset (or from the start if
/// the fingerprint no longer matches, meaning the inode was reused). Streams
/// the file line by line rather than reading it whole; a trailing partial
/// line is left for next time.
fn scan_file(path: &Path, prior: Option<&FileState>) -> Option<(Vec<TokenEvent>, FileState)> {
    let mut file = std::fs::File::open(path).ok()?;
    let fingerprint = fingerprint_of(&mut file)?;
    let len = file.metadata().ok()?.len();

    let reset = match prior {
        Some(p) => p.fingerprint != Some(fingerprint) || p.offset > len,
        None => false,
    };
    let start_offset = if reset {
        0
    } else {
        prior.map(|p| p.offset).unwrap_or(0)
    };

    file.seek(SeekFrom::Start(start_offset)).ok()?;
    let mut reader = BufReader::new(file);
    let mut events = Vec::new();
    let mut consumed = start_offset;
    loop {
        let mut line = Vec::new();
        let n = match reader.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        if line.last() != Some(&b'\n') {
            break; // incomplete trailing line: wait for the next scan
        }
        consumed += n as u64;
        let Some(without_newline) = line.strip_suffix(b"\n") else {
            break;
        };
        if let Ok(text) = std::str::from_utf8(without_newline) {
            if let Some(event) = parse_line(text) {
                events.push(event);
            }
        }
    }
    Some((
        events,
        FileState {
            offset: consumed,
            fingerprint: Some(fingerprint),
        },
    ))
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

    // `uuid` is unique per streamed line for the same message, not per
    // request, so it must never be used as (part of) the dedup key: that
    // would defeat dedup entirely for streamed content-block lines.
    let request_id = value.get("requestId").and_then(Value::as_str);
    let dedup_key = match request_id {
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

    /// Write `lines` to a single fresh `a.jsonl` and scan it once.
    fn scan_lines(lines: &[String]) -> Vec<TokenEvent> {
        let dir = tempfile::tempdir().unwrap();
        write_jsonl(dir.path(), "a.jsonl", lines);
        let mut state = LogScanState::default();
        scan(&[dir.path().to_path_buf()], &mut state, NOW)
    }

    #[test]
    fn skips_synthetic_model_and_non_assistant_lines() {
        let synthetic = assistant_line("msg1", Some("req1"), "<synthetic>", "2026-09-28T11:00:00Z");
        let user_line = r#"{"type":"user","timestamp":"2026-09-28T11:00:00Z"}"#.to_string();
        let events = scan_lines(&[synthetic, user_line]);
        assert!(events.is_empty());
    }

    #[test]
    fn falls_back_to_message_id_alone_when_request_id_missing() {
        // `uuid` must never be used: it is unique per streamed line for the
        // same message, so folding it into the key would defeat dedup.
        let l1 = assistant_line("msg1", None, "m", "2026-09-28T11:00:00Z");
        let events = scan_lines(&[l1]);
        assert_eq!(events[0].dedup_key, "msg1");
    }

    #[test]
    fn inode_reuse_resets_offset_instead_of_skipping_new_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        let l1 = assistant_line(
            "msg1",
            Some("req1"),
            "claude-opus-5-5-with-a-long-enough-name",
            "2026-09-28T11:00:00Z",
        );
        write_jsonl(dir.path(), "a.jsonl", &[l1]);
        let mut state = LogScanState::default();
        let events = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert_eq!(events.len(), 1);

        // Simulate inode reuse: delete and recreate a file with unrelated,
        // shorter content at the same path. On most filesystems a fresh
        // inode is likely, but the fingerprint check must catch it even if
        // the OS happened to reuse the same inode.
        std::fs::remove_file(&path).unwrap();
        let l2 = assistant_line("msg2", Some("req2"), "m", "2026-09-28T11:05:00Z");
        write_jsonl(dir.path(), "a.jsonl", &[l2]);
        let events2 = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert_eq!(events2.len(), 1);
        assert_eq!(events2[0].dedup_key, "msg2:req2");
    }

    #[test]
    fn prunes_state_for_files_removed_since_last_scan() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jsonl");
        let l1 = assistant_line("msg1", Some("req1"), "m", "2026-09-28T11:00:00Z");
        write_jsonl(dir.path(), "a.jsonl", &[l1]);
        let mut state = LogScanState::default();
        scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert_eq!(state.files.len(), 1);

        std::fs::remove_file(&path).unwrap();
        scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert!(
            state.files.is_empty(),
            "removed file's state must be pruned"
        );
    }

    #[test]
    fn unchanged_file_is_not_reopened() {
        let dir = tempfile::tempdir().unwrap();
        let l1 = assistant_line("msg1", Some("req1"), "m", "2026-09-28T11:00:00Z");
        write_jsonl(dir.path(), "a.jsonl", &[l1]);
        let mut state = LogScanState::default();
        let events = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert_eq!(events.len(), 1);

        // Second scan with no file change must return no new events (and,
        // per the fast path, never open the file at all).
        let events2 = scan(&[dir.path().to_path_buf()], &mut state, NOW);
        assert!(events2.is_empty());
    }

    #[test]
    fn a_transcript_older_than_the_lookback_stays_skipped_on_every_scan() {
        let dir = tempfile::tempdir().unwrap();
        let line = assistant_line("msg1", Some("req1"), "m", "2026-09-10T11:00:00Z");
        let path = write_jsonl(dir.path(), "old.jsonl", &[line]);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(20 * 86_400);
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();

        // The cutoff is taken from `now` on every pass, not just the first, so an
        // old file is never picked up later and read from byte 0.
        let mut state = LogScanState::default();
        let now = unix_seconds_now();
        assert!(scan(&[dir.path().to_path_buf()], &mut state, now).is_empty());
        assert!(scan(&[dir.path().to_path_buf()], &mut state, now).is_empty());
        assert!(state.files.is_empty(), "nothing was recorded for it either");
    }

    fn unix_seconds_now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}
