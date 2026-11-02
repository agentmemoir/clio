// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the transaction-scoped serialization and the
//! duplicate lookup the Postgres journal append runs inside its transaction.
//!
//! ## Owns
//! - The per-bank journal append advisory-lock label and the lock helper
//! - The stored-`seq` lookup for an already journalled mutation
//!
//! ## Does not own
//! - The journal append transaction body or its seq allocation
//!   (`postgres_sync_records`)
//! - The session-level shared-Postgres test lock (`pg_lock`)
//!
//! ## Boundary
//!
//! Changes to what serializes a journal append should normally require changes
//! here. Changes to the append's seq allocation should not require changes here.
//!
//! Keep this module focused on journal append locking and duplicate lookup.

use crate::migrate::IntoAmError;
use clio_types::AmError;
use postgres::Transaction;

/// Advisory-lock label for the per-bank journal append (`"journal-append"`).
///
/// A transaction-scoped advisory lock is keyed by this label plus the bank id,
/// so appends to different banks normally do not wait on each other. The key is
/// a 32-bit `hashtext` of the label and the bank id, so two different pairs can
/// hash to one key and serialize, and this label's keys share one advisory-lock
/// namespace with every other label-derived key in the crate: a collision costs
/// contention, never correctness. The label is the only place this key is
/// written; a second literal would create a second, unrelated lock.
pub(crate) const JOURNAL_APPEND_LOCK: &str = "journal-append";

/// Bound on how long an append waits for another session's bank key (`"5s"`).
///
/// Without a bound the wait is unbounded: PostgreSQL's own default is `0`, and
/// the append holds the store's single-client mutex while it waits, so a stalled
/// holder would park every other operation of the process. Exceeding the bound
/// raises SQLSTATE `55P03`, which the store's error mapper reports as the
/// retryable `StorageBusy` code, so the caller retries instead of hanging.
pub(crate) const JOURNAL_APPEND_LOCK_TIMEOUT: &str = "5s";

/// Serialize journal append for one bank inside the append's transaction.
///
/// The append allocates `seq` by reading the bank's current maximum, so two
/// transactions that compute it concurrently would collide on the
/// `(bank_id, seq)` unique index. The lock is taken before the duplicate probe
/// that opens the same transaction, which makes both the probe and the
/// allocation observe the latest committed journal.
///
/// The wait is bounded by [`JOURNAL_APPEND_LOCK_TIMEOUT`], set transaction-local
/// so it dies with this transaction and never leaks onto the next statement of
/// the store's long-lived connection. The bound is what turns a stalled holder of
/// the bank key into a retryable error for this caller instead of a hang for
/// every operation queued behind the same client.
pub(crate) fn lock_journal_append(tx: &mut Transaction<'_>, bank_id: &str) -> Result<(), AmError> {
    tx.execute(
        "SELECT set_config('lock_timeout', $1, true)",
        &[&JOURNAL_APPEND_LOCK_TIMEOUT],
    )
    .am("sync journal lock timeout")?;
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtext($1 || ':' || $2))",
        &[&JOURNAL_APPEND_LOCK, &bank_id],
    )
    .am("sync journal advisory lock")?;
    Ok(())
}

/// Stored `seq` of the journal row for `(bank, mutation)`, when one exists.
pub(crate) fn journal_seq_probe(
    tx: &mut Transaction<'_>,
    bank_id: &str,
    sync_mutation_id: &str,
) -> Result<Option<i32>, AmError> {
    let found = tx
        .query_opt(
            "SELECT seq FROM sync_mutations WHERE bank_id = $1 AND sync_mutation_id = $2",
            &[&bank_id, &sync_mutation_id],
        )
        .am("sync journal dedupe probe")?;
    Ok(found.map(|row| row.get(0)))
}

#[cfg(test)]
#[path = "postgres_sync_journal_locks_tests.rs"]
mod postgres_sync_journal_locks_tests;
