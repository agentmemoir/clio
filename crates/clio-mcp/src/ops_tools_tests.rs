// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the MCP-bound operator tools (`ops_tools.rs`): argument
//! decoding, the top-level `{ok}` envelope, dry-run vs confirm gating, the
//! repair/verify round trip, and schema publication for both transports.
//!
//! ## Owns
//! - Test coverage for `diagnose`, `verify`, `doctor`, `repair`, and `reindex`
//!   dispatch over an in-memory SQLite state
//!
//! ## Does not own
//! - The operator semantics themselves (`clio-ops`)
//! - Schema definition content (`schema_ops_defs`)
//!
//! ## Boundary
//!
//! Tests here break when MCP operator dispatch changes; they never change it.
//!
//! Keep this module focused on MCP operator tool tests.

use crate::runtime::{McpOpenOptions, McpState};
use crate::write_tools::dispatch;
use clio_store::ItemInspectFilter;
use serde_json::{Value, json};
use std::sync::Arc;

const TS: &str = "2026-01-01T00:00:00Z";

fn state() -> Arc<McpState> {
    McpState::open_with_effective(
        &McpOpenOptions {
            backend: "sqlite".into(),
            database_url: "sqlite::memory:".into(),
            default_bank: "bank-ops".into(),
            default_actor: clio_types::Actor::Agent,
            shared_bank_enabled: false,
            kms_path: None,
        },
        &serde_json::json!({}),
    )
    .expect("mcp state")
}

fn seed_item(st: &McpState, id: &str) {
    let item = clio_types::MemoryItem {
        id: id.into(),
        bank_id: "bank-ops".into(),
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
        gist: Some("mcp ops fixture item".into()),
        valid_from: TS.into(),
        valid_until: None,
        tx_from: TS.into(),
        tx_until: None,
        created_at: TS.into(),
        updated_at: TS.into(),
        context: None,
    };
    st.store.create_memory_item(&item, "actor").expect("seed");
}

fn plant_orphan_edge(st: &McpState) {
    st.store
        .put_assoc_edge(&clio_store::AssocEdgeRecord {
            id: "ghost-edge".into(),
            bank_id: "bank-ops".into(),
            source_id: "ghost-a".into(),
            target_id: "ghost-b".into(),
            assoc_kind: clio_store::AssocKind::Explicit,
            relationship: Some("r".into()),
            weight: 0.5,
            last_updated_at: TS.into(),
        })
        .expect("plant orphan edge");
}

fn has_finding(value: &Value, code: &str) -> bool {
    value["findings"]
        .as_array()
        .is_some_and(|fs| fs.iter().any(|f| f["code"] == code))
}

#[test]
fn diagnose_fresh_store_is_ok() {
    let st = state();
    let result = dispatch(&st, "diagnose", &json!({ "timestamp": TS }));
    assert_eq!(result["ok"], Value::Bool(true), "{result}");
    assert_eq!(result["timestamp"], TS);
    assert_eq!(result["backend_reachable"], Value::Bool(true));
}

#[test]
fn verify_reports_orphan_assoc_edge() {
    let st = state();
    plant_orphan_edge(&st);
    let result = dispatch(
        &st,
        "verify",
        &json!({ "bank": "bank-ops", "timestamp": TS }),
    );
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert!(has_finding(&result, "ORPHAN_ASSOC_EDGE"), "{result}");
}

#[test]
fn doctor_default_prints_plan_without_registering() {
    let st = state();
    let result = dispatch(&st, "doctor", &json!({ "timestamp": TS }));
    assert_eq!(result["dry_run"], Value::Bool(true), "{result}");
    assert!(
        result["plan_id"].as_str().is_some_and(|id| !id.is_empty()),
        "{result}"
    );
    assert_eq!(st.ops_plans.len(), 0);
}

#[test]
fn doctor_live_registers_one_plan_and_does_not_write_items() {
    let st = state();
    seed_item(&st, "ops-doctor-1");
    let before = st
        .store
        .count_items(&ItemInspectFilter::new("bank-ops"))
        .expect("count");
    let result = dispatch(
        &st,
        "doctor",
        &json!({ "bank": "bank-ops", "dry_run": false, "timestamp": TS }),
    );
    assert_eq!(result["dry_run"], Value::Bool(false), "{result}");
    assert!(
        result["plan_id"].as_str().is_some_and(|id| !id.is_empty()),
        "{result}"
    );
    assert_eq!(st.ops_plans.len(), 1);
    let after = st
        .store
        .count_items(&ItemInspectFilter::new("bank-ops"))
        .expect("count");
    assert_eq!(before, after);
}

#[test]
fn repair_without_confirm_is_refused() {
    let st = state();
    let result = dispatch(
        &st,
        "repair",
        &json!({
            "bank": "bank-ops",
            "actions": ["drop_orphan_assoc_edge"],
            "timestamp": TS
        }),
    );
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert!(has_finding(&result, "CONFIRM_REQUIRED"), "{result}");
}

#[test]
fn repair_unknown_action_is_refused() {
    let st = state();
    let result = dispatch(
        &st,
        "repair",
        &json!({
            "bank": "bank-ops",
            "actions": ["nuke_everything"],
            "confirm": true,
            "timestamp": TS
        }),
    );
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert!(has_finding(&result, "UNKNOWN_REPAIR_ACTION"), "{result}");
}

#[test]
fn repair_confirmed_drops_orphan_edge_and_reverify_clears() {
    let st = state();
    plant_orphan_edge(&st);
    let result = dispatch(
        &st,
        "repair",
        &json!({
            "bank": "bank-ops",
            "actions": ["drop_orphan_assoc_edge"],
            "confirm": true,
            "timestamp": TS
        }),
    );
    assert_eq!(result["ok"], Value::Bool(true), "{result}");
    let after = dispatch(
        &st,
        "verify",
        &json!({ "bank": "bank-ops", "timestamp": TS }),
    );
    assert!(!has_finding(&after, "ORPHAN_ASSOC_EDGE"), "{after}");
}

#[test]
fn reindex_dry_run_reports_cost_and_rejects_unknown_target() {
    let st = state();
    seed_item(&st, "ops-reindex-1");
    let result = dispatch(
        &st,
        "reindex",
        &json!({ "bank": "bank-ops", "target": "all", "timestamp": TS }),
    );
    // A dry run applies nothing, so its report is not `ok`.
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert_eq!(result["dry_run"], Value::Bool(true), "{result}");
    assert!(result["indexable_items"].is_u64(), "{result}");
    assert!(result["dense_missing"].is_u64(), "{result}");
    let bogus = dispatch(
        &st,
        "reindex",
        &json!({ "bank": "bank-ops", "target": "bogus", "timestamp": TS }),
    );
    assert_eq!(bogus["ok"], Value::Bool(false), "{bogus}");
}

#[test]
fn ops_tools_are_schema_published_and_bound() {
    let defs = crate::schema::catalog_defs();
    for name in ["diagnose", "verify", "doctor", "repair", "reindex"] {
        let def = defs
            .iter()
            .find(|d| d.name == name)
            .expect("schema def present");
        let defects = crate::schema::validate_tool_schema(&def.to_tool());
        assert!(defects.is_empty(), "{name}: {defects:?}");
    }
    for name in ["diagnose", "verify", "doctor"] {
        assert!(crate::bound_read_tools().contains(&name), "{name}");
        assert!(crate::bound_tools().contains(&name), "{name}");
    }
    for name in ["repair", "reindex"] {
        assert!(crate::bound_write_tools().contains(&name), "{name}");
        assert!(crate::bound_tools().contains(&name), "{name}");
    }
}

#[test]
fn repair_shared_bank_without_enablement_is_forbidden() {
    let st = state();
    let result = dispatch(
        &st,
        "repair",
        &json!({
            "bank": "shared",
            "actions": ["drop_orphan_assoc_edge"],
            "confirm": true,
            "timestamp": TS
        }),
    );
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert_eq!(result["code"], "forbidden", "{result}");
}

#[test]
fn reindex_without_bank_while_shared_disabled_is_forbidden() {
    let st = state();
    let result = dispatch(&st, "reindex", &json!({ "timestamp": TS }));
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert_eq!(result["code"], "forbidden", "{result}");
}

#[test]
fn reindex_without_bank_with_shared_enabled_sweeps_whole_store() {
    let st = McpState::open_with_effective(
        &McpOpenOptions {
            backend: "sqlite".into(),
            database_url: "sqlite::memory:".into(),
            default_bank: "bank-ops".into(),
            default_actor: clio_types::Actor::Agent,
            shared_bank_enabled: true,
            kms_path: None,
        },
        &serde_json::json!({}),
    )
    .expect("mcp state");
    let result = dispatch(&st, "reindex", &json!({ "timestamp": TS }));
    // The default is a report, which applies nothing and is not `ok`.
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert_eq!(result["dry_run"], Value::Bool(true), "{result}");
}

#[test]
fn ops_error_envelope_redacts_connection_urls() {
    use clio_types::{AmError, ErrorCode};
    let err = AmError::new(
        ErrorCode::Internal,
        "connection failed for postgres://ops:hunter2@db.host:5432/am",
    );
    let out = super::error_envelope(&json!({}), &err);
    assert!(!out.to_string().contains("hunter2"), "{out}");
    assert!(
        out["message"]
            .as_str()
            .is_some_and(|m| m.contains("postgres://***@db.host:5432/am")),
        "{out}"
    );
}

#[test]
fn reindex_dense_live_without_embedder_fails() {
    // Hermetic by construction: `state()` boots `Runtime::new()`, which layers
    // the machine's deployment file, so on any host with embed configured the
    // "without embedder" premise silently breaks. Resolve providers from an
    // empty document instead, so this test never depends on the host env.
    let st = McpState::open_with_effective(
        &McpOpenOptions {
            backend: "sqlite".into(),
            database_url: "sqlite::memory:".into(),
            default_bank: "bank-ops".into(),
            default_actor: clio_types::Actor::Agent,
            shared_bank_enabled: false,
            kms_path: None,
        },
        &json!({}),
    )
    .expect("mcp state");
    let result = dispatch(
        &st,
        "reindex",
        &json!({
            "target": "dense",
            "dry_run": false,
            "confirm": true,
            "bank": "bank-ops",
            "timestamp": TS
        }),
    );
    assert_eq!(result["ok"], Value::Bool(false), "{result}");
    assert_eq!(result["code"], "config_corrupt", "{result}");
}

#[test]
fn repair_actions_decoding_fails_closed() {
    let st = state();
    let not_array = dispatch(
        &st,
        "repair",
        &json!({
            "actions": "drop_orphan_assoc_edge",
            "confirm": true,
            "bank": "bank-ops",
            "timestamp": TS
        }),
    );
    assert_eq!(not_array["ok"], Value::Bool(false), "{not_array}");
    let not_string = dispatch(
        &st,
        "repair",
        &json!({
            "actions": [7],
            "confirm": true,
            "bank": "bank-ops",
            "timestamp": TS
        }),
    );
    assert_eq!(not_string["ok"], Value::Bool(false), "{not_string}");
    let no_actions = dispatch(
        &st,
        "repair",
        &json!({
            "plan_id": "plan-missing",
            "confirm": true,
            "bank": "bank-ops",
            "timestamp": TS
        }),
    );
    assert_eq!(no_actions["ok"], Value::Bool(false), "{no_actions}");
    assert!(has_finding(&no_actions, "PLAN_NOT_FOUND"), "{no_actions}");
}

#[test]
fn ops_output_never_contains_item_content_or_secrets() {
    let st = state();
    let secret = "sk-live-supersecret-1234567890";
    let item = clio_types::MemoryItem {
        id: "mcp-secret-1".into(),
        bank_id: "bank-ops".into(),
        subject_id: "subj-secret".into(),
        kind: clio_types::ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.2),
        source_ref: None,
        snapshot: Some(json!({"token": secret})),
        gist: Some(format!("api_key: {secret}")),
        valid_from: TS.into(),
        valid_until: None,
        tx_from: TS.into(),
        tx_until: None,
        created_at: TS.into(),
        updated_at: TS.into(),
        context: None,
    };
    st.store
        .create_memory_item(&item, "actor")
        .expect("seed secret");
    for tool in ["diagnose", "verify", "doctor"] {
        let out = dispatch(&st, tool, &json!({ "bank": "bank-ops", "timestamp": TS }));
        assert!(
            !out.to_string().contains(secret),
            "{tool} leaked item content: {out}"
        );
    }
}
