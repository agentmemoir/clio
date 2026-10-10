// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `trust_factor` carried on the fused hits
//! of the rank shaping stage: the provenance-to-ratio mapping and its
//! fail-soft boundaries, the carried value equalling the applied multiplier
//! exactly (fused and graph-expanded), its independence from the age knob, the
//! fact-first ordering it produces, and the guarantee that it never drops a
//! hit.
//!
//! ## Owns
//! - Pure `fuse_candidates` and `trust_factor_for` assertions over the shared
//!   classed fixture, with exact bit equality and a fixed clock
//!
//! ## Does not own
//! - The shared shaping fixtures (`hybrid_rank_test_support`)
//! - The age curve and its own carried value (`hybrid_rank_curve_tests`,
//!   `hybrid_rank_curve_age_tests`)
//! - The reported `scores.trust_factor` contract end to end
//!   (`stage_score_trust_tests`, `finalize_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the carried trust factor and its boundaries.

use super::hybrid_rank_test_support::{
    FIXED_NOW, FLOOR, active_curve, disabled_curve, fixture_expanded, fixture_item, fixture_items,
    fixture_legs, fixture_stamps, fixture_weights, ids, score_of, shape_fixture,
    shape_fixture_trust,
};
use super::*;
use crate::fusion::DEFAULT_RRF_K;
use clio_config::ranking::{
    DEFAULT_RETENTION_HALF_LIFE_DAYS, DEFAULT_TRUST_AGENT_INFERRED, DEFAULT_TRUST_THIRD_PARTY,
    DEFAULT_TRUST_USER_STATED, TrustRatios,
};
use clio_types::{EpistemicKind, MemoryItem, SourceType};
use std::collections::{BTreeMap, BTreeSet};

/// A trust environment whose ratios are distinct and not the defaults, so a
/// mapping that ignored the configured value could not pass.
fn distinct_trust() -> TrustRatios {
    TrustRatios {
        user_stated: 0.7,
        third_party: 0.8,
        agent_inferred: 0.9,
    }
}

/// The neutral trust environment: every ratio at `1.0`.
fn neutral_trust() -> TrustRatios {
    TrustRatios {
        user_stated: 1.0,
        third_party: 1.0,
        agent_inferred: 1.0,
    }
}

/// The four support fixture items, each tagged with its own epistemic class:
/// A is a user-stated belief, B a fact, C a third-party belief, and D an
/// agent-inferred belief that joins only through graph expansion. The stamps
/// are the support fixture's, so A is ten years old and C and D one year old.
fn classed_items() -> BTreeMap<String, MemoryItem> {
    let mut items = fixture_items(fixture_stamps());
    let classes = [
        ("A", EpistemicKind::Belief, Some(SourceType::UserStated)),
        ("B", EpistemicKind::Fact, None),
        ("C", EpistemicKind::Belief, Some(SourceType::ThirdParty)),
        ("D", EpistemicKind::Belief, Some(SourceType::AgentInferred)),
    ];
    for (id, kind, source) in classes {
        let item = items.get_mut(id).expect("fixture item");
        item.epistemic_kind = kind;
        item.source_type = source;
    }
    items
}

/// The classed fixture fused under `retention` and `trust`.
fn classed_page(retention: RetentionArgs<'_>, trust: TrustRatios) -> Vec<FusedHit> {
    let (dense, lexical) = fixture_legs();
    fuse_candidates(
        &fixture_weights(),
        DEFAULT_RRF_K,
        &dense,
        &lexical,
        &[],
        &classed_items(),
        &fixture_expanded(),
        retention,
        trust,
    )
}

#[test]
fn trust_factor_for_maps_every_provenance_class() {
    let trust = distinct_trust();
    assert_eq!(
        trust_factor_for(EpistemicKind::Fact, None, trust).to_bits(),
        1.0f64.to_bits(),
        "a fact is not demoted"
    );
    assert_eq!(
        trust_factor_for(EpistemicKind::Fact, Some(SourceType::AgentInferred), trust).to_bits(),
        1.0f64.to_bits(),
        "a fact keeps 1.0 even when a source type rides along"
    );
    for (source, expected) in [
        (SourceType::UserStated, trust.user_stated),
        (SourceType::ThirdParty, trust.third_party),
        (SourceType::AgentInferred, trust.agent_inferred),
    ] {
        assert_eq!(
            trust_factor_for(EpistemicKind::Belief, Some(source), trust).to_bits(),
            expected.to_bits(),
            "{source:?}"
        );
    }
    assert_eq!(
        trust_factor_for(EpistemicKind::Belief, None, trust).to_bits(),
        1.0f64.to_bits(),
        "a belief with no source type has no declared ratio"
    );
}

#[test]
fn trust_factor_for_neutralizes_an_unusable_ratio() {
    // A bad knob leaves the hit's score untouched instead of demoting it; the
    // ranking-env validator is the boundary that refuses to store such a value.
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -0.5, 1.5] {
        let trust = TrustRatios {
            user_stated: bad,
            third_party: bad,
            agent_inferred: bad,
        };
        for source in [
            SourceType::UserStated,
            SourceType::ThirdParty,
            SourceType::AgentInferred,
        ] {
            assert_eq!(
                trust_factor_for(EpistemicKind::Belief, Some(source), trust).to_bits(),
                1.0f64.to_bits(),
                "ratio {bad} for {source:?}"
            );
        }
    }
    // A usable ratio is still read, so the neutralization is about the value.
    let trust = TrustRatios {
        user_stated: 1.0,
        third_party: 0.5,
        agent_inferred: 0.25,
    };
    assert_eq!(
        trust_factor_for(EpistemicKind::Belief, Some(SourceType::ThirdParty), trust).to_bits(),
        0.5f64.to_bits()
    );
}

#[test]
fn default_trust_spread_is_monotone_and_bounded() {
    let trust = TrustRatios::default();
    let ratios = [trust.user_stated, trust.third_party, trust.agent_inferred];
    assert!(
        ratios.windows(2).all(|pair| pair[0] >= pair[1]),
        "the default spread must not invert: {ratios:?}"
    );
    for ratio in ratios {
        assert!(ratio.is_finite(), "default ratio {ratio} must be finite");
        assert!(
            ratio > 0.0 && ratio <= 1.0,
            "default ratio {ratio} outside (0, 1]"
        );
    }
    assert_eq!(
        trust.user_stated.to_bits(),
        DEFAULT_TRUST_USER_STATED.to_bits()
    );
    assert_eq!(
        trust.third_party.to_bits(),
        DEFAULT_TRUST_THIRD_PARTY.to_bits()
    );
    assert_eq!(
        trust.agent_inferred.to_bits(),
        DEFAULT_TRUST_AGENT_INFERRED.to_bits()
    );
}

#[test]
fn facts_only_page_keeps_every_factor_neutral_and_the_shaped_score() {
    let baseline = shape_fixture(disabled_curve());
    let demoted = shape_fixture_trust(disabled_curve(), distinct_trust());
    for hit in &demoted {
        assert_eq!(hit.trust_factor.to_bits(), 1.0f64.to_bits(), "{}", hit.id);
    }
    assert_eq!(ids(&baseline), ids(&demoted));
    for id in ["A", "B", "C", "D"] {
        assert_eq!(
            score_of(&demoted, id).to_bits(),
            score_of(&baseline, id).to_bits(),
            "{id}"
        );
    }
}

#[test]
fn carried_trust_factor_equals_the_applied_factor() {
    let trust = distinct_trust();
    let neutral = classed_page(disabled_curve(), neutral_trust());
    let active = classed_page(disabled_curve(), trust);
    let items = classed_items();
    for hit in &active {
        let base = neutral.iter().find(|b| b.id == hit.id).expect("paired hit");
        // The carried value is the exact multiplier that reached the score.
        assert_eq!(
            hit.score.to_bits(),
            (base.score * hit.trust_factor).to_bits(),
            "{}",
            hit.id
        );
        let item = &items[&hit.id];
        let applied = trust_factor_for(item.epistemic_kind, item.source_type, trust);
        assert_eq!(hit.trust_factor.to_bits(), applied.to_bits(), "{}", hit.id);
        assert!(
            hit.trust_factor > 0.0 && hit.trust_factor <= 1.0,
            "{}: {} outside (0, 1]",
            hit.id,
            hit.trust_factor
        );
    }
}

#[test]
fn graph_expanded_belief_receives_the_factor_exactly_once() {
    let trust = distinct_trust();
    let neutral = classed_page(disabled_curve(), neutral_trust());
    let active = classed_page(disabled_curve(), trust);
    let expanded = active.iter().find(|h| h.id == "D").expect("D hit");
    let base = neutral.iter().find(|h| h.id == "D").expect("D hit");
    assert_eq!(
        expanded.trust_factor.to_bits(),
        trust.agent_inferred.to_bits(),
        "D reports its own class's ratio"
    );
    assert_eq!(
        expanded.score.to_bits(),
        (base.score * trust.agent_inferred).to_bits(),
        "D is multiplied exactly once"
    );
    // The neutral page's D is still the best score times the graph decay, so
    // the factor moved D rather than reshaping the expanded literal.
    assert_eq!(
        base.score.to_bits(),
        (score_of(&neutral, "B") * DEFAULT_GRAPH_DECAY).to_bits()
    );
}

#[test]
fn moving_the_age_knob_leaves_the_trust_factor_unchanged() {
    let trust = distinct_trust();
    let off = classed_page(disabled_curve(), trust);
    let on = classed_page(active_curve(FLOOR, DEFAULT_RETENTION_HALF_LIFE_DAYS), trust);
    for hit in &on {
        let base = off.iter().find(|b| b.id == hit.id).expect("paired hit");
        assert_eq!(
            hit.trust_factor.to_bits(),
            base.trust_factor.to_bits(),
            "{}",
            hit.id
        );
    }
    // A is ten years old, so the age knob really moved it: the assertion above
    // is not vacuous.
    let a_on = on.iter().find(|h| h.id == "A").expect("A");
    let a_off = off.iter().find(|h| h.id == "A").expect("A");
    assert_ne!(a_on.age_factor.to_bits(), a_off.age_factor.to_bits());
}

#[test]
fn moving_a_trust_ratio_leaves_the_age_factor_unchanged() {
    let curve = active_curve(FLOOR, DEFAULT_RETENTION_HALF_LIFE_DAYS);
    let defaults = classed_page(curve, TrustRatios::default());
    let custom = classed_page(curve, distinct_trust());
    for hit in &custom {
        let base = defaults
            .iter()
            .find(|b| b.id == hit.id)
            .expect("paired hit");
        assert_eq!(
            hit.age_factor.to_bits(),
            base.age_factor.to_bits(),
            "{}",
            hit.id
        );
    }
    // The user-stated ratio moved A, so the trust knob really moved.
    let a_custom = custom.iter().find(|h| h.id == "A").expect("A");
    let a_defaults = defaults.iter().find(|h| h.id == "A").expect("A");
    assert_ne!(
        a_custom.trust_factor.to_bits(),
        a_defaults.trust_factor.to_bits()
    );
}

#[test]
fn fact_ranks_above_an_otherwise_equal_inferred_belief() {
    // One rank-one slot per arm and equal admissions: the fused score is the
    // same, and with all ratios at 1.0 the tie-break puts the belief id first.
    let dense = [StageHit {
        id: "Z".into(),
        score: 0.9,
    }];
    let lexical = [StageHit {
        id: "A".into(),
        score: 12.0,
    }];
    let fact = fixture_item("Z", 0.5, FIXED_NOW);
    let mut belief = fixture_item("A", 0.5, FIXED_NOW);
    belief.epistemic_kind = EpistemicKind::Belief;
    belief.source_type = Some(SourceType::AgentInferred);
    let items = BTreeMap::from([("Z".to_owned(), fact), ("A".to_owned(), belief)]);
    let run = |trust: TrustRatios| {
        fuse_candidates(
            &fixture_weights(),
            DEFAULT_RRF_K,
            &dense,
            &lexical,
            &[],
            &items,
            &BTreeSet::new(),
            disabled_curve(),
            trust,
        )
    };
    let neutral = run(neutral_trust());
    assert_eq!(
        ids(&neutral),
        vec!["A", "Z"],
        "equal scores tie-break by id"
    );
    let default = run(TrustRatios::default());
    assert_eq!(ids(&default), vec!["Z", "A"], "the fact ranks first");
    assert_eq!(
        score_of(&default, "Z").to_bits(),
        score_of(&neutral, "Z").to_bits(),
        "the fact is unchanged"
    );
    assert_eq!(
        score_of(&default, "A").to_bits(),
        (score_of(&neutral, "A") * DEFAULT_TRUST_AGENT_INFERRED).to_bits()
    );
}

#[test]
fn the_configured_default_never_excludes_a_hit() {
    let all_ones = classed_page(disabled_curve(), neutral_trust());
    let defaults = classed_page(disabled_curve(), TrustRatios::default());
    let mut baseline: Vec<&str> = ids(&all_ones);
    let mut demoted: Vec<&str> = ids(&defaults);
    baseline.sort_unstable();
    demoted.sort_unstable();
    assert_eq!(baseline, demoted, "a multiplier only moves, never drops");
    assert_eq!(baseline.len(), 4);
}
