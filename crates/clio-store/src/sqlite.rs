// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for opening and bootstrapping the SQLite +
//! sqlite-vec backend: extension registration, width-parameterized vector DDL,
//! the recorded active embedding space, and the space switch.
//!
//! ## Owns
//! - `SqliteStore::open` / `open_in_memory` / `from_connection` bootstrap order
//! - The recorded-space read/write and the confirmed space switch
//! - Active-width validation for stored and queried vectors
//! - The cross-process shared hold each file-backed open takes for the store's
//!   lifetime, released after the connection closes
//!
//! ## Does not own
//! - Backend CRUD SQL (the `sqlite_*` modules)
//! - Space identity or width policy (`embedding_space`)
//! - Vector DDL text (`vector_ddl`)
//!
//! ## Boundary
//!
//! Changes to SQLite bootstrap, extension wiring, or the space switch should
//! normally require changes here. Changes to width policy or DDL text should not.
//!
//! Keep this module focused on SQLite store startup and space reconciliation.

use crate::dek::DekProvider;
use crate::embedding_space::{
    EmbeddingSpace, SETTING_EMBEDDING_DIM, SETTING_EMBEDDING_MODEL, SpaceSwitchReport, log_switch,
    refused_switch,
};
use crate::migrate::{IntoAmError, SQL_COMPLIANCE_SLOTS, SQL_CORE, SQL_VECTORS_SQLITE, now_iso};
use crate::sqlite_path::ensure_parent;
use crate::vec_register::ensure_sqlite_vec_registered;
use crate::vector_ddl::sqlite_vector_ddl;
use clio_types::AmError;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// SQLite-backed store. Requires a loaded sqlite-vec extension.
pub struct SqliteStore {
    pub(crate) conn: Mutex<Connection>,
    pub(crate) kms: Arc<dyn DekProvider>,
    space: EmbeddingSpace,
    /// True when this open recorded the default space because none was stored
    /// (a genuinely fresh database).
    space_was_absent: bool,
    /// The process-wide registration of this file-backed handle, so a restore
    /// can prove every handle is closed before it touches a sidecar. `None`
    /// for an in-memory store, which has no file to replace. Held for its
    /// `Drop`; nothing reads it.
    #[allow(dead_code)]
    handle: Option<crate::open_handles::OpenHandle>,
    /// The cross-process shared hold on the store's lock file, taken at open
    /// and held for the store's lifetime. Declared after `conn` so it is
    /// released after the connection closes: a concurrent restore must never
    /// see the hold gone while a connection is still open, or it could swap
    /// the file under that connection. Held for its `Drop`; nothing reads it.
    #[allow(dead_code)]
    lock: Option<crate::store_lock::SharedHold>,
}

impl SqliteStore {
    /// Open (or create) a SQLite database, migrate, and verify vec0 works.
    ///
    /// # Errors
    /// Migration or extension failures.
    pub fn open(path: &Path, kms: Arc<dyn DekProvider>) -> Result<Self, AmError> {
        ensure_sqlite_vec_registered()?;
        ensure_parent(path)?;
        // `:memory:` is a valid SQLite path that gives an in-memory database:
        // it has no file a restore could replace, so it takes no hold and
        // leaves no lock file in the working directory.
        let lock = if path.file_name() == Some(std::ffi::OsStr::new(":memory:")) {
            None
        } else {
            // The shared hold is taken before the file opens, so a concurrent
            // restore cannot swap the file under the open; a failure later in
            // this function releases the hold through the local's drop.
            Some(crate::store_lock::SharedHold::acquire(path)?)
        };
        let conn = Connection::open(path).am("sqlite open")?;
        // The handle is registered before bootstrap so a failure inside it
        // releases the count like any other path out of this function.
        let handle = crate::open_handles::register(path);
        let mut store = Self::from_connection(conn, kms)?;
        store.handle = Some(handle);
        store.lock = lock;
        Ok(store)
    }

    /// In-memory store for tests and cross-crate suites.
    ///
    /// # Errors
    /// Migration or extension failures.
    pub fn open_in_memory(kms: Arc<dyn DekProvider>) -> Result<Self, AmError> {
        // Exempt from the shared hold on purpose: an in-memory store has no
        // file to lock, so no restore can ever replace it under this
        // connection.
        ensure_sqlite_vec_registered()?;
        let conn = Connection::open_in_memory().expect("in-memory sqlite always opens");
        Self::from_connection(conn, kms)
    }

    fn from_connection(conn: Connection, kms: Arc<dyn DekProvider>) -> Result<Self, AmError> {
        // Validate the MemTree table shape BEFORE core DDL stamps schema_version:
        // a refused reshape must never leave version metadata ahead of the real schema.
        crate::memtree_schema::ensure_memtree_shape_sqlite(&conn)?;
        // The legacy plaintext-shape guard must run BEFORE the convergence
        // passes: those write columns on an existing database, and a refused
        // open must leave the file unchanged.
        crate::triple_reshape::refuse_legacy_triples_sqlite(&conn)?;
        // Additive lineage convergence must run BEFORE portable DDL: an existing
        // database lacks the columns that 001/002 declare and index.
        crate::schema_converge::converge_lineage_columns_sqlite(&conn)?;
        crate::schema_converge::converge_context_columns_sqlite(&conn)?;
        crate::schema_converge::converge_distilled_state_sqlite(&conn)?;
        // Duplicate-open-triple guard must run BEFORE the unique index build:
        // a legacy duplicate fails CREATE UNIQUE INDEX with a raw driver error.
        crate::triple_invariant::ensure_no_duplicate_open_triples_sqlite(&conn)?;
        let bootstrap = format!("PRAGMA foreign_keys = ON;\n{SQL_CORE}\n{SQL_COMPLIANCE_SLOTS}");
        conn.execute_batch(&bootstrap).am("migrate core")?;
        crate::schema_reshape::bump_schema_version_sqlite(&conn)?;
        conn.execute_batch(SQL_VECTORS_SQLITE)
            .map_err(crate::migrate::sqlite_vec_missing)?;
        crate::bank_registration::install_sqlite_triggers(&conn)?;
        let _version: String = conn
            .query_row("SELECT vec_version()", [], |r| r.get(0))
            .map_err(crate::migrate::sqlite_vec_missing)?;
        // The vector table is created for the recorded space, so an existing
        // database keeps its width and a fresh one gets the documented default
        // recorded (and can then adopt the configured space with nothing to purge).
        let (space, space_was_absent) = if let Some(space) = read_space(&conn)? {
            (space, false)
        } else {
            let space = EmbeddingSpace::default_space();
            write_space(&conn, &space)?;
            (space, true)
        };
        conn.execute_batch(&sqlite_vector_ddl(&space)?)
            .map_err(crate::migrate::sqlite_vec_missing)?;
        Ok(Self {
            conn: Mutex::new(conn),
            kms,
            space,
            space_was_absent,
            handle: None,
            lock: None,
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
    /// Refused confirmation, or storage/DDL failures.
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
        let empty = self.with_conn(vector_row_count)?;
        let fresh = self.space_was_absent && empty == 0;
        if !fresh && !confirm {
            return Err(refused_switch(&self.space, space));
        }
        let from = self.space.clone();
        let purged = self.with_conn(|conn| switch_sqlite(conn, space))?;
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
        self.with_conn(|conn| conn.execute_batch(sql).am("test_execute"))
    }

    pub(crate) fn kms(&self) -> &Arc<dyn DekProvider> {
        &self.kms
    }

    pub(crate) fn with_conn<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, AmError>,
    ) -> Result<T, AmError> {
        let mut guard = self.conn.lock().expect("sqlite store mutex poisoned");
        f(&mut guard)
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

/// Read the recorded active space (`None` when neither settings row exists).
pub(crate) fn read_space(conn: &Connection) -> Result<Option<EmbeddingSpace>, AmError> {
    let read = |key: &str| -> Result<Option<String>, AmError> {
        conn.query_row(
            "SELECT value FROM schema_settings WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .am("space setting read")
    };
    let model = read(SETTING_EMBEDDING_MODEL)?;
    let dims = read(SETTING_EMBEDDING_DIM)?;
    EmbeddingSpace::from_configured(model.as_deref(), dims.as_deref())
}

/// Number of derived dense rows (used to detect a genuinely fresh database).
fn vector_row_count(conn: &mut Connection) -> Result<u64, AmError> {
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM item_embeddings", [], |r| r.get(0))
        .am("space purge count")?;
    Ok(u64::try_from(n.max(0)).unwrap_or(0))
}

/// Persist the recorded active space rows.
pub(crate) fn write_space(conn: &Connection, space: &EmbeddingSpace) -> Result<(), AmError> {
    let ts = now_iso();
    let dims = space.dims.to_string();
    for (key, value) in [
        (SETTING_EMBEDDING_MODEL, space.model_id.as_str()),
        (SETTING_EMBEDDING_DIM, dims.as_str()),
    ] {
        conn.execute(
            "INSERT INTO schema_settings (key, value, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, ts],
        )
        .am("space setting write")?;
    }
    Ok(())
}

/// Drop and recreate the vector table at `space`'s width; returns purged row count.
fn switch_sqlite(conn: &mut Connection, space: &EmbeddingSpace) -> Result<u64, AmError> {
    let purged: i64 = conn
        .query_row("SELECT COUNT(*) FROM item_embeddings", [], |r| r.get(0))
        .am("space purge count")?;
    conn.execute_batch("DROP TABLE IF EXISTS item_embeddings;")
        .am("space drop")?;
    conn.execute_batch(&sqlite_vector_ddl(space)?)
        .map_err(crate::migrate::sqlite_vec_missing)?;
    write_space(conn, space)?;
    Ok(u64::try_from(purged.max(0)).unwrap_or(0))
}

#[cfg(test)]
#[path = "sqlite_space_tests.rs"]
mod sqlite_space_tests;

#[cfg(test)]
#[path = "sqlite_open_lock_tests.rs"]
mod sqlite_open_lock_tests;

#[cfg(test)]
#[path = "sqlite_history_tests.rs"]
pub(crate) mod sqlite_history_tests;

#[cfg(test)]
#[path = "sqlite_history_context_tests.rs"]
mod sqlite_history_context_tests;
