// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the local dev-KMS SQLite key
//! store: schema round-trips, permissions, damaged-file refusals,
//! fail-closed format handling, and concurrent writers.
//!
//! ## Owns
//! - In-memory and file-backed row operations and conflict safety
//! - Damaged-file and format-version refusals
//! - Multi-writer consistency and the point-lookup query plan
//!
//! ## Does not own
//! - The DEK provider behavior (see `dek_tests`)
//! - The slot operations (see `dek_slots_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the key-store format or its concurrency behavior
//! changes; they never change the implementation.
//!
//! Keep this module focused on the key-store file and rows.

use super::*;
use crate::kms_migrate::install_test_key;
use std::fs;
use std::io::Read;
use std::sync::{Arc, Barrier};

/// Fresh temp directory for one test, cleaned up by the caller.
///
/// A file-backed key store seals what it writes, so every test that reaches one
/// installs the shared process key here. It has to be the process key and not a
/// per-thread one: the concurrency tests below seal on threads they spawned.
fn temp_dir(tag: &str) -> PathBuf {
    install_test_key();
    let dir = std::env::temp_dir().join(format!("clio-kms-db-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

fn active(subject_id: &str, version: u32, byte: u8) -> PersistedSlot {
    PersistedSlot {
        subject_id: subject_id.to_owned(),
        version,
        key_b64: Some(encode_key(&[byte; KEY_LEN])),
    }
}

fn shredded(subject_id: &str, version: u32) -> PersistedSlot {
    PersistedSlot {
        subject_id: subject_id.to_owned(),
        version,
        key_b64: None,
    }
}

/// Test-only access to the open connection.
fn conn(db: &KmsDb) -> &Connection {
    match &db.backing {
        Backing::Memory(conn) => conn,
        Backing::File { conn, .. } => conn.as_ref().expect("open connection"),
    }
}

#[test]
fn in_memory_roundtrip() {
    let mut db = KmsDb::in_memory().unwrap();
    assert_eq!(db.get("s").unwrap(), None);
    db.insert_if_absent("s", 1, &encode_key(&[7u8; KEY_LEN]))
        .unwrap();
    assert_eq!(db.get("s").unwrap(), Some(active("s", 1, 7)));
    db.shred("s").unwrap();
    assert_eq!(db.get("s").unwrap(), Some(shredded("s", 1)));
}

#[test]
fn insert_if_absent_keeps_the_first_key() {
    let mut db = KmsDb::in_memory().unwrap();
    db.insert_if_absent("s", 1, &encode_key(&[1u8; KEY_LEN]))
        .unwrap();
    db.insert_if_absent("s", 1, &encode_key(&[2u8; KEY_LEN]))
        .unwrap();
    assert_eq!(db.get("s").unwrap(), Some(active("s", 1, 1)));
}

#[test]
fn shred_keeps_the_existing_version_and_is_idempotent() {
    let mut db = KmsDb::in_memory().unwrap();
    db.insert_if_absent("s", 4, &encode_key(&[1u8; KEY_LEN]))
        .unwrap();
    db.shred("s").unwrap();
    db.shred("s").unwrap();
    assert_eq!(db.get("s").unwrap(), Some(shredded("s", 4)));
    // A later create must never resurrect a shredded subject.
    db.insert_if_absent("s", 1, &encode_key(&[2u8; KEY_LEN]))
        .unwrap();
    assert_eq!(db.get("s").unwrap(), Some(shredded("s", 4)));
}

#[test]
fn a_memory_row_is_returned_exactly_as_stored() {
    // The backing boundary, not the opener, decides what a memory row is: its
    // rows are never wrapped, so even a value that looks like a broken
    // envelope comes back as stored instead of failing to open.
    let mut db = KmsDb::in_memory().unwrap();
    let stored = format!("{}!!!", clio_config::secret_seal::SEALED_PREFIX);
    insert_slot_if_absent(conn(&db), "s", 1, &stored).unwrap();
    assert_eq!(
        db.get("s").unwrap().unwrap().key_b64.as_deref(),
        Some(stored.as_str())
    );
}

#[test]
fn missing_file_is_not_created_until_a_write() {
    let dir = temp_dir("lazy");
    let path = dir.join("clio.kms.json");
    let mut db = KmsDb::open(path.clone()).unwrap();
    assert_eq!(db.get("s").unwrap(), None);
    assert!(!path.exists(), "a read must not create the store");
    db.insert_if_absent("s", 1, &encode_key(&[3u8; KEY_LEN]))
        .unwrap();
    assert!(path.exists());
    cleanup(&dir);
}

#[test]
fn nested_parent_directories_are_created_on_write() {
    let dir = temp_dir("nested");
    let path = dir.join("deep").join("keys").join("clio.kms.json");
    let mut db = KmsDb::open(path.clone()).unwrap();
    db.insert_if_absent("s", 1, &encode_key(&[3u8; KEY_LEN]))
        .unwrap();
    assert!(path.exists());
    cleanup(&dir);
}

#[cfg(unix)]
#[test]
fn file_store_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("perms");
    let path = dir.join("clio.kms.json");
    let mut db = KmsDb::open(path.clone()).unwrap();
    db.insert_if_absent("s", 1, &encode_key(&[3u8; KEY_LEN]))
        .unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    cleanup(&dir);
}

/// A key-store file that is not a SQLite database must fail open, and the
/// failed open must not rewrite it.
///
/// The `.json` in the resolved key-store name is historical; the store is
/// SQLite. A file holding anything else — for example a JSON document from an
/// older Clio — can never be read as key material, so open fails closed and
/// leaves the operator's file as it is.
#[test]
fn a_key_store_file_that_is_not_a_sqlite_database_fails_closed() {
    let dir = temp_dir("not-sqlite");
    let path = dir.join("clio.kms.json");
    let planted = br#"{"version":1,"slots":[]}"#;
    fs::write(&path, planted).unwrap();

    assert!(KmsDb::open(path.clone()).is_err());

    let mut magic = [0u8; 16];
    fs::File::open(&path)
        .unwrap()
        .read_exact(&mut magic)
        .unwrap();
    assert_ne!(&magic, SQLITE_MAGIC, "the file must stay as it was");
    assert_eq!(fs::read(&path).unwrap(), planted.to_vec());
    cleanup(&dir);
}

#[test]
fn corrupt_file_fails_closed() {
    let dir = temp_dir("corrupt");
    let path = dir.join("clio.kms.json");
    fs::write(&path, b"{not json").unwrap();
    assert!(KmsDb::open(path).is_err());
    cleanup(&dir);
}

#[test]
fn unsupported_store_format_version_fails_closed() {
    let dir = temp_dir("format-version");
    let path = dir.join("clio.kms.json");
    let mut db = KmsDb::open(path.clone()).unwrap();
    db.insert_if_absent("s", 1, &encode_key(&[1u8; KEY_LEN]))
        .unwrap();
    drop(db);
    // A version this Clio has never heard of, which is what a store written by
    // a newer release looks like. Reading it with the wrong assumptions about
    // its values is the one thing the gate exists to prevent.
    let raw = Connection::open(&path).unwrap();
    raw.execute(
        "UPDATE meta SET value = '99' WHERE key = 'format_version'",
        [],
    )
    .unwrap();
    drop(raw);
    assert!(KmsDb::open(path).is_err());
    cleanup(&dir);
}

#[test]
fn a_v1_store_is_refused_with_a_clear_error_and_no_write() {
    let dir = temp_dir("v1-refused");
    let path = dir.join("clio.kms.json");
    let mut db = KmsDb::open(path.clone()).unwrap();
    db.insert_if_absent("s", 1, &encode_key(&[1u8; KEY_LEN]))
        .unwrap();
    drop(db);
    // A store an older Clio wrote: format 1 holds plain base64, which this
    // Clio has no reader for. The gate must refuse it rather than read the
    // base64 as if it were sealed or rewrite it.
    let raw = Connection::open(&path).unwrap();
    raw.execute(
        "UPDATE meta SET value = '1' WHERE key = 'format_version'",
        [],
    )
    .unwrap();
    drop(raw);

    let error = KmsDb::open(path.clone())
        .err()
        .expect("a v1 store must be refused");
    assert!(
        error.message.contains("format version 1"),
        "the error names the refused version: {}",
        error.message
    );

    let raw = Connection::open(&path).unwrap();
    let version: String = raw
        .query_row(
            "SELECT value FROM meta WHERE key = 'format_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "1", "a refused open must not rewrite the store");
    cleanup(&dir);
}

#[test]
fn missing_format_version_fails_closed() {
    let dir = temp_dir("no-format");
    let path = dir.join("clio.kms.json");
    let mut db = KmsDb::open(path.clone()).unwrap();
    db.insert_if_absent("s", 1, &encode_key(&[1u8; KEY_LEN]))
        .unwrap();
    drop(db);
    let raw = Connection::open(&path).unwrap();
    raw.execute("DELETE FROM meta WHERE key = 'format_version'", [])
        .unwrap();
    drop(raw);
    assert!(KmsDb::open(path).is_err());
    cleanup(&dir);
}

#[test]
fn point_lookup_uses_the_primary_key_index() {
    let db = KmsDb::in_memory().unwrap();
    let plan: String = conn(&db)
        .query_row(
            "EXPLAIN QUERY PLAN SELECT version, key_b64 FROM slots WHERE subject_id = ?1",
            params!["s"],
            |row| row.get(3),
        )
        .unwrap();
    assert!(plan.contains("slots"), "unexpected plan: {plan}");
    assert!(
        !plan.contains("SCAN"),
        "point lookup must not scan the table: {plan}"
    );
}

#[test]
fn concurrent_creates_of_the_same_subject_keep_one_key() {
    let dir = temp_dir("concurrent-create");
    let path = dir.join("clio.kms.json");
    let writers = 4usize;
    let subjects = 12usize;
    let barrier = Arc::new(Barrier::new(writers));
    let handles: Vec<_> = (0..writers)
        .map(|w| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut db = KmsDb::open(path).unwrap();
                barrier.wait();
                for subject in 0..subjects {
                    db.insert_if_absent(
                        &format!("sub-{subject}"),
                        1,
                        &encode_key(&[w as u8; KEY_LEN]),
                    )
                    .unwrap();
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let mut db = KmsDb::open(path).unwrap();
    for subject in 0..subjects {
        let slot = db.get(&format!("sub-{subject}")).unwrap().unwrap();
        assert_eq!(slot.version, 1);
        let key = decode_key(slot.key_b64.as_deref().unwrap()).unwrap();
        assert!(
            key[0] < writers as u8,
            "stored key must come from one writer"
        );
    }
    cleanup(&dir);
}

#[test]
fn concurrent_create_and_shred_stays_consistent() {
    let dir = temp_dir("concurrent-race");
    let path = dir.join("clio.kms.json");
    for round in 0..20u32 {
        let subject = format!("race-{round}");
        let create = {
            let path = path.clone();
            let subject = subject.clone();
            std::thread::spawn(move || {
                let mut db = KmsDb::open(path).unwrap();
                db.insert_if_absent(&subject, 1, &encode_key(&[9u8; KEY_LEN]))
            })
        };
        let shred = {
            let path = path.clone();
            let subject = subject.clone();
            std::thread::spawn(move || {
                let mut db = KmsDb::open(path).unwrap();
                db.shred(&subject)
            })
        };
        create.join().unwrap().unwrap();
        shred.join().unwrap().unwrap();
        let mut db = KmsDb::open(path.clone()).unwrap();
        let slot = db.get(&subject).unwrap().unwrap();
        if let Some(encoded) = slot.key_b64 {
            assert_eq!(decode_key(&encoded).unwrap(), [9u8; KEY_LEN]);
        } else {
            // A shred is terminal: a later create must not resurrect it.
            db.insert_if_absent(&subject, 1, &encode_key(&[7u8; KEY_LEN]))
                .unwrap();
            assert_eq!(db.get(&subject).unwrap().unwrap().key_b64, None);
        }
    }
    cleanup(&dir);
}
