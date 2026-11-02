// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared apply-test fixtures: the in-memory
//! store with a local dev key provider, item / edge / envelope builders, the
//! engine-level apply runner, and the admission-receipt helper. Every apply
//! test module builds its cases from these, so the fixture shape lives once.
//!
//! ## Owns
//! - The in-memory `SqliteStore` + `LocalDevKms` fixture
//! - Item, association-edge, and mutation builders used by the apply suites
//! - `apply` / `apply_result`: the engine-level runner the suites share
//!
//! ## Does not own
//! - Any case or assertion (the `*_tests` modules own those)
//! - Production apply behavior (see `apply`)
//!
//! ## Boundary
//!
//! Changes to the fixture shape should normally require changes here. Changes
//! to a case should not.
//!
//! Test-only. Never compiled into production builds.

use crate::apply::{ApplyOutcome, apply_mutation};
use crate::protocol::{ENTITY_KIND_ASSOC, ENTITY_KIND_ITEM, OP_UPSERT, SyncContent, SyncMutation};
use clio_store::{AssocEdgeRecord, AssocKind, DekProvider, LocalDevKms, SqliteStore};
use clio_types::{EpistemicKind, ItemKind, MemoryItem, SemanticCategory};
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) fn fixture() -> (Arc<SqliteStore>, Arc<dyn DekProvider>) {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = Arc::new(SqliteStore::open_in_memory(Arc::clone(&kms)).expect("sqlite open"));
    (store, kms)
}

pub(crate) fn item(bank: &str, id: &str, subject: &str, stamp: &str, gist: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::ToolConfig),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.6),
        admission_score: Some(0.5),
        source_ref: Some("ev".into()),
        snapshot: Some(json!({ "k": id })),
        gist: Some(gist.into()),
        valid_from: stamp.into(),
        valid_until: None,
        tx_from: stamp.into(),
        tx_until: None,
        created_at: stamp.into(),
        updated_at: stamp.into(),
        context: None,
    }
}

pub(crate) fn item_value(bank: &str, id: &str, subject: &str, stamp: &str, gist: &str) -> Value {
    serde_json::to_value(item(bank, id, subject, stamp, gist)).expect("serialize item")
}

pub(crate) fn hash_of(content: &Value) -> String {
    let text = serde_json::to_string(content).unwrap_or_default();
    clio_store::sha256_hex(crate::keyed_roles::published_digest_salt(), &text)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn env(
    mid: &str,
    bank: &str,
    kind: &str,
    entity: &str,
    op: &str,
    stamp: &str,
    dev: &str,
    content: Value,
) -> SyncMutation {
    let content_hash = hash_of(&content);
    SyncMutation {
        sync_mutation_id: mid.into(),
        bank: bank.into(),
        entity_kind: kind.into(),
        entity_id: entity.into(),
        op: op.into(),
        transaction_time: stamp.into(),
        device_id: dev.into(),
        content: SyncContent::Plain(content),
        content_hash,
        admission_receipt: None,
    }
}

pub(crate) fn upsert(mid: &str, bank: &str, stamp: &str, dev: &str, gist: &str) -> SyncMutation {
    env(
        mid,
        bank,
        ENTITY_KIND_ITEM,
        "i-1",
        OP_UPSERT,
        stamp,
        dev,
        item_value(bank, "i-1", "s1", stamp, gist),
    )
}

pub(crate) fn edge(
    bank: &str,
    id: &str,
    source: &str,
    target: &str,
    stamp: &str,
) -> AssocEdgeRecord {
    AssocEdgeRecord {
        id: id.into(),
        bank_id: bank.into(),
        source_id: source.into(),
        target_id: target.into(),
        assoc_kind: AssocKind::Coactivation,
        relationship: None,
        weight: 0.5,
        last_updated_at: stamp.into(),
    }
}

pub(crate) fn edge_value(bank: &str, id: &str, source: &str, target: &str, stamp: &str) -> Value {
    serde_json::to_value(edge(bank, id, source, target, stamp)).expect("serialize edge")
}

pub(crate) fn assoc(
    mid: &str,
    bank: &str,
    entity: &str,
    op: &str,
    stamp: &str,
    dev: &str,
    content: Value,
) -> SyncMutation {
    env(
        mid,
        bank,
        ENTITY_KIND_ASSOC,
        entity,
        op,
        stamp,
        dev,
        content,
    )
}

pub(crate) fn apply(
    store: &SqliteStore,
    kms: Option<&dyn DekProvider>,
    m: &SyncMutation,
) -> ApplyOutcome {
    apply_result(store, kms, m).expect("apply succeeds")
}

pub(crate) fn apply_result(
    store: &SqliteStore,
    kms: Option<&dyn DekProvider>,
    m: &SyncMutation,
) -> Result<ApplyOutcome, clio_types::AmError> {
    // A fixture stamp this engine cannot place is a fixture defect, so
    // `test_engine` panics instead of pinning the clock to the epoch.
    let engine = crate::apply::apply_freshness::test_engine(
        store,
        store,
        "node-1",
        kms,
        &m.transaction_time,
    );
    run_apply(&engine, m)
}

/// Apply with the engine clock pinned to an explicit epoch second, for a case
/// whose fixture stamp is deliberately unplaceable and must still be applied.
pub(crate) fn apply_at_epoch(
    store: &SqliteStore,
    kms: Option<&dyn DekProvider>,
    m: &SyncMutation,
    now: i64,
) -> ApplyOutcome {
    let engine = crate::apply::apply_freshness::test_engine_at(store, store, "node-1", kms, now);
    run_apply(&engine, m).expect("apply succeeds")
}

fn run_apply(
    engine: &crate::apply::ApplyEngine<'_>,
    m: &SyncMutation,
) -> Result<ApplyOutcome, clio_types::AmError> {
    let envelope = serde_json::to_string(m).expect("serialize envelope");
    apply_mutation(engine, m, &envelope)
}

pub(crate) fn receipt(m: &SyncMutation, score: f64) -> SyncMutation {
    SyncMutation {
        admission_receipt: Some(json!({ "admitted": true, "score": score })),
        ..m.clone()
    }
}
