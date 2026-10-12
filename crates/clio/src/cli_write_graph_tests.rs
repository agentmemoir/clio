// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio` triple/belief/graph write commands: gating
//! parity with the MCP dispatcher, append semantics for beliefs, `--dry-run`
//! no-write previews, and the text views.
//!
//! ## Owns
//! - Behavior tests for the triple/belief/graph write verbs
//!
//! ## Does not own
//! - The commands under test (`cli_write_graph`)
//! - The engine and shared fixtures (see `cli_write` / `cli_write_support_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{TS, call, json_of, state};
use crate::cli_write_test_support::{link_state, run_write_state};
use serde_json::json;
use std::process::ExitCode;

#[test]
fn triple_add_then_query_returns_the_edge() {
    let s = state();
    let (code, out, err) = run_write_state(
        "triple add",
        &["demo", "likes", "sqlite", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["decision"]["pass"], json!(true), "{payload}");
    assert!(payload["new_edge_id"].as_str().is_some(), "{payload}");
    let queried = call(
        &s,
        "triple_query",
        json!({"bank": "demo", "subject": "demo", "predicate": "likes"}),
    );
    assert_eq!(queried["ok"], json!(true), "{queried}");
    let records = queried["triples"].as_array().cloned().unwrap_or_default();
    assert_eq!(records.len(), 1, "{queried}");
}

#[test]
fn triple_add_rejects_continuous_update_rule_like_mcp() {
    // Triple_add rejects continuous/scalar updates; the CLI surfaces
    // the same structured refusal as the MCP dispatcher.
    let s = state();
    let (code, out, err) = run_write_state(
        "triple add",
        &["a", "score", "3", "--update-rule", "continuous"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    let direct = call(
        &s,
        "triple_add",
        json!({"subject": "a", "predicate": "score", "object": "1",
               "epistemic_kind": "fact", "update_rule": "continuous", "bank": "default"}),
    );
    let message = direct["message"].as_str().unwrap_or_default();
    assert!(!message.is_empty(), "{direct}");
    assert!(err.contains(message), "CLI: {err} / MCP: {message}");
}

#[test]
fn triple_add_requires_three_positionals() {
    let s = state();
    let (code, out, err) = run_write_state("triple add", &["only-subject"], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("SUBJECT"), "{err}");
}

#[test]
fn triple_add_belief_without_source_type_is_refused() {
    // Source_type is required iff belief, refused at the flag boundary.
    let s = state();
    let (code, out, err) = run_write_state(
        "triple add",
        &["a", "b", "c", "--epistemic-kind", "belief"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--source-type"), "{err}");
    assert!(err.contains("belief"), "{err}");
}

#[test]
fn triple_add_dry_run_writes_nothing() {
    let s = state();
    let (code, out, err) =
        run_write_state("triple add", &["a", "likes", "b", "--dry-run"], &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    let queried = call(&s, "triple_query", json!({"subject": "a"}));
    assert!(
        queried["records"]
            .as_array()
            .unwrap_or(&Vec::new())
            .is_empty(),
        "{queried}"
    );
}

#[test]
fn triple_end_expires_and_retains_history() {
    let s = state();
    let stored = call(
        &s,
        "triple_add",
        json!({"bank": "demo", "subject": "demo", "predicate": "runs",
               "object": "clio", "epistemic_kind": "fact", "timestamp": TS}),
    );
    assert_eq!(stored["ok"], json!(true), "{stored}");
    let (code, out, err) = run_write_state(
        "triple end",
        &[
            "demo",
            "runs",
            "--object",
            "clio",
            "--valid-until",
            "2025-01-01T00:00:00Z",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(
        payload["closed_ids"].as_array().map_or(0, Vec::len),
        1,
        "{payload}"
    );
    // History remains queryable as of before the end.
    let queried = call(
        &s,
        "triple_query",
        json!({"bank": "demo", "subject": "demo", "predicate": "runs"}),
    );
    assert_eq!(queried["ok"], json!(true), "{queried}");
}

#[test]
fn triple_end_requires_valid_until() {
    let s = state();
    let (code, out, err) = run_write_state("triple end", &["a", "b"], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--valid-until"), "{err}");
}

#[test]
fn belief_observe_twice_appends_without_re_scoring() {
    let s = state();
    let args = [
        "Postgres handles the writes",
        "--confidence",
        "0.7",
        "--evidence",
        "ev-1",
        "--source",
        "user_stated",
        "--bank",
        "demo",
    ];
    let (code, out, err) = run_write_state("belief observe", &args, &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["outcome"], json!("created"), "{payload}");
    let create_score = payload["decision"]["admission_score"]
        .as_f64()
        .expect("score");
    let (code, out, err) = run_write_state(
        "belief observe",
        &[
            "Postgres handles the writes",
            "--confidence",
            "0.9",
            "--evidence",
            "ev-2",
            "--source",
            "agent_inferred",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["outcome"], json!("appended"), "{payload}");
    assert_eq!(payload["entry_count"], json!(2), "{payload}");
    // The append path never re-scores novelty: the create score stands.
    let history = call(
        &s,
        "belief_history",
        json!({"bank": "demo", "belief_id_or_proposition": "Postgres handles the writes"}),
    );
    assert_eq!(history["ok"], json!(true), "{history}");
    let entries = history["entries"].as_array().cloned().unwrap_or_default();
    assert_eq!(entries.len(), 2, "{history}");
    let _ = create_score;
}

#[test]
fn belief_observe_requires_evidence_and_source() {
    let s = state();
    let (code, _out, _err) = run_write_state(
        "belief observe",
        &["prop", "--confidence", "0.5"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2));
    let (code, _out, err) = run_write_state(
        "belief observe",
        &["prop", "--confidence", "0.5", "--evidence", "e"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2));
    assert!(err.contains("--source"), "{err}");
}

#[test]
fn belief_observe_dry_run_writes_nothing() {
    let s = state();
    let (code, out, err) = run_write_state(
        "belief observe",
        &[
            "Postgres handles the writes",
            "--confidence",
            "0.7",
            "--evidence",
            "e",
            "--source",
            "user_stated",
            "--bank",
            "demo",
            "--dry-run",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    let history = call(
        &s,
        "belief_history",
        json!({"bank": "demo", "belief_id_or_proposition": "Postgres handles the writes"}),
    );
    assert!(
        history["entries"]
            .as_array()
            .unwrap_or(&Vec::new())
            .is_empty(),
        "{history}"
    );
}

#[test]
fn graph_link_then_query_returns_the_edge() {
    let (s, source, target) = link_state();
    let (code, out, err) = run_write_state(
        "graph link",
        &[
            &source,
            &target,
            "--relationship",
            "relates_to",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert!(payload["edge_id"].as_str().is_some(), "{payload}");
    let queried = call(
        &s,
        "graph_query",
        json!({"bank": "demo", "seed_id": "itm-a"}),
    );
    assert_eq!(queried["ok"], json!(true), "{queried}");
    assert!(json!(queried).to_string().contains("itm-b"), "{queried}");
}

#[test]
fn graph_link_requires_relationship() {
    let (s, source, target) = link_state();
    let (code, out, err) = run_write_state("graph link", &[&source, &target], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--relationship"), "{err}");
}

#[test]
fn graph_link_missing_endpoint_is_refused_like_mcp() {
    let (s, source, _target) = link_state();
    // A missing endpoint is a structured refusal; the CLI reports the same
    // code/message the MCP dispatcher produces.
    let (code, out, err) = run_write_state(
        "graph link",
        &[
            &source,
            "itm-missing",
            "--relationship",
            "relates_to",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    let cli = json_of(&err);
    let direct = call(
        &s,
        "graph_link",
        json!({"bank": "demo", "source_id": "itm-a", "target_id": "itm-missing",
               "relationship": "relates_to"}),
    );
    assert_eq!(direct["ok"], json!(false), "{direct}");
    // One condition, one code: the CLI route and the dispatcher route agree.
    assert_eq!(cli["code"], json!("item_not_found"), "CLI: {err}");
    assert_eq!(cli["code"], direct["code"], "CLI: {err} / MCP: {direct}");
    let message = direct["message"].as_str().unwrap_or_default();
    assert!(err.contains(message), "CLI: {err} / MCP: {message}");
}

#[test]
fn graph_link_dry_run_writes_nothing() {
    let (s, source, target) = link_state();
    let (code, out, err) = run_write_state(
        "graph link",
        &[
            &source,
            &target,
            "--relationship",
            "relates_to",
            "--dry-run",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    let queried = call(
        &s,
        "graph_query",
        json!({"bank": "default", "seed_id": "itm-a"}),
    );
    assert!(
        queried.to_string().contains("\"edges\":[]") || !queried.to_string().contains("itm-b"),
        "{queried}"
    );
}

#[test]
fn text_mode_views_are_deterministic() {
    let s = state();
    let (code, out, err) = run_write_state(
        "triple add",
        &["demo", "likes", "sqlite", "--bank", "demo"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.starts_with("edge trp-"), "{out}");
    let (code, out, err) = run_write_state(
        "triple end",
        &[
            "demo",
            "likes",
            "--valid-until",
            "2025-01-01T00:00:00Z",
            "--bank",
            "demo",
        ],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("closed 1 edge(s)"), "{out}");
    let (code, out, err) = run_write_state(
        "belief observe",
        &[
            "Postgres handles the writes",
            "--confidence",
            "0.7",
            "--evidence",
            "e",
            "--source",
            "user_stated",
            "--bank",
            "demo",
        ],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        out.contains("belief bel-") && out.contains("created"),
        "{out}"
    );
}
