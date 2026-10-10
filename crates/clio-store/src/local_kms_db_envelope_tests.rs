// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the envelope boundary of the local
//! dev-KMS key store: what a file-backed store seals at rest and opens through
//! the strict opener, and what happens to a row that cannot open.
//!
//! ## Owns
//! - The sealed-at-rest and open-through-the-opener behavior of a file store
//! - The clear, masked failure of a malformed sealed row, with no plaintext
//!   fallback
//!
//! ## Does not own
//! - The envelope format itself (see `clio-config`'s `secret_seal_tests`)
//! - Row operations, the schema, and the format gate (see `local_kms_db_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the envelope boundary between memory rows and file
//! rows changes; they never change the implementation.
//!
//! Keep this module focused on the file store's envelope boundary.

use super::*;
use crate::kms_migrate::install_test_key;
use std::fs;

/// Fresh temp directory for one test, cleaned up by the caller.
///
/// A file-backed key store seals what it writes, so every test here installs
/// the shared process key, exactly as `local_kms_db_tests` does.
fn temp_dir(tag: &str) -> PathBuf {
    install_test_key();
    let dir =
        std::env::temp_dir().join(format!("clio-kms-db-envelope-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn cleanup(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

/// Test-only access to the open connection.
fn conn(db: &KmsDb) -> &Connection {
    match &db.backing {
        Backing::Memory(conn) => conn,
        Backing::File { conn, .. } => conn.as_ref().expect("open connection"),
    }
}

#[test]
fn a_file_store_seals_at_rest_and_opens_through_the_opener() {
    let dir = temp_dir("file-envelope");
    let path = dir.join("clio.kms.json");
    let plain = encode_key(&[4u8; KEY_LEN]);
    let mut db = KmsDb::open(path.clone()).unwrap();
    db.insert_if_absent("s", 1, &plain).unwrap();
    let stored: String = conn(&db)
        .query_row(
            "SELECT key_b64 FROM slots WHERE subject_id = 's'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        clio_config::secret_seal::is_sealed(&stored),
        "the key is raw at rest: {stored}"
    );
    assert!(!stored.contains(&plain), "the plaintext is in the row");
    assert_eq!(
        db.get("s").unwrap(),
        Some(PersistedSlot {
            subject_id: "s".to_owned(),
            version: 1,
            key_b64: Some(plain),
        })
    );
    cleanup(&dir);
}

#[test]
fn a_malformed_sealed_row_fails_without_a_plaintext_fallback() {
    let dir = temp_dir("bad-envelope");
    let path = dir.join("clio.kms.json");
    let mut db = KmsDb::open(path.clone()).unwrap();
    db.insert_if_absent("s", 1, &encode_key(&[4u8; KEY_LEN]))
        .unwrap();
    let bad = format!("{}!!!", clio_config::secret_seal::SEALED_PREFIX);
    conn(&db)
        .execute(
            "UPDATE slots SET key_b64 = ?1 WHERE subject_id = 's'",
            params![bad],
        )
        .unwrap();
    let error = db.get("s").unwrap_err();
    assert_eq!(error.code, ErrorCode::ConfigCorrupt);
    assert!(
        !error.message.contains(&encode_key(&[4u8; KEY_LEN])),
        "the message echoes the key: {}",
        error.message
    );
    // No fallback: the broken row never comes back as if it were plaintext.
    assert!(db.get("s").is_err());
    cleanup(&dir);
}
