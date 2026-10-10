// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the terminal reconcile outcomes of a wave: the
//! `DistillOutcome` value each non-revising reconcile decision reports.
//!
//! ## Owns
//! - The status strings and reasons for `stale_hub_gone`,
//!   `unit_superseded`, `purged`, `reconcile_no_neighbors`,
//!   `revision_failed`, and `reconciled_current`
//!
//! ## Does not own
//! - The decision that picks each outcome (`hub_distill_reconcile`)
//! - The status vocabulary and report routing (`hub_distill_model`)
//! - The store primitives the outcomes describe (`clio-store`)
//!
//! ## Boundary
//!
//! Changes to what a terminal reconcile outcome reports should normally require
//! changes here. Changes to how an outcome is chosen should not require changes
//! here.
//!
//! Keep this module focused on the terminal outcome vocabulary.

use super::DistillOutcome;
use clio_store::DistilledUnitRecord;

/// A unit whose hub left active use: the cascade already marked it, so it stays
/// stale and readable and is never rebuilt here.
pub(super) fn stale_hub_outcome(record: &DistilledUnitRecord) -> DistillOutcome {
    DistillOutcome {
        hub_id: record.hub_id.clone(),
        item_id: record.current_revision_item_id(),
        status: "stale_hub_gone".into(),
        admission_score: None,
        reason: Some("hub is not in active use".into()),
    }
}

/// A unit whose tracked current revision was superseded by an operator
/// correction: the record no longer tracks the unit's text, so reconciliation
/// leaves it alone.
pub(super) fn superseded_outcome(record: &DistilledUnitRecord) -> DistillOutcome {
    DistillOutcome {
        hub_id: record.hub_id.clone(),
        item_id: record.current_revision_item_id(),
        status: "unit_superseded".into(),
        admission_score: None,
        reason: Some("current revision was superseded by an operator correction".into()),
    }
}

/// A unit whose content erasure purged: the unit is dead and is never
/// resurrected.
pub(super) fn purged_outcome(record: &DistilledUnitRecord) -> DistillOutcome {
    DistillOutcome {
        hub_id: record.hub_id.clone(),
        item_id: record.current_revision_item_id(),
        status: "purged".into(),
        admission_score: None,
        reason: Some("unit content was purged by erasure".into()),
    }
}

/// A unit whose entire claim set left active use. There is no cluster left to
/// summarize, so nothing is re-derived and the unit keeps its stale reason.
pub(super) fn no_neighbors_outcome(record: &DistilledUnitRecord) -> DistillOutcome {
    DistillOutcome {
        hub_id: record.hub_id.clone(),
        item_id: record.current_revision_item_id(),
        status: "reconcile_no_neighbors".into(),
        admission_score: None,
        reason: Some("no remaining sources to summarize".into()),
    }
}

/// A unit whose reconcile write could not land because its next revision id is
/// occupied by another row. The unit is left exactly as it was — prior current,
/// stale state unchanged — and reconciliation continues for every other unit.
pub(super) fn failed_outcome(record: &DistilledUnitRecord, detail: &str) -> DistillOutcome {
    DistillOutcome {
        hub_id: record.hub_id.clone(),
        item_id: record.current_revision_item_id(),
        status: "revision_failed".into(),
        admission_score: None,
        reason: Some(detail.to_owned()),
    }
}

/// A unit the comparison found unchanged: no writes of any kind.
pub(super) fn unchanged_outcome(record: &DistilledUnitRecord) -> DistillOutcome {
    DistillOutcome {
        hub_id: record.hub_id.clone(),
        item_id: record.current_revision_item_id(),
        status: "reconciled_current".into(),
        admission_score: None,
        reason: None,
    }
}
