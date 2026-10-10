// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Tests for the open guard against a legacy plaintext-shaped `triples` table.
//!
//! Split out of `triple_reshape` so both stay within the source-size limit.

use super::*;
use crate::dek::{DekProvider, LocalDevKms};
use crate::sqlite::SqliteStore;
use clio_types::ErrorCode;
use std::sync::Arc;

/// The current fresh-open shape: the sealed column exists and no plaintext
/// content column remains. `subject`/`predicate` stay plaintext on purpose —
/// they are structure, not content.
#[test]
fn a_current_store_has_no_plaintext_object_column() {
    let store = SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).expect("fresh store");
    let (has_object, has_cipher) = store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare("PRAGMA table_info(triples)")
                .expect("table_info");
            let mut has_object = false;
            let mut has_cipher = false;
            let names = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .expect("column rows");
            for name in names {
                match name.expect("column name").as_str() {
                    "object" => has_object = true,
                    "object_ciphertext" => has_cipher = true,
                    _ => {}
                }
            }
            Ok((has_object, has_cipher))
        })
        .expect("column probe");
    assert!(!has_object, "plaintext content column must be absent");
    assert!(has_cipher, "the sealed column must exist");
}

/// A fresh database (no `triples` table yet) and a current-shaped table are
/// both left alone by the guard.
#[test]
fn fresh_and_current_shapes_are_not_refused() {
    let empty = rusqlite::Connection::open_in_memory().expect("in-memory");
    assert!(!sqlite_triples_stale(&empty).expect("stale probe"));
    refuse_legacy_triples_sqlite(&empty).expect("fresh database opens");
    let conn = rusqlite::Connection::open_in_memory().expect("in-memory");
    conn.execute_batch(
        "CREATE TABLE triples (
             id text PRIMARY KEY, bank_id text NOT NULL, subject text NOT NULL,
             predicate text NOT NULL, object_ciphertext text NOT NULL
         );",
    )
    .expect("current shape");
    assert!(!sqlite_triples_stale(&conn).expect("stale probe"));
    refuse_legacy_triples_sqlite(&conn).expect("current shape opens");
}

/// The v11 refusal pin: a legacy-shaped store file refuses to open with the
/// stable error, and the refusal writes nothing — the file keeps the legacy
/// shape, the old version stamp, and every row it had.
#[test]
fn a_legacy_shaped_store_file_refuses_to_open_and_writes_nothing() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let path = std::env::temp_dir().join(format!(
        "am-tripleguard-{}-{}.db",
        std::process::id(),
        crate::test_unique::unique("t"),
    ));
    {
        let store = SqliteStore::open(&path, Arc::clone(&kms)).expect("fresh open");
        // Rebuild the pre-removal shape behind the store's back: a plaintext
        // `object` and no sealed column, version stamp rewound to 11, one
        // triple with a real carrier and one whose carrier item is missing.
        store
            .test_execute(
                "PRAGMA foreign_keys = OFF; \
                 ALTER TABLE triples ADD COLUMN object text; \
                 UPDATE triples SET object = 'hunter2' WHERE object_ciphertext IS NULL; \
                 ALTER TABLE triples DROP COLUMN object_ciphertext; \
                 UPDATE schema_settings SET value = '11' WHERE key = 'schema_version'; \
                 INSERT INTO items (id, bank_id, kind, episodic_type, epistemic_kind, \
                   valid_from, tx_from, created_at, updated_at) VALUES \
                   ('i-1', 'b-guard', 'episodic', 'triple', 'fact', '2024-01-01', \
                   '2024-01-01', '2024-01-01', '2024-01-01'); \
                 INSERT INTO triples (id, bank_id, item_id, subject, predicate, object, \
                   epistemic_kind, valid_from, tx_from, created_at) VALUES \
                   ('t-carried', 'b-guard', 'i-1', 'svc', 'runs_on', 'hunter2', 'fact', \
                   '2024-01-01', '2024-01-01', '2024-01-01'), \
                   ('t-orphan', 'b-guard', 'i-missing', 'svc', 'runs_on_alt', 'hunter2', 'fact', \
                   '2024-01-01', '2024-01-01', '2024-01-01');",
            )
            .expect("rebuild the legacy shape");
    }
    let Err(err) = SqliteStore::open(&path, kms) else {
        panic!("legacy shape must refuse to open");
    };
    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.to_string().contains("legacy plaintext"), "{err}");
    assert!(err.to_string().contains("no longer"), "{err}");
    // No partial write: a raw connection still sees the legacy shape, the
    // version stamp, and both rows.
    let conn = rusqlite::Connection::open(&path).expect("raw reopen");
    let names: Vec<String> = {
        let mut stmt = conn
            .prepare("PRAGMA table_info(triples)")
            .expect("table_info");
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .expect("column rows");
        rows.flatten().collect()
    };
    assert!(names.contains(&"object".to_string()), "{names:?}");
    assert!(
        !names.contains(&"object_ciphertext".to_string()),
        "{names:?}"
    );
    // The remaining no-partial-write evidence: the version stamp stays at 11
    // (a successful open would bump it) and both rows survive.
    let version: String = conn
        .query_row(
            "SELECT value FROM schema_settings WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .expect("version stamp");
    assert_eq!(version, "11", "the version stamp must not move");
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM triples", [], |r| r.get(0))
        .expect("row count");
    assert_eq!(rows, 2, "the refusal must not delete or promote content");
    let _ = std::fs::remove_file(&path);
}
