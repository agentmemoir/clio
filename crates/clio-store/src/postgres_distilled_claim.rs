// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres read of a distilled unit's
//! claimed sources' metadata: each id's last-change time and whether it is still
//! in active use, without touching any payload.
//!
//! ## Owns
//! - The `distilled_unit_claim_state` Postgres implementation
//!
//! ## Does not own
//! - The rest of the Postgres record SQL (`postgres_distilled_unit`)
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
use crate::postgres::PostgresStore;
use clio_types::AmError;
use std::fmt::Write as _;

impl PostgresStore {
    /// Bank-scoped claim-state read. See [`crate::DistilledUnitStore`].
    pub(crate) fn distilled_unit_claim_state_impl(
        &self,
        bank_id: &str,
        claim_ids: &[String],
    ) -> Result<Vec<ClaimedSourceState>, AmError> {
        if claim_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut sql = String::from(
            "SELECT id, updated_at, discarded_at IS NULL FROM items
             WHERE bank_id = $1 AND id IN (",
        );
        for (index, _) in claim_ids.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            let _ = write!(sql, "${}", index + 2);
        }
        sql.push(')');
        self.with_client(|client| {
            let bank = bank_id.to_owned();
            let mut bind: Vec<&(dyn postgres::types::ToSql + Sync)> = vec![&bank];
            for id in claim_ids {
                bind.push(id);
            }
            let rows = client
                .query(&sql, &bind)
                .am("distilled claim state query pg")?;
            let mut states: Vec<ClaimedSourceState> = rows
                .iter()
                .map(|row| {
                    Ok(ClaimedSourceState {
                        source_id: row.get(0),
                        updated_at: Some(row.get(1)),
                        active: row.get(2),
                    })
                })
                .collect::<Result<Vec<_>, AmError>>()?;
            for id in claim_ids {
                if !states.iter().any(|state| &state.source_id == id) {
                    states.push(ClaimedSourceState {
                        source_id: id.clone(),
                        updated_at: None,
                        active: false,
                    });
                }
            }
            states.sort_by(|a, b| a.source_id.cmp(&b.source_id));
            states.dedup_by(|a, b| a.source_id == b.source_id);
            Ok(states)
        })
    }
}
