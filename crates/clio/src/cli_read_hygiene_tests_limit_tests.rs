// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `hygiene audit` CLI page-limit contract end to end:
//! a limit above the cap now succeeds at the cap with the requested value
//! disclosed in both output forms, and a non-positive limit is refused at the
//! CLI layer before the tool.
//!
//! ## Owns
//! - The `hygiene audit` clamp-and-disclose behavior over `run_state`
//!
//! ## Does not own
//! - The tool validator's own page bounds (`clio-hygiene` audit tests)
//! - The shared `page_limit` clamp (`cli_read_args_page_limit_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the audit's CLI page-limit contract.

use crate::cli_read_test_support::{json_of, run_state, seed_item, state};
use serde_json::json;
use std::process::ExitCode;

/// `--limit 500` succeeds (the CLI clamps it to the cap instead of the tool
/// validator refusing it) and both output forms disclose the lowering.
#[test]
fn audit_serves_the_cap_for_a_limit_above_it_and_discloses_the_request() {
    let s = state();
    seed_item(&s, "demo", "itm-cap-1", "stale duplicated note");
    let args = ["--bank", "demo", "--min-score", "0", "--limit", "500"];
    let (code, out, err) = run_state("hygiene audit", &args, &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["requested_limit"], json!(500), "{payload}");

    let (_, text, _) = run_state("hygiene audit", &args, &s, true);
    assert!(text.contains("(limit 200, lowered from 500)"), "{text}");
}

/// `--limit 0` is refused at the CLI layer as a usage error, before the tool
/// is asked for a page of nothing.
#[test]
fn audit_refuses_a_zero_limit_at_the_cli_layer() {
    let s = state();
    let (code, out, err) = run_state(
        "hygiene audit",
        &["--bank", "demo", "--limit", "0"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(out.is_empty(), "{out}");
    assert!(err.contains("positive page size"), "{err}");
}
