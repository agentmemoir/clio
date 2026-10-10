// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//! Unit tests for the duplicate write CLI contract.

use super::{allow_duplicate_arg, is_duplicate_noop, render_duplicate};
use crate::cli_args::{FlagSpec, ParsedArgs, parse};
use serde_json::json;

fn parsed(args: &[&str]) -> ParsedArgs {
    let spec = FlagSpec::new(&["category"], &["allow-duplicate"]);
    let tokens = args
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    parse(&spec, tokens).expect("parse")
}

#[test]
fn allow_duplicate_is_absent_without_the_flag() {
    assert_eq!(allow_duplicate_arg(&parsed(&["remember", "text"])), None);
}

#[test]
fn allow_duplicate_is_true_with_the_flag() {
    assert_eq!(
        allow_duplicate_arg(&parsed(&["remember", "text", "--allow-duplicate"])),
        Some(json!(true))
    );
}

#[test]
fn a_duplicate_payload_is_recognised() {
    assert!(is_duplicate_noop(&json!({"duplicate": true})));
}

#[test]
fn a_stored_payload_is_not_a_duplicate() {
    assert!(!is_duplicate_noop(&json!({"id": "itm-1", "pass": true})));
    assert!(!is_duplicate_noop(&json!({})));
}

#[test]
fn the_duplicate_view_says_nothing_was_stored() {
    let text = render_duplicate(&json!({
        "duplicate": true,
        "rejection_reason": "exact_duplicate: identical text is already stored for `persona`",
    }));
    assert!(text.starts_with("duplicate: nothing stored"), "{text}");
    assert!(text.contains("exact_duplicate"), "{text}");
}

#[test]
fn the_duplicate_view_falls_back_when_no_reason_is_present() {
    let text = render_duplicate(&json!({"duplicate": true}));
    assert_eq!(
        text,
        "duplicate: nothing stored (identical text is already stored)"
    );
}
