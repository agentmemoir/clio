// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the recall-path regression tests at the
//! CLI/MCP entry point: an unreadable candidate (missing or crypto-shredded
//! DEK) is skipped with a single warning instead of aborting the query, a
//! direct read of that id still fails, and text-mode recall shows hit content.
//!
//! ## Owns
//! - Assertions over `recall`/`get` for a shredded candidate through
//!   `run_with_state` (the same seam the CLI uses)
//!
//! ## Does not own
//! - The retrieval skip policy (`clio-retrieve::hybrid`) or the text renderer
//!   (`cli_read_render`), which are only exercised here
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::cli_read_test_support::{TS, call, json_of, run_state};
use clio_mcp::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::process::ExitCode;
use std::sync::Arc;

/// Store one admitted item under an explicit subject id and register its
/// lexical doc, so a test can shred exactly one candidate's DEK.
fn seed_subject(state: &McpState, bank: &str, id: &str, gist: &str, subject: &str) {
    let stored = call(
        state,
        "store",
        json!({
            "item": {
                "id": id,
                "subject_id": subject,
                "gist": gist,
                "snapshot": {"entity": gist, "amount": 1, "date": "2024-01-01"}
            },
            "category": "task_spec",
            "epistemic_kind": "fact",
            "source_text": format!("The user prefers {gist} (1 note since 2024-01-01)."),
            "bank": bank,
            "actor": "agent",
            "timestamp": TS
        }),
    );
    assert_eq!(stored["ok"], json!(true), "seed {id}: {stored}");
    state
        .index
        .put_lexical_doc(bank, id, gist)
        .expect("lexical doc");
}

/// In-memory state with one readable and one crypto-shredded candidate.
fn state_with_shredded_candidate() -> Arc<McpState> {
    let state = McpState::open(&McpOpenOptions::default()).expect("state");
    seed_subject(&state, "bank-a", "itm-ok", "alpha readable", "subj-ok");
    seed_subject(
        &state,
        "bank-a",
        "itm-shred",
        "alpha shredded",
        "subj-shred",
    );
    state.kms.destroy_dek("subj-shred").expect("destroy DEK");
    state
}

#[test]
fn recall_skips_exactly_one_unreadable_candidate() {
    let state = state_with_shredded_candidate();
    let (code, out, err) = run_state("recall", &["alpha", "--bank", "bank-a"], &state, false);
    assert_eq!(code, ExitCode::from(0));
    let value = json_of(&out);
    assert_eq!(value["ok"], json!(true), "payload: {value}");
    let hits = value["hits"].as_array().expect("hits");
    let ids: Vec<&str> = hits.iter().filter_map(|h| h["item_id"].as_str()).collect();
    assert!(ids.contains(&"itm-ok"), "hits: {ids:?}");
    assert!(!ids.contains(&"itm-shred"), "hits: {ids:?}");

    let warnings = value["warnings"].as_array().expect("warnings");
    let skips: Vec<&Value> = warnings
        .iter()
        .filter(|w| w.as_str().is_some_and(|s| s.contains("unreadable")))
        .collect();
    assert_eq!(skips.len(), 1, "warnings: {warnings:?}");
    let skip = skips[0].as_str().expect("warning string");
    assert!(skip.contains("itm-shred"), "warning: {skip}");
    assert!(err.is_empty(), "stderr: {err}");
}

#[test]
fn direct_get_of_unreadable_candidate_still_errors() {
    let state = state_with_shredded_candidate();
    let (code, out, err) = run_state("get", &["itm-shred", "--bank", "bank-a"], &state, false);
    assert_ne!(code, ExitCode::from(0));
    assert!(out.is_empty(), "stdout: {out}");
    let envelope = json_of(&err);
    assert_eq!(envelope["ok"], json!(false));

    // The readable sibling still reads fine.
    let (code, out, _) = run_state("get", &["itm-ok", "--bank", "bank-a"], &state, false);
    assert_eq!(code, ExitCode::from(0));
    assert_eq!(json_of(&out)["id"], json!("itm-ok"));
}

#[test]
fn text_mode_recall_shows_hit_content() {
    let state = state_with_shredded_candidate();
    let (code, out, _) = run_state("recall", &["alpha", "--bank", "bank-a"], &state, true);
    assert_eq!(code, ExitCode::from(0));
    assert!(out.contains("alpha readable"), "{out}");
    assert!(!out.contains("itm-shred"), "{out}");
}
