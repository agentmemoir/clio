// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for asserting the pinned "2025-11-25" MCP
//! protocol revision in the schema pack, the initialize response, and the
//! effective config diagnostics.
//!
//! ## Owns
//! - `revision_pin_is_asserted_everywhere`, the single revision contract test.
//!
//! ## Does not own
//! - The shared state and handler builders (`main`).
//! - Transport framing and parity assertions (`stdio`, `http`).
//!
//! ## Boundary
//!
//! Changes to the pinned revision should normally require changes here.
//! Changes to a transport's assertions should not require changes here.
//!
//! Keep this module focused on the pinned-revision contract.

use clio_mcp::MCP_PROTOCOL_REVISION;
use serde_json::json;

use crate::handler;

/// Item 7: the pinned revision is asserted in the pack, initialize, and
/// the config constant.
#[test]
fn revision_pin_is_asserted_everywhere() {
    assert_eq!(MCP_PROTOCOL_REVISION, "2025-11-25");
    let pack = clio_mcp::schema::schema_pack();
    assert_eq!(pack["mcp_protocol_revision"], json!("2025-11-25"));
    let h = handler();
    let init = h
        .handle_message(&json!({
            "jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":MCP_PROTOCOL_REVISION,"capabilities":{}}
        }))
        .expect("init response");
    assert_eq!(
        init["result"]["protocolVersion"],
        json!(MCP_PROTOCOL_REVISION)
    );
    // Effective config diagnostics report the same pinned revision.
    let runtime = clio_config::Runtime::with_env(&|_| None);
    let effective = runtime.effective_masked();
    assert_eq!(effective["mcp"]["revision"], json!(MCP_PROTOCOL_REVISION));
}
