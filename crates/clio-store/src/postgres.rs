// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres store handle: opening it behind
//! the bootstrap advisory lock, reconciling the configured embedding space with
//! the recorded one, and validating the width and model of a vector.
//!
//! ## Owns
//! - `PostgresStore::open` order: extension check, bootstrap, timestamp rewrite
//! - The bootstrap advisory lock held by an open and by the confirmed switch
//! - The confirmed space switch and the derived-row purge count it reports
//! - Active-width and active-model validation for stored and queried vectors
//!
//! ## Does not own
//! - The bootstrap DDL pass, the recorded space rows, and the vector DDL
//!   (`postgres_bootstrap`)
//! - Backend CRUD SQL (the `postgres_*` modules)
//! - Space identity or width policy (`embedding_space`)
//!
//! ## Boundary
//!
//! Changes to when a bootstrap or a switch runs, or to what an open does around
//! them, should normally require changes here. Changes to the DDL a bootstrap
//! pass runs, or to width policy, should not.
//!
//! Keep this module focused on the store handle and the lock it holds.

use crate::dek::DekProvider;
use crate::embedding_space::{EmbeddingSpace, SpaceSwitchReport, log_switch, refused_switch};
use crate::migrate::IntoAmError;
use crate::postgres_bootstrap::{
    apply_vector_ddl, bootstrap_schema, vector_row_count, write_space,
};
use clio_types::AmError;
use postgres::Client;
use std::sync::{Arc, Mutex};

// Width-appropriate distance SQL; re-exported so its `postgres` path still resolves.
pub(crate) use crate::postgres_bootstrap::distance_expr;

/// Postgres-backed store using pgvector for dense slots.
pub struct PostgresStore {
    pub(crate) client: Mutex<Client>,
    pub(crate) kms: Arc<dyn DekProvider>,
    space: EmbeddingSpace,
    /// True when this open recorded the default space because none was stored
    /// (a genuinely fresh database).
    space_was_absent: bool,
}

/// Advisory-lock key that serializes the Postgres store bootstrap (`"clio_bst"`).
///
/// One key serves every open, not one key per schema. The bootstrap touches
/// `public` whatever the connection's `search_path` is, because guards such as
/// `memtree_schema` name `public.*` explicitly, so a schema-derived key would
/// let two opens into the same tables at once.
const PG_BOOTSTRAP_ADVISORY_KEY: i64 = crate::pg_advisory_keys::STORE_BOOTSTRAP;

impl PostgresStore {
    /// Connect, migrate, and verify the `vector` extension.
    ///
    /// # Errors
    /// Connection or migration failures.
    pub fn open(database_url: &str, kms: Arc<dyn DekProvider>) -> Result<Self, AmError> {
        let mut client = crate::pg_tls::connect(database_url).am("postgres connect")?;
        crate::postgres_ext::require_extension(&mut client, "vector")?;
        // Serialize schema checks and actual migrations. A complete current
        // schema opens without DDL that could conflict with an existing writer.
        let (space, space_was_absent) = with_bootstrap_lock(&mut client, |client| {
            let state = bootstrap_schema(client)?;
            // Timestamps a previous build stored as epoch millis are rewritten into
            // the canonical form the item time columns are compared in. A database
            // with nothing to fix runs one filtered read, so the pass runs on every
            // open: a database whose bootstrap is already current skips the DDL
            // above, and is exactly one that still needs its stored stamps fixed.
            crate::migrate::ts_backfill::backfill_postgres(client)?;
            Ok(state)
        })?;
        Ok(Self {
            client: Mutex::new(client),
            kms,
            space,
            space_was_absent,
        })
    }

    /// The active embedding space this store serves.
    #[must_use]
    pub(crate) fn space(&self) -> &EmbeddingSpace {
        &self.space
    }

    /// Reconcile the configured space with the recorded one.
    ///
    /// Equal spaces are a no-op. A different space is destructive to derived
    /// data and is therefore refused unless `confirm` is set; a confirmed run
    /// drops and recreates only the vector table, records the new space, and
    /// leaves every non-vector table untouched. A genuinely fresh database
    /// (no recorded space before this open and no derived rows) records the
    /// configured space with nothing to destroy, so it needs no opt-in.
    ///
    /// # Errors
    /// Refused confirmation, an old pgvector without `halfvec`, or storage/DDL failures.
    pub fn apply_configured_space(
        &mut self,
        space: &EmbeddingSpace,
        confirm: bool,
    ) -> Result<SpaceSwitchReport, AmError> {
        if self.space == *space {
            self.space_was_absent = false;
            return Ok(SpaceSwitchReport {
                from: self.space.clone(),
                to: space.clone(),
                switched: false,
                purged_rows: 0,
            });
        }
        let empty = self.with_client(vector_row_count)?;
        let fresh = self.space_was_absent && empty == 0;
        if !fresh && !confirm {
            return Err(refused_switch(&self.space, space));
        }
        let from = self.space.clone();
        let purged = self.with_client(|client| switch_pg(client, space))?;
        self.space = space.clone();
        self.space_was_absent = false;
        log_switch(&from, space, purged);
        Ok(SpaceSwitchReport {
            from,
            to: space.clone(),
            switched: true,
            purged_rows: purged,
        })
    }

    #[cfg(test)]
    pub(crate) fn test_execute(&self, sql: &str) -> Result<(), AmError> {
        self.with_client(|client| client.batch_execute(sql).am("test_execute"))
    }

    pub(crate) fn with_client<T>(
        &self,
        f: impl FnOnce(&mut Client) -> Result<T, AmError>,
    ) -> Result<T, AmError> {
        let mut guard = self.client.lock().expect("postgres store mutex poisoned");
        f(&mut guard)
    }

    pub(crate) fn kms(&self) -> &Arc<dyn DekProvider> {
        &self.kms
    }

    /// Fail closed when a vector's width is not the active space width.
    pub(crate) fn validate_dims(&self, embedding: &[f32]) -> Result<(), AmError> {
        self.space.require_dims(embedding.len())
    }

    /// Fail closed when a vector's model id is not the active space model.
    pub(crate) fn validate_model(&self, model_id: &str) -> Result<(), AmError> {
        self.space.require_model(model_id)
    }
}

/// Take [`PG_BOOTSTRAP_ADVISORY_KEY`], run `body`, and release the key on both
/// the success and the error path.
///
/// The release is what stops a refused bootstrap from stalling the next opener:
/// a session advisory lock outlives a rolled-back transaction, so nothing else
/// frees it before the connection ends. `PostgresStore::open` runs its bootstrap
/// through this helper, and so does the confirmed space switch, whose DDL takes
/// the same table locks.
fn with_bootstrap_lock<T>(
    client: &mut Client,
    body: impl FnOnce(&mut Client) -> Result<T, AmError>,
) -> Result<T, AmError> {
    lock_bootstrap(client)?;
    let outcome = body(client);
    // Released on both paths; a release failure is only worth reporting when the
    // body succeeded, because otherwise the client is dropped on the error path
    // and the session lock goes with it.
    let released = unlock_bootstrap(client);
    let value = outcome?;
    released?;
    Ok(value)
}

/// Take the bootstrap advisory lock, blocking while another open holds it.
fn lock_bootstrap(client: &mut Client) -> Result<(), AmError> {
    client
        .query_one("SELECT pg_advisory_lock($1)", &[&PG_BOOTSTRAP_ADVISORY_KEY])
        .am("postgres bootstrap lock")?;
    Ok(())
}

/// Release the bootstrap advisory lock taken by [`lock_bootstrap`].
fn unlock_bootstrap(client: &mut Client) -> Result<(), AmError> {
    client
        .query_one(
            "SELECT pg_advisory_unlock($1)",
            &[&PG_BOOTSTRAP_ADVISORY_KEY],
        )
        .am("postgres bootstrap unlock")?;
    Ok(())
}

/// Drop and recreate the vector table at `space`'s width; returns purged row count.
///
/// Runs under [`PG_BOOTSTRAP_ADVISORY_KEY`] like the bootstrap does: the drop
/// and the vector DDL take the same table locks an open takes, so an unguarded
/// switch could deadlock against a concurrent open.
/// The transaction keeps the table and recorded space consistent with the
/// completion stamp if any switch step fails.
fn switch_pg(client: &mut Client, space: &EmbeddingSpace) -> Result<u64, AmError> {
    with_bootstrap_lock(client, |client| {
        client.batch_execute("BEGIN").am("space switch begin")?;
        let outcome = (|| {
            let purged = vector_row_count(client)?;
            client
                .batch_execute("DROP TABLE IF EXISTS item_embeddings;")
                .am("space drop")?;
            apply_vector_ddl(client, space)?;
            write_space(client, space)?;
            Ok(purged)
        })();
        match outcome {
            Ok(purged) => {
                client.batch_execute("COMMIT").am("space switch commit")?;
                Ok(purged)
            }
            Err(error) => {
                client
                    .batch_execute("ROLLBACK")
                    .am("space switch rollback")?;
                Err(error)
            }
        }
    })
}

#[cfg(test)]
#[path = "postgres_distilled_evidence_tests.rs"]
mod postgres_distilled_evidence_tests;

#[cfg(test)]
#[path = "postgres_bootstrap_lock_tests.rs"]
mod postgres_bootstrap_lock_tests;
