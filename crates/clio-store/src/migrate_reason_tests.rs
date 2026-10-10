// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that a driver failure reaches the operator with the
//! reason behind it, because `map_db_err` is the one mapping every statement
//! goes through and a placeholder there hides every server-side refusal.
//!
//! ## Owns
//! - The Postgres half of `map_db_err`: a real server refusal, routed through
//!   the real mapping, must carry the server's words
//! - The non-Postgres half: an error that is not a driver error keeps the text
//!   it already had
//!
//! ## Does not own
//! - The chain-walking itself (`pg_tls::describe`), which the connect and probe
//!   paths' own tests cover
//! - Which statements exist or what they mean (`migrate`)
//!
//! ## Boundary
//!
//! Test-only, and the Postgres test serializes on the shared Postgres test lock.
//! Never compiled into production builds.

use super::*;
use crate::db_url::postgres_url_or_default;
use crate::pg_lock::lock_pg_test;

/// A refusal the server owns, produced without touching shared state.
///
/// `pg_scoped_schema` would give a private schema, but this only needs a
/// statement the server must refuse, and reading a table that does not exist
/// needs no setup at all: it fails before it can touch a row, so it cannot
/// disturb the rows another test owns even though they share one database.
fn servers_own_refusal() -> AmError {
    let _guard = lock_pg_test();
    let url = postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let mut client = match crate::pg_tls::connect(&url) {
        Ok(client) => client,
        // No server means the mapping was never reached, so there is nothing to
        // assert. Printing the error rather than the URL keeps any credential in
        // `CLIO_DATABASE_URL` out of the log.
        Err(error) => {
            eprintln!(
                "{}",
                crate::pg_skip::pg_skip_note("driver error reason", &error)
            );
            return AmError::new(ErrorCode::NotImplemented, "no postgres to ask");
        }
    };
    let error = client
        .query_one("SELECT 1 FROM a_table_that_does_not_exist", &[])
        .expect_err("a missing table must be refused");
    map_db_err("probe", error)
}

/// Regression cover for issue #63: the mapping took the driver's `Display`,
/// which for a server-side refusal is the bare string `db error`. An operator
/// reading `items insert: db error` cannot tell a constraint violation from a
/// syntax error from a failed load, because the constraint name, the `DETAIL`
/// and the `HINT` were all dropped.
#[test]
fn a_server_refusal_reaches_the_operator_with_the_servers_reason() {
    let error = servers_own_refusal();
    if error.code == ErrorCode::NotImplemented {
        return; // no Postgres on this host; nothing was exercised
    }
    let message = &error.message;
    assert!(
        !message.ends_with("db error"),
        "the placeholder is the whole message again: {message}"
    );
    assert!(
        message.contains("a_table_that_does_not_exist"),
        "the server names the object it refused, and that name is what tells an \
         operator which statement failed: {message}"
    );
    assert!(
        message.starts_with("probe: "),
        "the caller's context still leads, so the reason is attributed: {message}"
    );
}

/// The same mapping must not lose the reason when it is reached through the
/// single sink every statement uses, because that sink is the path the store
/// actually takes and a fix that only held for a direct call would not hold for
/// a real statement.
#[test]
fn the_single_statement_sink_also_keeps_the_servers_reason() {
    let _guard = lock_pg_test();
    let url = postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let mut client = match crate::pg_tls::connect(&url) {
        Ok(client) => client,
        Err(error) => {
            eprintln!(
                "{}",
                crate::pg_skip::pg_skip_note("driver error reason", &error)
            );
            return;
        }
    };
    let failed: Result<u64, AmError> = client
        .execute("SELECT 1 FROM a_table_that_does_not_exist", &[])
        .am("scoped probe");
    let message = &failed.expect_err("a missing table must be refused").message;
    assert!(!message.ends_with("db error"), "{message}");
    assert!(
        message.contains("a_table_that_does_not_exist"),
        "the server's own naming is the point: {message}"
    );
}

/// The other backend is unaffected. `rusqlite` already puts its message in
/// `Display`, so there is nothing behind it to find — and a change that started
/// rewriting those messages would be a regression nobody asked for.
#[test]
fn an_error_that_is_not_a_driver_error_keeps_its_own_text() {
    let error = map_db_err("read a file", std::io::Error::other("disk went away"));
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(error.message, "read a file: disk went away");
}
