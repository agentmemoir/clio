// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the MCP-bound hygiene tools (`hygiene_tools.rs`):
//! argument decoding, target normalization, dry-run vs confirm shapes, log-row
//! alias fields, and schema publication for both transports.
//!
//! ## Owns
//! - Test coverage for hygiene tool dispatch, argument validation, and the
//!   `hygiene_log_list` response shape (`timestamp` / `bank` aliases)
//!
//! ## Does not own
//! - The tool behavior itself (`hygiene_tools.rs` or `clio-hygiene`)
//! - Core/additive tool schema publication (`schema_defs` tests)
//!
//! ## Boundary
//!
//! Tests here break when MCP hygiene dispatch changes; they never change it.
//!
//! Keep this module focused on MCP hygiene tool tests.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn state() -> Arc<McpState> {
    McpState::open(&McpOpenOptions {
        backend: "sqlite".into(),
        database_url: "sqlite::memory:".into(),
        default_bank: "bank-mcp".into(),
        default_actor: clio_types::Actor::Agent,
        shared_bank_enabled: false,
        kms_path: None,
    })
    .expect("mcp state")
}

fn file_state(path: &std::path::Path) -> Arc<McpState> {
    McpState::open(&McpOpenOptions {
        backend: "sqlite".into(),
        database_url: path.to_string_lossy().into_owned(),
        default_bank: "bank-mcp".into(),
        default_actor: clio_types::Actor::Agent,
        shared_bank_enabled: false,
        kms_path: None,
    })
    .expect("file-backed mcp state")
}

pub(super) fn seed_item(st: &McpState, id: &str) {
    let item = clio_types::MemoryItem {
        id: id.into(),
        bank_id: "bank-mcp".into(),
        subject_id: format!("subj-{id}"),
        kind: clio_types::ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.2),
        source_ref: None,
        snapshot: None,
        gist: Some("mcp hygiene fixture item".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    };
    st.store.create_memory_item(&item, "actor").expect("seed");
}

pub(super) fn ok_payload(result: &Value) -> Value {
    result.clone()
}

#[test]
fn hygiene_audit_read_is_bound_and_returns_ranked_candidates() {
    let st = state();
    seed_item(&st, "mcp-audit-1");
    let result = dispatch(
        &st,
        "hygiene_audit",
        &json!({ "min_score": 0.0, "limit": 10 }),
    );
    assert_eq!(result["ok"], Value::Bool(true), "{result}");
    let payload = ok_payload(&result);
    assert_eq!(payload["bank"], "bank-mcp");
    let candidates = payload["candidates"].as_array().expect("candidates");
    assert!(candidates.iter().any(|c| c["item_id"] == "mcp-audit-1"));
}

#[test]
fn hygiene_clean_dry_run_then_confirm_via_mcp() {
    let st = state();
    seed_item(&st, "mcp-clean-1");
    // Dry-run: counts only.
    let dry = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "item_ids": ["mcp-clean-1"], "action": "archive" }),
    );
    let payload = ok_payload(&dry);
    assert_eq!(payload["dry_run"], Value::Bool(true));
    assert_eq!(payload["would_archive"], 1);
    // Confirmed: applied id list.
    let confirmed = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "item_ids": ["mcp-clean-1"], "action": "archive", "confirm": true }),
    );
    let payload = ok_payload(&confirmed);
    assert_eq!(payload["dry_run"], Value::Bool(false));
    assert_eq!(payload["applied"], json!(["mcp-clean-1"]));
    // `delete` alias maps to discard and dry-runs without confirm.
    let alias = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "item_ids": ["mcp-clean-1"], "action": "delete" }),
    );
    assert_eq!(ok_payload(&alias)["dry_run"], Value::Bool(true));
}

#[test]
fn hygiene_clean_dry_run_then_confirm_across_fresh_runtime_instances() {
    let path = std::env::temp_dir().join(format!("clio-hygiene-xproc-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);

    let first = file_state(&path);
    seed_item(&first, "xproc-1");
    let dry = dispatch(
        &first,
        "hygiene_clean",
        &json!({ "item_ids": ["xproc-1"], "action": "archive" }),
    );
    assert_eq!(dry["ok"], Value::Bool(true), "{dry}");
    assert_eq!(dry["dry_run"], Value::Bool(true), "{dry}");
    drop(first);

    // A fresh runtime restarts the per-process counter; the dry-run rows are
    // already durable, so the confirmed insert must not collide on their ids.
    let second = file_state(&path);
    let confirm = dispatch(
        &second,
        "hygiene_clean",
        &json!({ "item_ids": ["xproc-1"], "action": "archive", "confirm": true }),
    );
    assert_eq!(confirm["ok"], Value::Bool(true), "{confirm}");
    assert_eq!(confirm["applied"], json!(["xproc-1"]), "{confirm}");
    drop(second);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn hygiene_clean_reports_batch_failure_shape() {
    let st = state();
    seed_item(&st, "mcp-batch-1");
    let result = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "item_ids": ["mcp-batch-1", "mcp-ghost", "mcp-never"], "action": "archive", "confirm": true }),
    );
    let payload = ok_payload(&result);
    assert_eq!(payload["applied"], json!(["mcp-batch-1"]));
    assert_eq!(payload["pending"], json!(["mcp-never"]));
    assert_eq!(payload["failed"]["item_id"], "mcp-ghost");
}

#[test]
fn hygiene_log_list_returns_masked_rows() {
    let st = state();
    seed_item(&st, "mcp-log-1");
    let flagged = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "item_ids": ["mcp-log-1"], "action": "flag", "confirm": true, "note": "n" }),
    );
    assert_eq!(flagged["ok"], Value::Bool(true), "{flagged}");
    let result = dispatch(&st, "hygiene_log_list", &json!({ "limit": 10 }));
    let payload = ok_payload(&result);
    assert!(payload["count"].as_u64().expect("count") >= 1);
    let rows = payload["rows"].as_array().expect("rows");
    let mine = rows
        .iter()
        .find(|r| r["item_id"] == "mcp-log-1")
        .expect("row present");
    assert_eq!(mine["action"], "flag");
    assert_eq!(mine["dry_run"], Value::Bool(false));
    // Log-entry terminology: timestamp/bank aliases of the store
    // columns created_at/bank_id, alongside them.
    assert_eq!(mine["timestamp"], mine["created_at"]);
    assert_eq!(mine["bank"], mine["bank_id"]);
    // Detail carries ids/labels only — no content or secret-shaped keys.
    let detail = mine["detail"].to_string();
    assert!(!detail.contains("gist") && !detail.contains("api_key"));
}

#[test]
fn hygiene_errors_surface_as_tool_failures() {
    let st = state();
    // A per-item failure is reported as a structured failure object inside the
    // outcome (applied vs pending) — the batch abort is surfaced, not silent.
    let result = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "item_ids": ["missing-1"], "action": "archive", "confirm": true }),
    );
    let payload = ok_payload(&result);
    assert_eq!(payload["failed"]["item_id"], "missing-1");
    assert!(
        payload["failed"]["error"]
            .as_str()
            .expect("error text")
            .contains("not found")
    );
    let bad = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "item_ids": ["x"], "action": "erase", "confirm": true }),
    );
    assert_eq!(bad["ok"], Value::Bool(false), "{bad}");
}

#[test]
fn hygiene_tools_are_schema_published() {
    let defs = crate::schema::catalog_defs();
    for name in ["hygiene_audit", "hygiene_clean", "hygiene_log_list"] {
        let def = defs
            .iter()
            .find(|d| d.name == name)
            .expect("schema def present");
        let tool = def.to_tool();
        let defects = crate::schema::validate_tool_schema(&tool);
        assert!(defects.is_empty(), "{name}: {defects:?}");
    }
    // Bound on both transports via the shared bound sets.
    assert!(crate::bound_read_tools().contains(&"hygiene_audit"));
    assert!(crate::bound_read_tools().contains(&"hygiene_log_list"));
    assert!(crate::bound_write_tools().contains(&"hygiene_clean"));
    assert!(crate::bound_tools().contains(&"hygiene_audit"));
}

#[test]
fn hygiene_clean_accepts_prior_audit_candidates() {
    let st = state();
    seed_item(&st, "mcp-cand-1");
    // Audit first, then feed the candidates straight back into clean with an
    // explicit action (the fixture item scores below the suggested-action band).
    let audit = ok_payload(&dispatch(
        &st,
        "hygiene_audit",
        &json!({ "min_score": 0.0, "limit": 10, "offset": 0 }),
    ));
    let candidates = audit["candidates"].as_array().expect("candidates").clone();
    let result = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "candidates": candidates, "action": "archive", "confirm": false }),
    );
    let payload = ok_payload(&result);
    assert_eq!(payload["dry_run"], Value::Bool(true));
    assert_eq!(payload["would_archive"], candidates.len());
    // `keep` resolves each candidate's suggested_action when one exists: a
    // secret fixture lands in the discard band (≥ 0.90).
    let secret_item = clio_types::MemoryItem {
        id: "mcp-cand-2".into(),
        bank_id: "bank-mcp".into(),
        subject_id: "subj-mcp-cand-2".into(),
        kind: clio_types::ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.0),
        source_ref: None,
        snapshot: None,
        gist: Some("api_key: sk-live-abcdefgh1234".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    };
    st.store
        .create_memory_item(&secret_item, "actor")
        .expect("seed 2");
    let audit = ok_payload(&dispatch(
        &st,
        "hygiene_audit",
        &json!({ "min_score": 0.90, "limit": 10 }),
    ));
    let candidates = audit["candidates"]
        .as_array()
        .expect("discard-band candidates")
        .clone();
    assert!(
        !candidates.is_empty(),
        "secret fixture should reach the discard band"
    );
    let result = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "candidates": candidates, "action": "keep", "confirm": false }),
    );
    let payload = ok_payload(&result);
    assert_eq!(payload["would_discard"], candidates.len());
}

#[test]
fn hygiene_clean_accepts_a_single_item_id_string() {
    let st = state();
    seed_item(&st, "mcp-single-1");
    let result = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "item_id": "mcp-single-1", "action": "flag", "confirm": false }),
    );
    let payload = ok_payload(&result);
    assert_eq!(payload["would_flag"], 1);
}

#[test]
fn hygiene_audit_invalid_min_score_fails_as_tool_error() {
    let st = state();
    let result = dispatch(&st, "hygiene_audit", &json!({ "min_score": 1.5 }));
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
}

#[test]
fn hygiene_log_list_pagination_bounds_fail_closed() {
    let st = state();
    let result = dispatch(&st, "hygiene_log_list", &json!({ "limit": 0 }));
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    let result = dispatch(&st, "hygiene_log_list", &json!({ "limit": 201 }));
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
}

#[test]
fn hygiene_target_normalization_fails_closed() {
    let st = state();
    // Candidate without item_id is rejected.
    let result = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "candidates": [{ "noise_score": 0.5 }], "action": "flag", "confirm": true }),
    );
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .contains("candidate.item_id")
    );
    // Non-string item_ids entries are rejected.
    let result = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "item_ids": [7], "action": "flag", "confirm": true }),
    );
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert!(result["message"].as_str().unwrap().contains("item_ids"));
    // No targets at all (and no item_id string) falls through to the empty
    // targets rejection of the in-process clean.
    let result = dispatch(
        &st,
        "hygiene_clean",
        &json!({ "action": "flag", "confirm": true }),
    );
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    // Unknown names routed here fail as not-implemented.
    let err = crate::hygiene_tools::dispatch_inner(&st, "hygiene_unknown", &json!({}));
    assert_eq!(err.unwrap_err().code, clio_types::ErrorCode::NotImplemented);
}
