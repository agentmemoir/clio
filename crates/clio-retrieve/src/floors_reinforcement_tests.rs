// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for how the score floors interact with
//! co-activation reinforcement: the reinforced page is the pre-dedup candidate
//! page of the query that actually ran, so an arm floor or the `reranker` floor
//! shrinks it while the `final` floor leaves it alone.
//!
//! ## Owns
//! - `pairs_reinforced` under an arm floor, under a `reranker` floor, and under
//!   a `final` floor, each against the unfloored baseline
//! - The asymmetry that the `final` floor shortens the response without touching
//!   reinforcement
//!
//! ## Does not own
//! - The floor predicates themselves (`floors_tests`)
//! - The arm floors' own pruning evidence (`floors_integration_tests`) and the
//!   post-ranking floors' evidence (`floors_ranking_tests`)
//! - The reinforcement engine and its durable handoff (`assoc`, `hybrid`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the floors' effect on the reinforced page.

use crate::fixtures_tests as fx;
use crate::floors::floors_integration_tests::{hit_ids, request};
use clio_index::{LexicalSearchArgs, lexical_search};
use clio_store::SqliteStore;
use std::sync::Arc;

/// Three lexical candidates whose raw backend scores differ, so a `keyword`
/// floor between two of them removes exactly one.
fn seed_three(store: &Arc<SqliteStore>) {
    for (id, text) in [
        ("A", "alpha memory exact"),
        ("B", "alpha memory alpha memory padding"),
        (
            "C",
            "alpha memory alpha memory alpha memory padding padding",
        ),
    ] {
        // The gist carries the id so the three items are distinct facts; a
        // shared gist would make them exact duplicates, and read-time collapse
        // would drop two of them before the floor under test ever ran.
        let entry = fx::item(id, "bank", &format!("alpha memory {id}"));
        fx::put_lexical(store, &entry, text);
    }
}

/// The raw backend lexical score the lexical arm sees for one id.
fn raw_keyword(store: &Arc<SqliteStore>, id: &str) -> f64 {
    lexical_search(
        store.as_ref(),
        &LexicalSearchArgs {
            bank: "bank",
            query: "alpha",
            domains: Vec::new(),
            limit: 10,
        },
    )
    .expect("lexical search")
    .iter()
    .find(|hit| hit.item_id == id)
    .unwrap_or_else(|| panic!("lexical hit `{id}`"))
    .score
}

/// Number of unordered pairs in a reinforced page of `n` ids.
fn pairs(n: usize) -> usize {
    n * n.saturating_sub(1) / 2
}

/// An arm floor and the `reranker` floor shrink the reinforced page, because
/// both prune before or at the point the pre-dedup page is taken. The `final`
/// floor does not: it runs later, on the returned page only, so the response
/// shrinks while every retrieved pair is still reinforced.
#[test]
fn a_final_floor_leaves_reinforcement_alone_and_the_earlier_floors_do_not() {
    let store = fx::store();
    seed_three(&store);
    let retriever = fx::retriever(&store);

    let mut probe = request("bank", "alpha");
    probe.limit = 5;
    let baseline = retriever.retrieve(&probe).expect("unfloored retrieve");
    assert_eq!(
        hit_ids(&baseline).len(),
        3,
        "three candidates: {baseline:?}"
    );
    assert_eq!(
        baseline.pairs_reinforced,
        pairs(3),
        "all three retrieved ids are reinforced together: {baseline:?}"
    );
    let (best, second, _) = (
        baseline.hits[0].score,
        baseline.hits[1].score,
        baseline.hits[2].score,
    );
    assert!(best > second, "fixture scores must differ: {best} {second}");

    // `final`: one hit comes back, and the reinforced page is still all three.
    let mut final_req = request("bank", "alpha");
    final_req.limit = 5;
    final_req.min_scores.final_score = Some(f64::midpoint(best, second));
    let final_floored = retriever.retrieve(&final_req).expect("final-floored");
    assert_eq!(
        hit_ids(&final_floored).len(),
        1,
        "the response is shorter: {final_floored:?}"
    );
    assert_eq!(
        final_floored.pairs_reinforced,
        pairs(3),
        "`final` runs after the reinforced page, so reinforcement is untouched: \
         {final_floored:?}"
    );

    // `keyword`: the arm prunes before the candidate page exists, so the
    // reinforced page is the two survivors and not three.
    let mut keyword_req = request("bank", "alpha");
    keyword_req.limit = 5;
    keyword_req.min_scores.keyword = Some(f64::midpoint(
        raw_keyword(&store, "A"),
        raw_keyword(&store, "B"),
    ));
    let keyword_floored = retriever.retrieve(&keyword_req).expect("keyword-floored");
    assert_eq!(
        hit_ids(&keyword_floored).len(),
        2,
        "the arm kept two candidates: {keyword_floored:?}"
    );
    assert_eq!(
        keyword_floored.pairs_reinforced,
        pairs(2),
        "the arm floor shrank the reinforced page too: {keyword_floored:?}"
    );
}

/// The `reranker` floor is applied to the fused list before the reinforced page
/// is selected, so it shrinks that page as well. Pinned here so the rule is
/// deliberate rather than implied by the order of two statements.
#[test]
fn a_reranker_floor_shrinks_the_reinforced_page() {
    let store = fx::store();
    seed_three(&store);
    let mut retriever = fx::retriever(&store);
    retriever.set_reranker(Arc::new(fx::ReverseReranker));

    let mut probe = request("bank", "alpha");
    probe.limit = 5;
    let baseline = retriever.retrieve(&probe).expect("unfloored retrieve");
    assert!(baseline.reranked, "the reranker ran: {baseline:?}");
    assert_eq!(
        baseline.pairs_reinforced,
        pairs(3),
        "nothing is floored, so all three are reinforced: {baseline:?}"
    );

    // `fx::ReverseReranker` scores the candidates 0.9 / 0.8 / 0.7, so 0.85 keeps
    // one of them and a page of one id reinforces no pair at all.
    let mut req = request("bank", "alpha");
    req.limit = 5;
    req.min_scores.reranker = Some(0.85);
    let floored = retriever.retrieve(&req).expect("floored retrieve");
    assert_eq!(
        hit_ids(&floored).len(),
        1,
        "only the top rerank score clears 0.85: {floored:?}"
    );
    assert_eq!(
        floored.pairs_reinforced, 0,
        "the below-floor candidates left the reinforced page: {floored:?}"
    );
}
