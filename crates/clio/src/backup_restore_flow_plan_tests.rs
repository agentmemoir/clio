// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the restore plan's refusals at the `plan` boundary: the
//! backup directory that cannot be read, and the key-size rule the manifest
//! read enforces.
//!
//! ## Owns
//! - The unreadable-backup-directory refusal assertion
//! - The keyed and keyless plan round trips, and the reservation each one makes
//! - The refusal for a keyed manifest whose key size is missing or zero, and
//!   the proof that such a refusal touches nothing
//!
//! ## Does not own
//! - The key-size rule itself (see `clio-backup`'s manifest validation)
//! - The plan's other refusals (see `backup_restore_flow_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{Scratch, archive_with_manifest, key_store_path, populated_store};
use crate::backup_cli_fixtures_handmade::hand_built_archive;
use crate::cli_args::{self, FlagSpec};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Parsed restore flags over `args`, the tokens the parser sees after the verb.
fn parsed_restore(args: &[&str]) -> cli_args::ParsedArgs {
    let spec = FlagSpec::new(
        &[],
        &[
            "latest",
            "yes",
            "no-pre-backup",
            "allow-missing-keys",
            "dry-run",
        ],
    );
    cli_args::parse(&spec, args.iter().map(|arg| (*arg).to_owned())).expect("parse flags")
}

/// A space check that grants every ask and records each one, so a plan's
/// reservations can be read back.
struct Asked(RefCell<Vec<(PathBuf, u64)>>);

impl Asked {
    fn new() -> Self {
        Self(RefCell::new(Vec::new()))
    }
}

impl clio_store::SpaceCheck for Asked {
    fn can_hold(&self, dir: &Path, bytes: u64) -> Result<(), clio_types::AmError> {
        self.0.borrow_mut().push((dir.to_path_buf(), bytes));
        Ok(())
    }
}

/// Plan the restore of `args` against `scratch`, with every footprint ask read
/// back afterwards.
fn plan_in(scratch: &Scratch, args: &[&str]) -> (Result<Plan, CliError>, Vec<(PathBuf, u64)>) {
    let (env, cwd) = scratch.targets();
    let pairs = env.pairs();
    let lookup = crate::backup_cli_fixtures::env_from(&pairs);
    let asked = Asked::new();
    let result = plan(&lookup, &parsed_restore(args), &cwd, &asked);
    let asks = asked.0.into_inner();
    (result, asks)
}

/// The published archive names in the scratch backup directory.
fn archives(scratch: &Scratch) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(scratch.join("backups"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(clio_backup::SNAPSHOT_EXTENSION))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// The bytes the pre-restore snapshot is projected to need: the target file
/// plus the key store beside it.
fn projected_bytes(store: &Path) -> u64 {
    let keys = key_store_path(store);
    std::fs::metadata(store).expect("stat the target").len()
        + std::fs::metadata(keys).expect("stat the key store").len()
}

#[test]
fn a_backup_path_that_cannot_be_read_refuses_the_plan() {
    let scratch = Scratch::new("restore-plan-blocked");
    // A regular file where the backup directory belongs: the listing cannot
    // read it, and that is not a NotFound, so the plan refuses.
    let blocker = scratch.join("backups");
    std::fs::write(&blocker, b"not a directory").expect("write the blocker file");
    let (env, cwd) = scratch.targets();
    let pairs = env.pairs();
    let lookup = crate::backup_cli_fixtures::env_from(&pairs);
    let parsed = parsed_restore(&[]);

    let Err(error) = plan(&lookup, &parsed, &cwd, &clio_store::DiskSpace) else {
        panic!("the backup path cannot be listed");
    };

    assert_eq!(error.exit_code(), ExitCode::from(1), "{}", error.message());
    assert!(error.message().contains("reading"), "{}", error.message());
    assert!(error.message().contains("backups"), "{}", error.message());
}

#[test]
fn a_keyed_archives_plan_reserves_room_using_the_manifests_key_bytes() {
    let scratch = Scratch::new("restore-plan-keyed");
    let store = scratch.join("clio.db");
    populated_store(&store, 2, 1);
    let (id, archive) = hand_built_archive(
        &scratch,
        &store,
        "clio-20260927T000000Z-keyed01",
        false,
        |_| (),
    );
    let manifest = clio_backup::read_manifest(&archive).expect("read the manifest back");
    let key_bytes = manifest
        .keys
        .bytes
        .expect("a keyed manifest records its size");

    let (planned, asks) = plan_in(&scratch, &[&id]);

    let plan = planned.expect("the keyed archive plans");
    assert!(
        manifest.keys.member.is_some(),
        "the archive carries a key member"
    );
    assert_eq!(
        asks,
        vec![
            (
                scratch.path().to_path_buf(),
                manifest.payload.bytes + key_bytes,
            ),
            (scratch.join("backups"), projected_bytes(&store)),
        ],
        "the payload and the key member are reserved together in the target's directory, at \
         the bytes the manifest declares"
    );
    assert!(plan.key.is_some(), "the plan carries the key placement");
}

#[test]
fn a_keyless_archives_plan_reserves_no_key_footprint() {
    let scratch = Scratch::new("restore-plan-keyless");
    let store = scratch.join("clio.db");
    populated_store(&store, 2, 1);
    let (id, archive) = archive_with_manifest(&scratch, &store, "sqlite", Some("1"), false);
    let manifest = clio_backup::read_manifest(&archive).expect("read the manifest back");
    assert!(
        manifest.keys.member.is_none() && manifest.keys.bytes.is_none(),
        "the archive is keyless"
    );

    let (planned, asks) = plan_in(&scratch, &[&id, "--allow-missing-keys"]);

    let plan = planned.expect("a keyless archive plans without a key size");
    assert_eq!(
        asks,
        vec![
            (scratch.path().to_path_buf(), manifest.payload.bytes),
            (scratch.join("backups"), projected_bytes(&store)),
        ],
        "the target's directory is asked for the payload alone, so no key footprint is \
         reserved and nothing is over-reserved"
    );
    assert!(plan.key.is_none(), "there is no key member to place");
}

#[test]
fn a_keyed_manifest_without_a_key_size_refuses_the_plan_and_touches_nothing() {
    let scratch = Scratch::new("restore-plan-keysize-missing");
    let store = scratch.join("clio.db");
    populated_store(&store, 2, 1);
    let (id, _) = hand_built_archive(
        &scratch,
        &store,
        "clio-20260927T000000Z-nosize01",
        false,
        |manifest| manifest.keys.bytes = None,
    );
    let keys = key_store_path(&store);
    let target_before = std::fs::read(&store).expect("read the target");
    let keys_before = std::fs::read(&keys).expect("read the key store");

    let (planned, asks) = plan_in(&scratch, &[&id]);

    let Err(error) = planned else {
        panic!("a keyed manifest without a size is invalid");
    };
    assert_eq!(error.exit_code(), ExitCode::from(2), "{}", error.message());
    assert_eq!(
        error.code(),
        "archive_corrupt",
        "the manifest read's own machine code"
    );
    assert!(
        error.message().contains("keys.bytes"),
        "{}",
        error.message()
    );
    assert!(
        asks.is_empty(),
        "no footprint is reserved for a manifest that cannot be trusted"
    );
    assert_eq!(
        std::fs::read(&store).expect("read the target"),
        target_before,
        "the target store is byte-identical"
    );
    assert_eq!(
        std::fs::read(&keys).expect("read the key store"),
        keys_before,
        "the live key store is byte-identical"
    );
    assert_eq!(
        archives(&scratch).len(),
        1,
        "no pre-restore snapshot was taken"
    );
}

#[test]
fn a_keyed_manifest_with_a_zero_key_size_refuses_the_plan_and_touches_nothing() {
    let scratch = Scratch::new("restore-plan-keysize-zero");
    let store = scratch.join("clio.db");
    populated_store(&store, 2, 1);
    let (id, _) = hand_built_archive(
        &scratch,
        &store,
        "clio-20260927T000000Z-zerosize1",
        false,
        |manifest| manifest.keys.bytes = Some(0),
    );
    let keys = key_store_path(&store);
    let target_before = std::fs::read(&store).expect("read the target");
    let keys_before = std::fs::read(&keys).expect("read the key store");

    let (planned, asks) = plan_in(&scratch, &[&id]);

    let Err(error) = planned else {
        panic!("a keyed manifest with a zero size is invalid");
    };
    assert_eq!(error.exit_code(), ExitCode::from(2), "{}", error.message());
    assert!(
        error.message().contains("keys.bytes"),
        "{}",
        error.message()
    );
    assert!(
        error.message().contains("greater than zero"),
        "{}",
        error.message()
    );
    assert!(
        asks.is_empty(),
        "no footprint is reserved for a manifest that cannot be trusted"
    );
    assert_eq!(
        std::fs::read(&store).expect("read the target"),
        target_before,
        "the target store is byte-identical"
    );
    assert_eq!(
        std::fs::read(&keys).expect("read the key store"),
        keys_before,
        "the live key store is byte-identical"
    );
    assert_eq!(
        archives(&scratch).len(),
        1,
        "no pre-restore snapshot was taken"
    );
}
