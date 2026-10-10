// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the record shapes the relay-blindness tests push: the
//! synthetic caller-chosen strings a relay must not read, the seeding of one
//! persona row pair, one task, one failure, and one explicit edge, and the
//! reading of everything a relay could hold off one stored journal row. Shared
//! by the journal-level test modules, and holding no test of its own.
//!
//! ## Owns
//! - The synthetic strings the journal assertions look for
//! - The store seeders for the handled record kinds
//! - `stored_bytes`, the full readable content of one stored row
//!
//! ## Does not own
//! - Any assertion (`client_wire_blindness_tests`, `client_wire_digest_tests`)
//! - Packaging fixtures used before a push (`client_feed_package_fixtures`)
//!
//! ## Boundary
//!
//! Changes to the record shapes these tests push should normally require
//! changes here. Changes to what is asserted about the stored bytes should not.
//!
//! Keep this module focused on relay-blindness test fixtures.

use clio_store::{AssocEdgeRecord, AssocKind, HistoryStore, PersonaStore, Store, SyncJournalRow};
use clio_types::{
    BeliefObject, ConfidenceEntry, EpisodicType, EpistemicKind, FailureRecord, ItemKind,
    MemoryItem, PersonaPreference, PersonaStableEntry, SourceType, TaskRecord,
};
use serde_json::json;

/// The strings a relay must not read off its own journal. They are synthetic:
/// the property under test is that they do not reach the relay, not what they
/// say.
pub(crate) const ATTR_NAME: &str = "synthetic_attr_name";
pub(crate) const PREF_NAME: &str = "synthetic_pref_name";
pub(crate) const TASK_ID: &str = "synthetic-task-id-42";
pub(crate) const FAILURE_ID: &str = "synthetic-failure-record-7";
pub(crate) const RELATIONSHIP: &str = "synthetic_relationship_label";

/// An episodic carrier item, which every history record rides on.
pub(crate) fn carrier(id: &str, bank: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(EpisodicType::Triple),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.9),
        admission_score: Some(0.85),
        source_ref: Some("turn".into()),
        snapshot: Some(json!({"object": "obj"})),
        gist: Some("carrier".into()),
        valid_from: "2026-01-01T00:00:00Z".into(),
        valid_until: None,
        tx_from: "2026-01-01T00:00:00Z".into(),
        tx_until: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        context: None,
    }
}

/// Every stored column a relay could read off one row, joined: the idempotency
/// key, the routing fields, the identifier, the digest, the sealed content, the
/// encryption flag, and the admission receipt.
///
/// Every column is here on purpose. A substring assertion over a subset cannot
/// see a leak in an omitted column, and it cannot see a digest oracle at all,
/// which is why the guess-confirmation assertions hash candidate preimages
/// against the stored idempotency key rather than searching for a substring.
pub(crate) fn stored_bytes(row: &SyncJournalRow) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        row.sync_mutation_id,
        row.bank_id,
        row.entity_kind,
        row.entity_id,
        row.op,
        row.transaction_time,
        row.device_id,
        row.content_hash,
        row.content_sealed.as_deref().unwrap_or_default(),
        i32::from(row.client_encrypted),
        row.admission_receipt.as_deref().unwrap_or_default(),
    )
}

/// One stable attribute and one preference, both keyed by caller-chosen names.
pub(crate) fn seed_persona(a: &std::sync::Arc<clio_store::SqliteStore>, bank: &str) {
    let entry = PersonaStableEntry {
        bank_id: bank.into(),
        key: ATTR_NAME.into(),
        value: "terse".into(),
        confidence: None,
        source: "explicit".into(),
        source_ref: Some("turn-1".into()),
        context: None,
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
        valid_until: None,
    };
    a.put_stable_entry(&entry, "seed").expect("persona stable");
    let pref = PersonaPreference {
        bank_id: bank.into(),
        key: PREF_NAME.into(),
        value: 0.4,
        trend: 0.1,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: None,
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
    };
    a.put_preference_state(&pref, "seed")
        .expect("persona preference");
}

/// One task and one failure whose id embeds that task's caller-chosen id.
pub(crate) fn seed_task_and_failure(a: &std::sync::Arc<clio_store::SqliteStore>, bank: &str) {
    let mut task_item = carrier("car-task", bank);
    task_item.episodic_type = Some(EpisodicType::Task);
    task_item.gist = Some("do the thing".into());
    let task = TaskRecord {
        id: "task-row-1".into(),
        bank_id: bank.into(),
        item_id: "car-task".into(),
        task_id: TASK_ID.into(),
        subject_id: "subj".into(),
        version: 1,
        definition: "do the thing".into(),
        steps: vec!["step".into()],
        status: "open".into(),
        evidence_ref: None,
        context: None,
        admission_score: 0.85,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    a.commit_task_upsert(&task_item, &task, "agent")
        .expect("a task");

    let mut failure_item = carrier("car-fail", bank);
    failure_item.episodic_type = Some(EpisodicType::Failure);
    failure_item.epistemic_kind = EpistemicKind::Belief;
    failure_item.source_type = Some(SourceType::AgentInferred);
    failure_item.gist = Some("what failed".into());
    let failure = FailureRecord {
        id: FAILURE_ID.into(),
        bank_id: bank.into(),
        item_id: "car-fail".into(),
        task_id: TASK_ID.into(),
        subject_id: "subj".into(),
        attempt_n: 1,
        what_failed: "boom".into(),
        lesson: "do not".into(),
        lesson_tokens: 2,
        evidence_ref: None,
        context: None,
        admission_score: 0.85,
        epistemic_kind: EpistemicKind::Belief,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    a.commit_failure_record(&failure_item, &failure, "agent")
        .expect("a failure");
}

/// One explicit edge between two local endpoint items, carrying the caller's
/// relationship label inside its id.
pub(crate) fn seed_explicit_edge(
    a: &std::sync::Arc<clio_store::SqliteStore>,
    bank: &str,
    rel: &str,
) {
    for id in ["i-1", "i-2"] {
        crate::client::client_tests::create_item(a, bank, id, id);
    }
    let edge = AssocEdgeRecord {
        id: clio_store::explicit_id(bank, "i-1", "i-2", rel),
        bank_id: bank.into(),
        source_id: "i-1".into(),
        target_id: "i-2".into(),
        assoc_kind: AssocKind::Explicit,
        relationship: Some(rel.into()),
        weight: 0.5,
        last_updated_at: "2026-01-01T02:00:00Z".into(),
    };
    a.put_assoc_edge(&edge).expect("explicit edge");
}

/// A belief with one confidence entry, keyed by a machine-generated id.
pub(crate) fn belief(id: &str, bank: &str) -> BeliefObject {
    BeliefObject {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "s1".into(),
        proposition: "a proposition".into(),
        context: None,
        context_as_of: None,
        confidence_history: vec![
            ConfidenceEntry::new("2026-01-01", 0.7, "ev-1", SourceType::UserStated).expect("entry"),
        ],
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}
