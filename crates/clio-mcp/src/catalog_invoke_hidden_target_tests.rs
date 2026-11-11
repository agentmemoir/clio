// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the tests that hold `tool_invoke` to the
//! profile-narrowed view `tools/list` advertises: a target the active profile
//! hides is refused exactly as a direct call to it is, and that refusal cannot be
//! told from the one a name nothing binds gets.
//!
//! ## Owns
//! - `a_hidden_target_is_refused_by_the_wrapper_as_an_unknown_tool`
//! - `a_hidden_target_and_an_unbound_one_are_refused_the_same_way`
//!
//! ## Does not own
//! - The direct route's own gate (see `protocol_dispatch_gate_tests`)
//! - Which tools a profile lists (see `tool_profile_tests`)
//! - The wrapper's argument checking and its discovery refusals (see
//!   `catalog_invoke_tests`)
//!
//! ## Boundary
//!
//! Changes to what the wrapper refuses for a name the active profile hides should
//! normally require changes here. Every test opens its own in-memory state and
//! reads nothing from the environment.

use super::TOOL_INVOKE;
use super::tests::call;
use crate::protocol::{ERR_INVALID_PARAMS, McpHandler};
use crate::runtime::{McpOpenOptions, McpState};
use serde_json::{Value, json};
use std::sync::Arc;

/// A bound tool behind a confirmation gate, which the `minimal` profile does not
/// advertise.
const HIDDEN: &str = "discard";

/// A handler whose profile hides [`HIDDEN`] and whose discovery switch keeps the
/// wrapper reachable, so the wrapper is the surface under test.
fn handler() -> (McpHandler, Arc<McpState>) {
    let state = McpState::open_with_effective(
        &McpOpenOptions::default(),
        &json!({ "mcp": { "discovery": true, "tool_profile": "minimal" } }),
    )
    .expect("a state over the in-memory backend");
    (McpHandler::new(Arc::clone(&state)), state)
}

/// The message that names one unknown tool.
fn unknown(name: &str) -> Value {
    json!(format!("unknown tool `{name}`"))
}

#[test]
fn a_hidden_target_is_refused_by_the_wrapper_as_an_unknown_tool() {
    let (handler, state) = handler();
    // The first arguments would run the tool's body, the second would produce its
    // confirmation prompt; neither may be reached through either route.
    for args in [
        json!({ "item_id": "itm-hidden-target", "confirm": true }),
        json!({ "item_id": "itm-hidden-target" }),
    ] {
        let direct = call(&handler, HIDDEN, &args);
        let invoked = call(
            &handler,
            TOOL_INVOKE,
            &json!({ "name": HIDDEN, "arguments": args }),
        );
        for response in [&direct, &invoked] {
            assert_eq!(
                response["error"]["code"],
                json!(ERR_INVALID_PARAMS),
                "{response}"
            );
            assert_eq!(response["error"]["message"], unknown(HIDDEN), "{response}");
            assert!(
                response.get("result").is_none(),
                "a refusal carries no result: {response}"
            );
        }
        assert_eq!(
            invoked, direct,
            "the wrapper answers what the direct call answers"
        );
    }
    assert!(
        state.calls.snapshot().is_empty(),
        "neither a hidden confirmation nor a hidden body ran: {:?}",
        state.calls.snapshot()
    );
}

#[test]
fn a_hidden_target_and_an_unbound_one_are_refused_the_same_way() {
    let (handler, _state) = handler();
    let hidden = call(&handler, TOOL_INVOKE, &json!({ "name": HIDDEN }));
    let unbound = call(&handler, TOOL_INVOKE, &json!({ "name": "no_such_tool" }));

    // One code, one envelope, one message shape: the only difference is the name
    // the caller supplied, so a hidden tool cannot be read as an existing one.
    for response in [&hidden, &unbound] {
        assert_eq!(
            response["error"]["code"],
            json!(ERR_INVALID_PARAMS),
            "{response}"
        );
        assert_eq!(
            response.as_object().map(serde_json::Map::len),
            Some(3),
            "the refusal is the plain JSON-RPC envelope: {response}"
        );
        assert!(
            response["error"].get("data").is_none(),
            "the refusal carries nothing that tells the two apart: {response}"
        );
    }
    assert_eq!(hidden["error"]["message"], unknown(HIDDEN));
    assert_eq!(unbound["error"]["message"], unknown("no_such_tool"));
    assert_eq!(
        hidden["error"]["message"].as_str().unwrap_or_default().replace(HIDDEN, "<name>"),
        unbound["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .replace("no_such_tool", "<name>"),
        "the two refusals read alike apart from the name"
    );
}
