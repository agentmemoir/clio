// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests that concurrent pushes against the real sync server stay
//! correct. The bounded connection pool made the apply path reachable in
//! parallel for the first time, and the apply path is a check-then-act sequence
//! over the item row, the apply guard, and the journal, so these tests pin the
//! outcome a caller can actually observe when two workers race.
//!
//! ## Owns
//! - Concurrent-push correctness over the real `sync_serve` entry point: no
//!   spurious failure, no lost mutation, and no duplicate durable apply
//!
//! ## Does not own
//! - The apply matrix and its serial dedupe and LWW branches
//!   (`apply_matrix_tests`)
//! - The connection-cap and deadline bounds (`server_conn_tests`)
//!
//! ## Boundary
//!
//! Tests here break when concurrent-apply correctness changes; they never
//! change it.
//!
//! Keep this module focused on the concurrent-apply case.

use crate::envelope_fixtures::seal_own_content;
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncContent, SyncMutation};
use crate::server::server_tests::{http, item_value, mutation, push_body, serve};
use clio_store::{Store as _, SyncStore as _};
use std::thread;
use std::time::Duration;

const STAMP: &str = "2026-01-01T00:00:00Z";

/// One item mutation for `entity_id`, so distinct mutations can be built for
/// distinct entities. The content is serialized from the same `MemoryItem` the
/// existing server tests use and sealed under the shared sync key, so the route
/// admits it as the client-sealed row a feeding node sends.
fn item_mutation(mid: &str, bank: &str, entity_id: &str) -> SyncMutation {
    seal_own_content(SyncMutation {
        sync_mutation_id: mid.into(),
        bank: bank.into(),
        entity_kind: ENTITY_KIND_ITEM.into(),
        entity_id: entity_id.into(),
        op: OP_UPSERT.into(),
        transaction_time: STAMP.into(),
        device_id: "dev-a".into(),
        content: SyncContent::Plain(item_value(bank, entity_id, STAMP, mid)),
        content_hash: format!("h-{mid}"),
        admission_receipt: None,
    })
}

/// Push `m` on its own thread against the running server, and return the ack.
fn push_in_thread(addr: &str, m: SyncMutation) -> thread::JoinHandle<serde_json::Value> {
    let addr = addr.to_owned();
    thread::spawn(move || {
        let (status, ack) = http(
            &addr,
            "POST",
            "/sync/push",
            Some("t1"),
            Some(&push_body("b1", &[m])),
        );
        assert_eq!(
            status, 200,
            "every concurrent push is answered, not refused or failed: {ack}"
        );
        ack
    })
}

/// Two pushes of the *same* mutation id race for the same entity row. The
/// journal's per-bank unique key keeps the durable state to one row, so the
/// defect this pins is the window before it: without per-bank serialization
/// both workers can pass every dedupe check before either journals, and the
/// loser's append is what decides the outcome — a transient failure there makes
/// the client retry a mutation that already succeeded. Every push is answered,
/// the id is settled exactly once, and a blind relay keeps the seal it was sent.
#[test]
fn concurrent_pushes_of_one_mutation_id_are_answered_without_a_transient_failure() {
    let (server, store) = serve(&["b1"], Some("t1"), false, None, None);
    let addr = server.addr().to_owned();
    let raced = seal_own_content(mutation("m-race", "b1", STAMP, "raced gist"));
    let workers: Vec<_> = (0..4)
        .map(|_| push_in_thread(&addr, raced.clone()))
        .collect();
    for w in workers {
        w.join().expect("push thread");
    }
    assert_eq!(
        store.journal_max_seq("b1").expect("journal"),
        1,
        "one mutation id settles into exactly one journal row"
    );
    let rows = store.journal_list_since("b1", 0, 10).expect("journal");
    assert!(
        rows[0].client_encrypted,
        "the settled row is the pusher's seal: {rows:?}"
    );
    assert!(
        store
            .get_memory_item("b1", "i-1")
            .expect("get item")
            .is_none(),
        "a blind node holds no plaintext item for a sealed row"
    );
    server.stop();
}

/// Distinct mutations for *distinct* entities are independent work and must all
/// land. This is the companion to the test above: a fix that simply dropped or
/// refused every second push would pass the idempotence test and fail here.
#[test]
fn concurrent_pushes_of_distinct_entities_all_apply() {
    let (server, store) = serve(&["b1"], Some("t1"), false, None, None);
    let addr = server.addr().to_owned();
    let workers: Vec<_> = ["m-a", "m-b", "m-c", "m-d"]
        .into_iter()
        .enumerate()
        .map(|(i, mid)| push_in_thread(&addr, item_mutation(mid, "b1", &format!("i-conc-{i}"))))
        .collect();
    for w in workers {
        w.join().expect("push thread");
    }
    assert_eq!(
        store.journal_max_seq("b1").expect("journal"),
        4,
        "four distinct mutations must produce four journal rows: none dropped"
    );
    let rows = store.journal_list_since("b1", 0, 10).expect("journal");
    assert!(
        rows.iter().all(|r| r.client_encrypted),
        "every row is the pusher's seal: {rows:?}"
    );
    for i in 0..4 {
        let id = format!("i-conc-{i}");
        assert!(
            rows.iter().any(|r| r.entity_id == id),
            "entity {id} must reach the relay exactly once: {rows:?}"
        );
    }
    server.stop();
}

/// Three concurrent pushes write the *same* entity under different mutation
/// ids. This node is blind: it stores no item row for a sealed row and has no
/// content to compare, so every sealed row is applied and journaled on its own
/// and no push is refused or left needing a retry. What this proves: all three
/// pushes are acknowledged as applied with nothing quarantined, one journal row
/// each, and nothing materialized from content the node cannot read.
#[test]
fn concurrent_pushes_to_one_entity_resolve_without_a_transient_failure() {
    let (server, store) = serve(&["b1"], Some("t1"), false, None, None);
    let addr = server.addr().to_owned();
    let ids = ["m-x", "m-y", "m-z"];
    // Every push is in flight before the first one is joined, so the three
    // really do race for the same entity and the same bank lock.
    let workers: Vec<_> = ids
        .into_iter()
        .map(|mid| push_in_thread(&addr, item_mutation(mid, "b1", "i-shared")))
        .collect();
    let mut acks = Vec::new();
    for w in workers {
        acks.push(w.join().expect("push thread"));
    }
    assert_eq!(
        store.journal_max_seq("b1").expect("journal"),
        3,
        "each racing push journals its own row: none dropped, none failed"
    );
    let rows = store.journal_list_since("b1", 0, 10).expect("journal");
    assert_eq!(rows.len(), 3, "three sealed rows reached the journal");
    for row in &rows {
        assert_eq!(row.entity_id, "i-shared", "every row targets the entity");
        assert!(
            row.client_encrypted,
            "the settled row is the pusher's seal: {row:?}"
        );
    }
    for (mid, ack) in ids.iter().zip(&acks) {
        assert_eq!(
            ack["accepted"],
            serde_json::json!([mid]),
            "{mid} is answered as applied, never as a failure to retry: {ack}"
        );
        assert_eq!(
            ack["rejected"],
            serde_json::json!([]),
            "nothing is quarantined and nothing needs a retry: {ack}"
        );
    }
    assert!(
        store
            .get_memory_item("b1", "i-shared")
            .expect("get item")
            .is_none(),
        "a blind node holds no plaintext item for a sealed row"
    );
    server.stop();
}

/// Two banks are independent work and must not serialize behind each other.
/// The per-bank lock is per bank by design; a regression that widened it to a
/// single global lock would still pass every test above and fail this one on
/// timing, so the bound here is generous and only catches a gross stall.
#[test]
fn pushes_to_different_banks_are_not_serialized_behind_each_other() {
    let (server, store) = serve(&["b1", "b2"], Some("t1"), false, None, None);
    let addr = server.addr().to_owned();
    let m1 = seal_own_content(mutation("m-b1", "b1", STAMP, "bank one"));
    let m2 = item_mutation("m-b2", "b2", "i-b2");
    let started = std::time::Instant::now();
    let addr_b1 = addr.clone();
    let handles = vec![
        thread::spawn(move || {
            let body = push_body("b1", &[m1]);
            http(&addr_b1, "POST", "/sync/push", Some("t1"), Some(&body)).0
        }),
        thread::spawn(move || {
            let body = push_body("b2", &[m2]);
            http(&addr, "POST", "/sync/push", Some("t1"), Some(&body)).0
        }),
    ];
    for h in handles {
        assert_eq!(h.join().expect("push thread"), 200);
    }
    let elapsed = started.elapsed();
    // Loopback pushes take milliseconds. The bound only has to be loose enough
    // that a loaded host cannot trip it, while still failing if one bank ever
    // waits on the other for a request-sized duration.
    assert!(
        elapsed < Duration::from_secs(5),
        "pushes to independent banks must not wait on each other: {elapsed:?}"
    );
    assert_eq!(store.journal_max_seq("b1").expect("journal"), 1);
    assert_eq!(store.journal_max_seq("b2").expect("journal"), 1);
    server.stop();
}
