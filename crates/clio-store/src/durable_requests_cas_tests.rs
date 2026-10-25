// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//! Test atomic durable-request updates with independent SQLite connections.
//! ## Owns
//! Backend-neutral CAS assertions and the SQLite scratch-store fixture.
//! ## Does not own
//! Request service rules or Postgres fixture setup.
//! ## Boundary
//! Test-only assertions over the public durable-store contract.

use crate::durable_requests::{DurableRequestRow, DurableRequestStore};
use crate::erase_test_support::local_kms;
use crate::sqlite::SqliteStore;
use clio_types::ErrorCode;

fn row() -> DurableRequestRow {
    DurableRequestRow {
        request_id: "request".into(),
        kind: "erasure".into(),
        state: "in_progress".into(),
        scope: r#"{"bank":{"bank_id":"bank"}}"#.into(),
        deadline: None,
        record: r#"{"paths":{}}"#.into(),
    }
}

pub(super) fn assert_conditional_save_contract(
    first: &dyn DurableRequestStore,
    second: &dyn DurableRequestStore,
) {
    let original = row();
    assert!(!first.compare_and_save(Some(&original), &original).unwrap());
    assert_eq!(first.load("request").unwrap(), None);
    assert!(first.compare_and_save(None, &original).unwrap());
    assert!(!second.compare_and_save(None, &original).unwrap());
    assert!(second.compare_and_save(Some(&original), &original).unwrap());
    let mismatches: [fn(&mut DurableRequestRow); 6] = [
        |row| row.request_id = "other".into(),
        |row| row.kind = "export".into(),
        |row| row.scope = r#"{"bank":{"bank_id":"other"}}"#.into(),
        |row| row.state = "final".into(),
        |row| row.deadline = Some("2026-02-01T00:00:00Z".into()),
        |row| row.record = r#"{"paths":{"other":"done"}}"#.into(),
    ];
    for mismatch in mismatches {
        let mut stale = original.clone();
        mismatch(&mut stale);
        assert!(
            !second.compare_and_save(Some(&stale), &stale).unwrap(),
            "every expected column must match: {stale:?}"
        );
        assert_eq!(first.load("request").unwrap(), Some(original.clone()));
    }
    for mutate_identity in &mismatches[..3] {
        let mut changed = original.clone();
        mutate_identity(&mut changed);
        let error = first
            .compare_and_save(Some(&original), &changed)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert_eq!(second.load("request").unwrap(), Some(original.clone()));
    }
    let mut blank = original.clone();
    blank.record.clear();
    assert_eq!(
        first
            .compare_and_save(Some(&original), &blank)
            .unwrap_err()
            .code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(second.load("request").unwrap(), Some(original));
}

pub(super) fn assert_stale_writes_preserve_committed_rows(
    first: &dyn DurableRequestStore,
    second: &dyn DurableRequestStore,
) {
    let original = row();
    assert!(first.compare_and_save(None, &original).unwrap());
    let stale = second.load("request").unwrap().unwrap();
    let mut acknowledged = original.clone();
    acknowledged.record = r#"{"paths":{"first":"done"}}"#.into();
    assert!(
        first
            .compare_and_save(Some(&original), &acknowledged)
            .unwrap()
    );
    let mut competing = stale.clone();
    competing.record = r#"{"paths":{"second":"done"}}"#.into();
    assert!(!second.compare_and_save(Some(&stale), &competing).unwrap());
    assert_eq!(second.load("request").unwrap(), Some(acknowledged.clone()));

    let mut final_row = acknowledged.clone();
    final_row.state = "final".into();
    assert!(
        second
            .compare_and_save(Some(&acknowledged), &final_row)
            .unwrap()
    );
    assert!(
        !first
            .compare_and_save(Some(&acknowledged), &competing)
            .unwrap()
    );
    assert_eq!(first.load("request").unwrap(), Some(final_row.clone()));

    let mut deadline = final_row.clone();
    deadline.deadline = Some("2026-03-01T00:00:00Z".into());
    assert!(first.compare_and_save(Some(&final_row), &deadline).unwrap());
    assert!(
        !second
            .compare_and_save(Some(&final_row), &final_row)
            .unwrap()
    );
    assert_eq!(second.load("request").unwrap(), Some(deadline));
}

struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn sqlite_conditional_save_requires_an_exact_snapshot() {
    let scratch = Scratch(crate::test_unique::scratch_dir("clio-cas"));
    let path = scratch.0.join("store.db");
    let first = SqliteStore::open(&path, local_kms()).unwrap();
    let second = SqliteStore::open(&path, local_kms()).unwrap();
    assert_conditional_save_contract(&first, &second);
}

#[test]
fn sqlite_stale_writes_preserve_committed_rows() {
    let scratch = Scratch(crate::test_unique::scratch_dir("clio-cas"));
    let path = scratch.0.join("store.db");
    let first = SqliteStore::open(&path, local_kms()).unwrap();
    let second = SqliteStore::open(&path, local_kms()).unwrap();
    assert_stale_writes_preserve_committed_rows(&first, &second);
}
