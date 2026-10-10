// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for integration tests of the config path/value
//! validation rules enforced by `validate::validate_set_path` and
//! `validate::validate_set_value`.
//!
//! ## Owns
//! - Test coverage for path allowlist/forbid rules and per-path value checks
//! - Test coverage for `ConfigScope::parse` / `as_str`
//!
//! ## Does not own
//! - The validation rules themselves (see `validate`)
//! - Other config tests (see `tests`, `embed_knob_tests`)
//!
//! ## Boundary
//!
//! Changes to validation error codes or allowlists should normally require
//! changes here (assertions updated). Changes to validation implementation
//! should not require changes here.
//!
//! Keep this module focused on validation-rule test coverage.

use super::*;
use serde_json::json;
#[test]
fn rejects_gate_disable() {
    let err = validate_set_path("gates.admission_enabled").expect_err("forbid");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn rejects_ranking_via_config_set() {
    let err = validate_set_path("ranking.theta_admit").expect_err("forbid");
    assert_eq!(err.code, ErrorCode::Forbidden);
}

#[test]
fn rejects_unknown_path() {
    let err = validate_set_path("not.a.real.path").expect_err("unknown");
    assert_eq!(err.code, ErrorCode::UnknownPath);
}

#[test]
fn accepts_allowlisted_backend() {
    validate_set_path("backend").expect("ok");
    validate_set_value("backend", &json!("sqlite")).expect("ok");
}

#[test]
fn scope_parse_and_as_str() {
    assert_eq!(ConfigScope::parse("session").unwrap().as_str(), "session");
    assert_eq!(ConfigScope::parse("profile").unwrap().as_str(), "profile");
    assert_eq!(
        ConfigScope::parse("deployment").unwrap().as_str(),
        "deployment"
    );
    assert_eq!(
        ConfigScope::parse("nope").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn path_empty_and_dotdot() {
    assert_eq!(
        validate_set_path("").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        validate_set_path("a..b").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        validate_set_path("fr10_satisfied").unwrap_err().code,
        ErrorCode::Forbidden
    );
    assert_eq!(
        validate_set_path("bypass_x").unwrap_err().code,
        ErrorCode::Forbidden
    );
}

#[test]
fn value_validation_matrix() {
    validate_set_value("log.level", &json!("info")).unwrap();
    assert_eq!(
        validate_set_value("log.level", &json!(1)).unwrap_err().code,
        ErrorCode::InvalidType
    );
    assert_eq!(
        validate_set_value("log.level", &json!("loud"))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );

    validate_set_value("injection.budget_tokens", &json!(10)).unwrap();
    assert_eq!(
        validate_set_value("injection.budget_tokens", &json!("x"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
    assert_eq!(
        validate_set_value("injection.budget_tokens", &json!(0))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );

    assert_eq!(
        validate_set_value("backend", &json!(1)).unwrap_err().code,
        ErrorCode::InvalidType
    );
    assert_eq!(
        validate_set_value("backend", &json!("redis"))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );

    validate_set_value("bank.fixed", &json!("b")).unwrap();
    assert_eq!(
        validate_set_value("bank.fixed", &json!(1))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
    assert_eq!(
        validate_set_value("bank.fixed", &json!(""))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );

    validate_set_value("bank.policy", &json!({"kind": "required"})).unwrap();
    assert_eq!(
        validate_set_value("bank.policy", &json!(1))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );

    validate_set_value("credentials.api_key", &json!("sk")).unwrap();
    assert_eq!(
        validate_set_value("credentials.api_key", &json!(1))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );

    validate_set_value("sync.omit", &json!(true)).unwrap();
    assert_eq!(
        validate_set_value("sync.omit", &json!("x"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );

    validate_set_value("store.database_url", &json!("postgres://x")).unwrap();
    assert_eq!(
        validate_set_value("store.database_url", &json!(1))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
    validate_set_value("store.sqlite_path", &json!("./data/x.db")).unwrap();
    assert_eq!(
        validate_set_value("store.sqlite_path", &json!(1))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
    assert_eq!(
        validate_set_value("store.sqlite_path", &json!(""))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );

    validate_set_value("unknown", &json!(null)).unwrap();
}

#[test]
fn provider_value_validation() {
    validate_set_path("embed.provider").unwrap();
    validate_set_path("rerank.provider").unwrap();
    validate_set_path("rerank.url").unwrap();
    validate_set_path("rerank.model").unwrap();
    validate_set_path("credentials.rerank_api_key").unwrap();

    for ok in ["tei", "openai"] {
        validate_set_value("embed.provider", &json!(ok)).unwrap();
    }
    for ok in ["tei", "cohere"] {
        validate_set_value("rerank.provider", &json!(ok)).unwrap();
    }

    // An explicitly empty provider is rejected instead of silently meaning
    // "unset"; an omitted field receives the tei default at read time.
    let e = validate_set_value("embed.provider", &json!("")).unwrap_err();
    assert_eq!(e.code, ErrorCode::OutOfRange);
    assert!(
        e.message.contains("embed.provider must be tei|openai"),
        "{e}"
    );
    let e = validate_set_value("rerank.provider", &json!("")).unwrap_err();
    assert_eq!(e.code, ErrorCode::OutOfRange);
    assert!(
        e.message.contains("rerank.provider must be tei|cohere"),
        "{e}"
    );

    let e = validate_set_value("embed.provider", &json!("mistral")).unwrap_err();
    assert_eq!(e.code, ErrorCode::OutOfRange);
    assert!(e.message.contains("embed.provider must be tei|openai"));

    let e = validate_set_value("rerank.provider", &json!("openai")).unwrap_err();
    assert_eq!(e.code, ErrorCode::OutOfRange);
    assert!(e.message.contains("rerank.provider must be tei|cohere"));

    assert_eq!(
        validate_set_value("embed.provider", &json!(1))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
    assert_eq!(
        validate_set_value("rerank.provider", &json!(1))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );

    validate_set_value("rerank.url", &json!("http://127.0.0.1:34312")).unwrap();
    assert_eq!(
        validate_set_value("rerank.url", &json!(1))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
    validate_set_value("rerank.model", &json!("BAAI/bge-reranker-base")).unwrap();
    assert_eq!(
        validate_set_value("rerank.model", &json!(1))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
    validate_set_value("credentials.rerank_api_key", &json!("sk")).unwrap();
    assert_eq!(
        validate_set_value("credentials.rerank_api_key", &json!(1))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
}

#[test]
fn history_knob_validation() {
    validate_set_path("history.lesson_max_tokens").unwrap();
    validate_set_value("history.lesson_max_tokens", &json!(10)).unwrap();
    validate_set_value("history.similarity_threshold", &json!(0.5)).unwrap();
    validate_set_value("history.default_limit", &json!(5)).unwrap();
    validate_set_value("history.max_limit", &json!(10)).unwrap();
    assert_eq!(
        validate_set_value("history.similarity_threshold", &json!("x"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
    assert_eq!(
        validate_set_value("history.similarity_threshold", &json!(2.0))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        validate_set_value("history.default_limit", &json!(0))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        validate_set_value("history.max_limit", &json!("x"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
}

#[test]
fn extract_knob_validation() {
    validate_set_path("extract.url").unwrap();
    validate_set_value("extract.url", &json!("http://localhost:34311")).unwrap();
    validate_set_value("extract.model", &json!("numind/NuExtract-1.5-tiny")).unwrap();
    validate_set_value("extract.temperature", &json!(0.0)).unwrap();
    validate_set_value("extract.day_first", &json!(true)).unwrap();
    validate_set_value("extract.day_first", &json!(null)).unwrap();
    assert_eq!(
        validate_set_value("extract.temperature", &json!(9.0))
            .unwrap_err()
            .code,
        ErrorCode::OutOfRange
    );
    assert_eq!(
        validate_set_value("extract.day_first", &json!("yes"))
            .unwrap_err()
            .code,
        ErrorCode::InvalidType
    );
}
