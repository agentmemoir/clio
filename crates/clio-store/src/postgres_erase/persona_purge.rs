// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres erasure transaction's
//! persona-context purge SQL (erasure lifecycle).
//!
//! ## Owns
//! - Postgres persona-context purge and presence check for `erase_subject`
//! - The Postgres purge of the persona context's sync-derived copies (journal
//!   payload, dead-letter envelope)
//! - The Postgres `persona-erase` advisory lock and the bank-level
//!   `persona_erase_tombstones` write that fail-closes later persona writes
//!   into an erased bank
//!
//! ## Does not own
//! - The erasure transaction orchestration and tombstone writes (`postgres_erase`)
//! - SQLite parity (see `sqlite_erase_purge`)
//! - Persona write/read SQL (`postgres_persona`)
//!
//! ## Boundary
//!
//! Changes to which persona rows the erasure purges should normally require
//! changes here and in `compliance_store`. Persona repository changes should
//! not require changes here.
//!
//! Keep this module focused on the Postgres persona-context erasure SQL.

use crate::migrate::IntoAmError;
use clio_types::AmError;

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
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
) -> Result<bool, AmError> {
    let stable: i64 = tx
        .query_one(
            "SELECT COUNT(*)::bigint FROM persona_stable_entries
             WHERE bank_id = $1 AND context_cipher IS NOT NULL",
            &[&bank_id],
        )
        .am("erase persona stable count")?
        .get(0);
    let preferences: i64 = tx
        .query_one(
            "SELECT COUNT(*)::bigint FROM persona_preferences
             WHERE bank_id = $1 AND context_cipher IS NOT NULL",
            &[&bank_id],
        )
        .am("erase persona preference count")?
        .get(0);
    let journal: i64 = tx
        .query_one(
            "SELECT COUNT(*)::bigint FROM sync_mutations
             WHERE bank_id = $1 AND entity_kind = 'persona'
               AND (content_sealed IS NOT NULL OR encrypted <> 0)",
            &[&bank_id],
        )
        .am("erase persona journal count")?
        .get(0);
    if stable > 0 || preferences > 0 || journal > 0 {
        return Ok(true);
    }
    dead_letter_has_persona(tx, bank_id)
}

/// Whether any of the bank's quarantined dead-letter envelopes carries a
/// persona mutation. The table has no `entity_kind` column, so the match is
/// the same Rust-side envelope parse the purge uses.
fn dead_letter_has_persona(
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
) -> Result<bool, AmError> {
    Ok(tx
        .query(
            "SELECT envelope_json FROM sync_dead_letter WHERE bank_id = $1",
            &[&bank_id],
        )
        .am("erase persona dead letter count")?
        .iter()
        .any(|r| envelope_entity_kind(&r.get::<_, String>(0)).as_deref() == Some("persona")))
}

/// Serialize one bank's persona erasure against persona write transactions:
/// the erase and every persona write for the bank take this transaction-scoped
/// advisory lock first (keyed `persona-erase:<bank>`), so an in-flight persona
/// write commits before the erase purges and tombstones the bank, and a write
/// that starts after the erase observes the tombstone and refuses.
pub(crate) fn lock_persona_erase(
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
) -> Result<(), AmError> {
    tx.execute(
        "SELECT pg_advisory_xact_lock(hashtext('persona-erase:' || $1))",
        &[&bank_id],
    )
    .am("persona erase advisory lock")?;
    Ok(())
}

/// Record the bank-level persona erasure tombstone. Persona rows carry no
/// subject attribution, so the purge is bank-scoped; this tombstone is what
/// fail-closes later persona writes into the erased bank (the persona write
/// transactions probe it under the same advisory lock and refuse with
/// `ErasedSubject`).
pub(crate) fn write_persona_erase_tombstone(
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
    erased_at: &str,
) -> Result<(), AmError> {
    tx.execute(
        "INSERT INTO persona_erase_tombstones (bank_id, erased_at)
         VALUES ($1, $2)
         ON CONFLICT (bank_id) DO UPDATE SET erased_at = EXCLUDED.erased_at",
        &[&bank_id, &erased_at],
    )
    .am("persona erase tombstone write")?;
    Ok(())
}

/// Content-free replacement written over an erased persona-bearing dead-letter
/// envelope: the quarantine row and its ack bookkeeping survive, the payload
/// does not.
const ERASED_SYNC_ENVELOPE: &str = r#"{"erased":true}"#;

/// The `entity_kind` recorded inside a dead-letter envelope's mutation JSON.
fn envelope_entity_kind(envelope_json: &str) -> Option<String> {
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
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
) -> Result<usize, AmError> {
    let purged = tx
        .execute(
            "UPDATE sync_mutations SET content_sealed = NULL, encrypted = 0
             WHERE bank_id = $1 AND entity_kind = 'persona'
               AND (content_sealed IS NOT NULL OR encrypted <> 0)",
            &[&bank_id],
        )
        .am("erase sync journal persona")?;
    let mut purged: usize = purged.try_into().unwrap_or(0usize);
    // The dead-letter table stores the whole mutation envelope and has no
    // `entity_kind` column, so match the persona rows in Rust.
    let ids: Vec<String> = tx
        .query(
            "SELECT sync_mutation_id, envelope_json FROM sync_dead_letter
             WHERE bank_id = $1",
            &[&bank_id],
        )
        .am("erase sync dead letter query")?
        .iter()
        .filter(|r| envelope_entity_kind(&r.get::<_, String>(1)).as_deref() == Some("persona"))
        .map(|r| r.get::<_, String>(0))
        .collect();
    for id in ids {
        purged += tx
            .execute(
                "UPDATE sync_dead_letter SET envelope_json = $2
                 WHERE sync_mutation_id = $1",
                &[&id, &ERASED_SYNC_ENVELOPE],
            )
            .am("erase sync dead letter blank")?
            .try_into()
            .unwrap_or(0usize);
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
    tx: &mut postgres::Transaction<'_>,
    bank_id: &str,
) -> Result<usize, AmError> {
    let mut purged = tx
        .execute(
            "UPDATE persona_stable_entries SET context_cipher = NULL
             WHERE bank_id = $1 AND context_cipher IS NOT NULL",
            &[&bank_id],
        )
        .am("erase persona stable context")?
        .try_into()
        .unwrap_or(0usize);
    purged += tx
        .execute(
            "UPDATE persona_preferences SET context_cipher = NULL
             WHERE bank_id = $1 AND context_cipher IS NOT NULL",
            &[&bank_id],
        )
        .am("erase persona preference purge")?
        .try_into()
        .unwrap_or(0usize);
    purged += purge_sync_persona_copies(tx, bank_id)?;
    Ok(purged)
}
