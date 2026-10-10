// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres item-row delete body
//! (`Store::delete_item`), kept out of the `Store` wiring surface.
//!
//! ## Owns
//! - `delete_item` Postgres SQL: the item row plus every derived row Postgres
//!   does not cascade, in one transaction
//!
//! ## Does not own
//! - Which `Store` methods Postgres exposes (see `postgres_store`)
//! - Typed memory-item CRUD (`postgres_memory`)
//!
//! ## Boundary
//!
//! Changes to the Postgres item-row delete SQL should normally require changes
//! here. Changes to the `Store` wiring should not require changes here.
//!
//! Keep this module focused on the Postgres item-row delete.

use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use clio_types::AmError;

impl PostgresStore {
    /// Delete one item row plus the queue rows Postgres does not cascade.
    ///
    /// The whole sequence is one transaction, so the item row and every derived
    /// row (including the distilled-unit record) commit or roll back together.
    pub(crate) fn delete_item_impl(&self, bank_id: &str, item_id: &str) -> Result<bool, AmError> {
        self.with_client(|client| {
            let mut tx = client.transaction().am("items delete begin")?;
            let n = tx
                .execute(
                    "DELETE FROM items WHERE id = $1 AND bank_id = $2",
                    &[&item_id, &bank_id],
                )
                .am("items delete")?;
            if n > 0 {
                // Vector rows cascade; lexical lives on the deleted row; queue
                // rows for a vanished item can never be applied.
                tx.execute("DELETE FROM index_pending WHERE item_id = $1", &[&item_id])
                    .am("index queue delete")?;
                tx.execute(
                    "DELETE FROM distilled_units WHERE bank_id = $1 AND item_id = $2",
                    &[&bank_id, &item_id],
                )
                .am("distilled unit delete")?;
                // A claim names two handles on the erased item: the hub it
                // distils and the unit it writes. Either one going leaves the
                // row meaningless, so one statement takes both.
                tx.execute(
                    "DELETE FROM distill_intents WHERE bank_id = $1
                     AND (hub_id = $2 OR unit_item_id = $2)",
                    &[&bank_id, &item_id],
                )
                .am("distill intent delete")?;
            }
            tx.commit().am("items delete commit")?;
            Ok(n > 0)
        })
    }
}
