// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the numeric `config_set` range
//! rules: which values each path accepts, and what a rejected value says.
//!
//! ## Owns
//! - The accepted range of every numeric path in `validate_set`
//! - The wording of the reranking cap rejection, ceiling included
//!
//! ## Does not own
//! - The path allowlist and the value dispatcher (see `validate_tests`)
//! - Overlay merging or config storage (see `config`)
//!
//! ## Boundary
//!
//! Tests here break when an accepted range or a rejection message changes; they
//! never change the implementation.
//!
//! Keep this module focused on numeric value validation behavior.

use super::{MAX_EMBED_DIMS, MAX_RERANK_BATCH, validate_numeric_value};
use crate::error::ErrorCode;
use serde_json::json;

#[test]
fn token_budget_and_history_limits_accept_a_positive_integer() {
    validate_numeric_value("injection.budget_tokens", &json!(10)).unwrap();
    validate_numeric_value("injection.budget_tokens", &json!(100_000)).unwrap();
    validate_numeric_value("history.lesson_max_tokens", &json!(10)).unwrap();
    validate_numeric_value("history.default_limit", &json!(5)).unwrap();
    validate_numeric_value("history.max_limit", &json!(10)).unwrap();
    assert_eq!(
        validate_numeric_value("injection.budget_tokens", &json!(0))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        validate_numeric_value("injection.budget_tokens", &json!(100_001))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        validate_numeric_value("history.max_limit", &json!("x"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
}

#[test]
fn similarity_threshold_and_temperature_accept_their_ranges() {
    validate_numeric_value("history.similarity_threshold", &json!(0.0)).unwrap();
    validate_numeric_value("history.similarity_threshold", &json!(0.5)).unwrap();
    validate_numeric_value("extract.temperature", &json!(0.0)).unwrap();
    validate_numeric_value("extract.temperature", &json!(2.0)).unwrap();
    assert_eq!(
        validate_numeric_value("history.similarity_threshold", &json!(1.5))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        validate_numeric_value("extract.temperature", &json!("warm"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
}

#[test]
fn an_embedding_width_stops_at_the_storage_ceiling() {
    validate_numeric_value("embed.dims", &json!(384)).unwrap();
    validate_numeric_value("embed.dims", &json!(MAX_EMBED_DIMS)).unwrap();
    for rejected in [json!(0), json!(MAX_EMBED_DIMS + 1)] {
        assert_eq!(
            validate_numeric_value("embed.dims", &rejected)
                .unwrap_err()
                .code,
            ErrorCode::OutOfRange,
            "accepted {rejected}"
        );
    }
    assert_eq!(
        validate_numeric_value("embed.dims", &json!(384.5))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
}

/// The documented per-request document limit is the last accepted cap, so a
/// shortlist of that size stays a single truthful request.
#[test]
fn the_rerank_cap_accepts_the_documented_service_limit() {
    for accepted in [1, 32, MAX_RERANK_BATCH] {
        validate_numeric_value("rerank.max_batch", &json!(accepted))
            .unwrap_or_else(|e| panic!("rejected {accepted}: {e}"));
    }
    assert_eq!(MAX_RERANK_BATCH, 1_000);
}

/// The message states the accepted shape and both ceilings, because an operator
/// who typed a bad cap has no other way to learn what the sidecar will take.
#[test]
fn a_rerank_cap_above_the_service_limit_is_rejected_with_both_ceilings() {
    let err = validate_numeric_value("rerank.max_batch", &json!(MAX_RERANK_BATCH + 1))
        .expect_err("1001 is above the per-request document limit");
    assert_eq!(err.code, ErrorCode::OutOfRange);
    assert!(
        err.message
            .contains("a positive integer no greater than 1000"),
        "message does not name the accepted shape: {}",
        err.message
    );
    assert!(
        err.message.contains("--max-client-batch-size"),
        "message does not name the sidecar ceiling: {}",
        err.message
    );
    assert!(
        err.message.contains("TEI"),
        "message does not say which provider honours the cap: {}",
        err.message
    );
}

#[test]
fn a_zero_rerank_cap_is_rejected() {
    let err = validate_numeric_value("rerank.max_batch", &json!(0)).expect_err("0 is not a cap");
    assert_eq!(err.code, ErrorCode::InvalidType);
    assert!(
        err.message
            .contains("a positive integer no greater than 1000"),
        "message does not name the accepted shape: {}",
        err.message
    );
}

#[test]
fn a_non_integer_rerank_cap_is_rejected() {
    for rejected in [json!("32"), json!(7.5), json!(null), json!(true)] {
        let err = validate_numeric_value("rerank.max_batch", &rejected)
            .expect_err("a non-integer is not a document count");
        assert_eq!(err.code, ErrorCode::InvalidType, "accepted {rejected}");
    }
}

/// The dispatcher routes only this module's paths here; anything else is
/// another module's answer, never a silent range rule of this one.
#[test]
fn a_path_this_module_does_not_own_is_left_alone() {
    validate_numeric_value("log.level", &json!(1)).unwrap();
    validate_numeric_value("backend", &json!(0)).unwrap();
}
