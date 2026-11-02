// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the Postgres memory-item repository bodies.
//!
//! ## Owns
//! - The Postgres item writers, getters, snapshot/gist access, and the
//!   shredded-subject probe
//!
//! ## Does not own
//! - Domain validation or the SQLite twin
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::postgres_belief::postgres_belief_context_tests::open_pg;
use crate::sqlite::sqlite_history_tests::item;
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::{EpisodicType, ErrorCode};

#[test]
#[allow(clippy::too_many_lines)]
fn postgres_memory_item_round_trip_and_bank_isolation() {
    let (_guard, store, _kms) = open_pg();
    let bank = unique("pg-memory");
    let item_id = unique("pg-memory-item");
    let mut a = item(&item_id, &bank, EpisodicType::Task, false);
    a.snapshot = Some(serde_json::json!({"k": "a"}));
    a.gist = Some("gist-a".into());
    a.context = Some("ctx-a".into());
    store.create_memory_item_impl(&a, "agent").unwrap();

    let read = store
        .get_memory_item_impl(&bank, &item_id)
        .unwrap()
        .expect("stored item");
    assert_eq!(read.snapshot, Some(serde_json::json!({"k": "a"})));
    assert_eq!(read.gist.as_deref(), Some("gist-a"));
    assert_eq!(read.context.as_deref(), Some("ctx-a"));
    assert_eq!(
        store.get_snapshot_impl(&bank, &item_id).unwrap(),
        Some(serde_json::json!({"k": "a"}))
    );
    assert_eq!(
        store.get_gist_impl(&bank, &item_id).unwrap().as_deref(),
        Some("gist-a")
    );
    assert!(
        store
            .get_memory_item_impl(&unique("other-bank"), &item_id)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_snapshot_impl(&bank, &unique("missing"))
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_gist_impl(&bank, &unique("missing"))
            .unwrap()
            .is_none()
    );

    let mut updated = a.clone();
    updated.gist = Some("gist-b".into());
    updated.context = Some("ctx-b".into());
    store.update_memory_item_impl(&updated, "agent").unwrap();
    let reread = store
        .get_memory_item_impl(&bank, &item_id)
        .unwrap()
        .expect("updated item");
    assert_eq!(reread.gist.as_deref(), Some("gist-b"));
    assert_eq!(reread.context.as_deref(), Some("ctx-b"));

    let active = store
        .get_item_by_subject_impl(&bank, "subject-1", None)
        .unwrap()
        .expect("active by subject");
    assert_eq!(active.id, item_id);
    assert!(
        store
            .get_item_by_subject_impl(&bank, "subject-1", Some("2026-05-01T00:00:00Z"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn postgres_ciphertext_from_another_row_does_not_open() {
    let (_guard, store, _kms) = open_pg();
    let bank = unique("pg-memory-binding");
    let subject = unique("pg-memory-binding-subject");
    let first_id = unique("pg-memory-binding-a");
    let mut first = item(&first_id, &bank, EpisodicType::Task, false);
    first.subject_id.clone_from(&subject);
    store.create_memory_item_impl(&first, "agent").unwrap();

    let second_id = unique("pg-memory-binding-b");
    let mut second = first.clone();
    second.id.clone_from(&second_id);
    store.create_memory_item_impl(&second, "agent").unwrap();

    let cipher = store
        .inspect_content_ciphertext(&bank, &first_id)
        .unwrap()
        .expect("sealed body");
    store
        .test_execute(&format!(
            "UPDATE items SET content_ciphertext = '{cipher}' WHERE id = '{second_id}'"
        ))
        .unwrap();
    let err = store.get_memory_item_impl(&bank, &second_id).unwrap_err();
    assert_eq!(err.code, ErrorCode::Forbidden);
}
