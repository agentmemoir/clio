// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the test-only fixture of the subject-family
//! mapping tests: a file-backed SQLite store holding one row class per
//! subject-link class, the KMS that seals it, the resolved ledger paths, and
//! the live facts measured from the store file.
//!
//! ## Owns
//! - `MapFixture`, the store the four link classes are written through, with
//!   the KMS `Arc` a restart reopens under
//! - `restart`, which drops the store and reopens the same file with the same
//!   KMS
//! - Seeding the families the store can write through public paths: the
//!   sync-state tables, the bank-level persona-erase tombstone, and the
//!   local event path
//! - The pinned per-family count table the mapping tests assert against
//! - The measured facts and the ledger paths one mapping run needs
//!
//! ## Does not own
//! - The class writers, their fixture ids, and the shared fixture directory
//!   (`inventory_fixture.rs`, reused)
//! - The mapping (`inventory_map.rs`) and its assertions
//!   (`inventory_map_tests.rs`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on the mapping test fixture.

use crate::inventory::{InventoryFacts, InventoryPaths};
use crate::inventory_fixture::{
    ACTOR, FixtureDir, OTHER, SELECTED, TS, write_direct_link_class, write_hashed_link_class,
    write_shared_class,
};
use crate::test_unique::unique;
use clio_config::kms_path::KMS_SIBLING_SUFFIX;
use clio_store::{
    DeadLetterStatus, DekProvider, LocalDevKms, LocalEventStore, SqliteStore, SyncConflictRow,
    SyncDeadLetterRow, SyncJournalRow, SyncStore, sqlite_file_state,
};
use std::path::PathBuf;
use std::sync::Arc;

/// The fixture store's exact per-family counts and whether the family names
/// an omission: (family, selected, omitted, has omission).
pub(crate) const FIXTURE_COUNTS: &[(&str, u64, u64, bool)] = &[
    ("items", 5, 6, true),
    ("triples", 2, 1, true),
    ("assoc_edges", 0, 1, true),
    ("persona_stable", 0, 1, true),
    ("persona_preferences", 0, 1, true),
    ("tasks", 1, 1, true),
    ("failures", 1, 1, true),
    ("beliefs", 1, 3, true),
    ("tombstones", 0, 2, true),
    ("derived_item_structures", 0, 16, true),
    ("bank_structures", 0, 6, true),
    ("events", 0, 16, true),
    ("sync_state", 0, 5, true),
    ("subject_keys", 0, 4, true),
    ("persona_erase_tombstones", 0, 1, true),
    ("legacy_tasks", 0, 0, true),
    ("legacy_failures", 0, 0, true),
    ("legacy_tombstones", 0, 0, true),
    ("archive_experiments", 0, 0, true),
    ("copy_inventory", 0, 0, true),
    ("durable_requests", 0, 0, true),
    ("erasure_adapters", 0, 0, true),
    ("local_erasure_journal", 0, 0, true),
    ("local_events", 0, 1, true),
    ("peer_actions", 0, 0, true),
    ("restore_guards", 0, 0, true),
    ("subject_family_links", 0, 0, true),
    ("store_file", 0, 0, true),
    ("kms_file", 0, 0, true),
    ("backup_archives", 0, 0, true),
    ("export_copies", 0, 0, true),
    ("stderr_events", 0, 0, true),
    ("enabled_peers", 0, 0, true),
    ("restore_routes", 0, 0, true),
    ("erasure_journal", 0, 0, true),
];

/// A file-backed store holding one row class per subject-link class, plus the
/// paths and facts one mapping run needs.
pub(crate) struct MapFixture {
    /// The key provider the store is sealed under; held so a restart reopens
    /// the same file under the same keys.
    pub(crate) kms: Arc<dyn DekProvider>,
    /// The store the fixture rows were written through. Declared before the
    /// directory below so a drop releases its hold before the directory goes.
    pub(crate) store: SqliteStore,
    /// Bank holding the selected, other, and shared classes.
    pub(crate) bank: String,
    /// The store file every measurement reads.
    pub(crate) db_path: PathBuf,
    /// The per-store key file path the ledger reports; a placeholder file,
    /// because the fixture's `LocalDevKms` is in memory.
    pub(crate) kms_path: PathBuf,
    /// The directory every fixture path lives under.
    dir: FixtureDir,
}

impl MapFixture {
    /// Write one row class per subject-link class into a fresh file-backed
    /// store, using the store's own public write paths.
    pub(crate) fn create() -> Self {
        let dir = FixtureDir::create(&unique("inv-map-fx"));
        let db_path = dir.path().join("store").join("clio.db");
        let kms_path = PathBuf::from(format!("{}{KMS_SIBLING_SUFFIX}", db_path.display()));
        let backup_dir = dir.path().join("backups");
        std::fs::create_dir_all(&backup_dir).expect("create the backup archives root");
        let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
        let store = SqliteStore::open(&db_path, Arc::clone(&kms)).expect("open the fixture store");
        std::fs::write(&kms_path, "{}\n").expect("write the key file placeholder");
        let bank = unique("inv-map-bank");
        let erased_bank = unique("inv-map-erased-bank");
        write_direct_link_class(&store, &bank, SELECTED);
        write_direct_link_class(&store, &bank, OTHER);
        write_shared_class(&store, &bank);
        // A persona sync copy in the erased bank: the erasure below purges it and
        // fail-closes that bank with a persona-erase tombstone.
        store
            .journal_append(&SyncJournalRow {
                seq: 1,
                sync_mutation_id: unique("inv-map-persona"),
                bank_id: erased_bank.clone(),
                entity_kind: "persona".into(),
                entity_id: unique("inv-map-persona-entity"),
                op: "upsert".into(),
                transaction_time: TS.into(),
                device_id: ACTOR.into(),
                content_hash: "harness-persona-hash".into(),
                content_sealed: Some(r#"{"harness":true}"#.into()),
                client_encrypted: false,
                admission_receipt: None,
                created_at: TS.into(),
            })
            .expect("write the erased bank's persona sync copy");
        write_hashed_link_class(&store, &erased_bank);
        seed_sync_state(&store, &bank, ACTOR);
        Self {
            kms,
            store,
            bank,
            db_path,
            kms_path,
            dir,
        }
    }

    /// Close the store and reopen the same file under the same KMS.
    pub(crate) fn restart(&mut self) {
        self.store = SqliteStore::open(&self.db_path, Arc::clone(&self.kms))
            .expect("reopen the fixture store on the same file");
    }

    /// The ledger paths for this fixture.
    pub(crate) fn paths(&self) -> InventoryPaths {
        InventoryPaths {
            db_path: self.db_path.clone(),
            kms_path: self.kms_path.clone(),
            backup_dir: self.dir.path().join("backups"),
        }
    }

    /// The live-store facts measured from the store file at call time.
    pub(crate) fn facts(&self) -> InventoryFacts {
        InventoryFacts::Sqlite(sqlite_file_state(&self.db_path).expect("measure the fixture store"))
    }
}

/// Seed the sync-state family's four tables through the store's public sync
/// writes, one row each, and one local event through the public local-event
/// path.
fn seed_sync_state(store: &SqliteStore, bank: &str, actor: &str) {
    let mutation = unique("inv-map-mut");
    store
        .journal_append(&SyncJournalRow {
            seq: 1,
            sync_mutation_id: mutation.clone(),
            bank_id: bank.to_owned(),
            entity_kind: "item".into(),
            entity_id: mutation.clone(),
            op: "upsert".into(),
            transaction_time: TS.into(),
            device_id: actor.into(),
            content_hash: "harness-content-hash".into(),
            content_sealed: None,
            client_encrypted: false,
            admission_receipt: None,
            created_at: TS.into(),
        })
        .expect("write the sync journal row");
    store
        .cursor_set(bank, actor, "cursor-token")
        .expect("write the sync cursor");
    store
        .conflict_insert(&SyncConflictRow {
            bank_id: bank.to_owned(),
            entity_kind: "item".into(),
            entity_id: mutation.clone(),
            winner_mutation_id: mutation.clone(),
            loser_mutation_id: unique("inv-map-loser"),
            winner_transaction_time: TS.into(),
            loser_transaction_time: TS.into(),
            winner_device_id: actor.into(),
            loser_device_id: "harness-device-2".into(),
            created_at: TS.into(),
        })
        .expect("write the sync conflict");
    store
        .dead_letter_open(&SyncDeadLetterRow {
            sync_mutation_id: unique("inv-map-dead"),
            bank_id: bank.to_owned(),
            reason_code: "harness".into(),
            detail: "fixture dead letter".into(),
            envelope_json: r#"{"harness":true}"#.into(),
            status: DeadLetterStatus::Open,
            first_seen: TS.into(),
            last_error: None,
            updated_at: None,
        })
        .expect("write the sync dead letter");
    store
        .record_intent(&clio_store::LocalEventIntent {
            action_id: &mutation,
            actor,
            scope: "harness-scope",
            kind: "harness-action",
            event_time: TS,
        })
        .expect("write the local event intent");
}
