// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres metadata-only item listing used
//! by the memory inspector (`inspect`).
//!
//! ## Owns
//! - `inspect_items` Postgres implementation (filtered, paginated, content-free)
//!
//! ## Does not own
//! - Filter compilation (see `inspect_query`) or the SQLite implementation
//! - Tool-level argument parsing or secret masking (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to Postgres inspector SQL should normally require changes here.
//! Changes to SQLite inspector SQL should not require changes here.
//!
//! Keep this module focused on Postgres item inspection.

use crate::inspect_query::compile_inspect;
use crate::migrate::IntoAmError;
use crate::model::{ItemInspectFilter, ItemInspectRow};
use crate::postgres::PostgresStore;
use clio_types::AmError;
use postgres::types::ToSql;

impl PostgresStore {
    pub(crate) fn inspect_items_impl(
        &self,
        filter: &ItemInspectFilter,
    ) -> Result<Vec<ItemInspectRow>, AmError> {
        let compiled = compile_inspect(filter, '$')?;
        let limit_slot = compiled.params.len() + 1;
        let offset_slot = compiled.params.len() + 2;
        let sql = format!(
            "SELECT id, kind, category, episodic_type, epistemic_kind, source_type,
                    confidence::float8, admission_score::float8, source_ref, valid_from,
                    valid_until, tx_from, tx_until, discarded_at, archived_at,
                    created_at, updated_at
             FROM items WHERE {} ORDER BY created_at, id
             LIMIT ${limit_slot} OFFSET ${offset_slot}",
            compiled.where_sql
        );
        let limit = i64::try_from(filter.limit).unwrap_or(i64::MAX);
        let offset = i64::try_from(filter.offset).unwrap_or(i64::MAX);
        self.with_client(|client| {
            let mut params: Vec<&(dyn ToSql + Sync)> = compiled
                .params
                .iter()
                .map(|p| p as &(dyn ToSql + Sync))
                .collect();
            params.push(&limit);
            params.push(&offset);
            let rows = client.query(&sql, &params).am("inspect query")?;
            Ok(rows
                .iter()
                .map(|r| ItemInspectRow {
                    item_id: r.get(0),
                    kind: r.get(1),
                    category: r.get(2),
                    episodic_type: r.get(3),
                    epistemic_kind: r.get(4),
                    source_type: r.get(5),
                    confidence: r.get(6),
                    admission_score: r.get(7),
                    source_ref: r.get(8),
                    valid_from: r.get(9),
                    valid_until: r.get(10),
                    tx_from: r.get(11),
                    tx_until: r.get(12),
                    discarded: r.get::<_, Option<String>>(13).is_some(),
                    archived: r.get::<_, Option<String>>(14).is_some(),
                    created_at: r.get(15),
                    updated_at: r.get(16),
                })
                .collect())
        })
    }

    /// Total rows matching the filter predicates (ignores `limit`/`offset`).
    pub(crate) fn count_items_impl(&self, filter: &ItemInspectFilter) -> Result<usize, AmError> {
        let compiled = compile_inspect(filter, '$')?;
        let sql = format!(
            "SELECT COUNT(*)::bigint FROM items WHERE {}",
            compiled.where_sql
        );
        self.with_client(|client| {
            let params: Vec<&(dyn ToSql + Sync)> = compiled
                .params
                .iter()
                .map(|p| p as &(dyn ToSql + Sync))
                .collect();
            let count: i64 = client
                .query_one(&sql, &params)
                .am("inspect count query")?
                .get(0);
            Ok(usize::try_from(count).unwrap_or(0))
        })
    }
}

#[cfg(test)]
#[path = "postgres_inspect_tests.rs"]
mod postgres_inspect_tests;
