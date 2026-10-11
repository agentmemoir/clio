// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0

use super::{json_of, run_write_state, state};
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
