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

/// The consolidate dry-run validates the bank the caller named, exactly like
/// the summarize dry-run: same exit code, same `not_found` code and message.
#[test]
fn consolidate_dry_run_rejects_an_unknown_bank() {
    let (code, out, err) = run_write_open(
        "consolidate",
        &[
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
    let payload = json_of(&err);
    assert_eq!(payload["code"], json!("not_found"), "{payload}");
    assert_eq!(
        payload["message"],
        json!("--bank `nope` not found"),
        "{payload}"
    );
}

/// A name that is not a valid bank name at all is refused the same way: it
/// names no bank the store holds, so the check cannot pass.
#[test]
fn consolidate_dry_run_rejects_an_invalid_bank_name() {
    let (code, out, err) = run_write_open(
        "consolidate",
        &[
            "--dry-run",
            "--bank",
            "bad bank!",
            "--db",
            "sqlite::memory:",
            "--output",
            "json",
        ],
        false,
    );
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(out.is_empty(), "stdout: {out}");
    let payload = json_of(&err);
    assert_eq!(payload["code"], json!("not_found"), "{payload}");
    assert_eq!(
        payload["message"],
        json!("--bank `bad bank!` not found"),
        "{payload}"
    );
}

/// The two dry runs report one missing bank identically: a script checking the
/// exit code gets the same answer from either command.
#[test]
fn consolidate_and_summarize_dry_runs_refuse_a_bank_identically() {
    let summarize = run_write_open(
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
    let consolidate = run_write_open(
        "consolidate",
        &[
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
    assert_eq!(summarize.0, consolidate.0, "{summarize:?}{consolidate:?}");
    assert_eq!(summarize.1, consolidate.1, "{summarize:?}{consolidate:?}");
    let payload = json_of(&consolidate.2);
    assert_eq!(payload["code"], json!("not_found"), "{payload}");
    assert_eq!(payload["ok"], json!(false), "{payload}");
}

/// The consolidate dry run keeps its own behavior: a valid bank scope still
/// previews. The summarize twin of this test is the engine-level
/// `summarize_bank_dry_run_previews_without_calling_the_dispatcher`; this
/// opening-path variant cannot inject an extract config for summarize, so
/// only the consolidate case (which needs no extractor) lives here.
#[test]
fn consolidate_dry_run_still_previews_for_the_default_bank() {
    let (code, out, err) = run_write_open(
        "consolidate",
        &["--dry-run", "--db", "sqlite::memory:", "--output", "json"],
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["dry_run"], json!(true), "{payload}");
    assert_eq!(payload["bank"], json!(null), "{payload}");
}
