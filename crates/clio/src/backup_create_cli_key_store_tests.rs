// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests how `clio backup create` resolves the key store: that a
//! key store it cannot read fails the run and writes no archive, and that a
//! target with no key-store file at all is still snapshotted and reported as
//! keyless.
//!
//! ## Owns
//! - The refusal assertions for a non-SQLite, damaged, or unreadable key store
//! - The keyless target that has no key-store file
//!
//! ## Does not own
//! - The general `create` contract (see `backup_create_cli_tests`)
//! - The key-store read itself (see `clio-store`)
//
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{Scratch, key_store_path, populated_store, run_in, run_json_in};
use crate::cli_error::EXIT_OPERATIONAL;
use clio_backup::read_manifest;
use serde_json::json;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

/// The store a snapshot reads, inside `scratch`.
fn store_path(scratch: &Scratch) -> PathBuf {
    scratch.join("clio.db")
}

/// A file at the resolved key-store path that is not a SQLite key-store
/// database. The `.json` in the name is historical; this is what an operator
/// holding bytes the key store has not been since would put there.
fn plant_foreign_key_store(scratch: &Scratch) -> PathBuf {
    let kms = key_store_path(&store_path(scratch));
    std::fs::write(&kms, br#"{"version":1,"slots":[]}"#)
        .expect("write a non-SQLite key-store file");
    kms
}

/// The file names the backup directory holds, so a test can look for an archive
/// and for a leftover staged member. A directory the refused run never created
/// reads as empty.
fn backup_dir_entries(scratch: &Scratch) -> Vec<String> {
    std::fs::read_dir(scratch.join("backups"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The published archives among `entries`.
fn archives(entries: &[String]) -> Vec<String> {
    entries
        .iter()
        .filter(|name| name.ends_with(clio_backup::SNAPSHOT_EXTENSION))
        .cloned()
        .collect()
}

/// The staged members among `entries`: a `.tmp` file, matched through [`Path`]
/// so the check is the filesystem's notion of an extension and not a suffix
/// guess.
fn staged_members(entries: &[String]) -> Vec<String> {
    entries
        .iter()
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp"))
        })
        .cloned()
        .collect()
}

/// Root ignores mode bits, so the unreadable-file case only runs for a normal
/// user. `/proc/self/status` exists on Linux; where it is absent the case runs
/// anyway.
#[cfg(unix)]
fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:"))
                .map(|uids| uids.split_whitespace().next() == Some("0"))
        })
        .unwrap_or(false)
}

/// A key store this build cannot read must fail the run. A keyless archive that
/// looks complete is the failure this case exists to catch: the manifest would
/// say the store keeps its keys in process memory while the key store sits on
/// disk unread.
#[test]
fn a_non_sqlite_key_store_fails_the_run_and_writes_no_archive() {
    let scratch = Scratch::new("create-foreign-kms");
    populated_store(&store_path(&scratch), 2, 1);
    let kms = plant_foreign_key_store(&scratch);

    let (code, out, err) = run_in(&scratch, &["create"], true);

    assert_eq!(
        code,
        ExitCode::from(EXIT_OPERATIONAL),
        "a damaged key store is an operational failure: {err}"
    );
    assert!(out.is_empty(), "no report was emitted: {out}");
    // The refusal is the store half's own message: it names the key store, the
    // path, and what the operator should do about it.
    assert!(err.contains("is not a SQLite key-store database"), "{err}");
    assert!(err.contains(&kms.display().to_string()), "{err}");
    assert!(
        err.contains("restore the key store"),
        "the refusal says how to fix it: {err}"
    );

    let entries = backup_dir_entries(&scratch);
    let archives = archives(&entries);
    assert!(archives.is_empty(), "no archive was written: {entries:?}");
    let staged = staged_members(&entries);
    assert!(
        staged.is_empty(),
        "the staged payload removed itself: {entries:?}"
    );
    // The refusal is read-only: the planted key store is left exactly as found.
    assert_eq!(
        std::fs::read(&kms).unwrap(),
        br#"{"version":1,"slots":[]}"#.to_vec(),
        "a snapshot never rewrites the key store"
    );
}

/// A key store the store half cannot read through at all: the header read
/// fails. Root ignores mode bits, so the case is skipped for root.
#[cfg(unix)]
#[test]
fn an_unreadable_key_store_fails_the_run_and_writes_no_archive() {
    let scratch = Scratch::new("create-unreadable-kms");
    populated_store(&store_path(&scratch), 2, 1);
    let kms = plant_foreign_key_store(&scratch);
    if running_as_root() {
        // Root reads a mode-000 file, so there is nothing to refuse here.
        return;
    }
    std::fs::set_permissions(&kms, std::fs::Permissions::from_mode(0o000)).expect("chmod 000");

    let (code, out, err) = run_in(&scratch, &["create"], true);

    assert_eq!(
        code,
        ExitCode::from(EXIT_OPERATIONAL),
        "an unreadable key store is an operational failure: {err}"
    );
    assert!(out.is_empty(), "no report was emitted: {out}");
    assert!(
        err.contains("key store header"),
        "the refusal names the step that failed: {err}"
    );
    let entries = backup_dir_entries(&scratch);
    let staged = staged_members(&entries);
    assert!(
        staged.is_empty(),
        "no staged member was left behind: {entries:?}"
    );
    let archives = archives(&entries);
    assert!(archives.is_empty(), "no archive was written: {entries:?}");
    std::fs::set_permissions(&kms, std::fs::Permissions::from_mode(0o600)).expect("restore");
}

/// A target with no key-store file is the honest keyless case, and it must keep
/// working: the archive is written, the report says the keys are not included,
/// and the manifest says why. This is the boundary the refusal above must not
/// cross — refusing every snapshot would be worse than the bug.
#[test]
fn a_store_with_no_key_store_file_still_snapshots_as_keyless() {
    let scratch = Scratch::new("create-keyless");
    let store = store_path(&scratch);
    populated_store(&store, 2, 1);
    // A store whose keys were never persisted, or whose key store was removed:
    // the file the resolver points at does not exist, which is what "keeps its
    // keys in process memory" looks like on disk.
    let kms = key_store_path(&store);
    std::fs::remove_file(&kms).expect("remove the key store");

    let (code, out, err) = run_in(&scratch, &["create"], true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("keys not included"), "{out}");
    assert!(
        !out.contains("key material"),
        "an archive with no key member has nothing to warn about: {out}"
    );

    let (code, payload, err) = run_json_in(&scratch, &["create"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["keys"]["included"], json!(false), "{payload}");
    assert_eq!(payload["keys"]["subjects"], json!(0), "{payload}");
    assert!(
        payload["keys"]["warning"].is_null(),
        "no key material, no warning: {payload}"
    );

    let archive = Path::new(payload["path"].as_str().expect("a path"));
    let manifest = read_manifest(archive).expect("read the manifest");
    assert!(!manifest.keys.included, "the manifest agrees: {payload}");
    assert!(manifest.keys.member.is_none());
    assert_eq!(manifest.keys.subjects, 0);
    assert!(
        !manifest.keys.note.is_empty(),
        "an absent key member says why, so it is never silently true"
    );
}
