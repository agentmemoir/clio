// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the refusals and the store rules of prune
//! selection: `--keep 0`, a count that is not a count, an archive the listing
//! could not read, and a directory holding more than one store's snapshots.
//!
//! ## Owns
//! - The `select` refusal, unreadable-archive, and per-store floor assertions
//!
//! ## Does not own
//! - The plain keep-window cases (see `prune_tests`)
//! - Removing an archive (see `prune_delete_tests`)
//! - The selection itself (see `prune`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::fixtures::Scratch;

/// One hand-built listing row for a named store, so a multi-store case needs no
/// directory and the store a row belongs to is a choice in the test.
fn store_row(
    id: &str,
    created_at: &str,
    archive_bytes: u64,
    backend: &str,
    slug: &str,
) -> SnapshotRow {
    SnapshotRow {
        id: id.to_owned(),
        path: PathBuf::from(format!("/backups/{id}.cliobak.tar.gz")),
        created_at: created_at.to_owned(),
        backend: backend.to_owned(),
        slug: slug.to_owned(),
        archive_bytes,
        payload_bytes: archive_bytes * 2,
        keys_included: false,
        label: Some(format!("the {id} snapshot")),
        unreadable: None,
    }
}

/// One listing row the way the inventory reports an archive it could not read:
/// no time, no store, and the reason in place of its facts.
fn unreadable_row(id: &str, reason: &str, archive_bytes: u64) -> SnapshotRow {
    SnapshotRow {
        id: id.to_owned(),
        path: PathBuf::from(format!("/backups/{id}.cliobak.tar.gz")),
        created_at: String::new(),
        backend: "unreadable".to_owned(),
        slug: String::new(),
        archive_bytes,
        payload_bytes: 0,
        keys_included: false,
        label: None,
        unreadable: Some(reason.to_owned()),
    }
}

/// A keep reason as the report's one word, so a failed assertion names the rule
/// that decided rather than the variant it matched.
fn reason_label(reason: KeepReason) -> &'static str {
    match reason {
        KeepReason::KeepWindow => "keep_window",
        KeepReason::LastOfStore => "last_of_store",
    }
}

/// The kept archives as `(id, reason)`, in the order the plan lists them.
fn kept_with_reasons(plan: &PrunePlan) -> Vec<(&str, &'static str)> {
    plan.kept
        .iter()
        .map(|entry| (entry.archive.id.as_str(), reason_label(entry.reason)))
        .collect()
}

/// The candidates as `(id, backend, slug)`, so each entry must carry its own
/// store rather than the one the report happens to be grouped under.
fn candidates_by_store(plan: &PrunePlan) -> Vec<(&str, &str, &str)> {
    plan.candidates
        .iter()
        .map(|archive| {
            (
                archive.id.as_str(),
                archive.backend.as_str(),
                archive.slug.as_str(),
            )
        })
        .collect()
}

#[test]
fn a_keep_count_of_zero_is_refused_naming_the_newest_snapshot() {
    let rows = vec![
        store_row("newest", "2026-05-05T00:00:00Z", 500, "sqlite", "clio"),
        store_row("oldest", "2026-01-01T00:00:00Z", 100, "sqlite", "clio"),
    ];

    let refusal = select(&rows, &LinkedPaths::default(), 0)
        .expect_err("zero would leave the newest deletable");

    assert_eq!(refusal, PruneRefusal::KeepZero);
    let message = refusal_message(refusal);
    assert!(message.contains("--keep 0"), "{message}");
    assert!(
        message.contains("the newest snapshot is never a candidate"),
        "the message has to say why: {message}"
    );
}

#[test]
fn the_newest_snapshot_is_never_a_candidate_at_any_accepted_keep_count() {
    let rows: Vec<SnapshotRow> = [
        "2026-05-05T00:00:00Z",
        "2026-04-04T00:00:00Z",
        "2026-03-03T00:00:00Z",
    ]
    .iter()
    .enumerate()
    .map(|(index, created_at)| {
        store_row(
            &format!("snap-{index}"),
            created_at,
            100 + index as u64,
            "sqlite",
            "clio",
        )
    })
    .collect();

    for keep in 1..=rows.len() + 2 {
        let plan = select(&rows, &LinkedPaths::default(), keep).expect("an accepted keep count");
        assert!(
            plan.candidates.iter().all(|archive| archive.id != "snap-0"),
            "--keep {keep} offered the newest snapshot for deletion"
        );
        assert!(
            plan.kept.iter().any(|entry| entry.archive.id == "snap-0"),
            "--keep {keep} did not keep the newest snapshot at all"
        );
    }
}

#[test]
fn an_unreadable_archive_is_never_a_candidate_and_takes_no_keep_slot() {
    let rows = vec![
        store_row("newest", "2026-05-05T00:00:00Z", 500, "sqlite", "clio"),
        // The listing sorts these last, but it is placed in the middle here so
        // the case also shows the order the selection reads does not matter.
        unreadable_row("broken", "the manifest is not valid json", 900),
        store_row("second", "2026-04-04T00:00:00Z", 400, "sqlite", "clio"),
        store_row("third", "2026-03-03T00:00:00Z", 300, "sqlite", "clio"),
    ];

    let plan = select(&rows, &LinkedPaths::default(), 2).expect("a keep count of two is accepted");

    assert_eq!(
        kept_with_reasons(&plan),
        vec![("newest", "keep_window"), ("second", "keep_window")],
        "the window is the two newest readable archives, so the broken file costs no slot"
    );
    assert_eq!(
        candidates_by_store(&plan),
        vec![("third", "sqlite", "clio")],
        "a file the listing could not read is never a target"
    );
    assert_eq!(plan.unreadable.len(), 1, "it is reported separately");
    assert_eq!(plan.unreadable[0].id, "broken");
    assert_eq!(plan.unreadable[0].listed_bytes, 900);
    assert_eq!(
        plan.unreadable[0].reason, "the manifest is not valid json",
        "the reason the listing gave is passed through"
    );
    assert_eq!(plan.reclaimable_bytes, 300, "a broken file frees nothing");
    assert_eq!(
        plan.listed_bytes, 2_100,
        "the broken file still occupies its bytes in the listing total"
    );
}

#[test]
fn a_second_store_ages_out_on_the_merged_order_and_is_not_excluded() {
    // Two archives from each of two stores, interleaved by manifest time. A
    // prune names one backup directory, and this directory holds both stores.
    let rows = vec![
        store_row("p1", "2026-05-05T00:00:00Z", 500, "postgres", "journal"),
        store_row("s1", "2026-04-04T00:00:00Z", 400, "sqlite", "clio"),
        store_row("p2", "2026-03-03T00:00:00Z", 300, "postgres", "journal"),
        store_row("s2", "2026-02-02T00:00:00Z", 200, "sqlite", "clio"),
    ];

    let plan = select(&rows, &LinkedPaths::default(), 1).expect("a keep count of one is accepted");

    assert_eq!(
        kept_with_reasons(&plan),
        vec![("p1", "keep_window")],
        "the window is the merged order, not one window per store"
    );
    assert_eq!(
        candidates_by_store(&plan),
        vec![
            ("s1", "sqlite", "clio"),
            ("p2", "postgres", "journal"),
            ("s2", "sqlite", "clio"),
        ],
        "a snapshot of another store is not excluded by default, and each entry names its own store"
    );
    assert_eq!(plan.reclaimable_bytes, 900);
    assert_eq!(plan.listed_bytes, 1_400);
}

#[test]
fn a_store_with_one_archive_keeps_it_while_another_store_ages_out() {
    // Three snapshots under one slug and a single one under another slug of the
    // same backend: the store is the pair, so the lone snapshot is alone.
    let rows = vec![
        store_row("s1", "2026-05-05T00:00:00Z", 500, "sqlite", "clio"),
        store_row("s2", "2026-04-04T00:00:00Z", 400, "sqlite", "clio"),
        store_row("s3", "2026-03-03T00:00:00Z", 300, "sqlite", "clio"),
        store_row("d1", "2026-02-02T00:00:00Z", 200, "sqlite", "docs"),
    ];

    let plan = select(&rows, &LinkedPaths::default(), 1).expect("a keep count of one is accepted");

    assert_eq!(
        kept_with_reasons(&plan),
        vec![("s1", "keep_window"), ("d1", "last_of_store")],
        "the only snapshot of a store is kept whatever the keep count says"
    );
    assert_eq!(
        candidates_by_store(&plan),
        vec![("s2", "sqlite", "clio"), ("s3", "sqlite", "clio")],
        "the store that has older snapshots is the one that ages out"
    );
    assert_eq!(
        plan.reclaimable_bytes, 700,
        "the kept lone archive is not counted"
    );
    assert_eq!(plan.listed_bytes, 1_400, "it is still listed");
}

#[test]
fn a_store_whose_only_archive_is_unreadable_is_never_a_candidate_either() {
    let rows = vec![
        store_row("s1", "2026-05-05T00:00:00Z", 500, "sqlite", "clio"),
        store_row("s2", "2026-04-04T00:00:00Z", 400, "sqlite", "clio"),
        unreadable_row("d1", "the archive is truncated", 200),
    ];

    let plan = select(&rows, &LinkedPaths::default(), 1).expect("a keep count of one is accepted");

    assert_eq!(kept_with_reasons(&plan), vec![("s1", "keep_window")]);
    assert_eq!(
        candidates_by_store(&plan),
        vec![("s2", "sqlite", "clio")],
        "an unreadable archive of another store is still not a target"
    );
    assert_eq!(plan.unreadable.len(), 1);
    assert_eq!(
        plan.reclaimable_bytes, 400,
        "the unreadable archive's bytes are not reclaimable"
    );
}

/// One listing row at a real path, for the two cases that must go through the
/// filesystem. `LinkedPaths::among` reads the entries with `lstat`, so a
/// hand-written path that is not there would prove nothing about a link; every
/// other case here builds its rows by hand, which is what keeps selection
/// testable without a directory.
fn row_at(scratch: &Scratch, id: &str, created_at: &str, archive_bytes: u64) -> SnapshotRow {
    let path = scratch.join(&format!("{id}.cliobak.tar.gz"));
    SnapshotRow {
        path,
        ..store_row(id, created_at, archive_bytes, "sqlite", "clio")
    }
}

/// The one case the keep window must never lose a store's snapshot to: a link
/// whose id sorts ahead of the store's only real archive. The link's row is a
/// readable snapshot of the same store, because the listing reads a manifest
/// through a link, so before a link was excluded this ordering offered the real
/// archive for deletion and a confirmed run removed it. Unix-gated because it
/// names a link, and only Unix has them.
#[cfg(unix)]
#[test]
fn a_link_never_stands_in_as_a_stores_only_snapshot() {
    let scratch = Scratch::new("prune-select-link-first");
    let archive = row_at(
        &scratch,
        "clio-2026-05-05T000000Z-real",
        "2026-05-05T00:00:00Z",
        500,
    );
    let linked = row_at(&scratch, "aaa", "2026-05-05T00:00:00Z", 500);
    std::fs::write(&archive.path, b"a real archive").expect("write the archive");
    std::os::unix::fs::symlink(&archive.path, &linked.path).expect("link it under an earlier id");
    // The listing breaks the tie by id, so the link comes first in the order.
    let rows = vec![linked, archive];
    let links = LinkedPaths::among(&rows);

    let plan = select(&rows, &links, 1).expect("a keep count of one is accepted");

    assert_eq!(
        kept_with_reasons(&plan),
        vec![("clio-2026-05-05T000000Z-real", "keep_window")],
        "the store's only real archive is kept, and the link takes no slot"
    );
    assert_eq!(
        candidates_by_store(&plan),
        vec![("aaa", "sqlite", "clio")],
        "the link is the only thing on offer, so the report names it"
    );
    assert_eq!(
        plan.reclaimable_bytes, 0,
        "a link is never unlinked, so it frees nothing"
    );
    assert_eq!(
        plan.listed_bytes, 500,
        "the archive is counted once, on its own row"
    );
}

/// The byte half of the same rule, over a directory that has enough real
/// archives for a link's target to be a candidate as well. Counting a link's
/// target again would make the report claim twice the space the run can free.
#[cfg(unix)]
#[test]
fn a_links_bytes_are_counted_once_on_the_archive_it_points_at() {
    let scratch = Scratch::new("prune-select-link-bytes");
    let newest = row_at(
        &scratch,
        "clio-2026-05-05T000000Z-c",
        "2026-05-05T00:00:00Z",
        300,
    );
    let middle = row_at(
        &scratch,
        "clio-2026-05-05T000000Z-b",
        "2026-05-05T00:00:00Z",
        200,
    );
    let oldest = row_at(
        &scratch,
        "clio-2026-05-05T000000Z-a",
        "2026-05-05T00:00:00Z",
        100,
    );
    let linked = row_at(&scratch, "zzz", "2026-05-05T00:00:00Z", 100);
    for row in [&newest, &middle, &oldest] {
        std::fs::write(&row.path, b"a real archive").expect("write the archive");
    }
    // The link points at the oldest archive, which is a candidate in its own
    // right: its 100 bytes must not be added a second time.
    std::os::unix::fs::symlink(&oldest.path, &linked.path).expect("link to the oldest archive");
    let rows = vec![newest, middle, oldest, linked];
    let links = LinkedPaths::among(&rows);

    let plan = select(&rows, &links, 2).expect("a keep count of two is accepted");

    assert_eq!(
        candidates_by_store(&plan),
        vec![
            ("clio-2026-05-05T000000Z-a", "sqlite", "clio"),
            ("zzz", "sqlite", "clio"),
        ],
        "the real archive and the link that points at it"
    );
    assert_eq!(
        plan.reclaimable_bytes, 100,
        "only the real archive's bytes can be freed"
    );
    assert_eq!(plan.listed_bytes, 600, "three archives, counted once each");
}
