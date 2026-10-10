// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the keep-window rule of prune
//! selection: the newest `keep` archives by manifest time are kept, the rest are
//! candidates, a file's mtime plays no part, and the plan's byte totals add up.
//!
//! ## Owns
//! - The `select` keep-window, ordering, and byte-total assertions
//! - The manifest-time-over-file-time ordering, over archives built for real
//!
//! ## Does not own
//! - The refusals and the per-store floor (see `prune_selection_tests`)
//! - Removing an archive (see `prune_delete_tests`)
//! - The selection itself (see `prune`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::archive_name::snapshot_id_of;
use crate::create::create_archive;
use crate::fixtures::{Scratch, request, staged_payload};
use crate::inventory::list_snapshot_rows;
use std::path::Path;

/// One hand-built listing row, so a keep-window case needs no directory at all.
/// Selection is a function of the rows alone, and building them here is what
/// proves that: the rows carry a time, a size, and a store, and nothing else
/// exists in the process.
fn row(id: &str, created_at: &str, archive_bytes: u64) -> SnapshotRow {
    SnapshotRow {
        id: id.to_owned(),
        path: PathBuf::from(format!("/backups/{id}.cliobak.tar.gz")),
        created_at: created_at.to_owned(),
        backend: "sqlite".to_owned(),
        slug: "clio".to_owned(),
        archive_bytes,
        payload_bytes: archive_bytes * 2,
        keys_included: false,
        label: Some(format!("the {id} snapshot")),
        unreadable: None,
    }
}

/// The ids the plan keeps, in the order a report would list them.
fn kept_ids(plan: &PrunePlan) -> Vec<&str> {
    plan.kept
        .iter()
        .map(|entry| entry.archive.id.as_str())
        .collect()
}

/// The ids the plan may delete, in the order a report would list them.
fn candidate_ids(plan: &PrunePlan) -> Vec<&str> {
    plan.candidates
        .iter()
        .map(|archive| archive.id.as_str())
        .collect()
}

#[test]
fn the_newest_archives_are_kept_and_the_older_ones_are_candidates() {
    let rows = vec![
        row("newest", "2026-05-05T00:00:00Z", 500),
        row("second", "2026-04-04T00:00:00Z", 400),
        row("third", "2026-03-03T00:00:00Z", 300),
        row("fourth", "2026-02-02T00:00:00Z", 200),
        row("oldest", "2026-01-01T00:00:00Z", 100),
    ];

    let plan = select(&rows, &LinkedPaths::default(), 2).expect("a keep count of two is accepted");

    assert_eq!(plan.keep, 2, "the report names the count it applied");
    assert_eq!(
        kept_ids(&plan),
        vec!["newest", "second"],
        "the keep window is the newest two, not the two largest"
    );
    assert!(
        plan.kept
            .iter()
            .all(|entry| entry.reason == KeepReason::KeepWindow),
        "both are in the window, so the window is the reason"
    );
    assert_eq!(
        candidate_ids(&plan),
        vec!["third", "fourth", "oldest"],
        "candidates are listed newest first, so a wrong order fails here"
    );
}

#[test]
fn a_keep_count_larger_than_the_directory_keeps_everything() {
    let rows = vec![
        row("newest", "2026-05-05T00:00:00Z", 500),
        row("oldest", "2026-01-01T00:00:00Z", 100),
    ];

    let plan =
        select(&rows, &LinkedPaths::default(), 9).expect("any positive keep count is accepted");

    assert_eq!(kept_ids(&plan), vec!["newest", "oldest"]);
    assert!(plan.candidates.is_empty(), "{:?}", plan.candidates);
    assert_eq!(plan.reclaimable_bytes, 0, "nothing is reclaimable");
    assert_eq!(plan.listed_bytes, 600, "the listing total is unchanged");
    // An empty directory is the same rule with nothing to apply it to.
    let empty = select(&[], &LinkedPaths::default(), 3).expect("an empty listing is not an error");
    assert!(
        empty.kept.is_empty() && empty.candidates.is_empty(),
        "an empty listing keeps nothing and offers no candidates"
    );
    assert_eq!(empty.listed_bytes, 0);
}

#[test]
fn the_reclaimable_total_counts_the_candidates_and_the_listed_total_counts_all() {
    let rows = vec![
        row("newest", "2026-05-05T00:00:00Z", 500),
        row("second", "2026-04-04T00:00:00Z", 400),
        row("oldest", "2026-01-01T00:00:00Z", 100),
    ];

    let plan = select(&rows, &LinkedPaths::default(), 2).expect("a keep count of two is accepted");

    assert_eq!(
        plan.reclaimable_bytes, 100,
        "only the candidate's bytes are reclaimable"
    );
    assert_eq!(
        plan.listed_bytes, 1_000,
        "the listing total covers the kept archives too, not just the targets"
    );
}

/// Selection never re-sorts: it reads the order the listing handed over, which
/// is newest first by manifest time, and takes the first `keep` of it. The file
/// names here run the other way, so any code that sorted by name would answer
/// differently. The manifest-time half, over real archives whose file times say
/// the opposite, is the next test.
#[test]
fn the_plan_keeps_the_first_rows_of_the_listing_without_re_sorting() {
    let rows = vec![
        row("middle", "2026-02-01T00:00:00Z", 300),
        row("newest", "2026-03-01T00:00:00Z", 100),
        row("oldest", "2026-01-01T00:00:00Z", 200),
    ];

    let plan = select(&rows, &LinkedPaths::default(), 1).expect("a keep count of one is accepted");

    assert_eq!(
        kept_ids(&plan),
        vec!["middle"],
        "the first listed row is kept"
    );
    assert_eq!(candidate_ids(&plan), vec!["newest", "oldest"]);
    assert_eq!(
        candidate_ids(&plan)[0],
        "newest",
        "the smaller file comes first because the listing says it is newer"
    );
    // The same rows the other way round give the other answer, which is what
    // "never re-sorts" means: the caller owns the order.
    let reversed: Vec<SnapshotRow> = rows.iter().rev().cloned().collect();
    let flipped =
        select(&reversed, &LinkedPaths::default(), 1).expect("the same rows, the other way round");
    assert_eq!(kept_ids(&flipped), vec!["oldest"]);
    assert_eq!(candidate_ids(&flipped), vec!["newest", "middle"]);
}

/// The one selection test that touches the filesystem, and only because a prune
/// trusts the listing rather than the file times: a real archive's mtime is
/// pushed into the past, and the listing plus `select` must still order by the
/// manifest time. Every other selection test here builds its rows by hand, which
/// is the property that makes selection testable without a directory.
#[test]
fn a_real_archive_with_an_old_file_time_still_orders_by_its_manifest_time() {
    let scratch = Scratch::new("prune-mtime");
    let old = archive_at(&scratch, 61, "2026-01-01T00:00:00Z", "old");
    let fresh = archive_at(&scratch, 62, "2026-06-01T00:00:00Z", "fresh");
    // Say the opposite with the file times: the newer snapshot looks older.
    set_mtime(&fresh, 1_000_000_000);
    set_mtime(&old, 2_000_000_000);

    let rows = list_snapshot_rows(scratch.path(), None).expect("list the backup directory");
    let plan = select(&rows, &LinkedPaths::default(), 1).expect("a keep count of one is accepted");

    assert_eq!(
        kept_ids(&plan),
        vec![snapshot_id_of(&fresh).expect("a snapshot id")],
        "the manifest time decides, not the file time"
    );
    assert_eq!(
        candidate_ids(&plan),
        vec![snapshot_id_of(&old).expect("a snapshot id")]
    );
    assert!(
        old.exists() && fresh.exists(),
        "deciding what to delete removes nothing"
    );
}

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

/// Rebuild the archive with a different manifest `created_at`.
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
    let end = start + 20;
    let mut rewritten = text[..start].to_owned();
    rewritten.push_str(created_at);
    rewritten.push_str(&text[end..]);
    let rewritten = rewritten.into_bytes();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(
            &crate::tar_header::build_header(crate::MANIFEST_MEMBER, rewritten.len() as u64)
                .expect("a manifest header"),
        )
        .expect("write");
    encoder.write_all(&rewritten).expect("write the manifest");
    encoder
        .write_all(&vec![0u8; (512 - rewritten.len() % 512) % 512])
        .expect("pad");
    encoder
        .write_all(
            &crate::tar_header::build_header("store.db", payload.len() as u64).expect("a header"),
        )
        .expect("write");
    encoder.write_all(&payload).expect("write the payload");
    encoder
        .write_all(&vec![0u8; (512 - payload.len() % 512) % 512])
        .expect("pad");
    encoder.write_all(&[0u8; 1024]).expect("end the archive");
    std::fs::write(path, encoder.finish().expect("finish")).expect("write");
}

/// Set a file's modification time to `secs` since the epoch.
fn set_mtime(path: &Path, secs: u64) {
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open to touch");
    file.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .expect("set the modification time");
}
