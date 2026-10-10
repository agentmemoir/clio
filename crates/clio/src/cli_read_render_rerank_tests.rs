// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing that the recall text view says where
//! the hit order came from when a reranker decided it, since the printed score
//! is the pre-rerank value and does not reproduce that order.
//!
//! ## Owns
//! - The rerank-order note appearing only when a reranker scored a hit
//!
//! ## Does not own
//! - The per-hit score breakdown and the stage columns
//!   (`cli_read_render_scores_tests`)
//! - Whether the reranker reorders without rewriting `score`
//!   (`clio-retrieve`'s own tests, which pin the scoring side)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_render::render_text;
use crate::cli_style::Palette;
use serde_json::{Value, json};

/// The note that must survive every recall text view.
const RELATIVE_SCORE_NOTE: &str = "score is relative (rank fusion), not calibrated relevance";

/// The note that must appear when, and only when, a reranker ordered the page.
const RERANK_ORDER_NOTE: &str =
    "order follows the reranker; the score beside each hit is its pre-rerank rank-fusion value";

fn render_recall(payload: &Value) -> String {
    render_text("recall", payload, Palette::new(false))
}

/// One hit whose `scores` block is given. `reranker` is `None` for a page the
/// reranker did not score.
fn hit(reranker: Option<f64>, final_score: f64) -> Value {
    json!({
        "id": "itm-1",
        "content": {"gist": "User prefers dark mode"},
        "category": "persona",
        "epistemic_kind": "fact",
        "kind": "semantic",
        "lexical_rank": 1,
        "score": final_score,
        "scores": {"final": final_score, "semantic": 0.83, "keyword": 0.0, "reranker": reranker}
    })
}

/// Regression cover for issue #38: with a reranker configured, `clio recall`
/// ordered the page by the reranker but printed each hit's pre-rerank fused
/// score, so the numbers visibly jumped around and read as a broken ranking
/// rather than a deliberate one.
#[test]
fn a_reranked_page_says_the_order_came_from_the_reranker() {
    let text = render_recall(&json!({
        "query": "dark mode",
        "hits": [
            hit(Some(0.91), 0.0240),
            hit(Some(0.40), 0.0115),
            hit(Some(0.12), 0.0232)
        ]
    }));
    assert!(text.contains(RERANK_ORDER_NOTE), "{text}");
    // The note has to sit with the other caveat, above the hits, or a reader
    // meets the first number before learning the order is not that number.
    let note_at = text.find(RERANK_ORDER_NOTE).expect("the note is present");
    let first_score_at = text.find("score 0.024").expect("a score is printed");
    assert!(note_at < first_score_at, "{text}");
    // The existing caveat still stands; the new note adds to it.
    assert!(text.contains(RELATIVE_SCORE_NOTE), "{text}");
}

/// The note is only true when a reranker ran. Printing it unconditionally
/// would claim an ordering that did not happen, on the common no-reranker page.
#[test]
fn a_page_the_reranker_did_not_score_keeps_the_old_single_note() {
    let text = render_recall(&json!({
        "query": "dark mode",
        "hits": [hit(None, 0.0240), hit(None, 0.0115)]
    }));
    assert!(
        !text.contains(RERANK_ORDER_NOTE),
        "no reranker ran, so the note must not appear: {text}"
    );
    assert!(text.contains(RELATIVE_SCORE_NOTE), "{text}");
}

/// One hit of two scored by the reranker is enough: the page's order is that
/// reranker's doing, and a partial page is what a reranker floor produces.
#[test]
fn a_partly_reranked_page_still_says_so() {
    let text = render_recall(&json!({
        "query": "dark mode",
        "hits": [hit(Some(0.91), 0.0240), hit(None, 0.0115)]
    }));
    assert!(text.contains(RERANK_ORDER_NOTE), "{text}");
}
