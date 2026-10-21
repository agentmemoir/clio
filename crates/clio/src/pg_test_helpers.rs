// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared Postgres test-database address:
//! the gate's `CLIO_DATABASE_URL`, with the Compose stack's address as the
//! fallback.
//!
//! ## Owns
//! - The single `pg_base_url` helper the Postgres test modules share
//!
//! ## Does not own
//! - The shared Postgres test lock (`lock_pg_test`)
//! - Schema isolation for parallel tests (`ScopedSchema`)
//!
//! ## Boundary
//!
//! Changes to the shared test-database address should normally require changes
//! here. Changes to test locking or schema isolation should not require
//! changes here.
//!
//! Keep this module focused on the shared Postgres test address.

/// The shared test database's URL: the gate runs every test with
/// `CLIO_DATABASE_URL`, and the fallback is the Compose stack's address.
pub(crate) fn pg_base_url() -> String {
    resolve_pg_base_url(std::env::var("CLIO_DATABASE_URL"))
}

fn resolve_pg_base_url(database_url: Result<String, std::env::VarError>) -> String {
    database_url.unwrap_or_else(|_| "postgres://clio:clio@127.0.0.1:34310/clio".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{pg_base_url, resolve_pg_base_url};

    #[test]
    fn issue_258_shared_database_url_uses_postgres_scheme() {
        let url = pg_base_url();

        assert!(
            url.starts_with("postgres://") || url.starts_with("postgresql://"),
            "the shared database URL must use a PostgreSQL scheme"
        );
    }

    #[test]
    fn issue_258_uses_configured_database_url() {
        assert_eq!(
            resolve_pg_base_url(Ok("postgres://configured".to_owned())),
            "postgres://configured"
        );
    }

    #[test]
    fn issue_258_uses_compose_url_when_database_url_is_missing() {
        assert_eq!(
            resolve_pg_base_url(Err(std::env::VarError::NotPresent)),
            "postgres://clio:clio@127.0.0.1:34310/clio"
        );
    }
}
