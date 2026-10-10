// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the flag decoding, positional validation, and text
//! fallbacks of the `clio` triple/belief/graph write commands.
//!
//! ## Owns
//! - Flag-mapping and validation tests for the triple/belief/graph verbs
//!
//! ## Does not own
//! - The commands under test (`cli_write_graph`)
//! - The behavior tests (see `cli_write_graph_tests`) or the shared fixtures
//!   (see `cli_write_support_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{TS, json_of, state};
use crate::cli_write_test_support::{link_state, run_write_state};
use serde_json::json;
use std::process::ExitCode;

#[test]
fn triple_add_maps_every_optional_flag() {
    let s = state();
    let (code, out, err) = run_write_state(
        "triple add",
        &[
            "a",
            "likes",
            "b",
            "--epistemic-kind",
            "belief",
            "--source-type",
            "user_stated",
            "--confidence",
            "0.8",
            "--valid-from",
            TS,
            "--valid-until",
            "2025-01-01T00:00:00Z",
            "--supersede",
            "true",
            "--subject-id",
            "subj-1",
            "--update-rule",
            "discrete",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["decision"]["pass"], json!(true), "{payload}");
}

#[test]
fn triple_add_rejects_bad_supersede_choice() {
    let s = state();
    let (code, _out, err) = run_write_state(
        "triple add",
        &["a", "likes", "b", "--supersede", "maybe"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2));
    assert!(err.contains("--supersede"), "{err}");
}

#[test]
fn triple_end_requires_two_positionals() {
    let s = state();
    let (code, _out, err) = run_write_state("triple end", &["only-one"], &s, false);
    assert_eq!(code, ExitCode::from(2));
    assert!(
        err.contains("SUBJECT") && err.contains("PREDICATE"),
        "{err}"
    );
}

#[test]
fn triple_end_dry_run_maps_object_and_update_rule() {
    let s = state();
    let (code, out, err) = run_write_state(
        "triple end",
        &[
            "a",
            "b",
            "--object",
            "c",
            "--valid-until",
            "2025-01-01T00:00:00Z",
            "--update-rule",
            "discrete",
            "--dry-run",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(payload["arguments"]["object"], json!("c"), "{payload}");
    assert_eq!(
        payload["arguments"]["update_rule"],
        json!("discrete"),
        "{payload}"
    );
}

#[test]
fn belief_observe_maps_as_of_and_subject_id() {
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
            "--as-of",
            TS,
            "--subject-id",
            "subj-1",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["outcome"], json!("created"), "{payload}");
}

#[test]
fn graph_link_requires_two_positionals() {
    let (s, source, _target) = link_state();
    let (code, _out, err) = run_write_state("graph link", &[&source], &s, false);
    assert_eq!(code, ExitCode::from(2));
    assert!(err.contains("SOURCE") && err.contains("TARGET"), "{err}");
}

#[test]
fn graph_link_maps_weight() {
    let (s, source, target) = link_state();
    let (code, out, err) = run_write_state(
        "graph link",
        &[
            &source,
            &target,
            "--relationship",
            "relates_to",
            "--weight",
            "0.5",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert!(payload["edge_id"].as_str().is_some(), "{payload}");
}

#[test]
fn render_fallbacks_are_deterministic() {
    assert_eq!(
        super::render_triple_add(&json!({"ok": true})),
        "edge accepted"
    );
    assert_eq!(
        super::render_belief(&json!({"ok": true})),
        "belief observed"
    );
}

#[test]
fn triple_add_rejects_an_empty_context() {
    let s = state();
    let (code, _out, err) = run_write_state(
        "triple add",
        &["a", "likes", "b", "--context", "", "--dry-run"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("--context must not be empty"), "{err}");
}

#[test]
fn triple_add_rejects_an_oversized_context_without_echoing_it() {
    let s = state();
    let oversized = "x".repeat(clio_types::CONTEXT_MAX_BYTES + 1);
    let (code, _out, err) = run_write_state(
        "triple add",
        &["a", "likes", "b", "--context", &oversized, "--dry-run"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{err}");
    assert!(err.contains("at most 4096 UTF-8 bytes"), "{err}");
    assert!(
        !err.contains(&oversized),
        "error must not echo the raw value"
    );
}

#[test]
fn triple_add_accepts_a_context_at_the_byte_maximum() {
    let s = state();
    let max = "x".repeat(clio_types::CONTEXT_MAX_BYTES);
    let (code, out, err) = run_write_state(
        "triple add",
        &["a", "likes", "b", "--context", &max, "--dry-run"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(
        payload["arguments"]["context"]["present"],
        json!(true),
        "{payload}"
    );
    assert_eq!(
        payload["arguments"]["context"]["bytes"],
        json!(clio_types::CONTEXT_MAX_BYTES),
        "{payload}"
    );
}

#[test]
fn belief_dry_run_redacts_a_secret_shaped_context() {
    let s = state();
    let args = [
        "Postgres handles the writes",
        "--confidence",
        "0.7",
        "--evidence",
        "e",
        "--source",
        "user_stated",
        "--context",
        "api_key: supersecret",
        "--dry-run",
        "--bank",
        "demo",
    ];
    let (code, out, err) = run_write_state("belief observe", &args, &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert!(
        !payload.to_string().contains("supersecret"),
        "json preview leaks the raw context: {payload}"
    );
    assert_eq!(payload["arguments"]["context"]["present"], json!(true));
    assert_eq!(payload["arguments"]["context"]["bytes"], json!(20));
    let (code, text, err) = run_write_state("belief observe", &args, &s, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(!text.contains("supersecret"), "{text}");
    assert!(!err.contains("supersecret"), "{err}");
}
