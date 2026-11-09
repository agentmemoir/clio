// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the unit fixtures of the candidate HTTP verb guard: the one verb the candidate endpoint accepts, and the refusal shape every other verb draws.
//!
//! It also drives the candidate endpoint: the refusal every other verb draws before the POST continuation runs, and the untouched result the accepted verb returns from it.
//!
//! ## Owns
//! - Every assertion about `check_method`, `refuse`, `endpoint`, and the accepted verb and refusal constants
//! - The pin that the candidate guard stays test-only and off the live route
//! - The pin that the endpoint answers through the shared guard and reaches the POST continuation only for the accepted verb
//!
//! ## Does not own
//! - The guard and the endpoint themselves (`method_guard`)
//! - The live route and its framing (see the crate's HTTP suites)
//!
//! ## Boundary
//!
//! Changes to which verb the candidate endpoint accepts, to the refusal shape, or to the order of the guard and the POST continuation should normally require changes here.
//! Changes to the live transport or a tool should not.
//!
//! Keep this module focused on the candidate guard's fixtures.

use super::{
    ACCEPTED_METHOD, ALLOW_HEADER, METHOD_NOT_ALLOWED, REFUSAL, Refusal, Response, check_method,
    endpoint, refuse,
};
use std::cell::Cell;

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

/// The result the POST continuation in these fixtures answers with, so a pass-through is told apart from a refusal by content and not only by status.
fn post_result() -> Response {
    (
        200,
        r#"{"jsonrpc":"2.0","id":1,"result":{}}"#.to_owned(),
        vec![("MCP-Session-Id".to_owned(), "session-1".to_owned())],
    )
}

/// One POST continuation, which counts its calls and answers [`post_result`].
fn counting_post(calls: &Cell<usize>) -> impl FnOnce() -> Response + '_ {
    move || {
        calls.set(calls.get() + 1);
        post_result()
    }
}

/// The response one verb draws from the endpoint, and the number of times its POST continuation ran.
fn endpoint_reply(method: &str) -> (Response, usize) {
    let calls = Cell::new(0);
    let response = endpoint(method, counting_post(&calls));
    (response, calls.get())
}

#[test]
fn the_endpoint_refuses_get_with_405_and_allow_post() {
    let (response, _) = endpoint_reply("GET");
    assert_eq!(response.0, METHOD_NOT_ALLOWED, "{response:?}");
    assert_eq!(response.0, 405, "{response:?}");
    assert_eq!(
        response.2,
        [(ALLOW_HEADER.to_owned(), ACCEPTED_METHOD.to_owned())],
        "{response:?}"
    );
    assert_eq!(
        response.2,
        [("Allow".to_owned(), "POST".to_owned())],
        "{response:?}"
    );
}

#[test]
fn get_never_reaches_the_post_continuation() {
    let (_, calls) = endpoint_reply("GET");
    assert_eq!(calls, 0, "the POST path did not run for a refused verb");
}

#[test]
fn the_endpoint_refuses_delete_with_405_and_allow_post() {
    let (response, _) = endpoint_reply("DELETE");
    assert_eq!(response.0, METHOD_NOT_ALLOWED, "{response:?}");
    assert_eq!(
        response.2,
        [("Allow".to_owned(), "POST".to_owned())],
        "{response:?}"
    );
}

#[test]
fn delete_never_reaches_the_post_continuation() {
    let (_, calls) = endpoint_reply("DELETE");
    assert_eq!(calls, 0, "the POST path did not run for a refused verb");
}

#[test]
fn post_returns_the_continuation_result_unchanged() {
    let (response, _) = endpoint_reply(ACCEPTED_METHOD);
    assert_eq!(
        response,
        post_result(),
        "the POST path's result is untouched"
    );
}

#[test]
fn post_runs_the_continuation_exactly_once() {
    let (_, calls) = endpoint_reply(ACCEPTED_METHOD);
    assert_eq!(calls, 1, "the POST path ran exactly once");
}

#[test]
fn a_refused_endpoint_result_carries_no_body_and_no_protocol_payload() {
    for method in ["GET", "DELETE"] {
        let (response, _) = endpoint_reply(method);
        assert!(response.1.is_empty(), "{method}: {response:?}");
        assert!(
            serde_json::from_str::<serde_json::Value>(&response.1).is_err(),
            "{method}: an empty body is no JSON-RPC envelope"
        );
    }
}

#[test]
fn the_endpoint_answers_through_the_shared_guard() {
    let guard = source("method_guard.rs");
    assert!(
        guard.contains("refuse(method)"),
        "the endpoint asks the one guard rather than repeating its decision"
    );
    let (response, _) = endpoint_reply("GET");
    assert_eq!(response.0, REFUSAL.status, "{response:?}");
    assert_eq!(
        response.2,
        [(
            REFUSAL.headers[0].0.to_owned(),
            REFUSAL.headers[0].1.to_owned()
        )],
        "{response:?}"
    );
    assert_eq!(response.1, REFUSAL.body, "{response:?}");
}
