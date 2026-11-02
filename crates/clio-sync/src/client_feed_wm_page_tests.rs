// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the page-complete rule of the per-kind push watermarks:
//! when a page is not full the kind advances past every row that page
//! returned, including an item row whose close stamp (`tx_until`) the store
//! placed above its ordering stamp, so no returned row is selected again; when
//! a page is full the kind advances only to its last returned row.
//!
//! ## Owns
//! - Behavior coverage for the page-complete advance of the item kind
//!
//! ## Does not own
//! - Page-full drain and stamp-tie coverage (`client_feed_wm_tests`)
//! - Feed SQL resume semantics (`clio-store` sync feed tests)
//!
//! ## Boundary
//!
//! Tests here break when the page-complete advance changes; they never change
//! it.
//!
//! Keep this module focused on the page-complete cursor advance.

use crate::client::client_tests::{item, paired_cfg, serve, sqlite};
use crate::client::push;
use crate::client_feed::kind_watermark;
use crate::protocol::encode_watermark;
use clio_store::{CorrectionSpec, SqliteStore, Store, SyncStore};
use std::sync::Arc;

const BANK: &str = "bpage";

/// Seed one plain item on `store`.
fn seed(store: &Arc<SqliteStore>, bank: &str, id: &str, stamp: &str) {
    store
        .create_memory_item(&item(bank, id, "s1", stamp, "g"), "actor")
        .expect("create item");
}

/// Close `prior` with a correction stamped at `stamp`. A correction whose
/// stamp is not after the prior's own start makes the store's close stamp
/// (`tx_until`) land one second above `updated_at`, which is the state the
/// item feed's OR over `tx_until` selects on and the ordering stamp does not
/// cover.
fn close_prior(store: &Arc<SqliteStore>, bank: &str, prior: &str, new_id: &str, stamp: &str) {
    let mut corrected = item(bank, new_id, "s1", stamp, "corrected");
    corrected.tx_from = stamp.to_owned();
    store
        .correct_item(&CorrectionSpec {
            bank_id: bank.to_owned(),
            prior_item_id: prior.to_owned(),
            new_item: corrected,
            reason: "the gist was wrong".to_owned(),
            actor: "user".to_owned(),
            timestamp: stamp.to_owned(),
        })
        .expect("correct the prior item");
}

/// An item whose `tx_until` is post-dated is pushed once and not again.
///
/// The feed row's ordering stamp is `COALESCE(updated_at, created_at)` while
/// the predicate also selects on `tx_until`, so a cursor that stops at the
/// ordering stamp leaves the row selected on every later push.
#[test]
fn a_post_dated_item_close_is_pushed_once_and_not_again() {
    let (server, srv, _kms) = serve(&[BANK]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    seed(&a, BANK, "i-prior", "2026-01-01T00:00:00Z");
    close_prior(&a, BANK, "i-prior", "i-new", "2026-01-01T00:00:00Z");

    push(&cfg_a, &[BANK.to_owned()]).expect("push page 1");
    let rows = srv.journal_list_since(BANK, 0, 50).unwrap();
    let mut sent: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r.entity_id.clone(), r.op.clone()))
        .collect();
    sent.sort();
    assert_eq!(
        sent,
        vec![
            ("i-new".to_owned(), "upsert".to_owned()),
            ("i-prior".to_owned(), "invalidate".to_owned()),
        ],
        "both the close and the corrected row ship once"
    );
    assert_eq!(
        crate::client::sync_status(&cfg_a, &[BANK.to_owned()])
            .expect("status")
            .banks[0]
            .pending_push,
        0,
        "the post-dated close must not be selected again after it shipped"
    );

    // The second push sends nothing: the cursor already passed the close.
    push(&cfg_a, &[BANK.to_owned()]).expect("push page 2");
    assert_eq!(
        srv.journal_list_since(BANK, 0, 50).unwrap().len(),
        rows.len(),
        "the shipped close is not re-sent"
    );
    assert_eq!(
        crate::client::sync_status(&cfg_a, &[BANK.to_owned()])
            .expect("status")
            .banks[0]
            .pending_push,
        0
    );
    server.stop();
}

/// A page that is not full advances past every row it returned: the cursor
/// lands on the highest store stamp the item predicate compares, and the same
/// feed read with that cursor returns nothing.
#[test]
fn a_not_full_page_advances_past_every_returned_row() {
    let (server, _srv, _kms) = serve(&[BANK]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    seed(&a, BANK, "i-a", "2026-01-01T00:00:01Z");
    seed(&a, BANK, "i-b", "2026-01-01T00:00:02Z");
    seed(&a, BANK, "i-prior", "2026-01-01T00:00:09Z");
    close_prior(&a, BANK, "i-prior", "i-new", "2026-01-01T00:00:00Z");

    push(&cfg_a, &[BANK.to_owned()]).expect("push the page");
    let cursor = kind_watermark(&cfg_a, BANK, "item").expect("item cursor");
    assert_eq!(
        cursor, "2026-01-01T00:00:10Z\u{1f}i-prior",
        "the cursor is the page's highest store stamp, in store text"
    );
    assert!(
        a.feed_items(BANK, &cursor, crate::PULL_PAGE_LIMIT)
            .expect("feed with the new cursor")
            .is_empty(),
        "no returned row is selected by the advanced cursor"
    );
    assert_eq!(
        crate::client::sync_status(&cfg_a, &[BANK.to_owned()])
            .expect("status")
            .banks[0]
            .pending_push,
        0
    );
    server.stop();
}

/// A full page advances only to its last returned row, so the remaining rows
/// stay pending and the next push resumes there.
#[test]
fn a_full_page_advances_only_to_its_last_row() {
    let (server, _srv, _kms) = serve(&[BANK]);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, &addr);
    let n = crate::PULL_PAGE_LIMIT + 3;
    for i in 0..n {
        seed(
            &a,
            BANK,
            &format!("i-{i:03}"),
            &format!("2026-01-01T00:{:02}:{:02}Z", i / 60, i % 60),
        );
    }

    push(&cfg_a, &[BANK.to_owned()]).expect("push the full page");
    let cursor = kind_watermark(&cfg_a, BANK, "item").expect("item cursor");
    assert_eq!(
        cursor, "2026-01-01T00:03:19Z\u{1f}i-199",
        "a full page advances to its last row, not past the page"
    );
    let queued: Vec<String> = a
        .feed_items(BANK, &cursor, crate::PULL_PAGE_LIMIT)
        .expect("feed with the new cursor")
        .into_iter()
        .map(|r| r.entity_id)
        .collect();
    assert_eq!(
        queued,
        vec!["i-200".to_owned(), "i-201".to_owned(), "i-202".to_owned()],
        "rows after the page still await the next push"
    );
    server.stop();
}

/// The pre-encoded sentinel is what an empty cursor body carries, so a
/// first push reads the whole bank. Guards the fixture the cases above assume.
#[test]
fn an_empty_bank_starts_from_the_sentinel_body() {
    let a = sqlite();
    let cfg_a = paired_cfg("dev-a", &a, "127.0.0.1:1");
    seed(&a, BANK, "i-only", "2026-01-01T00:00:00Z");
    let since = kind_watermark(&cfg_a, BANK, "item").expect("item cursor");
    assert_eq!(
        encode_watermark(BANK, &since),
        encode_watermark(BANK, "1970-01-01T00:00:00Z\u{1f}"),
        "an unset kind starts from the empty-watermark sentinel"
    );
}
