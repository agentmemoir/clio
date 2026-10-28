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

    assert_eq!(render_triples(&json!({})), "triples: 0 of 0");
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
fn triple_query_refuses_a_zero_limit_at_the_cli_layer() {
    let s = belief_state();
    let (code, out, err) = run_state(
        "triple query",
        &["--limit", "0", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2));
    assert!(out.is_empty(), "{out}");
    assert!(
        err.contains("--limit must be a positive page size, got `0`"),
        "{err}"
    );
}

#[test]
fn triple_query_serves_the_cap_for_a_limit_above_it_and_discloses_the_request() {
    let s = belief_state();
    let (code, out, _) = run_state(
        "triple query",
        &["--limit", "500", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["limit"], json!(200));
    assert_eq!(value["requested_limit"], json!(500));
    assert_eq!(value["total"], json!(2));
    // Two rows on a 200-row page: the page is not short, so no truncation.
    assert_eq!(value["truncated"], json!(false));

    let (_, out, _) = run_state(
        "triple query",
        &["--limit", "500", "--bank", "bank-a"],
        &s,
        true,
    );
    assert!(
        out.contains("triples: 2 of 2 (limit 200, lowered from 500)"),
        "{out}"
    );
}

#[test]
fn triple_query_offset_pages_over_a_seeded_store() {
    let s = belief_state();
    let (code, out, _) = run_state(
        "triple query",
        &["--limit", "1", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    // One row of two matches: `total` still names the full match count, and
    // the page is short, so it reads as truncated.
    assert_eq!(value["triples"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["total"], json!(2));
    assert_eq!(value["offset"], json!(0));
    assert_eq!(value["truncated"], json!(true));

    // One row skipped by the offset: the remaining row fills page two.
    let (code, out, _) = run_state(
        "triple query",
        &["--limit", "1", "--offset", "1", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["triples"].as_array().map(Vec::len), Some(1));
    assert_eq!(value["total"], json!(2));
    assert_eq!(value["truncated"], json!(false));

    // An offset past the end serves an empty page that is not truncated.
    let (code, out, _) = run_state(
        "triple query",
        &["--offset", "5", "--bank", "bank-a"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["triples"].as_array().map(Vec::len), Some(0));
    assert_eq!(value["total"], json!(2));
    assert_eq!(value["truncated"], json!(false));

    let (_, out, _) = run_state(
        "triple query",
        &["--limit", "1", "--bank", "bank-a"],
        &s,
        true,
    );
    assert!(out.contains("triples: 1 of 2"), "{out}");
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

/// Every verb's spec is its record's, and the accepted set that record carries is
/// the one the group's own list held: same flags, same order, globals included.
#[test]
fn spec_answers_from_the_registry_with_the_original_accepted_set() {
    let expected: &[(&str, &[&str], &[&str])] = &[
        (
            "triple query",
            &[
                "db",
                "backend",
                "bank",
                "actor",
                "output",
                "subject",
                "predicate",
                "object",
                "as-of",
                "time-axis",
            ],
            &["no-input", "yes", "explain"],
        ),
        (
            "belief history",
            &["db", "backend", "bank", "actor", "output"],
            &["no-input", "yes", "explain"],
        ),
    ];
    let group = BeliefGroup;
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

/// A name the group does not own gets the same empty spec the core group gives:
/// the globals and nothing else, so an unknown verb cannot smuggle a flag in.
#[test]
fn an_unknown_verb_gets_the_empty_spec() {
    let spec = BeliefGroup.spec("not-a-belief-verb");
    assert_eq!(
        spec.value_flags(),
        ["db", "backend", "bank", "actor", "output"]
    );
    assert_eq!(spec.bool_flags(), ["no-input", "yes"]);
}
