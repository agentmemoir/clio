// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the test-only Postgres half of the
//! scope-ledger tests: the shared test-database address and a
//! search-path-scoped store holding one row class per subject-link class.
//!
//! ## Owns
//! - `open_pg_ledger_fixture`: the shared-Postgres lock, the scoped schema, the
//!   store, and the selected, other, shared, and hashed link-class writes
//! - `pg_base_url` and `pg_checked_url`, the shared-database address readers
//!
//! The credential readers and the fail-closed gate live in
//! `inventory_pg_credentials_tests`, and the opt-out decision lives in
//! `clio_store::pg_skip`; this module re-exports the pair gate for the callers
//! that already import through it.
//!
//! ## Does not own
//! - The ledger and its row plan (`inventory.rs`, `inventory_rows.rs`)
//! - The SQLite fixture (`inventory_fixture.rs`): its class names, ids, and the
//!   shared `FixtureDir` guard are reused; only its writers are SQLite-typed
//! - The fixture triples and their assertions (`inventory_pg_triples_tests.rs`,
//!   mounted as `inventory_pg_tests::triples`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the shared Postgres test fixtures.

use crate::erase::{EraseArgs, EraseVecSink, erase_request};
use crate::inventory::InventoryPaths;
use crate::inventory_fixture::{FixtureDir, OTHER, SELECTED, SHARED, UNKNOWN, fixture_id};
use crate::test_unique::unique;
use clio_config::db_url_env::database_url_from_process_env;
use clio_config::kms_path::KMS_SIBLING_SUFFIX;
use clio_store::{
    AssocEdgeRecord, AssocKind, BeliefStore, ComplianceStore, HistoryStore, PersonaStore,
    PgTestGuard, PostgresStore, ScopedSchema, Store, lock_pg_test, postgres_target_state,
    probe_postgres,
};
use clio_types::{
    Actor, AmError, Bank, BeliefObject, EpisodicType, EpistemicKind, ErrorCode, FailureRecord,
    ItemKind, MemoryItem, OpContext, PersonaPreference, PersonaStableEntry, SemanticCategory,
    TaskRecord,
};
use std::path::{Path, PathBuf};

/// The credential readers and gate, re-exported so the ledger suites keep
/// importing their helpers through this module.
pub(crate) use crate::inventory_pg_credentials_tests::{
    pg_gate_or_panic, url_password, url_userinfo,
};

/// The actor label every fixture write is stamped with.
const ACTOR: &str = "harness";
/// Timestamp every fixture row is written with.
const TS: &str = "2026-06-01T00:00:00Z";
/// The Compose stack's address, the fallback the store's own Postgres tests use.
const COMPOSE_URL: &str = "postgres://clio:clio@127.0.0.1:34310/clio";

/// The shared test database's URL: `CLIO_DATABASE_URL` when set, else the
/// Compose stack's address. The same resolution the store's own shared-Postgres
/// tests and its test lock use.
#[must_use]
pub(crate) fn pg_base_url() -> String {
    database_url_from_process_env().unwrap_or_else(|_| COMPOSE_URL.to_owned())
}

/// The shared-Postgres URL re-parsed: a bad URL is a harness fault, not a skip.
pub(crate) fn pg_checked_url() -> String {
    let url = pg_base_url();
    let ok = clio_store::parse_connection(&url).is_ok() && url_password(&url).is_some();
    assert!(
        ok,
        "CLIO_DATABASE_URL is misconfigured (bad URL or no password; a harness fault)"
    );
    url
}

/// The ledger paths one Postgres fixture reports, under `dir`.
///
/// `db_path` is a placeholder, because Postgres holds no store file; the key
/// file is a placeholder too, because the fixture's `LocalDevKms` is in memory,
/// the same shape the SQLite fixture's paths have.
pub(crate) fn ledger_paths(dir: &Path) -> InventoryPaths {
    let db_path = dir.join("store").join("store.db");
    let kms_path = PathBuf::from(format!("{}{KMS_SIBLING_SUFFIX}", db_path.display()));
    InventoryPaths {
        db_path,
        kms_path,
        backup_dir: dir.join("backups"),
    }
}

/// A store in a private schema holding one row class per subject-link class,
/// plus the ledger paths one Postgres run reports.
pub(crate) struct PgLedgerFixture {
    /// The store the fixture rows were written through. Declared before the
    /// schema so it drops first, releasing its connections before the schema
    /// goes away.
    pub(crate) store: PostgresStore,
    /// Bank holding the selected, other, and shared classes.
    pub(crate) bank: String,
    /// Bank holding the erased subject of the hashed-link class.
    pub(crate) erased_bank: String,
    /// The private schema; dropped after the store.
    schema: ScopedSchema,
    /// The directory every fixture path lives under; removed when this drops.
    dir: FixtureDir,
}

impl PgLedgerFixture {
    /// A fixture from its parts, writing the placeholder key file the ledger
    /// paths name.
    fn new(
        store: PostgresStore,
        schema: ScopedSchema,
        bank: String,
        erased_bank: String,
        dir: FixtureDir,
    ) -> Self {
        let paths = ledger_paths(dir.path());
        if let Some(parent) = paths.kms_path.parent() {
            std::fs::create_dir_all(parent).expect("create the key file's directory");
        }
        std::fs::write(&paths.kms_path, "{}\n").expect("write the key file placeholder");
        std::fs::create_dir_all(&paths.backup_dir).expect("create the backup archives root");
        Self {
            store,
            bank,
            erased_bank,
            schema,
            dir,
        }
    }

    /// The `search_path`-scoped store URL. Never printed: it carries the base
    /// URL's userinfo.
    #[must_use]
    pub(crate) fn scoped_url(&self) -> &str {
        self.schema.url()
    }

    /// The read-only target measurement of this fixture's store.
    #[must_use]
    pub(crate) fn target_state(&self) -> clio_store::PostgresTargetState {
        postgres_target_state(self.scoped_url()).expect("measure the scoped Postgres target")
    }

    /// The ledger paths for this fixture.
    #[must_use]
    pub(crate) fn paths(&self) -> InventoryPaths {
        ledger_paths(self.dir.path())
    }

    /// Read one catalog value (the first column of the first row) from the
    /// scoped schema, as text.
    #[must_use]
    pub(crate) fn catalog_value(&self, sql: &str) -> Option<String> {
        self.schema
            .query_value(sql)
            .expect("read a catalog value from the scoped schema")
    }
}

/// Open the shared-Postgres fixture: the lock first, then a private schema and
/// a store holding one row class per subject-link class.
///
/// The guard is returned to the caller rather than bound here: a helper that
/// holds the lock locally releases it on return, and the caller's queries would
/// then run unguarded.
///
/// # Errors
/// The shared database is unreachable; the caller routes the error through the
/// fail-closed gate, which prints the credential-safe note under the opt-out and
/// panics otherwise. Fixture-write failures panic, because a reachable database
/// that cannot hold the fixture rows is a test failure, not a skip.
pub(crate) fn open_pg_ledger_fixture() -> Result<(PgTestGuard, PgLedgerFixture), AmError> {
    let guard = lock_pg_test();
    let base = pg_base_url();
    probe_postgres(&base)
        .map_err(|detail| AmError::new(ErrorCode::Internal, format!("postgres probe: {detail}")))?;
    let schema = ScopedSchema::new(&base);
    let store = schema.open();
    let dir = FixtureDir::create(&unique("inv-pg-fx"));
    let bank = unique("inv-pg-bank");
    let erased_bank = unique("inv-pg-erased-bank");
    write_direct_link_class(&store, &bank, SELECTED);
    write_direct_link_class(&store, &bank, OTHER);
    write_shared_class(&store, &bank);
    crate::inventory_pg_tests::triples::write_triple_class(&store, &bank, &erased_bank);
    write_hashed_link_class(&store, &erased_bank);
    Ok((
        guard,
        PgLedgerFixture::new(store, schema, bank, erased_bank, dir),
    ))
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

/// An item, a task record, a failure record, and a belief that all carry one
/// data subject in a direct, non-null `subject_id` column.
pub(crate) fn write_direct_link_class(store: &PostgresStore, bank: &str, subject: &str) {
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
}

/// An association edge between the two subjects' items plus a bank-scoped
/// persona entry and preference: rows that name no data subject at all.
pub(crate) fn write_shared_class(store: &PostgresStore, bank: &str) {
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
}

/// A subject erased through the verified erasure path, so the only row that
/// still names it is a compliance tombstone carrying a one-way hash.
pub(crate) fn write_hashed_link_class(store: &PostgresStore, bank: &str) {
    store
        .create_memory_item(
            &semantic_item(&fixture_id(UNKNOWN, "item"), bank, UNKNOWN),
            ACTOR,
        )
        .expect("write the erased subject's item");
    let request_id = fixture_id(UNKNOWN, "request");
    let sink = EraseVecSink::default();
    let ctx = OpContext::new(Bank::new(bank.to_owned()), Actor::Harness);
    // A fresh salt per run: no test asserts the tombstone hash value.
    let salt = rand::random::<[u8; 16]>();
    // The journal lives in a Drop-guarded directory, so it is removed with the
    // fixture instead of being left behind under the shared temp root.
    let journal_dir = FixtureDir::create(&unique("inv-pg-erase-journal"));
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
}
