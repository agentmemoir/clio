// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! Test-only units owned by `journal_row.rs`: the journal content identity
//! built by `journal_row` and opened by `open_journal_content`. The whole path
//! runs, so a row identity that falls back to the entity id fails here. Never
//! compiled into production builds.

use crate::apply::ApplyEngine;
use crate::apply::apply_tests::{fixture, upsert};
use crate::journal_row::{journal_row, open_journal_content};
use clio_types::ErrorCode;

const STAMP: &str = "2026-01-01T00:00:00Z";

#[test]
fn journal_content_is_bound_to_one_mutation() {
    let (store, kms) = fixture();
    let engine = ApplyEngine {
        store: &*store,
        sync: &*store,
        device_id: "node-1".into(),
        kms: Some(&*kms),
        policy: crate::freshness::FreshnessPolicy::default(),
    };
    // Two mutations of one entity: only the mutation id tells them apart.
    let first = upsert("mutation-a", "bank-1", STAMP, "dev-a", "gist-a");
    let second = upsert("mutation-b", "bank-1", STAMP, "dev-a", "gist-b");
    let row_a = journal_row(&engine, &first).expect("row a");
    let row_b = journal_row(&engine, &second).expect("row b");
    let sealed_a = row_a.content_sealed.as_deref().expect("sealed a");
    let sealed_b = row_b.content_sealed.as_deref().expect("sealed b");

    let opened = open_journal_content(&row_a, sealed_a, &*kms).expect("open own body");
    assert_eq!(opened["gist"], "gist-a");
    assert_eq!(
        open_journal_content(&row_a, sealed_b, &*kms)
            .unwrap_err()
            .code,
        ErrorCode::Forbidden,
        "a mutation must not open another mutation's body"
    );
}

#[test]
fn journal_content_is_bound_to_its_bank() {
    let (store, kms) = fixture();
    let engine = ApplyEngine {
        store: &*store,
        sync: &*store,
        device_id: "node-1".into(),
        kms: Some(&*kms),
        policy: crate::freshness::FreshnessPolicy::default(),
    };
    let first = upsert("mutation-a", "bank-1", STAMP, "dev-a", "gist-a");
    let mut second = first.clone();
    second.bank = "bank-2".into();
    let row_a = journal_row(&engine, &first).expect("row a");
    let row_b = journal_row(&engine, &second).expect("row b");
    let sealed_b = row_b.content_sealed.as_deref().expect("sealed b");

    assert_eq!(
        open_journal_content(&row_a, sealed_b, &*kms)
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
}
