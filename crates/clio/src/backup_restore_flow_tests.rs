// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the restore sequence's decisions and refusals: the schema
//! gate, the key rules, the pre-restore decision, and the counts.
//!
//! ## Owns
//! - The unit assertions for the flow's decisions and refusals
//!
//! ## Does not own
//! - The flow implementation (see `backup_restore_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{
    Scratch, archive_with_manifest, archive_with_wrong_digest, populated_store, run_in_with_input,
    run_json_in, snapshot,
};
use crate::backup_restore_schema::{ACTION_MIGRATIONS, ACTION_NONE, ACTION_UNKNOWN};
use crate::backup_restore_sqlite::target_dir;
use clio_store::{SqliteFileState, SwapReport, sqlite_content_digest};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
                .filter(|name| name.ends_with(clio_backup::SNAPSHOT_EXTENSION))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}
/// One measured store file with the given existence and table counts.
fn state(exists: bool, counts: &[(&str, u64)]) -> SqliteFileState {
    SqliteFileState {
        path: PathBuf::from("/data/clio.db"),
        exists,
        bytes: 0,
        table_counts: counts
            .iter()
            .map(|(name, count)| ((*name).to_owned(), *count))
            .collect(),
        banks: Vec::new(),
    }
}
#[test]
fn the_schema_gate_refuses_newer_and_allows_equal_older_and_unknown() {
    let equal = compare_schema(Some("1"), "1").expect("equal is allowed");
    assert_eq!(equal.action, ACTION_NONE);
    assert_eq!(equal.archive.as_deref(), Some("1"));

    let older = compare_schema(Some("0"), "1").expect("older is allowed");
    assert_eq!(older.action, ACTION_MIGRATIONS);
    assert_eq!(older.archive.as_deref(), Some("0"));

    let missing = compare_schema(None, "1").expect("missing is unknown, not fatal");
    assert_eq!(missing.action, ACTION_UNKNOWN);
    assert_eq!(missing.archive, None);

    let unparseable = compare_schema(Some("twelve"), "1").expect("unparseable is unknown");
    assert_eq!(unparseable.action, ACTION_UNKNOWN);
    assert_eq!(unparseable.archive.as_deref(), Some("twelve"));

    let error = compare_schema(Some("99"), "1").expect_err("newer is refused");
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(error.message().contains("99"), "{}", error.message());
    assert!(error.message().contains("1"), "{}", error.message());
}
/// The prompt names the snapshot and the target.
#[test]
fn the_prompt_names_the_snapshot_and_the_target() {
    let prompt = crate::backup_restore_cli::prompt_text(
        "snap-1",
        "/data/clio.db",
        &PreRestore {
            reason: None,
            id: None,
            path: None,
        },
        false,
    );
    assert!(prompt.contains("snap-1"), "{prompt}");
    assert!(prompt.contains("/data/clio.db"), "{prompt}");
    assert!(prompt.contains("replaced, not merged"), "{prompt}");
    assert!(prompt.contains("is the way back"), "{prompt}");
}

#[test]
fn the_pre_restore_decision_skips_when_told_or_when_there_is_nothing_to_lose() {
    let missing = state(false, &[]);
    assert_eq!(
        pre_restore_decision(true, missing.exists, missing.is_empty()).reason,
        Some(SKIP_NO_PRE_BACKUP)
    );
    assert_eq!(
        pre_restore_decision(false, missing.exists, missing.is_empty()).reason,
        Some(SKIP_TARGET_MISSING)
    );

    let empty = state(true, &[("items", 0), ("triples", 0)]);
    assert_eq!(
        pre_restore_decision(false, empty.exists, empty.is_empty()).reason,
        Some(SKIP_TARGET_EMPTY)
    );

    // No memories, but real state in a table the two headline counts miss. A
    // restore replaces the whole file, so this must not skip the snapshot.
    let no_memories = state(true, &[("items", 0), ("triples", 0), ("sync_cursors", 3)]);
    assert_eq!(
        pre_restore_decision(false, no_memories.exists, no_memories.is_empty()).reason,
        None,
        "rows outside items and triples are still something to preserve"
    );

    let populated = state(true, &[("items", 3), ("triples", 0)]);
    let decision = pre_restore_decision(false, populated.exists, populated.is_empty());
    assert_eq!(decision.reason, None);
    assert_eq!(decision.id, None);
    assert_eq!(decision.path, None);
}

#[test]
fn counts_come_from_either_measurement() {
    let measured = state(true, &[("items", 4), ("triples", 2), ("settings", 1)]);
    assert_eq!(
        Counts::of_state(&measured),
        Counts {
            items: 4,
            triples: 2,
            tables: 3,
            banks: Vec::new()
        }
    );
    let swapped = SwapReport {
        bytes: 10,
        sidecars_removed: Vec::new(),
        directory_synced: true,
        key_replaced: false,
        failed_step: None,
        failure: None,
        table_counts: vec![("items".to_owned(), 4), ("triples".to_owned(), 1)],
        banks: vec!["demo".to_owned()],
    };
    assert_eq!(
        Counts::of_swap(&swapped),
        Counts {
            items: 4,
            triples: 1,
            tables: 2,
            banks: vec!["demo".to_owned()]
        }
    );
}

#[test]
fn a_bare_target_stages_in_the_current_directory() {
    assert_eq!(target_dir(Path::new("clio.db")), PathBuf::from("."));
    assert_eq!(
        target_dir(Path::new("/data/clio.db")),
        PathBuf::from("/data")
    );
}

#[test]
fn no_pre_backup_skips_the_snapshot_and_says_so() {
    let scratch = Scratch::new("restore-nopre");
    let id = snapshot(&scratch, 3);

    let (code, out, err) = run_in_with_input(
        &scratch,
        &["restore", &id, "--yes", "--no-pre-backup"],
        true,
        b"",
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("pre-restore snapshot: skipped"), "{out}");
    assert!(out.contains("--no-pre-backup"), "{out}");

    let (code, payload, err) = run_json_in(&scratch, &["restore", &id, "--yes", "--no-pre-backup"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        payload["pre_restore_snapshot"]["skipped"],
        json!(true),
        "{payload}"
    );
    assert_eq!(archives(&scratch).len(), 1);
}

#[test]
fn an_unknown_id_suggests_the_closest_one_and_changes_nothing() {
    let scratch = Scratch::new("restore-unknown");
    let id = snapshot(&scratch, 3);
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &format!("{id}x"), "--yes"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains(&format!("did you mean `{id}`")), "{err}");
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);
}

#[test]
fn latest_in_an_empty_directory_says_there_is_nothing_to_restore() {
    let scratch = Scratch::new("restore-latest-empty");

    let (code, _, err) = run_json_in(&scratch, &["restore", "--latest", "--yes"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("nothing to restore"), "{err}");
}

#[test]
fn a_memory_target_is_refused() {
    let scratch = Scratch::new("restore-memory");
    let id = snapshot(&scratch, 2);
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_json_in(
        &scratch,
        &["restore", &id, "--yes", "--db", "sqlite::memory:"],
    );

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("store file"), "{err}");
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);
}

#[test]
fn a_backend_mismatch_is_refused_naming_both_backends() {
    let scratch = Scratch::new("restore-backend");
    populated_store(&store_path(&scratch), 2, 1);
    let (id, _) = archive_with_manifest(
        &scratch,
        &store_path(&scratch),
        "postgres",
        Some("1"),
        false,
    );
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes", "--allow-missing-keys"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("postgres"), "{err}");
    assert!(err.contains("sqlite"), "{err}");
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);
}

#[test]
fn the_schema_gate_refuses_newer_and_allows_older() {
    let scratch = Scratch::new("restore-schema");
    populated_store(&store_path(&scratch), 2, 1);
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (newer, _) =
        archive_with_manifest(&scratch, &store_path(&scratch), "sqlite", Some("99"), false);
    let (code, _, err) = run_json_in(
        &scratch,
        &["restore", &newer, "--yes", "--allow-missing-keys"],
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("99"), "{err}");
    assert!(err.contains("1"), "{err}");
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before
    );

    let (older, _) =
        archive_with_manifest(&scratch, &store_path(&scratch), "sqlite", Some("0"), false);
    let (code, payload, err) = run_json_in(
        &scratch,
        &["restore", &older, "--yes", "--allow-missing-keys"],
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["schema"]["archive"], json!("0"), "{payload}");
    assert_eq!(
        payload["schema"]["action"],
        json!("migrations will run on first open"),
        "{payload}"
    );
}

#[test]
fn a_keyless_archive_needs_allow_missing_keys() {
    let scratch = Scratch::new("restore-keyless");
    populated_store(&store_path(&scratch), 2, 1);
    let (id, _) =
        archive_with_manifest(&scratch, &store_path(&scratch), "sqlite", Some("1"), false);
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes"]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("permanently unreadable"), "{err}");
    assert!(err.contains("--allow-missing-keys"), "{err}");
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);

    let (code, payload, err) =
        run_json_in(&scratch, &["restore", &id, "--yes", "--allow-missing-keys"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["result"], json!("restored"), "{payload}");
}

#[test]
fn a_corrupt_payload_is_refused_by_member_name() {
    let scratch = Scratch::new("restore-corrupt");
    populated_store(&store_path(&scratch), 2, 1);
    let (id, _) = archive_with_wrong_digest(&scratch, &store_path(&scratch));
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes", "--allow-missing-keys"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("store.db"), "{err}");
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);
}

#[test]
fn an_unknown_id_without_a_close_match_points_at_the_listing() {
    let scratch = Scratch::new("restore-unknown-far");
    let _ = snapshot(&scratch, 1);
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", "zzzz", "--yes"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("clio backup list"), "{err}");
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest the target"),
        before
    );
}

#[test]
fn a_truncated_archive_is_refused_with_the_manifest_message() {
    let scratch = Scratch::new("restore-truncated");
    let id = snapshot(&scratch, 2);
    let path = scratch
        .join("backups")
        .join(clio_backup::archive_file_name(&id));
    let bytes = std::fs::read(&path).expect("read the archive");
    // Only the container's head survives, so reading the manifest member fails.
    std::fs::write(&path, &bytes[..64]).expect("truncate the archive");
    let before = sqlite_content_digest(&store_path(&scratch)).expect("digest the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes"]);
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(
        err.contains("truncated") || err.contains("corrupt"),
        "{err}"
    );
    assert_eq!(
        sqlite_content_digest(&store_path(&scratch)).expect("digest"),
        before
    );
    assert_eq!(archives(&scratch).len(), 1);
}

#[test]
fn a_target_that_cannot_be_measured_is_refused_with_exit_two() {
    let scratch = Scratch::new("restore-unmeasurable");
    let _ = snapshot(&scratch, 3);
    let store = store_path(&scratch);
    std::fs::write(&store, b"not a sqlite database").expect("replace the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", "--latest", "--yes"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("not a database"), "{err}");
    assert!(err.contains("--db"), "{err}");
    assert_eq!(
        std::fs::read(&store).expect("read the target"),
        b"not a sqlite database"
    );
}

#[test]
fn a_payload_that_is_not_a_database_is_refused_with_exit_two() {
    let scratch = Scratch::new("restore-payload");
    populated_store(&store_path(&scratch), 2, 1);
    let junk = scratch.join("payload.bin");
    std::fs::write(&junk, b"not a sqlite database").expect("write a junk payload");
    let (id, _) = archive_with_manifest(&scratch, &junk, "sqlite", Some("1"), false);
    let before = std::fs::read(store_path(&scratch)).expect("read the target");

    let (code, _, err) = run_json_in(&scratch, &["restore", &id, "--yes", "--allow-missing-keys"]);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("damaged"), "{err}");
    assert_eq!(
        std::fs::read(store_path(&scratch)).expect("read the target"),
        before
    );
}
