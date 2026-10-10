// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres compliance-tombstone insert
//! SQL used by the erasure transaction.
//!
//! ## Owns
//! - The Postgres `compliance_tombstones` insert for subject- and item-level rows
//!
//! ## Does not own
//! - The erasure transaction orchestration (`postgres_erase`)
//! - Tombstone reads (`postgres_erase_read`)
//! - SQLite parity (see `sqlite_erase`)
//!
//! ## Boundary
//!
//! Changes to tombstone SQL should normally require changes here. Changes to
//! transaction ordering should require `postgres_erase`.
//!
//! Keep this module focused on the Postgres tombstone insert.

use crate::compliance_store::SubjectEraseSpec;
use crate::migrate::IntoAmError;
use clio_types::AmError;

pub(crate) fn insert_tombstone(
    tx: &mut postgres::Transaction<'_>,
    spec: &SubjectEraseSpec<'_>,
    subject_hash: &str,
    item_hash: Option<&str>,
    category: Option<&str>,
) -> Result<(), AmError> {
    let suffix = item_hash.unwrap_or(subject_hash);
    tx.execute(
        "INSERT INTO compliance_tombstones (
            id, bank_id, request_id, legal_basis, subject_hash, item_id_hash,
            category, reason_code, deleted_at
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        &[
            &format!("ct-{}-{}", spec.request_id, suffix),
            &spec.bank_id,
            &spec.request_id,
            &spec.legal_basis,
            &subject_hash,
            &item_hash,
            &category,
            &spec.reason_code,
            &spec.deleted_at,
        ],
    )
    .am("tombstone insert")?;
    Ok(())
}
