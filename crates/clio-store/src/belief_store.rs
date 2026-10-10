// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the belief-object repository contract,
//! its shared validation/id/seal helpers, and the authorized belief-context
//! correction write for both backends.
//!
//! ## Owns
//! - The `BeliefStore` trait (create / append / read / history / context
//!   replace / authorized context correction)
//! - Deterministic confidence-entry ids and duplicate-belief mapping
//! - Belief context seal/open helpers and PII-safe context audit telemetry
//! - The authorized belief-context correction SQL (sealed replacement +
//!   `correct` audit row with hash-only prior/new context stats)
//!
//! ## Does not own
//! - Belief create/append/history SQL (`sqlite_belief`, `postgres_belief`, `belief_tx`)
//! - Admission scoring or tool orchestration (`clio-belief`)
//!
//! ## Boundary
//!
//! Changes to the belief repository surface or the context-correction write
//! should normally require changes here. Changes to belief create/append SQL
//! should not require changes here.
//!
//! Keep this module focused on the belief repository contract and id/validation helpers.

use crate::dek::{CiphertextEnvelope, DekProvider};
use clio_types::{AmError, BeliefObject, ConfidenceEntry, ErrorCode};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Monotonic suffix so two entries sharing one caller `as_of` cannot collide.
static ENTRY_SEQ: AtomicU64 = AtomicU64::new(0);

/// Repository for belief objects and their append-only confidence history.
///
/// Writes are content-encrypted under the subject DEK; history rows are inserted
/// only — a prior confidence entry is never updated or deleted.
pub trait BeliefStore: Send + Sync {
    /// Create a belief and its initial trajectory atomically.
    ///
    /// # Errors
    /// Validation, duplicate proposition, crypto, or storage failures.
    fn create_belief(&self, belief: &BeliefObject, actor: &str) -> Result<(), AmError>;

    /// Append one confidence entry to an existing belief (insert only).
    ///
    /// # Errors
    /// Validation, unknown belief in bank, crypto, or storage failures.
    fn append_confidence(
        &self,
        bank_id: &str,
        belief_id: &str,
        entry: &ConfidenceEntry,
        actor: &str,
    ) -> Result<(), AmError>;

    /// Append one confidence entry, optionally observing the sealed context.
    ///
    /// `None` preserves the stored context untouched. `Some(context)` is an
    /// observation, not a mutation: it MUST be byte-identical to the stored
    /// value, even when none is stored, or the append fails closed with
    /// `InvalidArgument` before any write. Only the authorized
    /// `correct_belief_context` operation changes a stored context.
    fn append_confidence_with_context(
        &self,
        bank_id: &str,
        belief_id: &str,
        entry: &ConfidenceEntry,
        actor: &str,
        context: Option<&str>,
    ) -> Result<(), AmError>;

    /// Read a belief plus its full trajectory scoped by bank.
    ///
    /// # Errors
    /// Crypto (destroyed DEK) or storage failures. `Ok(None)` = missing / wrong bank.
    fn get_belief(&self, bank_id: &str, belief_id: &str) -> Result<Option<BeliefObject>, AmError>;

    /// Resolve a belief by its normalized proposition key, scoped by bank.
    ///
    /// # Errors
    /// Crypto or storage failures. `Ok(None)` = no belief / wrong bank.
    fn find_belief_by_proposition(
        &self,
        bank_id: &str,
        proposition_key: &str,
    ) -> Result<Option<BeliefObject>, AmError>;

    /// Ordered trajectory; `as_of` filters entries with `as_of <= as_of`.
    ///
    /// `as_of` compares as ISO-8601 text (lexicographic); pass the same canonical
    /// form the stored entries use (see `clio_types::ConfidenceEntry::as_of`).
    ///
    /// # Errors
    /// Storage failures. Cross-bank reads fail closed with an empty vector.
    fn belief_history(
        &self,
        bank_id: &str,
        belief_id: &str,
        as_of: Option<&str>,
    ) -> Result<Vec<ConfidenceEntry>, AmError>;

    /// Raw proposition ciphertext for security inspection tests.
    ///
    /// # Errors
    /// Storage failures.
    fn inspect_belief_ciphertext(
        &self,
        bank_id: &str,
        belief_id: &str,
    ) -> Result<Option<String>, AmError>;

    /// Every belief in the bank with its full trajectory, ordered by id.
    ///
    /// # Errors
    /// Crypto (destroyed DEK) or storage failures.
    fn list_beliefs(&self, bank_id: &str) -> Result<Vec<BeliefObject>, AmError>;
}

/// Reject empty required strings on the belief write path.
pub(crate) fn require_nonempty(value: &str, label: &str) -> Result<(), AmError> {
    if value.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("{label} is required"),
        ));
    }
    Ok(())
}

/// Collision-resistant confidence-entry id.
pub(crate) fn unique_entry_id(belief_id: &str, as_of: &str) -> String {
    let seq = ENTRY_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("bce-{belief_id}-{as_of}-{nanos}-{seq}")
}

/// Seal proposition text under the subject DEK for storage.
pub(crate) fn seal_proposition(
    kms: &dyn DekProvider,
    subject_id: &str,
    proposition: &str,
) -> Result<String, AmError> {
    Ok(kms
        .encrypt(subject_id, proposition.as_bytes())?
        .to_storage_string())
}

/// Open proposition ciphertext back into plaintext.
pub(crate) fn open_proposition(kms: &dyn DekProvider, cipher: &str) -> Result<String, AmError> {
    let envelope = CiphertextEnvelope::from_storage_string(cipher)?;
    let bytes = kms.decrypt(&envelope)?;
    String::from_utf8(bytes).map_err(|_| {
        AmError::new(
            ErrorCode::Internal,
            "belief proposition ciphertext is not valid UTF-8",
        )
    })
}

/// Seal the belief's optional context under the subject DEK (the same
/// `CiphertextEnvelope` mechanism as the proposition).
pub(crate) fn seal_belief_context(
    kms: &dyn DekProvider,
    subject_id: &str,
    context: Option<&str>,
) -> Result<Option<String>, AmError> {
    match context {
        Some(context) => Ok(Some(
            kms.encrypt(subject_id, context.as_bytes())?
                .to_storage_string(),
        )),
        None => Ok(None),
    }
}

/// Open sealed belief context with read tolerance.
///
/// `None` covers: never sealed (NULL/empty column) or an unreadable body (for
/// example a crypto-shredded subject whose DEK was destroyed). Belief reads
/// must keep working when only the context became unreadable; the erasure
/// tombstone remains the attestation.
pub(crate) fn open_belief_context(kms: &dyn DekProvider, cipher: Option<&str>) -> Option<String> {
    let cipher = cipher.filter(|s| !s.is_empty())?;
    let envelope = CiphertextEnvelope::from_storage_string(cipher).ok()?;
    let bytes = kms.decrypt(&envelope).ok()?;
    String::from_utf8(bytes).ok()
}

/// Fixed domain-separation salt for the belief context change-detection hash.
pub(crate) const BELIEF_CONTEXT_HASH_DOMAIN: &[u8] = b"clio-belief-context-telemetry";

/// PII-safe belief context telemetry for `audit_events.detail_json`: presence,
/// UTF-8 byte length, and a hash for change detection only. The raw context
/// value never enters telemetry.
pub(crate) fn belief_context_stats(context: Option<&str>) -> serde_json::Value {
    match context {
        Some(context) => serde_json::json!({
            "context_present": true,
            "context_len": context.len(),
            "context_hash": crate::erase_hash::sha256_hex(BELIEF_CONTEXT_HASH_DOMAIN, context),
        }),
        None => serde_json::json!({"context_present": false, "context_len": 0}),
    }
}

/// Validate an optional context argument so writes fail closed before any SQL.
pub(crate) fn validate_context_arg(context: Option<&str>) -> Result<(), AmError> {
    match context {
        Some(context) => clio_types::validate_context(context),
        None => Ok(()),
    }
}

/// The append-gate usage error: a supplied append context differs from
/// the stored value (including when none is stored). Only the authorized
/// `correct_belief_context` operation may change a stored context.
pub(crate) fn context_change_requires_correction() -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        "belief context differs from the stored value; changing a stored context \
         requires correct_belief_context",
    )
}

/// Durable audit detail for belief create/append rows, sufficient to reconstruct
/// the observe: outcome flag, observed confidence, provenance, the
/// trajectory length after the write, and optional hash-only context telemetry.
pub(crate) fn belief_audit_detail(
    outcome: &str,
    confidence: f64,
    source_type: &str,
    entry_count: usize,
    context_stats: Option<&serde_json::Value>,
) -> String {
    let mut detail = serde_json::Map::new();
    detail.insert(outcome.to_owned(), serde_json::Value::Bool(true));
    detail.insert("confidence".into(), serde_json::json!(confidence));
    detail.insert("source_type".into(), serde_json::json!(source_type));
    detail.insert("entry_count".into(), serde_json::json!(entry_count));
    if let Some(stats) = context_stats
        && let Some(stats) = stats.as_object()
    {
        for (key, value) in stats {
            detail.insert(key.clone(), value.clone());
        }
    }
    serde_json::Value::Object(detail).to_string()
}

/// Map a driver error from a beliefs insert into a structured duplicate error.
pub(crate) fn map_belief_insert(result: Result<u64, String>) -> Result<(), AmError> {
    match result {
        Ok(_) => Ok(()),
        Err(msg) => {
            if msg.contains("UNIQUE") || msg.contains("unique") || msg.contains("duplicate key") {
                Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "belief already exists for proposition key",
                ))
            } else {
                Err(AmError::new(
                    ErrorCode::Internal,
                    format!("belief insert: {msg}"),
                ))
            }
        }
    }
}

/// Validate an authorized belief-context correction so writes fail closed
/// before any SQL (only the item-level context is correctable).
pub(crate) fn validate_belief_context_correction(
    bank_id: &str,
    belief_id: &str,
    new_context: &str,
    reason: &str,
    actor: &str,
) -> Result<(), AmError> {
    require_nonempty(bank_id, "bank_id")?;
    require_nonempty(belief_id, "belief_id")?;
    require_nonempty(reason, "reason")?;
    crate::item_persist::require_actor(actor)?;
    clio_types::validate_context(new_context)
}

/// PII-safe audit detail for an authorized belief-context correction:
/// the reason plus hash-only prior/new context stats — presence, length, and a
/// change-detection hash only. No sealed or raw context value is
/// retained, so the prior context is unrecoverable by design; the hash proves
/// what changed without storing it.
pub(crate) fn belief_context_correction_detail(
    prior_plain: Option<&str>,
    new_context: &str,
    reason: &str,
) -> String {
    let mut fields = serde_json::Map::new();
    fields.insert("corrected".into(), serde_json::json!("context"));
    fields.insert("reason".into(), serde_json::json!(reason));
    for (prefix, stats) in [
        ("prior_", belief_context_stats(prior_plain)),
        ("", belief_context_stats(Some(new_context))),
    ] {
        if let Some(entries) = stats.as_object() {
            for (key, value) in entries {
                fields.insert(format!("{prefix}{key}"), value.clone());
            }
        }
    }
    serde_json::Value::Object(fields).to_string()
}

#[path = "belief_context_correct.rs"]
mod belief_context_correct;

#[cfg(test)]
#[path = "belief_store_belief_tests.rs"]
pub(crate) mod belief_store_belief_tests;
