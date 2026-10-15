// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for driving the placement-time half of the real
//! `clio backup restore` route on SQLite: a denial that lands between the
//! plan-time check and placement while the erase path's journal lock is held,
//! and the journal resolution the erase boundary and the restore share. The
//! placement-time refusal must leave the target and the journal unchanged.
//!
//! ## Owns
//! - The CLI-level denial-after-plan run and its lock-contention evidence
//! - The check that a document-named journal resolves to the journal lock
//!
//! ## Does not own
//! - The pre-decision and erasure-related refusals (see `guard_cli` and
//!   `guard_cli_erasure`)
//! - The guarded-restore sequence itself (see the durable-erasure-authority
//!   measurements)
//! - The shared CLI fixtures (see `guard_fixtures`)
//!
//! ## Boundary
//!
//! Changes to how the placement-time re-check and the journal lock are measured
//! should normally require changes here. Changes to the guard should not.
//!
//! Keep this module focused on the placement-time guard and the journal lock.

use crate::guard_fixtures::{
    RESTORE_WAIT, assert_refused, backup_dir, current_authority, denied_hash, file_sha256,
    journal_file, seed_sqlite, spawn_restore,
};
use crate::support::{
    BANK, CONTROL_ITEM, CONTROL_SUBJECT, DENIED_ITEM, DENIED_SUBJECT, Readback, Scratch,
    archive_sqlite_with_authority, open_sqlite, readback,
};
use clio_compliance::{JournalHold, initialize_new_empty, record_denial};

#[test]
fn sqlite_restore_holds_the_erase_journal_lock_and_refuses_a_denial_after_plan() {
    let scratch = Scratch::new("guard-cli-t03");
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
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    let archive = archive_sqlite_with_authority(
        &backups,
        &source_db,
        &source_kms,
        "sqlite-denial-after-plan",
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

    // The erase path and the restore contend for one journal lock. This process
    // takes the lock the erase boundary takes and records a denial under it: the
    // restore plans while the hold is live, then blocks at placement until the
    // hold is released, so the denial lands strictly after the plan-time check.
    let hold = JournalHold::acquire(&journal).expect("take the journal lock the erase path takes");
    let child = spawn_restore(
        &journal,
        &target_kms,
        &backups,
        target,
        &[archive.id.as_str()],
    );
    // The plan line proves the plan-time check passed and the child is about to
    // place, so the denial below is seen only by the placement-time re-check.
    child.await_line("restore plan:", RESTORE_WAIT);

    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");
    let journal_after = file_sha256(&journal);
    drop(hold);

    let output = child.finish(RESTORE_WAIT);
    assert_refused(&output, "denied subject");
    assert_eq!(
        file_sha256(&target_db),
        target_before,
        "the placement-time refusal placed no bytes"
    );
    assert_eq!(
        file_sha256(&journal),
        journal_after,
        "the placement-time refusal left the journal unchanged"
    );
    let store = open_sqlite(&target_db, &target_kms);
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the live permitted subject stays readable"
    );
    assert_eq!(
        readback(&store, DENIED_ITEM),
        Readback::Absent,
        "the denied subject never became active"
    );
    println!(
        "REFUSED sqlite denial-after-plan archive={} target_sha256={target_before}",
        archive.id
    );
}

#[test]
fn a_document_named_journal_resolves_to_the_lock_the_restore_takes() {
    let scratch = Scratch::new("guard-cli-journal-resolve");
    let document = scratch.join("journal/document-journal.json");
    std::fs::create_dir_all(document.parent().expect("the journal has a parent"))
        .expect("create the journal directory");
    // The environment names the same file the host document names.
    let get = |_: &str| Some(document.to_string_lossy().into_owned());

    let by_document =
        JournalHold::resolve_journal_path(Some(&document), &get).expect("the document path");
    let by_environment =
        JournalHold::resolve_journal_path(None, &get).expect("the environment path");
    assert_eq!(
        by_document, document,
        "a document-named journal resolves to the document path"
    );
    assert_eq!(
        by_environment, by_document,
        "the environment and the document resolve to one journal"
    );

    // The lock is keyed on the resolved path, so a second hold on either
    // resolution is refused as already held by this process.
    let hold = JournalHold::acquire(&by_document).expect("take the journal lock");
    assert!(
        JournalHold::acquire(&by_environment).is_err(),
        "the document and the environment name the same lock"
    );
    drop(hold);
    println!("RESOLVED journal={} lock=one", by_document.display());
}
