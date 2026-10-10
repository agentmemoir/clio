// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the `clio-mcp` crate root: machine-readable `tool_schema` pack,
//! shared tool dispatcher, and MCP transport bindings (stdio + Streamable HTTP)
//! for the pinned protocol revision.
//!
//! ## Owns
//! - Tool schema pack generation and export
//! - JSON-RPC message handling shared by both transports
//! - Stdio and Streamable HTTP transports
//! - Write/mutate and read/retrieve/compose tool registration and dispatch
//!
//! ## Does not own
//! - In-process tool semantics (domain crates own those)
//! - Gate logic; gates run inside the in-process tools
//! - Retrieval ranking or admission math (domain crates own those)
//!
//! ## Boundary
//!
//! Changes to MCP binding, schema publication, or transport behavior should
//! normally require changes here. Changes to admission, storage, or write-path
//! semantics should not require changes here.
//!
//! Keep this module focused on the MCP binding layer.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::module_name_repetitions)]

pub mod additive_tools;
mod arg_contract;
pub mod batch_preflight;
pub mod batch_preflight_records;
pub mod batch_tools;
pub mod config_tools;
mod conn_pool;
pub mod http;
pub mod http_auth;
mod http_guard;
mod http_read;
pub mod hygiene_tools;
pub mod index_drain;
pub mod maintenance_trigger;
pub mod mutator_record_tools;
pub mod mutator_tools;
mod ops_embedder;
pub mod ops_tools;
pub mod portability_tools;
mod pressure;
mod principal;

/// The pressure block's archive-scan constants, re-exported so external
/// consumers (the measurement target) time the exact values the block runs.
pub use pressure::{
    ARCHIVE_BAND_MIN_SCORE, ARCHIVE_PAGE_SIZE, ARCHIVE_SCAN_BUDGET, DISCARD_BAND_MIN_SCORE,
};
pub mod protocol;
pub mod provider_adapter;
pub mod read_explain;
mod read_history;
mod read_read_policy;
mod read_retrieve;
pub mod read_tools;
pub mod retention_tools;
pub mod runtime;
mod runtime_context;
mod runtime_index;
mod runtime_open;
mod runtime_ranking;
mod runtime_retention;
mod runtime_sinks;
pub mod schema;
mod schema_additive_defs;
mod schema_config_defs;
mod schema_defs;
mod schema_hygiene_defs;
mod schema_ops_defs;
mod schema_portability_defs;
mod schema_read_defs;
mod schema_read_props;
mod schema_record_defs;
mod schema_retention_defs;
mod schema_sync_defs;
pub mod stdio;
pub mod store_write;
mod summarize_tools;
pub mod sync_tools;
pub mod write_tools;

/// Pinned MCP specification revision (item 3).
///
/// Transport and tool rules come from this single revision; later revisions
/// (e.g. sessionless `2026-07-28`) are intentionally NOT mixed in.
pub const MCP_PROTOCOL_REVISION: &str = "2025-11-25";

/// Tool names bound over MCP in this build (the write/mutate set).
///
/// The list itself lives in `clio-types` so the config layer, which cannot
/// depend on this crate, reports the same set this crate advertises.
#[must_use]
pub fn bound_write_tools() -> &'static [&'static str] {
    clio_types::mcp_tools::BOUND_WRITE_TOOLS
}

/// Read/retrieve/compose tools bound over MCP in this build (read rows).
#[must_use]
pub fn bound_read_tools() -> &'static [&'static str] {
    clio_types::mcp_tools::BOUND_READ_TOOLS
}

/// Every tool name bound over MCP in this build (write set + read surface).
#[must_use]
pub fn bound_tools() -> Vec<&'static str> {
    clio_types::mcp_tools::bound_tools()
}
