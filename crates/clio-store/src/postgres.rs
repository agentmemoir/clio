// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for opening and bootstrapping the PostgreSQL +
//! pgvector backend: extension checks, width-parameterized vector DDL, the
//! recorded active embedding space, and the space switch.
//!
//! ## Owns
//! - `PostgresStore::open` bootstrap order and the pgvector version gate
//! - The recorded-space read/write and the confirmed space switch
//! - Active-width validation for stored and queried vectors
//!
//! ## Does not own
//! - Backend CRUD SQL (the `postgres_*` modules)
//! - Space identity or width policy (`embedding_space`)
//! - Vector DDL text (`vector_ddl`)
//!
//! ## Boundary
//!
//! Changes to Postgres bootstrap, extension checks, or the space switch should
//! normally require changes here. Changes to width policy or DDL text should not.
//!
//! Keep this module focused on Postgres store startup and space reconciliation.

use crate::dek::DekProvider;
use crate::embedding_space::{
    EmbeddingSpace, IndexMode, SETTING_EMBEDDING_DIM, SETTING_EMBEDDING_MODEL, SpaceSwitchReport,
    log_switch, refused_switch,
};
use crate::migrate::{IntoAmError, SQL_COMPLIANCE_SLOTS, SQL_CORE, SQL_VECTORS_POSTGRES, now_iso};
use crate::vector_ddl::{HALFVEC_MIN_VERSION, pgvector_supports_halfvec, postgres_vector_ddl};
use clio_types::{AmError, ErrorCode};
use postgres::Client;
use std::sync::{Arc, Mutex};

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
        // Every open runs schema and trigger DDL. Two opens that overlap take
        // the same table locks in different orders and write the same catalog
        // rows, so the server aborts one of them with `deadlock detected`,
        // `tuple concurrently updated`, or a `pg_type` unique-constraint
        // violation. The whole bootstrap runs under one cross-session
        // advisory lock.
        let (space, space_was_absent) = with_bootstrap_lock(&mut client, bootstrap_schema)?;
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

/// Run the open-time schema, trigger, and embedding-space work on one connection.
///
/// Every caller reaches this through `PostgresStore::open`, which holds
/// [`PG_BOOTSTRAP_ADVISORY_KEY`] for the whole call so two opens cannot run this
/// DDL at once.
fn bootstrap_schema(client: &mut Client) -> Result<(EmbeddingSpace, bool), AmError> {
    // Validate the MemTree table shape BEFORE core DDL stamps schema_version:
    // a refused reshape must never leave version metadata ahead of the real schema.
    crate::memtree_schema::ensure_memtree_shape_pg(client)?;
    // The legacy plaintext-shape guard must run BEFORE the migrate batch:
    // that batch writes columns on an existing database, and a refused
    // open must leave the database unchanged.
    crate::triple_reshape::refuse_legacy_triples_pg(client)?;
    // Duplicate-open-triple guard must run BEFORE the unique index build:
    // a legacy duplicate fails CREATE UNIQUE INDEX with a raw driver error.
    crate::triple_invariant::ensure_no_duplicate_open_triples_pg(client)?;
    let migrate = format!("{SQL_CORE}\n{SQL_VECTORS_POSTGRES}\n{SQL_COMPLIANCE_SLOTS}");
    client.batch_execute(&migrate).am("migrate postgres")?;
    crate::bank_registration::install_postgres_triggers(client)?;
    crate::schema_reshape::bump_schema_version_pg(client)?;
    // The vector table is created for the recorded space, so an existing
    // database keeps its width and a fresh one gets the documented default
    // recorded (and can then adopt the configured space with nothing to purge).
    let (space, space_was_absent) = if let Some(space) = read_space(client)? {
        (space, false)
    } else {
        let space = EmbeddingSpace::default_space();
        write_space(client, &space)?;
        (space, true)
    };
    apply_vector_ddl(client, &space)?;
    Ok((space, space_was_absent))
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

/// Distance expression for the active width, honouring the `halfvec` cast.
///
/// Above 2000 dims the ANN index is an expression index on
/// `embedding::halfvec(N)`, so the identical cast MUST be repeated in the
/// `ORDER BY` (and in every query) or the planner silently falls back to a
/// sequential scan. `alias` is the table alias (empty for an unaliased column).
#[must_use]
pub(crate) fn distance_expr(space: &EmbeddingSpace, alias: &str) -> String {
    let prefix = if alias.is_empty() {
        String::new()
    } else {
        format!("{alias}.")
    };
    match space.index_mode() {
        IndexMode::AnnHalfvec => format!(
            "{prefix}embedding::halfvec({dims}) <=> $1::vector::halfvec({dims})",
            dims = space.dims
        ),
        _ => format!("{prefix}embedding <=> $1"),
    }
}

/// Apply the width-appropriate vector DDL, asserting the engine supports it.
fn apply_vector_ddl(client: &mut Client, space: &EmbeddingSpace) -> Result<(), AmError> {
    apply_vector_ddl_with(client, space, pgvector_version)
}

/// Apply vector DDL using an injected pgvector-version reader.
///
/// The `halfvec` path requires pgvector >= 0.7.0; the version source is a seam so
/// the refusal path can be exercised without an old extension installed.
pub(crate) fn apply_vector_ddl_with(
    client: &mut Client,
    space: &EmbeddingSpace,
    version: impl FnOnce(&mut Client) -> Result<String, AmError>,
) -> Result<(), AmError> {
    if space.index_mode() == IndexMode::AnnHalfvec {
        let version = version(client)?;
        if !pgvector_supports_halfvec(&version) {
            return Err(halfvec_unsupported(&version));
        }
    }
    client
        .batch_execute(&postgres_vector_ddl(space)?)
        .am("vector ddl")
}

/// Guidance error for a pgvector too old to serve the `halfvec` index.
#[must_use]
pub(crate) fn halfvec_unsupported(version: &str) -> AmError {
    AmError::new(
        ErrorCode::ConfigCorrupt,
        format!(
            "pgvector {version} does not support halfvec (needs >= {HALFVEC_MIN_VERSION}); \
             upgrade the extension or configure embed.dims <= 2000 to use a plain vector index"
        ),
    )
}

/// Installed pgvector `extversion`, or an empty string when unreadable.
fn pgvector_version(client: &mut Client) -> Result<String, AmError> {
    let row = client
        .query_opt(
            "SELECT extversion FROM pg_extension WHERE extname = 'vector'",
            &[],
        )
        .am("pgvector version")?;
    Ok(row.map_or_else(String::new, |r| r.get::<_, String>(0)))
}

/// Read the recorded active space (`None` when neither settings row exists).
pub(crate) fn read_space(client: &mut Client) -> Result<Option<EmbeddingSpace>, AmError> {
    fn read_one(client: &mut Client, key: &str) -> Result<Option<String>, AmError> {
        let row = client
            .query_opt("SELECT value FROM schema_settings WHERE key = $1", &[&key])
            .am("space setting read")?;
        Ok(row.map(|r| r.get::<_, String>(0)))
    }
    let model = read_one(client, SETTING_EMBEDDING_MODEL)?;
    let dims = read_one(client, SETTING_EMBEDDING_DIM)?;
    EmbeddingSpace::from_configured(model.as_deref(), dims.as_deref())
}

/// Number of derived dense rows (used to detect a genuinely fresh database).
fn vector_row_count(client: &mut Client) -> Result<u64, AmError> {
    let n: i64 = client
        .query_one("SELECT COUNT(*) FROM item_embeddings", &[])
        .am("space purge count")?
        .get(0);
    Ok(u64::try_from(n.max(0)).unwrap_or(0))
}

/// Persist the recorded active space rows.
pub(crate) fn write_space(client: &mut Client, space: &EmbeddingSpace) -> Result<(), AmError> {
    let ts = now_iso();
    let dims = space.dims.to_string();
    for (key, value) in [
        (SETTING_EMBEDDING_MODEL, space.model_id.as_str()),
        (SETTING_EMBEDDING_DIM, dims.as_str()),
    ] {
        client
            .execute(
                "INSERT INTO schema_settings (key, value, created_at, updated_at)
                 VALUES ($1, $2, $3, $3)
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = EXCLUDED.updated_at",
                &[&key, &value, &ts],
            )
            .am("space setting write")?;
    }
    Ok(())
}

/// Drop and recreate the vector table at `space`'s width; returns purged row count.
///
/// Runs under [`PG_BOOTSTRAP_ADVISORY_KEY`] like the bootstrap does: the drop
/// and the vector DDL take the same table locks an open takes, so an unguarded
/// switch could deadlock against a concurrent open.
fn switch_pg(client: &mut Client, space: &EmbeddingSpace) -> Result<u64, AmError> {
    with_bootstrap_lock(client, |client| {
        let purged: i64 = client
            .query_one("SELECT COUNT(*) FROM item_embeddings", &[])
            .am("space purge count")?
            .get(0);
        client
            .batch_execute("DROP TABLE IF EXISTS item_embeddings;")
            .am("space drop")?;
        apply_vector_ddl(client, space)?;
        write_space(client, space)?;
        Ok(u64::try_from(purged.max(0)).unwrap_or(0))
    })
}

#[cfg(test)]
#[path = "postgres_distilled_evidence_tests.rs"]
mod postgres_distilled_evidence_tests;

#[cfg(test)]
#[path = "postgres_bootstrap_lock_tests.rs"]
mod postgres_bootstrap_lock_tests;
