// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres schema bootstrap: the one ordered
//! DDL pass an open runs on a connection, the completion stamp that marks the
//! pass done, and the vector schema of the embedding space that pass records.
//!
//! ## Owns
//! - The bootstrap guard order and the migration batch it precedes
//! - The recorded embedding-space rows and the derived-row count
//! - Width-appropriate vector DDL, the pgvector `halfvec` gate, and the distance
//!   expression that must repeat the index cast
//! - The bootstrap completion stamp that lets a later open skip this pass
//!
//! ## Does not own
//! - The bootstrap advisory lock and the confirmed space switch (`postgres`)
//! - Space identity or width policy (`embedding_space`)
//! - Vector DDL text (`vector_ddl`)
//! - Schema version bookkeeping (`schema_reshape`)
//!
//! ## Boundary
//!
//! Changes to what a bootstrap pass creates, in what order, or at which width
//! should normally require changes here. Changes to when a pass runs, or to width
//! policy, should not.
//!
//! Keep this module focused on the DDL a bootstrap pass runs.

use crate::embedding_space::{
    EmbeddingSpace, IndexMode, SETTING_EMBEDDING_DIM, SETTING_EMBEDDING_MODEL,
};
use crate::migrate::{IntoAmError, SQL_COMPLIANCE_SLOTS, SQL_CORE, SQL_VECTORS_POSTGRES, now_iso};
use crate::vector_ddl::{HALFVEC_MIN_VERSION, pgvector_supports_halfvec, postgres_vector_ddl};
use clio_types::{AmError, ErrorCode};
use postgres::Client;

// Increment when the complete bootstrap changes independently of schema_version.
const PG_BOOTSTRAP_VERSION: &str = "1";

/// Run the open-time schema, trigger, and embedding-space work on one connection.
///
/// Every caller reaches this through `PostgresStore::open`, which holds the
/// bootstrap advisory lock for the whole call so two opens cannot run this DDL
/// at once.
pub(crate) fn bootstrap_schema(client: &mut Client) -> Result<(EmbeddingSpace, bool), AmError> {
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
    // The local-event table convergence must run BEFORE the migrate batch: an
    // existing placeholder table lacks the columns the event writer names, and
    // the slot DDL only declares a fresh table.
    crate::local_events_converge::converge_pg(client)?;
    // The export-destination custody convergence must run BEFORE the migrate
    // batch: an existing export_destinations table lacks the copy-custody
    // columns the custody store reads, and the slot DDL only declares a fresh
    // table.
    crate::destination_custody_converge::converge_pg(client)?;
    if bootstrap_is_current(client)?
        && let Some(space) = read_space(client)?
    {
        return Ok((space, false));
    }
    // A partial bootstrap must never retain a previous completion stamp.
    if settings_exist(client)? {
        client
            .execute(
                "DELETE FROM schema_settings WHERE key = 'postgres_bootstrap_version'",
                &[],
            )
            .am("invalidate postgres bootstrap version")?;
    }
    let migrate = format!("{SQL_CORE}\n{SQL_VECTORS_POSTGRES}\n{SQL_COMPLIANCE_SLOTS}");
    client.batch_execute(&migrate).am("migrate postgres")?;
    crate::bank_registration::install_postgres_triggers(client)?;
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
    crate::schema_reshape::bump_schema_version_pg(client)?;
    client
        .execute(
            "INSERT INTO schema_settings (key, value, created_at, updated_at)
             VALUES ('postgres_bootstrap_version', $1, $2, $2)
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = EXCLUDED.updated_at",
            &[&PG_BOOTSTRAP_VERSION, &now_iso()],
        )
        .am("stamp postgres bootstrap version")?;
    Ok((space, space_was_absent))
}

fn settings_exist(client: &mut Client) -> Result<bool, AmError> {
    client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM information_schema.tables
             WHERE table_schema = current_schema() AND table_name = 'schema_settings')",
            &[],
        )
        .am("postgres schema settings probe")
        .map(|row| row.get(0))
}

fn bootstrap_is_current(client: &mut Client) -> Result<bool, AmError> {
    if !settings_exist(client)? {
        return Ok(false);
    }
    client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM schema_settings WHERE key = 'schema_version' AND value = $1)
             AND EXISTS (SELECT 1 FROM schema_settings WHERE key = 'postgres_bootstrap_version' AND value = $2)",
            &[&crate::schema_reshape::SCHEMA_VERSION, &PG_BOOTSTRAP_VERSION],
        )
        .am("postgres bootstrap version read")
        .map(|row| row.get(0))
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
pub(crate) fn apply_vector_ddl(client: &mut Client, space: &EmbeddingSpace) -> Result<(), AmError> {
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
pub(crate) fn vector_row_count(client: &mut Client) -> Result<u64, AmError> {
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

#[cfg(test)]
#[path = "postgres_bootstrap_tests.rs"]
mod postgres_bootstrap_tests;

#[cfg(test)]
#[path = "postgres_bootstrap_space_tests.rs"]
mod postgres_bootstrap_space_tests;
