// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the record shapes the packaging-level tests package: one
//! bank, one task id, one attribute key, one failure id, one relationship label,
//! and builders for each. Shared by the packaging test modules so both assert
//! against identical inputs, and holding no test of its own.
//!
//! ## Owns
//! - The synthetic record fixtures and the packaging-level client configs
//!
//! ## Does not own
//! - Any assertion (`client_feed_package_handle_tests`,
//!   `client_feed_package_digest_tests`)
//! - The packaging helpers themselves (`client_feed_package`)
//!
//! ## Boundary
//!
//! Changes to the record shapes these tests package should normally require
//! changes here. Changes to what is asserted about them should not.
//!
//! Keep this module focused on packaging test fixtures.

use crate::client::SyncClientConfig;
use crate::client::client_tests::{cfg, sqlite};
use crate::crypto::SyncCipherKey;
use clio_store::{AssocEdgeRecord, AssocKind};
use clio_types::{
    BeliefObject, ConfidenceEntry, EpisodicType, EpistemicKind, FailureRecord, ItemKind,
    MemoryItem, PersonaPreference, PersonaStableEntry, SourceType, TaskRecord,
};

/// The one bank every packaging fixture is scoped to.
pub(crate) const BANK: &str = "bh";
/// A caller-chosen task identifier.
pub(crate) const TASK_ID: &str = "synthetic-task-id-42";
/// A caller-chosen persona attribute key.
pub(crate) const ATTR_NAME: &str = "synthetic_attr_name";
/// A failure record id, which embeds a caller task identifier.
pub(crate) const FAILURE_ID: &str = "synthetic-failure-record-7";
/// A caller-chosen relationship label on an explicit edge.
pub(crate) const RELATIONSHIP: &str = "synthetic_relationship_label";
/// One stamp, so no fixture reads the clock.
pub(crate) const STAMP: &str = "2026-01-01T00:00:00Z";

/// A client config holding one cipher key built from `secret`, pointed at an
/// address nothing listens on: packaging never opens a socket.
pub(crate) fn sealed_cfg(secret: &str) -> SyncClientConfig {
    cfg(
        "dev-a",
        &sqlite(),
        "127.0.0.1:1",
        vec![SyncCipherKey::from_secret(secret)],
    )
}

/// A client config with no cipher, which is plaintext mode.
pub(crate) fn plain_cfg() -> SyncClientConfig {
    cfg("dev-a", &sqlite(), "127.0.0.1:1", Vec::new())
}

pub(crate) fn item(id: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: BANK.into(),
        subject_id: "s1".into(),
        kind: ItemKind::Semantic,
        category: None,
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.6),
        admission_score: Some(0.5),
        source_ref: None,
        snapshot: None,
        gist: Some("g".into()),
        valid_from: STAMP.into(),
        valid_until: None,
        tx_from: STAMP.into(),
        tx_until: None,
        created_at: STAMP.into(),
        updated_at: STAMP.into(),
        context: None,
    }
}

/// The episodic carrier item a task or failure record rides on.
pub(crate) fn carrier(id: &str) -> MemoryItem {
    MemoryItem {
        episodic_type: Some(EpisodicType::Task),
        ..item(id)
    }
}

pub(crate) fn task_record(task_id: &str, row_id: &str) -> TaskRecord {
    TaskRecord {
        id: row_id.into(),
        bank_id: BANK.into(),
        item_id: "car-task".into(),
        task_id: task_id.into(),
        subject_id: "s1".into(),
        version: 1,
        definition: "d".into(),
        steps: vec![],
        status: "open".into(),
        evidence_ref: None,
        context: None,
        admission_score: 0.5,
        created_at: STAMP.into(),
    }
}

pub(crate) fn failure_record(id: &str, task_id: &str) -> FailureRecord {
    FailureRecord {
        id: id.into(),
        bank_id: BANK.into(),
        item_id: "car-fail".into(),
        task_id: task_id.into(),
        subject_id: "s1".into(),
        attempt_n: 1,
        what_failed: "boom".into(),
        lesson: "l".into(),
        lesson_tokens: 1,
        evidence_ref: None,
        context: None,
        admission_score: 0.5,
        epistemic_kind: EpistemicKind::Belief,
        created_at: STAMP.into(),
    }
}

pub(crate) fn edge(id: &str, rel: &str) -> AssocEdgeRecord {
    AssocEdgeRecord {
        id: id.into(),
        bank_id: BANK.into(),
        source_id: "i-1".into(),
        target_id: "i-2".into(),
        assoc_kind: AssocKind::Explicit,
        relationship: Some(rel.into()),
        weight: 0.5,
        last_updated_at: "2026-01-01T02:00:00Z".into(),
    }
}

pub(crate) fn stable_entry(key: &str) -> PersonaStableEntry {
    PersonaStableEntry {
        bank_id: BANK.into(),
        key: key.into(),
        value: "terse".into(),
        confidence: None,
        source: "explicit".into(),
        source_ref: None,
        context: None,
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
        valid_until: None,
    }
}

pub(crate) fn preference(key: &str) -> PersonaPreference {
    PersonaPreference {
        bank_id: BANK.into(),
        key: key.into(),
        value: 0.4,
        trend: 0.1,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: None,
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
    }
}

pub(crate) fn belief(id: &str) -> BeliefObject {
    BeliefObject {
        id: id.into(),
        bank_id: BANK.into(),
        subject_id: "s1".into(),
        proposition: "p".into(),
        context: None,
        context_as_of: None,
        confidence_history: vec![
            ConfidenceEntry::new("2026-01-01", 0.7, "ev", SourceType::UserStated).expect("entry"),
        ],
        created_at: STAMP.into(),
        updated_at: STAMP.into(),
    }
}
