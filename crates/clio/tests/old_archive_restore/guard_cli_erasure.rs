// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for driving the real `clio backup restore` route
//! against erasure-related archives on SQLite: a fresh post-erasure archive
//! that restores without resurrecting the subject, an archive with an
//! unclassifiable member, a replaced empty journal, and a mixed
//! allowed/denied archive that still refuses. Each refusal must leave the
//! target and the journal unchanged and the live control subject readable.
//!
//! ## Owns
//! - The post-erasure restore, the unclassifiable-member, empty-journal, and
//!   mixed-archive SQLite behavior and their evidence lines
//!
//! ## Does not own
//! - The pre-decision authority refusals (see `guard_cli`)
//! - The placement-time re-check and the journal lock (see `guard_cli_placement`)
//! - The archive assembly and read-back probes (see `support`)
//! - The shared CLI fixtures (see `guard_fixtures`) or the Postgres half (see
//!   `guard_cli_pg`)
//!
//! ## Boundary
//!
//! Changes to which erasure-related CLI restore refusals are measured should
//! normally require changes here. Changes to the guard or the journal should not.
//!
//! Keep this module focused on the erasure-related SQLite CLI refusals.

use crate::guard_fixtures::{
    assert_refused, backup_dir, current_authority, denied_hash, file_sha256, journal_file,
    run_restore, seed_sqlite,
};
use crate::support::{
    BANK, CONTROL_ITEM, CONTROL_SUBJECT, DENIED_ITEM, DENIED_SUBJECT, Readback, Scratch,
    archive_sqlite_with_authority, erase_subject, open_sqlite, readback,
};
use clio::store::{SqliteStore, Store};
use clio_backup::archive_file_name;
use clio_compliance::{initialize_new_empty, record_denial};
use std::path::{Path, PathBuf};

/// One item's gist, or `None` when it is absent or unreadable.
fn gist(store: &SqliteStore, item_id: &str) -> Option<String> {
    store
        .get_memory_item(BANK, item_id)
        .ok()
        .flatten()
        .and_then(|item| item.gist)
}

/// Write a raw archive's bytes over `id`'s archive in `backups`, so the CLI
/// lists the same id and reads the hand-built container.
fn place_raw(backups: &Path, id: &str, raw: &Path) -> PathBuf {
    let destination = backups.join(archive_file_name(id));
    std::fs::copy(raw, &destination).expect("place the raw archive under the snapshot id");
    destination
}

#[test]
fn sqlite_post_erasure_archive_restores_without_resurrecting_the_subject() {
    let scratch = Scratch::new("guard-cli-t05a");
    let backups = backup_dir(&scratch);
    let source_db = scratch.join("source/clio.db");
    let source_kms = scratch.join("source/clio.kms.json");
    seed_sqlite(
        &source_db,
        &source_kms,
        &[
            (DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist"),
            (CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist"),
        ],
    );
    // A real erasure leaves the content-free records: a destroyed slot, a
    // tombstone, and a ciphertext-only row for the denied subject.
    let store = open_sqlite(&source_db, &source_kms);
    erase_subject(&store, DENIED_SUBJECT, "req-guard-cli-t05");
    drop(store);

    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    let post_erasure = archive_sqlite_with_authority(
        &backups,
        &source_db,
        &source_kms,
        "sqlite-post-erasure",
        Some(current_authority(&journal)),
    );
    crate::common::seed_restore_custody(&journal, &post_erasure.id, &post_erasure.path);
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");

    let target_db = scratch.join("target/clio.db");
    let target_kms = scratch.join("target/clio.kms.json");
    seed_sqlite(
        &target_db,
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let target = target_db.to_str().expect("the target path is UTF-8");
    let archive_before = file_sha256(&post_erasure.path);

    // The archive carries no usable key for the denied subject, so the backup
    // taken after the erasure restores. The archive itself is never rewritten.
    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        target,
        &[post_erasure.id.as_str()],
    );
    assert_eq!(
        output.code,
        Some(0),
        "a post-erasure archive must restore: {}",
        output.stderr
    );
    assert_eq!(
        file_sha256(&post_erasure.path),
        archive_before,
        "the restore does not sanitize the archive"
    );
    let store = open_sqlite(&target_db, &target_kms);
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the permitted subject restores readable"
    );
    assert_eq!(
        readback(&store, DENIED_ITEM),
        Readback::DestroyedKey,
        "the denied subject's row restores without a usable key"
    );
    println!("RESTORED sqlite post-erasure archive={}", post_erasure.id);
}

#[test]
fn sqlite_unclassifiable_member_refuses() {
    let scratch = Scratch::new("guard-cli-t05b1");
    let backups = backup_dir(&scratch);
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");

    let control_db = scratch.join("control/clio.db");
    let control_kms = scratch.join("control/clio.kms.json");
    seed_sqlite(
        &control_db,
        &control_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let control_archive = archive_sqlite_with_authority(
        &backups,
        &control_db,
        &control_kms,
        "sqlite-control-raw",
        Some(current_authority(&journal)),
    );

    let target_db = scratch.join("target/clio.db");
    let target_kms = scratch.join("target/clio.kms.json");
    seed_sqlite(
        &target_db,
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let target = target_db.to_str().expect("the target path is UTF-8");
    let target_before = file_sha256(&target_db);
    let journal_before = file_sha256(&journal);

    // An archive that references no denied subject but carries one undeclared
    // member refuses as a whole, classified as an unknown member.
    let raw_dir = scratch.join("raw");
    let raw = crate::members::raw_with_unclassifiable(&control_archive, &raw_dir);
    let _ = place_raw(&backups, &control_archive.id, &raw);
    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        target,
        &[control_archive.id.as_str()],
    );
    assert_refused(&output, "cannot be classified");
    assert_eq!(
        file_sha256(&target_db),
        target_before,
        "the unknown-member refusal placed no bytes"
    );
    assert_eq!(
        file_sha256(&journal),
        journal_before,
        "the unknown-member refusal left the journal unchanged"
    );
    let store = open_sqlite(&target_db, &target_kms);
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the live permitted subject stays readable"
    );
    println!(
        "REFUSED sqlite unclassifiable-member archive={} target_sha256={target_before}",
        control_archive.id
    );
}

#[test]
fn sqlite_replaced_empty_journal_refuses() {
    let scratch = Scratch::new("guard-cli-t05b2");
    let backups = backup_dir(&scratch);
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");

    let control_db = scratch.join("control/clio.db");
    let control_kms = scratch.join("control/clio.kms.json");
    seed_sqlite(
        &control_db,
        &control_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let control_archive = archive_sqlite_with_authority(
        &backups,
        &control_db,
        &control_kms,
        "sqlite-control-empty",
        Some(current_authority(&journal)),
    );

    let target_db = scratch.join("target/clio.db");
    let target_kms = scratch.join("target/clio.kms.json");
    seed_sqlite(
        &target_db,
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let target = target_db.to_str().expect("the target path is UTF-8");
    let target_before = file_sha256(&target_db);

    // A fresh empty journal cannot validate the old archive: no bypass.
    let saved = scratch.join("saved-journal.json");
    std::fs::copy(&journal, &saved).expect("save the real journal");
    std::fs::remove_file(&journal).expect("remove the real journal");
    initialize_new_empty(&journal).expect("replace with a new empty authority");
    let journal_before = file_sha256(&journal);

    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        target,
        &[control_archive.id.as_str()],
    );
    assert_refused(&output, "claims another deployment");
    assert_eq!(
        file_sha256(&target_db),
        target_before,
        "the empty-journal refusal placed no bytes"
    );
    assert_eq!(
        file_sha256(&journal),
        journal_before,
        "the empty-journal refusal left the journal unchanged"
    );
    let store = open_sqlite(&target_db, &target_kms);
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the live permitted subject stays readable"
    );
    println!("REFUSED sqlite replaced-empty-journal target_sha256={target_before}");
}

#[test]
fn sqlite_mixed_allowed_and_denied_archive_preserves_live_data() {
    let scratch = Scratch::new("guard-cli-t04");
    let backups = backup_dir(&scratch);
    let source_db = scratch.join("source/clio.db");
    let source_kms = scratch.join("source/clio.kms.json");
    seed_sqlite(
        &source_db,
        &source_kms,
        &[
            (DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist"),
            (CONTROL_ITEM, CONTROL_SUBJECT, "archived control gist"),
        ],
    );
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    let archive = archive_sqlite_with_authority(
        &backups,
        &source_db,
        &source_kms,
        "sqlite-mixed",
        Some(current_authority(&journal)),
    );
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");

    // The live target holds its own allowed subject under the control item id;
    // the mixed archive must not overwrite it.
    let target_db = scratch.join("target/clio.db");
    let target_kms = scratch.join("target/clio.kms.json");
    seed_sqlite(
        &target_db,
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "live control gist")],
    );
    let target = target_db.to_str().expect("the target path is UTF-8");
    let target_before = file_sha256(&target_db);
    let journal_before = file_sha256(&journal);

    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        target,
        &[archive.id.as_str()],
    );
    assert_refused(&output, "denied subject");
    assert_eq!(
        file_sha256(&target_db),
        target_before,
        "the mixed archive placed no bytes"
    );
    assert_eq!(
        file_sha256(&journal),
        journal_before,
        "the mixed-archive refusal left the journal unchanged"
    );

    let store = open_sqlite(&target_db, &target_kms);
    assert_eq!(
        gist(&store, CONTROL_ITEM).as_deref(),
        Some("live control gist"),
        "the live allowed subject is preserved, not overwritten"
    );
    assert_eq!(
        readback(&store, DENIED_ITEM),
        Readback::Absent,
        "the denied member never landed"
    );
    println!(
        "REFUSED sqlite mixed-archive archive={} target_sha256={target_before}",
        archive.id
    );
}
