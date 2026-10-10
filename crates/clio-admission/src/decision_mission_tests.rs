// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for tests covering the retention mission's
//! deterministic influence on admission decisions and the chatter calibration.
//!
//! ## Owns
//! - Drop-veto / keep-bonus arithmetic assertions on the five-factor path
//! - Gate-order guarantees (mission never rescues a gate) and drop-wins rule
//! - The standard chatter-fixture calibration split and factor distribution
//!
//! ## Does not own
//! - Mission schema validation and pattern matching (see `clio-config`)
//! - Preview tool behavior (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to mission arithmetic or the calibration fixture should normally
//! require changes here. Changes to matching internals should not.
//!
//! Keep this module focused on mission influence on decisions.

use crate::decision::{SignalInputs, decide_raw};
use crate::policy::AdmissionPolicy;
use clio_config::coding_default::{CHATTER_FIXTURE, coding_default_profile};
use clio_config::ranking::RankingEnv;
use clio_config::retention::RetentionProfile;
use clio_config::retention_mission::{MissionEntry, MissionMatchKind, RetentionMission};
use clio_types::SourceType;

/// Neutral scoring inputs: no neighbors (novelty 1.0), no prior (recency 1.0),
/// user-stated provenance (confidence 0.9).
fn signals(text: &str) -> SignalInputs {
    SignalInputs {
        text: text.to_owned(),
        source_type: Some(SourceType::UserStated),
        neighbors: Some(Vec::new()),
        prior_age_days: None,
        recency_unavailable: false,
    }
}

fn decide_with(profile: &RetentionProfile, text: &str) -> crate::decision::Decision {
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::with_retention(&env, profile.resolve());
    decide_raw("episodic", "gist", &signals(text), &policy).expect("decision")
}

/// The coding default's shape (selective + strict) with NO mission.
fn coding_default_without_mission() -> RetentionProfile {
    let mut profile = coding_default_profile();
    profile.mission = None;
    profile
}

fn mission_with(keep: &[&str], drop: &[(bool, &str)]) -> RetentionMission {
    RetentionMission {
        mission_statement: "Keep durable outcomes; drop routine status chatter.".to_owned(),
        keep_examples: keep
            .iter()
            .map(|text| MissionEntry {
                text: (*text).to_owned(),
                match_kind: MissionMatchKind::Literal,
                reject: false,
            })
            .collect(),
        drop_examples: drop
            .iter()
            .map(|(reject, text)| MissionEntry {
                text: (*text).to_owned(),
                match_kind: MissionMatchKind::Literal,
                reject: *reject,
            })
            .collect(),
    }
}

/// publish the per-item factor distribution and confirm the
/// 4-admit/16-reject split holds with margin under the coding default, and
/// that an empty mission with identical verbosity admits more.
#[test]
fn chatter_fixture_calibrates_under_coding_default() {
    let coding = coding_default_profile();
    let empty = coding_default_without_mission();
    let mut admitted_under_default = 0usize;
    let mut rejected_with_mission_drop = 0usize;
    let mut admitted_under_empty = 0usize;
    for (index, line) in CHATTER_FIXTURE.iter().enumerate() {
        let with_mission = decide_with(&coding, line);
        let without_mission = decide_with(&empty, line);
        println!(
            "fixture[{index:02}] pass(default)={:>5} score={:.3} utility={:.2} confidence={:.2} novelty={:.2} recency={:.2} prior={:.2} reason={:?} | pass(empty)={}",
            with_mission.pass,
            with_mission.admission_score,
            with_mission.factors.utility,
            with_mission.factors.confidence,
            with_mission.factors.novelty,
            with_mission.factors.recency,
            with_mission.factors.type_prior,
            with_mission.rejection_reason,
            without_mission.pass,
        );
        if with_mission.pass {
            admitted_under_default += 1;
        } else if with_mission
            .rejection_reason
            .as_deref()
            .is_some_and(|r| r.starts_with("mission_drop:"))
        {
            rejected_with_mission_drop += 1;
        }
        if without_mission.pass {
            admitted_under_empty += 1;
        }
    }
    assert_eq!(admitted_under_default, 4, "4 durable lines admitted");
    assert_eq!(rejected_with_mission_drop, 16, "16 chatter lines vetoed");
    assert!(
        admitted_under_empty > admitted_under_default,
        "empty mission admits more"
    );
    // Calibration margin: every admitted durable line clears the effective
    // threshold by at least 0.10.
    let policy_theta = 0.55 + 0.08;
    for line in CHATTER_FIXTURE.iter().take(4) {
        let decision = decide_with(&coding, line);
        assert!(decision.pass, "{line} must admit");
        assert!(
            decision.admission_score - policy_theta >= 0.10,
            "{line} margin"
        );
    }
    // Expected result: `Tests passed` is admitted under an empty
    // mission with identical verbosity, all else equal.
    let tests_passed = decide_with(&empty, "Tests passed");
    assert!(tests_passed.pass, "empty mission admits `Tests passed`");
}

/// the chatter veto cites the drop rule id and the decision
/// carries matched rule ids and labels for audit and preview.
#[test]
fn drop_veto_carries_rule_ids_and_labels() {
    let mut profile = coding_default_without_mission();
    profile.mission = Some(mission_with(&["session file"], &[(true, "tests passed")]));
    let decision = decide_with(&profile, "Tests passed; the session file grew");
    assert!(!decision.pass);
    let reason = decision.rejection_reason.expect("reason");
    assert!(reason.starts_with("mission_drop:"), "{reason}");
    assert!(
        decision
            .mission_rule_ids
            .iter()
            .any(|id| reason.contains(id))
    );
    assert!(
        decision
            .mission_rule_labels
            .iter()
            .any(|label| label.contains("tests passed"))
    );
}

/// when keep and drop both hit, drop wins and the keep bonus is ignored.
#[test]
fn drop_wins_over_keep() {
    // Distinct entries matching the same candidate: keep `passed`, drop
    // `tests passed`. Duplicates across the two lists are rejected.
    let mut profile = coding_default_without_mission();
    profile.mission = Some(mission_with(&["passed"], &[(true, "tests passed")]));
    let decision = decide_with(&profile, "Tests passed");
    assert!(!decision.pass);
    assert!(
        decision
            .rejection_reason
            .as_deref()
            .is_some_and(|r| r.starts_with("mission_drop:"))
    );
    // Non-veto drop (reject=false) still wins: the utility delta is the drop
    // penalty, not the keep bonus. Distinct entries: keep `ping`, drop
    // `status ping`; the same entry text in both lists would be a duplicate.
    let mut profile = coding_default_without_mission();
    profile.mission = Some(mission_with(&["ping"], &[(false, "status ping")]));
    let baseline = decide_with(&coding_default_without_mission(), "status ping noted");
    let decision = decide_with(&profile, "status ping noted");
    let expected = baseline.admission_score - 0.30 * 0.2;
    assert!((decision.admission_score - expected).abs() < 1e-9);
}

/// Keep hits add exactly +0.15 to the utility factor.
#[test]
fn keep_hit_adds_exact_bonus() {
    let mut profile = coding_default_without_mission();
    profile.mission = Some(mission_with(&["session file"], &[(true, "tests passed")]));
    let baseline = decide_with(&coding_default_without_mission(), "the session file grew");
    let decision = decide_with(&profile, "the session file grew");
    assert!(decision.pass);
    let expected = (baseline.factors.utility + 0.15).clamp(0.0, 1.0);
    assert!((decision.factors.utility - expected).abs() < 1e-9);
    assert!((decision.admission_score - (baseline.admission_score + 0.15 * 0.2)).abs() < 1e-9);
}

/// a keep hit can never rescue a category-gate failure.
#[test]
fn keep_hit_cannot_rescue_category_gate() {
    let mut profile = coding_default_without_mission();
    profile.mission = Some(mission_with(&["prefers"], &[(true, "tests passed")]));
    let env = RankingEnv::system_defaults();
    let policy = AdmissionPolicy::with_retention(&env, profile.resolve());
    let decision = decide_raw(
        "semantic",
        "not_a_category",
        &signals("User prefers tabs"),
        &policy,
    )
    .expect("gate decision");
    assert!(!decision.pass);
    assert!(decision.admission_score.abs() < 1e-12);
    assert!(
        decision
            .rejection_reason
            .as_deref()
            .is_some_and(|r| r.starts_with("category gate"))
    );
    assert!(
        decision.mission_rule_ids.is_empty(),
        "{:?}",
        decision.mission_rule_ids
    );
}

/// A mission that matches nothing is behavior-neutral: identical score and
/// empty rule ids.
#[test]
fn non_matching_mission_is_neutral() {
    let mut profile = coding_default_without_mission();
    profile.mission = Some(mission_with(&["zzz-unlikely"], &[(true, "yyy-unlikely")]));
    let baseline = decide_with(&coding_default_without_mission(), "the session file grew");
    let decision = decide_with(&profile, "the session file grew");
    assert!((decision.admission_score - baseline.admission_score).abs() < 1e-12);
    assert!(
        decision.mission_rule_ids.is_empty(),
        "{:?}",
        decision.mission_rule_ids
    );
    assert!(
        decision.mission_rule_labels.is_empty(),
        "{:?}",
        decision.mission_rule_labels
    );
}
