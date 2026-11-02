// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the refusal containment of the push feed: one row whose
//! `transaction_time` this build cannot place is refused, reported once
//! through the bank's refusal record, and holds only its own kind, so every
//! other kind in the same push still ships.
//!
//! ## Owns
//! - Behavior coverage for a packaging refusal that must not fail the bank push
//!
//! ## Does not own
//! - Page-complete cursor advance (`client_feed_wm_page_tests`)
//! - Apply-side freshness refusal (`client_freshness_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a packaging refusal stops holding its own kind; they
//! never change the packaging.
//!
//! Keep this module focused on refusal containment in one push.

use crate::apply::{last_stale_refusal, stale_refused};
use crate::client::client_tests::{item, paired_cfg, serve, sqlite};
use crate::client::{push, sync_status};
use crate::client_feed::kind_watermark;
use clio_store::{PersonaStore, Store, SyncStore};
use clio_types::PersonaStableEntry;

const BANK: &str = "bstamp";

/// A writer stamp no clock reading can place: a spelled month is not one of
/// the forms this build writes, so packaging refuses the row that carries it.
const UNPLACEABLE: &str = "January 1 2026";

/// One persona stable entry, the kind that must never be held by an item row's
/// refused stamp. Its bare-date `as_of` is a documented stored form.
fn stable_entry() -> PersonaStableEntry {
    PersonaStableEntry {
        bank_id: BANK.into(),
        key: "style".into(),
        value: "terse".into(),
        confidence: None,
        source: "explicit".into(),
        source_ref: Some("turn-1".into()),
        context: None,
        admission_score: 0.5,
        as_of: "2026-01-01".into(),
        valid_until: None,
    }
}

/// Seed one item row carrying `stamp` and one persona stable entry.
fn seed(a: &clio_store::SqliteStore, stamp: &str) {
    a.create_memory_item(&item(BANK, "i-bad", "s1", stamp, "g"), "actor")
        .expect("create item");
    a.put_stable_entry(&stable_entry(), "seed")
        .expect("persona stable");
}

/// One refused item row must not hold the persona kind: the persona mutation
/// still reaches the relay, and the refused row is never sent.
#[test]
fn an_unplaceable_stamp_holds_only_its_own_kind() {
    let (server, srv, _kms) = serve(&[BANK]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    seed(&a, UNPLACEABLE);

    push(&cfg_a, &[BANK.to_owned()]).expect("a refused row must not fail the bank push");

    let relayed = srv.journal_list_since(BANK, 0, 50).expect("server journal");
    assert!(
        relayed.iter().any(|r| r.entity_kind == "persona"),
        "the unrelated kind must still ship: {relayed:?}"
    );
    assert!(
        !relayed.iter().any(|r| r.entity_id == "i-bad"),
        "the refused row must never be sent: {relayed:?}"
    );
    server.stop();
}

/// The refusal is recorded once through the bank refusal record: one count and
/// a line naming the value, so it is reported rather than swallowed.
#[test]
fn an_unplaceable_stamp_is_reported_once() {
    let (server, _srv, _kms) = serve(&[BANK]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    seed(&a, UNPLACEABLE);

    push(&cfg_a, &[BANK.to_owned()]).expect("push the page");

    assert_eq!(
        stale_refused(cfg_a.sync.as_ref(), BANK).expect("refusal count"),
        1,
        "one refused row is one refusal, reported once"
    );
    let line = last_stale_refusal(cfg_a.sync.as_ref())
        .expect("read the status slot")
        .expect("a refusal line is written");
    assert!(
        line.contains(UNPLACEABLE),
        "the refusal line names the refused value: {line}"
    );
    server.stop();
}

/// The held kind does not claim progress: its cursor stays where it was while
/// the backlog it cannot ship is still counted.
#[test]
fn the_held_kind_keeps_its_cursor_and_reports_its_backlog() {
    let (server, _srv, _kms) = serve(&[BANK]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    seed(&a, UNPLACEABLE);
    let before = kind_watermark(&cfg_a, BANK, "item").expect("item cursor");

    push(&cfg_a, &[BANK.to_owned()]).expect("push the page");

    assert_eq!(
        kind_watermark(&cfg_a, BANK, "item").expect("item cursor"),
        before,
        "a kind that refused a row must not advance past it"
    );
    assert!(
        sync_status(&cfg_a, &[BANK.to_owned()])
            .expect("status")
            .banks[0]
            .pending_push
            >= 1,
        "the held row stays counted, so the kind does not read healthy"
    );
    server.stop();
}

/// A push with no refused row behaves exactly as before: every kind ships, the
/// item cursor advances past the page, and nothing is refused.
#[test]
fn a_push_with_no_refused_row_ships_every_kind() {
    let (server, srv, _kms) = serve(&[BANK]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    seed(&a, "2026-01-01T00:00:00Z");
    let before = kind_watermark(&cfg_a, BANK, "item").expect("item cursor");

    push(&cfg_a, &[BANK.to_owned()]).expect("push the page");

    let relayed = srv.journal_list_since(BANK, 0, 50).expect("server journal");
    assert!(
        relayed.iter().any(|r| r.entity_kind == "item"),
        "the item kind ships: {relayed:?}"
    );
    assert!(
        relayed.iter().any(|r| r.entity_kind == "persona"),
        "the persona kind ships: {relayed:?}"
    );
    assert_ne!(
        kind_watermark(&cfg_a, BANK, "item").expect("item cursor"),
        before,
        "an unrefused page still advances the item cursor"
    );
    assert_eq!(
        stale_refused(cfg_a.sync.as_ref(), BANK).expect("refusal count"),
        0,
        "a clean push records no refusal"
    );
    assert_eq!(
        sync_status(&cfg_a, &[BANK.to_owned()])
            .expect("status")
            .banks[0]
            .pending_push,
        0,
        "a clean push drains the bank"
    );
    server.stop();
}
