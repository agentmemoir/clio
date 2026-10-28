// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for rendering read-command success payloads as
//! human-readable text for the `clio` read surface (recall/get/show/inspect/stats),
//! including the per-stage score breakdown under each recall hit.
//!
//! ## Owns
//! - The per-verb text views over MCP success payloads
//! - The per-hit score breakdown and the derived entity-match reason line
//! - The `recall` limit line and the binding-floor reason under an empty page
//! - The empty-recall and missing-field fallbacks
//!
//! ## Does not own
//! - Score computation; the renderer only formats values it is handed
//! - JSON emission or output-mode selection (see `cli_output`)
//! - Read-command argument decoding and dispatch (see `cli_read`)
//!
//! ## Boundary
//!
//! Changes to a read command's human text view should normally require changes here.
//! Changes to the payload shape should not require changes here.
//!
//! Keep this module focused on human text rendering for read commands.

use crate::cli_style::{MatchStrength, Palette, term_overlap};
use serde_json::Value;

/// The note that keeps the fused score honest: rank fusion produces a relative
/// order, not a calibrated relevance number. Retained verbatim above the hits.
const RELATIVE_SCORE_NOTE: &str = "score is relative (rank fusion), not calibrated relevance";

/// Printed only when a reranker scored at least one hit in the page, because
/// only then does the printed order disagree with the printed number.
const RERANK_ORDER_NOTE: &str =
    "order follows the reranker; the score beside each hit is its pre-rerank rank-fusion value";

/// True when a hit carries a reranker score, which is how the page says the
/// reranker participated. The reranker's own number is in `scores.reranker` and
/// is printed in the per-hit breakdown; it deliberately does not become `score`.
fn hit_reranked(hit: &Value) -> bool {
    hit.get("scores")
        .and_then(|scores| scores.get("reranker"))
        .is_some_and(|value| !value.is_null())
}

/// Printed for a stage that did not score the hit. Absence is not a score, so
/// a missing stage must never render as `0`.
const STAGE_ABSENT: &str = "-";

/// The retrieval arms named in the score breakdown, in display order.
const STAGE_ARMS: [&str; 3] = ["semantic", "keyword", "reranker"];

/// The derived entity-match reason line.
///
/// Printed under the score breakdown for a hit whose query mentions one of its
/// entity names. It is labelled `[derived]` because the mention is computed at
/// read time, not verified against a source span, and it is display-only: the
/// flag never affects ordering or scores. The line names no entity — the flag
/// says a name was mentioned, not which one — so the text view stays free of
/// entity names.
const ENTITY_REASON_LINE: &str = "     [derived] entity match (display-only; not a ranking signal)";

/// Render a read-command payload as deterministic human text.
///
/// `palette` decides whether the view paints; [`Palette::new(false)`] yields the
/// same words with no escape codes, so piped output and the JSON contract are
/// untouched either way.
pub(crate) fn render_text(verb: &str, payload: &Value, palette: Palette) -> String {
    match verb {
        "recall" => render_recall(payload, palette),
        "get" | "show" => render_get(payload),
        "inspect" => render_inspect(payload),
        _ => render_stats(payload),
    }
}

fn render_recall(payload: &Value, palette: Palette) -> String {
    let query = payload
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let hits = payload.get("hits").and_then(Value::as_array);
    let Some(hits) = hits.filter(|hits| !hits.is_empty()) else {
        return empty_recall_text(payload);
    };
    let total = hits.len();
    let mut lines = Vec::new();
    if !query.is_empty() {
        lines.push(palette.heading(&format!("Results for: {query}")));
        lines.push(String::new());
    }
    // The limit that actually ran, because an absent one is a default the
    // caller did not type. Omitted when the payload carries no limit. A
    // clamped request is named on the same line, so the served page never
    // silently disagrees with the request.
    if let Some(limit) = payload.get("limit").and_then(Value::as_u64) {
        let mut line = format!("limit: {limit}");
        if let Some(requested) = payload.get("requested_limit").and_then(Value::as_u64) {
            use std::fmt::Write as _;
            let _ = write!(line, " (lowered from {requested})");
        }
        lines.push(palette.meta(&line));
        lines.push(String::new());
    }
    lines.push(RELATIVE_SCORE_NOTE.to_owned());
    // When a reranker ran it chose the order, and `score` still holds the
    // pre-rerank fused value. Without this the printed numbers visibly jump
    // around and read as a broken ranking rather than a deliberate one (#38).
    if hits.iter().any(hit_reranked) {
        lines.push(RERANK_ORDER_NOTE.to_owned());
    }
    lines.push(String::new());
    let last = total - 1;
    for (index, hit) in hits.iter().enumerate() {
        let final_score = recall_final_score(hit);
        // A hit that carries no fused score at all prints no score fragment,
        // because an absent value is not a zero.
        let score_fragment = final_score.map_or_else(String::new, |value| {
            format!(
                "   {}",
                palette.score(&format!("score {}", format_fused(value)))
            )
        });
        lines.push(format!(
            "  {} {}   {}{score_fragment}",
            palette.rank(index + 1),
            recall_content(hit),
            palette.reason(&format!("[{}]", recall_reason(hit))),
        ));
        lines.push(format!("     {}", palette.meta(&recall_meta(hit))));
        lines.push(format!(
            "     {}",
            palette.scores(&recall_score_breakdown(hit, final_score)),
        ));
        // The strength sits on its own line: the breakdown above is parsed by
        // callers, and appending prose to it would break that parse.
        if let Some(strength_line) = recall_strength_line(hit, &query, index + 1, total, palette) {
            lines.push(strength_line);
        }
        lines.extend(recall_entity_reason_lines(hit));
        if index != last {
            lines.push(String::new());
        }
    }
    lines.join("\n")
}

/// The text for a recall that matched nothing: `no results`, plus each floor
/// warning that names the floor that emptied the page.
///
/// The floor warnings are identified by the suffix their owner,
/// `clio_retrieve::floors`, builds them with, so the two crates share one
/// wording instead of a copied string.
fn empty_recall_text(payload: &Value) -> String {
    let mut lines = vec!["no results".to_owned()];
    // A clamped request is named even on an empty page, so a caller who asked
    // for more than the cap can see the page they were served.
    if let (Some(limit), Some(requested)) = (
        payload.get("limit").and_then(Value::as_u64),
        payload.get("requested_limit").and_then(Value::as_u64),
    ) {
        lines.push(format!("limit: {limit} (lowered from {requested})"));
    }
    if let Some(warnings) = payload.get("warnings").and_then(Value::as_array) {
        lines.extend(
            warnings
                .iter()
                .filter_map(Value::as_str)
                .filter(|warning| warning.contains(clio_retrieve::FLOOR_BINDING_WARNING_SUFFIX))
                .map(str::to_owned),
        );
    }
    lines.join("\n")
}

/// The plain-language strength line for one hit.
///
/// The fused score cannot answer "how close is this?" because rank fusion
/// compresses every result toward zero. This prefers the dense arm's cosine
/// similarity and falls back to query-term overlap, so a lexical-only page still
/// gets a word. The signal is named on the line and the number is printed beside
/// it, so the word can never disagree with the evidence. A hit with neither
/// signal gets no line at all.
fn recall_strength_line(
    hit: &Value,
    query: &str,
    rank: usize,
    total: usize,
    palette: Palette,
) -> Option<String> {
    let stage = |name: &str| {
        hit.get("scores")
            .and_then(|scores| scores.get(name))
            .and_then(Value::as_f64)
    };
    let lexical = term_overlap(
        query,
        &format!("{} {}", recall_content(hit), recall_meta(hit)),
    );
    let (strength, signal, score) = MatchStrength::resolve(stage("semantic"), lexical)?;
    Some(format!(
        "     {}",
        palette.strength(
            &format!(
                "match: {} | {signal} {score:.3} | rank {rank} of {total}",
                strength.as_str()
            ),
            strength.is_strong(),
        )
    ))
}

/// The fused score shown for one hit, or `None` when the hit carries none.
///
/// `scores.final` mirrors the top-level `score`, so preferring the score object
/// keeps the headline and the breakdown line reading the same number even when
/// only one of the two is present.
fn recall_final_score(hit: &Value) -> Option<f64> {
    hit.get("scores")
        .and_then(|scores| scores.get("final"))
        .and_then(Value::as_f64)
        .or_else(|| hit.get("score").and_then(Value::as_f64))
}

/// The headline `score` fragment and the breakdown's `final` entry share one
/// formatter, so the two can never print the same fused value differently.
fn format_fused(value: f64) -> String {
    format!("{value:.4}")
}

/// The per-stage score line for one hit: the fused `final` value plus every arm
/// that scored the hit, in a fixed order so the line is stable across hits.
///
/// Nothing is recomputed, summed, or rescaled, because the arms live on
/// incompatible scales. A stage that did not score the hit prints the
/// placeholder, never `0`; a stage that did is printed exactly as the retrieval
/// layer produced it, so a value too small to see is never rounded into a
/// misleading `0.0000`. No content or entity name can reach this line: it is
/// built from the score object alone.
fn recall_score_breakdown(hit: &Value, final_score: Option<f64>) -> String {
    let arm = |name: &str| {
        hit.get("scores")
            .and_then(|scores| scores.get(name))
            .and_then(Value::as_f64)
    };
    let arms: Vec<String> = STAGE_ARMS
        .iter()
        .map(|name| {
            let shown =
                arm(name).map_or_else(|| STAGE_ABSENT.to_owned(), |value| value.to_string());
            format!("{name} {shown}")
        })
        .collect();
    let final_shown = final_score.map_or_else(|| STAGE_ABSENT.to_owned(), format_fused);
    format!("scores: final {final_shown}  {}", arms.join("  "))
}

/// The derived entity-reason line for a hit, when the retrieval layer marked
/// it.
///
/// `entity_match` is the single derived flag computed by `clio-retrieve` from
/// the query and the hit's own entity names; this renderer only formats it, so
/// the text view and the JSON payload cannot disagree about whether a hit
/// matched. The line lands directly after the score breakdown, in the slot the
/// recall block reserved for it. A missing or false flag emits no line.
fn recall_entity_reason_lines(hit: &Value) -> Vec<String> {
    if hit.get("entity_match").and_then(Value::as_bool) == Some(true) {
        vec![ENTITY_REASON_LINE.to_owned()]
    } else {
        Vec::new()
    }
}

/// Content shown for one recall hit (its gist, else a placeholder).
fn recall_content(hit: &Value) -> &str {
    hit.get("gist")
        .and_then(Value::as_str)
        .filter(|gist| !gist.is_empty())
        .unwrap_or("(no content)")
}

/// Which retrieval leg(s) surfaced this hit, as a short match reason.
fn recall_reason(hit: &Value) -> &'static str {
    match (has_rank(hit, "dense_rank"), has_rank(hit, "lexical_rank")) {
        (true, true) => "semantic + lexical",
        (true, false) => "semantic match",
        (false, true) => "lexical match",
        (false, false) => "graph match",
    }
}

fn has_rank(hit: &Value, key: &str) -> bool {
    hit.get(key).is_some_and(|rank| !rank.is_null())
}

/// Compact metadata line for one recall hit: id, category, epistemic kind, kind.
fn recall_meta(hit: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(id) = hit.get("item_id").and_then(Value::as_str) {
        parts.push(format!("id: {id}"));
    }
    for key in ["category", "epistemic_kind", "kind"] {
        if let Some(value) = hit.get(key).and_then(Value::as_str) {
            parts.push(format!("{key}: {value}"));
        }
    }
    parts.join("  ")
}

fn render_get(payload: &Value) -> String {
    let mut lines = Vec::new();
    let id = payload
        .get("item_id")
        .or_else(|| payload.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("?");
    lines.push(format!("id: {id}"));
    for key in ["category", "epistemic_kind", "gist"] {
        if let Some(value) = payload.get(key).and_then(Value::as_str) {
            lines.push(format!("{key}: {value}"));
        }
    }
    if let Some(snapshot) = payload.get("snapshot") {
        lines.push(format!("snapshot: {snapshot}"));
    }
    if let Some(authoritative) = payload.get("authoritative").and_then(Value::as_bool) {
        lines.push(format!("authoritative: {authoritative}"));
    }
    for key in ["discarded", "archived"] {
        if payload.get(key).and_then(Value::as_bool) == Some(true) {
            lines.push(format!("{key}: true"));
        }
    }
    if let Some(context) = payload
        .get("context")
        .and_then(Value::as_str)
        .filter(|context| !context.is_empty())
    {
        lines.push(format!("context: {context}"));
    }
    lines.join("\n")
}

fn render_inspect(payload: &Value) -> String {
    let items = payload.get("items").and_then(Value::as_array);
    let returned = items.map_or(0, Vec::len);
    let total = payload.get("total").and_then(Value::as_u64);
    let total = total
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(returned);
    let mut header = format!("items: {returned} of {total}");
    // The page the caller asked for was lowered to the cap; name both values
    // so the header never silently disagrees with the request.
    if let (Some(limit), Some(requested)) = (
        payload.get("limit").and_then(Value::as_u64),
        payload.get("requested_limit").and_then(Value::as_u64),
    ) {
        use std::fmt::Write as _;
        let _ = write!(header, " (limit {limit}, lowered from {requested})");
    }
    let mut lines = vec![header];
    if let Some(items) = items {
        for item in items {
            let id = item.get("item_id").and_then(Value::as_str).unwrap_or("?");
            lines.push(id.to_owned());
        }
    }
    lines.join("\n")
}

fn render_stats(payload: &Value) -> String {
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_owned()
    };
    let number = |key: &str| match payload.get(key).and_then(Value::as_u64) {
        Some(n) => n.to_string(),
        None => "?".to_owned(),
    };
    format!(
        "bank: {}\nitems_total: {}\nitems_active: {}\nitems_discarded: {}",
        text("bank"),
        number("items_total"),
        number("items_active"),
        number("items_discarded")
    )
}

#[cfg(test)]
#[path = "cli_read_render_rerank_tests.rs"]
mod cli_read_render_rerank_tests;

#[cfg(test)]
#[path = "cli_read_render_get_tests.rs"]
mod cli_read_render_get_tests;

#[cfg(test)]
#[path = "cli_read_render_scores_tests.rs"]
mod cli_read_render_scores_tests;
