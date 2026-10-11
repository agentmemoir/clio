// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the dedup/scope unit tests: the consolidated
//! eligibility predicate, both recall scopes, prefer-consolidated supersede plus
//! backfill, the near-duplicate cap, counters, and the graceful no-links path.
//!
//! ## Owns
//! - Scope/dedup fixtures and assertions over `dedupe_candidates` + `finalize`
//!
//! ## Does not own
//! - Production dedup behavior or transport-level scope resolution
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use crate::finalize::{FinalizeArgs, finalize, select_page};
use crate::fixtures_tests::{item as fx_item, semantic_item};
use crate::fusion::ArmSignal;
use crate::types::{RetrieveMetrics, RetrieveOutcome, RetrieveRequest};
use clio_config::retention::{DuplicateTolerance, RecallScope};
use clio_types::{ItemKind, MemoryItem};
use serde_json::json;
use std::collections::BTreeMap;

pub(super) fn hit(id: &str, score: f64) -> FusedHit {
    FusedHit {
        id: id.into(),
        score,
        arms: vec![ArmSignal::default(); 2],
        rerank_score: None,
        age_factor: 1.0,
        trust_factor: 1.0,
    }
}

/// Semantic raw item (no consolidated marker).
pub(super) fn raw(id: &str, gist: &str) -> MemoryItem {
    semantic_item(id, "bank", gist)
}

/// Hub-distilled consolidated parent claiming `hub` + `neighbors`.
pub(super) fn parent(id: &str, hub: &str, neighbors: &[&str], gist: &str) -> MemoryItem {
    let mut it = semantic_item(id, "bank", gist);
    it.source_ref = Some(HUB_DISTILL_SOURCE_REF.into());
    it.snapshot = Some(json!({
        "hub_id": hub,
        "neighbor_ids": neighbors,
        "neighbor_count": neighbors.len(),
    }));
    it
}

pub(super) fn map(items: Vec<MemoryItem>) -> BTreeMap<String, MemoryItem> {
    items.into_iter().map(|i| (i.id.clone(), i)).collect()
}

fn request(limit: usize, budget: Option<usize>, cfg: DedupConfig) -> RetrieveRequest {
    let mut req = RetrieveRequest::new("bank", "q");
    req.limit = limit;
    req.budget_tokens = budget;
    req.recall_scope = cfg.scope;
    req.prefer_consolidated = cfg.prefer_consolidated;
    req.duplicate_tolerance_read = cfg.tolerance;
    req
}

/// Reproduce the hybrid scope/dedup/finalize flow over a hand-built rank order.
pub(super) fn run(
    fused: Vec<FusedHit>,
    items: &BTreeMap<String, MemoryItem>,
    limit: usize,
    cfg: DedupConfig,
) -> RetrieveOutcome {
    run_with_budget(fused, items, limit, None, cfg)
}

/// The same flow with an explicit token budget (the compose memory section path).
pub(super) fn run_with_budget(
    fused: Vec<FusedHit>,
    items: &BTreeMap<String, MemoryItem>,
    limit: usize,
    budget: Option<usize>,
    cfg: DedupConfig,
) -> RetrieveOutcome {
    let deduped = dedupe_candidates(fused, items, cfg, limit, budget);
    let backfill_baseline = select_page(&deduped.scope_page, items, limit, budget);
    let DedupedCandidates { hits, meta, .. } = deduped;
    let req = request(limit, budget, cfg);
    finalize(FinalizeArgs {
        req: &req,
        items,
        dense_rank: &BTreeMap::new(),
        lexical_rank: &BTreeMap::new(),
        fused: hits,
        dense_candidates: 0,
        lexical_candidates: 0,
        temporal_candidates: None,
        reranked: false,
        graph_expanded: 0,
        metrics: RetrieveMetrics {
            dense_ms: 0,
            lexical_ms: 0,
            expand_ms: 0,
            rerank_ms: 0,
            total_ms: 0,
        },
        warnings: Vec::new(),
        floor_binding: Vec::new(),
        dedup: meta,
        backfill_baseline,
    })
}

pub(super) fn ids(out: &RetrieveOutcome) -> Vec<String> {
    out.hits.iter().map(|h| h.hit.item_id.clone()).collect()
}

pub(super) fn cfg(scope: RecallScope, prefer: bool, tolerance: DuplicateTolerance) -> DedupConfig {
    DedupConfig {
        scope,
        prefer_consolidated: prefer,
        tolerance,
    }
}

#[test]
fn t27_01_consolidated_only_returns_parent_only() {
    let items = map(vec![
        parent("P", "HUB", &["A", "B"], "parent summary"),
        raw("A", "raw alpha"),
        raw("B", "raw bravo"),
        raw("U", "unrelated charlie"),
    ]);
    let out = run(
        vec![hit("P", 3.0), hit("A", 2.5), hit("B", 2.0), hit("U", 1.0)],
        &items,
        10,
        cfg(
            RecallScope::ConsolidatedOnly,
            false,
            DuplicateTolerance::Balanced,
        ),
    );
    assert_eq!(ids(&out), vec!["P".to_owned()]);
    assert_eq!(out.dedup.scope_applied, "consolidated_only");
    assert!(!out.dedup.scope_empty);
    // Eligibility and source links are visible on the returned hit metadata.
    assert!(out.hits[0].consolidated);
    assert_eq!(
        out.hits[0].source_ids,
        vec!["A".to_owned(), "B".into(), "HUB".into()]
    );
}

#[test]
fn raw_hits_omit_consolidated_metadata() {
    let items = map(vec![raw("A", "raw alpha")]);
    let out = run(
        vec![hit("A", 1.0)],
        &items,
        10,
        cfg(RecallScope::Full, false, DuplicateTolerance::Balanced),
    );
    assert!(!out.hits[0].consolidated);
    assert!(
        out.hits[0].source_ids.is_empty(),
        "{:?}",
        out.hits[0].source_ids
    );
}

#[test]
fn t27_04_consolidated_only_without_parents_is_empty_and_signalled() {
    let items = map(vec![raw("A", "raw alpha"), raw("U", "unrelated")]);
    let out = run(
        vec![hit("A", 2.0), hit("U", 1.0)],
        &items,
        10,
        cfg(
            RecallScope::ConsolidatedOnly,
            false,
            DuplicateTolerance::Balanced,
        ),
    );
    assert!(out.hits.is_empty(), "never silently widens to full");
    assert!(out.dedup.scope_empty, "scope_empty signal required");
    assert_eq!(out.dedup.raw_suppressed, 0);
}

#[test]
fn t27_02_prefer_consolidated_suppresses_raws_and_backfills() {
    let items = map(vec![
        parent("P", "HUB", &["A", "B"], "parent summary"),
        raw("A", "raw alpha"),
        raw("B", "raw bravo"),
        raw("U1", "unrelated hotel india"),
        raw("U2", "unrelated mike november"),
        raw("U3", "unrelated romeo sierra"),
    ]);
    let fused = vec![
        hit("P", 6.0),
        hit("A", 5.5),
        hit("B", 5.0),
        hit("U1", 4.0),
        hit("U2", 3.0),
        hit("U3", 2.0),
    ];
    let out = run(
        fused.clone(),
        &items,
        5,
        cfg(RecallScope::Full, true, DuplicateTolerance::Balanced),
    );
    assert_eq!(
        ids(&out),
        vec!["P".to_owned(), "U1".into(), "U2".into(), "U3".into()]
    );
    assert_eq!(out.dedup.raw_suppressed, 2);
    assert_eq!(
        out.dedup.suppressed_raw_ids,
        vec!["A".to_owned(), "B".to_owned()]
    );
    assert_eq!(out.dedup.backfilled, 1, "U3 entered through a freed slot");
    assert_eq!(out.dedup.backfilled_ids, vec!["U3".to_owned()]);
    assert!(
        out.dedup.backfill_exhausted,
        "only one spare candidate remained for two freed slots"
    );
    // Choice: reinforcement observes the pre-dedup page, so the suppressed
    // raw sources still reinforce even though they are not returned.
    assert_eq!(
        select_page(&fused, &items, 5, None),
        vec![
            "P".to_owned(),
            "A".into(),
            "B".into(),
            "U1".into(),
            "U2".into()
        ]
    );

    // Determinism: identical candidate set plus config yields identical order.
    let again = run(
        fused,
        &items,
        5,
        cfg(RecallScope::Full, true, DuplicateTolerance::Balanced),
    );
    assert_eq!(ids(&again), ids(&out));
}

#[test]
fn t27_03_full_without_prefer_returns_everything() {
    let items = map(vec![
        parent("P", "HUB", &["A", "B"], "parent summary"),
        raw("A", "raw alpha"),
        raw("B", "raw bravo"),
        raw("U", "unrelated charlie"),
    ]);
    let out = run(
        vec![hit("P", 4.0), hit("A", 3.0), hit("B", 2.0), hit("U", 1.0)],
        &items,
        10,
        cfg(RecallScope::Full, false, DuplicateTolerance::Balanced),
    );
    assert_eq!(
        ids(&out),
        vec!["P", "A", "B", "U"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(out.dedup.raw_suppressed, 0);
    assert!(
        out.dedup.is_inert(),
        "plain full retrieval keeps the earlier response shape"
    );
}

#[test]
fn t27_06_paraphrase_trio_with_strict_tolerance_keeps_one() {
    let items = map(vec![
        raw("A", "alpha bravo charlie delta echo"),
        raw("B", "alpha bravo charlie delta echo foxtrot"),
        raw("C", "alpha bravo charlie delta echo golf"),
    ]);
    let out = run(
        vec![hit("A", 3.0), hit("B", 2.0), hit("C", 1.0)],
        &items,
        10,
        cfg(RecallScope::Full, false, DuplicateTolerance::Strict),
    );
    assert_eq!(ids(&out), vec!["A".to_owned()]);
    assert_eq!(out.dedup.near_dup_suppressed, 2);
}

#[test]
fn read_cap_levels_differ_between_strict_and_balanced() {
    // 5/6 token overlap = 0.833: over the strict 0.80 cap, under balanced 0.90.
    let items = map(vec![
        raw("A", "alpha bravo charlie delta echo"),
        raw("B", "alpha bravo charlie delta echo foxtrot"),
    ]);
    let strict = run(
        vec![hit("A", 2.0), hit("B", 1.0)],
        &items,
        10,
        cfg(RecallScope::Full, false, DuplicateTolerance::Strict),
    );
    assert_eq!(ids(&strict), vec!["A".to_owned()]);
    let balanced = run(
        vec![hit("A", 2.0), hit("B", 1.0)],
        &items,
        10,
        cfg(RecallScope::Full, true, DuplicateTolerance::Balanced),
    );
    assert_eq!(ids(&balanced), vec!["A".to_owned(), "B".to_owned()]);
    assert_eq!(balanced.dedup.near_dup_suppressed, 0);
}

#[test]
fn t27_11_no_links_prefer_is_a_noop_and_cap_handles_duplicates() {
    // No eligible parent anywhere: prefer_consolidated suppresses nothing.
    let items = map(vec![
        raw("A", "shared token"),
        raw("B", "shared token"),
        raw("U", "unrelated content"),
    ]);
    let out = run(
        vec![hit("A", 3.0), hit("B", 2.0), hit("U", 1.0)],
        &items,
        10,
        cfg(RecallScope::Full, true, DuplicateTolerance::Balanced),
    );
    assert_eq!(
        out.dedup.raw_suppressed, 0,
        "supersede is a no-op without links"
    );
    assert_eq!(
        out.dedup.exact_dup_suppressed, 1,
        "an identical row is reported as an exact duplicate, not a near one"
    );
    assert_eq!(ids(&out), vec!["A".to_owned(), "U".to_owned()]);
}

#[test]
fn t27_10_no_links_consolidated_only_is_empty_but_cap_still_applies_on_full() {
    let items = map(vec![raw("A", "shared token"), raw("B", "shared token")]);
    let only = run(
        vec![hit("A", 2.0), hit("B", 1.0)],
        &items,
        10,
        cfg(
            RecallScope::ConsolidatedOnly,
            false,
            DuplicateTolerance::Balanced,
        ),
    );
    assert!(only.hits.is_empty(), "{:?}", only.hits);
    assert!(only.dedup.scope_empty);

    let full = run(
        vec![hit("A", 2.0), hit("B", 1.0)],
        &items,
        10,
        cfg(RecallScope::Full, true, DuplicateTolerance::Balanced),
    );
    assert_eq!(full.dedup.exact_dup_suppressed, 1);
    assert_eq!(
        full.dedup.near_dup_suppressed, 0,
        "identical text is never counted as a near duplicate"
    );
    assert_eq!(full.dedup.scope_applied, "full");
}

#[test]
fn eligibility_predicate_and_source_links_are_total() {
    let p = parent("P", "HUB", &["A", "B"], "summary");
    assert!(is_consolidated_item(&p));
    let links = consolidated_source_ids(&p);
    assert!(links.contains("HUB") && links.contains("A") && links.contains("B"));

    // Orphan raw item resolves to no parent and no links.
    let orphan = raw("O", "orphan");
    assert!(!is_consolidated_item(&orphan));
    let orphan_links = consolidated_source_ids(&orphan);
    assert!(orphan_links.is_empty(), "{orphan_links:?}");

    // Unknown/missing shapes are not consolidated and never panic.
    let mut malformed = semantic_item("M", "bank", "malformed");
    malformed.source_ref = Some(HUB_DISTILL_SOURCE_REF.into());
    malformed.snapshot = Some(json!(["not", "an", "object"]));
    assert!(is_consolidated_item(&malformed));
    let malformed_links = consolidated_source_ids(&malformed);
    assert!(malformed_links.is_empty(), "{malformed_links:?}");

    let mut empty_ids = parent("E", "", &["", "A"], "empty hub");
    empty_ids.snapshot = Some(json!({"hub_id": "", "neighbor_ids": ["", "A", 3]}));
    let ids = consolidated_source_ids(&empty_ids);
    assert_eq!(ids.into_iter().collect::<Vec<_>>(), vec!["A".to_owned()]);

    // Episodic items are never consolidated even with the marker.
    let mut episodic = fx_item("X", "bank", "episodic");
    episodic.source_ref = Some(HUB_DISTILL_SOURCE_REF.into());
    assert!(!is_consolidated_item(&episodic));
    assert_eq!(episodic.kind, ItemKind::Episodic);
}

#[test]
fn read_cap_inactive_on_neutral_balanced_without_dedup_mode() {
    let neutral = cfg(RecallScope::Full, false, DuplicateTolerance::Balanced);
    assert!(!neutral.read_cap_active());
    assert!(cfg(RecallScope::Full, true, DuplicateTolerance::Balanced).read_cap_active());
    assert!(cfg(RecallScope::Full, false, DuplicateTolerance::Lenient).read_cap_active());
    assert!(
        cfg(
            RecallScope::ConsolidatedOnly,
            false,
            DuplicateTolerance::Balanced
        )
        .read_cap_active()
    );
    assert_eq!(DedupConfig::default(), neutral);
    assert!((DuplicateTolerance::Strict.read_similarity_cap() - 0.80).abs() < 1e-12);
}
