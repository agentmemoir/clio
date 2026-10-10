// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the database resolver that reads the effective config
//! document: the precedence tiers, the `backend`/address pairing, and the
//! refusals for a value no store can open.
//!
//! ## Owns
//! - Each precedence tier, asserted one tier at a time
//! - The silent-SQLite regression: a configured address that names Postgres
//!   resolves to Postgres even when the document's `backend` still says sqlite
//! - Refusals for a malformed address and an unknown backend
//!
//! ## Does not own
//! - The resolver itself (see `crate::db_effective`)
//! - The environment-only chain (see `crate::db_path`)
//!
//! ## Boundary
//!
//! Tests here break when the resolver's precedence or its pairing rule change;
//! they never change the implementation.
//!
//! Keep this module focused on the effective-document resolver.

use super::*;
use crate::db_path::DB_FILE_NAME;
use serde_json::json;

/// A document as the defaults seed it: `backend: sqlite` and a file path.
fn seeded(url: &str) -> Value {
    json!({
        "backend": "sqlite",
        "store": { "database_url": url, "sqlite_path": "/data/clio.db" }
    })
}

/// An environment that answers nothing, so no host value can leak in.
fn no_env(_key: &str) -> Option<String> {
    None
}

/// A home-scoped environment, so the default path is a known value.
fn home_env() -> impl Fn(&str) -> Option<String> {
    |key: &str| (key == "HOME").then(|| "/tmp/am-t310-home".to_owned())
}

fn resolve(effective: &Value) -> Result<ResolvedDatabase, String> {
    resolve_effective_database(&home_env(), effective, None, None)
}

#[test]
fn a_configured_postgres_address_resolves_to_postgres() {
    let document = seeded("postgres://configured/db");
    let resolved = resolve(&document).expect("a postgres address is openable");
    assert_eq!(resolved.backend, "postgres");
    assert_eq!(resolved.url, "postgres://configured/db");
}

#[test]
fn the_address_scheme_outranks_a_stale_document_backend() {
    // The document is what a config file that set only `store.database_url`
    // produces: the seeded `backend` never changed. Reading it first is the
    // silent-SQLite bug.
    let document = seeded("postgres://configured/db");
    let resolved = resolve(&document).expect("resolves");
    assert_eq!(resolved.backend, "postgres", "seeded backend must not win");
}

#[test]
fn the_flag_address_outranks_the_document() {
    let document = seeded("postgres://configured/db");
    let resolved =
        resolve_effective_database(&home_env(), &document, Some("sqlite::memory:"), None)
            .expect("resolves");
    assert_eq!(resolved.url, "sqlite::memory:");
}

#[test]
fn the_flag_backend_outranks_the_document() {
    let document = seeded("/data/clio.db");
    let resolved =
        resolve_effective_database(&home_env(), &document, None, Some("sqlite")).expect("resolves");
    assert_eq!(resolved.backend, "sqlite");
}

#[test]
fn an_empty_flag_address_falls_through_to_the_document() {
    let document = seeded("postgres://configured/db");
    let resolved =
        resolve_effective_database(&home_env(), &document, Some("   "), None).expect("resolves");
    assert_eq!(resolved.url, "postgres://configured/db");
}

#[test]
fn an_absent_document_falls_back_to_the_data_directory() {
    let resolved = resolve(&json!({})).expect("resolves");
    assert_eq!(resolved.url, "/tmp/am-t310-home/.local/share/clio/clio.db");
    assert_eq!(resolved.backend, "sqlite");
}

#[test]
fn a_home_less_environment_falls_back_to_the_relative_file() {
    let resolved = resolve_effective_database(&no_env, &json!({}), None, None).expect("resolves");
    assert_eq!(resolved.url, DB_FILE_NAME);
}

#[test]
fn a_mysql_address_is_refused_with_the_reason() {
    let document = seeded("mysql://configured/db");
    let error = resolve(&document).expect_err("a mysql server cannot be opened");
    assert!(
        error.contains("mysql://"),
        "names what was configured: {error}"
    );
    assert!(
        error.contains("postgres:// or postgresql://"),
        "names what is supported: {error}"
    );
}

#[test]
fn a_sqlite_scheme_address_is_refused() {
    let document = seeded("sqlite://configured/db");
    let error = resolve(&document).expect_err("sqlite:// is not a database target here");
    assert!(error.contains("sqlite://configured/db"), "{error}");
}

#[test]
fn an_unknown_document_backend_is_refused() {
    let document = json!({ "backend": "mysql", "store": { "database_url": "/data/clio.db" } });
    let error = resolve(&document).expect_err("no store opens as mysql");
    assert!(
        error.contains("sqlite|postgres"),
        "names what is supported: {error}"
    );
}

#[test]
fn a_document_backend_without_an_address_stays_postgres() {
    // The mirror of the pairing case: the operator named a backend and gave no
    // address. It must not quietly become sqlite, because a wrong backend is
    // loud at open time and a wrong one is not.
    let document = json!({ "backend": "postgres", "store": { "database_url": "/data/clio.db" } });
    let resolved = resolve(&document).expect("resolves");
    assert_eq!(resolved.backend, "postgres");
}

#[test]
fn a_postgres_url_from_the_environment_keeps_its_backend() {
    let document = seeded("postgres://env/db");
    let resolved = resolve(&document).expect("resolves");
    assert_eq!(resolved.backend, "postgres");
}

#[test]
fn a_percent_encoded_password_survives_resolution_unchanged() {
    let document = seeded("postgres://u:a%20b%26c@h:5432/db?sslmode=require");
    let resolved = resolve(&document).expect("resolves");
    assert_eq!(
        resolved.url,
        "postgres://u:a%20b%26c@h:5432/db?sslmode=require"
    );
}

#[test]
fn a_whitespace_only_document_address_falls_back_to_the_data_directory() {
    let document = seeded("   ");
    let resolved = resolve(&document).expect("resolves");
    assert_eq!(resolved.url, "/tmp/am-t310-home/.local/share/clio/clio.db");
}

/// A flag address is paired with its own scheme, never with the document's
/// `backend`. A machine that exports a `CLIO_DATABASE_URL` seeds the document
/// with `backend: postgres`, and `clio --db ./notes.db` must still open a
/// SQLite file rather than Postgres against a path.
#[test]
fn a_flag_address_is_not_paired_with_the_document_backend() {
    let document = seeded("postgres://env/db");
    let resolved = resolve_effective_database(&home_env(), &document, Some("./notes.db"), None)
        .expect("resolves");
    assert_eq!(resolved.backend, "sqlite");
    assert_eq!(resolved.url, "./notes.db");
}
