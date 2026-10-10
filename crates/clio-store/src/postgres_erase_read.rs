// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres tombstone read SQL shared by the
//! compliance-erasure verification and export surfaces.
//!
//! ## Owns
//! - `compliance_tombstones` SELECT with caller-supplied WHERE clause
//!
//! ## Does not own
//! - The erasure transaction itself (`postgres_erase`)
//! - The tombstone row shape (see `compliance_store`)
//!
//! ## Boundary
//!
//! Changes to tombstone read SQL should normally require changes here.
//! Changes to the erasure transaction should not require changes here.
//!
//! Keep this module focused on tombstone reads.

use crate::compliance_store::ComplianceTombstoneView;
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use clio_types::AmError;

/// Read tombstone rows filtered by a caller-supplied WHERE clause.
pub(crate) fn tombstones_where(
    store: &PostgresStore,
    where_sql: &str,
    values: &[&(dyn postgres::types::ToSql + Sync)],
) -> Result<Vec<ComplianceTombstoneView>, AmError> {
    store.with_client(|client| {
        let sql = format!(
            "SELECT request_id, legal_basis, subject_hash, item_id_hash, category,
                    reason_code, deleted_at
             FROM compliance_tombstones
             WHERE {where_sql}
             ORDER BY item_id_hash NULLS LAST, request_id, deleted_at"
        );
        let rows = client.query(sql.as_str(), values).am("tombstones query")?;
        Ok(rows
            .iter()
            .map(|r| ComplianceTombstoneView {
                request_id: r.get(0),
                legal_basis: r.get(1),
                subject_hash: r.get(2),
                item_id_hash: r.get(3),
                category: r.get(4),
                reason_code: r.get(5),
                deleted_at: r.get(6),
            })
            .collect())
    })
}
