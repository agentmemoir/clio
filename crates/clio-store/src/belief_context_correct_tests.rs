// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the audited belief-context correction on both
//! storage backends.
//!
//! ## Owns
//! - `SqliteStore::correct_belief_context` and its Postgres counterpart
//!
//! ## Does not own
//! - Belief creation/append persistence (`sqlite_belief`, `postgres_belief`)
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::belief_store::BeliefStore;
use crate::belief_store::belief_store_belief_tests::mk_belief;
use crate::dek::LocalDevKms;
use crate::migrate::IntoAmError;
use crate::postgres_belief::postgres_belief_context_tests::open_pg;
use crate::sqlite::SqliteStore;
use crate::test_unique::unique;
use clio_types::ErrorCode;
use std::sync::Arc;

fn assert_correction_detail(detail: &str) {
    let parsed: serde_json::Value = serde_json::from_str(detail).unwrap();
    assert_eq!(parsed["prior_context_present"], true);
    assert_eq!(parsed["prior_context_len"], "ctx-a".len());
    assert_eq!(parsed["context_len"], "ctx-a2".len());
    assert!(!detail.contains("ctx-a2"), "{detail}");
}

#[test]
fn sqlite_belief_context_correction() {
    let store = SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).unwrap();
    let bank = unique("belief-correct");
    let a_id = unique("belief-correct-a");
    let b_id = unique("belief-correct-b");
    let a_subject = unique("belief-correct-subject-a");
    let b_subject = unique("belief-correct-subject-b");

    let mut a = mk_belief(&a_id, &bank, &a_subject);
    a.context = Some("ctx-a".into());
    store.create_belief(&a, "agent").unwrap();
    let mut b = mk_belief(&b_id, &bank, &b_subject);
    b.proposition = unique("belief-correct-proposition-b");
    b.context = Some("ctx-b".into());
    store.create_belief(&b, "agent").unwrap();

    store
        .correct_belief_context(&bank, &a_id, "ctx-a2", "operator fix", "agent")
        .unwrap();
    let corrected = store.get_belief(&bank, &a_id).unwrap().expect("belief");
    assert_eq!(corrected.context.as_deref(), Some("ctx-a2"));

    let detail: String = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT detail_json FROM audit_events \
                 WHERE operation = 'correct' AND item_id = ?1 AND bank_id = ?2 \
                 ORDER BY created_at DESC LIMIT 1",
                rusqlite::params![a_id, bank],
                |r| r.get(0),
            )
            .am("test read")
        })
        .unwrap();
    assert_correction_detail(&detail);

    let cipher: String = store
        .with_conn(|conn| {
            conn.query_row(
                "SELECT context_cipher FROM beliefs WHERE id = ?1 AND bank_id = ?2",
                rusqlite::params![a_id, bank],
                |r| r.get(0),
            )
            .am("test read")
        })
        .unwrap();
    store
        .test_execute(&format!(
            "UPDATE beliefs SET context_cipher = '{cipher}' WHERE id = '{b_id}'"
        ))
        .unwrap();
    let swapped = store.get_belief(&bank, &b_id).unwrap().expect("belief b");
    assert_eq!(swapped.context, None);
    assert_eq!(swapped.proposition, b.proposition);

    assert_eq!(
        store
            .correct_belief_context(&bank, &a_id, "", "reason", "agent")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    let unknown = unique("belief-correct-missing");
    let err = store
        .correct_belief_context(&bank, &unknown, "ctx", "reason", "agent")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(err.message, format!("belief `{unknown}` not found in bank"));
}

#[test]
fn postgres_belief_context_correction() {
    let (_guard, store, _kms) = open_pg();
    let bank = unique("belief-correct-pg");
    let a_id = unique("belief-correct-pg-a");
    let b_id = unique("belief-correct-pg-b");
    let a_subject = unique("belief-correct-pg-subject-a");
    let b_subject = unique("belief-correct-pg-subject-b");

    let mut a = mk_belief(&a_id, &bank, &a_subject);
    a.context = Some("ctx-a".into());
    store.create_belief(&a, "agent").unwrap();
    let mut b = mk_belief(&b_id, &bank, &b_subject);
    b.proposition = unique("belief-correct-pg-proposition-b");
    b.context = Some("ctx-b".into());
    store.create_belief(&b, "agent").unwrap();

    store
        .correct_belief_context(&bank, &a_id, "ctx-a2", "operator fix", "agent")
        .unwrap();
    let corrected = store.get_belief(&bank, &a_id).unwrap().expect("belief");
    assert_eq!(corrected.context.as_deref(), Some("ctx-a2"));

    let detail: String = store
        .with_client(|client| {
            client
                .query_one(
                    "SELECT detail_json FROM audit_events \
                     WHERE operation = 'correct' AND item_id = $1 AND bank_id = $2 \
                     ORDER BY created_at DESC LIMIT 1",
                    &[&a_id, &bank],
                )
                .am("test read")
        })
        .unwrap()
        .get(0);
    assert_correction_detail(&detail);

    let cipher: String = store
        .with_client(|client| {
            client
                .query_one(
                    "SELECT context_cipher FROM beliefs WHERE id = $1 AND bank_id = $2",
                    &[&a_id, &bank],
                )
                .am("test read")
        })
        .unwrap()
        .get(0);
    store
        .test_execute(&format!(
            "UPDATE beliefs SET context_cipher = '{cipher}' WHERE id = '{b_id}'"
        ))
        .unwrap();
    let swapped = store.get_belief(&bank, &b_id).unwrap().expect("belief b");
    assert_eq!(swapped.context, None);
    assert_eq!(swapped.proposition, b.proposition);

    assert_eq!(
        store
            .correct_belief_context(&bank, &a_id, "", "reason", "agent")
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    let unknown = unique("belief-correct-pg-missing");
    let err = store
        .correct_belief_context(&bank, &unknown, "ctx", "reason", "agent")
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(err.message, format!("belief `{unknown}` not found in bank"));
}
