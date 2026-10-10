// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `clio batch` command: file/stdin decoding
//! (fail-closed before any dispatcher call), committed and dry-run runs,
//! atomicity on rejected operations, and the missing-file usage gate.
//!
//! ## Owns
//! - Behavior, golden, and `parse_operations` unit tests for `cli_write_batch`
//!
//! ## Does not own
//! - The command under test (`cli_write_batch`)
//! - Batch pre-flight, `$ref` resolution, or atomicity (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{call, json_of, seed_item, state};
use crate::cli_write_group::WriteGroup;
use crate::cli_write_test_support::{run_write_resolved, run_write_state, run_write_state_stdin};
use serde_json::{Value, json};
use std::io::Cursor;
use std::process::ExitCode;

fn store_op(id: &str, gist: &str) -> Value {
    json!({
        "tool": "store",
        "params": {
            "item": {"id": id, "subject_id": format!("s-{id}"), "gist": gist},
            "category": "persona",
            "epistemic_kind": "fact"
        }
    })
}

fn ops_file(name: &str, content: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("clio-batch-{name}-{}.json", std::process::id()));
    std::fs::write(&path, content).expect("write ops file");
    path
}

fn bank_count(s: &clio_mcp::runtime::McpState, bank: &str) -> u64 {
    let report = call(s, "stats", json!({"bank": bank, "actor": "agent"}));
    report["items_total"].as_u64().unwrap_or(u64::MAX)
}

#[test]
fn batch_commits_every_operation_from_the_file() {
    let s = state();
    let path = ops_file(
        "commit",
        &json!({
            "operations": [
                store_op("itm-b1", "prefers terse answers"),
                store_op("itm-b2", "prefers aisle seats")
            ]
        })
        .to_string(),
    );
    let arg = format!("--file={}", path.display());
    let (code, out, err) = run_write_state("batch", &[&arg], &s, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("batch: 2 operations committed"), "{out}");
    assert_eq!(bank_count(&s, "default"), 2, "both ops must commit");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn batch_accepts_a_bare_top_level_operations_array() {
    let s = state();
    let path = ops_file(
        "bare",
        &json!([store_op("itm-bare", "prefers terse answers")]).to_string(),
    );
    let arg = format!("--file={}", path.display());
    let (code, out, err) = run_write_state("batch", &[&arg], &s, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("batch: 1 operations committed"), "{out}");
    assert_eq!(bank_count(&s, "default"), 1);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn batch_dry_run_writes_nothing() {
    let s = state();
    let path = ops_file(
        "dry",
        &json!({"operations": [store_op("itm-d1", "prefers terse answers")]}).to_string(),
    );
    let arg = format!("--file={}", path.display());
    let (code, out, err) = run_write_state("batch", &[&arg, "--dry-run"], &s, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(
        out.contains("batch dry-run: 1 operations would commit (zero writes)"),
        "{out}"
    );
    assert_eq!(bank_count(&s, "default"), 0, "dry-run must write nothing");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn batch_malformed_json_fails_closed_with_exit_two() {
    let s = state();
    let path = ops_file("malformed", "{not json");
    let arg = format!("--file={}", path.display());
    let (code, out, err) = run_write_state("batch", &[&arg], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--file"), "{err}");
    assert_eq!(bank_count(&s, "default"), 0, "fail closed: zero writes");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn batch_object_without_operations_array_fails_closed() {
    let s = state();
    let path = ops_file(
        "nokey",
        &json!({"items": [store_op("itm-x", "gist")]}).to_string(),
    );
    let arg = format!("--file={}", path.display());
    let (code, out, err) = run_write_state("batch", &[&arg], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("operations"), "{err}");
    assert_eq!(bank_count(&s, "default"), 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn batch_atomicity_rejects_the_whole_batch_on_one_bad_op() {
    let s = state();
    seed_item(&s, "default", "itm-seed", "prefers terse answers");
    // The SECOND op forces rejection (unrecognized tool): the first op's item
    // must never appear — the batch tool owns all-or-nothing commit.
    let path = ops_file(
        "atomic",
        &json!({
            "operations": [
                store_op("itm-first", "prefers aisle seats"),
                {"tool": "totally_unknown", "params": {}}
            ]
        })
        .to_string(),
    );
    let arg = format!("--file={}", path.display());
    let (code, out, err) = run_write_state("batch", &[&arg], &s, false);
    assert_eq!(code, ExitCode::from(1), "{out}{err}");
    assert!(
        s.store
            .get_memory_item("default", "itm-first")
            .unwrap()
            .is_none(),
        "the first op must roll back with the batch"
    );
    assert_eq!(bank_count(&s, "default"), 1, "only the seeded item remains");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn batch_reads_operations_from_stdin_with_dash() {
    let s = state();
    let document = json!({
        "operations": [
            store_op("itm-s1", "prefers terse answers"),
            store_op("itm-s2", "prefers aisle seats")
        ]
    })
    .to_string();
    let mut reader = Cursor::new(document.into_bytes());
    let (code, out, err) = run_write_state_stdin("batch", &["--file", "-"], &mut reader, &s, true);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("batch: 2 operations committed"), "{out}");
    assert_eq!(bank_count(&s, "default"), 2);
}

#[test]
fn batch_requires_the_file_flag() {
    let s = state();
    let (code, out, err) = run_write_state("batch", &[], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("try 'clio batch --file ops.json'"), "{err}");
}

#[test]
fn batch_unreadable_file_names_the_path() {
    let s = state();
    let (code, out, err) =
        run_write_state("batch", &["--file", "/nonexistent/ops.json"], &s, false);
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("/nonexistent/ops.json"), "{err}");
}

#[test]
fn batch_json_output_passes_the_payload_through() {
    let s = state();
    let path = ops_file(
        "json",
        &json!({"operations": [store_op("itm-j1", "prefers terse answers")]}).to_string(),
    );
    let arg = format!("--file={}", path.display());
    let (code, out, err) = run_write_state("batch", &[&arg], &s, false);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    let payload = json_of(&out);
    let ops = payload["operations"].as_array().expect("op reports");
    assert_eq!(ops.len(), 1, "{payload}");
    assert_eq!(ops[0]["status"], json!("COMMITTED"), "{payload}");
    assert!(ops[0]["id"].as_str().is_some(), "{payload}");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn batch_help_exits_zero_before_opening_a_store() {
    let (code, out, err) = run_write_resolved("batch", &["--help"]);
    assert_eq!(code, ExitCode::SUCCESS, "{err}");
    assert!(out.contains("usage: clio batch --file ops.json"), "{out}");
    assert!(out.contains("`batch` tool"), "{out}");
}

// ---------- parse_operations unit tests ----------

#[test]
fn build_without_streams_rejects_the_stdin_path() {
    // The engine always injects streams; the bare `build` path exists for the
    // trait default and must fail closed rather than read nothing.
    let parsed = crate::cli_args::parse(
        &super::BatchWriteGroup.spec("batch"),
        ["--file".to_owned(), "-".to_owned()],
    )
    .expect("parse");
    let Err(error) = super::BatchWriteGroup.build("batch", &parsed) else {
        panic!("no stdin on this path")
    };
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(error.message().contains("stdin"), "{}", error.message());
}

#[test]
fn parse_operations_accepts_both_document_shapes() {
    let object = r#"{"operations": [{"tool": "store", "params": {}}]}"#;
    assert_eq!(super::parse_operations(object).unwrap().len(), 1);
    let bare = r#"[{"tool": "store", "params": {}}, {"tool": "store", "params": {}}]"#;
    assert_eq!(super::parse_operations(bare).unwrap().len(), 2);
}

#[test]
fn parse_operations_passes_empty_arrays_through_to_the_tool() {
    assert_eq!(super::parse_operations("[]").unwrap().len(), 0);
    assert_eq!(
        super::parse_operations(r#"{"operations": []}"#)
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn parse_operations_rejects_everything_else_with_usage_errors() {
    for (raw, fragment) in [
        ("{not json", "--file"),
        (r#"{"items": []}"#, "operations"),
        (r#"{"operations": "two"}"#, "array"),
        (r#"{"operations": 3}"#, "array"),
        ("\"just a string\"", "array"),
        ("42", "array"),
        ("null", "array"),
    ] {
        let error = super::parse_operations(raw).expect_err(raw);
        assert_eq!(error.exit_code(), ExitCode::from(2), "{raw}");
        assert!(error.message().contains(fragment), "{}", error.message());
    }
}
