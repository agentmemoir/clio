// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for unit-testing the one owner of the database
//! environment variable: its name, and the shape of the values it accepts.
//!
//! ## Owns
//! - The name itself, so a rename cannot silently come back
//! - Which injected values resolve and which count as absent
//! - Agreement between the injected and the real-environment entry points
//!
//! ## Does not own
//! - The owner implementation (see `db_url_env`)
//! - Where a resolved URL sits in the precedence chain (see `db_path`)
//!
//! ## Boundary
//!
//! Tests here break when the variable name or the accepted value shape changes;
//! they never change the implementation.
//!
//! Keep this module focused on the variable, not on what a caller does with it.

use super::*;

/// Build an injected environment lookup from key/value pairs.
fn env(pairs: &[(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> + 'static {
    let pairs: Vec<(&'static str, &'static str)> = pairs.to_vec();
    move |key: &str| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).to_owned())
    }
}

#[test]
fn the_variable_is_the_prefixed_name() {
    assert_eq!(DATABASE_URL_ENV, "CLIO_DATABASE_URL");
}

#[test]
fn the_prefixed_name_resolves() {
    let get = env(&[(DATABASE_URL_ENV, "postgres://from-env/db")]);
    let found = database_url_from(&get);
    assert_eq!(found.as_deref(), Some("postgres://from-env/db"));
}

/// The hard rename, pinned from the other side: the bare name resolves to
/// nothing at all. If a second spelling ever comes back, this is the test that
/// says so.
#[test]
fn the_bare_name_is_not_read() {
    let get = env(&[("DATABASE_URL", "postgres://from-bare-name/db")]);
    assert_eq!(database_url_from(&get), None);
}

#[test]
fn a_missing_name_resolves_to_nothing() {
    let get = env(&[("CLIO_LOG_LEVEL", "debug")]);
    assert_eq!(database_url_from(&get), None);
}

/// A blank value leaves a caller with nothing openable, so it counts as absent
/// rather than as an empty address.
#[test]
fn blank_values_count_as_absent() {
    for blank in ["", "   ", "\t"] {
        let get = env(&[(DATABASE_URL_ENV, blank)]);
        let reason = format!("a blank value must not resolve: {blank:?}");
        assert_eq!(database_url_from(&get), None, "{reason}");
    }
}

/// An interior NUL cannot reach a driver, so it is treated as absent at the one
/// place every consumer reads rather than at each one separately.
#[test]
fn a_nul_bearing_value_counts_as_absent() {
    let get = env(&[(DATABASE_URL_ENV, "postgres://a\0b/db")]);
    assert_eq!(database_url_from(&get), None);
}

#[test]
fn surrounding_whitespace_is_trimmed() {
    let get = env(&[(DATABASE_URL_ENV, "  postgres://trimmed/db\n")]);
    let found = database_url_from(&get);
    assert_eq!(found.as_deref(), Some("postgres://trimmed/db"));
}

/// The store hands over a lookup it may only be able to call mutably, so the
/// owner has to accept one. This is the compile-level half of that promise: a
/// lookup that counts its own calls still resolves.
#[test]
fn a_mutable_lookup_is_accepted() {
    let mut calls = 0usize;
    let get = |key: &str| {
        calls += 1;
        let hit = key == DATABASE_URL_ENV;
        hit.then(|| "postgres://mutable/db".to_owned())
    };
    let found = database_url_from(get);
    assert_eq!(found.as_deref(), Some("postgres://mutable/db"));
    assert_eq!(calls, 1, "the owner reads exactly one name, once");
}

/// The two entry points must resolve the same name. Both sides read the same
/// host value, so this holds on every machine whatever the host exports; it pins
/// the name, never the value.
#[test]
fn both_entry_points_resolve_the_same_name() {
    let process = database_url_from_process_env();
    let injected = database_url_from(&|key: &str| std::env::var(key).ok());
    let mismatch = "the two entry points read different names";
    match (&process, &injected) {
        (Ok(read_one), Some(read_two)) => assert_eq!(read_one, read_two, "{mismatch}"),
        (Err(_), None) => {}
        _ => panic!("{mismatch}"),
    }
}
