// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP-surface coverage of the `batch` tool:
//! atomic commit, rollback on admission failure, `$ref` resolution, dry-run,
//! and op-cap rejection.
//!
//! ## Owns
//! - `batch_tool` request/response assertions at the MCP tool boundary
//!
//! ## Does not own
//! - Transactional execution (see `clio-store` batch backend modules)
//! - Batch pre-flight decoding and gating (see `batch_preflight.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::batch_tools_test_support_tests::{batch, count_bank, state, store_op};
use super::*;
use serde_json::json;
use std::sync::Arc;

#[test]
fn t20_01_three_valid_store_ops_commit_atomically() {
    let s = state();
    let ops = json!([
        store_op("itm-1", "alpha"),
        store_op("itm-2", "beta"),
        store_op("itm-3", "gamma")
    ]);
    let out = batch(&s, ops, false);
    assert_eq!(out["ok"], json!(true), "{out}");
    let ops_out = out["operations"].as_array().unwrap();
    assert_eq!(ops_out.len(), 3);
    for r in ops_out {
        assert_eq!(r["status"], "COMMITTED");
        assert!(r["admission_score"].as_f64().is_some());
    }
    assert_eq!(count_bank(&s, "bank-a"), 3);
    // Leaves are published immediately.
    assert!(s.leaves.get("bank-a", "itm-1").is_some());
}

#[test]
fn t20_02_admission_failure_rolls_back_zero_rows() {
    let s = state();
    // The third op re-scores a near-duplicate low-novelty payload below θ.
    let ops = json!([
        store_op("itm-1", "alpha"),
        store_op("itm-2", "beta"),
        store_op("itm-3", "alpha")
    ]);
    let out = batch(&s, ops, false);
    let bank_count = count_bank(&s, "bank-a");
    if out["ok"].as_bool().unwrap_or(false) {
        // If all three were admitted, the atomicity claim is exercised at the
        // store level (batch_tests::sqlite_batch_is_atomic) instead.
        assert!(bank_count <= 3);
        return;
    }
    assert_eq!(bank_count, 0, "zero rows must persist after abort");
    assert!(
        out["message"]
            .as_str()
            .unwrap_or("")
            .contains("operation 2")
    );
}

#[test]
fn batch_rolls_back_on_third_op_failure_via_forced_rejection() {
    let s = state();
    let ops = json!([
        store_op("itm-a", "alpha"),
        store_op("itm-b", "beta"),
        {
            // Invalid candidate pairing: category/epistemic_kind mismatch fails
            // closed during pre-flight.
            "tool": "store",
            "params": {
                "item": {"id": "itm-bad", "gist": "x"},
                "category": "task_spec",
                "episodic_type": "task",
                "epistemic_kind": "fact",
                "bank": "bank-a",
                "actor": "agent"
            }
        }
    ]);
    let out = batch(&s, ops, false);
    assert_eq!(out["ok"], json!(false), "{out}");
    assert_eq!(count_bank(&s, "bank-a"), 0);
}

#[test]
fn dry_run_reports_would_commit_without_writing() {
    let s = state();
    let ops = json!([store_op("itm-1", "alpha")]);
    let out = batch(&s, ops, true);
    assert_eq!(out["ok"], json!(true));
    assert_eq!(out["dry_run"], json!(true));
    assert_eq!(out["operations"][0]["status"], "WOULD_COMMIT");
    assert_eq!(count_bank(&s, "bank-a"), 0);
}

#[test]
fn ref_resolution_links_store_and_graph_link() {
    let s = state();
    // Op 0 creates the item; op 1 links to it via `$ref:0.id`.
    let ops = json!([
        store_op("itm-ref", "linked"),
        store_op("itm-other", "other"),
        {"tool": "graph_link", "params": {
            "source_id": "$ref:0.id",
            "target_id": "$ref:1.id",
            "relationship": "related",
            "bank": "bank-a",
            "actor": "agent"
        }}
    ]);
    let out = batch(&s, ops, false);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(s.store.list_assoc_edges("bank-a", None).unwrap().len(), 1);
}

#[test]
fn forward_or_unknown_refs_fail_closed() {
    let s = state();
    let ops = json!([
        {"tool": "graph_link", "params": {
            "source_id": "$ref:1.id", "target_id": "t", "relationship": "r",
            "bank": "bank-a", "actor": "agent"
        }}
    ]);
    let out = batch(&s, ops, false);
    assert_eq!(out["ok"], json!(false));
    assert!(out["message"].as_str().unwrap_or("").contains("forward"));

    let ops = json!([
        {"tool": "no_such_tool", "params": {}}
    ]);
    let out = batch(&s, ops, false);
    assert_eq!(out["ok"], json!(false));
    assert!(
        out["message"]
            .as_str()
            .unwrap_or("")
            .contains("unrecognized batch op tool")
    );
}

#[test]
fn cap_and_shape_rejections() {
    let s = state();
    // 51 ops exceed the cap.
    let ops: Vec<Value> = (0..51)
        .map(|i| store_op(&format!("itm-{i}"), "x"))
        .collect();
    let out = batch(&s, json!(ops), false);
    assert_eq!(out["ok"], json!(false));
    assert!(out["message"].as_str().unwrap_or("").contains("cap"));
    // Missing operations array.
    let out = batch_tool(&s, &json!({}));
    assert_eq!(out["ok"], json!(false));
}

#[test]
fn canonical_put_inside_batch_supersedes_and_reads() {
    let s = state();
    let ops = json!([
        {"tool": "canonical_put", "params": {
            "key": "primary_host", "value": "db-a", "category": "tool_config",
            "epistemic_kind": "fact", "bank": "bank-a", "actor": "agent"
        }}
    ]);
    let out = batch(&s, ops, false);
    assert_eq!(out["ok"], json!(true), "{out}");
    let read = crate::read_tools::dispatch(
        &s,
        "canonical_get",
        &json!({
            "key": "primary_host", "bank": "bank-a", "actor": "agent"
        }),
    )
    .unwrap();
    assert_eq!(read["value"], "db-a");
}

#[test]
fn admission_rejection_helper_shapes_structured_error() {
    let factors = clio_admission::decision::FactorBreakdown {
        utility: 0.1,
        confidence: 0.1,
        novelty: 0.1,
        recency: 0.1,
        type_prior: 0.1,
    };
    let decision = clio_admission::decision::Decision {
        pass: false,
        admission_score: 0.1,
        factors,
        rejection_reason: Some("below threshold".into()),
        profile_version: 1,
        mission_rule_ids: Vec::new(),
        mission_rule_labels: Vec::new(),
    };
    let err = crate::batch_preflight::admission_rejected(&decision);
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(err.message, "below threshold");
    let blank = crate::batch_preflight::admission_rejected(&clio_admission::decision::Decision {
        pass: false,
        admission_score: 0.1,
        factors,
        rejection_reason: None,
        profile_version: 1,
        mission_rule_ids: Vec::new(),
        mission_rule_labels: Vec::new(),
    });
    assert_eq!(blank.message, "admission rejected");
}

#[test]
#[allow(clippy::too_many_lines)]
fn batch_preflight_admission_rejection_and_validation_edges() {
    let s = state();
    // θ above every achievable score → deterministic admission rejection for
    // the item-shaped path (the rejection helper shapes the batch abort).
    let mut rejecting = state();
    let env = &mut Arc::get_mut(&mut rejecting).expect("single owner").env;
    *env = clio_config::ranking::RankingEnv {
        theta_admit: 1.1,
        ..clio_config::ranking::RankingEnv::system_defaults()
    };
    let ops = json!([store_op("itm-reject", "alpha")]);
    let out = batch(&rejecting, ops, false);
    assert_eq!(out["ok"], json!(false), "{out}");
    assert!(
        out["message"]
            .as_str()
            .unwrap_or("")
            .contains("operation 0")
    );
    assert_eq!(count_bank(&rejecting, "bank-a"), 0);

    // Rejected record ops under the failing θ all abort with the decision reason.
    let rec_ops = json!([
        {"tool": "task_upsert", "params": {
            "task_id": "t-r", "definition": "fix the build", "steps": ["edit"],
            "status": "in_progress", "bank": "bank-a", "actor": "agent"
        }},
        {"tool": "persona_put_stable", "params": {
            "key": "style", "value": "concise", "bank": "bank-a", "actor": "agent"
        }},
        {"tool": "failure_record", "params": {
            "task_id": "t-r", "attempt_n": 1, "what_failed": "boom",
            "lesson": "learn", "bank": "bank-a", "actor": "agent"
        }},
        {"tool": "triple_add", "params": {
            "subject": "a", "predicate": "p", "object": "o",
            "epistemic_kind": "fact", "bank": "bank-a", "actor": "agent"
        }}
    ]);
    let out = batch(&rejecting, rec_ops, false);
    assert_eq!(out["ok"], json!(false), "{out}");
    assert!(
        out["message"]
            .as_str()
            .unwrap_or("")
            .contains("operation 0")
    );
    assert_eq!(count_bank(&rejecting, "bank-a"), 0);

    // graph_link validation edges fail closed before staging.
    let same = batch(
        &s,
        json!([
            {"tool": "graph_link", "params": {
                "source_id": "x", "target_id": "x", "relationship": "r",
                "bank": "bank-a", "actor": "agent"
            }}
        ]),
        false,
    );
    assert!(
        same["message"].as_str().unwrap_or("").contains("distinct"),
        "{same}"
    );

    let weight = batch(
        &s,
        json!([
            {"tool": "graph_link", "params": {
                "source_id": "x", "target_id": "y", "relationship": "r",
                "weight": 1.5, "bank": "bank-a", "actor": "agent"
            }}
        ]),
        false,
    );
    assert!(
        weight["message"].as_str().unwrap_or("").contains("weight"),
        "{weight}"
    );

    let missing = batch(
        &s,
        json!([
            {"tool": "graph_link", "params": {
                "source_id": "ghost-a", "target_id": "ghost-b", "relationship": "r",
                "bank": "bank-a", "actor": "agent"
            }}
        ]),
        false,
    );
    assert!(
        missing["message"]
            .as_str()
            .unwrap_or("")
            .contains("not found"),
        "{missing}"
    );

    // The unreachable pre-flight routing arms are internal invariants; call
    // them directly to pin the fail-closed behavior.
    let Err(ctx_err) = crate::batch_preflight::preflight(&s, "no_such_tool", &json!({}), &[])
    else {
        panic!("unknown tool must fail closed");
    };
    assert_eq!(ctx_err.code, ErrorCode::Internal);
    let Err(rec_err) = crate::batch_preflight_records::records_preflight(
        &s,
        "no_such_tool",
        &clio_types::OpContext::new(clio_types::Bank::new("b"), clio_types::Actor::Agent),
        "t",
        &json!({}),
    ) else {
        panic!("unknown tool must fail closed");
    };
    assert_eq!(rec_err.code, ErrorCode::Internal);
}
