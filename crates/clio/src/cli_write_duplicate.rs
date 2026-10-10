// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the duplicate-write CLI contract: the
//! `remember --allow-duplicate` flag, and refusing an exact-duplicate write as a
//! failure with the reason named.
//!
//! ## Owns
//! - Decoding `--allow-duplicate` into the store tool's `allow_duplicate` argument
//! - Recognising a `duplicate: true` store payload
//! - The refusal for a duplicate write
//!
//! ## Does not own
//! - The admission rejection itself (see `clio-admission`)
//! - Verb routing and the generic refusal mapping (see `cli_write_core`)
//!
//! ## Boundary
//!
//! Changes to the duplicate CLI contract should normally require changes here.
//! Changes to admission math should not require changes here.
//!
//! Keep this module focused on the duplicate write contract.

use crate::cli_args::ParsedArgs;
use serde_json::Value;

/// The `allow_duplicate` tool argument when the caller passed
/// `--allow-duplicate`.
///
/// Named for what it waives rather than `force`, because `consolidate` already
/// has an unrelated `force` argument and one flag name should not mean two
/// things across the tool surface. Absent otherwise, so the store path keeps
/// refusing an exact duplicate by default.
pub(crate) fn allow_duplicate_arg(parsed: &ParsedArgs) -> Option<Value> {
    parsed.has("allow-duplicate").then_some(Value::Bool(true))
}

/// Whether a store payload reports an exact-duplicate no-op.
pub(crate) fn is_duplicate_noop(payload: &Value) -> bool {
    payload.get("duplicate").and_then(Value::as_bool) == Some(true)
}

/// The refusal for a `remember` payload, or `None` when there is nothing to
/// refuse.
///
/// An exact duplicate stores nothing, so it is refused like every other rejected
/// write: exit 1, `ok` false, and the reason named. The envelope must not report
/// `ok` true for a write that did not happen. A caller that wants the copy anyway
/// passes `--allow-duplicate`.
pub(crate) fn remember_refusal(payload: &Value) -> Option<crate::cli_write_group::Refusal> {
    if is_duplicate_noop(payload) {
        let reason = payload
            .get("rejection_reason")
            .and_then(Value::as_str)
            .unwrap_or("identical text is already stored");
        return Some(crate::cli_write_group::Refusal {
            code: "duplicate",
            message: format!("duplicate: nothing stored ({reason})"),
        });
    }
    crate::cli_write_group::decision_refusal(payload)
}

#[cfg(test)]
#[path = "cli_write_duplicate_tests.rs"]
mod cli_write_duplicate_tests;
