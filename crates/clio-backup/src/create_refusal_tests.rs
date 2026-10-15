// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the create-path refusal cases: the archive a
//! host cannot write because its backup directory refuses writes.
//!
//! ## Owns
//! - The read-only backup directory refusal, asserted for whichever outcome the
//!   running process can observe
//!
//! ## Does not own
//! - The create-path assertions over archives that are produced (see
//!   `create_tests.rs`)
//! - The staging primitives (see `staging_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::fixtures::{
    Scratch, dir_entries, key_source, names_with_extension, request, staged_payload,
    write_is_refused,
};

/// A read-only backup directory is a real refusal of its own: the directory is
/// there, so the earlier "missing directory" case never reaches the create at
/// all. Nothing may be written into it, and no archive may appear.
///
/// The refusal is only observable to a process the mode bits restrict. Root,
/// and any process holding `CAP_DAC_OVERRIDE`, writes into the directory
/// anyway, so the test asks the process's own access which case this is and
/// asserts what that case allows.
#[cfg(unix)]
#[test]
fn a_read_only_backup_directory_produces_no_archive() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("create-read-only");
    let payload = staged_payload(scratch.path(), 14);
    let readonly = scratch.join("readonly-backups");
    std::fs::create_dir(&readonly).expect("the backup directory");
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o500))
        .expect("make it read-only");
    let mut req = request(scratch.path(), payload);
    req.keys = Some(key_source(scratch.path(), 1));
    req.backup_dir = readonly.clone();

    if write_is_refused(&readonly) {
        let error = create_archive(&req).expect_err("a read-only directory takes no writes");
        assert_eq!(error.code, clio_types::ErrorCode::Internal);
        assert!(
            error.message.contains("keys.kms.db"),
            "the error names the member it was staging: {}",
            error.message
        );
        let entries = dir_entries(&readonly);
        assert!(entries.is_empty(), "nothing was written: {entries:?}");
        let archives = names_with_extension(scratch.path(), "gz");
        assert!(archives.is_empty(), "no archive appeared: {archives:?}");
    } else {
        // This process bypasses the mode bits, so the directory is not the
        // refusal under test: the archive is written where it was asked for.
        create_archive(&req).expect("a DAC-bypassing process writes into the directory");
        let archives = names_with_extension(&readonly, "gz");
        assert_eq!(
            archives.len(),
            1,
            "the archive lands in the backup directory: {archives:?}"
        );
    }
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o700))
        .expect("restore the mode so the scratch guard can remove it");
}
