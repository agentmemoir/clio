// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the test-only fixtures of the scope-ledger
//! tests: a file-backed SQLite store holding one row class per subject-link
//! class, the resolved ledger paths, and the live facts measured from that
//! store file.
//!
//! ## Owns
//! - `LedgerFixture`, one store with the selected, other, shared, and unknown
//!   link classes written through the store's own public write paths
//! - The fixture ids every class and record is named by
//! - `FixtureDir`, the test-owned directory every fixture path lives under
//!
//! ## Does not own
//! - The ledger itself (`inventory.rs`) and its assertions (`inventory_tests.rs`)
//! - The import harness (`import_fixture.rs`), reused only for its entry builder
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::erase::{EraseArgs, EraseVecSink, erase_request};
use crate::inventory::{InventoryFacts, InventoryInput, InventoryPaths};
use crate::test_unique::unique;
use clio_config::kms_path::KMS_SIBLING_SUFFIX;
use clio_store::{
    AssocEdgeRecord, AssocKind, BeliefStore, ComplianceStore, DekProvider, HistoryStore,
    LocalDevKms, PersonaStore, SqliteStore, Store, sqlite_file_state,
};
use clio_types::{
    Actor, Bank, BeliefObject, EpisodicType, EpistemicKind, FailureRecord, ItemKind, MemoryItem,
    OpContext, PersonaPreference, PersonaStableEntry, SemanticCategory, TaskRecord, TripleRecord,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The selected-subject class: a direct `subject_id` link on an item, a task
/// record, a failure record, and a belief.
pub(crate) const SELECTED: &str = "inv-fx-selected";
/// The other-subject class: the same four records for a second subject, which
/// the selected subject must not see.
pub(crate) const OTHER: &str = "inv-fx-other";
/// The shared class: an association edge between the two subjects' items and a
/// bank-scoped persona entry and preference, none of which has a subject column.
pub(crate) const SHARED: &str = "inv-fx-shared";
/// The unknown-link class: a subject erased through the verified erasure path,
/// whose only remaining row names it by a one-way hash.
pub(crate) const UNKNOWN: &str = "inv-fx-unknown";

/// The actor label every fixture write is stamped with.
pub(crate) const ACTOR: &str = "harness";
/// Timestamp every fixture row is written with.
pub(crate) const TS: &str = "2026-06-01T00:00:00Z";

/// One fixture id inside a class, e.g. `inv-fx-selected-item`.
pub(crate) fn fixture_id(class: &str, role: &str) -> String {
    format!("{class}-{role}")
}

/// A test-owned directory removed when the fixture drops.
pub(crate) struct FixtureDir(PathBuf);

impl FixtureDir {
    /// Create the directory under the system temporary root.
    pub(crate) fn create(name: &str) -> Self {
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&path).expect("create the fixture directory");
        Self(path)
    }

    /// The directory path.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for FixtureDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A file-backed store holding one row class per subject-link class, plus the
/// paths and facts one ledger run needs.
pub(crate) struct LedgerFixture {
    /// The store the fixture rows were written through. Declared first so it
    /// drops, releasing its store lock, before the directory below is removed.
    pub(crate) store: SqliteStore,
    /// Bank holding the selected, other, and shared classes.
    pub(crate) bank: String,
    /// Bank holding the erased subject of the unknown-link class.
    pub(crate) erased_bank: String,
    /// The store file the live facts are measured from.
    pub(crate) db_path: PathBuf,
    /// The per-store key file path the ledger reports; a placeholder file,
    /// because the fixture's `LocalDevKms` is in memory.
    pub(crate) kms_path: PathBuf,
    /// The backup archives root the ledger reports.
    pub(crate) backup_dir: PathBuf,
    /// The directory every fixture path lives under.
    dir: FixtureDir,
}

impl LedgerFixture {
    /// Write one row class per subject-link class into a fresh file-backed
    /// store, using the store's own public write paths.
    pub(crate) fn create() -> Self {
        let dir = FixtureDir::create(&unique("inv-fx"));
        let db_path = dir.path().join("store").join("clio.db");
        let kms_path = PathBuf::from(format!("{}{KMS_SIBLING_SUFFIX}", db_path.display()));
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).expect("create the backup archives root");
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        let store = SqliteStore::open(&db_path, kms).expect("open the fixture store");
        std::fs::write(&kms_path, "{}\n").expect("write the key file placeholder");
        let bank = unique("inv-fx-bank");
        let erased_bank = unique("inv-fx-erased-bank");
        write_direct_link_class(&store, &bank, SELECTED);
        write_direct_link_class(&store, &bank, OTHER);
        write_shared_class(&store, &bank);
        write_hashed_link_class(&store, &erased_bank);
        Self {
            store,
            bank,
            erased_bank,
            db_path,
            kms_path,
            backup_dir,
            dir,
        }
    }

    /// The ledger paths for this fixture.
    pub(crate) fn paths(&self) -> InventoryPaths {
        InventoryPaths {
            db_path: self.db_path.clone(),
            kms_path: self.kms_path.clone(),
            backup_dir: self.backup_dir.clone(),
        }
    }

    /// A ledger input with the given declared copies and peer fact, measured
    /// from the store file at call time.
    pub(crate) fn input(
        &self,
        declared_copies: Vec<PathBuf>,
        peers_enabled: Option<bool>,
    ) -> InventoryInput {
        let facts = sqlite_file_state(&self.db_path).expect("measure the fixture store");
        InventoryInput {
            facts: InventoryFacts::Sqlite(facts),
            paths: self.paths(),
            declared_copies,
            peers_enabled,
        }
    }

    /// A path for a caller-held copy: under the fixture directory but outside
    /// the store's own directory, the way a caller's export is.
    pub(crate) fn caller_path(&self, name: &str) -> PathBuf {
        self.dir.path().join("caller").join(name)
    }

    /// The fixture directory itself, for the files a test writes itself.
    pub(crate) fn dir(&self) -> &Path {
        self.dir.path()
    }
}

/// One semantic fixture item for a subject.
pub(crate) fn semantic_item(id: &str, bank: &str, subject: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::parse("schema").unwrap()),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({ "checklist": "smoke" })),
        gist: Some("the deploy runs the smoke task checklist".into()),
        valid_from: TS.into(),
        valid_until: None,
        tx_from: TS.into(),
        tx_until: None,
        created_at: TS.into(),
        updated_at: TS.into(),
        context: None,
    }
}

/// One episodic carrier item for a subject.
pub(crate) fn carrier(id: &str, bank: &str, subject: &str, tag: EpisodicType) -> MemoryItem {
    let mut item = semantic_item(id, bank, subject);
    item.kind = ItemKind::Episodic;
    item.category = None;
    item.episodic_type = Some(tag);
    item.confidence = None;
    item
}

/// One SPO triple carried by the item named by `class`'s triple id: the
/// triple names no data subject, so its only subject link runs through the
/// carrier item, never through the SPO subject.
fn write_fixture_triple(
    store: &SqliteStore,
    class: &str,
    bank: &str,
    carrier_subject: &str,
    spo: &str,
    predicate: &str,
    object: &str,
) {
    let carrier_item = carrier(
        &fixture_id(class, "triple"),
        bank,
        carrier_subject,
        EpisodicType::Triple,
    );
    let edge = TripleRecord {
        context: None,
        id: carrier_item.id.clone(),
        bank_id: bank.into(),
        item_id: carrier_item.id.clone(),
        subject: spo.into(),
        predicate: predicate.into(),
        object: object.into(),
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: Some(0.8),
        valid_from: TS.into(),
        valid_until: None,
        tx_from: TS.into(),
        tx_until: None,
        snapshot_ref: None,
        gist_ref: None,
        created_at: TS.into(),
        updated_at: None,
    };
    store
        .commit_triple_add(&carrier_item, &edge, false, ACTOR)
        .expect("write the fixture triple");
}

/// An item, a task record, a failure record, and a belief that all carry one
/// data subject in a direct, non-null `subject_id` column.
pub(crate) fn write_direct_link_class(store: &SqliteStore, bank: &str, subject: &str) {
    let mut direct = semantic_item(&fixture_id(subject, "item"), bank, subject);
    direct.admission_score = Some(0.9);
    store
        .create_memory_item(&direct, ACTOR)
        .expect("write the fixture item");

    let task_item_id = fixture_id(subject, "task");
    let task_item = carrier(&task_item_id, bank, subject, EpisodicType::Task);
    let task = TaskRecord {
        context: None,
        id: fixture_id(subject, "task-record"),
        bank_id: bank.into(),
        item_id: task_item_id,
        task_id: fixture_id(subject, "task"),
        subject_id: subject.into(),
        version: 1,
        definition: "fix the build".into(),
        steps: vec!["edit".into()],
        status: "in_progress".into(),
        evidence_ref: None,
        admission_score: 0.6,
        created_at: TS.into(),
    };
    store
        .commit_task_upsert(&task_item, &task, ACTOR)
        .expect("write the fixture task");

    let failure_item_id = fixture_id(subject, "failure");
    let mut failure_item = carrier(&failure_item_id, bank, subject, EpisodicType::Failure);
    failure_item.epistemic_kind = EpistemicKind::Belief;
    failure_item.source_type = Some(clio_types::SourceType::AgentInferred);
    let failure = FailureRecord {
        context: None,
        id: fixture_id(subject, "failure-record"),
        bank_id: bank.into(),
        item_id: failure_item_id,
        task_id: task.task_id,
        subject_id: subject.into(),
        attempt_n: 1,
        what_failed: "compile error".into(),
        lesson: "run the check first".into(),
        lesson_tokens: 5,
        evidence_ref: None,
        admission_score: 0.6,
        epistemic_kind: EpistemicKind::Belief,
        created_at: TS.into(),
    };
    store
        .commit_failure_record(&failure_item, &failure, ACTOR)
        .expect("write the fixture failure");

    let belief = BeliefObject {
        context: None,
        context_as_of: None,
        id: fixture_id(subject, "belief"),
        bank_id: bank.into(),
        subject_id: subject.into(),
        proposition: format!("the deploy runs the smoke task checklist for {subject}"),
        confidence_history: vec![crate::import_fixture::entry("2026-01-01", 0.7, "turn_1")],
        created_at: TS.into(),
        updated_at: TS.into(),
    };
    store
        .create_belief(&belief, ACTOR)
        .expect("write the fixture belief");

    // One SPO triple carried by its own item.
    write_fixture_triple(
        store,
        subject,
        bank,
        subject,
        "attribute",
        &format!("attribute-{subject}"),
        &format!("the {subject} attribute value"),
    );
}

/// An association edge between the two subjects' items plus a bank-scoped
/// persona entry and preference: rows that name no data subject at all.
pub(crate) fn write_shared_class(store: &SqliteStore, bank: &str) {
    store
        .put_assoc_edge(&AssocEdgeRecord {
            id: fixture_id(SHARED, "edge"),
            bank_id: bank.into(),
            source_id: fixture_id(SELECTED, "item"),
            target_id: fixture_id(OTHER, "item"),
            assoc_kind: AssocKind::Explicit,
            relationship: Some("relates".into()),
            weight: 0.4,
            last_updated_at: TS.into(),
        })
        .expect("write the fixture assoc edge");
    store
        .put_stable_entry(
            &PersonaStableEntry {
                context: None,
                bank_id: bank.into(),
                key: fixture_id(SHARED, "stable-key"),
                value: "concise".into(),
                confidence: Some(0.8),
                source: "explicit".into(),
                source_ref: Some("turn_1".into()),
                admission_score: 0.8,
                as_of: TS.into(),
                valid_until: None,
            },
            ACTOR,
        )
        .expect("write the fixture stable entry");
    store
        .put_preference_state(
            &PersonaPreference {
                context: None,
                bank_id: bank.into(),
                key: fixture_id(SHARED, "preference-key"),
                value: 0.7,
                trend: 0.6,
                confidence: None,
                source: "explicit".into(),
                source_ref: None,
                admission_score: 0.7,
                as_of: TS.into(),
            },
            ACTOR,
        )
        .expect("write the fixture preference");

    // A triple whose SPO subject is the shared marker, never a data-subject id.
    write_fixture_triple(
        store,
        SHARED,
        bank,
        SELECTED,
        SHARED,
        "attribute-shared",
        "the shared attribute value",
    );
}

/// A subject erased through the verified erasure path, so the only row that
/// still names it is a compliance tombstone carrying a one-way hash.
pub(crate) fn write_hashed_link_class(store: &SqliteStore, bank: &str) {
    store
        .create_memory_item(
            &semantic_item(&fixture_id(UNKNOWN, "item"), bank, UNKNOWN),
            ACTOR,
        )
        .expect("write the erased subject's item");
    // One triple for the erased subject, purged by the erasure below.
    write_fixture_triple(
        store,
        UNKNOWN,
        bank,
        UNKNOWN,
        "attribute",
        "attribute-erased",
        "the erased subject's attribute value",
    );
    let request_id = fixture_id(UNKNOWN, "request");
    let sink = EraseVecSink::default();
    let ctx = OpContext::new(Bank::new(bank.to_owned()), Actor::Harness);
    // A fresh salt per run: no test asserts the tombstone hash value.
    let salt = rand::random::<[u8; 16]>();
    let journal_dir = FixtureDir::create(&unique("inv-fx-erase-journal"));
    let journal = journal_dir.path().join("erasure-journal.json");
    let args = EraseArgs {
        store,
        journal_path: &journal,
        maint: None,
        ctx: &ctx,
        subject_id: UNKNOWN,
        legal_basis: "GDPR_ART_17",
        request_id: &request_id,
        reason_code: None,
        confirm: true,
        salt: &salt,
        timestamp: TS,
        sink: &sink,
    };
    erase_request(&args).expect("erase the fixture subject");
    let tombstones = store
        .list_tombstones(bank)
        .expect("read the fixture tombstones");
    assert!(
        tombstones.iter().any(|row| !row.subject_hash.is_empty()),
        "the erase path must leave a hashed-link tombstone: {tombstones:?}"
    );
    assert!(
        store
            .get_triple(bank, &fixture_id(UNKNOWN, "triple"))
            .expect("read the erased subject's triple")
            .is_none(),
        "the erasure purges the erased subject's triples"
    );
}
