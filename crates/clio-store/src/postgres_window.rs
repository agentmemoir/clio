// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres execution of the bi-temporal
//! item window query declared on [`crate::Store::item_ids_in_window`].
//!
//! ## Owns
//! - `item_ids_in_window` Postgres implementation (candidate ids only)
//!
//! ## Does not own
//! - Window predicate compilation (see `window_query`) or the SQLite
//!   implementation (see `sqlite_window`)
//! - Caller-side domain, archive, and `as_of` filtering of the returned ids
//!
//! ## Boundary
//!
//! Changes to Postgres window execution should normally require changes here.
//! Changes to the predicate text should not require changes here.
//!
//! Keep this module focused on Postgres item-window reads.

use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::window_query::compile_window;
use clio_types::{AmError, TimeAxis};
use postgres::types::ToSql;

impl PostgresStore {
    pub(crate) fn item_ids_in_window_impl(
        &self,
        bank_id: &str,
        axis: TimeAxis,
        start: &str,
        end: &str,
        limit: usize,
    ) -> Result<Vec<String>, AmError> {
        let compiled = compile_window(bank_id, axis, start, end, limit, '$')?;
        // The compiled params are bank, end, and start, bound as text. The
        // limit is bound by the backend as `bigint`, the shape `LIMIT`
        // expects on Postgres.
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.with_client(|client| {
            let mut params: Vec<&(dyn ToSql + Sync)> = compiled
                .params
                .iter()
                .map(|p| p as &(dyn ToSql + Sync))
                .collect();
            params.push(&limit);
            let rows = client.query(&compiled.sql, &params).am("window query")?;
            Ok(rows.iter().map(|r| r.get::<_, String>(0)).collect())
        })
    }
}

#[cfg(test)]
#[path = "postgres_window_tests.rs"]
mod postgres_window_tests;
