// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for holding the published `tool_invoke` examples
//! to the answers the handler gives: every declared valid, invalid, and refused
//! entry is driven through `tools/call`, one case per entry, and compared with
//! the outcome that entry declares.
//!
//! ## Owns
//! - `the_published_examples_answer_as_the_handler_does`
//!
//! ## Does not own
//! - What a target tool returns (see each tool's own test module)
//! - The wrapper's own refusals and accounting (see `catalog_invoke_tests`)
//! - The schema-level reading of the same examples (see
//!   `discovery_tool_defs_tests`)
//!
//! ## Boundary
//!
//! Changes to a published example, or to what the handler answers for one,
//! should normally require changes here. Changes to a business tool's payload
//! should not: a valid example is compared with the direct call, not pinned.
//!
//! The test opens its own state over the in-memory backend, and every example it
//! drives is refused before a tool body runs, so the module opens no path, holds
//! no clock, and depends on no environment.

use super::TOOL_INVOKE;
use super::tests::{call, handler, payload, refusal_of};
use serde_json::{Value, json};

/// The `examples` block of the published `tool_invoke` declaration.
fn published_examples() -> &'static Value {
    let tools = crate::discovery_tool_defs::definitions()["tools"]
        .as_array()
        .expect("the document declares a tools array");
    let invoke = tools
        .iter()
        .find(|tool| tool["name"] == json!(TOOL_INVOKE))
        .expect("the document declares the wrapper");
    &invoke["examples"]
}

#[test]
fn the_published_examples_answer_as_the_handler_does() {
    let (handler, _state) = handler();
    let examples = published_examples();

    // A valid example is valid wrapper input, so the call reaches the target and
    // comes back as the target's own answer.
    for example in examples["valid"].as_array().expect("valid examples") {
        let target = example["name"].as_str().expect("a target name");
        let args = example
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let invoked = payload(&call(&handler, TOOL_INVOKE, example));
        let direct = payload(&call(&handler, target, &args));
        assert_eq!(
            invoked["code"], direct["code"],
            "a valid example answers with the target's own code: {example}"
        );
        // The one difference the wrapper introduces is the target's name, which
        // it adds to a refusal of the target's own arguments.
        let message = invoked["message"].as_str().unwrap_or_default();
        let direct_message = direct["message"].as_str().unwrap_or_default();
        assert!(
            message == direct_message || message == format!("{target}: {direct_message}"),
            "a valid example answers with the target's own message: {invoked} \
             against the direct call's {direct}"
        );
    }

    // An invalid example is a fault inside a `tools/call` result, never a
    // JSON-RPC error, because that is what a direct call with bad arguments
    // answers too.
    for example in examples["invalid"].as_array().expect("invalid examples") {
        let input = &example["input"];
        let response = call(&handler, TOOL_INVOKE, input);
        assert!(
            response.get("error").is_none(),
            "an invalid argument answers a tool result, not a JSON-RPC error: {response}"
        );
        let (code, message) = refusal_of(&response);
        assert_eq!(
            Value::String(code),
            example["code"],
            "the declared code is the one the handler answers for {input}"
        );
        assert!(
            message.contains(TOOL_INVOKE),
            "the refusal names the wrapper: {message}"
        );
    }

    // A refused example names a target the wrapper will not hand to dispatch.
    for example in examples["refused"].as_array().expect("refused examples") {
        let input = &example["arguments"];
        let target = input["name"].as_str().expect("a refused target name");
        let response = call(&handler, TOOL_INVOKE, input);
        let (code, message) = refusal_of(&response);
        assert_eq!(code, "invalid_argument", "{example}");
        assert!(
            message.contains(target),
            "the refusal names the refused target `{target}`: {message}"
        );
    }
}
