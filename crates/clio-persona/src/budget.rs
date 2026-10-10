// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the persona token budget and its
//! deterministic score-ranked truncation.
//!
//! ## Owns
//! - `truncate_document`: rank by stored `admission_score` desc, tie-break `key`
//!   asc, drop lowest until within budget (never silently unbounded)
//! - `PersonaPackEntry`: the pre-rendered entry shape compose consumes
//!
//! ## Does not own
//! - The token estimator (`clio-types::estimate_tokens`, shared with compose)
//! - Persona persistence or admission scoring
//!
//! ## Boundary
//!
//! Changes to ranking/truncation policy should normally require changes here.
//! Changes to the estimator formula should not (`clio-types::tokens`).
//!
//! Keep this module focused on persona budget accounting and truncation.

use clio_types::{PersonaDocument, PersonaPreference, PersonaStableEntry, estimate_tokens};
use serde::{Deserialize, Serialize};

/// Budget result: the kept document plus the keys dropped for budget.
#[derive(Debug, Clone, PartialEq)]
pub struct Truncation {
    /// Entries that fit the budget (kept in rank order).
    pub document: PersonaDocument,
    /// Keys dropped, in drop order (lowest rank first).
    pub dropped_keys: Vec<String>,
}

/// One pre-rendered persona entry for the compose channel (`clio-retrieve` maps
/// this to its `PersonaEntry`; the shape is kept local so this crate does not
/// depend on the retrieval stack).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonaPackEntry {
    /// Stable entry key (ranked tie-break).
    pub key: String,
    /// Rendered entry text.
    pub text: String,
    /// Admission score used for ranked truncation.
    pub score: f64,
}

/// Render one stable entry's value for injection.
#[must_use]
pub fn render_stable(entry: &PersonaStableEntry) -> String {
    entry.value.clone()
}

/// Render one preference entry's value for injection (state + trend).
#[must_use]
pub fn render_preference(pref: &PersonaPreference) -> String {
    format!("{:.4} (trend {:.4})", pref.value, pref.trend)
}

/// Token cost of one persona entry, identical to compose's accounting:
/// `estimate_tokens(key) + estimate_tokens(text)`.
fn entry_tokens(key: &str, text: &str) -> usize {
    estimate_tokens(key) + estimate_tokens(text)
}

/// Rank rule: stored `admission_score` descending, tie-break `key`
/// ascending. Deterministic given identical scores and keys.
fn rank(a_score: f64, a_key: &str, b_score: f64, b_key: &str) -> std::cmp::Ordering {
    b_score
        .partial_cmp(&a_score)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a_key.cmp(b_key))
}

/// Truncate a document to a token budget by score-ranked drop order.
///
/// Keeps entries while they fit; drops the lowest-ranked remainder. The
/// returned document lists kept entries in rank order so `persona_get` and the
/// compose channel see the same truncated view (single source of truth).
#[must_use]
pub fn truncate_document(doc: &PersonaDocument, budget_tokens: usize) -> Truncation {
    enum Slot<'a> {
        Stable(&'a PersonaStableEntry),
        Preference(&'a PersonaPreference),
    }
    let mut slots: Vec<Slot<'_>> = doc
        .stable
        .iter()
        .map(Slot::Stable)
        .chain(doc.preferences.iter().map(Slot::Preference))
        .collect();
    slots.sort_by(|a, b| match (a, b) {
        (Slot::Stable(x), Slot::Stable(y)) => {
            rank(x.admission_score, &x.key, y.admission_score, &y.key)
        }
        (Slot::Preference(x), Slot::Preference(y)) => {
            rank(x.admission_score, &x.key, y.admission_score, &y.key)
        }
        (Slot::Stable(x), Slot::Preference(y)) => {
            rank(x.admission_score, &x.key, y.admission_score, &y.key)
        }
        (Slot::Preference(x), Slot::Stable(y)) => {
            rank(x.admission_score, &x.key, y.admission_score, &y.key)
        }
    });
    let mut kept_stable = Vec::new();
    let mut kept_pref = Vec::new();
    let mut dropped_keys = Vec::new();
    let mut used = 0_usize;
    for slot in slots {
        let (key, tokens) = match &slot {
            Slot::Stable(e) => (e.key.clone(), entry_tokens(&e.key, &render_stable(e))),
            Slot::Preference(e) => (e.key.clone(), entry_tokens(&e.key, &render_preference(e))),
        };
        if used + tokens > budget_tokens {
            dropped_keys.push(key);
            continue;
        }
        used += tokens;
        match slot {
            Slot::Stable(e) => kept_stable.push(e.clone()),
            Slot::Preference(e) => kept_pref.push(e.clone()),
        }
    }
    Truncation {
        document: PersonaDocument {
            stable: kept_stable,
            preferences: kept_pref,
        },
        dropped_keys,
    }
}

/// Build the compose-channel entries from an (already truncated) document.
#[must_use]
pub fn persona_pack_entries(doc: &PersonaDocument) -> Vec<PersonaPackEntry> {
    let mut entries: Vec<PersonaPackEntry> = doc
        .stable
        .iter()
        .map(|e| PersonaPackEntry {
            key: e.key.clone(),
            text: render_stable(e),
            score: e.admission_score,
        })
        .chain(doc.preferences.iter().map(|p| PersonaPackEntry {
            key: p.key.clone(),
            text: render_preference(p),
            score: p.admission_score,
        }))
        .collect();
    entries.sort_by(|a, b| rank(a.score, &a.key, b.score, &b.key));
    entries
}

#[cfg(test)]
#[path = "budget_tests.rs"]
mod budget_tests;
