// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite audit-trail reconstruction.
//!
//! ## Owns
//! - `audit_trail_impl` lineage, revision, and telemetry assembly
//!
//! ## Does not own
//! - The correction write itself (`sqlite_correct`) or Postgres audit SQL
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::audit_model::CorrectionSpec;
use crate::migrate::IntoAmError;
use crate::sqlite::sqlite_history_tests::{item, store};
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::{EpisodicType, ErrorCode};

#[test]
#[allow(clippy::too_many_lines)]
fn audit_trail_reports_lineage_revisions_and_events() {
    let store = store();
    let bank = unique("sqlite-audit");
    let prior_id = unique("sqlite-audit-prior");
    let mut prior = item(&prior_id, &bank, EpisodicType::Task, false);
    prior.context = Some("rev one".into());
    prior.snapshot = Some(serde_json::json!({"k": "v"}));
    prior.gist = Some("rev one gist".into());
    store.create_memory_item(&prior, "agent").unwrap();

    let available = store.audit_trail_impl(&bank, &prior_id).unwrap();
    assert_eq!(available.lineage_ids, vec![prior_id.clone()]);
    assert_eq!(available.revisions.len(), 1);
    assert_eq!(available.revisions[0].context.as_deref(), Some("rev one"));

    let timestamp = "2026-06-02T00:00:00Z";
    let corrected_id = unique("sqlite-audit-corrected");
    let mut corrected = prior.clone();
    corrected.id.clone_from(&corrected_id);
    corrected.context = Some("rev two".into());
    corrected.valid_from = timestamp.into();
    corrected.tx_from = timestamp.into();
    corrected.created_at = timestamp.into();
    corrected.updated_at = timestamp.into();

    let outcome = store
        .correct_item(&CorrectionSpec {
            bank_id: bank.clone(),
            prior_item_id: prior_id.clone(),
            new_item: corrected,
            reason: "operator fix".into(),
            actor: "agent".into(),
            timestamp: timestamp.into(),
        })
        .unwrap();
    assert_eq!(outcome.new_item_id, corrected_id);

    let trail = store.audit_trail_impl(&bank, &prior_id).unwrap();
    assert_eq!(
        trail.lineage_ids,
        vec![prior_id.clone(), corrected_id.clone()]
    );
    assert_eq!(trail.revisions.len(), 2);
    assert_eq!(trail.revisions[0].context.as_deref(), Some("rev one"));
    assert_eq!(trail.revisions[1].context.as_deref(), Some("rev two"));
    let operations: Vec<&str> = trail.events.iter().map(|e| e.operation.as_str()).collect();
    assert!(operations.contains(&"create_memory_item"), "{operations:?}");
    assert!(operations.contains(&"correct"), "{operations:?}");
    assert!(
        operations.iter().position(|op| *op == "create_memory_item")
            < operations.iter().position(|op| *op == "correct"),
        "{operations:?}"
    );

    let cipher: String = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT content_ciphertext FROM items WHERE id = ?1",
                rusqlite::params![prior_id],
                |r| r.get(0),
            )
            .am("test read")
        })
        .unwrap();
    store
        .test_execute(&format!(
            "UPDATE items SET content_ciphertext = '{cipher}' WHERE id = '{corrected_id}'"
        ))
        .unwrap();
    let rebuilt = store.audit_trail_impl(&bank, &prior_id).unwrap();
    assert_eq!(rebuilt.revisions.len(), 2);
    assert_eq!(rebuilt.revisions[1].context, None);
}

#[test]
fn audit_trail_rejects_unknown_item_and_wrong_bank() {
    let store = store();
    let bank = unique("sqlite-audit-missing");
    let missing = unique("sqlite-audit-absent");
    let err = store.audit_trail_impl(&bank, &missing).unwrap_err();
    assert_eq!(err.code, ErrorCode::ItemNotFound);
    assert_eq!(err.message, format!("item `{missing}` not found in bank"));

    let item_id = unique("sqlite-audit-item");
    store
        .create_memory_item(&item(&item_id, &bank, EpisodicType::Task, false), "agent")
        .unwrap();
    let wrong_bank = store
        .audit_trail_impl(&unique("sqlite-audit-other"), &item_id)
        .unwrap_err();
    assert_eq!(wrong_bank.code, ErrorCode::ItemNotFound);
}
