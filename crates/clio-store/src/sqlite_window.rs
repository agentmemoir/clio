// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite execution of the bi-temporal item
//! window query declared on [`crate::Store::item_ids_in_window`].
//!
//! ## Owns
//! - `item_ids_in_window` SQLite implementation (candidate ids only)
//!
//! ## Does not own
//! - Window predicate compilation (see `window_query`) or the Postgres
//!   implementation (see `postgres_window`)
//! - Caller-side domain, archive, and `as_of` filtering of the returned ids
//!
//! ## Boundary
//!
//! Changes to SQLite window execution should normally require changes here.
//! Changes to the predicate text should not require changes here.
//!
//! Keep this module focused on SQLite item-window reads.

use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use crate::window_query::compile_window;
use clio_types::{AmError, TimeAxis};
use rusqlite::params_from_iter;
use rusqlite::types::Value;

impl SqliteStore {
    pub(crate) fn item_ids_in_window_impl(
        &self,
        bank_id: &str,
        axis: TimeAxis,
        start: &str,
        end: &str,
        limit: usize,
    ) -> Result<Vec<String>, AmError> {
        let compiled = compile_window(bank_id, axis, start, end, limit, '?')?;
        // The compiled params are bank, end, and start, bound as text. The
        // limit is bound by the backend as an integer, the shape `LIMIT`
        // expects on SQLite.
        let mut binds: Vec<Value> = compiled
            .params
            .iter()
            .map(|p| Value::Text(p.clone()))
            .collect();
        binds.push(Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&compiled.sql).am("window prepare")?;
            let rows = stmt
                .query_map(params_from_iter(binds.iter()), |r| r.get::<_, String>(0))
                .am("window rows")?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.am("window row")?);
            }
            Ok(out)
        })
    }
}

#[cfg(test)]
#[path = "sqlite_window_tests.rs"]
pub(crate) mod sqlite_window_tests;
