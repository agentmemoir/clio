// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//! Test atomic durable-request updates with independent Postgres connections.
//! ## Owns
//! Postgres isolation and invocation of the shared CAS contract assertions.
//! ## Does not own
//! SQLite fixtures or request service rules.
//! ## Boundary
//! Test-only. Each test owns a scoped schema and both connection handles.

use super::durable_requests_cas_tests::{
    assert_conditional_save_contract, assert_stale_writes_preserve_committed_rows,
};
use crate::pg_scoped_schema::ScopedSchema;

#[test]
fn postgres_conditional_save_requires_an_exact_snapshot() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    if let Err(error) = crate::pg_tls::connect(&url) {
        crate::pg_skip::unreachable_pg_or_skip("postgres durable request CAS tests", &error);
        return;
    }
    let schema = ScopedSchema::new(&url);
    let first = schema.open();
    let second = schema.open();
    assert_conditional_save_contract(&first, &second);
}

#[test]
fn postgres_stale_writes_preserve_committed_rows() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    if let Err(error) = crate::pg_tls::connect(&url) {
        crate::pg_skip::unreachable_pg_or_skip("postgres durable request CAS tests", &error);
        return;
    }
    let schema = ScopedSchema::new(&url);
    let first = schema.open();
    let second = schema.open();
    assert_stale_writes_preserve_committed_rows(&first, &second);
}
