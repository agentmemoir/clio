// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres extension probes at open time.
//!
//! ## Owns
//! - Fail-closed checks against `pg_available_extensions`
//!
//! ## Does not own
//! - Migration DDL or MemTree CRUD
//!
//! ## Boundary
//!
//! Changes to extension gating should normally require changes here.
//! Changes to store CRUD should not require changes here.
//!
//! Keep this module focused on Postgres extension availability probes.

use crate::migrate::IntoAmError;
use clio_types::{AmError, ErrorCode};
use postgres::Client;

/// Fail closed when a required Postgres extension is unavailable.
///
/// Probes `pg_available_extensions` (server capability, not per-DB state) so
/// a missing `vector` install surfaces as `ConfigCorrupt` with operator
/// guidance before migration runs.
pub(crate) fn require_extension(client: &mut Client, name: &str) -> Result<(), AmError> {
    let available: bool = client
        .query_one(
            "SELECT EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = $1)",
            &[&name],
        )
        .am("pg extension check")?
        .get(0);
    if !available {
        return Err(AmError::new(
            ErrorCode::ConfigCorrupt,
            format!(
                "postgres extension `{name}` is not installed on the server; \
                 install the pgvector package — refusing to open"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "postgres_ext_tests.rs"]
mod postgres_ext_tests;
