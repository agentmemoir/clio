// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite compliance-erasure purge SQL:
//! selecting erased rows and deleting derived index rows per bank.
//!
//! ## Owns
//! - SQLite item/bank selection for erasure
//! - The SQLite derived-row purge (vectors, FTS/outbox, assoc edges, MemTree)
//! - The SQLite persona-context purge for the erasure lifecycle
//!   including its sync-derived copies (journal payload, dead-letter envelope)
//! - The SQLite persona-erase eligibility check (live persona rows plus
//!   sync-derived persona copies) and the bank-level `persona_erase_tombstones`
//!   write that fail-closes later persona writes into an erased bank
//! - Cross-bank fan-out helper for the derived-structure purge
//!
//! ## Does not own
//! - The SPO triple-row purge and its sync-derived copies (`sqlite_erase_triples`)
//! - The erasure transaction orchestration and tombstone writes (`sqlite_erase`)
//! - Postgres parity (see `postgres_erase`)
//!
//! ## Boundary
//!
//! Changes to the SQLite erasure purge SQL should normally require changes here.
//! Changes to transaction ordering or tombstones should require `sqlite_erase`.
//!
//! Keep this module focused on the SQLite erasure purge SQL.

use crate::compliance_store::{BankPurge, EraseRow, PurgedTriples};
use crate::distilled_unit::HUB_DISTILL_EDGE_RELATIONSHIP;
use crate::migrate::IntoAmError;
use crate::sqlite_erase_triples::{purge_item_triples, purge_sync_copies};
use clio_types::AmError;
use rusqlite::{OptionalExtension, params};

/// Item rows (metadata only) for one subject in one bank.
pub(crate) fn select_items(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    subject_id: &str,
) -> Result<Vec<EraseRow>, AmError> {
    let mut stmt = tx
        .prepare(
            "SELECT id, category FROM items
             WHERE bank_id = ?1 AND subject_id = ?2 ORDER BY id",
        )
        .am("erase items prepare")?;
    let rows = stmt
        .query_map(params![bank_id, subject_id], |r| {
            Ok(EraseRow {
                item_id: r.get(0)?,
                category: r.get(1)?,
            })
        })
        .am("erase items query")?;
    rows.collect::<Result<Vec<_>, _>>().am("erase items rows")
}

/// Every bank (including the requesting one) holding items for the subject.
pub(crate) fn select_subject_banks(
    tx: &rusqlite::Transaction<'_>,
    subject_id: &str,
) -> Result<Vec<String>, AmError> {
    let mut stmt = tx
        .prepare("SELECT DISTINCT bank_id FROM items WHERE subject_id = ?1 ORDER BY bank_id")
        .am("erase banks prepare")?;
    let rows = stmt
        .query_map(params![subject_id], |r| r.get(0))
        .am("erase banks query")?;
    rows.collect::<Result<Vec<_>, _>>().am("erase banks rows")
}

/// Whether the bank holds erasable persona content. Persona context is
/// bank-scoped (persona rows carry no subject attribution), so these rows are
/// erasable content for any verified erasure in their bank: an erase that
/// would otherwise find nothing still proceeds to purge them.
///
/// The sealed context lives in more places than the live persona columns: the
/// sync journal seals persona payloads in `sync_mutations.content_sealed` and
/// the dead-letter quarantine stores whole persona mutation envelopes in
/// `sync_dead_letter.envelope_json`. A bank whose only persona content is one
/// of those sync-derived copies is still a persona-bearing bank, so
/// both structures count here — the same rows [`purge_sync_persona_copies`]
/// blanks.
pub(crate) fn bank_has_persona_context(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
) -> Result<bool, AmError> {
    let stable: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM persona_stable_entries
             WHERE bank_id = ?1 AND context_cipher IS NOT NULL",
            params![bank_id],
            |r| r.get(0),
        )
        .am("erase persona stable count")?;
    let preferences: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM persona_preferences
             WHERE bank_id = ?1 AND context_cipher IS NOT NULL",
            params![bank_id],
            |r| r.get(0),
        )
        .am("erase persona preference count")?;
    let journal: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM sync_mutations
             WHERE bank_id = ?1 AND entity_kind = 'persona'
               AND (content_sealed IS NOT NULL OR encrypted <> 0)",
            params![bank_id],
            |r| r.get(0),
        )
        .am("erase persona journal count")?;
    if stable > 0 || preferences > 0 || journal > 0 {
        return Ok(true);
    }
    dead_letter_has_persona(tx, bank_id)
}

/// Whether any of the bank's quarantined dead-letter envelopes carries a
/// persona mutation. The table has no `entity_kind` column, so the match is
/// the same Rust-side envelope parse the purge uses.
fn dead_letter_has_persona(tx: &rusqlite::Transaction<'_>, bank_id: &str) -> Result<bool, AmError> {
    let mut stmt = tx
        .prepare("SELECT envelope_json FROM sync_dead_letter WHERE bank_id = ?1")
        .am("erase persona dead letter count prepare")?;
    let mut rows = stmt
        .query(params![bank_id])
        .am("erase persona dead letter count query")?;
    while let Some(row) = rows.next().am("erase persona dead letter row")? {
        let envelope: String = row.get(0).am("erase persona dead letter value")?;
        if envelope_entity_kind(&envelope).as_deref() == Some("persona") {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Record the bank-level persona erasure tombstone. Persona rows carry no
/// subject attribution, so the purge is bank-scoped; this tombstone is what
/// fail-closes later persona writes into the erased bank (the persona write
/// transactions probe it first and refuse with `ErasedSubject`).
pub(crate) fn write_persona_erase_tombstone(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    erased_at: &str,
) -> Result<(), AmError> {
    tx.execute(
        "INSERT INTO persona_erase_tombstones (bank_id, erased_at)
         VALUES (?1, ?2)
         ON CONFLICT(bank_id) DO UPDATE SET erased_at = excluded.erased_at",
        params![bank_id, erased_at],
    )
    .am("persona erase tombstone write")?;
    Ok(())
}

/// Content-free replacement written over an erased persona-bearing dead-letter
/// envelope: the quarantine row and its ack bookkeeping survive, the payload
/// does not. The triple purge writes the same marker (see
/// `sqlite_erase_triples`).
pub(crate) const ERASED_SYNC_ENVELOPE: &str = r#"{"erased":true}"#;

/// The `entity_kind` recorded inside a dead-letter envelope's mutation JSON.
pub(crate) fn envelope_entity_kind(envelope_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(envelope_json)
        .ok()?
        .get("entity_kind")?
        .as_str()
        .map(str::to_owned)
}

/// Purge the bank's sync-derived persona copies. Erasing a
/// subject must destroy every subject-owned derived copy of its context, not
/// only the live persona columns. Two sync structures hold such a copy:
///
/// - `sync_mutations.content_sealed` seals a plaintext persona payload under
///   `item_subject_id`, which reads only a top-level `subject_id` and defaults
///   to the literal `sync-journal` for persona payloads — a DEK erasure never
///   destroys, so the sealed context stays decryptable after the erase.
/// - `sync_dead_letter.envelope_json` stores the whole mutation envelope
///   verbatim, including the readable persona context string.
///
/// Persona content is bank-scoped (the persona rows carry no subject
/// attribution), so any verified erasure in the bank blanks these copies. The
/// rows and their bookkeeping columns survive (seq/status stay sane for sync's
/// own replay and ack accounting) and only the payloads are replaced with a
/// content-free marker, so no decryptable body remains. Returns the number of
/// rows blanked.
pub(crate) fn purge_sync_persona_copies(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
) -> Result<usize, AmError> {
    let mut purged = tx
        .execute(
            "UPDATE sync_mutations SET content_sealed = NULL, encrypted = 0
             WHERE bank_id = ?1 AND entity_kind = 'persona'
               AND (content_sealed IS NOT NULL OR encrypted <> 0)",
            params![bank_id],
        )
        .am("erase sync journal persona")?;
    // The dead-letter table stores the whole mutation envelope and has no
    // `entity_kind` column, so match the persona rows in Rust.
    let rows = {
        let mut stmt = tx
            .prepare(
                "SELECT sync_mutation_id, envelope_json FROM sync_dead_letter
                 WHERE bank_id = ?1",
            )
            .am("erase sync dead letter prepare")?;
        stmt.query_map(params![bank_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .am("erase sync dead letter query")?
        .collect::<Result<Vec<_>, _>>()
        .am("erase sync dead letter rows")?
    };
    for (id, envelope) in rows {
        if envelope_entity_kind(&envelope).as_deref() == Some("persona") {
            purged += tx
                .execute(
                    "UPDATE sync_dead_letter SET envelope_json = ?2
                     WHERE sync_mutation_id = ?1",
                    params![id, ERASED_SYNC_ENVELOPE],
                )
                .am("erase sync dead letter blank")?;
        }
    }
    Ok(purged)
}

/// Purge the bank's persona sealed context: persona context is sealed
/// under the bank DEK with no finer subject attribution, so any verified
/// erasure in this bank clears `context_cipher` on every stable version and
/// preference row in the bank. The sync-derived copies of that context (the
/// sealed journal payload and the dead-letter envelope) are purged here too,
/// right where the erase transaction clears the live columns (see
/// [`purge_sync_persona_copies`]). Returns the number of rows rendered
/// unreadable. Persona reads keep working afterwards and surface
/// `context: None` (the tolerant open treats a NULL column as absent).
pub(crate) fn purge_persona_context(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
) -> Result<usize, AmError> {
    let mut purged = tx
        .execute(
            "UPDATE persona_stable_entries SET context_cipher = NULL
             WHERE bank_id = ?1 AND context_cipher IS NOT NULL",
            params![bank_id],
        )
        .am("erase persona stable context")?;
    purged += tx
        .execute(
            "UPDATE persona_preferences SET context_cipher = NULL
             WHERE bank_id = ?1 AND context_cipher IS NOT NULL",
            params![bank_id],
        )
        .am("erase persona preference purge")?;
    purged += purge_sync_persona_copies(tx, bank_id)?;
    Ok(purged)
}

/// Purge one bank's index rows for the subject's items and their distilled
/// referrers, and blank the sync-derived copies of the purged triples.
pub(crate) fn purge_bank(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    items: &[EraseRow],
) -> Result<BankPurge, AmError> {
    let erased_ids: Vec<String> = items.iter().map(|r| r.item_id.clone()).collect();
    let derived = distilled_referrers(tx, bank_id, &erased_ids)?;
    let mut out = BankPurge {
        derived_items: derived.clone(),
        ..BankPurge::default()
    };
    for row in items {
        out.vectors += tx
            .execute(
                "DELETE FROM item_embeddings WHERE item_id = ?1 AND bank_id = ?2",
                params![row.item_id, bank_id],
            )
            .am("erase vector")?;
        tx.execute(
            "DELETE FROM index_pending WHERE item_id = ?1 AND bank_id = ?2",
            params![row.item_id, bank_id],
        )
        .am("erase outbox")?;
        tx.execute(
            "DELETE FROM items_fts WHERE item_id = ?1",
            params![row.item_id],
        )
        .am("erase fts")?;
        out.edges += tx
            .execute(
                "DELETE FROM assoc_edges WHERE bank_id = ?1 AND (source_id = ?2 OR target_id = ?2)",
                params![bank_id, row.item_id],
            )
            .am("erase assoc")?;
        out.memtree += purge_memtree(tx, bank_id, &row.item_id)?;
        let purged = purge_item_triples(tx, bank_id, &row.item_id)?;
        out.triples += purged.rows;
        out.triple_ids.extend(purged.ids);
    }
    for item_id in &derived {
        let (v, e, m, t) = purge_derived_item(tx, bank_id, item_id)?;
        out.vectors += v;
        out.edges += e;
        out.memtree += m;
        out.triples += t.rows;
        out.triple_ids.extend(t.ids);
    }
    // The sync journal and the dead-letter quarantine hold their own copies of
    // a triple's payload, sealed under this node's journal DEK — which no
    // data-subject erasure destroys. Blank them here, while the erased triple
    // ids are still in hand.
    purge_sync_copies(tx, bank_id, &out.triple_ids)?;
    Ok(out)
}

/// Delete the subject's MemTree leaf nodes and dirty their ancestor chain.
fn purge_memtree(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    item_id: &str,
) -> Result<usize, AmError> {
    let leaf: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT id, parent_id FROM memtree_nodes WHERE bank_id = ?1 AND item_id = ?2",
            params![bank_id, item_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .am("erase memtree leaf")?;
    let Some((node_id, mut parent)) = leaf else {
        return Ok(0);
    };
    tx.execute("DELETE FROM memtree_nodes WHERE id = ?1", params![node_id])
        .am("erase memtree delete")?;
    let mut purged = 1usize;
    let mut hops = 0usize;
    while let Some(pid) = parent.take() {
        hops += 1;
        if hops > 64 {
            break;
        }
        let updated = tx
            .execute(
                "UPDATE memtree_nodes SET dirty = 1, summary_gist = NULL,
                        generation = generation + 1
                 WHERE id = ?1 AND bank_id = ?2",
                params![pid, bank_id],
            )
            .am("erase memtree dirty")?;
        if updated == 0 {
            break;
        }
        purged += updated;
        parent = tx
            .query_row(
                "SELECT parent_id FROM memtree_nodes WHERE id = ?1",
                params![pid],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()
            .am("erase memtree parent")?
            .flatten();
    }
    Ok(purged)
}

/// Derived hub-distilled items that reference an erased item via the
/// `HUB_DISTILL_EDGE_RELATIONSHIP` edge. Every referrer is purged: Clio keeps no
/// retention path for an affirmatively anonymized derivative.
fn distilled_referrers(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    item_ids: &[String],
) -> Result<Vec<String>, AmError> {
    let mut out: Vec<String> = Vec::new();
    let mut stmt = tx
        .prepare(
            "SELECT source_id, target_id FROM assoc_edges
             WHERE bank_id = ?1 AND relationship = ?3
               AND (source_id = ?2 OR target_id = ?2)",
        )
        .am("erase derived prepare")?;
    for item_id in item_ids {
        let rows = stmt
            .query_map(
                params![bank_id, item_id, HUB_DISTILL_EDGE_RELATIONSHIP],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .am("erase derived query")?;
        for row in rows {
            let (source, target) = row.am("erase derived row")?;
            let other = if &source == item_id { target } else { source };
            if !item_ids.contains(&other) && !out.contains(&other) {
                out.push(other);
            }
        }
    }
    Ok(out)
}

/// Purge a derived item's index rows, its SPO triple rows, and its sealed content.
fn purge_derived_item(
    tx: &rusqlite::Transaction<'_>,
    bank_id: &str,
    item_id: &str,
) -> Result<(usize, usize, usize, PurgedTriples), AmError> {
    let vectors = tx
        .execute(
            "DELETE FROM item_embeddings WHERE item_id = ?1 AND bank_id = ?2",
            params![item_id, bank_id],
        )
        .am("erase derived vector")?;
    tx.execute(
        "DELETE FROM index_pending WHERE item_id = ?1 AND bank_id = ?2",
        params![item_id, bank_id],
    )
    .am("erase derived outbox")?;
    tx.execute("DELETE FROM items_fts WHERE item_id = ?1", params![item_id])
        .am("erase derived fts")?;
    let edges = tx
        .execute(
            "DELETE FROM assoc_edges WHERE bank_id = ?1 AND (source_id = ?2 OR target_id = ?2)",
            params![bank_id, item_id],
        )
        .am("erase derived assoc")?;
    let memtree = purge_memtree(tx, bank_id, item_id)?;
    let triples = purge_item_triples(tx, bank_id, item_id)?;
    tx.execute(
        "UPDATE items SET content_ciphertext = NULL WHERE id = ?1 AND bank_id = ?2",
        params![item_id, bank_id],
    )
    .am("erase derived content")?;
    Ok((vectors, edges, memtree, triples))
}
