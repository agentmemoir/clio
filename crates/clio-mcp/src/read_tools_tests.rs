// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Read-surface dispatcher tests: item primitives, persona, inspect, gate,
//! retrieve/explain, compose budgets, associations/graph, and history reads.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

#[allow(clippy::needless_pass_by_value)]
fn call(state: &McpState, name: &str, args: Value) -> Value {
    dispatch(state, name, &args)
}

/// Store one admitted item and register its lexical doc so retrieval can hit it.
fn store_and_index(state: &McpState, bank: &str, id: &str, category: &str, text: &str) {
    let args = json!({
        "item": {
            "id": id,
            "subject_id": "subj",
            "gist": text,
            "snapshot": {"entity": text, "amount": 1, "date": "2024-01-01"}
        },
        "category": category,
        "epistemic_kind": "fact",
        "source_text": format!("The user prefers {text} (1 note since 2024-01-01)."),
        "bank": bank,
        "actor": "agent",
        "timestamp": "2024-01-01T00:00:00Z"
    });
    let out = call(state, "store", args);
    assert_eq!(out["ok"], json!(true), "store {id}: {out}");
    state
        .index
        .put_lexical_doc(bank, id, text)
        .expect("lexical doc");
}

#[test]
fn item_reads_roundtrip_and_not_found() {
    let s = state();
    store_and_index(&s, "bank-a", "itm-1", "task_spec", "terse answers");
    let get = call(&s, "get", json!({"bank": "bank-a", "item_id": "itm-1"}));
    assert_eq!(get["ok"], json!(true), "{get}");
    assert_eq!(get["id"], json!("itm-1"));
    assert_eq!(get["category"], json!("task_spec"));
    assert_eq!(get["epistemic_kind"], json!("fact"));

    let snap = call(
        &s,
        "get_snapshot",
        json!({"bank": "bank-a", "item_id": "itm-1"}),
    );
    assert_eq!(snap["authoritative"], json!(true));
    assert_eq!(snap["snapshot"]["amount"], json!(1));

    let gist = call(
        &s,
        "get_gist",
        json!({"bank": "bank-a", "item_id": "itm-1"}),
    );
    assert_eq!(gist["authoritative"], json!(false));
    assert_eq!(gist["gist"], json!("terse answers"));

    let missing = call(&s, "get", json!({"bank": "bank-a", "item_id": "ghost"}));
    assert_eq!(missing["ok"], json!(false));
    assert_eq!(missing["code"], json!("item_not_found"));
    assert_eq!(
        call(
            &s,
            "get_snapshot",
            json!({"bank": "bank-a", "item_id": "ghost"})
        )["code"],
        json!("item_not_found")
    );
    assert_eq!(
        call(
            &s,
            "get_gist",
            json!({"bank": "bank-a", "item_id": "ghost"})
        )["code"],
        json!("item_not_found")
    );
}

#[test]
fn maintenance_status_and_persona_get() {
    let s = state();
    store_and_index(&s, "bank-a", "itm-1", "task_spec", "terse answers");
    let status = call(
        &s,
        "maintenance_status",
        json!({"bank": "bank-a", "item_id": "itm-1"}),
    );
    assert_eq!(status["ok"], json!(true), "{status}");
    assert_eq!(status["leaf_queryable"], json!(true));

    let put = call(
        &s,
        "persona_put_stable",
        json!({"bank": "bank-a", "key": "style", "value": "concise", "timestamp": "2024-01-01T00:00:00Z"}),
    );
    assert_eq!(put["ok"], json!(true), "{put}");
    let persona = call(&s, "persona_get", json!({"bank": "bank-a"}));
    assert_eq!(persona["ok"], json!(true), "{persona}");
    assert_eq!(persona["document"]["stable"][0]["key"], json!("style"));
}

#[test]
fn inspect_filters_pagination_and_discard_visibility() {
    let s = state();
    // Distinct texts per id: a shared one is an exact duplicate, and the
    // unconditional write-side rule refuses the second store.
    store_and_index(&s, "bank-a", "itm-1", "task_spec", "alpha beta itm-1");
    store_and_index(&s, "bank-a", "itm-2", "persona", "alpha beta itm-2");
    store_and_index(&s, "bank-b", "itm-3", "task_spec", "alpha beta itm-3");

    let all = call(&s, "inspect", json!({"bank": "bank-a"}));
    assert_eq!(all["ok"], json!(true), "{all}");
    assert_eq!(all["items"].as_array().expect("items").len(), 2);
    assert_eq!(all["limit"], json!(50));
    assert_eq!(all["total"], json!(2));

    let filtered = call(
        &s,
        "inspect",
        json!({"bank": "bank-a", "filter": {"category": "persona"}}),
    );
    assert_eq!(filtered["items"].as_array().expect("items").len(), 1);
    assert_eq!(filtered["items"][0]["item_id"], json!("itm-2"));

    let by_kind = call(
        &s,
        "inspect",
        json!({"bank": "bank-a", "filter": {"epistemic_kind": "belief"}}),
    );
    let belief_items = by_kind["items"].as_array().expect("items");
    assert!(belief_items.is_empty(), "{belief_items:?}");

    let paged = call(
        &s,
        "inspect",
        json!({"bank": "bank-a", "limit": 1, "offset": 1}),
    );
    let page = paged["items"].as_array().expect("items");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0]["item_id"], json!("itm-2"));
    assert_eq!(paged["total"], json!(2));

    // Discard itm-1, then confirm default listing hides it and the flag shows it.
    call(
        &s,
        "discard",
        json!({"bank": "bank-a", "item_id": "itm-1", "reason": "ops", "confirm": true, "timestamp": "2024-02-01T00:00:00Z"}),
    );
    let hidden = call(&s, "inspect", json!({"bank": "bank-a"}));
    assert_eq!(hidden["items"].as_array().expect("items").len(), 1);
    let shown = call(
        &s,
        "inspect",
        json!({"bank": "bank-a", "filter": {"include_discarded": true}}),
    );
    assert_eq!(shown["items"].as_array().expect("items").len(), 2);

    // Invalid validity label is a structured invalid_argument.
    let bad = call(
        &s,
        "inspect",
        json!({"bank": "bank-a", "filter": {"validity": "maybe"}}),
    );
    assert_eq!(bad["code"], json!("invalid_argument"));
}

#[test]
fn intent_gate_and_explicit_retrieve_with_explain() {
    let s = state();
    // Distinct texts per id: a shared one is an exact duplicate, and the
    // unconditional write-side rule refuses the second store.
    store_and_index(&s, "bank-a", "itm-1", "task_spec", "terse answers itm-1");
    store_and_index(&s, "bank-a", "itm-2", "persona", "terse answers itm-2");

    let gate = call(&s, "intent_gate", json!({"turn_text": "hi"}));
    assert_eq!(gate["retrieval_needed"], json!(false), "{gate}");

    // Explicit retrieve runs even though the gate would skip.
    let out = call(
        &s,
        "retrieve",
        json!({"bank": "bank-a", "query": "terse answers", "explain": true}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let hits = out["hits"].as_array().expect("hits");
    assert!(hits.len() >= 2, "expected both indexed items: {out}");
    assert_eq!(hits[0]["epistemic_kind"], json!("fact"));
    let explanation = &out["explanation"];
    assert_eq!(explanation["domains"], json!([]));
    assert!(explanation["hits"].as_array().expect("trace hits").len() >= 2);
    assert!(explanation["pairs_reinforced"].as_u64().is_some());
}

#[test]
fn retrieve_reinforces_assoc_edges_durably() {
    let s = state();
    // Distinct texts per id: a shared one is an exact duplicate, and the
    // unconditional write-side rule refuses the second store.
    store_and_index(&s, "bank-a", "itm-1", "task_spec", "terse answers itm-1");
    store_and_index(&s, "bank-a", "itm-2", "persona", "terse answers itm-2");
    let out = call(
        &s,
        "retrieve",
        json!({"bank": "bank-a", "query": "terse answers"}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    let edges = s.store.list_assoc_edges("bank-a", None).expect("edges");
    assert!(
        !edges.is_empty(),
        "co-activation edges must be persisted after retrieve"
    );
}

#[test]
fn compose_context_respects_budget_and_prioritizes_persona() {
    let s = state();
    for i in 0..10 {
        let put = call(
            &s,
            "persona_put_stable",
            json!({"bank": "bank-a", "key": format!("k{i}"), "value": "value-value-value", "timestamp": "2024-01-01T00:00:00Z"}),
        );
        assert_eq!(put["ok"], json!(true), "{put}");
    }
    store_and_index(&s, "bank-a", "itm-1", "task_spec", "terse answers");
    let out = call(
        &s,
        "compose_context",
        json!({"bank": "bank-a", "query": "terse answers", "budget_tokens": 300, "explain": true}),
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert!(
        out["total_tokens"].as_u64().unwrap_or(u64::MAX) <= 300,
        "budget violated: {out}"
    );
    let persona = out["sections"]
        .as_array()
        .expect("sections")
        .iter()
        .find(|s| s["name"] == json!("persona"))
        .expect("persona section");
    let persona_text = persona["text"].as_str().unwrap_or_default();
    assert!(!persona_text.is_empty(), "{persona_text:?}");
    assert!(out["explanation"]["persona_budget_tokens"].is_number());
}

#[test]
fn associations_and_graph_query_with_hop_cap() {
    let s = state();
    store_and_index(&s, "bank-a", "itm-1", "task_spec", "alpha");
    store_and_index(&s, "bank-a", "itm-2", "persona", "beta");
    let link = call(
        &s,
        "graph_link",
        json!({"bank": "bank-a", "source_id": "itm-1", "target_id": "itm-2", "relationship": "relates_to"}),
    );
    assert_eq!(link["ok"], json!(true), "{link}");

    let assoc = call(
        &s,
        "associations",
        json!({"bank": "bank-a", "item_id": "itm-1"}),
    );
    assert_eq!(assoc["ok"], json!(true), "{assoc}");
    let listed = assoc["associations"].as_array().expect("assoc");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["neighbor_id"], json!("itm-2"));

    let graph = call(
        &s,
        "graph_query",
        json!({"bank": "bank-a", "seed_id": "itm-1", "max_hops": 2}),
    );
    assert_eq!(graph["nodes"].as_array().expect("graph").len(), 1);

    let too_deep = call(
        &s,
        "graph_query",
        json!({"bank": "bank-a", "seed_id": "itm-1", "max_hops": 9}),
    );
    assert_eq!(too_deep["ok"], json!(false));
    assert_eq!(too_deep["code"], json!("out_of_range"));
}

#[test]
fn history_triple_and_belief_reads() {
    let s = state();
    let ts = "2024-01-01T00:00:00Z";
    assert_eq!(
        call(
            &s,
            "task_upsert",
            json!({"bank": "bank-a", "task_id": "t1", "definition": "Run tests", "status": "running", "timestamp": ts})
        )["ok"],
        json!(true)
    );
    assert_eq!(
        call(
            &s,
            "failure_record",
            json!({"bank": "bank-a", "task_id": "t1", "attempt_n": 1, "what_failed": "flake", "lesson": "retry once", "timestamp": ts})
        )["ok"],
        json!(true)
    );
    let task = call(&s, "task_get", json!({"bank": "bank-a", "task_id": "t1"}));
    assert_eq!(task["record"]["task_id"], json!("t1"));
    let hist = call(
        &s,
        "task_history",
        json!({"bank": "bank-a", "task_id": "t1"}),
    );
    assert_eq!(hist["records"].as_array().expect("records").len(), 1);
    let fails = call(
        &s,
        "failures_for_task",
        json!({"bank": "bank-a", "task_id_or_query": "t1"}),
    );
    assert_eq!(fails["mode"], json!("exact"));
    assert_eq!(fails["failures"].as_array().expect("failures").len(), 1);

    assert_eq!(
        call(
            &s,
            "triple_add",
            json!({"bank": "bank-a", "subject": "rust", "predicate": "used_by", "object": "agent", "epistemic_kind": "fact", "timestamp": ts})
        )["ok"],
        json!(true)
    );
    let triples = call(
        &s,
        "triple_query",
        json!({"bank": "bank-a", "subject": "rust"}),
    );
    assert_eq!(triples["triples"].as_array().expect("triples").len(), 1);
    let temporal = call(
        &s,
        "temporal_history",
        json!({"bank": "bank-a", "target": "triple:rust:used_by"}),
    );
    assert_eq!(temporal["target_kind"], json!("triple"));
    assert_eq!(temporal["entries"].as_array().expect("entries").len(), 1);

    assert_eq!(
        call(
            &s,
            "belief_observe",
            json!({"bank": "bank-a", "proposition": "Friday deploys are risky", "confidence": 0.7, "evidence_ref": "ev-1", "source_type": "user_stated", "timestamp": ts})
        )["ok"],
        json!(true)
    );
    let belief = call(
        &s,
        "belief_history",
        json!({"bank": "bank-a", "belief_id_or_proposition": "Friday deploys are risky"}),
    );
    assert_eq!(belief["ok"], json!(true), "{belief}");
    let entries = belief["entries"].as_array().expect("entries");
    assert!(!entries.is_empty(), "{entries:?}");
    let belief_temporal = call(
        &s,
        "temporal_history",
        json!({"bank": "bank-a", "target": "belief:Friday deploys are risky"}),
    );
    assert_eq!(belief_temporal["target_kind"], json!("belief"));

    let unknown = call(
        &s,
        "temporal_history",
        json!({"bank": "bank-a", "target": "bogus:thing"}),
    );
    assert_eq!(unknown["code"], json!("invalid_argument"));
}

#[test]
fn memtree_query_and_get() {
    let s = state();
    store_and_index(&s, "bank-a", "itm-1", "task_spec", "terse answers");
    let nodes = call(&s, "memtree_query", json!({"bank": "bank-a"}));
    assert_eq!(nodes["ok"], json!(true), "{nodes}");
    let list = nodes["nodes"].as_array().expect("node list");
    assert!(!list.is_empty(), "leaf attach must create a MemTree node");
    let node_id = list[0]["id"].as_str().expect("node id").to_owned();
    let one = call(
        &s,
        "memtree_get",
        json!({"bank": "bank-a", "node_id": node_id}),
    );
    assert_eq!(one["ok"], json!(true), "{one}");
    let missing = call(
        &s,
        "memtree_get",
        json!({"bank": "bank-a", "node_id": "ghost"}),
    );
    assert_eq!(missing["ok"], json!(false));
    let ranged = call(
        &s,
        "memtree_query",
        json!({"bank": "bank-a", "time_range": {"start": "2020-01-01", "end": "2030-01-01"}, "depth": 0, "limit": 5}),
    );
    assert_eq!(ranged["ok"], json!(true), "{ranged}");
    let too_deep = call(&s, "memtree_query", json!({"bank": "bank-a", "depth": 99}));
    assert_eq!(too_deep["code"], json!("out_of_range"));
}

#[test]
fn maintenance_status_for_an_unknown_item_is_item_not_found() {
    let s = state();
    let out = call(&s, "maintenance_status", json!({"item_id": "itm-nope"}));
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(out["code"], json!("item_not_found"), "{out}");
}
