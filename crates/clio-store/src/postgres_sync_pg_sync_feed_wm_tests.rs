// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the Postgres compound change-feed watermark bodies on
//! the shared Compose `clio` database: a `stamp\u{1f}entity` body resumes
//! strictly after the `(stamp, entity)` position, and the canonical empty
//! pair (`stamp\u{1f}`) resumes inclusively of the stamp tie, exactly like a
//! bare stamp body.
//!
//! ## Owns
//! - Postgres parity coverage for the compound feed resume predicate
//!
//! ## Does not own
//! - SQLite feed SQL parity (`sync_store_feed_tests`)
//! - Client cursor policy (`clio-sync`)
//!
//! ## Boundary
//!
//! Tests here break when the Postgres feed SQL resume semantics change; they
//! never change it.
//!
//! Keep this module focused on the Postgres compound watermark tests.

use crate::db_url::postgres_url_from_env;
use crate::pg_lock::lock_pg_test;
use crate::sync_store::SyncStore;
use crate::{DekProvider, LocalDevKms, PostgresStore, Store};
use clio_types::{EpistemicKind, ItemKind, MemoryItem};
use std::sync::Arc;

fn open_pg() -> PostgresStore {
    let url = postgres_url_from_env();
    let kms = Arc::new(LocalDevKms::new()) as Arc<dyn DekProvider>;
    PostgresStore::open(&url, kms).expect("postgres")
}

use crate::test_unique::unique;

fn tied_item(bank: &str, id: &str, stamp: &str) -> MemoryItem {
    MemoryItem {
        id: id.into(),
        bank_id: bank.into(),
        subject_id: "subj".into(),
        kind: ItemKind::Semantic,
        category: None,
        episodic_type: None,
        epistemic_kind: EpistemicKind::Fact,
        source_type: None,
        confidence: None,
        admission_score: Some(0.9),
        source_ref: None,
        snapshot: Some(serde_json::json!({"k": id})),
        gist: Some("g".into()),
        valid_from: stamp.into(),
        valid_until: None,
        tx_from: stamp.into(),
        tx_until: None,
        created_at: stamp.into(),
        updated_at: stamp.into(),
        context: None,
    }
}

#[test]
fn pg_feed_items_resumes_on_the_compound_stamp_entity_watermark() {
    use clio_types::SemanticCategory;

    let _guard = lock_pg_test();
    let bank = unique("bank");
    let s = open_pg();
    let stamp = "2026-01-01T00:00:00Z";
    let first = unique("wm-a");
    let second = unique("wm-b");
    for id in [&first, &second] {
        let mut item = tied_item(&bank, id, stamp);
        item.category = Some(SemanticCategory::ToolConfig);
        s.create_memory_item(&item, "actor").expect("create");
    }
    // Bare stamp body: both rows at the tie are fed (id > '').
    let both = s.feed_items(&bank, stamp, 10).expect("bare stamp feed");
    assert_eq!(
        both.iter()
            .map(|r| r.entity_id.as_str())
            .collect::<Vec<_>>(),
        vec![first.as_str(), second.as_str()],
        "a bare stamp body resumes inclusive of the tie"
    );
    // The canonical empty pair is what the cursor conversion writes for that
    // body: it binds the same resume predicate, so the tie is not lost.
    let empty_pair = format!("{stamp}\u{1f}");
    assert_eq!(
        s.feed_items(&bank, &empty_pair, 10)
            .expect("empty pair feed")
            .iter()
            .map(|r| r.entity_id.as_str())
            .collect::<Vec<_>>(),
        both.iter()
            .map(|r| r.entity_id.as_str())
            .collect::<Vec<_>>(),
        "the canonical empty pair resumes inclusively of the tie, like the bare stamp"
    );
    // Compound body: resume strictly after (stamp, first).
    let since = format!("{stamp}\u{1f}{first}");
    let after = s.feed_items(&bank, &since, 10).expect("compound feed");
    assert_eq!(
        after
            .iter()
            .map(|r| r.entity_id.as_str())
            .collect::<Vec<_>>(),
        vec![second.as_str()],
        "the compound body resumes strictly after the (stamp, entity) cursor"
    );
    // The full compound cursor drains the tie: nothing after the last row.
    let since = format!("{stamp}\u{1f}{second}");
    assert!(
        s.feed_items(&bank, &since, 10)
            .expect("drained feed")
            .is_empty(),
        "no row is skipped or re-fed past the last cursor position"
    );
}
