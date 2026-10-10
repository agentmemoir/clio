// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the task/failure history repository
//! contract and its shared id, crypto, and error-mapping helpers.
//!
//! ## Owns
//! - The `HistoryStore` trait (task upsert/read/history, failure write/read,
//!   triple trajectory)
//! - Collision-resistant history row ids and DEK seal/open helpers
//! - The shared task/failure/triple audit-detail builders with PII-safe
//!   context statistics (`history_audit_detail` / `triple_audit_detail`)
//!
//! ## Does not own
//! - SQL for either backend (see `sqlite_history`, `postgres_history`)
//! - Admission scoring or tool orchestration (`clio-history`)
//!
//! ## Boundary
//!
//! Changes to the history repository surface should normally require changes here.
//! Changes to history domain shapes should not require changes here.
//!
//! Keep this module focused on the history repository contract and helpers.

use crate::dek::{CiphertextEnvelope, DekProvider};
use clio_types::{
    AmError, EpisodicType, ErrorCode, FailureRecord, MemoryItem, TaskRecord, TimeAxis, TripleRecord,
};

/// Repository for task and failure history.
///
/// Each write is a gated episodic carrier item plus one structured history row,
/// committed atomically. Definition/steps/`what_failed`/`lesson` content is
/// sealed under the subject DEK; key columns and structure stay plaintext. The
/// carrier item's optional bounded `context` is sealed inside the
/// carrier's dual content and surfaced again by the task/failure reads.
pub trait HistoryStore: Send + Sync {
    /// Atomically insert a carrier `task` item and one task-history version.
    ///
    /// The stored version is `max(existing)+1` within `(bank, task_id)`,
    /// computed inside the write transaction. Returns the persisted record.
    ///
    /// # Errors
    /// Validation, crypto, duplicate, or storage failures.
    fn commit_task_upsert(
        &self,
        item: &MemoryItem,
        draft: &TaskRecord,
        actor: &str,
    ) -> Result<TaskRecord, AmError>;

    /// Latest task version for `(bank, task_id)`, or `None` when absent.
    ///
    /// # Errors
    /// Crypto or storage failures.
    fn get_task(&self, bank_id: &str, task_id: &str) -> Result<Option<TaskRecord>, AmError>;

    /// Task versions for `(bank, task_id)`, newest first, capped by `limit`.
    ///
    /// # Errors
    /// Crypto or storage failures.
    fn task_history(
        &self,
        bank_id: &str,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<TaskRecord>, AmError>;

    /// Distinct task ids with their latest record, newest first, capped by `limit`.
    ///
    /// Used for bounded similar-task matching; never a full bank dump.
    ///
    /// # Errors
    /// Crypto or storage failures.
    fn list_tasks(&self, bank_id: &str, limit: usize) -> Result<Vec<TaskRecord>, AmError>;

    /// Atomically insert a carrier `failure` item and one failure record.
    ///
    /// # Errors
    /// Validation, duplicate `(bank, task_id, attempt_n)`, crypto, or storage failures.
    fn commit_failure_record(
        &self,
        item: &MemoryItem,
        draft: &FailureRecord,
        actor: &str,
    ) -> Result<FailureRecord, AmError>;

    /// Read one failure record by id, scoped by bank.
    ///
    /// # Errors
    /// Crypto or storage failures. `Ok(None)` = missing / wrong bank.
    fn get_failure(
        &self,
        bank_id: &str,
        failure_id: &str,
    ) -> Result<Option<FailureRecord>, AmError>;

    /// Failures for an exact `(bank, task_id)`, newest attempt first, capped.
    ///
    /// # Errors
    /// Crypto or storage failures.
    fn failures_for_task_exact(
        &self,
        bank_id: &str,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<FailureRecord>, AmError>;

    /// All failures in a bank, newest first, capped by `limit`.
    ///
    /// Scoped for bounded similar-task matching after the candidate set is
    /// narrowed by definition similarity; never called as a full-bank dump.
    ///
    /// # Errors
    /// Crypto or storage failures.
    fn list_failures(&self, bank_id: &str, limit: usize) -> Result<Vec<FailureRecord>, AmError>;

    /// Every version of a `(subject, predicate)` triple, oldest transaction
    /// time first, capped by `limit` (includes closed/superseded edges).
    ///
    /// # Errors
    /// Storage failures.
    fn triple_history(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        limit: usize,
    ) -> Result<Vec<TripleRecord>, AmError>;

    /// Point-in-time `(subject, predicate)` versions visible on `axis` at
    /// `as_of`, oldest first.
    ///
    /// The axis predicate is applied inside the query **before** `LIMIT`, and
    /// the newest `limit` versions at/under `as_of` are selected, so a bounded
    /// trajectory can never drop the version current at `as_of`.
    ///
    /// # Errors
    /// Storage failures.
    fn triple_history_at(
        &self,
        bank_id: &str,
        subject: &str,
        predicate: &str,
        as_of: &str,
        axis: TimeAxis,
        limit: usize,
    ) -> Result<Vec<TripleRecord>, AmError>;
}

/// Reject a task carrier/draft pair that cannot be committed.
pub(crate) fn validate_task_carrier(item: &MemoryItem, draft: &TaskRecord) -> Result<(), AmError> {
    if item.id != draft.item_id || item.bank_id != draft.bank_id {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "task item_id/bank_id must match carrier MemoryItem",
        ));
    }
    if item.episodic_type != Some(EpisodicType::Task) {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "task carrier item must have episodic_type=task",
        ));
    }
    reject_dropped_record_context(draft.context.as_deref(), item.context.as_deref(), "task")?;
    Ok(())
}

/// Reject a failure carrier/draft pair that cannot be committed.
pub(crate) fn validate_failure_carrier(
    item: &MemoryItem,
    draft: &FailureRecord,
) -> Result<(), AmError> {
    if item.id != draft.item_id || item.bank_id != draft.bank_id {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "failure item_id/bank_id must match carrier MemoryItem",
        ));
    }
    if item.episodic_type != Some(EpisodicType::Failure) {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            "failure carrier item must have episodic_type=failure",
        ));
    }
    reject_dropped_record_context(draft.context.as_deref(), item.context.as_deref(), "failure")?;
    Ok(())
}

/// The stored record context is the carrier item's `context` (the domain reads
/// resolve it from the carrier), so a structured record that carries its own
/// context the carrier does not store would silently lose it. Fail closed
/// instead of committing a divergent pair.
fn reject_dropped_record_context(
    record_context: Option<&str>,
    carrier_context: Option<&str>,
    kind: &str,
) -> Result<(), AmError> {
    if record_context.is_some() && record_context != carrier_context {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("{kind} context must match the carrier item's context"),
        ));
    }
    Ok(())
}

/// Seal one text field under the subject DEK for storage.
pub(crate) fn seal_history_text(
    kms: &dyn DekProvider,
    subject_id: &str,
    value: &str,
) -> Result<String, AmError> {
    Ok(kms
        .encrypt(subject_id, value.as_bytes())?
        .to_storage_string())
}

/// Open one history text field back into plaintext.
pub(crate) fn open_history_text(kms: &dyn DekProvider, cipher: &str) -> Result<String, AmError> {
    let envelope = CiphertextEnvelope::from_storage_string(cipher)?;
    let bytes = kms.decrypt(&envelope)?;
    String::from_utf8(bytes).map_err(|_| {
        AmError::new(
            ErrorCode::Internal,
            "history text ciphertext is not valid UTF-8",
        )
    })
}

/// Seal an ordered `steps` list as a JSON array string.
pub(crate) fn seal_steps(
    kms: &dyn DekProvider,
    subject_id: &str,
    steps: &[String],
) -> Result<String, AmError> {
    let encoded = serde_json::to_string(steps).expect("steps always serialize");
    seal_history_text(kms, subject_id, &encoded)
}

/// Open a sealed `steps` list; an undecodable payload fails closed.
pub(crate) fn open_steps(kms: &dyn DekProvider, cipher: &str) -> Result<Vec<String>, AmError> {
    let raw = open_history_text(kms, cipher)?;
    serde_json::from_str(&raw)
        .map_err(|e| AmError::new(ErrorCode::Internal, format!("steps decode: {e}")))
}

/// Reject empty required strings on the history write path.
pub(crate) fn require_nonempty(value: &str, label: &str) -> Result<(), AmError> {
    if value.is_empty() {
        return Err(AmError::new(
            ErrorCode::InvalidArgument,
            format!("{label} is required"),
        ));
    }
    Ok(())
}

/// Durable audit detail for a history write: ids and counts
/// plus PII-safe context telemetry — presence, UTF-8 byte length, and a hash
/// for change detection only. The raw context value never enters telemetry.
pub(crate) fn history_audit_detail(
    item: &MemoryItem,
    kind: &str,
    key: &str,
    version: i64,
    steps: usize,
) -> String {
    audit_detail_with_context(
        serde_json::json!({
            "record": kind,
            "key": key,
            "version": version,
            "steps": steps,
        }),
        item,
    )
}

/// Durable audit detail for a triple add/supersede: edge ids plus the same
/// PII-safe context statistics (shared by both backends for parity).
pub(crate) fn triple_audit_detail(
    item: &MemoryItem,
    new_edge_id: &str,
    prior_edge_ids: &[String],
) -> String {
    audit_detail_with_context(
        serde_json::json!({
            "new_edge_id": new_edge_id,
            "prior_edge_ids": prior_edge_ids,
        }),
        item,
    )
}

/// Merge PII-safe context statistics into one audit detail object.
fn audit_detail_with_context(mut detail: serde_json::Value, item: &MemoryItem) -> String {
    if let (Some(fields), serde_json::Value::Object(stats)) = (
        detail.as_object_mut(),
        crate::item_persist::context_stats(item),
    ) {
        fields.extend(stats);
    }
    detail.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers_seal_open_and_map() {
        require_nonempty("x", "x").unwrap();
        assert_eq!(
            require_nonempty("", "bank_id").unwrap_err().code,
            ErrorCode::InvalidArgument
        );
        let kms = crate::dek::LocalDevKms::new();
        let cipher = seal_history_text(&kms, "s", "hello").expect("seal");
        assert_eq!(open_history_text(&kms, &cipher).expect("open"), "hello");
        assert_eq!(
            open_history_text(&kms, "not-json").unwrap_err().code,
            ErrorCode::UnreadableContent
        );
        let steps = vec!["a".to_owned(), "b".to_owned()];
        let sealed = seal_steps(&kms, "s", &steps).expect("seal steps");
        assert_eq!(open_steps(&kms, &sealed).expect("open steps"), steps);
        let bad = seal_history_text(&kms, "s", "not json").expect("seal bad");
        assert_eq!(
            open_steps(&kms, &bad).unwrap_err().code,
            ErrorCode::Internal
        );
        let detail = history_audit_detail(&item("i", "b", EpisodicType::Task), "task", "t1", 2, 3);
        assert!(detail.contains("\"version\":2"));
        assert!(detail.contains("\"context_present\":false"));
        // Non-UTF-8 plaintext hits the open_history_text conversion guard.
        let raw = crate::dek::DekProvider::encrypt(&kms, "s", &[0xFF_u8, 0xFE])
            .expect("encrypt raw")
            .to_storage_string();
        assert_eq!(
            open_history_text(&kms, &raw).unwrap_err().code,
            ErrorCode::Internal
        );
    }

    fn item(id: &str, bank: &str, episodic: EpisodicType) -> MemoryItem {
        MemoryItem {
            id: id.into(),
            bank_id: bank.into(),
            subject_id: "s".into(),
            kind: clio_types::ItemKind::Episodic,
            category: None,
            episodic_type: Some(episodic),
            epistemic_kind: clio_types::EpistemicKind::Fact,
            source_type: None,
            confidence: None,
            admission_score: Some(0.5),
            source_ref: None,
            snapshot: None,
            gist: Some("g".into()),
            valid_from: "t".into(),
            valid_until: None,
            tx_from: "t".into(),
            tx_until: None,
            created_at: "t".into(),
            updated_at: "t".into(),
            context: None,
        }
    }

    #[test]
    fn carrier_guards_reject_mismatches() {
        let task = TaskRecord {
            id: "r".into(),
            bank_id: "b".into(),
            item_id: "i".into(),
            task_id: "t".into(),
            subject_id: "s".into(),
            version: 1,
            definition: "d".into(),
            steps: vec![],
            status: "s".into(),
            evidence_ref: None,
            context: None,
            admission_score: 0.5,
            created_at: "t".into(),
        };
        let failure = FailureRecord {
            id: "r".into(),
            bank_id: "b".into(),
            item_id: "i".into(),
            task_id: "t".into(),
            subject_id: "s".into(),
            attempt_n: 1,
            what_failed: "w".into(),
            lesson: "l".into(),
            lesson_tokens: 1,
            evidence_ref: None,
            context: None,
            admission_score: 0.5,
            epistemic_kind: clio_types::EpistemicKind::Belief,
            created_at: "t".into(),
        };
        assert!(validate_task_carrier(&item("i", "b", EpisodicType::Task), &task).is_ok());
        assert!(validate_failure_carrier(&item("i", "b", EpisodicType::Failure), &failure).is_ok());
        assert_eq!(
            validate_task_carrier(&item("other", "b", EpisodicType::Task), &task)
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            validate_task_carrier(&item("i", "b", EpisodicType::Failure), &task)
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            validate_failure_carrier(&item("other", "b", EpisodicType::Failure), &failure)
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
        assert_eq!(
            validate_failure_carrier(&item("i", "b", EpisodicType::Task), &failure)
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );
    }
}

#[cfg(test)]
#[path = "history_store_audit_tests.rs"]
mod history_store_audit_tests;

#[cfg(test)]
#[path = "history_store_pg_history_tests.rs"]
mod history_store_pg_history_tests;
