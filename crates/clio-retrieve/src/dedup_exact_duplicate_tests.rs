// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//!
//! # Responsibility
//!
//! Tests that a default-configuration recall never returns the same text twice,
//! because exact-duplicate collapse is unconditional.
//!
//! ## Owns
//! - Exact-duplicate collapse under the shipped default `DedupConfig`
//! - The `effective_cap` floor that keeps near-duplicates out of scope
//!
//! ## Does not own
//! - Tolerance-level near-duplicate caps (see `dedup_tests`)
//!
//! ## Boundary
//!
//! Changes to unconditional read-time collapse should normally require changes
//! here. Test-only; never compiled into production builds.

use super::dedup_tests::{cfg, hit, ids, map, raw, run};
use super::*;
use crate::dedup_cap::{EXACT_DUPLICATE_CAP, effective_cap, normalize_exact};
use crate::hybrid_util::doc_text;
use clio_config::retention::{DuplicateTolerance, RecallScope};

/// The default configuration a caller gets with no configuration at all.
fn neutral() -> DedupConfig {
    DedupConfig::default()
}

#[test]
fn the_default_config_never_returns_the_same_text_twice() {
    assert!(
        !neutral().read_cap_active(),
        "the near-duplicate cap is still off"
    );

    let items = map(vec![
        raw("A", "User prefers dark mode"),
        raw("B", "User prefers dark mode"),
        raw("C", "User prefers dark mode"),
    ]);
    let fused = vec![hit("A", 0.9), hit("B", 0.8), hit("C", 0.7)];

    let out = run(fused, &items, 5, neutral());

    assert_eq!(ids(&out), vec!["A".to_owned()], "one representative only");
    assert_eq!(
        out.dedup.exact_dup_suppressed, 2,
        "identical rows are reported separately from near duplicates"
    );
    assert_eq!(
        out.dedup.near_dup_suppressed, 0,
        "the default configuration widens the cap to nothing"
    );
}

#[test]
fn exact_collapse_ignores_case_and_punctuation() {
    let items = map(vec![
        raw("A", "User prefers dark mode"),
        raw("B", "user prefers dark mode!"),
    ]);
    let fused = vec![hit("A", 0.9), hit("B", 0.8)];

    let out = run(fused, &items, 5, neutral());

    assert_eq!(ids(&out), vec!["A".to_owned()]);
}

#[test]
fn word_order_makes_a_different_fact() {
    // These share every word, so a token-set comparison would call them the
    // same sentence. They are not, and dropping either one would lose a fact.
    let items = map(vec![
        raw("A", "Tom walks with Alice"),
        raw("B", "Alice walks with Tom"),
    ]);
    let fused = vec![hit("A", 0.9), hit("B", 0.8)];

    let out = run(fused, &items, 5, neutral());

    assert_eq!(ids(&out).len(), 2, "word order is part of the fact");
    assert_eq!(out.dedup.near_dup_suppressed, 0);
}

#[test]
fn the_default_config_keeps_near_duplicates() {
    // These share most words but are different facts. Collapsing them is the
    // tolerance cap's job, and the default does not run it.
    let items = map(vec![
        raw("A", "User prefers dark mode"),
        raw("B", "User prefers dark mode in the terminal"),
    ]);
    let fused = vec![hit("A", 0.9), hit("B", 0.8)];

    let out = run(fused, &items, 5, neutral());

    assert_eq!(
        ids(&out).len(),
        2,
        "near-duplicates are not exact duplicates"
    );
    assert_eq!(out.dedup.exact_dup_suppressed, 0);
    assert_eq!(out.dedup.near_dup_suppressed, 0);
}

#[test]
fn distinct_facts_all_survive_the_default_config() {
    let items = map(vec![
        raw("A", "User prefers dark mode"),
        raw("B", "User prefers light mode"),
        raw("C", "Tom walks on Mondays"),
    ]);
    let fused = vec![hit("A", 0.9), hit("B", 0.8), hit("C", 0.7)];

    let out = run(fused, &items, 5, neutral());

    assert_eq!(ids(&out).len(), 3);
    assert_eq!(out.dedup.exact_dup_suppressed, 0);
    assert_eq!(out.dedup.exact_dup_suppressed, 0);
    assert_eq!(out.dedup.near_dup_suppressed, 0);
}

#[test]
fn the_inactive_cap_becomes_exact_only_collapse() {
    assert!((effective_cap(neutral()) - EXACT_DUPLICATE_CAP).abs() < 1e-12);
    assert!((EXACT_DUPLICATE_CAP - 1.0).abs() < 1e-12);
}

#[test]
fn an_active_tolerance_keeps_its_own_wider_cap() {
    for (tolerance, expected) in [
        (DuplicateTolerance::Strict, 0.80),
        (DuplicateTolerance::Balanced, 0.90),
        (DuplicateTolerance::Lenient, 0.95),
    ] {
        let active = cfg(RecallScope::Full, true, tolerance);
        assert!(active.read_cap_active());
        assert!(
            (effective_cap(active) - expected).abs() < 1e-12,
            "{tolerance:?}"
        );
    }
}

#[test]
fn no_tolerance_level_ever_returns_two_equal_texts_on_one_page() {
    // The guarantee in issue #41: whatever tolerance a bank is configured for,
    // and whether or not the widened near-duplicate cap is active, a page must
    // never carry two items with equal normalized text. Without this, a future
    // tolerance level could reintroduce repeated text and a fixture built from
    // one shared gist would hide it — which is exactly how the duplicate
    // defect shipped in the first place.
    let items = map(vec![
        raw("A", "User prefers dark mode"),
        raw("B", "user  prefers dark mode!"),
        raw("C", "Tom walks on Mondays"),
        raw("D", "User prefers dark mode"),
    ]);
    let fused = vec![hit("A", 0.9), hit("B", 0.85), hit("C", 0.7), hit("D", 0.65)];

    for tolerance in [
        DuplicateTolerance::Strict,
        DuplicateTolerance::Balanced,
        DuplicateTolerance::Lenient,
    ] {
        for cap_active in [false, true] {
            let config = cfg(RecallScope::Full, cap_active, tolerance);
            let out = run(fused.clone(), &items, 10, config);
            let mut seen: Vec<String> = Vec::new();
            for id in ids(&out) {
                let text = items
                    .get(&id)
                    .map(|item| normalize_exact(&doc_text(Some(item))))
                    .unwrap_or_default();
                assert!(
                    !text.is_empty(),
                    "{tolerance:?} cap_active={cap_active}: empty text for {id}"
                );
                assert!(
                    !seen.contains(&text),
                    "{tolerance:?} cap_active={cap_active}: page repeats normalized \
                     text {text:?} across ids {seen:?} + {id}"
                );
                seen.push(text);
            }
            // The two genuinely different facts both survive; the three
            // equivalent ones collapse to the highest-ranked representative.
            assert_eq!(
                ids(&out),
                vec!["A".to_owned(), "C".to_owned()],
                "{tolerance:?} cap_active={cap_active}"
            );
        }
    }
}

#[test]
fn a_duplicate_below_the_page_limit_still_collapses() {
    // The page is smaller than the duplicate group, which is the case a
    // page-local cap can silently miss.
    let items = map(vec![
        raw("A", "User prefers dark mode"),
        raw("B", "User prefers dark mode"),
        raw("C", "User prefers dark mode"),
    ]);
    let fused = vec![hit("A", 0.9), hit("B", 0.8), hit("C", 0.7)];

    let out = run(fused, &items, 2, neutral());

    assert_eq!(ids(&out), vec!["A".to_owned()]);
}
