// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the gates key material passes on its
//! way to and from the filesystem: a FIFO at the sync key file or at the archive,
//! the owner check on a descriptor, and a symlinked parent directory.
//!
//! ## Owns
//! - The cases where the *shape* of what sits at a key path is wrong: a FIFO that
//!   would block a blocking open, a file another user owns, and a parent
//!   directory that is a link
//!
//! ## Does not own
//! - The archive's format, mode on create, and owner-only refusals (see
//!   `sync_key_rotation_tests`)
//! - The key file's path resolution, read, write, and sidecar (see
//!   `sync_key_file_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds. Every path here comes from a
//! unique temp directory removed when the guard drops, and no test here reaches
//! the machine's real key material.
//!
//! Two cases can only run with a privilege this suite does not assume — root to
//! plant a file owned by another user, and a `mkfifo` binary to plant a pipe — so
//! each says so on stdout and returns instead of passing quietly for the wrong
//! reason.
//!
//! Keep this module focused on the shape of what sits at a key path.

// Every case here plants a FIFO, a link, or a foreign owner, none of which a
// non-Unix build can express, so the whole module is Unix-only rather than a
// module of `#[cfg(unix)]` items whose imports nothing would use.
#![cfg(unix)]

use crate::secret_key::KEY_LEN;
use crate::sync_key_rotation::{ARCHIVE_NAME, ArchivedSyncKey, append_entry};
use crate::sync_key_test_support::TempDir;

// ---------------------------------------------------------------------- FIFOs

#[cfg(unix)]
#[test]
fn a_fifo_at_the_archive_path_is_refused_without_hanging() {
    // `append(true)` is `O_WRONLY|O_APPEND`, and opening a FIFO with no reader
    // blocks before any descriptor check can run. The opener adds `O_NONBLOCK`
    // for exactly this case, so the verdict arrives instead of the process
    // hanging on a path another account chose.
    use std::sync::mpsc;
    use std::time::Duration;
    let dir = TempDir::new("gate-archive-fifo");
    let fifo = dir.path().join(ARCHIVE_NAME);
    let created = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .is_ok_and(|status| status.success());
    if !created {
        eprintln!("skipping: mkfifo unavailable for {}", fifo.display());
        return;
    }
    let (tx, rx) = mpsc::channel();
    let key = dir.key();
    let handle = std::thread::spawn(move || {
        let _ = tx.send(append_entry(
            Some(&key),
            &ArchivedSyncKey {
                key_b64: "AAAA".to_owned(),
                created_at: None,
            },
        ));
    });
    // The open is non-blocking, so the verdict arrives at once; the bound only
    // turns a regression back into a blocking open into a failure.
    let err = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the append to a FIFO blocked instead of refusing")
        .expect_err("a FIFO is not the archive file");
    handle.join().expect("appender thread");

    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    assert!(
        err.message.contains("not a regular file"),
        "the FIFO is refused as the wrong shape of file: {}",
        err.message
    );
    assert!(
        err.message.contains("sync key archive"),
        "the refusal names the archive: {}",
        err.message
    );
}

#[cfg(unix)]
#[test]
fn a_fifo_at_the_key_path_is_refused_without_hanging() {
    // The read path uses the same non-blocking no-follow open, so a FIFO there
    // cannot hang the process either.
    use std::sync::mpsc;
    use std::time::Duration;
    let dir = TempDir::new("gate-key-fifo");
    let created = std::process::Command::new("mkfifo")
        .arg(dir.key())
        .status()
        .is_ok_and(|status| status.success());
    if !created {
        eprintln!("skipping: mkfifo unavailable for {}", dir.key().display());
        return;
    }
    let read_path = dir.key();
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _ = tx.send(crate::sync_key_file::read_key(&read_path));
    });
    let err = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("reading a FIFO blocked instead of refusing")
        .expect_err("a FIFO is not a key file");
    handle.join().expect("reader thread");

    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    assert!(
        err.message.contains("not a regular file"),
        "{}",
        err.message
    );
}

// --------------------------------------------------------- symlinked parent

#[cfg(unix)]
#[test]
fn a_symlinked_archive_directory_is_refused() {
    // `O_NOFOLLOW` guards the archive's final name only, so the directory that
    // name is resolved in is judged itself: a `chmod` through a link would
    // tighten a directory Clio never meant to touch and leave the real parent
    // loose enough for another account to swap the archive entry.
    use std::os::unix::fs::PermissionsExt;
    let outside = TempDir::new("gate-archive-parent-target");
    std::fs::set_permissions(outside.path(), std::fs::Permissions::from_mode(0o755))
        .expect("chmod");
    let holder = TempDir::new("gate-archive-parent-link");
    let linked = holder.join("keys");
    std::os::unix::fs::symlink(outside.path(), &linked).expect("plant the directory link");

    let err = append_entry(
        Some(&linked.join(crate::sync_key_file::SYNC_KEY_FILE_NAME)),
        &ArchivedSyncKey {
            key_b64: "AAAA".to_owned(),
            created_at: None,
        },
    )
    .expect_err("a link is not a directory to keep key material in");

    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    let expected = format!(
        "sync key directory {}: refused because it is a symbolic link; replace it with a real \
         directory only the owner can enter and run again",
        linked.display()
    );
    assert_eq!(err.message, expected);
    let mode = std::fs::metadata(outside.path())
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o777,
        0o755,
        "the refused write neither tightened nor used the directory it was pointed at"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_key_directory_is_refused_for_the_key_and_the_sidecar() {
    // The same rule the archive applies: the directory the key's final name is
    // resolved in is judged itself, so the write neither follows the link nor
    // chmods through it.
    use std::os::unix::fs::PermissionsExt;
    let outside = TempDir::new("gate-key-parent-target");
    std::fs::set_permissions(outside.path(), std::fs::Permissions::from_mode(0o755))
        .expect("chmod");
    let holder = TempDir::new("gate-key-parent-link");
    let linked = holder.join("keys");
    std::os::unix::fs::symlink(outside.path(), &linked).expect("plant the directory link");
    let key = linked.join(crate::sync_key_file::SYNC_KEY_FILE_NAME);
    let expected = format!(
        "sync key directory {}: refused because it is a symbolic link; replace it with a real \
         directory only the owner can enter and run again",
        linked.display()
    );

    let write_err =
        crate::sync_key_file::write_key(&key, &[1_u8; KEY_LEN]).expect_err("a link is not a dir");
    assert_eq!(write_err.code, clio_types::ErrorCode::Forbidden);
    assert_eq!(write_err.message, expected);

    let sidecar_err = crate::sync_key_file::write_created(Some(&key), "2026-10-01T00:00:00Z")
        .expect_err("the sidecar's directory is refused the same way");
    assert_eq!(sidecar_err.code, clio_types::ErrorCode::Forbidden);
    assert_eq!(sidecar_err.message, expected);

    assert!(
        std::fs::read_dir(outside.path())
            .expect("read the real directory")
            .next()
            .is_none(),
        "nothing was written through the link"
    );
    let mode = std::fs::metadata(outside.path())
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o777,
        0o755,
        "the refusal did not chmod through the link either"
    );
}

#[cfg(unix)]
#[test]
fn a_key_directory_that_is_a_regular_file_is_refused() {
    // A parent that exists but is not a directory is the other half of the same
    // rule, and it must be refused by the shape check rather than by a `chmod`
    // that happened to fail.
    let dir = TempDir::new("gate-parent-file");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"not a directory").expect("blocker");

    let err = crate::sync_key_file::write_created(
        Some(&blocker.join(crate::sync_key_file::SYNC_KEY_FILE_NAME)),
        "2026-10-01T00:00:00Z",
    )
    .expect_err("a file is not a directory");

    assert_eq!(err.code, clio_types::ErrorCode::Internal);
    assert!(
        err.message.contains("create the directory for"),
        "the step that failed is named: {}",
        err.message
    );
}

// ------------------------------------------------------------------- owner

#[cfg(unix)]
#[test]
fn a_key_file_owned_by_another_user_is_refused_for_its_owner_not_its_mode() {
    // The owner check is explicit, because a successful open does not imply
    // ownership: root appends to any file whatever its mode, and an ACL can grant
    // a 0600 file another account owns. The calling user is supplied here so one
    // machine and one account can exercise the rule, on the same seam `auth_tokens`
    // uses for its token store.
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = TempDir::new("gate-foreign-owner");
    std::fs::write(dir.key(), [1_u8; KEY_LEN]).expect("seed");
    std::fs::set_permissions(dir.key(), std::fs::Permissions::from_mode(0o600)).expect("chmod");
    let file = std::fs::File::open(dir.key()).expect("open the key file");
    let real_owner = file.metadata().expect("metadata").uid();

    let err = super::descriptor_refusal_for_uid(
        &file,
        &dir.key(),
        "sync key file",
        real_owner.wrapping_add(1),
    )
    .expect_err("a file owned by another user is not ours");

    assert_eq!(err.code, clio_types::ErrorCode::Forbidden);
    let expected = format!(
        "sync key file {}: refused because it is owned by a user other than the one running Clio; \
         change its owner to the one running Clio and run again",
        dir.key().display()
    );
    assert_eq!(
        err.message, expected,
        "the refusal names the owner as the reason and reports no mode value"
    );
    assert_eq!(
        file.metadata().expect("metadata").permissions().mode() & 0o777,
        0o600,
        "the file was refused for its owner, not for its mode"
    );
    super::descriptor_refusal_for_uid(&file, &dir.key(), "sync key file", real_owner)
        .expect("the same file is accepted when the calling user owns it");
}
