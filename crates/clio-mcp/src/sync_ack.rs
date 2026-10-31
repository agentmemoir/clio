// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the MCP `sync_ack_skip` tool: the dry-run
//! and confirmed shapes of an operator acknowledgement of one permanently
//! rejected mutation, and the bank gate that keeps a bound caller inside its
//! credential's grant.
//!
//! ## Owns
//! - `sync_ack_skip` argument decoding and result shaping
//! - Resolving the acknowledged mutation's bank inside the caller's grant and
//!   passing it to the per-bank gate
//!
//! ## Does not own
//! - The per-bank gate itself (`runtime_context`)
//! - The dead-letter row and ack write (`clio-store`, `clio-sync`)
//! - The other sync surfaces and dispatch (`sync_tools`)
//!
//! ## Boundary
//!
//! Changes to the ack-skip surface or its bank scope should normally require
//! changes here. Changes to sync protocol semantics should not.
//!
//! Keep this module focused on the ack-skip tool and its bank gate.

use crate::runtime::{McpState, now_utc};
use crate::runtime_context::BANK_SCOPE_REFUSAL;
use crate::sync_tools::client_cfg;
use crate::write_tools::{opt_bool, opt_str, req_str};
use clio_sync::prelude::ack_skip;
use clio_types::{AmError, ErrorCode};
use serde_json::{Value, json};

/// The scan limit for locating a dead letter inside the caller's grant. A row
/// beyond it is refused rather than acked, so the gate fails closed.
const ACK_SCAN_LIMIT: usize = 10_000;

/// Resolve and gate the bank one ack-skip would change.
///
/// The dead-letter row's own bank decides the scope, so a bound caller may
/// acknowledge only a row its grant reaches. The id is located in the granted
/// banks, and that bank passes [`McpState::require_sync_bank`]; an id no
/// granted bank holds is refused, because the row may sit in a bank this
/// credential does not cover. An unbound local caller holds the store and
/// keeps the ungated path.
///
/// # Errors
/// [`ErrorCode::Forbidden`] when no granted bank holds the mutation, or when
/// the bank fails the per-bank gate.
fn require_ack_bank(state: &McpState, id: &str) -> Result<(), AmError> {
    let Some(principal) = &state.principal else {
        return Ok(());
    };
    for bank in &principal.banks {
        state.require_sync_bank(bank)?;
        let open = state.sync.dead_letter_list(
            bank,
            clio_store::DeadLetterStatus::Open,
            ACK_SCAN_LIMIT,
        )?;
        if open.iter().any(|row| row.sync_mutation_id == id) {
            return Ok(());
        }
    }
    Err(AmError::new(ErrorCode::Forbidden, BANK_SCOPE_REFUSAL))
}

/// `sync_ack_skip(sync_mutation_id, confirm?, reason?)`: operator ack-skip of
/// one permanently rejected mutation. Without `confirm=true` the call is a
/// dry-run shape (`{acked: false, would_ack: id}`); with it, the dead-letter
/// is acknowledged and the pull cursor may advance past the id. The mutation's
/// bank passes the per-bank gate before the row changes.
#[inline(never)]
pub(crate) fn ack_skip_tool(state: &McpState, args: &Value) -> Result<Value, AmError> {
    let id = req_str(args, "sync_mutation_id")?.to_owned();
    let confirm = opt_bool(args, "confirm", false)?;
    if !confirm {
        return Ok(json!({ "acked": false, "would_ack": id }));
    }
    // The mutation's own bank is the scope this write reaches, so it passes
    // the same per-bank gate as a transfer before the dead letter changes.
    require_ack_bank(state, &id)?;
    // Acknowledging a dead letter is a local store write: it needs neither an
    // endpoint, a token, nor keys, so none is read from the call.
    let cfg = client_cfg(state, None, None);
    let acked = ack_skip(&cfg, &id, &state.resolve_timestamp(args)?)?;
    Ok(json!({
        "acked": acked,
        "sync_mutation_id": id,
        "reason": opt_str(args, "reason"),
        "timestamp": now_utc(),
    }))
}

#[cfg(test)]
#[path = "sync_ack_tests.rs"]
mod sync_ack_tests;
