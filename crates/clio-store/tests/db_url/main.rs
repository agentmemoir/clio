// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the entry point of one target: hand one
//! injected environment to every database-address read path a running binary
//! has, and require one answer from each.
//!
//! ## Owns
//! - The declarations that give this target its subject files
//! - How many read paths this target reaches, and which two it cannot
//!
//! ## Does not own
//! - Which variable names a database (see the config crate's `db_url_env`)
//! - Overlay wiring and precedence (see the config crate's `config::env` and
//!   `db_path`)
//! - Store opening and the shared-database test lock (see the store's
//!   `bootstrap` and `pg_lock`)
//!
//! ## Boundary
//!
//! Changes to the set of read paths under test should normally require changes
//! here. Changes to how one read path resolves a value should not require
//! changes here.
//!
//! Keep this module focused on the `db_url` target's entry point.
//!
//! ## How many paths this target reaches
//!
//! Three of the five. The config overlay, the database path resolver and the
//! store bootstrap all take an injected lookup, so all three run here.
//!
//! The store's own shared-test database URL helper and the store's shared
//! database lock cannot be reached from an integration target. The first lives
//! in a module the crate does not export, so an external crate cannot name it at
//! all; the second reads the real process environment and offers no lookup
//! parameter, so the only way to drive it is to mutate a global, which races
//! every other test in the binary. Neither is faked here: a stand-in that
//! reimplemented the rule would pass whether or not the real code still obeyed
//! it, which is the one thing this target exists to catch.

/// Every declaration below carries an explicit `#[path]`.
///
/// A bare `mod` declared from a target entry point resolves against the parent
/// `tests/` directory rather than beside this file, so the subject files would
/// never be found, never compiled, and never reported missing.
#[path = "db_url_env_test_support.rs"]
mod db_url_env_test_support;

#[path = "prefixed_name_tests.rs"]
mod prefixed_name_tests;

#[path = "absent_name_tests.rs"]
mod absent_name_tests;

#[path = "old_name_ignored_tests.rs"]
mod old_name_ignored_tests;
