// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for testing that a damaged neighbor row degrades
//! admission signals instead of failing every write in the bank.
//!
//! ## Owns
//! - Corrupt-row skip tests for the store-backed signal source
//! - Fail-closed tests proving backend failures are not swallowed
//!
//! ## Does not own
//! - Production signal-source behavior (see `runtime_sinks`)
//! - Admission scoring or durable item storage
//!
//! ## Boundary
//!
//! Changes to how unreadable rows are classified should normally require changes here.
//! Changes to admission scoring should not require changes here.
//!
//! Keep this module focused on damaged-row resilience in admission lookups.

use super::*;
use clio_admission::gated::candidate_text;
use clio_store::{CiphertextEnvelope, DekProvider, LocalDevKms, SqliteStore};
use clio_types::{AmError, ErrorCode};
use std::sync::Arc;

/// Subject whose stored content is deliberately damaged at seal time.
const CORRUPT_SUBJECT: &str = "corrupt-subject";
/// Subject whose key store fails, standing in for a broken backend.
const BROKEN_SUBJECT: &str = "broken-subject";

/// A KMS that writes an unopenable envelope for [`CORRUPT_SUBJECT`] and fails to
/// open anything for [`BROKEN_SUBJECT`], delegating every other subject to a
/// real `LocalDevKms`.
///
/// Both faults are produced through the public provider surface, so the rows
/// need no direct SQL tampering.
#[derive(Debug)]
struct FaultyKms {
    inner: LocalDevKms,
}

impl DekProvider for FaultyKms {
    fn ensure_dek(&self, subject_id: &str) -> Result<String, AmError> {
        self.inner.ensure_dek(subject_id)
    }

    fn encrypt(&self, subject_id: &str, plaintext: &[u8]) -> Result<CiphertextEnvelope, AmError> {
        let mut envelope = self.inner.encrypt(subject_id, plaintext)?;
        if subject_id == CORRUPT_SUBJECT {
            // Valid envelope JSON, undecodable ciphertext: the row is stored
            // normally and only fails when a reader tries to open it.
            envelope.ciphertext_b64 = "!!!not-base64!!!".to_owned();
        }
        Ok(envelope)
    }

    fn decrypt(&self, envelope: &CiphertextEnvelope) -> Result<Vec<u8>, AmError> {
        if envelope.subject_id == BROKEN_SUBJECT {
            // What a real key-store failure looks like: `Internal`, which the
            // skip predicate must not treat as unreadable content.
            return Err(AmError::new(
                ErrorCode::Internal,
                "read DEK slot: database is locked",
            ));
        }
        self.inner.decrypt(envelope)
    }

    fn destroy_dek(&self, subject_id: &str) -> Result<(), AmError> {
        self.inner.destroy_dek(subject_id)
    }

    fn diagnose_masked(&self, subject_id: &str) -> String {
        self.inner.diagnose_masked(subject_id)
    }

    fn blind_index_key(&self) -> Result<[u8; 32], AmError> {
        self.inner.blind_index_key()
    }

    fn ensure_blind_index_key(&self) -> Result<[u8; 32], AmError> {
        self.inner.ensure_blind_index_key()
    }
}

fn item(bank: &str, id: &str, subject: &str, gist: &str, created_at: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: subject.into(),
        kind: clio_types::ItemKind::Semantic,
        category: Some(clio_types::SemanticCategory::Persona),
        episodic_type: None,
        epistemic_kind: clio_types::EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        snapshot: None,
        gist: Some(gist.into()),
        valid_from: created_at.into(),
        valid_until: None,
        tx_from: created_at.into(),
        tx_until: None,
        created_at: created_at.into(),
        updated_at: created_at.into(),
        context: None,
    }
}

fn faulty_store() -> Arc<SqliteStore> {
    let kms = Arc::new(FaultyKms {
        inner: LocalDevKms::new(),
    });
    Arc::new(SqliteStore::open_in_memory(kms as Arc<dyn DekProvider>).expect("store"))
}

/// A row whose content cannot be opened costs only its own novelty contribution:
/// the readable neighbor is still scored, while recency stays neutral.
#[test]
fn corrupt_neighbor_is_skipped_while_a_readable_neighbor_remains() {
    let store = faulty_store();
    let readable = item(
        "bank-corrupt",
        "itm-readable",
        "readable-subject",
        "readable neighbor",
        "2026-01-01T00:00:00Z",
    );
    let corrupt = item(
        "bank-corrupt",
        "itm-corrupt",
        CORRUPT_SUBJECT,
        "corrupt neighbor",
        "2026-01-02T00:00:00Z",
    );
    store.create_memory_item(&readable, "agent").expect("seed");
    store.create_memory_item(&corrupt, "agent").expect("seed");

    // The damage is real: the row is present and opens with an error.
    let err = store
        .get_memory_item("bank-corrupt", "itm-corrupt")
        .expect_err("corrupt row must not open");
    assert!(
        clio_store::is_unreadable_content(&err),
        "unexpected code: {err}"
    );

    let signals = StoreSignals::new(store.clone(), store);
    let probe = item(
        "bank-corrupt",
        "probe",
        "probe-subject",
        "new candidate",
        "2026-01-03T00:00:00Z",
    );
    let (neighbors, age) = signals.neighbor_signals(&probe).expect("signals");

    assert_eq!(neighbors, vec![candidate_text(&readable)]);
    assert!(
        age.is_none(),
        "recency stays neutral even with a readable neighbor"
    );
}

/// A corrupt row in the bank must not fail the write that triggered the scan.
#[test]
fn corrupt_neighbor_never_fails_the_new_write() {
    let store = faulty_store();
    store
        .create_memory_item(
            &item(
                "bank-write",
                "itm-corrupt",
                CORRUPT_SUBJECT,
                "corrupt neighbor",
                "2026-01-01T00:00:00Z",
            ),
            "agent",
        )
        .expect("seed");
    let signals = StoreSignals::new(store.clone(), store);
    let probe = item(
        "bank-write",
        "probe",
        "probe-subject",
        "a genuinely new note",
        "2026-01-02T00:00:00Z",
    );

    let texts = SignalSource::neighbor_texts(&signals, &probe).expect("novelty neighbors");
    assert!(texts.is_empty(), "the only neighbor was skipped: {texts:?}");
    assert!(
        SignalSource::prior_age_days(&signals, &probe)
            .expect("recency")
            .is_none(),
        "no readable neighbor means no prior"
    );
}

/// A backend failure is not unreadable content and must stay fail-closed: if the
/// store cannot answer, admission must not proceed on guessed signals.
#[test]
fn backend_failure_still_fails_closed() {
    let store = faulty_store();
    store
        .create_memory_item(
            &item(
                "bank-broken",
                "itm-broken",
                BROKEN_SUBJECT,
                "neighbor behind a broken key store",
                "2026-01-01T00:00:00Z",
            ),
            "agent",
        )
        .expect("seed");
    let signals = StoreSignals::new(store.clone(), store);
    let probe = item(
        "bank-broken",
        "probe",
        "probe-subject",
        "new candidate",
        "2026-01-02T00:00:00Z",
    );

    assert!(
        SignalSource::neighbor_texts(&signals, &probe).is_err(),
        "a broken backend must not be treated as an unreadable row"
    );
}

/// The accepted cost of skipping: a damaged row is invisible to the duplicate
/// gate, so a near-duplicate of it is admitted.
///
/// This test exists to make the trade explicit rather than to defend it. If
/// someone later makes damaged rows count toward novelty, this test is the one
/// that should fail and force the decision to be made on purpose. See the
/// "What the skip costs" section on `readable_neighbor`.
#[test]
fn unreadable_neighbor_lets_a_duplicate_of_a_damaged_row_through() {
    let store = faulty_store();
    // The stored row says one thing; its content is unreadable, so nothing can
    // ever compare against it again.
    store
        .create_memory_item(
            &item(
                "bank-dup",
                "itm-damaged",
                CORRUPT_SUBJECT,
                "the user moved to Lisbon in 2026",
                "2026-01-01T00:00:00Z",
            ),
            "agent",
        )
        .expect("seed");
    let signals = StoreSignals::new(store.clone(), store);
    let probe = item(
        "bank-dup",
        "probe",
        "probe-subject",
        "the user moved to Lisbon in 2026",
        "2026-01-02T00:00:00Z",
    );

    // Damaged row readable -> the probe's own text is a novelty neighbor.
    // Damaged row skipped -> nothing is, so novelty is computed as if the
    // duplicate did not exist and the duplicate gate cannot fire.
    let texts = SignalSource::neighbor_texts(&signals, &probe).expect("novelty neighbors");
    assert!(
        texts.is_empty(),
        "the only neighbor is unreadable, so the duplicate gate sees nothing: {texts:?}"
    );
}
