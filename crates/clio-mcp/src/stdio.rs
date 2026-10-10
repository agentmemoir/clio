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
use std::io::BufRead;

/// Serve MCP over newline-delimited JSON-RPC on `input`/`output`.
///
/// Every response is one compact JSON line; any log output goes to stderr so
/// stdout stays a pure message channel (stdout purity).
///
/// # Errors
/// Propagates I/O failures from the reader or writer.
pub fn serve<I, O>(handler: &McpHandler, mut input: I, output: &mut O) -> std::io::Result<()>
where
    I: BufRead,
    O: std::io::Write,
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
        if let Some(response) = handler.handle_frame(frame) {
            // Compact single-line output: no embedded newlines.
            writeln!(output, "{response}")?;
            output.flush()?;
        }
    }
}

#[cfg(test)]
#[path = "stdio_tests.rs"]
mod stdio_tests;
