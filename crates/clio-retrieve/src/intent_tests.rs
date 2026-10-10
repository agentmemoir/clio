// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for intent-gate behavior tests: skip/retrieve
//! decisions, domain cues, thresholds, latency budget, metrics counters, and
//! the labeled FN/FP evaluation harness.
//!
//! ## Owns
//! - Gate assertions, including a latency smoke check; the structural
//!   guarantee is the gate's pure no-I/O signature
//! - `evaluate_gate` labeled-fixture assertions (FN=0, FP reported separately)
//!
//! ## Does not own
//! - Gate heuristics themselves (`intent`) or retrieval behavior (`hybrid_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::intent::{
    DEFAULT_FALSE_NEGATIVE_BOUND, DEFAULT_SKIP_CONFIDENCE, GateDecision, GateMetrics,
    evaluate_gate, intent_gate,
};
use clio_config::ranking::IntentGateThresholds;

fn defaults() -> IntentGateThresholds {
    IntentGateThresholds::default()
}

fn threshold(value: f64) -> IntentGateThresholds {
    IntentGateThresholds {
        skip_confidence: Some(value),
    }
}

#[test]
fn t01_gate_on_greeting_skips() {
    let decision = intent_gate("hi", &defaults());
    assert!(!decision.retrieval_needed);
    assert!(decision.domains.is_empty(), "{:?}", decision.domains);
    assert_eq!(decision.reason, "phatic");
    assert!(decision.confidence >= DEFAULT_SKIP_CONFIDENCE);
}

#[test]
fn t01_gate_on_acknowledgement_skips() {
    assert!(!intent_gate("ok thanks", &defaults()).retrieval_needed);
    assert!(!intent_gate("sounds good", &defaults()).retrieval_needed);
    assert!(!intent_gate("Nice!", &defaults()).retrieval_needed);
    assert!(!intent_gate("   ", &defaults()).retrieval_needed);
    let empty = intent_gate("", &defaults());
    assert_eq!(empty.reason, "empty-turn");
    assert_eq!(empty.confidence, 1.0);
}

#[test]
fn t02_gate_on_memory_needed_turn_retrieves_with_domains() {
    let decision = intent_gate("what city did I say I live in?", &defaults());
    assert!(decision.retrieval_needed);
    assert!(!decision.domains.is_empty(), "{:?}", decision.domains);
}

#[test]
fn t02_memory_cues_select_domains() {
    let decision = intent_gate("remember the error we discussed last time?", &defaults());
    assert!(decision.retrieval_needed);
    assert!(decision.domains.contains(&"failure".to_owned()));
    assert!(decision.domains.contains(&"episodic".to_owned()));
    assert!(decision.domains.contains(&"temporal".to_owned()));
    assert_eq!(decision.reason, "memory-cue");

    let persona = intent_gate("what is my name again", &defaults());
    assert!(persona.domains.contains(&"persona".to_owned()));

    let task = intent_gate("continue with the ticket", &defaults());
    assert!(task.domains.contains(&"task".to_owned()));

    let semantic = intent_gate("how do I configure the schema?", &defaults());
    assert!(semantic.domains.contains(&"semantic".to_owned()));
}

#[test]
fn t02_recall_question_defaults_to_semantic_and_episodic() {
    let decision = intent_gate("where did we leave off", &defaults());
    assert!(decision.retrieval_needed);
    assert!(decision.domains.contains(&"semantic".to_owned()));
    assert!(decision.domains.contains(&"episodic".to_owned()));
    assert_eq!(decision.reason, "default-retrieve");
}

#[test]
fn skip_confidence_threshold_can_force_retrieval() {
    // A greeting is only skipped when the configured threshold is satisfied.
    let forced = intent_gate("hi", &threshold(0.99));
    assert!(forced.retrieval_needed);
    let allowed = intent_gate("hi", &threshold(0.5));
    assert!(!allowed.retrieval_needed);
}

#[test]
fn longer_acknowledgement_with_task_cue_still_retrieves() {
    let decision = intent_gate("ok let's continue with the task", &defaults());
    assert!(decision.retrieval_needed);
    assert!(decision.domains.contains(&"task".to_owned()));
}

#[test]
fn t03_gate_latency_budget() {
    let mut samples: Vec<u128> = Vec::with_capacity(2000);
    for i in 0..2000 {
        let text = if i % 3 == 0 {
            "hi".to_owned()
        } else {
            format!("what did we discuss about error number {i} last time?")
        };
        let start = std::time::Instant::now();
        let _ = intent_gate(&text, &defaults());
        samples.push(start.elapsed().as_micros());
    }
    samples.sort_unstable();
    let p95 = samples[samples.len() * 95 / 100];
    assert!(p95 <= 50_000, "gate p95 {p95} us exceeds 50 ms");
}

#[test]
fn gate_metrics_track_skips() {
    let metrics = GateMetrics::default();
    metrics.record(&intent_gate("hi", &defaults()));
    metrics.record(&intent_gate("what is my name", &defaults()));
    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.evaluated, 2);
    assert_eq!(snapshot.skipped, 1);
}

#[test]
fn gate_evaluation_tracks_fn_and_fp_separately() {
    // Labeled fixture: four memory-needed turns, four self-contained turns,
    // plus one self-contained turn the heuristic cannot safely skip so the
    // false-positive rate is nonzero while the false-negative rate stays zero.
    let labeled: &[(&str, bool)] = &[
        ("hi", false),
        ("ok thanks", false),
        ("sounds good", false),
        ("totally", false),
        ("what city did I say I live in?", true),
        ("remember the error we discussed last time?", true),
        ("continue with the ticket", true),
        ("how do I configure the schema?", true),
    ];
    let eval = evaluate_gate(labeled, &defaults());
    assert_eq!(eval.labeled_needed, 4);
    assert_eq!(eval.labeled_unneeded, 4);
    assert_eq!(
        eval.false_negatives, 0,
        "no memory-needed turn may be skipped"
    );
    assert_eq!(
        eval.false_positives, 1,
        "the unsafely-skippable turn retrieves"
    );
    assert!(eval.false_negative_rate() <= DEFAULT_FALSE_NEGATIVE_BOUND);
    assert!(eval.meets_false_negative_bound());
}

#[test]
fn gate_decision_serde_round_trip() {
    let decision = GateDecision {
        retrieval_needed: true,
        domains: vec!["semantic".into()],
        confidence: 0.5,
        reason: "default-retrieve".into(),
    };
    let json = serde_json::to_value(&decision).expect("serialize");
    let back: GateDecision = serde_json::from_value(json).expect("deserialize");
    assert_eq!(decision, back);
}
