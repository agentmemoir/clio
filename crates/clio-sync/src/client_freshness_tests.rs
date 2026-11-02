// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the pull-path coverage of the envelope
//! freshness window: a relay-delivered row written outside the window is
//! refused with no effect and no cursor freeze, while a row written inside the
//! window applies through the same path.
//!
//! ## Owns
//! - End-to-end coverage of `ApplyOutcome::Stale` through `client::pull`
//! - The cursor advance past a stale row, and the absence of a store row,
//!   journal row, or dead letter for it
//! - The fresh control that proves the window is not refusing everything
//!
//! ## Does not own
//! - The window rule itself (`apply_freshness_tests`)
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
use crate::apply::apply_freshness::{is_stale, test_engine};
use crate::client::client_tests::item;
use crate::client::pull;
use crate::client::client_envelope_tests::{BANK, Receiver, hand_push, held_key, receiver};
use crate::clock::{parse_utc_epoch, test_pin};
use crate::crypto::SyncCipherKey;
use crate::envelope_fixtures::seal_content_for;
use crate::protocol::{ENTITY_KIND_ITEM, OP_UPSERT, SyncContent, SyncMutation, decode_cursor};
use clio_store::{DeadLetterStatus, Store, SyncStore};

/// The instant the pinned clock reads. A fixed stamp, so the verdict of a case
/// never depends on the day it runs.
const PINNED_CLOCK: &str = "2026-06-01T00:00:00Z";

/// A writer stamp months behind [`PINNED_CLOCK`], so far outside a 300 second
/// window that no rounding can bring it back inside.
const STALE_STAMP: &str = "2026-01-01T00:00:00Z";

const STALE_ID: &str = "stale-a";
const FRESH_ID: &str = "fresh-a";

/// Pin the clock for the whole case; the guard unpins it when the test ends.
fn pinned_clock() -> test_pin::Pinned {
    test_pin::pin(parse_utc_epoch(PINNED_CLOCK).expect("the pinned stamp parses"))
}

/// A receiver whose pull applies under the shipped window, not the disabled one
/// the shared fixture uses.
fn windowed_receiver(key: &SyncCipherKey) -> Receiver {
    let mut r = receiver(&[BANK], key);
    r.cfg.max_envelope_skew_secs = ENVELOPE_SKEW_DEFAULT_SECS;
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
fn stale_per_the_pinned_clock(r: &Receiver, row: &SyncMutation) -> bool {
    let engine = test_engine(
        r.store.as_ref(),
        r.store.as_ref(),
        &r.cfg.device_id,
        None,
        PINNED_CLOCK,
    );
    is_stale(&engine, &row.transaction_time)
}

/// The issue's expected behaviour: a relay delivers a validly sealed row whose
/// authenticated writer stamp sits outside the window, and the pull refuses it
/// with no effect while the cursor still moves past it.
#[test]
fn a_pull_refuses_a_stale_envelope_and_advances_the_cursor_past_it() {
    let _clock = pinned_clock();
    let key = held_key("client-freshness-stale-key");
    let r = windowed_receiver(&key);
    let row = sealed_at(BANK, STALE_ID, STALE_STAMP, &key);
    assert!(
        stale_per_the_pinned_clock(&r, &row),
        "the row is outside the window, so pull must answer Stale"
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

/// The positive control: the same pull path, the same window, and a row written
/// inside the window, so the guard is not simply refusing every envelope.
#[test]
fn a_pull_applies_a_fresh_envelope_under_the_same_window() {
    let _clock = pinned_clock();
    let key = held_key("client-freshness-fresh-key");
    let r = windowed_receiver(&key);
    let row = sealed_at(BANK, FRESH_ID, PINNED_CLOCK, &key);
    assert!(
        !stale_per_the_pinned_clock(&r, &row),
        "the row is inside the window, so pull must apply it"
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
