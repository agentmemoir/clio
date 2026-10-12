// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the graph and workspace-inspection read commands
//! (`graph query`, `associations`, `persona get`, `maintenance status`).
//!
//! ## Owns
//! - Assertions over the graph group's builds and text views
//!
//! ## Does not own
//! - The command implementation under test (`cli_read_graph`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{GraphGroup, render_associations, render_id_list, render_maintenance, render_persona};
use crate::cli_read_group::ReadGroup;
use crate::cli_read_test_support::{TS, call, json_of, run_state, seed_item, seed_link, state};
use clio_mcp::runtime::McpState;
use serde_json::json;
use std::process::ExitCode;
use std::sync::Arc;

fn graph_state() -> Arc<McpState> {
    let s = state();
    seed_item(&s, "bank-a", "itm-1", "alpha");
    seed_item(&s, "bank-a", "itm-2", "beta");
    seed_item(&s, "bank-a", "itm-3", "gamma");
    seed_link(&s, "bank-a", "itm-1", "itm-2");
    seed_link(&s, "bank-a", "itm-2", "itm-3");
    let put = call(
        &s,
        "persona_put_stable",
        json!({"bank": "bank-a", "key": "style", "value": "concise", "timestamp": TS}),
    );
    assert_eq!(put["ok"], json!(true), "{put}");
    s
}

#[test]
fn graph_query_success_cap_and_text() {
    let s = graph_state();
    let (code, out, err) = run_state(
        "graph query",
        &[
            "itm-1",
            "--bank",
            "bank-a",
            "--max-hops",
            "2",
            "--edge-type",
            "explicit",
            "--min-weight",
            "0.0",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    assert!(json_of(&out)["nodes"].is_array());
    assert!(err.is_empty(), "{err}");

    let (code, _, _) = run_state(
        "graph query",
        &["itm-1", "--bank", "bank-a", "--max-hops", "9"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(1));

    let (_, out, _) = run_state("graph query", &["itm-1", "--bank", "bank-a"], &s, true);
    assert!(out.starts_with("nodes: "), "{out}");
}

#[test]
fn associations_list_and_text() {
    let s = graph_state();
    let (code, out, _) = run_state("associations", &["itm-1", "--bank", "bank-a"], &s, false);
    assert_eq!(code, ExitCode::from(0));
    let rows = json_of(&out)["associations"]
        .as_array()
        .expect("rows")
        .len();
    assert_eq!(rows, 1);

    let (_, out, _) = run_state(
        "associations",
        &["itm-1", "--bank", "bank-a", "--min-weight", "0.0"],
        &s,
        true,
    );
    assert!(out.contains("associations: 1"), "{out}");
    assert!(out.contains("itm-2"), "{out}");
}

#[test]
fn persona_get_budget_and_text() {
    let s = graph_state();
    let (code, out, _) = run_state("persona get", &["--bank", "bank-a"], &s, false);
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["document"]["stable"][0]["key"], json!("style"));

    let (_, out, _) = run_state(
        "persona get",
        &["--bank", "bank-a", "--budget-tokens", "1000"],
        &s,
        true,
    );
    assert!(out.contains("stable: 1"), "{out}");
    assert!(out.contains("preferences: 0"), "{out}");
}

#[test]
fn maintenance_status_scoped_and_text() {
    let s = graph_state();
    let (code, out, _) = run_state(
        "maintenance status",
        &["--bank", "bank-a", "--item-id", "itm-1"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    let payload = json_of(&out);
    assert_eq!(payload["leaf_queryable"], json!(true));
    assert!(payload["time_to_queryable_ms"].is_number(), "{payload}");
    assert!(
        payload["structural_maintenance_ms"].is_number(),
        "{payload}"
    );

    let (_, out, _) = run_state("maintenance status", &["--bank", "bank-a"], &s, true);
    assert!(out.contains("leaf_queryable: true"), "{out}");
    assert!(out.contains("dirty_ancestors: "), "{out}");
    assert!(out.contains("refreshing: "), "{out}");
    assert!(out.contains("time_to_queryable_ms: "), "{out}");
    assert!(out.contains("structural_maintenance_ms: "), "{out}");
}

#[test]
fn usage_errors_exit_two() {
    let s = graph_state();
    let cases: &[(&str, &[&str])] = &[
        ("graph query", &[]),
        ("graph query", &["itm-1", "--max-hops", "x"]),
        ("graph query", &["itm-1", "--edge-type", "bogus"]),
        ("graph query", &["itm-1", "--min-weight", "NaN"]),
        ("associations", &[]),
        ("associations", &["itm-1", "--min-weight", "x"]),
        ("persona get", &["extra"]),
        ("persona get", &["--budget-tokens", "x"]),
        ("maintenance status", &["extra"]),
    ];
    for &(verb, args) in cases {
        let (code, out, err) = run_state(verb, args, &s, false);
        assert_eq!(code, ExitCode::from(2), "{verb} {args:?}");
        assert!(out.is_empty(), "{verb} {args:?}: {out}");
        assert!(err.contains("\"code\":\"usage\""), "{verb} {args:?}: {err}");
    }
}

#[test]
fn render_variants_cover_missing_fields() {
    assert_eq!(render_id_list("nodes", &json!({})), "nodes: 0");
    assert_eq!(
        render_id_list("nodes", &json!({"nodes": [{"item_id": "x"}]})),
        "nodes: 1\nx"
    );
    assert_eq!(render_associations(&json!({})), "associations: 0");
    assert_eq!(
        render_associations(&json!({"associations": [{"neighbor_id": "n"}]})),
        "associations: 1\nn"
    );
    assert_eq!(
        render_persona(
            &json!({"document": {"stable": [{}], "preferences": [{}]}, "dropped_keys": ["k"]})
        ),
        "stable: 1\npreferences: 1\ndropped_keys: 1"
    );
    assert_eq!(
        render_maintenance(&json!({"leaf_queryable": true, "item_id": "i"})),
        "leaf_queryable: true\nitem_id: i\ndirty_ancestors: 0\nrefreshing: 0\ntime_to_queryable_ms: 0\nstructural_maintenance_ms: 0"
    );
    assert_eq!(
        render_maintenance(&json!({
            "leaf_queryable": true,
            "item_id": "i",
            "time_to_queryable_ms": null,
            "structural_maintenance_ms": null
        })),
        "leaf_queryable: true\nitem_id: i\ndirty_ancestors: 0\nrefreshing: 0\ntime_to_queryable_ms: 0\nstructural_maintenance_ms: 0"
    );
    assert_eq!(
        render_maintenance(&json!({
            "leaf_queryable": true,
            "item_id": "i",
            "time_to_queryable_ms": 12,
            "structural_maintenance_ms": 34
        })),
        "leaf_queryable: true\nitem_id: i\ndirty_ancestors: 0\nrefreshing: 0\ntime_to_queryable_ms: 12\nstructural_maintenance_ms: 34"
    );
}

/// Every verb's spec is its record's, and the accepted set that record carries is
/// the one the group's own list held: same flags, same order, globals included.
#[test]
fn spec_answers_from_the_registry_with_the_original_accepted_set() {
    let expected: &[(&str, &[&str], &[&str])] = &[
        (
            "graph query",
            &[
                "db",
                "backend",
                "bank",
                "actor",
                "output",
                "max-hops",
                "edge-type",
                "min-weight",
            ],
            &["no-input", "yes", "explain"],
        ),
        (
            "associations",
            &["db", "backend", "bank", "actor", "output", "min-weight"],
            &["no-input", "yes", "explain"],
        ),
        (
            "persona get",
            &["db", "backend", "bank", "actor", "output", "budget-tokens"],
            &["no-input", "yes", "explain"],
        ),
        (
            "maintenance status",
            &["db", "backend", "bank", "actor", "output", "item-id"],
            &["no-input", "yes", "explain"],
        ),
    ];
    let group = GraphGroup;
    assert_eq!(group.verbs().len(), expected.len(), "every verb is covered");
    for &(verb, value, boolean) in expected {
        let spec = group.spec(verb);
        assert_eq!(spec.value_flags(), value, "{verb} value flags");
        assert_eq!(spec.bool_flags(), boolean, "{verb} bool flags");
        let record = crate::cli_command_registry::find(verb).expect("a record");
        assert_eq!(
            spec.value_flags(),
            record.parser_spec().value_flags(),
            "{verb}: the group must answer with the record"
        );
        assert_eq!(
            spec.bool_flags(),
            record.parser_spec().bool_flags(),
            "{verb}: the group must answer with the record"
        );
    }
}
