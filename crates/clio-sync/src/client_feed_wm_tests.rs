// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the per-kind push watermarks: a page-full feed
//! drains across consecutive pushes with no re-send loss, rows sharing a
//! stamp are delivered exactly once across a page boundary, and a skipped
//! task row holds its kind's cursor without freezing the other kinds. The
//! one-shot conversion of the retired shared cursor is covered by the
//! sibling `client_feed_cursor_conversion_tests` module.
//!
//! ## Owns
//! - Behavior coverage for the per-kind push watermark advance and hold rules
//!
//! ## Does not own
//! - Feed SQL resume semantics (`clio-store` sync feed tests)
//! - Core transfer paths (`client_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the watermark cursor contract changes; they never
//! change it.
//!
//! Keep this module focused on the push watermark contract tests.

use crate::client::client_tests::{TOKEN, cfg, item, serve, sqlite};
use crate::client::{SyncClientConfig, push, sync_status};
use crate::protocol::encode_watermark;
use clio_store::{HistoryStore, SqliteStore, Store, SyncStore};
use clio_types::{EpisodicType, ItemKind, MemoryItem, TaskRecord};
use std::sync::Arc;

fn seed_items(store: &Arc<SqliteStore>, bank: &str, n: usize, stamp: &str) {
    for i in 0..n {
        let stamp = if stamp.is_empty() {
            format!("2026-01-01T00:{:02}:{:02}Z", i / 60, i % 60)
        } else {
            stamp.to_owned()
        };
        let id = format!("i-{i:03}");
        store
            .create_memory_item(&item(bank, &id, "s1", &stamp, "g"), "actor")
            .expect("create item");
    }
}

/// A kind with more than `PULL_PAGE_LIMIT` changed rows drains across
/// consecutive pushes with no re-send loss: the page-full push advances the
/// compound cursor, and the next push resumes strictly after it.
#[test]
fn full_item_page_drains_across_consecutive_pushes() {
    let (server, srv, _kms) = serve(&["bdrain"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = cfg("dev-a", &a, &addr, Vec::new());
    let n = crate::PULL_PAGE_LIMIT + 5;
    seed_items(&a, "bdrain", n, "");

    push(&cfg_a, &["bdrain".to_owned()]).expect("push page 1");
    let after_first = srv.journal_list_since("bdrain", 0, 400).unwrap().len();
    assert_eq!(
        after_first,
        crate::PULL_PAGE_LIMIT,
        "a page-full push packages exactly the page cap"
    );

    push(&cfg_a, &["bdrain".to_owned()]).expect("push page 2");
    let rows = srv.journal_list_since("bdrain", 0, 400).unwrap();
    let mut ids: Vec<String> = rows.iter().map(|r| r.sync_mutation_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n, "every row reaches the relay exactly once");
    assert_eq!(rows.len(), n, "no re-send loss and no duplicates");
    let report = sync_status(&cfg_a, &["bdrain".to_owned()]).expect("status");
    assert_eq!(report.banks[0].pending_push, 0, "the backlog drained");
    server.stop();
}

/// Rows sharing one stamp across a page boundary are all delivered exactly
/// once: the compound `(stamp, entity)` cursor resumes inside the tie.
#[test]
fn stamp_ties_across_page_boundary_deliver_exactly_once() {
    let (server, srv, _kms) = serve(&["btie"]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = cfg("dev-a", &a, &addr, Vec::new());
    let n = crate::PULL_PAGE_LIMIT + 5;
    // Every item shares the same change stamp: the old strict `>` watermark
    // skipped the tie forever once the page filled.
    seed_items(&a, "btie", n, "2026-01-01T00:00:00Z");

    push(&cfg_a, &["btie".to_owned()]).expect("push page 1");
    push(&cfg_a, &["btie".to_owned()]).expect("push page 2");
    let rows = srv.journal_list_since("btie", 0, 400).unwrap();
    assert_eq!(rows.len(), n, "tied rows across the boundary all delivered");
    let report = sync_status(&cfg_a, &["btie".to_owned()]).expect("status");
    assert_eq!(
        report.banks[0].pending_push, 0,
        "no tied row is left pending"
    );
    server.stop();
}

/// A task row whose carrier item is missing locally is skipped: the task
/// cursor holds (never jumps over the row) while the item kind still
/// advances; once the carrier exists the task row is packaged and the task
/// cursor advances.
#[test]
fn missing_carrier_holds_task_cursor_but_not_other_kinds() {
    let (server, srv, _kms) = serve(&["bhold"]);
    let addr = server.addr().to_owned();
    // Two stores: the sync-feed store owns the journal/cursors and feeds; the
    // data store backs the item lookups. The task carrier exists only on the
    // feed store, so packaging skips it.
    let feed_store = sqlite();
    let data_store = sqlite();
    let mut task_item = MemoryItem {
        kind: ItemKind::Episodic,
        category: None,
        ..item("bhold", "car-task", "s1", "2026-01-01T00:00:00Z", "do it")
    };
    task_item.episodic_type = Some(EpisodicType::Task);
    let task = TaskRecord {
        id: "trec-1".into(),
        bank_id: "bhold".into(),
        item_id: "car-task".into(),
        task_id: "task-1".into(),
        subject_id: "s1".into(),
        version: 1,
        definition: "do it".into(),
        steps: vec!["step".into()],
        status: "open".into(),
        evidence_ref: None,
        context: None,
        admission_score: 0.8,
        created_at: "2026-01-01T00:00:00Z".into(),
    };
    feed_store
        .commit_task_upsert(&task_item, &task, "agent")
        .expect("seed task on the feed store");
    // A plain item exists on both stores, so the item kind packages fine.
    let plain = item("bhold", "i-1", "s1", "2026-01-02T00:00:00Z", "g");
    feed_store
        .create_memory_item(&plain, "actor")
        .expect("feed item");
    data_store
        .create_memory_item(&plain, "actor")
        .expect("data item");
    let cfg = SyncClientConfig {
        device_id: "dev-a".into(),
        store: data_store.clone(),
        sync: feed_store,
        remote: Some(addr),
        token: Some(TOKEN.to_owned()),
        cipher: Vec::new(),
    };

    push(&cfg, &["bhold".to_owned()]).expect("push with missing carrier");
    let rows = srv.journal_list_since("bhold", 0, 50).unwrap();
    assert!(
        rows.iter().any(|r| r.entity_kind == "item"),
        "the item kind must not freeze behind the skipped task: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.entity_kind == "task"),
        "the skipped task row must not be packaged: {rows:?}"
    );
    // The item kind advanced; the task kind holds (no own cursor yet).
    assert!(
        cfg.sync
            .cursor_get("bhold", "push_wm:item")
            .unwrap()
            .is_some(),
        "the item cursor advances while the task kind is skipped"
    );
    assert!(
        cfg.sync
            .cursor_get("bhold", "push_wm:task")
            .unwrap()
            .is_none(),
        "the task cursor must hold when its carrier item is missing"
    );
    assert_eq!(
        sync_status(&cfg, &["bhold".to_owned()]).unwrap().banks[0].pending_push,
        1,
        "the held task row stays pending (item kind drained)"
    );

    // Heal: the carrier arrives on the data store. The next push packages the
    // held task row and the task cursor advances.
    data_store
        .commit_task_upsert(&task_item, &task, "agent")
        .expect("heal carrier");
    push(&cfg, &["bhold".to_owned()]).expect("push after heal");
    let rows = srv.journal_list_since("bhold", 0, 50).unwrap();
    assert!(
        rows.iter().any(|r| r.entity_kind == "task"),
        "the healed carrier lets the task row package: {rows:?}"
    );
    assert!(
        cfg.sync
            .cursor_get("bhold", "push_wm:task")
            .unwrap()
            .is_some(),
        "the task cursor advances once no row is skipped"
    );
    assert_eq!(
        sync_status(&cfg, &["bhold".to_owned()]).unwrap().banks[0].pending_push,
        0
    );
    server.stop();
}

/// The tie-break entity part of the compound cursor: two rows sharing a stamp
/// are drained one per push once the first advances the cursor to
/// `(stamp, entity)`, proving the strict-after resume inside a stamp tie.
#[test]
fn compound_cursor_resumes_strictly_after_the_tiebreak_entity() {
    let a = sqlite();
    let cfg_a = cfg("dev-a", &a, "127.0.0.1:1", Vec::new());
    let stamp = "2026-01-01T00:00:00Z";
    for id in ["i-a", "i-b"] {
        a.create_memory_item(&item("btie2", id, "s1", stamp, "g"), "actor")
            .expect("create");
    }
    // First page packages "i-a" (id order) and advances the cursor to it.
    a.cursor_set(
        "btie2",
        "push_wm:item",
        &encode_watermark("btie2", &format!("{stamp}\u{1f}i-a")),
    )
    .unwrap();
    let since = crate::client_feed::kind_watermark(&cfg_a, "btie2", "item").unwrap();
    let rows = a
        .feed_items("btie2", &since, crate::PULL_PAGE_LIMIT)
        .unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r.entity_id.as_str())
            .collect::<Vec<_>>(),
        vec!["i-b"],
        "the compound cursor resumes strictly after (stamp, i-a)"
    );
}

/// A kind's watermark holds only when one of *its own* mutations was
/// rejected. Before the fix every per-kind advance sat inside the
/// `response.rejected.is_empty()` guard, so a single dead-lettered mutation in
/// any kind froze all eight watermarks and the whole backlog was re-sent on
/// every retry. These cases pin the predicate that decides which kinds freeze.
#[test]
fn a_kind_holds_only_for_its_own_rejected_mutations() {
    use crate::client_feed::KindBatch;
    use crate::protocol::{SyncContent, SyncMutation};

    let mutation = |id: &str| SyncMutation {
        sync_mutation_id: id.to_owned(),
        bank: "brej".to_owned(),
        entity_kind: "item".to_owned(),
        entity_id: id.to_owned(),
        op: "upsert".to_owned(),
        transaction_time: "2026-01-01T00:00:00Z".to_owned(),
        device_id: "dev-a".to_owned(),
        content: SyncContent::Plain(serde_json::json!({})),
        content_hash: "h".to_owned(),
        admission_receipt: None,
    };
    // Two kinds packaged in one push: item owns mutations 0..1, belief 1..2.
    let mutations = vec![mutation("m-item"), mutation("m-belief")];
    let batch = |kind, base, end| KindBatch {
        kind,
        last: Some(("2026-01-01T00:00:00Z".to_owned(), "e".to_owned())),
        page_rows: 1,
        skipped: false,
        max_mutation_stamp: None,
        base,
        end,
    };
    let item_batch = batch("item", 0, 1);
    let belief_batch = batch("belief", 1, 2);

    // The belief mutation is the only rejection: belief freezes, item advances.
    let rejected = vec!["m-belief".to_owned()];
    assert!(
        belief_batch.has_rejection(&mutations, &rejected),
        "the kind that owns a rejected mutation must hold its cursor"
    );
    assert!(
        !item_batch.has_rejection(&mutations, &rejected),
        "an unrelated rejection must not freeze a different kind's cursor"
    );

    // A rejection id that belongs to no packaged mutation advances everything:
    // the bank-level cursor is what holds in that case, not the kinds.
    let unknown = vec!["m-not-packaged".to_owned()];
    assert!(!item_batch.has_rejection(&mutations, &unknown));
    assert!(!belief_batch.has_rejection(&mutations, &unknown));

    // Nothing rejected: every kind advances.
    assert!(!item_batch.has_rejection(&mutations, &[]));
    assert!(!belief_batch.has_rejection(&mutations, &[]));
}
