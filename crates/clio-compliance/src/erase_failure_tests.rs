// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the failure semantics of the compliance
//! erasure workflow against a failing store or an unwritable journal: partial
//! completion reported as `PROCESSING`, plain error propagation, the credential
//! label a partial erasure event carries, the refusal a failed journal write
//! runs before any store effect, and the durable denial observed on disk at
//! the store call. Test-only: never compiled into production builds.
//!
//! ## Owns
//! - The failing-store test double for `erase_request` and its journal probe
//! - The partial-completion and error-propagation assertions
//! - The unwritable-journal refusal probe (no store effect, no key loss, no
//!   telemetry)
//! - The denial-before-erase ordering probe and its assertion
//!
//! ## Does not own
//! - The erasure workflow itself (`erase.rs`)
//! - The happy-path, authorization, and confirm coverage (`erase_tests.rs`)
//! - The journal's file mechanics (`erasure_journal_file.rs`)
//!
//! ## Boundary
//!
//! Tests here break when the failing-store semantics of the erasure workflow
//! change; they never change it.
//!
//! Keep this module focused on the failing-store failure semantics.

use crate::erase::{EraseArgs, EraseVecSink, erase_request};
use crate::erasure_journal::{JournalState, load_journal};
use crate::inventory_fixture::{FixtureDir, semantic_item};
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, ComplianceTombstoneView, DekProvider, ItemIdentity, LocalDevKms, SqliteStore,
    Store, SubjectEraseOutcome, SubjectEraseSpec, sha256_hex,
};
use clio_types::{Actor, Bank, ErrorCode, OpContext, PrincipalLabel};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Test double: erase always fails; optionally reports the DEK as destroyed
/// and optionally probes the erasure journal at the store call.
struct MockEraseStore {
    destroy: bool,
    destroyed: Mutex<bool>,
    /// Armed by the ordering test to observe the journal on disk when the
    /// store call runs; `None` keeps the double unarmed.
    probe: Option<EraseProbe>,
}

/// The denial observation one armed store call records: at `erase_subject`
/// time the journal is loaded from disk and whether it denied the armed
/// subject hash is kept for the test to read afterwards.
struct EraseProbe {
    journal_path: PathBuf,
    subject_hash: String,
    denied_at_store_call: Mutex<Option<bool>>,
}

impl EraseProbe {
    /// Arm the probe with the journal path the production path writes and the
    /// one-way subject hash it records there.
    fn arm(journal_path: &Path, subject_hash: &str) -> Self {
        Self {
            journal_path: journal_path.to_path_buf(),
            subject_hash: subject_hash.to_owned(),
            denied_at_store_call: Mutex::new(None),
        }
    }

    /// Load the journal from disk and record whether the current authority
    /// denied the armed hash; runs at the store call, before the injected
    /// failure returns.
    fn observe(&self) {
        let denied = matches!(
            load_journal(&self.journal_path),
            JournalState::Current(journal) if journal.is_denied(&self.subject_hash)
        );
        *self.denied_at_store_call.lock().expect("probe") = Some(denied);
    }

    /// Whether the journal denied the armed hash when the store call ran;
    /// panics unless the store call actually reached the probe.
    fn denied_at_store_call(&self) -> bool {
        self.denied_at_store_call
            .lock()
            .expect("probe")
            .expect("the store call must reach the probe")
    }
}

impl ComplianceStore for MockEraseStore {
    fn list_subject_items(
        &self,
        _bank_id: &str,
        _subject_id: &str,
    ) -> Result<Vec<ItemIdentity>, clio_types::AmError> {
        Ok(vec![])
    }

    fn erase_subject(
        &self,
        _spec: &SubjectEraseSpec<'_>,
    ) -> Result<SubjectEraseOutcome, clio_types::AmError> {
        if let Some(probe) = &self.probe {
            probe.observe();
        }
        if self.destroy {
            *self.destroyed.lock().expect("mock") = true;
        }
        Err(clio_types::AmError::new(
            ErrorCode::Internal,
            "injected erase failure",
        ))
    }

    fn subject_dek_destroyed(&self, _subject_id: &str) -> Result<bool, clio_types::AmError> {
        Ok(*self.destroyed.lock().expect("mock"))
    }

    fn read_tombstones(
        &self,
        _bank_id: &str,
        _request_id: &str,
    ) -> Result<Vec<ComplianceTombstoneView>, clio_types::AmError> {
        Ok(vec![])
    }

    fn list_tombstones(
        &self,
        _bank_id: &str,
    ) -> Result<Vec<ComplianceTombstoneView>, clio_types::AmError> {
        Ok(vec![])
    }
}

#[test]
fn partial_shred_reports_processing_and_plain_error_propagates() {
    let sink = EraseVecSink::default();
    let ctx = OpContext::new(Bank::new("mock-bank"), Actor::Harness);
    let journal_dir = FixtureDir::create(&unique("erase-journal"));
    let journal = journal_dir.path().join("erasure-journal.json");

    let destroyed = MockEraseStore {
        destroy: true,
        destroyed: Mutex::new(false),
        probe: None,
    };
    // A fresh salt per run: no test asserts the tombstone hash value.
    let salt = rand::random::<[u8; 16]>();
    let e = EraseArgs {
        store: &destroyed,
        journal_path: &journal,
        maint: None,
        ctx: &ctx,
        subject_id: "subject-1",
        legal_basis: "CCPA_DELETION",
        request_id: "req-partial",
        reason_code: Some("CUSTOM_REASON"),
        confirm: true,
        salt: &salt,
        timestamp: "2026-09-17T11:00:00Z",
        sink: &sink,
    };
    let out = erase_request(&e).unwrap();
    assert_eq!(out.status, "PROCESSING");
    assert_eq!(out.item_count, 0);
    assert_eq!(sink.events().len(), 1);
    assert_eq!(sink.events()[0].reason_code, "CUSTOM_REASON");

    let failed = MockEraseStore {
        destroy: false,
        destroyed: Mutex::new(false),
        probe: None,
    };
    let e = EraseArgs {
        store: &failed,
        journal_path: &journal,
        maint: None,
        ctx: &ctx,
        subject_id: "subject-1",
        legal_basis: "CCPA_DELETION",
        request_id: "req-fail",
        reason_code: None,
        confirm: true,
        salt: &salt,
        timestamp: "2026-09-17T11:00:00Z",
        sink: &sink,
    };
    assert_eq!(erase_request(&e).unwrap_err().code, ErrorCode::Internal);
}

#[test]
fn a_partial_erasure_event_also_names_the_credential() {
    let sink = EraseVecSink::default();
    let ctx = OpContext::with_principal(
        Bank::new("mock-bank"),
        Actor::Harness,
        PrincipalLabel::new("ops-key"),
    );
    let journal_dir = FixtureDir::create(&unique("erase-journal"));
    let journal = journal_dir.path().join("erasure-journal.json");
    let destroyed = MockEraseStore {
        destroy: true,
        destroyed: Mutex::new(false),
        probe: None,
    };
    // A fresh salt per run: no test asserts the tombstone hash value.
    let salt = rand::random::<[u8; 16]>();
    let out = erase_request(&EraseArgs {
        store: &destroyed,
        journal_path: &journal,
        maint: None,
        ctx: &ctx,
        subject_id: "subject-1",
        legal_basis: "CCPA_DELETION",
        request_id: "req-partial",
        reason_code: None,
        confirm: true,
        salt: &salt,
        timestamp: "2026-09-17T11:00:00Z",
        sink: &sink,
    })
    .unwrap();
    assert_eq!(out.status, "PROCESSING");
    assert_eq!(sink.events()[0].principal.as_deref(), Some("ops-key"));
}

/// A journal write that fails refuses the erasure before the store runs: the
/// durable denial precedes key loss, so nothing is destroyed, nothing is
/// written, and no telemetry is emitted.
#[test]
fn a_failed_journal_write_refuses_the_erasure_before_any_store_effect() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(kms).expect("open the fixture store");
    let bank = unique("erase-bank");
    let subject = unique("subject");
    let item_id = unique("item");
    store
        .create_memory_item(&semantic_item(&item_id, &bank, &subject), "tester")
        .expect("seed the subject's item");

    let sink = EraseVecSink::default();
    let ctx = OpContext::new(Bank::new(bank.clone()), Actor::Harness);
    // The journal's parent is deterministically obstructed (a fresh per-test
    // directory holds a regular file where the journal's directory should
    // be, and the call itself creates nothing under): the read classifies
    // the path as missing and neither the journal lock nor the write can
    // create the parent, which is the `Internal` refusal this probe is
    // about.
    let dir = FixtureDir::create(&unique("erase-journal-unwritable"));
    let obstruction = dir.path().join("absent");
    std::fs::write(&obstruction, b"not a directory").expect("plant the obstructing file");
    let journal = obstruction.join("erasure-journal.json");
    let salt = rand::random::<[u8; 16]>();
    let err = erase_request(&EraseArgs {
        store: &store,
        journal_path: &journal,
        maint: None,
        ctx: &ctx,
        subject_id: &subject,
        legal_basis: "GDPR_ART_17",
        request_id: "req-journal-refused",
        reason_code: None,
        confirm: true,
        salt: &salt,
        timestamp: "2026-09-17T11:00:00Z",
        sink: &sink,
    })
    .expect_err("an unwritable journal must refuse the erasure");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert!(sink.events().is_empty(), "{:?}", sink.events());
    assert!(
        !store
            .subject_dek_destroyed(&subject)
            .expect("probe the subject's key"),
        "the subject's key must survive a refused erasure"
    );
    let tombstones = store
        .read_tombstones(&bank, "req-journal-refused")
        .expect("read the request's tombstones");
    assert!(tombstones.is_empty(), "no store effect: {tombstones:?}");
    assert!(
        store
            .get_memory_item(&bank, &item_id)
            .expect("read the subject's item")
            .is_some(),
        "the subject's item must stay readable"
    );
}

/// The durable denial is already on disk before the store is asked to erase:
/// the armed double loads the journal at `erase_subject` time and sees the
/// subject denied, so the ordering is observed dynamically at the store call,
/// not only inferred from code order.
#[test]
fn the_denial_is_durable_on_disk_before_the_store_is_asked_to_erase() {
    let sink = EraseVecSink::default();
    let ctx = OpContext::new(Bank::new("mock-bank"), Actor::Harness);
    let journal_dir = FixtureDir::create(&unique("erase-journal"));
    let journal = journal_dir.path().join("erasure-journal.json");
    // A known salt, so this test derives the one-way subject hash exactly as
    // `erase.rs` does: `sha256_hex(salt, subject_id)`.
    let salt = rand::random::<[u8; 16]>();
    let subject = "subject-1";
    let expected_hash = sha256_hex(&salt, subject);
    let store = MockEraseStore {
        destroy: false,
        destroyed: Mutex::new(false),
        probe: Some(EraseProbe::arm(&journal, &expected_hash)),
    };

    let err = erase_request(&EraseArgs {
        store: &store,
        journal_path: &journal,
        maint: None,
        ctx: &ctx,
        subject_id: subject,
        legal_basis: "CCPA_DELETION",
        request_id: "req-ordering",
        reason_code: None,
        confirm: true,
        salt: &salt,
        timestamp: "2026-09-17T11:00:00Z",
        sink: &sink,
    })
    .expect_err("the injected store failure must propagate");
    assert_eq!(err.code, ErrorCode::Internal, "{}", err.message);
    assert_eq!(err.message, "injected erase failure");

    assert!(
        store
            .probe
            .as_ref()
            .expect("the probe is armed")
            .denied_at_store_call(),
        "the journal must already deny the subject hash when erase_subject runs"
    );
    let JournalState::Current(loaded) = load_journal(&journal) else {
        panic!("a failed store call must leave the recorded denial current");
    };
    assert!(
        loaded.is_denied(&expected_hash),
        "the durable denial must survive the failed store call"
    );
}
