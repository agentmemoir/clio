// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for Postgres evidence about the erasure crash
//! window: the shred marker must be committed before the irreversible DEK
//! destroy, a forced failure after that commit leaves the subject closed for
//! writes, and a destroy error is tolerated only on a retry.
//!
//! ## Owns
//! - The witness that reads the subject's shred marker from a store opened
//!   before the erasure, at the destroy call
//! - The Postgres tombstone-failure scenario and its marker, purge, and
//!   tombstone claims
//! - The Postgres refused-destroy retry the tolerance exists for
//!
//! ## Does not own
//! - The erasure transaction under test (`postgres_erase`)
//! - The Postgres scoped-schema fixture and skip discipline
//!   (`postgres_erase_retry_tests`)
//! - The SQLite twin of these scenarios (`erase_crash_window_tests`)
//! - Shared erasure fixtures and assertions (`erase_test_support`)
//!
//! ## Boundary
//!
//! Changes to what a Postgres erasure crash leaves durable should normally
//! require changes here. Changes to the purge SQL itself should not require
//! changes here.
//!
//! Keep this module focused on the Postgres erasure crash window.
//!
//! Test-only. Never compiled into production builds.

use super::postgres_erase_retry_tests::{open_pg, open_pg_with, second_store};
use crate::compliance_store::ComplianceStore;
use crate::dek::{CiphertextEnvelope, DekProvider};
use crate::erase_test_support::{memory_item, seed, spec, unique};
use crate::migrate::IntoAmError;
use crate::postgres::PostgresStore;
use crate::store::Store;
use crate::sync_store::SyncStore;
use clio_types::{AmError, ErrorCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// DEK provider that wraps the fixture KMS and reports what the erasure has
/// made durable when it asks for the key destruction.
///
/// With `witness` set, the destroy reads the subject's shred marker from a store
/// that was opened before the erasure and asserts it is already committed: under
/// an ordering that destroys the key before the marker commits, that store reads
/// the pre-commit snapshot, sees no shredded marker, and the assertion fails.
/// `refuse_until` names the last 1-based call that fails without touching the
/// key, the shape of a provider that refuses a destroy while the key is still
/// live. `fail_after` names the 1-based call whose destroy runs and then
/// reports a failure, the shape of a provider that destroys the key and then
/// loses the acknowledgement.
struct DestroyWindow {
    inner: Arc<dyn DekProvider>,
    witness: Option<PostgresStore>,
    refuse_until: AtomicUsize,
    fail_after: AtomicUsize,
    destroy_calls: AtomicUsize,
}

impl DestroyWindow {
    /// A destroy that reads the shred marker through `witness` first, then
    /// delegates to the fixture KMS.
    fn witnessing(inner: Arc<dyn DekProvider>, witness: PostgresStore) -> Arc<dyn DekProvider> {
        Arc::new(Self {
            inner,
            witness: Some(witness),
            refuse_until: AtomicUsize::new(0),
            fail_after: AtomicUsize::new(0),
            destroy_calls: AtomicUsize::new(0),
        })
    }

    /// A destroy that refuses without destroying the key up to and including
    /// its `refuse_calls`-th call, then destroys it; later calls succeed.
    fn refusing_until(inner: Arc<dyn DekProvider>, refuse_calls: usize) -> Arc<dyn DekProvider> {
        Arc::new(Self {
            inner,
            witness: None,
            refuse_until: AtomicUsize::new(refuse_calls),
            fail_after: AtomicUsize::new(0),
            destroy_calls: AtomicUsize::new(0),
        })
    }

    /// A destroy that refuses without destroying until its `fail_on_call`-th
    /// call, then destroys the key and reports a failure; later calls succeed.
    /// Call 1 is a first attempt, call 2 a retry over the committed marker.
    fn failing_after_once(
        inner: Arc<dyn DekProvider>,
        fail_on_call: usize,
    ) -> Arc<dyn DekProvider> {
        Arc::new(Self {
            inner,
            witness: None,
            refuse_until: AtomicUsize::new(fail_on_call.saturating_sub(1)),
            fail_after: AtomicUsize::new(fail_on_call),
            destroy_calls: AtomicUsize::new(0),
        })
    }
}

impl DekProvider for DestroyWindow {
    fn ensure_dek(&self, subject_id: &str) -> Result<String, AmError> {
        self.inner.ensure_dek(subject_id)
    }

    fn blind_index_key(&self) -> Result<[u8; 32], AmError> {
        self.inner.blind_index_key()
    }

    fn ensure_blind_index_key(&self) -> Result<[u8; 32], AmError> {
        self.inner.ensure_blind_index_key()
    }

    fn encrypt(&self, subject_id: &str, plaintext: &[u8]) -> Result<CiphertextEnvelope, AmError> {
        self.inner.encrypt(subject_id, plaintext)
    }

    fn decrypt(&self, envelope: &CiphertextEnvelope) -> Result<Vec<u8>, AmError> {
        self.inner.decrypt(envelope)
    }

    fn destroy_dek(&self, subject_id: &str) -> Result<(), AmError> {
        let call = self.destroy_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(witness) = &self.witness {
            // The witness store was opened before the erasure was called, so
            // this read uses an already-open connection and starts no store
            // bootstrap or migration inside the destroy.
            assert!(
                witness
                    .subject_is_shredded(subject_id)
                    .expect("witness shred marker read"),
                "the shred marker is committed before the destroy"
            );
            // That predicate reads `status`; the write guard reads
            // `shredded_at`, so assert the guard's own column as well.
            let guard_marker: bool = witness
                .with_client(|client| {
                    let sql = "SELECT shredded_at IS NOT NULL FROM subject_keys \
                               WHERE subject_id = $1";
                    let row = client
                        .query_one(sql, &[&subject_id])
                        .am("witness shredded_at read")?;
                    Ok(row.get(0))
                })
                .expect("witness shredded_at read");
            assert!(
                guard_marker,
                "the guard's shredded_at column is set before the destroy"
            );
        }
        if call <= self.refuse_until.load(Ordering::SeqCst) {
            return Err(AmError::new(
                ErrorCode::Internal,
                "the provider refused this destroy",
            ));
        }
        self.inner.destroy_dek(subject_id)?;
        if call == self.fail_after.load(Ordering::SeqCst) {
            return Err(AmError::new(
                ErrorCode::Internal,
                "the provider failed after destroying the key",
            ));
        }
        Ok(())
    }

    fn diagnose_masked(&self, subject_id: &str) -> String {
        self.inner.diagnose_masked(subject_id)
    }
}

/// A forced failure in the tombstone transaction leaves the committed purge and
/// shred marker in place: the subject stays closed for writes, no tombstone
/// claims the request completed, and a retry finishes the ledger.
#[test]
fn postgres_tombstone_failure_after_the_shred_marker_leaves_the_subject_closed() {
    let Some(env) = open_pg() else {
        return;
    };
    let bank = unique("pg-rollback");
    let subject = unique("pg-subject");
    let survivor = unique("pg-survivor-subject");
    let survivor_item = unique("pg-survivor-item");
    let (item_a, _parent) = seed(&env.store, &bank, &subject);
    env.store
        .create_memory_item(&memory_item(&bank, &survivor_item, &survivor), "tester")
        .unwrap();

    // A BEFORE INSERT trigger aborts every tombstone insert, so the erasure
    // fails in its second transaction: the purge and the marker commit first.
    env.store
        .test_execute(
            "CREATE FUNCTION fail_erase_tombstone() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'forced tombstone failure'; END $$;
             CREATE TRIGGER fail_erase_tombstone BEFORE INSERT ON compliance_tombstones
             FOR EACH ROW EXECUTE FUNCTION fail_erase_tombstone()",
        )
        .unwrap();

    let request = unique("pg-req");
    let failed = env.store.erase_subject(&spec(&bank, &subject, &request));
    assert!(failed.is_err(), "{failed:?}");

    // Tidy the trigger so the retry can commit its ledger.
    env.store
        .test_execute(
            "DROP TRIGGER IF EXISTS fail_erase_tombstone ON compliance_tombstones;
             DROP FUNCTION IF EXISTS fail_erase_tombstone()",
        )
        .unwrap();

    // The key destroyed between the two transactions stays destroyed.
    assert!(env.store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(
        env.kms.ensure_dek(&subject).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    // The purge and the shred marker committed before the destroy, so the
    // subject is closed for writes even though the ledger rolled back.
    assert!(
        env.store.subject_is_shredded(&subject).unwrap(),
        "the shred marker committed"
    );
    assert!(
        env.store
            .read_tombstones(&bank, &request)
            .unwrap()
            .is_empty(),
        "the request is not falsely completed"
    );
    // The purge committed with the marker: the dense vector row is gone, so a
    // rolled-back purge would fail this line.
    let embeddings: i64 = env
        .store
        .with_client(|client| {
            let sql = "SELECT count(*) FROM item_embeddings WHERE item_id = $1";
            Ok(client
                .query_one(sql, &[&item_a])
                .am("embedding count")?
                .get(0))
        })
        .unwrap();
    assert_eq!(embeddings, 0, "the purge committed");
    // A new content key is refused: the committed marker is the guard.
    let refused = env
        .store
        .create_memory_item(&memory_item(&bank, &unique("pg-late"), &subject), "tester")
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::ErasedSubject);
    // The other subject stays readable, and a retry writes the rolled-back ledger.
    let survivor_row = env
        .store
        .get_memory_item(&bank, &survivor_item)
        .unwrap()
        .unwrap();
    assert_eq!(
        survivor_row.snapshot,
        Some(serde_json::json!({"entity": "secret-value"}))
    );
    env.store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect("the retried erasure completes");
    assert_eq!(
        env.store.read_tombstones(&bank, &request).unwrap().len(),
        2,
        "the retry commits the ledger the failed attempt rolled back"
    );
}

/// The shred marker is already committed on a second connection when the
/// erasure asks the KMS to destroy the key, and the erasure completes.
///
/// The witness store is opened before the erasure is called, so the destroy
/// reads an already-open connection: no store bootstrap or migration runs
/// inside the destroy. That is what makes this test discriminate an ordering:
/// under an ordering that destroys the key before the marker commits, the
/// witness reads the pre-commit snapshot, sees no shredded marker, and the test
/// fails at the destroy call instead of only in the end state.
#[test]
fn postgres_the_shred_marker_is_committed_before_the_dek_destroy() {
    let Some(env) = open_pg_with(|scoped, inner| {
        let witness = second_store(scoped, &inner);
        DestroyWindow::witnessing(inner, witness)
    }) else {
        return;
    };
    let bank = unique("pg-crash-bank");
    let subject = unique("pg-subject");
    seed(&env.store, &bank, &subject);
    let request = unique("pg-req");

    let outcome = env
        .store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect("the erasure completes");
    assert_eq!(outcome.tombstones_written, 2, "{outcome:?}");
    assert!(
        env.store.subject_is_shredded(&subject).unwrap(),
        "the shred marker is committed"
    );
    assert!(
        env.store.subject_dek_destroyed(&subject).unwrap(),
        "the key is destroyed"
    );
    assert_eq!(
        env.kms.ensure_dek(&subject).unwrap_err().code,
        ErrorCode::ErasedSubject,
        "the key is never recreated"
    );
}

/// A retry that meets a destroy error over the marker an earlier attempt
/// committed belongs to the same erasure, so it completes: the key is gone and
/// the tombstones land.
#[test]
fn postgres_a_retry_tolerates_a_destroy_error_over_the_committed_marker() {
    // The first attempt refuses before destroying; the second destroys the key
    // and then reports a failure.
    let Some(env) = open_pg_with(|_, inner| DestroyWindow::failing_after_once(inner, 2)) else {
        return;
    };
    let bank = unique("pg-crash-bank");
    let subject = unique("pg-subject");
    seed(&env.store, &bank, &subject);
    let request = unique("pg-req");

    let err = env
        .store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect_err("a refused destroy fails the erasure");
    assert_eq!(err.code, ErrorCode::Internal, "{err:?}");
    assert!(env.store.subject_is_shredded(&subject).unwrap());
    assert!(
        !env.store.subject_dek_destroyed(&subject).unwrap(),
        "the refused destroy left the key alive"
    );
    assert!(
        env.store
            .read_tombstones(&bank, &request)
            .unwrap()
            .is_empty(),
        "the failed attempt writes no tombstone"
    );

    // The marker was already shredded by the first attempt, so the destroy
    // error cannot hide a first-attempt failure and the erasure completes.
    let outcome = env
        .store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect("the retry completes although the destroy reported an error");
    assert_eq!(outcome.tombstones_written, 2, "{outcome:?}");
    assert!(env.store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(env.store.read_tombstones(&bank, &request).unwrap().len(), 2);
}

/// A retry over the committed marker whose destroy fails while the key survives
/// must not be read as the tolerated case: the marker being shredded is not
/// enough, so the destroy error surfaces, the marker stays committed, no
/// tombstone is written, and only a later attempt with a working destroy
/// completes the erasure.
#[test]
fn postgres_a_retry_with_a_live_key_still_reports_the_destroy_error() {
    // Both attempts refuse before destroying, so the second one meets a
    // committed marker and a key that is still live.
    let Some(env) = open_pg_with(|_, inner| DestroyWindow::refusing_until(inner, 2)) else {
        return;
    };
    let bank = unique("pg-crash-bank");
    let subject = unique("pg-subject");
    seed(&env.store, &bank, &subject);
    let request = unique("pg-req");

    // The first attempt commits the marker and then refuses the destroy.
    let first = env
        .store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect_err("a refused destroy fails the first attempt");
    assert_eq!(first.code, ErrorCode::Internal, "{first:?}");
    assert!(env.store.subject_is_shredded(&subject).unwrap());

    let err = env
        .store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect_err("a retry over a live key still reports the destroy error");
    assert_eq!(err.code, ErrorCode::Internal, "{err:?}");
    assert!(env.store.subject_is_shredded(&subject).unwrap());
    assert!(
        !env.store.subject_dek_destroyed(&subject).unwrap(),
        "the refused destroy left the key alive"
    );
    assert!(
        env.store
            .read_tombstones(&bank, &request)
            .unwrap()
            .is_empty(),
        "neither failed attempt writes a tombstone"
    );

    // The provider accepts the destroy now: the third attempt completes.
    let outcome = env
        .store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect("the third attempt completes");
    assert_eq!(outcome.tombstones_written, 2, "{outcome:?}");
    assert!(env.store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(env.store.read_tombstones(&bank, &request).unwrap().len(), 2);
}
