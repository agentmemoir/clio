// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that drive the shipped backup path
//! with its stamp fixed, so the name the call will build is known before the
//! call and an entry can be planted at exactly that name.
//!
//! ## Owns
//! - The stamped-name refusal: a symlink planted at the exact name the shipped
//!   path builds is refused by that path, and neither the link, nor the bytes,
//!   nor the mode of the file it points at change
//! - The stamped-name success: with no entry at that name, the shipped path
//!   produces the backup its caller is promised
//!
//! ## Does not own
//! - The exclusive-create, dangling-link, regular-file, creation-time mode and
//!   directory-source tests, which drive `create_backup` / `write_backup` with
//!   an explicit target (`agent_harness_write_backup_tests`)
//! - The clock: `backup_file` still reads it. Only the stamp it reads is
//!   supplied here, which is the whole point of the seam
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

#![cfg(unix)]

use super::backup_tests::{Scratch, mode_bits};
use super::*;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

/// The stamp every test here hands the shipped path, so the name it builds is
/// known in advance instead of read from the clock.
const STAMP: u128 = 1_700_000_000_000_000_000;

/// The name the shipped path builds for `source` under `nanos`, spelled out in
/// the test rather than imported, so a change to the format has to be a change
/// in two places instead of silently agreeing with itself.
fn stamped_path(source: &Path, nanos: u128) -> PathBuf {
    let name = file_name(source);
    source.with_file_name(format!("{name}.clio-backup-{nanos}"))
}

/// A source config in its own directory, with a fixed mode, returned with the
/// name the shipped path will copy it to.
fn fixed_source(scratch: &Scratch, mode: u32) -> (PathBuf, PathBuf) {
    fs::create_dir_all(scratch.path("config")).expect("config directory can be created");
    let source = scratch.write("config/opencode.json", b"{\"mcp\":{\"servers\":{}}}");
    fs::set_permissions(&source, fs::Permissions::from_mode(mode)).expect("mode can be set");
    let target = stamped_path(&source, STAMP);
    (source, target)
}

/// A link planted at the stamped name must be refused by the shipped path
/// itself, not merely by the create it happens to call.
///
/// This is the assertion a plain copy at the call site cannot satisfy, and the
/// reason the stamp is a parameter: `std::fs::copy` reproduces the bytes, the
/// directory, and the mode for a fresh name, so every other observable of this
/// entry point is identical whether it copies or creates exclusively. Its only
/// distinguishing behaviour is a name that is already taken. Planting one at the
/// name the call will build needs the name before the call, which is the seam.
/// A copy here follows the link, truncates the victim, and stamps the source's
/// mode onto it, so the victim's bytes and mode both change.
///
/// The link is absolute on purpose: a relative one would resolve against the
/// source's directory, dangle, and let a copy "succeed" without touching the
/// victim — a test that proves nothing. The victim also sits outside the
/// source's directory and carries a mode the source does not, so neither
/// observable can coincide by accident.
#[test]
fn the_shipped_backup_path_refuses_a_symlink_planted_at_the_stamped_name() {
    let scratch = Scratch::new("stamped-link");
    let (source, target) = fixed_source(&scratch, 0o640);
    let victim = scratch.write("victim.json", b"the file behind the link");
    fs::set_permissions(&victim, fs::Permissions::from_mode(0o600)).expect("mode can be set");
    let victim_bytes = fs::read(&victim).expect("victim can be read");
    let victim_mode = mode_bits(&victim);
    assert!(
        victim.is_absolute(),
        "the planted link must be absolute, or it resolves against the \
         source's directory and dangles: {}",
        victim.display()
    );
    assert_ne!(victim_mode, mode_bits(&source), "the two modes must differ");
    symlink(&victim, &target).expect("a symlink can be planted at the stamped name");

    let outcome = backup_file_at(&source, STAMP);

    assert!(
        matches!(outcome, Err(WriteError::Io { .. })),
        "a link at the stamped name must be refused as an I/O error, got {outcome:?}"
    );
    assert_eq!(
        fs::read(&victim).expect("victim can be read"),
        victim_bytes,
        "nothing may be written through the link, so the victim's bytes stay \
         exactly as they were"
    );
    assert_eq!(
        mode_bits(&victim),
        victim_mode,
        "nothing may reach the victim, so its mode stays exactly as it was"
    );
    assert!(
        fs::symlink_metadata(&target)
            .expect("an entry survives at the stamped name")
            .file_type()
            .is_symlink(),
        "the stamped name must still hold the planted symlink, not a regular file"
    );
    assert_eq!(
        fs::read_link(&target).expect("the link can be read"),
        victim,
        "the link must still point at the victim"
    );
}

/// With nothing at the stamped name, the shipped path creates the backup its
/// caller is promised: the source's bytes, beside the source, under the stamped
/// name, and — the name being fresh — with the source's exact permission bits.
///
/// This pins the happy path through the same function the refusal test drives,
/// so the seam is shown to be the shipped path and not a test-only side door.
#[test]
fn the_shipped_backup_path_creates_the_backup_at_the_stamped_name() {
    let scratch = Scratch::new("stamped-fresh");
    let (source, target) = fixed_source(&scratch, 0o666);
    let source_bytes = fs::read(&source).expect("source can be read");
    assert!(
        fs::symlink_metadata(&target).is_err(),
        "nothing may hold the stamped name before the call: {}",
        target.display()
    );

    let outcome = backup_file_at(&source, STAMP);

    let backup = outcome
        .expect("the source exists, so it is backed up")
        .expect("a source that exists yields a backup path");
    assert_eq!(
        backup, target,
        "the backup must be created at the stamped name beside the source"
    );
    assert_eq!(
        fs::read(&backup).expect("the backup can be read"),
        source_bytes,
        "the backup must hold exactly the source's bytes"
    );
    assert_eq!(
        mode_bits(&backup),
        0o666,
        "a fresh backup must carry the source's exact permission bits, not the \
         creation mode the process mask left"
    );
    assert_eq!(
        fs::read(&source).expect("source can still be read"),
        source_bytes,
        "backing up must leave the source's bytes unchanged"
    );
}
