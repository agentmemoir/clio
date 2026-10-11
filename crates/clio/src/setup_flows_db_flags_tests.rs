// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests of the database flag applicability
//! table: that every database flag is either read or refused on every route.
//!
//! ## Owns
//! - The table's coverage under test for `setup_flows_db_flags`
//!
//! ## Does not own
//! - The questions each route asks (see `setup_flows_db_tests`)
//! - The value checks a flag's contents go through (see `setup_validate_tests`)
//!
//! ## Boundary
//!
//! A database flag added without a route that reads it should fail a test here.
//! Changes to a flag's meaning should not require changes here.
//!
//! Keep this module focused on the route-to-flags mapping.

use super::{DATABASE_FLAGS, DbRoute};
use crate::setup_preview_fixtures::Harness;

/// Every route, so a new variant cannot be left out of the matrix.
const ROUTES: [DbRoute; 4] = [
    DbRoute::Managed,
    DbRoute::Sqlite,
    DbRoute::Pasted,
    DbRoute::Typed,
];

/// The three service questions answered by flag, so a run that reaches them is
/// not what these tests are measuring.
const SERVICES_BY_FLAG: [(&str, &str); 3] =
    [("embed", "local"), ("rerank", "skip"), ("extract", "skip")];

/// The flags that name a route, set so `refuse_foreign` is reached. Each route's
/// refusal fires before any field is read, so no other flag is needed.
fn route_flags(route: DbRoute) -> Vec<(&'static str, &'static str)> {
    match route {
        DbRoute::Managed => vec![("database", "managed")],
        DbRoute::Sqlite => vec![("database", "sqlite")],
        DbRoute::Pasted => vec![("database", "existing"), ("database-connect", "pasted")],
        DbRoute::Typed => vec![("database", "existing"), ("database-connect", "typed")],
    }
}

#[test]
fn every_route_reads_only_known_database_flags_and_none_twice() {
    for route in ROUTES {
        let reads = route.reads();
        for flag in reads {
            assert!(
                DATABASE_FLAGS.contains(flag),
                "{route:?} reads `{flag}`, which is not in the closed flag set"
            );
        }
        let mut sorted = reads.to_vec();
        sorted.sort_unstable();
        let mut deduped = sorted.clone();
        deduped.dedup();
        assert_eq!(sorted, deduped, "{route:?} reads a flag twice");
    }
}

#[test]
fn every_tracked_flag_is_read_by_at_least_one_route() {
    // A flag no route reads would make `allowed` return the empty string, and
    // its refusal would name no route to use instead.
    for flag in DATABASE_FLAGS {
        let read_somewhere = ROUTES.into_iter().any(|route| route.reads().contains(flag));
        assert!(read_somewhere, "`{flag}` is tracked but no route reads it");
    }
}

#[test]
fn the_flag_set_has_no_duplicates() {
    let mut sorted = DATABASE_FLAGS.to_vec();
    sorted.sort_unstable();
    let mut deduped = sorted.clone();
    deduped.dedup();
    assert_eq!(sorted, deduped, "a flag is listed twice");
}

#[test]
fn every_database_flag_is_refused_on_every_route_that_does_not_read_it() {
    // The matrix. For each route, every flag outside that route's `reads` list
    // is refused by name; the flag is either read there or refused there.
    for route in ROUTES {
        let reads = route.reads();
        for flag in DATABASE_FLAGS {
            if reads.contains(flag) {
                continue;
            }
            let mut run = Harness::fresh().non_interactive();
            for (name, value) in route_flags(route) {
                run = run.flag(name, value);
            }
            run = run.flag(flag, "probe-value").flags(SERVICES_BY_FLAG);
            let reason = run.expect_refusal();
            assert!(
                reason.contains(&format!("--{flag}")),
                "{route:?} with `--{flag}` did not name the flag: {reason}"
            );
            assert!(
                reason.contains("applies to"),
                "{route:?} with `--{flag}` used the wrong refusal: {reason}"
            );
        }
    }
}
