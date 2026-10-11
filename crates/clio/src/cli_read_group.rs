// Copyright 2026 Agent Memoir (https://agentmemoir.com/)
// SPDX-License-Identifier: Apache-2.0
//
//! # Responsibility
//!
//! This module is responsible for the `ReadGroup` contract: the uniform shape a
//! disjoint CLI read-command group presents to the read engine.
//!
//! ## Owns
//! - The `ReadGroup` trait (verbs, flag spec, MCP call builder, text renderer,
//!   and optional payload post-processing)
//!
//! ## Does not own
//! - The engine that resolves verbs and dispatches calls (see `cli_read`)
//! - Any concrete command group's semantics (see the `cli_read_*` modules)
//!
//! ## Boundary
//!
//! Changes to how the engine talks to a command group should normally require
//! changes here. Changes to a specific group's verbs or flags should not.
//!
//! Keep this module focused on the read-group contract.

use crate::cli_args::{FlagSpec, ParsedArgs};
use crate::cli_error::CliError;
use serde_json::Value;

/// One disjoint read-command group.
///
/// The engine resolves a canonical verb (for example `task history`), then asks
/// the owning group for its flag spec, MCP tool call, human text view, and any
/// post-processing such as an in-band `truncated` marker.
pub(crate) trait ReadGroup: Sync {
    /// Canonical verb keys this group owns (for example `["task get", "task history"]`).
    fn verbs(&self) -> &'static [&'static str];

    /// The flags `verb` accepts, globals included.
    fn spec(&self, verb: &str) -> FlagSpec;

    /// Decode `verb` into its MCP tool name and argument object.
    fn build(&self, verb: &str, parsed: &ParsedArgs) -> Result<(&'static str, Value), CliError>;

    /// Render a success payload as deterministic human text.
    fn render(&self, verb: &str, payload: &Value, _palette: crate::cli_style::Palette) -> String;

    /// Optional post-processing over a success payload before emission.
    ///
    /// `arguments` is the tool request [`build`](Self::build) produced, so a group
    /// can echo a value it sent without parsing the flag a second time.
    fn decorate(
        &self,
        _verb: &str,
        _parsed: &ParsedArgs,
        _arguments: &Value,
        _payload: &mut Value,
    ) {
    }
}
