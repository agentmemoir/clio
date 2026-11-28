// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests which banks one push locks before it applies anything: the
//! envelope bank, every authorized bank a mutation writes item state to, and
//! every covered bank of a denial carrier it can record — including a SEALED
//! carrier, whose banks are read from the content this peer opens — sorted and
//! deduplicated so every push takes its locks in one global order. One test
//! drives the route itself, so the lock set is proved wired into `handle_push`
//! rather than merely computed.
//!
//! ## Owns
//! - The lock set one push computes from its envelope bank and its mutations
//! - The route-level proof that `handle_push` holds the lock of a mutation bank
//!
//! ## Does not own
//! - The push route's denial, stale, and sealed-guard behavior
//!   (`server_push_denial_tests`, `server_push_stale_tests`,
//!   `server_push_sealed_tests`)
//! - The apply lock map itself (`server`)
//!
//! ## Boundary
//!
//! These tests break when the set of banks a push locks changes; they never
//! change it.
//!
//! Keep this module focused on the push route's lock set.

use super::{banks_to_lock, handle_push};
use crate::apply_fixtures::{env, item_value};
use crate::client::client_tests::paired_key;
use crate::credential::AuthOutcome;
use crate::crypto::SyncCipherKey;
use crate::denial::{DenialCarrier, denial_mutation};
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncMutation};
use crate::server::ServeOptions;
use crate::server::server_tests::{push_body, sqlite};
use clio_store::{Store, SyncStore};
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

/// A stamp every fixture in this module shares; nothing here reads a clock.
const STAMP: &str = "2026-01-01T00:00:00Z";

/// One item mutation whose own bank is `bank`, built the way the other push
/// tests build theirs.
fn item_op(mid: &str, bank: &str) -> SyncMutation {
    env(
        mid,
        bank,
        ENTITY_KIND_ITEM,
        "i-1",
        OP_UPSERT,
        STAMP,
        "dev-a",
        item_value(bank, "i-1", "s-1", STAMP, "gist"),
    )
}

/// The same item mutation sealed under `key` in its own execution envelope,
/// which is the only item shape the push route accepts.
fn sealed_item_op(mid: &str, bank: &str, key: &SyncCipherKey) -> SyncMutation {
    let op = item_op(mid, bank);
    let content = op.content_value().clone();
    crate::envelope_fixtures::seal_content_for(op, key, &content)
}

/// One denial operation whose covered banks are `banks`, the shape the push
/// route reads as a carrier.
fn carrier_op(bank: &str, request_ref: &str, banks: &[&str]) -> SyncMutation {
    denial_mutation(
        bank,
        "s-held",
        &DenialCarrier::new(request_ref, banks),
        "dev-a",
        STAMP,
    )
}

/// One denial operation sealed under `key` in its own execution envelope: the
/// carrier travels as ciphertext, so only a key holder can name its banks.
fn sealed_carrier_op(
    bank: &str,
    request_ref: &str,
    banks: &[&str],
    key: &SyncCipherKey,
) -> SyncMutation {
    let op = carrier_op(bank, request_ref, banks);
    let content = op.content_value().clone();
    crate::envelope_fixtures::seal_content_for(op, key, &content)
}

/// A mutation that names the envelope bank asks for that one bank.
#[test]
fn envelope_only_when_every_mutation_stays_in_the_envelope_bank() {
    let mutations = vec![item_op("m-1", "a")];
    assert_eq!(banks_to_lock("a", &mutations, None), vec!["a".to_owned()]);
}

/// The mutation bank is locked even though the envelope names another bank:
/// the item row the mutation writes lives in the mutation's bank, so a push
/// that left it unlocked could race another push on that same row.
#[test]
fn locks_the_bank_a_mutation_writes_not_only_the_envelope_bank() {
    let mutations = vec![item_op("m-1", "b")];
    assert_eq!(
        banks_to_lock("a", &mutations, None),
        vec!["a".to_owned(), "b".to_owned()]
    );
}

/// A repeated bank costs one lock and the set comes back sorted, so every push
/// acquires its locks in the same global order.
#[test]
fn dedupes_and_sorts_the_banks_it_locks() {
    let mutations = vec![
        item_op("m-1", "a"),
        item_op("m-2", "b"),
        item_op("m-3", "a"),
    ];
    assert_eq!(
        banks_to_lock("b", &mutations, None),
        vec!["a".to_owned(), "b".to_owned()]
    );
}

/// A denial carrier writes a serve marker for every bank it covers, so the
/// covered bank is locked even when no mutation names it.
#[test]
fn locks_every_covered_bank_of_a_denial_carrier() {
    let mutations = vec![carrier_op("a", "req-1", &["c"])];
    assert_eq!(
        banks_to_lock("a", &mutations, None),
        vec!["a".to_owned(), "c".to_owned()]
    );
}

/// A carrier covering several banks, one of them the envelope bank, still
/// yields one sorted entry per bank.
#[test]
fn a_carrier_covering_the_envelope_bank_adds_only_the_other_banks() {
    let mutations = vec![carrier_op("a", "req-2", &["d", "a", "c"])];
    assert_eq!(
        banks_to_lock("a", &mutations, None),
        vec!["a".to_owned(), "c".to_owned(), "d".to_owned()]
    );
}

/// A SEALED carrier's covered banks are locked too: the apply path opens the
/// carrier under this peer's sync key and then writes one serve marker per
/// covered bank, so a set built from the ciphertext alone would leave every one
/// of those marker writes unlocked.
#[test]
fn locks_every_covered_bank_of_a_sealed_denial_carrier() {
    let key = paired_key();
    let mutations = vec![sealed_carrier_op("a", "req-sealed", &["c", "a"], &key)];
    // The ciphertext names no bank at all: without the key the covered bank is
    // invisible, which is exactly the gap this test closes.
    assert_eq!(
        banks_to_lock("a", &mutations, None),
        vec!["a".to_owned()],
        "a carrier this peer cannot open contributes no covered bank"
    );
    assert_eq!(
        banks_to_lock("a", &mutations, Some(&key)),
        vec!["a".to_owned(), "c".to_owned()],
        "the opened carrier's covered bank is locked"
    );
}

/// The route's own lock: a push blocked on a mutation bank's lock stays blocked
/// until that lock is released, then writes that bank. On a route that computed
/// the set but never held it, the push returns immediately and the first
/// `recv_timeout` reports `Ok` instead of `Timeout`.
///
/// The construction reads no clock and sleeps nowhere. The worker sends on
/// `started` immediately before it calls the route, so the main thread's timed
/// wait begins while the only work left before the lock is decoding one
/// in-memory body — the wait is a bound on that work, not on thread startup.
/// The lock is released only after the timed wait has already reported the
/// push blocked, so the second wait measures the push resuming, not a race.
#[test]
fn the_route_holds_the_lock_of_a_mutation_bank() {
    let (store, kms) = sqlite();
    let key = paired_key();
    let options = ServeOptions {
        store: Arc::clone(&store) as Arc<dyn Store>,
        sync: Arc::clone(&store) as Arc<dyn SyncStore>,
        kms,
        // The mutation writes bank "b" while the envelope names "a", so the
        // push can only be serialized by locking "b" itself.
        banks: vec!["a".into(), "b".into()],
        token_file: None,
        dev_mode: true,
        deadline_secs: 30,
        max_envelope_skew_secs: 0,
        device_id: "srv-1".into(),
        denial: None,
        sync_key: Some(Arc::new(key.clone())),
        bank_locks: crate::server::BankLocks::default(),
    };
    let held_handle = options.bank_locks.for_bank("b");
    let held = held_handle.lock().unwrap();
    let body = serde_json::to_string(&push_body("a", &[sealed_item_op("m-1", "b", &key)]))
        .expect("serialize push body");
    // A paired loopback stream, both ends in hand: the worker writes its answer
    // to the server end and the main thread reads it from the client end. No
    // accept loop runs, so nothing here contends for a connection slot.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind pair");
    let mut client =
        TcpStream::connect(listener.local_addr().expect("pair addr")).expect("connect");
    let (server_stream, _) = listener.accept().expect("accept the paired stream");
    let (started_tx, started_rx) = channel();
    let (done_tx, done_rx) = channel();
    let worker = std::thread::spawn(move || {
        let _ = started_tx.send(());
        let outcome = handle_push(&options, server_stream, &body, &AuthOutcome::Unbound)
            .map_err(|err| err.message);
        let _ = done_tx.send(outcome);
    });

    started_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the worker reaches the route");
    assert_eq!(
        done_rx.recv_timeout(Duration::from_millis(300)),
        Err(RecvTimeoutError::Timeout),
        "the push must still be blocked on bank b's lock"
    );
    drop(held);
    assert_eq!(
        done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the push resumes once the lock is released"),
        Ok(()),
        "the push completes normally after the lock is released"
    );
    worker.join().expect("worker joins");

    let mut response = String::new();
    // The route shuts its write side down after the answer, so this read ends
    // at the FIN rather than waiting on a timeout.
    client
        .read_to_string(&mut response)
        .expect("read the acknowledgement");
    assert!(
        response.contains("200"),
        "the push was answered: {response}"
    );
    assert!(
        store
            .get_memory_item("b", "i-1")
            .expect("read bank b")
            .is_some(),
        "the push wrote the mutation bank it held the lock for"
    );
}
