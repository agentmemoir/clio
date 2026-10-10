// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for resolving the shared Postgres test URL: the
//! canonical Compose database, and the one environment variable that overrides
//! it.
//!
//! ## Owns
//! - `DEFAULT_DATABASE_URL` and the absent-value → default mapping
//!
//! ## Does not own
//! - The variable's name and how it is read (`clio_config`'s `db_url_env`)
//! - Backend selection or store bootstrap (`bootstrap` / `postgres`)
//! - Test serialization against that database (`pg_lock`)
//!
//! ## Boundary
//!
//! Changes to the canonical test database name should normally require changes
//! here (and in the Makefile, `.env.example`, and the coverage playbook).
//! Changes to lock mechanics or store behavior should not require changes here.
//!
//! Keep this module focused on shared-test-database URL resolution.

/// Canonical local Compose DB (matches `.env.example` / `POSTGRES_DB`).
pub(crate) const DEFAULT_DATABASE_URL: &str = "postgres://clio:clio@127.0.0.1:34310/clio";

/// Resolve an already-read database URL, falling back to
/// [`DEFAULT_DATABASE_URL`] when it is absent.
///
/// The read happens in [`clio_config::db_url_env`], so this half only decides
/// what a missing value becomes. It is the same substitution every shared-Postgres
/// test makes, which is why the parameter stays a `Result` rather than becoming
/// a second env lookup.
#[must_use]
pub(crate) fn postgres_url_or_default(var: Result<String, std::env::VarError>) -> String {
    var.unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned())
}

/// Resolve the shared test database URL from the process environment.
#[cfg(test)]
pub(crate) fn postgres_url_from_env() -> String {
    postgres_url_or_default(clio_config::db_url_env::database_url_from_process_env())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clio_config::db_url_env::{
        DATABASE_URL_ENV, database_url_from, database_url_from_process_env,
    };

    #[test]
    fn a_read_value_is_used_as_is() {
        assert_eq!(
            postgres_url_or_default(Ok("postgres://u:p@host:5432/am".to_owned())),
            "postgres://u:p@host:5432/am"
        );
    }

    #[test]
    fn an_absent_value_falls_back_to_the_compose_database() {
        assert_eq!(
            postgres_url_or_default(Err(std::env::VarError::NotPresent)),
            DEFAULT_DATABASE_URL
        );
        assert_eq!(
            postgres_url_or_default(Err(std::env::VarError::NotUnicode(
                std::ffi::OsString::from("postgres://\u{fffd}/am")
            ))),
            DEFAULT_DATABASE_URL,
            "a value the OS could not hand over as text is absent, not a URL"
        );
    }

    /// The store's own env read and the config crate's owner must name the same
    /// variable. Both sides read the same host environment, so this holds
    /// whatever the host exports; it pins the name, not the value.
    #[test]
    fn the_store_reads_the_variable_the_config_crate_owns() {
        let from_store = database_url_from_process_env();
        let from_owner = database_url_from(&|key: &str| std::env::var(key).ok());
        match (&from_store, &from_owner) {
            (Ok(store), Some(owner)) => assert_eq!(store, owner, "two different names were read"),
            (Err(_), None) => {}
            _ => panic!("the store and the config crate read different variables"),
        }
        assert!(DATABASE_URL_ENV.starts_with("CLIO_"));
    }

    /// The helper a shared-Postgres test opens the database with, resolved the
    /// same way as a direct read of the owned name. Both sides read the same
    /// host environment, so this holds on every machine; it pins the name, not
    /// the value.
    #[test]
    fn the_test_helper_agrees_with_a_direct_read_of_the_owned_name() {
        let direct = postgres_url_or_default(std::env::var(DATABASE_URL_ENV));
        assert_eq!(postgres_url_from_env(), direct);
    }
}
