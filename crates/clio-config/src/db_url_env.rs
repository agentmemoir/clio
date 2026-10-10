// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for one thing: the name of the environment
//! variable that points at a database, and reading that one name the one way.
//!
//! ## Owns
//! - [`DATABASE_URL_ENV`], the single variable name
//! - [`database_url_from`] for an injected lookup and
//!   [`database_url_from_process_env`] for the real environment
//!
//! ## Does not own
//! - Where a database URL sits in the precedence chain (see `db_path`)
//! - The process-env overlay that folds the URL into a config document (see
//!   `config::env`)
//! - The Compose fallback the shared-Postgres tests use (see `clio-store`)
//!
//! ## Boundary
//!
//! Changes to which variable names a database should normally require changes
//! here and nowhere else. Changes to precedence, config merging, or store
//! opening should not require changes here.
//!
//! Keep this module focused on naming and reading the database variable.

use crate::db_path::env_value;

/// The environment variable that names a database.
///
/// There is one name and no second spelling to fall back on. A fallback is how
/// two code paths come to read two different databases out of one shell, with
/// nothing printed to say which one each read.
pub const DATABASE_URL_ENV: &str = "CLIO_DATABASE_URL";

/// Resolve the database URL from an injected environment lookup.
///
/// `None` means the variable is unset, blank, or carries an interior NUL. Each
/// of those leaves a caller with nothing openable, so they count as absent and
/// the caller's own next precedence tier applies. The lookup is injected rather
/// than read from the process so no caller and no test has to touch a global.
/// It arrives by value, so a shared reference to a non-mutating lookup and a
/// mutable reference to a boxed one are both accepted.
#[must_use]
pub fn database_url_from<F>(get: F) -> Option<String>
where
    F: FnMut(&str) -> Option<String>,
{
    env_value(get, DATABASE_URL_ENV)
}

/// Read the database URL from the real process environment.
///
/// # Errors
/// The variable is not set, or it is not valid Unicode. A caller that
/// substitutes its own default for a missing value hands this straight through.
/// The returned `Result` is already must-use, so this one carries no attribute.
pub fn database_url_from_process_env() -> Result<String, std::env::VarError> {
    std::env::var(DATABASE_URL_ENV)
}

#[cfg(test)]
#[path = "db_url_env_tests.rs"]
mod tests;
