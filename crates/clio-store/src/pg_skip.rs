// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for how a Postgres test reports an unreachable
//! store: the one skip wording that can never carry the connection string, and
//! the fail-closed rule that makes a skipped backend a failure unless the
//! operator explicitly opted out.
//!
//! ## Owns
//! - `pg_skip_note`, the credential-safe wording for an unreachable store
//! - `skip_requested` and the fail-closed gates `unreachable_pg_or_skip`,
//!   `pg_required_or_skip`, and `pg_url_or_skip`: the strict default and the
//!   sole `CLIO_SKIP_PG_TESTS` opt-out that turns it into a printed note
//! - `unreachable_pg_or_skip_with`, the crate-private seam that takes the
//!   opt-out decision already read, so a test can hold one decision across its
//!   own assertion and the gate instead of reading the environment twice
//!
//! The public gates are the only way a Postgres test may skip: each reads
//! `CLIO_SKIP_PG_TESTS` itself and fails closed otherwise, so a suite cannot
//! reach the opt-out path by formatting a note and then returning. The `_with`
//! seam is crate-private and test-only, so a dependent crate still has no entry
//! that skips without the environment's opt-out.
//!
//! The seam is compiled only under `cfg(test)` or the `test-support` feature,
//! which a dependent crate turns on from `[dev-dependencies]`; no shipped build
//! carries it.
//!
//! ## Does not own
//! - Whether a test probes for a skippable failure at all (each test's own
//!   `open_pg`)
//! - The connection itself, and the credential-safe description of a target
//!   (`pg_test_target`)
//!
//! ## Boundary
//!
//! Changes to the fail-closed rule or to the skip wording should normally
//! require changes here. Changes to a test's skip condition should not require
//! changes here.
//!
//! Test-only: the skip note is a test diagnostic and is never compiled into a
//! production build.

use clio_config::db_url_env::database_url_from_process_env;
use clio_types::{AmError, ErrorCode};

/// The scheme prefixes a Postgres connection URL may carry.
const PG_SCHEMES: [&str; 2] = ["postgres://", "postgresql://"];

/// The line a Postgres test prints when it cannot open the store.
///
/// Callers may format this note, but must not gate a skip on it alone: the
/// opt-out read lives in the gates below, and only [`fail_or_skip`] reaches the
/// note after the decision.
///
/// The signature is the guarantee, and it is deliberate: this function takes
/// the suite name and the driver's error, and has no parameter a connection
/// string could arrive through. `CLIO_DATABASE_URL` routinely carries a
/// password in its `user:password` userinfo, and test output reaches CI logs,
/// so a skip message that interpolated the URL would publish the credential to
/// anyone who can read the run. Printing the error instead is safe because a
/// connect failure reports the server's refusal (`FATAL: password
/// authentication failed for user "clio"`), never the address it was dialled
/// with.
#[must_use]
pub fn pg_skip_note(suite: &str, err: &AmError) -> String {
    format!("skipping {suite}: cannot open the Postgres store: {err}")
}

/// True only when the operator explicitly opted out of Postgres-backed tests.
///
/// Without the opt-out an unreachable server fails the suite: a skipped backend
/// is not a pass, so the store's Postgres suites cannot report green without
/// touching a database.
#[must_use]
pub fn skip_requested() -> bool {
    opted_out(std::env::var("CLIO_SKIP_PG_TESTS").ok().as_deref())
}

/// The opt-out spellings, as a pure read of the variable: a test cannot mutate
/// the process environment it shares with every other test in the run.
fn opted_out(value: Option<&str>) -> bool {
    matches!(value, Some("1" | "true" | "yes"))
}

/// Fail closed on an unreachable Postgres for `suite`: panic with the reason
/// unless the operator set `CLIO_SKIP_PG_TESTS`, which prints the
/// credential-safe skip note and lets the caller return early.
///
/// This is the only way a Postgres test may skip. The function owns the
/// `CLIO_SKIP_PG_TESTS` read, so a caller cannot skip without the operator's
/// opt-out; absent it, an unreachable store is a failure and never a pass. The
/// panic carries the driver's error, never the connection string.
pub fn unreachable_pg_or_skip(suite: &str, err: &AmError) {
    unreachable_pg_or_skip_with(suite, err, skip_requested());
}

/// [`unreachable_pg_or_skip`] with the opt-out decision already read.
///
/// The public gate reads `CLIO_SKIP_PG_TESTS` exactly once, but a test that
/// asserts on which branch it took would otherwise read the variable a second
/// time to choose its own expectation. A concurrent `set_var` between the two
/// reads could make them disagree and the test could false-pass or false-fail.
/// This seam lets that test read the decision once and thread the same value
/// through both its assertion and the gate. Crate-private on purpose: a
/// dependent crate keeps the entry that reads the environment.
pub(crate) fn unreachable_pg_or_skip_with(suite: &str, err: &AmError, skip: bool) {
    fail_or_skip(suite, err, skip);
}

/// Fail closed on a missing Postgres for `suite`, with `detail` naming what is
/// missing in place of a driver error.
///
/// The reachability half of [`unreachable_pg_or_skip`], for a suite that probes
/// with a plain reason string rather than an [`AmError`]. It owns the same
/// `CLIO_SKIP_PG_TESTS` read and the same strict default, so the two entries
/// cannot drift into two policies.
pub fn pg_required_or_skip(suite: &str, detail: &str) {
    let err = AmError::new(ErrorCode::Internal, detail.to_owned());
    fail_or_skip(suite, &err, skip_requested());
}

/// The configured Postgres URL, or a skip: a suite that runs only against a
/// configured database requests it here.
///
/// Returns the URL verbatim for the caller to dial, and never prints it. When
/// `CLIO_DATABASE_URL` is unset or names no Postgres database this fails closed
/// unless the operator opted out; the opt-out prints the credential-safe note
/// and yields `None`.
#[must_use]
pub fn pg_url_or_skip(suite: &str) -> Option<String> {
    match database_url_from_process_env() {
        Ok(url) if PG_SCHEMES.iter().any(|prefix| url.starts_with(prefix)) => Some(url),
        Ok(_) => {
            pg_required_or_skip(suite, "CLIO_DATABASE_URL is not a postgres:// URL");
            None
        }
        Err(_) => {
            pg_required_or_skip(suite, "CLIO_DATABASE_URL is not set");
            None
        }
    }
}

/// The decision, split from the environment read so both branches are
/// testable: `skip` prints the note, the strict default panics.
fn fail_or_skip(suite: &str, err: &AmError, skip: bool) {
    if skip {
        eprintln!("{}", pg_skip_note(suite, err));
        return;
    }
    panic!("{suite} require a reachable Postgres: {err}");
}

#[cfg(test)]
mod tests {
    use clio_types::{AmError, ErrorCode};

    use super::{fail_or_skip, opted_out, pg_skip_note};

    /// The connect refusal every test in this module reports.
    fn connect_error() -> AmError {
        AmError::new(
            ErrorCode::Internal,
            "postgres connect: FATAL: password authentication failed for user \"clio\"",
        )
    }

    /// Regression cover: eight Postgres test helpers printed the whole
    /// `CLIO_DATABASE_URL` in their skip message, so a run against an
    /// unreachable database published `user:password` to the test log.
    #[test]
    fn a_skip_note_never_prints_a_connection_string() {
        let note = pg_skip_note("postgres continuous tests", &connect_error());
        assert!(
            !note.contains("://"),
            "a skip note must carry no connection string: {note}"
        );
        assert!(
            !note.contains('@'),
            "a skip note must carry no userinfo: {note}"
        );
    }

    /// Dropping the URL is only worth it if the line still says why the test
    /// skipped, so the suite and the server's own words have to survive.
    #[test]
    fn a_skip_note_still_names_the_suite_and_the_servers_reason() {
        let note = pg_skip_note("postgres continuous tests", &connect_error());
        assert!(note.contains("postgres continuous tests"), "{note}");
        assert!(note.contains("password authentication failed"), "{note}");
    }

    /// The strict default: without the opt-out an unreachable server fails the
    /// suite, so a skipped backend can never read as a pass.
    #[test]
    #[should_panic(expected = "require a reachable Postgres")]
    fn an_unreachable_postgres_is_fatal_without_the_opt_out() {
        fail_or_skip("postgres peer action tests", &connect_error(), false);
    }

    /// The explicit opt-out is the only path that turns the unreachable server
    /// into a note and lets the caller return early.
    #[test]
    fn an_explicit_opt_out_turns_the_unreachable_postgres_into_a_note() {
        fail_or_skip("postgres peer action tests", &connect_error(), true);
    }

    /// Only the documented spellings opt out; anything else stays strict.
    #[test]
    fn only_the_documented_opt_out_spellings_skip() {
        for value in [Some("1"), Some("true"), Some("yes")] {
            assert!(opted_out(value), "{value:?} opts out");
        }
        for value in [None, Some("0"), Some("no"), Some("TRUE"), Some("")] {
            assert!(!opted_out(value), "{value:?} stays strict");
        }
    }
}
