// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the freshness block a read surface attaches to
//! a distilled unit: resolving which derivation record a read names, and
//! rendering that record's freshness and evidence trail as one additive key.
//!
//! ## Owns
//! - `block_for`, the record lookup plus the rendered block
//! - `render`, the block's shape (ids, counts, and times only)
//! - `FRESHNESS_KEY`, the response key the block is carried under
//!
//! ## Does not own
//! - The freshness decision and the record shape (`clio-store`'s
//!   `unit_freshness` and `DistilledUnitRecord`)
//! - Which read surfaces carry the block (`read_tools`)
//! - The consolidation wave report (`clio-write`)
//!
//! ## Boundary
//!
//! Changes to the freshness block's shape should normally require changes here.
//! Changes to the decision it reports should not require changes here.
//!
//! Keep this module focused on rendering one unit's freshness, not on deciding
//! it.

use clio_store::{DistilledUnitRecord, Store};
use clio_types::AmError;
use serde_json::{Value, json};

/// The key `get` and `inspect` carry a unit's freshness under.
///
/// A new top-level key rather than a field inside the item: a read of anything
/// that is not a distilled unit stays byte-identical to the plain item, and the
/// block cannot collide with a column, a snapshot key, or a context field the
/// item already owns.
pub(crate) const FRESHNESS_KEY: &str = "freshness";

/// The freshness block for the distilled unit `item_id` names, or `None` when
/// the item is not a distilled unit.
///
/// `snapshot` is the item's own snapshot, needed because the derivation record
/// is keyed by the stable unit id while a read may hold only a revision item id
/// (`{unit}:v2`).
///
/// # Errors
/// Storage failures reading the record or the claimed sources' metadata.
pub(crate) fn block_for(
    store: &dyn Store,
    bank_id: &str,
    item_id: &str,
    snapshot: Option<&Value>,
) -> Result<Option<Value>, AmError> {
    let Some(record) = record_named_by(store, bank_id, item_id, snapshot)? else {
        return Ok(None);
    };
    Ok(Some(render(store, bank_id, &record)?))
}

/// The derivation record a read of `item_id` should report on.
///
/// Resolution order, and why:
///
/// 1. `snapshot["unit_id"]` — the record names itself in the projection written
///    from that same record, so it is found at every revision, including the
///    `{unit}:v{n}` item ids that do not match the record's own key.
/// 2. `item_id` itself — the record key at revision 1, where the item id and
///    the unit id are the same string, and the only answer available for a
///    snapshot written before the projection carried `unit_id`.
///
/// One lookup, never two: once the snapshot names a unit, the item id is not a
/// second candidate, and a second read could only surface a record the item
/// does not claim.
fn record_named_by(
    store: &dyn Store,
    bank_id: &str,
    item_id: &str,
    snapshot: Option<&Value>,
) -> Result<Option<DistilledUnitRecord>, AmError> {
    let key = clio_store::projected_unit_id(snapshot).unwrap_or(item_id);
    store.get_distilled_unit(bank_id, key)
}

/// Render one record's freshness block: what the unit is, never what its sources
/// say.
///
/// - `revision` — the record's own current revision, so a reader can tell which
///   reconciliation the rest of the block describes. A unit whose current
///   revision predates the evidence trail reports `proof_count` 0 and has no
///   entry for this revision in `evidence`; naming the revision is what makes
///   that state readable rather than ambiguous.
/// - `proof_count` — distinct sources behind the current revision.
/// - `last_reconciled_at` — when the current revision was reconciled, which is
///   the record's own stamp.
/// - `stale` — `false` current, `true` not current, and **`null` for unknown**:
///   the store could not place a stamp, so the unit is reported as unknown and
///   never as current. The key is always present, so `null` is a value here, not
///   a gap.
/// - `stale_reason` — the one reason when stale, `null` otherwise. The
///   vocabulary is the store's and is not extended here: `source_removed`,
///   `source_changed`, `revision_rejected`.
/// - `evidence` — the per-revision trail, oldest revision first, one entry per
///   claimed source holding that source's id and its `as_of` time.
///
/// Ids, counts, and times only: no content, gist, or source text. A caller
/// authorized for the unit learns nothing about a source beyond the id and the
/// time the unit's own snapshot already names, and no source is opened to
/// answer this — the comparison is made from source metadata, never from a
/// decrypted body.
///
/// # Errors
/// Storage failures reading the claimed sources' metadata.
fn render(
    store: &dyn Store,
    bank_id: &str,
    record: &DistilledUnitRecord,
) -> Result<Value, AmError> {
    let freshness = clio_store::unit_freshness(store, bank_id, record)?;
    Ok(json!({
        "revision": record.revision,
        "proof_count": freshness.proof_count,
        "last_reconciled_at": freshness.last_reconciled_at,
        "stale": freshness.stale,
        "stale_reason": freshness.stale_reason,
        "evidence": record.evidence_history(),
    }))
}

#[cfg(test)]
#[path = "read_freshness_tests.rs"]
mod read_freshness_tests;
