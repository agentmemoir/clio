// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the gated Postgres end-to-end check that a
//! `retrieve` `min_scores.keyword` floor is applied on the live backend's own
//! lexical scale, so the backend dependence of that floor is observed against
//! the running store instead of only documented.
//!
//! ## Owns
//! - The `keyword` floor proof on live Postgres: a floor just above the weaker
//!   item's raw `ts_rank_cd` removes it while the stronger item stays
//! - The observed `ts_rank_cd` values printed for the log
//!
//! ## Does not own
//! - The floor rules and validation (`clio-retrieve` `floors`)
//! - The MCP payload parity and SQLite checks (`read_floors_tests` in-crate tests)
//!
//! ## Boundary
//!
//! Changes to the Postgres `keyword`-floor evidence should normally require
//! changes here. Changes to the floor rules or to SQLite-only binding checks
//! should not require changes here.
//!
//! # The same floor value means something different on SQLite
//!
//! `min_scores.keyword` compares against the lexical arm's raw backend score.
//! On Postgres that is `ts_rank_cd`, a small positive number; on SQLite it is
//! FTS5 `-bm25`, a negative number whose magnitude depends on document length.
//! The two scales are not comparable, so **no cross-backend parity is claimed**
//! for a `keyword` floor: the same numeric floor removes a different set of
//! items on each backend, and a cutoff tuned on one must be re-observed on the
//! other. This module proves the floor works against the live Postgres scale
//! only; it makes no claim about the SQLite scale.
//!
//! Keep this module focused on the live-backend keyword-floor evidence.

use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinct words the query and the two seeded documents share, so both items
/// match on the live backend and their `ts_rank_cd` values genuinely differ.
const QUERY: &str = "postgres keyword floor probe";

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn unique_id(prefix: &str) -> String {
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{sequence}", std::process::id())
}

/// The shared `CLIO_DATABASE_URL`, but only when it points at a Postgres
/// backend. Any other value (unset, SQLite) makes the caller return early.
fn postgres_url() -> Option<String> {
    let url = std::env::var("CLIO_DATABASE_URL").ok()?;
    (url.starts_with("postgres://") || url.starts_with("postgresql://")).then_some(url)
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// The store-tool argument shape used to seed one item.
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

/// The raw backend `ts_rank_cd` the lexical arm reads for one id, straight from
/// the same `clio_index::lexical_search` facade retrieval uses.
fn raw_ts_rank_cd(state: &McpState, bank: &str, id: &str) -> f64 {
    clio_index::lexical_search(
        state.index.as_ref(),
        &clio_index::LexicalSearchArgs {
            bank,
            query: QUERY,
            domains: vec![],
            limit: 10,
        },
    )
    .expect("lexical search")
    .iter()
    .find(|hit| hit.item_id == id)
    .unwrap_or_else(|| panic!("lexical hit for `{id}`"))
    .score
}

/// A `keyword` floor just above the weaker item's live `ts_rank_cd` removes that
/// item and keeps the stronger one; a floor of `0` keeps both. The printed
/// `ts_rank_cd` values make the backend's real scale visible in the test log.
#[test]
fn postgres_keyword_floor_applies_on_the_live_ts_rank_cd_scale() {
    let Some(database_url) = postgres_url() else {
        return;
    };
    let _guard = clio_store::lock_pg_test();
    // The shared database is store-global; run in a private schema so parallel
    // Postgres suites cannot see this test's rows and vice versa.
    let schema = clio_store::ScopedSchema::new(&database_url);
    let bank = unique_id("mcp-pg-floors-bank");
    let id_strong = unique_id("mcp-pg-floors-item-strong");
    let id_weak = unique_id("mcp-pg-floors-item-weak");
    // A document whose four query terms sit next to each other outranks one
    // where the same terms are spread apart: `ts_rank_cd` scores cover
    // density, so a contiguous match scores higher. The two raw values
    // therefore differ, and a floor between them is meaningful.
    let text_strong = "postgres keyword floor probe alpha";
    let text_weak = "postgres filler keyword filler floor filler probe filler alpha";

    // No embedder: the dense arm is off, so only the lexical arm scores the
    // hits and the `keyword` floor is the only floor in play.
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

    for (id, text) in [(&id_strong, text_strong), (&id_weak, text_weak)] {
        let gist = text.to_owned();
        let stored = call(&state, "store", store_args(&bank, id, &gist));
        assert_eq!(stored["ok"], json!(true), "{stored}");
        assert_eq!(stored["id"], json!(id));
        state
            .index
            .put_lexical_doc(&bank, id, text)
            .expect("register lexical doc");
    }

    // Read the live backend scale through the same facade retrieval uses, so the
    // floor value is derived from the real `ts_rank_cd` and not guessed.
    let strong_score = raw_ts_rank_cd(&state, &bank, &id_strong);
    let weak_score = raw_ts_rank_cd(&state, &bank, &id_weak);
    eprintln!(
        "postgres keyword floor: ts_rank_cd strong={strong_score} weak={weak_score} \
         (SQLite would report -bm25 instead, a negative scale, so the same floor \
         value means something different)"
    );
    assert!(
        strong_score > weak_score,
        "the seeded documents must produce different ts_rank_cd values: \
         {strong_score} vs {weak_score}"
    );

    // A floor of 0 keeps both, so the floor is not simply emptying the page.
    let unfloored = call(
        &state,
        "retrieve",
        json!({"bank": bank, "query": QUERY, "limit": 5, "min_scores": {"keyword": 0.0}}),
    );
    assert_eq!(unfloored["ok"], json!(true), "{unfloored}");
    let unfloored_ids = hit_ids(&unfloored);
    assert!(unfloored_ids.contains(&id_strong), "{unfloored_ids:?}");
    assert!(unfloored_ids.contains(&id_weak), "{unfloored_ids:?}");

    // A floor just above the weaker item's raw value drops exactly that item and
    // keeps the stronger one: the floor is applied on the live `ts_rank_cd`
    // scale, not on a normalized or rank-derived one.
    let floor = f64::midpoint(strong_score, weak_score);
    let floored = call(
        &state,
        "retrieve",
        json!({"bank": bank, "query": QUERY, "limit": 5, "min_scores": {"keyword": floor}}),
    );
    assert_eq!(floored["ok"], json!(true), "{floored}");
    let floored_ids = hit_ids(&floored);
    eprintln!("postgres keyword floor={floor} kept={floored_ids:?}");
    assert!(floored_ids.contains(&id_strong), "{floored_ids:?}");
    assert!(
        !floored_ids.contains(&id_weak),
        "the weaker item is below the floor: {floored_ids:?}"
    );
    assert_eq!(
        floored_ids.len() + 1,
        unfloored_ids.len(),
        "exactly the weaker item left: {floored_ids:?} from {unfloored_ids:?}"
    );
    assert_eq!(floored["lexical_candidates"], json!(1), "{floored}");

    for id in [&id_strong, &id_weak] {
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
