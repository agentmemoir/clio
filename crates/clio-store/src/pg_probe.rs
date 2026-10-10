// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the setup-time connection test: open the
//! database the operator just named, wait out a server that is still starting,
//! and report what is there.
//!
//! ## Owns
//! - The retry window, and the rule for which failures fall inside it
//! - The [`DatabaseReport`] a caller reads: server version, and whether
//!   pgvector is installed
//!
//! ## Does not own
//! - The connector or the SSL mode (`pg_tls` owns both)
//! - Whether a probed database is good enough to keep: the report is a value
//!   and the caller decides what a missing extension means
//! - Waiting for a container at all: that belongs to whoever started it
//!
//! ## Boundary
//!
//! Changes to what a connection test reports, or to how long it waits, should
//! normally require changes here. Changes to the connector should not require
//! changes here.
//!
//! Keep this module focused on the probe and its report.

use crate::pg_tls::{Failure, describe, open};
use std::time::Duration;

/// How many times [`probe`] opens a connection before it gives up.
///
/// A container that has just been started refuses the socket for a while, and a
/// data folder that has never been used is initialised on that first boot, so
/// one attempt is not a fair test of a database that is on its way.
pub(crate) const PROBE_ATTEMPTS: u32 = 30;

/// Pause between [`probe`] attempts.
pub(crate) const PROBE_RETRY_DELAY: Duration = Duration::from_millis(500);

/// The one statement the probe runs: the server version and whether pgvector is
/// installed, as two columns in the order [`DatabaseReport`] reads them.
const REPORT_SQL: &str = "SELECT current_setting('server_version'), EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector')";

/// What a connection test found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseReport {
    /// `server_version`, as the server reported it.
    pub server_version: String,
    /// Whether the `vector` extension is installed in this database.
    ///
    /// A `false` here is a value and not an error. What to do about a database
    /// without pgvector is the caller's decision, and setup is the caller that
    /// makes it.
    pub has_vector: bool,
}

/// Open a connection, retrying a refused socket, and report what is there.
///
/// A server that *answered* — a wrong password, a database that does not exist —
/// is not retried, because it will answer the same way every time, and neither
/// is a client-side TLS failure. The last reason is returned when the attempts
/// run out.
///
/// # Errors
/// The last connection or query failure, as a message an operator can act on.
pub fn probe(url: &str) -> Result<DatabaseReport, String> {
    probe_within(url, PROBE_ATTEMPTS, PROBE_RETRY_DELAY)
}

/// The probe with its window and pause given by the caller.
///
/// [`probe`] passes the shipped constants. A test passes a short window and no
/// pause, so the retry path and the give-up message are both reachable without
/// a test that sits out the real fifteen seconds.
pub(crate) fn probe_within(
    url: &str,
    attempts: u32,
    pause: Duration,
) -> Result<DatabaseReport, String> {
    retry_until_ready(
        attempts,
        || probe_once(url),
        || {
            std::thread::sleep(pause);
        },
    )
    .map_err(|(reason, report)| {
        // The count is here because a wait the operator did not ask for is
        // otherwise invisible: "could not connect" after fifteen seconds reads
        // like a hang, and saying so tells them the container never answered.
        if report.made > 1 {
            format!("{reason} (gave up after {} attempts)", report.made)
        } else {
            reason
        }
    })
}

/// How many attempts the probe actually made, and the last reason it failed.
///
/// Production reads the count to say how hard it tried before giving up, and a
/// test asserts it directly, which is the whole content of "a refused socket is
/// retried and an answered one is not". Counting is a fact; measuring elapsed
/// time is a guess about a fact, and on a loaded machine the guess is wrong.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Attempts {
    /// How many times an attempt was actually made.
    pub(crate) made: u32,
    /// The last reason reported, which is the one a caller would have read.
    pub(crate) last: String,
}

/// The retry loop, with the pause and the attempt injected.
///
/// Production passes [`std::thread::sleep`] and [`probe_once`]. A test passes a
/// counter and a pause that does not pause, and then knows the attempt count
/// exactly instead of inferring it from how long the call took.
pub(crate) fn retry_until_ready<T, A, P>(
    attempts: u32,
    mut attempt: A,
    mut pause: P,
) -> Result<T, (String, Attempts)>
where
    A: FnMut() -> Result<T, Failure>,
    P: FnMut(),
{
    let mut made = 0;
    loop {
        made += 1;
        let failure = match attempt() {
            Ok(value) => return Ok(value),
            Err(failure) => failure,
        };
        let report = Attempts {
            made,
            last: failure.message.clone(),
        };
        if !failure.retryable || made >= attempts {
            return Err((failure.message, report));
        }
        pause();
    }
}

/// One attempt against the running server.
pub(crate) fn probe_once(url: &str) -> Result<DatabaseReport, Failure> {
    let mut client = open(url)?;
    // One round trip for both answers. A second query would mean a second thing
    // that can fail after the connection opened, and a half-answered report is
    // worse for an operator than no report.
    //
    // The error is a `match` arm rather than a `map_err` closure, and that is
    // deliberate: a closure is a function of its own to the coverage report, and
    // this one runs only when the server refuses the statement. No test can make
    // it run — the statement reads only `current_setting` and `pg_extension`,
    // which every role may always read, so there is no address, setting, or
    // timeout that makes a healthy server refuse it. As a `match` arm the path
    // belongs to the function that owns it, and the coverage report states
    // plainly that it was not exercised rather than reporting a phantom function
    // as dead code.
    //
    // A failed query is not retried: the connection was open, so whatever the
    // server disliked will not change. The reason is the server's own, because
    // "the probe could not read the version" is not something an operator can
    // act on and the server's words are.
    let row = match client.query_one(REPORT_SQL, &[]) {
        Ok(row) => row,
        Err(e) => {
            return Err(Failure::settled(format!(
                "read the server: {}",
                describe(&e)
            )));
        }
    };
    Ok(DatabaseReport {
        server_version: row.get(0),
        has_vector: row.get(1),
    })
}
