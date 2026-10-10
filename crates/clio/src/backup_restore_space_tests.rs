// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the key placement a restore computes from the archive's
//! key member, including the refusal when the named target has nowhere to put
//! it.
//!
//! ## Owns
//! - `key_destination` behavior and its `missing_key_store` refusal
//! - `preflight`'s footprints: which members it reserves room for
//!
//! ## Does not own
//! - The key swap mechanics (see `clio-store`'s `restore_swap`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{key_destination, preflight};
use crate::backup_cli::ResolvedTarget;
use clio_store::SpaceCheck;
use clio_types::{AmError, ErrorCode};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// A SQLite target whose key store is at `kms_path`.
fn target_with(kms_path: Option<PathBuf>) -> ResolvedTarget {
    ResolvedTarget {
        url: "/data/clio.db".to_owned(),
        backend: "sqlite".to_owned(),
        database: "clio.db".to_owned(),
        display: "/data/clio.db".to_owned(),
        path: PathBuf::from("/data/clio.db"),
        kms_path,
    }
}

/// A Postgres target, which has no key-store file path at all.
fn postgres_target() -> ResolvedTarget {
    ResolvedTarget {
        url: "postgres://clio:clio@127.0.0.1:34310/clio".to_owned(),
        backend: "postgres".to_owned(),
        database: "clio".to_owned(),
        display: "clio@127.0.0.1:34310/clio".to_owned(),
        path: PathBuf::from("/data/clio.db"),
        kms_path: None,
    }
}

#[test]
fn a_key_member_needs_a_live_key_store_path() {
    let staged = PathBuf::from("/staged/keys.kms.db");
    let without = target_with(None);
    let error = key_destination(Some(&staged), &without).expect_err("no live key-store path");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(
        error.message().contains("key member"),
        "{}",
        error.message()
    );

    let target = target_with(Some(PathBuf::from("/live/keys.kms.db")));
    let placement = key_destination(Some(&staged), &target)
        .expect("a placement")
        .expect("the key member is placed");
    assert_eq!(placement.staged, staged);
    assert_eq!(placement.destination, PathBuf::from("/live/keys.kms.db"));

    assert!(
        key_destination(None, &target)
            .expect("no key member")
            .is_none()
    );
}

#[test]
fn a_postgres_target_is_told_its_key_store_has_no_path() {
    // A Postgres key store is not a file, so the "point --db at a file-backed
    // SQLite store" hint would be wrong advice for this target.
    let staged = PathBuf::from("/staged/keys.kms.db");
    let error = key_destination(Some(&staged), &postgres_target()).expect_err("no file path");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    let hint = error.hint().expect("a hint");
    assert!(hint.contains("no file path"), "{hint}");
}

/// A probe that answers from a byte limit and records every footprint it was
/// asked about, so a pre-flight's room can be shaped without a real
/// filesystem and the footprints it built can be read back.
struct Room {
    limit: u64,
    asked: RefCell<Vec<(PathBuf, u64)>>,
}

impl Room {
    fn new(limit: u64) -> Self {
        Self {
            limit,
            asked: RefCell::new(Vec::new()),
        }
    }
}

impl SpaceCheck for Room {
    fn can_hold(&self, dir: &Path, bytes: u64) -> Result<(), AmError> {
        self.asked.borrow_mut().push((dir.to_path_buf(), bytes));
        if bytes > self.limit {
            return Err(AmError::new(ErrorCode::QuotaExceeded, "no room here"));
        }
        Ok(())
    }
}

#[test]
fn a_footprint_the_manifest_has_no_size_for_is_not_reserved() {
    // An archive written before the manifest recorded a key size reads as
    // `None`, and reserving a guess would be reserving a number nothing backs.
    let room = Room::new(1_000);
    preflight(
        Path::new("/data"),
        Path::new("/backups"),
        10,
        None,
        20,
        &room,
    )
    .expect("the payload and the snapshot fit");

    assert_eq!(
        *room.asked.borrow(),
        vec![
            (PathBuf::from("/data"), 10),
            (PathBuf::from("/backups"), 20),
        ],
        "no key footprint is reserved for an archive that records no key size"
    );
}

#[test]
fn a_key_member_that_does_not_fit_is_refused_before_anything_is_staged() {
    // The payload and the pre-restore snapshot both fit; only the key member
    // does not. The refusal has to name the key member, because that is the
    // footprint that pushed the shared directory's total over the limit.
    let room = Room::new(100);
    let error = preflight(
        Path::new("/data"),
        Path::new("/backups"),
        10,
        Some(500),
        20,
        &room,
    )
    .expect_err("the key member does not fit");

    assert_eq!(error.exit_code(), ExitCode::from(1), "{}", error.message());
    assert!(
        error.message().contains("key member"),
        "{}",
        error.message()
    );
    assert_eq!(
        *room.asked.borrow(),
        vec![(PathBuf::from("/data"), 510)],
        "the key member is staged beside the payload, so the payload's directory is asked once \
         for their sum, and the pre-restore snapshot's directory is never reached"
    );
}

#[test]
fn two_members_that_each_fit_but_whose_sum_does_not_are_refused() {
    // The case the sum exists for: 60 and 60 both fit under 100, the pair does
    // not, and a pre-flight that asked per footprint would say yes. The run
    // must stop here, before anything is staged, naming both members and the
    // directory they share.
    let room = Room::new(100);
    let error = preflight(
        Path::new("/data"),
        Path::new("/backups"),
        60,
        Some(60),
        20,
        &room,
    )
    .expect_err("60 + 60 does not fit where 100 are available");

    assert_eq!(error.exit_code(), ExitCode::from(1), "{}", error.message());
    assert!(
        error.message().contains(
            "the restored payload and the archive's key member cannot be placed in /data"
        ),
        "the refusal names both members of the shared directory and the directory itself: {}",
        error.message()
    );
    assert_eq!(
        *room.asked.borrow(),
        vec![(PathBuf::from("/data"), 120)],
        "the shared directory is asked once for the sum, and the snapshot's directory is never \
         reached"
    );
}

#[test]
fn two_members_whose_sum_fits_are_asked_once_for_that_sum() {
    let room = Room::new(100);
    preflight(
        Path::new("/data"),
        Path::new("/backups"),
        60,
        Some(30),
        20,
        &room,
    )
    .expect("90 fits where 100 are available");

    assert_eq!(
        *room.asked.borrow(),
        vec![
            (PathBuf::from("/data"), 90),
            (PathBuf::from("/backups"), 20),
        ],
        "the payload and the key member are staged side by side, so their directory is asked \
         once for 90; the snapshot's own directory is still asked separately"
    );
}

#[test]
fn a_postgres_shaped_restore_asks_one_directory_for_all_three_members() {
    // This is the shape `backup_restore_postgres.rs` builds: the dump, the key
    // member, and the snapshot all go in the backup directory, so all three
    // footprints share it. There is nothing to sum against another directory,
    // and the whole restore's room requirement is one number for one directory.
    let room = Room::new(100);
    let error = preflight(
        Path::new("/backups"),
        Path::new("/backups"),
        60,
        Some(60),
        20,
        &room,
    )
    .expect_err("60 + 60 + 20 does not fit where 100 are available");

    assert_eq!(error.exit_code(), ExitCode::from(1), "{}", error.message());
    assert_eq!(
        *room.asked.borrow(),
        vec![(PathBuf::from("/backups"), 140)],
        "one directory, one ask, carrying the dump, the key member, and the snapshot together"
    );
    assert!(
        error.message().contains(
            "the restored payload, the archive's key member and the pre-restore snapshot cannot \
             be placed in /backups"
        ),
        "every member of the shared directory is named: {}",
        error.message()
    );
}

#[test]
fn a_key_member_that_fits_is_reserved_beside_the_payload() {
    let room = Room::new(1_000);
    preflight(
        Path::new("/data"),
        Path::new("/backups"),
        10,
        Some(30),
        20,
        &room,
    )
    .expect("every footprint fits");

    assert_eq!(
        *room.asked.borrow(),
        vec![
            (PathBuf::from("/data"), 40),
            (PathBuf::from("/backups"), 20),
        ],
        "the payload and the key member are staged side by side, so the directory they share is \
         asked once for their sum of 40, and the snapshot's own directory is asked separately"
    );
}
