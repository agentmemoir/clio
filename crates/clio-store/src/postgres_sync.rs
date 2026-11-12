// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the `SyncStore for PostgresStore` impl for the sync
//! protocol state: the per-bank mutation journal, opaque
//! cursor tokens, the LWW conflict journal, the dead-letter quarantine, and the
//! client-side change-feed watermark queries. The SQL bodies live in sibling
//! modules (`postgres_sync_state`, `postgres_sync_history`,
//! `postgres_sync_records`).
//!
//! ## Owns
//! - The `SyncStore for PostgresStore` impl block and its delegation to bodies
//!
//! ## Does not own
//! - Sync state SQL bodies (`postgres_sync_state`) and history bodies
//!   (`postgres_sync_history`)
//! - Change-feed SQL bodies (`postgres_sync_records`)
//! - Apply/conflict/DLQ policy (see `clio-sync`)
//! - SQLite sync SQL (`sqlite_sync`)
//!
//! ## Boundary
//!
//! Changes to Postgres sync SQL should normally require changes in the body
//! modules. Changes to sync protocol semantics should not require changes here.
//!
//! Keep this module focused on the Postgres `SyncStore` impl surface.

use crate::postgres::PostgresStore;
use crate::sync_store::{
    DeadLetterStatus, JournalAppendOutcome, SyncConflictRow, SyncDeadLetterRow, SyncFeedRow,
    SyncJournalRow, SyncStore,
};
use crate::{BeliefStore, HistoryStore, PersonaStore};
use clio_types::{
    AmError, BeliefObject, ConfidenceEntry, FailureRecord, MemoryItem, PersonaPreference,
    PersonaStableEntry, TaskRecord,
};

#[path = "postgres_sync_history.rs"]
mod history;
#[path = "postgres_sync_state.rs"]
mod state;

impl SyncStore for PostgresStore {
    fn journal_append(&self, row: &SyncJournalRow) -> Result<JournalAppendOutcome, AmError> {
        crate::postgres_sync_records::journal_append_pg(self, row)
    }

    fn journal_get(
        &self,
        bank_id: &str,
        sync_mutation_id: &str,
    ) -> Result<Option<SyncJournalRow>, AmError> {
        state::journal_get_pg(self, bank_id, sync_mutation_id)
    }

    fn journal_list_since(
        &self,
        bank_id: &str,
        since_seq: i64,
        limit: usize,
    ) -> Result<Vec<SyncJournalRow>, AmError> {
        state::journal_list_since_pg(self, bank_id, since_seq, limit)
    }

    fn journal_max_seq(&self, bank_id: &str) -> Result<i64, AmError> {
        state::journal_max_seq_pg(self, bank_id)
    }

    fn journal_apply_guard_hit(
        &self,
        bank_id: &str,
        entity_kind: &str,
        entity_id: &str,
        op: &str,
        content_hash: &str,
    ) -> Result<bool, AmError> {
        state::journal_apply_guard_hit_pg(self, bank_id, entity_kind, entity_id, op, content_hash)
    }

    fn cursor_get(&self, bank_id: &str, peer_label: &str) -> Result<Option<String>, AmError> {
        state::cursor_get_pg(self, bank_id, peer_label)
    }

    fn cursor_set(&self, bank_id: &str, peer_label: &str, token: &str) -> Result<(), AmError> {
        state::cursor_set_pg(self, bank_id, peer_label, token)
    }

    fn conflict_insert(&self, row: &SyncConflictRow) -> Result<(), AmError> {
        state::conflict_insert_pg(self, row)
    }

    fn conflict_list(&self, bank_id: &str, limit: usize) -> Result<Vec<SyncConflictRow>, AmError> {
        state::conflict_list_pg(self, bank_id, limit)
    }

    fn conflict_count(&self, bank_id: &str) -> Result<i64, AmError> {
        state::conflict_count_pg(self, bank_id)
    }

    fn dead_letter_open(&self, row: &SyncDeadLetterRow) -> Result<(), AmError> {
        state::dead_letter_open_pg(self, row)
    }

    fn dead_letter_status(
        &self,
        sync_mutation_id: &str,
    ) -> Result<Option<DeadLetterStatus>, AmError> {
        state::dead_letter_status_pg(self, sync_mutation_id)
    }

    fn dead_letter_ack_skip(&self, sync_mutation_id: &str, at: &str) -> Result<bool, AmError> {
        state::dead_letter_ack_skip_pg(self, sync_mutation_id, at)
    }

    fn dead_letter_list(
        &self,
        bank_id: &str,
        status: DeadLetterStatus,
        limit: usize,
    ) -> Result<Vec<SyncDeadLetterRow>, AmError> {
        state::dead_letter_list_pg(self, bank_id, status, limit)
    }

    fn dead_letter_count(&self, bank_id: &str, status: DeadLetterStatus) -> Result<i64, AmError> {
        state::dead_letter_count_pg(self, bank_id, status)
    }

    fn subject_is_shredded(&self, subject_id: &str) -> Result<bool, AmError> {
        state::subject_is_shredded_pg(self, subject_id)
    }

    fn persona_context_erased(&self, bank_id: &str) -> Result<bool, AmError> {
        state::persona_context_erased_pg(self, bank_id)
    }

    fn feed_items(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<SyncFeedRow>, AmError> {
        crate::postgres_sync_records::feed_items_pg(self, bank_id, since, limit)
    }

    fn feed_assoc(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<SyncFeedRow>, AmError> {
        crate::postgres_sync_records::feed_assoc_pg(self, bank_id, since, limit)
    }

    fn feed_triples(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<SyncFeedRow>, AmError> {
        crate::postgres_sync_records::feed_triples_pg(self, bank_id, since, limit)
    }

    fn feed_persona_stable(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<(String, PersonaStableEntry)>, AmError> {
        crate::postgres_sync_records::feed_persona_stable_pg(self, bank_id, since, limit)
    }

    fn feed_persona_preferences(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<(String, PersonaPreference)>, AmError> {
        crate::postgres_sync_records::feed_persona_prefs_pg(self, bank_id, since, limit)
    }

    fn feed_beliefs(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<(String, BeliefObject)>, AmError> {
        crate::postgres_sync_records::feed_beliefs_pg(self, bank_id, since, limit)
    }

    fn persona_stable_current(
        &self,
        bank_id: &str,
        attr_key: &str,
    ) -> Result<Option<PersonaStableEntry>, AmError> {
        Ok(self
            .stable_history(bank_id, attr_key)?
            .into_iter()
            .rev()
            .find(|entry| entry.valid_until.is_none()))
    }

    fn persona_stable_put(&self, entry: &PersonaStableEntry, actor: &str) -> Result<(), AmError> {
        self.put_stable_entry(entry, actor)
    }

    fn persona_preference_current(
        &self,
        bank_id: &str,
        attr_key: &str,
    ) -> Result<Option<PersonaPreference>, AmError> {
        self.get_preference(bank_id, attr_key)
    }

    fn persona_preference_put(&self, pref: &PersonaPreference, actor: &str) -> Result<(), AmError> {
        self.put_preference_state(pref, actor)
    }

    fn belief_get(&self, bank_id: &str, belief_id: &str) -> Result<Option<BeliefObject>, AmError> {
        self.get_belief(bank_id, belief_id)
    }

    fn belief_by_proposition(
        &self,
        bank_id: &str,
        proposition_key: &str,
    ) -> Result<Option<BeliefObject>, AmError> {
        self.find_belief_by_proposition(bank_id, proposition_key)
    }

    fn belief_create(&self, belief: &BeliefObject, actor: &str) -> Result<(), AmError> {
        self.create_belief(belief, actor)
    }

    fn belief_append(
        &self,
        bank_id: &str,
        belief_id: &str,
        entry: &ConfidenceEntry,
        actor: &str,
        context: Option<&str>,
    ) -> Result<(), AmError> {
        self.append_confidence_with_context(bank_id, belief_id, entry, actor, context)
    }

    fn feed_tasks(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<(String, TaskRecord)>, AmError> {
        history::feed_tasks_pg(self, bank_id, since, limit)
    }

    fn feed_failures(
        &self,
        bank_id: &str,
        since: &str,
        limit: usize,
    ) -> Result<Vec<(String, FailureRecord)>, AmError> {
        history::feed_failures_pg(self, bank_id, since, limit)
    }

    fn task_current(&self, bank_id: &str, task_id: &str) -> Result<Option<TaskRecord>, AmError> {
        HistoryStore::get_task(self, bank_id, task_id)
    }

    fn task_put(
        &self,
        item: &MemoryItem,
        draft: &TaskRecord,
        actor: &str,
    ) -> Result<TaskRecord, AmError> {
        HistoryStore::commit_task_upsert(self, item, draft, actor)
    }

    fn failure_by_attempt(
        &self,
        bank_id: &str,
        task_id: &str,
        attempt_n: i64,
    ) -> Result<Option<FailureRecord>, AmError> {
        history::failure_by_attempt_pg(self, bank_id, task_id, attempt_n)
    }

    fn failure_put(
        &self,
        item: &MemoryItem,
        draft: &FailureRecord,
        actor: &str,
    ) -> Result<FailureRecord, AmError> {
        HistoryStore::commit_failure_record(self, item, draft, actor)
    }

    fn belief_correct_context(
        &self,
        bank_id: &str,
        belief_id: &str,
        new_context: &str,
        reason: &str,
        actor: &str,
    ) -> Result<(), AmError> {
        self.correct_belief_context(bank_id, belief_id, new_context, reason, actor)
    }
}

#[cfg(test)]
#[path = "postgres_sync_pg_sync_feed_wm_tests.rs"]
mod postgres_sync_pg_sync_feed_wm_tests;
#[cfg(test)]
#[path = "postgres_sync_tests.rs"]
mod postgres_sync_tests;
