// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the synthetic gist a distilled unit
//! carries: a short cluster keeps its whole summary, and a large one is cut to
//! the store's published bound instead of failing the write.
//!
//! ## Owns
//! - The bound and the whole-summary assertions for `distill_gist`
//!
//! ## Does not own
//! - The snapshot projection or the write itself (see `hub_distill_model`)
//!
//! ## Boundary
//!
//! Keep this file focused on the synthetic gist.

use super::*;

/// A minimal item carrying one gist, for a synthetic cluster.
fn item(id: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: "bank".into(),
        subject_id: "subject".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        context: None,
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

#[test]
fn a_large_cluster_is_cut_to_the_store_bound() {
    let hub = item("hub", &"h".repeat(GIST_MAX_CHARS));
    let sources: Vec<MemoryItem> = (0..DEFAULT_HUB_NEIGHBORS)
        .map(|index| item(&format!("s{index}"), &"n".repeat(200)))
        .collect();

    let gist = distill_gist(&hub, &sources);

    assert_eq!(gist.chars().count(), GIST_MAX_CHARS);
    assert!(gist.starts_with("hub_distill: "), "{gist}");
}

#[test]
fn a_short_cluster_keeps_its_whole_summary() {
    let gist = distill_gist(&item("hub", "short hub"), &[item("s", "short source")]);
    assert_eq!(gist, "hub_distill: short hub related to short source");
}
