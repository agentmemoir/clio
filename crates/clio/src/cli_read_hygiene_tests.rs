// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the hygiene read commands (`hygiene audit`,
//! `hygiene log`): ranked-candidate output without secret leakage, bounded
//! pagination markers, flag validation, and the log rows after a confirmed
//! clean.
//!
//! ## Owns
//! - Behavior and text-view tests for the hygiene read verbs
//!
//! ## Does not own
//! - The command implementation under test (`cli_read_hygiene`)
//! - Audit scoring or clean semantics (see `clio-hygiene`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{HygieneReadGroup, render_audit, render_log};
use crate::cli_read_group::ReadGroup;
use crate::cli_read_test_support::{json_of, run_state, seed_item, state};
use crate::cli_write_test_support::run_write_state;
use serde_json::json;
use std::process::ExitCode;

/// A planted secret the inline scrubber rewrites (so the audit sees a secret
/// hit) but that must never appear in CLI output.
const SECRET: &str = "sk-abcdef0123456789abcdef";

/// One item whose gist carries the planted secret in bank `demo`.
fn secret_state() -> std::sync::Arc<clio_mcp::runtime::McpState> {
    let s = state();
    seed_item(
        &s,
        "demo",
        "itm-sec-1",
        &format!("stale note with api_key={SECRET}"),
    );
    s
}

/// Write a candidates JSON file under the process-unique temp name (same
/// pattern as the write suites).
fn candidates_file(name: &str, content: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "clio-hygiene-read-{name}-{}.json",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write candidates file");
    path
}

#[test]
fn audit_ranks_candidates_and_never_leaks_the_planted_secret() {
    let s = secret_state();
    let (code, out, err) = run_state(
        "hygiene audit",
        &["--bank", "demo", "--limit", "1"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    let candidates = payload["candidates"].as_array().expect("candidates");
    assert!(!candidates.is_empty(), "{payload}");
    assert!(
        candidates
            .iter()
            .any(|c| c["item_id"] == json!("itm-sec-1")),
        "{payload}"
    );
    assert!(
        candidates
            .windows(2)
            .all(|pair| pair[0]["noise_score"].as_f64() >= pair[1]["noise_score"].as_f64()),
        "candidates must be ranked descending: {payload}"
    );
    assert!(!out.contains(SECRET), "{out}");
    assert!(!err.contains(SECRET), "{err}");

    let (_, text, _) = run_state("hygiene audit", &["--bank", "demo"], &s, true);
    assert!(
        text.starts_with("hygiene audit (bank demo): scanned 1 active item, 1 candidate"),
        "{text}"
    );
    assert!(text.contains("  discard  itm-sec-1"), "{text}");
    assert!(text.contains("secret_credential"), "{text}");
    assert!(!text.contains("api_key"), "{text}");
}

#[test]
fn audit_on_an_empty_bank_reports_zero_candidates() {
    let s = state();
    let (code, out, err) = run_state("hygiene audit", &["--bank", "empty"], &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    assert_eq!(payload["scanned"], json!(0), "{payload}");
    assert_eq!(payload["total_candidates"], json!(0), "{payload}");
    assert_eq!(payload["candidates"].as_array().map(Vec::len), Some(0));
    let (_, text, _) = run_state("hygiene audit", &["--bank", "empty"], &s, true);
    assert!(
        text.contains("hygiene audit (bank empty): scanned 0 active items, 0 candidates"),
        "{text}"
    );
}

#[test]
fn audit_marks_the_page_truncated_when_more_candidates_exist() {
    let s = state();
    seed_item(&s, "demo", "itm-p1", "stale duplicated note one");
    seed_item(&s, "demo", "itm-p2", "stale duplicated note two");
    let (_, out, _) = run_state(
        "hygiene audit",
        &["--bank", "demo", "--min-score", "0", "--limit", "1"],
        &s,
        false,
    );
    let page = json_of(&out);
    assert_eq!(page["candidates"].as_array().map(Vec::len), Some(1));
    assert_eq!(page["truncated"], json!(true), "{page}");

    let (_, out, _) = run_state(
        "hygiene audit",
        &["--bank", "demo", "--min-score", "0"],
        &s,
        false,
    );
    let full = json_of(&out);
    assert_eq!(full["candidates"].as_array().map(Vec::len), Some(2));
    assert_eq!(full["truncated"], json!(false), "{full}");
}

#[test]
fn audit_rejects_invalid_numeric_flags_with_exit_two() {
    let s = state();
    for flag in ["--min-score", "--limit", "--offset"] {
        let (code, out, err) =
            run_state("hygiene audit", &["--bank", "demo", flag, "abc"], &s, false);
        assert_eq!(code, ExitCode::from(2), "{out}{err}");
        assert!(err.contains(flag), "{err}");
    }
    let (code, out, err) = run_state(
        "hygiene log",
        &["--bank", "demo", "--limit", "abc"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
}

#[test]
fn hygiene_log_renders_rows_after_a_confirmed_clean() {
    let s = state();
    seed_item(&s, "demo", "itm-log-1", "stale duplicated note");
    let file = candidates_file(
        "log",
        &json!({"candidates": [{"item_id": "itm-log-1", "noise_score": 0.9}]}).to_string(),
    );
    let file_flag = format!("--file={}", file.display());
    let (code, _, err) = run_write_state(
        "hygiene clean",
        &[
            &file_flag,
            "--action",
            "flag",
            "--confirm",
            "--bank",
            "demo",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let _ = std::fs::remove_file(&file);

    let (code, out, err) = run_state("hygiene log", &["--bank", "demo"], &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    let payload = json_of(&out);
    let rows = payload["rows"].as_array().expect("rows");
    assert!(
        rows.iter().any(|r| r["item_id"] == json!("itm-log-1")),
        "{payload}"
    );
    let (_, text, _) = run_state("hygiene log", &["--bank", "demo"], &s, true);
    assert!(text.starts_with("hygiene log: 1 entry\n"), "{text}");
    assert!(text.contains("flag"), "{text}");
    assert!(text.contains("itm-log-1"), "{text}");
}

#[test]
fn hygiene_log_on_a_fresh_bank_renders_zero_entries() {
    let s = state();
    let (code, out, err) = run_state("hygiene log", &["--bank", "demo"], &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{out}{err}");
    assert_eq!(json_of(&out)["count"], json!(0), "{out}");
    let (_, text, _) = run_state("hygiene log", &["--bank", "demo"], &s, true);
    assert_eq!(text.trim(), "hygiene log: 0 entries", "{text}");
}

// ---------- render and spec unit tests ----------

#[test]
fn render_audit_and_log_default_their_missing_fields() {
    assert_eq!(
        render_audit(&json!({})),
        "hygiene audit (bank ?): scanned 0 active items, 0 candidates"
    );
    assert_eq!(
        render_audit(&json!({
            "bank": "demo", "scanned": 1, "total_candidates": 1,
            "candidates": [{"item_id": "itm-1", "noise_score": 0.5}]
        })),
        "hygiene audit (bank demo): scanned 1 active item, 1 candidate\n  0.50  -  itm-1  "
    );
    assert_eq!(render_log(&json!({})), "hygiene log: 0 entries");
    assert_eq!(
        render_log(&json!({"rows": [{
            "timestamp": "2024-01-01T00:00:00Z", "bank": "demo",
            "action": "flag", "item_id": "itm-1"
        }]})),
        "hygiene log: 1 entry\n2024-01-01T00:00:00Z  demo  flag  itm-1"
    );
}

#[test]
fn audit_text_marks_a_partial_scan() {
    assert_eq!(
        render_audit(&json!({
            "bank": "big", "scanned": 5000, "total_candidates": 3,
            "incomplete": true, "candidates": []
        })),
        "hygiene audit (bank big): scanned 5000 active items, 3 candidates \
         (partial scan: bank exceeds the scan budget)"
    );
    // A complete scan carries no marker.
    assert_eq!(
        render_audit(&json!({
            "bank": "small", "scanned": 2, "total_candidates": 1,
            "incomplete": false, "candidates": []
        })),
        "hygiene audit (bank small): scanned 2 active items, 1 candidate"
    );
}

#[test]
fn hygiene_read_group_declares_its_verbs_and_specs() {
    assert_eq!(
        HygieneReadGroup.verbs(),
        &["hygiene audit", "hygiene log"][..]
    );
    let audit = HygieneReadGroup.spec("hygiene audit");
    for flag in ["min-score", "limit", "offset"] {
        assert!(audit.value_flags().contains(&flag), "{flag}");
    }
    let log = HygieneReadGroup.spec("hygiene log");
    assert!(log.value_flags().contains(&"limit"));
    assert!(!log.value_flags().contains(&"min-score"));
}

#[test]
fn audit_decorate_uses_the_offset_argument_and_skips_the_log() {
    use crate::cli_args::parse;
    let parsed = parse(
        &HygieneReadGroup.spec("hygiene audit"),
        ["--offset".to_owned(), "1".to_owned()],
    )
    .expect("parse");
    let mut page = json!({"total_candidates": 2, "candidates": [{}]});
    HygieneReadGroup.decorate("hygiene audit", &parsed, &mut page);
    assert_eq!(page["truncated"], json!(false));

    let mut untouched = json!({"total_candidates": 2, "candidates": [{}]});
    HygieneReadGroup.decorate("hygiene log", &parsed, &mut untouched);
    assert_eq!(untouched.get("truncated"), None);
}
