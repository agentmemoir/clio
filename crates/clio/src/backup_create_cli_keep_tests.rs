// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for `create --keep N`: that the create still
//! succeeds, that the same prune runs afterwards, and that the confirmation rule
//! is the prune's own.
//!
//! ## Owns
//! - The `create --keep` behavior and refusal assertions
//! - That the create's own report is unchanged beside the `prune` key
//! - The exit code a create-with-keep earns, per outcome
//! - That the create path and the `prune` command reach the same verdict
//!
//! ## Does not own
//! - The plain `create` cases (see `backup_create_cli_tests`)
//! - The prune command's own behavior (see `backup_prune_cli_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::backup_cli_fixtures::{Scratch, populated_store, run_in, run_json_in};
use crate::backup_cli_fixtures_handmade::hand_built_archive;
use std::path::{Path, PathBuf};

/// The backup directory the runner resolves to.
fn backups(scratch: &Scratch) -> PathBuf {
    scratch.join("backups")
}

/// `count` archives of the scratch's own `clio.db` in the backup directory, one
/// manifest day apart, oldest first, returning their ids in that order.
///
/// The stamp is written by the fixture rather than read from the clock, because
/// two creates can land in the same second and a tie is broken by the id, whose
/// collision suffix does not follow the creation order. The stamps are in the
/// past on purpose: a real `create` stamps its own archive from the clock, so a
/// fixed past stamp is always older and "the newest is the one the create just
/// wrote" is a fact about the run, not about the day the suite runs.
fn timed_archives(scratch: &Scratch, count: usize) -> Vec<String> {
    let store = scratch.join("clio.db");
    populated_store(&store, 2, 1);
    (0..count)
        .map(|index| {
            let id = format!("clio-2020-01-{:02}T000000Z-keep", index + 1);
            let stamp = format!("2020-01-{:02}T00:00:00Z", index + 1);
            hand_built_archive(scratch, &store, &id, false, |m| m.created_at = stamp);
            id
        })
        .collect()
}

/// The archive ids still on disk in the backup directory, sorted.
fn present(scratch: &Scratch) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(backups(scratch))
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| clio_backup::snapshot_id_of(&entry.path()))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids
}

/// `ids` sorted, so an assertion on a set of survivors does not depend on the
/// order the report or the filesystem happened to use.
fn sorted(ids: &[String]) -> Vec<String> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids
}

/// The ids in one of a prune report's lists, in report order.
fn ids_on(prune: &Value, list: &str) -> Vec<String> {
    prune[list]
        .as_array()
        .unwrap_or_else(|| panic!("`{list}` is a list in the report: {prune}"))
        .iter()
        .map(|entry| entry["id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// `ids` oldest-first, which is the reverse of the order every report list uses.
fn report_order(ids: &[String]) -> Vec<String> {
    ids.iter().rev().cloned().collect()
}

/// The summed `reclaimed_bytes` of one report list, read back off the report.
fn summed(prune: &Value, list: &str) -> u64 {
    prune[list]
        .as_array()
        .unwrap_or_else(|| panic!("`{list}` is a list in the report: {prune}"))
        .iter()
        .map(|entry| entry["reclaimed_bytes"].as_u64().unwrap_or_default())
        .sum()
}

/// The verdict of a prune report as one list of facts: who was kept, who became a
/// candidate, and what happened to every one of them.
///
/// Paths and byte figures are left out: two runs face two directories, and an
/// archive a fixture writes twice is written twice.
fn verdict(prune: &Value) -> Vec<String> {
    let head = ["ok", "keep", "dry_run", "confirmed"];
    let lists = ["kept", "candidates", "deleted", "failed", "skipped"];
    let mut lines: Vec<String> = head.iter().map(|k| format!("{k} {}", prune[*k])).collect();
    for list in lists {
        for entry in prune[list]
            .as_array()
            .unwrap_or_else(|| panic!("{list}: {prune}"))
        {
            // A kept entry names the rule that kept it; a candidate names its state.
            let state = entry["state"].as_str().or(entry["reason"].as_str());
            lines.push(format!("{list} {} {}", entry["id"], state.unwrap_or("?")));
        }
    }
    lines
}

#[test]
fn a_create_without_a_keep_count_reports_no_prune_and_keeps_every_archive() {
    let scratch = Scratch::new("create-keep-none");
    let mut expected = timed_archives(&scratch, 2);

    let (code, payload, err) = run_json_in(&scratch, &["create"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["ok"], json!(true), "{payload}");
    // No `prune` key at all: a script reading the create's report sees the shape
    // it saw before the flag existed.
    assert!(payload.get("prune").is_none(), "no prune report: {payload}");
    expected.push(payload["snapshot_id"].as_str().expect("an id").to_owned());
    assert_eq!(
        present(&scratch),
        sorted(&expected),
        "nothing was deleted: {payload}"
    );
}

#[test]
fn a_keep_count_without_a_confirmation_reports_the_prune_and_removes_nothing() {
    let scratch = Scratch::new("create-keep-report");
    let ids = timed_archives(&scratch, 2);
    let before = present(&scratch);

    let (code, payload, err) = run_json_in(&scratch, &["create", "--keep", "2"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let id = payload["snapshot_id"].as_str().expect("an id").to_owned();
    // The create's own report is beside the prune's, not replaced by it.
    assert_eq!(payload["ok"], json!(true), "{payload}");
    assert_eq!(payload["backend"], json!("sqlite"), "{payload}");
    assert!(payload["counts"]["items"].as_u64().is_some(), "{payload}");
    let prune = &payload["prune"];
    assert_eq!(prune["dry_run"], json!(true), "{prune}");
    assert_eq!(prune["keep"], json!(2), "{prune}");
    assert_eq!(prune["confirmed"], json!(false), "{prune}");
    // Three archives are in the directory by the time the prune looks, so the
    // oldest is the only one outside the keep window of two.
    let kept = ids_on(prune, "kept");
    assert_eq!(kept, vec![id.clone(), ids[1].clone()], "{prune}");
    assert_eq!(
        ids_on(prune, "candidates"),
        report_order(&ids[..1]),
        "{prune}"
    );
    assert_eq!(
        prune["deleted"].as_array().map(Vec::len),
        Some(0),
        "{prune}"
    );
    assert_eq!(prune["reclaimed_bytes"], json!(0), "{prune}");
    // The create wrote exactly one archive and the prune removed none of them.
    let after = present(&scratch);
    assert_eq!(after.len(), before.len() + 1, "{payload}");
    assert!(after.contains(&id), "{payload}");

    // A second scratch for the text view, so the count above stays exact.
    let text = Scratch::new("create-keep-report-text");
    timed_archives(&text, 2);
    let (code, out, err) = run_in(&text, &["create", "--keep", "2"], true);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    for claim in [
        "snapshot ",
        "prune ",
        "--keep 2",
        "report only; nothing was removed",
    ] {
        assert!(out.contains(claim), "the text must show `{claim}`: {out}");
    }
}

#[test]
fn a_confirmed_keep_count_removes_the_older_archives_and_leaves_the_directory() {
    let scratch = Scratch::new("create-keep-confirm");
    // Three archives are already there, so the create's fourth is the newest and
    // exactly the two oldest fall outside the keep window of two.
    let ids = timed_archives(&scratch, 3);

    let (code, payload, err) = run_json_in(&scratch, &["create", "--keep", "2", "--confirm"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let id = payload["snapshot_id"].as_str().expect("an id").to_owned();
    let prune = &payload["prune"];
    assert_eq!(prune["confirmed"], json!(true), "{prune}");
    let kept = ids_on(prune, "kept");
    assert_eq!(kept, vec![id.clone(), ids[2].clone()], "{prune}");
    let deleted = ids_on(prune, "deleted");
    assert_eq!(deleted, report_order(&ids[..2]), "{prune}");
    // The set left on disk is the set the report kept: a deleted entry is gone.
    assert_eq!(
        present(&scratch),
        sorted(&[id, ids[2].clone()]),
        "{payload}"
    );
    assert!(backups(&scratch).is_dir(), "the directory stays: {payload}");
    // The reclaimed total is what was really unlinked, and nothing else.
    let reclaimed = summed(prune, "deleted");
    assert!(reclaimed > 0, "real bytes were reclaimed: {prune}");
    assert_eq!(prune["reclaimed_bytes"], json!(reclaimed), "{prune}");
}

#[test]
fn a_confirmed_keep_count_in_an_empty_directory_keeps_the_snapshot_it_wrote() {
    let scratch = Scratch::new("create-keep-empty");
    populated_store(&scratch.join("clio.db"), 2, 1);

    let (code, payload, err) = run_json_in(&scratch, &["create", "--keep", "2", "--confirm"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let id = payload["snapshot_id"].as_str().expect("an id").to_owned();
    let prune = &payload["prune"];
    assert_eq!(ids_on(prune, "kept"), vec![id.clone()], "{prune}");
    assert_eq!(ids_on(prune, "candidates"), Vec::<String>::new(), "{prune}");
    assert_eq!(
        prune["deleted"].as_array().map(Vec::len),
        Some(0),
        "{prune}"
    );
    assert_eq!(prune["reclaimed_bytes"], json!(0), "{prune}");
    // The archive is on disk, so the one on disk is the one the report kept.
    assert_eq!(present(&scratch), vec![id], "one archive: {payload}");
    assert!(
        backups(&scratch).is_dir(),
        "the directory is there: {payload}"
    );
}

#[test]
fn the_archive_the_create_just_wrote_is_never_a_candidate() {
    let scratch = Scratch::new("create-keep-newest");
    let ids = timed_archives(&scratch, 2);

    let (code, payload, err) = run_json_in(&scratch, &["create", "--keep", "1", "--confirm"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let id = payload["snapshot_id"].as_str().expect("an id").to_owned();
    let prune = &payload["prune"];
    assert_eq!(ids_on(prune, "kept"), vec![id.clone()], "{prune}");
    let deleted = ids_on(prune, "deleted");
    assert_eq!(deleted, report_order(&ids), "{prune}");
    assert!(!deleted.contains(&id), "never a candidate: {prune}");
    // The one that survives is the one the create just wrote, so it is on disk.
    assert_eq!(
        present(&scratch),
        vec![id],
        "the newest survives: {payload}"
    );
}

#[test]
fn a_keep_count_of_zero_is_refused_before_the_snapshot_is_taken() {
    let scratch = Scratch::new("create-keep-zero");
    timed_archives(&scratch, 2);
    let before = present(&scratch);

    let (code, out, err) = run_in(&scratch, &["create", "--keep", "0"], true);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("newest snapshot"), "{err}");
    assert!(out.is_empty(), "a refusal reports nothing: {out}");
    // The ordering is the point: the count is refused before the snapshot, so a
    // refused run leaves the directory exactly as it found it.
    assert_eq!(present(&scratch), before, "no archive was written: {err}");
}

#[test]
fn a_keep_count_that_is_not_a_count_is_refused_before_the_snapshot() {
    let scratch = Scratch::new("create-keep-bad");
    timed_archives(&scratch, 2);
    let before = present(&scratch);

    for bad in ["abc", "-1"] {
        let (code, out, err) = run_in(&scratch, &["create", "--keep", bad], true);

        assert_eq!(code, ExitCode::from(2), "`--keep {bad}` is refused: {err}");
        assert!(!err.trim().is_empty(), "`--keep {bad}` must say why");
        assert!(out.is_empty(), "a refusal reports nothing: {out}");
        assert_eq!(present(&scratch), before, "`--keep {bad}` wrote: {err}");
    }
}

#[test]
fn a_confirmation_with_nothing_to_confirm_is_refused_before_the_snapshot() {
    let scratch = Scratch::new("create-confirm-alone");
    timed_archives(&scratch, 2);
    let before = present(&scratch);

    let (code, out, err) = run_in(&scratch, &["create", "--confirm"], true);

    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--keep"), "{err}");
    assert!(out.is_empty(), "a refusal reports nothing: {out}");
    assert_eq!(present(&scratch), before, "no archive was written: {err}");
}

#[test]
fn a_dry_run_reports_the_keep_count_and_writes_nothing() {
    let scratch = Scratch::new("create-dry-keep");
    timed_archives(&scratch, 2);
    let before = present(&scratch);

    let (code, payload, err) = run_json_in(&scratch, &["create", "--dry-run", "--keep", "2"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    // The count itself, not the wording of the text note: a script reads this.
    assert_eq!(payload["keep"], json!(2), "{payload}");
    // The create's own plan is unchanged apart from that one field.
    assert_eq!(payload["existing_snapshots"], json!(2), "{payload}");
    for field in [
        "ok",
        "backend",
        "path",
        "store_bytes",
        "estimated_bytes_needed",
    ] {
        assert!(
            payload.get(field).is_some(),
            "`{field}` is present: {payload}"
        );
    }
    assert!(
        payload["store_bytes"].as_u64().unwrap_or(0) > 0,
        "{payload}"
    );
    // A dry run runs no prune, so it has no `prune` report to show.
    assert!(payload.get("prune").is_none(), "{payload}");
    assert_eq!(present(&scratch), before, "nothing written: {payload}");

    let (code, payload, err) = run_json_in(&scratch, &["create", "--dry-run"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    // With no count there is none to report, so the field is null rather than a
    // zero that would read as "keep nothing".
    assert_eq!(payload["keep"], Value::Null, "{payload}");
}

#[test]
fn a_corrupt_archive_beside_the_snapshots_is_reported_and_never_deleted() {
    let scratch = Scratch::new("create-keep-unreadable");
    let ids = timed_archives(&scratch, 2);
    // The corrupt file carries an older stamp than every real archive, so age is
    // the only thing that would ever make it a candidate.
    let broken = "clio-2019-12-31T000000Z-broken";
    let broken_path = backups(&scratch).join(clio_backup::archive_file_name(broken));
    std::fs::write(&broken_path, b"not an archive at all").expect("write a corrupt archive");

    let (code, payload, err) = run_json_in(&scratch, &["create", "--keep", "2", "--confirm"]);

    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let id = payload["snapshot_id"].as_str().expect("an id").to_owned();
    let prune = &payload["prune"];
    let unreadable = prune["unreadable"].as_array().expect("unreadable");
    assert_eq!(unreadable.len(), 1, "{prune}");
    assert_eq!(unreadable[0]["id"], json!(broken), "{prune}");
    let reason = unreadable[0]["reason"].as_str().unwrap_or_default();
    assert!(!reason.is_empty(), "the report gives the reason: {prune}");
    for list in ["candidates", "deleted"] {
        let on = ids_on(prune, list);
        assert!(!on.contains(&broken.to_owned()), "not in `{list}`: {prune}");
    }
    // The readable archives around it are still kept and pruned normally.
    let kept = ids_on(prune, "kept");
    assert_eq!(kept, vec![id.clone(), ids[1].clone()], "{prune}");
    let deleted = ids_on(prune, "deleted");
    assert_eq!(deleted, report_order(&ids[..1]), "{prune}");
    let left = present(&scratch);
    assert_eq!(
        left,
        sorted(&[broken.to_owned(), id, ids[1].clone()]),
        "{prune}"
    );
    assert!(broken_path.exists(), "the corrupt file is still there");
}

#[test]
fn the_create_path_and_the_prune_command_reach_the_same_verdict() {
    let via_create = Scratch::new("create-keep-verdict");
    let older = timed_archives(&via_create, 2);

    let (create_code, create_payload, err) =
        run_json_in(&via_create, &["create", "--keep", "2", "--confirm"]);

    assert_eq!(create_code, ExitCode::SUCCESS, "{err}");
    let new_id = create_payload["snapshot_id"].as_str().expect("an id");
    let new_archive = Path::new(create_payload["path"].as_str().expect("a path"));
    assert!(new_archive.exists(), "the kept archive is on disk: {err}");
    // The same set of archives, reached without a create: the two older ones plus
    // a byte-for-byte copy of the archive the create wrote, so the second run's
    // listing is the first run's listing, manifest stamps included.
    let via_prune = Scratch::new("prune-keep-verdict");
    let same = timed_archives(&via_prune, 2);
    assert_eq!(same, older, "both directories start alike");
    let copied = backups(&via_prune).join(clio_backup::archive_file_name(new_id));
    std::fs::copy(new_archive, &copied).expect("copy the created archive across");

    let (prune_code, prune_payload, err) =
        run_json_in(&via_prune, &["prune", "--keep", "2", "--confirm"]);

    assert_eq!(prune_code, ExitCode::SUCCESS, "{err}");
    assert_eq!(prune_code, create_code, "one verdict, one exit code");
    let from_create = &create_payload["prune"];
    let from_command = &prune_payload;
    assert_eq!(
        verdict(from_create),
        verdict(from_command),
        "one rule, one verdict"
    );
    // The reclaim arithmetic is checked inside each report rather than across the
    // two, because the fixture writes each archive twice and two writes of the
    // same archive may differ by a few compressed bytes.
    for report in [from_create, from_command] {
        assert_eq!(
            report["reclaimed_bytes"],
            json!(summed(report, "deleted")),
            "{report}"
        );
        let deleted = ids_on(report, "deleted");
        assert_eq!(deleted, report_order(&older[..1]), "{report}");
        assert!(summed(report, "deleted") > 0, "one archive went: {report}");
    }
}
