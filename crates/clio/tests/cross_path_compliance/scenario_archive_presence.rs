// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving the archive-presence claim: that a
//! fresh same-deployment archive carrying only content-free records for a
//! denied subject — a destroyed-key slot, a tombstone, or a ciphertext-only
//! row — restores cleanly, because none of them can make the erased content
//! readable, while an unclassifiable member still refuses the whole archive.
//!
//! ## Owns
//! - The allowed restore, the denied subject's continued unreadability, and the
//!   unclassifiable-member refusal assertions
//!
//! ## Does not own
//! - The crafted archive and the shared build state (see
//!   `scenario_archive_presence_test_support`)
//! - The archive format and member extraction (see `clio-backup`)
//! - The guarded restore admission (see `clio-compliance`)
//! - The one-artifact-at-a-time isolation (see
//!   `scenario_archive_artifact_isolation`)
//!
//! ## Boundary
//!
//! Changes to which archive shapes this claim proves should normally require
//! changes here. Changes to the archive format should not.
//!
//! Keep this module focused on the archive-presence claim.

use crate::case_fixtures::{
    BANK, CONTROL_ITEM, CONTROL_SUBJECT, DENIED_ITEM, DENIED_SUBJECT, backup_dir, journal_file,
    open_pg, open_sqlite, scratch, seed_pg, seed_sqlite,
};
use crate::cli_test_support::{
    UNCLASSIFIED_MEMBER_REFUSAL, UNPROVEN_SUBJECT_REFUSAL, assert_refused, denied_hash,
};
use crate::scenario_archive_presence_test_support::{
    gated_pg, gated_sqlite, place_raw, raw_with_unclassifiable, run_restore, unknown_pg,
    unknown_sqlite,
};
use crate::support_pg_test_support::{lock_pg, pg_url, postgres_reachable, require_postgres};
use crate::support_test_support::{
    Readback, archive_sqlite_keyless, current_authority, erase_subject, file_sha256, items,
    readback,
};
use clio::store::ScopedSchema;
use clio_compliance::{initialize_new_empty, record_denial};

#[test]
fn sqlite_content_free_presence_restores_while_unknown_member_refuses() {
    let scratch = scratch("scenario05-sqlite");
    let backups = backup_dir(&scratch);
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    let presence = gated_sqlite(&scratch, &backups, &journal);
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");

    let target_db = scratch.join("target/clio.db");
    let target_kms = scratch.join("target/clio.kms.json");
    seed_sqlite(
        &target_db,
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let target = target_db.to_str().expect("the target path is UTF-8");
    let archive_before = file_sha256(&presence.path);
    let journal_before = file_sha256(&journal);

    // The unchanged post-erasure archive carries the denied subject's destroyed
    // key slot, tombstone, and ciphertext-only row. None of them carries a
    // usable key, so every real selector restores it, and no restore rewrites
    // the archive or the journal.
    for selector in [
        vec!["--latest"],
        vec![presence.id.as_str()],
        vec![presence.id.as_str(), "--no-pre-backup"],
        vec![presence.id.as_str(), "--dry-run"],
    ] {
        let output = run_restore(&journal, &target_kms, &backups, target, &selector);
        assert_eq!(
            output.code,
            Some(0),
            "selector {selector:?} must restore a content-free post-erasure archive: {}",
            output.stderr
        );
        assert_eq!(
            file_sha256(&presence.path),
            archive_before,
            "selector {selector:?} rewrote the archive"
        );
        assert_eq!(
            file_sha256(&journal),
            journal_before,
            "selector {selector:?} changed the journal"
        );
    }

    // The permitted subject is readable after the restore, and the denied
    // subject's row is present but unreadable: the content-free records carried
    // no usable key.
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
    drop(store);

    // Each refusal must place nothing and leave the journal and archive alone.
    let target_before = file_sha256(&target_db);
    let archive_before = file_sha256(&presence.path);
    let journal_before = file_sha256(&journal);

    // The archive carrying both the content-free presence and an unclassifiable
    // member refuses as a whole on the unclassifiable member.
    let combined = format!("{}-combined", presence.id);
    let raw = raw_with_unclassifiable(&presence, &scratch.join("raw-combined"));
    place_raw(&backups, &combined, &raw);
    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        target,
        &[combined.as_str()],
    );
    assert_refused(&output, &UNCLASSIFIED_MEMBER_REFUSAL);
    assert_eq!(
        file_sha256(&target_db),
        target_before,
        "the combined refusal placed bytes"
    );

    // A member the manifest does not classify refuses the whole archive too.
    let unknown = unknown_sqlite(&scratch, &backups, &journal);
    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        target,
        &[unknown.id.as_str()],
    );
    assert_refused(&output, &UNCLASSIFIED_MEMBER_REFUSAL);
    assert_eq!(
        file_sha256(&target_db),
        target_before,
        "the unclassifiable-member refusal placed bytes"
    );
    assert_eq!(
        file_sha256(&journal),
        journal_before,
        "the refusals changed the journal"
    );
    assert_eq!(
        file_sha256(&presence.path),
        archive_before,
        "the refusals rewrote the presence archive"
    );

    println!(
        "RESTORED sqlite presence={} unknown={}",
        presence.id, unknown.id
    );
}

/// A keyless post-erasure archive cannot prove its content-free reference
/// unreadable, because the restore leaves the target's key store in place, so
/// it refuses rather than admitting the reference.
#[test]
fn sqlite_keyless_post_erasure_archive_refuses_as_unproven() {
    let scratch = scratch("scenario05-keyless");
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
    let store = open_sqlite(&source_db, &source_kms);
    erase_subject(&store, DENIED_SUBJECT, "req-scenario05-keyless");
    drop(store);

    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    initialize_new_empty(&journal).expect("initialize the deployment");
    let keyless = archive_sqlite_keyless(
        &backups,
        &source_db,
        &source_kms,
        "sqlite-keyless",
        Some(current_authority(&journal)),
    );
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");

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

    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        target,
        &[keyless.id.as_str(), "--allow-missing-keys"],
    );
    assert_refused(&output, &UNPROVEN_SUBJECT_REFUSAL);
    assert_eq!(
        file_sha256(&target_db),
        target_before,
        "the keyless refusal placed bytes"
    );
    assert_eq!(
        file_sha256(&journal),
        journal_before,
        "the keyless refusal changed the journal"
    );
    println!("REFUSED sqlite keyless archive={}", keyless.id);
}

#[test]
fn postgres_content_free_presence_restores_while_unknown_member_refuses() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg();
    let scratch = scratch("scenario05-pg");
    let backups = backup_dir(&scratch);
    let journal = journal_file(&scratch, "journal/erasure-journal.json");
    let presence = gated_pg(&base, &scratch, &backups, &journal);
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record the denial");

    let target = ScopedSchema::new(&base);
    let target_kms = scratch.join("target/clio.kms.json");
    seed_pg(
        target.url(),
        &target_kms,
        &[(CONTROL_ITEM, CONTROL_SUBJECT, "control sentinel gist")],
    );
    let target_db = target.url().to_owned();
    let archive_before = file_sha256(&presence.path);
    let journal_before = file_sha256(&journal);

    // The unchanged post-erasure archive carries only content-free records for
    // the denied subject, so every real selector restores it, and no restore
    // rewrites the archive or the journal.
    for selector in [
        vec!["--latest"],
        vec![presence.id.as_str()],
        vec![presence.id.as_str(), "--no-pre-backup"],
        vec![presence.id.as_str(), "--dry-run"],
    ] {
        let output = run_restore(&journal, &target_kms, &backups, &target_db, &selector);
        assert_eq!(
            output.code,
            Some(0),
            "selector {selector:?} must restore a content-free post-erasure archive: {}",
            output.stderr
        );
        assert_eq!(
            file_sha256(&presence.path),
            archive_before,
            "selector {selector:?} rewrote the archive"
        );
        assert_eq!(
            file_sha256(&journal),
            journal_before,
            "selector {selector:?} changed the journal"
        );
    }

    // The permitted subject is readable after the restore, and the denied
    // subject's row is present but unreadable: the content-free records carried
    // no usable key.
    let store = open_pg(&target_db, &target_kms);
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
    drop(store);

    // The refusal must place no rows and leave the journal and archive alone.
    let items_before = items(&target_db);
    let archive_before = file_sha256(&presence.path);
    let journal_before = file_sha256(&journal);
    let unknown = unknown_pg(&base, &scratch, &backups, &journal);
    let output = run_restore(
        &journal,
        &target_kms,
        &backups,
        &target_db,
        &[unknown.id.as_str()],
    );
    assert_refused(&output, &UNCLASSIFIED_MEMBER_REFUSAL);
    assert_eq!(
        items(&target_db),
        items_before,
        "the unclassifiable-member refusal placed rows"
    );
    assert_eq!(
        file_sha256(&journal),
        journal_before,
        "the unclassifiable-member refusal changed the journal"
    );
    assert_eq!(
        file_sha256(&presence.path),
        archive_before,
        "the unclassifiable-member refusal rewrote the presence archive"
    );

    println!(
        "RESTORED postgres presence={} unknown={}",
        presence.id, unknown.id
    );
}
