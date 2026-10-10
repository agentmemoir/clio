// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the store's connector against a
//! running Postgres: which modes connect, which are refused, and what the setup
//! probe reports.
//!
//! ## Owns
//! - `disable` and `prefer` reaching a server that offers no TLS
//! - `require` being refused by such a server, and not retried
//! - The probe's version and `vector` report, including a database without the
//!   extension
//! - The failures that must not be retried, and the error text one produces
//!
//! The shared server uses trust authentication and has no password, so no test
//! here can provoke an authentication refusal. Refusals that the server still
//! names — a database that does not exist, a bad address, a client-side TLS
//! refusal — carry the same meaning to the code under test, because the driver
//! reports all of them as a database error or a client refusal.
//!
//! ## Does not own
//! - Reading a connection string (`pg_tls_tests` owns that)
//! - Store schema or query behavior
//! - The shared test-database guard (`pg_lock` owns it)
//!
//! ## Boundary
//!
//! Changes to the connector decision or the probe should normally require
//! changes here. Changes to the parse should not require changes here.

use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::pg_probe::{
    PROBE_ATTEMPTS, PROBE_RETRY_DELAY, probe, probe_once, probe_within, retry_until_ready,
};
use crate::pg_tls::connect;
use std::time::Duration;

/// The shared URL with a query string appended, so the database path is not
/// doubled.
fn url_with_query(query: &str) -> String {
    format!("{}?{query}", server_url())
}

/// The shared URL pointed at a different database on the same server.
fn url_for_database(name: &str) -> String {
    format!("{}/{name}", server_url())
}

/// The shared URL with its database name removed: server, port, and credentials.
fn server_url() -> String {
    let base = postgres_url_from_env();
    base.rsplit_once('/')
        .map_or(base.as_str(), |(server, _)| server)
        .to_owned()
}

#[test]
fn disable_and_prefer_both_reach_a_plain_server() {
    let _guard = lock_pg_test();
    for mode in ["disable", "prefer"] {
        let mut client = connect(&url_with_query(&format!("sslmode={mode}")))
            .unwrap_or_else(|e| panic!("sslmode={mode} must reach a plain server: {e}"));
        let one: i32 = client
            .query_one("SELECT 1", &[])
            .unwrap_or_else(|e| panic!("query after sslmode={mode}: {e}"))
            .get(0);
        assert_eq!(one, 1, "sslmode={mode}");
    }
}

#[test]
fn require_against_a_plain_server_is_the_drivers_tls_refusal() {
    let _guard = lock_pg_test();
    let url = url_with_query("sslmode=require");
    let text = match connect(&url) {
        Ok(_) => panic!("a plain server cannot satisfy require"),
        Err(err) => err.to_string(),
    };
    // The server's own answer, not a timeout and not a failure to open the
    // socket: this is a decision the server made after the connection opened.
    assert!(
        text.contains("does not support TLS"),
        "expected the server's TLS refusal, got: {text}"
    );
    // A handshake failure is on the list of things not worth repeating, and the
    // attempt count is what proves the list still matches this driver's wording.
    let (_, report) = retry_until_ready(3, || probe_once(&url), || {})
        .expect_err("a plain server cannot satisfy require");
    assert_eq!(report.made, 1, "a TLS refusal must not be retried");
}

#[test]
fn prefer_with_direct_negotiation_is_refused_by_the_driver() {
    let _guard = lock_pg_test();
    // The driver is the one that knows this combination is meaningless, so its
    // message is what the operator reads.
    let text = match connect(&url_with_query("sslmode=prefer&sslnegotiation=direct")) {
        Ok(_) => panic!("prefer and direct cannot be combined"),
        Err(err) => err.to_string(),
    };
    assert!(text.contains("sslnegotiation=direct"), "{text}");
    assert!(text.contains("prefer"), "{text}");
}

#[test]
fn a_refusal_the_server_names_says_why_and_is_not_retried() {
    let _guard = lock_pg_test();
    // A database that does not exist, not a wrong password. The managed database
    // uses trust authentication and has no password, so it can no longer refuse
    // a credential — and a test that still reached for a wrong password would
    // connect, not fail. This refusal is the same thing to the code under test:
    // the server answered, so the driver reports a database error, and an
    // answered refusal is never worth repeating.
    let url = url_for_database("no_such_database_for_this_test");

    let (reason, report) = retry_until_ready(3, || probe_once(&url), || {})
        .expect_err("a database that does not exist does not connect");

    // The server's own reason, not the driver's bare placeholder.
    assert!(
        reason.contains("does not exist"),
        "expected the server's reason, got: {reason}"
    );
    assert!(
        !reason.contains("db error"),
        "the bare placeholder must not be what the operator reads: {reason}"
    );
    // A server that answered is not a server that is still starting, so the
    // attempt count is read rather than inferred from elapsed time.
    assert_eq!(report.made, 1, "an answered refusal must not be retried");
}

#[test]
fn a_missing_host_is_a_configuration_failure_and_is_not_retried() {
    let _guard = lock_pg_test();
    let (reason, report) = retry_until_ready(3, || probe_once("postgres://u@/clio"), || {})
        .expect_err("no host is not a connection");
    // The driver's own wording, and the reason under it: an operator reading
    // "invalid configuration: both host and hostaddr are missing" knows what to
    // type next, and reading only the first half would not tell them.
    assert!(reason.contains("invalid configuration"), "{reason}");
    assert!(reason.contains("missing"), "{reason}");
    assert_eq!(report.made, 1, "a bad address must not be retried");
}

#[test]
fn the_probe_reports_the_version_and_the_extension() {
    let _guard = lock_pg_test();
    let report = probe(&postgres_url_from_env()).expect("the shared database is up");
    assert!(
        !report.server_version.is_empty(),
        "{:?}",
        report.server_version
    );
    let major: u32 = report
        .server_version
        .split('.')
        .next()
        .and_then(|part| part.parse().ok())
        .unwrap_or_else(|| panic!("unreadable version {:?}", report.server_version));
    assert!(major >= 14, "unexpected server version {major}");
    assert!(report.has_vector, "the compose database installs pgvector");
}

/// A database created for one test, dropped when the test ends — including when
/// it ends in a panic.
///
/// A test that creates something and only removes it on the happy path leaves
/// that thing behind for every later run, and the failure that does it is exactly
/// the failure a developer most needs a clean machine to investigate.
struct ScratchDatabase {
    name: String,
}

impl ScratchDatabase {
    /// Create a database named after this test run, after clearing any leftover
    /// from a previous run that did not clean up.
    fn create(admin: &mut postgres::Client, label: &str) -> Self {
        let name = format!("clio_probe_{label}_{}", std::process::id());
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS {name}"))
            .expect("drop a leftover scratch database");
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .expect("create a scratch database");
        Self { name }
    }
}

impl Drop for ScratchDatabase {
    fn drop(&mut self) {
        // A fresh admin connection: the one the test used may be the very client
        // still holding the scratch database open.
        if let Ok(mut admin) = connect(&postgres_url_from_env()) {
            let _ = admin.batch_execute(&format!("DROP DATABASE IF EXISTS {}", self.name));
        }
    }
}

#[test]
fn a_database_without_the_extension_is_a_value_not_an_error() {
    let _guard = lock_pg_test();
    let mut admin = connect(&postgres_url_from_env()).expect("admin connect");
    let scratch = ScratchDatabase::create(&mut admin, "novector");
    drop(admin);

    let report = probe(&url_for_database(&scratch.name))
        .expect("a database without pgvector still connects");
    assert!(!report.has_vector, "the fresh database has no extension");
    assert!(
        !report.server_version.is_empty(),
        "{:?}",
        report.server_version
    );
}

#[test]
fn a_refused_socket_is_retried_and_the_last_reason_is_reported() {
    let _guard = lock_pg_test();
    // A port nothing is listening on is the failure the retry window exists for:
    // a container that has just started refuses the socket for a while and
    // accepts it a moment later.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a closed port");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let url = format!("postgres://clio:clio@127.0.0.1:{port}/clio?connect_timeout=2");

    // The attempt count is read, not measured: this drives the real loop with a
    // pause that does not pause, so the number of tries is a fact rather than
    // something inferred from how long the call took.
    let (reason, report) = retry_until_ready(3, || probe_once(&url), || {})
        .expect_err("nothing is listening, so no attempt can succeed");

    assert_eq!(report.made, 3, "a refused socket must be retried");
    assert_eq!(report.last, reason, "the last reason is the one reported");
    // The reason is the one an operator can act on: the server is not there.
    assert!(reason.contains("postgres connect"), "got: {reason}");
}

#[test]
fn a_url_the_driver_refuses_never_opens_a_socket() {
    let _guard = lock_pg_test();
    // The driver rejects the string before any address is resolved, so this is a
    // parse failure rather than a connection failure. It is reported, and it is
    // not retried: nothing about it can change between attempts.
    for bad in [
        "postgres://clio:clio@127.0.0.1:notaport/clio",
        "postgres://clio:clio@[::1/clio",
        "not a connection string at all",
    ] {
        let (reason, report) =
            retry_until_ready(3, || probe_once(bad), || {}).expect_err("the driver refuses it");
        assert!(
            reason.contains("postgres url is not usable"),
            "{bad}: {reason}"
        );
        assert_eq!(report.made, 1, "{bad} must fail on the first attempt");
    }
}

#[test]
fn a_refused_socket_reaches_the_operator_with_the_attempt_count() {
    let _guard = lock_pg_test();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a closed port");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let url = format!("postgres://clio:clio@127.0.0.1:{port}/clio?connect_timeout=2");

    // Three attempts and no pause, so the retry path runs without a test that
    // sits out the shipped fifteen-second window.
    let reason = probe_within(&url, 3, Duration::ZERO)
        .expect_err("nothing is listening, so no attempt can succeed");

    // An operator who waited on a container that never answered needs to be told
    // the probe tried and gave up, not left reading a bare refusal.
    assert!(
        reason.contains("gave up after 3 attempts"),
        "the wait must say how hard the probe tried: {reason}"
    );
    assert!(reason.contains("postgres connect"), "got: {reason}");
}

#[test]
fn a_settled_failure_is_reported_without_an_attempt_count() {
    let _guard = lock_pg_test();
    // A missing host is refused before a socket is opened, so the probe gives up
    // on the first try. There is no wait to account for, and claiming one would
    // send the operator looking for a container that was never involved.
    let reason = probe_within("postgres://u@/clio", 3, Duration::ZERO)
        .expect_err("no host is not a connection");
    assert!(
        !reason.contains("gave up after"),
        "a first-attempt failure has no wait to report: {reason}"
    );
    assert!(reason.contains("invalid configuration"), "{reason}");
}

#[test]
fn the_retry_window_is_bounded() {
    // Nothing here connects: this is the arithmetic of the loop, so a future
    // edit that widened the window or dropped the bound would show up.
    // A constant, so the guard is decided when the crate is built rather than
    // when the test runs: changing the window fails the build, not one run.
    const {
        assert!(PROBE_ATTEMPTS > 1, "one attempt is not a retry");
    }
    let window = PROBE_RETRY_DELAY * (PROBE_ATTEMPTS - 1);
    assert!(
        window <= Duration::from_secs(30),
        "the setup wait must stay under half a minute, got {window:?}"
    );
}
