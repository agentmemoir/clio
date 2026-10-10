// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for how a provenance floor reaches the two read
//! paths through the in-process JSON surface: the unchanged no-floor response,
//! the bank default, an explicit floor's precedence over that default, the
//! refusal of a value outside the ladder, and `compose_context`'s use of the
//! bank default for its own retrieval.
//!
//! ## Owns
//! - The no-floor regression: the whole response pinned value by value under an
//!   injected clock, and a response carrying no provenance key
//! - The bank default, and an explicit floor beating it in both directions
//! - The refusal of a mistyped or empty rung, with no page returned and a
//!   message naming the wire argument
//! - `compose_context`'s memory section observing the bank default
//!
//! ## Does not own
//! - The floor's effect on the retrieval path (`provenance_tests`), the rung
//!   predicate (`floors`), and the rest of the JSON argument contract
//!   (`surface_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the provenance floor's wire resolution.

use crate::compose::Composer;
use crate::fixtures_tests as fx;
use crate::floors::floors_provenance_tests::{ALL_CLASSES, seed_rungs};
use crate::surface::{
    ReadPolicyResolver, compose_from_json, compose_from_json_with_policy, retrieve_from_json,
    retrieve_from_json_with_policy,
};
use clio_config::ranking::{
    DEFAULT_TRUST_AGENT_INFERRED, DEFAULT_TRUST_THIRD_PARTY, DEFAULT_TRUST_USER_STATED,
};
use clio_config::retention::RetentionProfile;
use clio_config::retention_levels::ProvenanceFloorDefault as Floor;
use serde_json::{Value, json};
use std::sync::Arc;

/// The response's pre-existing top-level key set, in name order: every key a
/// default `retrieve` emitted before the provenance floor existed, and no floor
/// key. `dedup` is absent because the default call's report is inert.
const NO_FLOOR_KEYS: [&str; 11] = [
    "ok",
    "hits",
    "dense_candidates",
    "lexical_candidates",
    "fused_candidates",
    "reranked",
    "graph_expanded",
    "pairs_reinforced",
    "estimated_tokens",
    "metrics",
    "warnings",
];

/// The five timing keys, whose values are wall-clock measurements.
const TIMING_KEYS: [&str; 5] = [
    "dense_ms",
    "lexical_ms",
    "expand_ms",
    "rerank_ms",
    "total_ms",
];

/// The floor it would narrow to, so an unrecognised one is caught by name.
const RUNGS: [&str; 4] = ["verified", "stated", "third_party", "inferred"];

/// The clock the request injects, so the recency boost is not the system clock.
const FIXED_NOW: &str = "2026-01-02T00:00:00Z";

/// The age factor one day after the fixture timestamps, at the fixed clock.
const AGE: f64 = 0.982_869_976_325_684_4;

/// A key list in name order, for a key-set comparison.
fn sorted<const N: usize>(keys: [&'static str; N]) -> Vec<&'static str> {
    let mut keys = keys.to_vec();
    keys.sort_unstable();
    keys
}

/// The trust ratio the default environment applies to one fixture source.
fn trust_ratio(source: Option<&str>) -> f64 {
    match source {
        None | Some("user_stated") => DEFAULT_TRUST_USER_STATED,
        Some("third_party") => DEFAULT_TRUST_THIRD_PARTY,
        Some("agent_inferred") => DEFAULT_TRUST_AGENT_INFERRED,
        Some(other) => panic!("no trust ratio for {other}"),
    }
}

/// One expected hit: the frozen fixture item's whole response contract.
///
/// The fixture exercises every epistemic class, so each belief's score is its
/// pre-trust base times its default trust ratio, and its `scores` object
/// carries that same factor.
fn hit(id: &str, rank: usize, score: f64, keyword: f64, source: Option<&str>) -> Value {
    let belief = source.is_some();
    let mut expected = json!({
        "bank_id": "bank",
        "category": "schema",
        "entities": [],
        "epistemic_kind": if belief { "belief" } else { "fact" },
        "gist": format!("alpha memory {id}"),
        "item_id": id,
        "kind": "semantic",
        "lexical_rank": rank,
        "requires_confidence_check": belief,
        "score": score,
        "scores": {
            "age_factor": AGE,
            "final": score,
            "keyword": keyword,
            "reranker": null,
            "semantic": null,
            "trust_factor": trust_ratio(source)
        },
        "snapshot_ref": id
    });
    if belief {
        let object = expected.as_object_mut().expect("hit object");
        object.insert("confidence".into(), json!(0.42));
        object.insert(
            "source_type".into(),
            json!(source.expect("a belief has one")),
        );
    }
    expected
}

/// The response with its whole timing block replaced by one constant, so the
/// full-equality assertion covers every other field and value exactly.
fn page_without_timings(value: &Value) -> Value {
    let mut page = value.clone();
    page.as_object_mut()
        .expect("outcome object")
        .insert("metrics".into(), json!("timings are wall-clock"));
    page
}

/// The response with each hit's `keyword` rounded to 12 significant digits.
///
/// `scores.keyword` is the raw backend lexical score, `-bm25(items_fts)` on
/// SQLite, which `stage_score` documents as backend-dependent with no parity
/// claimed. The SQLite C `bm25()` is compiled per host through `rusqlite`'s
/// `bundled` feature, so two hosts can disagree by one unit in the last place
/// on an identical query. Twelve digits clears that by many orders of magnitude
/// and stays far below any real change to the lexical arm.
fn page_with_rounded_keyword(value: &Value) -> Value {
    let mut page = value.clone();
    if let Some(hits) = page["hits"].as_array_mut() {
        for hit in hits {
            let rounded = hit["scores"]["keyword"]
                .as_f64()
                .map(|keyword| json!(format!("{keyword:.12e}")));
            if let (Some(scores), Some(rounded)) = (hit["scores"].as_object_mut(), rounded) {
                scores.insert("keyword".into(), rounded);
            }
        }
    }
    page
}

/// A `retrieve` outcome names no provenance key of its own. The whole
/// pre-existing key set is pinned once, by the default-call test in
/// `floors_integration_tests`; what is new here is only that the floor named no
/// key in the response.
fn assert_no_provenance_key(value: &Value) {
    let leaked: Vec<&str> = value
        .as_object()
        .expect("outcome object")
        .keys()
        .map(String::as_str)
        .filter(|key| key.contains("provenance") || key.contains("floor"))
        .collect();
    assert!(leaked.is_empty(), "the floor published a key: {leaked:?}");
}

/// Bank read-policy stand-in whose profile carries a `stated` provenance floor
/// and nothing else that differs from the builtin default.
struct StatedFloorBank;

impl ReadPolicyResolver for StatedFloorBank {
    fn read_policy(&self, _bank: &str) -> RetentionProfile {
        RetentionProfile {
            provenance_floor_default: Some(Floor::Stated),
            ..RetentionProfile::default()
        }
    }
}

/// The item ids of a `retrieve` response's page, in rank order.
fn ids(value: &Value) -> Vec<String> {
    value["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["item_id"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// A request that names no floor returns the page and the payload shape it
/// returned before the floor existed: no new key, and every class present.
#[test]
fn a_request_without_a_floor_returns_the_pre_existing_payload() {
    let store = fx::store();
    seed_rungs(&store);
    let retriever = fx::retriever(&store);

    let result = retrieve_from_json(&retriever, &json!({"bank": "bank", "query": "alpha"}));
    let value = serde_json::to_value(&result).expect("serialize");
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(
        ids(&value),
        ALL_CLASSES,
        "no floor means every class: {value}"
    );

    assert_no_provenance_key(&value);
}

/// With a fixed clock and a fixed corpus the whole no-floor response is pinned
/// value by value, so an added, removed, or changed field fails this test.
///
/// The expected value is written out here rather than kept in a golden file, so
/// a reviewer can read the promise in one place and a legitimate field addition
/// has to be spelled out here on purpose.
#[test]
fn a_request_without_a_floor_returns_the_whole_pre_existing_response() {
    let store = fx::store();
    seed_rungs(&store);
    let retriever = fx::retriever(&store);

    let result = retrieve_from_json(
        &retriever,
        &json!({"bank": "bank", "query": "alpha", "now": FIXED_NOW}),
    );
    let value = serde_json::to_value(&result).expect("serialize");

    // The key set, which the value equality below also proves.
    let mut keys: Vec<&str> = value
        .as_object()
        .expect("outcome object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, sorted(NO_FLOOR_KEYS), "top-level keys: {value}");
    // The timing values are wall-clock measurements, so only their key set is
    // pinned; every other value below is compared exactly.
    let mut timings: Vec<&str> = value["metrics"]
        .as_object()
        .expect("metrics")
        .keys()
        .map(String::as_str)
        .collect();
    timings.sort_unstable();
    assert_eq!(timings, sorted(TIMING_KEYS), "timing keys: {value}");

    let expected = json!({
        "ok": true,
        "dense_candidates": 0,
        "estimated_tokens": 37,
        "fused_candidates": 4,
        "graph_expanded": 0,
        "hits": [
            hit("FACT", 1, 0.009_667_573_537_629_684, 1.257_142_857_142_857_3e-6, None),
            hit("STATED", 2, 0.009_511_644_932_184_041, 1.113_924_050_632_911_3e-6, Some("user_stated")),
            hit("THIRD", 3, 0.008_892_633_119_137_143, 9.072_164_948_453_609e-7, Some("third_party")),
            hit("INFERRED", 4, 0.007_832_245_123_845_297, 8.301_886_792_452_831e-7, Some("agent_inferred"))
        ],
        "lexical_candidates": 4,
        "metrics": "timings are wall-clock",
        "pairs_reinforced": 6,
        "reranked": false,
        "warnings": []
    });
    // The backend-owned lexical score is compared at 12 significant digits,
    // because its last bits come from the host's SQLite build. Every other
    // value, including each hit's `score` and `scores.final`, is still compared
    // exactly below.
    assert_eq!(
        page_with_rounded_keyword(&page_without_timings(&value)),
        page_with_rounded_keyword(&expected),
        "the whole response"
    );
}

/// A rung outside the closed ladder is refused instead of ignored, so a mistyped
/// floor can never return a page the caller believes was narrowed. The four
/// documented names still work, so the refusal is about the value.
#[test]
fn a_mistyped_provenance_rung_is_refused_instead_of_ignored() {
    let store = fx::store();
    seed_rungs(&store);
    let retriever = fx::retriever(&store);

    for bad in ["trusted", "Stated", ""] {
        let result = retrieve_from_json(
            &retriever,
            &json!({"bank": "bank", "query": "alpha", "provenance_floor": bad}),
        );
        let value = serde_json::to_value(&result).expect("serialize");
        assert_eq!(value["ok"], false, "`{bad}` must not return a page");
        assert_eq!(value["code"], "invalid_argument", "`{bad}`: {value}");
        assert!(
            value.get("hits").is_none(),
            "a refused floor returns no page at all: {value}"
        );
        // The refusal names the argument the caller sent, not the profile field
        // the parser reads, and offers the four rungs it accepts.
        let message = value["message"].as_str().expect("a message").to_owned();
        assert!(
            message.starts_with("provenance_floor must be"),
            "the message must name the wire argument: {message}"
        );
        for rung in RUNGS {
            assert!(
                message.contains(rung),
                "the message must offer `{rung}`: {message}"
            );
        }
    }

    let ok = retrieve_from_json(
        &retriever,
        &json!({"bank": "bank", "query": "alpha", "provenance_floor": "stated"}),
    );
    let value = serde_json::to_value(&ok).expect("serialize");
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(ids(&value), vec!["FACT", "STATED"], "{value}");
}

/// The bank's default supplies the floor when the caller gave none, and only
/// then: the explicit-parameter-only surface has no floor to supply.
#[test]
fn the_bank_default_floor_applies_when_the_caller_gave_none() {
    let store = fx::store();
    seed_rungs(&store);
    let retriever = fx::retriever(&store);

    let plain = retrieve_from_json(&retriever, &json!({"bank": "bank", "query": "alpha"}));
    let value = serde_json::to_value(&plain).expect("serialize");
    assert_eq!(
        ids(&value),
        ALL_CLASSES,
        "the builtin default profile has no floor: {value}"
    );

    let banked = retrieve_from_json_with_policy(
        &retriever,
        &json!({"bank": "bank", "query": "alpha"}),
        Some(&StatedFloorBank),
    );
    let value = serde_json::to_value(&banked).expect("serialize");
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(
        ids(&value),
        vec!["FACT".to_owned(), "STATED".to_owned()],
        "the bank default narrows the page: {value}"
    );
}

/// The bank default supplies a value and changes nothing else: a named floor
/// wins over it, whether that floor is looser or stricter.
#[test]
fn an_explicit_floor_overrides_the_bank_default() {
    let store = fx::store();
    seed_rungs(&store);
    let retriever = fx::retriever(&store);

    let looser = retrieve_from_json_with_policy(
        &retriever,
        &json!({"bank": "bank", "query": "alpha", "provenance_floor": "inferred"}),
        Some(&StatedFloorBank),
    );
    let value = serde_json::to_value(&looser).expect("serialize");
    assert_eq!(
        ids(&value),
        ALL_CLASSES,
        "`inferred` is looser than the bank default and wins: {value}"
    );

    let stricter = retrieve_from_json_with_policy(
        &retriever,
        &json!({"bank": "bank", "query": "alpha", "provenance_floor": "verified"}),
        Some(&StatedFloorBank),
    );
    let value = serde_json::to_value(&stricter).expect("serialize");
    assert_eq!(
        ids(&value),
        vec!["FACT".to_owned()],
        "`verified` is stricter than the bank default and wins: {value}"
    );
}

/// `compose_context` takes its floor from the bank default for the memory
/// section's own retrieval, and gains no user-facing argument of its own.
#[test]
fn compose_context_honours_the_bank_default_floor() {
    let store = fx::store();
    seed_rungs(&store);
    let composer = Composer::new(Arc::new(fx::retriever(&store)));

    let plain = compose_from_json(
        &composer,
        &json!({"bank": "bank", "query": "alpha", "budget_tokens": 400}),
        None,
    );
    let value = serde_json::to_value(&plain).expect("serialize");
    assert_eq!(
        value["sections"][1]["item_ids"],
        json!(ALL_CLASSES),
        "no resolver means no floor: {value}"
    );

    let banked = compose_from_json_with_policy(
        &composer,
        &json!({"bank": "bank", "query": "alpha", "budget_tokens": 400}),
        None,
        Some(&StatedFloorBank),
    );
    let value = serde_json::to_value(&banked).expect("serialize");
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(
        value["sections"][1]["item_ids"],
        json!(["FACT", "STATED"]),
        "the memory section observes the bank default: {value}"
    );
}
