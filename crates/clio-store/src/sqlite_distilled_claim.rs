// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite read of a distilled unit's claimed
//! sources' metadata: each id's last-change time and whether it is still in
//! active use, without touching any payload.
//!
//! ## Owns
//! - The `distilled_unit_claim_state` SQLite implementation
//!
//! ## Does not own
//! - The rest of the SQLite record SQL (`sqlite_distilled_unit`)
//! - The freshness decision (`distilled_freshness`)
//!
//! ## Boundary
//!
//! Changes to this metadata read should normally require changes here. Changes
//! to the freshness decision should not require changes here.
//!
//! Keep this module focused on the claim-state read.

use crate::distilled_freshness::ClaimedSourceState;
use crate::migrate::IntoAmError;
use crate::sqlite::SqliteStore;
use clio_types::AmError;

impl SqliteStore {
    /// Bank-scoped claim-state read. See [`crate::DistilledUnitStore`].
    pub(crate) fn distilled_unit_claim_state_impl(
        &self,
        bank_id: &str,
        claim_ids: &[String],
    ) -> Result<Vec<ClaimedSourceState>, AmError> {
        if claim_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (0..claim_ids.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, updated_at, discarded_at IS NULL FROM items
             WHERE bank_id = ?1 AND id IN ({placeholders})"
        );
        let mut bind: Vec<String> = vec![bank_id.to_owned()];
        bind.extend(claim_ids.iter().cloned());
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(&sql).am("distilled claim state prepare")?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(bind.iter()), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .am("distilled claim state query")?;
            let mut found = Vec::new();
            for row in rows {
                found.push(row.am("distilled claim state row")?);
            }
            // An id the bank does not hold still answers, as inactive: a caller
            // comparing a claim set against the store must be able to see that a
            // source is gone rather than infer it from an absent row.
            let mut states: Vec<ClaimedSourceState> = found
                .into_iter()
                .map(|(source_id, updated_at, active)| ClaimedSourceState {
                    source_id,
                    updated_at: Some(updated_at),
                    active: active != 0,
                })
                .collect();
            for id in claim_ids {
                if !states.iter().any(|state| &state.source_id == id) {
                    states.push(ClaimedSourceState {
                        source_id: id.clone(),
                        updated_at: None,
                        active: false,
                    });
                }
            }
            // Deterministic order by id, so a caller never sees a page that
            // depends on the query plan.
            states.sort_by(|a, b| a.source_id.cmp(&b.source_id));
            states.dedup_by(|a, b| a.source_id == b.source_id);
            Ok(states)
        })
    }
}
