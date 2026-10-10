// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync-persistence row models and their
//! shared column projections: the durable journal row, the LWW conflict row,
//! the dead-letter row and its status, the client change-feed row, and the
//! idempotent append outcome.
//!
//! ## Owns
//! - `SyncJournalRow`, `SyncConflictRow`, `SyncDeadLetterRow`, `DeadLetterStatus`
//! - `SyncFeedRow`, `JournalAppendOutcome`
//! - `JOURNAL_COLS`, `CONFLICT_COLS`, `DLQ_COLS` (shared by both backends)
//! - The compound push-watermark body split (`WM_ENTITY_SEP`,
//!   `split_push_watermark`)
//!
//! ## Does not own
//! - The `SyncStore` persistence contract (`sync_store`)
//! - Sync protocol semantics (apply matrix, LWW comparison, DLQ policy — `clio-sync`)
//!
//! ## Boundary
//!
//! Changes to the persisted sync row shape or its column projection should
//! normally require changes here. Changes to apply semantics should not
//! require changes here.
//!
//! Keep this module focused on the sync persistence row models.

pub(crate) const JOURNAL_COLS: &str = "seq, sync_mutation_id, bank_id, entity_kind, entity_id, op, \
                            transaction_time, device_id, content_hash, content_sealed, \
                            encrypted, admission_receipt, created_at";

pub(crate) const CONFLICT_COLS: &str = "bank_id, entity_kind, entity_id, winner_mutation_id, \
                            loser_mutation_id, winner_transaction_time, loser_transaction_time, \
                            winner_device_id, loser_device_id, created_at";

pub(crate) const DLQ_COLS: &str = "sync_mutation_id, bank_id, reason_code, detail, envelope_json, \
                            status, first_seen, last_error, updated_at";

/// One durable row of the per-bank sync journal.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncJournalRow {
    /// Commit-ordered position within the bank (unique per bank).
    pub seq: i64,
    /// Stable idempotency key for the mutation event.
    pub sync_mutation_id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Entity kind (`item` | `triple` | `assoc_edge` | ... per the wire schema).
    pub entity_kind: String,
    /// Stable business id of the mutated entity.
    pub entity_id: String,
    /// Operation (`upsert` | `invalidate` | `discard` | `archive` | `reinforce` | `prune`).
    pub op: String,
    /// Writer's transaction-time stamp (LWW key).
    pub transaction_time: String,
    /// Origin device id.
    pub device_id: String,
    /// Hash of the plaintext logical content (even when wire form is ciphertext).
    pub content_hash: String,
    /// Sealed content: `CiphertextEnvelope` JSON under local DEKs, or opaque
    /// client ciphertext JSON when `client_encrypted` is set.
    pub content_sealed: Option<String>,
    /// `true` when content is client-encrypted (server-blind).
    pub client_encrypted: bool,
    /// Optional origin admission receipt JSON.
    pub admission_receipt: Option<String>,
    /// Journal insert timestamp (ISO-8601 UTC text).
    pub created_at: String,
}

/// One recorded LWW conflict resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncConflictRow {
    /// Bank isolation key.
    pub bank_id: String,
    /// Entity kind of the contested entity.
    pub entity_kind: String,
    /// Entity id of the contested entity.
    pub entity_id: String,
    /// Winner `sync_mutation_id`.
    pub winner_mutation_id: String,
    /// Loser `sync_mutation_id` (retained, never deleted).
    pub loser_mutation_id: String,
    /// Winner transaction time.
    pub winner_transaction_time: String,
    /// Loser transaction time.
    pub loser_transaction_time: String,
    /// Winner device id.
    pub winner_device_id: String,
    /// Loser device id.
    pub loser_device_id: String,
    /// Resolution timestamp (ISO-8601 UTC text).
    pub created_at: String,
}

/// Dead-letter status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadLetterStatus {
    /// Open: blocks `pull_cursor` advance past this id.
    Open,
    /// Operator acknowledged a permanent skip; cursors may advance.
    AckedSkip,
}

impl DeadLetterStatus {
    /// Column encoding.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::AckedSkip => "acked_skip",
        }
    }

    /// Parse the column encoding.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "open" => Some(Self::Open),
            "acked_skip" => Some(Self::AckedSkip),
            _ => None,
        }
    }
}

/// One quarantined unapplicable mutation awaiting operator ack.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncDeadLetterRow {
    /// Stable idempotency key of the quarantined mutation.
    pub sync_mutation_id: String,
    /// Bank isolation key.
    pub bank_id: String,
    /// Stable reject reason (`malformed`, `unauthorized_bank`, `erased_subject`, ...).
    pub reason_code: String,
    /// Human-readable detail (PII-safe).
    pub detail: String,
    /// Original envelope JSON for operator review (may hold client ciphertext).
    pub envelope_json: String,
    /// Open vs acked-skip.
    pub status: DeadLetterStatus,
    /// First-seen timestamp (ISO-8601 UTC text).
    pub first_seen: String,
    /// Last apply error text, when retries were attempted.
    pub last_error: Option<String>,
    /// Last status-change timestamp.
    pub updated_at: Option<String>,
}

/// One change-feed entry (client-side outgoing mutation candidates).
#[derive(Debug, Clone, PartialEq)]
pub struct SyncFeedRow {
    /// Entity kind (`item` | `assoc_edge`).
    pub entity_kind: String,
    /// Stable business id.
    pub entity_id: String,
    /// Operation implied by the row state (`upsert` | `invalidate` | `discard` | `reinforce` | `prune`).
    pub op: String,
    /// Change watermark (transaction/last-update time).
    pub transaction_time: String,
}

/// Appended journal outcome (idempotent append).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalAppendOutcome {
    /// Sequence of the row (existing seq when the id was already journaled).
    pub seq: i64,
    /// `false` when the `sync_mutation_id` was already present (no-op append).
    pub inserted: bool,
}

/// Separator between the stamp and the entity-id parts of a canonical push
/// watermark body. The unit-separator control character cannot appear inside
/// a store stamp (ISO-8601 UTC text or epoch-millis digits), so the split is
/// unambiguous.
pub(crate) const WM_ENTITY_SEP: char = '\u{1f}';

/// Split a canonical push watermark body into `(stamp, entity_id)`.
///
/// The splitter accepts both spellings of the body. A body without the
/// separator carries no entity part — it is the empty-entity spelling, not
/// an error — and binds the same `(stamp, "")` predicate; it is the
/// degenerate spelling of the canonical pair used by the empty-watermark
/// sentinel and bare-stamp filters. The client's cursor conversion is the
/// only code that reads an older stored body: it decodes the body,
/// re-encodes it as `stamp<sep>entity_id` with the same decoded pair, and
/// persists it before any feed resumes.
pub(crate) fn split_push_watermark(since: &str) -> (String, String) {
    let (stamp, entity) = since.split_once(WM_ENTITY_SEP).unwrap_or((since, ""));
    (stamp.to_owned(), entity.to_owned())
}

#[cfg(test)]
#[path = "sync_store_rows_tests.rs"]
mod sync_store_rows_tests;
