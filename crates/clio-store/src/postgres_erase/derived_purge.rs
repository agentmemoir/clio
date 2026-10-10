// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres erasure transaction's
//! derived-row purge SQL: item/bank selection plus deletion of vectors,
//! tsvector, index-outbox rows, association edges, and MemTree nodes per bank.
//!
//! ## Owns
//! - Postgres item/bank selection for erasure
//! - The Postgres derived-row purge (vectors, tsvector, assoc edges, MemTree)
//! - Hub-distilled referrer detection and its derived-item purge
//!
//! ## Does not own
//! - The SPO triple-row purge and its sync-derived copies
//!   (`postgres_erase::triple_purge`)
//!
//! ## Does not own
//! - The erasure transaction orchestration and tombstone writes (`postgres_erase`)
//! - The persona-context purge (`postgres_erase::persona_purge`)
//! - SQLite parity (see `sqlite_erase_purge`)
//!
//! ## Boundary
//!
//! Changes to which derived rows the erasure deletes should normally require
//! changes here. Changes to transaction ordering should require `postgres_erase`.
//!
//! Keep this module focused on the Postgres erasure purge SQL.

use crate::compliance_store::{BankPurge, EraseRow, PurgedTriples};
use crate::distilled_unit::HUB_DISTILL_EDGE_RELATIONSHIP;
use crate::migrate::IntoAmError;
use crate::postgres_erase::triple_purge::{purge_item_triples, purge_sync_copies};
use clio_types::AmError;

/// Item rows (metadata only) for one subject in one bank.
pub(crate) fn select_items(
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
    subject_id: &str,
) -> Result<Vec<EraseRow>, AmError> {
    let rows = tx
        .query(
            "SELECT id, category FROM items
             WHERE bank_id = $1 AND subject_id = $2 ORDER BY id",
            &[&bank_id, &subject_id],
        )
        .am("erase items")?;
    Ok(rows
        .iter()
        .map(|r| EraseRow {
            item_id: r.get(0),
            category: r.get(1),
        })
        .collect())
}

/// Every bank (including the requesting one) holding items for the subject.
pub(crate) fn select_subject_banks(
    tx: &mut postgres::Transaction<'_>,
    subject_id: &str,
) -> Result<Vec<String>, AmError> {
    let rows = tx
        .query(
            "SELECT DISTINCT bank_id FROM items WHERE subject_id = $1 ORDER BY bank_id",
            &[&subject_id],
        )
        .am("erase banks")?;
    Ok(rows.iter().map(|r| r.get(0)).collect())
}

/// Purge one bank's index rows for the subject's items and their distilled
/// referrers, and blank the sync-derived copies of the purged triples.
pub(crate) fn purge_bank(
    tx: &mut postgres::Transaction<'_>,
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
                "DELETE FROM item_embeddings WHERE item_id = $1 AND bank_id = $2",
                &[&row.item_id, &bank_id],
            )
            .am("erase vector")?
            .try_into()
            .unwrap_or(0usize);
        tx.execute(
            "DELETE FROM index_pending WHERE item_id = $1 AND bank_id = $2",
            &[&row.item_id, &bank_id],
        )
        .am("erase outbox")?;
        tx.execute(
            "UPDATE items SET content_tsv = NULL WHERE id = $1 AND bank_id = $2",
            &[&row.item_id, &bank_id],
        )
        .am("erase tsv")?;
        out.edges += tx
            .execute(
                "DELETE FROM assoc_edges WHERE bank_id = $1 AND (source_id = $2 OR target_id = $2)",
                &[&bank_id, &row.item_id],
            )
            .am("erase assoc")?
            .try_into()
            .unwrap_or(0usize);
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
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
    item_id: &str,
) -> Result<usize, AmError> {
    let Some(row) = tx
        .query_opt(
            "SELECT id, parent_id FROM memtree_nodes WHERE bank_id = $1 AND item_id = $2",
            &[&bank_id, &item_id],
        )
        .am("erase memtree leaf")?
    else {
        return Ok(0);
    };
    let node_id: String = row.get(0);
    let mut parent: Option<String> = row.get(1);
    tx.execute("DELETE FROM memtree_nodes WHERE id = $1", &[&node_id])
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
                 WHERE id = $1 AND bank_id = $2",
                &[&pid, &bank_id],
            )
            .am("erase memtree dirty")?;
        if updated == 0 {
            break;
        }
        purged += updated.try_into().unwrap_or(0usize);
        parent = tx
            .query_opt("SELECT parent_id FROM memtree_nodes WHERE id = $1", &[&pid])
            .am("erase memtree parent")?
            .and_then(|r| r.get::<_, Option<String>>(0));
    }
    Ok(purged)
}

/// Derived hub-distilled items that reference an erased item via the
/// `HUB_DISTILL_EDGE_RELATIONSHIP` edge and lack affirmative anonymization.
fn distilled_referrers(
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
    item_ids: &[String],
) -> Result<Vec<String>, AmError> {
    let mut out: Vec<String> = Vec::new();
    for item_id in item_ids {
        let rows = tx
            .query(
                "SELECT source_id, target_id FROM assoc_edges
                 WHERE bank_id = $1 AND relationship = $3
                   AND (source_id = $2 OR target_id = $2)",
                &[&bank_id, &item_id, &HUB_DISTILL_EDGE_RELATIONSHIP],
            )
            .am("erase derived query")?;
        for row in &rows {
            let source: String = row.get(0);
            let target: String = row.get(1);
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
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
    item_id: &str,
) -> Result<(usize, usize, usize, PurgedTriples), AmError> {
    let vectors: usize = tx
        .execute(
            "DELETE FROM item_embeddings WHERE item_id = $1 AND bank_id = $2",
            &[&item_id, &bank_id],
        )
        .am("erase derived vector")?
        .try_into()
        .unwrap_or(0usize);
    tx.execute(
        "DELETE FROM index_pending WHERE item_id = $1 AND bank_id = $2",
        &[&item_id, &bank_id],
    )
    .am("erase derived outbox")?;
    tx.execute(
        "UPDATE items SET content_tsv = NULL WHERE id = $1 AND bank_id = $2",
        &[&item_id, &bank_id],
    )
    .am("erase derived tsv")?;
    let edges: usize = tx
        .execute(
            "DELETE FROM assoc_edges WHERE bank_id = $1 AND (source_id = $2 OR target_id = $2)",
            &[&bank_id, &item_id],
        )
        .am("erase derived assoc")?
        .try_into()
        .unwrap_or(0usize);
    let memtree = purge_memtree(tx, bank_id, item_id)?;
    let triples = purge_item_triples(tx, bank_id, item_id)?;
    tx.execute(
        "UPDATE items SET content_ciphertext = NULL WHERE id = $1 AND bank_id = $2",
        &[&item_id, &bank_id],
    )
    .am("erase derived content")?;
    Ok((vectors, edges, memtree, triples))
}
