// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the store-injected sync key
//! lifecycle: loading, generating, deleting, and reporting from either the
//! file store or an injected keystore, including the archived keys a report
//! shows when the current key is absent.
//!
//! ## Owns
//! - Every test of `load_with`, `generate_with`, `delete_with`, and
//!   `report_with`
//! - The report's archived entries beside an absent current key
//!
//! ## Does not own
//! - The env-driven lifecycle wrappers (see `sync_key_tests` and
//!   `sync_key_env_tests`)
//! - The archive file's format and rotation rules (see
//!   `sync_key_rotation_tests`)
//! - The keystore wrapper itself (see `secret_key_store_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. The keystore tests drive
//! the production wrapper through an injected in-memory credential, so no test
//! here can reach the machine's real Keychain or Secret Service.
//!
//! Keep this module focused on the store-injected lifecycle variants.

use base64::Engine as _;

use super::*;
use crate::secret_key::KEY_LEN;
use crate::secret_key_store::{KeyringStore, NoKeystore, SYNC_ACCOUNT, SYNC_KEY};
use crate::secret_key_store_test_support::{FakeCredential, store_over_credential};
use crate::sync_key_rotation::{ARCHIVE_STORE_LABEL, ArchivedSyncKey, append_entry};
use crate::sync_key_test_support::TempDir;
use crate::sync_key_value::fingerprint_of_base64;

/// A store the keystore rung reads through, wired to one credential so a test
/// can watch what the lifecycle wrote.
fn keystore_over(credential: &FakeCredential) -> KeyringStore {
    store_over_credential(SYNC_ACCOUNT, SYNC_KEY, credential.clone())
}

/// One valid archived key: 32 real bytes in standard base64, so the report's
/// fingerprint derivation accepts it.
fn archived_entry() -> ArchivedSyncKey {
    ArchivedSyncKey {
        key_b64: base64::engine::general_purpose::STANDARD.encode([9_u8; KEY_LEN]),
        created_at: Some("2026-02-02T00:00:00Z".to_owned()),
    }
}

/// Seed the archive beside `dir.key()` with one valid entry.
fn seed_archive(dir: &TempDir) -> ArchivedSyncKey {
    let entry = archived_entry();
    append_entry(Some(&dir.key()), &entry).expect("append the archived key");
    entry
}

// ---------------------------------------------------------------- loading

#[test]
fn a_written_file_key_loads_back_with_its_location() {
    let dir = TempDir::new("load-file");
    file_store::write_key(&dir.key(), &[7_u8; KEY_LEN]).expect("seed the key file");

    let loaded = load_with(&NoKeystore, KeySource::File, Some(dir.key()))
        .expect("load")
        .expect("a key exists");

    assert_eq!(*loaded.expose(), [7_u8; KEY_LEN]);
    assert_eq!(loaded.location, SyncKeyLocation::File(dir.key()));
}

#[test]
fn a_missing_file_key_loads_as_nothing() {
    let dir = TempDir::new("load-file-absent");

    assert!(
        load_with(&NoKeystore, KeySource::File, Some(dir.key()))
            .expect("load")
            .is_none(),
        "no file means no key"
    );
    assert!(
        load_with(&NoKeystore, KeySource::File, None)
            .expect("load")
            .is_none(),
        "no resolved path means no key"
    );
}

#[test]
fn a_keystore_key_loads_back_through_the_injected_entry() {
    let store = keystore_over(&FakeCredential::holding(&[9_u8; KEY_LEN]));

    let loaded = load_with(&store, KeySource::Keystore, None)
        .expect("load")
        .expect("a key exists");

    assert_eq!(*loaded.expose(), [9_u8; KEY_LEN]);
    assert_eq!(loaded.location, SyncKeyLocation::Keystore);
}

#[test]
fn an_empty_keystore_loads_as_nothing() {
    let store = keystore_over(&FakeCredential::empty());

    assert!(
        load_with(&store, KeySource::Keystore, None)
            .expect("load")
            .is_none()
    );
}

// ------------------------------------------------------------ generating

#[test]
fn a_generated_file_key_reads_back_and_records_its_creation_time() {
    let dir = TempDir::new("generate-file");

    let key = generate_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("generate");

    let loaded = load_with(&NoKeystore, KeySource::File, Some(dir.key()))
        .expect("load")
        .expect("a key exists");
    assert_eq!(loaded.expose(), key.expose());
    let stamp = file_store::read_created(Some(&dir.key()))
        .expect("read")
        .expect("the sidecar recorded a time");
    assert!(
        stamp.len() >= 20 && stamp.ends_with('Z') && stamp.contains('T'),
        "a UTC RFC 3339 stamp, whatever second it names: {stamp}"
    );
}

#[test]
fn generation_refuses_a_file_that_already_holds_a_key() {
    let dir = TempDir::new("generate-file-exists");
    let first = generate_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("first");

    let err = generate_with(&NoKeystore, KeySource::File, Some(dir.key()))
        .expect_err("a second key is refused");

    assert!(err.message.contains("already exists"), "{}", err.message);
    let loaded = load_with(&NoKeystore, KeySource::File, Some(dir.key()))
        .expect("load")
        .expect("the first key is still there");
    assert_eq!(loaded.expose(), first.expose());
}

#[test]
fn a_generated_keystore_key_reaches_the_injected_entry() {
    let credential = FakeCredential::empty();
    let store = keystore_over(&credential);

    let key = generate_with(&store, KeySource::Keystore, None).expect("generate");

    assert_eq!(
        credential.secret().expect("stored bytes"),
        key.expose().to_vec()
    );
}

#[test]
fn keystore_generation_refuses_an_existing_entry() {
    let store = keystore_over(&FakeCredential::holding(&[9_u8; KEY_LEN]));

    let err = generate_with(&store, KeySource::Keystore, None)
        .expect_err("an existing key is not replaced");

    assert!(err.message.contains("already exists"), "{}", err.message);
}

#[test]
fn keystore_generation_refuses_a_shadowing_key_file() {
    // A key file beside a keystore key is a second copy of the same secret, so
    // the generation refusal covers it for both stores.
    let dir = TempDir::new("generate-keystore-shadow");
    file_store::write_key(&dir.key(), &[7_u8; KEY_LEN]).expect("seed a shadowing file");
    let store = keystore_over(&FakeCredential::empty());

    let err = generate_with(&store, KeySource::Keystore, Some(dir.key()))
        .expect_err("a second copy of the same secret is refused");

    assert!(err.message.contains("already exists"), "{}", err.message);
    assert!(dir.key().exists(), "a refused generation touches nothing");
}

// -------------------------------------------------------------- deleting

#[test]
fn deleting_a_file_key_leaves_nothing_behind_and_is_repeatable() {
    let dir = TempDir::new("delete-file");
    generate_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("generate");

    delete_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("delete");
    delete_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("deleting nothing");

    assert!(
        load_with(&NoKeystore, KeySource::File, Some(dir.key()))
            .expect("load")
            .is_none()
    );
}

#[test]
fn deleting_a_keystore_key_empties_the_entry() {
    let credential = FakeCredential::holding(&[9_u8; KEY_LEN]);
    let store = keystore_over(&credential);

    delete_with(&store, KeySource::Keystore, None).expect("delete");

    assert!(credential.secret().is_none());
}

// -------------------------------------------------------------- reporting

#[test]
fn a_file_report_names_the_store_the_fingerprint_and_the_count() {
    let dir = TempDir::new("report-file");
    let key = generate_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("generate");

    let report = report_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("report");

    assert_eq!(report.store, KeySource::File);
    assert_eq!(
        report.fingerprint.as_deref(),
        Some(key.fingerprint().as_str())
    );
    assert_eq!(report.count, 1);
    assert!(report.keys[0].current);
}

#[test]
fn a_report_with_no_key_and_no_archive_says_zero() {
    let dir = TempDir::new("report-empty");

    let report = report_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("report");

    assert_eq!(report.store, KeySource::File);
    assert_eq!(report.fingerprint, None);
    assert_eq!(report.created_at, None);
    assert_eq!(report.count, 0);
    assert!(report.keys.is_empty(), "{:?}", report.keys);
}

#[test]
fn a_keystore_report_names_the_stored_key() {
    let store = keystore_over(&FakeCredential::holding(&[9_u8; KEY_LEN]));

    let report = report_with(&store, KeySource::Keystore, None).expect("report");

    assert_eq!(report.store, KeySource::Keystore);
    assert_eq!(report.count, 1);
    assert_eq!(
        report.fingerprint.as_deref(),
        Some(
            fingerprint_of_base64(
                &base64::engine::general_purpose::STANDARD.encode([9_u8; KEY_LEN])
            )
            .expect("fingerprint")
            .as_str()
        )
    );
}

#[test]
fn a_report_shows_the_archived_key_when_the_current_key_file_is_absent() {
    // The state an interrupted rotation leaves behind: the archive succeeded
    // and the new-key write failed, so the node holds keys but no current one.
    // The count must say what the node holds, not zero.
    let dir = TempDir::new("report-archive-only");
    let entry = seed_archive(&dir);

    let report = report_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("report");

    assert_eq!(report.store, KeySource::File);
    assert_eq!(report.count, 1, "the archived key is held");
    assert_eq!(report.fingerprint, None, "no current key exists");
    assert_eq!(report.keys.len(), 1);
    assert_eq!(
        report.keys[0].fingerprint,
        fingerprint_of_base64(&entry.key_b64).expect("fingerprint")
    );
    assert_eq!(report.keys[0].created_at, entry.created_at);
    assert!(!report.keys[0].current);
    assert_eq!(report.keys[0].store, ARCHIVE_STORE_LABEL);
}

#[test]
fn a_keystore_report_shows_the_archived_key_when_the_store_is_empty() {
    // Same interrupted-rotation shape, keystore rung: the store answers empty
    // and the archive beside the resolved key-file path still holds the key.
    let dir = TempDir::new("report-keystore-archive");
    let entry = seed_archive(&dir);

    let report = report_with(&NoKeystore, KeySource::Keystore, Some(dir.key())).expect("report");

    assert_eq!(report.store, KeySource::Keystore);
    assert_eq!(report.count, 1, "the archived key is held");
    assert_eq!(report.fingerprint, None, "no current key exists");
    assert_eq!(report.keys.len(), 1);
    assert_eq!(
        report.keys[0].fingerprint,
        fingerprint_of_base64(&entry.key_b64).expect("fingerprint")
    );
    assert_eq!(report.keys[0].created_at, entry.created_at);
    assert_eq!(report.keys[0].store, ARCHIVE_STORE_LABEL);
}
