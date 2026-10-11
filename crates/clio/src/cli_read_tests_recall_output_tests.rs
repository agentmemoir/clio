// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `recall` response contract the argument-error sweep
//! named: the effective limit is echoed, an unknown bank is an error rather
//! than an empty page, and a floor that empties the page names the flag the
//! caller typed in both the JSON warnings and the text view.
//!
//! ## Owns
//! - Assertions over the `recall` payload and text for the limit, the unknown
//!   bank, and the empty-page floor warning
//!
//! ## Does not own
//! - The floor binding rule itself (`clio-retrieve` `floors` / `finalize`)
//! - The flag-to-argument mapping (`cli_read_tests_floors_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the recall response contract.

use super::*;

/// The limit that actually ran is echoed, whether it was typed or defaulted.
#[test]
fn recall_echoes_the_effective_limit() {
    let state = state_with_items();

    let (_, out, _) = run_state("recall", &["terse", "--bank", "bank-a"], &state, false);
    assert_eq!(
        json_of(&out)["limit"],
        json!(10),
        "the default limit: {out}"
    );

    let (_, out, _) = run_state(
        "recall",
        &["terse", "--bank", "bank-a", "--limit", "3"],
        &state,
        false,
    );
    assert_eq!(json_of(&out)["limit"], json!(3), "the typed limit: {out}");
}

/// A bank the store does not hold is an error, not an empty result.
#[test]
fn recall_unknown_bank_is_not_found() {
    let state = state_with_items();
    let (code, out, err) = run_state("recall", &["terse", "--bank", "nope"], &state, false);
    assert_eq!(code, ExitCode::from(1), "stderr: {err}");
    assert!(out.is_empty(), "no payload on failure: {out}");
    assert_eq!(json_of(&err)["code"], json!("not_found"), "{err}");
}

/// A floor that removes every candidate names the flag the caller typed, in
/// both the JSON warnings and the text view, and never the tool's own field
/// name.
#[test]
fn recall_empty_page_names_the_binding_floor_with_the_flag() {
    let state = state_with_items();
    let args = ["terse", "--bank", "bank-a", "--min-final", "1000"];

    let (code, out, _) = run_state("recall", &args, &state, false);
    assert_eq!(code, ExitCode::from(0), "{out}");
    let value = json_of(&out);
    assert_eq!(value["hits"].as_array().map(Vec::len), Some(0), "{value}");
    let warnings: Vec<&str> = value["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("--min-final")),
        "{value}"
    );
    assert!(
        !warnings
            .iter()
            .any(|warning| warning.contains("min_scores")),
        "the tool's field name must not reach the shell: {value}"
    );

    let (_, text, _) = run_state("recall", &args, &state, true);
    assert!(text.contains("no results"), "{text}");
    assert!(text.contains("--min-final"), "{text}");
}
