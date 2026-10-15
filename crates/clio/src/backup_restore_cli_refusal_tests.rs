// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the restore surface's refusal edges that the
//! other restore suites do not cover: a repeated member name, a store handle
//! held open across the restore, which of two simultaneous refusals is
//! reported, an unwritable target directory, a target that is opened but
//! empty, and the two hostile-manifest refusals.
//!
//! ## Owns
//! - The CLI-level assertions for those six refusal edges
//!
//! ## Does not own
//! - The refusal rules themselves (see `backup_restore_flow` and
//!   `clio-backup`'s reader)
//! - The shared fixtures (see `backup_cli_fixtures` and the hand-built
//!   archive fixture)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{Scratch, key_store_path, populated_store, run_in, run_json_in};
use crate::backup_cli_fixtures_handmade::{hand_built_archive, running_as_root};
use clio_store::{DekProvider, LocalDevKms, SqliteStore, sqlite_content_digest};
use std::path::PathBuf;
use std::sync::Arc;

/// The default target store inside `scratch`.
fn store_path(scratch: &Scratch) -> PathBuf {
    scratch.join("clio.db")
}

/// The published archive names in the scratch backup directory.
fn archives(scratch: &Scratch) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(scratch.join("backups"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn a_repeated_key_member_is_refused_and_the_live_key_store_is_untouched() {
    let scratch = Scratch::new("restore-dup-key");
    populated_store(&store_path(&scratch), 2, 1);
    let (id, _) = hand_built_archive(
        &scratch,
        &store_path(&scratch),
        "clio-20260927T000000Z-dupkey01",
        true,
        |_| (),
    );
    let keys = key_store_path(&store_path(&scratch));
    let keys_before = std::fs::read(&keys).expect("read the live key store");
    let target_before = std::fs::read(store_path(&scratch)).expect("read the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("repeats the member name"), "the rule: {err}");
    assert!(err.contains("keys.kms.db"), "the member: {err}");
    assert_eq!(
        std::fs::read(&keys).expect("read the key store"),
        keys_before
    );
    assert_eq!(
        std::fs::read(store_path(&scratch)).expect("read the target"),
        target_before
    );
    assert_eq!(archives(&scratch).len(), 1);
}

#[test]
fn the_first_of_two_refusals_is_the_one_reported() {
    let scratch = Scratch::new("restore-order");
    populated_store(&store_path(&scratch), 2, 1);
    // The archive mismatches the backend AND carries a newer schema, so both
    // refusals hold at once; the backend check comes first in the flow.
    let (id, _) = crate::backup_cli_fixtures::archive_with_manifest(
        &scratch,
        &store_path(&scratch),
        "postgres",
        Some("99"),
        false,
    );
    let before = std::fs::read(store_path(&scratch)).expect("read the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes", "--allow-missing-keys"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("postgres"), "the reported refusal: {err}");
    assert!(
        !err.contains("newer than this build"),
        "the schema refusal must not win: {err}"
    );
    assert_eq!(
        std::fs::read(store_path(&scratch)).expect("read the target"),
        before
    );
}

#[test]
fn a_store_handle_held_open_across_the_restore_is_refused() {
    let scratch = Scratch::new("restore-hold-open");
    populated_store(&store_path(&scratch), 3, 1);
    let (id, _) = hand_built_archive(
        &scratch,
        &store_path(&scratch),
        "clio-20260927T000000Z-hold001",
        false,
        |_| (),
    );
    let kms: Arc<dyn DekProvider> = Arc::new(
        LocalDevKms::open_persistent(key_store_path(&store_path(&scratch)))
            .expect("open the key store"),
    );
    let held = SqliteStore::open(&store_path(&scratch), kms).expect("hold the target open");
    let target_before = std::fs::read(store_path(&scratch)).expect("read the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("open store handle"), "actionable: {err}");
    drop(held);
    assert_eq!(
        std::fs::read(store_path(&scratch)).expect("read the target"),
        target_before
    );
    assert_eq!(
        clio_store::sqlite_file_state(&store_path(&scratch))
            .expect("measure the target")
            .count_of("items"),
        Some(3),
        "the old store is intact"
    );
}

#[cfg(unix)]
#[test]
fn an_unwritable_target_directory_is_refused_before_any_snapshot() {
    use std::os::unix::fs::PermissionsExt as _;
    if running_as_root() {
        return;
    }
    let scratch = Scratch::new("restore-unwritable-target");
    let id = snapshot_fixture(&scratch);
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");
    std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o555))
        .expect("chmod");

    let (code, _, err) = run_in(&scratch, &["restore", &id, "--yes"], false);

    std::fs::set_permissions(scratch.path(), std::fs::Permissions::from_mode(0o700))
        .expect("chmod back");
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("not writable"), "{err}");
    assert!(
        err.contains(scratch.path().display().to_string().as_str()),
        "the target's directory is named: {err}"
    );
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);
}

/// A populated store plus one archive of it, through the real create path.
fn snapshot_fixture(scratch: &Scratch) -> String {
    populated_store(&store_path(scratch), 3, 1);
    crate::backup_cli_fixtures::ensure_journal(scratch);
    let (_, payload, _) = run_json_in(scratch, &["create"]);
    payload["snapshot_id"]
        .as_str()
        .expect("the create fixture produced a snapshot id")
        .to_owned()
}

#[test]
fn a_target_opened_once_but_bootsrapped_still_gets_the_snapshot() {
    let scratch = Scratch::new("restore-opened-empty");
    let store = store_path(&scratch);
    let kms_file =
        LocalDevKms::open_persistent(key_store_path(&store)).expect("open the key store");
    // Writing one slot is what makes the key store file exist, which the
    // archive's key member reads.
    kms_file.ensure_dek("subject-0000").expect("write a slot");
    let kms: Arc<dyn DekProvider> = Arc::new(kms_file);
    let opened = SqliteStore::open(&store, kms).expect("open the store once");
    drop(opened);
    let (id, _) = hand_built_archive(
        &scratch,
        &store,
        "clio-20260927T000000Z-empty01",
        false,
        |_| (),
    );

    let (code, payload, err) = run_json_in(&scratch, &["restore", &id, "--yes"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        payload["result"],
        serde_json::json!("restored"),
        "{payload}"
    );
    // Opening a store bootstraps it, so it holds settings rows even with no
    // memories. A restore replaces the whole file, so those rows are something
    // to preserve: skipping the snapshot here destroyed them with no way back.
    assert_eq!(
        payload["pre_restore_snapshot"]["skipped"],
        serde_json::json!(false),
        "{payload}"
    );
    assert!(
        payload["pre_restore_snapshot"]["id"].is_string(),
        "the snapshot that is the way back is named: {payload}"
    );
}

#[test]
fn an_unknown_manifest_format_version_is_refused() {
    let scratch = Scratch::new("restore-format");
    populated_store(&store_path(&scratch), 2, 1);
    let (id, _) = hand_built_archive(
        &scratch,
        &store_path(&scratch),
        "clio-20260927T000000Z-format01",
        false,
        |manifest| manifest.format_version = 999,
    );
    let before = std::fs::read(store_path(&scratch)).expect("read the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("format_version"), "{err}");
    assert_eq!(
        std::fs::read(store_path(&scratch)).expect("read the target"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);
}

#[test]
fn a_wrong_manifest_kind_is_refused() {
    let scratch = Scratch::new("restore-kind");
    populated_store(&store_path(&scratch), 2, 1);
    let (id, _) = hand_built_archive(
        &scratch,
        &store_path(&scratch),
        "clio-20260927T000000Z-kind001",
        false,
        |manifest| manifest.kind = "another-kind".to_owned(),
    );
    let before = std::fs::read(store_path(&scratch)).expect("read the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("kind"), "{err}");
    assert_eq!(
        std::fs::read(store_path(&scratch)).expect("read the target"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);
}
