// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module holds the shared test fixtures for the request-facade suites:
//! the in-memory store, the scope, the stored-record builder, the copy-row
//! builder, and the retry item, arguments, and context.
//!
//! ## Owns
//! - The in-memory store fixture, the record fixture, and the copy-row fixture
//! - The shared retry fixtures: the semantic item, the persist helper, the
//!   harness context, and the erase arguments
//!
//! ## Does not own
//! - The facade under test (`subject_request_service.rs`,
//!   `subject_request_report.rs`)
//! - The test assertions themselves (`subject_request_service_tests.rs`,
//!   `subject_request_service_report_tests.rs`,
//!   `subject_request_service_pg_tests.rs`)
//!
//! ## Boundary
//!
//! Changes to the shared fixtures should normally require changes here; changes
//! to one suite's assertions should not.
//!
//! Test-only. Never compiled into production builds.

use crate::erase::{EraseArgs, EraseSink};
use crate::subject_request::{
    RequestKind, RequestScope, RequestState, SubjectRequestLedger, SubjectRequestRecord,
};
use crate::subject_request_service::SubjectRequestEnv;
use crate::subject_request_store_wiring::record_to_row;
use crate::test_unique::unique;
use clio_store::{
    ComplianceStore, CopyInventoryRow, DekProvider, DurableRequestStore, LocalDevKms, SqliteStore,
};
use clio_types::{Actor, Bank, EpistemicKind, ItemKind, MemoryItem, OpContext, SemanticCategory};
use std::path::Path;
use std::sync::Arc;

/// The fixed timestamp every retry fixture writes.
pub(crate) const NOW: &str = "2026-01-01T00:00:00Z";

/// An in-memory SQLite store exposing every store trait the facade needs.
pub(crate) fn store() -> SqliteStore {
    SqliteStore::open_in_memory(Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>)
        .expect("in-memory sqlite opens")
}

/// The facade environment over one store handle.
pub(crate) fn env(store: &SqliteStore) -> SubjectRequestEnv<'_> {
    SubjectRequestEnv {
        durable: store,
        events: store,
        peers: store,
        copies: store,
    }
}

/// The fixed subject scope the fixtures bind to.
pub(crate) fn scope() -> RequestScope {
    RequestScope::Subject {
        bank_id: "bank-a".to_owned(),
        subject_id: "subject-1".to_owned(),
    }
}

/// A stored record in a chosen state, with no enabled peers.
pub(crate) fn record(request_id: &str, state: RequestState) -> SubjectRequestRecord {
    let mut ledger = SubjectRequestLedger::new();
    ledger
        .register(request_id, RequestKind::Erasure, &scope(), &[])
        .expect("register the fixture request");
    let mut record = ledger.record(request_id).cloned().expect("fixture record");
    record.state = state;
    record
}

/// Persist one record's current state to the durable store.
pub(crate) fn persist(durable: &dyn DurableRequestStore, record: &SubjectRequestRecord) {
    durable.save(&record_to_row(record)).expect("the row saves");
}

/// A harness context in `bank`.
pub(crate) fn ctx(bank: &str) -> OpContext {
    OpContext::new(Bank::new(bank.to_owned()), Actor::Harness)
}

/// One semantic item carrying one data subject, so an erasure has work to do.
pub(crate) fn item(bank: &str, subject: &str) -> MemoryItem {
    MemoryItem {
        id: unique("svc-item"),
        bank_id: bank.to_owned(),
        subject_id: subject.to_owned(),
        kind: ItemKind::Semantic,
        category: Some(SemanticCategory::parse("persona").expect("category")),
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.8),
        source_ref: None,
        snapshot: Some(serde_json::json!({"entity": "secret"})),
        gist: Some("secret gist".into()),
        valid_from: NOW.to_owned(),
        valid_until: None,
        tx_from: NOW.to_owned(),
        tx_until: None,
        created_at: NOW.to_owned(),
        updated_at: NOW.to_owned(),
        context: None,
    }
}

/// Erase arguments for one request over `store`, `ctx`, and a scratch journal.
pub(crate) fn retry_args<'a>(
    store: &'a dyn ComplianceStore,
    ctx: &'a OpContext,
    sink: &'a dyn EraseSink,
    journal: &'a Path,
    request_id: &'a str,
    subject_id: &'a str,
    salt: &'a [u8],
) -> EraseArgs<'a> {
    EraseArgs {
        store,
        journal_path: journal,
        maint: None,
        ctx,
        subject_id,
        legal_basis: "GDPR_ART_17",
        request_id,
        reason_code: None,
        confirm: true,
        salt,
        timestamp: NOW,
        sink,
    }
}

/// One copy-inventory row with fixed metadata fields.
pub(crate) fn copy_row(
    copy_id: &str,
    request_id: &str,
    state: &str,
    exception_reason: Option<&str>,
) -> CopyInventoryRow {
    CopyInventoryRow {
        copy_id: copy_id.to_owned(),
        request_id: request_id.to_owned(),
        custody: "clio".to_owned(),
        owner: "harness".to_owned(),
        required_action: "erase".to_owned(),
        state: state.to_owned(),
        exception_reason: exception_reason.map(str::to_owned),
        updated_at: NOW.to_owned(),
    }
}
