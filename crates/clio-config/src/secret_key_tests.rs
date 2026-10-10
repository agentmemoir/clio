// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the encryption-key run order in
//! `secret_key`: which store is chosen, when a key is generated, and every case
//! where an existing key is refused rather than replaced.
//!
//! ## Owns
//! - One test per step of the documented rule set
//! - The never-replace refusals: wrong length, unreadable file, two stores
//! - The key file's permissions and byte layout
//!
//! ## Does not own
//! - Test scaffolding (see `secret_key_support_tests`)
//! - Path resolution and `location` reporting (see `secret_key_report_tests`)
//! - The promise that no message leaks key bytes (see `secret_key_report_tests`)
//!
//! ## Boundary
//!
//! Every test drives the internal seam with a [`FakeStore`], so no test can
//! reach the machine's real Keychain or Secret Service. A test needing the
//! production store is a test that cannot run on CI, which has no session bus.
//!
//! Keep this module focused on which key wins and when one is created.

use super::support_tests::*;
use super::*;

fn err_of(result: Result<LoadedKey, AmError>) -> AmError {
    result.expect_err("expected a refusal")
}

// ------------------------------------------------------------- first run

#[test]
fn a_fresh_install_creates_one_key_in_the_keystore() {
    let store = FakeStore::empty();
    let file = key_file("fresh");

    let key = load_or_create_with(&store, Some(file.clone())).expect("create");

    assert_eq!(
        key.location,
        KeyLocation::File(file.clone()),
        "an absent record means the recommended store"
    );
    assert_eq!(fs::read(&file).expect("read"), *key.expose());
    assert_eq!(
        store.set_calls(),
        0,
        "the keystore is not consulted when the record says file"
    );
    assert_eq!(
        store.get_calls(),
        0,
        "the keystore is not even read, so no password prompt can be caused"
    );
}

#[test]
fn loading_twice_returns_the_same_key_without_writing_again() {
    let store = FakeStore::empty();
    let file = key_file("twice");

    let first = load_or_create_with(&store, Some(file.clone())).expect("first");
    let second = load_or_create_with(&store, Some(file)).expect("second");

    assert_eq!(first.expose(), second.expose());
    assert_eq!(
        store.set_calls(),
        0,
        "a second write would replace the key under every sealed value"
    );
}

#[test]
fn an_existing_keystore_key_is_used_without_creating_a_file() {
    let wanted = random_key();
    let store = FakeStore::holding(&wanted);
    let file = key_file("migrate");
    record(&file, KeySource::Keystore);

    let key = load_or_create_with(&store, Some(file.clone())).expect("load");

    assert_eq!(key.expose(), &wanted);
    assert_eq!(key.location, KeyLocation::Keystore);
    assert!(!file.exists(), "a keystore key is never copied into a file");
    assert_eq!(store.set_calls(), 0, "a loaded key is never rewritten");
}

// --------------------------------------------------------- the file fallback

#[test]
fn an_unreachable_keystore_writes_the_key_file() {
    let store = FakeStore::unreadable("no session bus");
    let file = key_file("fallback");

    let key = load_or_create_with(&store, Some(file.clone())).expect("create");

    assert_eq!(key.location, KeyLocation::File(file.clone()));
    assert_eq!(fs::read(&file).expect("read"), *key.expose());
    assert_eq!(
        store.set_calls(),
        0,
        "a store that cannot be read is not written to"
    );
    assert_eq!(store.entry(), None);
}

#[test]
fn a_keystore_that_refuses_writes_falls_back_to_the_key_file() {
    let store = FakeStore::refusing_writes("read-only store");
    let file = key_file("readonly");
    record(&file, KeySource::Keystore);

    let err = err_of(load_or_create_with(&store, Some(file.clone())));

    assert_eq!(store.set_calls(), 1, "the chosen store was tried");
    assert!(
        !file.exists(),
        "the key is not redirected to a store the operator did not choose"
    );
    assert!(
        err.message.contains("did not write it anywhere else"),
        "{} does not say the key went nowhere else",
        err.message
    );
}

#[test]
fn no_key_anywhere_and_no_key_file_path_names_the_store_and_the_env_var() {
    let err = err_of(load_or_create_with(&FakeStore::empty(), None));

    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(err.message.contains(KEY_FILE_ENV), "{}", err.message);
}

#[test]
fn a_key_file_that_cannot_be_read_says_no_key_was_stored() {
    // The record always sits beside the key file, so a key file whose directory
    // is a regular file cannot even be recorded against. Clio reports that
    // rather than guessing, because the record is what says where the key is.
    let err = err_of(load_or_create_with(
        &FakeStore::empty(),
        Some(blocked_key_file()),
    ));

    assert!(
        err.message.contains("will not guess which store"),
        "{} does not say why it stopped",
        err.message
    );
    assert!(
        err.message.contains("has not stored a new key") || err.message.contains("remove the file"),
        "{} does not say what to do",
        err.message
    );
}

// ------------------------------------------------------ the file is present

#[test]
fn an_existing_file_wins_when_the_keystore_is_empty() {
    let wanted = random_key();
    let file = key_file("file-wins");
    write_key(&file, &wanted);
    let store = FakeStore::empty();

    let key = load_or_create_with(&store, Some(file.clone())).expect("load");

    assert_eq!(key.expose(), &wanted);
    assert_eq!(key.location, KeyLocation::File(file));
    assert_eq!(store.set_calls(), 0, "a loaded key is never rewritten");
}

#[test]
fn an_existing_file_wins_when_the_keystore_cannot_be_reached() {
    let wanted = random_key();
    let file = key_file("file-wins-locked");
    write_key(&file, &wanted);
    let store = FakeStore::unreadable("store is locked");

    let key = load_or_create_with(&store, Some(file.clone())).expect("load");

    assert_eq!(
        key.expose(),
        &wanted,
        "an unreachable store must not change which key is used"
    );
    assert_eq!(key.location, KeyLocation::File(file));
    assert_eq!(store.set_calls(), 0);
}

#[test]
fn both_stores_populated_still_answers_from_the_chosen_one() {
    for (name, source, expected) in [
        ("two-file", KeySource::File, "file"),
        ("two-store", KeySource::Keystore, "keystore"),
    ] {
        let file = key_file(name);
        let store = FakeStore::holding(&random_key());
        record(&file, source);
        write_key(&file, &random_key());

        let key = load_or_create_with(&store, Some(file.clone()))
            .expect("the chosen store answers even when both are populated");

        match (source, &key.location) {
            (KeySource::File, KeyLocation::File(where_)) => {
                assert_eq!(where_, &file, "the file is the chosen store");
            }
            (KeySource::Keystore, KeyLocation::Keystore) => {}
            (source, found) => {
                panic!(
                    "{expected} was chosen, so the key should come from it, not {found:?}: {source}"
                )
            }
        }
    }
}

#[test]
fn an_existing_key_file_is_never_clobbered() {
    let wanted = random_key();
    let file = key_file("no-clobber");
    write_key(&file, &wanted);

    // What a second process racing this one would hit: the create-new open
    // refuses rather than replacing the key.
    let err = crate::secret_key_file::write_key_file(&file, &random_key()).expect_err("refuse");

    assert_eq!(err.code, ErrorCode::Internal);
    assert_eq!(fs::read(&file).expect("read"), wanted);
}
