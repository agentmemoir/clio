// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the pull loop's bounds: a peer that answers `has_more`
//! cannot keep one `pull_bank` call fetching forever. A page that hands back
//! the cursor it was asked from stops the pull with an error, a peer that never
//! finishes stops the pull at the page cap with an error, and a peer that
//! finishes after one page still succeeds, so the guards do not break normal
//! operation.
//!
//! ## Owns
//! - The cursor-repeat stop and the page-cap stop, each asserting the exact
//!   number of requests the fake peer received, and the one-page happy path
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

use crate::client::client_tests::{cfg, paired_key, sqlite};
use crate::client::pull;
use crate::protocol::encode_cursor;
use clio_store::SyncStore;
use clio_types::{AmError, ErrorCode};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

const BANK: &str = "bA";

/// One pull page as raw wire JSON: no rows, the given resume cursor, and the
/// given `has_more` flag. A row-free page keeps these tests about the loop's
/// bounds: nothing to apply, nothing to decrypt, only the cursor and the flag.
fn page(next_cursor: &str, has_more: bool) -> String {
    serde_json::to_string(&json!({
        "mutations": [],
        "next_cursor": next_cursor,
        "has_more": has_more,
    }))
    .expect("page json")
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

/// The refusal shape every bound shares: this crate's refusal code, and a
/// message naming the bank the pull was for.
fn assert_refusal(err: &AmError) {
    assert_eq!(
        err.code,
        ErrorCode::Forbidden,
        "a bound stops the pull as a refusal: {err:?}"
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
    assert_refusal(&err);
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
    assert_refusal(&err);
    assert!(
        err.message
            .contains(&format!("{}-page cap", crate::PULL_MAX_PAGES))
            && err.message.contains("did not finish"),
        "the refusal must name the cap and the unfinished pull: {}",
        err.message
    );
    assert_eq!(
        requests_seen(&seen),
        crate::PULL_MAX_PAGES,
        "exactly the capped number of pages, no more"
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
