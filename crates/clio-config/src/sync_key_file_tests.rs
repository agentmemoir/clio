// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the sync key file: path resolution,
//! the owner-only read refusal, the length check, and the creation-time sidecar.
//!
//! ## Owns
//! - Every test of `sync_key_file`'s path rules, read, write, and sidecar
//!
//! ## Does not own
//! - The lifecycle rules built on these functions (see `sync_key_tests`)
//! - The encryption key's file (see `secret_key_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every path here comes from
//! a unique temp directory removed when the guard drops.
//!
//! Keep this module focused on the file and its sidecar.

use super::*;
use crate::sync_key_test_support::{TempDir, env_of as env};
use base64::Engine as _;
use std::fs;

/// Seed a file directly, with owner-only permissions on Unix.
///
/// `write_key` type-checks the 32-byte length, so a test that deliberately
/// writes a wrong-length file has to go around it.
fn seed(path: &std::path::Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("seed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("chmod");
    }
}

/// Lower-case hex of a key, the encoding a rendered value would most plausibly
/// leak in.
fn hex_of(key: &[u8; KEY_LEN]) -> String {
    use std::fmt::Write as _;
    key.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

// ------------------------------------------------------------ path resolution

#[test]
fn an_explicit_env_path_wins_over_every_default() {
    let found = key_file_path(&env(&[
        ("CLIO_SYNC_KEY_FILE", "/secrets/sync.key"),
        ("CLIO_KEY_FILE", "/other/clio.key"),
        ("CLIO_DATA_DIR", "/data"),
    ]));

    assert_eq!(found, Some(PathBuf::from("/secrets/sync.key")));
}

#[test]
fn a_blank_env_path_counts_as_unset() {
    let found = key_file_path(&env(&[
        ("CLIO_SYNC_KEY_FILE", "   "),
        ("CLIO_DATA_DIR", "/data"),
    ]));

    assert_eq!(found, Some(PathBuf::from("/data").join(SYNC_KEY_FILE_NAME)));
}

#[test]
fn the_default_sits_beside_the_encryption_key_file() {
    let found = key_file_path(&env(&[("CLIO_DATA_DIR", "/data")]));

    assert_eq!(found, Some(PathBuf::from("/data").join(SYNC_KEY_FILE_NAME)));
}

#[test]
fn an_explicit_encryption_key_file_moves_the_sync_key_default() {
    let found = key_file_path(&env(&[("CLIO_KEY_FILE", "/keys/clio.key")]));

    assert_eq!(
        found,
        Some(PathBuf::from("/keys").join(SYNC_KEY_FILE_NAME)),
        "one override moves both keys so they cannot disagree about the directory"
    );
}

#[test]
fn a_bare_relative_encryption_key_file_keeps_the_default_relative() {
    let found = key_file_path(&env(&[("CLIO_KEY_FILE", "clio.key")]));

    assert_eq!(found, Some(PathBuf::from(SYNC_KEY_FILE_NAME)));
}

// --------------------------------------------------------------- reading

#[test]
fn a_written_key_reads_back_byte_for_byte() {
    let dir = TempDir::new("round-trip");
    let key = rand::random::<[u8; KEY_LEN]>();

    write_key(&dir.key(), &key).expect("write");

    assert_eq!(*read_key(&dir.key()).expect("read"), key);
}

#[test]
fn a_created_key_file_is_owner_only_by_shape() {
    let dir = TempDir::new("mode");
    write_key(&dir.key(), &[3_u8; KEY_LEN]).expect("write");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir.key())
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o077,
            0,
            "no group or other access, whatever the exact mode value is"
        );
    }
}

#[test]
fn a_second_write_never_replaces_the_first_key() {
    let dir = TempDir::new("create-new");
    write_key(&dir.key(), &[1_u8; KEY_LEN]).expect("first");

    let err = write_key(&dir.key(), &[2_u8; KEY_LEN]).expect_err("no replacement");

    assert!(err.message.contains("create"), "{}", err.message);
    assert_eq!(*read_key(&dir.key()).expect("read"), [1_u8; KEY_LEN]);
}

#[test]
fn a_wrong_length_file_is_refused_rather_than_trimmed() {
    let dir = TempDir::new("short");
    seed(&dir.key(), &[0_u8; 5]);

    let err = read_key(&dir.key()).expect_err("a short key is not a key");

    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("5 bytes"), "{}", err.message);
}

#[test]
fn a_missing_file_is_an_error_not_an_empty_key() {
    let dir = TempDir::new("missing");

    let err = read_key(&dir.key()).expect_err("nothing to read");

    assert_eq!(err.code, clio_types::ErrorCode::Internal);
}

#[cfg(unix)]
#[test]
fn a_group_readable_file_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("permissions");
    write_key(&dir.key(), &[4_u8; KEY_LEN]).expect("write");
    fs::set_permissions(dir.key(), fs::Permissions::from_mode(0o644)).expect("chmod");

    let err = read_key(&dir.key()).expect_err("not owner-only");

    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    assert!(err.message.contains("not owner-only"), "{}", err.message);
}

// ---------------------------------------------------------------- sidecar

#[test]
fn the_creation_time_round_trips_through_the_sidecar() {
    let dir = TempDir::new("created");
    write_key(&dir.key(), &[5_u8; KEY_LEN]).expect("write");

    write_created(Some(&dir.key()), "2026-10-01T00:00:00Z").expect("record");

    assert_eq!(
        read_created(Some(&dir.key())).expect("read"),
        Some("2026-10-01T00:00:00Z".to_owned())
    );
}

#[test]
fn a_missing_sidecar_is_an_unknown_creation_time_not_a_lost_key() {
    let dir = TempDir::new("no-created");
    write_key(&dir.key(), &[6_u8; KEY_LEN]).expect("write");

    assert_eq!(read_created(Some(&dir.key())).expect("read"), None);
    assert!(read_key(&dir.key()).is_ok(), "the key is untouched");
}

#[test]
fn an_empty_sidecar_reads_as_unknown_rather_than_as_an_empty_date() {
    let dir = TempDir::new("empty-created");
    fs::write(sidecar_path(Some(&dir.key())).expect("path"), "\n").expect("seed");

    assert_eq!(read_created(Some(&dir.key())).expect("read"), None);
}

#[test]
fn the_sidecar_never_carries_key_material() {
    let dir = TempDir::new("no-material");
    let key = rand::random::<[u8; KEY_LEN]>();
    write_key(&dir.key(), &key).expect("write");
    write_created(Some(&dir.key()), "2026-10-01T00:00:00Z").expect("record");

    let raw = fs::read_to_string(sidecar_path(Some(&dir.key())).expect("path")).expect("read");
    let hex = hex_of(&key);
    let b64 = base64::engine::general_purpose::STANDARD.encode(key);

    assert!(!raw.contains(&hex), "the sidecar carried the key in hex");
    assert!(!raw.contains(&b64), "the sidecar carried the key in base64");
}

#[test]
fn a_key_file_with_no_directory_has_no_sidecar() {
    assert_eq!(
        sidecar_path(Some(std::path::Path::new("clio-sync.key"))),
        None
    );
}

#[test]
fn a_stale_creation_record_never_survives_a_new_one() {
    // The sidecar's name comes from the directory, not from the key file's name,
    // so a record left by a previous key is exactly what a new key would
    // inherit. A stale creation time is worse than an absent one, because it is
    // what a retention decision would be made on.
    let dir = TempDir::new("stale-created");
    write_key(&dir.key(), &[1_u8; KEY_LEN]).expect("write");
    write_created(Some(&dir.key()), "1999-01-01T00:00:00Z").expect("seed the stale record");

    write_created(Some(&dir.key()), "2026-10-01T21:49:12Z").expect("record the new time");

    assert_eq!(
        read_created(Some(&dir.key())).expect("read"),
        Some("2026-10-01T21:49:12Z".to_owned()),
        "the new time replaces the stale one rather than losing to it"
    );
}

#[test]
fn the_creation_record_is_owner_only_by_shape() {
    let dir = TempDir::new("created-mode");
    write_key(&dir.key(), &[2_u8; KEY_LEN]).expect("write");

    write_created(Some(&dir.key()), "2026-10-01T00:00:00Z").expect("record");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(sidecar_path(Some(&dir.key())).expect("path"))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o077,
            0,
            "no group or other access, whatever the exact mode value is"
        );
    }
}

#[test]
fn a_stale_record_that_cannot_be_removed_says_which_step_failed() {
    let dir = TempDir::new("stale-blocked");
    write_key(&dir.key(), &[3_u8; KEY_LEN]).expect("write");
    // A directory where the record goes: it is removed as a file, so the removal
    // is refused and the error has to name what it stuck on.
    fs::create_dir(dir.join(CREATED_NAME)).expect("the record path is a directory");

    let err = write_created(Some(&dir.key()), "2026-10-01T00:00:00Z")
        .expect_err("a directory is not replaceable as a file");

    assert!(
        err.message.contains("cannot remove the stale"),
        "{}",
        err.message
    );
    assert!(err.message.contains(CREATED_NAME), "{}", err.message);
}

// ----------------------------------------------------------------- removal

#[test]
fn removal_drops_the_key_file_and_its_sidecar() {
    let dir = TempDir::new("remove");
    write_key(&dir.key(), &[7_u8; KEY_LEN]).expect("write");
    write_created(Some(&dir.key()), "2026-10-01T00:00:00Z").expect("record");

    remove_key(&dir.key()).expect("remove");

    assert!(!dir.key().exists());
    assert!(!dir.join(CREATED_NAME).exists());
}

#[test]
fn removing_what_is_not_there_succeeds() {
    let dir = TempDir::new("remove-absent");

    remove_key(&dir.key()).expect("deleting nothing is the state the caller asked for");
}

#[test]
fn a_bare_path_with_no_directory_still_removes_cleanly() {
    // The sidecar path is `None` for a bare filename, so the loop skips it and
    // the file itself is a NotFound, which is the state the caller asked for.
    let bare = format!("clio-sync-key-absent-{}.key", std::process::id());

    remove_key(std::path::Path::new(&bare)).expect("an absent bare path is not an error");
}

#[test]
fn a_removal_that_cannot_finish_names_the_path_it_stuck_on() {
    let dir = TempDir::new("remove-blocked");
    write_key(&dir.key(), &[8_u8; KEY_LEN]).expect("write");
    // A directory where the sidecar goes: `remove_file` refuses it, and the key
    // is already gone, so the error has to name what was left behind.
    fs::create_dir(dir.join(CREATED_NAME)).expect("the sidecar path is a directory");

    let err = remove_key(&dir.key()).expect_err("a directory is not removable as a file");

    assert!(err.message.contains("cannot remove"), "{}", err.message);
    assert!(err.message.contains(CREATED_NAME), "{}", err.message);
}

#[test]
fn a_key_path_that_is_a_readable_directory_is_refused_on_read() {
    let dir = TempDir::new("read-is-dir");
    fs::create_dir(dir.key()).expect("the key path is a directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.key(), fs::Permissions::from_mode(0o700))
            .expect("owner-only so the mode check passes");
    }

    let err = read_key(&dir.key()).expect_err("a directory is not a key");

    assert_eq!(err.code, clio_types::ErrorCode::Internal);
    assert!(err.message.contains("cannot read"), "{}", err.message);
}

#[test]
fn a_write_whose_directory_cannot_be_created_says_which_step_failed() {
    let dir = TempDir::new("write-blocked");
    let blocker = dir.join("blocker");
    fs::write(&blocker, b"not a directory").expect("blocker");
    let path = blocker.join("nested").join(SYNC_KEY_FILE_NAME);

    let err = write_key(&path, &[9_u8; KEY_LEN]).expect_err("no directory to write into");

    assert!(
        err.message.contains("create the directory for"),
        "{}",
        err.message
    );
}

#[test]
fn reading_a_sidecar_that_is_a_directory_is_an_error_not_an_absent_time() {
    let dir = TempDir::new("created-is-dir");
    fs::create_dir(sidecar_path(Some(&dir.key())).expect("path")).expect("directory");

    let err = read_created(Some(&dir.key())).expect_err("a directory is not a record");

    assert!(err.message.contains("cannot read"), "{}", err.message);
}

#[test]
fn a_sidecar_with_no_home_is_simply_not_written_or_read() {
    assert_eq!(read_created(None).expect("no path is not a failure"), None);
    write_created(None, "2026-10-01T00:00:00Z").expect("nowhere to write is not a failure");
}

#[test]
fn a_sidecar_whose_directory_cannot_be_created_says_which_step_failed() {
    let dir = TempDir::new("created-blocked");
    let blocker = dir.join("blocker");
    fs::write(&blocker, b"not a directory").expect("blocker");
    let key = blocker.join("nested").join(SYNC_KEY_FILE_NAME);

    let err =
        write_created(Some(&key), "2026-10-01T00:00:00Z").expect_err("no directory for the record");

    assert!(
        err.message.contains("create the directory for"),
        "{}",
        err.message
    );
}
