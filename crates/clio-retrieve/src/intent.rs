// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-turn intent gate: deciding whether a
//! turn plausibly needs old memory and, if so, which domains to search.
//!
//! ## Owns
//! - `intent_gate` heuristic classifier and `GateDecision`
//! - Lightweight PII-safe `GateMetrics` counters for false-negative monitoring
//! - Labeled `evaluate_gate` harness with separate FN/FP rates and the
//!   stricter false-negative bound
//!
//! ## Does not own
//! - Dense/lexical search or graph traversal (cost must stay outside the gate)
//! - Persona injection (never gated; owned by `compose`/clio-persona)
//! - Ranking weights beyond the optional `skip_confidence` threshold
//!
//! ## Boundary
//!
//! Changes to gate heuristics or thresholds should normally require changes
//! here. Changes to retrieval ranking should not require changes here.
//!
//! # Gate contract
//!
//! The gate is pure CPU work: lowercase/trim plus substring checks, no
//! embedding call and no graph access, so it stays inside (p95 ≤ 50 ms).
//! It is deliberately biased toward retrieving: only short phatic turns with no
//! memory cue are skipped, because a false negative (skip on a memory-needed
//! turn) produces a wrong answer while a false positive only costs latency.

use clio_config::ranking::IntentGateThresholds;
use clio_types::SearchDomain;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Default confidence required to skip when `intent_gate.skip_confidence` is unset.
pub const DEFAULT_SKIP_CONFIDENCE: f64 = 0.5;

/// Turns that carry no retrievable intent on their own.
const PHATIC_PHRASES: &[&str] = &[
    "hi",
    "hello",
    "hey",
    "yo",
    "thanks",
    "thank you",
    "thx",
    "ty",
    "ok",
    "okay",
    "k",
    "cool",
    "nice",
    "great",
    "got it",
    "sounds good",
    "sure",
    "yes",
    "no",
    "yep",
    "nope",
    "bye",
    "goodbye",
    "good morning",
    "good night",
    "lol",
    "haha",
];

/// Short-token whitelist for acknowledgement-style turns.
const ACK_TOKENS: &[&str] = &[
    "ok", "okay", "k", "cool", "nice", "great", "thanks", "thank", "thx", "ty", "yes", "no", "yep",
    "nope", "sure", "got", "it", "sounds", "good", "hello", "hi", "hey", "yo", "bye", "lol",
    "haha", "perfect", "awesome",
];

/// `{retrieval_needed, domains[]}` for one turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateDecision {
    /// Whether background retrieval should run for this turn.
    pub retrieval_needed: bool,
    /// Domain hints (empty only when retrieval is skipped).
    pub domains: Vec<String>,
    /// Heuristic confidence in the decision (0.0–1.0).
    pub confidence: f64,
    /// Short machine-readable reason (inspectability).
    pub reason: String,
}

/// Process-local gate counters (counts only; never turn text — PII-safe).
#[derive(Debug, Default)]
pub struct GateMetrics {
    evaluated: AtomicU64,
    skipped: AtomicU64,
}

/// Point-in-time copy of [`GateMetrics`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GateMetricsSnapshot {
    /// Turns evaluated.
    pub evaluated: u64,
    /// Turns where the gate skipped retrieval.
    pub skipped: u64,
}

impl GateMetrics {
    /// Record one decision.
    pub fn record(&self, decision: &GateDecision) {
        self.evaluated.fetch_add(1, Ordering::Relaxed);
        if !decision.retrieval_needed {
            self.skipped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Current counters.
    #[must_use]
    pub fn snapshot(&self) -> GateMetricsSnapshot {
        GateMetricsSnapshot {
            evaluated: self.evaluated.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
        }
    }
}

/// Stricter bound for the gate's false-negative rate (requirement): a false
/// negative produces a wrong answer while a false positive only costs latency,
/// so the FN rate is tracked separately and held well below the FP rate.
pub const DEFAULT_FALSE_NEGATIVE_BOUND: f64 = 0.05;

/// Ground-truth evaluation counters over labeled turns (requirement).
///
/// Production traffic carries no labels, so `GateMetrics` stays count-only;
/// this struct exists for offline/test evaluation where expected labels exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct GateEvaluation {
    /// Labeled memory-needed turns the gate skipped (wrong-answer risk).
    pub false_negatives: u64,
    /// Labeled self-contained turns the gate sent to retrieval (latency cost).
    pub false_positives: u64,
    /// Labeled turns that needed memory.
    pub labeled_needed: u64,
    /// Labeled turns that did not need memory.
    pub labeled_unneeded: u64,
}

impl GateEvaluation {
    /// False-negative rate over labeled memory-needed turns (0.0 when none).
    #[must_use]
    pub fn false_negative_rate(&self) -> f64 {
        if self.labeled_needed == 0 {
            0.0
        } else {
            f64::from(u32::try_from(self.false_negatives).unwrap_or(u32::MAX))
                / f64::from(u32::try_from(self.labeled_needed).unwrap_or(u32::MAX))
        }
    }

    /// False-positive rate over labeled self-contained turns (0.0 when none).
    #[must_use]
    pub fn false_positive_rate(&self) -> f64 {
        if self.labeled_unneeded == 0 {
            0.0
        } else {
            f64::from(u32::try_from(self.false_positives).unwrap_or(u32::MAX))
                / f64::from(u32::try_from(self.labeled_unneeded).unwrap_or(u32::MAX))
        }
    }

    /// Whether the evaluation satisfies the stricter false-negative bound.
    #[must_use]
    pub fn meets_false_negative_bound(&self) -> bool {
        self.false_negative_rate() <= DEFAULT_FALSE_NEGATIVE_BOUND
    }
}

/// Evaluate the gate against labeled turns: `(turn_text, expected_needed)`
/// pairs. Reports false negatives and false positives separately so the
/// stricter false-negative bound can be enforced (requirement).
#[must_use]
pub fn evaluate_gate(
    labeled: &[(&str, bool)],
    thresholds: &IntentGateThresholds,
) -> GateEvaluation {
    let mut eval = GateEvaluation::default();
    for (text, expected_needed) in labeled {
        let decision = intent_gate(text, thresholds);
        if *expected_needed {
            eval.labeled_needed += 1;
            if !decision.retrieval_needed {
                eval.false_negatives += 1;
            }
        } else {
            eval.labeled_unneeded += 1;
            if decision.retrieval_needed {
                eval.false_positives += 1;
            }
        }
    }
    eval
}

/// Classify one turn. Pure and cheap; never performs embedding or graph work.
#[must_use]
pub fn intent_gate(turn_text: &str, thresholds: &IntentGateThresholds) -> GateDecision {
    let norm = normalize(turn_text);
    if norm.is_empty() {
        return GateDecision {
            retrieval_needed: false,
            domains: Vec::new(),
            confidence: 1.0,
            reason: "empty-turn".to_owned(),
        };
    }
    let domains = cue_domains(&norm);
    if !domains.is_empty() {
        return GateDecision {
            retrieval_needed: true,
            domains: wire(&domains),
            confidence: 0.8,
            reason: "memory-cue".to_owned(),
        };
    }
    if is_phatic(&norm) && !looks_like_recall_question(&norm) {
        let confidence = if PHATIC_PHRASES.contains(&norm.as_str()) {
            0.95
        } else {
            0.7
        };
        let threshold = thresholds
            .skip_confidence
            .unwrap_or(DEFAULT_SKIP_CONFIDENCE);
        if confidence >= threshold {
            return GateDecision {
                retrieval_needed: false,
                domains: Vec::new(),
                confidence,
                reason: "phatic".to_owned(),
            };
        }
    }
    // Everything else retrieves: prefer a false positive over a wrong answer.
    GateDecision {
        retrieval_needed: true,
        domains: wire(&[SearchDomain::Semantic, SearchDomain::Episodic]),
        confidence: 0.5,
        reason: "default-retrieve".to_owned(),
    }
}

/// Lowercase, strip punctuation to spaces, collapse whitespace.
fn normalize(text: &str) -> String {
    let lower = text.to_lowercase();
    let cleaned: String = lower
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_phatic(norm: &str) -> bool {
    if PHATIC_PHRASES.contains(&norm) {
        return true;
    }
    let tokens: Vec<&str> = norm.split_whitespace().collect();
    !tokens.is_empty() && tokens.len() <= 3 && tokens.iter().all(|t| ACK_TOKENS.contains(t))
}

fn contains_any(norm: &str, cues: &[&str]) -> bool {
    cues.iter().any(|c| norm.contains(c))
}

fn looks_like_recall_question(norm: &str) -> bool {
    let question = norm.split_whitespace().next().is_some_and(|w| {
        matches!(
            w,
            "what" | "where" | "when" | "who" | "which" | "why" | "how" | "did" | "does" | "do"
        )
    });
    let first_person = ["i ", "my ", "we ", "our ", "me "]
        .iter()
        .any(|p| norm.starts_with(p) || norm.contains(&format!(" {p}")));
    question && (first_person || norm.contains(" you "))
}

fn cue_domains(norm: &str) -> Vec<SearchDomain> {
    let mut out: Vec<SearchDomain> = Vec::new();
    let mut push = |d: SearchDomain, cond: bool| {
        if cond && !out.contains(&d) {
            out.push(d);
        }
    };
    push(
        SearchDomain::Persona,
        contains_any(
            norm,
            &[
                "my name",
                "i prefer",
                "i like",
                "favorite",
                "favourite",
                "persona",
                "who am i",
                "call me",
                "my style",
                "communication style",
            ],
        ),
    );
    push(
        SearchDomain::Failure,
        contains_any(
            norm,
            &[
                "error",
                "failed",
                "failure",
                "bug",
                "crash",
                "exception",
                "traceback",
                "panic",
                "regression",
            ],
        ),
    );
    push(
        SearchDomain::Task,
        contains_any(
            norm,
            &[
                "task", "ticket", "issue", "todo", "continue", "resume", "status", "steps", "plan",
            ],
        ),
    );
    push(
        SearchDomain::Temporal,
        contains_any(
            norm,
            &[
                "remember",
                "recall",
                "earlier",
                "yesterday",
                "previously",
                "last time",
                "ago",
                "before",
                "when did",
                "history",
            ],
        ),
    );
    push(
        SearchDomain::Episodic,
        contains_any(
            norm,
            &[
                "discussed",
                "told you",
                "you said",
                "mentioned",
                "we use",
                "convention",
            ],
        ),
    );
    push(
        SearchDomain::Semantic,
        contains_any(
            norm,
            &["what is", "how do", "schema", "config", "documentation"],
        ),
    );
    out
}

fn wire(domains: &[SearchDomain]) -> Vec<String> {
    domains.iter().map(|d| d.as_str().to_owned()).collect()
}

#[cfg(test)]
#[path = "intent_tests.rs"]
mod intent_tests;
