// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite item-row delete body
//! (`Store::delete_item`), kept out of the `Store` wiring surface.
//!
//! ## Owns
//! - `delete_item` SQLite SQL: the item row plus every derived row SQLite
//!   does not cascade, in one transaction
//!
//! ## Does not own
//! - Which `Store` methods SQLite exposes (see `sqlite_store`)
//! - Typed memory-item CRUD (`sqlite_memory`)
//! - The vector-table delete SQL (`vector_backend`)
//!
//! ## Boundary
//!
//! Changes to the SQLite item-row delete SQL should normally require changes
//! here. Changes to the `Store` wiring should not require changes here.
//!
//! Keep this module focused on the SQLite item-row delete.

use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::vector_backend;
use clio_types::AmError;
use rusqlite::params;

impl SqliteStore {
    /// Delete one item row plus the derived rows SQLite does not cascade.
    ///
    /// The whole sequence is one transaction, so the item row and every derived
    /// row (including the distilled-unit record) commit or roll back together.
    pub(crate) fn delete_item_impl(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError> {
        self.with_conn(|conn| {
            let tx = conn.transaction().am("items delete begin")?;
            let n = tx
                .execute(
                    "DELETE FROM items WHERE id = ?1 AND bank_id = ?2",
                    params![item_id, bank_id],
                )
                .am("items delete")?;
            if n > 0 {
                // Derived rows are not FK-cascaded: remove the vector row
                // through the seam, the FTS5 row, and the outbox entry.
                vector_backend::delete_vector_rows(&tx, item_id, None)?;
                tx.execute(
                    "DELETE FROM index_pending WHERE item_id = ?1",
                    params![item_id],
                )
                .am("index queue delete")?;
                tx.execute("DELETE FROM items_fts WHERE item_id = ?1", params![item_id])
                    .am("items_fts delete")?;
                tx.execute(
                    "DELETE FROM distilled_units WHERE bank_id = ?1 AND item_id = ?2",
                    params![bank_id, item_id],
                )
                .am("distilled unit delete")?;
                // A claim names two handles on the erased item: the hub it
                // distils and the unit it writes. Either one going leaves the
                // row meaningless, so one statement takes both.
                tx.execute(
                    "DELETE FROM distill_intents WHERE bank_id = ?1
                     AND (hub_id = ?2 OR unit_item_id = ?2)",
                    params![bank_id, item_id],
                )
                .am("distill intent delete")?;
            }
            tx.commit().am("items delete commit")?;
            Ok(n > 0)
        })
    }
}
