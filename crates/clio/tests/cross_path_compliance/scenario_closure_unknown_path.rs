// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unknown-product-path closure refusal of
//! the cross-path compliance proof: a closure that names a product path no
//! caller has recorded as done refuses the completion claim with its own
//! named refusal, and the request stays open.
//!
//! ## Owns
//! - The unknown-product-path closure refusal and the exact refusal it returns
//! - The assertion that the request closes nothing
//! - The same case on SQLite and on loopback Postgres
//!
//! ## Does not own
//! - The request ledger and the closure gate (see `clio-compliance`)
//! - The CLI driver, the fixtures, and the shared request environment (see
//!   `cli_test_support`, `case_fixtures`, and `refusals_test_support`)
//!
//! ## Boundary
//!
//! Changes to which refused closure state this claim proves should normally
//! require changes here. Changes to the request ledger should not.
//!
//! Keep this module focused on the unknown-product-path closure refusal.

use crate::case_fixtures::{DENIED_ITEM, DENIED_MARKER, DENIED_SUBJECT, seed_pg, seed_sqlite};
use crate::refusals_test_support::{Env, assert_unknown_product_path_refusal, ok};
use crate::support_pg_test_support::{lock_pg, pg_url, postgres_reachable, require_postgres};
use clio::store::ScopedSchema;
use serde_json::json;

/// The product path the closure names: the erasure path of the request.
const PRODUCT_PATH: &str = "erasure";

/// The unknown-product-path refusal on one deployment.
///
/// The closure gate reads the named product path from the stored request, and
/// no path slot has been recorded, so the gate refuses the completion claim
/// with a message naming the missing recorded product path. The
/// request stays open.
fn unknown_product_path_refuses_closure(env: &Env, request: &str) {
    ok(&env.start(request, DENIED_SUBJECT), "request-start");

    let close = env.command(
        "request-close",
        &format!("--request-id {request} --product-path {PRODUCT_PATH}"),
    );
    assert_unknown_product_path_refusal(&close);

    // Nothing unsafe completes: the request stays open.
    let status = env.status(request);
    assert_eq!(
        status["state"],
        json!("InProgress"),
        "the request stays open: {status}"
    );
    assert_eq!(
        status["request_outcome"],
        json!("open"),
        "the request closes nothing: {status}"
    );
}

/// The unknown-product-path refusal on SQLite.
#[test]
fn scenario_closure_unknown_path_refuses_closure() {
    let env = Env::new("refusals-unknown-path");
    seed_sqlite(
        env.path(),
        &env.kms,
        &[(DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER)],
    );
    unknown_product_path_refuses_closure(&env, "req-unknown-path");
}

/// The same unknown-product-path refusal on loopback Postgres.
#[test]
fn scenario_closure_unknown_path_refuses_closure_postgres() {
    let base = pg_url();
    if !require_postgres(postgres_reachable(&base)) {
        return;
    }
    let _guard = lock_pg();
    let target = ScopedSchema::new(&base);
    let env = Env::postgres("refusals-unknown-path-pg", target.url());
    seed_pg(
        env.db(),
        &env.kms,
        &[(DENIED_ITEM, DENIED_SUBJECT, DENIED_MARKER)],
    );
    unknown_product_path_refuses_closure(&env, "req-unknown-path-pg");
}
