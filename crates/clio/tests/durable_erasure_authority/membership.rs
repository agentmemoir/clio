// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the check API's refusals without a store:
//! the presented-authority matrix (missing, damaged, other deployment,
//! known-old) and the member-fact matrix (denied, unknown, un-denied), with
//! the on-disk journal authority proven unchanged across each refusal batch.
//!
//! ## Owns
//! - The presented-authority refusal matrix and the new-empty certification
//!   refusal
//! - The denied and unknown membership refusals and their positive controls
//!
//! ## Does not own
//! - The fixtures and probes (see `support`)
//! - The store-backed scenarios (see `sqlite`, `postgres`)
//!
//! ## Boundary
//!
//! Changes to which check refusals are measured should normally require
//! changes here. Changes to the fixtures should not.
//!
//! Keep this module focused on the check API's refusal matrix.

use crate::support::{
    BANK, Scratch, control_hash, denied_hash, file_sha256, journal_path, load_current,
    observe_journal, prepare, save_copy, subject_hash, write_owner_only,
};
use clio_compliance::{
    ArchiveClaim, AuthorityRefusal, JournalState, MemberFacts, check_archive_authority,
    initialize_new_empty, load_journal, record_denial,
};

#[test]
fn missing_damaged_other_deployment_and_known_old_authority_all_refuse() {
    let scratch = Scratch::new("membership-matrix");
    let journal = journal_path(&scratch, "journal.json");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("seed the deployment");
    // A copy of the one-denial authority: one generation behind the current
    // journal, so the check classifies it as known-old.
    let older = save_copy(
        &journal,
        &prepare(&scratch, "known-old/erasure-journal.json"),
    );
    let generation = record_denial(&journal, &subject_hash("subject-later"), &[BANK.to_owned()])
        .expect("advance the generation");
    assert_eq!(generation, 2, "the second denial advances the generation");
    let current = observe_journal("membership-matrix-current", &journal);
    let claim = ArchiveClaim {
        deployment_id: current.deployment_id.clone(),
        generation: current.generation,
        salt_fingerprint: Some(clio_compliance::tombstone_salt_fingerprint()),
    };
    let current_state = JournalState::Current(current.clone());

    // A presented authority whose bounded, owner-only bytes do not parse.
    let damaged_path = journal_path(&scratch, "damaged.json");
    write_owner_only(
        &damaged_path,
        b"{\"schema_version\":1,\"deployment_id\":\"truncated\"",
    );
    let damaged_state = load_journal(&damaged_path);
    assert_eq!(
        damaged_state,
        JournalState::Damaged,
        "the planted bytes must load as damaged"
    );

    // Another deployment's authority: a fresh journal mints its own identity.
    let other_path = journal_path(&scratch, "other/erasure-journal.json");
    record_denial(
        &other_path,
        &subject_hash("subject-other"),
        &[BANK.to_owned()],
    )
    .expect("seed another deployment");
    let other_state = load_journal(&other_path);
    assert!(
        matches!(
            &other_state,
            JournalState::Current(other) if other.deployment_id != current.deployment_id
        ),
        "the other journal must be a different deployment: {other_state:?}"
    );

    // The on-disk authority the current state above was loaded from, hashed
    // across the refusal batch below: a refused check that rewrote its own
    // authority would change these bytes.
    let journal_before = file_sha256(&journal);

    let missing = check_archive_authority(&current_state, None, &claim, &[MemberFacts::None]);
    assert_eq!(
        missing.expect_err("no presented authority must refuse"),
        AuthorityRefusal::PresentedMissing
    );
    let damaged = check_archive_authority(
        &current_state,
        Some(&damaged_state),
        &claim,
        &[MemberFacts::None],
    );
    assert_eq!(
        damaged.expect_err("a damaged presented authority must refuse"),
        AuthorityRefusal::PresentedDamaged
    );
    let other = check_archive_authority(
        &current_state,
        Some(&other_state),
        &claim,
        &[MemberFacts::None],
    );
    assert_eq!(
        other.expect_err("another deployment's authority must refuse"),
        AuthorityRefusal::PresentedOtherDeployment
    );
    let older_state = JournalState::Current(load_current(&older));
    let known_old = check_archive_authority(
        &current_state,
        Some(&older_state),
        &claim,
        &[MemberFacts::None],
    );
    assert_eq!(
        known_old.expect_err("a known-old authority must refuse"),
        AuthorityRefusal::PresentedOlderGeneration
    );

    assert_eq!(
        file_sha256(&journal),
        journal_before,
        "no refused check changed the journal bytes the authority was loaded from"
    );
    println!(
        "EVIDENCE membership-matrix deployment={} generation={} journal_sha256={} refusals=missing,damaged,other-deployment,known-old",
        current.deployment_id, current.generation, journal_before
    );
}

#[test]
fn a_new_empty_journal_refuses_to_certify_an_old_archive() {
    let scratch = Scratch::new("membership-new-empty");
    let journal = journal_path(&scratch, "journal.json");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("seed the deployment");
    let generation = record_denial(&journal, &subject_hash("subject-later"), &[BANK.to_owned()])
        .expect("advance the generation");
    assert_eq!(generation, 2, "the second denial advances the generation");
    let current = observe_journal("membership-new-empty-current", &journal);

    // A new empty journal cannot validate an old archive: its claim still
    // names the deployment that recorded the denial.
    let fresh_path = journal_path(&scratch, "fresh/erasure-journal.json");
    let fresh = initialize_new_empty(&fresh_path).expect("initialize a new empty deployment");
    assert_eq!(fresh.generation, 0, "a new empty journal certifies nothing");
    assert!(
        fresh.denials.is_empty(),
        "a new empty journal holds no denial"
    );
    let fresh_state = JournalState::Current(fresh);
    let old_claim = ArchiveClaim {
        deployment_id: current.deployment_id.clone(),
        generation: current.generation,
        salt_fingerprint: Some(clio_compliance::tombstone_salt_fingerprint()),
    };
    // The fresh journal bytes the refusal below is measured against, hashed
    // across the call: a refused certification that rewrote its authority
    // would change them.
    let fresh_before = file_sha256(&fresh_path);

    let uncertified = check_archive_authority(
        &fresh_state,
        Some(&fresh_state),
        &old_claim,
        &[MemberFacts::None],
    );
    assert_eq!(
        uncertified.expect_err("a new empty journal must not certify an old archive"),
        AuthorityRefusal::ClaimOtherDeployment
    );

    assert_eq!(
        file_sha256(&fresh_path),
        fresh_before,
        "the refused certification left the fresh journal bytes unchanged"
    );
    println!(
        "EVIDENCE membership-new-empty deployment={} generation={} journal_sha256={} refusals=claim-other-deployment",
        current.deployment_id, current.generation, fresh_before
    );
}

#[test]
fn denied_and_unknown_membership_refuse_while_un_denied_membership_passes() {
    let scratch = Scratch::new("membership-facts");
    let journal = journal_path(&scratch, "journal.json");
    record_denial(&journal, &denied_hash(), &[BANK.to_owned()]).expect("seed the deployment");
    let current = observe_journal("membership-facts-current", &journal);
    let state = JournalState::Current(current.clone());
    let claim = ArchiveClaim {
        deployment_id: current.deployment_id.clone(),
        generation: current.generation,
        salt_fingerprint: Some(clio_compliance::tombstone_salt_fingerprint()),
    };
    // The on-disk authority the state above was loaded from, hashed across the
    // refusal batch below.
    let journal_before = file_sha256(&journal);

    // The content-free subject reference a destroyed slot, a tombstone, or a
    // ciphertext-only row carries for the denied subject.
    let denied = check_archive_authority(
        &state,
        Some(&state),
        &claim,
        &[MemberFacts::Reference {
            subject_hash: denied_hash(),
        }],
    );
    assert_eq!(
        denied.expect_err("a reference to the denied subject must refuse"),
        AuthorityRefusal::DeniedSubject
    );

    // A member whose subject membership cannot be classified refuses.
    let unknown = check_archive_authority(&state, Some(&state), &claim, &[MemberFacts::Unknown]);
    assert_eq!(
        unknown.expect_err("unknown membership must refuse"),
        AuthorityRefusal::UnknownMember
    );

    // Positive controls: the same deployment with no denied references passes,
    // so an over-broad refusal would fail this suite.
    assert_eq!(
        check_archive_authority(&state, Some(&state), &claim, &[MemberFacts::None]),
        Ok(())
    );
    assert!(
        !current.is_denied(&control_hash()),
        "the control subject must not be denied"
    );
    let control = check_archive_authority(
        &state,
        Some(&state),
        &claim,
        &[MemberFacts::Reference {
            subject_hash: control_hash(),
        }],
    );
    assert!(
        control.is_ok(),
        "an un-denied reference must pass: {control:?}",
    );

    assert_eq!(
        file_sha256(&journal),
        journal_before,
        "no refusal changed the journal bytes the authority was loaded from"
    );
    println!(
        "EVIDENCE membership-facts deployment={} generation={} denied_hash={} journal_sha256={}",
        current.deployment_id,
        current.generation,
        denied_hash(),
        journal_before
    );
}
