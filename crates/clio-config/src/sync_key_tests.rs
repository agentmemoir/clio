// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the sync key's lifecycle: generation
//! into either store, reading back, deleting, the report, and the recorded store
//! choice.
//!
//! ## Owns
//! - Every test of `sync_key`'s generate, load, delete, report, and marker rules
//!
//! ## Does not own
//! - The raw file and sidecar behaviour (see `sync_key_file_tests`)
//! - The loaded key and the fingerprint (see `sync_key_value_tests`)
//! - The keystore wrapper itself (see `secret_key_store_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. The keystore tests drive
//! the production wrapper through an injected in-memory credential, so no test
//! here can reach the machine's real Keychain or Secret Service.
//!
//! Keep this module focused on the lifecycle rules.

use super::*;
use crate::secret_key::KEY_LEN;
use crate::secret_key_store_test_support::{
    FakeCredential, PlainCredential, store_over_credential,
};
use crate::sync_key_test_support::TempDir;

/// An injected keystore that keeps attributes, as the Linux backend does.
fn attribute_store() -> KeyringStore {
    store_over_credential(SYNC_ACCOUNT, SYNC_KEY, FakeCredential::empty())
}

/// An injected keystore backed by one credential, so a test can read it back.
fn attribute_store_over(credential: &FakeCredential) -> KeyringStore {
    store_over_credential(SYNC_ACCOUNT, SYNC_KEY, credential.clone())
}

/// The keystore's account and the encryption key's, which must never converge.
#[test]
fn the_two_key_roles_have_separate_accounts_and_the_data_key_did_not_move() {
    use crate::secret_key::{KEY_FILE_ENV, KEY_FILE_NAME};
    use crate::secret_key_store::{ACCOUNT, SERVICE};

    assert_eq!(SERVICE, "clio");
    assert_eq!(
        ACCOUNT, "encryption-key",
        "the data key's account is frozen"
    );
    assert_eq!(SYNC_ACCOUNT, "sync-key");
    assert_ne!(
        SYNC_ACCOUNT, ACCOUNT,
        "one key must not answer for the other"
    );
    assert_eq!(KEY_FILE_NAME, "clio.key", "the data key's file is frozen");
    assert_eq!(KEY_FILE_ENV, "CLIO_KEY_FILE");
    assert_eq!(
        secret_key_source::MARKER_NAME,
        "key-source",
        "the data key's marker is frozen"
    );
    assert_eq!(
        secret_key_source::SYNC_MARKER_NAME,
        "sync-key-source",
        "the sync key's marker is its own file, never the data key's"
    );
    assert_ne!(
        secret_key_source::MARKER_NAME,
        secret_key_source::SYNC_MARKER_NAME
    );
}

// ---------------------------------------------------------------- file store

#[test]
fn a_generated_file_key_reads_back_byte_for_byte() {
    let dir = TempDir::new("generate-file");
    let key = generate_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("generate");

    let loaded = load_with(&NoKeystore, KeySource::File, Some(dir.key()))
        .expect("load")
        .expect("a key exists");

    assert_eq!(loaded.expose(), key.expose());
    assert_eq!(loaded.location, SyncKeyLocation::File(dir.key()));
}

#[test]
fn generation_refuses_to_replace_a_file_key() {
    let dir = TempDir::new("replace-file");
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
fn deleting_a_file_key_makes_the_next_load_empty_and_is_repeatable() {
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

// ------------------------------------------------------------- keystore

#[test]
fn a_generated_keystore_key_reads_back_through_the_injected_entry() {
    let credential = FakeCredential::empty();
    let store = attribute_store_over(&credential);

    let generated =
        generate_with(&store, KeySource::Keystore, None).expect("generate into the store");

    assert_eq!(
        credential.secret().expect("stored bytes"),
        generated.expose().to_vec()
    );
    let loaded = load_with(&store, KeySource::Keystore, None)
        .expect("load")
        .expect("a key exists");
    assert_eq!(loaded.expose(), generated.expose());
}

#[test]
fn keystore_generation_refuses_an_existing_entry() {
    let store = attribute_store_over(&FakeCredential::holding(&[9_u8; KEY_LEN]));

    let err = generate_with(&store, KeySource::Keystore, None)
        .expect_err("an existing key is not replaced");

    assert!(err.message.contains("already exists"), "{}", err.message);
}

#[test]
fn a_wrong_length_keystore_entry_is_refused() {
    let store = attribute_store_over(&FakeCredential::holding(&[0_u8; 5]));

    let err = load_with(&store, KeySource::Keystore, None).expect_err("short key");

    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
}

#[test]
fn an_unreachable_keystore_is_an_error_not_an_absent_key() {
    let store = store_over_credential(SYNC_ACCOUNT, SYNC_KEY, FakeCredential::locked_on_read());

    let err = load_with(&store, KeySource::Keystore, None).expect_err("locked store");

    assert_eq!(err.code, clio_types::ErrorCode::Internal);
    assert!(err.message.contains(SYNC_KEY), "{}", err.message);
}

// ------------------------------------------------------------------ report

#[test]
fn a_file_report_names_the_store_the_fingerprint_the_time_and_the_count() {
    let dir = TempDir::new("report-file");
    let key = generate_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("generate");

    let report = report_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("report");

    assert_eq!(report.store, KeySource::File);
    assert_eq!(
        report.fingerprint.as_deref(),
        Some(key.fingerprint().as_str())
    );
    assert_eq!(report.count, 1);
    let stamp = report.created_at.expect("the sidecar recorded a time");
    assert!(
        stamp.len() >= 20 && stamp.ends_with('Z') && stamp.contains('T'),
        "a UTC RFC 3339 stamp, whatever second it names: {stamp}"
    );
    assert!(
        !stamp.contains(&key.base64()),
        "the report must not hold the key"
    );
}

#[test]
fn a_report_with_no_key_says_zero_and_no_fingerprint() {
    let dir = TempDir::new("report-empty");

    let report = report_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("report");

    assert_eq!(report.store, KeySource::File);
    assert_eq!(report.fingerprint, None);
    assert_eq!(report.created_at, None);
    assert_eq!(report.count, 0);
}

#[test]
fn a_store_that_keeps_attributes_reports_a_creation_time() {
    let store = attribute_store();
    let key = generate_with(&store, KeySource::Keystore, None).expect("generate");

    let report = report_with(&store, KeySource::Keystore, None).expect("report");

    assert_eq!(
        report.fingerprint.as_deref(),
        Some(key.fingerprint().as_str())
    );
    assert_eq!(report.count, 1);
    let stamp = report.created_at.expect("the store kept the attribute");
    assert!(
        stamp.len() >= 20 && stamp.ends_with('Z') && stamp.contains('T'),
        "a UTC RFC 3339 stamp, never an invented value: {stamp}"
    );
}

#[test]
fn a_store_that_ignores_attributes_reports_unavailable_without_erroring() {
    let store = store_over_credential(
        SYNC_ACCOUNT,
        SYNC_KEY,
        PlainCredential::without_attributes(),
    );
    let _ = generate_with(&store, KeySource::Keystore, None).expect("generate");

    let report = report_with(&store, KeySource::Keystore, None).expect("report");

    assert_eq!(
        report.created_at, None,
        "an ignored attribute is unavailable"
    );
    assert_eq!(report.count, 1);
    assert!(report.fingerprint.is_some());
}

#[test]
fn a_missing_sidecar_keeps_the_rest_of_the_file_report() {
    let dir = TempDir::new("report-no-sidecar");
    let key = generate_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("generate");
    std::fs::remove_file(crate::sync_key_file::sidecar_path(Some(&dir.key())).expect("path"))
        .expect("drop the sidecar");

    let report = report_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("report");

    assert_eq!(report.created_at, None);
    assert_eq!(
        report.fingerprint.as_deref(),
        Some(key.fingerprint().as_str())
    );
    assert_eq!(report.count, 1);
}

#[test]
fn a_new_key_never_reports_the_previous_keys_creation_time() {
    // The case that reaches this: a key file removed without its sidecar, then a
    // new key generated in the same directory. Before the fix the record survived
    // and the new key was dated by the old one, which is a retention decision
    // made on a fact that is not about this key.
    let dir = TempDir::new("stale-report");
    let stale = "1999-01-01T00:00:00Z";
    std::fs::write(
        crate::sync_key_file::sidecar_path(Some(&dir.key())).expect("a directory holds a sidecar"),
        format!("{stale}\n"),
    )
    .expect("seed the record of a key that is gone");

    let key = generate_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("generate");

    let report = report_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("report");
    let reported = report.created_at.expect("the new key has its own record");
    assert_ne!(
        reported, stale,
        "a brand-new key must not be dated by the record that was already there"
    );
    assert_eq!(report.count, 1);
    assert_eq!(
        report.fingerprint.as_deref(),
        Some(key.fingerprint().as_str())
    );
}

#[test]
fn a_keystore_that_refuses_its_attributes_is_an_error_not_an_absent_time() {
    // A platform that keeps no creation time returns an empty map; a store that
    // cannot answer returns an error. Reporting both as "unavailable" would tell
    // an operator nothing about which one they have.
    let store = store_over_credential(
        SYNC_ACCOUNT,
        SYNC_KEY,
        FakeCredential::holding_with_failing_attributes(&[4_u8; KEY_LEN]),
    );

    let err = report_with(&store, KeySource::Keystore, None).expect_err("the store refuses");

    assert_eq!(err.code, clio_types::ErrorCode::Internal);
    assert!(
        err.message.contains("read the attributes of"),
        "{}",
        err.message
    );
}

#[test]
fn a_key_stored_where_its_time_cannot_be_recorded_is_still_reported_as_a_refusal() {
    // The write is left best-effort so the key is never withheld, and the refusal
    // reaches the operator through the next read instead of being lost.
    let credential = FakeCredential::empty();
    let store = store_over_credential(SYNC_ACCOUNT, SYNC_KEY, credential.clone());

    let generated = generate_with(&store, KeySource::Keystore, None).expect("the key is stored");
    assert_eq!(
        credential.secret().expect("the key reached the store"),
        generated.expose().to_vec()
    );

    credential.refuse_attributes_now();

    let err = report_with(&store, KeySource::Keystore, None).expect_err("the store now refuses");
    assert!(
        err.message.contains("read the attributes of"),
        "the refused write reaches the operator as a refusal, not as a missing time: {}",
        err.message
    );
}

#[test]
fn a_key_file_of_the_wrong_length_is_refused_before_it_can_be_replaced() {
    let dir = TempDir::new("wrong-length");
    std::fs::write(dir.key(), [1_u8; 3]).expect("seed");

    let err = generate_with(&NoKeystore, KeySource::File, Some(dir.key()))
        .expect_err("an existing file is never replaced");

    assert!(err.message.contains("already exists"), "{}", err.message);
    assert_eq!(std::fs::read(dir.key()).expect("read"), [1_u8; 3]);
}

#[test]
fn read_key_file_base64_matches_what_the_file_holds() {
    let dir = TempDir::new("base64");
    let key = generate_with(&NoKeystore, KeySource::File, Some(dir.key())).expect("generate");

    let encoded = read_key_file_base64(&dir.key()).expect("read");

    assert_eq!(encoded, key.base64());
}
