// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio` reviewer `validate` command: the attest and
//! update routes, the tool-side non-empty-note requirement, and collaborative
//! ownership preserved across corrections.
//!
//! ## Owns
//! - Behavior tests for the `validate` verb of `cli_write_shared`
//!
//! ## Does not own
//! - The command under test (`cli_write_shared`)
//! - Reviewer and correction semantics (see `clio-mcp` / `clio-compliance`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{TS, call, json_of, state};
use crate::cli_write_test_support::run_write_state;
use serde_json::json;
use std::process::ExitCode;

/// Seed one item in `demo` with original-author provenance for reviewer tests.
fn seed_review_item(s: &clio_mcp::runtime::McpState, id: &str, gist: &str) {
    let stored = call(
        s,
        "store",
        json!({
            "item": {"id": id, "subject_id": "s", "gist": gist},
            "category": "persona", "epistemic_kind": "fact",
            "evidence_ref": "ev-author",
            "bank": "demo", "actor": "agent", "timestamp": TS
        }),
    );
    assert_eq!(stored["ok"], json!(true), "{stored}");
}

#[test]
fn validate_attest_reports_applied_false_without_content_change() {
    let s = state();
    seed_review_item(&s, "itm-v", "prefers terse answers");
    let (code, out, err) = run_write_state(
        "validate",
        &["itm-v", "--action", "attest", "--bank", "demo"],
        &s,
        true,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert_eq!(
        out.trim(),
        "validated itm-v action=attest applied=false in bank `demo`",
        "{out}"
    );
}

#[test]
fn validate_update_applies_and_preserves_the_original_author() {
    // `update` routes through the auditable correction: provenance (the
    // original author half) is carried over untouched while the gist is
    // replaced (see clio-compliance correct.rs).
    let s = state();
    seed_review_item(&s, "itm-u", "meeting at 2pm");
    let (code, out, err) = run_write_state(
        "validate",
        &[
            "itm-u",
            "--action",
            "update",
            "--note",
            "reviewer-corrected time",
            "--new-content",
            "meeting at 3pm",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    assert_eq!(payload["item_id"], json!("itm-u"), "{payload}");
    assert_eq!(payload["applied"], json!(true), "{payload}");
    // The corrected revision carries the ORIGINAL authorship (provenance
    // `source_ref`) and the create-time admission score untouched (a
    // correction never re-scores), while the gist is replaced; the prior
    // revision stays queryable, closed on both axes.
    let trail = s.store.audit_trail("demo", "itm-u").expect("trail");
    let prior_score = trail
        .revisions
        .iter()
        .find(|rev| rev.item_id == "itm-u")
        .and_then(|rev| rev.admission_score)
        .expect("prior score");
    let corrected_id = trail
        .revisions
        .iter()
        .find(|rev| rev.corrects_item_id == Some("itm-u".into()))
        .map(|rev| rev.item_id.clone())
        .expect("corrected revision");
    let corrected = s
        .store
        .get_memory_item("demo", &corrected_id)
        .expect("readable")
        .expect("corrected revision exists");
    assert_eq!(
        corrected.source_ref.as_deref(),
        Some("ev-author"),
        "the original author's provenance must be carried over untouched"
    );
    assert_eq!(corrected.admission_score, Some(prior_score));
    assert_eq!(corrected.gist.as_deref(), Some("meeting at 3pm"));
    let prior = s
        .store
        .get_memory_item("demo", "itm-u")
        .expect("readable")
        .expect("prior exists");
    assert!(prior.tx_until.is_some(), "prior closed on the tx axis");
}

#[test]
fn validate_update_without_note_fails_at_the_tool_with_exit_one() {
    let s = state();
    seed_review_item(&s, "itm-n", "prefers terse answers");
    let (code, out, err) = run_write_state(
        "validate",
        &["itm-n", "--action", "update", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(err.contains("note"), "{err}");
}

#[test]
fn validate_requires_an_action() {
    let s = state();
    seed_review_item(&s, "itm-va", "prefers terse answers");
    let (code, out, err) = run_write_state("validate", &["itm-va", "--bank", "demo"], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--action"), "{err}");
}

#[test]
fn validate_rejects_unknown_action_with_exit_two() {
    let s = state();
    seed_review_item(&s, "itm-a", "prefers terse answers");
    let (code, out, err) = run_write_state(
        "validate",
        &["itm-a", "--action", "bogus", "--bank", "demo"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--action"), "{err}");
}
