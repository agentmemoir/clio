// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for shared seal/open helpers for ungated memory-item writes.
//!
//! ## Owns
//! - DualContent seal/open via DEK
//! - Column mapping helpers and insert/actor error mapping
//! - Distinct missing-half / format-mismatch errors
//! - PII-safe `context` telemetry detail for audit rows
//!
//! ## Does not own
//! - Backend SQL statements
//! - Domain `MemoryItem` validation
//!
//! ## Boundary
//!
//! Changes to seal/open contracts should normally require changes here.
//! Changes to Store trait surface should not require changes here.

use crate::content_envelope::DualContent;
use crate::dek::DekProvider;
use clio_types::{AmError, ErrorCode, MemoryItem};

/// Encrypt dual snapshot/gist into a storage string.
pub(crate) fn seal_dual(
    kms: &dyn DekProvider,
    subject_id: &str,
    item: &MemoryItem,
) -> Result<String, AmError> {
    seal_dual_content(
        kms,
        subject_id,
        item.snapshot.clone(),
        item.gist.clone(),
        item.context.clone(),
    )
}

/// Encrypt a snapshot/gist/context triple into a storage string. The snapshot
/// re-projection path uses this directly so it can move the snapshot half
/// while preserving the sealed gist and context byte for byte.
pub(crate) fn seal_dual_content(
    kms: &dyn DekProvider,
    subject_id: &str,
    snapshot: Option<serde_json::Value>,
    gist: Option<String>,
    context: Option<String>,
) -> Result<String, AmError> {
    let dual = DualContent::new(snapshot, gist, context);
    let bytes = dual.to_bytes();
    let envelope = kms.encrypt(subject_id, &bytes)?;
    Ok(envelope.to_storage_string())
}

/// Fixed domain-separation salt for the context change-detection hash.
const CONTEXT_HASH_DOMAIN: &[u8] = b"clio-context-telemetry";

/// PII-safe context telemetry for `audit_events.detail_json`:
/// presence, UTF-8 byte length, and a hash for change detection only. The raw
/// context value never enters telemetry.
pub(crate) fn context_stats(item: &MemoryItem) -> serde_json::Value {
    match &item.context {
        Some(context) => serde_json::json!({
            "context_present": true,
            "context_len": context.len(),
            "context_hash": crate::erase_hash::sha256_hex(CONTEXT_HASH_DOMAIN, context),
        }),
        None => serde_json::json!({"context_present": false, "context_len": 0}),
    }
}

/// [`context_stats`] serialized for one `detail_json` SQL bind.
pub(crate) fn context_audit_detail(item: &MemoryItem) -> String {
    context_stats(item).to_string()
}

/// Decrypt and parse dual content from storage.
///
/// Legacy `put_item` rows store opaque plaintext (not DualContent v1). Those
/// decrypt but fail parse and map to `content_format_mismatch`, as do rows a
/// newer build wrote in a version this build cannot read.
pub(crate) fn open_dual(kms: &dyn DekProvider, cipher_text: &str) -> Result<DualContent, AmError> {
    if cipher_text.is_empty() {
        return Err(missing_ciphertext());
    }
    let envelope = crate::dek::CiphertextEnvelope::from_storage_string(cipher_text)?;
    let bytes = kms.decrypt(&envelope)?;
    // The row decrypted, so a failure here is the row's own bytes, never the
    // backend. `from_bytes` documents that its only error is
    // `UnreadableContent`, so this mapping is total: no caller has to match on
    // the parse detail, and a newer content version is skippable exactly like a
    // legacy row instead of aborting the caller's scan.
    DualContent::from_bytes(&bytes).map_err(|_| content_format_mismatch())
}

/// Read only the sealed `context` for audit/diagnostic projections.
///
/// `None` covers three states: no sealed body, no context in it, or an
/// unreadable body (for example a crypto-shredded subject). Audit views must
/// keep working after erasure, so decryption failures are not errors here; the
/// erased subject's tombstone remains the erasure attestation.
pub(crate) fn open_context(kms: &dyn DekProvider, cipher_text: Option<&str>) -> Option<String> {
    let cipher_text = cipher_text.filter(|s| !s.is_empty())?;
    open_dual(kms, cipher_text)
        .ok()
        .and_then(|dual| dual.context)
}

/// SQL-facing category / episodic_type pair.
pub(crate) fn kind_columns(item: &MemoryItem) -> (Option<&'static str>, Option<&'static str>) {
    (
        item.category.map(|c| c.as_str()),
        item.episodic_type.map(|t| t.as_str()),
    )
}

/// SQL-facing source_type string.
pub(crate) fn source_type_sql(item: &MemoryItem) -> Option<&'static str> {
    item.source_type.map(|s| s.as_str())
}

/// Distinct missing-half errors (item exists).
pub(crate) fn snapshot_missing() -> AmError {
    AmError::new(ErrorCode::InvalidArgument, "snapshot_missing")
}

/// Distinct missing-half errors (item exists).
pub(crate) fn gist_missing() -> AmError {
    AmError::new(ErrorCode::InvalidArgument, "gist_missing")
}

/// Row has no usable `content_ciphertext`.
///
/// `UnreadableContent`, not `Internal`: the row is unreadable, the backend is fine.
pub(crate) fn missing_ciphertext() -> AmError {
    AmError::new(
        ErrorCode::UnreadableContent,
        "item row missing content_ciphertext",
    )
}

/// Ciphertext decrypted but is not content this build can read: legacy
/// opaque bytes, or a newer DualContent version.
pub(crate) fn content_format_mismatch() -> AmError {
    AmError::new(
        ErrorCode::UnreadableContent,
        "content_format_mismatch: expected DualContent v1 (create_memory_item), not legacy opaque bytes or newer-version bytes",
    )
}

/// Named error when a write tries to reactivate a crypto-shredded subject.
///
/// Erasure (crypto-shred) destroys the DEK and closes the subject for writes.
/// A non-fail-closed DEK provider must never be able to reopen the subject at
/// the storage layer, so every `subject_keys` upsert keeps `shredded_at` set
/// and reports zero rows for a shredded subject — mapped here.
pub(crate) fn shredded_subject_err(subject_id: &str) -> AmError {
    AmError::new(
        ErrorCode::ErasedSubject,
        format!(
            "subject `{subject_id}` was crypto-shredded (erasure); \
             writes that reactivate it are refused"
        ),
    )
}

/// Refuse a `subject_keys` upsert that was suppressed by the shred guard.
pub(crate) fn check_subject_reactivation(updated: u64, subject_id: &str) -> Result<(), AmError> {
    if updated == 0 {
        return Err(shredded_subject_err(subject_id));
    }
    Ok(())
}

/// Named error for an upsert suppressed by the cross-bank id guard.
pub(crate) fn cross_bank_item_err(item_id: &str) -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        format!(
            "item id `{item_id}` already exists in a different bank; \
             the upsert was refused (ids are deployment-unique)"
        ),
    )
}

/// Named error for an update that tries to move an item to another subject.
pub(crate) fn subject_immutable_err(item_id: &str) -> AmError {
    AmError::new(
        ErrorCode::InvalidArgument,
        format!(
            "item `{item_id}` cannot change subject_id on update; \
             the subject anchors the DEK and erasure scope"
        ),
    )
}

/// Refuse an items upsert that was suppressed by the same-bank guard.
pub(crate) fn check_item_upsert_applied(updated: u64, item_id: &str) -> Result<(), AmError> {
    if updated == 0 {
        return Err(cross_bank_item_err(item_id));
    }
    Ok(())
}

/// Reject empty actor on ungated writes.
pub(crate) fn require_actor(actor: &str) -> Result<(), AmError> {
    if actor.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "actor is required on memory-item writes",
        ));
    }
    Ok(())
}

/// Map insert row-count / unique violations.
pub(crate) fn map_item_insert(result: Result<u64, String>) -> Result<(), AmError> {
    match result {
        Ok(1) => Ok(()),
        Ok(_) => Err(AmError::new(
            ErrorCode::Internal,
            "unexpected items insert row count",
        )),
        Err(msg) => {
            if msg.contains("UNIQUE") || msg.contains("unique") || msg.contains("duplicate key") {
                Err(AmError::new(
                    ErrorCode::InvalidArgument,
                    "duplicate item id",
                ))
            } else {
                Err(AmError::new(
                    ErrorCode::Internal,
                    format!("items insert: {msg}"),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_envelope::DualContent;

    #[test]
    fn map_insert_and_actor_helpers() {
        require_actor("a").unwrap();
        assert!(require_actor("").is_err());
        map_item_insert(Ok(1)).unwrap();
        assert_eq!(
            map_item_insert(Ok(0)).unwrap_err().code,
            ErrorCode::Internal
        );
        assert_eq!(
            map_item_insert(Err("UNIQUE constraint".into()))
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            map_item_insert(Err("duplicate key".into()))
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            map_item_insert(Err("disk full".into())).unwrap_err().code,
            ErrorCode::Internal
        );
        assert_eq!(snapshot_missing().message, "snapshot_missing");
        assert_eq!(gist_missing().message, "gist_missing");
        assert_eq!(
            missing_ciphertext().message,
            "item row missing content_ciphertext"
        );
        assert!(
            content_format_mismatch()
                .message
                .contains("content_format_mismatch")
        );
        let kms = crate::dek::LocalDevKms::new();
        assert!(open_dual(&kms, "").is_err());
        let sealed = kms.encrypt("s", b"not-json").unwrap().to_storage_string();
        assert_eq!(
            open_dual(&kms, &sealed).unwrap_err().message,
            content_format_mismatch().message
        );
        let dual = DualContent::new(None, Some("g".into()), None);
        let ok = kms
            .encrypt("s", &dual.to_bytes())
            .unwrap()
            .to_storage_string();
        assert_eq!(open_dual(&kms, &ok).unwrap().gist.as_deref(), Some("g"));
        // A content version this build cannot read is skippable for the same
        // reason a legacy row is: the bytes are the row's, and the backend is
        // fine. It used to keep its own message, which meant the index workers
        // — which matched on that text — aborted the whole drain on it.
        let bad_ver = kms.encrypt("s", br#"{"v":9}"#).unwrap().to_storage_string();
        let err = open_dual(&kms, &bad_ver).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnreadableContent);
        assert_eq!(err.message, content_format_mismatch().message);
        // An envelope-shaped column whose JSON is broken is skippable too.
        let not_envelope = "not-json".to_owned();
        let err = open_dual(&kms, &not_envelope).unwrap_err();
        assert_eq!(err.code, ErrorCode::UnreadableContent);
    }

    fn context_item(context: Option<&str>) -> MemoryItem {
        MemoryItem {
            id: "i-ctx".into(),
            bank_id: "b".into(),
            subject_id: "s".into(),
            kind: clio_types::ItemKind::Semantic,
            category: Some(clio_types::SemanticCategory::Persona),
            episodic_type: None,
            epistemic_kind: clio_types::EpistemicKind::Fact,
            source_type: None,
            confidence: None,
            admission_score: None,
            source_ref: None,
            context: context.map(str::to_owned),
            snapshot: None,
            gist: Some("g".into()),
            valid_from: "t0".into(),
            valid_until: None,
            tx_from: "t0".into(),
            tx_until: None,
            created_at: "t0".into(),
            updated_at: "t0".into(),
        }
    }

    #[test]
    fn seal_open_carries_context_inside_the_content_boundary() {
        let kms = crate::dek::LocalDevKms::new();
        let sealed = seal_dual(&kms, "s", &context_item(Some("team chat"))).unwrap();
        assert!(!sealed.contains("team chat"), "context must be encrypted");
        let opened = open_dual(&kms, &sealed).unwrap();
        assert_eq!(opened.context.as_deref(), Some("team chat"));
        let absent = seal_dual(&kms, "s", &context_item(None)).unwrap();
        assert!(open_dual(&kms, &absent).unwrap().context.is_none());
    }

    #[test]
    fn context_audit_detail_is_pii_safe() {
        let absent: serde_json::Value =
            serde_json::from_str(&context_audit_detail(&context_item(None))).unwrap();
        assert_eq!(absent["context_present"], false);
        assert_eq!(absent["context_len"], 0);
        assert!(absent.get("context_hash").is_none());

        let with = context_audit_detail(&context_item(Some("team chat")));
        assert!(
            !with.contains("team chat"),
            "telemetry must not carry raw context"
        );
        let detail: serde_json::Value = serde_json::from_str(&with).unwrap();
        assert_eq!(detail["context_present"], true);
        assert_eq!(detail["context_len"], "team chat".len());
        assert_eq!(
            detail["context_hash"],
            crate::erase_hash::sha256_hex(CONTEXT_HASH_DOMAIN, "team chat")
        );
    }
}

#[cfg(test)]
#[path = "item_persist_route_matrix_tests.rs"]
mod item_persist_route_matrix_tests;
