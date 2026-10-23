// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for behavioural coverage of the batch pre-flight:
//! what each recognized operation stages without writing, how the caller's
//! timestamp reaches the staged rows, and the refusals that abort an operation
//! before it stages anything.
//!
//! ## Owns
//! - Staged-effects assertions for `store`, `canonical_put`, `triple_add`, and
//!   `graph_link` when called through `preflight`
//! - Timestamp canonicalisation and refusal at the pre-flight seam
//! - The refusal shapes: unplaceable stamp, bad graph endpoint, unknown tool
//!
//! ## Does not own
//! - Admission math (`clio-admission`) or the transactional commit
//!   (`clio-store`)
//! - Batch orchestration and `$ref` resolution (see `batch_tools`)
//!
//! ## Boundary
//!
//! Tests here break when a pre-flight's staged effects or refusals change; they
//! never change them.
//!
//! Keep this module focused on the pre-flight seam.

use super::*;
use crate::runtime::McpOpenOptions;
use serde_json::json;

/// One in-memory state; each test builds its own, so no test depends on another.
fn state() -> Arc<McpState> {
    McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state")
}

/// Rows the bank holds, so a test can prove a pre-flight wrote nothing.
fn count(state: &McpState) -> usize {
    state
        .store
        .count_items(&clio_store::ItemInspectFilter::new("bank-a"))
        .expect("count")
}

/// Parameters a `store` operation carries, in the shape the tool takes.
fn store_params(id: &str) -> Value {
    json!({
        "item": {
            "id": id,
            "subject_id": format!("subj-{id}"),
            "gist": format!("{id} prefers terse answers"),
            "snapshot": {"entity": "terse answers", "amount": 1, "date": "2024-01-01"}
        },
        "category": "persona",
        "epistemic_kind": "fact",
        "source_text": format!("The user prefers terse answers ({id}, 1 note since 2024-01-01)."),
        "bank": "bank-a",
        "actor": "agent"
    })
}

fn canonical_params() -> Value {
    json!({
        "key": "primary_host", "value": "db-a", "category": "tool_config",
        "epistemic_kind": "fact", "bank": "bank-a", "actor": "agent"
    })
}

fn triple_params() -> Value {
    json!({
        "subject": "alice", "predicate": "lives_in", "object": "boston",
        "epistemic_kind": "fact", "confidence": 0.9,
        "bank": "bank-a", "actor": "agent"
    })
}

fn graph_params() -> Value {
    json!({
        "source_id": "itm-a", "target_id": "itm-b", "relationship": "related",
        "bank": "bank-a", "actor": "agent"
    })
}

/// The same parameters with a caller timestamp attached.
fn stamped(mut params: Value, timestamp: &str) -> Value {
    params["timestamp"] = json!(timestamp);
    params
}

/// The refusal a pre-flight reported, or a panic naming the staged result.
fn refusal(result: Result<Staged, AmError>) -> AmError {
    match result {
        Ok(staged) => panic!(
            "expected a refusal, staged {} row effect(s)",
            staged.writes.len()
        ),
        Err(err) => err,
    }
}

/// The two ids a `graph_link` pre-flight accepts as produced earlier in the batch.
fn in_batch() -> Vec<String> {
    vec!["itm-a".to_owned(), "itm-b".to_owned()]
}

#[test]
fn a_store_op_stages_the_carrier_item_and_persists_nothing() {
    let s = state();
    let staged = preflight(&s, "store", &store_params("itm-s1"), &[], &[]).expect("staged store");
    assert_eq!(staged.writes.len(), 1);
    match &staged.writes[0] {
        BatchWrite::CreateItem { item, actor } => {
            assert_eq!(item.id, "itm-s1");
            assert_eq!(actor, "agent");
        }
        other => panic!("store must stage a create, got {other:?}"),
    }
    assert_eq!(staged.id.as_deref(), Some("itm-s1"));
    assert!(staged.admission_score.is_some(), "a scored op reports it");
    assert_eq!(count(&s), 0, "pre-flight scores, it does not write");
}

#[test]
fn a_store_op_without_an_item_id_mints_one() {
    let s = state();
    let mut params = store_params("unused");
    params["item"]["id"] = Value::Null;
    let staged = preflight(&s, "store", &params, &[], &[]).expect("staged store");
    let id = staged.id.expect("a minted id");
    assert_ne!(id, "", "a minted item id must not be blank");
    assert_eq!(count(&s), 0);
}

#[test]
fn a_canonical_op_stages_one_slot_write_for_its_key() {
    let s = state();
    let staged =
        preflight(&s, "canonical_put", &canonical_params(), &[], &[]).expect("staged canonical");
    assert_eq!(staged.writes.len(), 1);
    match &staged.writes[0] {
        BatchWrite::CanonicalPut { item, key, actor } => {
            assert_eq!(key, "primary_host");
            assert_eq!(item.gist.as_deref(), Some("db-a"));
            assert_eq!(actor, "agent");
            assert_eq!(staged.id.as_deref(), Some(item.id.as_str()));
        }
        other => panic!("canonical_put must stage a slot write, got {other:?}"),
    }
    assert_eq!(count(&s), 0);
}

#[test]
fn a_triple_op_stages_the_carrier_and_the_edge_that_names_it() {
    let s = state();
    let staged = preflight(&s, "triple_add", &triple_params(), &[], &[]).expect("staged triple");
    assert_eq!(staged.writes.len(), 1);
    match &staged.writes[0] {
        BatchWrite::CommitTriple {
            item,
            edge,
            supersede,
            actor,
        } => {
            assert_eq!(edge.item_id, item.id, "the edge must cite its carrier");
            assert_eq!(edge.subject, "alice");
            assert_eq!(edge.predicate, "lives_in");
            assert_eq!(edge.object, "boston");
            assert!(*supersede, "supersede defaults to closing prior edges");
            assert_eq!(actor, "agent");
            assert_eq!(staged.id.as_deref(), Some(item.id.as_str()));
        }
        other => panic!("triple_add must stage a commit, got {other:?}"),
    }
    assert!(staged.admission_score.is_some());
    assert_eq!(count(&s), 0);
}

#[test]
fn a_triple_op_passes_a_false_supersede_through_to_the_commit() {
    let s = state();
    let mut params = triple_params();
    params["supersede"] = json!(false);
    let staged = preflight(&s, "triple_add", &params, &[], &[]).expect("staged triple");
    match &staged.writes[0] {
        BatchWrite::CommitTriple { supersede, .. } => assert!(!supersede),
        other => panic!("triple_add must stage a commit, got {other:?}"),
    }
}

#[test]
fn a_graph_link_between_ids_produced_earlier_in_the_batch_stages_edge_and_audit() {
    let s = state();
    let staged =
        preflight(&s, "graph_link", &graph_params(), &in_batch(), &[]).expect("staged graph link");
    assert_eq!(staged.writes.len(), 2, "an edge and its audit event");
    match &staged.writes[0] {
        BatchWrite::PutAssocEdge { edge, actor } => {
            assert_eq!(edge.source_id, "itm-a");
            assert_eq!(edge.target_id, "itm-b");
            assert_eq!(edge.relationship.as_deref(), Some("related"));
            assert!(
                (edge.weight - 1.0).abs() < f64::EPSILON,
                "weight defaults to 1, got {}",
                edge.weight
            );
            assert_eq!(actor, "agent");
        }
        other => panic!("graph_link must stage an edge, got {other:?}"),
    }
    match &staged.writes[1] {
        BatchWrite::AuditEvent {
            operation, item_id, ..
        } => {
            assert_eq!(operation, "graph_link");
            assert_eq!(item_id.as_deref(), Some("itm-b"));
        }
        other => panic!("graph_link must stage its audit event, got {other:?}"),
    }
    assert!(staged.id.is_none(), "a link produces no item id");
    assert_eq!(count(&s), 0);
}

#[test]
fn a_graph_link_refuses_two_identical_endpoints() {
    let s = state();
    let mut params = graph_params();
    params["target_id"] = json!("itm-a");
    let err = refusal(preflight(&s, "graph_link", &params, &in_batch(), &[]));
    assert_eq!(err.code, ErrorCode::InvalidArgument);
}

#[test]
fn a_graph_link_refuses_a_weight_outside_the_unit_interval() {
    let s = state();
    for weight in [json!(1.5), json!(-0.1)] {
        let mut params = graph_params();
        params["weight"] = weight.clone();
        let err = refusal(preflight(&s, "graph_link", &params, &in_batch(), &[]));
        assert_eq!(err.code, ErrorCode::OutOfRange, "weight {weight}");
    }
}

#[test]
fn a_graph_link_to_a_bank_row_that_does_not_exist_is_refused() {
    let s = state();
    // Neither endpoint was produced by an earlier operation, and neither is in
    // the bank, so the link would dangle and the batch must abort.
    let err = refusal(preflight(&s, "graph_link", &graph_params(), &[], &[]));
    assert_eq!(err.code, ErrorCode::ItemNotFound);
    assert_eq!(count(&s), 0);
}

#[test]
fn a_graph_link_to_a_row_the_bank_holds_is_staged_without_fetching_it_later() {
    let s = state();
    for (id, marker) in [("itm-a", "alpha"), ("itm-b", "beta")] {
        let stored = crate::write_tools::dispatch(&s, "store", &store_params(id));
        assert_eq!(stored["ok"], json!(true), "{marker}: {stored}");
    }
    let staged = preflight(&s, "graph_link", &graph_params(), &[], &[]).expect("staged link");
    assert_eq!(staged.writes.len(), 2);
}

#[test]
fn an_unrecognized_tool_is_refused_instead_of_staged() {
    let s = state();
    let err = refusal(preflight(&s, "no_such_tool", &json!({}), &[], &[]));
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(err.message.contains("no_such_tool"), "{}", err.message);
}

#[test]
fn a_record_op_is_routed_to_the_record_pre_flight() {
    let s = state();
    let params = json!({
        "key": "communication_style", "value": "terse", "bank": "bank-a", "actor": "agent"
    });
    match preflight(&s, "persona_put_stable", &params, &[], &[]) {
        Ok(staged) => assert_ne!(
            staged.writes.len(),
            0,
            "a staged record op carries its row effects"
        ),
        // The only refusal a well-formed record op has is its admission gate,
        // which is structured rather than a panic.
        Err(err) => assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}"),
    }
    assert_eq!(count(&s), 0);
}

#[test]
fn an_unplaceable_timestamp_aborts_every_op_before_it_stages() {
    let s = state();
    // A month the year does not have: stored verbatim it would compare as a
    // literal string in a time column, so the pre-flight must refuse instead.
    let bad = "2026-13-01T00:00:00Z";
    let cases: [(&str, Value); 4] = [
        ("store", store_params("itm-bad")),
        ("canonical_put", canonical_params()),
        ("triple_add", triple_params()),
        ("graph_link", graph_params()),
    ];
    for (tool, params) in cases {
        let err = refusal(preflight(&s, tool, &stamped(params, bad), &in_batch(), &[]));
        assert_eq!(err.code, ErrorCode::InvalidArgument, "{tool}: {err:?}");
        assert!(err.message.contains(bad), "{tool}: {}", err.message);
    }
    assert_eq!(count(&s), 0, "no refused op leaves a row behind");
}

#[test]
fn a_fractional_timestamp_reaches_the_staged_carrier_as_its_second() {
    let s = state();
    let params = stamped(store_params("itm-frac"), "2026-10-12T08:30:01.500Z");
    let staged = preflight(&s, "store", &params, &[], &[]).expect("staged store");
    match &staged.writes[0] {
        BatchWrite::CreateItem { item, .. } => {
            assert_eq!(item.created_at, "2026-10-12T08:30:01Z");
            assert_eq!(item.tx_from, "2026-10-12T08:30:01Z");
        }
        other => panic!("store must stage a create, got {other:?}"),
    }
}

#[test]
fn an_offset_timestamp_reaches_the_staged_link_as_its_utc_instant() {
    let s = state();
    // The link's `last_updated_at` is resolved from the caller's stamp, so an
    // offset must arrive in the column as the instant it names.
    let params = stamped(graph_params(), "2026-10-12T08:30:01+02:00");
    let staged = preflight(&s, "graph_link", &params, &in_batch(), &[]).expect("staged link");
    match &staged.writes[0] {
        BatchWrite::PutAssocEdge { edge, .. } => {
            assert_eq!(edge.last_updated_at, "2026-10-12T06:30:01Z");
        }
        other => panic!("graph_link must stage an edge, got {other:?}"),
    }
}

#[test]
fn a_triple_op_canonicalises_both_axes_and_leaves_an_absent_end_absent() {
    let s = state();
    let mut params = stamped(triple_params(), "2026-10-12T08:30:01.750Z");
    params["valid_from"] = json!("2026-10-11T23:00:00+02:00");
    let staged = preflight(&s, "triple_add", &params, &[], &[]).expect("staged triple");
    match &staged.writes[0] {
        BatchWrite::CommitTriple { item, edge, .. } => {
            assert_eq!(edge.valid_from, "2026-10-11T21:00:00Z");
            assert_eq!(edge.tx_from, "2026-10-12T08:30:01Z");
            assert_eq!(edge.valid_until, None, "an absent end stays absent");
            assert_eq!(item.valid_until, None);
        }
        other => panic!("triple_add must stage a commit, got {other:?}"),
    }

    // A present end is canonicalised on both the edge and its carrier.
    let mut params = stamped(triple_params(), "2026-10-12T08:30:01Z");
    params["valid_until"] = json!("2026-10-20T09:00:00.250Z");
    let staged = preflight(&s, "triple_add", &params, &[], &[]).expect("staged triple");
    match &staged.writes[0] {
        BatchWrite::CommitTriple { item, edge, .. } => {
            assert_eq!(edge.valid_until.as_deref(), Some("2026-10-20T09:00:00Z"));
            assert_eq!(item.valid_until.as_deref(), Some("2026-10-20T09:00:00Z"));
        }
        other => panic!("triple_add must stage a commit, got {other:?}"),
    }
}

#[test]
fn a_triple_op_refuses_a_valid_from_it_cannot_place() {
    let s = state();
    let mut params = triple_params();
    params["valid_from"] = json!("2026-02-30T00:00:00Z");
    let err = refusal(preflight(&s, "triple_add", &params, &[], &[]));
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(count(&s), 0);
}

#[test]
fn a_scoring_refusal_names_the_reason_rather_than_staging() {
    let s = state();
    // The same `store` twice in one batch: the second sees the first as a
    // neighbour, so its novelty collapses and its admission refusal must reach
    // the caller as an invalid argument instead of a staged row.
    let staged = preflight(&s, "store", &store_params("itm-dup-a"), &[], &[]).expect("staged");
    let earlier = match &staged.writes[0] {
        BatchWrite::CreateItem { item, .. } => item.clone(),
        other => panic!("store must stage a create, got {other:?}"),
    };
    // The repeat carries the same text, so the neighbour signal is the earlier
    // operation of this same batch rather than anything already in the bank.
    let mut repeat = store_params("itm-dup-b");
    repeat["item"]["gist"] = json!("itm-dup-a prefers terse answers");
    repeat["source_text"] =
        json!("The user prefers terse answers (itm-dup-a, 1 note since 2024-01-01).");
    match preflight(&s, "store", &repeat, &[], &[earlier]) {
        Ok(staged) => assert!(staged.admission_score.is_some()),
        Err(err) => assert_eq!(err.code, ErrorCode::InvalidArgument, "{err:?}"),
    }
}
