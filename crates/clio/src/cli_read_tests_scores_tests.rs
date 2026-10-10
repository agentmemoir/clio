// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the CLI `recall` score object end to end: the documented
//! `final` / `reranker` / `semantic` / `keyword` key contract, the lexical-only
//! `semantic: null` case, a populated `semantic` with a live embedder, a
//! populated `reranker` with a configured reranker sidecar, the binding parity
//! between the CLI JSON payload and the MCP `retrieve` payload, and the text
//! view's per-stage breakdown when every arm is populated at once.
//!
//! ## Owns
//! - `recall --output json` `scores` assertions and CLI/MCP score parity
//! - `recall --output text` breakdown assertions for the populated arms
//!
//! ## Does not own
//! - Score computation or the dense transform (`clio-retrieve` stage scoring)
//! - The local TEI doubles (see `cli_read_tei_doubles_tests`)
//! - The read-command runners and seed fixtures (shared from `cli_read_tests`)
//!
//! ## Boundary
//!
//! Changes to the score-object shape should normally require changes here.
//! Changes to the read-command surface should not require changes here.
//!
//! Keep this module focused on the CLI recall score-object tests.

use super::tei_doubles_tests::{
    fake_rerank_server, fake_tei_server, state_with_embedder_and_reranker,
    state_with_live_embedder, state_with_reranker,
};
use super::*;
use serde_json::Value;
use std::process::ExitCode;
use std::sync::atomic::Ordering;

/// The note the recall text view must keep above the hits.
const RELATIVE_SCORE_NOTE: &str = "score is relative (rank fusion), not calibrated relevance";

/// One `scores: final ...` line of a recall text view, with every number kept as
/// the digits the view actually printed alongside the parsed value.
struct Breakdown<'a> {
    final_text: &'a str,
    final_value: f64,
    arms: Vec<(String, f64, String)>,
}

/// Every well-formed breakdown line in a recall text view.
///
/// A line that is not a breakdown, or that carries an arm token which is not a
/// number, is skipped rather than counted as a match: the absent placeholder is
/// exactly what must never be mistaken for a value.
fn breakdown_lines(text: &str) -> Vec<Breakdown<'_>> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("scores: final ")?;
            let mut tokens = rest.split_whitespace();
            let final_text = tokens.next()?;
            let arms = tokens
                .collect::<Vec<_>>()
                .chunks(2)
                .map(|pair| {
                    Some((
                        pair[0].to_owned(),
                        pair[1].parse().ok()?,
                        pair[1].to_owned(),
                    ))
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Breakdown {
                final_text,
                final_value: final_text.parse().ok()?,
                arms,
            })
        })
        .collect()
}

#[test]
fn recall_json_scores_match_mcp_retrieve() {
    let state = state_with_items();
    let (code, out, err) = run_state("recall", &["terse", "--bank", "bank-a"], &state, false);
    assert_eq!(code, ExitCode::from(0));
    assert!(err.is_empty(), "stderr: {err}");
    let cli = json_of(&out);
    let cli_hits = cli["hits"].as_array().expect("cli hits");
    assert!(!cli_hits.is_empty(), "expected a hit: {cli}");

    // The CLI recall path and this call share the dispatcher, so comparing the
    // two payloads proves the CLI passes the score values through unchanged.
    let mcp = clio_mcp::write_tools::dispatch(
        &state,
        "retrieve",
        &json!({"bank": "bank-a", "query": "terse"}),
    );
    let mcp_hits = mcp["hits"].as_array().expect("mcp hits");
    assert_eq!(cli_hits.len(), mcp_hits.len(), "same hits on both bindings");

    for hit in cli_hits {
        let scores = hit["scores"].as_object().expect("cli scores object");
        let mut keys: Vec<&str> = scores.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "age_factor",
                "final",
                "keyword",
                "reranker",
                "semantic",
                "trust_factor"
            ],
            "{hit}"
        );
        assert_eq!(scores["final"], hit["score"], "final mirrors score: {hit}");
        assert_eq!(scores["reranker"], Value::Null, "{hit}");
        assert!(scores["semantic"].is_null(), "no embedder: {hit}");
        assert!(scores["keyword"].is_number(), "{hit}");
        for (name, value) in scores {
            assert!(
                value.is_number() || value.is_null(),
                "scores.{name} must be a number or null, not text: {hit}"
            );
        }

        let item_id = hit["item_id"].as_str().expect("item_id");
        let twin = mcp_hits
            .iter()
            .find(|candidate| candidate["item_id"] == json!(item_id))
            .unwrap_or_else(|| panic!("MCP retrieve omitted {item_id}: {mcp}"));
        let twin_scores = twin["scores"].as_object().expect("mcp scores object");
        assert_eq!(twin_scores.len(), 6, "mcp score keys: {twin}");
        // `final` is the fused score recomputed on each call, so pin it to its
        // own top-level `score` and compare the arm values exactly.
        assert_eq!(twin_scores["final"], twin["score"], "{twin}");
        for arm in ["reranker", "semantic", "keyword"] {
            assert_eq!(scores[arm], twin_scores[arm], "{arm} for {item_id}");
        }
        let cli_final = scores["final"].as_f64().expect("numeric final");
        let mcp_final = twin_scores["final"].as_f64().expect("numeric final");
        assert!(
            (cli_final - mcp_final).abs() <= 1e-9 * cli_final.abs().max(mcp_final.abs()),
            "final parity for {item_id}: {cli_final} vs {mcp_final}"
        );
    }
}

/// Dense parity: a live embedder makes the CLI `recall` payload carry a
/// POPULATED `scores.semantic`, and the MCP `retrieve` binding reports the same
/// value for the same item.
#[test]
fn recall_json_semantic_is_populated_with_live_embedder() {
    let tei = fake_tei_server();
    let state = state_with_live_embedder(&tei.url);
    assert!(
        state.pipeline.dense_available,
        "the embed block must attach the dense arm"
    );

    seed(&state, "bank-a", "itm-dense", "terse answers");
    let report = state.drain_index().expect("drain index");
    assert_eq!(report.applied, 1, "{report:?}");
    assert_eq!(report.failed, 0, "{report:?}");

    let (code, out, err) = run_state(
        "recall",
        &["terse answers", "--bank", "bank-a"],
        &state,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    assert!(err.is_empty(), "stderr: {err}");
    let cli = json_of(&out);
    let hit = cli["hits"]
        .as_array()
        .expect("cli hits")
        .iter()
        .find(|hit| hit["item_id"] == json!("itm-dense"))
        .unwrap_or_else(|| panic!("dense hit missing: {cli}"));

    let scores = hit["scores"].as_object().expect("cli scores object");
    let semantic = scores["semantic"]
        .as_f64()
        .unwrap_or_else(|| panic!("populated numeric semantic: {hit}"));
    assert!(
        (0.0..=1.0).contains(&semantic),
        "semantic must be in [0,1]: {hit}"
    );
    assert!(!scores["semantic"].is_null(), "not the null case: {hit}");
    assert!(hit["dense_rank"].is_number(), "dense rank present: {hit}");
    assert_eq!(scores["final"], hit["score"], "final mirrors score: {hit}");
    assert_eq!(scores["reranker"], Value::Null, "{hit}");
    assert!(scores["keyword"].is_number(), "{hit}");

    // Binding parity on the populated case: the MCP `retrieve` dispatch shares
    // the store, so the same item reports the same raw `semantic`.
    let mcp = clio_mcp::write_tools::dispatch(
        &state,
        "retrieve",
        &json!({"bank": "bank-a", "query": "terse answers"}),
    );
    let twin = mcp["hits"]
        .as_array()
        .expect("mcp hits")
        .iter()
        .find(|candidate| candidate["item_id"] == json!("itm-dense"))
        .unwrap_or_else(|| panic!("MCP retrieve omitted itm-dense: {mcp}"));
    assert_eq!(
        twin["scores"]["semantic"], scores["semantic"],
        "populated semantic parity for itm-dense"
    );

    tei.stop.store(true, Ordering::Relaxed);
    let served = tei.handle.join().expect("join fake TEI double");
    assert!(
        served >= 1,
        "the drain embedded through the double: {served}"
    );
}

/// A CONFIGURED reranker reaches the real CLI `recall` path, the provider's
/// order is applied, and every hit carries a POPULATED numeric
/// `scores.reranker` bound to the right document.
#[test]
fn recall_json_reranker_is_populated_with_configured_reranker() {
    let rerank = fake_rerank_server();
    let state = state_with_reranker(&rerank.url);
    assert!(
        state.pipeline.rerank_active,
        "the rerank block must attach the second stage"
    );

    seed(&state, "bank-a", "itm-rerank-a", "terse answers");
    seed(&state, "bank-a", "itm-rerank-b", "plain terse notes");

    let (code, out, err) = run_state("recall", &["terse", "--bank", "bank-a"], &state, false);
    assert_eq!(code, ExitCode::from(0));
    assert!(err.is_empty(), "stderr: {err}");
    let cli = json_of(&out);
    assert_eq!(cli["reranked"], json!(true), "{cli}");

    rerank.stop.store(true, Ordering::Relaxed);
    let served = rerank.handle.join().expect("join fake reranker");
    assert!(served >= 1, "the CLI recall reranked: {served}");
    let texts = rerank.texts.lock().expect("rerank texts").clone();

    let hits = cli["hits"].as_array().expect("cli hits");
    assert!(texts.len() >= 2, "need a reorderable shortlist: {cli}");
    assert_eq!(hits.len(), texts.len(), "same hits as reranked docs: {cli}");

    let mut expected = 0.9_f64;
    for (position, hit) in hits.iter().enumerate() {
        let scores = hit["scores"].as_object().expect("cli scores object");
        let reranker = scores["reranker"]
            .as_f64()
            .unwrap_or_else(|| panic!("populated numeric reranker: {hit}"));
        // The sidecar's score is carried through unchanged (identity clamp), so
        // the expected value is exact, not approximate.
        assert!(
            (reranker - expected).abs() < f64::EPSILON,
            "provider rank {position} must be {expected}: {hit}"
        );
        assert!((0.0..=1.0).contains(&reranker), "{hit}");
        assert_eq!(scores["final"], hit["score"], "final mirrors score: {hit}");
        assert!(scores["semantic"].is_null(), "no embedder: {hit}");
        assert!(scores["keyword"].is_number(), "{hit}");

        // The reversed permutation is bound to the right document: the hit at
        // `position` is the text the sidecar ranked at `texts[n - 1 - position]`.
        let provider_index = texts.len() - 1 - position;
        let gist = hit["gist"].as_str().expect("hit gist");
        assert!(
            texts[provider_index].contains(gist),
            "hit {position} must be provider index {provider_index}: {hit} vs {texts:?}"
        );
        expected -= 0.1;
    }
}

/// The text view's breakdown, checked with EVERY arm populated on the same hit:
/// a live embedder fills `semantic`, the lexical arm fills `keyword`, and the
/// configured sidecar fills `reranker`. The text must show all four values, and
/// each shown value must be the exact value the JSON payload carries, so a layout
/// change in the text view cannot hide a wrong number.
#[test]
fn recall_text_shows_every_populated_stage() {
    let tei = fake_tei_server();
    let rerank = fake_rerank_server();
    let state = state_with_embedder_and_reranker(&tei.url, &rerank.url);
    assert!(
        state.pipeline.dense_available,
        "the embed block must attach the dense arm"
    );
    assert!(
        state.pipeline.rerank_active,
        "the rerank block must attach the second stage"
    );

    seed(&state, "bank-a", "itm-all-a", "terse answers");
    seed(&state, "bank-a", "itm-all-b", "plain terse notes");
    let report = state.drain_index().expect("drain index");
    assert_eq!(report.applied, 2, "{report:?}");
    assert_eq!(report.failed, 0, "{report:?}");

    // JSON first: the payload is the source of the values the text must show.
    let (code, json_out, err) = run_state("recall", &["terse", "--bank", "bank-a"], &state, false);
    assert_eq!(code, ExitCode::from(0));
    assert!(err.is_empty(), "stderr: {err}");
    let payload = json_of(&json_out);
    let hits = payload["hits"].as_array().expect("cli hits").clone();
    assert!(hits.len() >= 2, "need a multi-hit shortlist: {payload}");
    for hit in &hits {
        let scores = hit["scores"].as_object().expect("cli scores object");
        for arm in ["final", "semantic", "keyword", "reranker"] {
            assert!(scores[arm].is_number(), "{arm} must be populated: {hit}");
        }
        assert_eq!(scores["final"], hit["score"], "final mirrors score: {hit}");
    }

    // Text second: the same query, the same payload, rendered for a human.
    let (code, text_out, err) = run_state("recall", &["terse", "--bank", "bank-a"], &state, true);
    assert_eq!(code, ExitCode::from(0));
    assert!(err.is_empty(), "stderr: {err}");
    let lines: Vec<&str> = text_out.lines().collect();
    assert!(
        lines.contains(&RELATIVE_SCORE_NOTE),
        "the relative-score disclaimer is retained: {text_out}"
    );
    let breakdown = breakdown_lines(&text_out);
    assert_eq!(
        breakdown.len(),
        hits.len(),
        "one well-formed breakdown per hit: {text_out}"
    );
    for hit in &hits {
        assert_hit_breakdown_line(hit, &lines, &breakdown);
    }

    rerank.stop.store(true, Ordering::Relaxed);
    assert!(
        rerank.handle.join().expect("join fake reranker") >= 1,
        "the text recall reranked through the double"
    );
    tei.stop.store(true, Ordering::Relaxed);
    assert!(
        tei.handle.join().expect("join fake TEI double") >= 1,
        "the text recall embedded through the double"
    );
}

/// Check one hit's breakdown line against the payload it was rendered from.
///
/// The line is keyed on `final` at four decimals plus the exact `reranker`
/// value: both come from the same in-memory numbers the renderer was handed, and
/// together they name one hit. `semantic` and `reranker` are short decimals, so
/// the text must show the exact value the payload carries, bit for bit.
/// `keyword` is a long decimal and re-parsing the JSON text is not always
/// correctly rounded, so its exact digits are not compared; what is pinned is the
/// property this phase is about, namely that the arm is present, positive, and
/// printed at full precision rather than rounded into a misleading zero.
fn assert_hit_breakdown_line(hit: &Value, lines: &[&str], breakdown: &[Breakdown<'_>]) {
    let scores = hit["scores"].as_object().expect("cli scores object");
    let item_id = hit["item_id"].as_str().expect("item_id");
    let want_final = scores["final"].as_f64().expect("final");
    let want_reranker = scores["reranker"].as_f64().expect("reranker");
    let line = breakdown
        .iter()
        .find(|line| {
            (line.final_value - want_final).abs() <= 0.00005
                && line.arms.iter().any(|(name, value, _)| {
                    name == "reranker" && value.to_bits() == want_reranker.to_bits()
                })
        })
        .unwrap_or_else(|| panic!("no breakdown line for {item_id}"));
    assert_eq!(
        line.arms.len(),
        3,
        "final plus three arms per line for {item_id}"
    );
    let arm = |name: &str| {
        line.arms
            .iter()
            .find(|(shown, _, _)| shown == name)
            .unwrap_or_else(|| panic!("{name} must be shown for {item_id}"))
    };

    for name in ["semantic", "reranker"] {
        let (_, shown, shown_text) = arm(name);
        assert_eq!(
            shown_text,
            &shown.to_string(),
            "{name} printed at reduced precision"
        );
        assert_eq!(
            shown.to_bits(),
            scores[name].as_f64().expect(name).to_bits(),
            "{name} for {item_id}"
        );
        assert!(*shown > 0.0, "{name} must not be zero for {item_id}");
    }

    let (_, keyword, keyword_text) = arm("keyword");
    assert!(
        *keyword > 0.0,
        "the keyword arm must not be zero for {item_id}"
    );
    assert_ne!(
        format!("{keyword:.4}"),
        *keyword_text,
        "an arm must print the exact value, not a 4-decimal rounding for {item_id}"
    );
    assert!(
        scores["keyword"].is_number(),
        "keyword stays a number in the payload: {hit}"
    );

    // `final` keeps the headline's four decimals, and the two agree. The per-hit
    // block is headline, metadata, then the breakdown line.
    assert_eq!(
        line.final_text,
        format!("{want_final:.4}"),
        "final must be shown at four decimals for {item_id}"
    );
    let headline = lines
        .iter()
        .position(|line| line.contains(&format!("id: {item_id}")))
        .and_then(|at| at.checked_sub(1))
        .map_or_else(|| panic!("the headline for {item_id}"), |at| lines[at]);
    assert!(
        headline.ends_with(&format!("score {}", line.final_text)),
        "headline and breakdown agree: {headline} vs {}",
        line.final_text
    );
}
