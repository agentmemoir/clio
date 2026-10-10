// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the never-replace rule: that a key Clio has
//! already stored is never clobbered, that a malformed one is refused and left
//! alone, and that the keystore bypass needs both the switch and a key file path.
//!
//! ## Owns
//! - The key file's own shape: owner-only, thirty-two raw bytes, directories made
//! - Never-replace behavior under test for `secret_key`
//!
//! ## Does not own
//! - Which store a first run picks and how a key is created (see
//!   `secret_key_tests`)
//! - The report lines the shadowed-file case prints (see
//!   `secret_key_report_tests`)
//! - Production logic under test
//!
//! ## Boundary
//!
//! Changes to what happens to a key that already exists should normally update
//! tests here. Changes to how a first run picks a store should not require changes
//! here.
//!
//! Keep this module focused on not destroying a key.

use super::support_tests::*;
use super::*;

fn err_of(result: Result<LoadedKey, AmError>) -> AmError {
    result.expect_err("expected a refusal")
}

// ---------------------------------------------------- the never-replace rule

#[test]
fn a_short_key_in_the_file_is_refused_and_left_alone() {
    let file = key_file("short-file");
    write_key(&file, &[7_u8; 5]);
    let store = FakeStore::empty();

    let err = err_of(load_or_create_with(&store, Some(file.clone())));

    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(
        err.message.contains(&format!(
            "the key in key file {} is 5 bytes; expected 32",
            file.display()
        )),
        "{}",
        err.message
    );
    assert!(
        err.message.contains("will not replace it"),
        "the operator is told this is not automatic: {}",
        err.message
    );
    assert_eq!(store.set_calls(), 0, "a wrong length is never re-generated");
    assert_eq!(
        fs::read(&file).expect("read"),
        [7_u8; 5],
        "the damaged key is left exactly as found"
    );
}

#[test]
fn a_long_key_in_the_keystore_is_refused_and_left_alone() {
    let store = FakeStore::holding(&[9_u8; 64]);
    let file = key_file("long-store");
    record(&file, KeySource::Keystore);

    let err = err_of(load_or_create_with(&store, Some(file)));

    assert_eq!(err.code, ErrorCode::ConfigCorrupt);
    assert!(
        err.message.contains(&format!(
            "the key in {} is 64 bytes; expected 32",
            display_name()
        )),
        "{}",
        err.message
    );
    assert_eq!(store.set_calls(), 0);
    assert_eq!(
        store.entry().expect("untouched"),
        vec![9_u8; 64],
        "the stored value is left exactly as found"
    );
}

#[test]
fn an_empty_key_is_refused_rather_than_treated_as_absent() {
    let store = FakeStore::holding(&[]);
    let file = key_file("empty-store");
    record(&file, KeySource::Keystore);

    let err = err_of(load_or_create_with(&store, Some(file)));

    assert!(
        err.message.contains("is 0 bytes; expected 32"),
        "an empty entry is a damaged key, not a missing one: {}",
        err.message
    );
    assert_eq!(store.set_calls(), 0);
}

#[test]
fn an_unreadable_key_file_names_the_file_and_the_env_var() {
    let directory = unreadable_file();

    let err = err_of(load_or_create_with(
        &FakeStore::empty(),
        Some(directory.clone()),
    ));

    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        err.message
            .contains(&format!("cannot read the key file {}", directory.display())),
        "{}",
        err.message
    );
    assert!(err.message.contains(KEY_FILE_ENV), "{}", err.message);
    assert!(
        err.message.contains("will not generate a replacement"),
        "{}",
        err.message
    );
}

// -------------------------------------------------------------- the key file

#[test]
fn the_key_file_is_created_owner_only() {
    let store = FakeStore::unreadable("no session bus");
    let file = key_file("owner-only");

    load_or_create_with(&store, Some(file.clone())).expect("create");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&file).expect("metadata").permissions().mode();
        assert_eq!(mode & 0o077, 0, "not group or world readable");
        assert_eq!(mode & 0o600, 0o600, "the owner can read it");
    }
}

#[test]
fn the_key_file_is_thirty_two_raw_bytes() {
    let file = key_file("raw-bytes");

    let key = load_or_create_with(&FakeStore::unreadable("no session bus"), Some(file.clone()))
        .expect("create");

    assert_eq!(fs::read(&file).expect("read"), *key.expose());
    assert_eq!(
        fs::read(&file).expect("read").len(),
        KEY_LEN,
        "32 raw bytes, with no encoding and no trailing newline"
    );
}

#[test]
fn missing_key_file_directories_are_created() {
    let file = tmp("nested-dirs").join("a").join("b").join(KEY_FILE_NAME);

    let key = load_or_create_with(&FakeStore::unreadable("no session bus"), Some(file.clone()))
        .expect("create");

    assert_eq!(key.location, KeyLocation::File(file.clone()));
    assert_eq!(fs::read(&file).expect("read").len(), KEY_LEN);
}

#[test]
fn a_loaded_key_exposes_exactly_its_own_bytes() {
    let wanted = random_key();
    let key = LoadedKey::new(Zeroizing::new(wanted), KeyLocation::Keystore);

    assert_eq!(key.expose(), &wanted);
    assert_eq!(key.expose().len(), KEY_LEN);
}

// --- the keystore bypass switch -------------------------------------------------
//
// A key file on its own is not enough to keep the operating system's secret store
// out of the picture: the lifecycle always asks it one question, and on macOS
// that question can cost the operator a Keychain password prompt per process.

#[test]
fn an_existing_key_file_is_used_with_no_keystore_at_all() {
    let path = key_file("only");
    let seeded = random_key();
    write_key(&path, &seeded);

    let loaded = load_or_create_with(&absent_keystore(), Some(path.clone())).expect("load");

    assert_eq!(loaded.expose(), &seeded);
    assert_eq!(loaded.location, KeyLocation::File(path));
}

#[test]
fn a_missing_key_file_is_created_in_the_file_because_the_keystore_refuses_writes() {
    let path = key_file("only-create");

    let loaded = load_or_create_with(&absent_keystore(), Some(path.clone())).expect("create");

    assert_eq!(loaded.location, KeyLocation::File(path.clone()));
    assert_eq!(&fs::read(&path).expect("read"), loaded.expose());
}

#[test]
fn the_bypass_needs_both_the_switch_and_a_key_file_path() {
    let home = |key: &str| (key == "HOME").then(|| "/home/u".to_owned());
    // Neither half set: a key file exists in the plan, but the operator did not
    // ask for the bypass.
    assert!(!crate::secret_key_file::keystore_is_bypassed(&home));
    // The switch set, and no key file anywhere. There is nowhere to keep the
    // key, so the keystore stays the only store that could hold it; dropping it
    // here would be data loss.
    let switch_but_no_file = |key: &str| (key == "CLIO_KEY_FILE_ONLY").then(|| "1".to_owned());
    assert!(!crate::secret_key_file::keystore_is_bypassed(
        &switch_but_no_file
    ));
    // Both.
    let both = |key: &str| match key {
        "HOME" => Some("/home/u".to_owned()),
        "CLIO_KEY_FILE_ONLY" => Some("1".to_owned()),
        _ => None,
    };
    assert!(crate::secret_key_file::keystore_is_bypassed(&both));
    // A blank value counts as unset, like every other override in this crate.
    let blank = |key: &str| match key {
        "HOME" => Some("/home/u".to_owned()),
        "CLIO_KEY_FILE_ONLY" => Some("   ".to_owned()),
        _ => None,
    };
    assert!(!crate::secret_key_file::keystore_is_bypassed(&blank));
}

#[test]
fn a_key_file_never_follows_the_process_working_directory() {
    // The database resolver falls back to a bare `clio.db` with no home
    // directory, and a key file beside it would land in whatever directory the
    // process happened to start in: a source tree, a shared folder, a container
    // layer. That is not a key store, so no path is offered at all.
    let no_home = |_: &str| None;
    assert_eq!(key_file_path(&no_home), None);
    // An absolute home still resolves as before.
    let home = |key: &str| (key == "HOME").then(|| "/home/u".to_owned());
    assert_eq!(
        key_file_path(&home),
        Some(PathBuf::from("/home/u/.local/share/clio/clio.key"))
    );
    // An explicitly named path is the operator's decision, so a relative one is
    // honoured rather than second-guessed.
    let explicit = |key: &str| (key == "CLIO_KEY_FILE").then(|| "./mine.key".to_owned());
    assert_eq!(key_file_path(&explicit), Some(PathBuf::from("./mine.key")));
}
