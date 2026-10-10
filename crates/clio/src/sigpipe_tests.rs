// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the process's `SIGPIPE` disposition helpers: each installs
//! the disposition it names.
//!
//! ## Owns
//!
//! - Disposition assertions for `restore_default` and `ignore`
//!
//! ## Does not own
//!
//! - How the kernel delivers the signal, or the signal number (`libc`)
//!
//! ## Boundary
//!
//! Tests here break when the module's disposition contract changes; they never
//! change it.
//!
//! `SIGPIPE` is a Unix signal, so this whole file is Unix-only.

#![cfg(unix)]

/// The `SIGPIPE` handler installed right now, read back by installing one and
/// returning the previous value.
#[allow(unsafe_code)]
fn current() -> libc::sighandler_t {
    // SAFETY: reading a disposition by re-installing it; the disposition this
    // test wants is set explicitly immediately after each call.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) }
}

/// Each helper installs the disposition it names.
///
/// The dispositions are process-global, so this test is the only place that
/// changes them: it starts by setting the non-fatal one, checks the other, and
/// leaves the process at Rust's own default. No other test in this crate writes
/// to a pipe whose reader has gone, so the brief `SIG_DFL` window cannot end the
/// test process.
#[test]
fn each_helper_sets_the_disposition_it_names() {
    super::ignore();
    assert_eq!(current(), libc::SIG_IGN);
    super::restore_default();
    assert_eq!(current(), libc::SIG_DFL);
    super::ignore();
    assert_eq!(current(), libc::SIG_IGN);
}
