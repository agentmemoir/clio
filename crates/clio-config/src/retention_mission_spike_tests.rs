// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the retention mission semantic matching spike,
//! measuring whether semantic matching on keep/drop examples improves hit rate
//! over literal substring matching.
//!
//! ## Owns
//! - Prototype token-similarity and concept-matching spike implementation
//! - Empirical measurement of hit rates across literal and paraphrased datasets
//! - Representative exact and paraphrased keep/drop test suites
//! - Architectural learning decision documentation on retention semantic matching
//!
//! ## Does not own
//! - Production retention mission evaluation (see `retention_mission`)
//! - Bounded regex matching execution (see `mission_match`)
//! - Profile admission pipeline scoring (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to the spike test cases or semantic comparison methodology should
//! normally require changes here. Changes to production retention matching or
//! profile schema should not require changes here.
//!
//! Keep this module focused on evaluating semantic matching for retention missions.
//!
//! ## Recorded Spike Outcome
//!
//! Two corpora are measured; they answer different questions and must not be
//! conflated:
//!
//! - Calibration suite (exact + paraphrased): hit rates are exact literal =
//!   100.0%, exact semantic = 100.0%, paraphrased literal = 0.0%, paraphrased
//!   semantic = 100.0% (>= 87.5%+). This corpus is synthetic and
//!   self-confirming: each paraphrase's distinguishing words are members of
//!   this module's own `SYNONYM_CLUSTERS`, so its 100% semantic hit rate
//!   demonstrates calibration to the prototype's table, not generalization.
//! - Held-out suite (unseen vocabulary, no `SYNONYM_CLUSTERS` membership and
//!   no rule-text reuse): semantic hit rate = 0.0%. Unseen vocabulary is not
//!   recovered, which is the honest generalization measurement.
//!
//! Literal substring matching fails completely (0.0% hit rate) when developers and
//! agents express intentions using synonyms, reordered phrasing, or alternative
//! vocabulary. The prototype's semantic matching recovers only the calibrated
//! phrasings it was tuned for; it does not generalize to unseen phrasing.
//!
//! ## Semantic Admission Decision
//!
//! Keep literal/regex matching on the synchronous write/admission path to preserve
//! zero-sidecar speed, low latency, and determinism without false-positive
//! drop penalties. Semantic matching introduces tokenization/embedding overhead
//! and potential false-positive drops that could inadvertently purge valuable context.
//!
//! ## Learning Decision
//!
//! Explicitly defer automatic online rule learning (which risks unbounded rule drift,
//! policy instability, and adversarial prompt poisoning). Retention missions remain
//! explicit operator-configured policy objects.

use crate::coding_default::coding_default_profile;
use crate::retention_mission::{
    MISSION_DROP_PENALTY, MISSION_KEEP_BONUS, ResolvedMission, RetentionMission,
};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvaluationMatch {
    Keep,
    Drop,
    None,
}

const SIMILARITY_THRESHOLD: f64 = 0.60;

const SYNONYM_CLUSTERS: &[&[&str]] = &[
    &["prefer", "prefers", "preferred", "favor", "favors", "likes"],
    &["decision", "decided", "chose", "chosen", "resolution"],
    &[
        "constraint",
        "constraints",
        "boundary",
        "restriction",
        "never",
    ],
    &["root", "underlying", "fundamental"],
    &["cause", "defect", "flaw"],
    &["verified", "validated", "confirmed"],
    &["config", "configuration", "parameters", "settings", "setup"],
    &["workflow", "procedure", "process", "automation"],
    &["rule", "rules", "policy", "guideline"],
    &[
        "alert",
        "warning",
        "notification",
        "alarm",
        "critical",
        "urgent",
    ],
    &["tests", "test", "suite"],
    &["passed", "pass", "succeeded", "success"],
    &["pr", "pull"],
    &["created", "opened", "filed", "submitted"],
    &["issue", "bug", "ticket", "report"],
    &["heartbeat", "keepalive", "keep-alive", "ping"],
    &["command", "terminal", "cli", "session"],
    &["dump", "dumped", "transcript"],
    &["thanks", "thank", "appreciated"],
    &["got", "understood", "acknowledged"],
    &["progress", "working", "ongoing", "underway"],
];

fn is_stopword(token: &str) -> bool {
    matches!(
        token,
        "a" | "an" | "the" | "is" | "it" | "in" | "on" | "for" | "of" | "to" | "my"
    )
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '-')
        .filter_map(|w| {
            let trimmed = w.trim_matches('-');
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_lowercase())
            }
        })
        .collect()
}

fn char_ngram_similarity(s1: &str, s2: &str) -> f64 {
    if s1 == s2 {
        return 1.0;
    }
    if s1.len() < 3 || s2.len() < 3 {
        return 0.0;
    }
    let ngrams1: HashSet<&[u8]> = s1.as_bytes().windows(3).collect();
    let ngrams2: HashSet<&[u8]> = s2.as_bytes().windows(3).collect();
    let intersection = ngrams1.intersection(&ngrams2).count();
    let union = ngrams1.union(&ngrams2).count();
    if union == 0 {
        0.0
    } else {
        let numer = f64::from(u32::try_from(intersection).unwrap_or(0));
        let denom = f64::from(u32::try_from(union).unwrap_or(1));
        numer / denom
    }
}

fn token_similarity(t1: &str, t2: &str) -> f64 {
    if t1 == t2 {
        return 1.0;
    }
    let min_len = t1.len().min(t2.len());
    if min_len >= 4 && (t1.starts_with(t2) || t2.starts_with(t1)) {
        return 0.95;
    }
    for cluster in SYNONYM_CLUSTERS {
        if cluster.contains(&t1) && cluster.contains(&t2) {
            return 0.90;
        }
    }
    let ngram_sim = char_ngram_similarity(t1, t2);
    if ngram_sim >= 0.5 {
        ngram_sim * 0.8
    } else {
        0.0
    }
}

fn semantic_similarity(rule_text: &str, candidate_text: &str) -> f64 {
    let raw_tokens = tokenize(rule_text);
    let mut rule_tokens: Vec<String> = raw_tokens
        .iter()
        .filter(|t| !is_stopword(t))
        .cloned()
        .collect();
    if rule_tokens.is_empty() {
        rule_tokens = raw_tokens;
    }
    let candidate_tokens = tokenize(candidate_text);
    if rule_tokens.is_empty() || candidate_tokens.is_empty() {
        return 0.0;
    }
    let total_score: f64 = rule_tokens
        .iter()
        .map(|r_tok| {
            candidate_tokens
                .iter()
                .map(|c_tok| token_similarity(r_tok, c_tok))
                .fold(0.0, f64::max)
        })
        .sum();
    let denom = f64::from(u32::try_from(rule_tokens.len()).unwrap_or(1));
    total_score / denom
}

fn evaluate_literal(mission: &RetentionMission, text: &str) -> EvaluationMatch {
    let resolved = ResolvedMission::from_mission(Some(mission));
    let eval = resolved.evaluate(text);
    if eval.utility_delta == MISSION_DROP_PENALTY {
        EvaluationMatch::Drop
    } else if eval.utility_delta == MISSION_KEEP_BONUS {
        EvaluationMatch::Keep
    } else {
        EvaluationMatch::None
    }
}

fn evaluate_semantic(mission: &RetentionMission, text: &str) -> EvaluationMatch {
    let drop_hit = mission
        .drop_examples
        .iter()
        .any(|entry| semantic_similarity(&entry.text, text) >= SIMILARITY_THRESHOLD);
    if drop_hit {
        return EvaluationMatch::Drop;
    }
    let keep_hit = mission
        .keep_examples
        .iter()
        .any(|entry| semantic_similarity(&entry.text, text) >= SIMILARITY_THRESHOLD);
    if keep_hit {
        EvaluationMatch::Keep
    } else {
        EvaluationMatch::None
    }
}

fn compute_set_hit_rate<F>(keep: &[&str], drop: &[&str], evaluator: F) -> f64
where
    F: Fn(&str) -> EvaluationMatch,
{
    let total = keep.len() + drop.len();
    if total == 0 {
        return 0.0;
    }
    let keep_hits = keep
        .iter()
        .filter(|t| evaluator(t) == EvaluationMatch::Keep)
        .count();
    let drop_hits = drop
        .iter()
        .filter(|t| evaluator(t) == EvaluationMatch::Drop)
        .count();
    let numer = f64::from(u32::try_from(keep_hits + drop_hits).unwrap_or(0));
    let denom = f64::from(u32::try_from(total).unwrap_or(1));
    numer / denom
}

const EXACT_KEEP: &[&str] = &[
    "User prefers tabs over spaces in this project.",
    "Decision: the store module must use strict tolerance.",
    "Constraint: the deploy script must never touch prod.",
    "Root cause: retry loop must serialize session file.",
    "Verified: database migration succeeds on restart.",
    "Config: max connections set to 64 for pool.",
    "Workflow rule: always run clippy before commit.",
    "Alert: disk usage exceeded 90 percent on volume.",
];

const EXACT_DROP: &[&str] = &[
    "Tests passed for all unit suites.",
    "PR created: add support for retention policies.",
    "Issue created: investigate pool timeout.",
    "Heartbeat check ok at 10:00 UTC.",
    "Command dump written to debug log.",
    "Thanks for the quick review and comments.",
    "Got it, will proceed with the rollout.",
    "In progress: refactoring configuration loader.",
];

const PARAPHRASED_KEEP: &[&str] = &[
    "User favors dark theme over light mode.",
    "Resolution: the team chose SQLite as engine.",
    "Architectural boundary: never touch prod storage.",
    "Underlying defect identified: race condition in lock.",
    "Validated that the patch resolves the memory leak.",
    "System parameters adjusted for runtime execution.",
    "Automation policy: format code before submitting.",
    "Urgent warning: memory reached critical threshold.",
];

const PARAPHRASED_DROP: &[&str] = &[
    "The test suite succeeded without any failures.",
    "Opened a pull request for branch review.",
    "Filed a bug report in the tracking system.",
    "Keep-alive ping acknowledged by remote peer.",
    "Terminal session transcript written to disk.",
    "Much appreciated, thank you for your help.",
    "Understood the instructions, moving ahead.",
    "Currently working on updating the schema.",
];

// Held-out suite: same keep/drop intents as the mission rules but phrased with
// vocabulary that appears in no SYNONYM_CLUSTERS entry and reuses no rule text,
// so a hit cannot be explained by the prototype's own synonym table.
const HELD_OUT_KEEP: &[&str] = &[
    "Ruling recorded: the team would rather have tabs here.",
    "Ruling: the bank settled its storage engine on SQLite.",
    "Hard limitation: deploys may only target staging.",
    "Primary source of the outage was a locked table.",
    "Independently checked: the replica resumes after reboot.",
    "Engineer-set values cap the pool at 64.",
    "The standing convention: lint before every commit.",
    "Flag raised: disk almost full on the main volume.",
];

const HELD_OUT_DROP: &[&str] = &[
    "All specs green after the build.",
    "Put up a merge proposal for review.",
    "Chalked up a fresh incident for later digging.",
    "Liveness probe answered promptly at noon.",
    "Shell history saved to the log file.",
    "Cheers, that cleared things up quickly.",
    "Noted, and moving ahead to the next item.",
    "Wiring up the migration helper right now.",
];

#[cfg(test)]
fn assert_approx_eq(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn t04_retention_mission_differently_phrased_spike() {
    let profile = coding_default_profile();
    let mission = profile
        .mission
        .as_ref()
        .expect("coding default has mission");

    let literal_exact =
        compute_set_hit_rate(EXACT_KEEP, EXACT_DROP, |t| evaluate_literal(mission, t));
    let semantic_exact =
        compute_set_hit_rate(EXACT_KEEP, EXACT_DROP, |t| evaluate_semantic(mission, t));

    let literal_paraphrased = compute_set_hit_rate(PARAPHRASED_KEEP, PARAPHRASED_DROP, |t| {
        evaluate_literal(mission, t)
    });
    let semantic_paraphrased = compute_set_hit_rate(PARAPHRASED_KEEP, PARAPHRASED_DROP, |t| {
        evaluate_semantic(mission, t)
    });

    let literal_held_out = compute_set_hit_rate(HELD_OUT_KEEP, HELD_OUT_DROP, |t| {
        evaluate_literal(mission, t)
    });
    let semantic_held_out = compute_set_hit_rate(HELD_OUT_KEEP, HELD_OUT_DROP, |t| {
        evaluate_semantic(mission, t)
    });

    println!(
        "semantic spike: exact literal={literal_exact:.4} semantic={semantic_exact:.4}; \
         paraphrased literal={literal_paraphrased:.4} semantic={semantic_paraphrased:.4}; \
         held-out literal={literal_held_out:.4} semantic={semantic_held_out:.4}"
    );

    assert_approx_eq(literal_exact, 1.0);
    assert_approx_eq(semantic_exact, 1.0);
    assert_approx_eq(literal_paraphrased, 0.0);

    assert!(
        semantic_paraphrased >= 0.875,
        "semantic paraphrased hit rate must be >= 87.5%, got {semantic_paraphrased}"
    );
    assert!(
        semantic_paraphrased > literal_paraphrased,
        "semantic matching must improve hit rate over literal matching"
    );

    // Held-out evidence: with no cluster membership and no rule-text reuse the
    // prototype recovers nothing, so its paraphrase strength is calibration to
    // its own synonym table, not generalization. Re-record the doc header if
    // this ever changes.
    assert_approx_eq(literal_held_out, 0.0);
    assert_approx_eq(semantic_held_out, 0.0);
}

#[test]
fn test_semantic_spike_token_similarity_branches() {
    assert_approx_eq(token_similarity("same", "same"), 1.0);
    assert_approx_eq(token_similarity("testing", "test"), 0.95);
    assert_approx_eq(token_similarity("prefers", "favors"), 0.90);
    assert_approx_eq(token_similarity("unrelated", "completelydifferent"), 0.0);
    assert_approx_eq(char_ngram_similarity("ab", "cd"), 0.0);
    assert_approx_eq(char_ngram_similarity("same", "same"), 1.0);
    assert!(char_ngram_similarity("configuration", "configurations") > 0.8);
}

#[test]
fn test_semantic_spike_helpers_edge_cases() {
    assert!(is_stopword("the"));
    assert!(!is_stopword("database"));

    let empty_tokens = tokenize("   ---   ");
    assert!(empty_tokens.is_empty(), "{empty_tokens:?}");

    let sim_empty = semantic_similarity("", "some text");
    assert_approx_eq(sim_empty, 0.0);

    let sim_empty_candidate = semantic_similarity("some rule", "");
    assert_approx_eq(sim_empty_candidate, 0.0);

    let hit_rate_empty = compute_set_hit_rate(&[], &[], |_| EvaluationMatch::Keep);
    assert_approx_eq(hit_rate_empty, 0.0);

    let sim_stopword_rule = semantic_similarity("it is in", "it is in here");
    assert!(sim_stopword_rule > 0.0);
}
