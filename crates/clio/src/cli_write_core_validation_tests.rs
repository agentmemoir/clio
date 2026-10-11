// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

use super::{json_of, run_write_state, state};
use crate::cli_write_test_support::run_write_open;
use serde_json::json;
use std::process::ExitCode;

#[test]
fn summarize_dry_run_rejects_unknown_item_scope() {
    let state = state();
    let (code, out, err) = run_write_state(
        "summarize",
        &["--scope", "missing-item", "--dry-run"],
        &state,
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(out.is_empty(), "stdout: {out}");
    let payload = json_of(&err);
    assert_eq!(payload["code"], json!("item_not_found"), "{payload}");
}

/// The summarize dry-run validates the bank the caller named. The runtime
/// default must not be the caller's `--bank`, or the check is exempted and an
/// unknown bank previews as if it existed.
#[test]
fn summarize_dry_run_rejects_an_unknown_bank() {
    let (code, out, err) = run_write_open(
        "summarize",
        &[
            "--scope",
            "bank",
            "--dry-run",
            "--bank",
            "nope",
            "--db",
            "sqlite::memory:",
            "--output",
            "json",
        ],
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(out.is_empty(), "stdout: {out}");
    assert_eq!(json_of(&err)["code"], json!("not_found"), "{err}");
}

#[test]
fn remember_rejects_source_text_without_snapshot_support() {
    let state = state();
    let (code, out, err) = run_write_state(
        "remember",
        &[
            "text",
            "--category",
            "persona",
            "--source-text",
            "unverified",
        ],
        &state,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--source-text"), "{err}");
}
