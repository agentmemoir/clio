// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for proving that a connection opened through
//! `pg_tls::open` with `sslmode=encrypt-no-verify` writes the unverified
//! connection warning to the process stderr, so removing or misrouting that
//! emission fails a test instead of passing quietly.
//!
//! ## Owns
//! - The re-executed child that drives the real connection path, and the parent
//!   that asserts the warning fields on the child's captured stderr
//! - The child that proves a level above `warn` defers the warning instead of
//!   spending the once-per-process guard
//!
//! ## Does not own
//! - Which modes warn (`pg_tls_warning` owns that decision)
//! - The connector each mode selects (`pg_tls` owns that)
//! - The log line format (`clio-config` owns it)
//!
//! ## Boundary
//!
//! Changes to where the warning is emitted, or to what it says, should normally
//! require changes here. Changes to the log-level machinery should not.
//!
//! Keep this module focused on the warning reaching stderr from the real
//! connection path.

use std::process::{Command, Stdio};

use clio_config::log::{LogLevel, set_level};

/// Set only in the re-executed child, so the parent cannot recurse into itself.
const CHILD_MARKER: &str = "CLIO_STORE_WARNING_EMISSION_CHILD";

/// The name fragment used to run exactly this test in the child process.
const TEST_NAME: &str = "an_unverified_connection_warns_on_stderr";

/// A loopback port nothing listens on. The connect attempt is refused at once,
/// so the test needs no database, no name resolution, and no network, and it
/// never waits on a timeout.
const REFUSED_URL: &str =
    "postgres://127.0.0.1:1/memories?sslmode=encrypt-no-verify&connect_timeout=1";

/// The warning is written by the connection path itself, and the fields an
/// operator greps for are on the line.
///
/// The test re-executes this binary and runs only the child branch below, which
/// calls `pg_tls::open` for an unverified URL and ignores the connection error
/// that a closed port produces. The parent then asserts on the child's stderr,
/// which no other test can write to because the child runs this test alone.
#[test]
fn an_unverified_connection_warns_on_stderr() {
    if std::env::var_os(CHILD_MARKER).is_some() {
        open_and_ignore_the_refusal();
        return;
    }

    let exe = std::env::current_exe().expect("the test binary has a path");
    let output = Command::new(exe)
        // `--nocapture` disables the harness's own stderr capture, so the log
        // line reaches the process stderr this parent reads.
        .args([TEST_NAME, "--nocapture"])
        .env(CHILD_MARKER, "1")
        // The level is pinned here as well as in the child, so neither a host's
        // environment nor a future default can silence the line.
        .env("CLIO_LOG_LEVEL", "info")
        .stdin(Stdio::null())
        .output()
        .expect("re-execute the test binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the child run failed: {}: {stderr}",
        output.status
    );
    assert!(
        stdout.contains("1 passed"),
        "the child must run exactly one test: {stdout}"
    );
    assert!(
        stderr.contains("[warn]"),
        "the unverified connection must log a warning: {stderr:?}"
    );
    for field in [
        "sslmode=encrypt-no-verify",
        "certificate_validation=disabled",
    ] {
        assert!(
            stderr.contains(field),
            "the warning line is missing `{field}`: {stderr:?}"
        );
    }
}

/// Drive the real connection path in the child process.
///
/// The connection is expected to fail, because nothing is listening on the
/// port; the warning is written before the attempt, and that is the only thing
/// this process is here to produce.
fn open_and_ignore_the_refusal() {
    set_level(LogLevel::Info);
    let _ = crate::pg_tls::open(REFUSED_URL);
}

/// Set only in the child that drives the level transition.
const LEVEL_CHILD_MARKER: &str = "CLIO_STORE_WARNING_LEVEL_CHILD";

/// The name fragment used to run exactly the level-transition test in the child.
const LEVEL_TEST_NAME: &str = "a_level_above_warn_defers_the_warning_until_the_level_allows_it";

/// A level above `warn` must not spend the once-per-process guard.
///
/// The child opens the unverified URL while the process level hides warnings,
/// then raises the level and opens it again. The parent requires the warning on
/// stderr exactly once, so a guard taken on the suppressed call fails the test:
/// that call would have left nothing written and nothing for the second call to
/// emit.
#[test]
fn a_level_above_warn_defers_the_warning_until_the_level_allows_it() {
    if std::env::var_os(LEVEL_CHILD_MARKER).is_some() {
        open_hidden_and_then_raise_the_level();
        return;
    }

    let exe = std::env::current_exe().expect("the test binary has a path");
    let output = Command::new(exe)
        .args([LEVEL_TEST_NAME, "--nocapture"])
        .env(LEVEL_CHILD_MARKER, "1")
        .stdin(Stdio::null())
        .output()
        .expect("re-execute the test binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the child run failed: {}: {stderr}",
        output.status
    );
    assert!(
        stdout.contains("1 passed"),
        "the child must run exactly one test: {stdout}"
    );
    assert!(
        stderr.contains("[warn]"),
        "raising the level must still emit the deferred warning: {stderr:?}"
    );
    assert_eq!(
        stderr.matches("sslmode=encrypt-no-verify").count(),
        1,
        "the warning must appear once, from the call the level allowed: {stderr:?}"
    );
}

/// Open the unverified URL while warnings are hidden, then raise the level.
fn open_hidden_and_then_raise_the_level() {
    set_level(LogLevel::Error);
    let _ = crate::pg_tls::open(REFUSED_URL);
    set_level(LogLevel::Info);
    let _ = crate::pg_tls::open(REFUSED_URL);
}
