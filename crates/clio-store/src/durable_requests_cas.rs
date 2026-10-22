// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module owns atomic conditional writes of durable request rows on
//! SQLite and Postgres. Each write compares the entire expected row in SQL.
//!
//! ## Owns
//! - Insert-only and exact-row replacement SQL for both backends
//! - Expected/replacement identity validation
//!
//! ## Does not own
//! - Row vocabulary, unconditional writes, or request state transitions
//!
//! ## Boundary
//!
//! Conditional persistence changes belong here. Domain decisions belong in
//! the consumer that builds the replacement row.

use super::{DurableRequestRow, identity_conflict, validate_row};
use crate::durable_requests_converge::{converge_pg, converge_sqlite};
use crate::migrate::IntoAmError;
use crate::{PostgresStore, SqliteStore};
use clio_types::AmError;

fn validate(expected: Option<&DurableRequestRow>, row: &DurableRequestRow) -> Result<(), AmError> {
    validate_row(row)?;
    if let Some(expected) = expected {
        validate_row(expected)?;
        if expected.request_id != row.request_id
            || expected.kind != row.kind
            || expected.scope != row.scope
        {
            return Err(identity_conflict(&row.request_id));
        }
    }
    Ok(())
}

pub(super) fn save_sqlite(
    store: &SqliteStore,
    expected: Option<&DurableRequestRow>,
    row: &DurableRequestRow,
) -> Result<bool, AmError> {
    validate(expected, row)?;
    store.with_conn(|conn| {
        converge_sqlite(conn)?;
        let changed = if let Some(expected) = expected {
            conn.execute(
                "UPDATE durable_requests SET state = ?1, deadline = ?2, record = ?3
                 WHERE request_id = ?4 AND kind = ?5 AND state = ?6 AND scope = ?7
                   AND deadline IS ?8 AND record = ?9",
                rusqlite::params![
                    row.state,
                    row.deadline,
                    row.record,
                    expected.request_id,
                    expected.kind,
                    expected.state,
                    expected.scope,
                    expected.deadline,
                    expected.record
                ],
            )
        } else {
            conn.execute(
                "INSERT INTO durable_requests (request_id, kind, state, scope, deadline, record)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (request_id) DO NOTHING",
                rusqlite::params![
                    row.request_id,
                    row.kind,
                    row.state,
                    row.scope,
                    row.deadline,
                    row.record
                ],
            )
        }
        .am("durable request conditional save")?;
        Ok(changed == 1)
    })
}

pub(super) fn save_pg(
    store: &PostgresStore,
    expected: Option<&DurableRequestRow>,
    row: &DurableRequestRow,
) -> Result<bool, AmError> {
    validate(expected, row)?;
    store.with_client(|client| {
        converge_pg(client)?;
        let changed =
            if let Some(expected) = expected {
                client.execute(
                    "UPDATE durable_requests SET state = $1, deadline = $2, record = $3
                 WHERE request_id = $4 AND kind = $5 AND state = $6 AND scope = $7
                   AND deadline IS NOT DISTINCT FROM $8 AND record = $9",
                    &[
                        &row.state,
                        &row.deadline,
                        &row.record,
                        &expected.request_id,
                        &expected.kind,
                        &expected.state,
                        &expected.scope,
                        &expected.deadline,
                        &expected.record,
                    ],
                )
            } else {
                client.execute(
                "INSERT INTO durable_requests (request_id, kind, state, scope, deadline, record)
                 VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (request_id) DO NOTHING",
                &[&row.request_id, &row.kind, &row.state, &row.scope, &row.deadline, &row.record],
            )
            }
            .am("durable request conditional save")?;
        Ok(changed == 1)
    })
}

#[cfg(test)]
#[path = "durable_requests_cas_tests.rs"]
mod durable_requests_cas_tests;

#[cfg(test)]
#[path = "durable_requests_cas_pg_tests.rs"]
mod durable_requests_cas_pg_tests;
