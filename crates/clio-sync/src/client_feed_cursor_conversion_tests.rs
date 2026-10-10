// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the one-shot conversion of the retired shared push
//! watermark (`push_wm`) into a kind's own compound cursor: identity
//! preservation of the decoded `(stamp, entity)` pair, canonical
//! re-encoding, replay determinism through the real push path, and the
//! malformed/empty token semantics.
//!
//! ## Owns
//! - Behavior coverage for the shared-cursor conversion and its resume point
//! - Behavior coverage for the guarded conversion write's non-overwrite rule
//!
//! ## Does not own
//! - Per-kind advance and hold rules (`client_feed_wm_tests`)
//! - Feed SQL resume semantics (`clio-store` sync feed tests)
//!
//! ## Boundary
//!
//! Tests here break when the cursor conversion changes; they never change it.
//!
//! Keep this module focused on the shared-cursor conversion tests.

use crate::client::client_tests::{cfg, item, serve, sqlite};
use crate::client::push;
use crate::protocol::{decode_watermark, encode_watermark};
use crate::server::SyncServer;
use clio_store::{Store, SyncStore};
use clio_types::ErrorCode;

/// Drop guard for the shared `serve` test server: stops the accept loop even
/// when an assertion panics, so a failed test never leaks the server thread.
/// Drop-guard pattern from `crates/clio/tests/sync_wire_support/mod.rs`
/// (`impl Drop for Relay`); `SyncServer::stop` is idempotent, so the success
/// path needs no explicit call.
struct StopServer<'a>(&'a SyncServer);

impl Drop for StopServer<'_> {
    fn drop(&mut self) {
        self.0.stop();
    }
}

/// Canonical compound body with no entity part: the exact form the cursor
/// conversion produces for a retired stamp-only pair.
fn empty_pair(stamp: &str) -> String {
    format!("{stamp}\u{1f}")
}

/// The retired shared cursor is converted once into the kind's own cursor:
/// the canonical re-encode preserves the decoded `(stamp, "")` pair, the
/// shared row may remain stored, and it never resumes the feed again.
#[test]
fn legacy_shared_cursor_is_converted_once_into_the_kind_cursor() {
    let a = sqlite();
    let cfg_a = cfg("dev-a", &a, "127.0.0.1:1", Vec::new());
    a.create_memory_item(
        &item("bleg", "i-1", "s1", "2026-01-01T00:00:00Z", "g"),
        "actor",
    )
    .expect("create");
    // Retired single-cursor state: only the shared watermark exists, and it
    // sits after every row's stamp.
    let shared = encode_watermark("bleg", "2030-01-01T00:00:00Z");
    a.cursor_set("bleg", "push_wm", &shared)
        .expect("shared cursor");

    let since = crate::client_feed::kind_watermark(&cfg_a, "bleg", "item").expect("convert");
    assert_eq!(
        since,
        empty_pair("2030-01-01T00:00:00Z"),
        "the converted body is the canonical compound form of the decoded pair"
    );
    let stored = a
        .cursor_get("bleg", "push_wm:item")
        .expect("kind cursor read")
        .expect("the conversion persists the kind's own cursor");
    assert_eq!(
        decode_watermark("bleg", &stored).expect("decode"),
        since,
        "the persisted kind cursor decodes to the same resume point"
    );
    assert_eq!(
        a.cursor_get("bleg", "push_wm")
            .expect("shared read")
            .as_deref(),
        Some(shared.as_str()),
        "the shared row may remain stored"
    );
    assert_eq!(
        crate::client_feed::feed_kind_count(&cfg_a, "bleg", "item", &since).expect("count"),
        0,
        "the converted watermark still filters rows already past it"
    );

    // The shared row is never a resume point again: moving it later leaves
    // the converted kind cursor in charge and overwrites nothing.
    a.cursor_set(
        "bleg",
        "push_wm",
        &encode_watermark("bleg", "2040-01-01T00:00:00Z"),
    )
    .expect("move shared cursor");
    assert_eq!(
        crate::client_feed::kind_watermark(&cfg_a, "bleg", "item").expect("re-read"),
        since
    );
    assert_eq!(
        a.cursor_get("bleg", "push_wm:item")
            .expect("kind cursor reread")
            .as_deref(),
        Some(stored.as_str()),
        "a stored per-kind cursor is never overwritten"
    );
}

/// Concurrent-interleave model for the guarded conversion write: the
/// per-kind cursor was absent when the conversion first read, then a
/// concurrent push advanced it before the conversion wrote. The guarded
/// write re-reads and skips, so the advanced resume point survives
/// byte-identical. The store has no conditional write, so the guard is a
/// probe-then-set, not one atomic operation; this test pins the skip, not
/// the whole race window.
#[test]
fn conversion_write_skips_a_kind_cursor_that_appeared_after_the_first_read() {
    let a = sqlite();
    let cfg_a = cfg("dev-a", &a, "127.0.0.1:1", Vec::new());
    let advanced = encode_watermark("brace", "2027-01-01T00:00:00Z\u{1f}i-9");
    a.cursor_set("brace", "push_wm:item", &advanced)
        .expect("advanced kind cursor");
    // The stale value a racing conversion computes from the still-stored
    // shared row: an older position than the advanced cursor.
    let stale = encode_watermark("brace", "2026-01-01T00:00:00Z");
    super::kind_watermark_convert_write(&cfg_a, "brace", "item", &stale);
    assert_eq!(
        a.cursor_get("brace", "push_wm:item")
            .expect("kind cursor read")
            .as_deref(),
        Some(advanced.as_str()),
        "the conversion never resets an advanced per-kind cursor"
    );
}

/// A retired stamp-only pair resumes inclusive of its stamp tie: the
/// converted canonical body feeds exactly the rows the retired body fed.
#[test]
fn converted_stamp_only_pair_feeds_the_same_rows_as_the_retired_body() {
    let a = sqlite();
    let cfg_a = cfg("dev-a", &a, "127.0.0.1:1", Vec::new());
    let stamp = "2026-01-01T00:00:00Z";
    for id in ["i-a", "i-b"] {
        a.create_memory_item(&item("btie", id, "s1", stamp, "g"), "actor")
            .expect("create");
    }
    a.cursor_set("btie", "push_wm", &encode_watermark("btie", stamp))
        .expect("shared cursor");

    let converted = crate::client_feed::kind_watermark(&cfg_a, "btie", "item").expect("convert");
    assert_eq!(converted, empty_pair(stamp));
    let retired: Vec<String> = a
        .feed_items("btie", stamp, crate::PULL_PAGE_LIMIT)
        .expect("retired feed")
        .iter()
        .map(|r| r.entity_id.clone())
        .collect();
    let canonical: Vec<String> = a
        .feed_items("btie", &converted, crate::PULL_PAGE_LIMIT)
        .expect("canonical feed")
        .iter()
        .map(|r| r.entity_id.clone())
        .collect();
    assert_eq!(
        canonical, retired,
        "the canonical re-encode resumes inclusively of the stamp tie, like the retired body"
    );
    assert_eq!(canonical, vec!["i-a".to_owned(), "i-b".to_owned()]);
}

/// A current per-kind cursor resumes normally and beats a stored shared row:
/// no conversion runs and the feed resumes strictly after the pair.
#[test]
fn current_kind_cursor_resumes_normally_and_beats_the_shared_row() {
    let a = sqlite();
    let cfg_a = cfg("dev-a", &a, "127.0.0.1:1", Vec::new());
    let body = "2026-01-01T00:00:00Z\u{1f}i-a".to_owned();
    a.cursor_set("bcur", "push_wm:item", &encode_watermark("bcur", &body))
        .expect("kind cursor");
    a.cursor_set(
        "bcur",
        "push_wm",
        &encode_watermark("bcur", "2030-01-01T00:00:00Z"),
    )
    .expect("shared cursor");
    for id in ["i-a", "i-b"] {
        a.create_memory_item(
            &item("bcur", id, "s1", "2026-01-01T00:00:00Z", "g"),
            "actor",
        )
        .expect("create");
    }

    let since = crate::client_feed::kind_watermark(&cfg_a, "bcur", "item").expect("read");
    assert_eq!(
        since, body,
        "the current opaque checkpoint resumes normally"
    );
    let ids: Vec<String> = a
        .feed_items("bcur", &since, crate::PULL_PAGE_LIMIT)
        .expect("feed")
        .iter()
        .map(|r| r.entity_id.clone())
        .collect();
    assert_eq!(
        ids,
        vec!["i-b".to_owned()],
        "resume is strictly after (stamp, i-a)"
    );
}

/// The real push path converts the retired shared cursor and then drains
/// every pending row exactly once; replay leaves the converted resume point
/// byte-identical and the shared row unused.
#[test]
fn real_push_path_converts_the_shared_cursor_and_drains_exactly_once() {
    let (server, srv, _kms) = serve(&["bconv"]);
    let _server_stop = StopServer(&server);
    let addr = server.addr().to_owned();
    let a = sqlite();
    let cfg_a = cfg("dev-a", &a, &addr, Vec::new());
    let stamp = "2026-01-01T00:00:00Z";
    let n = 3;
    for i in 0..n {
        let id = format!("i-{i:03}");
        a.create_memory_item(&item("bconv", &id, "s1", stamp, "g"), "actor")
            .expect("create");
    }
    // Retired single-cursor state before every row: the durable resume point
    // says "everything past the shared stamp", so all three rows are owed.
    let shared = encode_watermark("bconv", "2025-12-31T00:00:00Z");
    a.cursor_set("bconv", "push_wm", &shared)
        .expect("shared cursor");

    push(&cfg_a, &["bconv".to_owned()]).expect("push 1");
    let first = a
        .cursor_get("bconv", "push_wm:item")
        .expect("read kind cursor")
        .expect("the real path persists the converted kind cursor");
    assert_eq!(
        decode_watermark("bconv", &first).expect("decode"),
        format!("{stamp}\u{1f}i-002"),
        "the converted cursor resumes at the last delivered row's pair"
    );
    push(&cfg_a, &["bconv".to_owned()]).expect("push 2");
    let rows = srv.journal_list_since("bconv", 0, 50).expect("journal");
    let mut ids: Vec<String> = rows.iter().map(|r| r.sync_mutation_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(rows.len(), n, "no row is lost or duplicated across replay");
    assert_eq!(ids.len(), n, "every mutation id is unique");
    assert_eq!(
        a.cursor_get("bconv", "push_wm:item")
            .expect("kind cursor reread")
            .as_deref(),
        Some(first.as_str()),
        "replay is deterministic: the converted resume point does not drift"
    );
    assert_eq!(
        a.cursor_get("bconv", "push_wm")
            .expect("shared reread")
            .as_deref(),
        Some(shared.as_str()),
        "the shared row stays stored and unused"
    );
}

/// A bank with no cursor of any kind starts from the canonical compound
/// sentinel, and the epoch-millis guard is unchanged.
#[test]
fn a_fresh_bank_starts_from_the_canonical_compound_sentinel() {
    let a = sqlite();
    let cfg_a = cfg("dev-a", &a, "127.0.0.1:1", Vec::new());
    let since = crate::client_feed::kind_watermark(&cfg_a, "bfresh", "item").expect("read");
    assert_eq!(since, empty_pair("1970-01-01T00:00:00Z"));
    assert!(
        a.cursor_get("bfresh", "push_wm:item")
            .expect("read cursor")
            .is_none(),
        "no cursor is written without a stored legacy row"
    );
    a.create_memory_item(
        &item("bfresh", "millis-1", "s1", "1735689600000", "g"),
        "actor",
    )
    .expect("create");
    let ids: Vec<String> = a
        .feed_items("bfresh", &since, crate::PULL_PAGE_LIMIT)
        .expect("feed")
        .iter()
        .map(|r| r.entity_id.clone())
        .collect();
    assert_eq!(
        ids,
        vec!["millis-1".to_owned()],
        "the sentinel guard still includes epoch-millis rows"
    );
}

/// Garbage and empty tokens keep their existing semantics: malformed kind or
/// shared tokens error, a wrong-bank token errors, and an empty token means
/// the canonical empty-watermark sentinel.
#[test]
fn malformed_and_empty_tokens_keep_their_semantics() {
    let a = sqlite();
    let cfg_a = cfg("dev-a", &a, "127.0.0.1:1", Vec::new());

    a.cursor_set("bneg-kind", "push_wm:item", "not-a-watermark")
        .expect("kind cursor");
    let err = crate::client_feed::kind_watermark(&cfg_a, "bneg-kind", "item")
        .expect_err("a malformed kind token is an error");
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    a.cursor_set("bneg-shared", "push_wm", "not-a-watermark")
        .expect("shared cursor");
    let err = crate::client_feed::kind_watermark(&cfg_a, "bneg-shared", "item")
        .expect_err("a malformed shared token is an error");
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    a.cursor_set(
        "bneg-bank",
        "push_wm",
        &encode_watermark("other-bank", "2030-01-01T00:00:00Z"),
    )
    .expect("wrong-bank shared cursor");
    let err = crate::client_feed::kind_watermark(&cfg_a, "bneg-bank", "item")
        .expect_err("a checksum mismatch is an error");
    assert_eq!(err.code, ErrorCode::InvalidArgument);

    a.cursor_set("bempty", "push_wm", "")
        .expect("empty shared cursor");
    assert_eq!(
        crate::client_feed::kind_watermark(&cfg_a, "bempty", "item").expect("empty token"),
        empty_pair("1970-01-01T00:00:00Z"),
        "an empty token still means the empty-watermark sentinel"
    );
    assert!(
        a.cursor_get("bempty", "push_wm:item")
            .expect("read cursor")
            .is_some(),
        "the conversion persists the sentinel as the kind cursor"
    );
}
