// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the sync apply effects of the
//! context-bearing domain records (persona stable entries and
//! preferences, beliefs): payload decoding, bank/erasure checks, and
//! the idempotent durable writes behind the `persona` / `belief` apply matrix
//!
//! ## Owns
//! - Persona / belief wire payload decoding (plaintext record JSON)
//! - The persona apply effect (version ordering, LWW ties, equal-state journal
//!   self-heal, bank-mismatch fail-closed, the bank-level persona erasure probe)
//! - The belief apply effect (create vs append-only merge, context rule,
//!   merge-decides-first correction ordering)
//!
//! ## Journal self-heal
//!
//! `apply` absorbs genuine redeliveries with the pre-checked journal guards
//! (`journal_get` and the content-hash guard). Reaching an equal-state branch
//! here therefore means this mutation id was never journaled, so every
//! equal-state branch returns `Applied`: the durable record already exists
//! and the engine journals it, keeping the mutation relayable via pull.
//!
//! ## Does not own
//! - Item / assoc / triple apply effects (see `apply`)
//! - Record persistence SQL (`clio-store` `SyncStore` record primitives)
//! - Push-side packaging of the same records (see `client_feed`)
//! - The `persona_erase_tombstones` writer (compliance erasure, `clio-store`)
//!
//! ## Boundary
//!
//! Changes to persona/belief apply semantics should normally require changes
//! here. Changes to transport or cursor behavior should not.
//!
//! Keep this module focused on the domain-record apply effects.

use crate::apply::{ApplyEngine, ApplyOutcome, dead_letter, lww_loser};
use crate::apply_history::{journal_conflict, reject};
use crate::protocol::{SyncContent, SyncMutation};
use clio_types::{
    AmError, BeliefObject, ConfidenceEntry, ErrorCode, PersonaPreference, PersonaStableEntry,
    proposition_key,
};

/// Persona payload discriminator: `stable` = discrete attribute version,
/// `preference` = continuous EMA row.
const RECORD_STABLE: &str = "stable";
/// Persona payload discriminator for preference rows.
const RECORD_PREFERENCE: &str = "preference";

/// Deterministic tie resolution for domain records whose ordering key is the
/// record's own version stamp (`as_of`): at an equal `transaction_time`,
/// admission score then device id decide.
pub(crate) fn lww_tie_loser(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    existing_score: f64,
) -> Result<bool, AmError> {
    lww_loser(engine, m, &m.transaction_time, Some(existing_score))
}

/// Apply one `persona` upsert: a stable-entry version or a preference row.
///
/// Bank scope comes from the mutation envelope; payloads normally carry no
/// `bank_id` (serde skips it), and one that is present anyway must match the
/// envelope bank or the mutation fail-closes to the dead-letter quarantine.
/// An absent (or null) `context` field preserves the stored value on the
/// current row (preserve-on-absent, the same rule as item upserts).
///
/// # Errors
/// Storage failures propagate (retryable); schema and policy rejects are
/// dead-lettered and reported as `DeadLettered`.
pub(crate) fn persona_apply(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<ApplyOutcome, AmError> {
    let SyncContent::Plain(v) = &m.content else {
        return Ok(ApplyOutcome::Applied);
    };
    let entry_v = v.get("entry").cloned().unwrap_or(serde_json::Value::Null);
    match v.get("record").and_then(serde_json::Value::as_str) {
        Some(RECORD_STABLE) => persona_stable_apply(engine, m, envelope_json, entry_v),
        Some(RECORD_PREFERENCE) => persona_pref_apply(engine, m, envelope_json, entry_v),
        _ => dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::InvalidArgument,
                "persona payload requires record = stable|preference",
            ),
        ),
    }
}

fn persona_stable_apply(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
    entry_v: serde_json::Value,
) -> Result<ApplyOutcome, AmError> {
    let has_context = has_context_field(&entry_v);
    if let Some(outcome) = persona_bank_mismatch(engine, m, envelope_json, &entry_v)? {
        return Ok(outcome);
    }
    let mut entry: PersonaStableEntry = match serde_json::from_value(entry_v) {
        Ok(entry) => entry,
        Err(e) => return reject(engine, m, envelope_json, "bad persona stable entry", &e),
    };
    entry.bank_id.clone_from(&m.bank);
    if m.entity_id != entry.key {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::InvalidArgument,
                "persona entity_id must be the attr key",
            ),
        );
    }
    if engine.sync.persona_context_erased(&m.bank)? {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::ErasedSubject,
                "bank persona context purged by a verified erasure (persona erase tombstone)",
            ),
        );
    }
    if let Err(err) = entry.validate() {
        return dead_letter(engine, m, envelope_json, &err);
    }
    let current = engine.sync.persona_stable_current(&m.bank, &entry.key)?;
    if let Some(current) = &current {
        // Absent wire context preserves the stored version context.
        if !has_context {
            entry.context.clone_from(&current.context);
        }
        if current.as_of > entry.as_of {
            journal_conflict(engine, m, &current.as_of)?;
            return Ok(ApplyOutcome::ConflictLoser);
        }
        if current.as_of == entry.as_of && stable_equal(current, &entry) {
            // Equal-state redelivery of an unjournaled mutation id: the
            // durable row already exists, so report Applied to let the
            // engine journal it (journal self-heal).
            return Ok(ApplyOutcome::Applied);
        }
        if current.as_of == entry.as_of {
            // Same version slot, differing content: the documented tie rule
            // (admission score, then device id) decides deterministically.
            if lww_tie_loser(engine, m, current.admission_score)? {
                return Ok(ApplyOutcome::ConflictLoser);
            }
        }
    }
    engine.sync.persona_stable_put(&entry, "sync-apply")?;
    Ok(ApplyOutcome::Applied)
}

fn persona_pref_apply(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
    entry_v: serde_json::Value,
) -> Result<ApplyOutcome, AmError> {
    let has_context = has_context_field(&entry_v);
    if let Some(outcome) = persona_bank_mismatch(engine, m, envelope_json, &entry_v)? {
        return Ok(outcome);
    }
    let mut pref: PersonaPreference = match serde_json::from_value(entry_v) {
        Ok(pref) => pref,
        Err(e) => return reject(engine, m, envelope_json, "bad persona preference", &e),
    };
    pref.bank_id.clone_from(&m.bank);
    if m.entity_id != pref.key {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::InvalidArgument,
                "persona entity_id must be the attr key",
            ),
        );
    }
    if engine.sync.persona_context_erased(&m.bank)? {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::ErasedSubject,
                "bank persona context purged by a verified erasure (persona erase tombstone)",
            ),
        );
    }
    if let Err(err) = pref.validate() {
        return dead_letter(engine, m, envelope_json, &err);
    }
    let current = engine.sync.persona_preference_current(&m.bank, &pref.key)?;
    if let Some(current) = &current {
        if current.as_of > pref.as_of {
            journal_conflict(engine, m, &current.as_of)?;
            return Ok(ApplyOutcome::ConflictLoser);
        }
        if current.as_of == pref.as_of && pref_equal(current, &pref, has_context) {
            // Equal-state redelivery of an unjournaled mutation id: see the
            // journal self-heal note in `persona_stable_apply`.
            return Ok(ApplyOutcome::Applied);
        }
        if current.as_of == pref.as_of && lww_tie_loser(engine, m, current.admission_score)? {
            return Ok(ApplyOutcome::ConflictLoser);
        }
    }
    // `None` context preserves the stored context (local preference contract).
    engine.sync.persona_preference_put(&pref, "sync-apply")?;
    Ok(ApplyOutcome::Applied)
}

/// Apply one `belief` upsert: create, or append the missing tail of the
/// confidence trajectory. History rows are insert-only and admission
/// is never re-run.
///
/// Context rule: an omitted (or null) `context` preserves the stored
/// value; a present value that differs from the stored one is a replicated
/// authorized correction and lands on the audited correction path (the sender
/// can only have changed the context through `correct_belief_context`, since
/// appends are gated locally too).
///
/// Merge-decides-first ordering: the trajectory merge is decided
/// before any context correction. A diverged trajectory loses with its
/// context UNCHANGED; only an applied outcome (`LocalAhead` or `Extends`)
/// may carry the correction. `LocalAhead` returns `Applied` (journal
/// self-heal), which also keeps a context-only correction journaled.
pub(crate) fn belief_apply(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
) -> Result<ApplyOutcome, AmError> {
    let SyncContent::Plain(v) = &m.content else {
        return Ok(ApplyOutcome::Applied);
    };
    let belief: BeliefObject = match serde_json::from_value(v.clone()) {
        Ok(belief) => belief,
        Err(e) => return reject(engine, m, envelope_json, "bad belief content", &e),
    };
    if m.entity_id != belief.id {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::InvalidArgument,
                "belief entity_id must be the belief id",
            ),
        );
    }
    if belief.bank_id != m.bank {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::Forbidden,
                "mutation bank does not match belief bank",
            ),
        );
    }
    if engine.sync.subject_is_shredded(&belief.subject_id)? {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::ErasedSubject,
                "erased subject content resurrection",
            ),
        );
    }
    if let Err(err) = belief.validate() {
        return dead_letter(engine, m, envelope_json, &err);
    }
    let stored = engine
        .sync
        .belief_by_proposition(&m.bank, &proposition_key(&belief.proposition))?;
    let Some(stored) = stored else {
        if engine.sync.belief_get(&m.bank, &belief.id)?.is_some() {
            return dead_letter(
                engine,
                m,
                envelope_json,
                &AmError::new(
                    ErrorCode::InvalidArgument,
                    "belief id already belongs to a different proposition",
                ),
            );
        }
        engine.sync.belief_create(&belief, "sync-apply")?;
        return Ok(ApplyOutcome::Applied);
    };
    // Merge-decides-first: a diverged trajectory loses with its stored
    // context untouched; the correction applies only on an applying outcome.
    match merge_position(&stored.confidence_history, &belief.confidence_history) {
        Merge::LocalAhead => {
            correct_context(engine, m, &stored, v)?;
            Ok(ApplyOutcome::Applied)
        }
        Merge::Extends(len) => {
            correct_context(engine, m, &stored, v)?;
            for entry in &belief.confidence_history[len..] {
                engine
                    .sync
                    .belief_append(&m.bank, &stored.id, entry, "sync-apply", None)?;
            }
            Ok(ApplyOutcome::Applied)
        }
        Merge::Diverged => {
            journal_conflict(engine, m, &stored.updated_at)?;
            Ok(ApplyOutcome::ConflictLoser)
        }
    }
}

/// The replicated authorized context correction on the audited path.
/// Called only after the merge decided the incoming mutation applies; an
/// omitted (or null) wire context, or one equal to the stored value, is a no-op.
fn correct_context(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    stored: &BeliefObject,
    v: &serde_json::Value,
) -> Result<(), AmError> {
    if let Some(ctx) = raw_context(v)
        && stored.context.as_deref() != Some(ctx)
    {
        engine.sync.belief_correct_context(
            &m.bank,
            &stored.id,
            ctx,
            "sync: replicated authorized belief context correction",
            "sync-apply",
        )?;
    }
    Ok(())
}

/// History merge outcome between the stored trajectory and the incoming one.
enum Merge {
    /// The stored trajectory already contains the incoming one (or it is
    /// identical): no durable append, but the mutation still applies so the
    /// journal self-heals (and a differing wire context can land as a
    /// replicated correction).
    LocalAhead,
    /// The stored trajectory is a prefix; append from this index.
    Extends(usize),
    /// The trajectories diverge: insert-only history cannot merge them.
    Diverged,
}

fn merge_position(stored: &[ConfidenceEntry], incoming: &[ConfidenceEntry]) -> Merge {
    let shared = stored.len().min(incoming.len());
    if stored[..shared] != incoming[..shared] {
        return Merge::Diverged;
    }
    if incoming.len() > stored.len() {
        Merge::Extends(shared)
    } else {
        Merge::LocalAhead
    }
}

/// `true` when the payload entry carried a present `context` string.
fn has_context_field(entry_v: &serde_json::Value) -> bool {
    matches!(entry_v.get("context"), Some(serde_json::Value::String(_)))
}

/// The raw payload's `context` string, `None` when absent or null.
fn raw_context(v: &serde_json::Value) -> Option<&str> {
    v.get("context").and_then(serde_json::Value::as_str)
}

/// Fail closed when a persona payload carries a foreign `bank_id` field.
/// Persona payloads normally carry none (serde skips it and would silently
/// ignore a present one), so the raw payload JSON is checked before decoding
/// Returns the dead-letter outcome to propagate, `None` if in-bank.
fn persona_bank_mismatch(
    engine: &ApplyEngine<'_>,
    m: &SyncMutation,
    envelope_json: &str,
    entry_v: &serde_json::Value,
) -> Result<Option<ApplyOutcome>, AmError> {
    if let Some(wire_bank) = entry_v.get("bank_id").and_then(serde_json::Value::as_str)
        && wire_bank != m.bank
    {
        return dead_letter(
            engine,
            m,
            envelope_json,
            &AmError::new(
                ErrorCode::Forbidden,
                "mutation bank does not match persona entry bank",
            ),
        )
        .map(Some);
    }
    Ok(None)
}

/// Exact round-trip equality: duplicate detection compares values that ride
/// serde/SQL unchanged, so bit-identical comparison is the intended contract.
#[allow(clippy::float_cmp)]
fn stable_equal(a: &PersonaStableEntry, b: &PersonaStableEntry) -> bool {
    a.key == b.key
        && a.value == b.value
        && a.confidence == b.confidence
        && a.source == b.source
        && a.source_ref == b.source_ref
        && a.context == b.context
        && a.admission_score == b.admission_score
}

/// Exact round-trip equality (see `stable_equal`); an incoming payload that
/// omits context never counts as a difference.
#[allow(clippy::float_cmp)]
fn pref_equal(a: &PersonaPreference, b: &PersonaPreference, incoming_has_context: bool) -> bool {
    let context_equal = !incoming_has_context || a.context == b.context;
    context_equal
        && a.key == b.key
        && a.value == b.value
        && a.trend == b.trend
        && a.confidence == b.confidence
        && a.source == b.source
        && a.source_ref == b.source_ref
        && a.admission_score == b.admission_score
}

#[cfg(test)]
#[path = "apply_records_selfheal_tests.rs"]
mod apply_records_selfheal_tests;
#[cfg(test)]
#[path = "apply_records_tests.rs"]
mod apply_records_tests;
