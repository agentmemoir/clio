// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the per-kind push watermark cursors: their
//! durable cursor keys, the read path that falls back to the one-shot
//! conversion of the retired shared cursor, the guarded conversion write, and
//! the durable write after a successful push ack.
//!
//! ## Owns
//! - The per-kind cursor keys (`push_wm:<kind>`) and the retired shared
//!   `push_wm` label they convert from
//! - `kind_watermark`: read the kind's own cursor, else convert the shared
//!   cursor once, else the canonical empty-watermark sentinel
//! - The guarded conversion write (`kind_watermark_convert_write`) that never
//!   resets a concurrently advanced cursor
//! - `kind_watermark_set`, the durable write of one kind's compound body
//! - The page-complete position of one item row: the greatest store stamp it
//!   was selected by, ordered by the instant it names (`fold_item_position`)
//! - `KindBatch`, the per-kind record of the position to advance to and of the
//!   reasons to hold (`skipped`, `has_rejection`), and `record_kind_batch`
//!
//! ## Does not own
//! - The push run that decides when each cursor advances (`client_feed`)
//! - Watermark token encoding/decoding and the canonical body forms
//!   (`protocol`)
//! - The cursor storage itself (`clio-store` `SyncStore`)
//!
//! ## Boundary
//!
//! Changes to how a kind's watermark cursor is read, converted, or written
//! should normally require changes here. Changes to feed packaging or the push
//! request itself should not.
//!
//! Keep this module focused on the per-kind push watermark cursors.

use crate::apply::apply_freshness::writer_stamp_order;
use crate::client::SyncClientConfig;
use crate::protocol::SyncMutation;
use crate::protocol::{canonical_watermark_body, decode_watermark, encode_watermark};
use clio_store::SyncFeedRow;
use clio_types::{AmError, MemoryItem};

/// Cursor label of the retired shared push watermark that all kinds advanced
/// together before the per-kind cursors. It is read only as the one-shot
/// conversion input below, never as a resume point.
const LEGACY_SHARED_WM_LABEL: &str = "push_wm";

/// Durable cursor label for one kind's push watermark.
fn push_wm_label(kind: &str) -> String {
    format!("push_wm:{kind}")
}

/// Read one kind's canonical watermark body (`stamp` or `stamp\u{1f}entity`):
/// its own cursor when present, else the one-shot conversion of the retired
/// shared `push_wm` cursor, else the canonical empty-watermark sentinel. Read
/// surfaces reach this through `pending_push` (`client.rs`), so a status read
/// can write the converted cursor once and a failed write is non-fatal.
pub(crate) fn kind_watermark(
    cfg: &SyncClientConfig,
    bank: &str,
    kind: &str,
) -> Result<String, AmError> {
    if let Some(token) = cfg.sync.cursor_get(bank, &push_wm_label(kind))? {
        return decode_watermark(bank, &token);
    }
    if let Some(token) = cfg.sync.cursor_get(bank, LEGACY_SHARED_WM_LABEL)? {
        let canonical = canonical_watermark_body(&decode_watermark(bank, &token)?);
        kind_watermark_convert_write(cfg, bank, kind, &canonical);
        return Ok(canonical);
    }
    Ok(canonical_watermark_body(&decode_watermark(bank, "")?))
}

/// The guarded conversion write: re-read the kind cursor immediately before
/// writing and skip when a concurrent push advanced it since the first read,
/// so the conversion never resets durable resume progress. A failed or
/// unprobed write is logged and non-fatal: the decoded pair is unchanged, the
/// shared row remains the recovery input, and the next read retries, so
/// callers continue from the unconverted cursor. Not one atomic store
/// operation (no conditional write in the store API); racing conversions
/// compute the same value, so only an advance can win the gap.
pub(crate) fn kind_watermark_convert_write(
    cfg: &SyncClientConfig,
    bank: &str,
    kind: &str,
    canonical: &str,
) {
    let absent = cfg
        .sync
        .cursor_get(bank, &push_wm_label(kind))
        .is_ok_and(|t| t.is_none());
    if absent && let Err(err) = kind_watermark_set(cfg, bank, kind, canonical) {
        eprintln!("sync watermark conversion write failed for {bank}/{kind}: {err}");
    }
}

/// Every store-local stamp the item feed's predicate compares for one returned
/// row: the ordering stamp the feed returned, the transaction-time bounds, and
/// the creation stamp. The close stamp can sit above `updated_at` (the store
/// places it one second past `tx_from` when a correction is not later), so the
/// ordering stamp alone does not bound the row; a discard writes `discarded_at`
/// and `updated_at` from one stamp, which the ordering stamp covers.
pub(crate) fn item_row_stamps<'a>(row: &'a SyncFeedRow, item: &'a MemoryItem) -> [&'a str; 5] {
    [
        row.transaction_time.as_str(),
        item.tx_from.as_str(),
        item.tx_until.as_deref().unwrap_or(""),
        item.updated_at.as_str(),
        item.created_at.as_str(),
    ]
}

/// Fold one item row's compared stamps into `best`: the greatest stamp by the
/// instant it names, with the greatest entity id carrying that stamp as the
/// compound tiebreak. The winner keeps the text it was read as.
pub(crate) fn fold_item_position(
    best: &mut Option<(String, String)>,
    entity_id: &str,
    stamps: &[&str],
) {
    for stamp in stamps {
        if stamp.is_empty() {
            continue;
        }
        match best
            .as_ref()
            .map(|(held, _)| writer_stamp_order(stamp, held))
        {
            None | Some(std::cmp::Ordering::Greater) => {
                *best = Some(((*stamp).to_owned(), entity_id.to_owned()));
            }
            Some(std::cmp::Ordering::Equal) => {
                let keep = best
                    .as_ref()
                    .is_none_or(|(_, held_entity)| held_entity.as_str() > entity_id);
                if !keep && let Some(slot) = best.as_mut() {
                    entity_id.clone_into(&mut slot.1);
                }
            }
            Some(std::cmp::Ordering::Less) => {}
        }
    }
}

/// One feed kind's packaging result for a push: the compound `(stamp, entity)`
/// the kind advances to on ack, in the store's own stamp text, and the
/// half-open `mutations` range this kind contributed.
pub(crate) struct KindBatch {
    pub(crate) kind: &'static str,
    pub(crate) last: Option<(String, String)>,
    pub(crate) skipped: bool,
    pub(crate) base: usize,
    pub(crate) end: usize,
}

impl KindBatch {
    /// True when the server rejected one of this kind's own mutations, so its
    /// watermark must hold: advancing would skip a row the peer never stored.
    pub(crate) fn has_rejection(&self, mutations: &[SyncMutation], rejected: &[String]) -> bool {
        mutations[self.base..self.end]
            .iter()
            .any(|m| rejected.contains(&m.sync_mutation_id))
    }
}

/// Record one packaged feed page's bookkeeping (last row position, skipped
/// rows) for the post-ack advance.
pub(crate) fn record_kind_batch(
    kind: &'static str,
    rows: &[SyncFeedRow],
    base: usize,
    mutations: &[SyncMutation],
    skipped: bool,
    batches: &mut Vec<KindBatch>,
) {
    batches.push(KindBatch {
        kind,
        last: rows
            .last()
            .map(|r| (r.transaction_time.clone(), r.entity_id.clone())),
        skipped,
        base,
        end: mutations.len(),
    });
}

/// The eight push feed kinds in their stable packaging order.
pub(crate) const PUSH_KINDS: [&str; 8] = [
    "item",
    "assoc_edge",
    "triple",
    "persona_stable",
    "persona_preference",
    "belief",
    "task",
    "failure",
];

/// Persist one kind's compound watermark body after a successful push ack.
pub(crate) fn kind_watermark_set(
    cfg: &SyncClientConfig,
    bank: &str,
    kind: &str,
    body: &str,
) -> Result<(), AmError> {
    cfg.sync
        .cursor_set(bank, &push_wm_label(kind), &encode_watermark(bank, body))
}

#[cfg(test)]
#[path = "client_feed_cursor_conversion_tests.rs"]
mod client_feed_cursor_conversion_tests;
