// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the provenance-marker tests: the predicate
//! accepts only the current marker, a current row stays visible under
//! `consolidated_only`, a row carrying the retired job-kind literal does not,
//! and a canonical-only bank stays empty.
//!
//! ## Owns
//! - Marker literals and the eligibility assertions over a hand-built row
//!
//! ## Does not own
//! - Production narrowing behavior (`dedup`) or the shared scope fixtures and
//!   flow reproduction (`dedup_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::dedup_tests::{cfg, hit, ids, map, raw, run};
use super::{DedupConfig, HUB_DISTILL_SOURCE_REF, is_consolidated_item};
use clio_config::retention::{DuplicateTolerance, RecallScope};
use clio_types::MemoryItem;
use serde_json::json;

/// The retired literal an earlier writer stored in `source_ref` (the old
/// job-kind value). The predicate no longer exposes it as a constant, so the
/// negative case pins the exact string here.
const RETIRED_HUB_DISTILL_SOURCE_REF: &str = "hub_distill";

/// A consolidated parent carrying the given `source_ref` value.
fn marked(id: &str, marker: &str, hub: &str, neighbors: &[&str]) -> MemoryItem {
    let mut item = raw(id, "parent summary");
    item.source_ref = Some(marker.into());
    item.snapshot = Some(json!({
        "hub_id": hub,
        "neighbor_ids": neighbors,
        "neighbor_count": neighbors.len(),
    }));
    item
}

fn only_scope() -> DedupConfig {
    cfg(
        RecallScope::ConsolidatedOnly,
        false,
        DuplicateTolerance::Balanced,
    )
}

#[test]
fn the_predicate_accepts_the_current_marker_and_rejects_a_canonical_row() {
    assert_eq!(HUB_DISTILL_SOURCE_REF, "hub_distilled");

    let current = marked("CUR", HUB_DISTILL_SOURCE_REF, "HUB", &["A"]);
    assert!(is_consolidated_item(&current));

    // Canonical consolidation keeps a provenance string, not this marker, so it
    // stays ineligible: a provenance string is not a consolidated unit here.
    let mut canonical = raw("CAN", "canonical merge");
    canonical.source_ref = Some("turn-1,turn-2".into());
    assert!(!is_consolidated_item(&canonical));
}

#[test]
fn the_retired_job_kind_literal_is_not_a_consolidated_marker() {
    let retired = marked("OLD", RETIRED_HUB_DISTILL_SOURCE_REF, "HUB", &["A"]);
    assert!(
        !is_consolidated_item(&retired),
        "the retired literal must not classify as consolidated"
    );
}

#[test]
fn a_current_row_is_still_returned_by_consolidated_only() {
    let items = map(vec![
        marked("CUR", HUB_DISTILL_SOURCE_REF, "HUB", &["A"]),
        raw("A", "raw alpha"),
    ]);
    let out = run(
        vec![hit("CUR", 2.0), hit("A", 1.0)],
        &items,
        10,
        only_scope(),
    );
    assert_eq!(ids(&out), vec!["CUR".to_owned()]);
    assert!(
        !out.dedup.scope_empty,
        "a current row must not read as an empty scope"
    );
    assert!(out.hits[0].consolidated);
    assert_eq!(
        out.hits[0].source_ids,
        vec!["A".to_owned(), "HUB".into()],
        "the links come from the snapshot, untouched by the literal retirement"
    );
}

#[test]
fn a_retired_literal_row_is_not_returned_by_consolidated_only() {
    let items = map(vec![marked(
        "OLD",
        RETIRED_HUB_DISTILL_SOURCE_REF,
        "HUB",
        &["A"],
    )]);
    let out = run(vec![hit("OLD", 2.0)], &items, 10, only_scope());
    assert!(
        out.hits.is_empty(),
        "the retired literal is not a consolidated unit"
    );
    assert!(
        out.dedup.scope_empty,
        "a bank of retired-literal rows reads as an empty scope"
    );
    assert_eq!(out.dedup.raw_suppressed, 0);
}

#[test]
fn a_canonical_only_bank_keeps_returning_an_empty_scope() {
    let mut canonical = raw("CAN", "canonical merge");
    canonical.source_ref = Some("turn-1,turn-2".into());
    canonical.snapshot = Some(json!({"chunk_ids": ["c1", "c2"]}));
    let items = map(vec![canonical]);
    let out = run(vec![hit("CAN", 1.0)], &items, 10, only_scope());
    assert!(out.hits.is_empty(), "never silently widens to full");
    assert!(out.dedup.scope_empty, "scope_empty signal required");
    assert_eq!(out.dedup.raw_suppressed, 0);
}
