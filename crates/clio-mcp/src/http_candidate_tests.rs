// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the candidate `2026-07-28` scenario
//! registry and the metadata, discovery, envelope, and domain-read scenarios
//! driven through the real HTTP adapter.
//!
//! ## Owns
//! - The scenario registry: each candidate HTTP scenario's id, implementation
//!   owner, and bounded check receipt
//! - The required and optional metadata, discovery identity, result/error
//!   envelope, and domain-read scenarios
//!
//! ## Does not own
//! - The candidate HTTP seam (`http_candidate`), its guards
//!   (`method_guard`, `conditional_http`), or its admission (`stateless`)
//! - The shared server, request, and checker fixtures
//!   (`http_candidate_fixtures`)
//! - The conditional-header, verb, session, and removed-method scenarios (see
//!   `http_candidate_guard_tests`)
//!
//! ## Boundary
//!
//! Changes to a registered candidate HTTP scenario should normally require
//! changes here. Changes to the active transport or the active pin should not.
//!
//! Keep this module focused on the registry and the metadata, envelope, and
//! domain-read scenarios.

use crate::http_candidate_fixtures::{
    CANDIDATE_FIXTURE_REVISION, body, check_complete_result, check_discovery_identity,
    check_error_envelope, check_success_envelope, headers, name_headers, post, spawn_candidate,
    spawn_domain_candidate,
};
use crate::stateless::CANDIDATE_REVISION;
use crate::stateless_fixtures::{
    DOMAIN_ITEM_ID, check_domain_listing, check_gist_read, full_meta, read_call, request,
    required_meta,
};
use serde_json::json;

/// One candidate HTTP scenario and the bounded check that proves it.
struct Scenario {
    /// Stable scenario id.
    id: &'static str,
    /// The candidate implementation the scenario covers.
    owner: &'static str,
    /// The one command that runs the test proving the scenario.
    receipt: &'static str,
}

/// Every candidate HTTP scenario this module covers.
const SCENARIOS: &[Scenario] = &[
    Scenario {
        id: "http/required-metadata",
        owner: "stateless dispatcher",
        receipt: "cargo test -p clio-mcp --lib http_required_metadata_refusal",
    },
    Scenario {
        id: "http/optional-metadata",
        owner: "stateless dispatcher",
        receipt: "cargo test -p clio-mcp --lib http_optional_metadata_accepted",
    },
    Scenario {
        id: "http/discovery-identity",
        owner: "result envelopes",
        receipt: "cargo test -p clio-mcp --lib http_discovery_identity_lives_in_result_meta",
    },
    Scenario {
        id: "http/success-envelope",
        owner: "result envelopes",
        receipt: "cargo test -p clio-mcp --lib http_success_envelope_shape",
    },
    Scenario {
        id: "http/error-envelope",
        owner: "result envelopes",
        receipt: "cargo test -p clio-mcp --lib http_error_envelope_shape",
    },
    Scenario {
        id: "http/conditional-method-header",
        owner: "conditional headers",
        receipt: "cargo test -p clio-mcp --lib http_conditional_method_header_mismatch_refused",
    },
    Scenario {
        id: "http/conditional-name-header",
        owner: "conditional headers",
        receipt: "cargo test -p clio-mcp --lib http_conditional_name_header_mismatch_refused",
    },
    Scenario {
        id: "http/get-refused",
        owner: "HTTP method guard",
        receipt: "cargo test -p clio-mcp --lib http_get_is_refused",
    },
    Scenario {
        id: "http/no-session-identifier",
        owner: "session lifecycle",
        receipt: "cargo test -p clio-mcp --lib http_no_session_identifier_minted_or_required",
    },
    Scenario {
        id: "http/removed-methods",
        owner: "stateless dispatcher",
        receipt: "cargo test -p clio-mcp --lib http_removed_methods_are_refused",
    },
    Scenario {
        id: "http/origin-gate",
        owner: "HTTP gate",
        receipt: "cargo test -p clio-mcp --lib http_disallowed_origin_is_403",
    },
    Scenario {
        id: "http/path-gate",
        owner: "HTTP gate",
        receipt: "cargo test -p clio-mcp --lib http_wrong_path_is_404",
    },
    Scenario {
        id: "http/content-type-gate",
        owner: "HTTP gate",
        receipt: "cargo test -p clio-mcp --lib http_wrong_media_type_is_415",
    },
    Scenario {
        id: "http/accept-gate",
        owner: "HTTP gate",
        receipt: "cargo test -p clio-mcp --lib http_unacceptable_accept_is_406",
    },
    Scenario {
        id: "http/auth-gate",
        owner: "HTTP gate",
        receipt: "cargo test -p clio-mcp --lib http_missing_and_present_credentials_are_gated",
    },
    Scenario {
        id: "http/domain-read",
        owner: "read tools",
        receipt: "cargo test -p clio-mcp --lib http_domain_read_payloads",
    },
    Scenario {
        id: "http/listing-pagination",
        owner: "tool listing paging",
        receipt: "cargo test -p clio-mcp --lib http_listing_pagination",
    },
    Scenario {
        id: "harness/successful-removed-response",
        owner: "conformance harness",
        receipt: "cargo test -p clio-mcp --lib a_successful_response_to_a_removed_method_fails_the_candidate_check",
    },
    Scenario {
        id: "harness/wrong-result-shape",
        owner: "conformance harness",
        receipt: "cargo test -p clio-mcp --lib a_deliberately_wrong_result_shape_fails_the_http_checkers",
    },
    Scenario {
        id: "harness/tampered-listing-walk",
        owner: "conformance harness",
        receipt: "cargo test -p clio-mcp --lib a_tampered_listing_walk_fails_the_checker",
    },
];

#[test]
fn every_scenario_records_one_owner_and_one_bounded_check() {
    let mut ids: Vec<&str> = Vec::new();
    for scenario in SCENARIOS {
        let filter = scenario
            .receipt
            .strip_prefix("cargo test -p clio-mcp --lib ")
            .unwrap_or_else(|| panic!("{}: the receipt runs the crate's lib tests", scenario.id));
        assert!(
            !scenario.id.is_empty()
                && !scenario.owner.is_empty()
                && !filter.is_empty()
                && !filter.contains(' '),
            "{}: an id, an owner, and one bounded receipt are required",
            scenario.id
        );
        assert!(
            !ids.contains(&scenario.id),
            "{}: the scenario id is unique",
            scenario.id
        );
        ids.push(scenario.id);
    }
    assert_eq!(ids.len(), 20, "every covered scenario is registered");
}

#[test]
fn the_candidate_fixture_pin_is_a_future_expectation() {
    assert_eq!(
        CANDIDATE_FIXTURE_REVISION, CANDIDATE_REVISION,
        "the fixture pin follows the candidate dispatcher"
    );
    assert_eq!(
        crate::MCP_PROTOCOL_REVISION,
        "2025-11-25",
        "the live pin does not move"
    );
    assert_ne!(
        CANDIDATE_FIXTURE_REVISION,
        crate::MCP_PROTOCOL_REVISION,
        "the candidate pin is not a second live protocol setting"
    );
}

#[test]
fn http_required_metadata_refusal() {
    let port = spawn_candidate();
    let discover = request(
        "server/discover",
        &required_meta(CANDIDATE_FIXTURE_REVISION),
    );
    let without_version_header = post(port, &discover, "Mcp-Method: server/discover\r\n");
    assert_eq!(without_version_header.status, 400);
    let answer = body(&without_version_header);
    check_error_envelope(&answer, 7).expect("the missing header is refused");
    assert_eq!(answer["error"]["code"], json!(-32020), "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .expect("an error message")
            .contains("MCP-Protocol-Version"),
        "{answer}"
    );

    let cases = [
        (
            "absent _meta",
            json!({ "jsonrpc": "2.0", "id": 7, "method": "server/discover", "params": {} }),
            "params._meta",
        ),
        (
            "absent protocolVersion",
            json!({
                "jsonrpc": "2.0", "id": 7, "method": "server/discover",
                "params": { "_meta": { "io.modelcontextprotocol/clientCapabilities": {} } }
            }),
            "protocolVersion",
        ),
        (
            "absent clientCapabilities",
            json!({
                "jsonrpc": "2.0", "id": 7, "method": "server/discover",
                "params": {
                    "_meta": { "io.modelcontextprotocol/protocolVersion": CANDIDATE_FIXTURE_REVISION }
                }
            }),
            "clientCapabilities",
        ),
    ];
    for (label, frame, names) in cases {
        let response = post(port, &frame, &headers("server/discover"));
        assert_eq!(response.status, 400, "{label}");
        let answer = body(&response);
        check_error_envelope(&answer, 7).unwrap_or_else(|error| panic!("{label}: {error}"));
        assert_eq!(answer["error"]["code"], json!(-32602), "{label}: {answer}");
        assert!(
            answer["error"]["message"]
                .as_str()
                .unwrap_or_else(|| panic!("{label}: an error message"))
                .contains(names),
            "{label}: {answer}"
        );
    }
}

#[test]
fn http_optional_metadata_accepted() {
    let port = spawn_candidate();
    let required = post(
        port,
        &request(
            "server/discover",
            &required_meta(CANDIDATE_FIXTURE_REVISION),
        ),
        &headers("server/discover"),
    );
    let full = post(
        port,
        &request("server/discover", &full_meta(CANDIDATE_FIXTURE_REVISION)),
        &headers("server/discover"),
    );
    assert_eq!(required.status, 200);
    assert_eq!(full.status, 200);
    check_success_envelope(&body(&required), 7).expect("the required-only request is served");
    check_success_envelope(&body(&full), 7).expect("the full-metadata request is served");
    assert_eq!(
        body(&required)["result"],
        body(&full)["result"],
        "optional metadata never changes the result"
    );
}

#[test]
fn http_discovery_identity_lives_in_result_meta() {
    let port = spawn_candidate();
    let response = post(
        port,
        &request(
            "server/discover",
            &required_meta(CANDIDATE_FIXTURE_REVISION),
        ),
        &headers("server/discover"),
    );
    assert_eq!(response.status, 200);
    let answer = body(&response);
    check_success_envelope(&answer, 7).expect("discovery is served");
    let result = &answer["result"];
    check_complete_result(result).expect("discovery is a complete result");
    check_discovery_identity(result).expect("discovery carries its identity in result metadata");
    assert_eq!(
        result["supportedVersions"],
        json!([CANDIDATE_FIXTURE_REVISION]),
        "discovery answers the candidate revision only: {result}"
    );
}

#[test]
fn http_success_envelope_shape() {
    let port = spawn_candidate();
    let response = post(
        port,
        &request("tools/list", &required_meta(CANDIDATE_FIXTURE_REVISION)),
        &headers("tools/list"),
    );
    assert_eq!(response.status, 200);
    let answer = body(&response);
    check_success_envelope(&answer, 7).expect("a served request is a success envelope");
    assert_eq!(answer["result"]["resultType"], json!("complete"));
    assert!(answer.get("error").is_none(), "{answer}");
}

#[test]
fn http_error_envelope_shape() {
    let port = spawn_candidate();
    let response = post(
        port,
        &request("resources/list", &required_meta(CANDIDATE_FIXTURE_REVISION)),
        &headers("resources/list"),
    );
    assert_eq!(response.status, 404);
    let answer = body(&response);
    check_error_envelope(&answer, 7).expect("an unserved request is an error envelope");
    assert_eq!(answer["error"]["code"], json!(-32601), "{answer}");
}

/// The domain read pair served by the real HTTP adapter: the transport's success
/// envelope around the payloads the shared read checkers accept.
#[test]
fn http_domain_read_payloads() {
    let port = spawn_domain_candidate();
    let meta = required_meta(CANDIDATE_FIXTURE_REVISION);

    let gist = post(
        port,
        &read_call(&meta, "get_gist", json!({ "item_id": DOMAIN_ITEM_ID })),
        &name_headers("tools/call", "get_gist"),
    );
    assert_eq!(gist.status, 200, "the candidate serves a domain read");
    let answer = body(&gist);
    check_success_envelope(&answer, 7).expect("the candidate serves a domain read");
    check_gist_read(&answer["result"]).expect("the read carries the stored gist");

    let listing = post(
        port,
        &read_call(&meta, "inspect", json!({})),
        &name_headers("tools/call", "inspect"),
    );
    assert_eq!(
        listing.status, 200,
        "the candidate serves the domain listing"
    );
    let answer = body(&listing);
    check_success_envelope(&answer, 7).expect("the candidate serves the domain listing");
    check_domain_listing(&answer["result"]).expect("the listing carries the stored item");
}

#[test]
fn a_deliberately_wrong_result_shape_fails_the_http_checkers() {
    let partial = json!({ "resultType": "partial" });
    assert!(
        check_complete_result(&partial).is_err(),
        "a result that is not complete fails the HTTP check"
    );

    let missing_identity = json!({ "resultType": "complete", "_meta": {} });
    assert!(
        check_discovery_identity(&missing_identity).is_err(),
        "a result without identity fails the HTTP check"
    );

    let top_level_identity = json!({
        "resultType": "complete",
        "serverInfo": { "name": "wrong", "version": "0" }
    });
    assert!(
        check_discovery_identity(&top_level_identity).is_err(),
        "a top-level identity fails the HTTP check"
    );

    let ambiguous = json!({
        "jsonrpc": "2.0", "id": 7,
        "result": {},
        "error": { "code": -32601, "message": "refused" }
    });
    assert!(
        check_success_envelope(&ambiguous, 7).is_err(),
        "an envelope carrying an error is no success"
    );
    assert!(
        check_error_envelope(&ambiguous, 7).is_err(),
        "an envelope carrying a result is no error"
    );
}
