// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing how superseded sync keys are
//! produced and consumed: the rotation that archives the current key before it
//! lets go, the newest-first lists the transfer path loads, and generate's
//! rotation behaviour across several keys.
//!
//! ## Owns
//! - Every test of `rotate_existing`, `refuse_shadowing_custody`,
//!   `load_archive_base64`, `load_all_base64`, and generate's rotation through
//!   several keys
//!
//! ## Does not own
//! - The archive file's format, permissions, and append/read refusals (see
//!   `sync_key_rotation_tests`)
//! - The generation and refusal rules outside rotation (see `sync_key_tests`)
//! - The marker's record rules (see `sync_key_marker_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. The keystore tests drive
//! the production wrapper through an injected in-memory credential, so no test
//! here can reach the machine's real Keychain or Secret Service.
//!
//! Keep this module focused on the rotation and key-list rules.

use super::*;
use crate::secret_key::KEY_LEN;
use crate::secret_key_store::{NoKeystore, SYNC_ACCOUNT, SYNC_KEY};
use crate::secret_key_store_test_support::{FakeCredential, store_over_credential};
use crate::sync_key_file as file_store;
use crate::sync_key_test_support::{TempDir, env};
use crate::sync_key_value::CREATED_ATTRIBUTE;

use crate::sync_key::{generate, report};

// ---------------------------------------------------------------- rotation

#[test]
fn rotation_archives_the_file_key_before_it_lets_go() {
    let dir = TempDir::new("rotate-file");
    file_store::write_key(&dir.key(), &[7_u8; KEY_LEN]).expect("seed the key file");
    file_store::write_created(Some(&dir.key()), "2026-01-01T00:00:00Z").expect("seed the time");

    rotate_existing(&NoKeystore, KeySource::File, Some(&dir.key())).expect("rotate");

    assert!(!dir.key().exists(), "the current key left the file store");
    let entries = read_entries(Some(&dir.key())).expect("the archive holds the key");
    assert_eq!(entries.len(), 1, "one superseded key is archived");
    assert!(
        file_store::read_key(&dir.key()).is_err(),
        "nothing is left for a later load to find"
    );
    let decoded = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        entries[0].key_b64.as_bytes(),
    )
    .expect("the archived key is base64");
    assert_eq!(decoded, vec![7_u8; KEY_LEN], "the bytes are archived whole");
    assert_eq!(
        entries[0].created_at.as_deref(),
        Some("2026-01-01T00:00:00Z"),
        "the creation time the store kept travels with the key"
    );
}

#[test]
fn rotation_of_an_absent_key_is_a_quiet_no_op() {
    let dir = TempDir::new("rotate-absent");

    rotate_existing(&NoKeystore, KeySource::File, Some(&dir.key())).expect("rotate nothing");
    rotate_existing(&NoKeystore, KeySource::File, None).expect("no path, nothing to rotate");

    assert!(
        read_entries(Some(&dir.key())).expect("read").is_empty(),
        "no rotation means no archive entry"
    );
}

#[test]
fn rotation_archives_the_keystore_key_and_deletes_the_entry() {
    let credential = FakeCredential::holding(&[9_u8; KEY_LEN]);
    let store = store_over_credential(SYNC_ACCOUNT, SYNC_KEY, credential.clone());
    store
        .set_attribute(CREATED_ATTRIBUTE, "2026-02-02T00:00:00Z")
        .expect("seed the creation attribute");
    // The keystore path: the key file does not exist, but its directory is
    // where the archive goes, so the rotation has somewhere to record into.
    let dir = TempDir::new("rotate-keystore");

    rotate_existing(&store, KeySource::Keystore, Some(&dir.key())).expect("rotate");

    assert!(
        credential.secret().is_none(),
        "the current key left the OS store"
    );
    let entries = read_entries(Some(&dir.key())).expect("the archive holds the key");
    assert_eq!(entries.len(), 1, "one superseded key is archived");
    let decoded = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        entries[0].key_b64.as_bytes(),
    )
    .expect("the archived key is base64");
    assert_eq!(decoded, vec![9_u8; KEY_LEN], "the bytes are archived whole");
    assert_eq!(
        entries[0].created_at.as_deref(),
        Some("2026-02-02T00:00:00Z"),
        "the creation attribute travels with the key"
    );
}

#[test]
fn rotation_refuses_a_shadowing_file_instead_of_guessing() {
    let dir = TempDir::new("rotate-shadow");
    file_store::write_key(&dir.key(), &[7_u8; KEY_LEN]).expect("seed a shadowing file");

    let err = refuse_shadowing_custody(KeySource::Keystore, Some(&dir.key()))
        .expect_err("a second copy of the same secret is ambiguous");

    assert!(err.message.contains("already exists"), "{}", err.message);
    assert!(dir.key().exists(), "a refused rotation touches nothing");
    refuse_shadowing_custody(KeySource::File, Some(&dir.key()))
        .expect("the file store's own key is the one rotation takes");
}

// ---------------------------------------------------------------- lists

#[test]
fn the_archived_list_is_newest_first() {
    let dir = TempDir::new("archive-order");
    append_entry(
        Some(&dir.key()),
        &ArchivedSyncKey {
            key_b64: "OLDEST".to_owned(),
            created_at: None,
        },
    )
    .expect("append oldest");
    append_entry(
        Some(&dir.key()),
        &ArchivedSyncKey {
            key_b64: "MIDDLE".to_owned(),
            created_at: None,
        },
    )
    .expect("append middle");
    let get = env(vec![(
        crate::sync_key_file::SYNC_KEY_FILE_ENV,
        dir.key().display().to_string(),
    )]);

    let list = load_archive_base64(&get).expect("load the archive");

    assert_eq!(list, vec!["MIDDLE".to_owned(), "OLDEST".to_owned()]);
}

#[test]
fn load_all_lists_the_current_key_then_the_archived_ones() {
    let dir = TempDir::new("load-all");
    file_store::write_key(&dir.key(), &[3_u8; KEY_LEN]).expect("seed the current key");
    append_entry(
        Some(&dir.key()),
        &ArchivedSyncKey {
            key_b64: "OLDER".to_owned(),
            created_at: None,
        },
    )
    .expect("append one");
    append_entry(
        Some(&dir.key()),
        &ArchivedSyncKey {
            key_b64: "NEWER".to_owned(),
            created_at: None,
        },
    )
    .expect("append two");
    let get = env(vec![(
        crate::sync_key_file::SYNC_KEY_FILE_ENV,
        dir.key().display().to_string(),
    )]);

    let list = load_all_base64(&get).expect("load every key");

    assert_eq!(list.len(), 3, "the current key and both archived ones");
    assert_ne!(
        list[0], "OLDER",
        "the current key is first, so the client seals with it"
    );
    assert_eq!(list[1], "NEWER");
    assert_eq!(list[2], "OLDER");
}

#[test]
fn load_all_is_empty_for_a_node_that_never_held_a_key() {
    let dir = TempDir::new("load-empty");
    let get = env(vec![(
        crate::sync_key_file::SYNC_KEY_FILE_ENV,
        dir.key().display().to_string(),
    )]);

    let keys = load_all_base64(&get).expect("load");
    assert!(keys.is_empty(), "a node that never held a key has no keys");
}

// ---------------------------------------------------------------- generate

#[test]
fn a_second_generation_keeps_the_first_key_readable() {
    let dir = TempDir::new("rotate-generate");
    let get = env(vec![(
        crate::sync_key_file::SYNC_KEY_FILE_ENV,
        dir.key().display().to_string(),
    )]);
    let first = generate(&get, Some(KeySource::File)).expect("first generation");
    let second = generate(&get, Some(KeySource::File)).expect("second generation rotates");

    let list = load_all_base64(&get).expect("load every key");

    assert_eq!(list.len(), 2, "both keys are held");
    assert_eq!(
        list[0],
        second.base64(),
        "the newest key is first, so the client seals with it"
    );
    assert_eq!(list[1], first.base64(), "the superseded key is retained");

    let report = report(&get).expect("report");
    assert_eq!(report.count, 2);
    assert_eq!(report.keys.len(), 2);
    assert!(report.keys[0].current, "the newest key is the current one");
    assert_eq!(report.keys[0].fingerprint, second.fingerprint());
    assert!(!report.keys[1].current, "the superseded key is archived");
    assert_eq!(report.keys[1].fingerprint, first.fingerprint());
    assert_eq!(report.keys[1].store, "archive file");
    assert_ne!(
        report.keys[0].fingerprint, report.keys[1].fingerprint,
        "the two keys are distinguishable by fingerprint"
    );
    assert_eq!(
        report.fingerprint.as_deref(),
        Some(report.keys[0].fingerprint.as_str()),
        "the newest key's fingerprint is the report's fingerprint"
    );
}

#[test]
fn a_third_generation_keeps_every_previous_key_in_creation_order() {
    let dir = TempDir::new("rotate-three");
    let get = env(vec![(
        crate::sync_key_file::SYNC_KEY_FILE_ENV,
        dir.key().display().to_string(),
    )]);
    let first = generate(&get, Some(KeySource::File)).expect("first");
    let second = generate(&get, Some(KeySource::File)).expect("second");
    let third = generate(&get, Some(KeySource::File)).expect("third");

    let list = load_all_base64(&get).expect("load every key");

    assert_eq!(
        list,
        vec![third.base64(), second.base64(), first.base64()],
        "newest first, and the creation order is stable across runs"
    );
    let report = report(&get).expect("report");
    assert_eq!(report.count, 3);
    assert!(
        report.keys.iter().any(|key| key.current),
        "exactly the newest key is current"
    );
    assert_eq!(
        report.keys[1].fingerprint,
        second.fingerprint(),
        "archived entries are listed newest first, like the key list"
    );
    assert_eq!(report.keys[2].fingerprint, first.fingerprint());
}

// ------------------------------------------------------- rotation refusals

#[test]
fn rotation_refuses_to_let_go_when_the_archive_cannot_be_written() {
    let dir = TempDir::new("rotate-blocked");
    file_store::write_key(&dir.key(), &[7_u8; KEY_LEN]).expect("seed the key file");
    // The archive path is occupied by a directory, so the entry cannot land.
    std::fs::create_dir(dir.path().join(ARCHIVE_NAME)).expect("seed a directory");
    // Owner-only still passes the mode check, so the open itself is what
    // fails: the archive path is a directory.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            dir.path().join(ARCHIVE_NAME),
            std::fs::Permissions::from_mode(0o600),
        )
        .expect("chmod");
    }

    let err = rotate_existing(&NoKeystore, KeySource::File, Some(&dir.key()))
        .expect_err("the archive refused the entry");

    assert_eq!(err.code, clio_types::ErrorCode::Internal);
    assert!(
        dir.key().exists(),
        "the current key is never deleted when the archive write failed"
    );
}

#[test]
fn keystore_rotation_refuses_a_shadowing_file_instead_of_guessing() {
    let dir = TempDir::new("rotate-keystore-shadow");
    file_store::write_key(&dir.key(), &[7_u8; KEY_LEN]).expect("seed a shadowing file");
    let store = store_over_credential(SYNC_ACCOUNT, SYNC_KEY, FakeCredential::empty());

    let err = rotate_existing(&store, KeySource::Keystore, Some(&dir.key()))
        .expect_err("a second copy of the same secret is ambiguous");

    assert!(err.message.contains("already exists"));
    assert!(dir.key().exists(), "a refused rotation touches nothing");
}

#[test]
fn keystore_rotation_of_an_empty_store_is_a_quiet_no_op() {
    let dir = TempDir::new("rotate-keystore-empty");
    let store = store_over_credential(SYNC_ACCOUNT, SYNC_KEY, FakeCredential::empty());

    rotate_existing(&store, KeySource::Keystore, Some(&dir.key()))
        .expect("nothing held means nothing to rotate");

    assert!(
        read_entries(Some(&dir.key())).expect("read").is_empty(),
        "no rotation means no archive entry"
    );
}

#[test]
fn keystore_rotation_refuses_a_wrong_length_entry() {
    let dir = TempDir::new("rotate-keystore-length");
    let store = store_over_credential(SYNC_ACCOUNT, SYNC_KEY, FakeCredential::holding(&[9_u8; 5]));

    let err = rotate_existing(&store, KeySource::Keystore, Some(&dir.key()))
        .expect_err("a damaged entry is not rotated");

    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("expected"));
}

#[test]
fn keystore_rotation_reports_an_attribute_read_that_the_store_refuses() {
    let dir = TempDir::new("rotate-keystore-attrs");
    let store = store_over_credential(
        SYNC_ACCOUNT,
        SYNC_KEY,
        FakeCredential::holding_with_failing_attributes(&[9_u8; KEY_LEN]),
    );

    let err = rotate_existing(&store, KeySource::Keystore, Some(&dir.key()))
        .expect_err("the store refuses the attribute read");

    assert_eq!(err.code, clio_types::ErrorCode::Internal);
}
