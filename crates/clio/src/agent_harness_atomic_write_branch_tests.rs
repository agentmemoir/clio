// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the branch tests of the anchored atomic
//! write that need no scratch directory: the temp-name shape, the errno
//! mapper, a bare entry name, and the permission step of the sync helper.
//!
//! ## Owns
//! - One test per branch of `agent_harness_atomic_write` that a fixture cannot
//!   reach without a directory
//!
//! ## Does not own
//! - The atomic write implementation under test (`agent_harness_atomic_write`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

#![cfg(unix)]

use super::*;
use std::fs;
use std::path::Path;

/// The error detail of a refused write, for a message assertion.
fn io_detail(err: WriteError) -> String {
    match err {
        WriteError::Io { detail } => detail,
        WriteError::Unparseable { .. } => panic!("expected an I/O error"),
    }
}

/// A permission refusal is only observable without root, which bypasses the
/// check. Coverage runs as the runner user.
fn running_as_root() -> bool {
    rustix::process::geteuid().is_root()
}

/// The temp sibling is hidden, carries the entry name, is unique, and sits
/// beside its target.
#[test]
fn the_temp_sibling_name_is_hidden_and_unique() {
    let target = Path::new("/tmp/clio-atomic/opencode.json");
    let first = temp_sibling(target);
    let second = temp_sibling(target);
    let name = first
        .file_name()
        .and_then(|name| name.to_str())
        .expect("the temp sibling has a name");

    assert!(name.starts_with(".opencode.json."), "got {name}");
    assert!(
        Path::new(name).extension().is_some_and(|ext| ext == "tmp"),
        "got {name}"
    );
    assert!(name.contains("opencode.json"), "got {name}");
    assert_eq!(
        first.parent(),
        target.parent(),
        "the sibling stays beside it"
    );
    assert_ne!(first, second, "two calls make two names");
}

/// The errno mapper names the action, the path, and the errno.
#[test]
fn io_errno_names_the_action_the_path_and_the_errno() {
    let path = Path::new("/tmp/clio-atomic/missing.json");
    let missing = io_detail(io_errno("inspect", path, rustix::io::Errno::NOENT));
    let denied = io_detail(io_errno("inspect", path, rustix::io::Errno::ACCESS));

    assert!(
        missing.starts_with("failed to inspect /tmp/clio-atomic/missing.json: "),
        "got {missing}"
    );
    assert_ne!(missing, denied, "the errno text is part of the detail");
}

/// A name with no parent component opens the current directory.
#[test]
fn open_holds_the_current_directory_for_a_bare_name() {
    let anchored = AnchoredTarget::open(Path::new("clio-atomic-write-bare-name.json"))
        .expect("the current directory opens");
    drop(anchored);
}

/// A file that the process does not own refuses the permission step.
#[test]
fn write_and_sync_at_maps_a_permission_failure() {
    if running_as_root() {
        return;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open("/dev/null")
        .expect("/dev/null opens for writing");
    let err = write_and_sync_at(
        &mut file,
        b"bytes",
        rustix::fs::Mode::from_raw_mode(0o644),
        Path::new("/dev/null"),
    )
    .expect_err("a file owned by another user refuses fchmod");

    assert!(
        io_detail(err).contains("failed to set permissions on"),
        "the failure names the permission step"
    );
}
