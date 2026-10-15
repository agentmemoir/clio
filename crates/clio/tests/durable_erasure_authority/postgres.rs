// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the store-backed scenarios on loopback
//! Postgres: the durable denial surviving a restart with no content key
//! minted by the journal query, and the refusal of denied membership derived
//! from a destroyed slot, a tombstone, and a ciphertext-only row.
//!
//! ## Owns
//! - The Postgres restart scenario and its key-store inventory comparison
//! - The Postgres denied-membership scenario and its journal-bytes hash
//!
//! ## Does not own
//! - The fixtures and probes (see `support`)
//! - The same scenarios on SQLite (see `sqlite`)
//!
//! ## Boundary
//!
//! Changes to which Postgres facts are measured should normally require
//! changes here. Changes to the journal or the store backend should not.
//!
//! Keep this module focused on the Postgres store-backed scenarios.

use crate::support::{
    BANK, DENIED_ITEM, DENIED_SUBJECT, Scratch, denied_hash, erase_subject, file_sha256,
    journal_path, keys_inventory_digest, observe_journal, open_pg, read_error, require_postgres,
    write_item,
};
use clio::ErrorCode;
use clio::store::{ComplianceStore, ScopedSchema, lock_pg_test};
use clio_compliance::{
    ArchiveClaim, AuthorityRefusal, JournalState, MemberFacts, check_archive_authority,
};

#[test]
fn postgres_restart_keeps_the_durable_denial_and_the_journal_query_mints_no_key() {
    let Some(base) = require_postgres() else {
        return;
    };
    let _guard = lock_pg_test();
    let scratch = Scratch::new("pg-restart");
    let journal = journal_path(&scratch, "journal.json");
    let kms = scratch.join("a/clio.kms.json");
    let schema = ScopedSchema::new(&base);
    let store = open_pg(schema.url(), &kms);
    write_item(&store, DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist");

    // The production erasure: the durable denial is recorded strictly before
    // the key loss.
    let outcome = erase_subject(&store, &journal, DENIED_SUBJECT, "req-pg-restart");
    assert_eq!(outcome.status, "COMPLETED", "the erasure completed");
    assert_eq!(
        outcome.subject_hash,
        denied_hash(),
        "the erased subject hash matches the journal denial"
    );
    let before = observe_journal("pg-restart-before", &journal);
    assert_eq!(before.generation, 1, "one denial was recorded");
    drop(store);
    let inventory_before_restart = keys_inventory_digest(&kms);

    // Restart: reopen the store, then query the journal from disk.
    let store = open_pg(schema.url(), &kms);
    let inventory_after_reopen = keys_inventory_digest(&kms);
    let after = observe_journal("pg-restart-after", &journal);

    assert_eq!(
        after, before,
        "the exact denial, generation, and deployment identity survive the restart"
    );
    let denial = after
        .denied(&denied_hash())
        .expect("the journal denies the erased subject");
    assert_eq!(
        denial.banks,
        vec![BANK.to_owned()],
        "the denial carries the confirmed bank"
    );

    // No new content key across the restart: reopening the store is the first
    // operation that could mint one, and the inventory is sampled right after
    // it.
    assert_eq!(
        inventory_after_reopen, inventory_before_restart,
        "reopening the store minted no content key"
    );
    // The denied slot stays destroyed and the ciphertext-only row unreadable.
    // These probes are the operations that could mint or resurrect key
    // material, so the inventory is sampled again after them; the sample spans
    // the journal query too, so a query that minted a key would change it.
    assert!(
        store
            .subject_dek_destroyed(DENIED_SUBJECT)
            .expect("probe the denied slot"),
        "the destroyed slot is retained"
    );
    assert_eq!(
        read_error(&store, DENIED_ITEM),
        Some(ErrorCode::ErasedSubject),
        "the ciphertext-only row stays unreadable"
    );
    let inventory_after_probes = keys_inventory_digest(&kms);
    assert_eq!(
        inventory_after_probes, inventory_after_reopen,
        "no content key was minted by the journal query or the read-back probes"
    );

    println!(
        "EVIDENCE pg-restart deployment={} generation={} denial_banks={:?} slots={} \
         inventory={} journal_sha256={}",
        after.deployment_id,
        after.generation,
        denial.banks,
        inventory_after_probes.1,
        inventory_after_probes.0,
        file_sha256(&journal),
    );
}

#[test]
fn postgres_denied_membership_from_a_tombstone_refuses_with_the_journal_unchanged() {
    let Some(base) = require_postgres() else {
        return;
    };
    let _guard = lock_pg_test();
    let scratch = Scratch::new("pg-membership");
    let journal = journal_path(&scratch, "journal.json");
    let kms = scratch.join("a/clio.kms.json");
    let schema = ScopedSchema::new(&base);
    let store = open_pg(schema.url(), &kms);
    write_item(&store, DENIED_ITEM, DENIED_SUBJECT, "denied sentinel gist");
    let outcome = erase_subject(&store, &journal, DENIED_SUBJECT, "req-pg-membership");
    assert_eq!(outcome.status, "COMPLETED", "the erasure completed");

    // The content-free records a destroyed erase leaves behind: a destroyed
    // slot, tombstone rows carrying the same subject hash the journal denies,
    // and the ciphertext-only item row.
    assert!(
        store
            .subject_dek_destroyed(DENIED_SUBJECT)
            .expect("probe the denied slot"),
        "the destroyed slot is retained"
    );
    assert_eq!(
        read_error(&store, DENIED_ITEM),
        Some(ErrorCode::ErasedSubject),
        "the ciphertext-only row stays unreadable"
    );
    let tombstones = store.list_tombstones(BANK).expect("list the tombstones");
    let subject_hash = tombstones
        .iter()
        .find(|row| row.subject_hash == denied_hash())
        .map_or_else(
            || panic!("no tombstone carries the denied hash: {tombstones:?}"),
            |row| row.subject_hash.clone(),
        );

    let current = observe_journal("pg-membership-current", &journal);
    let state = JournalState::Current(current.clone());
    let claim = ArchiveClaim {
        deployment_id: current.deployment_id.clone(),
        generation: current.generation,
    };
    let journal_before = file_sha256(&journal);

    let denied = check_archive_authority(
        &state,
        Some(&state),
        &claim,
        &[MemberFacts::Reference {
            subject_hash: subject_hash.clone(),
        }],
    );
    assert_eq!(
        denied.expect_err("a tombstone reference to the denied subject must refuse"),
        AuthorityRefusal::DeniedSubject
    );
    let unknown = check_archive_authority(&state, Some(&state), &claim, &[MemberFacts::Unknown]);
    assert_eq!(
        unknown.expect_err("unknown membership must refuse"),
        AuthorityRefusal::UnknownMember
    );
    let passed = check_archive_authority(&state, Some(&state), &claim, &[MemberFacts::None]);
    assert!(
        passed.is_ok(),
        "an empty membership must pass so an over-broad refusal fails: {passed:?}"
    );

    assert_eq!(
        file_sha256(&journal),
        journal_before,
        "the refusals left the journal authority bytes unchanged"
    );
    println!(
        "EVIDENCE pg-membership deployment={} generation={} subject_hash={} tombstones={} \
         journal_sha256={}",
        current.deployment_id,
        current.generation,
        subject_hash,
        tombstones.len(),
        journal_before,
    );
}
