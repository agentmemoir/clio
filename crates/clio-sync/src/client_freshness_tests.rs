// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pull-path coverage of the envelope
//! freshness bounds: a relay-delivered row older than a positive age bound is
//! refused with no effect and no cursor freeze, a row inside the bound applies
//! through the same path, and the shipped default (no bound) applies an old
//! backlog row.
//!
//! ## Owns
//! - End-to-end coverage of `ApplyOutcome::Stale` through `client::pull`
//! - The cursor advance past a stale row, and the absence of a store row,
//!   journal row, or dead letter for it
//! - The fresh control that proves the bound is not refusing everything
//! - The default-off control that keeps a backlog from being dropped
//!
//! ## Does not own
//! - The bound rule itself (`apply_freshness_tests`)
//! - Stamp parsing (`clock_tests`)
//! - The sealed-envelope refusals (`client_envelope_tests`)
//!
//! ## Boundary
//!
//! These tests break when the pull path's handling of a stale row changes;
//! they never change it.
//!
//! Test-only. Never compiled into production builds.

use crate::ENVELOPE_SKEW_DEFAULT_SECS;
use crate::apply::ApplyEngine;
use crate::apply::apply_freshness::{is_stale, pinned};
use crate::client::client_envelope_tests::{BANK, Receiver, hand_push, held_key, receiver};
use crate::client::client_tests::item;
use crate::client::pull;
use crate::clock::{parse_utc_epoch, test_pin};
use crate::crypto::SyncCipherKey;
use crate::envelope_fixtures::seal_content_for;
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncContent, SyncMutation, decode_cursor};
use clio_store::{DeadLetterStatus, Store, SyncStore};

/// The instant the pinned clock reads. A fixed stamp, so the verdict of a case
/// never depends on the day it runs.
const PINNED_CLOCK: &str = "2026-06-01T00:00:00Z";

/// The positive age bound the windowed cases set: the value an always-online
/// fleet would choose. It is explicit because the shipped default is `0`.
const WINDOW: u64 = 300;

/// A writer stamp months behind [`PINNED_CLOCK`], so far outside a 300 second
/// bound that no rounding can bring it back inside.
const STALE_STAMP: &str = "2026-01-01T00:00:00Z";

/// A writer stamp ten minutes behind [`PINNED_CLOCK`]: an ordinary backlog row,
/// the one the shipped default must keep.
const BACKLOG_STAMP: &str = "2026-05-31T23:50:00Z";

const STALE_ID: &str = "stale-a";
const FRESH_ID: &str = "fresh-a";
const BACKLOG_ID: &str = "backlog-a";

/// Pin the clock for the whole case; the guard unpins it when the test ends.
fn pinned_clock() -> test_pin::Pinned {
    test_pin::pin(parse_utc_epoch(PINNED_CLOCK).expect("the pinned stamp parses"))
}

/// A receiver whose pull applies under an explicit positive age bound, not the
/// disabled one the shipped default holds.
fn windowed_receiver(key: &SyncCipherKey) -> Receiver {
    let mut r = receiver(&[BANK], key);
    r.cfg.max_envelope_skew_secs = WINDOW;
    r
}

fn mutation_id(id: &str, stamp: &str, op: &str) -> String {
    clio_store::sha256_hex(
        crate::keyed_roles::published_mutation_id_salt(),
        &format!("{id}|{stamp}|{op}"),
    )
}

/// One legitimately sealed item upsert written at `stamp`: the row a peer whose
/// clock disagrees with this node's would have produced.
fn sealed_at(bank: &str, id: &str, stamp: &str, key: &SyncCipherKey) -> SyncMutation {
    let fixture = item(bank, id, "s1", stamp, "gist");
    let plain = serde_json::to_value(&fixture).expect("serialize the item");
    seal_content_for(
        SyncMutation {
            sync_mutation_id: mutation_id(id, stamp, OP_UPSERT),
            bank: bank.to_owned(),
            entity_kind: ENTITY_KIND_ITEM.into(),
            entity_id: id.to_owned(),
            op: OP_UPSERT.to_owned(),
            transaction_time: stamp.to_owned(),
            device_id: "dev-hand".into(),
            content: SyncContent::Plain(serde_json::Value::Null),
            content_hash: clio_store::sha256_hex(
                crate::keyed_roles::published_digest_salt(),
                "anything",
            ),
            admission_receipt: None,
        },
        key,
        &plain,
    )
}

/// The predicate the pull path itself calls, over the same pinned clock, so a
/// refusal this module observes is the freshness one and not another guard.
fn stale_per_the_pinned_clock(r: &Receiver, row: &SyncMutation, window: u64) -> bool {
    let engine = ApplyEngine {
        store: r.store.as_ref(),
        sync: r.store.as_ref(),
        device_id: r.cfg.device_id.clone(),
        kms: None,
        clock: pinned(parse_utc_epoch(PINNED_CLOCK).expect("the pinned stamp parses")),
        max_envelope_skew_secs: window,
    };
    is_stale(&engine, &row.transaction_time)
}

/// The issue's expected behaviour at a positive bound: a relay delivers a
/// validly sealed row whose authenticated writer stamp sits outside the bound,
/// and the pull refuses it with no effect while the cursor still moves past it.
#[test]
fn a_pull_refuses_a_stale_envelope_and_advances_the_cursor_past_it() {
    let _clock = pinned_clock();
    let key = held_key("client-freshness-stale-key");
    let r = windowed_receiver(&key);
    let row = sealed_at(BANK, STALE_ID, STALE_STAMP, &key);
    assert!(
        stale_per_the_pinned_clock(&r, &row, WINDOW),
        "the row is outside the bound, so pull must answer Stale"
    );
    hand_push(&r.addr, BANK, row);
    pull(&r.cfg, &[BANK.to_owned()]).expect("pull");
    // The four effects `Stale` leaves behind, and the ones it must not leave:
    // nothing materialised, nothing journaled, and no dead letter, which is what
    // separates it from a quarantine that would freeze the cursor.
    assert!(
        r.store
            .get_memory_item(BANK, STALE_ID)
            .expect("get")
            .is_none(),
        "a stale row materialised an entity"
    );
    assert_eq!(
        r.store.journal_max_seq(BANK).expect("max seq"),
        0,
        "a stale row appended a journal row"
    );
    assert!(
        r.store
            .journal_list_since(BANK, 0, 200)
            .expect("journal")
            .is_empty(),
        "a stale row left journal rows"
    );
    assert_eq!(
        r.store
            .dead_letter_count(BANK, DeadLetterStatus::Open)
            .expect("count"),
        0,
        "a stale row was quarantined instead of skipped"
    );
    let cursor = r
        .store
        .cursor_get(BANK, "pull")
        .expect("cursor")
        .expect("the pull cursor advanced past the refused row");
    // The relay serves from this sequence number, so the refused row is behind
    // the cursor rather than in front of it.
    assert!(
        decode_cursor(&cursor, BANK).expect("the cursor decodes") >= 1,
        "the cursor did not move past the refused row"
    );
    // A second pull from that cursor is not stuck on the same row: the cursor
    // holds, and the row still applies nothing.
    let again = pull(&r.cfg, &[BANK.to_owned()]).expect("second pull");
    assert_eq!(
        again[0].pull_cursor.as_deref(),
        Some(cursor.as_str()),
        "the next pull is stuck on the refused row"
    );
    assert!(
        r.store
            .get_memory_item(BANK, STALE_ID)
            .expect("get")
            .is_none(),
        "the redelivered stale row applied"
    );
    assert_eq!(
        r.store.journal_max_seq(BANK).expect("max seq"),
        0,
        "the redelivered stale row journaled"
    );
    r.server.stop();
}

/// The positive control: the same pull path, the same bound, and a row written
/// inside it, so the guard is not simply refusing every envelope.
#[test]
fn a_pull_applies_a_fresh_envelope_under_the_same_window() {
    let _clock = pinned_clock();
    let key = held_key("client-freshness-fresh-key");
    let r = windowed_receiver(&key);
    let row = sealed_at(BANK, FRESH_ID, PINNED_CLOCK, &key);
    assert!(
        !stale_per_the_pinned_clock(&r, &row, WINDOW),
        "the row is inside the bound, so pull must apply it"
    );
    hand_push(&r.addr, BANK, row);
    pull(&r.cfg, &[BANK.to_owned()]).expect("pull");
    let got = r
        .store
        .get_memory_item(BANK, FRESH_ID)
        .expect("get")
        .expect("a fresh row applies");
    assert_eq!(got.gist.as_deref(), Some("gist"));
    assert_eq!(
        r.store.journal_max_seq(BANK).expect("max seq"),
        1,
        "the fresh row journaled once"
    );
    assert_eq!(
        r.store
            .dead_letter_count(BANK, DeadLetterStatus::Open)
            .expect("count"),
        0,
        "the fresh row was not quarantined"
    );
    assert!(
        r.store.cursor_get(BANK, "pull").expect("cursor").is_some(),
        "the cursor advanced past the applied row"
    );
    r.server.stop();
}

/// The shipped default is no age bound, so a row a peer wrote ten minutes
/// before this node's clock applies through the real pull path. A positive
/// default would have dropped it and advanced the cursor past it, which is the
/// backlog regression this pins.
#[test]
fn a_pull_applies_a_ten_minute_old_row_under_the_shipped_default() {
    let _clock = pinned_clock();
    let key = held_key("client-freshness-backlog-key");
    let r = receiver(&[BANK], &key);
    assert_eq!(
        r.cfg.max_envelope_skew_secs, ENVELOPE_SKEW_DEFAULT_SECS,
        "this case must run the shipped default, not a fixture override"
    );
    let row = sealed_at(BANK, BACKLOG_ID, BACKLOG_STAMP, &key);
    assert!(
        !stale_per_the_pinned_clock(&r, &row, ENVELOPE_SKEW_DEFAULT_SECS),
        "ten minutes of age must not be refused under the shipped default"
    );
    hand_push(&r.addr, BANK, row);
    pull(&r.cfg, &[BANK.to_owned()]).expect("pull");
    let got = r
        .store
        .get_memory_item(BANK, BACKLOG_ID)
        .expect("get")
        .expect("a ten-minute-old row applies when no age bound is set");
    assert_eq!(got.gist.as_deref(), Some("gist"));
    assert_eq!(
        r.store.journal_max_seq(BANK).expect("max seq"),
        1,
        "the backlog row journaled once instead of being dropped"
    );
    r.server.stop();
}
