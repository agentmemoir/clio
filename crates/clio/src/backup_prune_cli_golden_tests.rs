// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the exact shape of a prune report: the JSON
//! field set in both output modes, the text layout, and the exit code each
//! outcome earns.
//!
//! ## Owns
//! - The golden JSON and text assertions, and the exit-code contract
//! - The exact top-level, candidate, and kept key sets, pinned as sorted lists
//! - The byte arithmetic: what is reclaimable, what was reclaimed, what remains
//! - The exit code each outcome earns, including a partial removal
//!
//! ## Does not own
//! - The behavior cases and the refusals (see `backup_prune_cli_tests`)
//! - The command implementation (`backup_prune_cli`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::tests::{backups, ids_on, present, report_order, timed_archives};
use super::*;
use crate::backup_cli_fixtures::{Scratch, run_in, run_json_in};
use crate::backup_cli_fixtures_handmade::hand_built_archive;
#[cfg(unix)]
use crate::backup_cli_fixtures_handmade::running_as_root;
use std::path::PathBuf;

/// The store the archives are built from.
fn store(scratch: &Scratch) -> PathBuf {
    scratch.join("clio.db")
}

/// One archive whose manifest time and store backend this test chooses, so a
/// mixed-store directory is a fixture and not a race against the clock.
fn archive_of_store(scratch: &Scratch, id: &str, backend: &str, created_at: &str) {
    hand_built_archive(scratch, &store(scratch), id, false, |manifest| {
        manifest.store.backend = backend.to_owned();
        manifest.created_at = created_at.to_owned();
    });
}

/// The ids sorted, so an assertion on a set of survivors does not depend on the
/// order the report happened to use.
fn sorted(ids: &[String]) -> Vec<String> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids
}

/// An object's field names, sorted, so an added or renamed field fails the test
/// rather than slipping through an `assert!(contains)` check.
fn object_keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap_or_else(|| panic!("the report is an object: {value}"))
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

#[test]
fn the_json_report_has_exactly_the_documented_fields() {
    let scratch = Scratch::new("prune-golden-keys");
    timed_archives(&scratch, 3);

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "1"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        object_keys(&payload),
        vec![
            "already_gone",
            "backup_dir",
            "candidates",
            "confirmed",
            "deleted",
            "dry_run",
            "failed",
            "keep",
            "kept",
            "ok",
            "reclaimable_bytes",
            "reclaimed_bytes",
            "remaining_bytes",
            "skipped",
            "unreadable",
        ],
        "a renamed or added field must fail this test: {payload}"
    );
}

#[test]
fn a_candidate_and_a_kept_entry_have_exactly_the_documented_fields() {
    let scratch = Scratch::new("prune-golden-entry-keys");
    timed_archives(&scratch, 3);

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "1"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let candidate = &payload["candidates"][0];
    assert_eq!(
        object_keys(candidate),
        vec![
            "created_at",
            "id",
            "listed_bytes",
            "path",
            "reclaimed_bytes",
            "state",
            "store",
        ],
        "a pending candidate has no reason to give, so `reason` must be absent: {candidate}"
    );
    let kept = &payload["kept"][0];
    assert_eq!(
        object_keys(kept),
        vec![
            "backend",
            "created_at",
            "id",
            "listed_bytes",
            "path",
            "reason",
            "slug",
        ],
        "a kept entry names the rule that kept it: {kept}"
    );
}

#[test]
fn the_byte_totals_agree_with_the_entries_they_describe() {
    let scratch = Scratch::new("prune-golden-bytes");
    timed_archives(&scratch, 5);

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "2", "--confirm"]);

    // Every figure is read back off the report's own lists, not off the
    // filesystem: `reclaimable_bytes` must be the summed `listed_bytes` of the
    // candidates, `reclaimed_bytes` the summed `reclaimed_bytes` of the entries
    // in `deleted`, and `remaining_bytes` the listing's own total minus what this
    // run really removed. That total is `reclaimable` plus the `listed_bytes` of
    // the `kept` entries, because this directory holds only readable archives.
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let sum = |list: &str, field: &str| -> u64 {
        payload[list]
            .as_array()
            .unwrap_or_else(|| panic!("`{list}` is a list: {payload}"))
            .iter()
            .map(|entry| entry[field].as_u64().unwrap_or_default())
            .sum()
    };
    let reclaimable = payload["reclaimable_bytes"].as_u64().expect("a number");
    let reclaimed = payload["reclaimed_bytes"].as_u64().expect("a number");
    assert_eq!(reclaimable, sum("candidates", "listed_bytes"), "{payload}");
    assert_eq!(reclaimed, sum("deleted", "reclaimed_bytes"), "{payload}");
    assert!(
        reclaimed > 0 && reclaimed <= reclaimable,
        "a real removal reclaimed real bytes, and never more than the listing \
         measured: {payload}"
    );
    assert_eq!(
        payload["remaining_bytes"].as_u64(),
        Some(reclaimable + sum("kept", "listed_bytes") - reclaimed),
        "{payload}"
    );
}

#[test]
fn the_text_view_has_a_header_counts_a_line_per_archive_and_a_totals_line() {
    let scratch = Scratch::new("prune-golden-text");
    let ids = timed_archives(&scratch, 3);

    let (code, out, err) = run_in(&scratch, &["prune", "--keep", "1", "--confirm"], true);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 7, "{out}");
    assert!(
        lines[0].contains(&format!(
            "prune {} --keep 1 (removed)",
            backups(&scratch).display()
        )),
        "the header names the directory, the count, and that nothing was left: {out}"
    );
    assert_eq!(lines[1].trim(), "kept (1)", "{out}");
    assert!(lines[2].contains(&ids[2]), "{out}");
    assert!(lines[2].contains("sqlite/clio"), "{out}");
    assert!(lines[2].contains("newest"), "{out}");
    assert_eq!(lines[3].trim(), "candidates (2)", "{out}");
    for (line, id) in lines[4..6].iter().zip([&ids[1], &ids[0]]) {
        assert!(line.contains(id), "{line}");
        assert!(line.contains("deleted"), "{line}");
        assert!(line.contains(" B"), "{line}");
    }
    let totals = lines[6];
    assert!(totals.trim_start().starts_with("reclaimable "), "{totals}");
    assert!(totals.contains(" · reclaimed "), "{totals}");
    assert!(totals.contains(" · remaining "), "{totals}");
}

#[test]
fn a_report_a_complete_prune_and_a_usage_refusal_have_their_own_exit_codes() {
    let scratch = Scratch::new("prune-golden-exit");
    timed_archives(&scratch, 3);

    let (code, _, err) = run_in(&scratch, &["prune", "--keep", "1"], true);
    assert_eq!(code, ExitCode::from(0), "a report is a success: {err}");

    let (code, _, err) = run_in(&scratch, &["prune", "--keep", "1", "--confirm"], true);
    assert_eq!(
        code,
        ExitCode::from(0),
        "a complete prune is a success: {err}"
    );

    let (code, _, err) = run_in(&scratch, &["prune", "--keep", "0"], true);
    assert_eq!(
        code,
        ExitCode::from(2),
        "a usage refusal is its own code: {err}"
    );

    let (code, _, err) = run_in(&scratch, &["prune"], true);
    assert_eq!(
        code,
        ExitCode::from(2),
        "a missing keep count is a refusal: {err}"
    );
}

#[cfg(unix)]
#[test]
fn a_partial_removal_has_its_own_exit_code() {
    use std::os::unix::fs::PermissionsExt as _;
    let scratch = Scratch::new("prune-golden-partial");
    let ids = timed_archives(&scratch, 3);
    // Unlinking needs write permission on the parent directory, so the directory
    // is what is locked, not the file.
    std::fs::set_permissions(backups(&scratch), std::fs::Permissions::from_mode(0o500))
        .expect("chmod the backup directory");

    let (code, _, err) = run_in(&scratch, &["prune", "--keep", "1", "--confirm"], true);

    // Restored before any assertion, so a panic cannot leave a locked tree.
    std::fs::set_permissions(backups(&scratch), std::fs::Permissions::from_mode(0o700))
        .expect("chmod the backup directory back");
    if running_as_root() {
        // Root ignores mode bits, so the unlink succeeds and there is no partial
        // run to observe on this host.
        assert_eq!(code, ExitCode::from(0), "{err}");
        assert_eq!(present(&scratch), vec![ids[2].clone()], "{err}");
    } else {
        assert_eq!(code, ExitCode::from(3), "{err}");
        assert_eq!(present(&scratch), ids, "nothing was removed: {err}");
    }
}

#[test]
fn the_directory_and_the_newest_archive_of_each_store_survive_a_prune() {
    let scratch = Scratch::new("prune-golden-one");
    let ids = timed_archives(&scratch, 3);
    // The other store's only archive is the oldest in the directory, so the keep
    // window covers the newest sqlite archive and the per-store floor covers the
    // postgres one. Each store keeps exactly one archive and two are removed.
    let pg = "clio-2025-12-31T000000Z-pg0001";
    archive_of_store(&scratch, pg, "postgres", "2025-12-31T00:00:00Z");

    let (code, payload, err) = run_json_in(&scratch, &["prune", "--keep", "1", "--confirm"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        ids_on(&payload, "kept"),
        vec![ids[2].clone(), pg.to_owned()],
        "{payload}"
    );
    assert_eq!(
        ids_on(&payload, "deleted"),
        report_order(&ids[..2]),
        "{payload}"
    );
    assert!(
        backups(&scratch).is_dir(),
        "the directory itself is never removed: {payload}"
    );
    assert_eq!(
        present(&scratch),
        sorted(&[pg.to_owned(), ids[2].clone()]),
        "{payload}"
    );
}
