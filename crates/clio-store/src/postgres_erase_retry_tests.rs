// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres evidence that a key destroyed
//! straight through the KMS survives a retried, duplicated, or concurrent
//! erasure.
//!
//! ## Owns
//! - The Postgres direct-key-loss, repeat, duplicate, and concurrent erasure
//!   scenarios
//! - The shared scoped-schema Postgres fixture and its skip discipline
//!
//! ## Does not own
//! - The erasure transaction under test (`postgres_erase`)
//! - The crash window at the DEK destroy
//!   (`postgres_erase_crash_window_tests`)
//! - The SQLite twins of the direct-key-loss and later-write scenarios
//!   (`compliance_store_erase_preserve_tests`)
//! - Shared fixtures and assertions (`erase_test_support`)
//!
//! ## Boundary
//!
//! Changes to what the Postgres erasure keeps after a direct key loss or a
//! retried erasure should normally require changes here. Changes to the erasure
//! SQL itself should not require changes here.
//!
//! Keep this module focused on Postgres erasure retry evidence.
//!
//! Test-only. Never compiled into production builds.

use crate::compliance_store::{ComplianceStore, SubjectEraseOutcome};
use crate::db_url::postgres_url_from_env;
use crate::dek::{DekProvider, LocalDevKms};
use crate::erase_test_support::{assert_erased, memory_item, seed, spec, unique};
use crate::migrate::IntoAmError;
use crate::pg_lock::{PgTestGuard, lock_pg_test};
use crate::pg_scoped_schema::ScopedSchema;
use crate::postgres::PostgresStore;
use crate::store::Store;
use crate::sync_store::SyncStore;
use clio_types::{AmError, ErrorCode};
use std::sync::Arc;

/// Every `subject_keys` row for one subject as `(status, dek_ref, shredded_at)`,
/// ordered the same way every run, so a before/after comparison is stable.
fn subject_key_rows(store: &PostgresStore, subject: &str) -> Vec<(String, String, Option<String>)> {
    store
        .with_client(|client| {
            let rows = client
                .query(
                    "SELECT status, dek_ref, shredded_at::text FROM subject_keys
                     WHERE subject_id = $1 ORDER BY status, dek_ref",
                    &[&subject],
                )
                .am("subject key rows query")?;
            Ok(rows
                .iter()
                .map(|r| (r.get(0), r.get(1), r.get(2)))
                .collect())
        })
        .expect("subject key rows readable")
}

/// One private Postgres schema, a store over it, the KMS handle the test keeps,
/// and the lock for the caller's whole body.
///
/// The store and the KMS are readable from the crash-window tests in a sibling
/// module; the schema and the lock stay private to this fixture.
pub(crate) struct PgEnv {
    scoped: ScopedSchema,
    pub(crate) store: PostgresStore,
    pub(crate) kms: Arc<dyn DekProvider>,
    _guard: PgTestGuard,
}

impl PgEnv {
    /// A second store over the same private schema, sharing this KMS, for a
    /// test that needs two connections.
    fn second_store(&self) -> PostgresStore {
        second_store(&self.scoped, &self.kms)
    }
}

/// A second store over the same private schema, sharing this KMS, for a test
/// that needs a second connection opened before the first write.
pub(crate) fn second_store(scoped: &ScopedSchema, kms: &Arc<dyn DekProvider>) -> PostgresStore {
    PostgresStore::open(scoped.url(), Arc::clone(kms)).expect("open second scoped store")
}

/// Probe the shared server and build a private-schema environment.
///
/// `None` only when the operator opted out in `CLIO_SKIP_PG_TESTS`, so a run
/// without Postgres stays green; otherwise an unreachable server panics so the
/// suite cannot pass with zero assertions. Both the probe and the open report
/// through the shared fail-closed helper, which owns the opt-out read.
pub(crate) fn open_pg() -> Option<PgEnv> {
    open_pg_with(|_, kms| kms)
}

/// The same environment over a KMS the caller builds from the private schema
/// and the fixture KMS, so a test can watch the store's own destroy calls.
///
/// `None` and the panic follow the same rules as [open_pg].
pub(crate) fn open_pg_with(
    make_kms: impl FnOnce(&ScopedSchema, Arc<dyn DekProvider>) -> Arc<dyn DekProvider>,
) -> Option<PgEnv> {
    let guard = lock_pg_test();
    let base = postgres_url_from_env();
    if let Err(err) = crate::probe_postgres(&base) {
        crate::pg_skip::unreachable_pg_or_skip(
            "postgres erasure tests",
            &AmError::new(ErrorCode::Internal, err),
        );
        return None;
    }
    let scoped = ScopedSchema::new(&base);
    let fixture_kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let kms = make_kms(&scoped, fixture_kms);
    match PostgresStore::open(scoped.url(), Arc::clone(&kms)) {
        Ok(store) => Some(PgEnv {
            scoped,
            store,
            kms,
            _guard: guard,
        }),
        Err(err) => {
            crate::pg_skip::unreachable_pg_or_skip("postgres erasure tests", &err);
            None
        }
    }
}

/// One bank whose subject's key was destroyed straight through the KMS before
/// the erasure, plus the erasure that followed it and the key rows it left.
struct DirectKeyLoss {
    env: PgEnv,
    bank: String,
    subject: String,
    survivor_item: String,
    erased_key_rows: Vec<(String, String, Option<String>)>,
    outcome: SubjectEraseOutcome,
}

/// Destroy one subject's key straight through the KMS, then erase the subject:
/// the setup the direct-key-loss claims share. The loss happens outside the
/// store's transaction, so the store's rows do not move and the two effects are
/// independent; the erasure's own effect is pinned here too.
fn erased_after_a_direct_key_loss(env: PgEnv) -> DirectKeyLoss {
    let bank = unique("pg-pre-destroyed");
    let subject = unique("pg-subject");
    let survivor = unique("pg-survivor-subject");
    let survivor_item = unique("pg-survivor-item");
    let (item_a, parent) = seed(&env.store, &bank, &subject);
    env.store
        .create_memory_item(&memory_item(&bank, &survivor_item, &survivor), "tester")
        .unwrap();
    let key_rows = subject_key_rows(&env.store, &subject);
    assert_eq!(key_rows.len(), 1, "{key_rows:?}");
    // The key loss happens outside the store's transaction, so the store's own
    // rows do not move: the two effects are independent.
    env.kms.destroy_dek(&subject).unwrap();
    assert!(env.store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(subject_key_rows(&env.store, &subject), key_rows);
    assert!(
        !env.store.subject_is_shredded(&subject).unwrap(),
        "the direct loss leaves no SQL shred marker"
    );

    // The erasure still performs its SQL work, and the key stays destroyed.
    let request = unique("pg-req");
    let outcome = env
        .store
        .erase_subject(&spec(&bank, &subject, &request))
        .unwrap();
    assert_erased(
        &env.store, &env.store, &bank, &subject, &request, &parent, &item_a,
    );
    let erased_key_rows = subject_key_rows(&env.store, &subject);
    DirectKeyLoss {
        env,
        bank,
        subject,
        survivor_item,
        erased_key_rows,
        outcome,
    }
}

/// A key destroyed straight through the KMS before the erasure stays destroyed
/// while the erasure's own SQL work completes, and a new content key is refused
/// while the other subject stays readable.
#[test]
fn postgres_erase_after_a_direct_key_loss_completes_and_refuses_a_new_key() {
    let Some(env) = open_pg() else {
        return;
    };
    let DirectKeyLoss {
        env,
        bank,
        subject,
        survivor_item,
        erased_key_rows,
        outcome,
    } = erased_after_a_direct_key_loss(env);

    assert!(outcome.vectors_purged >= 1, "{outcome:?}");
    assert!(outcome.memtree_nodes_purged >= 2, "{outcome:?}");
    assert_eq!(outcome.tombstones_written, 2);
    assert_eq!(erased_key_rows.len(), 1, "{erased_key_rows:?}");
    assert_eq!(erased_key_rows[0].0, "shredded", "{erased_key_rows:?}");
    assert!(erased_key_rows[0].2.is_some(), "{erased_key_rows:?}");
    assert!(env.store.subject_is_shredded(&subject).unwrap());
    assert!(env.store.subject_dek_destroyed(&subject).unwrap());

    // A new content key is refused for the erased subject.
    let err = env
        .store
        .create_memory_item(
            &memory_item(&bank, &unique("pg-late-item"), &subject),
            "tester",
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ErasedSubject);
    assert_eq!(
        env.kms.ensure_dek(&subject).unwrap_err().code,
        ErrorCode::ErasedSubject,
        "the key is never recreated"
    );

    // The other subject stays readable and its key alive.
    let survivor = env
        .store
        .get_memory_item(&bank, &survivor_item)
        .unwrap()
        .unwrap();
    assert_eq!(
        survivor.snapshot,
        Some(serde_json::json!({"entity": "secret-value"}))
    );
    assert!(
        !env.store
            .subject_dek_destroyed(&unique("pg-untouched"))
            .unwrap()
    );
}

/// A retried erasure over a key destroyed straight through the KMS never
/// recreates it: the repeat adds no key row and the tombstone state stays honest.
#[test]
fn postgres_erase_after_a_direct_key_loss_never_recreates_the_key_on_a_retry() {
    let Some(env) = open_pg() else {
        return;
    };
    let DirectKeyLoss {
        env,
        bank,
        subject,
        erased_key_rows,
        ..
    } = erased_after_a_direct_key_loss(env);

    env.store
        .erase_subject(&spec(&bank, &subject, &unique("pg-retry")))
        .expect("the retried erasure completes");
    assert_eq!(
        subject_key_rows(&env.store, &subject),
        erased_key_rows,
        "the repeat adds no key row"
    );
    assert!(env.store.subject_is_shredded(&subject).unwrap());
    assert!(env.store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(
        env.kms.ensure_dek(&subject).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
}

/// A duplicate erasure with the same request id is refused inside its own
/// transaction, so it writes no second tombstone and adds no new key.
#[test]
fn postgres_duplicate_erase_creates_no_second_effect() {
    let Some(env) = open_pg() else {
        return;
    };
    let bank = unique("pg-duplicate");
    let subject = unique("pg-subject");
    seed(&env.store, &bank, &subject);
    let request = unique("pg-req");
    env.store
        .erase_subject(&spec(&bank, &subject, &request))
        .unwrap();
    let tombstones = env.store.read_tombstones(&bank, &request).unwrap();
    assert_eq!(tombstones.len(), 2, "{tombstones:?}");
    let key_rows = subject_key_rows(&env.store, &subject);

    let duplicate = env.store.erase_subject(&spec(&bank, &subject, &request));
    assert!(duplicate.is_err(), "{duplicate:?}");

    assert_eq!(
        env.store.read_tombstones(&bank, &request).unwrap().len(),
        2,
        "the duplicate writes no second tombstone"
    );
    assert_eq!(
        subject_key_rows(&env.store, &subject),
        key_rows,
        "the duplicate adds no key row"
    );
    assert!(env.store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(
        env.kms.ensure_dek(&subject).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
}

/// Two concurrent erasures with the same request id leave exactly one key and
/// one request ledger: whichever call loses the race rolls back whole.
#[test]
fn postgres_concurrent_duplicate_erase_creates_one_key_and_one_ledger() {
    let Some(env) = open_pg() else {
        return;
    };
    let bank = unique("pg-race");
    let subject = unique("pg-subject");
    seed(&env.store, &bank, &subject);
    let request = unique("pg-req");

    let store2 = env.second_store();
    let store1 = env.store;
    let (bank1, subject1, request1) = (bank.clone(), subject.clone(), request.clone());
    let (bank2, subject2, request2) = (bank.clone(), subject.clone(), request.clone());
    let (first, second) = std::thread::scope(|scope| {
        let h1 = scope.spawn(move || store1.erase_subject(&spec(&bank1, &subject1, &request1)));
        let h2 = scope.spawn(move || store2.erase_subject(&spec(&bank2, &subject2, &request2)));
        (
            h1.join().expect("race thread one"),
            h2.join().expect("race thread two"),
        )
    });
    let committed = usize::from(first.is_ok()) + usize::from(second.is_ok());
    assert_eq!(
        committed, 1,
        "one racing call commits: {first:?} / {second:?}"
    );

    // Verify on a fresh connection: the loser's transaction left nothing.
    let verify =
        PostgresStore::open(env.scoped.url(), Arc::clone(&env.kms)).expect("open verify store");
    let key_rows = subject_key_rows(&verify, &subject);
    assert_eq!(key_rows.len(), 1, "{key_rows:?}");
    assert_eq!(key_rows[0].0, "shredded", "{key_rows:?}");
    assert!(verify.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(
        verify.read_tombstones(&bank, &request).unwrap().len(),
        2,
        "one request ledger, not two"
    );
    assert_eq!(
        env.kms.ensure_dek(&subject).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
}
