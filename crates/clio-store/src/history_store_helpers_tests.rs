// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Test-only units owned by `history_store.rs`: the sealed text/steps helpers
//! and the carrier guards. Never compiled into production builds.

use super::*;

#[test]
fn helpers_seal_open_and_map() {
    require_nonempty("x", "x").unwrap();
    assert_eq!(
        require_nonempty("", "bank_id").unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    let kms = crate::dek::LocalDevKms::new();
    let cipher = seal_history_text(&kms, "history.text", "rec-1", "s", "hello").expect("seal");
    let opened = open_history_text(&kms, "history.text", "rec-1", &cipher).expect("open");
    assert_eq!(opened, "hello");
    let err = open_history_text(&kms, "history.text", "rec-1", "not-json").unwrap_err();
    assert_eq!(err.code, ErrorCode::UnreadableContent);
    let steps = vec!["a".to_owned(), "b".to_owned()];
    let sealed = seal_steps(&kms, "task.steps", "rec-1", "s", &steps).expect("seal steps");
    let opened_steps = open_steps(&kms, "task.steps", "rec-1", &sealed).expect("open steps");
    assert_eq!(opened_steps, steps);
    let bad = seal_history_text(&kms, "task.steps", "rec-1", "s", "not json").expect("seal bad");
    assert_eq!(
        open_steps(&kms, "task.steps", "rec-1", &bad)
            .unwrap_err()
            .code,
        ErrorCode::Internal
    );
    let detail = history_audit_detail(&item("i", "b", EpisodicType::Task), "task", "t1", 2, 3);
    assert!(detail.contains("\"version\":2"));
    assert!(detail.contains("\"context_present\":false"));
    // Non-UTF-8 plaintext hits the open_history_text conversion guard.
    let raw =
        crate::dek::DekProvider::encrypt(&kms, "history.text", "rec-1", "s", &[0xFF_u8, 0xFE])
            .expect("encrypt raw")
            .to_storage_string();
    let raw_err = open_history_text(&kms, "history.text", "rec-1", &raw).unwrap_err();
    assert_eq!(raw_err.code, ErrorCode::Internal);
}

#[test]
fn sealed_fields_of_one_row_are_not_interchangeable() {
    let kms = crate::dek::LocalDevKms::new();
    let proposition =
        seal_history_text(&kms, "belief.proposition", "row-1", "s", "text").expect("seal");
    assert_eq!(
        open_history_text(&kms, "belief.context", "row-1", &proposition)
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
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
