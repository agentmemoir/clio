// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the gist decoding of the store-path write: an absent gist
//! stays absent, and a blank gist is refused before anything is stored.
//!
//! ## Owns
//! - Assertions over `gist_of`
//!
//! ## Does not own
//! - The store path itself (see `store_write`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use serde_json::json;

#[test]
fn an_absent_gist_stays_absent() {
    assert_eq!(gist_of(&json!({})).expect("absent gist"), None);
}

#[test]
fn a_non_blank_gist_is_kept_as_given() {
    assert_eq!(
        gist_of(&json!({"gist": "Tom walks on Fridays"})).expect("gist"),
        Some("Tom walks on Fridays".to_owned())
    );
}

#[test]
fn an_empty_or_whitespace_only_gist_is_refused() {
    for blank in ["", "   ", "\t\n"] {
        let error = gist_of(&json!({ "gist": blank })).expect_err("blank gist");
        assert_eq!(error.code, ErrorCode::InvalidArgument, "{blank:?}");
        assert!(
            error
                .message
                .contains("must not be empty or whitespace-only"),
            "{blank:?}: {}",
            error.message
        );
    }
}

#[test]
fn an_item_needs_a_gist_or_a_snapshot() {
    let refused = require_content(&json!({"item": {"id": "itm-x"}})).expect_err("no content");
    assert_eq!(refused.code, ErrorCode::InvalidArgument);
    assert!(
        refused.message.contains("item.gist is required"),
        "{}",
        refused.message
    );
    assert!(
        require_content(&json!({})).is_err(),
        "a call with no item has no content"
    );
    assert!(require_content(&json!({"item": {"snapshot": {"amount": 1}}})).is_ok());
}

#[test]
fn a_repeated_store_is_refused_as_not_ok_and_a_dry_run_keeps_its_verdict() {
    use crate::runtime::McpOpenOptions;
    let s = McpState::open_with_effective(&McpOpenOptions::default(), &serde_json::json!({}))
        .expect("state");
    let args = json!({
        "item": {
            "id": "itm-dup-1", "subject_id": "subj-dup", "gist": "terse dup answers",
            "snapshot": {"entity": "terse dup answers", "amount": 1, "date": "2024-01-01"}
        },
        "category": "persona", "epistemic_kind": "fact", "bank": "bank-a", "actor": "agent",
        "source_text": "The user prefers terse dup answers (1 note since 2024-01-01)."
    });
    let first = store_envelope(&s, &args);
    assert_eq!(first["ok"], json!(true), "{first}");
    let mut again = args;
    again["item"]["id"] = json!("itm-dup-2");
    let refused = store_envelope(&s, &again);
    assert_eq!(refused["ok"], json!(false), "{refused}");
    assert_eq!(refused["code"], json!("duplicate"), "{refused}");
    assert_eq!(refused["duplicate"], json!(true), "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .unwrap_or_default()
            .starts_with("duplicate: nothing stored"),
        "{refused}"
    );
    let mut preview = again;
    preview["dry_run"] = json!(true);
    let verdict = store_envelope(&s, &preview);
    assert_eq!(verdict["ok"], json!(true), "{verdict}");
    assert_eq!(verdict["pass"], json!(false), "{verdict}");
}
