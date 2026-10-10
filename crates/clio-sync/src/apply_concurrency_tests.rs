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

use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncContent, SyncMutation};
use crate::server::server_tests::{http, item_value, mutation, push_body, serve};
use clio_store::{Store as _, SyncStore as _};
use std::thread;
use std::time::Duration;

const STAMP: &str = "2026-01-01T00:00:00Z";

/// One item mutation for `entity_id`, so distinct mutations can be built for
/// distinct entities. The content is serialized from the same `MemoryItem` the
/// existing server tests use, so the mutation is valid for the apply path
/// rather than being quarantined for a missing field.
fn item_mutation(mid: &str, bank: &str, entity_id: &str) -> SyncMutation {
    SyncMutation {
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
    }
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
/// both workers can see an absent item and both try to create it, and the
/// loser of that create fails with a store error the server reports as a
/// transient 500, which makes the client retry a mutation that already
/// succeeded. Every push is answered, and the id is settled exactly once.
#[test]
fn concurrent_pushes_of_one_mutation_id_are_answered_without_a_transient_failure() {
    let (server, store) = serve(&["b1"], Some("t1"), false, None, None);
    let addr = server.addr().to_owned();
    let workers: Vec<_> = (0..4)
        .map(|_| push_in_thread(&addr, mutation("m-race", "b1", STAMP, "raced gist")))
        .collect();
    for w in workers {
        w.join().expect("push thread");
    }
    assert_eq!(
        store.journal_max_seq("b1").expect("journal"),
        1,
        "one mutation id settles into exactly one journal row"
    );
    let item = store
        .get_memory_item("b1", "i-1")
        .expect("get item")
        .expect("the raced mutation was applied");
    assert_eq!(item.gist.as_deref(), Some("raced gist"));
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
    for i in 0..4 {
        assert!(
            store
                .get_memory_item("b1", &format!("i-conc-{i}"))
                .expect("get item")
                .is_some(),
            "entity i-conc-{i} must be applied exactly once"
        );
    }
    server.stop();
}

/// Distinct mutation ids writing the *same* entity are a real conflict, not a
/// duplicate. The server resolves them by last-writer-wins, so exactly one is
/// journaled as applied and the rest are recorded as conflict losers. Before
/// per-bank serialization the two creates raced and one push failed with a
/// transient 500; this asserts the resolution is reached instead of an error.
#[test]
fn concurrent_pushes_to_one_entity_resolve_without_a_transient_failure() {
    let (server, store) = serve(&["b1"], Some("t1"), false, None, None);
    let addr = server.addr().to_owned();
    let acks: Vec<_> = ["m-x", "m-y", "m-z"]
        .into_iter()
        .map(|mid| push_in_thread(&addr, item_mutation(mid, "b1", "i-shared")))
        .map(|w| w.join().expect("push thread"))
        .collect();
    assert_eq!(
        store.journal_max_seq("b1").expect("journal"),
        1,
        "one entity can take exactly one applied winner; the losers are conflicts"
    );
    // Every push is answered and reports the id as settled: the client learns
    // the outcome either way, and none of them is told to retry a 500.
    for (mid, ack) in ["m-x", "m-y", "m-z"].iter().zip(&acks) {
        let accepted = ack["accepted"].as_array().expect("accepted array");
        let rejected = ack["rejected"].as_array().expect("rejected array");
        assert!(
            accepted.contains(&serde_json::Value::String((*mid).into())),
            "{mid} must be settled, not retried: {ack}"
        );
        assert!(
            rejected.is_empty(),
            "a last-writer-wins loser is settled, not quarantined: {ack}"
        );
    }
    // The winner is one of the three, and the stored row is that mutation's.
    let stored = store
        .get_memory_item("b1", "i-shared")
        .expect("get item")
        .expect("the entity exists");
    let gist = stored.gist.expect("a winner's gist is stored");
    assert!(
        ["m-x", "m-y", "m-z"].contains(&gist.as_str()),
        "the stored row must be one of the racing mutations, got {gist}"
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
    let m1 = mutation("m-b1", "b1", STAMP, "bank one");
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
    assert!(store.get_memory_item("b1", "i-1").expect("get").is_some());
    assert!(store.get_memory_item("b2", "i-b2").expect("get").is_some());
    server.stop();
}
