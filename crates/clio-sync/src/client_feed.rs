// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for packaging the client's outgoing change feed
//! into `sync_payload` batches: item, association-edge, and closed-triple
//! mutations plus the context-bearing domain records (persona stable entries,
//! persona preferences, beliefs, task and failure records), each sealed with
//! the node's optional sync cipher before push, and the durable per-kind push
//! watermark cursors that page that feed.
//!
//! ## Owns
//! - `push_bank` (local feed → wire mutations → remote push → cursor advance)
//! - The hold rule: a kind whose rows were skipped during packaging does not
//!   advance its cursor
//!
//! ## Does not own
//! - The per-kind watermark cursors themselves: their keys, their read and
//!   one-shot shared-cursor conversion, and their writes
//!   (`client_feed_cursor_conversion`)
//! - Per-row mutation packaging (`client_feed_package`)
//! - Pull/apply/cursor policy (see `client`)
//! - Wire envelope shapes (see `protocol`)
//! - Change-feed SQL (see `clio-store` `SyncStore`)
//!
//! ## Boundary
//!
//! Changes to how local rows become outbound mutations and how the per-kind
//! cursors advance should normally require changes here. Changes to pull or
//! status behavior should not.
//!
//! Keep this module focused on push-side change-feed packaging.
//!
//! # Per-kind push watermarks
//!
//! Each of the eight feed kinds (item, assoc edge, triple, persona stable,
//! persona preference, belief, task, failure) owns its own durable cursor
//! `push_wm:<kind>`, so one big feed cannot freeze the other kinds'
//! backlogs. A kind's own cursor is the only resume point for that kind; the
//! `push` cursor (the response cursor) is unchanged.
//!
//! The cursor token body is the canonical compound `stamp\u{1f}entity_id`,
//! the same tiebreak the packaged mutation uses, except task versions (their
//! tiebreak is the version record id). Each feed SQL resumes strictly after
//! `(stamp, entity_id)` in `ORDER BY stamp, entity_id` order, so a page-full
//! push advances safely and rows sharing a stamp are never skipped.
//!
//! A kind without its own cursor converts the retired shared `push_wm`
//! cursor once (see `kind_watermark`); a bank with neither cursor starts
//! from the canonical empty-watermark sentinel (`sync_store_rows` mirrors
//! this body format).
//!
//! A kind whose feed rows were skipped during packaging (carrier item missing
//! locally, missing assoc edge or triple) does not advance: it re-sends from
//! its current cursor and peer-side dedupe absorbs the re-sends until the
//! carrier arrives, without blocking the other kinds.

use crate::apply::{apply_freshness::unplaceable_stamp_value, record_refusal};
use crate::client::SyncClientConfig;
use crate::clock::now_utc;
use crate::http::post_json;
use crate::protocol::{
    BankSyncStatus, PushAck, REASON_MALFORMED_STAMP as MALFORMED, SyncMutation, SyncPayload,
    join_watermark_body,
};
use clio_store::DeadLetterStatus;
use clio_types::AmError;

#[path = "client_feed_package.rs"]
pub(crate) mod package;

#[path = "client_feed_cursor_conversion.rs"]
pub(crate) mod cursor_conversion;

pub(crate) use cursor_conversion::{
    KindBatch, PUSH_KINDS, fold_item_position, item_row_stamps, kind_watermark, kind_watermark_set,
    record_kind_batch,
};

use package::{
    assoc_mutation, belief_mutation, failure_mutation, item_mutation, persona_pref_mutation,
    persona_stable_mutation, task_mutation, triple_mutation,
};

/// The bank one push packages: the refusal record a packaging refusal writes
/// needs the config and the bank, not the row.
struct BankCtx<'a> {
    cfg: &'a SyncClientConfig,
    bank: &'a str,
}

impl BankCtx<'_> {
    /// Package one row into `mutations`, holding its own kind instead of failing
    /// the bank push when the row's writer stamp cannot be placed.
    ///
    /// Only that one refusal is contained; a storage or seal failure still fails
    /// the push. It is reported once through `record_refusal`, so the kind holds
    /// with an operator-visible record rather than the row disappearing.
    fn pack(
        &self,
        entity_id: &str,
        mutations: &mut Vec<SyncMutation>,
        skipped: &mut bool,
        result: Result<Option<SyncMutation>, AmError>,
    ) -> Result<(), AmError> {
        let stamp = match result {
            Ok(Some(mutation)) => {
                mutations.push(mutation);
                return Ok(());
            }
            Ok(None) => None,
            Err(err) => Some(
                unplaceable_stamp_value(&err)
                    .map(str::to_owned)
                    .ok_or(err)?,
            ),
        };
        *skipped = true;
        if let Some(stamp) = &stamp {
            let cfg = self.cfg;
            record_refusal(
                cfg.sync.as_ref(),
                self.bank,
                entity_id,
                stamp,
                &now_utc(),
                MALFORMED,
            )?;
        }
        Ok(())
    }
}

/// Push one bank's outgoing feed and advance each kind's watermark on ack.
///
/// # Errors
/// Transport, auth, or transient storage failures (cursors unchanged).
pub(crate) fn push_bank(
    cfg: &SyncClientConfig,
    remote: &str,
    bank: &str,
) -> Result<BankSyncStatus, AmError> {
    let page = crate::PULL_PAGE_LIMIT;
    let ctx = BankCtx { cfg, bank };
    let mut mutations = Vec::new();
    let mut batches = Vec::new();

    // Item feed: a missing local item is skipped, holding the item cursor
    // (never jumped over; peer-side dedupe absorbs the re-send). A page that is
    // not full advances past every close stamp it returned, not just the last
    // row's ordering stamp; otherwise a post-dated close re-sends forever.
    let since = kind_watermark(cfg, bank, "item")?;
    let rows = cfg.sync.feed_items(bank, &since, page)?;
    let base = mutations.len();
    let mut skipped = false;
    let mut reach: Option<(String, String)> = None;
    for row in &rows {
        let Some(item) = cfg.store.get_memory_item(bank, &row.entity_id)? else {
            skipped = true;
            continue;
        };
        fold_item_position(&mut reach, &row.entity_id, &item_row_stamps(row, &item));
        let packed = item_mutation(cfg, &item, row.op.as_str()).map(Some);
        ctx.pack(&row.entity_id, &mut mutations, &mut skipped, packed)?;
    }
    record_kind_batch("item", &rows, base, &mutations, skipped, &mut batches);
    // A page that is not full has seen every row the predicate selects, so the
    // kind advances past every stamp it compared, not just the last row's
    // ordering stamp: a post-dated close would otherwise re-select its row.
    if rows.len() < page
        && let Some(batch) = batches.last_mut()
    {
        batch.last = reach;
    }

    // Association-edge feed.
    let since = kind_watermark(cfg, bank, "assoc_edge")?;
    let rows = cfg.sync.feed_assoc(bank, &since, page)?;
    let base = mutations.len();
    let mut skipped = false;
    for row in &rows {
        let Some(edge) = cfg.store.get_assoc_edge(bank, &row.entity_id)? else {
            skipped = true;
            continue;
        };
        let packed = assoc_mutation(cfg, bank, &edge).map(Some);
        ctx.pack(&row.entity_id, &mut mutations, &mut skipped, packed)?;
    }
    record_kind_batch("assoc_edge", &rows, base, &mutations, skipped, &mut batches);

    // Closed-triple feed (open/missing triples are skipped, holding the kind).
    let since = kind_watermark(cfg, bank, "triple")?;
    let rows = cfg.sync.feed_triples(bank, &since, page)?;
    let base = mutations.len();
    let mut skipped = false;
    for row in &rows {
        let packed = triple_mutation(cfg, bank, &row.entity_id);
        ctx.pack(&row.entity_id, &mut mutations, &mut skipped, packed)?;
    }
    record_kind_batch("triple", &rows, base, &mutations, skipped, &mut batches);

    batches.extend(package_record_kinds(cfg, bank, &mut mutations)?);

    let payload = SyncPayload {
        device_id: cfg.device_id.clone(),
        bank: bank.to_owned(),
        mutations: mutations.clone(),
    };
    let response: PushAck = post_json(
        remote,
        "/sync/push",
        cfg.token.as_deref(),
        &serde_json::to_value(&payload).expect("serialize payload"),
    )?;
    if response.rejected.is_empty() {
        cfg.sync.cursor_set(bank, "push", &response.cursor)?;
    }
    // Per-kind advance: each kind moves to the compound position of its last
    // returned row, so a page-full push resumes strictly after it and no row
    // is skipped by a stamp tie. A kind that skipped any row holds.
    //
    // The stamp written here is always the store's own row text, never the
    // mutation's wire stamp. A store column may carry epoch millis while the
    // wire now carries canonical ISO-8601, and every feed predicate compares
    // that column to this cursor as text: an ISO cursor over millis rows
    // selects nothing, and the kind silently reports an empty backlog forever.
    //
    // The advance is deliberately NOT nested inside the `rejected.is_empty()`
    // guard above: the bank-level cursor must hold when anything was rejected,
    // but a rejection in one kind says nothing about the others, and gating
    // them all froze all eight watermarks on a single dead-lettered mutation,
    // which re-sent the whole backlog on every retry. A kind advances only
    // when none of its own mutations was rejected.
    for batch in &batches {
        if batch.skipped || batch.has_rejection(&mutations, &response.rejected) {
            continue;
        }
        let Some((stamp, entity)) = &batch.last else {
            continue;
        };
        kind_watermark_set(cfg, bank, batch.kind, &join_watermark_body(stamp, entity))?;
    }
    Ok(BankSyncStatus {
        bank_id: bank.to_owned(),
        push_cursor: Some(response.cursor),
        pull_cursor: cfg.sync.cursor_get(bank, "pull")?,
        pending_push: pending_push(cfg, bank)?,
        dead_letter_open: cfg.sync.dead_letter_count(bank, DeadLetterStatus::Open)?,
        conflicts: cfg.sync.conflict_count(bank)?,
        stale_refused: crate::apply::stale_refused(cfg.sync.as_ref(), bank)?,
        conflict_sample: Vec::new(),
    })
}

/// Package the five domain-record kinds (persona stable/preferences, beliefs,
/// tasks, failures) and report each kind's batch.
///
/// Task and failure records whose carrier item is missing locally are
/// skipped: the carrier travels on the item feed, and its structured row is
/// only sent once the item exists. A skipped row holds that kind's cursor.
fn package_record_kinds(
    cfg: &SyncClientConfig,
    bank: &str,
    mutations: &mut Vec<SyncMutation>,
) -> Result<Vec<KindBatch>, AmError> {
    let page = crate::PULL_PAGE_LIMIT;
    let ctx = BankCtx { cfg, bank };
    let mut batches = Vec::new();

    let since = kind_watermark(cfg, bank, "persona_stable")?;
    let rows = cfg.sync.feed_persona_stable(bank, &since, page)?;
    let base = mutations.len();
    let mut skipped = false;
    for (stamp, entry) in &rows {
        let packed = persona_stable_mutation(cfg, bank, stamp.as_str(), entry).map(Some);
        ctx.pack(&entry.key, mutations, &mut skipped, packed)?;
    }
    batches.push(KindBatch {
        kind: "persona_stable",
        last: rows.last().map(|(s, e)| (s.clone(), e.key.clone())),
        skipped,
        base,
        end: mutations.len(),
    });

    let since = kind_watermark(cfg, bank, "persona_preference")?;
    let rows = cfg.sync.feed_persona_preferences(bank, &since, page)?;
    let base = mutations.len();
    let mut skipped = false;
    for (stamp, pref) in &rows {
        let packed = persona_pref_mutation(cfg, bank, stamp.as_str(), pref).map(Some);
        ctx.pack(&pref.key, mutations, &mut skipped, packed)?;
    }
    batches.push(KindBatch {
        kind: "persona_preference",
        last: rows.last().map(|(s, p)| (s.clone(), p.key.clone())),
        skipped,
        base,
        end: mutations.len(),
    });

    let since = kind_watermark(cfg, bank, "belief")?;
    let rows = cfg.sync.feed_beliefs(bank, &since, page)?;
    let base = mutations.len();
    let mut skipped = false;
    for (stamp, belief) in &rows {
        let packed = belief_mutation(cfg, bank, stamp.as_str(), belief).map(Some);
        ctx.pack(&belief.id, mutations, &mut skipped, packed)?;
    }
    batches.push(KindBatch {
        kind: "belief",
        last: rows.last().map(|(s, b)| (s.clone(), b.id.clone())),
        skipped,
        base,
        end: mutations.len(),
    });

    let since = kind_watermark(cfg, bank, "task")?;
    let rows = cfg.sync.feed_tasks(bank, &since, page)?;
    let base = mutations.len();
    let mut skipped = false;
    for (stamp, task) in &rows {
        let Some(item) = cfg.store.get_memory_item(bank, &task.item_id)? else {
            skipped = true;
            continue;
        };
        let packed = task_mutation(cfg, bank, stamp.as_str(), task, &item).map(Some);
        ctx.pack(&task.id, mutations, &mut skipped, packed)?;
    }
    batches.push(KindBatch {
        // The tiebreak entity id is the version record id (`task.id`), not the
        // wire `task_id`: several versions share one task id, and the feed
        // orders versions by record id.
        kind: "task",
        last: rows.last().map(|(s, t)| (s.clone(), t.id.clone())),
        skipped,
        base,
        end: mutations.len(),
    });

    let since = kind_watermark(cfg, bank, "failure")?;
    let rows = cfg.sync.feed_failures(bank, &since, page)?;
    let base = mutations.len();
    let mut skipped = false;
    for (stamp, failure) in &rows {
        let Some(item) = cfg.store.get_memory_item(bank, &failure.item_id)? else {
            skipped = true;
            continue;
        };
        let packed = failure_mutation(cfg, bank, stamp.as_str(), failure, &item).map(Some);
        ctx.pack(&failure.id, mutations, &mut skipped, packed)?;
    }
    batches.push(KindBatch {
        kind: "failure",
        last: rows.last().map(|(s, f)| (s.clone(), f.id.clone())),
        skipped,
        base,
        end: mutations.len(),
    });
    Ok(batches)
}

/// Outbound mutation count for `bank`: each feed kind's rows past that kind's
/// own push watermark (item, assoc edge, triple, persona stable/preference,
/// belief, task, failure; the retired shared `push_wm` converts once).
/// `push_bank`, `pull_bank`, and `sync_status` report it, never a hard-coded
/// zero. Bounded by `PULL_PAGE_LIMIT` per kind and approximate: it never
/// subtracts rows a push would skip for a missing carrier.
pub(crate) fn pending_push(cfg: &SyncClientConfig, bank: &str) -> Result<i64, AmError> {
    let mut count = 0usize;
    for kind in PUSH_KINDS {
        let since = kind_watermark(cfg, bank, kind)?;
        count += feed_kind_count(cfg, bank, kind, &since)?;
    }
    Ok(i64::try_from(count).unwrap_or(i64::MAX))
}

/// One kind's next feed page size (bounded by `PULL_PAGE_LIMIT`), used for
/// the pending counts. The `since` body is the kind's compound watermark.
pub(crate) fn feed_kind_count(
    cfg: &SyncClientConfig,
    bank: &str,
    kind: &str,
    since: &str,
) -> Result<usize, AmError> {
    let page = crate::PULL_PAGE_LIMIT;
    Ok(match kind {
        "item" => cfg.sync.feed_items(bank, since, page)?.len(),
        "assoc_edge" => cfg.sync.feed_assoc(bank, since, page)?.len(),
        "triple" => cfg.sync.feed_triples(bank, since, page)?.len(),
        "persona_stable" => cfg.sync.feed_persona_stable(bank, since, page)?.len(),
        "persona_preference" => cfg.sync.feed_persona_preferences(bank, since, page)?.len(),
        "belief" => cfg.sync.feed_beliefs(bank, since, page)?.len(),
        "task" => cfg.sync.feed_tasks(bank, since, page)?.len(),
        "failure" => cfg.sync.feed_failures(bank, since, page)?.len(),
        _ => 0,
    })
}

#[cfg(test)]
#[path = "client_feed_documentation_tests.rs"]
mod client_feed_documentation_tests;
#[cfg(test)]
#[path = "client_feed_stamp_hold_tests.rs"]
mod client_feed_stamp_hold_tests;
#[cfg(test)]
#[path = "client_feed_tests.rs"]
mod client_feed_tests;
#[cfg(test)]
#[path = "client_feed_wm_page_tests.rs"]
mod client_feed_wm_page_tests;
#[cfg(test)]
#[path = "client_feed_wm_tests.rs"]
mod client_feed_wm_tests;
