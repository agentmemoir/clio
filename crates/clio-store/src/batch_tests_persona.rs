// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite batch persona-write and
//! record-context coverage: batch persona writes must emit the same
//! audit rows as the single-op path, and a divergent or oversized record
//! context must fail closed before any write in the batch.
//!
//! ## Owns
//! - Batch persona audit-row assertions
//! - Batch record-context fail-closed assertions
//!
//! ## Does not own
//! - Shared fixtures (see `batch_test_support_tests`)
//! - Atomic-commit and `BatchWrite` variant coverage (see `batch_tests`)
//! - Postgres parity tests (see `batch_pg_tests`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.
//!
//! Keep this module focused on batch persona and record-context coverage.

use crate::batch_model::{BatchStore, BatchWrite};
use crate::model::ItemInspectFilter;
use crate::store::Store;
use clio_types::{EpisodicType, ErrorCode, ItemKind, PersonaPreference, PersonaStableEntry};

use crate::batch_test_support_tests::{item, store};
use crate::test_unique::unique;

/// Batch persona writes emit the same audit rows as the single-op path
/// (presence/length/hash, never the raw context; covers every
/// store/update operation, including batch-executed ones).
#[test]
fn sqlite_batch_persona_writes_emit_audit_rows() {
    let store = store();
    let bank = unique("b");
    let stable = PersonaStableEntry {
        bank_id: bank.clone(),
        key: "style".into(),
        value: "concise".into(),
        confidence: Some(0.5),
        source: "explicit".into(),
        source_ref: None,
        context: Some("batch stable note".into()),
        admission_score: 0.5,
        as_of: "t".into(),
        valid_until: None,
    };
    let pref = PersonaPreference {
        bank_id: bank.clone(),
        key: "brevity_affinity".into(),
        value: 0.5,
        trend: 0.5,
        confidence: None,
        source: "inferred".into(),
        source_ref: None,
        context: Some("week one note".into()),
        admission_score: 0.5,
        as_of: "t".into(),
    };
    store
        .apply_batch(&[
            BatchWrite::PutStableEntry {
                entry: stable,
                actor: "batch-actor".into(),
            },
            BatchWrite::PutPreferenceState {
                preference: pref,
                actor: "batch-actor".into(),
            },
        ])
        .unwrap();

    let rows: Vec<(String, String, String)> = store
        .with_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT operation, actor, detail_json FROM audit_events
                      WHERE bank_id = ?1 AND operation LIKE 'persona_%'
                      ORDER BY created_at, id",
                )
                .expect("audit prepare");
            let rows = stmt
                .query_map(rusqlite::params![bank], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .expect("audit query");
            Ok(rows.collect::<Result<Vec<_>, _>>().expect("audit rows map"))
        })
        .expect("audit rows");
    assert_eq!(
        rows.len(),
        2,
        "one audit row per batch persona write: {rows:?}"
    );
    assert!(rows.iter().all(|(_, actor, _)| actor == "batch-actor"));
    for raw in ["batch stable note", "week one note"] {
        for (_, _, d) in &rows {
            assert!(!d.contains(raw), "raw context leaked into audit: {d}");
        }
    }
    let stable: serde_json::Value = serde_json::from_str(rows[0].2.as_str()).unwrap();
    assert_eq!(stable["context_action"], "set");
    assert_eq!(stable["context_present"], serde_json::json!(true));
    let pref: serde_json::Value = serde_json::from_str(rows[1].2.as_str()).unwrap();
    assert_eq!(pref["context_action"], "set");
    assert_eq!(pref["context_present"], serde_json::json!(true));
}

#[test]
fn sqlite_batch_task_record_context_fails_closed_before_any_write() {
    use clio_types::TaskRecord;
    let store = store();
    let bank = unique("bctx");
    let carrier_id = format!("carrier-{}", unique("c"));
    let mut carrier = item(&carrier_id, &bank);
    carrier.kind = ItemKind::Episodic;
    carrier.category = None;
    carrier.episodic_type = Some(EpisodicType::Task);
    let mut record = TaskRecord {
        id: unique("task"),
        bank_id: bank.clone(),
        item_id: carrier_id.clone(),
        task_id: "task-ctx".into(),
        subject_id: carrier.subject_id.clone(),
        version: 1,
        definition: "fix the build".into(),
        steps: vec!["edit".into()],
        status: "in_progress".into(),
        evidence_ref: None,
        context: Some("x".repeat(4097)),
        admission_score: 0.7,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    let err = store
        .apply_batch(&[BatchWrite::CommitTaskUpsert {
            item: carrier.clone(),
            record: record.clone(),
            actor: "agent".into(),
        }])
        .expect_err("an oversized record context must abort the whole batch");
    assert_eq!(err.code, ErrorCode::OutOfRange, "{err}");
    assert_eq!(
        store.count_items(&ItemInspectFilter::new(&bank)).unwrap(),
        0,
        "the batch must not leave a carrier row behind"
    );

    // Bounded but divergent (carrier stores no context) is still rejected.
    record.context = Some("bounded note".into());
    let err = store
        .apply_batch(&[BatchWrite::CommitTaskUpsert {
            item: carrier.clone(),
            record: record.clone(),
            actor: "agent".into(),
        }])
        .expect_err("a divergent record context must abort the whole batch");
    assert_eq!(err.code, ErrorCode::InvalidArgument, "{err}");
    assert_eq!(
        store.count_items(&ItemInspectFilter::new(&bank)).unwrap(),
        0
    );

    // A consistent pair commits.
    carrier.context = Some("bounded note".into());
    store
        .apply_batch(&[BatchWrite::CommitTaskUpsert {
            item: carrier,
            record,
            actor: "agent".into(),
        }])
        .expect("consistent carrier/record context commits");
    assert_eq!(
        store.count_items(&ItemInspectFilter::new(&bank)).unwrap(),
        1
    );
}
