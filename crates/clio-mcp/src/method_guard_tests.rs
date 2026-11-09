// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unit fixtures of the candidate HTTP verb guard: the one verb the candidate endpoint accepts, and the refusal shape every other verb draws.
//!
//! ## Owns
//! - Every assertion about `check_method`, `refuse`, and the accepted verb and refusal constants
//! - The pin that the candidate guard stays test-only and off the live route
//!
//! ## Does not own
//! - The guard itself (`method_guard`)
//! - The live route and its framing (see the crate's HTTP suites)
//!
//! ## Boundary
//!
//! Changes to which verb the candidate endpoint accepts, or to the refusal shape, should normally require changes here.
//! Changes to the live transport or a tool should not.
//!
//! Keep this module focused on the candidate guard's fixtures.

use super::{
    ACCEPTED_METHOD, ALLOW_HEADER, METHOD_NOT_ALLOWED, REFUSAL, Refusal, check_method, refuse,
};

/// Verbs a request line can carry that the candidate endpoint does not accept.
///
/// The lower-case forms are there because the request reader upper-cases the verb it parses; the empty one because a request line can be malformed.
const REFUSED_METHODS: &[&str] = &[
    "GET", "DELETE", "PUT", "HEAD", "OPTIONS", "PATCH", "TRACE", "CONNECT", "post", "Post", "",
];

/// The refusal one verb draws, asserted to be a refusal.
fn fault(method: &str) -> Refusal {
    check_method(method).expect_err("the guard refuses this verb")
}

/// The source of one file of this crate.
fn source(file: &str) -> String {
    let path = format!("{}/src/{file}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("`{file}` is readable"))
}

#[test]
fn post_is_the_one_accepted_verb() {
    assert_eq!(ACCEPTED_METHOD, "POST");
    assert_eq!(check_method("POST"), Ok(()));
}

#[test]
fn post_draws_no_refusal() {
    assert!(
        refuse("POST").is_none(),
        "the route serves the accepted verb"
    );
}

#[test]
fn get_is_refused_with_405_and_allow_post() {
    let refusal = fault("GET");
    assert_eq!(refusal.status, 405, "{refusal:?}");
    assert_eq!(refusal.status, METHOD_NOT_ALLOWED, "{refusal:?}");
    assert_eq!(refusal.headers, [(ALLOW_HEADER, ACCEPTED_METHOD)]);
    assert_eq!(refusal.headers[0], ("Allow", "POST"));
}

#[test]
fn delete_is_refused_with_405_and_allow_post() {
    let refusal = fault("DELETE");
    assert_eq!(refusal.status, 405, "{refusal:?}");
    assert_eq!(refusal.headers[0], ("Allow", "POST"), "{refusal:?}");
}

#[test]
fn every_verb_but_post_is_refused() {
    for method in REFUSED_METHODS {
        assert_eq!(
            fault(method),
            REFUSAL,
            "`{method}` is answered with the one refusal"
        );
    }
}

#[test]
fn a_refused_verb_carries_no_body_and_no_protocol_payload() {
    for method in ["GET", "DELETE"] {
        let refusal = fault(method);
        assert!(refusal.body.is_empty(), "{method}: {refusal:?}");
        assert!(
            serde_json::from_str::<serde_json::Value>(refusal.body).is_err(),
            "{method}: an empty body is no JSON-RPC envelope"
        );
    }
}

#[test]
fn a_refused_verb_opens_no_stream() {
    for method in ["GET", "DELETE"] {
        let refusal = fault(method);
        assert_eq!(
            refusal.headers,
            [(ALLOW_HEADER, ACCEPTED_METHOD)],
            "{method}: the `Allow` header is the only one, so no content type names a stream"
        );
    }
}

#[test]
fn the_guard_reads_the_verb_and_nothing_else() {
    let guard = source("method_guard.rs");
    assert!(
        !guard.contains("serde_json") && !guard.contains("Value"),
        "the guard holds no JSON type, so no body is parsed before the decision"
    );
    assert!(
        guard.contains("fn check_method(method: &str)"),
        "the verb is the decision's one input"
    );
}

#[test]
fn the_candidate_guard_stays_test_only_and_off_the_live_route() {
    let lib = source("lib.rs");
    assert!(
        lib.contains("#[cfg(test)]\nmod method_guard;"),
        "the candidate guard stays test-only"
    );
    let route = source("http.rs");
    assert!(
        !route.contains("method_guard"),
        "the live route is not wired to the candidate guard"
    );
    assert!(
        route.contains("req.method == \"DELETE\""),
        "the live route still serves the verbs it served before"
    );
}
