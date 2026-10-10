// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for tests of the persona token budget.
//!
//! ## Owns
//! - deterministic drop order (score desc, key asc) fixture
//! - `persona_pack_entries` ordering contract
//!
//! ## Boundary
//!
//! Truncation policy changes should normally update tests here.

use crate::budget::{
    PersonaPackEntry, persona_pack_entries, render_preference, render_stable, truncate_document,
};
use clio_types::{PersonaDocument, PersonaPreference, PersonaStableEntry, estimate_tokens};

fn stable(key: &str, score: f64) -> PersonaStableEntry {
    PersonaStableEntry {
        context: None,
        bank_id: "b".into(),
        key: key.into(),
        value: "v".repeat(12),
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        admission_score: score,
        as_of: "2026-09-10".into(),
        valid_until: None,
    }
}

fn pref(key: &str, score: f64, value: f64) -> PersonaPreference {
    PersonaPreference {
        context: None,
        bank_id: "b".into(),
        key: key.into(),
        value,
        trend: value,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        admission_score: score,
        as_of: "2026-09-10".into(),
    }
}

/// Known scores ⇒ deterministic drop order = score desc, key asc;
/// token count stays within budget.
#[test]
fn over_budget_truncates_by_score_then_key() {
    // Four stables with known scores; each entry costs tokens(key)+tokens(text).
    let doc = PersonaDocument {
        stable: vec![
            stable("alpha", 0.9),
            stable("beta", 0.9),
            stable("gamma", 0.5),
            stable("delta", 0.1),
        ],
        preferences: vec![],
    };
    // Budget admits exactly two "v…v" entries with their keys.
    let entry_tokens = |e: &PersonaStableEntry| estimate_tokens(&e.key) + estimate_tokens(&e.value);
    let budget = 2 * entry_tokens(&doc.stable[0]);
    let t = truncate_document(&doc, budget);
    // alpha and beta tie at 0.9 → key asc keeps alpha first; gamma and delta drop.
    let kept: Vec<String> = t.document.stable.iter().map(|e| e.key.clone()).collect();
    assert_eq!(kept, vec!["alpha", "beta"]);
    assert_eq!(t.dropped_keys, vec!["gamma", "delta"]);
    let used: usize = t
        .document
        .stable
        .iter()
        .map(|e| estimate_tokens(&e.key) + estimate_tokens(&e.value))
        .sum();
    assert!(used <= budget, "used {used} > budget {budget}");
}

/// Tie-break proof: equal scores keep keys in ascending order and drop the
/// later keys first.
#[test]
fn equal_scores_tie_break_by_key_ascending() {
    let doc = PersonaDocument {
        stable: vec![stable("b", 0.5), stable("a", 0.5), stable("c", 0.5)],
        preferences: vec![],
    };
    let t = truncate_document(
        &doc,
        estimate_tokens("a") + estimate_tokens(&"v".repeat(10)),
    );
    let kept: Vec<String> = t.document.stable.iter().map(|e| e.key.clone()).collect();
    assert_eq!(kept, vec!["a"]);
    assert_eq!(t.dropped_keys, vec!["b", "c"]);
}

/// Preference entries participate in the same ranking as stables.
#[test]
fn preferences_rank_against_stables() {
    let doc = PersonaDocument {
        stable: vec![stable("low", 0.2)],
        preferences: vec![pref("high", 0.9, 0.8)],
    };
    let t = truncate_document(
        &doc,
        estimate_tokens("high") + estimate_tokens(&render_preference(&pref("high", 0.2, 0.8))),
    );
    assert!(t.document.stable.is_empty(), "{:?}", t.document.stable);
    assert_eq!(t.dropped_keys, vec!["low"]);
    assert_eq!(t.document.preferences[0].key, "high");
}

/// Empty document and zero budget never produce an unbounded document.
#[test]
fn empty_and_zero_budget() {
    let t = truncate_document(&PersonaDocument::default(), 400);
    assert!(t.document.stable.is_empty() && t.document.preferences.is_empty());
    let doc = PersonaDocument {
        stable: vec![stable("a", 0.9)],
        preferences: vec![],
    };
    let t = truncate_document(&doc, 0);
    assert_eq!(t.dropped_keys, vec!["a"]);
}

/// Edge cases: an oversized highest-ranked entry is dropped (no overflow, no
/// unbounded keep); an exact-budget fit is kept.
#[test]
fn oversized_entry_dropped_and_exact_budget_kept() {
    let big = PersonaStableEntry {
        value: "v".repeat(10_000),
        ..stable("huge", 1.0)
    };
    let small = stable("tiny", 0.5);
    let doc = PersonaDocument {
        stable: vec![big, small],
        preferences: vec![],
    };
    let t = truncate_document(&doc, 100);
    // The 10k-char entry alone exceeds the budget; the small one still fits.
    let kept: Vec<&str> = t.document.stable.iter().map(|e| e.key.as_str()).collect();
    assert_eq!(kept, vec!["tiny"]);
    assert_eq!(t.dropped_keys, vec!["huge"]);
    let exact = truncate_document(
        &PersonaDocument {
            stable: vec![stable("a", 0.9)],
            preferences: vec![],
        },
        estimate_tokens("a") + estimate_tokens(&"v".repeat(12)),
    );
    let kept: Vec<&str> = exact
        .document
        .stable
        .iter()
        .map(|e| e.key.as_str())
        .collect();
    assert_eq!(kept, vec!["a"]);
    assert!(exact.dropped_keys.is_empty(), "{:?}", exact.dropped_keys);
}

/// `persona_pack_entries` returns rank order (score desc, key asc) for compose.
#[test]
fn pack_entries_are_ranked() {
    let doc = PersonaDocument {
        stable: vec![stable("b", 0.9), stable("a", 0.9)],
        preferences: vec![pref("z", 0.1, 0.5)],
    };
    let entries = persona_pack_entries(&doc);
    let keys: Vec<String> = entries.iter().map(|e| e.key.clone()).collect();
    assert_eq!(keys, vec!["a", "b", "z"]);
    assert!((entries[0].score - 0.9).abs() < 1e-12);
    assert!((entries[2].score - 0.1).abs() < 1e-12);
    assert_eq!(entries[2].text, "0.5000 (trend 0.5000)");
}

/// Rendered texts are deterministic strings.
#[test]
fn renders_are_deterministic() {
    assert_eq!(render_stable(&stable("k", 0.9)), "v".repeat(12));
    assert_eq!(
        render_preference(&pref("k", 0.9, 0.815)),
        "0.8150 (trend 0.8150)"
    );
    let _ = PersonaPackEntry {
        key: "k".into(),
        text: "t".into(),
        score: 1.0,
    };
}
