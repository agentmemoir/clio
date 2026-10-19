// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the every-family fixture of the cross-path
//! compliance proof: one row of each stored family the scope ledger names,
//! written through the production write paths of the store.
//!
//! ## Owns
//! - `seed_sqlite`, `seed_pg`, and `seed_every_family`
//! - The one row of each family the subject under test carries
//!
//! ## Does not own
//! - The ledger's per-family expectations and the manifest check (see
//!   `family_ledger`)
//! - The plain item fixture, the scratch directory, and the store openers (see
//!   `fixtures`)
//!
//! ## Boundary
//!
//! Changes to which rows the fixture writes should normally require changes
//! here. Changes to what the export must report should not.
//!
//! Keep this module focused on the every-family rows the fixture writes.

use crate::family_ledger::FamilyFixture;
use crate::fixtures::{
    BANK, CONTROL_ITEM, CONTROL_MARKER, CONTROL_SUBJECT, DENIED_ITEM, DENIED_MARKER,
    DENIED_SUBJECT, NOW, open_pg, open_sqlite, write_item,
};
use clio::store::{AssocEdgeRecord, AssocKind, BeliefStore, HistoryStore, PersonaStore, Store};
use clio::{
    BeliefObject, ConfidenceEntry, EpisodicType, EpistemicKind, ItemKind, MemoryItem,
    SemanticCategory, SourceType, TripleRecord,
};
use clio_types::{FailureRecord, PersonaPreference, PersonaStableEntry, TaskRecord};
use serde_json::json;
use std::path::Path;

/// The actor every fixture write is attributed to.
const ACTOR: &str = "harness";

/// Seed the every-family fixture into a fresh SQLite store and report what it
/// wrote.
pub fn seed_sqlite(db: &Path, kms: &Path) -> FamilyFixture {
    std::fs::create_dir_all(db.parent().expect("the db has a parent")).expect("create the db dir");
    let store = open_sqlite(db, kms);
    seed_every_family(&store, BANK)
}

/// Seed the every-family fixture into a fresh Postgres store and report what
/// it wrote.
pub fn seed_pg(url: &str, kms: &Path) -> FamilyFixture {
    let store = open_pg(url, kms);
    seed_every_family(&store, BANK)
}

/// Seed one row of every stored family the ledger names, and report the counts
/// and the ledger omissions the export must then carry.
///
/// The subject under test gets one row of every family the ledger links by a
/// stored subject column. The other subject gets the one item the readability
/// control reads back. The two families the ledger reports as inseparable get
/// their bank-scoped row too, so the store holds a row in them as well. The
/// tombstone family stores only a one-way subject hash, so it holds no row
/// until the erasure writes one.
pub fn seed_every_family<S>(store: &S, bank: &str) -> FamilyFixture
where
    S: Store + HistoryStore + BeliefStore + PersonaStore,
{
    write_item(store, CONTROL_ITEM, CONTROL_SUBJECT, CONTROL_MARKER);
    write_subject_item(store, bank, DENIED_SUBJECT, DENIED_ITEM, DENIED_MARKER);
    write_triple(store, bank, DENIED_SUBJECT);
    write_task(store, bank, DENIED_SUBJECT);
    write_failure(store, bank, DENIED_SUBJECT);
    write_belief(store, bank, DENIED_SUBJECT);
    write_assoc_edge(store, bank);
    write_persona(store, bank);
    FamilyFixture::expected()
}

/// The subject's one semantic item: the family the ledger links by its own
/// stored subject column.
fn write_subject_item<S: Store>(store: &S, bank: &str, subject: &str, id: &str, gist: &str) {
    store
        .create_memory_item(
            &MemoryItem {
                id: id.to_owned(),
                bank_id: bank.to_owned(),
                subject_id: subject.to_owned(),
                kind: ItemKind::Semantic,
                category: Some(SemanticCategory::Schema),
                episodic_type: None,
                epistemic_kind: EpistemicKind::Fact,
                source_type: None,
                confidence: Some(0.8),
                admission_score: Some(0.8),
                source_ref: None,
                snapshot: Some(json!({"entity": "synthetic cross-path subject"})),
                gist: Some(gist.to_owned()),
                valid_from: NOW.into(),
                valid_until: None,
                tx_from: NOW.into(),
                tx_until: None,
                created_at: NOW.into(),
                updated_at: NOW.into(),
                context: None,
            },
            ACTOR,
        )
        .expect("write the fixture item");
}

/// The episodic carrier item one triple, task, or failure row is written
/// under. A carrier is an item of the subject, so it counts in the item family
/// as well.
fn carrier(bank: &str, subject: &str, id: &str, tag: EpisodicType) -> MemoryItem {
    MemoryItem {
        id: id.to_owned(),
        bank_id: bank.to_owned(),
        subject_id: subject.to_owned(),
        kind: ItemKind::Episodic,
        category: None,
        episodic_type: Some(tag),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(json!({"entity": "synthetic cross-path subject"})),
        gist: Some("synthetic cross-path carrier".to_owned()),
        valid_from: NOW.into(),
        valid_until: None,
        tx_from: NOW.into(),
        tx_until: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        context: None,
    }
}

/// One SPO triple carried by the subject's own carrier item. The triple names
/// no data subject, so its only subject link runs through the carrier.
fn write_triple<S: Store>(store: &S, bank: &str, subject: &str) {
    let carrier = carrier(
        bank,
        subject,
        "cross-path-denied-triple-carrier",
        EpisodicType::Triple,
    );
    let edge = TripleRecord {
        id: "cross-path-denied-triple".to_owned(),
        bank_id: bank.to_owned(),
        item_id: carrier.id.clone(),
        subject: "attribute".to_owned(),
        predicate: "attribute".to_owned(),
        object: "the synthetic attribute value".to_owned(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        valid_from: NOW.into(),
        valid_until: None,
        tx_from: NOW.into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        context: None,
        created_at: NOW.into(),
        updated_at: None,
    };
    store
        .commit_triple_add(&carrier, &edge, false, ACTOR)
        .expect("write the fixture triple");
}

/// One task-history row, written under the subject's own carrier item.
fn write_task<S: HistoryStore>(store: &S, bank: &str, subject: &str) {
    let carrier = carrier(
        bank,
        subject,
        "cross-path-denied-task-carrier",
        EpisodicType::Task,
    );
    let task = TaskRecord {
        id: "cross-path-denied-task-record".to_owned(),
        bank_id: bank.to_owned(),
        item_id: carrier.id.clone(),
        task_id: "cross-path-denied-task".to_owned(),
        subject_id: subject.to_owned(),
        version: 1,
        definition: "run the cross-path compliance check".to_owned(),
        steps: vec!["export".to_owned(), "erase".to_owned()],
        status: "in_progress".to_owned(),
        evidence_ref: None,
        context: None,
        admission_score: 0.6,
        created_at: NOW.into(),
    };
    store
        .commit_task_upsert(&carrier, &task, ACTOR)
        .expect("write the fixture task");
}

/// One failure-history row, written under the subject's own carrier item.
///
/// The carrier is a belief item, because the store writes the carrier's
/// epistemic kind into the failure row, and a failure row is always a belief.
fn write_failure<S: HistoryStore>(store: &S, bank: &str, subject: &str) {
    let mut carrier = carrier(
        bank,
        subject,
        "cross-path-denied-failure-carrier",
        EpisodicType::Failure,
    );
    carrier.epistemic_kind = EpistemicKind::Belief;
    carrier.source_type = Some(SourceType::AgentInferred);
    let failure = FailureRecord {
        id: "cross-path-denied-failure-record".to_owned(),
        bank_id: bank.to_owned(),
        item_id: carrier.id.clone(),
        task_id: "cross-path-denied-task".to_owned(),
        subject_id: subject.to_owned(),
        attempt_n: 1,
        what_failed: "the cross-path export check".to_owned(),
        lesson: "run the check first".to_owned(),
        lesson_tokens: 5,
        evidence_ref: None,
        context: None,
        admission_score: 0.6,
        epistemic_kind: EpistemicKind::Belief,
        created_at: NOW.into(),
    };
    store
        .commit_failure_record(&carrier, &failure, ACTOR)
        .expect("write the fixture failure");
}

/// One belief object with its first confidence entry: the family the ledger
/// links by the belief's own stored subject column.
fn write_belief<S: BeliefStore>(store: &S, bank: &str, subject: &str) {
    let entry = ConfidenceEntry::new(NOW, 0.7, "cross-path-evidence", SourceType::UserStated)
        .expect("build the confidence entry");
    let belief = BeliefObject {
        context: None,
        context_as_of: None,
        id: "cross-path-denied-belief".to_owned(),
        bank_id: bank.to_owned(),
        subject_id: subject.to_owned(),
        proposition: "the cross-path compliance check holds".to_owned(),
        confidence_history: vec![entry],
        created_at: NOW.into(),
        updated_at: NOW.into(),
    };
    store
        .create_belief(&belief, ACTOR)
        .expect("write the fixture belief");
}

/// One association edge between the two subjects' items: the family the ledger
/// reports as inseparable, because the edge names no data subject.
fn write_assoc_edge<S: Store>(store: &S, bank: &str) {
    store
        .put_assoc_edge(&AssocEdgeRecord {
            id: "cross-path-assoc-edge".to_owned(),
            bank_id: bank.to_owned(),
            source_id: DENIED_ITEM.to_owned(),
            target_id: CONTROL_ITEM.to_owned(),
            assoc_kind: AssocKind::Explicit,
            relationship: Some("relates".to_owned()),
            weight: 0.4,
            last_updated_at: NOW.into(),
        })
        .expect("write the fixture assoc edge");
}

/// One bank-scoped persona entry and preference: the families the ledger
/// reports as inseparable, because they name no data subject. Both carry no
/// context, so the erasure path leaves them alone.
fn write_persona<S: PersonaStore>(store: &S, bank: &str) {
    store
        .put_stable_entry(
            &PersonaStableEntry {
                context: None,
                bank_id: bank.to_owned(),
                key: "cross-path-persona-stable".to_owned(),
                value: "concise".to_owned(),
                confidence: Some(0.8),
                source: "explicit".to_owned(),
                source_ref: Some("cross-path-evidence".to_owned()),
                admission_score: 0.8,
                as_of: NOW.into(),
                valid_until: None,
            },
            ACTOR,
        )
        .expect("write the fixture persona entry");
    store
        .put_preference_state(
            &PersonaPreference {
                context: None,
                bank_id: bank.to_owned(),
                key: "cross-path-persona-preference".to_owned(),
                value: 0.7,
                trend: 0.6,
                confidence: None,
                source: "explicit".to_owned(),
                source_ref: None,
                admission_score: 0.7,
                as_of: NOW.into(),
            },
            ACTOR,
        )
        .expect("write the fixture persona preference");
}
