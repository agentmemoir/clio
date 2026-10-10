// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the persona-document repository contract
//! and its shared write helpers.
//!
//! ## Owns
//! - The `PersonaStore` trait (stable put / preference upsert+read / document
//!   read / history)
//! - Deterministic persona-row ids and DEK seal/open helpers (bank-scoped),
//!   including the optional sealed `context` on persona records
//! - Shared persona write helpers: PII-safe context stats and the
//!   `audit_events` detail builder for persona writes
//!
//! ## Does not own
//! - SQL for either backend (see `sqlite_persona`, `postgres_persona`)
//! - Admission scoring, budget truncation, or tool orchestration (`clio-persona`)
//! - EMA math (the shared continuous engine owns preference value/trend state)
//!
//! ## Boundary
//!
//! Changes to the persona repository surface should normally require changes here.
//! Changes to persona domain shapes should not require changes here.
//!
//! Keep this module focused on the persona repository contract and id/crypto helpers.

use crate::dek::{CiphertextEnvelope, DekProvider};
use clio_types::{
    AmError, ErrorCode, PersonaDocument, PersonaPreference, PersonaStableEntry, validate_context,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Monotonic suffix so two rows sharing one caller timestamp cannot collide.
static PERSONA_SEQ: AtomicU64 = AtomicU64::new(0);

/// Repository for the persona companion document.
///
/// Storage model (documented choice): **dedicated persona tables**, not
/// item-backed rows. Semantic `store(..., category=persona)` items are a
/// different subsystem and never feed `persona_get`; the companion document is
/// assembled only from `persona_stable_entries` + `persona_preferences`, so
/// category-`persona` items cannot bypass the companion token budget.
///
/// Stable values are content-encrypted under the bank's DEK (subject id =
/// bank id); preferences stay numeric at rest.
pub trait PersonaStore: Send + Sync {
    /// Insert one new open stable version; closes the prior open version of the
    /// same `(bank, key)` on both time axes (never deletes invalidation).
    ///
    /// Context (ordinary updates preserve context): a version written
    /// without a caller context inherits the currently-open version's sealed
    /// context into the new version (a truly new key stays without one). A
    /// supplied context is the explicit audited replacement act for the new
    /// version; prior versions keep their own context in history.
    ///
    /// # Errors
    /// Validation, crypto, or storage failures. Fail-closed with
    /// `ErasedSubject` when the bank's persona context was purged by a
    /// verified compliance erasure (`persona_erase_tombstones`): persona rows
    /// cannot name the erased subject, so a later persona write into the bank
    /// would resurrect purged context.
    fn put_stable_entry(&self, entry: &PersonaStableEntry, actor: &str) -> Result<(), AmError>;

    /// Upsert one preference row. `value`/`trend`/`as_of` come from the shared
    /// engine; on conflict the stored create-time `admission_score` is retained.
    ///
    /// Context (ordinary updates preserve context): a supplied `Some`
    /// context is sealed and replaces the row's stored context, taking effect
    /// at this write's `as_of`; a `None` context is an ordinary update that
    /// preserves both the stored context and its effective time (never
    /// cleared). The effective time gates point-in-time series reads
    /// (`ContinuousStore::continuous_series`).
    ///
    /// # Errors
    /// Validation or storage failures. Fail-closed with `ErasedSubject` when
    /// the bank's persona context was purged by a verified compliance erasure
    /// (`persona_erase_tombstones`).
    fn put_preference_state(&self, pref: &PersonaPreference, actor: &str) -> Result<(), AmError>;

    /// Assemble the current document for one bank: open stable entries plus
    /// preference rows. Empty document when the bank has none.
    ///
    /// # Errors
    /// Crypto or storage failures.
    fn get_persona_document(&self, bank_id: &str) -> Result<PersonaDocument, AmError>;

    /// Read one preference row directly from `persona_preferences` (no stable
    /// entries loaded; values stay numeric at rest). Only the optional context
    /// is DEK-opened, tolerantly (`None` when absent or unreadable). Missing
    /// key or bank returns `None`.
    ///
    /// # Errors
    /// Storage failures.
    fn get_preference(
        &self,
        bank_id: &str,
        key: &str,
    ) -> Result<Option<PersonaPreference>, AmError>;

    /// All retained versions of one stable key, oldest first (slice-15 feed:
    /// invalidation history is retained, never deleted).
    ///
    /// # Errors
    /// Crypto or storage failures. Cross-bank reads fail closed with an empty vector.
    fn stable_history(&self, bank_id: &str, key: &str) -> Result<Vec<PersonaStableEntry>, AmError>;

    /// Raw stable-value ciphertext for security inspection tests.
    ///
    /// # Errors
    /// Storage failures.
    fn inspect_stable_ciphertext(
        &self,
        bank_id: &str,
        key: &str,
    ) -> Result<Option<String>, AmError>;
}

/// Collision-resistant persona row id.
pub(crate) fn unique_persona_id(prefix: &str, bank_id: &str, key: &str) -> String {
    let seq = PERSONA_SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{prefix}-{bank_id}-{key}-{nanos}-{seq}")
}

/// Seal a persona value string under the bank's DEK (subject id = bank id).
pub(crate) fn seal_persona_value(
    kms: &dyn DekProvider,
    bank_id: &str,
    value: &str,
) -> Result<String, AmError> {
    Ok(kms.encrypt(bank_id, value.as_bytes())?.to_storage_string())
}

/// Seal an optional persona `context` string under the subject's DEK (for
/// persona records the subject is the bank id).
///
/// `None` yields `None` (nothing written to `context_cipher`); a supplied
/// value is validated (`validate_context`) and fails closed, so an invalid
/// context reaches the caller before any write happens.
///
/// # Errors
/// Invalid context (empty or over `CONTEXT_MAX_BYTES`) or crypto failures.
pub(crate) fn seal_persona_context(
    kms: &dyn DekProvider,
    subject: &str,
    context: Option<&str>,
) -> Result<Option<String>, AmError> {
    match context {
        None => Ok(None),
        Some(text) => {
            validate_context(text)?;
            Ok(Some(
                kms.encrypt(subject, text.as_bytes())?.to_storage_string(),
            ))
        }
    }
}

/// Open sealed persona `context` ciphertext back into plaintext.
///
/// `None` covers three states: no sealed value, an empty column, or an
/// unreadable body (for example a crypto-shredded subject). Persona reads must
/// keep working after erasure, so decryption failures are not errors here —
/// the same tolerance as [`crate::item_persist::open_context`].
pub(crate) fn open_persona_context(kms: &dyn DekProvider, cipher: Option<&str>) -> Option<String> {
    let cipher = cipher.filter(|s| !s.is_empty())?;
    let envelope = CiphertextEnvelope::from_storage_string(cipher).ok()?;
    let bytes = kms.decrypt(&envelope).ok()?;
    String::from_utf8(bytes).ok()
}

/// Open persona value ciphertext back into plaintext.
pub(crate) fn open_persona_value(kms: &dyn DekProvider, cipher: &str) -> Result<String, AmError> {
    let envelope = CiphertextEnvelope::from_storage_string(cipher)?;
    let bytes = kms.decrypt(&envelope)?;
    String::from_utf8(bytes).map_err(|_| {
        AmError::new(
            ErrorCode::Internal,
            "persona value ciphertext is not valid UTF-8",
        )
    })
}

/// Fixed domain-separation salt for the persona context change-detection
/// hash.
pub(crate) const PERSONA_CONTEXT_HASH_DOMAIN: &[u8] = b"clio-persona-context-telemetry";

/// PII-safe persona context telemetry for `audit_events.detail_json`:
/// presence, UTF-8 byte length, and a hash for change detection only. The raw
/// context value never enters telemetry.
pub(crate) fn persona_context_stats(context: Option<&str>) -> serde_json::Value {
    match context {
        Some(context) => serde_json::json!({
            "context_present": true,
            "context_len": context.len(),
            "context_hash": crate::erase_hash::sha256_hex(PERSONA_CONTEXT_HASH_DOMAIN, context),
        }),
        None => serde_json::json!({"context_present": false, "context_len": 0}),
    }
}

/// audit detail for one persona write: record kind and key, the context
/// action (replaced / preserved / set / none), and presence + length + hash
/// stats for the prior and effective context values. Never the raw text and
/// never a sealed copy of it.
///
/// No ciphertext is retained here on purpose: `audit_events` is append-only
/// and erasure cannot rewrite it, so a sealed prior value stored in the detail
/// would survive the subject's erasure as a decryptable derived copy.
///  Change detection uses the presence/length/hash stats only; versioned
/// rows (stable entries) keep their own prior ciphertext in the closed history
/// row, which the erase path clears.
pub(crate) fn persona_audit_detail(
    kind: &str,
    key: &str,
    action: &str,
    prior_context: Option<&str>,
    new_context: Option<&str>,
) -> String {
    let mut detail = serde_json::Map::new();
    detail.insert("persona".into(), serde_json::json!(kind));
    detail.insert("key".into(), serde_json::json!(key));
    detail.insert("context_action".into(), serde_json::json!(action));
    if let Some(prior) = prior_context {
        detail.insert("context_prior_present".into(), serde_json::json!(true));
        detail.insert("context_prior_len".into(), serde_json::json!(prior.len()));
        detail.insert(
            "context_prior_hash".into(),
            serde_json::json!(crate::erase_hash::sha256_hex(
                PERSONA_CONTEXT_HASH_DOMAIN,
                prior
            )),
        );
    } else {
        detail.insert("context_prior_present".into(), serde_json::json!(false));
        detail.insert("context_prior_len".into(), serde_json::json!(0));
    }
    let effective = persona_context_stats(new_context);
    if let Some(map) = effective.as_object() {
        for (key, value) in map {
            detail.insert(key.clone(), value.clone());
        }
    }
    serde_json::Value::Object(detail).to_string()
}

/// The context action label for one persona write: a caller-supplied
/// context is the explicit replacement act (`replaced`, or `set` when the
/// record had no prior context); an omitted context is an ordinary update
/// (`preserved` when a context existed, `none` otherwise).
pub(crate) fn persona_context_action(supplied: bool, prior_present: bool) -> &'static str {
    match (supplied, prior_present) {
        (true, true) => "replaced",
        (true, false) => "set",
        (false, true) => "preserved",
        (false, false) => "none",
    }
}

/// Reject empty required strings on the persona write path.
pub(crate) fn persona_require_nonempty(value: &str, label: &str) -> Result<(), AmError> {
    if value.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("{label} is required"),
        ));
    }
    Ok(())
}

/// Map a driver error from an open-row insert into a structured conflict error.
pub(crate) fn map_persona_insert(result: Result<u64, String>) -> Result<(), AmError> {
    match result {
        Ok(_) => Ok(()),
        Err(msg) => {
            if msg.contains("UNIQUE") || msg.contains("unique") || msg.contains("duplicate key") {
                Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "concurrent persona stable put for the same key; retry the put",
                ))
            } else {
                Err(AmError::new(
                    ErrorCode::Internal,
                    format!("persona insert: {msg}"),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_validate_and_map() {
        persona_require_nonempty("x", "x").unwrap();
        assert_eq!(
            persona_require_nonempty("", "bank_id").unwrap_err().code,
            ErrorCode::InvalidArgument
        );
        let a = unique_persona_id("pse", "b1", "k");
        let b = unique_persona_id("pse", "b1", "k");
        assert_ne!(a, b);
        assert!(a.starts_with("pse-b1-k"));
        map_persona_insert(Ok(1)).unwrap();
        assert_eq!(
            map_persona_insert(Err("UNIQUE constraint failed".into()))
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            map_persona_insert(Err("duplicate key value".into()))
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            map_persona_insert(Err("disk full".into()))
                .unwrap_err()
                .code,
            ErrorCode::Internal
        );
        // DEK seal/open roundtrip, plus the non-UTF-8 plaintext boundary.
        let kms = crate::dek::LocalDevKms::new();
        let cipher = seal_persona_value(&kms, "bank", "plain-value").expect("seal");
        assert_eq!(
            open_persona_value(&kms, &cipher).expect("open"),
            "plain-value"
        );
        let bad = kms
            .encrypt("bank", &[0xFF_u8, 0xFE])
            .expect("encrypt raw bytes")
            .to_storage_string();
        // Non-UTF-8 sealed bytes keep persona's own mapping; only the envelope
        // decode is classified as unreadable content.
        assert_eq!(
            open_persona_value(&kms, &bad).unwrap_err().code,
            ErrorCode::Internal
        );
        assert_eq!(
            open_persona_value(&kms, "not-json").unwrap_err().code,
            ErrorCode::UnreadableContent
        );
        // Optional context seal/open: None stays None, valid round-trips,
        // invalid fails closed, and unreadable ciphertext opens to None.
        assert_eq!(seal_persona_context(&kms, "bank", None).unwrap(), None);
        let sealed = seal_persona_context(&kms, "bank", Some("why recorded"))
            .expect("seal context")
            .expect("some cipher");
        assert!(!sealed.contains("why recorded"));
        assert_eq!(
            open_persona_context(&kms, Some(&sealed)).as_deref(),
            Some("why recorded")
        );
        assert_eq!(open_persona_context(&kms, None), None);
        assert_eq!(open_persona_context(&kms, Some("")), None);
        assert_eq!(open_persona_context(&kms, Some("not-json")), None);
        let err = seal_persona_context(&kms, "bank", Some("")).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidArgument);
        let err = seal_persona_context(&kms, "bank", Some(&"a".repeat(4097))).unwrap_err();
        assert_eq!(err.code, ErrorCode::OutOfRange);
    }
}

#[cfg(test)]
#[path = "persona_store_persona_tests.rs"]
pub(crate) mod persona_store_persona_tests;
