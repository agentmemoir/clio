// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Category gate and scoring decision tests (no writes).

use super::*;
use clio_config::ranking::{AdmissionWeights, RankingEnv, RankingEnvPatch, preview_patch};
use clio_config::retention::{
    DuplicateTolerance, RETENTION_PROFILE_VERSION, RetentionProfile, Verbosity,
};
use clio_types::{EpisodicType, ErrorCode, ItemKind, SemanticCategory, SourceType};

fn strong_signals() -> SignalInputs {
    SignalInputs {
        text: "user decision: must use concise output".into(),
        source_type: Some(SourceType::UserStated),
        neighbors: Some(vec![]),
    }
}

fn weak_signals() -> SignalInputs {
    SignalInputs {
        text: "hi".into(),
        source_type: None,
        neighbors: Some(vec![String::from("hi")]),
    }
}

#[test]
fn semantic_admit_and_reject() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let ok = decide(
        Candidate::Semantic(SemanticCategory::Persona),
        &strong_signals(),
        &policy,
    )
    .expect("score");
    assert!(ok.pass);
    assert!(ok.rejection_reason.is_none());

    let no = decide(
        Candidate::Semantic(SemanticCategory::Persona),
        &weak_signals(),
        &policy,
    )
    .expect("score");
    assert!(!no.pass);
    assert!(no.rejection_reason.is_some());
}

#[test]
fn raw_decide_paths() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let ok = decide_raw("semantic", "persona", &strong_signals(), &policy).expect("valid");
    assert!(ok.pass);
    let bad = decide_raw("semantic", "brain_dump", &strong_signals(), &policy).expect("reject");
    assert!(!bad.pass);
    let no = admit_preview_raw("episodic", "note", &strong_signals(), &policy).expect("reject");
    assert!(!no.pass);
    let err = decide_raw("archive", "persona", &strong_signals(), &policy).expect_err("bad kind");
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn preview_matches_decide_without_side_effects() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let candidate = Candidate::Episodic(EpisodicType::Task);
    let preview = admit_preview(candidate, &strong_signals(), &policy).expect("preview");
    let direct = decide(candidate, &strong_signals(), &policy).expect("decide");
    assert_eq!(preview, direct);
    let raw = admit_preview_raw("episodic", "task", &strong_signals(), &policy).expect("raw");
    assert_eq!(preview, raw);
}

#[test]
fn decisions_are_deterministic() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let a = decide(
        Candidate::Semantic(SemanticCategory::Schema),
        &strong_signals(),
        &policy,
    )
    .expect("first");
    let b = decide(
        Candidate::Semantic(SemanticCategory::Schema),
        &strong_signals(),
        &policy,
    )
    .expect("second");
    assert_eq!(a, b);
}

#[test]
fn novelty_backend_down_errors() {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::new(&env);
    let mut signals = strong_signals();
    signals.neighbors = None;
    let err = decide(
        Candidate::Semantic(SemanticCategory::TaskSpec),
        &signals,
        &policy,
    )
    .expect_err("novelty down");
    assert_eq!(err.code, ErrorCode::FactorUnavailable);
}

#[test]
fn weight_changes_move_scores_deterministically() {
    let base = RankingEnv::system_defaults();
    let tilted = preview_patch(
        &base,
        &RankingEnvPatch {
            admission: Some(AdmissionWeights {
                w1: 10.0,
                w2: 1.0,
                w3: 1.0,
                w4: 1.0,
            }),
            ..RankingEnvPatch::default()
        },
    )
    .expect("tilt");
    let a = decide(
        Candidate::Semantic(SemanticCategory::ToolConfig),
        &weak_signals(),
        &AdmissionPolicy::new(&base),
    )
    .expect("base");
    let b = decide(
        Candidate::Semantic(SemanticCategory::ToolConfig),
        &weak_signals(),
        &AdmissionPolicy::new(&tilted),
    )
    .expect("tilted");
    assert!((a.admission_score - b.admission_score).abs() > 1e-12);
}

#[test]
fn candidate_parse_helpers() {
    assert_eq!(
        Candidate::parse_semantic("persona").expect("parse"),
        Candidate::Semantic(SemanticCategory::Persona)
    );
    assert_eq!(
        Candidate::parse_episodic("task").expect("parse"),
        Candidate::Episodic(EpisodicType::Task)
    );
    assert!(Candidate::parse_semantic("nope").is_err());
    assert!(Candidate::parse_episodic("nope").is_err());
    assert_eq!(
        Candidate::Semantic(SemanticCategory::Schema).kind(),
        ItemKind::Semantic
    );
    assert_eq!(Candidate::Episodic(EpisodicType::Gist).key(), "gist");
}

fn policy_with<'a>(env: &'a RankingEnv, profile: &RetentionProfile) -> AdmissionPolicy<'a> {
    AdmissionPolicy::with_retention(env, profile.resolve())
}

fn routine_fixture() -> Vec<SignalInputs> {
    let shared = String::from("routine status item 0");
    (0..20)
        .map(|i| SignalInputs {
            text: if i % 2 == 0 {
                format!("user decision: routine status item {i}")
            } else {
                format!("routine status item {i}")
            },
            source_type: None,
            neighbors: Some(if i % 5 == 0 {
                vec![]
            } else {
                vec![shared.clone()]
            }),
        })
        .collect()
}

#[test]
fn verbosity_admit_sets_are_monotone_on_a_shared_fixture() {
    let env = RankingEnv::system_defaults();
    let selective = RetentionProfile {
        verbosity: Verbosity::Selective,
        ..RetentionProfile::default()
    };
    let balanced = RetentionProfile::default();
    let permissive = RetentionProfile {
        verbosity: Verbosity::Permissive,
        ..RetentionProfile::default()
    };
    let candidate = Candidate::Episodic(EpisodicType::Gist);
    let score = |policy: &AdmissionPolicy<'_>, signals: &SignalInputs| {
        let decision = decide(candidate, signals, policy).expect("score");
        assert!(
            decision.pass || decision.rejection_reason.is_some(),
            "every rejection carries a reason"
        );
        assert_eq!(decision.profile_version, RETENTION_PROFILE_VERSION);
        decision.pass
    };
    let sel_policy = policy_with(&env, &selective);
    let bal_policy = policy_with(&env, &balanced);
    let per_policy = policy_with(&env, &permissive);
    let fixture = routine_fixture();
    let sel: Vec<bool> = fixture.iter().map(|s| score(&sel_policy, s)).collect();
    let bal: Vec<bool> = fixture.iter().map(|s| score(&bal_policy, s)).collect();
    let per: Vec<bool> = fixture.iter().map(|s| score(&per_policy, s)).collect();

    let count = |v: &[bool]| v.iter().filter(|p| **p).count();
    assert!(count(&sel) < count(&bal), "selective must admit fewest");
    assert!(count(&bal) <= count(&per));
    for i in 0..fixture.len() {
        assert!(!sel[i] || bal[i], "selective ⊆ balanced at item {i}");
        assert!(!bal[i] || per[i], "balanced ⊆ permissive at item {i}");
    }
    assert_eq!(count(&bal), 20);
}

#[test]
fn strict_duplicate_tolerance_rejects_a_high_scoring_near_duplicate() {
    let env = RankingEnv::system_defaults();
    // Shares five of six tokens with the candidate, so novelty is 0.14: above
    // the default floor of 0.0 and below the strict floor of 0.25.
    let near = "user decision must use concise output notes";
    let signals = SignalInputs {
        text: "user decision: must use concise output".into(),
        source_type: Some(SourceType::UserStated),
        neighbors: Some(vec![near.into()]),
    };
    let candidate = Candidate::Semantic(SemanticCategory::Persona);
    let balanced = policy_with(&env, &RetentionProfile::default());
    let admitted = decide(candidate, &signals, &balanced).expect("balanced");
    assert!(
        admitted.pass,
        "the default floor of 0.0 admits a near-duplicate"
    );

    let strict = policy_with(
        &env,
        &RetentionProfile {
            duplicate_tolerance_write: DuplicateTolerance::Strict,
            ..RetentionProfile::default()
        },
    );
    let rejected = decide(candidate, &signals, &strict).expect("strict");
    assert!(!rejected.pass);
    assert!(
        rejected
            .rejection_reason
            .as_deref()
            .expect("reason")
            .contains("duplicate tolerance"),
        "{:?}",
        rejected.rejection_reason
    );
    assert!(rejected.admission_score > strict.threshold_for("persona"));
}

#[test]
fn an_exact_duplicate_is_rejected_even_under_the_default_profile() {
    // This is the regression this rule exists for: the default floor of 0.0
    // used to admit an identical re-write, because 0.0 is not below 0.0.
    let env = RankingEnv::system_defaults();
    let signals = SignalInputs {
        text: "user decision: must use concise output".into(),
        source_type: Some(SourceType::UserStated),
        neighbors: Some(vec!["user decision: must use concise output".into()]),
    };
    let decision = decide(
        Candidate::Semantic(SemanticCategory::Persona),
        &signals,
        &policy_with(&env, &RetentionProfile::default()),
    )
    .expect("balanced");

    assert!(!decision.pass, "an identical write must not be admitted");
    assert!(
        decision
            .rejection_reason
            .as_deref()
            .expect("reason")
            .starts_with("exact_duplicate:"),
        "{:?}",
        decision.rejection_reason
    );
}
