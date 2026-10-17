// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres parity tests of the atomic
//! batch executor: the same all-or-nothing contract and full `BatchWrite`
//! variant coverage as SQLite, plus canonical subject-keyed reads and audit
//! telemetry on the Postgres backend.
//!
//! ## Owns
//! - Postgres `apply_batch` parity assertions (skip only when the operator sets
//!   `CLIO_SKIP_PG_TESTS`; an unreachable Postgres otherwise fails the test)
//!
//! ## Does not own
//! - Shared fixtures (see `batch_test_support_tests`)
//! - SQLite batch tests (see `batch_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use super::batch_subject_ids;
use crate::batch_model::{BatchStore, BatchWrite, MAX_BATCH_OPS};
use crate::db_url::postgres_url_or_default;
use crate::dek::{DekProvider, LocalDevKms};
use crate::model::{AssocEdgeRecord, AssocKind, OpaqueItemRecord};
use crate::postgres::PostgresStore;
use crate::store::Store;
use clio_types::{BeliefObject, ConfidenceEntry, ErrorCode, SourceType};
use std::sync::Arc;

use crate::batch_test_support_tests::{
    assert_all_or_nothing, extended_writes, item, record_shaped_writes, triple, triple_item,
};
use crate::test_unique::unique;

/// True only when the operator explicitly opted out of Postgres-backed tests.
/// Without this opt-out an unreachable Postgres fails the parity test instead
/// of silently reporting green.
fn skip_requested() -> bool {
    matches!(
        std::env::var("CLIO_SKIP_PG_TESTS").as_deref(),
        Ok("1" | "true" | "yes")
    )
}

#[test]
fn batch_subject_locks_are_sorted_and_deduplicated() {
    let bank = "lock-order-bank";
    let mut later_subject = item("lock-order-later", bank);
    later_subject.subject_id = "subject-z".into();
    let mut earlier_subject = item("lock-order-earlier", bank);
    earlier_subject.subject_id = "subject-a".into();
    let opaque = OpaqueItemRecord {
        id: "lock-order-opaque".into(),
        bank_id: bank.into(),
        subject_id: "subject-a".into(),
        kind: "semantic".into(),
        category: Some("schema".into()),
        episodic_type: None,
        epistemic_kind: "fact".into(),
        source_type: None,
        confidence: None,
        admission_score: None,
        source_ref: None,
        valid_from: "t0".into(),
        valid_until: None,
        tx_from: "t0".into(),
        tx_until: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        content_cipher: "opaque".into(),
    };
    let writes = [
        BatchWrite::CreateItem {
            item: later_subject,
            actor: "agent".into(),
        },
        BatchWrite::PutOpaqueItem {
            item: opaque,
            actor: "agent".into(),
        },
        BatchWrite::CanonicalPut {
            item: earlier_subject,
            key: "key".into(),
            actor: "agent".into(),
        },
        BatchWrite::CreateBelief {
            belief: BeliefObject {
                id: "lock-order-belief".into(),
                bank_id: bank.into(),
                subject_id: "subject-m".into(),
                proposition: "test proposition".into(),
                context: None,
                context_as_of: None,
                confidence_history: vec![
                    ConfidenceEntry::new("t0", 0.8, "turn", SourceType::UserStated).unwrap(),
                ],
                created_at: "t0".into(),
                updated_at: "t0".into(),
            },
            actor: "agent".into(),
        },
    ];

    assert_eq!(
        batch_subject_ids(&writes),
        ["subject-a", "subject-m", "subject-z"]
    );
}

#[test]
#[allow(clippy::too_many_lines)]
#[allow(clippy::float_cmp)] // exact restored values from the batch payload
fn postgres_batch_parity() {
    let _guard = crate::pg_lock::lock_pg_test();
    let url = postgres_url_or_default(std::env::var("CLIO_DATABASE_URL"));
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    let pg = match PostgresStore::open(&url, kms) {
        Ok(pg) => pg,
        Err(err) if skip_requested() => {
            eprintln!("skipping postgres batch parity (CLIO_SKIP_PG_TESTS): {err}");
            return;
        }
        Err(err) => panic!("postgres batch parity requires Postgres at {url}: {err}"),
    };
    assert_all_or_nothing(&pg);

    // Full variant coverage on Postgres: supersession, triples, history
    // records, persona, assoc edges, and audit rows in ONE transaction.
    let bank = unique("pb");
    let id = unique("pt");
    let mut writes = vec![
        BatchWrite::CommitTriple {
            item: triple_item(&id, &bank),
            edge: triple(&bank, &id),
            supersede: true,
            actor: "agent".into(),
        },
        BatchWrite::CloseItem {
            bank_id: bank.clone(),
            item_id: id.clone(),
            valid_until: "2026-01-02T00:00:00Z".into(),
            tx_until: "2026-01-02T00:00:00Z".into(),
            replacement_id: Some("pb-replacement".into()),
            actor: "agent".into(),
        },
        BatchWrite::PutAssocEdge {
            edge: AssocEdgeRecord {
                id: unique("edge"),
                bank_id: bank.clone(),
                source_id: id.clone(),
                target_id: "pb-target".into(),
                assoc_kind: AssocKind::Explicit,
                relationship: Some("replaced_by".into()),
                weight: 0.5,
                last_updated_at: "2026-01-02T00:00:00Z".into(),
            },
            actor: "agent".into(),
        },
        BatchWrite::AuditEvent {
            bank_id: bank.clone(),
            operation: "batch_test".into(),
            item_id: Some(id.clone()),
            actor: "agent".into(),
            detail: serde_json::json!({ "k": "v" }),
        },
    ];
    writes.extend(record_shaped_writes(&bank));
    writes.extend(extended_writes(&bank));
    pg.apply_batch(&writes).unwrap();

    // Triple + supersession + assoc edge + audit event.
    assert!(
        pg.get_triple(&bank, &format!("edge-{id}"))
            .unwrap()
            .is_some()
    );
    let triple_row = pg.get_memory_item(&bank, &id).unwrap().unwrap();
    assert_eq!(triple_row.tx_until.as_deref(), Some("2026-01-02T00:00:00Z"));
    assert_eq!(pg.list_assoc_edges(&bank, None).unwrap().len(), 1);
    let trail = pg.audit_trail(&bank, &id).unwrap();
    assert!(trail.events.iter().any(|e| e.operation == "batch_test"));
    let tasks = crate::history_store::HistoryStore::list_tasks(&pg, &bank, 10).unwrap();
    assert_eq!(tasks.len(), 1);
    let history = crate::history_store::HistoryStore::list_failures(&pg, &bank, 10).unwrap();
    assert_eq!(history.len(), 1);
    let doc = crate::persona_store::PersonaStore::get_persona_document(&pg, &bank).unwrap();
    assert!(!doc.stable.is_empty(), "{:?}", doc.stable);

    // Import-facing variants land on Postgres too: forced overwrite, opaque
    // raw ciphertext + metadata, preference upsert, belief create + append.
    let rows = pg
        .inspect_items(&crate::model::ItemInspectFilter::new(&bank))
        .unwrap();
    let opaque = rows
        .iter()
        .find(|r| r.item_id.starts_with("opq-"))
        .expect("opaque row listed");
    assert_eq!(opaque.category.as_deref(), Some("schema"));
    assert_eq!(
        pg.inspect_content_ciphertext(&bank, &opaque.item_id)
            .unwrap()
            .as_deref(),
        Some("opaque-cipher")
    );
    let upd = rows
        .iter()
        .find(|r| r.item_id.starts_with("upd-"))
        .expect("upserted item listed");
    assert_eq!(
        pg.get_memory_item(&bank, &upd.item_id)
            .unwrap()
            .unwrap()
            .gist
            .as_deref(),
        Some("updated gist")
    );
    let pref = crate::persona_store::PersonaStore::get_preference(&pg, &bank, "brevity_affinity")
        .unwrap()
        .unwrap();
    assert_eq!(pref.value, 0.7);
    let beliefs = crate::belief_store::BeliefStore::list_beliefs(&pg, &bank).unwrap();
    assert_eq!(beliefs.len(), 1);
    assert_eq!(beliefs[0].confidence_history.len(), 2);

    // Canonical reads on Postgres: subject-keyed active + point-in-time.
    let subject = format!("canon-{}", unique("s"));
    let mut first = item(&unique("c1"), &bank);
    first.subject_id = subject.clone();
    first.gist = Some("v1".into());
    pg.create_memory_item(&first, "agent").unwrap();
    let got = pg
        .get_item_by_subject(&bank, &subject, None)
        .unwrap()
        .expect("active");
    assert_eq!(got.id, first.id);
    let mut second = item(&unique("c2"), &bank);
    second.subject_id = subject.clone();
    second.gist = Some("v2".into());
    second.valid_from = "2026-02-01T00:00:00Z".into();
    second.tx_from = "2026-02-01T00:00:00Z".into();
    second.created_at = "2026-02-01T00:00:00Z".into();
    second.updated_at = "2026-02-01T00:00:00Z".into();
    pg.create_memory_item(&second, "agent").unwrap();
    // Both revisions share the subject; the active read returns the latest.
    let active = pg
        .get_item_by_subject(&bank, &subject, None)
        .unwrap()
        .expect("active");
    assert_eq!(active.gist.as_deref(), Some("v2"));
    let historical = pg
        .get_item_by_subject(&bank, &subject, Some("2026-01-15T00:00:00Z"))
        .unwrap()
        .expect("historical");
    assert_eq!(historical.gist.as_deref(), Some("v1"));

    // CanonicalPut executes in-transaction on Postgres too: two puts on one key
    // leave exactly one active revision (the first is closed, not deleted).
    let canon_key = unique("ck");
    let canon_subject = format!("canonical:{canon_key}");
    let mut c1 = item(&unique("cq1"), &bank);
    c1.subject_id = canon_subject.clone();
    c1.gist = Some("v1".into());
    c1.valid_from = "2026-03-01T00:00:00Z".into();
    c1.tx_from = "2026-03-01T00:00:00Z".into();
    let mut c2 = item(&unique("cq2"), &bank);
    c2.subject_id = canon_subject.clone();
    c2.gist = Some("v2".into());
    c2.valid_from = "2026-03-02T00:00:00Z".into();
    c2.tx_from = "2026-03-02T00:00:00Z".into();
    pg.apply_batch(&[
        BatchWrite::CanonicalPut {
            item: c1.clone(),
            key: canon_key.clone(),
            actor: "agent".into(),
        },
        BatchWrite::CanonicalPut {
            item: c2.clone(),
            key: canon_key,
            actor: "agent".into(),
        },
    ])
    .unwrap();
    let active = pg
        .get_item_by_subject(&bank, &canon_subject, None)
        .unwrap()
        .expect("active canonical");
    assert_eq!(active.id, c2.id);
    assert!(active.tx_until.is_none());
    let closed = pg.get_memory_item(&bank, &c1.id).unwrap().unwrap();
    assert_eq!(closed.tx_until.as_deref(), Some("2026-03-02T00:00:00Z"));
    assert!(closed.valid_until.is_some());

    // Generic audit events on Postgres (workspace tool telemetry).
    pg.record_audit_event(
        &bank,
        "validate_attest",
        Some(&id),
        "agent",
        &serde_json::json!({"note": "ok"}),
    )
    .unwrap();
    let trail = pg.audit_trail(&bank, &id).unwrap();
    assert!(
        trail
            .events
            .iter()
            .any(|e| e.operation == "validate_attest")
    );

    // Validation edges on Postgres mirror the SQLite executor: carrier-id
    // mismatch and out-of-range assoc weight fail closed before any commit.
    let mismatched = {
        let mut c = triple_item(&unique("mm"), &bank);
        c.id = format!("{}-other", c.id);
        c
    };
    let edge_id = triple(&bank, &unique("e2")).id;
    let err = pg
        .apply_batch(&[BatchWrite::CommitTriple {
            item: {
                let mut c = triple_item(&unique("e2c"), &bank);
                c.id = format!("{}-other", c.id);
                c
            },
            edge: {
                let mut e = triple(&bank, "whatever");
                e.id = edge_id;
                e
            },
            supersede: true,
            actor: "agent".into(),
        }])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let err = pg
        .apply_batch(&[BatchWrite::PutAssocEdge {
            edge: AssocEdgeRecord {
                id: unique("edge"),
                bank_id: bank.clone(),
                source_id: "a".into(),
                target_id: "b".into(),
                assoc_kind: AssocKind::Explicit,
                relationship: Some("r".into()),
                weight: 1.5,
                last_updated_at: "2026-01-01T00:00:00Z".into(),
            },
            actor: "agent".into(),
        }])
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::OutOfRange);
    let _ = mismatched;

    // The per-batch cap also fails closed on Postgres.
    let many: Vec<BatchWrite> = (0..=MAX_BATCH_OPS)
        .map(|i| BatchWrite::CreateItem {
            item: item(&format!("pg-itm-{i}"), &bank),
            actor: "agent".into(),
        })
        .collect();
    let err = pg.apply_batch(&many).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(err.message.contains("cap"));
}
