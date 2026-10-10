// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite compliance purge of SPO triple
//! rows and of the sync-derived copies of the same triple payloads.
//!
//! ## Owns
//! - Deleting the `triples` rows carried by an erased (or content-cleared) item
//! - Blanking the sync-journal payloads and dead-letter envelopes that still
//!   hold a purged triple's plaintext `subject` / `predicate` / value
//!
//! ## Does not own
//! - The rest of the derived-row purge (`sqlite_erase_purge`)
//! - The erasure transaction, DEK shredding, and tombstones (`sqlite_erase`)
//! - Postgres parity (see `postgres_erase::triple_purge`)
//!
//! ## Boundary
//!
//! Changes to which triple rows or triple payload copies the erasure destroys
//! should normally require changes here. Changes to the erasure transaction
//! ordering should require `sqlite_erase`.

use crate::compliance_store::PurgedTriples;
use crate::migrate::IntoAmError;
use clio_types::AmError;
use rusqlite::params;

/// Delete the SPO triple rows carried by one item (issue 20).
///
/// A triple row holds two different things. The asserted VALUE is sealed under
/// the carrier item's subject DEK, so crypto-shredding that DEK already makes it
/// unreadable. The `subject` + `predicate` identity key is plaintext by
/// requirement makes it the supersession identity, backed by the
/// `triples_open_uniq` index — so shredding the DEK leaves it perfectly
/// readable. Deleting the row is what makes the recorded erasure attestation
/// true instead of merely reported.
///
/// Returns the deleted row count and ids.
pub(crate) fn purge_item_triples(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    item_id: &str,
) -> Result<PurgedTriples, AmError> {
    let ids = {
        let mut stmt = tx
            .prepare("SELECT id FROM triples WHERE bank_id = ?1 AND item_id = ?2 ORDER BY id")
            .am("erase triples select")?;
        let rows = stmt
            .query_map(params![bank_id, item_id], |r| r.get::<_, String>(0))
            .am("erase triples query")?;
        rows.collect::<Result<Vec<_>, _>>()
            .am("erase triples rows")?
    };
    if ids.is_empty() {
        return Ok(PurgedTriples::default());
    }
    tx.execute(
        "DELETE FROM triples WHERE bank_id = ?1 AND item_id = ?2",
        params![bank_id, item_id],
    )
    .am("erase triples")?;
    Ok(PurgedTriples {
        rows: ids.len(),
        ids,
    })
}

/// Blank the sync-derived copies of the purged triples' payloads.
///
/// A closed triple is journaled as an `invalidate` mutation carrying its
/// plaintext `subject`, `predicate`, and value. The journal seals that payload
/// under the node-local journal DEK rather than under the data subject's DEK
/// (the triple payload has no top-level `subject_id`, so the extractor falls
/// back to a fixed journal subject), which means no data-subject erasure ever
/// destroys it. The dead-letter quarantine stores the whole mutation envelope
/// verbatim on top of that.
///
/// Both rows keep their bookkeeping columns so sync's own replay and ack
/// accounting stay sane; only the payload is replaced with a content-free
/// marker. Returns the number of journal rows blanked; the dead-letter rows
/// blanked are counted into the same total.
pub(crate) fn purge_sync_copies(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    triple_ids: &[String],
) -> Result<usize, AmError> {
    if triple_ids.is_empty() {
        return Ok(0);
    }
    let mut purged = 0usize;
    for id in triple_ids {
        purged += tx
            .execute(
                "UPDATE sync_mutations SET content_sealed = NULL, encrypted = 0
                 WHERE bank_id = ?1 AND entity_kind = 'triple' AND entity_id = ?2",
                params![bank_id, id],
            )
            .am("erase sync journal triple")?;
    }
    purged += purge_dead_letter_copies(tx, bank_id, triple_ids)?;
    Ok(purged)
}

/// Blank the dead-letter envelopes of the purged triples' mutations.
///
/// `sync_dead_letter` has no `entity_kind` or `entity_id` column, so the match is
/// the same Rust-side envelope parse the persona purge uses. Returns the number
/// of envelopes blanked.
fn purge_dead_letter_copies(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    triple_ids: &[String],
) -> Result<usize, AmError> {
    let rows = {
        let mut stmt = tx
            .prepare(
                "SELECT sync_mutation_id, envelope_json FROM sync_dead_letter WHERE bank_id = ?1",
            )
            .am("erase triple dead letter prepare")?;
        let rows = stmt
            .query_map(params![bank_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .am("erase triple dead letter query")?;
        rows.collect::<Result<Vec<_>, _>>()
            .am("erase triple dead letter rows")?
    };
    let mut purged = 0usize;
    for (mutation_id, envelope) in rows {
        let is_purged_triple =
            crate::sqlite_erase_purge::envelope_entity_kind(&envelope).as_deref() == Some("triple")
                && envelope_entity_id(&envelope).is_some_and(|id| triple_ids.contains(&id));
        if is_purged_triple {
            purged += tx
                .execute(
                    "UPDATE sync_dead_letter SET envelope_json = ?2 WHERE sync_mutation_id = ?1",
                    params![mutation_id, crate::sqlite_erase_purge::ERASED_SYNC_ENVELOPE],
                )
                .am("erase triple dead letter blank")?;
        }
    }
    Ok(purged)
}

/// The `entity_id` recorded inside a dead-letter envelope's mutation JSON.
pub(crate) fn envelope_entity_id(envelope_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(envelope_json)
        .ok()?
        .get("entity_id")?
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
#[path = "sqlite_erase_triples_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sqlite_erase_triples_seal_erase_tests.rs"]
pub(crate) mod sqlite_erase_triples_seal_erase_tests;
