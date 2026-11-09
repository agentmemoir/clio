// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the candidate `2026-07-28` HTTP scenario
//! driven through the real adapter, the multi-page cursor walk over the
//! advertised catalogue, and for the self-check that a tampered walk is
//! rejected.
//!
//! ## Owns
//! - The HTTP listing walk over the fixtures' live candidate server, page by
//!   page, following `nextCursor`
//! - The negative self-check of the shared walk checker
//!
//! ## Does not own
//! - The candidate HTTP scenario registry, or the metadata, envelope, and
//!   domain-read scenarios (see `http_candidate_tests`)
//! - The candidate HTTP seam and its guards (`http_candidate`, `method_guard`)
//! - The live-server fixtures (`http_candidate_fixtures`), including the
//!   `spawn_candidate` and `post` helpers, and the shared walk checker
//!   (`stateless_fixtures`)
//!
//! ## Boundary
//!
//! Changes to a candidate HTTP listing scenario should normally require changes
//! here. Changes to the registry, the seam, or the shared checker should not.
//!
//! Keep this module focused on the candidate's HTTP listing walk.

use crate::http_candidate_fixtures::{body, headers, post, spawn_candidate};
use crate::stateless_fixtures::{
    CANDIDATE_FIXTURE_REVISION, catalogue_names, check_listing_walk, check_success_envelope,
    paged_list, required_meta, state,
};
use serde_json::{Value, json};

/// The catalogue walked through the HTTP adapter: more than one page, every
/// advertised tool once, each cursor followed as advertised.
#[test]
fn http_listing_pagination() {
    let port = spawn_candidate();
    let meta = required_meta(CANDIDATE_FIXTURE_REVISION);
    let catalogue = catalogue_names(&state());
    // Half the catalogue is two pages and keeps the walk short.
    let size = (catalogue.len() / 2).max(1);
    let mut pages: Vec<Value> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..=catalogue.len() {
        let frame = paged_list(&meta, size, cursor.as_deref());
        let response = post(port, &frame, &headers("tools/list"));
        assert_eq!(response.status, 200, "the candidate serves a listing page");
        let answer = body(&response);
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
    assert_page_sizes(&pages, size);
    check_listing_walk(&pages, &catalogue).unwrap_or_else(|error| panic!("the http walk: {error}"));
}

/// The walker's requested page size is honoured: every page but the last is
/// exactly `size` tools long, so a page that silently ignores the request and
/// answers the default size cannot pass.
fn assert_page_sizes(pages: &[Value], size: usize) {
    assert!(
        pages.len() > 1,
        "the walk covers more than one page, not {}",
        pages.len()
    );
    let last = pages.len() - 1;
    for (index, page) in pages[..last].iter().enumerate() {
        let tools = page["tools"]
            .as_array()
            .expect("a listed page carries tools");
        assert_eq!(tools.len(), size, "page {index} holds {size} tools: {page}");
    }
}

/// A two-page synthetic walk in the shape a listing page carries: the tools each
/// page lists, and the cursor the first page advertises.
fn walk(first: &[&str], cursor: &str, second: &[&str]) -> Vec<Value> {
    vec![
        json!({ "tools": tools(first), "nextCursor": cursor }),
        json!({ "tools": tools(second) }),
    ]
}

/// One page's tool entries as the listing carries them.
fn tools(names: &[&str]) -> Value {
    json!(
        names
            .iter()
            .map(|name| json!({ "name": name }))
            .collect::<Vec<_>>()
    )
}

/// The walk checker rejects a tampered sequence, one regression at a time: an
/// omitted tool, a repeated tool, a cursor that does not advance, and a sequence
/// that is not a walk at all.
#[test]
fn a_tampered_listing_walk_fails_the_checker() {
    let catalogue: Vec<String> = ["alpha", "beta", "gamma", "delta"]
        .map(str::to_owned)
        .to_vec();

    check_listing_walk(
        &walk(&["alpha", "beta"], "c1", &["gamma", "delta"]),
        &catalogue,
    )
    .expect("the intact walk is accepted");

    let omitted = walk(&["alpha", "beta"], "c1", &["gamma"]);
    let error = check_listing_walk(&omitted, &catalogue).expect_err("an omitted tool fails");
    assert!(error.contains("3 of the 4"), "{error}");

    let repeated = walk(&["alpha", "beta"], "c1", &["gamma", "gamma"]);
    let error = check_listing_walk(&repeated, &catalogue).expect_err("a repeated tool fails");
    assert!(error.contains("appears twice"), "{error}");

    let stalled = vec![
        json!({ "tools": tools(&["alpha", "beta"]), "nextCursor": "c1" }),
        json!({ "tools": tools(&["gamma"]), "nextCursor": "c1" }),
        json!({ "tools": tools(&["delta"]) }),
    ];
    let error = check_listing_walk(&stalled, &catalogue).expect_err("a stalled cursor fails");
    assert!(error.contains("repeats the cursor"), "{error}");

    let one_page = vec![json!({ "tools": tools(&["alpha"]) })];
    let error = check_listing_walk(&one_page, &catalogue).expect_err("one page is no walk");
    assert!(error.contains("more than one page"), "{error}");
}
