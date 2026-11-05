// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the recorded `tools/call` session guard: one
//! real stdio transcript pins the fixed result envelope, and a stored legacy
//! transcript proves the guard can still tell the old envelope apart.
//!
//! ## Owns
//! - The byte-for-byte replay of the recorded session through `stdio::serve`
//! - The decoded payload-copy count that separates one copy from two
//!
//! ## Does not own
//! - Envelope construction (see `protocol`)
//! - Transport framing (see `stdio`)
//!
//! ## Boundary
//!
//! Changes to the fixed `tools/call` result envelope should normally require
//! changes here. Changes to tool dispatch should not require changes here.
//!
//! Keep this module focused on the recorded session guard.

use clio_mcp::protocol::McpHandler;
use clio_mcp::runtime::{McpOpenOptions, McpState};
use clio_mcp::stdio::serve;
use serde_json::{Value, json};
use std::io::Cursor;

const SESSION: &str = include_str!("../fixtures/tools_call_session.json");
const SESSION_BEFORE: &str = include_str!("../fixtures/tools_call_session_before.json");

const REGENERATE: &str = "regenerate with `cargo build -p clio --bin clio` and `python3 \
benchmarks/mcp_token_budget/capture_transcript.py --binary target/debug/clio --out \
crates/clio-mcp/tests/fixtures/tools_call_session.json`; write the legacy fixture by running \
the same capture from a checkout of the parent commit to \
`crates/clio-mcp/tests/fixtures/tools_call_session_before.json`";

const BASE_COMMIT: &str = "ecab87064255b9312232b5cda61ee52e073319c8";

const REGENERATE_BASE: &str = "regenerate with `git archive ecab87064255b9312232b5cda61ee52e073319c8 \
| tar -x -C <scratch>`, `git init <scratch>`, `git -C <scratch> symbolic-ref HEAD \
refs/heads/provenance`, write that commit to `<scratch>/.git/refs/heads/provenance`, \
`touch` every `*.rs` under `<scratch>`, `CARGO_TARGET_DIR=$PWD/target cargo build -p clio --bin \
clio` from `<scratch>`, then `python3 benchmarks/mcp_token_budget/capture_transcript.py \
--source-dir <scratch> --binary target/debug/clio --out \
crates/clio-mcp/tests/fixtures/tools_call_session_before.json`; regenerate the current fixture \
with `touch` on every `*.rs` under `crates/`, the same build from the working tree, and the same \
capture without `--source-dir` to `crates/clio-mcp/tests/fixtures/tools_call_session.json`";

/// The two request frames the recorded session holds, rebuilt in code.
fn request_frames() -> Vec<Value> {
    vec![
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "clio-mcp-transcript", "version": "1"},
            },
        }),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "ranking_env_get", "arguments": {}},
        }),
    ]
}

/// One compact request line per frame, the way the capture wrote them.
fn session_input(frames: &[Value]) -> String {
    let mut input = String::new();
    for frame in frames {
        input.push_str(&frame.to_string());
        input.push('\n');
    }
    input
}

/// Serve one transport session and return its response lines.
fn responses(input: &str) -> Vec<String> {
    let state = McpState::open_with_effective(&McpOpenOptions::default(), &json!({}))
        .expect("open the in-memory state");
    let handler = McpHandler::new(state);
    let mut out = Vec::new();
    serve(&handler, Cursor::new(input.as_bytes().to_vec()), &mut out).expect("serve the session");
    String::from_utf8(out)
        .expect("responses are UTF-8")
        .lines()
        .map(str::to_owned)
        .collect()
}

/// One recorded exchange field: the request or the response line as stored.
fn recorded(fixture: &str, index: usize, field: &str) -> String {
    let parsed: Value = serde_json::from_str(fixture)
        .unwrap_or_else(|err| panic!("fixture is not JSON ({err}): {REGENERATE}"));
    parsed["exchanges"][index][field]
        .as_str()
        .unwrap_or_else(|| panic!("exchange {index} has no `{field}` string: {REGENERATE}"))
        .to_owned()
}

/// Count the decoded copies of the payload in one `tools/call` response line.
///
/// A present `structuredContent` is one copy. A `content[0].text` that parses
/// back to the same value is a second copy, which is what the old envelope
/// produced. The fixed envelope leaves one copy.
fn payload_copies(line: &str) -> (usize, String) {
    let response: Value = serde_json::from_str(line)
        .unwrap_or_else(|err| panic!("response is not JSON ({err}): {REGENERATE}"));
    let result = response
        .get("result")
        .unwrap_or_else(|| panic!("response has no result: {REGENERATE}"));
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("response has no content array: {REGENERATE}"));
    assert_eq!(content.len(), 1, "expected one content block: {REGENERATE}");
    let text = content[0]
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("content block has no text: {REGENERATE}"));
    let structured = result.get("structuredContent");
    let mut copies = usize::from(structured.is_some());
    if let Some(structured) = structured
        && serde_json::from_str::<Value>(text).ok().as_ref() == Some(structured)
    {
        copies += 1;
    }
    (copies, text.to_owned())
}

#[test]
fn recorded_session_replays_the_fixed_envelope() {
    let frames = request_frames();
    for (index, frame) in frames.iter().enumerate() {
        let recorded_request: Value = serde_json::from_str(&recorded(SESSION, index, "request"))
            .unwrap_or_else(|err| panic!("recorded request is not JSON ({err}): {REGENERATE}"));
        assert_eq!(
            *frame, recorded_request,
            "rebuilt frame {index} drifted from the recorded session: {REGENERATE}"
        );
    }
    let lines = responses(&session_input(&frames));
    assert_eq!(
        lines.len(),
        frames.len(),
        "session produced {} response lines: {REGENERATE}",
        lines.len()
    );
    assert_eq!(
        lines[1],
        recorded(SESSION, 1, "response"),
        "tools/call response drifted from the recorded session: {REGENERATE}"
    );
    let (copies, text) = payload_copies(&lines[1]);
    assert_eq!(
        copies, 1,
        "the payload is carried twice in one response: {REGENERATE}"
    );
    assert_eq!(
        text, "ok",
        "a schema-declaring success must report a short status line: {REGENERATE}"
    );
}

#[test]
fn legacy_session_holds_a_duplicated_payload() {
    let (copies, _text) = payload_copies(&recorded(SESSION_BEFORE, 1, "response"));
    assert_eq!(
        copies, 2,
        "the stored legacy response no longer shows two payload copies, so this guard proves \
         nothing: {REGENERATE}"
    );
}

#[test]
fn legacy_base_result_is_the_current_payload_copied_twice() {
    let after: Value = serde_json::from_str(&recorded(SESSION, 1, "response"))
        .unwrap_or_else(|err| panic!("current fixture is not JSON ({err}): {REGENERATE_BASE}"));
    let before: Value = serde_json::from_str(&recorded(SESSION_BEFORE, 1, "response"))
        .unwrap_or_else(|err| panic!("base fixture is not JSON ({err}): {REGENERATE_BASE}"));
    let payload = after
        .pointer("/result/structuredContent")
        .unwrap_or_else(|| panic!("current response has no structuredContent: {REGENERATE_BASE}"))
        .clone();
    let text = payload.to_string();
    let rebuilt = json!({
        "content": [{"type": "text", "text": text}],
        "isError": false,
        "structuredContent": payload,
    });
    assert_eq!(
        rebuilt, before["result"],
        "the base result is not the current payload copied into one text block: {REGENERATE_BASE}"
    );
}

#[test]
fn fixtures_record_the_source_commit_and_binary_digest() {
    let base: Value = serde_json::from_str(SESSION_BEFORE)
        .unwrap_or_else(|err| panic!("base fixture is not JSON ({err}): {REGENERATE_BASE}"));
    let current: Value = serde_json::from_str(SESSION)
        .unwrap_or_else(|err| panic!("current fixture is not JSON ({err}): {REGENERATE_BASE}"));
    assert_eq!(
        base.pointer("/capture/source_commit")
            .and_then(Value::as_str),
        Some(BASE_COMMIT),
        "the base fixture does not name the commit it was captured from: {REGENERATE_BASE}"
    );
    for (name, fixture) in [("base", &base), ("current", &current)] {
        let digest = fixture
            .pointer("/capture/binary_sha256")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{name} fixture has no binary digest: {REGENERATE_BASE}"));
        assert!(
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{name} fixture binary digest is not 64 hex characters: {REGENERATE_BASE}"
        );
    }
}

#[test]
fn unknown_tool_is_an_invalid_params_error() {
    let frame = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {"name": "no_such_tool_here", "arguments": {}},
    });
    let lines = responses(&session_input(&[frame]));
    assert_eq!(lines.len(), 1, "expected one error line: {REGENERATE}");
    let response: Value = serde_json::from_str(&lines[0])
        .unwrap_or_else(|err| panic!("error line is not JSON ({err}): {REGENERATE}"));
    assert_eq!(
        response.pointer("/error/code").and_then(Value::as_i64),
        Some(-32602),
        "an unknown tool must be an invalid-params error: {REGENERATE}"
    );
    assert!(
        response.get("result").is_none(),
        "an unknown tool must not return a result envelope: {REGENERATE}"
    );
}
