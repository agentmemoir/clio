// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the recall text view's per-stage score breakdown: every
//! arm present, each arm absent, the score object missing entirely, no fused
//! score at all, the derived entity-match reason line, and the unchanged
//! empty-recall text.
//!
//! ## Owns
//! - `render_text("recall", …)` formatting assertions over synthetic payloads
//!
//! ## Does not own
//! - Score computation or the score object itself (see `clio-retrieve`)
//! - The live CLI recall path (`cli_read_output_tests`) and the score-object
//!   doubles (`cli_read_scores_tests`)
//!
//! ## Boundary
//!
//! Changes to the recall text layout should normally require changes here.
//! Changes to how the CLI reaches the renderer should not require changes here.
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_render::render_text;
use crate::cli_style::Palette;
use serde_json::{Value, json};

/// The note that must survive every recall text view.
const RELATIVE_SCORE_NOTE: &str = "score is relative (rank fusion), not calibrated relevance";

fn render_recall(payload: &Value) -> String {
    render_text("recall", payload, Palette::new(false))
}

/// The `scores:` breakdown line for the first hit, or `""` when absent.
fn breakdown_line(text: &str) -> &str {
    text.lines()
        .find(|line| line.trim_start().starts_with("scores:"))
        .unwrap_or("")
}

/// The headline line for the first hit.
fn headline_line(text: &str) -> &str {
    text.lines()
        .find(|line| line.trim_start().starts_with("1."))
        .unwrap_or("")
}

/// One hit in the shape the retrieve surface emits, with the score object as
/// the only carrier of the fused value. `lexical_rank` is set so the hit reads
/// as the lexical match it is; the dense leg did not score it.
fn hit(scores: &Value) -> Value {
    json!({
        "item_id": "itm-1",
        "gist": "terse answers",
        "category": "task_spec",
        "epistemic_kind": "fact",
        "lexical_rank": 1,
        "scores": scores
    })
}

fn recall_with(scores: &Value) -> String {
    render_recall(&json!({"query": "terse", "hits": [hit(scores)]}))
}

#[test]
fn every_stage_is_shown_with_its_own_value() {
    let text = recall_with(&json!({
        "final": 0.0118,
        "semantic": 0.83,
        "keyword": 2.48,
        "reranker": 0.9
    }));
    assert_eq!(
        breakdown_line(&text),
        "     scores: final 0.0118  semantic 0.83  keyword 2.48  reranker 0.9",
        "{text}"
    );
    // The headline and the breakdown read the same fused value.
    assert!(headline_line(&text).ends_with("   score 0.0118"), "{text}");
    assert!(text.contains(RELATIVE_SCORE_NOTE), "{text}");
    assert!(
        text.contains("id: itm-1  category: task_spec  epistemic_kind: fact"),
        "{text}"
    );
    assert!(text.contains("[lexical match]"), "{text}");
}

#[test]
fn an_absent_stage_renders_the_placeholder_and_never_zero() {
    // No embedder and no reranker: the dense and rerank arms never scored the
    // hit, so both are absent while the lexical arm did score it.
    let text = recall_with(&json!({
        "final": 0.0118,
        "semantic": null,
        "keyword": 2.48,
        "reranker": null
    }));
    assert_eq!(
        breakdown_line(&text),
        "     scores: final 0.0118  semantic -  keyword 2.48  reranker -",
        "{text}"
    );
    for absent in ["semantic", "reranker"] {
        assert!(
            !breakdown_line(&text).contains(&format!("{absent} 0")),
            "{absent} must not render as a zero: {text}"
        );
    }
}

/// A real score small enough to look like zero must not be rounded into one.
/// The lexical arm on a single-item bank reports `1e-6`, which a four-decimal
/// format would print as `0.0000` — indistinguishable from a missing stage.
#[test]
fn a_tiny_stage_value_is_not_rounded_into_a_zero() {
    let text = recall_with(&json!({
        "final": 0.011_967_209_826_831_813,
        "semantic": null,
        "keyword": 1e-6,
        "reranker": null
    }));
    assert_eq!(
        breakdown_line(&text),
        "     scores: final 0.0120  semantic -  keyword 0.000001  reranker -",
        "{text}"
    );
    assert!(!breakdown_line(&text).contains("keyword 0.0000 "), "{text}");
    // The fused value keeps the headline's four-decimal form, so the two agree.
    assert!(headline_line(&text).ends_with("   score 0.0120"), "{text}");
}

/// A stage that really scored zero is distinguishable from a stage that never
/// scored: the first prints `0`, the second prints the placeholder.
#[test]
fn a_present_zero_prints_zero_and_an_absent_stage_prints_the_placeholder() {
    let text = recall_with(&json!({
        "final": 0.0118,
        "semantic": 0.0,
        "keyword": 0.0,
        "reranker": null
    }));
    assert_eq!(
        breakdown_line(&text),
        "     scores: final 0.0118  semantic 0  keyword 0  reranker -",
        "{text}"
    );
}

#[test]
fn an_all_null_score_object_never_shows_a_zero() {
    let text = recall_with(&json!({
        "final": null,
        "semantic": null,
        "keyword": null,
        "reranker": null
    }));
    assert_eq!(
        breakdown_line(&text),
        "     scores: final -  semantic -  keyword -  reranker -",
        "{text}"
    );
    assert!(
        !headline_line(&text).contains("score "),
        "no fused score means no score fragment: {text}"
    );
}

#[test]
fn a_missing_score_object_falls_back_to_the_top_level_score() {
    let payload = json!({
        "query": "terse",
        "hits": [{
            "item_id": "itm-1",
            "gist": "terse answers",
            "score": 0.0118
        }]
    });
    let text = render_recall(&payload);
    assert_eq!(
        breakdown_line(&text),
        "     scores: final 0.0118  semantic -  keyword -  reranker -",
        "{text}"
    );
    assert!(headline_line(&text).ends_with("   score 0.0118"), "{text}");
}

#[test]
fn the_score_object_wins_when_only_it_carries_the_fused_value() {
    // `scores.final` mirrors the top-level `score`; the two never disagree, so
    // the renderer reads the object and the headline keeps the same number.
    let payload = json!({
        "hits": [{
            "item_id": "itm-1",
            "gist": "terse answers",
            "scores": {"final": 0.42, "semantic": 0.5, "keyword": null, "reranker": null}
        }]
    });
    let text = render_recall(&payload);
    assert_eq!(
        breakdown_line(&text),
        "     scores: final 0.4200  semantic 0.5  keyword -  reranker -",
        "{text}"
    );
    assert!(headline_line(&text).ends_with("   score 0.4200"), "{text}");
}

#[test]
fn a_hit_without_any_score_prints_no_score_fragment() {
    let payload = json!({"hits": [{"item_id": "itm-1", "gist": "terse answers"}]});
    let text = render_recall(&payload);
    assert_eq!(
        headline_line(&text),
        "  1. terse answers   [graph match]",
        "{text}"
    );
    assert_eq!(
        breakdown_line(&text),
        "     scores: final -  semantic -  keyword -  reranker -",
        "{text}"
    );
}

#[test]
fn a_matched_hit_prints_the_derived_entity_reason() {
    // The derived flag lands in the slot directly under the score breakdown,
    // exactly once, labelled and scoped by the line itself.
    let mut matched = hit(&json!({"final": 0.0118, "keyword": 2.48}));
    matched["entity_match"] = json!(true);
    let text = render_recall(&json!({"query": "terse answers", "hits": [matched]}));
    let lines: Vec<&str> = text.lines().collect();
    let breakdown = lines
        .iter()
        .position(|line| line.trim_start().starts_with("scores:"))
        .expect("breakdown line");
    // The strength line sits between the breakdown and the entity reason; the
    // reason must still land in its own reserved slot, exactly once.
    let reason = lines
        .iter()
        .position(|line| line.contains("entity match"))
        .expect("entity reason line");
    assert_eq!(
        lines[reason], "     [derived] entity match (display-only; not a ranking signal)",
        "{text}"
    );
    assert!(
        lines[breakdown + 1].trim_start().starts_with("match:"),
        "the strength line follows the breakdown: {text}"
    );
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.contains("entity match"))
            .count(),
        1,
        "exactly one reason line: {text}"
    );
    // Regression: the provenance reason, the disclaimer, and the breakdown are
    // untouched by the derived line.
    assert!(text.contains(RELATIVE_SCORE_NOTE), "{text}");
    assert!(text.contains("[lexical match]"), "{text}");
    assert_eq!(
        breakdown_line(&text),
        "     scores: final 0.0118  semantic -  keyword 2.48  reranker -",
        "{text}"
    );
}

/// The reason line is derived and non-authoritative, and it prints no entity
/// name even when the hit payload itself carries one.
#[test]
fn the_derived_reason_never_prints_an_entity_name() {
    let payload = json!({
        "query": "Acme Corp renewal",
        "hits": [{
            "item_id": "itm-1",
            "gist": "terse answers",
            "snapshot": {"entity": "Acme Corp", "amount": 4200},
            "entity_match": true,
            "scores": {"final": 0.0118, "keyword": 2.48}
        }]
    });
    let text = render_recall(&payload);
    let reason = text
        .lines()
        .find(|line| line.contains("[derived]"))
        .expect("reason line present");
    assert!(
        reason.contains("display-only"),
        "labelled display-only: {reason}"
    );
    for leak in ["Acme", "Corp", "4200"] {
        assert!(!reason.contains(leak), "reason leaked {leak}: {reason}");
    }
}

#[test]
fn the_entity_reason_slot_is_reserved_but_emits_nothing() {
    // The slot sits between the breakdown and the next hit. A hit with no
    // match (flag absent or false) keeps the block at exactly three lines, so
    // the derived reason lands in the reserved place without moving any line.
    let text = recall_with(&json!({"final": 0.0118, "keyword": 2.48}));
    let block = text
        .lines()
        .skip_while(|line| *line != RELATIVE_SCORE_NOTE)
        .skip(2)
        .count();
    assert_eq!(block, 4, "headline, meta, breakdown, strength: {text}");
    assert!(
        !text.contains("entity"),
        "no entity reason is emitted: {text}"
    );

    let mut unmatched = hit(&json!({"final": 0.0118, "keyword": 2.48}));
    unmatched["entity_match"] = json!(false);
    let text = render_recall(&json!({"query": "terse", "hits": [unmatched]}));
    assert!(
        !text.contains("entity"),
        "an explicit false emits nothing: {text}"
    );
}

#[test]
fn the_breakdown_carries_no_content_or_entity_names() {
    // The breakdown is built from the score object alone, so a hit carrying a
    // snapshot and source text cannot push either into the rendered line.
    let payload = json!({
        "hits": [{
            "item_id": "itm-1",
            "gist": "terse answers",
            "score": 0.0118,
            "snapshot": {"entity": "Acme Corp", "amount": 4200},
            "source_text": "the user prefers terse answers",
            "scores": {"final": 0.0118, "semantic": 0.83, "keyword": 2.48, "reranker": 0.9}
        }]
    });
    let text = render_recall(&payload);
    let line = breakdown_line(&text);
    assert_eq!(
        line, "     scores: final 0.0118  semantic 0.83  keyword 2.48  reranker 0.9",
        "{text}"
    );
    for leak in ["Acme", "Corp", "4200", "prefers", "terse"] {
        assert!(!line.contains(leak), "breakdown leaked {leak}: {line}");
    }
}

#[test]
fn the_empty_recall_text_is_unchanged() {
    assert_eq!(render_recall(&json!({"hits": []})), "no results");
    assert_eq!(render_recall(&json!({})), "no results");
}

#[test]
fn every_hit_gets_its_own_breakdown_in_order() {
    let text = render_recall(&json!({
        "hits": [
            hit(&json!({"final": 0.02, "keyword": 3.0})),
            hit(&json!({"final": 0.01, "semantic": 0.25, "keyword": 1.0}))
        ]
    }));
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| line.trim_start().starts_with("scores:"))
        .collect();
    assert_eq!(
        lines,
        [
            "     scores: final 0.0200  semantic -  keyword 3  reranker -",
            "     scores: final 0.0100  semantic 0.25  keyword 1  reranker -",
        ],
        "{text}"
    );
}

#[test]
fn the_strength_line_names_its_signal_and_prints_the_number() {
    // The word must never float free of its evidence: the signal it came from
    // and the number behind it are on the same line.
    let text = render_recall(&json!({
        "query": "terse answers",
        "hits": [hit(&json!({"final": 0.0118, "semantic": 0.83, "keyword": 2.48}))]
    }));
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("match:"))
        .expect("strength line");
    assert_eq!(
        line.trim(),
        "match: strong match | cosine 0.830 | rank 1 of 1",
        "{text}"
    );
}

#[test]
fn a_lexical_only_page_still_prints_a_strength_line() {
    // No dense arm: the word must still appear, sourced from term overlap,
    // rather than leaving the reader with only the rank-fusion number.
    let text = render_recall(&json!({
        "query": "terse answers",
        "hits": [hit(&json!({"final": 0.0118, "keyword": 2.48}))]
    }));
    let line = text
        .lines()
        .find(|l| l.trim_start().starts_with("match:"))
        .expect("strength line");
    assert!(line.contains("term overlap"), "{line}");
    assert!(line.contains("strong match"), "{line}");
}
