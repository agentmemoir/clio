// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is the `clio-sync` crate root: the multi-host sync protocol
//! — wire contract, durable journal plumbing, idempotent
//! apply engine, authenticated server, and client push/pull/status.
//!
//! ## Owns
//! - Sync wire types, opaque cursors, and the default LWW rule (`protocol`)
//! - Optional client-side sync encryption (`crypto`)
//! - Idempotent apply matrix, conflict journal, dead-letter policy (`apply`)
//! - HTTP sync server with auth and bank authorization (`server`)
//! - Client push / pull / bidirectional modes and masked status (`client`)
//!
//! ## Does not own
//! - Sync persistence SQL (`clio-store` `SyncStore` owns the durable state)
//! - Admission scoring or gating (see `clio-admission`)
//! - Subject DEKs / KMS (see `clio-store` `dek`)
//! - MCP tool binding (see `clio-mcp`)
//!
//! ## Boundary
//!
//! Changes to sync protocol semantics should normally require changes here.
//! Changes to single-host behavior should not require changes here — the
//! crate is additive and may be omitted entirely on single-host profiles.
//!
//! Keep this crate focused on the multi-host sync protocol.
//!
//! ## Single-host omission
//!
//! Deployments that are single-host MAY omit this crate and its tools
//! entirely. If omitted, the deployment docs must state the omission plainly;
//! nothing on the single-host path depends on sync. The coding-agent profile
//! gates the five sync tools (`sync_serve`, `sync_push`, `sync_pull`,
//! `sync_status`, `sync_ack_skip`) behind the profile's sync omission flag.
//!
//! ## Serve exposure notes (operator hygiene)
//!
//! - `sync_serve` prefers ephemeral high ports (`bind = "127.0.0.1:0"`);
//!   do not default to a well-known privileged port.
//! - Opening the bind may require an explicit firewall / OS permission —
//!   this is an operator decision, never automatic.
//! - Interactive serve defaults to a 30-minute auto-stop TTL; opt into a
//!   long-running service unit explicitly via `ttl_secs`.
//! - Pushing does NOT require a local `sync_serve` on the pushing host:
//!   `sync_push` talks directly to the remote endpoint.
//! - Outside dev mode the server refuses to bind without auth material,
//!   and every request must present a valid bearer token.
//! - `sync_status` and logs never carry tokens or DEK material (masked).

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::module_name_repetitions)]

pub mod apply;
mod apply_history;
mod apply_records;
mod bind_warning;
pub mod client;
mod client_feed;
pub mod clock;
mod conn_pool;
mod credential;
pub mod crypto;
mod handle_resolution;
pub(crate) mod http;
mod journal_row;
pub(crate) mod keyed_roles;
pub mod protocol;
pub mod server;
mod server_conn;

#[cfg(test)]
mod client_feed_package_fixtures;
#[cfg(test)]
mod client_wire_fixtures;
#[cfg(test)]
mod credential_fixtures;
#[cfg(test)]
mod field_documentation_test_support;
#[cfg(test)]
#[path = "lib_tests.rs"]
mod lib_tests;

/// Re-exports of the surfaces consumers (MCP bindings, CLI) need.
pub mod prelude {
    pub use crate::apply::{ApplyEngine, ApplyOutcome, apply_mutation};
    pub use crate::client::{SyncClientConfig, ack_skip, bidirectional, pull, push, sync_status};
    pub use crate::crypto::{SyncCipher, SyncCipherKey};
    pub use crate::protocol::{
        BankSyncStatus, PullRequest, PullResponse, PushAck, SyncContent, SyncMutation, SyncPayload,
        SyncStatusReport, decode_cursor, encode_cursor, lww_compare, reason_code_for,
    };
    pub use crate::server::{SyncServer, SyncServerOptions, sync_serve};
}

/// Default page size for pull batches (never unbounded result sets).
pub const PULL_PAGE_LIMIT: usize = 200;

/// Default auto-stop TTL for interactive `sync_serve`.
pub const SERVE_TTL_DEFAULT_SECS: u64 = 30 * 60;
