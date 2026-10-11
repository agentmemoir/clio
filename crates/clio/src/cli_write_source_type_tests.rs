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

/// The message lists the wire values the enum accepts, so the list cannot drift
/// from `SourceType`. A variant added to the enum is a failing assertion here.
#[test]
fn the_value_list_is_the_enum_wire_values() {
    let expected: &[&str] = &[
        SourceType::UserStated.as_str(),
        SourceType::AgentInferred.as_str(),
        SourceType::ThirdParty.as_str(),
    ];
    assert_eq!(SOURCE_TYPES, expected);
}
