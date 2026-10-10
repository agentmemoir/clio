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

use crate::client::SyncClientConfig;
use crate::protocol::{canonical_watermark_body, decode_watermark, encode_watermark};
use clio_types::AmError;

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
