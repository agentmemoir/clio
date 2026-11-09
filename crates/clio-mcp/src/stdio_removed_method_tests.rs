// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the removed-method rule on the stdio
//! transport: a legacy handshake frame is answered as an unknown method
//! whether or not it carries the metadata the revision requires.
//!
//! ## Owns
//! - The bare removed-method frame and its method-not-found answer
//!
//! ## Does not own
//! - The rule itself and its place in the request order (see `stateless`)
//! - Framing and the serving loop (see `stdio`)
//! - The metadata-bearing scenarios (see `stdio_candidate_tests`)
//!
//! ## Boundary
//!
//! Changes to how stdio answers a removed method should normally require
//! changes here. Changes to the metadata rules or a tool should not.
//!
//! Keep this module focused on the removed-method rule over stdio.

use crate::stateless_fixtures::{check_error_envelope, state};
use serde_json::{Value, json};
use std::io::Cursor;

/// The one answer the serving loop writes for one frame.
fn answer_one(frame: &Value) -> Value {
    let mut output = Vec::new();
    super::serve(
        &state(),
        Cursor::new(format!("{frame}\n").into_bytes()),
        &mut output,
    )
    .expect("the stdio transport serves the frame");
    let text = String::from_utf8(output).expect("the transport writes UTF-8");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "a request answers one line: {frame}");
    serde_json::from_str(lines[0]).expect("one compact JSON answer per line")
}

#[test]
fn a_bare_removed_method_is_a_method_not_found_answer() {
    for method in ["initialize", "ping"] {
        let frame = json!({
            "jsonrpc": "2.0", "id": 1, "method": method,
            "params": { "protocolVersion": "2025-11-25" }
        });
        let answer = answer_one(&frame);
        check_error_envelope(&answer, 1).expect("a removed request is refused");
        assert_eq!(
            answer["error"]["code"],
            json!(-32601),
            "{method}: a removed method is not found: {answer}"
        );
        assert!(
            answer["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(method)),
            "{method}: the refusal cites it: {answer}"
        );
    }
}
