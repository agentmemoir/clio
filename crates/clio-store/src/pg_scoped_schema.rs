// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the private-schema Postgres test harness: a
//! per-test schema whose scoped URL lands the store's tables in that schema
//! instead of `public`.
//!
//! ## Owns
//! - `ScopedSchema`: schema creation, the search_path-scoped store URL,
//!   drop-on-exit
//!
//! ## Does not own
//! - Serializing shared-database tests inside one process (`pg_lock`)
//! - Store bootstrap, migration, or embedding-space reconciliation (`postgres`)
//!
//! ## Boundary
//!
//! Changes to how a test isolates its Postgres rows from other tests and from
//! earlier runs should normally require changes here. Changes to the behavior a
//! test asserts should not require changes here.
//!
//! Keep this module focused on the scoped-schema harness.
//!
//! # Why tests need this
//!
//! One Compose Postgres serves the whole suite, and `public` keeps rows that no
//! test owns: a suite that never drains its outbox, a suite that deletes items
//! with raw SQL instead of `delete_item`, or a run killed mid-test. A test that
//! asserts a store-global view —
//! coverage counts, a drain report, a whole-store sweep — runs inside a private
//! schema instead, so the view holds only the rows that test created.
//!
//! The schema scopes the tables this store creates. Bootstrap guards that name
//! `public.*` explicitly (`memtree_schema`)
//! still inspect `public` on every open, so this isolates assertions rather than
//! sandboxing the whole store. `Drop` removes the schema; a process killed before
//! it runs leaves an `am_scoped_` schema behind.

use crate::dek::{DekProvider, LocalDevKms};
use crate::postgres::PostgresStore;
use clio_types::AmError;
use postgres::Client;
use std::sync::Arc;

/// A private schema with a search_path-scoped store URL, dropped on exit.
pub struct ScopedSchema {
    base_url: String,
    name: String,
    url: String,
}

impl ScopedSchema {
    /// Create a private schema on `base_url` (the shared test database).
    ///
    /// The `vector` extension stays in the shared install location (`public`) and
    /// is put on the scoped search_path, so `vector`/`halfvec` operator classes
    /// resolve while every table this store creates lands in the private schema.
    ///
    /// # Panics
    /// The database is unreachable or the schema cannot be created.
    #[must_use]
    pub fn new(base_url: &str) -> Self {
        let name = format!(
            "am_scoped_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let mut client = crate::pg_tls::connect(base_url).expect("scoped schema connect");
        client
            .batch_execute(&format!(
                "CREATE SCHEMA {name}; CREATE EXTENSION IF NOT EXISTS vector;"
            ))
            .expect("create scoped schema");
        let separator = if base_url.contains('?') { '&' } else { '?' };
        Self {
            url: format!("{base_url}{separator}options=-csearch_path%3D{name}%2Cpublic"),
            base_url: base_url.to_owned(),
            name,
        }
    }

    /// The `search_path`-scoped URL to open a store or runtime state with.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Run one batch of SQL against the scoped schema's database.
    ///
    /// A test-support call: a test in another crate that needs raw SQL (a
    /// trigger, a role, an orphan insert) goes through this one connection
    /// path instead of importing a driver of its own.
    ///
    /// # Errors
    /// The database is unreachable or the SQL fails.
    pub fn execute(&self, sql: &str) -> Result<(), AmError> {
        let mut client = self.connect();
        client
            .batch_execute(sql)
            .map_err(|error| crate::migrate::map_db_err("scoped schema execute", error))
    }

    /// Read one value (the first column of the first row) as text.
    ///
    /// A test-support call, paired with [`Self::execute`]: a test that reads
    /// one catalog fact first casts it to text in the SQL and parses here.
    ///
    /// # Errors
    /// The database is unreachable or the query fails.
    pub fn query_value(&self, sql: &str) -> Result<Option<String>, AmError> {
        let mut client = self.connect();
        let row = client
            .query_opt(sql, &[])
            .map_err(|error| crate::migrate::map_db_err("scoped schema query", error))?;
        Ok(row.map(|row| row.get::<_, String>(0)))
    }

    /// One connection to the scoped URL.
    fn connect(&self) -> Client {
        crate::pg_tls::connect(&self.url).expect("scoped schema connect")
    }

    /// Open a store over the private schema with a process-local KMS.
    ///
    /// # Panics
    /// Store bootstrap or migration fails.
    pub fn open(&self) -> PostgresStore {
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        PostgresStore::open(&self.url, kms).expect("open scoped store")
    }
}

impl Drop for ScopedSchema {
    fn drop(&mut self) {
        if let Ok(mut client) = crate::pg_tls::connect(&self.base_url) {
            let _ = client.batch_execute(&format!("DROP SCHEMA IF EXISTS {} CASCADE", self.name));
        }
    }
}

#[cfg(test)]
#[path = "pg_scoped_schema_tests.rs"]
mod pg_scoped_schema_tests;
