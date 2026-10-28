// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the text-mode views of the core write commands: what
//! `remember`, `admit`, and the batch preview print when the output is text,
//! and the deterministic fallbacks of the render helpers.
//!
//! ## Owns
//! - Text-mode and render-view tests for `cli_write_core`
//!
//! ## Does not own
//! - The commands under test (`cli_write_core`)
//! - The JSON payload tests (`cli_write_core_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::state;
use crate::cli_write_test_support::run_write_state;
use serde_json::json;
use std::process::ExitCode;

#[test]
fn text_mode_remember_prints_the_stored_id() {
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &[
            "prefers aisle seats",
            "--category",
            "persona",
            "--bank",
            "demo",
        ],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("stored itm-cli-"), "{out}");
    assert!(out.contains("admission_score"), "{out}");
}

#[test]
fn text_mode_remember_dry_run_renders_the_preview_not_a_write() {
    // The dry-run payload is a read-only admission decision; the text view
    // must show pass/score/factors and never claim something was stored.
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &[
            "prefers aisle seats",
            "--category",
            "persona",
            "--bank",
            "demo",
            "--dry-run",
        ],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("preview:"), "{out}");
    assert!(out.contains("score:"), "{out}");
    assert!(out.contains("factors:"), "{out}");
    assert!(
        !out.contains("stored"),
        "dry-run must not claim a write: {out}"
    );
    assert!(s.leaves.list_leaves("demo").is_empty(), "{out}");
}

#[test]
fn render_views_fall_back_deterministically() {
    // The fallback arms keep text output stable when an optional field is
    // absent from a tool payload.
    assert_eq!(
        super::render_stored(&json!({"id": "itm-1"})),
        "stored itm-1"
    );
    assert_eq!(super::render_preview(&json!({})), "preview");
    assert_eq!(
        super::render_preview(&json!({"pass": false, "rejection_reason": "nope"})),
        "preview: refused\nreason: nope"
    );
    assert_eq!(
        super::render_job(&json!({"job_id": "j-1", "status": "accepted"})),
        "job j-1 status: accepted"
    );
    assert_eq!(
        super::render_batch_preview(&json!({
            "count": 2,
            "summary": {"would_admit": 1, "would_reject": 1},
            "writes": 0
        })),
        "preview: 2 items (would_admit: 1, would_reject: 1, writes: 0)"
    );
    // Missing optional fields fall back to zero counts, never a write claim.
    assert_eq!(
        super::render_batch_preview(&json!({})),
        "preview: 0 items (would_admit: 0, would_reject: 0, writes: 0)"
    );
}
