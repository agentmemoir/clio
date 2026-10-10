// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for integration tests of the temporal retrieval
//! arm: window membership, deterministic rank contribution, no-op defaults,
//! `as_of`/`time_axis` composition, limits, budgets, and domain scope.
//!
//! ## Owns
//! - End-to-end retriever assertions for the relative-window arm
//!
//! ## Does not own
//! - Expression parsing (`temporal_tests`) or fusion math (`fusion_tests`)
//! - Binding wire shapes (`clio-mcp` read tests)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::fixtures_tests as fx;
use crate::types::RetrieveRequest;
use clio_types::{MemoryItem, TimeAxis};
use serde_json::Value;

/// Noon UTC on Saturday 2026-09-26; `this week` is 2026-09-21..2026-09-28.
const ANCHOR: &str = "2026-09-26T12:00:00Z";
/// The instant every request in this file is evaluated at.
///
/// The window is resolved from the request's own anchor and its candidates are
/// ordered by interval start, so the arm's contribution does not depend on when
/// the suite ran. This pin is belt and braces: it removes "now" from the set of
/// things that could differ between two back-to-back requests, so the
/// determinism assertion below is comparing the arm and the order rather than
/// the clock the process happened to read.
const NOW: &str = "2026-09-26T12:00:00Z";
/// The window query the seeded store is built around.
const WINDOW_QUERY: &str = "what happened this week";

fn request(query: &str, anchor: Option<&str>) -> RetrieveRequest {
    let mut req = RetrieveRequest::new("bank", query);
    req.query_anchor = anchor.map(str::to_owned);
    req.now = Some(NOW.to_owned());
    req
}

fn item_with(id: &str, gist: &str, valid_from: &str, valid_until: Option<&str>) -> MemoryItem {
    let mut entry = fx::item(id, "bank", gist);
    entry.valid_from = valid_from.to_owned();
    entry.valid_until = valid_until.map(str::to_owned);
    entry
}

fn ids(out: &crate::types::RetrieveOutcome) -> Vec<String> {
    out.hits.iter().map(|h| h.hit.item_id.clone()).collect()
}

/// Three in-window items (newest interval start first) plus one item closed
/// before the window that must never appear.
fn seeded_store() -> std::sync::Arc<clio_store::SqliteStore> {
    let store = fx::store();
    for (id, gist, from, until) in [
        ("C", "later", "2026-09-25T00:00:00Z", None),
        ("A", "middle", "2026-09-22T00:00:00Z", None),
        (
            "B",
            "overlap",
            "2026-09-19T00:00:00Z",
            Some("2026-09-24T00:00:00Z"),
        ),
        (
            "D",
            "closed",
            "2026-01-01T00:00:00Z",
            Some("2026-06-01T00:00:00Z"),
        ),
    ] {
        fx::put_lexical(&store, &item_with(id, gist, from, until), "unrelated");
    }
    store
}

#[test]
fn a_relative_query_ranks_its_window_candidates() {
    let store = seeded_store();
    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&request(WINDOW_QUERY, Some(ANCHOR)))
        .expect("retrieve");

    // Interval start descending: 09-25, 09-22, then the open-ended 09-19 item
    // that overlaps the window from before it.
    assert_eq!(ids(&out), vec!["C", "A", "B"]);
    assert_eq!(out.temporal_candidates, Some(3));
    let ranks: Vec<Option<usize>> = out.hits.iter().map(|h| h.temporal_rank).collect();
    assert_eq!(ranks, vec![Some(1), Some(2), Some(3)]);
    // The arm is rank-only: it contributes no score of any kind.
    assert!(
        out.hits
            .iter()
            .all(|h| h.scores.semantic.is_none() && h.scores.keyword.is_none())
    );
}

#[test]
fn the_same_query_and_anchor_are_deterministic() {
    let store = seeded_store();
    let retriever = fx::retriever(&store);
    let first = retriever
        .retrieve(&request(WINDOW_QUERY, Some(ANCHOR)))
        .expect("retrieve");
    let second = retriever
        .retrieve(&request(WINDOW_QUERY, Some(ANCHOR)))
        .expect("retrieve");
    // Compare the ranked page and the arm counter, not the wall-clock metrics.
    let (first, second) = (
        serde_json::to_value(&first).expect("json"),
        serde_json::to_value(&second).expect("json"),
    );
    assert_eq!(first["hits"], second["hits"], "same ranked page");
    assert_eq!(
        first["temporal_candidates"], second["temporal_candidates"],
        "same arm participation"
    );
}

#[test]
fn a_relative_query_without_an_anchor_is_a_no_op() {
    let store = seeded_store();
    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&request(WINDOW_QUERY, None))
        .expect("retrieve");
    assert!(out.hits.is_empty(), "hits: {:?}", ids(&out));
    assert_eq!(out.temporal_candidates, None);
}

#[test]
fn an_unsupported_or_ambiguous_expression_is_a_no_op() {
    let store = seeded_store();
    let retriever = fx::retriever(&store);
    for query in [
        "what happened last quarter",
        "today or this week",
        "what happened recently",
    ] {
        let out = retriever
            .retrieve(&request(query, Some(ANCHOR)))
            .expect("retrieve");
        assert!(out.hits.is_empty(), "{query}: {:?}", ids(&out));
        assert_eq!(out.temporal_candidates, None, "{query}");
    }
}

#[test]
fn a_malformed_anchor_fails_closed() {
    let store = seeded_store();
    let retriever = fx::retriever(&store);
    let error = retriever
        .retrieve(&request("this week", Some("2026-09-26")))
        .expect_err("a bare date must be rejected");
    assert_eq!(error.code, clio_types::ErrorCode::InvalidArgument);
}

#[test]
fn a_non_temporal_call_keeps_its_pre_arm_wire_shape() {
    let store = seeded_store();
    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&request("postgres connection pool", None))
        .expect("retrieve");
    let value = serde_json::to_value(&out).expect("json");
    assert!(value.get("temporal_candidates").is_none(), "{value}");
    let hits = value["hits"].as_array().expect("hits array");
    assert!(
        hits.iter().all(|hit| hit.get("temporal_rank").is_none()),
        "{value}"
    );
}

#[test]
fn the_arm_composes_with_as_of_without_widening_it() {
    let store = fx::store();
    fx::put_lexical(
        &store,
        &item_with("A", "visible", "2026-09-22T00:00:00Z", None),
        "unrelated",
    );
    let retriever = fx::retriever(&store);

    let mut visible = request("this week", Some(ANCHOR));
    visible.as_of = Some("2026-09-23T00:00:00Z".into());
    let out = retriever.retrieve(&visible).expect("retrieve");
    assert_eq!(ids(&out), vec!["A"]);
    assert_eq!(out.temporal_candidates, Some(1));

    // Before the item's validity start the point filter drops it, and the arm
    // reports that it ran with nothing kept rather than resurrecting the item.
    let mut hidden = request("this week", Some(ANCHOR));
    hidden.as_of = Some("2026-09-01T00:00:00Z".into());
    let out = retriever.retrieve(&hidden).expect("retrieve");
    assert!(out.hits.is_empty(), "hits: {:?}", ids(&out));
    assert_eq!(out.temporal_candidates, Some(0));
}

#[test]
fn the_transaction_axis_reads_transaction_intervals() {
    let store = fx::store();
    // Closed before the window on the valid axis; first recorded inside it on
    // the transaction axis.
    let mut recorded = item_with(
        "T",
        "recorded",
        "2026-01-01T00:00:00Z",
        Some("2026-06-01T00:00:00Z"),
    );
    recorded.tx_from = "2026-09-22T00:00:00Z".into();
    fx::put_lexical(&store, &recorded, "unrelated");
    let retriever = fx::retriever(&store);

    let mut valid = request("this week", Some(ANCHOR));
    let out = retriever.retrieve(&valid).expect("retrieve");
    assert!(out.hits.is_empty(), "valid axis: {:?}", ids(&out));

    valid.time_axis = TimeAxis::Transaction;
    let out = retriever.retrieve(&valid).expect("retrieve");
    assert_eq!(ids(&out), vec!["T"]);
    assert_eq!(out.temporal_candidates, Some(1));
}

#[test]
fn a_superseded_item_is_never_a_window_candidate() {
    let store = fx::store();
    let mut superseded = item_with("S", "superseded", "2026-09-22T00:00:00Z", None);
    superseded.tx_until = Some("2026-09-23T00:00:00Z".into());
    fx::put_lexical(&store, &superseded, "unrelated");
    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&request("this week", Some(ANCHOR)))
        .expect("retrieve");
    assert!(out.hits.is_empty(), "hits: {:?}", ids(&out));
    assert_eq!(out.temporal_candidates, Some(0));
}

#[test]
fn domain_and_budget_scope_still_bound_the_arm() {
    let store = seeded_store();
    let retriever = fx::retriever(&store);

    let mut scoped = request("this week", Some(ANCHOR));
    scoped.domains = vec!["persona".into()];
    let out = retriever.retrieve(&scoped).expect("retrieve");
    assert!(out.hits.is_empty(), "hits: {:?}", ids(&out));
    assert_eq!(out.temporal_candidates, Some(0));

    let mut budgeted = request("this week", Some(ANCHOR));
    budgeted.budget_tokens = Some(0);
    let out = retriever.retrieve(&budgeted).expect("retrieve");
    assert_eq!(ids(&out), vec!["C"], "only the always-admitted first hit");
    assert_eq!(out.temporal_candidates, Some(3));
}

#[test]
fn the_candidate_fetch_limit_bounds_the_arm() {
    let store = seeded_store();
    let mut retriever = fx::retriever(&store);
    retriever.set_fusion(60, 1, 60);
    let out = retriever
        .retrieve(&request("this week", Some(ANCHOR)))
        .expect("retrieve");
    assert_eq!(ids(&out), vec!["C"]);
    assert_eq!(out.temporal_candidates, Some(1));
}

#[test]
fn window_hits_fuse_alongside_a_lexical_hit() {
    let store = seeded_store();
    let matching = item_with(
        "E",
        "closed but matching",
        "2026-01-01T00:00:00Z",
        Some("2026-06-01T00:00:00Z"),
    );
    fx::put_lexical(&store, &matching, "ECONNRESET exact");
    let retriever = fx::retriever(&store);

    let out = retriever
        .retrieve(&request("ECONNRESET this week", Some(ANCHOR)))
        .expect("retrieve");
    let hits = &out.hits;
    let lexical = hits.iter().find(|h| h.hit.item_id == "E").expect("E");
    assert!(lexical.lexical_rank.is_some());
    assert!(lexical.temporal_rank.is_none(), "E is outside the window");
    let temporal = hits.iter().find(|h| h.hit.item_id == "C").expect("C");
    assert!(temporal.temporal_rank.is_some());
    assert!(temporal.lexical_rank.is_none());
    assert!(temporal.score > 0.0, "the temporal rank contributed");
    assert_eq!(out.temporal_candidates, Some(3));
}

#[test]
fn a_serialized_outcome_with_the_arm_carries_the_ranks() {
    let store = seeded_store();
    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&request("this week", Some(ANCHOR)))
        .expect("retrieve");
    let value: Value = serde_json::to_value(&out).expect("json");
    assert_eq!(value["temporal_candidates"], Value::from(3));
    assert_eq!(value["hits"][0]["temporal_rank"], Value::from(1));
}

#[test]
fn the_reported_temporal_rank_is_the_arms_fusion_rank() {
    let store = seeded_store();
    let matching = item_with("E", "in the window too", "2026-09-23T00:00:00Z", None);
    fx::put_lexical(&store, &matching, "ECONNRESET exact");
    let retriever = fx::retriever(&store);
    let out = retriever
        .retrieve(&request("ECONNRESET this week", Some(ANCHOR)))
        .expect("retrieve");

    // The arm's kept candidates in its documented order (interval start
    // descending, then id ascending): C 09-25, E 09-23, A 09-22, B 09-19.
    // Every hit must report its 1-based position in that list as its
    // `temporal_rank` — the hit the temporal arm and a search leg both
    // returned (E) and the temporal-only hits (C, A, B) alike.
    for (id, rank) in [("C", 1), ("E", 2), ("A", 3), ("B", 4)] {
        let hit = out.hits.iter().find(|h| h.hit.item_id == id).expect(id);
        assert_eq!(hit.temporal_rank, Some(rank), "{id}");
    }
}

#[test]
fn an_archived_newest_item_does_not_starve_the_arms_quota() {
    use clio_store::HygieneStore;
    let store = seeded_store();
    store
        .archive_item("bank", "C", "2026-09-26T12:00:00Z", "agent")
        .expect("archive the newest in-window item");
    let mut retriever = fx::retriever(&store);
    retriever.set_fusion(60, 2, 60);
    let out = retriever
        .retrieve(&request(WINDOW_QUERY, Some(ANCHOR)))
        .expect("retrieve");
    // The arm's fetch quota of 2 must be filled by live rows: the archived
    // newest item (C) never takes a quota slot, so A and B — not just A —
    // come back.
    assert_eq!(out.temporal_candidates, Some(2));
    assert_eq!(ids(&out), vec!["A", "B"]);
}
