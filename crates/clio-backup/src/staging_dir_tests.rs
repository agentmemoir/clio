// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the backup directory's creation: a
//! fresh leaf is owner-only, and a directory the operator already widened keeps
//! the mode it has.
//!
//! ## Owns
//! - The mode assertions over `ensure_backup_dir`, and the `bool` it reports for
//!   a directory it created and for one that was already there
//!
//! ## Does not own
//! - Staging a member and digesting it (see `staging_tests`)
//! - The atomic publish (see `atomic_publish_tests`)
//!
//! ## Boundary
//!
//! Keep this file focused on the directory the staged members live in.

use super::*;
use crate::fixtures::Scratch;

#[test]
fn the_backup_directory_is_created_owner_only_and_left_alone_when_present() {
    let scratch = Scratch::new("dir-create");
    let fresh = scratch.join("backups");

    assert!(ensure_backup_dir(&fresh).expect("create"));
    assert!(fresh.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }
    assert!(
        !ensure_backup_dir(&fresh).expect("second call"),
        "already there"
    );
}

/// A directory the operator widened on purpose is not this function's to
/// tighten, so it keeps its mode and still reports that it created nothing.
#[cfg(unix)]
#[test]
fn an_existing_backup_directory_keeps_its_widened_mode() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("dir-existing-mode");
    let existing = scratch.join("backups");
    std::fs::create_dir(&existing).expect("an existing directory");
    std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o750))
        .expect("widen the mode");

    assert!(!ensure_backup_dir(&existing).expect("existing directory"));

    let mode = std::fs::metadata(&existing).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o750, "the widened mode is left as it was");
}

/// A path that cannot become a directory because its parent is a regular file
/// is a fault in this code, and the message names the path it could not make.
#[test]
fn a_path_a_file_stands_in_for_is_an_internal_fault_naming_the_path() {
    let scratch = Scratch::new("dir-impossible");
    let file = scratch.join("not-a-directory");
    std::fs::write(&file, b"a regular file").expect("write the parent");
    let blocked = file.join("backups");

    let error = ensure_backup_dir(&blocked).expect_err("a file cannot hold a directory");

    assert_eq!(error.code, clio_types::ErrorCode::Internal);
    assert!(
        error.message.contains(blocked.to_string_lossy().as_ref()),
        "the message names the path: {}",
        error.message
    );
}
