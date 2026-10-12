// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MCP-surface coverage of the `batch` tool's
//! in-batch duplicate gate: two operations that store the same text abort the
//! whole batch unless the later one sets `allow_duplicate`.
//!
//! ## Owns
//! - In-batch exact-duplicate abort and waiver assertions for `batch`
//!
//! ## Does not own
//! - Command-level bank/actor defaults (see `batch_tools_command_context_tests`)
//! - General batch orchestration assertions (see `batch_tools_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::batch_tools_test_support_tests::{batch, count_bank, state, store_op};
use serde_json::json;

#[test]
fn a_duplicate_of_an_earlier_store_op_aborts_the_batch() {
    let s = state();
    let out = batch(
        &s,
        json!([store_op("itm-dup-a", "dupe"), store_op("itm-dup-b", "dupe")]),
        false,
    );
    assert_eq!(out["ok"], json!(false), "{out}");
    let message = out["message"].as_str().unwrap_or("");
    assert!(message.contains("operation 1"), "{out}");
    assert!(message.contains("exact_duplicate"), "{out}");
    assert_eq!(
        count_bank(&s, "bank-a"),
        0,
        "an aborted batch leaves no rows"
    );
}

#[test]
fn allow_duplicate_waives_the_in_batch_duplicate_gate() {
    let s = state();
    let mut second = store_op("itm-dup-b", "dupe");
    second["params"]["allow_duplicate"] = json!(true);
    let out = batch(&s, json!([store_op("itm-dup-a", "dupe"), second]), false);
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(count_bank(&s, "bank-a"), 2);
}

#[test]
fn the_same_text_in_two_banks_commits_in_both() {
    let s = state();
    let op = |id: &str, bank: &str| {
        json!({
            "tool": "store",
            "params": {
                "item": {
                    "id": id,
                    "subject_id": format!("subj-{id}"),
                    "gist": "shared text across banks",
                    "snapshot": {"entity": "x", "amount": 1, "date": "2024-01-01"}
                },
                "category": "persona",
                "epistemic_kind": "fact",
                "source_text": "x (1 note since 2024-01-01).",
                "bank": bank,
                "actor": "agent"
            }
        })
    };
    let out = batch(
        &s,
        json!([op("itm-bank-a", "bank-a"), op("itm-bank-b", "bank-b")]),
        false,
    );
    assert_eq!(out["ok"], json!(true), "{out}");
    assert_eq!(count_bank(&s, "bank-a"), 1, "a duplicate is bank-scoped");
    assert_eq!(count_bank(&s, "bank-b"), 1);
}

#[test]
fn a_duplicate_of_a_preexisting_row_still_aborts() {
    let s = state();
    let seeded = batch(&s, json!([store_op("itm-seed", "seed")]), false);
    assert_eq!(seeded["ok"], json!(true), "{seeded}");
    let out = batch(&s, json!([store_op("itm-again", "seed")]), false);
    assert_eq!(out["ok"], json!(false), "{out}");
    assert!(
        out["message"]
            .as_str()
            .unwrap_or("")
            .contains("exact_duplicate"),
        "{out}"
    );
    assert_eq!(count_bank(&s, "bank-a"), 1);
}
