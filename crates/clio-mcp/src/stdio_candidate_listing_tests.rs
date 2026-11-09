// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the candidate `2026-07-28` listing scenario
//! driven through the real stdio adapter: the multi-page cursor walk over the
//! advertised catalogue.
//!
//! ## Owns
//! - The `serve_candidate` listing walk, page by page, following `nextCursor`
//!
//! ## Does not own
//! - The candidate stdio scenario registry, or the metadata, envelope, and
//!   domain-read scenarios (see `stdio_candidate_tests`)
//! - The paging rules themselves (`tools_page`)
//! - The shared walk checker, its catalogue accessor, and its self-check
//!   (`stateless_fixtures`, `http_candidate_listing_tests`)
//!
//! ## Boundary
//!
//! Changes to a candidate stdio listing scenario should normally require changes
//! here. Changes to the registry, the paging rules, or the shared checker should
//! not.
//!
//! Keep this module focused on the candidate's stdio listing walk.

use super::serve_candidate;
use crate::runtime::McpState;
use crate::stateless_fixtures::{
    catalogue_names, check_listing_walk, check_success_envelope, paged_list, required_meta, state,
};
use serde_json::Value;
use std::io::Cursor;

/// The candidate fixture pin: the revision the candidate scenarios expect, not a
/// live protocol setting.
const CANDIDATE_FIXTURE_REVISION: &str = "2026-07-28";

/// Serve one frame over the real candidate stdio adapter and return its answer.
fn serve_one(state: &McpState, frame: &Value) -> Value {
    let mut input = serde_json::to_string(frame).expect("a fixture serializes");
    input.push('\n');
    let mut output = Vec::new();
    serve_candidate(state, Cursor::new(input.into_bytes()), &mut output)
        .expect("the candidate stdio adapter serves the frame");
    let text = String::from_utf8(output).expect("the adapter writes UTF-8");
    let line = text
        .lines()
        .next()
        .expect("a served frame answers one line");
    serde_json::from_str(line).expect("the answer is a JSON line")
}

/// The catalogue walked through the stdio adapter: more than one page, every
/// advertised tool once, each cursor followed as advertised.
#[test]
fn stdio_listing_pagination() {
    let state = state();
    let meta = required_meta(CANDIDATE_FIXTURE_REVISION);
    let catalogue = catalogue_names(&state);
    // Half the catalogue is two pages and keeps the walk short.
    let size = (catalogue.len() / 2).max(1);
    let mut pages: Vec<Value> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..=catalogue.len() {
        let answer = serve_one(&state, &paged_list(&meta, size, cursor.as_deref()));
        check_success_envelope(&answer, 7).expect("the candidate serves a listing page");
        let page = answer["result"].clone();
        cursor = page["nextCursor"].as_str().map(str::to_owned);
        pages.push(page);
        if cursor.is_none() {
            break;
        }
    }
    assert!(
        cursor.is_none(),
        "the walk reaches a terminal page in {} page(s)",
        pages.len()
    );
    check_listing_walk(&pages, &catalogue)
        .unwrap_or_else(|error| panic!("the stdio walk: {error}"));
}
