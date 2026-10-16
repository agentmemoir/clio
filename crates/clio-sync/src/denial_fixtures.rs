// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the shared erasure-denial test doubles: one
//! in-memory authority that answers verdicts, records denials with a
//! generation, reports its salt fingerprint, and can be made unreadable, plus
//! a KMS wrapper that records which subjects it minted a key for or sealed
//! content under.
//!
//! ## Owns
//! - `FakeDenial`: the in-memory `ErasureDenial` implementation and its
//!   inspection helpers (recorded banks, per-subject record count, generation)
//! - `CountingKms` and `store_with_counting_kms`: key-material observation
//!
//! ## Does not own
//! - The seams they implement (`clio-store` `ErasureDenial`, `DekProvider`)
//! - The guards under test (`denial_guard`) or the routes that call them
//! - Durable storage: the doubles keep state in memory and never touch disk
//!
//! ## Boundary
//!
//! Tests that need an authority or key-material observation use these doubles;
//! they never change them.
//!
//! Keep this module focused on the in-memory test doubles.

use clio_store::{
    CiphertextEnvelope, DekProvider, DenialVerdict, ErasureDenial, LocalDevKms, SqliteStore,
};
use clio_types::{AmError, ErrorCode};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The in-memory authority's recorded state.
#[derive(Default)]
struct State {
    /// One recorded denial per subject id, in the order its banks were given.
    denials: BTreeMap<String, Vec<String>>,
    /// How many *new* records each subject id cost (a redelivery costs none).
    writes: BTreeMap<String, usize>,
    /// The authority generation, one per recorded denial.
    generation: u64,
}

/// An in-memory erasure-denial authority for tests.
pub(crate) struct FakeDenial {
    state: Mutex<State>,
    fingerprint: String,
    unavailable: AtomicBool,
}

impl FakeDenial {
    /// An authority with the salt fingerprint `fingerprint`.
    pub(crate) fn new(fingerprint: &str) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            fingerprint: fingerprint.to_owned(),
            unavailable: AtomicBool::new(false),
        })
    }

    /// Record a denial of `subject_id` covering `banks`, returning the
    /// generation; a second call for the same subject changes nothing.
    pub(crate) fn deny(&self, subject_id: &str, banks: &[&str]) -> u64 {
        let banks: Vec<String> = banks.iter().map(|bank| (*bank).to_owned()).collect();
        self.record(subject_id, &banks)
            .expect("the fake authority is readable")
    }

    /// Record the serve guard for `bank` the way an applied denial does.
    pub(crate) fn deny_bank(&self, bank: &str) -> u64 {
        self.deny(&crate::denial::denied_bank_marker(bank), &[bank])
    }

    /// Make the authority unreadable: every verdict is unavailable and every
    /// write is refused, a damaged authority that must never read as clear.
    pub(crate) fn damage(&self) {
        self.unavailable.store(true, Ordering::SeqCst);
    }

    /// The salt fingerprint this authority reports.
    pub(crate) fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// The authority's current generation.
    pub(crate) fn generation(&self) -> u64 {
        self.state.lock().expect("fake denial state").generation
    }

    /// How many new records `subject_id` cost: one for a first denial, zero
    /// for every redelivery.
    pub(crate) fn records_for(&self, subject_id: &str) -> usize {
        self.state
            .lock()
            .expect("fake denial state")
            .writes
            .get(subject_id)
            .copied()
            .unwrap_or(0)
    }

    /// The recorded bank scope for `subject_id`, `None` when not recorded.
    pub(crate) fn banks_for(&self, subject_id: &str) -> Option<Vec<String>> {
        self.state
            .lock()
            .expect("fake denial state")
            .denials
            .get(subject_id)
            .cloned()
    }

    fn record(&self, subject_id: &str, banks: &[String]) -> Result<u64, AmError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(AmError::new(
                ErrorCode::ConfigCorrupt,
                "the fake denial authority is unavailable",
            ));
        }
        let mut state = self.state.lock().expect("fake denial state");
        if state.denials.contains_key(subject_id) {
            // An already-recorded identity makes no change and returns the
            // generation the authority carries, exactly like the seam's
            // contract requires.
            return Ok(state.generation);
        }
        state.denials.insert(subject_id.to_owned(), banks.to_vec());
        *state.writes.entry(subject_id.to_owned()).or_insert(0) += 1;
        state.generation = state.generation.checked_add(1).expect("generation");
        Ok(state.generation)
    }
}

impl ErasureDenial for FakeDenial {
    fn verdict(&self, subject_id: &str) -> DenialVerdict {
        if self.unavailable.load(Ordering::SeqCst) {
            return DenialVerdict::AuthorityUnavailable;
        }
        let state = self.state.lock().expect("fake denial state");
        if state.denials.contains_key(subject_id) {
            DenialVerdict::Denied
        } else {
            DenialVerdict::Clear
        }
    }

    fn record_denial(&self, subject_id: &str, banks: &[String]) -> Result<u64, AmError> {
        self.record(subject_id, banks)
    }

    fn salt_fingerprint(&self) -> String {
        self.fingerprint.clone()
    }
}

/// A DEK provider that delegates to an inner provider and records the subjects
/// it minted a key for or sealed content under, so a test can assert that a
/// refused mutation minted nothing.
pub(crate) struct CountingKms {
    inner: Arc<dyn DekProvider>,
    minted: Mutex<BTreeSet<String>>,
    sealed: Mutex<BTreeSet<String>>,
}

impl CountingKms {
    /// Wrap `inner`.
    pub(crate) fn new(inner: Arc<dyn DekProvider>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            minted: Mutex::new(BTreeSet::new()),
            sealed: Mutex::new(BTreeSet::new()),
        })
    }

    /// Whether a DEK was minted for `subject_id`.
    pub(crate) fn minted_for(&self, subject_id: &str) -> bool {
        self.minted
            .lock()
            .expect("counting kms")
            .contains(subject_id)
    }

    /// Whether content was sealed for `subject_id`.
    pub(crate) fn sealed_for(&self, subject_id: &str) -> bool {
        self.sealed
            .lock()
            .expect("counting kms")
            .contains(subject_id)
    }
}

impl DekProvider for CountingKms {
    fn ensure_dek(&self, subject_id: &str) -> Result<String, AmError> {
        self.minted
            .lock()
            .expect("counting kms")
            .insert(subject_id.to_owned());
        self.inner.ensure_dek(subject_id)
    }

    fn blind_index_key(&self) -> Result<[u8; 32], AmError> {
        self.inner.blind_index_key()
    }

    fn ensure_blind_index_key(&self) -> Result<[u8; 32], AmError> {
        self.inner.ensure_blind_index_key()
    }

    fn encrypt(&self, subject_id: &str, plaintext: &[u8]) -> Result<CiphertextEnvelope, AmError> {
        self.sealed
            .lock()
            .expect("counting kms")
            .insert(subject_id.to_owned());
        self.inner.encrypt(subject_id, plaintext)
    }

    fn decrypt(&self, envelope: &CiphertextEnvelope) -> Result<Vec<u8>, AmError> {
        self.inner.decrypt(envelope)
    }

    fn destroy_dek(&self, subject_id: &str) -> Result<(), AmError> {
        self.inner.destroy_dek(subject_id)
    }

    fn diagnose_masked(&self, subject_id: &str) -> String {
        self.inner.diagnose_masked(subject_id)
    }
}

/// An in-memory store over a local KMS wrapped by [`CountingKms`], so a test
/// can assert whether key material was minted for one subject.
pub(crate) fn store_with_counting_kms() -> (Arc<SqliteStore>, Arc<CountingKms>) {
    let kms = CountingKms::new(Arc::new(LocalDevKms::new()));
    let store = Arc::new(
        SqliteStore::open_in_memory(Arc::clone(&kms) as Arc<dyn DekProvider>).expect("sqlite open"),
    );
    (store, kms)
}
