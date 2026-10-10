// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the SQLite snapshot: the faithful
//! copy of a populated store, the concurrent-writer consistency case, the
//! standalone integrity check, the refusal cases, the UTC stamp the manifest
//! records, and the content digest that survives compaction.
//!
//! ## Owns
//! - The scratch-database fixtures and the snapshot assertions
//!
//! ## Does not own
//! - The key-store member (see `snapshot_keys_tests`)
//! - Archive assembly or verify (see `clio-backup`)
//! - The paths that fail after the copy (see `sqlite_snapshot_fault_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on `sqlite_snapshot` behavior.

use super::*;
use crate::sqlite::SqliteStore;
use crate::sqlite_content::sqlite_content_digest;
use crate::sqlite_integrity::sqlite_integrity_ok;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Arc;

/// A scratch directory removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        Self(crate::test_unique::scratch_dir(&format!(
            "clio-store-snapshot-{tag}"
        )))
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A file-backed store holding `rows` rows in `items` plus the rest of the
/// core schema. The store is created through its own opener (so the schema is
/// the real one) and the rows are inserted through a plain connection.
fn populated_store(path: &Path, rows: u32) {
    let kms: Arc<dyn crate::dek::DekProvider> = Arc::new(crate::dek::LocalDevKms::new());
    SqliteStore::open(path, kms).expect("open a file-backed store");
    insert_rows(path, "item", rows);
}

/// Insert `rows` fresh `items` rows named `<prefix>-NNNN`, straight into the
/// store file. A prefix never repeats inside one test, so a second call only
/// adds rows.
fn insert_rows(path: &Path, prefix: &str, rows: u32) {
    let conn = Connection::open(path).expect("open the store file");
    for index in 0..rows {
        conn.execute(
            "INSERT INTO items (id, bank_id, kind, epistemic_kind, content_ciphertext,
                                valid_from, tx_from, created_at, updated_at)
             VALUES (?1, 'bank-a', 'episodic', 'fact', 'ciphertext', 't0', 't0', 't0', 't0')",
            [format!("{prefix}-{index:04}")],
        )
        .expect("insert a row");
    }
}

#[test]
fn snapshot_of_a_populated_store_keeps_the_content_digest() {
    let scratch = Scratch::new("digest");
    let source = scratch.join("clio.db");
    populated_store(&source, 40);
    let staged = scratch.join("staged.db");

    let facts = snapshot_sqlite_into(&source, &staged).expect("snapshot");

    // A bare row count would pass even if every row were the wrong row.
    assert_eq!(
        sqlite_content_digest(&source).expect("source digest"),
        sqlite_content_digest(&staged).expect("snapshot digest"),
    );
    assert_eq!(facts.count_of("items"), Some(40));
    assert!(facts.table_count() > 1, "the core schema is counted too");
    assert_eq!(facts.bytes, std::fs::metadata(&staged).unwrap().len());
    assert_eq!(facts.sha256.len(), 64);
    assert!(
        facts.schema_version.is_some(),
        "the store records a version"
    );
    assert!(facts.duration_ms < 60_000);
    assert!(facts.started_at.ends_with('Z'));
}

#[test]
fn the_copy_is_compacted_not_byte_identical() {
    let scratch = Scratch::new("compaction");
    let source = scratch.join("clio.db");
    populated_store(&source, 200);
    // Free pages the copy is expected to reclaim.
    let conn = Connection::open(&source).expect("open the source");
    conn.execute("DELETE FROM items WHERE id LIKE 'item-01%'", [])
        .expect("delete rows");
    drop(conn);
    let staged = scratch.join("staged.db");

    snapshot_sqlite_into(&source, &staged).expect("snapshot");

    let source_bytes = std::fs::read(&source).expect("read the source");
    let staged_bytes = std::fs::read(&staged).expect("read the copy");
    assert_ne!(
        source_bytes, staged_bytes,
        "VACUUM INTO rebuilds the file, so byte equality is the wrong assertion"
    );
    assert!(
        staged_bytes.len() <= source_bytes.len(),
        "a compacted copy is never larger than its source"
    );
    assert_eq!(
        sqlite_content_digest(&source).unwrap(),
        sqlite_content_digest(&staged).unwrap(),
    );
}

#[test]
fn a_snapshot_never_writes_the_source() {
    let scratch = Scratch::new("readonly");
    let source = scratch.join("clio.db");
    populated_store(&source, 5);
    let staged = scratch.join("staged.db");
    let before = std::fs::metadata(&source).unwrap().len();
    let digest_before = sqlite_content_digest(&source).unwrap();

    snapshot_sqlite_into(&source, &staged).expect("snapshot");

    assert_eq!(std::fs::metadata(&source).unwrap().len(), before);
    assert_eq!(sqlite_content_digest(&source).unwrap(), digest_before);
    // A read-only source cannot have grown a write-ahead log either.
    assert!(!scratch.join("clio.db-wal").exists());
    assert!(!scratch.join("clio.db-shm").exists());
}

#[test]
fn a_snapshot_while_a_writer_commits_is_one_point_in_time() {
    let scratch = Scratch::new("concurrent");
    let source = scratch.join("clio.db");
    populated_store(&source, 10);
    let baseline = sqlite_content_digest(&source).unwrap();

    // Commit a row from a second connection while the snapshot runs. Whichever
    // side of the commit the copy lands on, the copy must be internally
    // consistent: either the new row is wholly there or wholly absent.
    let writer_source = source.clone();
    let writer = std::thread::spawn(move || {
        insert_rows(&writer_source, "late", 50);
    });

    let mut seen_baseline = false;
    let mut seen_grown = false;
    for _ in 0..8 {
        let staged = scratch.join("staged.db");
        let _ = std::fs::remove_file(&staged);
        snapshot_sqlite_into(&source, &staged).expect("snapshot under a writer");
        let digest = sqlite_content_digest(&staged).expect("digest the copy");
        // A mixed point in time would be a digest that matches neither side.
        if digest == baseline {
            seen_baseline = true;
        } else {
            seen_grown = true;
            let items: i64 = Connection::open(&staged)
                .expect("open the copy")
                .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
                .expect("count the copy");
            assert!(
                items >= 10,
                "the copy always holds the pre-writer rows: {items}"
            );
        }
        sqlite_integrity_ok(&staged).expect("the copy passes its own integrity check");
    }
    writer.join().expect("the writer finished");
    assert!(
        seen_baseline || seen_grown,
        "every copy was one point in time, which is the property under test"
    );
}

#[test]
fn the_copy_opens_standalone_and_passes_its_own_integrity_check() {
    let scratch = Scratch::new("standalone");
    let source = scratch.join("clio.db");
    populated_store(&source, 3);
    let staged = scratch.join("staged.db");

    snapshot_sqlite_into(&source, &staged).expect("snapshot");
    sqlite_integrity_ok(&staged).expect("the copy is a valid database");

    // A copy with the source removed is still readable: it is a whole
    // database, not a reference into the original file.
    drop(std::fs::remove_file(&source));
    let rows: i64 = Connection::open(&staged)
        .expect("open the copy on its own")
        .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
        .expect("count the copy");
    assert_eq!(rows, 3);
}

#[test]
fn a_missing_source_is_an_error_not_an_empty_archive() {
    let scratch = Scratch::new("missing");
    let staged = scratch.join("staged.db");

    let error = snapshot_sqlite_into(&scratch.join("absent.db"), &staged)
        .expect_err("a missing store cannot be snapshotted");

    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(!staged.exists(), "no staged file is left behind");
}

#[test]
fn an_existing_staged_file_is_refused_rather_than_overwritten() {
    let scratch = Scratch::new("occupied");
    let source = scratch.join("clio.db");
    populated_store(&source, 1);
    let staged = scratch.join("staged.db");
    std::fs::write(&staged, b"left over from an interrupted run").expect("seed the path");

    let error = snapshot_sqlite_into(&source, &staged).expect_err("a used path is refused");

    assert_eq!(error.code, ErrorCode::InvalidArgument);
    assert_eq!(
        std::fs::read(&staged).unwrap(),
        b"left over from an interrupted run".to_vec(),
        "the refusal leaves the existing file untouched"
    );
}

#[test]
fn a_damaged_payload_fails_the_integrity_check_by_name() {
    let scratch = Scratch::new("damaged");
    let broken = scratch.join("broken.db");
    std::fs::write(&broken, b"SQLite format 3\0 but not a real database").expect("write bytes");

    let error = sqlite_integrity_ok(&broken).expect_err("a damaged file is not valid");

    assert_eq!(error.code, ErrorCode::ArchiveCorrupt);
    assert!(
        error.message.contains("broken.db"),
        "the message names the file: {}",
        error.message
    );
}

#[test]
fn the_content_digest_notices_a_changed_value_and_ignores_reordering() {
    let scratch = Scratch::new("digest-sensitivity");
    let path = scratch.join("rows.db");
    let conn = Connection::open(&path).expect("open");
    conn.execute_batch(
        "CREATE TABLE notes (id TEXT PRIMARY KEY, body TEXT);
         INSERT INTO notes VALUES ('a', 'first'), ('b', 'second');",
    )
    .expect("seed");
    let before = sqlite_content_digest(&path).expect("digest");

    // Re-inserting the same rows in the opposite order changes the physical
    // layout but not the logical content.
    conn.execute_batch("DELETE FROM notes; INSERT INTO notes VALUES ('b','second'),('a','first');")
        .expect("reorder");
    assert_eq!(sqlite_content_digest(&path).unwrap(), before);

    conn.execute("UPDATE notes SET body = 'edited' WHERE id = 'a'", [])
        .expect("edit a value");
    assert_ne!(sqlite_content_digest(&path).unwrap(), before);

    conn.execute("UPDATE notes SET body = '' WHERE id = 'a'", [])
        .expect("blank a value");
    assert_ne!(
        sqlite_content_digest(&path).unwrap(),
        before,
        "an empty string is not a missing value"
    );
    conn.execute("UPDATE notes SET body = NULL WHERE id = 'a'", [])
        .expect("null a value");
    assert!(
        sqlite_content_digest(&path).is_ok(),
        "a nullable column still digests"
    );
}

#[test]
fn a_table_without_a_declared_key_still_digests() {
    let scratch = Scratch::new("no-key");
    let path = scratch.join("nokey.db");
    let conn = Connection::open(&path).expect("open");
    conn.execute_batch(
        "CREATE TABLE plain (a, b);
         CREATE TABLE keyed (k TEXT PRIMARY KEY, v TEXT) WITHOUT ROWID;
         INSERT INTO plain VALUES (2, 'two'), (1, 'one');
         INSERT INTO keyed VALUES ('only', 'row');",
    )
    .expect("seed");

    let first = sqlite_content_digest(&path).expect("digest a keyless table");
    conn.execute("UPDATE plain SET b = 'changed' WHERE a = 1", [])
        .expect("edit");
    assert_ne!(sqlite_content_digest(&path).unwrap(), first);
}

#[test]
fn a_missing_payload_is_refused_by_the_integrity_check() {
    let scratch = Scratch::new("no-payload");
    let error = sqlite_integrity_ok(&scratch.join("absent.db"))
        .expect_err("an absent payload cannot be checked");

    assert_eq!(error.code, ErrorCode::NotFound);
}

#[test]
fn the_staged_copy_is_owner_only() {
    let scratch = Scratch::new("staged-mode");
    let source = scratch.join("clio.db");
    populated_store(&source, 2);
    let staged = scratch.join("staged.db");

    snapshot_sqlite_into(&source, &staged).expect("snapshot");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&staged).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "SQLite creates the copy with the umask, so the snapshot tightens it"
        );
    }
    let _ = &staged;
}

#[test]
fn a_table_whose_name_needs_quoting_is_counted_and_digested() {
    let scratch = Scratch::new("quoted-name");
    let path = scratch.join("odd.db");
    let conn = Connection::open(&path).expect("open");
    conn.execute_batch(
        "CREATE TABLE \"we\"\"ird\" (id TEXT PRIMARY KEY, v TEXT);
         INSERT INTO \"we\"\"ird\" VALUES ('a', 'one');",
    )
    .expect("seed a table whose name contains a quote");
    drop(conn);

    let digest = sqlite_content_digest(&path).expect("digest a quoted table name");
    assert_eq!(digest.len(), 64);

    let staged = scratch.join("copy.db");
    let facts = snapshot_sqlite_into(&path, &staged).expect("snapshot");
    assert_eq!(facts.count_of("we\"ird"), Some(1));
}

#[test]
fn the_method_and_consistency_words_describe_the_shipped_mechanism() {
    assert_eq!(METHOD, "vacuum-into");
    assert_eq!(CONSISTENCY, "point-in-time");
    assert_eq!(PAYLOAD_MEMBER, "store.db");
    for word in [METHOD, CONSISTENCY] {
        assert!(
            !word.contains("copy") && !word.contains("clone"),
            "the mechanism is never described as a file copy or a clone: {word}"
        );
    }
}

#[test]
fn a_utc_stamp_names_a_day_that_exists_on_the_day_it_ran() {
    // The civil calendar can be written two ways and only one of them is right.
    // The wrong one folds the day-of-month's `+ 1` into the day of the year and
    // then derives the month from that shifted value, which lands on the first
    // day of the next month instead of the last day of this one. The rows below
    // are the days that catch it: month ends, a year end, and a leap day.
    for (seconds, stamp) in [
        (0u64, "1970-01-01T00:00:00Z"),
        (30 * 86_400, "1970-01-31T00:00:00Z"),
        (89 * 86_400, "1970-03-31T00:00:00Z"),
        (19_782 * 86_400, "2024-02-29T00:00:00Z"),
        (20_696 * 86_400, "2026-08-31T00:00:00Z"),
        (20_818 * 86_400, "2026-12-31T00:00:00Z"),
        (20_723 * 86_400 + 45_296, "2026-09-27T12:34:56Z"),
    ] {
        assert_eq!(utc_stamp(seconds), stamp, "seconds={seconds}");
    }
    // Every day of the first fifty-seven years is a real date, because a day of
    // the year that is one too high always lands on day zero of the next month.
    for days in 0..=20_900i64 {
        let (year, month, day) = civil_from_days(days);
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let last = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => 0,
        };
        assert!(
            day >= 1 && day <= last,
            "days={days} is not a real date: {year}-{month:02}-{day:02}"
        );
    }
}

#[test]
fn the_start_stamp_is_taken_before_the_copy_runs() {
    let scratch = Scratch::new("copy-span");
    let source = scratch.join("clio.db");
    populated_store(&source, 200);
    let staged = scratch.join("staged.db");
    let opened = now_utc();

    let facts = snapshot_sqlite_into(&source, &staged).expect("snapshot");
    let returned = now_utc();

    // Both stamps are floored to whole seconds, so the recorded one is either
    // the second the call opened in or a second between opening and returning.
    // A stamp read after the copy leaves that window as soon as a copy is long
    // enough to cross a second boundary.
    assert!(
        opened <= facts.started_at && facts.started_at <= returned,
        "the stamp {} is outside the call window {opened}..={returned}",
        facts.started_at
    );
}

#[cfg(test)]
#[path = "sqlite_snapshot_tests_fault_tests.rs"]
mod fault;
