// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the stdio MCP transport: newline-delimited
//! JSON-RPC over stdin/stdout with stderr-only logging.
//!
//! ## Owns
//! - Line framing, request loop, and stdout purity (no logs on stdout)
//!
//! ## Does not own
//! - Protocol handling (see `protocol`)
//! - Tool semantics (see `write_tools`/`mutator_tools`)
//!
//! ## Boundary
//!
//! Changes to stdio framing should normally require changes here.
//! Changes to protocol behavior should not require changes here.
//!
//! Keep this module focused on the stdio transport.

use crate::protocol::McpHandler;
#[cfg(test)]
use crate::runtime::McpState;
use serde_json::Value;
use std::io::BufRead;

/// Run the newline frame loop, asking `answer` for each non-blank frame.
///
/// A frame the answer leaves unanswered writes no line. Both the active and
/// the candidate entry points share this one framing routine.
fn serve_frames<I, O, F>(mut input: I, output: &mut O, mut answer: F) -> std::io::Result<()>
where
    I: BufRead,
    O: std::io::Write,
    F: FnMut(&str) -> Option<Value>,
{
    let mut line = String::new();
    loop {
        line.clear();
        let read = input.read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        let frame = line.trim();
        if frame.is_empty() {
            continue;
        }
        if let Some(response) = answer(frame) {
            // Compact single-line output: no embedded newlines.
            writeln!(output, "{response}")?;
            output.flush()?;
        }
    }
}

/// Serve MCP over newline-delimited JSON-RPC on `input`/`output`.
///
/// Every response is one compact JSON line; any log output goes to stderr so
/// stdout stays a pure message channel (stdout purity).
///
/// # Errors
/// Propagates I/O failures from the reader or writer.
pub fn serve<I, O>(handler: &McpHandler, input: I, output: &mut O) -> std::io::Result<()>
where
    I: BufRead,
    O: std::io::Write,
{
    serve_frames(input, output, |frame| handler.handle_frame(frame))
}

/// Serve candidate-revision frames over the real stdio framing.
///
/// The stdio transport carries no HTTP headers, so this seam parses each frame
/// and runs the candidate dispatcher's metadata, revision, capability, and
/// removed-method rules. No header is read or required here.
///
/// # Errors
/// Propagates I/O failures from the reader or writer.
#[cfg(test)]
pub(crate) fn serve_candidate<I, O>(
    state: &McpState,
    input: I,
    output: &mut O,
) -> std::io::Result<()>
where
    I: BufRead,
    O: std::io::Write,
{
    serve_frames(input, output, |frame| {
        crate::stateless::admit_stdio(state, frame)
    })
}

#[cfg(test)]
#[path = "stdio_tests.rs"]
mod stdio_tests;

#[cfg(test)]
#[path = "stdio_candidate_tests.rs"]
mod stdio_candidate_tests;
