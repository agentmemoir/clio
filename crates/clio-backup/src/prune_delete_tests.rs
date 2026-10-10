// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the removal half of a prune: the
//! report-only run, the confirmed run, the byte accounting, an already-gone
//! file, a linked archive, a path outside the backup directory, and a failure
//! that does not strand the rest of the run.
//!
//! ## Owns
//! - The `assess` and `delete` behavior assertions
//! - The per-archive outcome a report can carry, and what a report never claims
//!
//! ## Does not own
//! - Which archives are selected (see `prune_tests`)
//! - The selection itself (see `prune`) and the removal rules (see `prune_delete`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::archive_name::snapshot_id_of;
use crate::create::create_archive;
use crate::fixtures::{Scratch, dir_entries, request, staged_payload};
use crate::inventory::list_snapshot_rows;
use crate::prune::select;
use std::path::Path;

/// One archive, with its manifest time rewritten so the listing order is
/// testable without waiting for the clock.
fn archive_at(scratch: &Scratch, seed: u8, created_at: &str, label: &str) -> PathBuf {
    let payload = staged_payload(scratch.path(), seed);
    let mut req = request(scratch.path(), payload);
    req.label = Some(label.to_owned());
    let report = create_archive(&req).expect("create");
    rewrite_created_at(&report.path, created_at);
    report.path
}

/// The plan a prune would really build over `dir`, keep window and all.
fn plan_over(dir: &Path, keep: usize) -> PrunePlan {
    let rows = list_snapshot_rows(dir, None).expect("list the backup directory");
    select(&rows, &crate::prune_kind::LinkedPaths::among(&rows), keep)
        .expect("a positive keep count is accepted")
}

/// One hand-built candidate, for the cases that are about one path rather than
/// about a keep window.
fn candidate(id: &str, path: &Path, listed_bytes: u64) -> ArchiveRef {
    ArchiveRef {
        id: id.to_owned(),
        path: path.to_path_buf(),
        created_at: "2026-01-01T00:00:00Z".to_owned(),
        backend: "sqlite".to_owned(),
        slug: "clio".to_owned(),
        listed_bytes,
    }
}

/// A plan whose only targets are `candidates`, plus `kept_bytes` of archives it
/// keeps, so the listing total covers files this run must not touch.
fn plan_with(candidates: Vec<ArchiveRef>, kept_bytes: u64) -> PrunePlan {
    let reclaimable_bytes = candidates.iter().map(|c| c.listed_bytes).sum();
    PrunePlan {
        keep: 1,
        kept: Vec::new(),
        candidates,
        unreadable: Vec::new(),
        reclaimable_bytes,
        listed_bytes: reclaimable_bytes + kept_bytes,
    }
}

/// The size of a file as it is on disk right now.
fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |metadata| metadata.len())
}

/// The snapshot files in `dir`, so an assertion names archives and not the
/// other files a fixture leaves beside them.
fn archives_in(dir: &Path) -> Vec<String> {
    dir_entries(dir)
        .into_iter()
        .filter(|name| name.ends_with(crate::SNAPSHOT_EXTENSION))
        .collect()
}

/// The report's candidates as `(id, state)`, in the order it lists them.
fn states_of(report: &PruneReport) -> Vec<(&str, CandidateState)> {
    report
        .candidates
        .iter()
        .map(|outcome| (outcome.id.as_str(), outcome.state))
        .collect()
}

/// The reason an outcome carries, or the empty string when it carries none.
fn detail_of(outcome: &CandidateOutcome) -> &str {
    outcome.detail.as_deref().unwrap_or_default()
}

#[test]
fn a_report_only_run_removes_nothing_and_claims_no_bytes() {
    let scratch = Scratch::new("prune-assess");
    let newest = archive_at(&scratch, 71, "2026-05-05T00:00:00Z", "newest");
    let second = archive_at(&scratch, 72, "2026-04-04T00:00:00Z", "second");
    let oldest = archive_at(&scratch, 73, "2026-03-03T00:00:00Z", "third");
    let plan = plan_over(scratch.path(), 1);

    let report = assess(scratch.path(), &plan);

    assert!(report.dry_run, "an unconfirmed run says so");
    let all_there = newest.exists() && second.exists() && oldest.exists();
    assert!(all_there, "a report-only run removes nothing");
    assert_eq!(report.candidates.len(), 2, "the two older archives");
    let all_pending = report
        .candidates
        .iter()
        .all(|outcome| outcome.state == CandidateState::Pending);
    assert!(all_pending, "{:?}", states_of(&report));
    assert_eq!(
        report.candidates[0].state.label(),
        "pending",
        "nothing gone"
    );
    assert_eq!(report.reclaimable_bytes, plan.reclaimable_bytes);
    assert_eq!(report.reclaimed_bytes, 0, "nothing was removed");
    assert_eq!(report.remaining_bytes, plan.listed_bytes);
    assert!(!report.has_failures());
}

#[test]
fn a_confirmed_run_removes_the_candidates_and_leaves_the_directory() {
    let scratch = Scratch::new("prune-delete");
    let newest = archive_at(&scratch, 74, "2026-05-05T00:00:00Z", "newest");
    let second = archive_at(&scratch, 75, "2026-04-04T00:00:00Z", "second");
    let oldest = archive_at(&scratch, 76, "2026-03-03T00:00:00Z", "third");
    let broken = scratch.join("broken.cliobak.tar.gz");
    std::fs::write(&broken, b"not an archive at all").expect("write a broken archive");

    let report = delete(scratch.path(), &plan_over(scratch.path(), 1));

    assert!(!report.dry_run, "a confirmed run is not a report-only run");
    let kept = snapshot_id_of(&newest).expect("a snapshot id");
    let second_id = snapshot_id_of(&second).expect("a snapshot id");
    let oldest_id = snapshot_id_of(&oldest).expect("a snapshot id");
    let mut expected = vec![
        "broken.cliobak.tar.gz".to_owned(),
        format!("{kept}{}", crate::SNAPSHOT_EXTENSION),
    ];
    expected.sort();
    assert_eq!(archives_in(scratch.path()), expected, "left behind");
    assert!(!second.exists() && !oldest.exists(), "both candidates went");
    assert!(scratch.path().is_dir(), "the directory itself stays");
    let removed: Vec<&str> = report.candidates.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(removed, vec![second_id.as_str(), oldest_id.as_str()]);
    assert_eq!(report.kept[0].archive.id, kept, "and the keep set names it");
    assert_eq!(report.candidates[0].state.label(), "deleted", "as one word");
    assert_eq!(
        report.unreadable.len(),
        1,
        "the broken file is still reported"
    );
    assert_eq!(report.unreadable[0].id, "broken");
}

#[test]
fn the_reclaimed_total_is_only_what_the_run_really_removed() {
    let scratch = Scratch::new("prune-bytes");
    archive_at(&scratch, 77, "2026-05-05T00:00:00Z", "newest");
    let second = archive_at(&scratch, 78, "2026-04-04T00:00:00Z", "second");
    let oldest = archive_at(&scratch, 79, "2026-03-03T00:00:00Z", "third");
    let measured = file_len(&second) + file_len(&oldest);
    let plan = plan_over(scratch.path(), 1);
    assert_eq!(
        plan.reclaimable_bytes, measured,
        "the listing measured these two"
    );

    let report = delete(scratch.path(), &plan);

    assert_eq!(report.reclaimed_bytes, measured, "the two that went");
    assert_eq!(
        report.reclaimable_bytes, measured,
        "and that is what it promised"
    );
    assert_eq!(report.remaining_bytes, plan.listed_bytes - measured);
    for outcome in &report.candidates {
        assert_eq!(outcome.reclaimed_bytes, outcome.listed_bytes, "{outcome:?}");
        assert_eq!(detail_of(outcome), "", "a plain removal has no reason");
    }
}

#[test]
fn a_candidate_another_process_removed_is_reported_as_already_gone() {
    let scratch = Scratch::new("prune-gone");
    archive_at(&scratch, 80, "2026-05-05T00:00:00Z", "newest");
    let doomed = archive_at(&scratch, 81, "2026-04-04T00:00:00Z", "second");
    let plan = plan_over(scratch.path(), 1);
    assert_eq!(plan.candidates.len(), 1, "one candidate out of two");
    std::fs::remove_file(&doomed).expect("remove the candidate first");

    let report = delete(scratch.path(), &plan);

    let outcome = &report.candidates[0];
    assert_eq!(outcome.state, CandidateState::AlreadyGone);
    assert_eq!(outcome.state.label(), "already_gone");
    assert!(!outcome.state.is_failure(), "the end state already holds");
    assert!(
        detail_of(outcome).contains("another process"),
        "{outcome:?}"
    );
    assert!(!report.has_failures(), "a gone file is not a failure");
    assert_eq!(report.reclaimed_bytes, 0, "this run freed nothing");
}

#[cfg(unix)]
#[test]
fn a_symlinked_archive_is_reported_and_its_target_is_never_removed() {
    let backup = Scratch::new("prune-link-backup");
    let elsewhere = Scratch::new("prune-link-outside");
    let archive = archive_at(&backup, 82, "2026-05-05T00:00:00Z", "newest");
    let target = elsewhere.join("kept-elsewhere.cliobak.tar.gz");
    std::fs::rename(&archive, &target).expect("move the archive out of the directory");
    let target_bytes = file_len(&target);
    let link = backup.join("linked.cliobak.tar.gz");
    std::os::unix::fs::symlink(&target, &link).expect("link to the moved archive");
    let plan = plan_with(vec![candidate("linked", &link, target_bytes)], 0);

    let report = delete(backup.path(), &plan);

    let outcome = &report.candidates[0];
    assert_eq!(outcome.state, CandidateState::Symlink);
    assert_eq!(outcome.state.label(), "symlink", "as one word");
    assert!(
        !outcome.state.is_failure(),
        "a link is skipped, not a failure"
    );
    assert!(link.symlink_metadata().is_ok(), "the link is still there");
    assert_eq!(file_len(&target), target_bytes, "the target is whole");
    assert_eq!(report.reclaimed_bytes, 0, "nothing was unlinked");
}

/// Unix-gated because it names a linked archive, and only Unix has links. The
/// other two cases are here so both entry points can be compared in one place:
/// a confirmed run reports them the same way.
#[cfg(unix)]
#[test]
fn a_report_only_run_reports_the_same_skips_a_confirmed_run_would() {
    let backup = Scratch::new("prune-assess-skips");
    let elsewhere = Scratch::new("prune-assess-outside");
    let target = elsewhere.join("elsewhere.cliobak.tar.gz");
    std::fs::write(&target, b"not yours").expect("write a file next door");
    let missing = backup.join("already-removed.cliobak.tar.gz");
    let archive = archive_at(&backup, 84, "2026-05-05T00:00:00Z", "newest");
    let link = backup.join("linked.cliobak.tar.gz");
    std::os::unix::fs::symlink(&archive, &link).expect("link to the archive");
    let bytes = file_len(&archive);
    let candidates = vec![
        candidate("elsewhere", &target, 0),
        candidate("already-removed", &missing, 0),
        candidate("linked", &link, bytes),
    ];

    let plan = plan_with(candidates, 0);

    let report = assess(backup.path(), &plan);

    let states = states_of(&report);
    assert_eq!(states[0], ("elsewhere", CandidateState::OutsideDirectory));
    assert_eq!(states[1], ("already-removed", CandidateState::AlreadyGone));
    assert_eq!(states[2], ("linked", CandidateState::Symlink));
    assert!(!report.has_failures(), "a skip is not a failure");
    assert_eq!(report.reclaimed_bytes, 0, "a report-only run frees nothing");
    assert_eq!(
        report.remaining_bytes, plan.listed_bytes,
        "every byte is still there"
    );
    assert!(target.exists(), "the file next door is untouched");
    assert!(link.symlink_metadata().is_ok(), "the link is still there");
    assert!(archive.exists(), "and so is what it pointed at");
}

#[test]
fn a_candidate_outside_the_backup_directory_is_refused_and_left_alone() {
    let backup = Scratch::new("prune-outside-backup");
    let sibling = Scratch::new("prune-outside-sibling");
    let sneaky = sibling.join("sneaky.cliobak.tar.gz");
    std::fs::write(&sneaky, b"not yours to delete").expect("write a file next door");
    let bytes = file_len(&sneaky);
    let sibling_name = sibling
        .path()
        .file_name()
        .expect("the sibling directory name")
        .to_owned();
    // The same file, reached through `..`, which no prefix check would accept
    // even though it names exactly the same bytes on disk.
    let via_parent = backup
        .join("..")
        .join(sibling_name)
        .join("sneaky.cliobak.tar.gz");
    let candidates = vec![
        candidate("sneaky", &sneaky, bytes),
        candidate("via-parent", &via_parent, bytes),
    ];

    let report = delete(backup.path(), &plan_with(candidates, 0));

    let states = states_of(&report);
    assert_eq!(states[0], ("sneaky", CandidateState::OutsideDirectory));
    assert_eq!(states[1], ("via-parent", CandidateState::OutsideDirectory));
    assert_eq!(
        report.candidates[0].state.label(),
        "outside_directory",
        "as one word"
    );
    assert!(
        !report.candidates[0].state.is_failure(),
        "refusal, not failure"
    );
    assert!(detail_of(&report.candidates[0]).contains("is not a file inside"));
    assert!(sneaky.exists(), "a file next door is never removed");
    assert_eq!(file_len(&sneaky), bytes, "and it keeps its bytes");
    assert_eq!(report.reclaimed_bytes, 0, "nothing was removed");
    assert!(!report.has_failures());
}

#[test]
fn a_failed_removal_is_reported_and_the_rest_of_the_run_continues() {
    let backup = Scratch::new("prune-fail-backup");
    let oldest = archive_at(&backup, 83, "2026-01-01T00:00:00Z", "oldest");
    let oldest_bytes = file_len(&oldest);
    // A directory where an archive should be. The file system refuses to
    // unlink it, and it needs no root and no special permission to arrange.
    let blocked = backup.join("2026-02-02-blocked.cliobak.tar.gz");
    std::fs::create_dir(&blocked).expect("a directory in the archive's place");
    let candidates = vec![
        candidate("blocked", &blocked, 0),
        candidate("oldest", &oldest, oldest_bytes),
    ];

    let report = delete(backup.path(), &plan_with(candidates, 4_096));

    let states = states_of(&report);
    assert_eq!(states[0], ("blocked", CandidateState::Failed));
    assert_eq!(report.candidates[0].state.label(), "failed");
    assert!(
        detail_of(&report.candidates[0]).contains("directory"),
        "{:?}",
        report.candidates[0].detail
    );
    assert_eq!(
        states[1],
        ("oldest", CandidateState::Deleted),
        "the run continued"
    );
    assert!(!oldest.exists(), "the removable archive went anyway");
    assert!(blocked.is_dir(), "and the directory is still there");
    assert!(report.has_failures());
    let failed: Vec<&str> = report.failed().map(|o| o.id.as_str()).collect();
    assert_eq!(failed, vec!["blocked"], "the failed list names it");
    assert_eq!(
        report.reclaimed_bytes, oldest_bytes,
        "only the real removal counts"
    );
    assert_eq!(
        report.remaining_bytes, 4_096,
        "the kept archive's bytes remain"
    );
}

/// Rebuild the archive with a different manifest `created_at`, member by member.
fn rewrite_created_at(path: &Path, created_at: &str) {
    use std::io::Write;
    let file = std::fs::File::open(path).expect("open");
    let mut reader = crate::container::ArchiveReader::new(file).expect("read");
    let mut manifest = Vec::new();
    assert!(reader.next_member().expect("a member").is_some());
    reader.read_current(&mut manifest).expect("read");
    let mut payload = Vec::new();
    assert!(reader.next_member().expect("a member").is_some());
    reader.read_current(&mut payload).expect("read");
    let text = String::from_utf8(manifest).expect("utf-8");
    let start = text.find("\"created_at\":\"").expect("created_at field") + 14;
    let mut rewritten = text[..start].to_owned();
    rewritten.push_str(created_at);
    rewritten.push_str(&text[start + 20..]);
    let members = [
        (crate::MANIFEST_MEMBER, rewritten.as_bytes()),
        ("store.db", payload.as_slice()),
    ];
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    for (name, bytes) in members {
        let header = crate::tar_header::build_header(name, bytes.len() as u64).expect("a header");
        encoder.write_all(&header).expect("write a header");
        encoder.write_all(bytes).expect("write a member");
        encoder.write_all(&pad(bytes.len())).expect("pad");
    }
    encoder.write_all(&[0u8; 1024]).expect("end the archive");
    std::fs::write(path, encoder.finish().expect("finish")).expect("write");
}

/// The NUL padding a member of `len` bytes needs, up to the 512-byte multiple
/// the next header starts on.
fn pad(len: usize) -> Vec<u8> {
    vec![0u8; (512 - len % 512) % 512]
}
