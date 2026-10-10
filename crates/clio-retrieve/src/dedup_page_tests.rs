// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the page-aware narrowing tests: suppression and
//! the near-duplicate cap observe only the page the caller receives, including a
//! page bounded by the token budget and a page that grows through freed slots.
//!
//! ## Owns
//! - Fixtures where a consolidated parent falls outside the returned page
//! - Page-accurate near-duplicate counter assertions and backfill re-checks
//!
//! ## Does not own
//! - Production narrowing behavior (`dedup`) or the shared scope fixtures and
//!   flow reproduction (`dedup_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::dedup_tests::{cfg, hit, ids, map, parent, raw, run_with_budget};
use crate::hybrid_util::doc_text;
use crate::token::estimate_tokens;
use clio_config::retention::{DuplicateTolerance, RecallScope};
use clio_types::MemoryItem;

/// Token weight of one item exactly as the page selector measures it.
fn doc_tokens(item: &MemoryItem) -> usize {
    estimate_tokens(&doc_text(Some(item)))
}

#[test]
fn prefer_consolidated_keeps_raws_when_the_parent_falls_below_the_limit() {
    let items = map(vec![
        raw("A", "alpha bravo charlie delta"),
        raw("B", "alpha bravo charlie echo"),
        raw("U1", "alpha unrelated hotel"),
        raw("U2", "alpha unrelated india"),
        parent("P", "HUB", &["A", "B"], "alpha bravo charlie summary"),
    ]);
    let out = run_with_budget(
        vec![
            hit("A", 5.0),
            hit("B", 4.0),
            hit("U1", 3.0),
            hit("U2", 2.0),
            hit("P", 1.0),
        ],
        &items,
        3,
        None,
        cfg(RecallScope::Full, true, DuplicateTolerance::Balanced),
    );
    assert_eq!(ids(&out), vec!["A".to_owned(), "B".into(), "U1".into()]);
    assert_eq!(
        out.dedup.raw_suppressed, 0,
        "P is not in the returned page, so it claims nothing"
    );
    assert!(
        out.dedup.suppressed_raw_ids.is_empty(),
        "{:?}",
        out.dedup.suppressed_raw_ids
    );
}

#[test]
fn prefer_consolidated_keeps_raws_when_the_parent_falls_below_the_budget() {
    let a = raw("A", "alpha bravo charlie");
    let b = raw("B", "delta echo foxtrot");
    let items = map(vec![
        a.clone(),
        b.clone(),
        parent("P", "HUB", &["A", "B"], "golf hotel india"),
    ]);
    // Exactly the two raws fit: the parent is outside the budget-bounded page.
    let budget = doc_tokens(&a) + doc_tokens(&b);
    let out = run_with_budget(
        vec![hit("A", 3.0), hit("B", 2.0), hit("P", 1.0)],
        &items,
        10,
        Some(budget),
        cfg(RecallScope::Full, true, DuplicateTolerance::Balanced),
    );
    assert_eq!(ids(&out), vec!["A".to_owned(), "B".into()]);
    assert_eq!(out.dedup.raw_suppressed, 0);
}

#[test]
fn cap_rechecks_the_page_after_a_freed_slot_backfills_a_duplicate() {
    let items = map(vec![
        raw("A", "alpha bravo charlie delta echo"),
        raw("D", "alpha bravo charlie delta echo foxtrot"),
        raw("U", "alpha unrelated hotel india"),
        raw("C", "alpha bravo charlie delta echo golf"),
    ]);
    let out = run_with_budget(
        vec![hit("A", 5.0), hit("D", 4.0), hit("U", 3.0), hit("C", 2.0)],
        &items,
        3,
        None,
        cfg(RecallScope::Full, false, DuplicateTolerance::Strict),
    );
    assert_eq!(ids(&out), vec!["A".to_owned(), "U".into()]);
    assert_eq!(
        out.dedup.near_dup_suppressed, 2,
        "D is dropped from the first page and C, admitted by the freed slot, is dropped too"
    );
}

#[test]
fn page_narrowing_suppresses_a_raw_that_only_the_backfill_admits() {
    let items = map(vec![
        raw("X", "alpha bravo charlie delta echo"),
        parent("P", "HUB", &["R"], "alpha hub summary"),
        raw("D", "alpha bravo charlie delta echo foxtrot"),
        raw("U", "alpha unrelated hotel india"),
        raw("R", "alpha bravo charlie golf"),
    ]);
    let out = run_with_budget(
        vec![
            hit("X", 5.0),
            hit("P", 4.0),
            hit("D", 3.0),
            hit("U", 2.0),
            hit("R", 1.0),
        ],
        &items,
        4,
        None,
        cfg(RecallScope::Full, true, DuplicateTolerance::Strict),
    );
    // D is capped (a near-duplicate of X, not claimed by any parent), R enters
    // through the freed slot, and the next pass suppresses R because the
    // returned parent P claims it.
    assert_eq!(ids(&out), vec!["X".to_owned(), "P".into(), "U".into()]);
    assert_eq!(out.dedup.raw_suppressed, 1);
    assert_eq!(out.dedup.suppressed_raw_ids, vec!["R".to_owned()]);
    assert_eq!(out.dedup.near_dup_suppressed, 1);
}

#[test]
fn near_dup_counter_counts_only_returned_page_items() {
    let items = map(vec![
        raw("A", "alpha bravo charlie delta echo"),
        raw("B", "alpha bravo charlie delta echo foxtrot"),
        raw("C", "alpha bravo charlie delta echo golf"),
    ]);
    let fused = vec![hit("A", 3.0), hit("B", 2.0), hit("C", 1.0)];
    let one = run_with_budget(
        fused.clone(),
        &items,
        1,
        None,
        cfg(RecallScope::Full, false, DuplicateTolerance::Strict),
    );
    assert_eq!(ids(&one), vec!["A".to_owned()]);
    assert_eq!(
        one.dedup.near_dup_suppressed, 0,
        "B and C never appear in a one-item page, so they are not page suppressions"
    );
    let two = run_with_budget(
        fused,
        &items,
        2,
        None,
        cfg(RecallScope::Full, false, DuplicateTolerance::Strict),
    );
    assert_eq!(ids(&two), vec!["A".to_owned()]);
    assert_eq!(two.dedup.near_dup_suppressed, 2);
}
