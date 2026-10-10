// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync-protocol persistence contract
//! The durable per-bank mutation journal, opaque cursor
//! tokens, the LWW conflict journal, and the sync dead-letter quarantine.
//!
//! ## Owns
//! - `SyncStore` repository contract shared by both backends
//! - Re-exports of the journal / conflict / dead-letter / change-feed row
//!   models and their shared column lists (defined in `sync_store_rows`)
//!
//! ## Does not own
//! - Sync protocol semantics (apply matrix, LWW comparison, DLQ policy — `clio-sync`)
//! - Wire envelope encoding (see `clio-sync` protocol module)
//! - Admission scoring or gating (see `clio-admission`)
//!
//! ## Boundary
//!
//! Changes to sync persistence shape or cursor storage should normally
//! require changes here. Changes to apply/conflict semantics should not
//! require changes here.
//!
//! Keep this module focused on the sync persistence contract.
//!
//! Journal/conflict/dead-letter column lists are shared by both backends.
//!

pub(crate) use crate::sync_store_rows::{CONFLICT_COLS, DLQ_COLS, JOURNAL_COLS};
pub use crate::sync_store_rows::{
    DeadLetterStatus, JournalAppendOutcome, SyncConflictRow, SyncDeadLetterRow, SyncFeedRow,
    SyncJournalRow,
};

use clio_types::{
    AmError, BeliefObject, ConfidenceEntry, FailureRecord, MemoryItem, PersonaPreference,
    PersonaStableEntry, TaskRecord,
};

/// Sync persistence contract: journal, cursors, conflicts, dead letters, feed.
pub trait SyncStore: Send + Sync {
    /// Append one mutation to the bank journal inside a single transaction
    /// (seq = max(seq) + 1). Idempotent on `sync_mutation_id`: re-append is a
    /// no-op returning the existing seq.
    ///
    /// # Errors
    /// Storage failures.
    fn journal_append(&self, row: &SyncJournalRow) -> Result<JournalAppendOutcome, AmError>;

    /// Read one journaled mutation by id, bank-scoped.
    ///
    /// # Errors
    /// Storage failures.
    fn journal_get(
        &self,
        bank_id: &str,
        sync_mutation_id: &str,
    ) -> Result<Option<SyncJournalRow>, AmError>;

    /// List journal rows with `seq > since`, ordered by seq, capped at `limit`.
    ///
    /// # Errors
    /// Storage failures.
    fn journal_list_since(
        &self,
        bank_id: &str,
        since_seq: i64,
        limit: usize,
    ) -> Result<Vec<SyncJournalRow>, AmError>;

    /// Highest journal seq in the bank (0 when empty).
    ///
    /// # Errors
    /// Storage failures.
    fn journal_max_seq(&self, bank_id: &str) -> Result<i64, AmError>;

    /// Secondary idempotence guard: same `(entity_kind, entity_id, op, content_hash)`
    /// already applied in the bank.
    ///
    /// # Errors
    /// Storage failures.
    fn journal_apply_guard_hit(
        &self,
        bank_id: &str,
        entity_kind: &str,
        entity_id: &str,
        op: &str,
        content_hash: &str,
    ) -> Result<bool, AmError>;

    /// Read the stored opaque cursor token for `(bank, peer_label)`.
    ///
    /// # Errors
    /// Storage failures.
    fn cursor_get(&self, bank_id: &str, peer_label: &str) -> Result<Option<String>, AmError>;

    /// Durably store the opaque cursor token for `(bank, peer_label)`.
    ///
    /// # Errors
    /// Storage failures.
    fn cursor_set(&self, bank_id: &str, peer_label: &str, token: &str) -> Result<(), AmError>;

    /// Record one LWW conflict resolution (loser retained for history).
    ///
    /// # Errors
    /// Storage failures.
    fn conflict_insert(&self, row: &SyncConflictRow) -> Result<(), AmError>;

    /// List recent conflicts for the bank, newest first, capped at `limit`.
    ///
    /// # Errors
    /// Storage failures.
    fn conflict_list(&self, bank_id: &str, limit: usize) -> Result<Vec<SyncConflictRow>, AmError>;

    /// Total conflict count for the bank.
    ///
    /// # Errors
    /// Storage failures.
    fn conflict_count(&self, bank_id: &str) -> Result<i64, AmError>;

    /// Upsert a dead-letter row (open). Keeps `first_seen` on re-open.
    ///
    /// # Errors
    /// Storage failures.
    fn dead_letter_open(&self, row: &SyncDeadLetterRow) -> Result<(), AmError>;

    /// Read the current dead-letter status of one mutation id.
    ///
    /// # Errors
    /// Storage failures.
    fn dead_letter_status(
        &self,
        sync_mutation_id: &str,
    ) -> Result<Option<DeadLetterStatus>, AmError>;

    /// Record an operator ack-skip; returns `false` when the id is unknown.
    ///
    /// # Errors
    /// Storage failures.
    fn dead_letter_ack_skip(&self, sync_mutation_id: &str, at: &str) -> Result<bool, AmError>;

    /// List dead-letter rows for the bank with the given status.
    ///
    /// # Errors
    /// Storage failures.
    fn dead_letter_list(
        &self,
        bank_id: &str,
        status: DeadLetterStatus,
        limit: usize,
    ) -> Result<Vec<SyncDeadLetterRow>, AmError>;

    /// Count dead-letter rows for the bank with the given status.
    ///
    /// # Errors
    /// Storage failures.
    fn dead_letter_count(&self, bank_id: &str, status: DeadLetterStatus) -> Result<i64, AmError>;

    /// Fail-closed probe: `true` when the subject's DEK was shredded.
    /// Sync apply MUST reject items for shredded subjects (no resurrect).
    ///
    /// # Errors
    /// Storage failures.
    fn subject_is_shredded(&self, subject_id: &str) -> Result<bool, AmError>;

    /// Fail-closed probe: `true` when a verified compliance erasure purged
    /// this bank's persona context (`persona_erase_tombstones`). Persona rows
    /// carry no subject attribution, so the bank-level tombstone is
    /// the erasure probe for persona content: a persona mutation that would
    /// recreate purged context must fail closed against it.
    ///
    /// # Errors
    /// Storage failures.
    fn persona_context_erased(&self, bank_id: &str) -> Result<bool, AmError>;

    /// Change feed: items in the bank whose bi-temporal or update stamps moved
    /// past the push watermark (ISO-8601 UTC text), capped at `limit`.
    ///
    /// `since` is the watermark body: the canonical `stamp\u{1f}entity_id`
    /// pair, or its empty-entity spelling (`stamp` or `stamp\u{1f}`). Rows
    /// resume strictly after that `(stamp, entity_id)` position (`ORDER BY
    /// stamp, entity_id`); an empty-entity body resumes inclusive of the
    /// stamp tie. The client converts older stored bodies before any feed
    /// resumes. The epoch-millis sentinel guard is unchanged.
    ///
    /// Returns identity + operation class only; no content.
    ///
    /// # Errors
    /// Storage failures.
    fn feed_items(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<SyncFeedRow>, AmError>;

    /// Change feed: association edges in the bank updated past `since`
    /// (compound `(last_updated_at, entity_id)` resume; see `feed_items`).
    ///
    /// # Errors
    /// Storage failures.
    fn feed_assoc(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<SyncFeedRow>, AmError>;

    /// Change feed: triples in the bank closed (either time axis) past `since`
    /// (compound `(close stamp, entity_id)` resume). Each row's
    /// `transaction_time` is the row's close stamp
    /// (`COALESCE(tx_until, valid_until)`), the tiebreak key. Full row content
    /// is re-read via `Store::get_triple` when packaging.
    ///
    /// # Errors
    /// Storage failures.
    fn feed_triples(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<SyncFeedRow>, AmError>;

    /// Change feed: persona stable-entry versions changed past `since` as
    /// `(change stamp, opened plaintext entry)` with compound
    /// `(change stamp, attr_key)` resume. Only the latest version per
    /// `(bank, attr_key)` is emitted; the open-row write feeds the closure of
    /// the prior version implicitly (same key, append semantics).
    ///
    /// # Errors
    /// Storage or decrypt failures.
    fn feed_persona_stable(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<(String, PersonaStableEntry)>, AmError>;

    /// Change feed: persona preference rows changed past `since` as
    /// `(change stamp, opened row)` with compound
    /// `(change stamp, attr_key)` resume; values stay numeric, context opened.
    ///
    /// # Errors
    /// Storage or decrypt failures.
    fn feed_persona_preferences(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<(String, PersonaPreference)>, AmError>;

    /// Change feed: belief objects changed past `since` as
    /// `(change stamp, opened belief with full trajectory)` with compound
    /// `(change stamp, belief id)` resume.
    ///
    /// # Errors
    /// Storage or decrypt failures.
    fn feed_beliefs(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<(String, BeliefObject)>, AmError>;

    /// Change feed: task-record versions changed past `since` as
    /// `(change stamp, opened record)` with compound
    /// `(change stamp, record id)` resume — the record id, not the wire
    /// `task_id`, because several versions share one task id.
    fn feed_tasks(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<(String, TaskRecord)>, AmError>;

    /// Change feed: failure records changed past `since` as
    /// `(change stamp, opened record)` with compound
    /// `(change stamp, failure id)` resume.
    fn feed_failures(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<(String, FailureRecord)>, AmError>;

    /// Task apply read: the current (highest-version) structured row for `(bank, task_id)`.
    fn task_current(&self, bank_id: &str, task_id: &str) -> Result<Option<TaskRecord>, AmError>;

    /// Task apply write: carrier item + structured row via the atomic `HistoryStore::commit_task_upsert`.
    fn task_put(
        &self,
        item: &MemoryItem,
        draft: &TaskRecord,
        actor: &str,
    ) -> Result<TaskRecord, AmError>;

    /// Failure apply read: the stored failure at `(bank, task_id, attempt_n)`.
    fn failure_by_attempt(
        &self,
        bank_id: &str,
        task_id: &str,
        attempt_n: i64,
    ) -> Result<Option<FailureRecord>, AmError>;

    /// Failure apply write: carrier item + row via the atomic `HistoryStore::commit_failure_record`.
    fn failure_put(
        &self,
        item: &MemoryItem,
        draft: &FailureRecord,
        actor: &str,
    ) -> Result<FailureRecord, AmError>;

    /// Persona apply read: the current open stable row for `(bank, attr_key)`.
    fn persona_stable_current(
        &self,
        bank_id: &str,
        attr_key: &str,
    ) -> Result<Option<PersonaStableEntry>, AmError>;

    /// Persona apply write: insert one open stable version; closes the prior
    /// open version of the same key and seals value/context under the bank DEK.
    fn persona_stable_put(&self, entry: &PersonaStableEntry, actor: &str) -> Result<(), AmError>;

    /// Persona apply read: the stored preference row for `(bank, attr_key)`.
    fn persona_preference_current(
        &self,
        bank_id: &str,
        attr_key: &str,
    ) -> Result<Option<PersonaPreference>, AmError>;

    /// Persona apply write: upsert one preference row (`None` context preserves
    /// the stored context; create-time admission score retained on conflict).
    fn persona_preference_put(&self, pref: &PersonaPreference, actor: &str) -> Result<(), AmError>;

    /// Belief apply read: one belief by id, opened with full trajectory.
    fn belief_get(&self, bank_id: &str, belief_id: &str) -> Result<Option<BeliefObject>, AmError>;

    /// Belief apply read: resolve one belief by normalized proposition key.
    fn belief_by_proposition(
        &self,
        bank_id: &str,
        proposition_key: &str,
    ) -> Result<Option<BeliefObject>, AmError>;

    /// Belief apply write: create a belief with its initial trajectory
    /// (proposition/context sealed under the subject DEK; never re-scored).
    fn belief_create(&self, belief: &BeliefObject, actor: &str) -> Result<(), AmError>;

    /// Belief apply write: append one confidence entry (insert only);
    /// `Some(context)` sets the context (apply uses this only when none is
    /// stored — a differing stored context fails closed in the apply layer).
    fn belief_append(
        &self,
        bank_id: &str,
        belief_id: &str,
        entry: &ConfidenceEntry,
        actor: &str,
        context: Option<&str>,
    ) -> Result<(), AmError>;

    /// Belief apply: replace a stored belief's context through the authorized,
    /// audited correction path. An append may never change a
    /// stored context, so a replicated mutation carrying a differing context
    /// lands here instead of being dropped.
    fn belief_correct_context(
        &self,
        bank_id: &str,
        belief_id: &str,
        new_context: &str,
        reason: &str,
        actor: &str,
    ) -> Result<(), AmError>;
}

#[cfg(test)]
#[path = "sync_store_model_tests.rs"]
mod sync_store_model_tests;

#[cfg(test)]
#[path = "sync_store_tests.rs"]
mod sync_store_tests;
