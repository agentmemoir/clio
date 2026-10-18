// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the gated Postgres end-to-end check of the MCP
//! `retrieve` `scores` payload, so the backend's real `ts_rank_cd` `keyword`
//! scale is observed against the live Postgres store instead of only documented.
//!
//! ## Owns
//! - The Postgres `retrieve` payload assertions over `final` / `keyword` /
//!   `reranker` / `semantic`
//! - The `scores.keyword` pass-through proof against `clio_index::lexical_search`
//!
//! ## Does not own
//! - The `scores` object contract or the dense transform (`clio-retrieve`)
//! - The SQLite no-embedder `scores` checks (`read_scores` in-crate tests)
//!
//! ## Boundary
//!
//! Changes to the Postgres `retrieve` `scores` payload should normally require
//! changes here. Changes to score computation or SQLite-only binding checks
//! should not require changes here.
//!
//! Keep this module focused on Postgres `scores` payload coverage.

use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::write_tools::dispatch;
use serde_json::{Map, Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

/// Relative tolerance for the `keyword` pass-through comparison. Both sides are
/// the same `f64` produced by the same `ts_rank_cd` SQL expression for the same
/// bank and query, so the observed delta should be exactly zero; the epsilon
/// only absorbs floating-point compare noise, never a real scale change.
const KEYWORD_EPSILON: f64 = 1e-12;

/// Distinct words the query and the seeded lexical documents share, so the
/// lexical arm matches both items on the live backend.
const QUERY: &str = "postgres lexical rank probe";

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn unique_id(prefix: &str) -> String {
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{sequence}", std::process::id())
}

/// The shared `CLIO_DATABASE_URL`, but only when it points at a Postgres
/// backend. A missing or non-Postgres value fails closed unless the operator
/// opted out, which prints a note and makes the caller return early.
fn postgres_url() -> Option<String> {
    clio_store::pg_skip::pg_url_or_skip("the mcp retrieve-scores Postgres test")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// The store-tool argument shape the drain hardening suite seeds with.
fn store_args(bank: &str, id: &str, gist: &str) -> Value {
    json!({
        "item": {
            "id": id,
            "subject_id": id,
            "gist": gist,
            "snapshot": {"entity": gist}
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": format!("{gist} (1 note since 2024-01-01)."),
        "bank": bank,
        "actor": "agent"
    })
}

fn hit_ids(payload: &Value) -> Vec<String> {
    payload["hits"]
        .as_array()
        .map(|hits| {
            hits.iter()
                .filter_map(|hit| hit["item_id"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn hit_by_id<'a>(payload: &'a Value, id: &str) -> &'a Value {
    payload["hits"]
        .as_array()
        .and_then(|hits| hits.iter().find(|hit| hit["item_id"] == json!(id)))
        .unwrap_or_else(|| panic!("hit `{id}` present: {payload}"))
}

/// Exactly the documented `scores` key set, returned for value assertions.
fn scores_of(hit: &Value) -> &Map<String, Value> {
    let scores = hit["scores"]
        .as_object()
        .unwrap_or_else(|| panic!("scores object on every hit: {hit}"));
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
    scores
}

/// The raw backend `ts_rank_cd` the lexical leg reads for one id, straight from
/// the same `clio_index::lexical_search` facade retrieval uses.
fn lexical_keyword(state: &McpState, bank: &str, id: &str) -> f64 {
    let hits = clio_index::lexical_search(
        state.index.as_ref(),
        &clio_index::LexicalSearchArgs {
            bank,
            query: QUERY,
            domains: vec![],
            limit: 5,
        },
    )
    .expect("lexical search");
    hits.iter()
        .find(|hit| hit.item_id == id)
        .expect("lexical hit for seeded item")
        .score
}

#[test]
fn postgres_retrieve_scores_observes_ts_rank_cd_keyword_end_to_end() {
    let Some(database_url) = postgres_url() else {
        return;
    };
    let _guard = clio_store::lock_pg_test();
    // The shared database is store-global; run in a private schema so parallel
    // Postgres suites cannot see this test's rows and vice versa.
    let schema = clio_store::ScopedSchema::new(&database_url);
    let bank = unique_id("mcp-pg-scores-bank");
    let id_a = unique_id("mcp-pg-scores-item-a");
    let id_b = unique_id("mcp-pg-scores-item-b");
    let text_a = "postgres lexical rank probe alpha";
    let text_b = "postgres lexical rank probe beta";

    // No embedder: the dense arm is off, so `semantic` stays null and only the
    // lexical arm scores the hits.
    let state = McpState::open_with_effective(
        &McpOpenOptions {
            backend: "postgres".into(),
            database_url: schema.url().to_owned(),
            default_bank: bank.clone(),
            ..McpOpenOptions::default()
        },
        &json!({}),
    )
    .expect("open Postgres state");

    for (id, gist, text) in [
        (&id_a, "postgres lexical rank probe alpha", text_a),
        (&id_b, "postgres lexical rank probe beta", text_b),
    ] {
        let stored = call(&state, "store", store_args(&bank, id, gist));
        assert_eq!(stored["ok"], json!(true), "{stored}");
        assert_eq!(stored["id"], json!(id));
        state
            .index
            .put_lexical_doc(&bank, id, text)
            .expect("register lexical doc");
    }

    let retrieved = call(
        &state,
        "retrieve",
        json!({"bank": bank, "query": QUERY, "limit": 5}),
    );
    assert_eq!(retrieved["ok"], json!(true), "{retrieved}");
    let ids = hit_ids(&retrieved);
    assert!(ids.contains(&id_a), "seeded item A present: {retrieved}");
    assert!(ids.contains(&id_b), "seeded item B present: {retrieved}");

    for id in [&id_a, &id_b] {
        let hit = hit_by_id(&retrieved, id);
        let scores = scores_of(hit);

        let keyword = scores["keyword"]
            .as_f64()
            .unwrap_or_else(|| panic!("numeric keyword on `{id}`: {hit}"));
        assert!(
            keyword >= 0.0,
            "Postgres ts_rank_cd is non-negative on `{id}`: {hit}"
        );
        let pass_through = lexical_keyword(&state, &bank, id);
        assert!(
            (keyword - pass_through).abs() < KEYWORD_EPSILON,
            "keyword must pass through lexical_search on `{id}`: \
             payload {keyword}, backend {pass_through}"
        );

        // Observability for the live backend: the observed `ts_rank_cd` value
        // against the raw facade value it must pass through.
        eprintln!("postgres scores `{id}`: keyword={keyword} backend={pass_through}");

        assert_eq!(
            scores["semantic"],
            json!(null),
            "no embedder on `{id}`: {hit}"
        );
        assert_eq!(
            scores["reranker"],
            json!(null),
            "rerank inert on `{id}`: {hit}"
        );
        assert_eq!(
            scores["final"], hit["score"],
            "final mirrors score on `{id}`: {hit}"
        );

        let rendered = serde_json::to_string(scores).expect("serialize scores");
        for (key, value) in scores {
            assert!(
                value.is_number() || value.is_null(),
                "`{key}` must be a number or null on `{id}`: {hit}"
            );
        }
        assert!(
            !rendered.contains(QUERY),
            "no query text in scores on `{id}`: {rendered}"
        );
        for word in ["postgres", "alpha", "beta"] {
            assert!(
                !rendered.contains(word),
                "no content text in scores on `{id}`: {rendered}"
            );
        }
    }

    for id in [&id_a, &id_b] {
        assert!(
            state
                .store
                .delete_item(&bank, id)
                .expect("remove test item"),
            "seeded item `{id}` removed"
        );
    }
    drop(state);
}
