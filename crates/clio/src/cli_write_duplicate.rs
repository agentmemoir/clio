// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the duplicate-write CLI contract: the
//! `remember --allow-duplicate` flag, and treating an exact-duplicate rejection as a
//! no-op success instead of a failure.
//!
//! ## Owns
//! - Decoding `--allow-duplicate` into the store tool's `allow_duplicate` argument
//! - Recognising a `duplicate: true` store payload
//! - The human text for a no-op duplicate write
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
/// An exact duplicate was already rejected inside the store path, so the write
/// stored nothing and the caller already holds the fact. Mapping that to the
/// exit-1 domain refusal would abort agent loops and `set -e` scripts over a
/// non-error, which is why it returns `None` here.
pub(crate) fn remember_refusal(payload: &Value) -> Option<crate::cli_write_group::Refusal> {
    if is_duplicate_noop(payload) {
        return None;
    }
    crate::cli_write_group::decision_refusal(payload)
}

/// The `remember` view of a duplicate: nothing was written, and that is the
/// expected outcome rather than an error.
pub(crate) fn render_duplicate(payload: &Value) -> String {
    let reason = payload
        .get("rejection_reason")
        .and_then(Value::as_str)
        .unwrap_or("identical text is already stored");
    format!("duplicate: nothing stored ({reason})")
}

#[cfg(test)]
#[path = "cli_write_duplicate_tests.rs"]
mod cli_write_duplicate_tests;
