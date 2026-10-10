// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for context-field store contract tests: sealed
//! round trips, audit exposure, legacy payload compatibility, and erasure
//! behavior on both backends.
//!
//! ## Owns
//! - Context round-trip, audit, legacy-payload, and erasure test scenarios
//!
//! ## Does not own
//! - Test fixtures (see `erase_test_support`) or item CRUD suites
//!   (`memory_item_tests`)
//!
//! ## Boundary
//!
//! Changes to context persistence scenarios should normally require changes
//! here. Changes to the general item CRUD suite should not.
//!
//! Test-only. Never compiled into production builds.

use crate::compliance_store::ComplianceStore;
use crate::content_envelope::DualContent;
use crate::dek::{DekProvider, LocalDevKms};
use crate::erase_test_support::{local_kms, memory_item, spec, unique};
use crate::postgres::PostgresStore;
use crate::sqlite::SqliteStore;
use crate::store::Store;
use clio_types::{ErrorCode, MemoryItem};
use std::sync::Arc;

fn with_context(mut item: MemoryItem, context: &str) -> MemoryItem {
    item.context = Some(context.into());
    item
}

/// Round trip: context reads back, stays sealed at rest, and appears in the
/// audit revision while telemetry carries stats only.
fn assert_context_round_trip(store: &dyn Store, bank: &str) {
    let id = unique("ctx");
    let item = with_context(memory_item(bank, &id, "ctx-subject"), "team chat");
    store.create_memory_item(&item, "tester").unwrap();
    let got = store.get_memory_item(bank, &id).unwrap().expect("present");
    assert_eq!(got.context.as_deref(), Some("team chat"));
    let raw = store
        .inspect_content_ciphertext(bank, &id)
        .unwrap()
        .unwrap();
    assert!(
        !raw.contains("team chat"),
        "context plaintext leaked: {raw}"
    );

    let trail = store.audit_trail(bank, &id).unwrap();
    assert_eq!(trail.revisions.len(), 1);
    assert_eq!(trail.revisions[0].context.as_deref(), Some("team chat"));
    let create = trail
        .events
        .iter()
        .find(|e| e.operation == "create_memory_item")
        .expect("create event");
    assert_eq!(create.detail["context_present"], true);
    assert_eq!(create.detail["context_len"], "team chat".len());
    assert!(create.detail["context_hash"].is_string());
    let telemetry = serde_json::to_string(&trail.events).unwrap();
    assert!(
        !telemetry.contains("team chat"),
        "raw context leaked into telemetry events"
    );
}

/// An item stored without context stays absent: no empty-string sentinel.
fn assert_context_absent_item(store: &dyn Store, bank: &str) {
    let id = unique("noctx");
    store
        .create_memory_item(&memory_item(bank, &id, "noctx-subject"), "tester")
        .unwrap();
    let got = store.get_memory_item(bank, &id).unwrap().unwrap();
    assert!(got.context.is_none());
    let trail = store.audit_trail(bank, &id).unwrap();
    assert!(trail.revisions[0].context.is_none());
    let create = trail
        .events
        .iter()
        .find(|e| e.operation == "create_memory_item")
        .expect("create event");
    assert_eq!(create.detail["context_present"], false);
    assert_eq!(create.detail["context_len"], 0);
    assert!(create.detail.get("context_hash").is_none());
}

/// A v1 payload sealed before the context field existed still reads.
fn assert_legacy_payload_reads(
    store: &dyn Store,
    kms: &dyn DekProvider,
    bank: &str,
    exec: &dyn Fn(&str),
) {
    let id = unique("legacy-ctx");
    store
        .create_memory_item(&memory_item(bank, &id, "legacy-subject"), "tester")
        .unwrap();
    let legacy = DualContent::new(None, Some("legacy gist".into()), None).to_bytes();
    let sealed = kms
        .encrypt("legacy-subject", &legacy)
        .unwrap()
        .to_storage_string();
    exec(&format!(
        "UPDATE items SET content_ciphertext = '{sealed}' WHERE id = '{id}'"
    ));
    let got = store.get_memory_item(bank, &id).unwrap().expect("present");
    assert!(got.context.is_none());
    assert_eq!(got.gist.as_deref(), Some("legacy gist"));
}

#[test]
fn sqlite_context_round_trip_audit_and_legacy_payload() {
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = SqliteStore::open_in_memory(Arc::clone(&kms)).unwrap();
    let bank = unique("sqlite-ctx");
    assert_context_round_trip(&store, &bank);
    assert_context_absent_item(&store, &bank);
    assert_legacy_payload_reads(&store, kms.as_ref(), &bank, &|sql| {
        store.test_execute(sql).unwrap();
    });
}

#[test]
fn postgres_context_parity() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let kms: Arc<dyn DekProvider> = Arc::new(LocalDevKms::new());
    let store = PostgresStore::open(&url, Arc::clone(&kms))
        .unwrap_or_else(|e| panic!("postgres required: {e}"));
    let bank = unique("pg-ctx");
    assert_context_round_trip(&store, &bank);
    assert_context_absent_item(&store, &bank);
    assert_legacy_payload_reads(&store, kms.as_ref(), &bank, &|sql| {
        store.test_execute(sql).unwrap();
    });
}

#[test]
fn sqlite_context_follows_subject_erasure() {
    let store = SqliteStore::open_in_memory(local_kms()).unwrap();
    let bank = unique("sqlite-ctx-erase");
    let subject = unique("ctx-subject");
    let id = unique("ctx-erase-item");
    let item = with_context(memory_item(&bank, &id, &subject), "team chat");
    store.create_memory_item(&item, "tester").unwrap();
    store
        .erase_subject(&spec(&bank, &subject, &unique("req")))
        .unwrap();
    assert_eq!(
        store.get_memory_item(&bank, &id).unwrap_err().code,
        ErrorCode::ErasedSubject
    );
    let trail = store.audit_trail(&bank, &id).unwrap();
    assert_eq!(trail.revisions.len(), 1);
    assert!(
        trail.revisions[0].context.is_none(),
        "erased context must not be readable"
    );
    let serialized = serde_json::to_string(&trail).unwrap();
    assert!(!serialized.contains("team chat"));
}
