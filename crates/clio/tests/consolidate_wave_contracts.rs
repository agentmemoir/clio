// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the cross-crate contract pins around the
//! consolidate wave: the provenance marker the writer stores and the one the
//! recall predicate matches, the stale reason the discard cascade writes and
//! the one recall withholds on, and the edge relationship the writer emits
//! and the erasure purge closures match.
//!
//! ## Owns
//! - The marker, stale-reason, and edge-relationship parity assertions
//! - The seeded behavioral pin that a row a real wave distils is the one the
//!   recall predicate accepts
//!
//! ## Does not own
//! - Wave execution and storage assertions (`consolidate_wave_harness`)
//! - The fixture (`consolidate_wave_support`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

/// The writer's marker and the read predicate's marker are one contract: a
/// rename on either side alone silently drops freshly distilled units out of
/// `consolidated_only`, which is the regression the split exists to prevent.
#[test]
fn the_distillation_marker_the_writer_writes_is_the_one_recall_accepts() {
    assert_eq!(
        clio_write::HUB_DISTILL_MARKER,
        clio_retrieve::HUB_DISTILL_SOURCE_REF,
        "the write site and the recall predicate must hold the same literal"
    );
    assert_ne!(
        clio_write::HUB_DISTILL_MARKER,
        clio_write::HUB_DISTILL_JOB,
        "the provenance marker is not the maintenance job kind"
    );
    assert_ne!(
        clio_retrieve::HUB_DISTILL_SOURCE_REF,
        clio_write::HUB_DISTILL_JOB,
        "the recall accepted marker must not be the maintenance job kind"
    );
    // The stale reason the discard cascade writes is the one the recall
    // predicate withholds on: a rename on either side alone would either
    // leak a removed source's text through `consolidated_only` or withhold
    // units recall should still return.
    assert_eq!(
        clio_store::STALE_SOURCE_REMOVED,
        clio_retrieve::STALE_SOURCE_REMOVED,
        "the cascade's stale reason and the recall predicate must hold the same literal"
    );
}

/// The edge relationship the writer emits and the one both erase purge
/// closures match are one contract: a rename on either side alone leaves the
/// erasure purge unable to find the hub-distill edges, so the purge
/// regression tests stay green while production stops purging.
#[test]
fn the_edge_relationship_the_writer_emits_is_the_one_the_erase_purge_matches() {
    assert_eq!(
        clio_store::HUB_DISTILL_EDGE_RELATIONSHIP,
        clio_write::HUB_DISTILL_JOB,
        "the writer emits this relationship on its cross-reference edges and \
         both erase purge closures match it"
    );
}

mod common;
#[path = "consolidate_wave_support/mod.rs"]
mod consolidate_wave_support;

use consolidate_wave_support::{McpSession, distilled_bank, hit_ids};
use serde_json::json;

/// The query the seeded notes share, so the page is about eligibility and not
/// about which text happens to rank.
const QUERY: &str = "kubernetes staging rollout";

/// The seeded behavioral pin for the marker contract: a unit a real wave
/// distilled through the CLI is a row the recall predicate accepts — the
/// consolidated scope returns it as a consolidated unit. The removed
/// legacy-literal parity pin guarded the retired `hub_distill` compatibility
/// constant, which no constant exists for any more; what the contract means for
/// a stored row is pinned here on real seed data instead of on a literal.
#[test]
fn a_row_a_real_wave_distils_is_the_one_recall_accepts() {
    let (f, unit) = distilled_bank();
    f.keep_only_unit(&unit);

    let mut session = McpSession::open(&f);
    let only = session.call(
        2,
        "retrieve",
        json!({"query": QUERY, "recall_scope": "consolidated_only", "limit": 20}),
    );
    assert_eq!(
        hit_ids(&only),
        vec![unit],
        "the distilled unit is the whole consolidated page: {only}"
    );
    assert_eq!(
        only["hits"][0]["consolidated"],
        json!(true),
        "and it is classified as a consolidated unit: {only}"
    );
    assert_eq!(
        only["dedup"]["scope_empty"],
        json!(false),
        "the scope is not empty: {only}"
    );
    assert!(
        session.finish().success(),
        "the MCP session must exit cleanly"
    );
}
