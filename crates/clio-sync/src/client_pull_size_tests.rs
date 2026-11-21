// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module tests the pull's per-response size bounds: a peer cannot make one
//! page cost unbounded memory through the cursor token or through the response
//! body itself.
//!
//! ## Owns
//! - The oversized-cursor refusal and the oversized-response refusal, each
//!   asserting the peer was asked once and the cursor was left unstored
//!
//! ## Does not own
//! - The bounds themselves (`client`, `http`, `PULL_MAX_CURSOR_BYTES`)
//! - The page-cap and cursor-repeat bounds (`client_pull_bound_tests`)
//!
//! ## Boundary
//!
//! Tests here break when a per-response size bound changes; they never change it.
//!
//! Keep this module focused on pull size-bound tests.

use super::client_pull_bound_tests::{assert_refusal, page, requests_seen, serve_pages};
use crate::client::client_tests::{cfg, paired_key, sqlite};
use crate::client::pull;
use clio_store::SyncStore;
use clio_types::ErrorCode;
use std::sync::{Arc, Mutex};

const BANK: &str = "bA";

#[test]
fn an_oversized_cursor_is_refused_before_it_is_stored() {
    let b = sqlite();
    let big = "c".repeat(crate::PULL_MAX_CURSOR_BYTES + 1);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let addr = serve_pages(&[page(&big, true)], Some(seen.clone()));
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let err = pull(&cfg_b, &[BANK.to_owned()]).expect_err("an oversized cursor is refused");
    assert_refusal(&err, ErrorCode::Forbidden);
    assert!(
        err.message.contains("cursor"),
        "the refusal must name the cursor: {}",
        err.message
    );
    assert_eq!(requests_seen(&seen), 1, "the oversized page ends the pull");
    assert_eq!(
        b.cursor_get(BANK, "pull").expect("cursor"),
        None,
        "the oversized cursor is never stored"
    );
}

#[test]
fn an_oversized_response_is_refused() {
    let b = sqlite();
    let big = "r".repeat(crate::http::MAX_RESPONSE_BYTES);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let addr = serve_pages(&[page(&big, false)], Some(seen.clone()));
    let cfg_b = cfg("dev-b", &b, &addr, vec![paired_key()]);
    let err = pull(&cfg_b, &[BANK.to_owned()]).expect_err("an oversized response is refused");
    let cap = crate::http::MAX_RESPONSE_BYTES.to_string();
    assert!(
        err.message.contains("larger than") && err.message.contains(&cap),
        "the refusal must name the cap: {}",
        err.message
    );
    assert_eq!(
        requests_seen(&seen),
        1,
        "the oversized response ends the pull"
    );
    assert_eq!(
        b.cursor_get(BANK, "pull").expect("cursor"),
        None,
        "a refused response stores no cursor"
    );
}
