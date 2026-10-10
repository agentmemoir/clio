// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the archive file that holds
//! superseded sync keys: its JSON-lines format, its owner-only rule on create,
//! read, and append, and the refusals that guard the file itself.
//!
//! ## Owns
//! - Every test of `append_entry` and `read_entries`, and of the archive's
//!   owner-only rule on all three paths
//!
//! ## Does not own
//! - The rotation, the key lists, and generate's rotation behaviour (see
//!   `sync_key_rotation_gen_tests`)
//! - The raw file and sidecar behaviour (see `sync_key_file_tests`)
//! - The keystore wrapper itself (see `secret_key_store_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every archive here lives
//! in a unique temp directory removed when the guard drops, and no test here
//! reaches the machine's real Keychain or Secret Service.
//!
//! Keep this module focused on the archive file's rules.

use std::path::Path;

use super::*;
use crate::sync_key_test_support::TempDir;

// ---------------------------------------------------------------- archive

#[test]
fn an_archived_key_round_trips_through_the_file() {
    let dir = TempDir::new("archive-roundtrip");
    let first = ArchivedSyncKey {
        key_b64: "AAAA".to_owned(),
        created_at: Some("2026-01-01T00:00:00Z".to_owned()),
    };
    let second = ArchivedSyncKey {
        key_b64: "BBBB".to_owned(),
        created_at: None,
    };
    append_entry(Some(&dir.key()), &first).expect("append first");
    append_entry(Some(&dir.key()), &second).expect("append second");

    let entries = read_entries(Some(&dir.key())).expect("read back");

    assert_eq!(entries, vec![first, second], "file order is creation order");
}

#[test]
fn a_missing_archive_reads_as_nothing_held() {
    let dir = TempDir::new("archive-missing");

    let no_path = read_entries(None).expect("no path means no archive");
    assert!(no_path.is_empty(), "{no_path:?}");
    let absent = read_entries(Some(&dir.key())).expect("no file means no archive");
    assert!(absent.is_empty(), "{absent:?}");
}

#[test]
fn a_damaged_archive_line_is_refused_not_skipped() {
    let dir = TempDir::new("archive-damaged");
    // The valid entry creates the archive owner-only; the overwrite keeps the
    // file's mode and plants a line that is not an archived key.
    append_entry(
        Some(&dir.key()),
        &ArchivedSyncKey {
            key_b64: "AAAA".to_owned(),
            created_at: None,
        },
    )
    .expect("append");
    std::fs::write(dir.path().join(ARCHIVE_NAME), "{\"key_b64\": oops\n").expect("seed");

    let err = read_entries(Some(&dir.key())).expect_err("a damaged record is refused");

    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
    assert!(err.message.contains("damaged"), "{}", err.message);
}

#[cfg(unix)]
#[test]
fn a_group_readable_archive_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("archive-permissions");
    append_entry(
        Some(&dir.key()),
        &ArchivedSyncKey {
            key_b64: "AAAA".to_owned(),
            created_at: None,
        },
    )
    .expect("append");
    std::fs::set_permissions(
        dir.path().join(ARCHIVE_NAME),
        std::fs::Permissions::from_mode(0o644),
    )
    .expect("chmod");

    let err = read_entries(Some(&dir.key())).expect_err("not owner-only");

    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    assert!(err.message.contains("not owner-only"), "{}", err.message);
}

#[test]
fn a_newly_created_archive_is_owner_only_by_shape() {
    let dir = TempDir::new("archive-mode");
    append_entry(
        Some(&dir.key()),
        &ArchivedSyncKey {
            key_b64: "AAAA".to_owned(),
            created_at: None,
        },
    )
    .expect("append");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join(ARCHIVE_NAME))
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

// ------------------------------------------------------------- refusals

#[test]
fn append_entry_without_a_directory_is_refused() {
    // A bare relative file name has no directory, so the archive has nowhere
    // to go; the refusal names the setting that fixes it.
    let err = append_entry(
        Some(Path::new("clio-sync.key")),
        &ArchivedSyncKey {
            key_b64: "AAAA".to_owned(),
            created_at: None,
        },
    )
    .expect_err("nowhere to archive");

    assert_eq!(err.code, clio_types::ErrorCode::ConfigCorrupt);
    assert!(
        err.message.contains("CLIO_SYNC_KEY_FILE"),
        "{}",
        err.message
    );
}

#[test]
fn an_archive_that_cannot_be_opened_for_writing_is_refused() {
    let dir = TempDir::new("archive-blocked");
    // The archive path occupied by a directory cannot be opened for writing.
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

    let err = append_entry(
        Some(&dir.key()),
        &ArchivedSyncKey {
            key_b64: "AAAA".to_owned(),
            created_at: None,
        },
    )
    .expect_err("the archive refuses the write");

    assert_eq!(err.code, clio_types::ErrorCode::Internal);
    assert!(
        err.message
            .contains("cannot write the Clio sync key archive"),
        "{}",
        err.message
    );
}

#[cfg(unix)]
#[test]
fn an_appended_key_refuses_an_archive_that_is_not_owner_only() {
    // The append path must enforce the same owner-only rule the read path does:
    // the entry is raw key material, so writing it next to whatever could
    // already read the file discloses the key before the next read refuses.
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("archive-append-permissions");
    append_entry(
        Some(&dir.key()),
        &ArchivedSyncKey {
            key_b64: "AAAA".to_owned(),
            created_at: None,
        },
    )
    .expect("append");
    std::fs::set_permissions(
        dir.path().join(ARCHIVE_NAME),
        std::fs::Permissions::from_mode(0o644),
    )
    .expect("chmod");

    let err = append_entry(
        Some(&dir.key()),
        &ArchivedSyncKey {
            key_b64: "BBBB".to_owned(),
            created_at: None,
        },
    )
    .expect_err("not owner-only");

    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    assert!(err.message.contains("not owner-only"), "{}", err.message);
    let raw = std::fs::read_to_string(dir.path().join(ARCHIVE_NAME)).expect("read the archive");
    assert_eq!(
        raw.lines().filter(|line| !line.trim().is_empty()).count(),
        1,
        "the refused append wrote nothing"
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_archive_is_refused_not_read_as_empty() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("archive-unreadable");
    append_entry(
        Some(&dir.key()),
        &ArchivedSyncKey {
            key_b64: "AAAA".to_owned(),
            created_at: None,
        },
    )
    .expect("append");
    // Owner-only still passes the mode check, so the read itself is what
    // fails: the archive path is a directory.
    std::fs::remove_file(dir.path().join(ARCHIVE_NAME)).expect("drop the archive");
    std::fs::create_dir(dir.path().join(ARCHIVE_NAME)).expect("seed a directory");
    std::fs::set_permissions(
        dir.path().join(ARCHIVE_NAME),
        std::fs::Permissions::from_mode(0o600),
    )
    .expect("chmod");

    let err = read_entries(Some(&dir.key())).expect_err("a directory cannot be read");

    assert_eq!(err.code, clio_types::ErrorCode::Internal);
    assert!(
        err.message
            .contains("cannot read the Clio sync key archive"),
        "{}",
        err.message
    );
}
