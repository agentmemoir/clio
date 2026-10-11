// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the `--epistemic-kind`/`--source-type` flag pair on the
//! candidate write verbs: the usage errors a mismatch produces and the value
//! list the message names.
//!
//! ## Owns
//! - The pairing and value checks for `cli_write_source_type`
//!
//! ## Does not own
//! - The rule under test (`cli_write_source_type`)
//! - The rest of the core write-command suite (see `cli_write_core_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::SOURCE_TYPES;
use crate::cli_read_test_support::state;
use crate::cli_write_test_support::run_write_state;
use clio_types::SourceType;
use serde_json::{Value, json};
use std::process::ExitCode;

/// Parse the JSON-mode error envelope printed to stderr.
fn error_envelope(err: &str) -> Value {
    serde_json::from_str(err).expect("error envelope is JSON")
}

#[test]
fn remember_belief_without_source_type_is_a_usage_error() {
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &["x", "--category", "persona", "--epistemic-kind", "belief"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    let envelope = error_envelope(&err);
    assert_eq!(envelope["code"], json!("usage"), "{err}");
    let message = envelope["message"].as_str().expect("message");
    assert!(message.contains("--epistemic-kind"), "{err}");
    assert!(message.contains("--source-type"), "{err}");
    assert!(message.contains("belief"), "{err}");
    for &value in SOURCE_TYPES {
        assert!(message.contains(value), "missing {value}: {message}");
    }
}

#[test]
fn remember_fact_with_source_type_is_a_usage_error() {
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &["x", "--category", "persona", "--source-type", "user_stated"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    let envelope = error_envelope(&err);
    assert_eq!(envelope["code"], json!("usage"), "{err}");
    let message = envelope["message"].as_str().expect("message");
    assert!(message.contains("--epistemic-kind"), "{err}");
    assert!(message.contains("--source-type"), "{err}");
    assert!(message.contains("fact"), "{err}");
    for &value in SOURCE_TYPES {
        assert!(message.contains(value), "missing {value}: {message}");
    }
}

#[test]
fn remember_rejects_unknown_source_type() {
    let s = state();
    let (code, out, err) = run_write_state(
        "remember",
        &[
            "x",
            "--category",
            "persona",
            "--epistemic-kind",
            "belief",
            "--source-type",
            "bogus",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    let envelope = error_envelope(&err);
    assert_eq!(envelope["code"], json!("usage"), "{err}");
    let message = envelope["message"].as_str().expect("message");
    for &value in SOURCE_TYPES {
        assert!(message.contains(value), "missing {value}: {message}");
    }
}

#[test]
fn admit_shares_the_source_type_pairing() {
    let s = state();
    let (code, out, err) = run_write_state(
        "admit",
        &["x", "--category", "persona", "--epistemic-kind", "belief"],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--source-type"), "{err}");
}

#[test]
fn admit_rejects_an_unknown_source_type() {
    let s = state();
    let (code, out, err) = run_write_state(
        "admit",
        &[
            "x",
            "--category",
            "persona",
            "--epistemic-kind",
            "belief",
            "--source-type",
            "bogus",
        ],
        &s,
        false,
    );
    assert_eq!(code, ExitCode::from(2), "{out}{err}");
    assert!(err.contains("--source-type"), "{err}");
}

/// Every write verb that takes the two flags applies the same pairing rule, so
/// the trap is fixed on the whole family and not only on `remember`/`admit`.
#[test]
fn the_sibling_write_verbs_share_the_pairing_rule() {
    let s = state();
    for (verb, args) in [
        (
            "triple add",
            vec!["a", "b", "c", "--epistemic-kind", "belief"],
        ),
        (
            "canonical put",
            vec![
                "k",
                "v",
                "--category",
                "persona",
                "--epistemic-kind",
                "belief",
            ],
        ),
        (
            "shared store",
            vec![
                "x",
                "--epistemic-kind",
                "belief",
                "--actor",
                "agent",
                "--shared-bank",
            ],
        ),
    ] {
        let (code, out, err) = run_write_state(verb, &args, &s, false);
        assert_eq!(code, ExitCode::from(2), "{verb}: {out}{err}");
        assert!(err.contains("--source-type"), "{verb}: {err}");
    }
}

/// The CLI's value list is the type's own wire list. The structural guard that
/// keeps the enum, `ALL`, and `WIRE` in step lives with the type in `clio-types`;
/// this pins that the CLI reads it rather than a second hardcoded copy.
#[test]
fn the_value_list_is_the_enum_wire_list() {
    assert_eq!(SOURCE_TYPES, SourceType::WIRE);
}
