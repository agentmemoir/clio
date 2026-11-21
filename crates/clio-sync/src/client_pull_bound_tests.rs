// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the pull loop's bounds: a peer that answers `has_more`
//! cannot keep one `pull_bank` call fetching forever. A page that hands back
//! the cursor it was asked from stops the pull with an error *before* its rows
//! are applied, a peer that never finishes stops the pull at the page cap with
//! an error that says the cursor is saved and a re-run continues, a page that
//! freezes the bank still breaks cleanly, and a peer that finishes after one
//! page still succeeds, so the guards do not break normal operation.
//!
//! ## Owns
//! - The cursor-repeat stop, the page-cap stop, the refused-page assertion that
//!   none of its rows were written, the freeze-beats-repeat control, and the
//!   one-page happy path, each asserting the exact number of requests the fake
//!   peer received
//! - The scripted fake peer used by these tests only
//!
//! ## Does not own
//! - The guards themselves (`client`, `PULL_MAX_PAGES` in the crate root)
//! - Requested-bank refusals (`client_bank_guard_tests`)
//!
//! ## Boundary
//!
//! Tests here break when the pull loop's bound changes; they never change it.
//!
//! Keep this module focused on pull-loop bound tests.

use crate::client::client_tests::{cfg, item, paired_key, sqlite};
use crate::client::pull;
use crate::envelope_fixtures::seal_own_content;
use crate::protocol::{SyncContent, SyncMutation, encode_cursor};
use clio_store::{DeadLetterStatus, Store, SyncStore};
use clio_types::{AmError, ErrorCode};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

const BANK: &str = "bA";

/// One pull page as raw wire JSON: the given rows, the given resume cursor, and
/// the given `has_more` flag.
fn page(next_cursor: &str, has_more: bool) -> String {
    raw_page(&[], next_cursor, has_more)
}

/// [`page`] with rows: the bound tests that must prove a refused page applied
/// nothing serve one real row through here.
fn raw_page(rows: &[SyncMutation], next_cursor: &str, has_more: bool) -> String {
    serde_json::to_string(&json!({
        "mutations": rows,
        "next_cursor": next_cursor,
        "has_more": has_more,
    }))
    .expect("page json")
}

/// One real `item` upsert for `BANK`, sealed under the one shared key so the
/// client can open it: the row a repeat-refused page must not write.
fn applied_row(id: &str) -> SyncMutation {
    let document = serde_json::to_value(item(BANK, id, "s1", "2026-01-01T00:00:00Z", "from relay"))
        .expect("item json");
    seal_own_content(SyncMutation {
        sync_mutation_id: format!("m-up-{id}"),
        bank: BANK.to_owned(),
        entity_kind: "item".into(),
        entity_id: id.to_owned(),
        op: "upsert".into(),
        transaction_time: "2026-01-01T00:00:00Z".into(),
        device_id: "relay".into(),
        content: SyncContent::Plain(document),
        content_hash: "h".into(),
        admission_receipt: None,
    })
}

/// Serve `pages` in order, one page per connection, and record each request
/// body when `seen` is given. The thread ends after the last page, so a pull
/// that kept fetching past the scripted pages finds the listener closed
/// instead of a page: the fixture cannot hide a runaway loop.
fn serve_pages(pages: &[String], seen: Option<Arc<Mutex<Vec<String>>>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr").to_string();
    let pages = pages.to_vec();
    thread::spawn(move || {
        for (stream, page) in listener.incoming().zip(pages) {
            let mut stream = stream.expect("accept");
            let request = read_declared_request(&mut stream);
            if let Some(seen) = &seen {
                seen.lock().expect("seen lock").push(request);
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            );
            stream.write_all(response.as_bytes()).expect("respond");
        }
    });
    addr
}

/// Read one client request to its declared end: the head terminator plus the
/// `Content-Length` body, so the page is written after the client finished
/// sending and the close never resets the client mid-read. Returns the body.
fn read_declared_request(stream: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).expect("read request");
        buf.extend_from_slice(&chunk[..n]);
        let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let declared = String::from_utf8_lossy(&buf[..pos])
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.trim().eq_ignore_ascii_case("content-length") {
                    value.trim().parse().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);
        if buf.len() >= pos + 4 + declared {
            return String::from_utf8_lossy(&buf[pos + 4..]).into_owned();
        }
    }
}

/// How many requests the fake peer answered.
fn requests_seen(seen: &Arc<Mutex<Vec<String>>>) -> usize {
    seen.lock().expect("seen lock").len()
}

/// The refusal shape every bound shares: the bound's own code, and a message
/// naming the bank the pull was for.
fn assert_refusal(err: &AmError, code: ErrorCode) {
    assert_eq!(
        err.code, code,
        "a bound stops the pull under its own code: {err:?}"
    );
    assert!(
        err.message.contains(BANK),
        "the refusal must name bank {BANK}: {}",
        err.message
    );
}

#[test]
fn repeated_cursor_stops_the_pull_with_an_error() {
    let b = sqlite();
    // The cursor the peer keeps handing back. Non-empty on purpose: the
    // first-pull cursor is the empty token, and a peer that repeats a real
    // cursor is the shape that would serve one page forever.
    let repeated = encode_cursor(BANK, 1);
    assert!(!repeated.is_empty(), "a served cursor is never empty");
    // The first page hands out `repeated` (the request asked from ""), the
    // second is served to a request that already carried `repeated` and hands
    // the very same token back: no progress. Two pages is all the pull may
    // fetch; a third request finds the listener closed.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let addr = serve_pages(
        &[page(&repeated, true), page(&repeated, true)],
        Some(seen.clone()),
    );
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let err = pull(&cfg_b, &[BANK.to_owned()]).expect_err("a repeated cursor is refused");
    assert_refusal(&err, ErrorCode::Forbidden);
    assert!(
        err.message.contains("repeated cursor") && err.message.contains(&repeated),
        "the refusal must name the cursor that did not advance: {}",
        err.message
    );
    assert_eq!(
        requests_seen(&seen),
        2,
        "the pull stops on the first repeat, not after an unbounded run"
    );
    // Nothing advanced past the cursor the first page persisted.
    let cursor = b.cursor_get(BANK, "pull").expect("cursor");
    assert_eq!(cursor.as_deref(), Some(repeated.as_str()));
}

/// A page that repeats the cursor it was asked from is refused *before* its
/// rows can land: the decision is taken right after the response is
/// deserialized, so the one real row that page carries is opened but never
/// applied. If the check ever drifts back below the apply loop, that row is
/// written and this test fails.
#[test]
fn a_repeat_refused_page_writes_none_of_its_rows() {
    let b = sqlite();
    let repeated = encode_cursor(BANK, 1);
    let row = applied_row("i-refused");
    // Page one hands out `repeated` and persists it; page two is served to a
    // request that already carried `repeated`, carries one real row, and hands
    // the same token back with `has_more: true`: no progress.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let addr = serve_pages(
        &[
            page(&repeated, true),
            raw_page(std::slice::from_ref(&row), &repeated, true),
        ],
        Some(seen.clone()),
    );
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let err = pull(&cfg_b, &[BANK.to_owned()]).expect_err("a repeated cursor is refused");
    assert_refusal(&err, ErrorCode::Forbidden);
    assert_eq!(
        requests_seen(&seen),
        2,
        "the pull stops on the first repeat, not after an unbounded run"
    );
    assert!(
        b.get_memory_item(BANK, "i-refused")
            .expect("row read")
            .is_none(),
        "the refused page's row must not be written"
    );
    assert!(
        b.journal_list_since(BANK, 0, 200)
            .expect("journal")
            .is_empty(),
        "the refused page's row must not be journaled"
    );
    let cursor = b.cursor_get(BANK, "pull").expect("cursor");
    assert_eq!(cursor.as_deref(), Some(repeated.as_str()));
}

#[test]
fn pull_stops_after_the_page_cap() {
    let b = sqlite();
    // A peer that advances the cursor every page and never finishes: the one
    // shape the per-page cursors cannot stop, so only the cap can.
    let pages: Vec<String> = (1..=crate::PULL_MAX_PAGES)
        .map(|seq| {
            page(
                &encode_cursor(BANK, i64::try_from(seq).expect("page seq")),
                true,
            )
        })
        .collect();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let addr = serve_pages(&pages, Some(seen.clone()));
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let err = pull(&cfg_b, &[BANK.to_owned()]).expect_err("a peer that never finishes is refused");
    // The cap is this loop's own bound, not a peer violation, so it carries a
    // range code rather than the `Forbidden` a repeated cursor carries: an
    // honest peer whose backlog exceeded the cap must not read as an attack.
    assert_refusal(&err, ErrorCode::OutOfRange);
    assert_ne!(
        err.code,
        ErrorCode::Forbidden,
        "the cap must not share the repeated cursor's refusal code: {err:?}"
    );
    assert!(
        err.message
            .contains(&format!("{}-page cap", crate::PULL_MAX_PAGES)),
        "the refusal must name the cap: {}",
        err.message
    );
    // The wording promises the operator that this is resumable: the cursor of
    // every page is durable, so re-running continues from it.
    assert!(
        err.message.contains("the cursor is saved")
            && err
                .message
                .contains("running the pull again continues from there"),
        "the refusal must say the cursor is saved and a re-run continues: {}",
        err.message
    );
    assert!(
        !err.message.contains("did not finish"),
        "the cap must not blame the peer for an honest oversized backlog: {}",
        err.message
    );
    assert_eq!(
        requests_seen(&seen),
        crate::PULL_MAX_PAGES,
        "exactly the capped number of pages, no more"
    );
    // The cap lost nothing: the last page's cursor is the one stored.
    let cursor = b.cursor_get(BANK, "pull").expect("cursor");
    assert_eq!(
        cursor.as_deref(),
        Some(encode_cursor(BANK, i64::try_from(crate::PULL_MAX_PAGES).expect("cap")).as_str()),
        "the cap leaves the last persisted cursor in place so a re-run resumes"
    );
}

/// A page that repeats its cursor *and* carries a row this node cannot trust
/// freezes the bank instead of failing as a repeat: the freeze is a stop for
/// the operator (the cursor must stay where the poisoned row is), not a peer
/// protocol violation, so it wins over the repeat refusal.
#[test]
fn a_frozen_page_wins_over_the_repeat_refusal() {
    let b = sqlite();
    // A plaintext row while this node holds a key: `decrypt_for_apply`
    // quarantines it, which freezes the cursor at the prior position.
    let mut poisoned = applied_row("i-frozen");
    poisoned.content = SyncContent::Plain(json!({}));
    // The empty first cursor repeated with `has_more: true` is the same fixed
    // point as a non-empty one, and it lets the poisoned row be the only row.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let addr = serve_pages(&[raw_page(&[poisoned], "", true)], Some(seen.clone()));
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let status = pull(&cfg_b, &[BANK.to_owned()]).expect("a freeze is a clean stop, not an error");
    assert_eq!(requests_seen(&seen), 1, "the frozen page ends the pull");
    assert_eq!(
        status.first().and_then(|s| s.pull_cursor.clone()),
        Some(String::new())
    );
    assert_eq!(
        b.dead_letter_count(BANK, DeadLetterStatus::Open)
            .expect("dead letters"),
        1,
        "the untrusted row is quarantined for the operator"
    );
    let cursor = b.cursor_get(BANK, "pull").expect("cursor");
    assert!(
        cursor.as_deref().unwrap_or_default().is_empty(),
        "the cursor stays frozen at the poisoned row: {cursor:?}"
    );
}

#[test]
fn single_page_pull_still_succeeds() {
    let b = sqlite();
    let done = encode_cursor(BANK, 1);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let addr = serve_pages(&[page(&done, false)], Some(seen.clone()));
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let status = pull(&cfg_b, &[BANK.to_owned()]).expect("one finishing page is a normal pull");
    assert_eq!(requests_seen(&seen), 1, "a finished pull stops at one page");
    assert_eq!(status.first().map(|s| s.bank_id.as_str()), Some(BANK));
    assert_eq!(
        status.first().and_then(|s| s.pull_cursor.clone()),
        Some(done.clone())
    );
    // The guard leaves the success path alone: the cursor is still persisted.
    let cursor = b.cursor_get(BANK, "pull").expect("cursor");
    assert_eq!(cursor, Some(done));
}

/// A peer that cycles between two cursors never hands back the token it was
/// asked from, so only the set of cursors this pull already consumed can stop
/// it: the pull fails on the second visit instead of running to the cap.
#[test]
fn a_two_cursor_cycle_stops_the_pull_before_the_cap() {
    let b = sqlite();
    let a = encode_cursor(BANK, 1);
    let second = encode_cursor(BANK, 2);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let addr = serve_pages(
        &[page(&a, true), page(&second, true), page(&a, true)],
        Some(seen.clone()),
    );
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let err = pull(&cfg_b, &[BANK.to_owned()]).expect_err("a cursor cycle is refused");
    assert_refusal(&err, ErrorCode::Forbidden);
    assert!(
        err.message.contains("repeated cursor") && err.message.contains(&a),
        "the refusal must name the cursor the peer handed back: {}",
        err.message
    );
    assert_eq!(
        requests_seen(&seen),
        3,
        "the cycle is caught on its second visit to {a}, before the cap"
    );
    // Only consumed cursors advanced: the last one is the second page's.
    let cursor = b.cursor_get(BANK, "pull").expect("cursor");
    assert_eq!(cursor.as_deref(), Some(second.as_str()));
}

/// A cursor that advances every page is honest work: the consumed-cursor set
/// must not refuse it, and the pull still applies both pages and exits.
#[test]
fn a_two_page_pull_applies_both_pages_and_exits_normally() {
    let b = sqlite();
    let a = encode_cursor(BANK, 1);
    let second = encode_cursor(BANK, 2);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let addr = serve_pages(
        &[
            raw_page(&[applied_row("i-first")], &a, true),
            raw_page(&[applied_row("i-second")], &second, false),
        ],
        Some(seen.clone()),
    );
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let status = pull(&cfg_b, &[BANK.to_owned()]).expect("advancing pages are a normal pull");
    assert_eq!(requests_seen(&seen), 2, "the pull stops at its last page");
    assert_eq!(
        status.first().and_then(|s| s.pull_cursor.clone()),
        Some(second.clone())
    );
    for id in ["i-first", "i-second"] {
        assert!(
            b.get_memory_item(BANK, id).expect("row read").is_some(),
            "{id} must be applied: only a cursor this pull already used is refused"
        );
    }
    let cursor = b.cursor_get(BANK, "pull").expect("cursor");
    assert_eq!(cursor, Some(second));
}

/// On a first pull the starting cursor is the empty token, so a peer that hands
/// the empty token back has not advanced either: the first page is refused.
#[test]
fn a_repeated_empty_cursor_stops_the_pull_with_an_error() {
    let b = sqlite();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let addr = serve_pages(&[page("", true)], Some(seen.clone()));
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let err = pull(&cfg_b, &[BANK.to_owned()]).expect_err("a repeated empty cursor is refused");
    assert_refusal(&err, ErrorCode::Forbidden);
    assert!(
        err.message.contains("repeated cursor"),
        "the refusal must name the repeat: {}",
        err.message
    );
    assert_eq!(
        requests_seen(&seen),
        1,
        "the first page already repeats the starting cursor"
    );
}
