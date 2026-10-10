// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for MemTree node persistence tests across both store backends.
//!
//! ## Owns
//! - SQLite + Postgres parity tests for memtree node upsert / get / query
//! - Missing-bank and filter-miss error-arm coverage for MemTree CRUD
//!
//! ## Does not own
//! - MemTree engine or attach logic (lives in `clio-write`)
//! - Store schema reshape logic
//!
//! ## Boundary
//!
//! Changes to MemTree persistence scenarios should normally require changes here.
//! Changes to MemTree engine behavior should not require changes here.
//!
//! Keep this module focused on dual-backend MemTree persistence coverage.

use crate::dek::LocalDevKms;
use crate::memtree_model::{MemTreeNodeRecord, MemTreeQuery};
use crate::sqlite::SqliteStore;
use crate::store::Store;
use std::sync::Arc;

fn sample_node(bank: &str, id: &str) -> MemTreeNodeRecord {
    MemTreeNodeRecord {
        id: id.into(),
        bank_id: bank.into(),
        parent_id: None,
        item_id: Some("item-1".into()),
        depth: 0,
        interval_start: "100".into(),
        interval_end: "100".into(),
        child_ids: vec![],
        summary_gist: Some("g".into()),
        dirty: true,
        generation: 1,
        refreshing: false,
        task_id: None,
        last_error: None,
        created_at: "100".into(),
        updated_at: "100".into(),
    }
}

#[test]
fn sqlite_memtree_upsert_get_query() {
    let store = SqliteStore::open_in_memory(Arc::new(LocalDevKms::new())).unwrap();
    let node = sample_node("bank-a", "n1");
    store.upsert_memtree_node(&node).unwrap();
    let got = store.get_memtree_node("bank-a", "n1").unwrap().unwrap();
    assert_eq!(got.summary_gist.as_deref(), Some("g"));
    assert!(store.get_memtree_node("bank-b", "n1").unwrap().is_none());
    let q = store
        .query_memtree_nodes(&MemTreeQuery {
            bank_id: "bank-a".into(),
            time_start: Some("050".into()),
            time_end: Some("150".into()),
            depth: Some(0),
            task_id: None,
            limit: Some(10),
        })
        .unwrap();
    assert_eq!(q.len(), 1);
    let err = store
        .upsert_memtree_node(&sample_node("", "x"))
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::MissingBank);
    let err = store
        .query_memtree_nodes(&MemTreeQuery {
            bank_id: String::new(),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::MissingBank);
    // Filter miss paths
    let miss = store
        .query_memtree_nodes(&MemTreeQuery {
            bank_id: "bank-a".into(),
            time_start: Some("200".into()),
            time_end: Some("300".into()),
            depth: Some(9),
            task_id: Some("t".into()),
            limit: Some(1),
        })
        .unwrap();
    assert!(miss.is_empty(), "{miss:?}");
    let no_children = crate::sqlite_memtree::decode_children_for_test("[]").unwrap();
    assert!(no_children.is_empty(), "{no_children:?}");
    assert!(crate::sqlite_memtree::decode_children_for_test("not-json").is_err());
    store
        .test_execute("UPDATE memtree_nodes SET child_ids_json = 'not-json' WHERE id = 'n1'")
        .unwrap();
    assert!(store.get_memtree_node("bank-a", "n1").is_err());
}

#[test]
fn postgres_memtree_parity() {
    let _guard = crate::lock_pg_test();
    let url = crate::db_url::postgres_url_from_env();
    let store = crate::postgres::PostgresStore::open(&url, Arc::new(LocalDevKms::new())).unwrap();
    let id = format!("mt-{}", uuid_like());
    let mut node = sample_node("bank-pg", &id);
    node.item_id = Some(format!("item-{id}"));
    node.task_id = Some("task-1".into());
    node.child_ids = vec!["c1".into()];
    store.upsert_memtree_node(&node).unwrap();
    let got = store.get_memtree_node("bank-pg", &id).unwrap().unwrap();
    assert_eq!(got.id, id);
    assert_eq!(got.child_ids, vec!["c1".to_string()]);
    assert!(store.get_memtree_node("other", &id).unwrap().is_none());
    let q = store
        .query_memtree_nodes(&MemTreeQuery {
            bank_id: "bank-pg".into(),
            time_start: Some("050".into()),
            time_end: Some("150".into()),
            depth: Some(0),
            task_id: Some("task-1".into()),
            limit: Some(5),
        })
        .unwrap();
    assert!(q.iter().any(|n| n.id == id));
    let err = store
        .upsert_memtree_node(&sample_node("", "x"))
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::MissingBank);
    let err = store
        .query_memtree_nodes(&MemTreeQuery {
            bank_id: String::new(),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(err.code, clio_types::ErrorCode::MissingBank);
    assert!(
        crate::postgres_memtree::decode_children_for_test("[\"a\"]")
            .unwrap()
            .contains(&"a".to_string())
    );
    assert!(crate::postgres_memtree::decode_children_for_test("{").is_err());
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|_| "0".into())
}
