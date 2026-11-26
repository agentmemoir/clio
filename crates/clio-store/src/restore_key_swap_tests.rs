// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the key-only key-store swap (`place_staged_key`): the
//! failure reporting the caller must surface, the staged-then-swapped sequence
//! when the staged key does not already sit beside its destination, and the
//! exclusive create that refuses a staging path this code did not just make.
//!
//! ## Owns
//! - The `place_staged_key` assertions
//! - The staging-file assertions: a planted entry is refused, owner-only mode
//!
//! ## Does not own
//! - The SQLite file swap (see `restore_swap_tests`)
//! - The restore flow that calls it (see `clio`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::restore_swap::tests::{Scratch, staged_key};
use std::path::PathBuf;

use super::*;

#[test]
fn a_staged_key_that_is_not_a_key_store_is_reported_and_the_destination_survives() {
    let scratch = Scratch::new("place-key-junk");
    let staged = scratch.fresh("junk.kms.json");
    std::fs::write(&staged, b"not a key store at all").expect("seed the staged file");
    let destination = scratch.join("live.kms.json");
    std::fs::write(&destination, b"the live key store").expect("seed the destination");

    let report = place_staged_key(&staged, &destination);

    assert!(report.is_partial());
    assert!(
        report
            .failure
            .as_deref()
            .unwrap_or("")
            .contains("key-store")
    );
    assert_eq!(
        std::fs::read(&destination).expect("the destination survived"),
        b"the live key store"
    );
}

#[test]
fn a_staged_key_outside_the_destination_directory_is_copied_then_swapped() {
    let scratch = Scratch::new("place-key-copy");
    let staged = staged_key(&scratch.0);
    let expected = std::fs::read(&staged).expect("read the staged key");
    // A subdirectory of the scratch dir: the staged key does not already
    // sit beside its destination, so the swap copies first.
    let destination_dir = scratch.join("data");
    std::fs::create_dir_all(&destination_dir).expect("the destination directory");
    let destination = destination_dir.join("clio.kms.json");

    let report = place_staged_key(&staged, &destination);

    assert!(!report.is_partial(), "{report:?}");
    assert!(report.replaced);
    assert_eq!(
        std::fs::read(&destination).expect("the replaced key store"),
        expected,
        "the key store is the staged bytes"
    );
    // On one filesystem the rename consumes the staged original, the same
    // way the SQLite swap's key rename does.
    assert!(
        !staged.exists(),
        "the staged key was consumed by the rename"
    );
}

#[test]
fn a_key_destination_with_no_directory_is_reported() {
    let scratch = Scratch::new("place-key-bare");
    let staged = staged_key(&scratch.0);
    // A destination with no parent directory: there is nowhere to stage the
    // copy, and the failure is reported, not a panic.
    //
    // `copy_then_swap` is called directly on purpose. `place_staged_key` only
    // reaches this branch after a rename has already failed, and a bare
    // relative destination renames successfully into the current directory
    // whenever that directory is writable. Going through the caller made this
    // test assert on the host's filesystem layout instead of on the code: it
    // failed on every machine, and it dropped a real key store into the crate
    // directory as a side effect.
    let bare = PathBuf::from("clio.kms.json");

    let report = copy_then_swap(&staged, &bare);

    assert!(report.is_partial());
    assert!(!report.replaced);
    assert!(
        report
            .failure
            .as_deref()
            .unwrap_or("")
            .contains("no directory"),
        "{report:?}"
    );
    // The branch reports before it touches the filesystem, so nothing is
    // created in the current directory.
    assert!(!bare.exists());
}

#[test]
fn a_copy_that_cannot_stage_is_reported_and_leaves_nothing() {
    let scratch = Scratch::new("place-key-nodir");
    let staged = staged_key(&scratch.0);
    // The destination's directory does not exist, so the staged copy
    // cannot be written; the failure is reported and the destination
    // stays absent.
    let destination = scratch.join("missing-dir").join("clio.kms.json");

    let report = place_staged_key(&staged, &destination);

    assert!(report.is_partial());
    assert!(!report.replaced);
    assert!(!destination.exists());
}

#[test]
fn a_rename_that_cannot_complete_is_reported_and_the_temp_copy_is_removed() {
    let scratch = Scratch::new("place-key-rename");
    let staged = staged_key(&scratch.0);
    // The destination is an existing directory: the copy stages fine, the
    // rename over it fails, and the staged temp copy must not survive.
    let destination = scratch.join("clio.kms.json");
    std::fs::create_dir_all(&destination).expect("make the destination a directory");

    let report = place_staged_key(&staged, &destination);

    assert!(report.is_partial());
    assert!(!report.replaced);
    assert!(
        report
            .failure
            .as_deref()
            .unwrap_or("")
            .contains("was not replaced"),
        "{report:?}"
    );
    // No temp copy survives the failed rename.
    let leftovers: Vec<_> = std::fs::read_dir(&scratch.0)
        .expect("read the scratch dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".clio-key-swap-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp copies left behind: {leftovers:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_at_the_staging_path_is_refused_and_the_victim_is_untouched() {
    use std::os::unix::fs::symlink;

    let scratch = Scratch::new("staging-symlink");
    let victim = scratch.fresh("victim.txt");
    std::fs::write(&victim, b"the victim's bytes").expect("seed the victim");
    // A planted symlink at the path this code will pick: writing through it
    // would put the key material into someone else's file.
    let planted = scratch.join("planted-swap");
    symlink(&victim, &planted).expect("plant the symlink");

    let error = create_staging_file(&planted).expect_err("a planted symlink is refused");

    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists, "{error:?}");
    assert_eq!(
        std::fs::read(&victim).expect("the victim survived"),
        b"the victim's bytes"
    );
}

#[test]
fn a_file_already_at_the_staging_path_is_refused_and_its_bytes_survive() {
    let scratch = Scratch::new("staging-existing");
    let planted = scratch.join("planted-swap");
    std::fs::write(&planted, b"planted bytes").expect("plant the file");

    let error = create_staging_file(&planted).expect_err("an existing file is refused");

    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists, "{error:?}");
    assert_eq!(
        std::fs::read(&planted).expect("the planted file survived"),
        b"planted bytes"
    );
}

#[cfg(unix)]
#[test]
fn a_staging_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let scratch = Scratch::new("staging-mode");
    let path = scratch.join("staging");

    let file = create_staging_file(&path).expect("create the staging file");
    drop(file);

    let mode = std::fs::metadata(&path)
        .expect("stat the staging file")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);
}
