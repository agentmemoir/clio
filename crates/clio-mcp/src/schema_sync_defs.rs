// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for per-tool JSON Schema definitions for the
//! multi-host sync tools, published in the same `tool_schema` pack as
//! the Core catalog.
//!
//! ## Owns
//! - One `inputSchema` per sync tool: `sync_push`, `sync_pull`, `sync_status`,
//!   `sync_key_status`, `sync_serve`, `sync_ack_skip`
//!
//! ## Does not own
//! - Core or other additive tool definitions (`schema_defs` /
//!   `schema_additive_defs` / `schema_hygiene_defs`)
//! - Tool dispatch or handler semantics
//!
//! ## Boundary
//!
//! Changes to a sync tool's parameter contract should normally require changes
//! here. Changes to sync protocol behavior should not require changes here.
//!
//! Keep this module focused on sync schema definitions.

use crate::schema_defs::{ToolDef, arr, b, en, num, s};
use serde_json::{Value, json};

/// The `sync_key` argument: one shared secret or a newest-first array of them.
///
/// A single string keeps the older single-key calls working; an array of
/// strings carries a rotation, newest first. The shared schema helpers type a
/// property as one shape or the other and never the pair, so this lives beside
/// the sync definitions that accept the shape. The `sync_status` override is
/// the only user now: the transfer tools no longer declare the argument.
fn sync_key_prop(desc: &str) -> Value {
    json!({
        "description": desc,
        "oneOf": [
            {"type": "string"},
            {"type": "array", "items": {"type": "string"}}
        ]
    })
}

/// All sync tool definitions (four mutating/operator rows + two read rows).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn sync_defs() -> Vec<ToolDef> {
    let banks = arr(
        "Banks to sync (default: the connection default bank).",
        &json!({"type": "string"}),
    );
    vec![
        ToolDef {
            name: "sync_push",
            description: "Push local mutations for the given banks to the remote sync server since the durable push cursor; cursors advance only after durable accept.",
            required: &["remote"],
            props: vec![
                ("remote", s("Remote sync server endpoint `host:port`.")),
                (
                    "mode",
                    en(
                        "Operation mode; `bidirectional` pulls then pushes in one action.",
                        &["push", "pull", "bidirectional"],
                    ),
                ),
                ("banks", banks.clone()),
                (
                    "token",
                    s(
                        "Bearer token for the remote; echoed masked (last 4) in responses, never in the clear.",
                    ),
                ),
            ],
        },
        ToolDef {
            name: "sync_pull",
            description: "Pull remote mutations and apply them idempotently (LWW + conflict journal); the pull cursor freezes at the last durable position when a mutation is dead-lettered.",
            required: &["remote"],
            props: vec![
                ("remote", s("Remote sync server endpoint `host:port`.")),
                (
                    "mode",
                    en(
                        "Operation mode; `bidirectional` pulls then pushes in one action.",
                        &["push", "pull", "bidirectional"],
                    ),
                ),
                ("banks", banks.clone()),
                (
                    "token",
                    s(
                        "Bearer token for the remote; echoed masked (last 4) in responses, never in the clear.",
                    ),
                ),
            ],
        },
        ToolDef {
            name: "sync_status",
            description: "Masked local sync status: per-bank cursors, pending pushes, conflicts, open dead letters, endpoint, and auth/encryption state. Never returns tokens or DEK material.",
            required: &[],
            props: vec![
                ("banks", banks.clone()),
                (
                    "remote",
                    s(
                        "Remote sync server endpoint `host:port` to report; the call makes no connection.",
                    ),
                ),
                (
                    "token",
                    s(
                        "Bearer token for the remote; the report says only whether auth is set, never the token.",
                    ),
                ),
                (
                    "sync_key",
                    sync_key_prop(
                        "Optional override for this report: with no argument the tool reads the keys the node itself holds, so the encryption state reflects the node's real custody; one string or a newest-first array of strings overrides those keys when the summary is computed. Never returns key material.",
                    ),
                ),
            ],
        },
        ToolDef {
            name: "sync_key_status",
            description: "Masked local sync-key report: the store as the literal `file` or `keystore`, a non-reversible fingerprint of 16 lowercase hex characters that two nodes can compare, the creation time when the store kept it and null when it did not, the key count, and a per-key list naming each key's fingerprint, creation time, whether it is the current key, and the store that holds it. Never returns the key or any prefix of it.",
            required: &[],
            props: vec![],
        },
        ToolDef {
            name: "sync_serve",
            description: "Start the sync server on a background thread for the given banks; auto-stops after the TTL (default 30 minutes). Requires a scoped credential store passed as `token_file` (add records with `clio auth token add`) unless dev=true (dev profile).",
            required: &[],
            props: vec![
                (
                    "bind",
                    s(
                        "Bind address `host:port`; port 0 picks an ephemeral port (default 127.0.0.1:0).",
                    ),
                ),
                ("banks", banks),
                (
                    "token_file",
                    s(
                        "Path to the shared credential store of scoped records added with `clio auth token add`: each client secret resolves to that record's banks, loaded once at start, so revocation takes effect at the next start. A configured store requires a secret on every request, dev mode included.",
                    ),
                ),
                ("ttl_secs", num("Auto-stop TTL in seconds (default 1800).")),
                (
                    "dev",
                    b("Dev profile: permits a token-less anonymous bind."),
                ),
            ],
        },
        ToolDef {
            name: "sync_ack_skip",
            description: "Operator ack-skip of one dead-lettered (permanently rejected) sync mutation so the pull cursor may advance past it. Without confirm=true: dry-run shape only.",
            required: &["sync_mutation_id"],
            props: vec![
                (
                    "sync_mutation_id",
                    s("Dead-lettered sync mutation id to acknowledge (never re-pushed)."),
                ),
                (
                    "confirm",
                    b("Perform the ack; default false (dry-run: {acked: false, would_ack: id})."),
                ),
                ("reason", s("Operator reason recorded with the ack.")),
            ],
        },
    ]
}

#[cfg(test)]
#[path = "schema_sync_defs_tests.rs"]
mod tests;
