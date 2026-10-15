// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for measuring the guarded-restore authority
//! re-check under the journal lock through its public API: the refusal matrix
//! for a missing, damaged, other-deployment, known-old, and newer-claim
//! authority and for an unknown member, and a denial injected between the
//! plan-time check and the locked check.
//!
//! ## Owns
//! - The locked-check refusal matrix and the injected-denial scenario
//!
//! ## Does not own
//! - The guard sequence (`clio-compliance` owns `begin_restore`)
//! - The fixtures (see `restore_guard_fixtures`) or the recovery scenarios
//!   (see `restore_guard_recovery`)
//!
//! ## Boundary
//!
//! Changes to which locked-check refusals are measured should normally require
//! changes here. Changes to the guard sequence should not.
//!
//! Keep this module focused on the locked-check refusals.

use crate::restore_guard_fixtures::{claim_of, event_for, events_store, request, target};
use crate::support::{
    BANK, Scratch, denied_hash, file_sha256, journal_path, load_current, save_copy, subject_hash,
    write_owner_only,
};
use clio::store::{LocalEventState, LocalEventStore};
use clio_compliance::{
    ArchiveClaim, AuthorityRefusal, JournalState, MemberFacts, RestoreGuardRefusal, begin_restore,
    initialize_new_empty, record_denial,
};
use std::path::Path;

/// The plan-time helper: the failure-closed check passes before the denial.
fn require_authority_at_plan(journal: &Path, claim: &ArchiveClaim, members: &[MemberFacts]) {
    clio_compliance::check_restore_at_plan(journal, None, claim, members)
        .expect("the archive passes the plan-time check");
}

#[test]
fn a_denial_between_plan_and_placement_is_caught_by_the_locked_check() {
    let scratch = Scratch::new("guard-api-inject");
    let journal = journal_path(&scratch, "journal.json");
    // The archive is valid at plan time: it references DENIED_SUBJECT, which
    // is not yet denied; only an unrelated subject is.
    record_denial(
        &journal,
        &subject_hash("subject-unrelated"),
        &[BANK.to_owned()],
    )
    .expect("record the unrelated denial");
    let claim = claim_of(&journal);
    let member = MemberFacts::Reference {
        subject_hash: denied_hash(),
    };
    require_authority_at_plan(&journal, &claim, std::slice::from_ref(&member));

    // The denial lands after the plan-time check and before placement.
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("inject the denial");
    let journal_after = file_sha256(&journal);

    let (target_db, _kms) = target(&scratch, "target");
    let target_before = file_sha256(&target_db);
    let store = events_store(&scratch, "events");
    let Err(refusal) = begin_restore(
        &store,
        &request(&journal, &claim, &[member], None, "restore-injected"),
    ) else {
        panic!("the locked check must catch the injected denial");
    };
    assert!(
        matches!(
            refusal,
            RestoreGuardRefusal::Authority(AuthorityRefusal::DeniedSubject)
        ),
        "the refusal is the denied subject: {refusal:?}"
    );
    assert_eq!(
        file_sha256(&target_db),
        target_before,
        "the refused locked check placed no bytes"
    );
    assert_eq!(
        file_sha256(&journal),
        journal_after,
        "the refusal left the journal authority unchanged"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "a refused locked check leaves no pending action"
    );
    let events = event_for(&store, "restore-injected");
    assert_eq!(events.len(), 1, "one terminal event for the refusal");
    assert_eq!(events[0].state, LocalEventState::Denied);
    println!(
        "REFUSED guard-api injected-denial deployment={} journal_sha256={journal_after}",
        claim.deployment_id
    );
}

#[test]
fn the_locked_check_refuses_missing_damaged_and_other_deployment_authority() {
    let scratch = Scratch::new("guard-api-matrix");
    let store = events_store(&scratch, "events");
    let journal = journal_path(&scratch, "journal.json");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record a denial");
    let claim = claim_of(&journal);
    let none = [MemberFacts::None];

    // The live journal is missing.
    let missing_journal = journal_path(&scratch, "no-such-journal.json");
    let Err(refusal) = begin_restore(
        &store,
        &request(
            &missing_journal,
            &claim,
            &none,
            None,
            "case-current-missing",
        ),
    ) else {
        panic!("a missing live journal refuses");
    };
    assert!(
        matches!(
            refusal,
            RestoreGuardRefusal::Authority(AuthorityRefusal::CurrentMissing)
        ),
        "current missing: {refusal:?}"
    );

    // The live journal is damaged.
    let damaged_journal = journal_path(&scratch, "damaged.json");
    write_owner_only(&damaged_journal, b"{\"schema_version\":1");
    let Err(refusal) = begin_restore(
        &store,
        &request(
            &damaged_journal,
            &claim,
            &none,
            None,
            "case-current-damaged",
        ),
    ) else {
        panic!("a damaged live journal refuses");
    };
    assert!(
        matches!(
            refusal,
            RestoreGuardRefusal::Authority(AuthorityRefusal::CurrentDamaged)
        ),
        "current damaged: {refusal:?}"
    );

    // The archive claims another deployment.
    let other_claim = ArchiveClaim {
        deployment_id: "another-deployment".to_owned(),
        generation: 0,
        salt_fingerprint: Some(clio_compliance::tombstone_salt_fingerprint()),
    };
    let Err(refusal) = begin_restore(
        &store,
        &request(&journal, &other_claim, &none, None, "case-other-deployment"),
    ) else {
        panic!("another deployment's claim refuses");
    };
    assert!(
        matches!(
            refusal,
            RestoreGuardRefusal::Authority(AuthorityRefusal::ClaimOtherDeployment)
        ),
        "other deployment: {refusal:?}"
    );

    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "every refused locked check settled its intent"
    );
    println!(
        "REFUSED guard-api locked-check cases=current-missing,current-damaged,other-deployment"
    );
}

#[test]
fn a_known_old_presented_authority_and_a_newer_claim_both_refuse_at_the_lock() {
    let scratch = Scratch::new("guard-api-older");
    let store = events_store(&scratch, "events");
    let journal = journal_path(&scratch, "journal.json");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record a denial");
    let older = save_copy(
        &journal,
        &crate::support::prepare(&scratch, "older/journal.json"),
    );
    // Advance the live authority so the saved copy is exactly one generation
    // known-old.
    record_denial(&journal, &subject_hash("subject-later"), &[BANK.to_owned()])
        .expect("advance the generation");
    let current = load_current(&journal);
    let claim = ArchiveClaim {
        deployment_id: current.deployment_id.clone(),
        generation: current.generation,
        salt_fingerprint: Some(clio_compliance::tombstone_salt_fingerprint()),
    };
    let none = [MemberFacts::None];

    // A known-old transferred authority is presented.
    let older_state = JournalState::Current(load_current(&older));
    let Err(refusal) = begin_restore(
        &store,
        &request(
            &journal,
            &claim,
            &none,
            Some(&older_state),
            "case-older-presented",
        ),
    ) else {
        panic!("a known-old presented authority refuses");
    };
    assert!(
        matches!(
            refusal,
            RestoreGuardRefusal::Authority(AuthorityRefusal::PresentedOlderGeneration)
        ),
        "older presented: {refusal:?}"
    );

    // The archive claims a generation newer than the live authority's.
    let newer_claim = ArchiveClaim {
        deployment_id: claim.deployment_id.clone(),
        generation: claim.generation + 1,
        salt_fingerprint: Some(clio_compliance::tombstone_salt_fingerprint()),
    };
    let Err(refusal) = begin_restore(
        &store,
        &request(&journal, &newer_claim, &none, None, "case-newer-claim"),
    ) else {
        panic!("a newer claim refuses");
    };
    assert!(
        matches!(
            refusal,
            RestoreGuardRefusal::Authority(AuthorityRefusal::ClaimNewerGeneration)
        ),
        "newer claim: {refusal:?}"
    );

    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "every refused locked check settled its intent"
    );
    println!("REFUSED guard-api locked-check cases=older-presented,newer-claim");
}

#[test]
fn an_unknown_member_refuses_and_a_new_empty_journal_cannot_certify_an_old_archive() {
    let scratch = Scratch::new("guard-api-unknown");
    let store = events_store(&scratch, "events");
    let journal = journal_path(&scratch, "journal.json");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("record a denial");
    let claim = claim_of(&journal);

    let Err(refusal) = begin_restore(
        &store,
        &request(
            &journal,
            &claim,
            &[MemberFacts::Unknown],
            None,
            "case-unknown",
        ),
    ) else {
        panic!("an unknown member refuses");
    };
    assert!(
        matches!(
            refusal,
            RestoreGuardRefusal::Authority(AuthorityRefusal::UnknownMember)
        ),
        "unknown member: {refusal:?}"
    );

    // A new empty authority cannot validate the old archive: its deployment is
    // not the archive's.
    let fresh_path = journal_path(&scratch, "fresh/journal.json");
    let fresh = initialize_new_empty(&fresh_path).expect("initialize a new empty deployment");
    let fresh_state = JournalState::Current(fresh);
    let Err(refusal) = begin_restore(
        &store,
        &request(
            &fresh_path,
            &claim,
            &[MemberFacts::None],
            Some(&fresh_state),
            "case-new-empty",
        ),
    ) else {
        panic!("a new empty authority must not certify an old archive");
    };
    assert!(
        matches!(
            refusal,
            RestoreGuardRefusal::Authority(AuthorityRefusal::ClaimOtherDeployment)
        ),
        "new empty cannot certify: {refusal:?}"
    );
    assert!(
        store.pending_actions().expect("pending").is_empty(),
        "every refusal settled its intent"
    );
    println!("REFUSED guard-api unknown-member,new-empty-cannot-certify");
}
