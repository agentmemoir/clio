// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing the setup-time probe: what it reports
//! from a running server, and what an operator reads when the server is not
//! there.
//!
//! ## Owns
//! - The report a database with the extension produces: version, `vector`, and
//!   transport
//! - How many attempts the give-up message names, and when it names none
//!
//! ## Does not own
//! - The connector each SSL mode picks (`pg_tls_tests` owns that)
//! - Which real refusals are retried (`pg_tls_db_tests` owns that)
//! - Store schema or query behavior
//!
//! ## Boundary
//!
//! Changes to what a probe reports, or to what its give-up message says, should
//! normally require changes here. Changes to the connector should not require
//! changes here.

use crate::db_url::postgres_url_from_env;
use crate::pg_fixtures::{ScratchDatabase, url_for_database};
use crate::pg_lock::lock_pg_test;
use crate::pg_probe::{probe, probe_within};
use crate::pg_tls::connect;
use std::time::Duration;

#[test]
fn the_probe_reports_the_version_and_the_extension() {
    let _guard = lock_pg_test();
    // A server can ship pgvector without installing it, so the test installs the
    // extension in a database of its own rather than assuming the shared one
    // already has it.
    let mut admin = connect(&postgres_url_from_env()).expect("admin connect");
    let scratch = ScratchDatabase::create(&mut admin, "vector");
    drop(admin);

    let scratch_url = url_for_database(&scratch.name);
    let mut scratch_admin = connect(&scratch_url).expect("scratch admin connect");
    scratch_admin
        .batch_execute("CREATE EXTENSION IF NOT EXISTS vector")
        .expect("install pgvector in the scratch database");
    drop(scratch_admin);

    let report = probe(&scratch_url).expect("the scratch database is up");
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
    assert!(
        report.has_vector,
        "the scratch database installed the extension"
    );
    // The version and cipher are present exactly when the connection is
    // encrypted, so a report cannot claim one half of the TLS answer.
    assert_eq!(
        report.tls_version.is_some(),
        report.tls,
        "the TLS version is reported exactly when the connection is encrypted"
    );
    assert_eq!(
        report.tls_cipher.is_some(),
        report.tls,
        "the cipher is reported exactly when the connection is encrypted"
    );
}

#[test]
fn the_wrappers_report_the_attempt_count_only_when_they_waited() {
    let _guard = lock_pg_test();
    // A port nothing listens on refuses at once, so three short attempts reach
    // the give-up message without waiting out the shipped window.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a closed port");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let url = format!("postgres://clio:clio@127.0.0.1:{port}/clio?connect_timeout=2");

    let reason = probe_within(&url, 3, Duration::ZERO)
        .expect_err("nothing is listening, so no attempt can succeed");
    assert!(
        reason.contains("gave up after 3 attempts"),
        "the wait must say how hard the probe tried: {reason}"
    );

    // A failure the client refuses before a socket exists settles on the first
    // attempt, so the message must not claim a wait that never happened.
    let reason = probe_within("postgres://u@/clio", 3, Duration::ZERO)
        .expect_err("no host is not a connection");
    assert!(
        !reason.contains("gave up after"),
        "a first-attempt failure has no wait to report: {reason}"
    );
}
