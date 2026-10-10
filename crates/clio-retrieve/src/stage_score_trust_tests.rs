// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `trust_factor` reporting-contract tests:
//! the reported factor is exactly the multiplier the trust stage applied
//! fuse → shape → finalize, it is a finite number in the configured range, it
//! never changes `epistemic_kind` / `category` / `confidence`, and it never
//! drops a hit from the page.
//!
//! ## Owns
//! - End-to-end assertions over `scores.trust_factor` with exact bit equality
//!   and the request's fixed clock
//! - The same-fixture comparison between the neutral `1.0` environment and the
//!   configured default environment
//!
//! ## Does not own
//! - The mapping and the carried value on the fused hit
//!   (`hybrid_rank_curve_trust_tests`)
//! - The neutral initialization at fusion (`fusion_tests`) and the carried
//!   read at finalize (`finalize_tests`)
//! - The serialized key-set and null-when-absent rules (`stage_score_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::fixtures_tests as fx;
use crate::floors::floors_provenance_tests::{ALL_CLASSES, seed_rungs};
use crate::types::{RetrieveOutcome, RetrieveRequest};
use clio_config::ranking::{
    DEFAULT_TRUST_AGENT_INFERRED, DEFAULT_TRUST_THIRD_PARTY, RankingEnv, TrustRatios,
};
use clio_types::EpistemicKind;
use std::sync::Arc;

/// Request with a frozen clock; the curve stays off, so only the trust
/// multiplier moves a score between the two environments.
fn request() -> RetrieveRequest {
    let mut req = RetrieveRequest::new("bank", "alpha");
    req.now = Some("2026-01-02T00:00:00Z".into());
    req
}

/// The neutral trust environment: no ratio demotes anything.
fn neutral_trust() -> TrustRatios {
    TrustRatios {
        user_stated: 1.0,
        third_party: 1.0,
        agent_inferred: 1.0,
    }
}

/// A retriever whose live trust ratios are exactly `trust`.
fn retriever_with_trust(
    store: &Arc<clio_store::SqliteStore>,
    trust: TrustRatios,
) -> crate::HybridRetriever {
    let retriever = fx::retriever(store);
    let mut env = RankingEnv::system_defaults();
    env.trust = trust;
    retriever.set_ranking_env(&env);
    retriever
}

/// The four-rung fixture page under `trust`, with its own fresh store.
fn page(trust: TrustRatios) -> RetrieveOutcome {
    let store = fx::store();
    seed_rungs(&store);
    retriever_with_trust(&store, trust)
        .retrieve(&request())
        .expect("retrieve")
}

/// The four-rung fixture page under `trust`, with the request shaped by
/// `shape`, on its own fresh store.
fn shaped_page(trust: TrustRatios, shape: impl FnOnce(&mut RetrieveRequest)) -> RetrieveOutcome {
    let store = fx::store();
    seed_rungs(&store);
    let mut req = request();
    shape(&mut req);
    retriever_with_trust(&store, trust)
        .retrieve(&req)
        .expect("retrieve")
}

/// The hit for `id`, or a panic naming it.
fn hit<'a>(out: &'a RetrieveOutcome, id: &str) -> &'a crate::types::ScoredHit {
    out.hits
        .iter()
        .find(|hit| hit.hit.item_id == id)
        .unwrap_or_else(|| panic!("hit `{id}` missing"))
}

#[test]
fn reported_trust_factor_equals_the_applied_factor() {
    let off = page(neutral_trust());
    let on = page(TrustRatios::default());
    let base = hit(&off, "INFERRED");
    let demoted = hit(&on, "INFERRED");
    let applied = DEFAULT_TRUST_AGENT_INFERRED;
    assert!(applied < 1.0, "fixture must be trust-demoted: {applied}");
    assert_eq!(
        demoted.scores.trust_factor.to_bits(),
        applied.to_bits(),
        "reported {}",
        demoted.scores.trust_factor
    );
    // The reported value is the exact multiplier that reached the score.
    assert_eq!(
        demoted.score.to_bits(),
        (base.score * demoted.scores.trust_factor).to_bits()
    );
    assert_eq!(base.scores.trust_factor.to_bits(), 1.0f64.to_bits());
    assert_eq!(
        hit(&on, "THIRD").scores.trust_factor.to_bits(),
        DEFAULT_TRUST_THIRD_PARTY.to_bits()
    );
    assert_eq!(
        hit(&on, "FACT").scores.trust_factor.to_bits(),
        1.0f64.to_bits()
    );
}

#[test]
fn serialized_trust_factor_is_a_finite_number_in_range() {
    let on = page(TrustRatios::default());
    for scored in &on.hits {
        let value = serde_json::to_value(scored).expect("serialize hit");
        let reported = &value["scores"]["trust_factor"];
        // serde_json writes NaN and infinity as null, so a non-null number here
        // is the proof that the carried factor is finite.
        assert!(
            reported.is_number(),
            "trust_factor must serialize as a number, got {reported}"
        );
        let factor = reported.as_f64().expect("finite f64");
        assert!(factor.is_finite(), "trust_factor must be finite: {factor}");
        assert!(
            (DEFAULT_TRUST_AGENT_INFERRED..=1.0).contains(&factor),
            "factor {factor} outside the configured range"
        );
    }
}

#[test]
fn default_ratios_leave_epistemic_kind_category_and_confidence_identical() {
    let off = page(neutral_trust());
    let on = page(TrustRatios::default());
    let base = hit(&off, "INFERRED");
    let demoted = hit(&on, "INFERRED");
    assert_eq!(demoted.hit.epistemic_kind, EpistemicKind::Belief);
    assert_eq!(demoted.hit.epistemic_kind, base.hit.epistemic_kind);
    assert_eq!(demoted.hit.category.as_deref(), Some("schema"));
    assert_eq!(demoted.hit.category, base.hit.category);
    assert_eq!(demoted.hit.confidence, Some(0.42));
    assert_eq!(demoted.hit.confidence, base.hit.confidence);
}

#[test]
fn default_ratios_never_exclude_a_hit() {
    let off = page(neutral_trust());
    let on = page(TrustRatios::default());
    let mut baseline: Vec<&str> = off.hits.iter().map(|h| h.hit.item_id.as_str()).collect();
    let mut demoted: Vec<&str> = on.hits.iter().map(|h| h.hit.item_id.as_str()).collect();
    baseline.sort_unstable();
    demoted.sort_unstable();
    assert_eq!(baseline, demoted, "a multiplier only moves, never drops");
    let mut expected: Vec<&str> = ALL_CLASSES.to_vec();
    expected.sort_unstable();
    assert_eq!(baseline, expected);
}

/// Records the page-shortening consequence of the bounds: the page takes only
/// the first `limit` of the trust-sorted list, so with a limit below the
/// candidate count a demoted item that sorts below the cut is off the page.
/// The multiplier itself excludes nothing; the limit does.
#[test]
fn a_short_limit_leaves_the_trust_demoted_item_off_the_page() {
    let out = shaped_page(TrustRatios::default(), |req| req.limit = 3);
    assert_eq!(
        out.fused_candidates,
        ALL_CLASSES.len(),
        "all four classes are candidates"
    );
    assert!(
        out.hits.len() < out.fused_candidates,
        "the page is shorter than the candidate set: {}",
        out.hits.len()
    );
    let mut kept: Vec<&str> = out.hits.iter().map(|h| h.hit.item_id.as_str()).collect();
    kept.sort_unstable();
    assert_eq!(
        kept,
        ["FACT", "STATED", "THIRD"],
        "the demoted item is off the page; the undemoted items remain"
    );
}

/// Records the page-shortening consequence of the bounds: the `final` floor
/// runs on the trust-sorted page after the limit, so a floor between the
/// neutral and the demoted score excludes the demoted hit and nothing refills
/// its slot. The multiplier itself excludes nothing; the floor does.
#[test]
fn a_final_floor_between_the_neutral_and_demoted_scores_excludes_the_demoted_hit() {
    let neutral = hit(&page(neutral_trust()), "INFERRED").score;
    let demoted = neutral * DEFAULT_TRUST_AGENT_INFERRED;
    let floor = neutral.midpoint(demoted);
    assert!(
        demoted < floor && floor < neutral,
        "the floor must sit between the neutral and demoted scores: {floor}"
    );
    let out = shaped_page(TrustRatios::default(), |req| {
        req.min_scores.final_score = Some(floor);
    });
    let mut kept: Vec<&str> = out.hits.iter().map(|h| h.hit.item_id.as_str()).collect();
    kept.sort_unstable();
    assert_eq!(
        kept,
        ["FACT", "STATED", "THIRD"],
        "the demoted hit is excluded by the floor; the neutral-scoring hits remain"
    );
}
