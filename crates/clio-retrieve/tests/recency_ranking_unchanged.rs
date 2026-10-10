// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the integration-level recency contract: the
//! public `recency_score` still produces the published tau-30 curve values at
//! 30 and 90 days, and the younger of two items ranks above the older one.
//!
//! ## Owns
//! - The public-API recency pins at 30 days (0.5) and 90 days (0.125) and the
//!   youngest-first ranking they induce
//!
//! ## Does not own
//! - The decay arithmetic itself (`clio_types::decay::half_life_factor`)
//! - The full 7/30/90/365-day row and the recency edge cases (see this
//!   crate's `filter_tests` module)
//!
//! ## Boundary
//!
//! Changes to the public recency score or the published curve should normally
//! require changes here. Changes to unrelated retrieval behavior should not
//! require changes here.
//!
//! Keep this module focused on the unchanged-curve ranking contract.

use clio_retrieve::recency_score;
use clio_types::{EpisodicType, EpistemicKind, ItemKind, MemoryItem};

/// Episodic fixture mirroring this crate's in-crate `fixtures_tests::item`,
/// with both timestamps at the given ISO instant.
fn item(id: &str, at: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: "b".into(),
        subject_id: "subj-b".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Task),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.6),
        source_ref: Some("turn_1".into()),
        snapshot: None,
        gist: Some("gist".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: at.into(),
        updated_at: at.into(),
        context: None,
    }
}

/// Where the decay curve is unchanged, retrieval ranking is unchanged: at
/// tau 30 a 30-day-old item scores one half-life (0.5) and a 90-day-old item
/// scores three (0.125), and the younger item ranks above the older.
#[test]
fn retrieval_ranking_is_unchanged_where_the_recency_curve_is_unchanged() {
    let now = "2026-04-01T00:00:00Z";
    let younger = item("younger", "2026-03-02T00:00:00Z"); // exactly 30 days
    let older = item("older", "2026-01-01T00:00:00Z"); // exactly 90 days

    let younger_score = recency_score(&younger, now, 30.0);
    let older_score = recency_score(&older, now, 30.0);

    // 0.5 and 0.125 are exact curve points; the tolerance only absorbs binary
    // floating-point noise in the exponential.
    assert!(
        (younger_score - 0.5).abs() < 1e-9,
        "30 days at tau 30: expected 0.5, got {younger_score}"
    );
    assert!(
        (older_score - 0.125).abs() < 1e-9,
        "90 days at tau 30: expected 0.125, got {older_score}"
    );

    // Ranking follows the curve: the item nearer `now` outranks the older one.
    assert!(
        younger_score > older_score,
        "younger ({younger_score}) must rank above older ({older_score})"
    );
}
