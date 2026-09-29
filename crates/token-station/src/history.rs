//! Recorded usage percentages, for the front-end sparkline.
//!
//! One `SQLite` file in the state directory. Every call blocks, so the daemon drives
//! this store through [`HistoryHandle`], which moves the work onto a blocking thread.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::Connection;
use ts_core::ProviderId;

/// Most points `GetHistory` ever returns.
pub const MAX_POINTS: usize = 120;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS samples (
    provider  TEXT NOT NULL,
    window_id TEXT NOT NULL,
    ts        INTEGER NOT NULL,
    percent   REAL NOT NULL,
    PRIMARY KEY (provider, window_id, ts)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS samples_ts ON samples (ts);
";

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("cannot open history database at {path}: {source}")]
    Open {
        path: String,
        source: rusqlite::Error,
    },
    #[error("history query failed: {0}")]
    Query(#[from] rusqlite::Error),
    #[error("history store is poisoned")]
    Poisoned,
}

/// One recorded observation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample<'a> {
    pub provider: ProviderId,
    pub window_id: &'a str,
    pub ts: i64,
    pub percent: f64,
}

/// The SQLite-backed store. Blocking; see [`HistoryHandle`].
pub struct HistoryStore {
    conn: Mutex<Connection>,
}

impl HistoryStore {
    /// Open (and create) the database at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`HistoryError::Open`] when the parent directory cannot be
    /// created or `SQLite` cannot open the file, and [`HistoryError::Query`]
    /// when the pragmas or the schema cannot be applied.
    pub fn open(path: &Path) -> Result<HistoryStore, HistoryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HistoryError::Open {
                path: path.display().to_string(),
                source: rusqlite::Error::ToSqlConversionFailure(Box::new(e)),
            })?;
        }
        let conn = Connection::open(path).map_err(|source| HistoryError::Open {
            path: path.display().to_string(),
            source,
        })?;
        HistoryStore::prepare(conn)
    }

    /// An in-memory store, for tests and for a state directory that cannot be written.
    ///
    /// # Errors
    ///
    /// Returns [`HistoryError::Query`] when `SQLite` refuses the in-memory
    /// database, the pragmas or the schema.
    pub fn in_memory() -> Result<HistoryStore, HistoryError> {
        HistoryStore::prepare(Connection::open_in_memory()?)
    }

    fn prepare(conn: Connection) -> Result<HistoryStore, HistoryError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        Ok(HistoryStore {
            conn: Mutex::new(conn),
        })
    }

    fn with_conn<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, HistoryError> {
        let mut guard = self.conn.lock().map_err(|_| HistoryError::Poisoned)?;
        Ok(f(&mut guard)?)
    }

    /// Store observations, replacing any sample with the same key.
    ///
    /// # Errors
    ///
    /// Returns [`HistoryError::Poisoned`] when another thread panicked holding
    /// the connection, and [`HistoryError::Query`] when the insert or the commit
    /// fails. The transaction means a failure stores nothing.
    pub fn record(&self, samples: &[Sample<'_>]) -> Result<(), HistoryError> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            {
                let mut stmt = tx.prepare(
                    "INSERT OR REPLACE INTO samples (provider, window_id, ts, percent)
                     VALUES (?1, ?2, ?3, ?4)",
                )?;
                for s in samples {
                    stmt.execute(rusqlite::params![
                        s.provider.as_str(),
                        s.window_id,
                        s.ts,
                        s.percent
                    ])?;
                }
            }
            tx.commit()
        })
    }

    /// Samples for one window since `since`, oldest first, down-sampled.
    ///
    /// # Errors
    ///
    /// Returns [`HistoryError::Poisoned`] when another thread panicked holding
    /// the connection, and [`HistoryError::Query`] when the select fails.
    pub fn query(
        &self,
        provider: ProviderId,
        window_id: &str,
        since: i64,
    ) -> Result<Vec<(i64, f64)>, HistoryError> {
        let raw = self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ts, percent FROM samples
                 WHERE provider = ?1 AND window_id = ?2 AND ts >= ?3
                 ORDER BY ts ASC",
            )?;
            let rows = stmt.query_map(
                rusqlite::params![provider.as_str(), window_id, since],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?)),
            )?;
            rows.collect::<Result<Vec<_>, _>>()
        })?;
        Ok(downsample(&raw, MAX_POINTS))
    }

    /// Delete samples older than `cutoff`; returns how many rows went away.
    ///
    /// # Errors
    ///
    /// Returns [`HistoryError::Poisoned`] when another thread panicked holding
    /// the connection, and [`HistoryError::Query`] when the delete fails.
    pub fn prune(&self, cutoff: i64) -> Result<usize, HistoryError> {
        self.with_conn(|conn| conn.execute("DELETE FROM samples WHERE ts < ?1", [cutoff]))
    }

    /// Total number of stored samples (diagnostics and tests).
    ///
    /// # Errors
    ///
    /// Returns [`HistoryError::Poisoned`] when another thread panicked holding
    /// the connection, and [`HistoryError::Query`] when the count fails.
    pub fn count(&self) -> Result<usize, HistoryError> {
        self.with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM samples", [], |r| {
                r.get::<_, i64>(0).map(|n| usize::try_from(n).unwrap_or(0))
            })
        })
    }
}

/// Reduce `points` to at most `max` by averaging consecutive buckets.
///
/// Buckets split the series evenly by index, so the first and last observations keep
/// their weight and the shape of the curve survives.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "sample timestamps and bucket sizes are far below 2^53; the averaged timestamp is rounded back to whole seconds"
)]
pub fn downsample(points: &[(i64, f64)], max: usize) -> Vec<(i64, f64)> {
    if max == 0 {
        return Vec::new();
    }
    if points.len() <= max {
        return points.to_vec();
    }
    let len = points.len();
    (0..max)
        .filter_map(|i| {
            let start = i * len / max;
            let end = ((i + 1) * len / max).max(start + 1).min(len);
            let bucket = points.get(start..end)?;
            if bucket.is_empty() {
                return None;
            }
            let n = bucket.len() as f64;
            let ts = bucket.iter().map(|p| p.0 as f64).sum::<f64>() / n;
            let percent = bucket.iter().map(|p| p.1).sum::<f64>() / n;
            Some((ts.round() as i64, percent))
        })
        .collect()
}

/// Render a series the way `GetHistory` returns it.
#[must_use]
pub fn to_json(points: &[(i64, f64)]) -> String {
    let rows: Vec<serde_json::Value> = points
        .iter()
        .map(|(ts, pct)| serde_json::json!([ts, round2(*pct)]))
        .collect();
    serde_json::Value::Array(rows).to_string()
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// Async front door: every call hops to a blocking thread.
///
/// A handle built by [`HistoryHandle::deferred`] opens its store the first time it
/// is used rather than when it is built. The daemon has to be assembled before it
/// owns its bus name — the D-Bus object must be exported first, or the very call
/// that activated the daemon is answered with an error — and a daemon that then
/// loses the name race should not have created, locked or journalled the database
/// of the one that won.
#[derive(Clone)]
pub struct HistoryHandle {
    store: Arc<OnceLock<Option<Arc<HistoryStore>>>>,
    /// Where to open the store on first use; `None` once one is already in hand.
    path: Option<PathBuf>,
}

impl HistoryHandle {
    /// A handle around a store that is already open.
    pub fn new(store: Arc<HistoryStore>) -> HistoryHandle {
        let cell = OnceLock::new();
        let _ = cell.set(Some(store));
        HistoryHandle {
            store: Arc::new(cell),
            path: None,
        }
    }

    /// A handle that opens (and creates) `path` the first time it is used.
    #[must_use]
    pub fn deferred(path: &Path) -> HistoryHandle {
        HistoryHandle {
            store: Arc::new(OnceLock::new()),
            path: Some(path.to_path_buf()),
        }
    }

    /// Open `path` now, falling back to an in-memory store when the file is unusable.
    ///
    /// Only a broken `SQLite` build makes this fail.
    ///
    /// # Errors
    ///
    /// Returns [`HistoryError::Query`] when even the in-memory fallback cannot
    /// be opened or given the schema.
    pub fn open_or_memory(path: &Path) -> Result<HistoryHandle, HistoryError> {
        let store = HistoryStore::open(path).or_else(|error| {
            tracing::error!(%error, "history database unavailable, keeping history in memory");
            HistoryStore::in_memory()
        })?;
        Ok(HistoryHandle::new(Arc::new(store)))
    }

    /// The store, opened on first use. Blocking; only called on blocking threads.
    fn store(&self) -> Option<Arc<HistoryStore>> {
        self.store.get_or_init(|| self.open()).clone()
    }

    fn open(&self) -> Option<Arc<HistoryStore>> {
        let path = self.path.as_ref()?;
        let opened = HistoryStore::open(path).or_else(|error| {
            tracing::error!(%error, "history database unavailable, keeping history in memory");
            HistoryStore::in_memory()
        });
        match opened {
            Ok(store) => Some(Arc::new(store)),
            // Nothing is left to try; the daemon still works, without a sparkline.
            Err(error) => {
                tracing::error!(%error, "no history store: usage history will not be recorded");
                None
            }
        }
    }

    pub async fn record(&self, samples: Vec<OwnedSample>) {
        let handle = self.clone();
        let result = tokio::task::spawn_blocking(move || {
            let Some(store) = handle.store() else {
                return Ok(());
            };
            let borrowed: Vec<Sample<'_>> = samples.iter().map(OwnedSample::as_sample).collect();
            store.record(&borrowed)
        })
        .await;
        log_failure("record", result);
    }

    pub async fn query(
        &self,
        provider: ProviderId,
        window_id: String,
        since: i64,
    ) -> Vec<(i64, f64)> {
        let handle = self.clone();
        tokio::task::spawn_blocking(move || match handle.store() {
            Some(store) => store.query(provider, &window_id, since),
            None => Ok(Vec::new()),
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()))
        .unwrap_or_else(|error| {
            tracing::error!(%error, "history query failed");
            Vec::new()
        })
    }

    pub async fn prune(&self, cutoff: i64) {
        let handle = self.clone();
        let result = tokio::task::spawn_blocking(move || match handle.store() {
            Some(store) => store.prune(cutoff).map(|_| ()),
            None => Ok(()),
        })
        .await;
        log_failure("prune", result);
    }
}

fn log_failure(what: &str, result: Result<Result<(), HistoryError>, tokio::task::JoinError>) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::error!(%error, "history {what} failed"),
        Err(error) => tracing::error!(%error, "history {what} task failed"),
    }
}

/// Owned form of [`Sample`], so it can cross a thread boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedSample {
    pub provider: ProviderId,
    pub window_id: String,
    pub ts: i64,
    pub percent: f64,
}

impl OwnedSample {
    fn as_sample(&self) -> Sample<'_> {
        Sample {
            provider: self.provider,
            window_id: &self.window_id,
            ts: self.ts,
            percent: self.percent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(window: &str, ts: i64, percent: f64) -> OwnedSample {
        OwnedSample {
            provider: ProviderId::Claude,
            window_id: window.into(),
            ts,
            percent,
        }
    }

    /// The series stored for the Claude session window.
    fn session_rows(store: &HistoryStore) -> Vec<(i64, f64)> {
        store
            .query(ProviderId::Claude, "session", 0)
            .expect("the query runs")
    }

    fn store_with(samples: &[OwnedSample]) -> HistoryStore {
        let store = HistoryStore::in_memory().unwrap();
        let borrowed: Vec<Sample<'_>> = samples.iter().map(OwnedSample::as_sample).collect();
        store.record(&borrowed).unwrap();
        store
    }

    #[test]
    fn opens_a_file_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/history.sqlite");
        let store = HistoryStore::open(&path).unwrap();
        store
            .record(&[Sample {
                provider: ProviderId::Codex,
                window_id: "codex:primary",
                ts: 100,
                percent: 12.5,
            }])
            .unwrap();
        drop(store);

        let reopened = HistoryStore::open(&path).unwrap();
        let rows = reopened
            .query(ProviderId::Codex, "codex:primary", 0)
            .unwrap();
        assert_eq!(rows, vec![(100, 12.5)]);
    }

    #[test]
    fn query_filters_by_provider_window_and_since() {
        let store = store_with(&[
            sample("session", 10, 1.0),
            sample("session", 20, 2.0),
            sample("weekly_all", 20, 9.0),
            OwnedSample {
                provider: ProviderId::Codex,
                window_id: "session".into(),
                ts: 20,
                percent: 7.0,
            },
        ]);
        assert_eq!(
            store.query(ProviderId::Claude, "session", 15).unwrap(),
            vec![(20, 2.0)]
        );
        assert!(
            store
                .query(ProviderId::Claude, "nope", 0)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_re_recorded_instant_is_replaced_and_only_old_rows_are_pruned() {
        let store = store_with(&[sample("session", 10, 1.0), sample("session", 10, 4.0)]);
        assert_eq!(store.count().unwrap(), 1, "the key is the same instant");
        assert_eq!(
            session_rows(&store),
            vec![(10, 4.0)],
            "the later value wins"
        );

        store
            .record(&[sample("session", 500, 2.0).as_sample()])
            .unwrap();
        assert_eq!(store.prune(100).unwrap(), 1, "only the older row goes");
        assert_eq!(session_rows(&store), vec![(500, 2.0)]);
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "sample indices and timestamps in this fixture are tiny"
    )]
    #[test]
    fn query_downsamples_to_the_cap() {
        let samples: Vec<OwnedSample> = (0..500)
            .map(|i| sample("session", i, (i % 100) as f64))
            .collect();
        let store = store_with(&samples);
        let rows = store.query(ProviderId::Claude, "session", 0).unwrap();
        assert_eq!(rows.len(), MAX_POINTS);
        assert!(rows.windows(2).all(|w| w[0].0 < w[1].0), "sorted by time");
    }

    #[test]
    fn downsample_is_identity_below_the_cap() {
        let points = vec![(1, 1.0), (2, 2.0)];
        assert_eq!(downsample(&points, 120), points);
        assert!(downsample(&points, 0).is_empty());
        assert!(downsample(&[], 120).is_empty());
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "sample indices and timestamps in this fixture are tiny"
    )]
    #[test]
    fn downsample_averages_each_bucket() {
        let points: Vec<(i64, f64)> = (0..10).map(|i| (i, i as f64)).collect();
        let out = downsample(&points, 5);
        assert_eq!(out.len(), 5);
        assert_eq!(out[0], (1, 0.5)); // mean of (0,0) and (1,1)
        assert_eq!(out[4], (9, 8.5));
    }

    #[test]
    fn json_shape_matches_the_dbus_contract() {
        assert_eq!(to_json(&[]), "[]");
        assert_eq!(
            to_json(&[(1_790_596_800, 34.0), (1_790_596_860, 34.567)]),
            "[[1790596800,34.0],[1790596860,34.57]]"
        );
    }
}
