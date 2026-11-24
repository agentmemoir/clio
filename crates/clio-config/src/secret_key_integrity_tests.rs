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
    // A name longer than the filesystem allows cannot be opened at all, so the
    // refusal is the read failure rather than the gate's shape verdict.
    let too_long = key_file("name-too-long").with_file_name("k".repeat(300));

    let err = err_of(load_or_create_with(
        &FakeStore::empty(),
        Some(too_long.clone()),
    ));

    assert_eq!(err.code, ErrorCode::Internal);
    assert!(
        err.message
            .contains(&format!("cannot read the key file {}", too_long.display())),
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

#[cfg(unix)]
#[test]
fn a_key_file_another_account_can_read_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let file = key_file("loose-mode");
    let seeded = [3_u8; KEY_LEN];
    write_key(&file, &seeded);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).expect("chmod");

    let err = err_of(load_or_create_with(&absent_keystore(), Some(file.clone())));

    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(
        err.message.contains(&format!(
            "encryption key file {}: refused because it is not owner-only",
            file.display()
        )),
        "the refusal names the file an operator has to restrict: {}",
        err.message
    );
    assert_eq!(
        fs::read(&file).expect("read"),
        seeded,
        "the key is left exactly as found"
    );
}

#[cfg(unix)]
#[test]
fn an_owner_only_key_file_is_read() {
    use std::os::unix::fs::PermissionsExt;
    let file = key_file("tight-mode");
    let seeded = [4_u8; KEY_LEN];
    write_key(&file, &seeded);
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).expect("chmod");

    let loaded = load_or_create_with(&absent_keystore(), Some(file.clone())).expect("load");

    assert_eq!(loaded.expose(), &seeded);
    assert_eq!(loaded.location, KeyLocation::File(file));
}

#[cfg(unix)]
#[test]
fn a_symlink_at_the_key_path_is_refused_rather_than_followed() {
    use std::os::unix::fs::symlink;
    let link = key_file("key-symlink");
    let planted = [6_u8; KEY_LEN];
    let target = link.with_file_name("planted-key");
    write_key(&target, &planted);
    symlink(&target, &link).expect("plant the link at the key path");

    let err = err_of(load_or_create_with(&absent_keystore(), Some(link.clone())));

    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(
        err.message.contains(&format!(
            "encryption key file {}: refused because it is a symbolic link",
            link.display()
        )),
        "the refusal names the link, not its target: {}",
        err.message
    );
    assert_eq!(
        fs::read(&target).expect("read"),
        planted,
        "the bytes behind the link are left where they were, never returned as the key"
    );
}

#[cfg(unix)]
#[test]
fn a_read_tightens_an_existing_key_directory_to_owner_only() {
    // A successful read already says the file belongs to this account, so a key
    // directory other accounts can enter is a refusal rather than a working
    // configuration. The read tightens it, and the key still loads.
    use std::os::unix::fs::PermissionsExt;
    let file = key_file("read-tightens-dir");
    let seeded = [5_u8; KEY_LEN];
    write_key(&file, &seeded);
    let dir = file
        .parent()
        .expect("the key file has a directory")
        .to_path_buf();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("chmod");

    let loaded = load_or_create_with(&absent_keystore(), Some(file)).expect("load");

    assert_eq!(loaded.expose(), &seeded);
    let mode = fs::metadata(&dir).expect("metadata").permissions().mode();
    assert_eq!(
        mode & 0o777,
        0o700,
        "the read made the key directory owner-only"
    );
}

#[cfg(unix)]
#[test]
fn a_directory_at_the_key_path_is_refused_as_a_forbidden_shape() {
    let file = key_file("key-is-a-dir");
    fs::create_dir_all(&file).expect("plant a directory at the key path");

    let err = err_of(load_or_create_with(&absent_keystore(), Some(file.clone())));

    assert_eq!(err.code, ErrorCode::Forbidden);
    assert!(
        err.message
            .contains("refused because it is not a regular file"),
        "the refusal names the shape: {}",
        err.message
    );
    assert!(
        fs::metadata(&file).expect("metadata").is_dir(),
        "the directory is left exactly as found"
    );
}

#[test]
fn a_missing_key_file_still_generates_one_on_first_run() {
    let file = key_file("first-run");

    let loaded = load_or_create_with(&absent_keystore(), Some(file.clone())).expect("create");

    assert_eq!(loaded.location, KeyLocation::File(file.clone()));
    assert_eq!(&fs::read(&file).expect("read"), loaded.expose());
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
