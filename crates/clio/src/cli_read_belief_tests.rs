// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the triple and belief read commands (`triple query`,
//! `belief history`) through the in-process dispatcher.
//!
//! ## Owns
//! - Assertions over the belief group's builds and text views
//!
//! ## Does not own
//! - The command implementation under test (`cli_read_belief`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{BeliefGroup, render_belief, render_triples};
use crate::cli_read_group::ReadGroup;
use crate::cli_read_test_support::{json_of, run_state, seed_belief, seed_triple, state};
use clio_mcp::runtime::McpState;
use serde_json::json;
use std::process::ExitCode;
use std::sync::Arc;

fn belief_state() -> Arc<McpState> {
    let s = state();
    seed_triple(&s, "bank-a", "rust", "used_by", "agent");
    seed_triple(&s, "bank-a", "rust", "used_by", "tool");
    seed_triple(&s, "bank-a", "python", "used_by", "agent");
    seed_belief(&s, "bank-a", "Friday deploys are risky", 0.7);
    s
}

#[test]
fn triple_query_positionals_flags_and_text() {
    let s = belief_state();
    let (code, out, err) = run_state("triple query", &["rust", "--bank", "bank-a"], &s, false);
    assert_eq!(code, ExitCode::from(0));
    assert_eq!(json_of(&out)["triples"].as_array().map(Vec::len), Some(1));
    assert!(err.is_empty(), "{err}");

    let (_, out, _) = run_state(
        "triple query",
        &["rust", "used_by", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(json_of(&out)["triples"].as_array().map(Vec::len), Some(1));

    let (_, out, _) = run_state(
        "triple query",
        &[
            "--subject",
            "python",
            "--predicate",
            "used_by",
            "--object",
            "agent",
            "--bank",
            "bank-a",
        ],
        &s,
        false,
    );
    assert_eq!(json_of(&out)["triples"].as_array().map(Vec::len), Some(1));

    let (_, out, _) = run_state("triple query", &["--bank", "bank-a"], &s, false);
    assert_eq!(json_of(&out)["triples"].as_array().map(Vec::len), Some(2));

    let (_, out, _) = run_state("triple query", &["rust", "--bank", "bank-a"], &s, true);
    assert!(out.contains("triples: 1"), "{out}");
    assert!(out.contains("rust used_by tool"), "{out}");

    assert_eq!(render_triples(&json!({})), "triples: 0");
}

#[test]
fn triple_query_usage_errors() {
    let s = belief_state();
    let cases: &[(&[&str], &str)] = &[
        (&["a", "b", "c", "d"], "unexpected extra"),
        (&["--time-axis", "bogus"], "--time-axis"),
    ];
    for &(args, needle) in cases {
        let (code, out, err) = run_state("triple query", args, &s, false);
        assert_eq!(code, ExitCode::from(2), "{args:?}");
        assert!(out.is_empty(), "{args:?}: {out}");
        assert!(err.contains(needle), "{args:?}: {err}");
    }
}

#[test]
fn belief_history_success_missing_and_text() {
    let s = belief_state();
    let (code, out, _) = run_state(
        "belief history",
        &["Friday deploys are risky", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["proposition"], json!("Friday deploys are risky"));
    let entries = value["entries"].as_array().expect("entries");
    assert!(!entries.is_empty(), "{entries:?}");

    let (_, out, _) = run_state(
        "belief history",
        &["Friday deploys are risky", "--bank", "bank-a"],
        &s,
        true,
    );
    assert!(
        out.contains("proposition: Friday deploys are risky"),
        "{out}"
    );

    let (code, _, _) = run_state("belief history", &["ghost", "--bank", "bank-a"], &s, false);
    assert_eq!(code, ExitCode::from(1));

    assert_eq!(
        render_belief(&json!({"belief_id": "b1", "entries": [], "current_confidence": 0.5})),
        "belief_id: b1\nentries: 0\ncurrent_confidence: 0.5"
    );
    assert_eq!(render_belief(&json!({})), "belief_id: ?\nentries: 0");
}

#[test]
fn belief_history_requires_a_positional() {
    let s = belief_state();
    let (code, out, err) = run_state("belief history", &["--bank", "bank-a"], &s, false);
    assert_eq!(code, ExitCode::from(2));
    assert!(out.is_empty(), "{out:?}");
    assert!(err.contains("requires a BELIEF_ID_OR_PROPOSITION"), "{err}");
}

#[test]
fn spec_covers_every_verb() {
    let group = BeliefGroup;
    for verb in group.verbs() {
        assert!(group.spec(verb).bool_flags().contains(&"explain"), "{verb}");
    }
    let spec = group.spec("triple query");
    assert!(spec.value_flags().contains(&"subject"));
    assert!(spec.value_flags().contains(&"time-axis"));
}
