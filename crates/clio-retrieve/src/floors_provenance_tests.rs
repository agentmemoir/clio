// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the provenance floor over the retrieval path:
//! which rung each epistemic class sits at, which classes each rung admits, and
//! what the fetch-time prune does to the page, the dedup scope, and the
//! reinforced set.
//!
//! ## Owns
//! - The shared fixtures: one item per rung, one parent/twin-text corpus
//! - Rung-boundary evidence for every class, the absent-floor and absent-source
//!   cases
//! - The fetch-time prune over the real path: the page, the dedup scope, the
//!   untouched fields of a survivor, reinforcement, and determinism
//!
//! ## Does not own
//! - The rung predicate (`floors`) and the ladder's names, wire values, and
//!   strength order (`clio-config`'s retention level enum)
//! - The wire resolution of a floor (`provenance_surface_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::fixtures_tests as fx;
use crate::floors::floors_integration_tests::{hit_ids, request, serialized};
use crate::floors::provenance_floor_keeps;
use crate::types::{RetrieveOutcome, ScoredHit};
use clio_config::retention_levels::ProvenanceFloorDefault as Floor;
use clio_store::{SqliteStore, Store};
use clio_types::{EpistemicKind, MemoryItem, SourceType};
use serde_json::{Value, json};
use std::sync::Arc;

/// One item at the requested rung: a fact when `source` is `None`, else a belief
/// carrying that source type. The confidences differ per class, so a survivor's
/// fields are worth comparing across a floor.
fn rung_item(id: &str, source: Option<SourceType>) -> MemoryItem {
    let mut item = fx::semantic_item(id, "bank", &format!("alpha memory {id}"));
    item.epistemic_kind = match source {
        None => EpistemicKind::Fact,
        Some(_) => EpistemicKind::Belief,
    };
    item.source_type = source;
    item.confidence = Some(if source.is_some() { 0.42 } else { 0.9 });
    item
}

/// Seed one lexically queryable item per rung. The documents differ in length so
/// the page order comes from the raw score rather than a tie-break; the gists
/// differ so duplicate collapse cannot drop one before a floor under test runs.
pub(crate) fn seed_rungs(store: &Arc<SqliteStore>) {
    let rungs = [
        ("FACT", None, "alpha memory"),
        (
            "STATED",
            Some(SourceType::UserStated),
            "alpha memory stated",
        ),
        (
            "THIRD",
            Some(SourceType::ThirdParty),
            "alpha memory third party reported",
        ),
        (
            "INFERRED",
            Some(SourceType::AgentInferred),
            "alpha memory inferred because of padding",
        ),
    ];
    for (id, source, text) in rungs {
        fx::put_lexical(store, &rung_item(id, source), text);
    }
}

/// A consolidated `verified` parent claiming one `agent_inferred` raw, plus two
/// more inferred raws of which `TWIN` repeats `WEAK`'s collapse text, so both
/// dedup paths can fire without a floor.
fn seed_twin_texts(store: &Arc<SqliteStore>) {
    let mut parent = fx::semantic_item("PARENT", "bank", "alpha twin parent");
    parent.source_ref = Some(crate::HUB_DISTILL_SOURCE_REF.into());
    parent.snapshot = Some(json!({"hub_id": "HUB", "neighbor_ids": ["CLAIMED"]}));
    fx::put_lexical(store, &parent, "alpha twin parent lexical");
    let raws = [
        ("CLAIMED", "twin claimed", "alpha twin lexical claimed"),
        ("WEAK", "twin weak", "alpha twin lexical weak"),
        ("TWIN", "twin weak", "alpha twin lexical twin filler"),
    ];
    for (id, gist, doc) in raws {
        let mut item = rung_item(id, Some(SourceType::AgentInferred));
        item.gist = Some(gist.into());
        fx::put_lexical(store, &item, doc);
    }
}

/// The four seeded classes in page order, for an unfloored baseline.
pub(crate) const ALL_CLASSES: [&str; 4] = ["FACT", "STATED", "THIRD", "INFERRED"];

/// A belief whose source type was cleared, so it sits at no rung at all.
fn unsourced_belief(id: &str) -> MemoryItem {
    let mut item = rung_item(id, Some(SourceType::UserStated));
    item.source_type = None;
    item
}

/// The hit for `id`, or a panic naming the page it was missing from.
fn hit<'a>(out: &'a RetrieveOutcome, id: &str) -> &'a ScoredHit {
    out.hits
        .iter()
        .find(|hit| hit.hit.item_id == id)
        .unwrap_or_else(|| panic!("hit `{id}` missing from {:?}", hit_ids(out)))
}

/// The response contract of one hit, for comparing a floored page hit by hit.
fn hit_contract(hit: &ScoredHit) -> Value {
    serde_json::to_value(&hit.hit).expect("serialize hit")
}

/// The wire payload without the timing block: two calls never agree on latency.
fn page_without_metrics(out: &RetrieveOutcome) -> Value {
    let mut value = serialized(out);
    let object = value.as_object_mut().expect("outcome object");
    object.remove("metrics");
    value
}

/// The rung an item sits at, read through the public predicate.
fn rung_of(item: &MemoryItem) -> Option<Floor> {
    crate::floors::provenance_rung(item.epistemic_kind, item.source_type)
}

/// The four rungs, strictest first.
const RUNGS: [Floor; 4] = [
    Floor::Verified,
    Floor::Stated,
    Floor::ThirdParty,
    Floor::Inferred,
];

/// A class sits at the rung its own provenance names: a fact is `verified`, a
/// belief sits at its source type's rung, and a source-less belief at no rung.
#[test]
fn every_class_sits_at_the_rung_its_provenance_names() {
    let fact = rung_of(&rung_item("FACT", None));
    assert_eq!(fact, Some(Floor::Verified), "a fact clears every rung");
    for (source, rung) in [
        (SourceType::UserStated, Floor::Stated),
        (SourceType::ThirdParty, Floor::ThirdParty),
        (SourceType::AgentInferred, Floor::Inferred),
    ] {
        let sat = rung_of(&rung_item("B", Some(source)));
        assert_eq!(sat, Some(rung), "{source:?} sits at {rung:?}");
    }
    assert_eq!(
        rung_of(&unsourced_belief("B")),
        None,
        "a belief with no source type has no rung to compare"
    );
}

/// A source-less belief cannot be stored, but if one is ever read no named floor
/// may invent a rung for it.
#[test]
fn a_belief_with_no_source_type_is_excluded_from_every_rung() {
    let item = unsourced_belief("B");
    for floor in RUNGS {
        let keeps = provenance_floor_keeps(&item, Some(floor));
        assert!(!keeps, "{floor:?} must exclude a source-less belief");
    }
}

/// The `stated` rung at both neighbour boundaries: `verified`, one rung stricter,
/// excludes it, and `third_party`, one rung looser, admits it.
#[test]
fn a_stated_belief_is_correct_at_both_neighbour_boundaries() {
    let item = rung_item("STATED", Some(SourceType::UserStated));
    let boundaries = [
        (Floor::Verified, false),
        (Floor::Stated, true),
        (Floor::ThirdParty, true),
    ];
    for (floor, keeps) in boundaries {
        assert_eq!(
            provenance_floor_keeps(&item, Some(floor)),
            keeps,
            "{floor:?} on a user_stated belief"
        );
    }
}

/// The `third_party` rung at both neighbour boundaries: `stated`, one rung stricter,
/// excludes it, and `inferred`, one rung looser, admits it.
#[test]
fn a_third_party_belief_is_correct_at_both_neighbour_boundaries() {
    let item = rung_item("THIRD", Some(SourceType::ThirdParty));
    let boundaries = [
        (Floor::Stated, false),
        (Floor::ThirdParty, true),
        (Floor::Inferred, true),
    ];
    for (floor, keeps) in boundaries {
        assert_eq!(
            provenance_floor_keeps(&item, Some(floor)),
            keeps,
            "{floor:?} on a third_party belief"
        );
    }
}

/// A `stated` floor keeps the fact and the belief stated by the user, and drops
/// both weaker classes. The set is a nested one, so a rung can only ever narrow.
#[test]
fn a_stated_floor_drops_the_weaker_beliefs_and_keeps_the_fact() {
    for (id, source, keeps) in [
        ("FACT", None, true),
        ("STATED", Some(SourceType::UserStated), true),
        ("THIRD", Some(SourceType::ThirdParty), false),
        ("INFERRED", Some(SourceType::AgentInferred), false),
    ] {
        let got = provenance_floor_keeps(&rung_item(id, source), Some(Floor::Stated));
        assert_eq!(got, keeps, "`{id}` at a `stated` floor: {got}");
    }
}

/// The strictest rung keeps the fact alone: no belief class is verified.
#[test]
fn a_verified_floor_keeps_the_fact_and_excludes_every_belief() {
    let classes = [
        ("FACT", None),
        ("USER", Some(SourceType::UserStated)),
        ("THIRD", Some(SourceType::ThirdParty)),
        ("INFERRED", Some(SourceType::AgentInferred)),
    ];
    for (id, source) in classes {
        let keeps = provenance_floor_keeps(&rung_item(id, source), Some(Floor::Verified));
        assert_eq!(keeps, id == "FACT", "`{id}` at the strictest rung: {keeps}");
    }
}

/// Absence is not a value: with no floor named every class is returned, so a
/// request that names none behaves as it did before the floor existed. The
/// unsourced belief makes it non-trivial.
#[test]
fn an_absent_floor_keeps_every_class() {
    let classes = [
        rung_item("FACT", None),
        rung_item("STATED", Some(SourceType::UserStated)),
        rung_item("THIRD", Some(SourceType::ThirdParty)),
        rung_item("INFERRED", Some(SourceType::AgentInferred)),
    ];
    for item in classes {
        let keeps = provenance_floor_keeps(&item, None);
        assert!(keeps, "no floor means no filter for {}", item.id);
    }
    let unsourced = unsourced_belief("UNSOURCED");
    assert!(provenance_floor_keeps(&unsourced, None), "no rung named");
    assert!(
        !provenance_floor_keeps(&unsourced, Some(Floor::Inferred)),
        "the loosest rung drops it"
    );
}

/// The same boundary at fetch time: the excluded candidates leave the fetched
/// item map, so they are gone from the page and from the arm's survivor count.
#[test]
fn a_stated_floor_prunes_the_weaker_beliefs_at_fetch_time() {
    let store = fx::store();
    seed_rungs(&store);
    let retriever = fx::retriever(&store);

    let plain = request("bank", "alpha");
    let unfloored = retriever.retrieve(&plain).expect("retrieve");
    assert_eq!(hit_ids(&unfloored), ALL_CLASSES, "four classes returned");

    let mut req = request("bank", "alpha");
    req.provenance_floor = Some(Floor::Stated);
    let out = retriever.retrieve(&req).expect("floored retrieve");
    assert_eq!(
        hit_ids(&out),
        vec!["FACT".to_owned(), "STATED".to_owned()],
        "the weaker classes never entered the page: {out:?}"
    );
    assert_eq!(out.lexical_candidates, 2, "survivors of the pruned map");
}

/// A below-floor id is outside the dedup scope, not only off the page: the same
/// request without a floor names a below-floor raw in the report, so the empty
/// lists under a floor are evidence the scope never held them.
#[test]
fn a_below_floor_id_is_absent_from_the_dedup_scope() {
    let store = fx::store();
    seed_twin_texts(&store);
    let retriever = fx::retriever(&store);
    let mut req = request("bank", "alpha");
    req.prefer_consolidated = true;

    let unfloored = retriever.retrieve(&req).expect("retrieve");
    let base = &unfloored.dedup;
    assert_eq!(base.suppressed_raw_ids, ["CLAIMED"], "{base:?}");
    assert_eq!(base.exact_dup_suppressed, 1, "twin text: {base:?}");
    let mut ids = hit_ids(&unfloored);
    ids.sort();
    assert_eq!(ids, ["PARENT", "WEAK"], "both paths fired: {base:?}");

    req.provenance_floor = Some(Floor::Stated);
    let out = retriever.retrieve(&req).expect("floored retrieve");
    let dedup = &out.dedup;
    for id in ["CLAIMED", "WEAK", "TWIN"] {
        let mut named = dedup.suppressed_raw_ids.iter().chain(&dedup.backfilled_ids);
        assert!(!named.any(|n| n == id), "`{id}` in scope: {dedup:?}");
    }
    assert_eq!(dedup.raw_suppressed, 0, "{dedup:?}");
    assert_eq!(dedup.exact_dup_suppressed, 0, "{dedup:?}");
    assert_eq!(hit_ids(&out), ["PARENT"], "pruned before fusion: {out:?}");
}

/// The excluded item is gone from the reinforcement set too, because that set is
/// taken from the same pruned map: the floor is one prune, not a page-level
/// filter with a second side effect left behind.
#[test]
fn a_floored_item_leaves_the_reinforced_set() {
    let store = fx::store();
    seed_rungs(&store);
    let retriever = fx::retriever(&store);

    let mut unfloored = request("bank", "alpha");
    unfloored.limit = 5;
    let baseline = retriever.retrieve(&unfloored).expect("retrieve");
    assert_eq!(baseline.pairs_reinforced, 6, "four ids reinforce six pairs");

    let mut req = request("bank", "alpha");
    req.limit = 5;
    req.provenance_floor = Some(Floor::Stated);
    let out = retriever.retrieve(&req).expect("floored retrieve");
    assert_eq!(hit_ids(&out).len(), 2, "two classes survive");
    assert_eq!(out.pairs_reinforced, 1, "one pair, not six");
}

/// A graph-expanded candidate is fetched through the same map, so the floor
/// stops it entering the fused result at all instead of dropping it later.
#[test]
fn a_floored_item_never_enters_as_a_graph_expansion() {
    let store = fx::store();
    seed_rungs(&store);
    // Stored but not lexically indexed: only ever a graph expansion, and so
    // counted as genuinely new.
    let weak = rung_item("WEAK", Some(SourceType::AgentInferred));
    store
        .create_memory_item(&weak, "agent")
        .expect("weak belief");
    let mut retriever = fx::retriever(&store);
    retriever.set_expander(Arc::new(fx::StaticExpander {
        ids: vec!["WEAK".to_owned()],
    }));

    let mut unfloored = request("bank", "alpha");
    unfloored.expand_graph = true;
    let baseline = retriever.retrieve(&unfloored).expect("retrieve");
    assert_eq!(baseline.graph_expanded, 1, "reachable without a floor");
    assert!(hit_ids(&baseline).contains(&"WEAK".to_owned()));

    let mut req = request("bank", "alpha");
    req.expand_graph = true;
    req.provenance_floor = Some(Floor::Stated);
    let out = retriever.retrieve(&req).expect("floored retrieve");
    assert_eq!(out.graph_expanded, 0, "pruned out of the fused list");
    assert_eq!(hit_ids(&out), vec!["FACT", "STATED"]);
}

/// A survivor comes back exactly as it was stored: the floor only removes
/// candidates, so it rewrites no field of the ones it keeps.
#[test]
fn a_floor_does_not_change_the_fields_of_a_surviving_hit() {
    let store = fx::store();
    seed_rungs(&store);
    let retriever = fx::retriever(&store);
    let plain = request("bank", "alpha");
    let unfloored = retriever.retrieve(&plain).expect("retrieve");

    let mut req = request("bank", "alpha");
    req.provenance_floor = Some(Floor::ThirdParty);
    let floored = retriever.retrieve(&req).expect("floored retrieve");
    assert_eq!(hit_ids(&floored), vec!["FACT", "STATED", "THIRD"]);

    for id in ["FACT", "STATED", "THIRD"] {
        let before = hit_contract(hit(&unfloored, id));
        let after = hit_contract(hit(&floored, id));
        assert_eq!(after, before, "`{id}` came back unchanged: {after}");
    }
}

/// A surviving belief still carries its own provenance and still demands a
/// confidence check: the floor never turns a belief into a fact.
#[test]
fn a_surviving_belief_still_requires_a_confidence_check() {
    let store = fx::store();
    seed_rungs(&store);
    let retriever = fx::retriever(&store);

    let mut req = request("bank", "alpha");
    req.provenance_floor = Some(Floor::ThirdParty);
    let out = retriever.retrieve(&req).expect("floored retrieve");
    for id in ["STATED", "THIRD"] {
        let contract = hit_contract(hit(&out, id));
        assert_eq!(
            contract["requires_confidence_check"], true,
            "`{id}` is still a belief: {contract}"
        );
    }
    let stated = hit_contract(hit(&out, "STATED"));
    assert_eq!(stated["source_type"], "user_stated", "keeps its source");
    assert_eq!(stated["confidence"], 0.42, "keeps its confidence");
}

/// The floor is pure and the request carries a fixed clock, so the same request
/// twice returns the same page, down to the reported scores.
#[test]
fn the_same_floored_request_twice_returns_the_same_page() {
    let store = fx::store();
    seed_rungs(&store);
    let retriever = fx::retriever(&store);
    let mut req = request("bank", "alpha");
    req.provenance_floor = Some(Floor::ThirdParty);

    let first = retriever.retrieve(&req).expect("first call");
    let second = retriever.retrieve(&req).expect("second call");
    assert_eq!(hit_ids(&first), vec!["FACT", "STATED", "THIRD"]);
    let (a, b) = (page_without_metrics(&first), page_without_metrics(&second));
    assert_eq!(a, b, "{a}");
}
