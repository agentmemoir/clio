// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the declared seam for the SQLite vector backend: the one
//! place that names the extension crate, registers its entry point, generates
//! its virtual-table DDL, and runs its `MATCH` scan, so the backend stays
//! replaceable behind one API.
//!
//! ## Owns
//! - Process-wide registration of the extension entry point (fail closed)
//! - `sqlite_vector_ddl`: width-parameterized virtual-table DDL for a space
//! - The fail-closed wrappers every caller routes through: applying the static
//!   vectors SQL, applying a space's DDL, and verifying the backend answers
//! - `knn_scan`: the virtual-table `MATCH` scan shape and its query binding
//! - The virtual-table write mechanics: the no-upsert replace
//!   (`upsert_vector_row`), explicit row removal (`delete_vector_rows`), the
//!   erase-path row removal (`erase_vector_rows`), the orphan sweep
//!   (`sweep_orphan_vector_rows`), and the width-change drop
//!   (`drop_vector_table`)
//!
//! ## Does not own
//! - Postgres and pgvector DDL (`vector_ddl`)
//! - Space identity or width policy (`embedding_space`)
//! - Bootstrap order or the space switch (`sqlite`)
//! - The KNN widening policy and the visibility post-filter (`sqlite_knn`)
//! - The width-independent static SQL asset, which stays under `migrate`:
//!   the numbered vectors migration holds the historical schema, and this
//!   seam owns the vector-table DDL going forward
//!
//! ## Boundary
//!
//! Swapping the vector backend should normally require changes here and
//! nowhere else. Nothing outside this module may name the extension crate,
//! its entry point, its virtual-table kind, or its `MATCH` scan.

use crate::embedding_space::{EmbeddingSpace, MAX_VECTOR_DIMS};
use crate::migrate::{IntoAmError, SQL_VECTORS_SQLITE, sqlite_vec_missing};
use crate::model::VectorSlot;
use clio_types::{AmError, ErrorCode};
use rusqlite::{Connection, params};
use std::sync::OnceLock;
use zerocopy::IntoBytes;

static REGISTER: OnceLock<Result<(), String>> = OnceLock::new();

/// Map a registration outcome to [`AmError`] (testable without failing process init).
pub(crate) fn map_register_outcome(outcome: &Result<(), String>) -> Result<(), AmError> {
    match outcome {
        Ok(()) => Ok(()),
        Err(detail) => Err(sqlite_vec_missing(detail)),
    }
}

/// Interpret `sqlite3_auto_extension` return codes (testable both arms).
pub(crate) fn interpret_auto_extension_rc(result: std::os::raw::c_int) -> Result<(), String> {
    if result == rusqlite::ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(format!("sqlite3_auto_extension returned {result}"))
    }
}

/// Register the extension entrypoint as a SQLite auto-extension (process-wide).
///
/// # Errors
/// When registration fails — callers must fail closed (no pretend vector mode).
pub fn ensure_registered() -> Result<(), AmError> {
    let outcome = REGISTER.get_or_init(|| {
        #[allow(unsafe_code, clippy::missing_transmute_annotations)]
        // SAFETY: transmute matches the C callback signature required by SQLite;
        // `sqlite3_vec_init` is the extension entrypoint from the sqlite-vec crate.
        let result = unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )))
        };
        interpret_auto_extension_rc(result)
    });
    map_register_outcome(outcome)
}

/// Right-pad a column name so its type starts at column 96.
fn col(name: &str, rest: &str) -> String {
    if name.len() + 3 >= 93 {
        format!("  {name} {rest}")
    } else {
        format!("  {name:<92} {rest}")
    }
}

/// SQLite vector-table DDL for `space` (exact KNN regardless of width).
///
/// # Errors
/// Zero/oversized width.
pub fn sqlite_vector_ddl(space: &EmbeddingSpace) -> Result<String, AmError> {
    let dims = space.dims;
    if dims == 0 || dims > MAX_VECTOR_DIMS {
        return Err(AmError::new(
            ErrorCode::OutOfRange,
            format!("embedding width {dims} is outside 1..=16000"),
        ));
    }
    let mut ddl =
        String::from("CREATE VIRTUAL TABLE IF NOT EXISTS item_embeddings USING vec0\n(\n");
    ddl.push_str(&col("item_id", "text PRIMARY KEY"));
    ddl.push_str(",\n");
    ddl.push_str(&col(
        "embedding",
        &format!("float[{dims}] distance_metric = cosine"),
    ));
    ddl.push_str(",\n");
    ddl.push_str(&col("+bank_id", "text"));
    ddl.push_str(",\n");
    ddl.push_str(&col("+model_id", "text"));
    ddl.push_str(",\n");
    ddl.push_str(&col("+created_at", "text"));
    ddl.push_str(",\n");
    ddl.push_str(&col("+updated_at", "text"));
    ddl.push_str("\n);\n");
    Ok(ddl)
}

/// Apply the static width-independent vectors SQL, failing closed when the
/// backend cannot serve it.
///
/// # Errors
/// [`sqlite_vec_missing`] wrapping the driver error.
pub(crate) fn apply_static_vectors(conn: &Connection) -> Result<(), AmError> {
    conn.execute_batch(SQL_VECTORS_SQLITE)
        .map_err(sqlite_vec_missing)
}

/// Apply `space`'s vector-table DDL, failing closed when the backend cannot
/// serve it.
///
/// # Errors
/// Zero/oversized width, or the driver refusing the DDL.
pub(crate) fn apply_space_ddl(conn: &Connection, space: &EmbeddingSpace) -> Result<(), AmError> {
    conn.execute_batch(&sqlite_vector_ddl(space)?)
        .map_err(sqlite_vec_missing)
}

/// Verify the backend answers a `vec_version()` probe, failing closed when it
/// does not.
///
/// # Errors
/// [`sqlite_vec_missing`] wrapping the driver error.
pub(crate) fn verify_vec_version(conn: &Connection) -> Result<(), AmError> {
    conn.query_row("SELECT vec_version()", [], |row| row.get::<_, String>(0))
        .map_err(sqlite_vec_missing)
        .map(|_| ())
}

/// One exact-KNN scan over the vector table, nearest first, unfiltered.
///
/// The virtual table's `MATCH` cannot filter auxiliary columns inside the
/// scan, so rows return with their `bank_id` attached and the caller applies
/// its own bank and visibility rules, widening `fetch_k` until `k` rows
/// survive. Each row is `(item_id, distance, bank_id)`.
///
/// # Errors
/// Driver failures from the scan.
pub(crate) fn knn_scan(
    conn: &Connection,
    query: &[f32],
    fetch_k: usize,
) -> Result<Vec<(String, f64, String)>, AmError> {
    let mut stmt = conn
        .prepare(
            "SELECT item_id, distance, bank_id
             FROM item_embeddings
             WHERE embedding MATCH ?1 AND k = ?2
             ORDER BY distance",
        )
        .am("knn prepare")?;
    let rows = stmt
        .query_map(params![query.as_bytes(), fetch_k as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .am("knn query_map")?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.am("knn row")?);
    }
    Ok(out)
}

/// Replace the vector row for `slot.item_id`: delete then insert, because the
/// virtual table has no upsert. The embedding binds as the little-endian
/// float blob the backend expects; `ts` stamps both time columns.
///
/// # Errors
/// Driver failures from the delete or the insert.
pub(crate) fn upsert_vector_row(
    conn: &Connection,
    slot: &VectorSlot,
    ts: &str,
) -> Result<(), AmError> {
    conn.execute(
        "DELETE FROM item_embeddings WHERE item_id = ?1",
        params![slot.item_id],
    )
    .am("vector delete-before-upsert")?;
    conn.execute(
        "INSERT INTO item_embeddings (item_id, embedding, bank_id, model_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![
            slot.item_id,
            slot.embedding.as_bytes(),
            slot.bank_id,
            slot.model_id,
            ts,
        ],
    )
    .am("vector insert")?;
    Ok(())
}

/// Remove vector rows for `item_id`, optionally scoped to `bank_id`, and
/// return how many rows went. The virtual table does not enforce FK cascade,
/// so item deletion removes its vector row explicitly.
///
/// # Errors
/// Driver failures from the delete.
pub(crate) fn delete_vector_rows(
    conn: &Connection,
    item_id: &str,
    bank_id: Option<&str>,
) -> Result<usize, AmError> {
    match bank_id {
        Some(bank) => conn
            .execute(
                "DELETE FROM item_embeddings WHERE item_id = ?1 AND bank_id = ?2",
                params![item_id, bank],
            )
            .am("vector delete"),
        None => conn
            .execute(
                "DELETE FROM item_embeddings WHERE item_id = ?1",
                params![item_id],
            )
            .am("vector delete"),
    }
}

/// Sweep vector rows whose item row is gone; the virtual table does not
/// enforce FK cascade. `NOT IN` over an empty items set removes every vector
/// row, which is correct (no items → all vector rows are orphans). A bank
/// scope filters by the row's `bank_id` column so a scoped reindex never
/// deletes vector rows of other banks. Returns removed rows.
///
/// # Errors
/// Driver failures from the delete.
pub(crate) fn sweep_orphan_vector_rows(
    conn: &Connection,
    bank_id: Option<&str>,
) -> Result<u64, AmError> {
    let bank_filter = if bank_id.is_some() {
        " AND bank_id = ?1"
    } else {
        ""
    };
    let sql = format!(
        "DELETE FROM item_embeddings WHERE item_id NOT IN (SELECT id FROM items){bank_filter}"
    );
    let n = match bank_id {
        Some(bank) => conn.execute(&sql, params![bank]).am("orphan cleanup")?,
        None => conn.execute(&sql, []).am("orphan cleanup")?,
    };
    Ok(n as u64)
}

/// Remove one item's vector rows inside one bank during erasure; `context`
/// names the erase operation in the error. The virtual table does not
/// enforce FK cascade, so erasure removes vector rows explicitly. Returns
/// removed rows.
///
/// # Errors
/// Driver failures from the delete, wrapped in `context`.
pub(crate) fn erase_vector_rows(
    conn: &Connection,
    item_id: &str,
    bank_id: &str,
    context: &'static str,
) -> Result<usize, AmError> {
    conn.execute(
        "DELETE FROM item_embeddings WHERE item_id = ?1 AND bank_id = ?2",
        params![item_id, bank_id],
    )
    .am(context)
}

/// Drop the vector table if present, for a width change or a rebuild.
///
/// # Errors
/// Driver failures from the drop.
pub(crate) fn drop_vector_table(conn: &Connection) -> Result<(), AmError> {
    conn.execute_batch("DROP TABLE IF EXISTS item_embeddings;")
        .am("space drop")
}

#[cfg(test)]
#[path = "vector_backend_tests.rs"]
mod vector_backend_tests;
