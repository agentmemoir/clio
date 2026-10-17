// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite erasure crash-window tests: the
//! shred marker must be durable before the irreversible DEK destroy, and a
//! refused destroy must leave the subject closed and the request retryable.
//!
//! ## Owns
//! - The probe that reads the shred marker from a second connection at the
//!   destroy call
//! - The refused-destroy and failed-destroy scenarios and their marker and
//!   tombstone claims, including which attempts may read a destroy error as a
//!   retry
//!
//! ## Does not own
//! - The erasure transaction under test (`sqlite_erase`)
//! - Shared erasure fixtures and assertions (`erase_test_support`)
//!
//! ## Boundary
//!
//! Changes to what an erasure crash leaves durable should normally require
//! changes here. Changes to the purge SQL itself should not require changes
//! here.
//!
//! Keep this module focused on the erasure crash window.
//!
//! Test-only. Never compiled into production builds.

use crate::compliance_store::ComplianceStore;
use crate::dek::{CiphertextEnvelope, DekProvider};
use crate::erase_test_support::{local_kms, memory_item, seed, spec, unique};
use crate::item_persist::shredded_subject_err;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use crate::sync_store::SyncStore;
use clio_types::{AmError, ErrorCode};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A scratch directory that is removed when the test ends, including on panic.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(crate::test_unique::scratch_dir("clio-erase-crash-window"))
    }

    fn db(&self) -> PathBuf {
        self.0.join("store.db")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// DEK provider that wraps the local KMS and reports what the erasure has made
/// durable at the moment it asks for the key destruction.
///
/// With `store_path` set, the destroy reads the subject's shred marker from a
/// second connection on that file and checks it is already committed: under an
/// ordering that destroys the key before the marker commits, the marker is not
/// yet visible to another connection. With `refuse_once` set, the next destroy
/// fails without destroying the key, which is the shape of a provider that
/// refuses a second destroy after a crash. `fail_after` names the 1-based call
/// whose destroy runs and then reports a failure, the shape of a provider that
/// destroys the key and then loses the acknowledgement.
struct DestroyWindow {
    inner: Arc<dyn DekProvider>,
    store_path: Option<PathBuf>,
    refuse_once: AtomicBool,
    fail_after: AtomicUsize,
    destroy_calls: AtomicUsize,
}

impl DestroyWindow {
    fn on_disk(path: &Path) -> Arc<Self> {
        Arc::new(Self {
            inner: local_kms(),
            store_path: Some(path.to_path_buf()),
            refuse_once: AtomicBool::new(false),
            fail_after: AtomicUsize::new(0),
            destroy_calls: AtomicUsize::new(0),
        })
    }

    fn refusing_once() -> Arc<Self> {
        Arc::new(Self {
            inner: local_kms(),
            store_path: None,
            refuse_once: AtomicBool::new(true),
            fail_after: AtomicUsize::new(0),
            destroy_calls: AtomicUsize::new(0),
        })
    }

    /// A destroy that refuses without destroying until its `fail_on_call`-th
    /// call, then destroys the key and reports a failure; later calls succeed.
    /// Call 1 is a first attempt, call 2 a retry over the committed marker.
    fn failing_after_once(fail_on_call: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: local_kms(),
            store_path: None,
            refuse_once: AtomicBool::new(fail_on_call > 1),
            fail_after: AtomicUsize::new(fail_on_call),
            destroy_calls: AtomicUsize::new(0),
        })
    }

    fn as_provider(self: &Arc<Self>) -> Arc<dyn DekProvider> {
        Arc::clone(self) as Arc<dyn DekProvider>
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
        if let Some(path) = &self.store_path {
            // The marker must be durable before this irreversible call, so a
            // crash cannot leave a destroyed key with no marker behind it. The
            // reader is a plain read-write open made read-only by `query_only`:
            // it sees only committed state, writes nothing, and takes no write
            // lock, so it cannot contend with a lock this call must not hold.
            let reader = rusqlite::Connection::open(path)
                .expect("open a reader on the store file while the destroy is pending");
            reader
                .execute_batch("PRAGMA query_only = ON")
                .expect("make the reader read-only");
            let marker: (String, Option<String>) = reader
                .query_row(
                    "SELECT status, shredded_at FROM subject_keys WHERE subject_id = ?1",
                    [subject_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .expect("the shred marker is committed before the destroy");
            assert_eq!(marker.0, "shredded", "the committed marker says shredded");
            assert!(
                marker.1.is_some(),
                "the committed marker carries shredded_at"
            );
        }
        if self.refuse_once.swap(false, Ordering::SeqCst) {
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

/// On an on-disk store, the shred marker is already visible to a second
/// connection when the erasure asks the KMS to destroy the key, and the erase
/// still completes with the key destroyed afterwards.
#[test]
fn sqlite_the_shred_marker_is_durable_before_the_dek_destroy() {
    let scratch = Scratch::new();
    let path = scratch.db();
    let window = DestroyWindow::on_disk(&path);
    let store = SqliteStore::open(&path, window.as_provider()).expect("open the on-disk store");
    let bank = unique("crash-bank");
    let subject = unique("crash-subject");
    seed(&store, &bank, &subject);
    let request = unique("crash-request");

    let outcome = store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect("the erasure completes");
    assert_eq!(outcome.tombstones_written, 2, "{outcome:?}");
    assert_eq!(
        window.destroy_calls.load(Ordering::SeqCst),
        1,
        "the destroy ran once"
    );
    assert!(store.subject_is_shredded(&subject).unwrap());
    assert!(store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(
        window.ensure_dek(&subject).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
}

/// A destroy that fails without destroying the key still leaves the committed
/// marker in place, so the subject is closed for writes and the request has no
/// tombstone; the same request then completes once the provider accepts the
/// destroy.
#[test]
fn sqlite_a_refused_destroy_leaves_the_subject_closed_and_the_request_retryable() {
    let window = DestroyWindow::refusing_once();
    let store =
        SqliteStore::open_in_memory(window.as_provider()).expect("open the in-memory store");
    let bank = unique("crash-bank");
    let subject = unique("crash-subject");
    seed(&store, &bank, &subject);
    let request = unique("crash-request");

    let err = store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect_err("a refused destroy fails the erasure");
    assert_eq!(err.code, ErrorCode::Internal, "{err:?}");
    assert_eq!(
        window.destroy_calls.load(Ordering::SeqCst),
        1,
        "the destroy was attempted once"
    );
    // The marker committed before the attempted destroy, so the subject is
    // closed for writes even though the key is still live.
    assert!(store.subject_is_shredded(&subject).unwrap());
    assert!(!store.subject_dek_destroyed(&subject).unwrap());
    assert!(
        store.read_tombstones(&bank, &request).unwrap().is_empty(),
        "the failed attempt writes no tombstone"
    );
    let refused_item = unique("crash-late-item");
    let refused = store
        .create_memory_item(&memory_item(&bank, &refused_item, &subject), "tester")
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::ErasedSubject);
    assert_eq!(refused, shredded_subject_err(&subject));

    // The provider accepts the destroy now: the same request completes.
    let outcome = store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect("the retried erasure completes");
    assert_eq!(outcome.tombstones_written, 2, "{outcome:?}");
    assert_eq!(
        window.destroy_calls.load(Ordering::SeqCst),
        2,
        "the retry attempts the destroy again"
    );
    assert!(store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(store.read_tombstones(&bank, &request).unwrap().len(), 2);
    assert!(
        store
            .get_memory_item(&bank, &refused_item)
            .unwrap()
            .is_none(),
        "the refused write left no item row"
    );
}

/// A first attempt whose destroy destroys the key and then reports a failure
/// must not be read as a retry: the marker was not shredded when the attempt
/// began, so the error surfaces, and only the next attempt completes the
/// erasure.
#[test]
fn sqlite_a_first_attempt_does_not_tolerate_a_destroy_error() {
    let window = DestroyWindow::failing_after_once(1);
    let store =
        SqliteStore::open_in_memory(window.as_provider()).expect("open the in-memory store");
    let bank = unique("crash-bank");
    let subject = unique("crash-subject");
    seed(&store, &bank, &subject);
    let request = unique("crash-request");

    let err = store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect_err("a first-attempt destroy error fails the erasure");
    assert_eq!(err.code, ErrorCode::Internal, "{err:?}");
    assert_eq!(
        window.destroy_calls.load(Ordering::SeqCst),
        1,
        "the destroy was attempted once"
    );
    // The marker committed before the failed destroy, so the subject is
    // closed for writes, and the failure kept the ledger empty.
    assert!(store.subject_is_shredded(&subject).unwrap());
    assert!(
        store.read_tombstones(&bank, &request).unwrap().is_empty(),
        "the failed attempt writes no tombstone"
    );

    // The marker is shredded now and the key is gone, so this attempt is the
    // retry the tolerance exists for, even though the destroy reports Ok.
    let outcome = store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect("the retried erasure completes");
    assert_eq!(outcome.tombstones_written, 2, "{outcome:?}");
    assert_eq!(
        window.destroy_calls.load(Ordering::SeqCst),
        2,
        "the retry attempts the destroy again"
    );
    assert!(store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(store.read_tombstones(&bank, &request).unwrap().len(), 2);
}

/// A retry that meets a destroy error over the marker an earlier attempt
/// committed belongs to the same erasure, so it completes: the key is gone and
/// the tombstones land.
#[test]
fn sqlite_a_retry_tolerates_a_destroy_error_over_the_committed_marker() {
    // The first attempt refuses before destroying; the second destroys the key
    // and then reports a failure.
    let window = DestroyWindow::failing_after_once(2);
    let store =
        SqliteStore::open_in_memory(window.as_provider()).expect("open the in-memory store");
    let bank = unique("crash-bank");
    let subject = unique("crash-subject");
    seed(&store, &bank, &subject);
    let request = unique("crash-request");

    let err = store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect_err("a refused destroy fails the erasure");
    assert_eq!(err.code, ErrorCode::Internal, "{err:?}");
    assert_eq!(
        window.destroy_calls.load(Ordering::SeqCst),
        1,
        "the destroy was attempted once"
    );
    assert!(store.subject_is_shredded(&subject).unwrap());
    assert!(!store.subject_dek_destroyed(&subject).unwrap());
    assert!(
        store.read_tombstones(&bank, &request).unwrap().is_empty(),
        "the failed attempt writes no tombstone"
    );

    // The marker was already shredded by the first attempt, so the destroy
    // error cannot hide a first-attempt failure and the erasure completes.
    let outcome = store
        .erase_subject(&spec(&bank, &subject, &request))
        .expect("the retry completes although the destroy reported an error");
    assert_eq!(outcome.tombstones_written, 2, "{outcome:?}");
    assert_eq!(
        window.destroy_calls.load(Ordering::SeqCst),
        2,
        "the retry attempted the destroy again"
    );
    assert!(store.subject_dek_destroyed(&subject).unwrap());
    assert_eq!(store.read_tombstones(&bank, &request).unwrap().len(), 2);
}
