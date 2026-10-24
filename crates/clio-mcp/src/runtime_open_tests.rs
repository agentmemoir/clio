// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the inventory measurement an open performs: a measurement
//! failure fails the open with the measurement's own error code instead of
//! yielding a silent empty inventory behind a successful open, an in-memory
//! store is the only store that opens unmeasured, and extraction readiness
//! stays fail-closed.
//!
//! ## Owns
//! - The returned-error contract of `measure_inventory` for both backends
//! - The unmeasured in-memory open, through `measure_inventory` and through the
//!   hermetic open constructor
//! - The refusal of an unknown backend and of an unopenable Postgres target
//! - The extraction-readiness fail-closed rules
//!
//! ## Does not own
//! - The measurement itself or its error codes (`clio-store`)
//! - The runtime state assembly (`runtime_open`)
//! - The CLI exit-code mapping for an open error (`clio`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::*;
use clio_types::ErrorCode;
use serde_json::json;

/// Options naming one backend and one database URL, with everything else at
/// its default. Keeps each test's intent to the two fields it varies.
fn options(backend: &str, database_url: &str) -> McpOpenOptions {
    McpOpenOptions {
        backend: backend.to_owned(),
        database_url: database_url.to_owned(),
        ..McpOpenOptions::default()
    }
}

#[test]
fn postgres_measurement_failure_is_returned() {
    let err = measure_inventory(&options("postgres", "not a postgres url"))
        .expect_err("an unusable postgres target must fail the measurement");
    assert_eq!(err.code, ErrorCode::Internal, "unexpected error: {err:?}");
    assert!(
        err.message.contains("postgres inventory"),
        "the message must name the postgres inventory: {err:?}"
    );
}

#[test]
fn sqlite_measurement_failure_is_returned() {
    let directory = std::env::temp_dir();
    let directory = directory.to_str().expect("a utf-8 temp directory");
    let err = measure_inventory(&options("sqlite", directory))
        .expect_err("a directory has no store to measure");
    assert_eq!(
        err.code,
        ErrorCode::InvalidArgument,
        "unexpected error: {err:?}"
    );
}

#[test]
fn in_memory_store_opens_unmeasured() {
    let (facts, backend, measured) = measure_inventory(&options("sqlite", "sqlite::memory:"))
        .expect("an in-memory store has nothing to measure, so it opens unmeasured");
    assert!(!measured, "an in-memory store must report unmeasured");
    assert_eq!(backend, InventoryBackend::Sqlite);
    assert!(
        matches!(facts, InventoryFacts::Sqlite(_)),
        "unexpected facts: {facts:?}"
    );
}

#[test]
fn sqlite_file_is_measured() {
    // The path is named and never created: a store that does not exist yet is
    // still a measured file, distinct from the unmeasured in-memory case.
    let absent = std::env::temp_dir().join(format!(
        "clio-mcp-runtime-open-absent-{}.db",
        std::process::id()
    ));
    let url = absent.to_str().expect("a utf-8 temp path");
    let (facts, backend, measured) =
        measure_inventory(&options("sqlite", url)).expect("an absent store file is measurable");
    assert!(measured, "a store file must report measured");
    assert_eq!(backend, InventoryBackend::Sqlite);
    assert!(
        matches!(facts, InventoryFacts::Sqlite(_)),
        "unexpected facts: {facts:?}"
    );
}

#[test]
fn hermetic_open_measures_no_inventory_for_an_in_memory_store() {
    let state = McpState::open_with_effective(&McpOpenOptions::default(), &json!({}))
        .expect("the default in-memory backend opens");
    assert!(
        !state.inventory_measured,
        "an in-memory store must open unmeasured"
    );
}

#[test]
fn unknown_backend_is_refused() {
    let err = McpState::open_with_effective(&options("bogus", "sqlite::memory:"), &json!({}))
        .expect_err("an unknown backend must be refused");
    assert_eq!(err.code, ErrorCode::OutOfRange, "unexpected error: {err:?}");
}

#[test]
fn postgres_open_failure_is_returned() {
    // The store-open step fails before any measurement, so this covers the
    // malformed-target open only, not the measurement propagation in `build`.
    let result =
        McpState::open_with_effective(&options("postgres", "not a postgres url"), &json!({}))
            .map(|_| ());
    assert!(
        result.is_err(),
        "an unusable postgres target must fail the open"
    );
}

/// Pins the propagation edge inside `build`: the helper tests above only pin
/// `measure_inventory`, so this test is the one that fails if `build` ever
/// tolerates a failed measurement again.
#[test]
fn build_refuses_a_failed_measurement() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = Arc::new(SqliteStore::open_in_memory(Arc::clone(&kms)).expect("in-memory store"));
    let belief_corrector: Arc<dyn clio_compliance::BeliefContextCorrector> = store.clone();
    let store: Arc<dyn Repos> = store;
    // The store opened fine and only the measurement is deliberately unusable,
    // so the refusal cannot come from the store-open step that precedes `build`.
    let measurement_options = options("postgres", "not a postgres url");
    let err = McpState::build(
        &store,
        belief_corrector,
        &measurement_options,
        &json!({}),
        kms,
        clio_config::Runtime::with_env(&|_| None),
        false,
        None,
    )
    .expect_err("a failed inventory measurement must fail the open");
    assert_eq!(err.code, ErrorCode::Internal, "unexpected error: {err:?}");
    assert!(
        err.message
            .contains("measure the postgres inventory at open"),
        "the message must name the measurement: {err:?}"
    );
}

#[test]
fn extract_available_from_fails_closed() {
    assert!(
        !extract_available_from(&json!({})),
        "no url means extraction cannot run"
    );
    assert!(
        extract_available_from(&json!({"extract": {
            "provider": "template",
            "url": "http://127.0.0.1:1/extract",
        }})),
        "the template provider needs only a url"
    );
    let openai = |model: bool, credential: bool| {
        let mut extract = json!({
            "provider": "openai",
            "url": "http://127.0.0.1:1/extract",
        });
        if model {
            extract["model"] = json!("gpt-mini");
        }
        let mut document = json!({"extract": extract});
        if credential {
            document["credentials"] = json!({"extract_api_key": "test-key"});
        }
        document
    };
    assert!(
        !extract_available_from(&openai(false, true)),
        "openai without a model must stay unavailable"
    );
    assert!(
        !extract_available_from(&openai(true, false)),
        "openai without a credential must stay unavailable"
    );
    assert!(
        extract_available_from(&openai(true, true)),
        "openai with a model and a credential is available"
    );
    assert!(
        !extract_available_from(&json!({
            "extract": {
                "provider": "unknown",
                "url": "http://127.0.0.1:1/extract",
                "model": "gpt-mini",
            },
            "credentials": {"extract_api_key": "test-key"},
        })),
        "an unknown provider must stay unavailable"
    );
}
