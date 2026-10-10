// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `WriteGroup` contract: the uniform shape a
//! disjoint CLI write-command group presents to the write engine, including the
//! no-write preview call shape and the admission-refusal mapping.
//!
//! ## Owns
//! - The `WriteGroup` trait (verbs, flag spec, call builder, text renderer)
//! - `WriteCall` (dispatcher call vs validated no-write preview)
//! - The `pass: false` → exit-1 refusal mapping shared by gated write payloads
//!
//! ## Does not own
//! - The engine that parses and dispatches calls (see `cli_write`)
//! - Any concrete command group's semantics (see the `cli_write_*` modules)
//!
//! ## Boundary
//!
//! Changes to how the engine talks to a command group should normally require
//! changes here. Changes to a specific group's verbs or flags should not.
//!
//! Keep this module focused on the write-group contract.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use crate::cli_read::CliIo;
use serde_json::Value;

/// One decoded write verb: a dispatcher call, or a validated no-write preview.
///
/// `Preview` is used by mutating commands whose in-process tool has no no-write
/// path: the engine validates and renders what would be written and never calls
/// the dispatcher, so nothing can be stored.
pub(crate) enum WriteCall {
    /// Call the bound tool through the in-process dispatcher (gates run inside).
    Tool {
        /// MCP tool name bound to the verb.
        tool: &'static str,
        /// Tool arguments.
        args: Value,
    },
    /// Emit this payload as a dry-run preview without any dispatcher call.
    Preview {
        /// Structured preview payload (`dry_run: true`).
        payload: Value,
        /// Human-readable text view of the same preview.
        text: String,
    },
}

/// A gated write the tool accepted structurally but refused by admission.
pub(crate) struct Refusal {
    /// Machine code for the CLI error envelope.
    pub(crate) code: &'static str,
    /// The same rejection reason the MCP path reports.
    pub(crate) message: String,
}

/// One disjoint write-command group.
///
/// The engine resolves a canonical verb (for example `triple add`), then asks
/// the owning group for its flag spec, tool call or preview, and human text view.
pub(crate) trait WriteGroup: Sync {
    /// Canonical verb keys this group owns (for example `["triple add"]`).
    fn verbs(&self) -> &'static [&'static str];

    /// The flags `verb` accepts, globals included.
    fn spec(&self, verb: &str) -> FlagSpec;

    /// Decode `verb` into a dispatcher call or a no-write preview.
    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<WriteCall, CliError>;

    /// Decode with injected streams; only commands that read stdin (`batch
    /// --file -`) need the override, everything else delegates to [`build`].
    fn build_with_io(
        &self,
        verb: &str,
        parsed: &ParsedArgs,
        _io: &mut CliIo<'_>,
    ) -> Result<WriteCall, CliError> {
        self.build(verb, parsed)
    }

    /// Render a success payload as deterministic human text.
    fn render(&self, verb: &str, payload: &Value, palette: crate::cli_style::Palette) -> String;

    /// Whether a successful call enqueues index work that a one-shot process
    /// must drain before exiting.
    fn mutates(&self, verb: &str) -> bool;

    /// The domain refusal carried in an `ok: true` payload, if any.
    fn refusal(&self, _verb: &str, payload: &Value) -> Option<Refusal> {
        decision_refusal(payload)
    }

    /// A non-zero process exit code for an `ok` payload that reports a partial
    /// failure (for example a batch that aborted after some items applied).
    ///
    /// The engine still emits the payload and text view; only the exit status
    /// changes, so scripts see the failure without losing the per-target detail.
    fn failure_exit(&self, _verb: &str, _payload: &Value) -> Option<u8> {
        None
    }
}

/// Map an admission refusal to the CLI contract: a `pass: false` decision either
/// flattened into the success envelope (`store`/`admit_preview`) or nested under
/// `decision` (`triple_add`, belief `Rejected` outcomes).
///
/// A passing decision returns `None`; the refusal message is the same rejection
/// reason the MCP path reports.
pub(crate) fn decision_refusal(payload: &Value) -> Option<Refusal> {
    let decision = payload.get("decision").unwrap_or(payload);
    if decision.get("pass").and_then(Value::as_bool) != Some(false) {
        return None;
    }
    let message = decision
        .get("rejection_reason")
        .and_then(Value::as_str)
        .unwrap_or("write refused by the admission gates")
        .to_owned();
    Some(Refusal {
        code: "admission_refused",
        message,
    })
}

/// Map an `admit --file` batch preview to a refusal when every item failed
/// validation. Such a preview previewed nothing, so it is not a success. The
/// refusal names the first failing item by index. Items refused by the
/// admission gates, with no validation error, keep the normal preview output.
pub(crate) fn batch_preview_refusal(payload: &Value) -> Option<Refusal> {
    let items = payload.get("items").and_then(Value::as_array)?;
    if items.is_empty() || items.iter().any(|item| item.get("error").is_none()) {
        return None;
    }
    let (index, first) = items.iter().enumerate().next()?;
    let reason = first
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("invalid item");
    Some(Refusal {
        code: "invalid_argument",
        message: format!(
            "no item previewed: all {} items failed validation; item {index}: {reason}",
            items.len()
        ),
    })
}

#[cfg(test)]
#[path = "cli_write_group_tests.rs"]
mod tests;
