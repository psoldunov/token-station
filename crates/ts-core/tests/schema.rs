//! Keeps `data/snapshot.schema.json` and the fixture snapshots in sync with the types.
//!
//! Regenerate the schema with `UPDATE_SCHEMA=1 cargo test -p ts-core --test schema`.

use std::path::PathBuf;

use ts_core::Snapshot;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn committed_schema_matches_types() {
    let path = repo_root().join("data/snapshot.schema.json");
    let generated = serde_json::to_string_pretty(&Snapshot::json_schema()).unwrap() + "\n";
    if std::env::var_os("UPDATE_SCHEMA").is_some() {
        std::fs::write(&path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        committed, generated,
        "data/snapshot.schema.json is stale; rerun with UPDATE_SCHEMA=1"
    );
}

#[test]
fn fixtures_deserialize() {
    let dir = repo_root().join("data/fixtures");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            let text = std::fs::read_to_string(&path).unwrap();
            let snap: Snapshot =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!(
                snap.schema_version,
                ts_core::SCHEMA_VERSION,
                "{}",
                path.display()
            );
            count += 1;
        }
    }
    assert!(count > 0, "no fixtures found");
}
