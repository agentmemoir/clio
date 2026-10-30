// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for driving the real `clio backup restore` route
//! against an archive the erasure authority denies, on loopback Postgres: a
//! pre-denial archive refused on every selector, a missing or wrong-deployment
//! authority, a fresh post-erasure archive that restores without resurrecting
//! the subject, and a mixed allowed/denied archive. Each refusal must leave the
//! target's items, the journal, the archive, and the live control subject
//! unchanged.
//!
//! ## Owns
//! - The Postgres CLI-restore refusal and restore scenarios and their evidence
//!   lines
//!
//! ## Does not own
//! - The archive assembly and read-back probes (see `support_pg`)
//! - The SQLite half (see `guard_cli` and `guard_cli_erasure`) or the guard API
//!   (see the durable-erasure-authority measurements)
//!
//! ## Boundary
//!
//! Changes to which CLI restore refusals are measured should normally require
//! changes here. Changes to the guard should not.
//!
//! Keep this module focused on the Postgres CLI restore refusals.

use crate::guard_fixtures::{
    assert_refused, backup_dir, current_authority, denied_hash, file_sha256, journal_file,
    run_restore,
};
use crate::support::{
    BANK, CONTROL_ITEM, CONTROL_SUBJECT, DENIED_ITEM, DENIED_SUBJECT, Readback, Scratch,
    erase_subject, open_pg, readback, write_item,
};
use crate::support_pg::{
    archive_postgres_with_authority, pg_url, postgres_reachable, require_postgres,
};
use clio::store::{ScopedSchema, lock_pg_test, postgres_target_state};
use clio_backup::manifest::ManifestAuthority;
use clio_compliance::{initialize_new_empty, record_denial};
use std::path::Path;

/// Seed a Postgres deployment with the given `(item, subject, gist)` triples.
fn seed(url: &str, kms: &Path, items: &[(&str, &str, &str)]) {
    let store = open_pg(url, kms);
    for (id, subject, gist) in items {
        write_item(&store, id, subject, gist);
    }
}

/// The target schema's `items` count, the Postgres stand-in for a file hash.
fn items(url: &str) -> Option<u64> {
    postgres_target_state(url)
        .expect("measure the target schema")
        .count_of("items")
}

#[test]
fn postgres_pre_denial_archive_is_refused_on_every_restore_route() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg_test();
    let scratch = Scratch::new("guard-cli-pg-t01");
    let backups = backup_dir(&scratch);
    let source = ScopedSchema::new(&base);
    let source_kms = scratch.join("source/clio.kms.json");
    seed(
        source.url(),
        &source_kms,
        &[
            (DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist"),
            (CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist"),
        ],
    );

    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    let archive = archive_postgres_with_authority(
        &backups,
        source.url(),
        &source_kms,
        "pg-guard",
        Some(current_authority(&journal)),
    );
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");

    let target = ScopedSchema::new(&base);
    let target_kms = scratch.join("target/clio.kms.json");
    seed(
        target.url(),
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let items_before = items(target.url());
    let journal_before = file_sha256(&journal);
    let target_db = target.url();

    let id = archive.id.as_str();
    for selector in [
        vec![id],
        vec![id, "--no-pre-backup"],
        vec![id, "--dry-run"],
        vec!["--latest"],
    ] {
        let output = run_restore(&journal, &target_kms, &backups, target_db, &selector);
        assert_refused(&output, "denied subject");
        assert_eq!(
            items(target_db),
            items_before,
            "selector {selector:?} changed the target's item count"
        );
        assert_eq!(
            file_sha256(&journal),
            journal_before,
            "selector {selector:?} changed the journal authority"
        );
    }

    let store = open_pg(target.url(), &target_kms);
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
        "REFUSED postgres routes=4 archive={} items={items_before:?} journal_sha256={journal_before}",
        archive.id
    );
}

#[test]
fn postgres_missing_and_wrong_deployment_authority_refuse() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg_test();
    let scratch = Scratch::new("guard-cli-pg-t02");
    let backups = backup_dir(&scratch);
    let source = ScopedSchema::new(&base);
    let source_kms = scratch.join("source/clio.kms.json");
    seed(
        source.url(),
        &source_kms,
        &[
            (DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist"),
            (CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist"),
        ],
    );
    let target = ScopedSchema::new(&base);
    let target_kms = scratch.join("target/clio.kms.json");
    seed(
        target.url(),
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let target_db = target.url();
    let items_before = items(target_db);

    let no_authority = archive_postgres_with_authority(
        &backups,
        source.url(),
        &source_kms,
        "pg-no-authority",
        None,
    );
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    let journal_before = file_sha256(&journal);
    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        target_db,
        &[no_authority.id.as_str()],
    );
    assert_refused(&output, "no erasure authority");

    let other_journal = journal_file(&scratch, "journal/other.json");
    let other = initialize_new_empty(&other_journal).expect("initialize another deployment");
    let relaunched = archive_postgres_with_authority(
        &backups,
        source.url(),
        &source_kms,
        "pg-other-deployment",
        Some(ManifestAuthority {
            deployment_id: other.deployment_id.clone(),
            generation: other.generation,
            salt_fingerprint: current_authority(&journal).salt_fingerprint,
        }),
    );
    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        target_db,
        &[relaunched.id.as_str()],
    );
    assert_refused(&output, "claims another deployment");

    assert_eq!(
        items(target_db),
        items_before,
        "no authority refusal changed the target's item count"
    );
    assert_eq!(
        file_sha256(&journal),
        journal_before,
        "no authority refusal changed the journal"
    );
    let store = open_pg(target.url(), &target_kms);
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the control subject stays readable"
    );
    println!("REFUSED postgres authority=missing,wrong-deployment items={items_before:?}");
}

#[test]
fn postgres_post_erasure_archive_restores_without_resurrecting_the_subject() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg_test();
    let scratch = Scratch::new("guard-cli-pg-t05");
    let backups = backup_dir(&scratch);
    let source = ScopedSchema::new(&base);
    let source_kms = scratch.join("source/clio.kms.json");
    seed(
        source.url(),
        &source_kms,
        &[
            (DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist"),
            (CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist"),
        ],
    );
    let store = open_pg(source.url(), &source_kms);
    erase_subject(&store, DENIED_SUBJECT, "req-guard-cli-pg-t05");
    drop(store);

    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    let archive = archive_postgres_with_authority(
        &backups,
        source.url(),
        &source_kms,
        "pg-post-erasure",
        Some(current_authority(&journal)),
    );
    crate::common::seed_restore_custody(&journal, &archive.id, &archive.path);
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");

    let target = ScopedSchema::new(&base);
    let target_kms = scratch.join("target/clio.kms.json");
    seed(
        target.url(),
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let target_db = target.url();
    let archive_before = file_sha256(&archive.path);

    // The archive carries no usable key for the denied subject, so the backup
    // taken after the erasure restores and the archive is never rewritten.
    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        target_db,
        &[archive.id.as_str()],
    );
    assert_eq!(
        output.code,
        Some(0),
        "a post-erasure archive must restore: {}",
        output.stderr
    );
    assert_eq!(
        file_sha256(&archive.path),
        archive_before,
        "the restore does not sanitize the archive"
    );

    let store = open_pg(target.url(), &target_kms);
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
    println!("RESTORED postgres post-erasure archive={}", archive.id);
}

#[test]
fn postgres_mixed_allowed_and_denied_archive_places_no_bytes() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg_test();
    let scratch = Scratch::new("guard-cli-pg-t04");
    let backups = backup_dir(&scratch);
    let source = ScopedSchema::new(&base);
    let source_kms = scratch.join("source/clio.kms.json");
    seed(
        source.url(),
        &source_kms,
        &[
            (DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist"),
            (CONTROL_ITEM, CONTROL_SUBJECT, "archived control gist"),
        ],
    );
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    let archive = archive_postgres_with_authority(
        &backups,
        source.url(),
        &source_kms,
        "pg-mixed",
        Some(current_authority(&journal)),
    );
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");

    // The live target holds its own allowed subject; the mixed archive must not
    // overwrite it.
    let target = ScopedSchema::new(&base);
    let target_kms = scratch.join("target/clio.kms.json");
    seed(
        target.url(),
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "live control gist")],
    );
    let target_db = target.url();
    let items_before = items(target_db);
    let journal_before = file_sha256(&journal);
    let archive_before = file_sha256(&archive.path);

    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        target_db,
        &[archive.id.as_str()],
    );
    assert_refused(&output, "denied subject");
    assert_eq!(
        items(target_db),
        items_before,
        "the mixed archive placed no rows"
    );
    assert_eq!(
        file_sha256(&journal),
        journal_before,
        "the mixed-archive refusal left the journal unchanged"
    );
    assert_eq!(
        file_sha256(&archive.path),
        archive_before,
        "the mixed archive is not sanitized"
    );

    let store = open_pg(target.url(), &target_kms);
    assert_eq!(
        readback(&store, CONTROL_ITEM),
        Readback::Readable,
        "the live allowed subject stays readable"
    );
    assert_eq!(
        readback(&store, DENIED_ITEM),
        Readback::Absent,
        "the denied member never landed"
    );
    println!(
        "REFUSED postgres mixed-archive archive={} items={items_before:?}",
        archive.id
    );
}
