// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the history read commands' (`task history`,
//! `failure list`, `memtree query`) CLI page-limit contract end to end: a
//! limit above the 200 cap is served at the cap with the requested value
//! disclosed in both output forms, and a non-positive limit is refused at the
//! CLI layer before the tool.
//!
//! ## Owns
//! - The task/failure/MemTree clamp-and-disclose behavior over `run_state` and
//!   over the built tool arguments
//!
//! ## Does not own
//! - The bounded-read policy's own bounds (`clio-types` `HistoryPolicy` tests)
//! - The shared `page_limit` clamp (`cli_read_args_page_limit_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the history read commands' CLI page-limit
//! contract.

use super::HistoryGroup;
use crate::cli_read_group::ReadGroup;
use crate::cli_read_test_support::{
    json_of, run_state, seed_failure, seed_item, seed_task, state, tokens,
};
use serde_json::{Value, json};
use std::process::ExitCode;
use std::sync::Arc;

fn history_state() -> Arc<clio_mcp::runtime::McpState> {
    let s = state();
    seed_task(&s, "bank-a", "t1", "Run tests");
    seed_task(&s, "bank-a", "t1", "Run tests again");
    seed_task(&s, "bank-a", "t1", "Run tests third");
    seed_failure(&s, "bank-a", "t1", 1, "retry once");
    seed_failure(&s, "bank-a", "t1", 2, "retry twice");
    seed_item(&s, "bank-a", "itm-1", "terse answers");
    s
}

/// The tool arguments the CLI parse path builds for one verb.
fn built_args(verb: &str, args: &[&str]) -> Value {
    let parsed = crate::cli_args::parse(&HistoryGroup.spec(verb), tokens(args)).expect("parse");
    let (_, built) = HistoryGroup.build(verb, &parsed).expect("build");
    built
}

/// `task history --limit 500` is served at the 200 cap, and both output forms
/// name the request the CLI lowered.
#[test]
fn task_history_serves_the_cap_for_a_limit_above_it_and_discloses_the_request() {
    let s = history_state();
    let args = ["t1", "--bank", "bank-a", "--limit", "500"];
    let (code, out, _) = run_state("task history", &args, &s, false);
    assert_eq!(code, ExitCode::from(0), "{out}");
    let page = json_of(&out);
    assert_eq!(page["limit"], json!(200), "{page}");
    assert_eq!(page["requested_limit"], json!(500), "{page}");

    let (_, text, _) = run_state("task history", &args, &s, true);
    assert!(
        text.contains("records: 3 of 200 (lowered from 500)"),
        "{text}"
    );
}

/// `task history --limit 0` is refused at the CLI layer as a usage error,
/// before the policy is asked for a page of nothing.
#[test]
fn task_history_refuses_a_zero_limit_at_the_cli_layer() {
    let s = history_state();
    let (code, out, err) = run_state(
        "task history",
        &["t1", "--bank", "bank-a", "--limit", "0"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("positive page size"), "{err}");
}

/// `failure list --limit 500` is served at the 200 cap, and both output forms
/// name the request the CLI lowered.
#[test]
fn failure_list_serves_the_cap_for_a_limit_above_it_and_discloses_the_request() {
    let s = history_state();
    let args = ["t1", "--bank", "bank-a", "--limit", "500"];
    let (code, out, _) = run_state("failure list", &args, &s, false);
    assert_eq!(code, ExitCode::from(0), "{out}");
    let page = json_of(&out);
    assert_eq!(page["limit"], json!(200), "{page}");
    assert_eq!(page["requested_limit"], json!(500), "{page}");

    let (_, text, _) = run_state("failure list", &args, &s, true);
    assert!(
        text.contains("failures: 2 of 200 (lowered from 500)"),
        "{text}"
    );
}

/// `failure list --limit 0` is refused at the CLI layer as a usage error,
/// before the policy is asked for a page of nothing.
#[test]
fn failure_list_refuses_a_zero_limit_at_the_cli_layer() {
    let s = history_state();
    let (code, out, err) = run_state(
        "failure list",
        &["t1", "--bank", "bank-a", "--limit", "0"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("positive page size"), "{err}");
}

/// `memtree query --limit 500` discloses the lowering, and the walk is asked
/// for the served cap, not the typed 500.
#[test]
fn memtree_query_serves_the_cap_for_a_limit_above_it_and_discloses_the_request() {
    let s = history_state();
    let args = ["--bank", "bank-a", "--limit", "500"];
    let (code, out, _) = run_state("memtree query", &args, &s, false);
    assert_eq!(code, ExitCode::from(0), "{out}");
    let page = json_of(&out);
    assert_eq!(page["requested_limit"], json!(500), "{page}");
    assert_eq!(page["truncated"], json!(false), "{page}");

    let (_, text, _) = run_state("memtree query", &args, &s, true);
    assert!(
        text.contains("nodes: 2 (limit 200, lowered from 500)"),
        "{text}"
    );

    assert_eq!(built_args("memtree query", &args)["limit"], json!(200));
}

/// `memtree query --limit 0` is refused at the CLI layer as a usage error,
/// before the walk is asked for a page of nothing.
#[test]
fn memtree_query_refuses_a_zero_limit_at_the_cli_layer() {
    let s = history_state();
    let (code, out, err) = run_state(
        "memtree query",
        &["--bank", "bank-a", "--limit", "0"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("positive page size"), "{err}");
}
