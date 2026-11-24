// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//!
//! # Responsibility
//!
//! Tests that an exact duplicate is rejected whatever the retention profile
//! says.
//!
//! ## Owns
//! - The unconditional exact-duplicate rejection in `decide_ungated`
//! - The novelty floor that still rejects after the duplicate check
//!
//! ## Does not own
//! - Novelty arithmetic (see `factors_tests`)
//! - Read-time duplicate collapse (see `clio-retrieve`)
//!
//! ## Boundary
//!
//! Changes to duplicate rejection should normally require changes here.

use super::*;
use clio_config::ranking::RankingEnv;
use clio_config::retention::{DuplicateTolerance, RetentionProfile, Verbosity};
use clio_types::SemanticCategory;

/// The text under test, kept long enough to clear the admission threshold on
/// its own so each assertion isolates the duplicate rule.
const FACT: &str = "user decision: the team prefers concise output in every review";

fn signals_with_neighbors(neighbors: &[&str]) -> SignalInputs {
    SignalInputs {
        text: FACT.to_owned(),
        source_type: None,
        neighbors: Some(neighbors.iter().map(|text| (*text).to_owned()).collect()),
    }
}

/// The shipped default profile: verbosity `balanced`, tolerance `balanced`.
fn default_policy(env: &RankingEnv) -> AdmissionPolicy<'_> {
    AdmissionPolicy::new(env)
}

#[test]
fn an_exact_duplicate_is_rejected_under_the_default_profile() {
    let env = RankingEnv::system_defaults();
    let policy = default_policy(&env);
    // The default floor is 0.0, so this rejection cannot come from the floor.
    assert!((policy.novelty_floor() - 0.0).abs() < 1e-12);

    let decision = decide(
        Candidate::Semantic(SemanticCategory::Persona),
        &signals_with_neighbors(&[FACT]),
        &policy,
    )
    .expect("score");

    assert!(!decision.pass, "an identical write must not be admitted");
    let reason = decision.rejection_reason.expect("reason");
    assert!(is_exact_duplicate_reason(&reason), "{reason}");
}

#[test]
fn the_rejection_survives_every_tolerance_and_verbosity_level() {
    let env = RankingEnv::system_defaults();
    for verbosity in [
        Verbosity::Permissive,
        Verbosity::Balanced,
        Verbosity::Selective,
    ] {
        for tolerance in [
            DuplicateTolerance::Lenient,
            DuplicateTolerance::Balanced,
            DuplicateTolerance::Strict,
        ] {
            let profile = RetentionProfile {
                verbosity,
                duplicate_tolerance_write: tolerance,
                ..RetentionProfile::default()
            };
            let policy = AdmissionPolicy::with_retention(&env, profile.resolve());
            let decision = decide(
                Candidate::Semantic(SemanticCategory::Persona),
                &signals_with_neighbors(&[FACT]),
                &policy,
            )
            .expect("score");
            assert!(
                !decision.pass
                    && is_exact_duplicate_reason(
                        decision.rejection_reason.as_deref().unwrap_or_default()
                    ),
                "{verbosity:?}/{tolerance:?} admitted an identical write"
            );
        }
    }
}

#[test]
fn a_near_duplicate_is_still_admitted_under_the_default_profile() {
    let env = RankingEnv::system_defaults();
    let policy = default_policy(&env);
    let near = "user decision: the team prefers concise output in every code review";

    let decision = decide(
        Candidate::Semantic(SemanticCategory::Persona),
        &signals_with_neighbors(&[near]),
        &policy,
    )
    .expect("score");

    assert!(decision.pass, "a near-duplicate is not an exact duplicate");
}

#[test]
fn a_first_write_with_no_neighbors_is_admitted() {
    let env = RankingEnv::system_defaults();
    let policy = default_policy(&env);

    let decision = decide(
        Candidate::Semantic(SemanticCategory::Persona),
        &signals_with_neighbors(&[]),
        &policy,
    )
    .expect("score");

    assert!(decision.pass);
}

#[test]
fn case_and_punctuation_variants_are_exact_duplicates() {
    let env = RankingEnv::system_defaults();
    let policy = default_policy(&env);
    let shouty = "USER DECISION: the team prefers concise output in every review!!!";

    let decision = decide(
        Candidate::Semantic(SemanticCategory::Persona),
        &signals_with_neighbors(&[shouty]),
        &policy,
    )
    .expect("score");

    assert!(!decision.pass, "token-set equality is the duplicate test");
}

#[test]
fn a_strict_bank_rejects_a_near_duplicate_at_the_floor() {
    let env = RankingEnv::system_defaults();
    let strict = RetentionProfile {
        verbosity: Verbosity::Balanced,
        duplicate_tolerance_write: DuplicateTolerance::Strict,
        ..RetentionProfile::default()
    };
    let policy = AdmissionPolicy::with_retention(&env, strict.resolve());
    let near = "user decision: the team prefers concise output in every code review";

    let decision = decide(
        Candidate::Semantic(SemanticCategory::Persona),
        &signals_with_neighbors(&[near]),
        &policy,
    )
    .expect("score");

    assert!(!decision.pass, "a near duplicate meets the strict floor");
    let reason = decision.rejection_reason.expect("reason");
    assert!(!is_exact_duplicate_reason(&reason), "{reason}");
    assert!(reason.contains("novelty"), "{reason}");
}
