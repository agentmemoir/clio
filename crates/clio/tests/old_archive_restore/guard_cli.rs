// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for driving the real `clio backup restore` route
//! against an archive the erasure authority denies on SQLite, before any
//! erasure-related archive: a pre-denial archive refused on every selector, and
//! a missing, damaged, or wrong-deployment authority. Each refusal must leave
//! the target and the journal unchanged and the live control subject readable.
//!
//! ## Owns
//! - The SQLite pre-denial per-selector refusal matrix and its evidence line
//! - The SQLite missing, damaged, and wrong-deployment authority refusals
//!
//! ## Does not own
//! - The erasure-related SQLite refusals (see `guard_cli_erasure`)
//! - The archive assembly and read-back probes (see `support`)
//! - The shared CLI fixtures (see `guard_fixtures`) or the Postgres half (see
//!   `guard_cli_pg`)
//!
//! ## Boundary
//!
//! Changes to which pre-decision CLI restore refusals are measured should
//! normally require changes here. Changes to the guard should not.
//!
//! Keep this module focused on the pre-decision SQLite CLI restore refusals.

use crate::guard_fixtures::{
    assert_refused, backup_dir, current_authority, denied_hash, file_sha256, journal_file,
    run_restore, seed_sqlite,
};
use crate::support::{
    BANK, CONTROL_ITEM, CONTROL_SUBJECT, DENIED_ITEM, DENIED_SUBJECT, Readback, Scratch,
    archive_sqlite_with_authority, open_sqlite, readback,
};
use clio_backup::manifest::ManifestAuthority;
use clio_compliance::{initialize_new_empty, record_denial};
use std::path::PathBuf;

#[test]
fn sqlite_pre_denial_archive_is_refused_on_every_restore_route() {
    let scratch = Scratch::new("guard-cli-t01");
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

    // The archive carries the authority it was made under, before the denial.
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    let archive = archive_sqlite_with_authority(
        &backups,
        &source_db,
        &source_kms,
        "sqlite-guard",
        Some(current_authority(&journal)),
    );
    // The synthetic denial: the journal denies the subject while its key and
    // content remain intact, so only the guard can stop the restore.
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");

    // The target holds only the unrelated control subject and must stay put.
    let target_db = scratch.join("target/clio.db");
    let target_kms = scratch.join("target/clio.kms.json");
    seed_sqlite(
        &target_db,
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let target_before = file_sha256(&target_db);
    let journal_before = file_sha256(&journal);
    let target = target_db.to_str().expect("the target path is UTF-8");

    let id = archive.id.as_str();
    for selector in [
        vec![id],
        vec![id, "--no-pre-backup"],
        vec![id, "--dry-run"],
        vec!["--latest"],
    ] {
        let output = run_restore(&journal, &target_kms, &backups, target, &selector);
        assert_refused(&output, "denied subject");
        assert_eq!(
            file_sha256(&target_db),
            target_before,
            "selector {selector:?} placed bytes in the target"
        );
        assert_eq!(
            file_sha256(&journal),
            journal_before,
            "selector {selector:?} changed the journal authority"
        );
    }

    let store = open_sqlite(&target_db, &target_kms);
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the unrelated control subject stays readable after every refused route"
    );
    assert_eq!(
        readback(&store, DENIED_ITEM),
        Readback::Absent,
        "the denied subject never became active in the target"
    );
    println!(
        "REFUSED sqlite routes=4 archive={} target_sha256={} journal_sha256={}",
        archive.id, target_before, journal_before
    );
}

/// A journal path under the scratch root for a refusal that never writes it.
fn journal_for(scratch: &Scratch, name: &str) -> PathBuf {
    journal_file(scratch, &format!("journal/{name}.json"))
}

#[test]
fn sqlite_missing_damaged_and_wrong_deployment_authority_refuse() {
    let scratch = Scratch::new("guard-cli-t02");
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
    let target_db = scratch.join("target/clio.db");
    let target_kms = scratch.join("target/clio.kms.json");
    seed_sqlite(
        &target_db,
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let target = target_db.to_str().expect("the target path is UTF-8");
    let target_before = file_sha256(&target_db);

    // An archive with no recorded authority fails closed at classification.
    let no_authority = archive_sqlite_with_authority(
        &backups,
        &source_db,
        &source_kms,
        "sqlite-no-authority",
        None,
    );
    let output = run_restore(
        &journal_for(&scratch, "missing"),
        &target_kms,
        &backups,
        target,
        &[no_authority.id.as_str()],
    );
    assert_refused(&output, "no erasure authority");

    // A damaged current journal refuses before anything is placed.
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    let archive = archive_sqlite_with_authority(
        &backups,
        &source_db,
        &source_kms,
        "sqlite-damaged",
        Some(current_authority(&journal)),
    );
    std::fs::write(
        &journal,
        b"{\"schema_version\":1,\"deployment_id\":\"truncated\"",
    )
    .expect("plant the damaged journal");
    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        target,
        &[archive.id.as_str()],
    );
    assert_refused(&output, "journal is damaged");

    // Another deployment's archive claims a deployment the live journal is not.
    let other_journal = journal_file(&scratch, "journal/other.json");
    let other = initialize_new_empty(&other_journal).expect("initialize another deployment");
    let deployment_journal = journal_file(&scratch, "journal/deployment.json");
    let deployment =
        initialize_new_empty(&deployment_journal).expect("initialize the live deployment");
    assert_ne!(other.deployment_id, deployment.deployment_id);
    let relaunched = archive_sqlite_with_authority(
        &backups,
        &source_db,
        &source_kms,
        "sqlite-other-deployment",
        Some(ManifestAuthority {
            deployment_id: other.deployment_id.clone(),
            generation: other.generation,
            salt_fingerprint: deployment.salt_fingerprint,
        }),
    );
    // A current journal on a path of its own, but a different deployment than
    // the archive claims.
    let output = run_restore(
        &deployment_journal,
        &target_kms,
        &backups,
        target,
        &[relaunched.id.as_str()],
    );
    assert_refused(&output, "claims another deployment");

    assert_eq!(
        file_sha256(&target_db),
        target_before,
        "no authority refusal placed bytes in the target"
    );
    let store = open_sqlite(&target_db, &target_kms);
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the control subject stays readable"
    );
    println!(
        "REFUSED sqlite authority=missing,damaged,wrong-deployment target_sha256={target_before}"
    );
}
