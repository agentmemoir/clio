// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the ways a prune can refuse: a keep count it
//! cannot use, an archive it could not read, a symlinked archive, a removal
//! that failed, and the report's promise never to claim a deletion that did not
//! happen.
//!
//! ## Owns
//! - The keep-count refusals: zero, a value that is not a count, a negative
//!   value, and no value at all
//! - An unreadable archive, which is reported and never deleted
//! - A symlinked archive, which is skipped with its target untouched
//! - A removal that failed, with its per-archive reason and its exit code
//! - The report's own honesty: a `deleted` entry means the file is gone, a
//!   `failed` entry means it is still there
//!
//! ## Does not own
//! - The plain report-only and confirmed runs (see `backup_prune_cli_tests`)
//! - The exact JSON shape and the exit-code contract (see
//!   `backup_prune_cli_golden_tests`)
//! - The command implementation (`backup_prune_cli`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::tests::{backups, ids_on, path_of, present, report_order, timed_archives};
use super::*;
use crate::backup_cli_fixtures::{Scratch, run_in, run_json_in};
#[cfg(unix)]
use crate::backup_cli_fixtures_handmade::running_as_root;
use serde_json::json;

#[test]
fn a_keep_count_that_is_not_a_usable_count_is_refused() {
    let scratch = Scratch::new("prune-keep-bad");
    let ids = timed_archives(&scratch, 2);

    for bad in ["0", "abc", "-1"] {
        let (code, out, err) = run_in(&scratch, &["prune", "--keep", bad], false);

        assert_eq!(
            code,
            ExitCode::from(2),
            "`--keep {bad}` must be refused: {err}"
        );
        assert!(out.is_empty(), "a refused prune reports nothing: {out}");
        assert!(!err.trim().is_empty(), "`--keep {bad}` must say why");
        assert_eq!(
            present(&scratch),
            ids,
            "a refused count removes nothing: {err}"
        );
    }
}

#[test]
fn a_prune_without_a_keep_count_is_a_usage_error() {
    let scratch = Scratch::new("prune-keep-missing");

    let (code, _, err) = run_in(&scratch, &["prune"], false);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--keep"), "{err}");
}

#[test]
fn an_unreadable_archive_is_reported_separately_and_never_deleted() {
    let scratch = Scratch::new("prune-unreadable");
    let ids = timed_archives(&scratch, 3);
    let broken = "clio-2025-12-31T000000Z-broken";
    let broken_path = backups(&scratch).join(clio_backup::archive_file_name(broken));
    std::fs::write(&broken_path, b"not an archive at all").expect("write a broken archive");

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "1", "--confirm"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let unreadable = payload["unreadable"].as_array().expect("unreadable");
    assert_eq!(unreadable.len(), 1, "{payload}");
    assert_eq!(unreadable[0]["id"], json!(broken), "{payload}");
    assert!(
        !unreadable[0]["reason"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "the report gives the reason: {payload}"
    );
    assert!(
        !ids_on(&payload, "candidates").contains(&broken.to_owned()),
        "an unreadable archive is never a candidate: {payload}"
    );
    assert!(
        !ids_on(&payload, "deleted").contains(&broken.to_owned()),
        "an unreadable archive is never deleted: {payload}"
    );
    // The readable archives around it are still processed normally.
    assert_eq!(ids_on(&payload, "kept"), vec![ids[2].clone()], "{payload}");
    assert_eq!(
        ids_on(&payload, "deleted"),
        report_order(&ids[..2]),
        "{payload}"
    );
    let left = present(&scratch);
    assert!(
        left.contains(&broken.to_owned()),
        "it is still on disk: {broken_path:?}"
    );
    assert_eq!(
        left.len(),
        2,
        "only the newest readable archive is left: {payload}"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_archive_is_skipped_and_its_target_is_untouched() {
    let scratch = Scratch::new("prune-symlink");
    let ids = timed_archives(&scratch, 2);
    let outside = scratch.join("outside.cliobak.tar.gz");
    std::fs::copy(
        backups(&scratch).join(clio_backup::archive_file_name(&ids[0])),
        &outside,
    )
    .expect("copy an archive outside the backup directory");
    // The link points at a real archive, not a broken target: a broken target
    // is unreadable, and an unreadable archive is a different case.
    let link = backups(&scratch).join(clio_backup::archive_file_name(
        "clio-2025-06-01T000000Z-link",
    ));
    std::os::unix::fs::symlink(&outside, &link).expect("link the archive in");

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "1", "--confirm"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let skipped = payload["skipped"].as_array().expect("skipped");
    assert_eq!(skipped.len(), 1, "{payload}");
    assert_eq!(skipped[0]["state"], json!("symlink"), "{payload}");
    assert!(
        !skipped[0]["reason"].as_str().unwrap_or_default().is_empty(),
        "the report gives the reason: {payload}"
    );
    assert_eq!(
        ids_on(&payload, "deleted"),
        vec![ids[0].clone()],
        "{payload}"
    );
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("lstat")
            .is_symlink(),
        "the link is still a link"
    );
    assert!(
        outside.exists(),
        "the file outside the directory is untouched"
    );
    // The human view explains the skip rather than showing a bare state word.
    let (code, out, err) = run_in(&scratch, &["prune", "--keep", "1", "--confirm"], true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("symlink"), "{out}");
    assert!(out.contains("never followed"), "{out}");
}

/// The case a confirmed run must never get wrong: a symlink that sorts ahead of
/// a store's only real archive. The link's row is a readable snapshot of the
/// same store, because the listing reads a manifest through a link, so a
/// selection that counted it took the keep slot and offered the real archive
/// for deletion. Here the real archive is all the store has, so deleting it
/// would leave nothing but a link that points at it. Unix-gated because it names
/// a link, and only Unix has them.
#[cfg(unix)]
#[test]
fn a_link_that_sorts_ahead_never_costs_a_store_its_only_archive() {
    let scratch = Scratch::new("prune-link-sorts-first");
    let ids = timed_archives(&scratch, 1);
    let real = backups(&scratch).join(clio_backup::archive_file_name(&ids[0]));
    // The link's id sorts before the archive's, so the listing puts it first.
    let link = backups(&scratch).join(clio_backup::archive_file_name("aaa"));

    std::os::unix::fs::symlink(&real, &link).expect("link the only archive in");

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "1", "--confirm"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(ids_on(&payload, "kept"), vec![ids[0].clone()], "{payload}");
    assert!(
        ids_on(&payload, "deleted").is_empty(),
        "a confirmed run must not delete the store's only archive: {payload}"
    );
    assert!(real.exists(), "the archive is on disk: {}", real.display());
    assert!(
        std::fs::symlink_metadata(&link).is_ok_and(|meta| meta.is_symlink()),
        "the link is still a link"
    );
    // The link is reported, and the report claims no bytes for it: a run never
    // unlinks a link, so the space its target occupies is not this run's to free.
    assert_eq!(
        ids_on(&payload, "skipped"),
        vec!["aaa".to_owned()],
        "{payload}"
    );
    assert_eq!(payload["reclaimable_bytes"], json!(0), "{payload}");
    assert_eq!(payload["reclaimed_bytes"], json!(0), "{payload}");
    assert_eq!(
        payload["remaining_bytes"],
        json!(payload["kept"][0]["listed_bytes"].as_u64().unwrap_or(0)),
        "the real archive is the whole of what remains: {payload}"
    );
}

#[cfg(unix)]
#[test]
fn a_candidate_the_run_cannot_remove_is_reported_as_a_failure() {
    use std::os::unix::fs::PermissionsExt as _;
    let scratch = Scratch::new("prune-unwritable");
    let ids = timed_archives(&scratch, 3);
    // Unlinking needs write permission on the parent directory, so the directory
    // is what is locked, not the file.
    std::fs::set_permissions(backups(&scratch), std::fs::Permissions::from_mode(0o500))
        .expect("chmod the backup directory");

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "1", "--confirm"]);

    // Restored before any assertion, so a panic cannot leave a locked tree.
    std::fs::set_permissions(backups(&scratch), std::fs::Permissions::from_mode(0o700))
        .expect("chmod the backup directory back");
    // Root ignores mode bits, so the unlink succeeds there and the run is a
    // complete prune. Both branches assert the same property: a `deleted` entry
    // means the file is gone, a `failed` entry means it is still there.
    if running_as_root() {
        assert_eq!(code, ExitCode::SUCCESS, "{err}");
        assert_eq!(
            payload["failed"].as_array().map(Vec::len),
            Some(0),
            "{payload}"
        );
        assert_eq!(present(&scratch), vec![ids[2].clone()], "{payload}");
    } else {
        assert_eq!(code, ExitCode::from(3), "{err}");
        assert_eq!(payload["ok"], json!(false), "{payload}");
        let failed = payload["failed"].as_array().expect("failed");
        assert!(!failed.is_empty(), "the failure is reported: {payload}");
        assert!(
            !failed[0]["reason"].as_str().unwrap_or_default().is_empty(),
            "the report gives the reason: {payload}"
        );
        assert_eq!(payload["reclaimed_bytes"], json!(0), "{payload}");
        assert_eq!(
            present(&scratch),
            ids,
            "every archive is still on disk: {payload}"
        );
    }
    for candidate in payload["candidates"].as_array().expect("candidates") {
        let on_disk = path_of(candidate).exists();
        match candidate["state"].as_str().unwrap_or_default() {
            "deleted" => assert!(!on_disk, "a deleted entry means it is gone: {candidate}"),
            "failed" => assert!(
                on_disk,
                "a failed entry means it is still there: {candidate}"
            ),
            other => panic!("unexpected candidate state `{other}`: {payload}"),
        }
    }
}

#[test]
fn the_report_never_claims_a_deletion_that_did_not_happen() {
    let scratch = Scratch::new("prune-truthful");
    let ids = timed_archives(&scratch, 4);

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "2", "--confirm"]);

    // The already-gone path needs a file to vanish between the listing and the
    // unlink, which no command line can stage, so the archive half's own unit
    // tests own that state. What this test owns is the property the command
    // exposes: `already_gone` is empty when nothing vanished and `failed` is
    // empty when nothing failed.
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        payload["already_gone"].as_array().map(Vec::len),
        Some(0),
        "{payload}"
    );
    assert_eq!(
        payload["failed"].as_array().map(Vec::len),
        Some(0),
        "{payload}"
    );
    assert_eq!(
        ids_on(&payload, "deleted"),
        report_order(&ids[..2]),
        "{payload}"
    );
    for entry in payload["deleted"].as_array().expect("deleted") {
        assert!(
            !path_of(entry).exists(),
            "a deleted entry means it is gone: {entry}"
        );
        assert!(
            entry["reclaimed_bytes"].as_u64().unwrap_or(0) > 0,
            "{entry}"
        );
    }
    assert!(
        payload["reclaimed_bytes"].as_u64().unwrap_or(0) > 0,
        "a real removal reclaimed real bytes: {payload}"
    );
}
