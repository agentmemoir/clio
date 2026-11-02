// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the SQLite memory-item repository bodies.
//!
//! ## Owns
//! - `create_memory_item_impl`, the item getters, snapshot/gist access, and
//!   the shredded-subject probe
//!
//! ## Does not own
//! - Domain validation or the Postgres twin
//!
//! ## Boundary
//!
//! Test-only. Never compiled into production builds.

use crate::sqlite::sqlite_history_tests::{item, store};
use crate::store::Store;
use crate::test_unique::unique;
use clio_types::{EpisodicType, ErrorCode};

#[test]
#[allow(clippy::too_many_lines)]
fn memory_item_round_trip_and_bank_isolation() {
    let store = store();
    let bank = unique("sqlite-memory");
    let item_id = unique("sqlite-memory-item");
    let mut a = item(&item_id, &bank, EpisodicType::Task, false);
    a.snapshot = Some(serde_json::json!({"k": "a"}));
    a.gist = Some("gist-a".into());
    a.context = Some("ctx-a".into());
    store.create_memory_item(&a, "agent").unwrap();

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
            .get_memory_item_impl(&bank, &unique("missing"))
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
}

#[test]
fn item_by_subject_honours_point_in_time() {
    let store = store();
    let bank = unique("sqlite-memory-subject");
    let item_id = unique("sqlite-memory-subject-item");
    let subject = unique("sqlite-memory-subject-id");
    let mut a = item(&item_id, &bank, EpisodicType::Task, false);
    a.subject_id.clone_from(&subject);
    store.create_memory_item(&a, "agent").unwrap();

    let active = store
        .get_item_by_subject_impl(&bank, &subject, None)
        .unwrap()
        .expect("active item");
    assert_eq!(active.id, item_id);
    assert!(
        store
            .get_item_by_subject_impl(&bank, &subject, Some("2026-05-01T00:00:00Z"))
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .get_item_by_subject_impl(&bank, &subject, Some("2026-06-02T00:00:00Z"))
            .unwrap()
            .is_some()
    );
}

#[test]
fn missing_content_halves_and_shredded_subject_fail_closed() {
    let store = store();
    let bank = unique("sqlite-memory-half");
    let item_id = unique("sqlite-memory-half-item");
    let mut half = item(&item_id, &bank, EpisodicType::Task, false);
    half.snapshot = None;
    half.gist = None;
    store.create_memory_item(&half, "agent").unwrap();
    assert_eq!(
        store.get_snapshot_impl(&bank, &item_id).unwrap_err().code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        store.get_gist_impl(&bank, &item_id).unwrap_err().code,
        ErrorCode::InvalidArgument
    );

    let subject = unique("sqlite-memory-shredded");
    let subject_item = unique("sqlite-memory-shredded-item");
    let mut owned = item(&subject_item, &bank, EpisodicType::Task, false);
    owned.subject_id.clone_from(&subject);
    store.create_memory_item(&owned, "agent").unwrap();
    let fresh = store
        .with_conn(|conn| crate::sqlite_memory::subject_is_shredded_sqlite(conn, &subject))
        .unwrap();
    assert!(!fresh);
    store
        .test_execute(&format!(
            "UPDATE subject_keys SET shredded_at = '2026-06-02T00:00:00Z' \
             WHERE subject_id = '{subject}'"
        ))
        .unwrap();
    let shredded = store
        .with_conn(|conn| crate::sqlite_memory::subject_is_shredded_sqlite(conn, &subject))
        .unwrap();
    assert!(shredded);
}

#[test]
fn ciphertext_from_another_row_does_not_open() {
    let store = store();
    let bank = unique("sqlite-memory-binding");
    let subject = unique("sqlite-memory-binding-subject");
    let first_id = unique("sqlite-memory-binding-a");
    let mut first = item(&first_id, &bank, EpisodicType::Task, false);
    first.subject_id.clone_from(&subject);
    store.create_memory_item(&first, "agent").unwrap();

    let second_id = unique("sqlite-memory-binding-b");
    let mut second = first.clone();
    second.id.clone_from(&second_id);
    store.create_memory_item(&second, "agent").unwrap();

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
