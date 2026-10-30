// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `hygiene clean` write verb's build and parse
//! units: the stdin-path refusal, the candidates JSON shapes
//! `parse_candidates` accepts and rejects, and the command spec's flags.
//!
//! ## Owns
//! - Unit tests for `parse_candidates`, the `hygiene clean` build path, and
//!   the `HygieneWriteGroup` spec declaration
//!
//! ## Does not own
//! - The command implementation under test (`cli_write_hygiene`)
//! - The end-to-end behavior tests (`cli_write_hygiene_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::{HygieneWriteGroup, parse_candidates};
use crate::cli_write_group::WriteGroup;
use serde_json::json;
use std::process::ExitCode;

#[test]
fn build_without_streams_rejects_the_stdin_path() {
    let parsed = crate::cli_args::parse(
        &HygieneWriteGroup.spec("hygiene clean"),
        ["--file".to_owned(), "-".to_owned()],
    )
    .expect("parse");
    let Err(error) = HygieneWriteGroup.build("hygiene clean", &parsed) else {
        panic!("no stdin on this path")
    };
    assert_eq!(error.exit_code(), ExitCode::from(2));
    assert!(error.message().contains("stdin"), "{}", error.message());
}

#[test]
fn parse_candidates_accepts_both_document_shapes() {
    let object = r#"{"candidates": [{"item_id": "a"}], "bank": "demo"}"#;
    assert_eq!(parse_candidates(object).unwrap().len(), 1);
    let bare = r#"[{"item_id": "a"}, {"item_id": "b", "noise_score": 0.5}]"#;
    assert_eq!(parse_candidates(bare).unwrap().len(), 2);
}

#[test]
fn parse_candidates_passes_optional_fields_and_drops_extras() {
    let raw = r#"[{"item_id": "a", "noise_score": 0.9, "suggested_action": "discard",
                   "noise_reasons": ["x"], "preview": "leak"}]"#;
    let candidates = parse_candidates(raw).unwrap();
    assert_eq!(candidates[0]["item_id"], json!("a"));
    assert_eq!(candidates[0]["noise_score"], json!(0.9));
    assert_eq!(candidates[0]["suggested_action"], json!("discard"));
    assert!(candidates[0].get("preview").is_none(), "{candidates:?}");
}

#[test]
fn parse_candidates_rejects_everything_else_with_usage_errors() {
    for (raw, fragment) in [
        ("{not json", "JSON"),
        (r#"{"items": [{"item_id": "a"}]}"#, "candidates"),
        (r#"{"candidates": "two"}"#, "array"),
        (r#"{"candidates": 3}"#, "array"),
        ("\"just a string\"", "array"),
        ("42", "array"),
        ("null", "array"),
        (r#"[{"noise_score": 0.5}]"#, "item_id"),
        (r#"[{"item_id": 7}]"#, "item_id"),
        (r#"["a"]"#, "objects"),
        (r#"[{"item_id": "a", "noise_score": "high"}]"#, "number"),
        (r#"[{"item_id": "a", "suggested_action": 5}]"#, "string"),
    ] {
        let error = parse_candidates(raw).expect_err(raw);
        assert_eq!(error.exit_code(), ExitCode::from(2), "{raw}");
        assert!(
            error.message().contains(fragment),
            "{raw}: {}",
            error.message()
        );
    }
}

#[test]
fn hygiene_write_group_declares_its_verb_and_spec() {
    assert_eq!(HygieneWriteGroup.verbs(), &["hygiene clean"][..]);
    let spec = HygieneWriteGroup.spec("hygiene clean");
    for flag in ["file", "action", "reason", "note"] {
        assert!(spec.value_flags().contains(&flag), "{flag}");
    }
    assert!(spec.bool_flags().contains(&"confirm"));
    assert!(spec.bool_flags().contains(&"dry-run"));
    assert!(HygieneWriteGroup.mutates("hygiene clean"));
    assert!(
        HygieneWriteGroup
            .refusal("hygiene clean", &json!({}))
            .is_none()
    );
}
