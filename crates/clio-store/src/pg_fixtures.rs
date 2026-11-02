// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the fixtures that tests against a real
//! Postgres share: the scratch database a test creates, and the URLs that point
//! at it.
//!
//! ## Owns
//! - [`ScratchDatabase`], which drops its database when the test ends
//! - [`url_with_query`] and [`url_for_database`]
//!
//! ## Does not own
//! - Test serialization against the shared database (`pg_lock` owns it)
//! - Any test case, which lives in the module that runs it
//!
//! ## Boundary
//!
//! Changes to how a scratch database is created or dropped should normally
//! require changes here. Changes to what a test asserts should not.
//!
//! Keep this module focused on shared Postgres fixtures.
//!
//! Test-only. Never compiled into production builds.

use crate::db_url::{postgres_url_from_env, with_database};
use crate::pg_tls::connect;

/// The shared URL's server, port, and credentials with `query` as the whole
/// query string, so the database path is not doubled.
pub(crate) fn url_with_query(query: &str) -> String {
    let base = postgres_url_from_env();
    let path = base.split_once('?').map_or(base.as_str(), |(p, _)| p);
    let server = path.rsplit_once('/').map_or(path, |(s, _)| s);
    format!("{server}?{query}")
}

/// The shared URL pointed at a different database on the same server.
pub(crate) fn url_for_database(name: &str) -> String {
    with_database(&postgres_url_from_env(), name)
}

/// A database created for one test, dropped when the test ends — including when
/// it ends in a panic.
///
/// A test that creates something and only removes it on the happy path leaves
/// that thing behind for every later run, and the failure that does it is exactly
/// the failure a developer most needs a clean machine to investigate.
pub(crate) struct ScratchDatabase {
    pub(crate) name: String,
}

impl ScratchDatabase {
    /// Create a database named after this test run, after clearing any leftover
    /// from a previous run that did not clean up.
    pub(crate) fn create(admin: &mut postgres::Client, label: &str) -> Self {
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
