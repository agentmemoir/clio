// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the per-hit `entities[]` array through the `clio recall`
//! CLI surface: the JSON payload value, its parity with the MCP `retrieve`
//! payload for the same store, the empty-array shape, and the text view's
//! deliberate omission of entity names.
//!
//! ## Owns
//! - `recall --output json` `entities` assertions and CLI/MCP parity
//! - The text-view boundary assertion
//!
//! ## Does not own
//! - The entity projection and trim rules (`clio-retrieve`)
//! - The MCP payload boundary sweep (`clio-mcp` read-entity tests)
//! - The read-command runners and seed fixtures (shared from this parent module)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the CLI recall entity contract.

use super::*;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};

const BANK: &str = "ent-cli";

/// A name the gist and the lexical text never contain, so a name in the text
/// view could only have come from `entities[]`.
const ENTITY: &str = "CliEntityProbe42";

/// Store one admitted item whose verified snapshot entity differs from its gist.
fn seed_named(state: &McpState, id: &str) {
    let stored = clio_mcp::write_tools::dispatch(
        state,
        "store",
        &json!({
            "item": {
                "id": id,
                "subject_id": "s",
                "gist": "a note that mentions nothing by name",
                "snapshot": {"entity": ENTITY, "amount": 1, "date": "2024-01-01"}
            },
            "category": "task_spec",
            "epistemic_kind": "fact",
            "source_text": format!("the {ENTITY} note records amount 1 on 2024-01-01"),
            "bank": BANK,
            "actor": "agent",
            "timestamp": "2024-01-01T00:00:00Z"
        }),
    );
    assert_eq!(stored["ok"], json!(true), "seed {id}: {stored}");
    state
        .index
        .put_lexical_doc(BANK, id, "a note that mentions nothing by name")
        .expect("lexical doc");
}

/// Store one item with no snapshot at all, the documented empty-coverage case.
fn seed_snapshot_less(state: &McpState, id: &str) {
    let item = MemoryItem {
        id: id.into(),
        bank_id: BANK.into(),
        subject_id: "s".into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::Schema),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: Some("turn-1".into()),
        snapshot: None,
        gist: Some("a note that mentions nothing by name".into()),
        valid_from: "2024-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2024-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2024-01-01T00:00:00Z".into(),
        updated_at: "2024-01-01T00:00:00Z".into(),
        context: None,
    };
    state
        .store
        .create_memory_item(&item, "agent")
        .expect("create item");
    state
        .index
        .put_lexical_doc(BANK, id, "a note that mentions nothing by name")
        .expect("lexical doc");
}

fn state_with_items() -> Arc<McpState> {
    let state = McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state");
    seed_named(&state, "cli-named");
    seed_snapshot_less(&state, "cli-plain");
    state
}

fn entities_of(hit: &Value) -> Vec<String> {
    hit.get("entities")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("entities key present: {hit}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("entity names are strings: {hit}"))
                .to_owned()
        })
        .collect()
}

fn hit_by_id<'a>(payload: &'a Value, id: &str) -> &'a Value {
    payload["hits"]
        .as_array()
        .expect("hits array")
        .iter()
        .find(|h| h["item_id"] == json!(id))
        .unwrap_or_else(|| panic!("hit `{id}` present: {payload}"))
}

fn recall_json(state: &Arc<McpState>) -> Value {
    let (code, out, err) = run_state(
        "recall",
        &[
            "mentions", "--bank", BANK, "--limit", "5", "--output", "json",
        ],
        state,
        false,
    );
    assert_eq!(code, ExitCode::from(0), "recall exits ok: {err}");
    json_of(&out)
}

/// The CLI JSON payload exposes the same entity names as the MCP
/// payload for the same store, and the empty case is an empty array.
#[test]
fn recall_json_entities_match_the_mcp_retrieve_payload() {
    let state = state_with_items();
    let cli = recall_json(&state);
    let mcp = clio_mcp::write_tools::dispatch(
        &state,
        "retrieve",
        &json!({"bank": BANK, "query": "mentions", "limit": 5}),
    );
    assert_eq!(mcp["ok"], json!(true), "{mcp}");

    for id in ["cli-named", "cli-plain"] {
        assert_eq!(
            entities_of(hit_by_id(&cli, id)),
            entities_of(hit_by_id(&mcp, id)),
            "entity parity for `{id}`"
        );
    }
    assert_eq!(
        entities_of(hit_by_id(&cli, "cli-named")),
        vec![ENTITY.to_owned()],
        "{cli}"
    );
    assert_eq!(
        entities_of(hit_by_id(&cli, "cli-plain")),
        Vec::<String>::new(),
        "a snapshot-less item exposes an empty array: {cli}"
    );
}

/// every recall hit carries the `entities` key, so a
/// consumer reads one key without branching on absence.
#[test]
fn every_recall_hit_carries_the_entities_key() {
    let state = state_with_items();
    let cli = recall_json(&state);
    let hits = cli["hits"].as_array().expect("hits array");
    assert_eq!(hits.len(), 2, "{cli}");
    for hit in hits {
        assert!(hit.get("entities").is_some(), "entities key: {hit}");
    }
}

/// the human text view stays gist-only. Entity names
/// live in the JSON read payload; the text renderer never adds them, and the
/// derived entity-reason line names no entity either.
#[test]
fn the_recall_text_view_never_prints_entity_names() {
    let state = state_with_items();
    let (code, out, err) = run_state(
        "recall",
        &[
            "mentions", "--bank", BANK, "--limit", "5", "--output", "text",
        ],
        &state,
        false,
    );
    assert_eq!(code, ExitCode::from(0), "recall exits ok: {err}");
    assert!(
        out.contains("a note that mentions nothing by name"),
        "{out}"
    );
    assert!(
        !out.contains(ENTITY),
        "no entity name in the text view: {out}"
    );
}

/// The CLI JSON and CLI text views agree on the derived reason for one store:
/// a query that mentions the hit's entity name is marked in the JSON payload
/// and shown as the derived reason in the text view, while the name itself
/// stays out of the text.
#[test]
fn recall_marks_and_renders_the_derived_entity_reason() {
    let state = state_with_items();
    let query = format!("{ENTITY} mentions");

    let (code, out, err) = run_state(
        "recall",
        &[&query, "--bank", BANK, "--limit", "5", "--output", "json"],
        &state,
        false,
    );
    assert_eq!(code, ExitCode::from(0), "recall exits ok: {err}");
    let payload = json_of(&out);
    let named = hit_by_id(&payload, "cli-named");
    assert_eq!(named["entity_match"], json!(true), "{payload}");
    assert_eq!(
        entities_of(named),
        vec![ENTITY.to_owned()],
        "the name still rides in entities[]: {payload}"
    );
    assert!(
        hit_by_id(&payload, "cli-plain")
            .get("entity_match")
            .is_none(),
        "a hit with no name carries no flag: {payload}"
    );

    let (code, text, err) = run_state(
        "recall",
        &[&query, "--bank", BANK, "--limit", "5", "--output", "text"],
        &state,
        false,
    );
    assert_eq!(code, ExitCode::from(0), "text recall exits ok: {err}");
    let reason = text
        .lines()
        .find(|line| line.contains("[derived] entity match"))
        .expect("the derived reason is rendered");
    assert_eq!(
        reason, "     [derived] entity match (display-only; not a ranking signal)",
        "{text}"
    );
    // The name may appear in the echoed query line (the caller supplied it),
    // but the renderer must not add it anywhere else.
    for line in text
        .lines()
        .filter(|line| !line.starts_with("Results for:"))
    {
        assert!(
            !line.contains(ENTITY),
            "no entity name outside the query echo: {line}"
        );
    }
}
