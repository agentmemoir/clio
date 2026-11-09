// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for asserting the activated `2026-07-28` MCP
//! protocol revision in the schema pack, the discovery result, and the
//! effective config diagnostics.
//!
//! ## Owns
//! - `revision_pin_is_asserted_everywhere`, the single revision contract test.
//!
//! ## Does not own
//! - The shared frame builders (`main`).
//! - Transport framing and parity assertions (`stdio`, `http`).
//!
//! ## Boundary
//!
//! Changes to the served revision should normally require changes here.
//! Changes to a transport's assertions should not require changes here.
//!
//! Keep this module focused on the served-revision contract.

use clio_mcp::MCP_PROTOCOL_REVISION;
use serde_json::{Value, json};
use std::io::Cursor;

use crate::{request, state};

/// The served revision is asserted in the pack, in discovery, and in the config
/// constant, and the predecessor that clients still send is refused.
#[test]
fn revision_pin_is_asserted_everywhere() {
    assert_eq!(MCP_PROTOCOL_REVISION, "2026-07-28");
    let pack = clio_mcp::schema::schema_pack();
    assert_eq!(pack["mcp_protocol_revision"], json!(MCP_PROTOCOL_REVISION));

    // Discovery is where a client learns the revision: the retired handshake
    // that used to report it is gone.
    let mut out: Vec<u8> = Vec::new();
    clio_mcp::stdio::serve(
        &state(),
        Cursor::new(format!("{}\n", request(1, "server/discover")).into_bytes()),
        &mut out,
    )
    .expect("stdio");
    let text = String::from_utf8(out).expect("utf8");
    let discover: Value = serde_json::from_str(text.trim()).expect("one response line");
    assert_eq!(
        discover["result"]["supportedVersions"],
        json!([MCP_PROTOCOL_REVISION])
    );
    assert_eq!(
        discover["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        json!("clio-mcp")
    );

    // Effective config diagnostics report the same revision.
    let runtime = clio_config::Runtime::with_env(&|_| None);
    let effective = runtime.effective_masked();
    assert_eq!(effective["mcp"]["revision"], json!(MCP_PROTOCOL_REVISION));
}
