// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the archive authority check: every
//! refusal in check order and the one passing case.
//!
//! ## Owns
//! - The in-memory journal fixtures and the check refusal assertions
//!
//! ## Does not own
//! - The journal model and operations tests (`erasure_journal_tests.rs`)
//! - The check itself (`archive_authority`)
//!
//! ## Boundary
//!
//! Tests here break when the archive authority check changes.
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the archive authority check.

use super::super::{DenialRecord, ErasureJournal, JournalState, SCHEMA_VERSION};
use super::{ArchiveClaim, AuthorityRefusal, MemberFacts, check_archive_authority};
use clio_types::ErrorCode;

/// An in-memory current state for the check tests.
fn current_of(deployment: &str, generation: u64, denied: &[&str]) -> JournalState {
    JournalState::Current(ErasureJournal {
        schema_version: SCHEMA_VERSION,
        deployment_id: deployment.to_owned(),
        generation,
        salt_fingerprint: Some("fp-live".to_owned()),
        denials: denied
            .iter()
            .map(|subject| DenialRecord {
                subject_hash: (*subject).to_owned(),
                banks: Vec::new(),
            })
            .collect(),
    })
}

/// An archive claim for the check tests.
fn claim_of(deployment: &str, generation: u64) -> ArchiveClaim {
    ArchiveClaim {
        deployment_id: deployment.to_owned(),
        generation,
        salt_fingerprint: Some("fp-live".to_owned()),
    }
}

/// A content-free subject reference member.
fn reference(subject: &str) -> MemberFacts {
    MemberFacts::Reference {
        subject_hash: subject.to_owned(),
    }
}

/// A member carrying usable key material for a subject.
fn live_key(subject: &str) -> MemberFacts {
    MemberFacts::LiveKey {
        subject_hash: subject.to_owned(),
    }
}

/// A member whose archive carries no key store to prove it content-free.
fn unproven_key(subject: &str) -> MemberFacts {
    MemberFacts::UnprovenKey {
        subject_hash: subject.to_owned(),
    }
}

/// Run the check and require a refusal.
fn refuse(
    current: &JournalState,
    presented: Option<&JournalState>,
    claim: &ArchiveClaim,
) -> AuthorityRefusal {
    check_archive_authority(current, presented, claim, &[]).expect_err("the check must refuse")
}

/// Run the check against `current` as the presented authority, with a
/// deployment-a claim and `members`.
fn check_members(current: &JournalState, members: &[MemberFacts]) -> Result<(), AuthorityRefusal> {
    let claim = claim_of("deployment-a", 0);
    check_archive_authority(current, Some(current), &claim, members)
}
#[test]
fn check_refuses_an_absent_or_damaged_current_authority() {
    let claim = claim_of("deployment-a", 0);
    let missing = refuse(&JournalState::Missing, None, &claim);
    assert_eq!(missing, AuthorityRefusal::CurrentMissing);
    let damaged = refuse(&JournalState::Damaged, None, &claim);
    assert_eq!(damaged, AuthorityRefusal::CurrentDamaged);
}

#[test]
fn check_refuses_a_missing_or_damaged_presented_authority() {
    let current = current_of("deployment-a", 1, &[]);
    let claim = claim_of("deployment-a", 0);
    let absent = refuse(&current, None, &claim);
    assert_eq!(absent, AuthorityRefusal::PresentedMissing);
    let damaged = refuse(&current, Some(&JournalState::Damaged), &claim);
    assert_eq!(damaged, AuthorityRefusal::PresentedDamaged);
}

#[test]
fn check_refuses_an_authority_from_another_deployment() {
    let current = current_of("deployment-a", 1, &[]);
    let presented = current_of("deployment-b", 1, &[]);
    let refusal = refuse(&current, Some(&presented), &claim_of("deployment-a", 0));
    assert_eq!(refusal, AuthorityRefusal::PresentedOtherDeployment);
}

#[test]
fn check_refuses_a_known_older_presented_authority() {
    let current = current_of("deployment-a", 2, &[]);
    let older = current_of("deployment-a", 1, &[]);
    let refusal = refuse(&current, Some(&older), &claim_of("deployment-a", 1));
    assert_eq!(refusal, AuthorityRefusal::PresentedOlderGeneration);
}

#[test]
fn check_refuses_a_presented_authority_ahead_of_the_live_one() {
    let current = current_of("deployment-a", 1, &[]);
    let ahead = current_of("deployment-a", 2, &[]);
    let refusal = refuse(&current, Some(&ahead), &claim_of("deployment-a", 1));
    assert_eq!(refusal, AuthorityRefusal::PresentedNewerGeneration);
}

#[test]
fn check_refuses_a_fresh_journal_for_an_archive_of_another_deployment() {
    let fresh = current_of("fresh-deployment", 0, &[]);
    let claim = claim_of("old-deployment", 1);
    let refusal = refuse(&fresh, Some(&fresh), &claim);
    assert_eq!(refusal, AuthorityRefusal::ClaimOtherDeployment);
}

#[test]
fn check_refuses_a_claim_from_a_newer_generation() {
    let current = current_of("deployment-a", 1, &[]);
    let claim = claim_of("deployment-a", 2);
    let refusal = refuse(&current, Some(&current), &claim);
    assert_eq!(refusal, AuthorityRefusal::ClaimNewerGeneration);
}

#[test]
fn check_refuses_a_salt_fingerprint_mismatch() {
    let current = current_of("deployment-a", 1, &[]);
    let claim = ArchiveClaim {
        deployment_id: "deployment-a".to_owned(),
        generation: 1,
        salt_fingerprint: Some("other-fingerprint".to_owned()),
    };
    let refusal = refuse(&current, Some(&current), &claim);
    assert_eq!(refusal, AuthorityRefusal::SaltFingerprintMismatch);
}

#[test]
fn check_refuses_an_absent_salt_fingerprint() {
    // A missing fingerprint on the archive fails closed, even when the live
    // authority carries one.
    let current = current_of("deployment-a", 1, &[]);
    let claim = ArchiveClaim {
        deployment_id: "deployment-a".to_owned(),
        generation: 1,
        salt_fingerprint: None,
    };
    let refusal = refuse(&current, Some(&current), &claim);
    assert_eq!(refusal, AuthorityRefusal::SaltFingerprintMismatch);
}

#[test]
fn check_refuses_a_live_key_for_a_denied_subject() {
    let current = current_of("deployment-a", 1, &["subject-a"]);
    let members = [MemberFacts::None, live_key("subject-a")];
    let refusal = check_members(&current, &members);
    assert_eq!(refusal, Err(AuthorityRefusal::DeniedSubject));
}

#[test]
fn check_allows_a_content_free_reference_to_a_denied_subject() {
    // A destroyed key slot, a tombstone, or a ciphertext row without a usable
    // DEK cannot make the erased content readable again, so a backup taken
    // after the erasure restores.
    let current = current_of("deployment-a", 1, &["subject-a"]);
    let members = [MemberFacts::None, reference("subject-a")];
    let outcome = check_members(&current, &members);
    assert_eq!(outcome, Ok(()));
}

#[test]
fn check_refuses_an_unproven_reference_to_a_denied_subject() {
    // Without a key member the restore cannot show the reference unreadable, so
    // the archive refuses even though the record it carries is content-free.
    let current = current_of("deployment-a", 1, &["subject-a"]);
    let members = [MemberFacts::None, unproven_key("subject-a")];
    let refusal = check_members(&current, &members);
    assert_eq!(refusal, Err(AuthorityRefusal::UnprovenReference));
}

#[test]
fn check_passes_an_unproven_reference_to_an_un_denied_subject() {
    let current = current_of("deployment-a", 1, &["subject-a"]);
    let members = [unproven_key("subject-b")];
    let outcome = check_members(&current, &members);
    assert_eq!(outcome, Ok(()));
}

#[test]
fn check_refuses_an_unknown_member() {
    let current = current_of("deployment-a", 1, &[]);
    let refusal = check_members(&current, &[MemberFacts::Unknown]);
    assert_eq!(refusal, Err(AuthorityRefusal::UnknownMember));
}

#[test]
fn check_passes_a_same_deployment_archive_without_denied_references() {
    let current = current_of("deployment-a", 2, &["subject-a"]);
    let members = [MemberFacts::None, reference("subject-b")];
    let outcome = check_members(&current, &members);
    assert_eq!(outcome, Ok(()), "only proved absence passes");
    let JournalState::Current(journal) = &current else {
        panic!("expected a current journal");
    };
    assert!(!journal.is_denied("subject-b"), "still readable");
}

/// Every refusal's code and rendered text, in variant order: (refusal, code, text).
#[rustfmt::skip]
#[test]
fn authority_refusal_codes_and_texts_are_stable() {
    use AuthorityRefusal as R;
    use ErrorCode as E;
    for (refusal, code, text) in [
        (R::CurrentMissing, E::NotFound, "the erasure journal is missing"),
        (R::CurrentDamaged, E::ConfigCorrupt, "the erasure journal is damaged"),
        (R::PresentedMissing, E::NotFound, "no authority was presented with the archive"),
        (R::PresentedDamaged, E::ConfigCorrupt, "the presented authority is damaged"),
        (R::PresentedOtherDeployment, E::Forbidden, "the presented authority is another deployment's"),
        (R::PresentedOlderGeneration, E::Forbidden, "the presented authority is older than the live one"),
        (R::PresentedNewerGeneration, E::Forbidden, "the presented authority is newer than the live one"),
        (R::ClaimOtherDeployment, E::Forbidden, "the archive claims another deployment"),
        (R::ClaimNewerGeneration, E::Forbidden, "the archive claims a generation newer than the live authority's"),
        (R::SaltFingerprintMismatch, E::Forbidden, "the archive's subject-salt fingerprint does not match the live authority's"),
        (R::UnknownMember, E::ConfigCorrupt, "an archive member cannot be classified"),
        (R::DeniedSubject, E::Forbidden, "an archive member carries usable key material for a currently denied subject"),
        (R::UnprovenReference, E::Forbidden, "an archive member references a currently denied subject and the archive carries no key store to prove it unreadable"),
    ] {
        assert_eq!(refusal.code(), code, "{refusal:?}");
        assert_eq!(refusal.to_string(), text, "{refusal:?}");
    }
}
