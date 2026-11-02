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

/// Advisory-lock label for the per-bank journal append (`"journal-append:"`).
///
/// A transaction-scoped advisory lock is keyed by this label plus the bank id,
/// so appends to different banks never wait on each other. The label is the
/// only place this key is written; a second literal would create a second,
/// unrelated lock.
pub(crate) const JOURNAL_APPEND_LOCK: &str = "journal-append";

/// Serialize journal append for one bank inside the append's transaction.
///
/// The append allocates `seq` by reading the bank's current maximum, so two
/// transactions that compute it concurrently would collide on the
/// `(bank_id, seq)` unique index. The lock is taken before the duplicate probe
/// that opens the same transaction, which makes both the probe and the
/// allocation observe the latest committed journal.
pub(crate) fn lock_journal_append(tx: &mut Transaction<'_>, bank_id: &str) -> Result<(), AmError> {
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
