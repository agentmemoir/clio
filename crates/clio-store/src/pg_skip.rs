// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the one wording every Postgres test uses when
//! it skips because the store could not be opened, so that message can never
//! carry the connection string.
//!
//! ## Owns
//! - `pg_skip_note`, the only way a Postgres test reports "could not open"
//!
//! ## Does not own
//! - Whether a test skips at all, or under what condition (each test's own
//!   `open_pg`)
//! - The connection itself, and any redaction of a URL an operator typed
//!
//! ## Boundary
//!
//! Changes to how a Postgres skip is reported should normally require changes
//! here. Changes to a test's skip condition should not require changes here.
//!
//! Test-only: the skip note is a test diagnostic and is never compiled into a
//! production build.

use clio_types::AmError;

/// The line a Postgres test prints when it cannot open the store.
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
pub(crate) fn pg_skip_note(suite: &str, err: &AmError) -> String {
    format!("skipping {suite}: cannot open the Postgres store: {err}")
}

#[cfg(test)]
mod tests {
    use clio_types::{AmError, ErrorCode};

    use super::pg_skip_note;

    /// Regression cover for issue #37: eight Postgres test helpers printed the
    /// whole `CLIO_DATABASE_URL` in their skip message, so a run against an
    /// unreachable database published `user:password` to the test log.
    #[test]
    fn a_skip_note_never_prints_a_connection_string() {
        let err = AmError::new(
            ErrorCode::Internal,
            "postgres connect: FATAL: password authentication failed for user \"clio\"",
        );
        let note = pg_skip_note("postgres continuous tests", &err);
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
        let err = AmError::new(
            ErrorCode::Internal,
            "postgres connect: FATAL: password authentication failed for user \"clio\"",
        );
        let note = pg_skip_note("postgres continuous tests", &err);
        assert!(note.contains("postgres continuous tests"), "{note}");
        assert!(note.contains("password authentication failed"), "{note}");
    }
}
